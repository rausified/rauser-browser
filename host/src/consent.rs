//! Short-lived, process-local grants for folder selection, capture consent,
//! and AI harness setup. Only the native host can mint these grants; the
//! extension cannot authorize a path, redirect writes, broaden capture, or
//! name a program to run by sending an arbitrary config.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use brauser_protocol::{
    AgentConfig, ConfigSnapshot, ConfirmHarnessSetupRequest, ErrorCode, HarnessOffer, SiteConfig,
    StorageConfig,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::brand::APP_NAME;
use crate::config::{self, HarnessCommit, HarnessRecord};
use crate::dialog::{self, DialogText};
use crate::harness::{self, Adapter, Candidate, HarnessEnv};
use crate::vault::selected_root_identity;

const TOKEN_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING_GRANTS: usize = 32;
const MAX_CONSENT_SUMMARY_BYTES: usize = 8192;
/// The harness dialog lists every argument and exclusion. The alert does not
/// scroll, so it has the consent dialog's bound and is refused, never
/// truncated, over it.
pub const MAX_HARNESS_SUMMARY_BYTES: usize = MAX_CONSENT_SUMMARY_BYTES;
/// The byte bound alone admits an alert far taller than the screen: the Codex
/// setup alert once reached about 45 lines. Tests also hold a realistic
/// harness confirmation (a few exclusions, one change) to
/// [`HARNESS_DIALOG_LINE_BUDGET`] rows, counting the title and each explicit
/// line word-wrapped at [`DIALOG_WRAP_COLUMNS`] characters, an estimate of
/// the macOS alert's width. Shorten the wording rather than raise the budget.
#[cfg(test)]
const DIALOG_WRAP_COLUMNS: usize = 60;
#[cfg(test)]
const HARNESS_DIALOG_LINE_BUDGET: usize = 30;
const MAX_OFFERS: usize = 8;
const MAX_REFUSAL_BYTES: usize = 512;
const MAX_OFFER_PATH_BYTES: usize = 4096;
const DEFAULT_SUMMARIES_DIR: &str = "summaries";
const HARNESS_CHANGED: &str = "harness changed; detect again";

pub struct FolderSelection {
    pub path: String,
    pub picker_token: String,
}

pub struct ConfigConfirmation {
    pub consent_token: String,
    pub summary: String,
}

struct PickerGrant {
    root: String,
    identity: String,
    revision: String,
    expires_at: Instant,
}

struct ConsentGrant {
    before: ConfigSnapshot,
    after: ConfigSnapshot,
    revision: String,
    expires_at: Instant,
}

/// One discovered harness the user may confirm. The candidate never leaves
/// the host; the extension only names it by `offer_id`.
struct HarnessOfferGrant {
    revision: String,
    candidate: Candidate,
    expires_at: Instant,
}

/// A confirmed and probed harness setup, bound to the exact config change.
struct HarnessGrant {
    before: ConfigSnapshot,
    after: ConfigSnapshot,
    revision: String,
    commit: HarnessCommit,
    expires_at: Instant,
}

/// What `authorize_update` checked. A harness commit is written only with the
/// grant that carries it.
#[derive(Debug, Default)]
pub struct Authorized {
    pub selected_identity: Option<String>,
    pub harness_commit: Option<HarnessCommit>,
}

pub struct HarnessConfirmation {
    pub harness_token: String,
    pub config: ConfigSnapshot,
    pub summary: String,
}

/// A harness setup refusal, with the protocol error code to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupError {
    pub code: ErrorCode,
    pub message: String,
}

impl SetupError {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unauthorized, message)
    }
}

#[derive(Default)]
pub struct ConsentAuthority {
    picker_grants: HashMap<String, PickerGrant>,
    consent_grants: HashMap<String, ConsentGrant>,
    offer_grants: HashMap<String, HarnessOfferGrant>,
    harness_grants: HashMap<String, HarnessGrant>,
}

impl ConsentAuthority {
    pub fn new() -> Self {
        Self::default()
    }

    /// Blocks this connection until the user closes the picker, which runs
    /// in a child process (see `dialog`).
    pub fn choose_folder(&mut self, revision: &str) -> Result<Option<FolderSelection>> {
        let Some(selected) = dialog::pick_folder(&format!("Choose {APP_NAME} notes folder"))?
        else {
            return Ok(None);
        };
        let canonical = fs::canonicalize(&selected)
            .with_context(|| format!("resolving selected folder {}", selected.display()))?;
        if !canonical.is_dir() {
            bail!("selected notes folder is not a directory");
        }
        let root = canonical
            .to_str()
            .context("selected notes folder path cannot be represented as UTF-8")?
            .to_owned();
        let identity = selected_root_identity(Path::new(&root))?;
        let picker_token = self.mint_picker(root.clone(), identity, revision);
        Ok(Some(FolderSelection {
            path: root,
            picker_token,
        }))
    }

    /// The dialog text is derived only from the actual policy delta. A caller
    /// supplied string never appears in the native confirmation as a claim.
    pub fn confirm_config(
        &mut self,
        current: &ConfigSnapshot,
        revision: &str,
        next: &ConfigSnapshot,
        picker_token: Option<&str>,
    ) -> Result<Option<ConfigConfirmation>> {
        self.confirm_config_with(current, revision, next, picker_token, dialog::confirm)
    }

    pub(crate) fn confirm_config_with(
        &mut self,
        current: &ConfigSnapshot,
        revision: &str,
        next: &ConfigSnapshot,
        picker_token: Option<&str>,
        confirm: fn(DialogText<'_>) -> Result<bool>,
    ) -> Result<Option<ConfigConfirmation>> {
        config::validate_update(current, next)?;
        self.check_picker(current, revision, next, picker_token)?;
        check_agent_fields(current, next)?;
        let changes = approval_lines(current, next);
        let summary = if changes.is_empty() {
            "No additional capture access is requested.".to_owned()
        } else {
            changes.join("\n")
        };
        if summary.len() > MAX_CONSENT_SUMMARY_BYTES {
            bail!("too many privacy changes for one confirmation; add fewer sites at once");
        }
        if !changes.is_empty() {
            let description =
                format!("{APP_NAME} will make these changes:\n\n{summary}\n\nAllow these changes?");
            if !confirm(DialogText {
                title: &format!("Confirm {APP_NAME} settings"),
                description: &description,
            })? {
                return Ok(None);
            }
        }
        let consent_token = self.mint_consent(current.clone(), next.clone(), revision);
        Ok(Some(ConfigConfirmation {
            consent_token,
            summary,
        }))
    }

    /// Look for supported harnesses and mint one single-use offer for each
    /// that passed its checks. Help text and command output never leave here.
    pub fn discover_harnesses(&mut self, revision: &str, env: &HarnessEnv) -> Vec<HarnessOffer> {
        harness::discover(env)
            .into_iter()
            .take(MAX_OFFERS)
            .map(|candidate| self.offer(revision, candidate))
            .collect()
    }

    fn offer(&mut self, revision: &str, candidate: Candidate) -> HarnessOffer {
        let mut refusal = candidate.refusal.clone();
        if !cfg!(unix) {
            refusal.get_or_insert_with(|| harness::UNSUPPORTED_PLATFORM.to_owned());
        }
        let binary = match candidate.found_at.to_str() {
            Some(path) if !config::has_unsafe_display_chars(path) => path.to_owned(),
            _ => {
                refusal.get_or_insert_with(|| {
                    "the harness path cannot be shown safely; move it to a plain path".to_owned()
                });
                candidate
                    .found_at
                    .to_string_lossy()
                    .escape_debug()
                    .to_string()
            }
        };
        let real_path = candidate.identity.as_ref().map(|identity| {
            if config::has_unsafe_display_chars(&identity.real_path) {
                refusal.get_or_insert_with(|| {
                    "the harness path cannot be shown safely; move it to a plain path".to_owned()
                });
                identity.real_path.escape_debug().to_string()
            } else {
                identity.real_path.clone()
            }
        });
        let (env_required, env_optional) = offered_env(&candidate);
        let offer_id = refusal
            .is_none()
            .then(|| self.mint_offer(revision, candidate.clone()));
        HarnessOffer {
            offer_id,
            adapter: candidate.adapter.into(),
            harness_id: candidate.harness_id,
            binary: bounded(binary, MAX_OFFER_PATH_BYTES),
            real_path: real_path.map(|path| bounded(path, MAX_OFFER_PATH_BYTES)),
            version: candidate.version,
            args: candidate.args,
            env_required,
            env_optional,
            refusal: refusal.map(|text| bounded(text, MAX_REFUSAL_BYTES)),
        }
    }

    /// Show the host-authored harness confirmation, then run one test prompt.
    /// The offer is spent first, so it is single-use even when a later step
    /// fails. `Ok(None)` means the user canceled; nothing was minted.
    pub fn confirm_harness_setup(
        &mut self,
        current: &ConfigSnapshot,
        revision: &str,
        request: &ConfirmHarnessSetupRequest,
        env: &HarnessEnv,
    ) -> Result<Option<HarnessConfirmation>, SetupError> {
        self.confirm_harness_setup_with(current, revision, request, env, dialog::confirm)
    }

    /// `confirm_harness_setup` with the dialog injected, for tests.
    pub(crate) fn confirm_harness_setup_with(
        &mut self,
        current: &ConfigSnapshot,
        revision: &str,
        request: &ConfirmHarnessSetupRequest,
        env: &HarnessEnv,
        confirm: impl FnOnce(DialogText<'_>) -> Result<bool>,
    ) -> Result<Option<HarnessConfirmation>, SetupError> {
        let Some(grant) = self.offer_grants.remove(&request.offer_id) else {
            return Err(SetupError::unauthorized(
                "harness offer is unknown or already used; detect again",
            ));
        };
        if grant.expires_at <= Instant::now() || grant.revision != revision {
            return Err(SetupError::unauthorized(
                "harness offer is stale; detect again",
            ));
        }
        let candidate = grant.candidate;
        if candidate.refusal.is_some() {
            return Err(SetupError::unauthorized(
                "this harness cannot be set up; detect again",
            ));
        }
        let (Some(checked), Some(version), Some(help_sha256)) = (
            candidate.identity.clone(),
            candidate.version.clone(),
            candidate.help_sha256.clone(),
        ) else {
            return Err(SetupError::unauthorized(
                "the harness was not checked; detect again",
            ));
        };
        let unchanged = |path: &Path| harness::identity(path).is_ok_and(|now| now == checked);
        if !unchanged(Path::new(&checked.real_path)) || !unchanged(&candidate.found_at) {
            return Err(SetupError::unauthorized(HARNESS_CHANGED));
        }
        let Some(storage) = &current.storage else {
            return Err(SetupError::new(
                ErrorCode::NotConfigured,
                "Choose a notes folder first",
            ));
        };
        let env_allow = requested_env(&candidate, &request.env_names)?;
        let binary = candidate
            .found_at
            .to_str()
            .ok_or_else(|| SetupError::unauthorized("the harness path is not valid UTF-8"))?
            .to_owned();
        if !harness::template_matches(candidate.adapter, &candidate.args, Some(&version)) {
            return Err(SetupError::unauthorized(
                "the harness arguments are not the reviewed template",
            ));
        }
        let agent = AgentConfig {
            harness_id: candidate.harness_id.clone(),
            adapter: candidate.adapter.into(),
            binary,
            args: candidate.args.clone(),
            env_allow,
            timeout_secs: config::DEFAULT_TIMEOUT_SECS,
        };
        let summaries_dir = request
            .summaries_dir
            .clone()
            .or_else(|| storage.summaries_dir.clone())
            .unwrap_or_else(|| DEFAULT_SUMMARIES_DIR.to_owned());
        let after = ConfigSnapshot {
            storage: Some(StorageConfig {
                summaries_dir: Some(summaries_dir),
                ..storage.clone()
            }),
            agent_denylist: request.agent_denylist.clone(),
            agent_denylist_confirmed: true,
            agent: Some(agent.clone()),
            ..current.clone()
        };
        config::validate_update(current, &after)
            .map_err(|error| SetupError::new(ErrorCode::InvalidConfig, error.to_string()))?;
        let (title, summary) = harness_confirmation_text(&candidate, &after, current)
            .map_err(|error| SetupError::new(ErrorCode::InvalidConfig, error.to_string()))?;
        let confirmed = confirm(DialogText {
            title: &title,
            description: &summary,
        })
        // A dialog left open past its timeout, or one whose host is going
        // away, is closed and reported like the settings confirmation (§4.3).
        .map_err(|error| {
            SetupError::new(
                ErrorCode::Internal,
                format!("the harness confirmation did not finish: {error}"),
            )
        })?;
        if !confirmed {
            return Ok(None);
        }
        harness::probe(&candidate, &agent.env_allow, env, agent.timeout_secs).map_err(
            |failure| {
                SetupError::new(
                    ErrorCode::InvalidConfig,
                    format!(
                        "{} failed its test run ({failure}); AI commands stay off",
                        candidate.adapter.display_name()
                    ),
                )
            },
        )?;
        let probe_passed_at = OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| {
            SetupError::new(ErrorCode::Internal, "could not record the test run time")
        })?;
        let commit = HarnessCommit {
            agent,
            record: HarnessRecord {
                harness_id: candidate.harness_id.clone(),
                real_path: checked.real_path,
                size: checked.size,
                mtime_ns: checked.mtime_ns,
                file_id: checked.file_id,
                version,
                help_sha256,
                confirmed_flags: candidate.confirmed_flags.clone(),
                probe_passed_at,
            },
        };
        let harness_token = self.mint_harness(current.clone(), after.clone(), revision, commit);
        Ok(Some(HarnessConfirmation {
            harness_token,
            config: after,
            summary,
        }))
    }

    /// Validate grants immediately before the revision-checked config write.
    /// Grants are removed only after the write succeeds.
    pub fn authorize_update(
        &self,
        current: &ConfigSnapshot,
        revision: &str,
        next: &ConfigSnapshot,
        picker_token: Option<&str>,
        consent_token: Option<&str>,
        harness_token: Option<&str>,
    ) -> Result<Authorized> {
        config::validate_update(current, next)?;
        if let Some(token) = harness_token {
            if picker_token.is_some() || consent_token.is_some() {
                bail!("a harness setup token cannot be combined with other tokens");
            }
            return self.check_harness(current, revision, next, token);
        }
        let selected_identity = self.check_picker(current, revision, next, picker_token)?;
        check_agent_fields(current, next)?;
        if !approval_lines(current, next).is_empty() {
            let token =
                consent_token.context("native confirmation is required for this config change")?;
            let Some(grant) = self.consent_grants.get(token) else {
                bail!("native confirmation token is unknown or already used");
            };
            if grant.expires_at <= Instant::now()
                || grant.revision != revision
                || grant.before != *current
                || grant.after != *next
            {
                bail!("native confirmation is stale or does not match this change");
            }
        }
        Ok(Authorized {
            selected_identity,
            harness_commit: None,
        })
    }

    /// The harness dialog showed every approval line for this change, so a
    /// valid grant is also its consent. The notes folder cannot change here.
    fn check_harness(
        &self,
        current: &ConfigSnapshot,
        revision: &str,
        next: &ConfigSnapshot,
        token: &str,
    ) -> Result<Authorized> {
        let Some(grant) = self.harness_grants.get(token) else {
            bail!("harness setup token is unknown or already used");
        };
        if grant.expires_at <= Instant::now()
            || grant.revision != revision
            || grant.before != *current
            || grant.after != *next
        {
            bail!("harness setup is stale or does not match this change");
        }
        let record = grant.commit.record.identity();
        let unchanged =
            |path: &str| harness::identity(Path::new(path)).is_ok_and(|now| now == record);
        if !unchanged(&record.real_path) || !unchanged(&grant.commit.agent.binary) {
            bail!("{HARNESS_CHANGED}");
        }
        Ok(Authorized {
            selected_identity: None,
            harness_commit: Some(grant.commit.clone()),
        })
    }

    /// Call only after ConfigStore::update returns a committed revision.
    pub fn consume_update(
        &mut self,
        _current: &ConfigSnapshot,
        _revision: &str,
        _next: &ConfigSnapshot,
        picker_token: Option<&str>,
        consent_token: Option<&str>,
        harness_token: Option<&str>,
    ) {
        if let Some(token) = picker_token {
            self.picker_grants.remove(token);
        }
        if let Some(token) = consent_token {
            self.consent_grants.remove(token);
        }
        if let Some(token) = harness_token {
            self.harness_grants.remove(token);
        }
    }

    fn check_picker(
        &self,
        current: &ConfigSnapshot,
        revision: &str,
        next: &ConfigSnapshot,
        picker_token: Option<&str>,
    ) -> Result<Option<String>> {
        let old_root = current
            .storage
            .as_ref()
            .map(|storage| storage.root.as_str());
        let new_root = next.storage.as_ref().map(|storage| storage.root.as_str());
        if new_root.is_none() || (new_root == old_root && picker_token.is_none()) {
            return Ok(None);
        }
        let root = new_root.expect("checked above");
        let token = picker_token.context("a native folder selection is required")?;
        let Some(grant) = self.picker_grants.get(token) else {
            bail!("folder selection token is unknown or already used");
        };
        if grant.expires_at <= Instant::now() || grant.revision != revision || grant.root != root {
            bail!("folder selection is stale or does not match the proposed notes folder");
        }
        if selected_root_identity(Path::new(root))? != grant.identity {
            bail!("selected notes folder changed; choose it again");
        }
        Ok(Some(grant.identity.clone()))
    }

    fn mint_picker(&mut self, root: String, identity: String, revision: &str) -> String {
        self.prune();
        let token = Uuid::new_v4().to_string();
        self.picker_grants.insert(
            token.clone(),
            PickerGrant {
                root,
                identity,
                revision: revision.to_owned(),
                expires_at: Instant::now() + TOKEN_LIFETIME,
            },
        );
        token
    }

    fn mint_consent(
        &mut self,
        before: ConfigSnapshot,
        after: ConfigSnapshot,
        revision: &str,
    ) -> String {
        self.prune();
        let token = Uuid::new_v4().to_string();
        self.consent_grants.insert(
            token.clone(),
            ConsentGrant {
                before,
                after,
                revision: revision.to_owned(),
                expires_at: Instant::now() + TOKEN_LIFETIME,
            },
        );
        token
    }

    fn mint_offer(&mut self, revision: &str, candidate: Candidate) -> String {
        self.prune();
        let token = Uuid::new_v4().to_string();
        self.offer_grants.insert(
            token.clone(),
            HarnessOfferGrant {
                revision: revision.to_owned(),
                candidate,
                expires_at: Instant::now() + TOKEN_LIFETIME,
            },
        );
        token
    }

    fn mint_harness(
        &mut self,
        before: ConfigSnapshot,
        after: ConfigSnapshot,
        revision: &str,
        commit: HarnessCommit,
    ) -> String {
        self.prune();
        let token = Uuid::new_v4().to_string();
        self.harness_grants.insert(
            token.clone(),
            HarnessGrant {
                before,
                after,
                revision: revision.to_owned(),
                commit,
                expires_at: Instant::now() + TOKEN_LIFETIME,
            },
        );
        token
    }

    fn prune(&mut self) {
        let now = Instant::now();
        self.picker_grants.retain(|_, grant| grant.expires_at > now);
        self.consent_grants
            .retain(|_, grant| grant.expires_at > now);
        self.offer_grants.retain(|_, grant| grant.expires_at > now);
        self.harness_grants
            .retain(|_, grant| grant.expires_at > now);
        if self.picker_grants.len() >= MAX_PENDING_GRANTS {
            self.picker_grants.clear();
        }
        if self.consent_grants.len() >= MAX_PENDING_GRANTS {
            self.consent_grants.clear();
        }
        if self.offer_grants.len() >= MAX_PENDING_GRANTS {
            self.offer_grants.clear();
        }
        if self.harness_grants.len() >= MAX_PENDING_GRANTS {
            self.harness_grants.clear();
        }
    }
}

/// `HOME`, `PATH`, and any home name the host has, always; the other names
/// are the adapter's optional ones.
fn offered_env(candidate: &Candidate) -> (Vec<String>, Vec<brauser_protocol::EnvName>) {
    let homes = candidate.adapter.home_names();
    let optional = candidate
        .env_names
        .iter()
        .filter(|env| {
            !harness::REQUIRED_ENV.contains(&env.name.as_str())
                && !homes.contains(&env.name.as_str())
        })
        .map(|env| brauser_protocol::EnvName {
            name: env.name.clone(),
            present: env.present,
        })
        .collect();
    (harness::required_env(candidate), optional)
}

/// The allowlist setup writes: the required names, then each requested
/// optional name the offer listed, in the offer's order.
fn requested_env(candidate: &Candidate, requested: &[String]) -> Result<Vec<String>, SetupError> {
    let (mut allow, optional) = offered_env(candidate);
    if let Some(name) = requested
        .iter()
        .find(|name| !allow.contains(name) && !optional.iter().any(|env| &env.name == *name))
    {
        return Err(SetupError::unauthorized(format!(
            "environment variable {name:?} was not offered for this harness"
        )));
    }
    for env in optional {
        if requested.contains(&env.name) && !allow.contains(&env.name) {
            allow.push(env.name);
        }
    }
    Ok(allow)
}

/// Truncate host-authored text at a character boundary.
fn bounded(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// The native harness confirmation, as `(title, description)`. Every value
/// is host-derived; the text is refused, never truncated, when a value
/// cannot be shown safely or the whole exceeds [`MAX_HARNESS_SUMMARY_BYTES`].
/// §14: Codex shares the user's own `HOME` and `CODEX_HOME`, so their global
/// instructions and skills reach it. The settings page says the same.
pub const CODEX_DISCLOSURE: &str = "Codex uses your own Codex home and login, so your AGENTS.md and skills can shape answers. Claude Code is recommended.";

pub fn harness_confirmation_text(
    candidate: &Candidate,
    after: &ConfigSnapshot,
    current: &ConfigSnapshot,
) -> Result<(String, String)> {
    let name = candidate.adapter.display_name();
    let agent = after
        .agent
        .as_ref()
        .context("harness setup has no harness entry")?;
    let storage = after
        .storage
        .as_ref()
        .context("harness setup has no notes folder")?;
    let summaries = storage
        .summaries_dir
        .as_deref()
        .context("harness setup has no summaries folder")?;
    let version = candidate
        .version
        .as_deref()
        .context("the harness version was not checked")?;
    let real_path = &candidate
        .identity
        .as_ref()
        .context("the harness was not checked")?
        .real_path;
    let changes = approval_lines(current, after);
    let values = [
        version,
        agent.binary.as_str(),
        real_path,
        summaries,
        &storage.root,
    ]
    .into_iter()
    .chain(agent.args.iter().map(String::as_str))
    .chain(agent.env_allow.iter().map(String::as_str))
    .chain(after.agent_denylist.iter().map(String::as_str))
    .chain(changes.iter().map(String::as_str));
    for value in values {
        if config::has_unsafe_display_chars(value) {
            bail!("the harness confirmation contains text that cannot be shown safely");
        }
    }

    let title = format!("Set up {name} for {APP_NAME} AI commands");
    let mut lines = Vec::new();
    // Weakening changes come first. The caller chooses the exclusion list,
    // and additions need no approval, so a long list must not push a removal
    // out of sight below it.
    if !changes.is_empty() {
        lines.push("This setup also makes these changes:".to_owned());
        lines.extend(changes.iter().map(|change| format!("• {change}")));
        lines.push(String::new());
    }
    lines.extend([
        format!("Harness: {name} {version} at {:?}", agent.binary),
        format!("Runs: {real_path:?}"),
    ]);
    if candidate.adapter == Adapter::Codex {
        lines.push(CODEX_DISCLOSURE.to_owned());
    }
    lines.push(String::new());
    // One shell-quoted line: every argument is shown, and quoting keeps
    // the word boundaries unambiguous without a line per argument. Codex's
    // many `--disable <feature>` pairs are named once on their own line.
    let (disabled, rest) = split_disabled(&agent.args);
    lines.push("Arguments (the page goes on stdin, never in arguments):".to_owned());
    lines.push(
        rest.iter()
            .map(|arg| shell_word(arg))
            .collect::<Vec<_>>()
            .join(" "),
    );
    if !disabled.is_empty() {
        lines.push(format!("Also --disable each of: {}", disabled.join(", ")));
    }
    lines.push(format!(
        "Variables passed (values hidden): {}",
        agent.env_allow.join(", ")
    ));
    lines.push(format!("Summaries: {summaries:?} in {:?}", storage.root));
    let exclusions = if after.agent_denylist.is_empty() {
        "No domains excluded.".to_owned()
    } else {
        after
            .agent_denylist
            .iter()
            .map(|entry| format!("{entry:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    lines.push(format!("AI privacy exclusions: {exclusions}"));
    lines.push(format!("Timeout: {} seconds", agent.timeout_secs));
    lines.push(String::new());
    lines.push(format!(
        "Next, one test prompt goes to {name}. Nothing turns on unless it passes."
    ));
    let description = lines.join("\n");
    if title.len() + 1 + description.len() > MAX_HARNESS_SUMMARY_BYTES {
        bail!(
            "the harness confirmation would be too long to review; shorten the AI privacy exclusions or remove fewer at once"
        );
    }
    Ok((title, description))
}

/// Split `args` into the features of its `--disable <feature>` pairs and the
/// other arguments in order. A feature that would need shell quoting stays
/// in place, so the grouped line never hides how an argument is spelled.
fn split_disabled(args: &[String]) -> (Vec<&str>, Vec<&str>) {
    let mut disabled = Vec::new();
    let mut rest = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args.get(index + 1) {
            Some(feature) if args[index] == "--disable" && shell_word(feature) == *feature => {
                disabled.push(feature.as_str());
                index += 2;
            }
            _ => {
                rest.push(args[index].as_str());
                index += 1;
            }
        }
    }
    (disabled, rest)
}

/// `arg` as one POSIX shell word: bare when it has no shell metacharacters,
/// otherwise single-quoted.
fn shell_word(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '-' | '_' | '.' | '/' | '=' | ':' | ',' | '@' | '+' | '%'
                )
        });
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Only native harness setup sets `agent_denylist_confirmed` or a harness
/// entry (§7.3). The extension may echo both unchanged or remove the harness;
/// removing it keeps the confirmation.
fn check_agent_fields(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<()> {
    let trusted_current = config::validate_base(current).is_ok();
    let confirmed = trusted_current && current.agent_denylist_confirmed;
    if next.agent_denylist_confirmed != confirmed
        || (next.agent.is_some() && (!trusted_current || next.agent != current.agent))
    {
        bail!(
            "agent_denylist_confirmed and the AI harness can only be set by native harness setup"
        );
    }
    Ok(())
}

fn approval_lines(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Vec<String> {
    let mut lines = Vec::new();
    // A hand-edited but unusable config is not a trusted prior grant. Repair
    // must ask again before it makes any of those rules effective.
    let trusted_current = config::validate_base(current).is_ok();
    // Adapter validity is deliberately separate from base config validity so
    // notes and AI settings remain usable during adapter repair. An invalid
    // adapter document is not, however, a prior capture grant: a later edit
    // such as enabling its missing origin must confirm every adapter that the
    // repair makes effective.
    let trusted_adapters = config::validate(current).is_ok();
    let preserving_inert_adapters = current.adapters == next.adapters
        && !config::adapter_policy_valid(current)
        && !config::adapter_policy_valid(next);
    // Adding an exclusion takes effect at once. Removing or replacing one
    // weakens privacy and needs confirmation, confirmed list or not (§7.3).
    // These come first: sites the caller adds cannot push them down the dialog.
    if trusted_current {
        for entry in &current.agent_denylist {
            if !next.agent_denylist.contains(entry) {
                lines.push(format!(
                    "Allow AI commands to read pages on {entry:?} and its subdomains."
                ));
            }
        }
    }
    if !(trusted_current && current.capture_enabled) && next.capture_enabled {
        lines.push("Turn on automatic visit logging.".to_owned());
    }
    let mut last_prior_adapter = None;
    for adapter in &next.adapters.artifacts {
        if preserving_inert_adapters {
            continue;
        }
        // Removing an earlier adapter shifts every following array index but
        // does not widen any survivor. Match the stable adapter identity
        // instead. The matched prior indexes must still increase: config
        // order decides which overlapping rule wins, so a real reorder is a
        // policy change even when the adapter bytes are otherwise unchanged.
        let prior_index = trusted_adapters
            .then(|| unique_adapter_index(&current.adapters.artifacts, adapter))
            .flatten();
        let unchanged = prior_index.is_some_and(|index| {
            current.adapters.artifacts.get(index) == Some(adapter)
                && last_prior_adapter.is_none_or(|last| index > last)
        });
        if let Some(index) = prior_index {
            last_prior_adapter = Some(index);
        }
        if unchanged {
            continue;
        }
        let artifact_type = adapter
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let origin = adapter
            .get("origin")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown origin");
        lines.push(format!(
            "Enable or change the {artifact_type:?} artifact adapter on {origin:?}."
        ));
        if adapter.get("unscoped").and_then(serde_json::Value::as_str) == Some("acknowledged") {
            lines.push(format!(
                "Record every matching artifact from every account on {origin:?} in this Chrome profile. Use a separate Chrome profile for work when personal accounts share this site."
            ));
        }
    }
    for site in &next.sites {
        if !trusted_current || !current.sites.iter().any(|prior| site_covers(prior, site)) {
            lines.push(format!(
                "Allow visit logging for {:?}",
                format!("{}{}", site.origin, site.path_prefix)
            ));
        }
    }
    let old_strip = if trusted_current {
        current.strip_params.clone()
    } else {
        config::default_strip_params()
    };
    for param in &old_strip {
        if !next.strip_params.iter().any(|value| value == param) {
            lines.push(format!(
                "Keep the query parameter {param:?} in logged URLs."
            ));
        }
    }
    if let Some(next_storage) = &next.storage {
        let old_storage = if trusted_current {
            current.storage.as_ref()
        } else {
            None
        };
        for (name, old, new) in [
            (
                "profile",
                old_storage.map(|value| value.profile.as_str()),
                Some(next_storage.profile.as_str()),
            ),
            (
                "visit log folder",
                old_storage.map(|value| value.log_dir.as_str()),
                Some(next_storage.log_dir.as_str()),
            ),
            (
                "page notes folder",
                old_storage.map(|value| value.pages_dir.as_str()),
                Some(next_storage.pages_dir.as_str()),
            ),
            (
                "read-later folder",
                old_storage.map(|value| value.later_dir.as_str()),
                Some(next_storage.later_dir.as_str()),
            ),
            (
                "artifact files folder",
                old_storage.map(|value| value.artifacts_dir.as_str()),
                Some(next_storage.artifacts_dir.as_str()),
            ),
            // Clearing the summaries folder only turns summaries off.
            (
                "summaries folder",
                old_storage.and_then(|value| value.summaries_dir.as_deref()),
                next_storage.summaries_dir.as_deref(),
            ),
        ] {
            if let Some(new) = new
                && old != Some(new)
            {
                if let Some(old) = old {
                    lines.push(format!("Change {name} from {old:?} to {new:?}."));
                } else {
                    lines.push(format!("Set {name} to {new:?}."));
                }
            }
        }
    }
    lines
}

fn unique_adapter_index(
    adapters: &[serde_json::Value],
    candidate: &serde_json::Value,
) -> Option<usize> {
    let identity = adapter_approval_identity(candidate)?;
    let mut matches = adapters
        .iter()
        .enumerate()
        .filter(|(_, adapter)| adapter_approval_identity(adapter) == Some(identity));
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

fn adapter_approval_identity(value: &serde_json::Value) -> Option<(&str, &str, &str)> {
    Some((
        value.get("origin")?.as_str()?,
        value.get("type")?.as_str()?,
        value.get("id")?.as_str()?,
    ))
}

fn site_covers(prior: &SiteConfig, next: &SiteConfig) -> bool {
    if prior.origin != next.origin {
        return false;
    }
    prior.path_prefix == "/"
        || prior.path_prefix == next.path_prefix
        || (next.path_prefix.starts_with(&prior.path_prefix)
            && (prior.path_prefix.ends_with('/')
                || next.path_prefix.as_bytes().get(prior.path_prefix.len()) == Some(&b'/')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use brauser_protocol::{AdapterDocument, StorageConfig};
    use serde_json::json;

    #[test]
    fn fresh_picker_token_can_reconfirm_the_same_path() {
        let folder = tempfile::tempdir().unwrap();
        let storage = StorageConfig {
            root: folder.path().to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: "pages".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: None,
        };
        let snapshot = ConfigSnapshot {
            storage: Some(storage.clone()),
            capture_enabled: false,
            sites: Vec::new(),
            strip_params: Vec::new(),
            ..crate::config::empty_config()
        };
        let mut authority = ConsentAuthority::new();
        let identity = selected_root_identity(folder.path()).unwrap();
        let token = authority.mint_picker(storage.root, identity.clone(), "revision");
        assert_eq!(
            authority
                .check_picker(&snapshot, "revision", &snapshot, Some(&token))
                .unwrap(),
            Some(identity)
        );
    }

    fn agent() -> brauser_protocol::AgentConfig {
        brauser_protocol::AgentConfig {
            harness_id: "claude-code".into(),
            adapter: brauser_protocol::HarnessAdapter::ClaudeCode,
            // Valid here, so `set_up()` is a trusted current config.
            binary: crate::config::fixtures::absent_binary("claude"),
            args: crate::harness::template_args(crate::harness::Adapter::ClaudeCode),
            env_allow: Vec::new(),
            timeout_secs: 120,
        }
    }

    fn set_up() -> ConfigSnapshot {
        ConfigSnapshot {
            agent_denylist: vec!["bank.example".into()],
            agent_denylist_confirmed: true,
            agent: Some(agent()),
            ..crate::config::empty_config()
        }
    }

    fn authorize(
        current: &ConfigSnapshot,
        next: &ConfigSnapshot,
        consent_token: Option<&str>,
        harness_token: Option<&str>,
    ) -> Result<Option<String>> {
        ConsentAuthority::new()
            .authorize_update(
                current,
                "revision",
                next,
                None,
                consent_token,
                harness_token,
            )
            .map(|authorized| authorized.selected_identity)
    }

    #[test]
    fn update_rejects_confirmed_true_without_harness_token() {
        let current = crate::config::empty_config();
        let next = ConfigSnapshot {
            agent_denylist_confirmed: true,
            ..current.clone()
        };
        assert!(authorize(&current, &next, None, None).is_err());
        // No harness grant exists in this build, so any token is refused.
        assert!(authorize(&current, &next, None, Some("forged")).is_err());
        assert!(authorize(&current, &current, None, Some("forged")).is_err());
    }

    #[test]
    fn update_rejects_confirmed_flip_to_false() {
        let current = set_up();
        let next = ConfigSnapshot {
            agent_denylist_confirmed: false,
            ..current.clone()
        };
        assert!(authorize(&current, &next, None, None).is_err());
    }

    #[test]
    fn update_accepts_unchanged_confirmed_and_agent_echo() {
        let current = set_up();
        let next = current.clone();
        assert!(approval_lines(&current, &next).is_empty());
        assert_eq!(authorize(&current, &next, None, None).unwrap(), None);
    }

    #[test]
    fn adapter_policy_repair_can_echo_an_existing_host_owned_agent() {
        let mut current = set_up();
        current.adapters.format = 2;
        assert!(crate::config::validate(&current).is_err());
        assert!(crate::config::validate_base(&current).is_ok());
        let next = ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts: Vec::new(),
            },
            ..current.clone()
        };
        check_agent_fields(&current, &next).unwrap();
        assert!(approval_lines(&current, &next).is_empty());
        assert_eq!(authorize(&current, &next, None, None).unwrap(), None);
    }

    #[test]
    fn unrelated_update_can_preserve_an_invalid_adapter_document() {
        let mut current = crate::config::empty_config();
        current.adapters.format = 2;
        let next = ConfigSnapshot {
            agent_denylist: vec!["bank.example".into()],
            ..current.clone()
        };

        assert!(crate::config::validate(&next).is_err());
        assert!(approval_lines(&current, &next).is_empty());
        assert_eq!(authorize(&current, &next, None, None).unwrap(), None);
    }

    #[test]
    fn update_rejects_extension_authored_agent_entry() {
        let current = ConfigSnapshot {
            agent: None,
            ..set_up()
        };
        let next = ConfigSnapshot {
            agent: Some(agent()),
            ..current.clone()
        };
        assert!(authorize(&current, &next, None, None).is_err());

        let current = set_up();
        let next = ConfigSnapshot {
            agent: Some(brauser_protocol::AgentConfig {
                binary: crate::config::fixtures::absent_binary("other-claude"),
                ..agent()
            }),
            ..current.clone()
        };
        assert!(authorize(&current, &next, None, None).is_err());
    }

    #[test]
    fn update_allows_agent_removal_without_token() {
        let current = set_up();
        let next = ConfigSnapshot {
            agent: None,
            ..current.clone()
        };
        assert!(approval_lines(&current, &next).is_empty());
        assert_eq!(authorize(&current, &next, None, None).unwrap(), None);
    }

    #[test]
    fn denylist_addition_needs_no_confirmation() {
        let current = set_up();
        let next = ConfigSnapshot {
            agent_denylist: vec!["bank.example".into(), "mail.example".into()],
            ..current.clone()
        };
        assert!(approval_lines(&current, &next).is_empty());
        assert_eq!(authorize(&current, &next, None, None).unwrap(), None);
    }

    #[test]
    fn denylist_removal_needs_native_confirmation() {
        for current in [
            set_up(),
            ConfigSnapshot {
                agent_denylist: vec!["bank.example".into()],
                ..crate::config::empty_config()
            },
        ] {
            let next = ConfigSnapshot {
                agent_denylist: Vec::new(),
                ..current.clone()
            };
            assert_eq!(
                approval_lines(&current, &next),
                vec![
                    "Allow AI commands to read pages on \"bank.example\" and its subdomains."
                        .to_owned()
                ]
            );
            assert!(authorize(&current, &next, None, None).is_err());
            let mut authority = ConsentAuthority::new();
            let token = authority.mint_consent(current.clone(), next.clone(), "revision");
            authority
                .authorize_update(&current, "revision", &next, None, Some(&token), None)
                .unwrap();
        }
    }

    #[test]
    fn denylist_replacement_counts_as_removal() {
        let current = set_up();
        let next = ConfigSnapshot {
            agent_denylist: vec!["other.example".into()],
            ..current.clone()
        };
        assert_eq!(approval_lines(&current, &next).len(), 1);
        assert!(authorize(&current, &next, None, None).is_err());
    }

    #[test]
    fn summaries_dir_change_needs_consent() {
        let folder = tempfile::tempdir().unwrap();
        let storage = |summaries: Option<&str>| StorageConfig {
            root: folder.path().to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: "pages".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: summaries.map(str::to_owned),
        };
        let with = |summaries| ConfigSnapshot {
            storage: Some(storage(summaries)),
            ..crate::config::empty_config()
        };
        assert_eq!(
            approval_lines(&with(None), &with(Some("summaries"))),
            vec!["Set summaries folder to \"summaries\".".to_owned()]
        );
        assert_eq!(
            approval_lines(&with(Some("summaries")), &with(Some("ai"))),
            vec!["Change summaries folder from \"summaries\" to \"ai\".".to_owned()]
        );
        assert!(approval_lines(&with(Some("summaries")), &with(None)).is_empty());
        assert!(authorize(&with(None), &with(Some("summaries")), None, None).is_err());
    }

    #[test]
    fn unscoped_shared_origin_adapter_confirmation_names_every_account() {
        let current = ConfigSnapshot {
            sites: vec![SiteConfig {
                origin: "https://docs.google.com".into(),
                path_prefix: "/".into(),
            }],
            ..crate::config::empty_config()
        };
        let next = ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts: vec![json!({
                    "type": "gdoc",
                    "origin": "https://docs.google.com",
                    "unscoped": "acknowledged",
                    "match": [{"path": "/document/d/{id:token(20,64)}/{mode?:(edit|view|preview)}"}],
                    "modes": {"edit": "edit", "view": "view", "preview": "view"},
                    "id": "gdoc:{host}/{id}",
                })],
            },
            ..current.clone()
        };
        crate::config::validate(&next).unwrap();
        let lines = approval_lines(&current, &next);
        assert!(lines.iter().any(|line| line.contains("gdoc")));
        assert!(lines.iter().any(|line| {
            line.contains("every account") && line.contains("separate Chrome profile")
        }));
        assert!(authorize(&current, &next, None, None).is_err());
    }

    fn unscoped_adapter(index: usize) -> serde_json::Value {
        json!({
            "type": "gdoc",
            "origin": "https://docs.google.com",
            "unscoped": "acknowledged",
            "match": [{
                "path": format!("/kind-{index}/{{id:int}}"),
            }],
            "id": format!("gdoc:{{host}}/kind-{index}/{{id}}"),
        })
    }

    #[test]
    fn removing_an_adapter_does_not_reconfirm_shifted_survivors() {
        let adapters = (0..31).map(unscoped_adapter).collect::<Vec<_>>();
        let current = ConfigSnapshot {
            sites: vec![SiteConfig {
                origin: "https://docs.google.com".into(),
                path_prefix: "/".into(),
            }],
            adapters: AdapterDocument {
                format: 1,
                artifacts: adapters.clone(),
            },
            ..crate::config::empty_config()
        };
        let next = ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts: adapters.into_iter().skip(1).collect(),
            },
            ..current.clone()
        };
        crate::config::validate(&current).unwrap();
        crate::config::validate(&next).unwrap();

        assert!(approval_lines(&current, &next).is_empty());
        assert_eq!(authorize(&current, &next, None, None).unwrap(), None);
    }

    #[test]
    fn reordering_adapters_still_needs_confirmation() {
        let current = ConfigSnapshot {
            sites: vec![SiteConfig {
                origin: "https://docs.google.com".into(),
                path_prefix: "/".into(),
            }],
            adapters: AdapterDocument {
                format: 1,
                artifacts: vec![unscoped_adapter(0), unscoped_adapter(1)],
            },
            ..crate::config::empty_config()
        };
        let mut next = current.clone();
        next.adapters.artifacts.swap(0, 1);

        crate::config::validate(&current).unwrap();
        crate::config::validate(&next).unwrap();
        assert!(
            approval_lines(&current, &next)
                .iter()
                .any(|line| line.contains("Enable or change"))
        );
        assert!(authorize(&current, &next, None, None).is_err());
    }

    #[test]
    fn activating_an_invalid_existing_adapter_requires_adapter_confirmation() {
        let adapter = json!({
            "type": "gdoc",
            "origin": "https://docs.google.com",
            "unscoped": "acknowledged",
            "match": [{"path": "/document/d/{id:token(20,64)}/{mode?:(edit|view|preview)}"}],
            "modes": {"edit": "edit", "view": "view", "preview": "view"},
            "id": "gdoc:{host}/{id}",
        });
        let current = ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts: vec![adapter],
            },
            ..crate::config::empty_config()
        };
        assert!(crate::config::validate_base(&current).is_ok());
        assert!(crate::config::validate(&current).is_err());

        // Adding the missing origin makes the already-present adapter active.
        // It must not be mistaken for a previously confirmed adapter merely
        // because its bytes and array position are unchanged.
        let next = ConfigSnapshot {
            sites: vec![SiteConfig {
                origin: "https://docs.google.com".into(),
                path_prefix: "/".into(),
            }],
            ..current.clone()
        };
        crate::config::validate(&next).unwrap();
        let lines = approval_lines(&current, &next);
        assert!(lines.iter().any(|line| line.contains("artifact adapter")));
        assert!(lines.iter().any(|line| {
            line.contains("every account") && line.contains("separate Chrome profile")
        }));
        assert!(authorize(&current, &next, None, None).is_err());
    }

    fn storage_at(root: &Path, summaries: Option<&str>) -> StorageConfig {
        StorageConfig {
            root: root.to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: "pages".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: summaries.map(str::to_owned),
        }
    }

    fn denylist(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|entry| (*entry).to_owned()).collect()
    }

    /// A minted harness grant for `current -> after`. The binary is a plain
    /// file that is never executed; only its identity matters here.
    struct Granted {
        _folder: tempfile::TempDir,
        binary: std::path::PathBuf,
        authority: ConsentAuthority,
        token: String,
        current: ConfigSnapshot,
        after: ConfigSnapshot,
    }

    fn granted() -> Granted {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        fs::create_dir(&notes).unwrap();
        let binary = folder.path().join("claude");
        fs::write(&binary, "#!/bin/sh\n").unwrap();
        let current = ConfigSnapshot {
            storage: Some(storage_at(&notes, None)),
            agent_denylist: denylist(&["bank.example"]),
            ..crate::config::empty_config()
        };
        let agent = brauser_protocol::AgentConfig {
            binary: binary.to_string_lossy().into_owned(),
            env_allow: denylist(&["HOME", "PATH"]),
            ..agent()
        };
        let after = ConfigSnapshot {
            storage: Some(storage_at(&notes, Some("summaries"))),
            agent_denylist_confirmed: true,
            agent: Some(agent.clone()),
            ..current.clone()
        };
        let identity = harness::identity(&binary).unwrap();
        let record = HarnessRecord {
            harness_id: "claude-code".into(),
            real_path: identity.real_path,
            size: identity.size,
            mtime_ns: identity.mtime_ns,
            file_id: identity.file_id,
            version: "2.1.284".into(),
            help_sha256: "0".repeat(64),
            confirmed_flags: Vec::new(),
            probe_passed_at: "2026-09-28T00:00:00Z".into(),
        };
        let mut authority = ConsentAuthority::new();
        let token = authority.mint_harness(
            current.clone(),
            after.clone(),
            "revision",
            HarnessCommit { agent, record },
        );
        Granted {
            _folder: folder,
            binary,
            authority,
            token,
            current,
            after,
        }
    }

    impl Granted {
        fn check(&self, next: &ConfigSnapshot) -> Result<Authorized> {
            self.authority.authorize_update(
                &self.current,
                "revision",
                next,
                None,
                None,
                Some(&self.token),
            )
        }

        fn with_agent(
            &self,
            change: impl FnOnce(&mut brauser_protocol::AgentConfig),
        ) -> ConfigSnapshot {
            let mut next = self.after.clone();
            change(next.agent.as_mut().unwrap());
            next
        }
    }

    #[test]
    fn harness_token_is_single_use() {
        let mut granted = granted();
        let authorized = granted.check(&granted.after).unwrap();
        let commit = authorized.harness_commit.expect("a harness commit");
        assert_eq!(Some(&commit.agent), granted.after.agent.as_ref());
        assert_eq!(authorized.selected_identity, None);
        let (current, after, token) = (
            granted.current.clone(),
            granted.after.clone(),
            granted.token.clone(),
        );
        granted
            .authority
            .consume_update(&current, "revision", &after, None, None, Some(&token));
        let error = granted.check(&granted.after).unwrap_err();
        assert!(error.to_string().contains("unknown or already used"));
    }

    #[test]
    fn harness_token_rejected_after_revision_change() {
        let granted = granted();
        assert!(
            granted
                .authority
                .authorize_update(
                    &granted.current,
                    "other-revision",
                    &granted.after,
                    None,
                    None,
                    Some(&granted.token),
                )
                .is_err()
        );
        // A different current config at the same revision is also stale.
        let moved = ConfigSnapshot {
            strip_params: vec!["different".into()],
            ..granted.current.clone()
        };
        assert!(
            granted
                .authority
                .authorize_update(
                    &moved,
                    "revision",
                    &granted.after,
                    None,
                    None,
                    Some(&granted.token)
                )
                .is_err()
        );
    }

    #[test]
    fn harness_token_rejected_when_denylist_differs() {
        let granted = granted();
        for entries in [
            &[][..],
            &["bank.example", "mail.example"][..],
            &["mail.example"][..],
        ] {
            let next = ConfigSnapshot {
                agent_denylist: denylist(entries),
                ..granted.after.clone()
            };
            assert!(granted.check(&next).is_err(), "{entries:?}");
        }
    }

    #[test]
    fn harness_token_rejected_when_harness_entry_differs() {
        let granted = granted();
        let other = granted.binary.with_file_name("other-claude");
        fs::write(&other, "#!/bin/sh\n").unwrap();
        let next = granted.with_agent(|agent| agent.binary = other.to_string_lossy().into_owned());
        assert!(granted.check(&next).is_err());
        let next = granted.with_agent(|agent| agent.timeout_secs = 600);
        assert!(granted.check(&next).is_err());
        let next = ConfigSnapshot {
            agent: None,
            ..granted.after.clone()
        };
        assert!(granted.check(&next).is_err());
    }

    #[test]
    fn harness_token_rejected_when_env_allow_differs() {
        let granted = granted();
        let next = granted.with_agent(|agent| agent.env_allow.push("ANTHROPIC_API_KEY".into()));
        assert!(granted.check(&next).is_err());
        let next = granted.with_agent(|agent| agent.env_allow = denylist(&["HOME"]));
        assert!(granted.check(&next).is_err());
    }

    #[test]
    fn harness_token_rejected_when_summaries_dir_differs() {
        let granted = granted();
        let mut next = granted.after.clone();
        next.storage.as_mut().unwrap().summaries_dir = Some("ai".into());
        assert!(granted.check(&next).is_err());
    }

    #[test]
    fn harness_token_rejected_after_binary_identity_change() {
        let granted = granted();
        let later = std::time::SystemTime::now() + Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(&granted.binary)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let error = granted.check(&granted.after).unwrap_err();
        assert!(error.to_string().contains("harness changed"), "{error}");
    }

    #[test]
    fn harness_token_expires() {
        let mut granted = granted();
        granted
            .authority
            .harness_grants
            .get_mut(&granted.token)
            .unwrap()
            .expires_at = Instant::now();
        assert!(granted.check(&granted.after).is_err());
    }

    #[test]
    fn harness_token_cannot_be_mixed_with_other_tokens() {
        let mut granted = granted();
        let consent = granted.authority.mint_consent(
            granted.current.clone(),
            granted.after.clone(),
            "revision",
        );
        for (picker, consent) in [(Some("picker"), None), (None, Some(consent.as_str()))] {
            let error = granted
                .authority
                .authorize_update(
                    &granted.current,
                    "revision",
                    &granted.after,
                    picker,
                    consent,
                    Some(&granted.token),
                )
                .unwrap_err();
            assert!(error.to_string().contains("cannot be combined"), "{error}");
        }
    }

    #[test]
    fn consent_grant_without_harness_cannot_set_agent() {
        let mut granted = granted();
        let consent = granted.authority.mint_consent(
            granted.current.clone(),
            granted.after.clone(),
            "revision",
        );
        assert!(
            granted
                .authority
                .authorize_update(
                    &granted.current,
                    "revision",
                    &granted.after,
                    None,
                    Some(&consent),
                    None,
                )
                .is_err()
        );
    }

    fn hand_candidate() -> Candidate {
        Candidate {
            adapter: Adapter::ClaudeCode,
            harness_id: "claude-code".into(),
            found_at: "/usr/local/bin/claude".into(),
            identity: Some(harness::BinaryIdentity {
                real_path: "/opt/claude/versions/2.1.284/claude".into(),
                size: 1,
                mtime_ns: "1".into(),
                file_id: None,
            }),
            version: Some("2.1.284".into()),
            args: harness::template_args(Adapter::ClaudeCode),
            env_names: Vec::new(),
            help_sha256: Some("0".repeat(64)),
            confirmed_flags: Vec::new(),
            refusal: None,
        }
    }

    /// `current -> after` for the text tests, with a real notes folder so
    /// `current` is trusted and its removals are listed.
    fn text_configs(
        notes: &Path,
        before: &[&str],
        after: &[&str],
    ) -> (ConfigSnapshot, ConfigSnapshot) {
        let current = ConfigSnapshot {
            storage: Some(storage_at(notes, None)),
            agent_denylist: denylist(before),
            ..crate::config::empty_config()
        };
        let next = ConfigSnapshot {
            storage: Some(storage_at(notes, Some("summaries"))),
            agent_denylist: denylist(after),
            agent_denylist_confirmed: true,
            agent: Some(brauser_protocol::AgentConfig {
                env_allow: denylist(&["HOME", "PATH", "ANTHROPIC_API_KEY"]),
                ..agent()
            }),
            ..current.clone()
        };
        (current, next)
    }

    #[test]
    fn harness_confirmation_text_lists_binary_args_env_names_summaries_dir() {
        let folder = tempfile::tempdir().unwrap();
        let (current, after) = text_configs(folder.path(), &[], &["bank.example"]);
        let (title, body) = harness_confirmation_text(&hand_candidate(), &after, &current).unwrap();
        assert_eq!(
            title,
            format!("Set up Claude Code for {APP_NAME} AI commands")
        );
        assert!(!title.contains('\n'));
        let all: Vec<&str> = body.lines().collect();
        // Choosing the first summaries folder is the one change, shown first.
        assert_eq!(all[0], "This setup also makes these changes:");
        assert!(all[1].starts_with("• "), "{body}");
        assert!(all[1].contains("summaries folder"), "{body}");
        assert_eq!(all[2], "");
        let lines = &all[3..];
        assert_eq!(
            lines[0],
            format!(
                "Harness: Claude Code 2.1.284 at {:?}",
                crate::config::fixtures::absent_binary("claude")
            )
        );
        assert_eq!(lines[1], "Runs: \"/opt/claude/versions/2.1.284/claude\"");
        assert_eq!(lines[2], "");
        assert_eq!(
            lines[3],
            "Arguments (the page goes on stdin, never in arguments):"
        );
        // Claude Code has no --disable pairs, so every argument is on one line.
        let args = harness::template_args(Adapter::ClaudeCode);
        let words: Vec<String> = args.iter().map(|arg| shell_word(arg)).collect();
        assert_eq!(lines[4], words.join(" "));
        let rest = &lines[5..];
        assert_eq!(
            rest[0],
            "Variables passed (values hidden): HOME, PATH, ANTHROPIC_API_KEY"
        );
        assert_eq!(
            rest[1],
            format!(
                "Summaries: \"summaries\" in {:?}",
                folder.path().to_string_lossy()
            )
        );
        assert!(body.contains("Timeout: 120 seconds"));
        assert!(body.ends_with(
            "Next, one test prompt goes to Claude Code. Nothing turns on unless it passes."
        ));
    }

    #[test]
    fn split_disabled_groups_plain_disable_pairs_in_order() {
        let args: Vec<String> = [
            "exec",
            "--disable",
            "apps",
            "-c",
            "x=1",
            "--disable",
            "a b",
            "--disable",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let (disabled, rest) = split_disabled(&args);
        assert_eq!(disabled, ["apps"]);
        // A feature needing quotes, and a trailing --disable, stay verbatim.
        assert_eq!(rest, ["exec", "-c", "x=1", "--disable", "a b", "--disable"]);
    }

    #[test]
    fn shell_word_quotes_only_when_needed() {
        assert_eq!(shell_word("--sandbox"), "--sandbox");
        assert_eq!(shell_word("read-only"), "read-only");
        assert_eq!(
            shell_word("approval_policy=\"never\""),
            "'approval_policy=\"never\"'"
        );
        assert_eq!(shell_word("{prompt}"), "'{prompt}'");
        assert_eq!(shell_word("a b"), "'a b'");
        assert_eq!(shell_word("it's"), r"'it'\''s'");
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn harness_confirmation_text_says_no_domains_excluded_when_empty() {
        let folder = tempfile::tempdir().unwrap();
        let (current, after) = text_configs(folder.path(), &[], &[]);
        let (_, body) = harness_confirmation_text(&hand_candidate(), &after, &current).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        let heading = lines
            .iter()
            .position(|line| line.starts_with("AI privacy exclusions"))
            .unwrap();
        assert!(lines[heading].ends_with(": No domains excluded."), "{body}");
    }

    #[test]
    fn harness_confirmation_text_lists_each_denylist_entry_and_removals() {
        let folder = tempfile::tempdir().unwrap();
        let (current, after) = text_configs(
            folder.path(),
            &["bank.example", "mail.example"],
            &["bank.example", "news.example"],
        );
        let (_, body) = harness_confirmation_text(&hand_candidate(), &after, &current).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        let heading = lines
            .iter()
            .position(|line| line.starts_with("AI privacy exclusions"))
            .unwrap();
        assert!(
            lines[heading].ends_with(": \"bank.example\", \"news.example\""),
            "{body}"
        );
        assert!(!body.contains("No domains excluded."));
        assert!(
            body.contains(
                "• Allow AI commands to read pages on \"mail.example\" and its subdomains."
            )
        );
        assert!(!body.contains("pages on \"bank.example\""));
    }

    /// An untrusted extension may add up to 127 exclusions in the same setup
    /// that drops one the user had. Adding needs no approval, so the removal
    /// must come before the list, where the dialog shows it first.
    #[test]
    fn removal_is_shown_before_a_long_list_of_additions() {
        let folder = tempfile::tempdir().unwrap();
        let added: Vec<String> = (0..127)
            .map(|index| format!("j{index:03}.example"))
            .collect();
        let (mut current, mut next) = text_configs(folder.path(), &["bank.example"], &[]);
        current.agent_denylist = denylist(&["bank.example"]);
        next.agent_denylist = added;
        crate::config::validate(&next).unwrap();
        let (_, body) = harness_confirmation_text(&hand_candidate(), &next, &current).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        let removal = lines
            .iter()
            .position(|line| line.contains("read pages on \"bank.example\""))
            .unwrap();
        let heading = lines
            .iter()
            .position(|line| line.starts_with("AI privacy exclusions"))
            .unwrap();
        assert!(removal < heading, "{body}");
        assert!(removal <= 2, "the removal is line {removal}:\n{body}");
    }

    #[test]
    fn consent_lists_denylist_removal_before_sites() {
        let current = ConfigSnapshot {
            agent_denylist: vec!["bank.example".into()],
            ..set_up()
        };
        let next = ConfigSnapshot {
            agent_denylist: Vec::new(),
            sites: (0..20)
                .map(|index| SiteConfig {
                    origin: format!("https://s{index}.example"),
                    path_prefix: "/".into(),
                })
                .collect(),
            ..current.clone()
        };
        let lines = approval_lines(&current, &next);
        assert!(lines.len() > 20);
        assert!(
            lines[0].contains("read pages on \"bank.example\""),
            "{lines:?}"
        );
    }

    /// Rows `text` takes in the alert: each explicit line, greedily
    /// word-wrapped at [`DIALOG_WRAP_COLUMNS`] characters, with a word too
    /// long for one row split across rows.
    fn visual_lines(text: &str) -> usize {
        text.split('\n')
            .map(|line| {
                let mut rows = 1;
                let mut column = 0;
                for word in line.split(' ') {
                    let width = word.chars().count();
                    let needed = if column == 0 {
                        width
                    } else {
                        column + 1 + width
                    };
                    if needed <= DIALOG_WRAP_COLUMNS {
                        column = needed;
                    } else if column == 0 {
                        rows += (width - 1) / DIALOG_WRAP_COLUMNS;
                        column = (width - 1) % DIALOG_WRAP_COLUMNS + 1;
                    } else {
                        rows += 1 + width.saturating_sub(1) / DIALOG_WRAP_COLUMNS;
                        column = width.saturating_sub(1) % DIALOG_WRAP_COLUMNS + 1;
                    }
                }
                rows
            })
            .sum()
    }

    #[test]
    fn visual_lines_wraps_at_the_documented_width() {
        assert_eq!(visual_lines(""), 1);
        assert_eq!(visual_lines("a\n\nb"), 3);
        assert_eq!(visual_lines(&"x".repeat(DIALOG_WRAP_COLUMNS)), 1);
        assert_eq!(visual_lines(&"x".repeat(DIALOG_WRAP_COLUMNS + 1)), 2);
        // Two half-width words and the space between them overflow a row.
        let word = "w".repeat(DIALOG_WRAP_COLUMNS / 2);
        assert_eq!(visual_lines(&format!("{word} {word}")), 2);
        let word = "w".repeat(DIALOG_WRAP_COLUMNS / 2 - 1);
        assert_eq!(visual_lines(&format!("{word} {word}")), 1);
        assert_eq!(visual_lines(&format!("{word} {word} {word}")), 2);
        assert_eq!(
            visual_lines(&format!("ab {}", "x".repeat(2 * DIALOG_WRAP_COLUMNS))),
            3
        );
        // Characters, not bytes.
        assert_eq!(visual_lines(&"é".repeat(DIALOG_WRAP_COLUMNS)), 1);
    }

    /// A realistic first setup: four exclusions and one change (choosing the
    /// first summaries folder), on typical install and notes paths.
    fn realistic_setup(adapter: Adapter) -> (Candidate, ConfigSnapshot, ConfigSnapshot) {
        let (found_at, real_path, version, args, env_allow) = match adapter {
            Adapter::ClaudeCode => (
                "/Users/someone/.local/bin/claude",
                "/Users/someone/.local/share/claude/versions/2.1.284",
                "2.1.284",
                harness::template_args(Adapter::ClaudeCode),
                denylist(&["HOME", "PATH", "ANTHROPIC_API_KEY"]),
            ),
            Adapter::Codex => (
                "/opt/homebrew/bin/codex",
                "/opt/homebrew/Caskroom/codex/0.144.4/codex-aarch64-apple-darwin",
                "0.144.4",
                harness::codex_args("0.144.4").unwrap(),
                denylist(&["HOME", "PATH", "CODEX_HOME"]),
            ),
        };
        let candidate = Candidate {
            adapter,
            harness_id: adapter.harness_id().into(),
            found_at: found_at.into(),
            identity: Some(harness::BinaryIdentity {
                real_path: real_path.into(),
                ..hand_candidate().identity.unwrap()
            }),
            version: Some(version.into()),
            args: args.clone(),
            ..hand_candidate()
        };
        let exclusions = denylist(&[
            "bank.example",
            "mail.google.com",
            "health.example.org",
            "intranet.corp.example",
        ]);
        let notes = Path::new("/Users/someone/Documents/Notes");
        let current = ConfigSnapshot {
            storage: Some(storage_at(notes, None)),
            agent_denylist: exclusions.clone(),
            ..crate::config::empty_config()
        };
        let next = ConfigSnapshot {
            storage: Some(storage_at(notes, Some("summaries"))),
            agent_denylist_confirmed: true,
            agent: Some(brauser_protocol::AgentConfig {
                harness_id: adapter.harness_id().into(),
                adapter: adapter.into(),
                binary: found_at.into(),
                args,
                env_allow,
                ..agent()
            }),
            ..current.clone()
        };
        assert_eq!(approval_lines(&current, &next).len(), 1);
        (candidate, current, next)
    }

    /// Rows the realistic `adapter` setup's title and description take.
    fn realistic_rows(adapter: Adapter) -> (usize, String) {
        let (candidate, current, next) = realistic_setup(adapter);
        let (title, body) = harness_confirmation_text(&candidate, &next, &current).unwrap();
        (
            visual_lines(&title) + visual_lines(&body),
            format!("{title}\n{body}"),
        )
    }

    /// The bytes bound alone allowed a 45-line Codex alert.
    #[test]
    fn claude_code_confirmation_fits_the_dialog_line_budget() {
        let (rows, text) = realistic_rows(Adapter::ClaudeCode);
        assert!(
            rows <= HARNESS_DIALOG_LINE_BUDGET,
            "{rows} rows, over {HARNESS_DIALOG_LINE_BUDGET}:\n{text}"
        );
    }

    /// Listed verbatim, Codex's 28 `--disable` pairs wrapped to 12 rows and
    /// the alert to 40; they are grouped on one labeled line instead.
    #[test]
    fn codex_confirmation_fits_the_dialog_line_budget() {
        let (rows, text) = realistic_rows(Adapter::Codex);
        assert!(
            rows <= HARNESS_DIALOG_LINE_BUDGET,
            "{rows} rows, over {HARNESS_DIALOG_LINE_BUDGET}:\n{text}"
        );
    }

    #[test]
    fn harness_confirmation_is_bounded_like_consent() {
        let folder = tempfile::tempdir().unwrap();
        // 64 maximum-length additions, over the consent dialog's 8 KiB.
        let host = |index: usize| {
            format!(
                "h{index:02}{}.{}.{}.{}",
                "a".repeat(60),
                "b".repeat(63),
                "c".repeat(63),
                "d".repeat(61)
            )
        };
        let (current, mut next) = text_configs(folder.path(), &[], &[]);
        next.agent_denylist = (0..64).map(host).collect();
        crate::config::validate(&next).unwrap();
        let error = harness_confirmation_text(&hand_candidate(), &next, &current).unwrap_err();
        assert!(error.to_string().contains("too long"), "{error}");
        next.agent_denylist.truncate(16);
        let (title, body) = harness_confirmation_text(&hand_candidate(), &next, &current).unwrap();
        assert!(title.len() + 1 + body.len() <= MAX_HARNESS_SUMMARY_BYTES);
    }

    #[test]
    fn oversized_confirmation_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        // 128 distinct 252-byte hostnames removed and 128 others added.
        let host = |prefix: char, index: usize| {
            format!(
                "{prefix}{index:02}{}.{}.{}.{}",
                "a".repeat(60),
                "b".repeat(63),
                "c".repeat(63),
                "d".repeat(60)
            )
        };
        let before: Vec<String> = (0..64)
            .flat_map(|index| [host('p', index), host('q', index)])
            .collect();
        let after: Vec<String> = (0..64)
            .flat_map(|index| [host('r', index), host('s', index)])
            .collect();
        let (mut current, mut next) = text_configs(folder.path(), &[], &[]);
        current.agent_denylist = before;
        next.agent_denylist = after;
        crate::config::validate(&current).unwrap();
        crate::config::validate(&next).unwrap();
        let error = harness_confirmation_text(&hand_candidate(), &next, &current).unwrap_err();
        assert!(error.to_string().contains("too long"), "{error}");
    }

    /// Step 1 refuses harness setup off unix: even a checked candidate gets
    /// a refusal and no offer id to confirm.
    #[cfg(not(unix))]
    #[test]
    fn setup_offer_is_refused_on_this_platform() {
        let offer = ConsentAuthority::new().offer("revision", hand_candidate());
        assert_eq!(offer.offer_id, None);
        assert_eq!(
            offer.refusal.as_deref(),
            Some(harness::UNSUPPORTED_PLATFORM)
        );
    }

    #[test]
    fn confirmation_with_unsafe_text_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let (current, after) = text_configs(folder.path(), &[], &[]);
        let mut candidate = hand_candidate();
        candidate.version = Some("2.1.284\u{202e}".into());
        assert!(harness_confirmation_text(&candidate, &after, &current).is_err());
        let mut candidate = hand_candidate();
        candidate.identity.as_mut().unwrap().real_path = "/opt/\u{2066}claude".into();
        assert!(harness_confirmation_text(&candidate, &after, &current).is_err());
    }
}

/// The setup flow against fake harnesses. Each fake is a `#!/bin/sh`
/// script in a temp dir; no real harness is ever found or run.
#[cfg(all(test, unix))]
mod setup_tests {
    use std::cell::RefCell;

    use brauser_protocol::{PROTOCOL_VERSION, StorageConfig};

    use super::*;
    use crate::harness::fake::{Fake, PASSING_BODY};

    const SECRET: &str = "sk-test-secret-value";

    struct Setup {
        fake: Fake,
        env: HarnessEnv,
        current: ConfigSnapshot,
        authority: ConsentAuthority,
    }

    fn setup(body: &str) -> Setup {
        let fake = Fake::new();
        fake.claude(body);
        let notes = fake.path("notes");
        fs::create_dir(&notes).unwrap();
        let env = fake.env(&[("ANTHROPIC_API_KEY", SECRET), ("OPENAI_API_KEY", SECRET)]);
        let current = ConfigSnapshot {
            storage: Some(StorageConfig {
                root: notes.to_string_lossy().into_owned(),
                profile: "neutral".into(),
                log_dir: "log".into(),
                pages_dir: "pages".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: None,
            }),
            agent_denylist: vec!["bank.example".into()],
            ..crate::config::empty_config()
        };
        Setup {
            fake,
            env,
            current,
            authority: ConsentAuthority::new(),
        }
    }

    impl Setup {
        fn offer(&mut self, revision: &str) -> String {
            let offers = self.authority.discover_harnesses(revision, &self.env);
            assert_eq!(offers.len(), 1);
            offers[0]
                .offer_id
                .clone()
                .expect("the fake claude is offered")
        }

        fn confirm(
            &mut self,
            revision: &str,
            offer_id: &str,
            env_names: &[&str],
            dialog: impl FnOnce(DialogText<'_>) -> Result<bool>,
        ) -> Result<Option<HarnessConfirmation>, SetupError> {
            let request = ConfirmHarnessSetupRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "r1".into(),
                expected_revision: revision.into(),
                offer_id: offer_id.into(),
                env_names: env_names.iter().map(|name| (*name).to_owned()).collect(),
                agent_denylist: vec!["bank.example".into()],
                summaries_dir: None,
            };
            let current = self.current.clone();
            self.authority
                .confirm_harness_setup_with(&current, revision, &request, &self.env, dialog)
        }

        fn probed(&self) -> bool {
            self.fake.log("claude", "stdin.log").is_some()
        }
    }

    fn no_dialog(_: DialogText<'_>) -> Result<bool> {
        panic!("the dialog must not be shown")
    }

    fn cancel(_: DialogText<'_>) -> Result<bool> {
        Ok(false)
    }

    fn accept(_: DialogText<'_>) -> Result<bool> {
        Ok(true)
    }

    #[test]
    fn offer_id_bound_to_revision() {
        let mut setup = setup(PASSING_BODY);
        let offer = setup.offer("revision");
        let error = setup
            .confirm("other", &offer, &[], no_dialog)
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::Unauthorized);
        // The stale attempt spent the offer.
        let error = setup
            .confirm("revision", &offer, &[], no_dialog)
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::Unauthorized);
        assert!(!setup.probed());
    }

    #[test]
    fn offer_is_single_use_even_on_cancel() {
        let mut setup = setup(PASSING_BODY);
        let offer = setup.offer("revision");
        assert!(
            setup
                .confirm("revision", &offer, &[], cancel)
                .unwrap()
                .is_none()
        );
        let error = setup
            .confirm("revision", &offer, &[], no_dialog)
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::Unauthorized);
        assert!(error.message.contains("unknown or already used"));
    }

    #[test]
    fn env_names_outside_offer_rejected() {
        let mut setup = setup(PASSING_BODY);
        for name in ["OPENAI_API_KEY", "DYLD_INSERT_LIBRARIES", "SHELL"] {
            let offer = setup.offer("revision");
            let error = setup
                .confirm("revision", &offer, &[name], no_dialog)
                .err()
                .unwrap();
            assert_eq!(error.code, ErrorCode::Unauthorized, "{name}");
        }
        assert!(setup.authority.harness_grants.is_empty());
        assert!(!setup.probed());
    }

    #[test]
    fn cancel_mints_nothing() {
        let mut setup = setup(PASSING_BODY);
        let offer = setup.offer("revision");
        assert!(
            setup
                .confirm("revision", &offer, &[], cancel)
                .unwrap()
                .is_none()
        );
        assert!(setup.authority.harness_grants.is_empty());
        assert!(!setup.probed());
    }

    #[test]
    fn a_timed_out_confirmation_spends_the_offer_and_runs_nothing() {
        let mut setup = setup(PASSING_BODY);
        let offer = setup.offer("revision");
        let error = setup
            .confirm("revision", &offer, &[], |_| {
                anyhow::bail!("native dialog was not answered in time and was closed")
            })
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(
            error.message.contains("not answered in time"),
            "{}",
            error.message
        );
        assert!(setup.authority.harness_grants.is_empty());
        assert!(!setup.probed());
        let error = setup
            .confirm("revision", &offer, &[], no_dialog)
            .err()
            .unwrap();
        assert!(error.message.contains("unknown or already used"));
    }

    #[test]
    fn failed_probe_mints_nothing() {
        let mut setup = setup("exit 3");
        let offer = setup.offer("revision");
        let error = setup
            .confirm("revision", &offer, &[], accept)
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::InvalidConfig);
        assert_eq!(
            error.message,
            "Claude Code failed its test run (test run failed (exit 3)); AI commands stay off"
        );
        assert!(setup.authority.harness_grants.is_empty());
    }

    #[test]
    fn missing_notes_folder_is_not_configured() {
        let mut setup = setup(PASSING_BODY);
        setup.current.storage = None;
        let offer = setup.offer("revision");
        let error = setup
            .confirm("revision", &offer, &[], no_dialog)
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::NotConfigured);
    }

    #[test]
    fn harness_confirmation_text_never_contains_env_values() {
        let mut setup = setup(PASSING_BODY);
        let offer = setup.offer("revision");
        let shown = RefCell::new(String::new());
        let confirmed = setup
            .confirm("revision", &offer, &["ANTHROPIC_API_KEY"], |text| {
                *shown.borrow_mut() = format!("{}\n{}", text.title, text.description);
                Ok(true)
            })
            .unwrap()
            .unwrap();
        let shown = shown.into_inner();
        assert!(shown.contains("HOME, PATH, ANTHROPIC_API_KEY"));
        assert!(!shown.contains(SECRET));
        assert!(!confirmed.summary.contains(SECRET));
        assert!(!format!("{:?}", confirmed.config).contains(SECRET));
        // The probe itself did get the allowlisted name.
        let names = setup.fake.env_names("claude");
        assert!(
            names
                .last()
                .unwrap()
                .contains(&"ANTHROPIC_API_KEY".to_owned())
        );
        assert!(!names.last().unwrap().contains(&"OPENAI_API_KEY".to_owned()));
    }

    #[test]
    fn confirmed_setup_authorizes_exactly_the_returned_config() {
        let mut setup = setup(PASSING_BODY);
        let offer = setup.offer("revision");
        let confirmed = setup
            .confirm("revision", &offer, &[], accept)
            .unwrap()
            .unwrap();
        let agent = confirmed.config.agent.clone().unwrap();
        assert_eq!(
            agent.binary,
            setup.fake.path("bin").join("claude").to_string_lossy()
        );
        assert_eq!(agent.env_allow, vec!["HOME".to_owned(), "PATH".to_owned()]);
        assert_eq!(agent.timeout_secs, 120);
        assert!(confirmed.config.agent_denylist_confirmed);
        assert_eq!(
            confirmed
                .config
                .storage
                .as_ref()
                .unwrap()
                .summaries_dir
                .as_deref(),
            Some("summaries")
        );
        let authorized = setup
            .authority
            .authorize_update(
                &setup.current,
                "revision",
                &confirmed.config,
                None,
                None,
                Some(&confirmed.harness_token),
            )
            .unwrap();
        let record = authorized.harness_commit.unwrap().record;
        assert_eq!(record.version, "2.1.284");
        assert!(time::OffsetDateTime::parse(&record.probe_passed_at, &Rfc3339).is_ok());
    }

    #[test]
    fn harness_setup_preserves_an_invalid_adapter_document() {
        let mut setup = setup(PASSING_BODY);
        setup.current.adapters.format = 2;
        let offer = setup.offer("revision");

        let confirmed = setup
            .confirm("revision", &offer, &[], accept)
            .unwrap()
            .unwrap();

        assert_eq!(confirmed.config.adapters, setup.current.adapters);
        setup
            .authority
            .authorize_update(
                &setup.current,
                "revision",
                &confirmed.config,
                None,
                None,
                Some(&confirmed.harness_token),
            )
            .unwrap();
    }

    #[test]
    fn discover_offers_no_id_for_refused_codex() {
        let mut setup = setup(PASSING_BODY);
        setup
            .fake
            .install("codex", "codex-cli 0.130.0", "", "exit 0");
        let offers = setup.authority.discover_harnesses("revision", &setup.env);
        let codex = offers
            .iter()
            .find(|offer| offer.harness_id == "codex")
            .unwrap();
        assert_eq!(codex.offer_id, None);
        assert_eq!(
            codex.refusal.as_deref(),
            Some("Codex 0.130.0 has not been reviewed for Brauser")
        );
        assert_eq!(setup.authority.offer_grants.len(), 1);
        assert_eq!(
            setup.fake.log("codex", "argv.log").as_deref(),
            Some("[--version]\n")
        );
        let claude = offers
            .iter()
            .find(|offer| offer.harness_id == "claude-code")
            .unwrap();
        assert_eq!(
            claude.env_required,
            vec!["HOME".to_owned(), "PATH".to_owned()]
        );
        assert!(
            claude
                .env_optional
                .iter()
                .any(|env| env.name == "ANTHROPIC_API_KEY" && env.present)
        );
        assert!(!claude.env_optional.iter().any(|env| env.name == "HOME"));
    }

    fn codex_setup(extra_env: &[(&str, &str)]) -> Setup {
        let fake = Fake::new();
        fake.codex(harness::fake::CODEX_PASSING_BODY);
        let notes = fake.path("notes");
        fs::create_dir(&notes).unwrap();
        let mut env = vec![("OPENAI_API_KEY", SECRET)];
        env.extend_from_slice(extra_env);
        Setup {
            env: fake.env(&env),
            current: ConfigSnapshot {
                storage: Some(StorageConfig {
                    root: notes.to_string_lossy().into_owned(),
                    profile: "neutral".into(),
                    log_dir: "log".into(),
                    pages_dir: "pages".into(),
                    later_dir: "later".into(),
                    artifacts_dir: "artifacts".into(),
                    summaries_dir: None,
                }),
                agent_denylist: vec!["bank.example".into()],
                ..crate::config::empty_config()
            },
            fake,
            authority: ConsentAuthority::new(),
        }
    }

    #[test]
    fn codex_setup_passes_the_users_codex_home_when_set() {
        let mut setup = codex_setup(&[("CODEX_HOME", "/users/codex-home")]);
        let offers = setup.authority.discover_harnesses("revision", &setup.env);
        assert_eq!(offers[0].env_required, ["HOME", "PATH", "CODEX_HOME"]);
        assert!(offers[0].env_optional.is_empty());
        let offer = offers[0].offer_id.clone().unwrap();
        let shown = RefCell::new(String::new());
        let confirmed = setup
            .confirm("revision", &offer, &[], |text| {
                *shown.borrow_mut() = text.description.to_owned();
                Ok(true)
            })
            .unwrap()
            .unwrap();
        let shown = shown.into_inner();
        assert!(
            shown.contains("Variables passed (values hidden): HOME, PATH, CODEX_HOME"),
            "{shown}"
        );
        assert!(!shown.contains("/users/codex-home"));
        config::validate(&confirmed.config).unwrap();
        let agent = confirmed.config.agent.unwrap();
        assert_eq!(agent.env_allow, ["HOME", "PATH", "CODEX_HOME"]);
        // The probe (the fourth run) got it too.
        assert_eq!(
            setup.fake.env_names("codex")[3],
            ["CODEX_HOME", "HOME", "PATH"]
        );
        // Without it in the host's environment, it is neither required nor
        // offered.
        let mut setup = codex_setup(&[]);
        let offers = setup.authority.discover_harnesses("revision", &setup.env);
        assert_eq!(offers[0].env_required, ["HOME", "PATH"]);
        assert!(offers[0].env_optional.is_empty());
    }

    #[test]
    fn codex_setup_discloses_shared_home_and_confirms_reviewed_args() {
        let fake = Fake::new();
        fake.codex(harness::fake::CODEX_PASSING_BODY);
        let notes = fake.path("notes");
        fs::create_dir(&notes).unwrap();
        let mut setup = Setup {
            env: fake.env(&[("OPENAI_API_KEY", SECRET)]),
            current: ConfigSnapshot {
                storage: Some(StorageConfig {
                    root: notes.to_string_lossy().into_owned(),
                    profile: "neutral".into(),
                    log_dir: "log".into(),
                    pages_dir: "pages".into(),
                    later_dir: "later".into(),
                    artifacts_dir: "artifacts".into(),
                    summaries_dir: None,
                }),
                agent_denylist: vec!["bank.example".into()],
                ..crate::config::empty_config()
            },
            fake,
            authority: ConsentAuthority::new(),
        };
        let offer = setup.offer("revision");
        let shown = RefCell::new(String::new());
        let confirmed = setup
            .confirm("revision", &offer, &[], |text| {
                *shown.borrow_mut() = text.description.to_owned();
                Ok(true)
            })
            .unwrap()
            .unwrap();
        let shown = shown.into_inner();
        let lines: Vec<&str> = shown.lines().collect();
        let harness_line = lines
            .iter()
            .position(|line| line.starts_with("Harness: Codex 0.144.4 at "))
            .unwrap();
        // The disclosure follows Runs directly.
        assert!(lines[harness_line + 1].starts_with("Runs: "), "{shown}");
        assert_eq!(lines[harness_line + 2], CODEX_DISCLOSURE, "{shown}");
        // Every reviewed argument is still shown: the plain ones on the
        // argument line and each --disable feature on its own grouped line.
        let reviewed = harness::codex_args("0.144.4").unwrap();
        let (disabled, rest) = split_disabled(&reviewed);
        let grouped = format!("Also --disable each of: {}", disabled.join(", "));
        assert!(lines.contains(&grouped.as_str()), "{shown}");
        let plain = rest
            .iter()
            .map(|arg| shell_word(arg))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(lines.contains(&plain.as_str()), "{shown}");
        assert!(!shown.contains(SECRET));
        let agent = confirmed.config.agent.unwrap();
        assert_eq!(agent.args, harness::codex_args("0.144.4").unwrap());
        assert_eq!(agent.env_allow, vec!["HOME".to_owned(), "PATH".to_owned()]);
        let argv = setup.fake.log("codex", "argv.log").unwrap();
        assert!(
            argv.lines()
                .nth(2)
                .unwrap()
                .starts_with("[features][list][--disable]")
        );
        assert_eq!(
            argv.lines().count(),
            4,
            "version, exec help, features, probe"
        );
    }
}

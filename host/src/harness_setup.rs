//! Harness setup (spec §6): detect a supported harness on the host's own
//! search path, run one short test prompt through its real invocation, and
//! commit it when the test passes. There is no native dialog and no token:
//! the extension names a detected `harness_id`, never a path or arguments,
//! and the host rediscovers and rechecks it before anything runs.

use std::path::Path;

use brauser_protocol::{
    AgentConfig, ConfigSnapshot, EnvName, ErrorCode, HarnessOffer, SetUpHarnessRequest,
    StorageConfig,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::config::{self, ConfigStore, HarnessCommit, HarnessRecord};
use crate::harness::{self, Candidate, HarnessEnv};

const MAX_OFFERS: usize = 8;
const MAX_REFUSAL_BYTES: usize = 512;
const MAX_OFFER_PATH_BYTES: usize = 4096;
const HARNESS_CHANGED: &str = "harness changed; detect again";
const UNSAFE_PATH: &str = "the harness path cannot be shown safely; move it to a plain path";

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

/// Look for supported harnesses and describe each one. Help text and command
/// output never leave the host.
pub(crate) fn offers(env: &HarnessEnv) -> Vec<HarnessOffer> {
    harness::discover(env)
        .into_iter()
        .take(MAX_OFFERS)
        .map(offer)
        .collect()
}

/// Why this candidate cannot be set up here, if anything: its own refusal,
/// this platform, or a path that cannot be shown safely.
fn refusal(candidate: &Candidate) -> Option<String> {
    if let Some(refusal) = &candidate.refusal {
        return Some(refusal.clone());
    }
    if !cfg!(unix) {
        return Some(harness::UNSUPPORTED_PLATFORM.to_owned());
    }
    let unsafe_found_at = candidate
        .found_at
        .to_str()
        .is_none_or(config::has_unsafe_display_chars);
    let unsafe_real_path = candidate
        .identity
        .as_ref()
        .is_some_and(|identity| config::has_unsafe_display_chars(&identity.real_path));
    (unsafe_found_at || unsafe_real_path).then(|| UNSAFE_PATH.to_owned())
}

fn offer(candidate: Candidate) -> HarnessOffer {
    let refusal = refusal(&candidate);
    let shown = |path: &str| {
        if config::has_unsafe_display_chars(path) {
            path.escape_debug().to_string()
        } else {
            path.to_owned()
        }
    };
    let binary = shown(&candidate.found_at.to_string_lossy());
    let real_path = candidate
        .identity
        .as_ref()
        .map(|identity| shown(&identity.real_path));
    let (env_required, env_optional) = offered_env(&candidate);
    HarnessOffer {
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

/// Set up `request.harness_id` and return the committed revision. In order:
/// check the revision, refuse a config that needs repair, rediscover the
/// harness and refuse a refusal, build the harness entry, probe it, and
/// commit the entry and its checked record in one revision-checked write.
pub(crate) fn set_up(
    config: &mut ConfigStore,
    request: &SetUpHarnessRequest,
    env: &HarnessEnv,
) -> Result<String, SetupError> {
    if config.refresh().is_err() {
        return Err(SetupError::new(
            ErrorCode::InvalidConfig,
            "configuration file is unavailable",
        ));
    }
    if request.expected_revision != config.revision() {
        return Err(SetupError::new(
            ErrorCode::Conflict,
            "configuration changed; read it again before setting up AI",
        ));
    }
    // Setup rewrites the whole file from this snapshot, so it must be the
    // file's own content, not a stand-in for one that needs repair.
    if config.config_issue().is_some() {
        return Err(SetupError::new(
            ErrorCode::InvalidConfig,
            "configuration needs repair before AI setup",
        ));
    }
    let current = config.snapshot().clone();
    let Some(storage) = &current.storage else {
        return Err(SetupError::new(
            ErrorCode::NotConfigured,
            "Choose a home folder first",
        ));
    };
    let Some(candidate) = harness::rediscover(env, &request.harness_id) else {
        return Err(SetupError::unauthorized(format!(
            "harness {:?} was not found; detect again",
            request.harness_id
        )));
    };
    if let Some(refusal) = refusal(&candidate) {
        return Err(SetupError::unauthorized(format!(
            "this harness cannot be set up: {}",
            bounded(refusal, MAX_REFUSAL_BYTES)
        )));
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
        .unwrap_or_else(|| config::DEFAULT_SUMMARIES_DIR.to_owned());
    let after = ConfigSnapshot {
        storage: Some(StorageConfig {
            summaries_dir: Some(summaries_dir),
            ..storage.clone()
        }),
        agent: Some(agent.clone()),
        ..current.clone()
    };
    config::validate_update(&current, &after)
        .map_err(|error| SetupError::new(ErrorCode::InvalidConfig, error.to_string()))?;
    harness::probe(&candidate, &agent.env_allow, env, agent.timeout_secs).map_err(|failure| {
        SetupError::new(
            ErrorCode::InvalidConfig,
            format!(
                "{} failed its test run ({failure}); AI commands stay off",
                candidate.adapter.display_name()
            ),
        )
    })?;
    let probe_passed_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| SetupError::new(ErrorCode::Internal, "could not record the test run time"))?;
    // The binary must still be the one that was checked and probed.
    let unchanged = |path: &Path| harness::identity(path).is_ok_and(|now| now == checked);
    if !unchanged(Path::new(&checked.real_path)) || !unchanged(&candidate.found_at) {
        return Err(SetupError::unauthorized(HARNESS_CHANGED));
    }
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
    match config.update_with_grants(after, &request.expected_revision, None, None, Some(commit)) {
        Ok(Some(revision)) => Ok(revision),
        Ok(None) => Err(SetupError::new(
            ErrorCode::Conflict,
            "configuration changed; read it again before setting up AI",
        )),
        Err(_) => Err(SetupError::new(
            ErrorCode::Internal,
            "could not save configuration",
        )),
    }
}

/// `HOME`, `PATH`, and any home name the host has, always; the other names
/// are the adapter's optional ones.
fn offered_env(candidate: &Candidate) -> (Vec<String>, Vec<EnvName>) {
    let homes = candidate.adapter.home_names();
    let optional = candidate
        .env_names
        .iter()
        .filter(|env| {
            !harness::REQUIRED_ENV.contains(&env.name.as_str())
                && !homes.contains(&env.name.as_str())
        })
        .map(|env| EnvName {
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

/// The setup flow against fake harnesses. Each fake is a `#!/bin/sh`
/// script in a temp dir; no real harness is ever found or run.
#[cfg(all(test, unix))]
mod tests {
    use std::fs;

    use brauser_protocol::PROTOCOL_VERSION;

    use super::*;
    use crate::harness::fake::{CODEX_PASSING_BODY, Fake, PASSING_BODY};

    const SECRET: &str = "sk-test-secret-value";

    struct Setup {
        fake: Fake,
        env: HarnessEnv,
        path: std::path::PathBuf,
        config: ConfigStore,
    }

    fn setup_with(install: impl FnOnce(&Fake), vars: &[(&str, &str)]) -> Setup {
        let fake = Fake::new();
        install(&fake);
        let notes = fake.path("notes");
        fs::create_dir(&notes).unwrap();
        let path = fake.path("config.toml");
        config::fixtures::write(&path, &notes, "", "", "");
        let mut config = ConfigStore::for_test(path.clone());
        config.refresh().unwrap();
        Setup {
            env: fake.env(vars),
            fake,
            path,
            config,
        }
    }

    fn setup(body: &str) -> Setup {
        setup_with(
            |fake| {
                fake.claude(body);
            },
            &[("ANTHROPIC_API_KEY", SECRET), ("OPENAI_API_KEY", SECRET)],
        )
    }

    impl Setup {
        fn run(&mut self, harness_id: &str, env_names: &[&str]) -> Result<String, SetupError> {
            self.config.refresh().unwrap();
            let request = SetUpHarnessRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "r1".into(),
                expected_revision: self.config.revision().to_owned(),
                harness_id: harness_id.into(),
                env_names: env_names.iter().map(|name| (*name).to_owned()).collect(),
                summaries_dir: None,
            };
            set_up(&mut self.config, &request, &self.env)
        }

        fn probed(&self, program: &str) -> bool {
            self.fake.log(program, "stdin.log").is_some()
        }

        fn agent(&self) -> AgentConfig {
            self.config.snapshot().agent.clone().unwrap()
        }
    }

    #[test]
    fn env_names_outside_offer_rejected() {
        let mut setup = setup(PASSING_BODY);
        let before = fs::read(&setup.path).unwrap();
        for name in ["OPENAI_API_KEY", "DYLD_INSERT_LIBRARIES", "SHELL"] {
            let error = setup.run("claude-code", &[name]).unwrap_err();
            assert_eq!(error.code, ErrorCode::Unauthorized, "{name}");
        }
        assert!(!setup.probed("claude"));
        assert_eq!(fs::read(&setup.path).unwrap(), before);
    }

    #[test]
    fn missing_notes_folder_is_not_configured() {
        let mut setup = setup(PASSING_BODY);
        let mut next = setup.config.snapshot().clone();
        next.storage = None;
        let revision = setup.config.revision().to_owned();
        setup.config.update(next, &revision).unwrap().unwrap();
        let error = setup.run("claude-code", &[]).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotConfigured);
        assert!(!setup.probed("claude"));
    }

    #[test]
    fn setup_passes_only_allowlisted_names_and_stores_no_values() {
        let mut setup = setup(PASSING_BODY);
        setup.run("claude-code", &["ANTHROPIC_API_KEY"]).unwrap();
        let agent = setup.agent();
        assert_eq!(agent.env_allow, ["HOME", "PATH", "ANTHROPIC_API_KEY"]);
        assert_eq!(agent.timeout_secs, 120);
        assert!(!fs::read_to_string(&setup.path).unwrap().contains(SECRET));
        // The probe itself did get the allowlisted name, and only it.
        let names = setup.fake.env_names("claude");
        let probe = names.last().unwrap();
        assert!(probe.contains(&"ANTHROPIC_API_KEY".to_owned()));
        assert!(!probe.contains(&"OPENAI_API_KEY".to_owned()));
    }

    #[test]
    fn harness_setup_preserves_an_invalid_adapter_document() {
        let mut setup = setup(PASSING_BODY);
        let tables = "[adapters]\nformat = 2\n";
        config::fixtures::write(&setup.path, &setup.fake.path("notes"), "", "", tables);
        setup.config.refresh().unwrap();
        let adapters = setup.config.snapshot().adapters.clone();
        assert_eq!(adapters.format, 2);
        setup.run("claude-code", &[]).unwrap();
        assert_eq!(setup.config.snapshot().adapters, adapters);
        assert_eq!(setup.agent().harness_id, "claude-code");
    }

    #[test]
    fn offers_list_required_and_optional_names_and_refuse_unreviewed_codex() {
        let setup = setup(PASSING_BODY);
        setup
            .fake
            .install("codex", "codex-cli 0.130.0", "", "exit 0");
        let found = offers(&setup.env);
        let codex = found
            .iter()
            .find(|offer| offer.harness_id == "codex")
            .unwrap();
        assert_eq!(
            codex.refusal.as_deref(),
            Some("Codex 0.130.0 has not been reviewed for Brauser")
        );
        let claude = found
            .iter()
            .find(|offer| offer.harness_id == "claude-code")
            .unwrap();
        assert_eq!(claude.refusal, None);
        assert_eq!(claude.env_required, ["HOME", "PATH"]);
        assert!(
            claude
                .env_optional
                .iter()
                .any(|env| env.name == "ANTHROPIC_API_KEY" && env.present)
        );
        assert!(!claude.env_optional.iter().any(|env| env.name == "HOME"));
    }

    #[test]
    fn setup_checks_only_the_named_harness() {
        let mut setup = setup(PASSING_BODY);
        setup
            .fake
            .install("codex", "codex-cli 0.130.0", "", "exit 0");
        setup.run("claude-code", &[]).unwrap();
        assert_eq!(setup.fake.log("codex", "argv.log"), None);
    }

    #[test]
    fn codex_setup_passes_the_users_codex_home_when_set() {
        let mut setup = setup_with(
            |fake| {
                fake.codex(CODEX_PASSING_BODY);
            },
            &[
                ("OPENAI_API_KEY", SECRET),
                ("CODEX_HOME", "/users/codex-home"),
            ],
        );
        let found = offers(&setup.env);
        assert_eq!(found[0].env_required, ["HOME", "PATH", "CODEX_HOME"]);
        assert!(found[0].env_optional.is_empty());
        setup.run("codex", &[]).unwrap();
        config::validate(setup.config.snapshot()).unwrap();
        let agent = setup.agent();
        assert_eq!(agent.env_allow, ["HOME", "PATH", "CODEX_HOME"]);
        assert_eq!(agent.args, harness::codex_args("0.144.4").unwrap());
        assert!(
            !fs::read_to_string(&setup.path)
                .unwrap()
                .contains("/users/codex-home")
        );
        // Discovery ran three checks, then setup ran them again and probed.
        let names = setup.fake.env_names("codex");
        assert_eq!(names.len(), 7);
        assert_eq!(names[6], ["CODEX_HOME", "HOME", "PATH"]);
        let argv = setup.fake.log("codex", "argv.log").unwrap();
        assert!(
            argv.lines()
                .nth(5)
                .unwrap()
                .starts_with("[features][list][--disable]")
        );

        // Without it in the host's environment, it is neither required nor
        // offered.
        let setup = setup_with(
            |fake| {
                fake.codex(CODEX_PASSING_BODY);
            },
            &[("OPENAI_API_KEY", SECRET)],
        );
        let found = offers(&setup.env);
        assert_eq!(found[0].env_required, ["HOME", "PATH"]);
        assert!(found[0].env_optional.is_empty());
    }
}

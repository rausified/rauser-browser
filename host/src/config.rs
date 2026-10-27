//! The host owns configuration. An absent file is an unconfigured, inert state.
//! An update is validated in full before the old file is atomically replaced.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adapter_templates::{Document as AdapterTemplates, NoMatch, Value as AdapterValue};
use anyhow::{Context, Result, bail};
use brauser_protocol::{
    AdapterDocument, AdapterWarning, AgentConfig, AgentState, AgentStatus, ConfigSnapshot,
    HarnessId, StorageConfig, SummaryVocabulary,
};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[cfg(test)]
thread_local! {
    static ADAPTER_COMPILE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static ADAPTER_OVERLAP_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

use crate::brand::{APP_NAME, NAMESPACE};
use crate::index::ArtifactIndex;
use crate::layout;
use crate::vault::{
    STALE_TEMPORARY_AGE, Vault, is_generated_temporary, open_selected_root, selected_root_identity,
};

const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_ROOT_PATH_BYTES: usize = 4 * 1024;
const MAX_LOCATION_PATH_BYTES: usize = 512;
const MAX_PATH_COMPONENT_BYTES: usize = 255;
const MAX_STRIP_PARAMS: usize = 64;
const MAX_RULE_BYTES: usize = 64;
const MAX_VERSION_BYTES: usize = 256;
const MAX_PROBED_AT_BYTES: usize = 64;
const MAX_ADAPTER_WARNINGS: usize = 256;
const MAX_VOCABULARY_ENTRIES: usize = 64;
const MAX_VOCABULARY_CHARS: usize = 64;
/// A summary run's time limit when setup is given none, and its bounds.
// Only the service, which is Unix-only, sets a harness up.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const DEFAULT_SUMMARY_TIMEOUT_SECS: u32 = 600;
pub(crate) const SUMMARY_TIMEOUT_SECS: std::ops::RangeInclusive<u32> = 60..=3600;
/// The summaries folder harness setup uses when none is chosen.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const DEFAULT_SUMMARIES_DIR: &str = "summaries";
const MISSING_REVISION: &str = "missing";
/// Never a real revision, so no update can be based on an unreadable file.
const UNREADABLE_REVISION: &str = "unreadable";
/// Hash at most this much of an oversized config. Past it, the revision
/// also covers the length so a growing file still changes revision.
const MAX_HASHED_CONFIG_BYTES: u64 = 16 * 1024 * 1024;

/// At most this many contracts, and this many bytes, in the adopted-contract
/// state file. A larger file is unreadable; a write that would exceed either
/// bound is skipped.
const MAX_ADOPTED_CONTRACTS: usize = 1024;
const MAX_ADOPTED_CONTRACTS_BYTES: usize = 1024 * 1024;
const ADOPTED_CONTRACTS_FORMAT: u32 = 1;

/// The host-owned record of every adapter contract a host has adopted under
/// the config lock (see `AdoptedContracts`). It lives beside config.toml in
/// the OS config directory, or beside an injected test config; like the rest
/// of the host's state, it stays out of the home folder (DESIGN.md §6.1).
pub(crate) fn adopted_contracts_path(config_dir: &Path) -> PathBuf {
    config_dir.join("adopted-contracts.json")
}

pub(crate) fn default_strip_params() -> Vec<String> {
    vec!["utm_*".into(), "fbclid".into(), "gclid".into()]
}

/// The document types the summary prompt offers until the user edits them.
pub const DEFAULT_DOCUMENT_TYPES: [&str; 6] = [
    "report",
    "announcement",
    "ticket",
    "specification",
    "email",
    "article",
];
/// The categories the summary prompt offers until the user edits them.
pub const DEFAULT_CATEGORIES: [&str; 7] = [
    "technical specification",
    "product plan",
    "administrative",
    "project plan",
    "marketing material",
    "research paper",
    "news",
];

pub(crate) fn default_summary_vocabulary() -> SummaryVocabulary {
    SummaryVocabulary {
        document_types: DEFAULT_DOCUMENT_TYPES.map(String::from).to_vec(),
        categories: DEFAULT_CATEGORIES.map(String::from).to_vec(),
    }
}

pub struct ConfigStore {
    path: PathBuf,
    config: ConfigSnapshot,
    revision: String,
    issue: Option<String>,
    root_identity: Option<String>,
    knowledge_base_identity: Option<String>,
    agent: AgentView,
    adapter_extensions: BTreeMap<String, toml::Value>,
    adapter_issue: Option<String>,
    adapter_warnings: Option<Vec<AdapterWarning>>,
    adapter_document: Option<AdapterTemplates>,
    /// Superseded recognition contracts from `[[adapter_id_history]]`.
    retired_adapter_contracts: Vec<serde_json::Value>,
    /// Note-only identity rules; see `note_key_rule`.
    note_key_rules: NoteKeyRules,
    /// Set while a `ConfigLock` from `lock_current` is alive.
    lock_held: Arc<AtomicBool>,
    /// The adopted-contract state as last read.
    adopted: AdoptedContracts,
    /// This revision's contracts in recordable form, computed on the first
    /// refresh under the lock.
    recordable: Option<Vec<serde_json::Value>>,
}

/// The config lock from `ConfigStore::lock_current`. While it is alive,
/// that store's refreshes record the contracts they adopt.
#[must_use]
pub struct ConfigLock {
    held: Arc<AtomicBool>,
    _file: File,
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        // Fields drop after this, so the flag clears before the unlock.
        self.held.store(false, Ordering::SeqCst);
    }
}

struct DiskState {
    config: ConfigSnapshot,
    revision: String,
    issue: Option<String>,
    needs_backup: bool,
    root_identity: Option<String>,
    knowledge_base_identity: Option<String>,
    agent: AgentView,
    adapter_extensions: BTreeMap<String, toml::Value>,
    retired_adapter_contracts: Vec<serde_json::Value>,
}

/// The raw `[summarizer]` table as last read from disk, and what it parses
/// to. It is parsed apart from the rest of the file, so a bad entry disables
/// only summaries and capture keeps working. An update without a harness
/// commit carries the raw table forward.
#[derive(Clone, Default)]
struct AgentView {
    table: Option<toml::Value>,
    summarizer: Option<StoredSummarizer>,
    issue: Option<String>,
}

const AGENT_ENTRY_INVALID: &str = concat!(
    "The summary harness entry in config.toml is invalid. Set up the harness again in ",
    crate::app_name!(),
    " settings."
);
const SET_UP_HARNESS: &str = concat!(
    "Set up a summary harness in ",
    crate::app_name!(),
    " settings."
);
const LAYOUT_PENDING: &str = concat!(
    crate::app_name!(),
    " is moving its folders to the fixed layout in your home folder. Reload this page to finish."
);

impl AgentView {
    fn parse(table: Option<toml::Value>) -> Self {
        let parsed = table.clone().map(|value| {
            let summarizer: StoredSummarizer = value.try_into()?;
            validate_summarizer(&summarizer.agent())?;
            anyhow::Ok(summarizer)
        });
        let (summarizer, issue) = match parsed {
            Some(Ok(summarizer)) => (Some(summarizer), None),
            Some(Err(_)) => (None, Some(AGENT_ENTRY_INVALID.to_owned())),
            None => (None, None),
        };
        Self {
            table,
            summarizer,
            issue,
        }
    }

    fn committed(commit: &HarnessCommit) -> Result<Self> {
        let table = toml::Value::try_from(&commit.summarizer)
            .context("serializing the summary harness entry")?;
        let view = Self::parse(Some(table));
        if view.summarizer.as_ref() != Some(&commit.summarizer) {
            bail!("the summary harness entry does not round-trip through config.toml");
        }
        Ok(view)
    }

    fn agent(&self) -> Option<AgentConfig> {
        self.summarizer.as_ref().map(StoredSummarizer::agent)
    }
}

/// `[summarizer]`: the harness the service set up after its probe passed
/// (spec §6.1). The wire calls it `ConfigSnapshot.agent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredSummarizer {
    pub harness: HarnessId,
    pub binary: String,
    #[serde(default)]
    pub version: Option<String>,
    pub timeout_secs: u32,
    pub probed_at: String,
}

impl StoredSummarizer {
    pub fn agent(&self) -> AgentConfig {
        AgentConfig {
            harness: self.harness,
            binary: self.binary.clone(),
            version: self.version.clone(),
            timeout_secs: self.timeout_secs,
            probed_at: self.probed_at.clone(),
        }
    }
}

impl From<&AgentConfig> for StoredSummarizer {
    fn from(agent: &AgentConfig) -> Self {
        Self {
            harness: agent.harness,
            binary: agent.binary.clone(),
            version: agent.version.clone(),
            timeout_secs: agent.timeout_secs,
            probed_at: agent.probed_at.clone(),
        }
    }
}

/// What harness setup commits. Host-internal: only the service's
/// `setup::set_up` builds one, after its probe passes, and passes it to
/// `update_with_grants`. Nothing on the wire can carry or authorize one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommit {
    pub summarizer: StoredSummarizer,
}

struct AgentRefusal {
    state: AgentState,
    message: String,
}

impl ConfigStore {
    pub fn default_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("", "", APP_NAME)
            .context("cannot locate this user's application config directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load config.toml, first moving an older folder layout to the fixed one
    /// (`migrate_layout`). Every native-messaging connection starts a host
    /// process, so this runs on each connection until it succeeds.
    pub fn load() -> Result<Self> {
        let mut store = Self::load_from(Self::default_path()?);
        store.migrate_layout_or_warn();
        Ok(store)
    }

    /// Migrate only when the last read found an older layout, so the usual
    /// path never waits for a lock another host holds for a long write.
    pub(crate) fn migrate_layout_or_warn(&mut self) {
        let older = self
            .config
            .storage
            .as_ref()
            .is_some_and(|storage| !layout::is_fixed(storage));
        if !older {
            return;
        }
        if let Err(error) = self.migrate_layout() {
            eprintln!(
                "{NAMESPACE}: warning: could not move folders to the fixed layout: {error:#}"
            );
        }
    }

    /// Rename an older layout's folders (for example `pages/` and `log/`)
    /// to the fixed names inside the same home folder, then record the fixed
    /// layout in config.toml. Runs under the config lock and the capture
    /// lock, which every vault writer in every host process holds, so no
    /// capture, note save, or regeneration runs meanwhile. Nothing is ever
    /// replaced or merged: if a fixed name already exists beside its older
    /// folder, nothing changes and `config_issue` says what to move. A crash
    /// after a rename and before the config write resumes on the next call,
    /// because a renamed folder leaves no older folder to move.
    pub(crate) fn migrate_layout(&mut self) -> Result<()> {
        let parent = self.dir()?.to_path_buf();
        let _config_lock = self.lock_current()?;
        let current = read_disk_state(&self.path)?;
        let Some(storage) = current
            .config
            .storage
            .clone()
            .filter(|storage| !layout::is_fixed(storage))
        else {
            self.adopt(current);
            return Ok(());
        };
        // An invalid, unconfirmed, or unavailable folder already has its own
        // issue and repair; never rename inside a folder that is not the one
        // the user chose.
        let Some(identity) = current
            .root_identity
            .clone()
            .filter(|_| !current.needs_backup)
        else {
            self.adopt(current);
            return Ok(());
        };
        let Ok((root, _)) = open_selected_root(Path::new(&storage.root), Some(&identity)) else {
            self.adopt(current);
            return Ok(());
        };
        let _capture_lock = crate::capture::lock_capture(&parent)?;
        let moves = match layout::plan(&root, &storage) {
            Ok(moves) => moves,
            // read_disk_state reported the same refusal as the issue.
            Err(_) => {
                self.adopt(current);
                return Ok(());
            }
        };
        let result = move_folders(&parent, &root, &identity, &moves).and_then(|()| {
            let mut next = current.config.clone();
            next.storage = Some(layout::fixed(&storage));
            let serialized = toml::to_string_pretty(&StoredConfig::from_snapshot(
                &next,
                current.root_identity.clone(),
                current.knowledge_base_identity.clone(),
                &current.agent,
                current.retired_adapter_contracts.clone(),
                current.adapter_extensions.clone(),
            ))
            .context("serializing config TOML")?;
            // A local editor might ignore our lock. Leave its bytes alone;
            // the next call finds the folders already renamed.
            if read_disk_state(&self.path)?.revision != current.revision {
                bail!("config.toml changed while folders were being renamed");
            }
            publish_config(&parent, &self.path, &serialized).map(|_| ())
        });
        let refusal = result
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<layout::Refusal>())
            .cloned();
        self.adopt(read_disk_state(&self.path)?);
        if let Some(refusal) = refusal {
            self.issue = Some(refusal.0);
            return Ok(());
        }
        result
    }

    fn load_from(path: PathBuf) -> Self {
        // The folder may have moved since setup. Keep the config available for
        // repair through update_config, but never use it for vault I/O until a
        // fresh validation succeeds.
        match read_disk_state(&path) {
            Ok(state) => {
                let mut store = Self::for_path(path);
                store.adopt(state);
                store
            }
            // Exiting here would show the extension only a disconnect. Start
            // inert instead; each request refreshes, fails the same way, and
            // answers invalid_config until the file is readable again.
            Err(error) => {
                eprintln!("{NAMESPACE}: warning: configuration is unreadable: {error:#}");
                let mut store = Self::for_path(path);
                store.become_inert();
                store
            }
        }
    }

    /// Forget the last readable config. `hello` and `get_config` report this
    /// state with its issue, so the extension can tell the user to repair the
    /// file; every other request refuses with invalid_config.
    fn become_inert(&mut self) {
        self.config = empty_config();
        self.revision = UNREADABLE_REVISION.into();
        self.issue = Some("The configuration file cannot be read or is not a regular file. Fix or remove it, then reload this page.".into());
        self.root_identity = None;
        self.knowledge_base_identity = None;
        self.agent = AgentView::default();
        self.adapter_extensions.clear();
        self.adapter_issue = None;
        self.adapter_warnings = Some(Vec::new());
        self.adapter_document = None;
        self.retired_adapter_contracts.clear();
        self.note_key_rules = NoteKeyRules::default();
        self.recordable = None;
    }

    /// Best-effort startup cleanup of temporaries a killed host left behind,
    /// in the config directory and, once the folder is trusted, the page
    /// notes directory. Failures are logged; they never block serving.
    pub fn sweep_stale_temporaries(&self) {
        if let Some(parent) = self.path.parent()
            && let Err(error) = sweep_config_temporaries(parent)
        {
            eprintln!("{NAMESPACE}: warning: could not sweep config temporaries: {error:#}");
        }
        if !self.configured() {
            return;
        }
        let Some(storage) = self.config.storage.as_ref() else {
            return;
        };
        if let Err(error) = Vault::open_checked(storage, self.root_identity())
            .and_then(|vault| vault.sweep_stale_temporaries())
        {
            eprintln!("{NAMESPACE}: warning: could not sweep page-note temporaries: {error:#}");
        }
        // A host killed during an artifact pass leaves its temporaries.
        if let Err(error) = self.cache_dir().and_then(|cache_dir| {
            ArtifactIndex::open_in(storage, self.root_identity(), &cache_dir)?
                .sweep_stale_temporaries()
        }) {
            eprintln!("{NAMESPACE}: warning: could not sweep artifact temporaries: {error:#}");
        }
    }

    fn for_path(path: PathBuf) -> Self {
        Self {
            path,
            config: empty_config(),
            revision: MISSING_REVISION.into(),
            issue: None,
            root_identity: None,
            knowledge_base_identity: None,
            agent: AgentView::default(),
            adapter_extensions: BTreeMap::new(),
            adapter_issue: None,
            adapter_warnings: Some(Vec::new()),
            adapter_document: Some(AdapterTemplates::default()),
            retired_adapter_contracts: Vec::new(),
            note_key_rules: NoteKeyRules::default(),
            lock_held: Arc::new(AtomicBool::new(false)),
            // Never equal to a read state, so the first adopt compiles rules.
            adopted: AdoptedContracts::Unread,
            recordable: None,
        }
    }

    fn adopt(&mut self, state: DiskState) {
        let revision_changed = self.revision != state.revision;
        self.config = state.config;
        self.revision = state.revision;
        self.issue = state.issue;
        self.root_identity = state.root_identity;
        self.knowledge_base_identity = state.knowledge_base_identity;
        self.agent = state.agent;
        self.adapter_extensions = state.adapter_extensions;
        self.retired_adapter_contracts = state.retired_adapter_contracts;
        if revision_changed {
            self.recordable = None;
            self.set_capture_adapter_policy(false);
        }
        // Another host or an editor can change the state file while
        // config.toml stays the same.
        if self.sync_adopted_contracts() || revision_changed {
            self.set_note_key_rules();
        }
    }

    /// Re-read the adopted-contract state and, under the config lock, add
    /// the current revision's contracts to it. Every note request refreshes
    /// under the lock before deriving a key, so a contract is recorded
    /// before it can key a note. Returns whether the state changed.
    fn sync_adopted_contracts(&mut self) -> bool {
        let Some(path) = self.path.parent().map(adopted_contracts_path) else {
            return false;
        };
        let adopted = if self.lock_held.load(Ordering::SeqCst) {
            let recordable = self
                .recordable
                .get_or_insert_with(|| individually_valid_adapter_contracts(&self.config));
            match record_adopted_contracts(&path, recordable) {
                Ok(adopted) => adopted,
                // The contracts stay current, so a later refresh retries.
                // Until then a hand removal of one leaves its pages on URL
                // keys (DESIGN.md §6.2).
                Err(error) => {
                    eprintln!(
                        "{NAMESPACE}: warning: could not record adopted adapter contracts: {error:#}"
                    );
                    read_adopted_contracts(&path)
                }
            }
        } else {
            read_adopted_contracts(&path)
        };
        let changed = adopted != self.adopted;
        self.adopted = adopted;
        changed
    }

    pub fn snapshot(&self) -> &ConfigSnapshot {
        &self.config
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// The directory holding config.toml, which also holds host-owned state
    /// such as the harness work directory.
    pub fn dir(&self) -> Result<&Path> {
        self.path.parent().context("config path has no parent")
    }

    /// Production uses the OS cache directory. Tests and embedders with an
    /// injected config path keep their disposable index beside that config,
    /// so parallel fixtures never share process-global cache state.
    pub fn cache_dir(&self) -> Result<PathBuf> {
        if self.path == Self::default_path()? {
            let dirs = ProjectDirs::from("", "", APP_NAME)
                .context("cannot locate this user's application cache directory")?;
            Ok(dirs.cache_dir().to_path_buf())
        } else {
            Ok(self.dir()?.join("cache"))
        }
    }

    pub fn root_identity(&self) -> Option<&str> {
        self.root_identity.as_deref()
    }

    /// The folder the summary agent reads: the chosen knowledge base folder,
    /// or the home folder while none is chosen. The host never writes here.
    pub fn knowledge_base_dir(&self) -> Option<PathBuf> {
        let storage = self.config.storage.as_ref()?;
        Some(PathBuf::from(
            storage
                .knowledge_base_dir
                .as_deref()
                .unwrap_or(&storage.root),
        ))
    }

    /// The picker-confirmed identity of `knowledge_base_dir()`.
    pub fn knowledge_base_identity(&self) -> Option<&str> {
        let storage = self.config.storage.as_ref()?;
        if storage.knowledge_base_dir.is_some() {
            self.knowledge_base_identity.as_deref()
        } else {
            self.root_identity()
        }
    }

    /// Hold this lock from the policy refresh through a privileged write.
    /// ConfigStore::update takes the same lock before replacing config.toml.
    pub fn lock_current(&self) -> Result<ConfigLock> {
        let parent = self.path.parent().context("config path has no parent")?;
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let path = parent.join(".config.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.lock().context("locking configuration")?;
        self.lock_held.store(true, Ordering::SeqCst);
        Ok(ConfigLock {
            held: Arc::clone(&self.lock_held),
            _file: file,
        })
    }

    /// A malformed or newer config is shown to setup, never used for capture.
    pub fn config_issue(&self) -> Option<&str> {
        self.issue.as_deref()
    }

    /// A malformed or newer adapter document disables automatic capture but
    /// remains available in the snapshot for repair. Notes and AI settings do
    /// not depend on adapter recognition and remain usable.
    pub fn adapter_issue(&self) -> Option<&str> {
        self.adapter_issue.as_deref()
    }

    /// The origins automatic capture may record, from the adapter document
    /// compiled for the current revision. Empty while `adapter_issue` is set.
    pub fn capture_origins(&self) -> Vec<String> {
        if self.adapter_issue.is_some() {
            return Vec::new();
        }
        self.adapter_document
            .as_ref()
            .map(document_origins)
            .unwrap_or_default()
    }

    pub fn adapter_warnings(&mut self) -> &[AdapterWarning] {
        if self.adapter_warnings.is_none() {
            self.adapter_warnings = Some(
                self.adapter_document
                    .as_ref()
                    .map(adapter_warnings)
                    .unwrap_or_default(),
            );
        }
        self.adapter_warnings.as_deref().unwrap_or_default()
    }

    /// Evaluate with the document compiled for the current on-disk revision.
    /// The worker's lease check is a privacy prefilter, never an authorization
    /// decision the privileged host trusts.
    pub(crate) fn adapter_matches(&self, url: &str, title: Option<&str>) -> Result<bool> {
        Ok(self.adapter_document()?.evaluate(url, title).is_ok())
    }

    pub(crate) fn adapter_match(
        &self,
        url: &str,
        title: Option<&str>,
    ) -> Result<adapter_templates::Match> {
        self.adapter_document()?
            .evaluate(url, title)
            .map_err(|reason| anyhow::anyhow!("URL is not an artifact: {reason}"))
    }

    /// Note identity for a URL the capture policy does not recognize. Record
    /// keys outlive capture (DESIGN.md §6.2): removing a site or adapter,
    /// narrowing its rules, or an adapter document this host cannot apply
    /// stops capture but must not split a note across a document's routes.
    /// The retired contracts in `[[adapter_id_history]]`, and the current
    /// document while it is inert, therefore still name artifacts for note
    /// keys only; they never authorize capture. Every recorded artifact URL
    /// was matched by one of these contracts or by the current policy, so an
    /// origin they cover is decided by them alone: a stripped canonical URL
    /// that happens to equal another page's is never identity evidence.
    ///
    /// Contracts that left config.toml by hand are read the same way from
    /// the host's adopted-contract state (`AdoptedContracts`).
    pub(crate) fn note_key_rule(&self, url: &str) -> NoteKeyRule {
        if self.note_key_rules.all_unavailable {
            return NoteKeyRule::Unavailable;
        }
        let mut covered = self.adapter_document.as_ref().is_some_and(|document| {
            !matches!(document.evaluate(url, None), Err(NoMatch::NoAdapter))
        });
        let mut ids = std::collections::BTreeSet::new();
        let origin = url::Url::parse(url)
            .ok()
            .map(|parsed| parsed.origin().ascii_serialization());
        if origin.is_some_and(|origin| self.note_key_rules.unavailable_origins.contains(&origin)) {
            return NoteKeyRule::Unavailable;
        }
        for document in &self.note_key_rules.documents {
            match document.evaluate(url, None) {
                Ok(found) => {
                    covered = true;
                    ids.insert(found.artifact_id);
                }
                Err(NoMatch::NoAdapter) => {}
                Err(_) => covered = true,
            }
        }
        match ids.len() {
            0 if covered => NoteKeyRule::Unrecognized,
            0 => NoteKeyRule::Uncovered,
            1 => NoteKeyRule::Artifact(ids.into_iter().next().expect("checked len == 1")),
            _ => NoteKeyRule::Ambiguous,
        }
    }

    pub fn adapter_document(&self) -> Result<&AdapterTemplates> {
        self.adapter_document
            .as_ref()
            .context("adapter policy is unavailable")
    }

    fn set_note_key_rules(&mut self) {
        self.note_key_rules = note_key_rules(
            &self.config,
            self.adapter_document.is_none(),
            &self.retired_adapter_contracts,
            &self.adopted,
        );
    }

    fn set_capture_adapter_policy(&mut self, include_warnings: bool) {
        if !self.adapter_extensions.is_empty() {
            let keys = self
                .adapter_extensions
                .keys()
                .take(8)
                .map(|key| format!("{key:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            let more = if self.adapter_extensions.len() > 8 {
                ", …"
            } else {
                ""
            };
            self.adapter_issue = Some(format!(
                "The adapter table contains unsupported keys ({keys}{more}); automatic capture is paused until the adapter document is repaired."
            ));
            self.adapter_warnings = Some(Vec::new());
            self.adapter_document = None;
            return;
        }
        match compile_validated_adapters(&self.config) {
            Ok(document) => {
                self.adapter_issue = None;
                self.adapter_warnings = include_warnings.then(|| adapter_warnings(&document));
                self.adapter_document = Some(document);
            }
            Err(error) => {
                self.adapter_issue = Some(error.to_string());
                self.adapter_warnings = Some(Vec::new());
                self.adapter_document = None;
            }
        }
    }

    /// Re-read config.toml. On failure the store becomes inert rather than
    /// keeping a policy the file no longer supports.
    pub fn refresh(&mut self) -> Result<()> {
        let state = match read_disk_state(&self.path) {
            Ok(state) => state,
            Err(error) => {
                self.become_inert();
                return Err(error);
            }
        };
        self.adopt(state);
        Ok(())
    }

    pub fn configured(&self) -> bool {
        self.issue.is_none()
            && self.config.storage.is_some()
            && validate_persisted(&self.config).is_ok()
    }

    /// The settings page's view of whether summaries may run: a committed
    /// summary harness and a summaries folder. The service checks the rest
    /// (the knowledge base folder, `agent.env`) when a run starts.
    pub fn agent_status(&self) -> AgentStatus {
        let harness_version = self
            .agent
            .summarizer
            .as_ref()
            .and_then(|summarizer| summarizer.version.clone());
        match self.agent_refusal() {
            None => AgentStatus {
                state: AgentState::Ready,
                harness_version,
                message: None,
            },
            Some(refusal) => AgentStatus {
                state: refusal.state,
                harness_version,
                message: Some(refusal.message),
            },
        }
    }

    fn agent_refusal(&self) -> Option<AgentRefusal> {
        let refuse = |state, message: &str| {
            Some(AgentRefusal {
                state,
                message: message.to_owned(),
            })
        };
        if self.issue.is_some() {
            return refuse(
                AgentState::NotSetUp,
                "configuration is unavailable or needs repair",
            );
        }
        if self.config.agent.is_none() {
            return match &self.agent.issue {
                Some(issue) => refuse(AgentState::HarnessProblem, issue),
                None => refuse(AgentState::NotSetUp, SET_UP_HARNESS),
            };
        }
        // The summaries folder is fixed (DESIGN.md §6.1): a layout that
        // lacks it is pending a move, which `issue` already refuses.
        None
    }

    /// Replace config only when the caller's revision still matches the file.
    /// `None` is a conflict; `Some(revision)` is a committed update.
    #[cfg(test)]
    pub(crate) fn update(
        &mut self,
        next: ConfigSnapshot,
        expected_revision: &str,
    ) -> Result<Option<String>> {
        self.update_with_grants(next, expected_revision, None, None, None)
    }

    #[cfg(test)]
    pub(crate) fn update_with_root_identity(
        &mut self,
        next: ConfigSnapshot,
        expected_revision: &str,
        selected_identity: Option<&str>,
    ) -> Result<Option<String>> {
        self.update_with_grants(next, expected_revision, selected_identity, None, None)
    }

    /// Native dispatch calls this only after `ConsentAuthority` has checked
    /// the folder grant; `harness_commit` comes only from the service's
    /// harness setup.
    /// The agent checks below repeat that decision against the file itself,
    /// under the lock, as defense in depth.
    pub(crate) fn update_with_grants(
        &mut self,
        next: ConfigSnapshot,
        expected_revision: &str,
        selected_identity: Option<&str>,
        knowledge_base_identity: Option<&str>,
        harness_commit: Option<HarnessCommit>,
    ) -> Result<Option<String>> {
        let json = serde_json::to_vec(&next).context("serializing config JSON")?;
        if json.len() > MAX_CONFIG_BYTES {
            bail!("configuration exceeds the 64 KiB size limit");
        }
        // Full adapter validation happens against the freshly read current
        // document under the config lock below. Base fields are never allowed
        // to use the invalid-adapter repair exception.
        validate_base(&next)?;
        if let Some(commit) = &harness_commit
            && next.agent.as_ref() != Some(&commit.summarizer.agent())
        {
            bail!("harness setup does not match this configuration");
        }
        let parent = self
            .path
            .parent()
            .context("config path has no parent")?
            .to_path_buf();
        let _lock_file = self.lock_current()?;
        let current = read_disk_state(&self.path)?;
        if expected_revision != current.revision {
            self.adopt(current);
            return Ok(None);
        }
        validate_update(&current.config, &next)?;
        // Another host may have recorded contracts since this one last read
        // the state file.
        let adopted = read_adopted_contracts(&adopted_contracts_path(&parent));
        validate_strip_params_readers(
            &current.config,
            &next,
            &recorded_readers(&current.retired_adapter_contracts, &adopted),
        )?;
        let adapters_unchanged = current.config.adapters == next.adapters;
        let empty_adapter_document = AdapterDocument {
            format: 1,
            artifacts: Vec::new(),
        };
        // A remove-all repair of an otherwise empty document is identical on
        // the wire because future `[adapters]` keys stay host-only. Treat an
        // exact no-op config update as that explicit narrowing; real unrelated
        // edits continue to preserve fields this host does not understand.
        let clear_adapter_extensions = !current.adapter_extensions.is_empty()
            && next.adapters == empty_adapter_document
            && (!adapters_unchanged || current.config == next);
        if !current.adapter_extensions.is_empty()
            && !adapters_unchanged
            && !clear_adapter_extensions
        {
            bail!(
                "unsupported adapter-table keys can only be removed by resetting the whole adapter document"
            );
        }
        if !adapters_unchanged {
            validate_adapter_transition_with_history(
                &current.config,
                &next,
                &current.retired_adapter_contracts,
            )?;
        }
        let retired_adapter_contracts = if adapters_unchanged {
            current.retired_adapter_contracts.clone()
        } else {
            updated_adapter_history(&current.retired_adapter_contracts, &current.config, &next)
        };
        let adapter_extensions = if clear_adapter_extensions {
            BTreeMap::new()
        } else {
            current.adapter_extensions.clone()
        };

        let agent = match &harness_commit {
            Some(commit) => AgentView::committed(commit)?,
            None => {
                // Only the service's harness setup sets a harness.
                match (&next.agent, &current.config.agent) {
                    (None, Some(_)) => AgentView::default(),
                    // Keep an entry this host cannot parse for the user to fix.
                    (None, None) => current.agent.clone(),
                    (Some(next_agent), Some(old)) if next_agent == old => current.agent.clone(),
                    _ => bail!("the summary harness can only be set by harness setup"),
                }
            }
        };

        let next_identity = match next.storage.as_ref() {
            None => None,
            Some(storage) => {
                let unchanged = current
                    .config
                    .storage
                    .as_ref()
                    .is_some_and(|old| old.root == storage.root);
                let expected = selected_identity
                    .or_else(|| {
                        unchanged
                            .then_some(current.root_identity.as_deref())
                            .flatten()
                    })
                    .context("a fresh native folder selection is required")?;
                let actual = selected_root_identity(Path::new(&storage.root))?;
                if actual != expected {
                    bail!("selected home folder changed; choose it again");
                }
                Some(actual)
            }
        };
        let next_knowledge_base_identity = match next
            .storage
            .as_ref()
            .and_then(|storage| storage.knowledge_base_dir.as_deref())
        {
            None => None,
            Some(knowledge_base) => {
                let unchanged = current
                    .config
                    .storage
                    .as_ref()
                    .and_then(|old| old.knowledge_base_dir.as_deref())
                    == Some(knowledge_base);
                match knowledge_base_identity {
                    // A fresh picker selection: the folder must still be the
                    // one the user chose.
                    Some(expected) => {
                        let actual = selected_root_identity(Path::new(knowledge_base))?;
                        if actual != expected {
                            bail!("selected knowledge base folder changed; choose it again");
                        }
                        Some(actual)
                    }
                    // Unchanged: carry the recorded identity without touching
                    // the filesystem. Only the summary agent reads this
                    // folder, and it re-checks the identity before use, so a
                    // missing or replaced folder must not block unrelated
                    // saves such as pausing capture.
                    None => Some(
                        unchanged
                            .then(|| current.knowledge_base_identity.clone())
                            .flatten()
                            .context(
                                "a fresh native selection of the knowledge base folder is required",
                            )?,
                    ),
                }
            }
        };
        let serialized = toml::to_string_pretty(&StoredConfig::from_snapshot(
            &next,
            next_identity.clone(),
            next_knowledge_base_identity.clone(),
            &agent,
            retired_adapter_contracts.clone(),
            adapter_extensions.clone(),
        ))
        .context("serializing config TOML")?;
        if serialized.len() > MAX_CONFIG_BYTES {
            bail!("configuration exceeds the 64 KiB size limit");
        }

        // Saving the fixed layout for the same home folder (for example
        // after reconnecting it) first moves its older folders, exactly as
        // `migrate_layout` does. A different home folder starts fresh and
        // leaves the old one untouched.
        let _capture_lock = match (current.config.storage.as_ref(), next.storage.as_ref()) {
            (Some(old), Some(new)) if old.root == new.root && !layout::is_fixed(old) => {
                let identity = next_identity
                    .as_deref()
                    .context("a fresh native folder selection is required")?;
                let capture_lock = crate::capture::lock_capture(&parent)?;
                let (root, _) = open_selected_root(Path::new(&new.root), Some(identity))?;
                let moves = layout::plan(&root, old)?;
                move_folders(&parent, &root, identity, &moves)?;
                Some(capture_lock)
            }
            _ => None,
        };

        // Repair must preserve the exact unreadable file. A no-clobber,
        // durable backup is created before the replacement can be published.
        if current.needs_backup {
            backup_invalid(&parent, &self.path, &current.revision)?;
            // A local editor might ignore our lock. Do not replace bytes that
            // changed while the backup was being made.
            let after_backup = read_disk_state(&self.path)?;
            if after_backup.revision != current.revision {
                self.adopt(after_backup);
                return Ok(None);
            }
        }

        let revision = publish_config(&parent, &self.path, &serialized)?;
        self.config = next;
        self.revision = revision.clone();
        self.issue = None;
        self.root_identity = next_identity;
        self.knowledge_base_identity = next_knowledge_base_identity;
        self.agent = agent;
        self.adapter_extensions = adapter_extensions;
        self.retired_adapter_contracts = retired_adapter_contracts;
        self.recordable = None;
        self.set_capture_adapter_policy(true);
        // Still under the lock: record what this write adopted.
        self.sync_adopted_contracts();
        self.set_note_key_rules();
        Ok(Some(revision))
    }

    /// A second store for the same file, loaded now. The service gives
    /// harness setup its own, so a long probe never holds the shared one.
    #[cfg(unix)]
    pub(crate) fn reopen(&self) -> Self {
        Self::load_from(self.path.clone())
    }

    #[cfg(test)]
    pub(crate) fn for_test(path: PathBuf) -> Self {
        Self::for_path(path)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredConfig {
    #[serde(default)]
    storage: Option<StoredStorage>,
    #[serde(default)]
    root_picker_confirmed: bool,
    #[serde(default)]
    root_identity: Option<String>,
    /// Host-only: the identity of the picker-selected knowledge base folder.
    /// Absent while the knowledge base follows `root`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    knowledge_base_identity: Option<String>,
    #[serde(default)]
    capture_enabled: bool,
    // Accept and ignore the retired v5 site allowlist. Capture origins now
    // come from the adapters; new config writes omit it.
    #[serde(default, skip_serializing, rename = "sites")]
    _sites: Option<toml::Value>,
    #[serde(default = "default_strip_params")]
    strip_params: Vec<String>,
    // Absent before protocol v6: the default lists.
    #[serde(default = "default_summary_vocabulary")]
    summary_vocabulary: SummaryVocabulary,
    // Accept and ignore the retired M1 key. New config writes omit it.
    #[serde(default, skip_serializing, rename = "near_repeat_secs")]
    _near_repeat_secs: Option<u32>,
    // Accept and ignore the retired v5 privacy keys. New config writes omit them.
    #[serde(default, skip_serializing, rename = "agent_denylist")]
    _agent_denylist: Option<toml::Value>,
    #[serde(default, skip_serializing, rename = "agent_denylist_confirmed")]
    _agent_denylist_confirmed: Option<toml::Value>,
    #[serde(default, skip_serializing, rename = "log_incognito")]
    _log_incognito: Option<toml::Value>,
    /// The summary harness the service set up, parsed apart; see `AgentView`.
    #[serde(default)]
    summarizer: Option<toml::Value>,
    // Accept and ignore the retired v5 harness entry and its checked record:
    // a v5 file loads as not set up. New config writes omit them.
    #[serde(default, skip_serializing, rename = "agent")]
    _agent: Option<toml::Value>,
    #[serde(default, skip_serializing, rename = "harness_record")]
    _harness_record: Option<toml::Value>,
    #[serde(default)]
    adapters: StoredAdapters,
    #[serde(default)]
    artifacts: Vec<serde_json::Value>,
    /// Host-owned ID contracts for removed adapters. These stay off the wire:
    /// an extension may re-enable the same contract, but cannot erase the
    /// evidence needed to detect a remove/edit/re-add ID migration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    adapter_id_history: Vec<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
struct StoredAdapters {
    format: u32,
    #[serde(default, flatten)]
    extensions: BTreeMap<String, toml::Value>,
}

impl Default for StoredAdapters {
    fn default() -> Self {
        Self {
            format: 1,
            extensions: BTreeMap::new(),
        }
    }
}

/// The on-disk storage table. Unlike the wire type, fields introduced after
/// M1 may be omitted, so every older config.toml still loads.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredStorage {
    root: String,
    profile: String,
    log_dir: String,
    pages_dir: String,
    later_dir: String,
    #[serde(default)]
    artifacts_dir: Option<String>,
    #[serde(default)]
    summaries_dir: Option<String>,
    // Added in v6. A missing key means the knowledge base follows `root`.
    #[serde(default)]
    knowledge_base_dir: Option<String>,
}

impl From<StorageConfig> for StoredStorage {
    fn from(value: StorageConfig) -> Self {
        Self {
            root: value.root,
            profile: value.profile,
            log_dir: value.log_dir,
            pages_dir: value.pages_dir,
            later_dir: value.later_dir,
            artifacts_dir: Some(value.artifacts_dir),
            summaries_dir: value.summaries_dir,
            knowledge_base_dir: value.knowledge_base_dir,
        }
    }
}

impl From<StoredStorage> for StorageConfig {
    fn from(value: StoredStorage) -> Self {
        let mut occupied = vec![
            Path::new(&value.log_dir),
            Path::new(&value.pages_dir),
            Path::new(&value.later_dir),
        ];
        if let Some(summaries_dir) = &value.summaries_dir {
            occupied.push(Path::new(summaries_dir));
        }
        let artifacts_dir = value
            .artifacts_dir
            .unwrap_or_else(|| unused_artifacts_dir(&occupied));
        Self {
            root: value.root,
            profile: value.profile,
            log_dir: value.log_dir,
            pages_dir: value.pages_dir,
            later_dir: value.later_dir,
            artifacts_dir,
            summaries_dir: value.summaries_dir,
            knowledge_base_dir: value.knowledge_base_dir,
        }
    }
}

/// Older configs did not name an artifact directory. Preserve their existing
/// layout even when one of its configurable locations already uses the usual
/// `artifacts` name; adopting a colliding default would make the whole config
/// unreadable before the user had any way to choose a different location.
fn unused_artifacts_dir(occupied: &[&Path]) -> String {
    // An empty location has no components and would block every name;
    // validation rejects it later. Every other location blocks at most the
    // one single-component name matching its first component, so one of
    // the first `occupied.len() + 1` candidates is always free.
    let occupied: Vec<Vec<String>> = occupied
        .iter()
        .map(|path| lower_components(path))
        .filter(|parts| !parts.is_empty())
        .collect();
    for suffix in 0..=occupied.len() {
        let candidate = match suffix {
            0 => "artifacts".to_owned(),
            1 => "brauser-artifacts".to_owned(),
            value => format!("brauser-artifacts-{value}"),
        };
        let candidate_parts = lower_components(Path::new(&candidate));
        if occupied.iter().all(|parts| {
            !parts.starts_with(&candidate_parts) && !candidate_parts.starts_with(parts)
        }) {
            return candidate;
        }
    }
    unreachable!("a finite set of content paths cannot occupy every generated name")
}

impl StoredConfig {
    fn from_snapshot(
        value: &ConfigSnapshot,
        root_identity: Option<String>,
        knowledge_base_identity: Option<String>,
        agent: &AgentView,
        retired_adapter_contracts: Vec<serde_json::Value>,
        adapter_extensions: BTreeMap<String, toml::Value>,
    ) -> Self {
        Self {
            storage: value.storage.clone().map(StoredStorage::from),
            // Only native dispatch calls ConfigStore::update, after verifying
            // the picker grant for a new root. This marks the root's M1 origin.
            root_picker_confirmed: value.storage.is_some(),
            root_identity,
            knowledge_base_identity,
            capture_enabled: value.capture_enabled,
            _sites: None,
            strip_params: value.strip_params.clone(),
            summary_vocabulary: value.summary_vocabulary.clone(),
            _near_repeat_secs: None,
            _agent_denylist: None,
            _agent_denylist_confirmed: None,
            _log_incognito: None,
            // The harness entry comes from the host's own view, never from
            // the extension's snapshot.
            summarizer: agent.table.clone(),
            _agent: None,
            _harness_record: None,
            adapters: StoredAdapters {
                format: value.adapters.format,
                extensions: adapter_extensions,
            },
            artifacts: value.adapters.artifacts.clone(),
            adapter_id_history: retired_adapter_contracts,
        }
    }
}

impl From<StoredConfig> for ConfigSnapshot {
    fn from(value: StoredConfig) -> Self {
        Self {
            storage: value.storage.map(StorageConfig::from),
            capture_enabled: value.capture_enabled,
            strip_params: value.strip_params,
            agent: None,
            summary_vocabulary: value.summary_vocabulary,
            adapters: AdapterDocument {
                format: value.adapters.format,
                artifacts: value.artifacts,
            },
        }
    }
}

fn read_disk_state(path: &Path) -> Result<DiskState> {
    // Check before opening: opening a FIFO for reading would block while the
    // config lock is held.
    match fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            bail!("{} is not a regular file", path.display())
        }
        _ => {}
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DiskState {
                config: empty_config(),
                revision: MISSING_REVISION.into(),
                issue: None,
                needs_backup: false,
                root_identity: None,
                knowledge_base_identity: None,
                agent: AgentView::default(),
                adapter_extensions: BTreeMap::new(),
                retired_adapter_contracts: Vec::new(),
            });
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a regular file", path.display());
    }
    let mut file = file.take(MAX_HASHED_CONFIG_BYTES);
    let mut contents = Vec::new();
    let mut hasher = RevisionHasher::new();
    let mut bytes_read = 0u64;
    let mut chunk = [0u8; 8192];
    loop {
        let count = file
            .read(&mut chunk)
            .with_context(|| format!("reading {}", path.display()))?;
        if count == 0 {
            break;
        }
        bytes_read += count as u64;
        hasher.update(&chunk[..count]);
        let keep = (MAX_CONFIG_BYTES + 1)
            .saturating_sub(contents.len())
            .min(count);
        contents.extend_from_slice(&chunk[..keep]);
    }
    let revision = hasher.finish(metadata.len());
    if bytes_read > MAX_CONFIG_BYTES as u64 {
        return Ok(DiskState {
            config: empty_config(),
            revision,
            issue: Some("Configuration exceeds 64 KiB. Choose a home folder and save a replacement; the original will be backed up.".into()),
            needs_backup: true,
            root_identity: None,
            knowledge_base_identity: None,
            agent: AgentView::default(),
            adapter_extensions: BTreeMap::new(),
            retired_adapter_contracts: Vec::new(),
        });
    }
    let parsed = std::str::from_utf8(&contents)
        .map_err(anyhow::Error::from)
        .and_then(|text| toml::from_str::<StoredConfig>(text).map_err(anyhow::Error::from));
    match parsed {
        Ok(mut stored) => {
            let root_identity = stored.root_identity.clone();
            let knowledge_base_identity = stored.knowledge_base_identity.clone();
            let retired_adapter_contracts = std::mem::take(&mut stored.adapter_id_history);
            let adapter_extensions = std::mem::take(&mut stored.adapters.extensions);
            let root_unconfirmed = stored.storage.is_some()
                && (!stored.root_picker_confirmed || root_identity.is_none());
            let agent = AgentView::parse(stored.summarizer.take());
            let mut config: ConfigSnapshot = stored.into();
            config.agent = agent.agent();
            if root_unconfirmed || validate_base(&config).is_err() {
                let retired_adapter_contracts = repairable_history(retired_adapter_contracts);
                let issue = if root_unconfirmed {
                    "This home folder predates native picker confirmation. Choose it again to activate capture; the original config will be backed up."
                } else {
                    "Configuration settings are invalid. Choose a home folder and save a repair; the original config will be backed up."
                };
                return Ok(DiskState {
                    config: empty_config(),
                    revision,
                    issue: Some(issue.to_owned()),
                    needs_backup: true,
                    root_identity: None,
                    knowledge_base_identity: None,
                    agent,
                    adapter_extensions: BTreeMap::new(),
                    retired_adapter_contracts,
                });
            }
            // The settings themselves are valid. A folder that is missing or
            // no longer matches its stored identity (an unmounted drive, or a
            // remount with a new device number) blocks vault I/O and needs
            // reselection, but it must not discard the allowlist: keep every
            // setting for the repair, which rewrites the file with a fresh
            // identity, so there is nothing to back up.
            let folder_changed = config.storage.as_ref().is_some_and(|storage| {
                match selected_root_identity(Path::new(&storage.root)) {
                    Ok(actual) => Some(actual.as_str()) != root_identity.as_deref(),
                    Err(_) => true,
                }
            });
            let issue = if folder_changed {
                Some("The selected home folder is unavailable or changed. Reconnect it, or choose it again to repair this configuration; your other settings are kept.".to_owned())
            } else {
                config.storage.as_ref().and_then(layout_issue)
            };
            Ok(DiskState {
                config,
                revision,
                issue,
                needs_backup: false,
                root_identity,
                knowledge_base_identity,
                agent,
                adapter_extensions,
                retired_adapter_contracts,
            })
        }
        Err(_) => Ok(DiskState {
            config: empty_config(),
            revision,
            issue: Some("Configuration is invalid or from a newer version. Choose a home folder and save a replacement; the original will be backed up.".into()),
            needs_backup: true,
            root_identity: None,
            knowledge_base_identity: None,
            agent: AgentView::default(),
            adapter_extensions: BTreeMap::new(),
            // A newer or partly invalid file may still be valid TOML.
            retired_adapter_contracts: std::str::from_utf8(&contents)
                .ok()
                .and_then(|text| toml::from_str::<toml::Table>(text).ok())
                .and_then(|mut table| table.remove("adapter_id_history"))
                .and_then(|history| history.try_into::<Vec<serde_json::Value>>().ok())
                .map(repairable_history)
                .unwrap_or_default(),
        }),
    }
}

/// The `[[adapter_id_history]]` a repair carries forward: every entry this
/// host can validate on its own. A repair replaces the whole file, so an
/// entry it cannot validate would block any later adapter edit; the backup
/// keeps the original. A file over the size limit is not parsed at all, so
/// its history is not recovered.
fn repairable_history(history: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let mut kept: Vec<serde_json::Value> = Vec::new();
    for contract in history {
        if validate_adapter_history(std::slice::from_ref(&contract)).is_ok()
            && !kept.contains(&contract)
        {
            kept.push(contract);
        }
    }
    kept
}

/// Atomically replace config.toml with `serialized` and return its revision.
/// Callers hold the config lock and have checked the current revision.
fn publish_config(parent: &Path, path: &Path, serialized: &str) -> Result<String> {
    let temporary = parent.join(format!(".config-{}.tmp", Uuid::new_v4()));
    let (mut file, _cleanup) = create_temporary(&temporary)?;
    let write_result = (|| -> Result<()> {
        file.write_all(serialized.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    // Close the handle before the cleanup guard runs on error. Windows may
    // refuse to remove a still-open temporary file.
    drop(file);
    write_result?;
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    if let Err(error) = sync_parent(parent) {
        eprintln!("{NAMESPACE}: warning: config was saved but directory sync failed: {error:#}");
    }
    Ok(revision_for(serialized.as_bytes()))
}

/// Rename the planned folders. Before the log folder moves, record where
/// its logs went, so visit and batch intents that name the old path still
/// find their logs. Callers hold the config and capture locks.
fn move_folders(
    state_dir: &Path,
    root: &cap_std::fs::Dir,
    root_identity: &str,
    moves: &[layout::Move],
) -> Result<()> {
    if let Some(logs) = moves.iter().find(|step| step.to == layout::LOGS_DIR) {
        crate::capture::record_log_relocation(state_dir, root_identity, &logs.from, logs.to)?;
    }
    layout::apply(root, moves)
}

/// An older folder layout is never used for vault I/O. It is moved by
/// `migrate_layout`; until then this says why it has not been.
fn layout_issue(storage: &StorageConfig) -> Option<String> {
    if layout::is_fixed(storage) {
        return None;
    }
    let refusal = open_selected_root(Path::new(&storage.root), None)
        .map_err(|_| layout::Refusal(LAYOUT_PENDING.to_owned()))
        .and_then(|(root, _)| layout::plan(&root, storage));
    Some(match refusal {
        Ok(_) => LAYOUT_PENDING.to_owned(),
        Err(refusal) => refusal.0,
    })
}

fn backup_invalid(parent: &Path, source_path: &Path, expected_revision: &str) -> Result<()> {
    let path = parent.join(format!("config-invalid-{}.toml", Uuid::new_v4()));
    let (mut file, mut cleanup) = create_temporary(&path)?;
    let result = (|| -> Result<()> {
        let mut source = File::open(source_path)
            .with_context(|| format!("opening invalid config at {}", source_path.display()))?;
        // Copy every byte, but compute the revision exactly as
        // read_disk_state does, or a file past the hash cap never matches.
        let mut hasher = RevisionHasher::new();
        let mut copied = 0u64;
        let mut chunk = [0u8; 8192];
        loop {
            let count = source.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            hasher.update(&chunk[..count]);
            copied += count as u64;
            file.write_all(&chunk[..count])?;
        }
        if hasher.finish(copied) != expected_revision {
            bail!("configuration changed while it was being backed up");
        }
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    result.with_context(|| format!("backing up invalid config to {}", path.display()))?;
    sync_parent(parent).context("syncing config backup directory")?;
    cleanup.disarm();
    Ok(())
}

/// Remove `.config-<uuid>.tmp` files old enough that no live update can still
/// own them. Symlinks and anything not exactly that name are left alone.
fn sweep_config_temporaries(parent: &Path) -> Result<usize> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| format!("listing {}", parent.display()));
        }
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", parent.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_generated_temporary(name, ".config-") {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let stale = metadata.is_file()
            && metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|elapsed| elapsed >= STALE_TEMPORARY_AGE);
        if stale && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

fn revision_for(contents: &[u8]) -> String {
    let mut hasher = RevisionHasher::new();
    hasher.update(contents);
    hasher.finish(contents.len() as u64)
}

/// The one definition of a config revision: the SHA-256 of at most
/// `MAX_HASHED_CONFIG_BYTES`, plus the file length when that cap is reached.
struct RevisionHasher {
    hasher: Sha256,
    hashed: u64,
}

impl RevisionHasher {
    fn new() -> Self {
        Self {
            hasher: Sha256::new(),
            hashed: 0,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        let room = MAX_HASHED_CONFIG_BYTES - self.hashed;
        let take = (bytes.len() as u64).min(room) as usize;
        self.hasher.update(&bytes[..take]);
        self.hashed += take as u64;
    }

    fn finish(mut self, total_len: u64) -> String {
        if self.hashed == MAX_HASHED_CONFIG_BYTES {
            self.hasher.update(total_len.to_le_bytes());
        }
        format!("sha256:{}", hex::encode(self.hasher.finalize()))
    }
}

fn create_temporary(path: &Path) -> Result<(File, TempFileCleanup<'_>)> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    // The guard is installed only after create_new succeeds. An existing file
    // with this name must never be removed on a collision or open failure.
    Ok((file, TempFileCleanup { path, armed: true }))
}

pub(crate) fn empty_config() -> ConfigSnapshot {
    ConfigSnapshot {
        storage: None,
        capture_enabled: false,
        strip_params: default_strip_params(),
        agent: None,
        summary_vocabulary: default_summary_vocabulary(),
        adapters: AdapterDocument {
            format: 1,
            artifacts: Vec::new(),
        },
    }
}

/// Check a proposed config's shape and limits. This never touches the
/// filesystem: an extension-supplied root is untrusted until a picker grant or
/// the persisted identity authorizes it, so probing it here would answer
/// whether arbitrary paths exist (and, on Windows, reach out to UNC shares).
/// Callers open the root only after authorization; see `validate_persisted`.
pub fn validate(config: &ConfigSnapshot) -> Result<()> {
    validate_base(config)?;
    compile_validated_adapters(config)?;
    Ok(())
}

/// Validate an extension-authored update without making an already-inert
/// adapter document block unrelated settings. The exception is deliberately
/// narrow: only the exact adapter bytes from an already-invalid policy may
/// remain invalid. Any adapter edit, or a base-policy change that would turn a
/// previously valid adapter document invalid, still has to pass the full
/// authoritative adapter validation.
pub(crate) fn validate_update(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<()> {
    validate_base(next)?;
    if let Some(storage) = &next.storage {
        layout::require_fixed(storage)?;
    }
    let preserving_inert_adapters = current.adapters == next.adapters
        && compile_validated_adapters(current).is_err()
        && compile_validated_adapters(next).is_err();
    if !preserving_inert_adapters {
        compile_validated_adapters(next)?;
    }
    Ok(())
}

pub(crate) fn validate_base(config: &ConfigSnapshot) -> Result<()> {
    if config.strip_params.len() > MAX_STRIP_PARAMS {
        bail!("tracking-parameter list exceeds the 64-entry limit");
    }
    for value in &config.strip_params {
        if value.is_empty()
            || value == "*"
            || value.len() > MAX_RULE_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'*'))
            || (value.contains('*') && !value.ends_with('*'))
            || value.matches('*').count() > 1
        {
            bail!("tracking parameters must be simple names or prefix patterns ending in *");
        }
    }
    validate_vocabulary("document type", &config.summary_vocabulary.document_types)?;
    validate_vocabulary("category", &config.summary_vocabulary.categories)?;
    if config.capture_enabled && config.storage.is_none() {
        bail!("capture requires a selected home folder");
    }
    if let Some(agent) = &config.agent {
        validate_summarizer(agent)?;
    }
    if let Some(storage) = &config.storage {
        validate_storage(storage)?;
    }
    Ok(())
}

/// A save refused because a `strip_params` entry would remove a query
/// parameter an adapter rule reads. Native dispatch reports it as
/// `invalid_config` with this message.
#[derive(Debug)]
pub(crate) struct StripParamsRefusal(pub String);

impl fmt::Display for StripParamsRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StripParamsRefusal {}

/// A `strip_params` entry that removes a query parameter an adapter rule
/// reads: a key in a rule's `query` table or `absent` list
/// (adapter-templates.md §5.3). Path, title, alias, and ref templates never
/// read the query. Stripped canonical URLs of the pages that rule tells
/// apart are equal, so recorded URLs and note candidates stop naming one
/// artifact (DESIGN.md §6.2).
///
/// A conflict is one reading rule, so a save that keeps an existing one is
/// not refused, but a new or changed rule reading the same parameter is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct StripParamConflict {
    pub entry: String,
    pub param: String,
    pub origin: String,
    pub artifact_type: String,
    /// The adapter's `id` template and the reading rule, as JSON.
    pub reader: String,
}

/// The query keys an adapter's rules read, each with its rule. Read from
/// the raw document, so an inert or individually invalid adapter still
/// counts.
fn query_keys_read(adapter: &serde_json::Value) -> Vec<(&serde_json::Value, &str)> {
    let mut keys = Vec::new();
    for rule in adapter
        .get("match")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(query) = rule.get("query").and_then(serde_json::Value::as_object) {
            keys.extend(query.keys().map(|key| (rule, key.as_str())));
        }
        keys.extend(
            rule.get("absent")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(|key| (rule, key)),
        );
    }
    keys
}

/// Every conflict between `strip_params` and `readers`, keyed with whether
/// each reader is current (`false`) or recorded only (`true`). A reader
/// that is both counts as current.
fn conflicts_with<'a>(
    strip_params: &[String],
    readers: impl IntoIterator<Item = (&'a serde_json::Value, bool)>,
) -> BTreeMap<StripParamConflict, bool> {
    let mut found = BTreeMap::new();
    for (adapter, recorded) in readers {
        let origin = adapter.get("origin").and_then(serde_json::Value::as_str);
        let artifact_type = adapter.get("type").and_then(serde_json::Value::as_str);
        let id = adapter.get("id").cloned().unwrap_or_default();
        for (rule, param) in query_keys_read(adapter) {
            for entry in strip_params {
                if crate::capture::matches_param(param, entry) {
                    let conflict = StripParamConflict {
                        entry: entry.clone(),
                        param: param.to_owned(),
                        origin: origin.unwrap_or("an unknown origin").to_owned(),
                        artifact_type: artifact_type.unwrap_or("unnamed").to_owned(),
                        reader: serde_json::json!([id, rule]).to_string(),
                    };
                    let slot = found.entry(conflict).or_insert(recorded);
                    *slot &= recorded;
                }
            }
        }
    }
    found
}

/// The conflicts between `strip_params` and the current adapter document,
/// as settings shows them (it sees no recorded contracts).
#[cfg(test)]
pub(crate) fn strip_param_conflicts(config: &ConfigSnapshot) -> Vec<StripParamConflict> {
    conflicts_with(
        &config.strip_params,
        config
            .adapters
            .artifacts
            .iter()
            .map(|adapter| (adapter, false)),
    )
    .into_keys()
    .collect()
}

/// Refuse a save that adds a `strip_params` conflict: a new or changed
/// entry that removes a parameter some adapter rule reads, or a new or
/// changed rule reading a parameter an entry removes. `recorded` holds the
/// contracts that still name note keys without being current
/// (`[[adapter_id_history]]` and hand-removed adopted contracts); they read
/// too. A conflict the current config already has is left alone, so an
/// existing config keeps loading and saving; settings offers to remove the
/// entry instead (brwr-f4s).
pub(crate) fn validate_strip_params_readers(
    current: &ConfigSnapshot,
    next: &ConfigSnapshot,
    recorded: &[serde_json::Value],
) -> Result<()> {
    if current.strip_params == next.strip_params && current.adapters == next.adapters {
        return Ok(());
    }
    let recorded = || recorded.iter().map(|contract| (contract, true));
    let before = conflicts_with(
        &current.strip_params,
        current
            .adapters
            .artifacts
            .iter()
            .map(|adapter| (adapter, false))
            .chain(recorded()),
    );
    // An adapter this save removes moves to [[adapter_id_history]], so it
    // still reads.
    let after = conflicts_with(
        &next.strip_params,
        next.adapters
            .artifacts
            .iter()
            .map(|adapter| (adapter, false))
            .chain(
                current
                    .adapters
                    .artifacts
                    .iter()
                    .map(|adapter| (adapter, true)),
            )
            .chain(recorded()),
    );
    let Some((conflict, recorded)) = after
        .into_iter()
        .find(|(conflict, _)| !before.contains_key(conflict))
    else {
        return Ok(());
    };
    let adapter = if recorded {
        "removed adapter"
    } else {
        "adapter"
    };
    Err(StripParamsRefusal(format!(
        "{adapter} {:?} on {} reads the query parameter {:?}, so strip_params cannot remove it with {:?}: pages the adapter tells apart would share one stripped address. Remove {:?} from strip_params or change the adapter.",
        conflict.artifact_type, conflict.origin, conflict.param, conflict.entry, conflict.entry,
    ))
    .into())
}

impl ConfigStore {
    /// `validate_strip_params_readers` against this store's last read.
    pub(crate) fn validate_strip_params(&self, next: &ConfigSnapshot) -> Result<()> {
        validate_strip_params_readers(
            &self.config,
            next,
            &recorded_readers(&self.retired_adapter_contracts, &self.adopted),
        )
    }
}

/// The recorded contracts `validate_strip_params_readers` counts. An
/// unreadable adopted-contract state adds none.
fn recorded_readers(
    retired: &[serde_json::Value],
    adopted: &AdoptedContracts,
) -> Vec<serde_json::Value> {
    let mut readers = retired.to_vec();
    if let AdoptedContracts::Read { contracts, .. } = adopted {
        readers.extend(contracts.iter().cloned());
    }
    readers
}

/// Whether both lists of `vocabulary` pass the checks a saved
/// configuration's `[summary_vocabulary]` must.
#[cfg(unix)] // Only the Unix-only service uses this.
pub(crate) fn valid_summary_vocabulary(vocabulary: &SummaryVocabulary) -> bool {
    validate_vocabulary("document type", &vocabulary.document_types).is_ok()
        && validate_vocabulary("category", &vocabulary.categories).is_ok()
}

/// One summary vocabulary list: 1-64 distinct entries (ignoring case) of
/// 1-64 characters, none of them control characters.
fn validate_vocabulary(name: &str, values: &[String]) -> Result<()> {
    if values.is_empty() || values.len() > MAX_VOCABULARY_ENTRIES {
        bail!("the summary {name} list must have 1 to 64 entries");
    }
    let mut seen = HashSet::new();
    for value in values {
        let chars = value.chars().count();
        if chars == 0 || chars > MAX_VOCABULARY_CHARS || value.chars().any(char::is_control) {
            bail!("each summary {name} must be 1 to 64 characters with no control characters");
        }
        if !seen.insert(value.to_lowercase()) {
            bail!("the summary {name} list repeats {value:?}");
        }
    }
    Ok(())
}

#[cfg(test)]
fn validate_adapters(config: &ConfigSnapshot) -> Result<Vec<AdapterWarning>> {
    let document = compile_validated_adapters(config)?;
    Ok(adapter_warnings(&document))
}

/// See `ConfigStore::note_key_rule`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoteKeyRule {
    /// Exactly one note-only contract names this artifact.
    Artifact(String),
    /// Several contracts name different artifacts; use the URL key.
    Ambiguous,
    /// A current or retired contract covers this origin and none names the
    /// URL, so it is not an artifact page.
    Unrecognized,
    /// No contract has ever covered this origin, as far as config.toml
    /// still records.
    Uncovered,
    /// A recorded contract for this origin cannot be compiled here, so the
    /// key cannot be established; loads and saves refuse.
    Unavailable,
}

/// The note-only identity rules (see `ConfigStore::note_key_rule`).
#[derive(Default)]
struct NoteKeyRules {
    documents: Vec<AdapterTemplates>,
    /// Origins of recorded contracts this host cannot compile. Such a
    /// contract may have named artifacts there, so identity on those origins
    /// is unavailable rather than decided by the contracts that remain.
    unavailable_origins: std::collections::BTreeSet<String>,
    /// The adopted-contract state is unreadable, or holds a damaged entry
    /// with no origin, so no unrecognized URL's key can be established.
    all_unavailable: bool,
}

/// Compile the note-only identity rules. Retired contracts are compiled
/// alone, as the history validator does, because their shapes may overlap
/// one another. An inert current document is used whole when it compiles
/// (only capture-specific checks failed), keeping first-match and scope
/// semantics, and otherwise per contract. A contract without a valid origin
/// never compiled on any host, so it cannot have produced a record key.
///
/// Adopted contracts config.toml no longer holds, as current or history,
/// were removed by hand and are added like history entries. Their file is
/// host-written, so a damaged entry is corruption: one with no readable
/// origin makes every unrecognized URL unavailable.
fn note_key_rules(
    config: &ConfigSnapshot,
    current_inert: bool,
    retired: &[serde_json::Value],
    adopted: &AdoptedContracts,
) -> NoteKeyRules {
    // History entries are stored in contract form; anything else is invalid.
    // Returns false for an invalid entry with no origin to attribute it to.
    fn add(
        rules: &mut NoteKeyRules,
        contract: &serde_json::Value,
        format: u32,
        exact: bool,
    ) -> bool {
        let compiled = (format == 1)
            .then(|| adapter_contract(contract))
            .flatten()
            .filter(|recognition| !exact || recognition == contract)
            .and_then(|contract| {
                compile_adapters(&ConfigSnapshot {
                    adapters: AdapterDocument {
                        format: 1,
                        artifacts: vec![contract],
                    },
                    ..empty_config()
                })
                .ok()
            });
        match compiled {
            Some(document) => rules.documents.push(document),
            None => {
                let Some(origin) = contract
                    .get("origin")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|origin| url::Url::parse(origin).ok())
                    // Only a page origin can localize the damage; any other
                    // scheme serializes to an origin no page has.
                    .filter(|origin| {
                        matches!(origin.scheme(), "http" | "https") && origin.host().is_some()
                    })
                    .map(|origin| origin.origin().ascii_serialization())
                else {
                    return false;
                };
                rules.unavailable_origins.insert(origin);
            }
        }
        true
    }
    let mut rules = NoteKeyRules::default();
    if current_inert {
        match compile_adapters(config) {
            Ok(document) => rules.documents.push(document),
            Err(_) => {
                for contract in &config.adapters.artifacts {
                    add(&mut rules, contract, config.adapters.format, false);
                }
            }
        }
    }
    for contract in retired {
        add(&mut rules, contract, 1, true);
    }
    match adopted {
        AdoptedContracts::Read { contracts, .. } => {
            let current = adapter_contracts(config);
            for contract in contracts {
                if !current.contains(contract)
                    && !retired.contains(contract)
                    && !add(&mut rules, contract, 1, true)
                {
                    rules.all_unavailable = true;
                }
            }
        }
        AdoptedContracts::Unreadable => rules.all_unavailable = true,
        AdoptedContracts::Unread => {}
    }
    rules
}

/// Every adapter contract, in the recognition form `[[adapter_id_history]]`
/// uses, that a host adopted from config.toml under the config lock. Only a
/// settings save records history, so a contract deleted from config.toml in
/// an editor would otherwise leave its routes on URL keys and its artifact
/// note unreachable. A recorded contract config.toml no longer holds names
/// note keys like a history entry; it never authorizes capture and never
/// guards ID changes. Contracts are only ever added: re-adding one makes it
/// current again, and one later moved to history is read from there.
///
/// Several hosts (one per native-messaging connection) share the file. Each
/// rereads it under the config lock, adds what it lacks, and replaces it
/// atomically; readers without the lock see one whole version or the other.
/// A file this host cannot read is never rewritten: it may name the
/// contracts that decide an origin, so note identity on every URL the
/// current policy does not recognize is unavailable until it is repaired or
/// removed. Removing it forgets those contracts, like a config repair.
#[derive(Debug, Clone, PartialEq)]
enum AdoptedContracts {
    /// Not read yet.
    Unread,
    /// The file as read, or nothing recorded when it is absent.
    Read {
        revision: String,
        contracts: Vec<serde_json::Value>,
    },
    /// Unreadable, not a regular file, over a bound, malformed, or from a
    /// newer host.
    Unreadable,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAdoptedContracts {
    format: u32,
    contracts: Vec<serde_json::Value>,
}

fn read_adopted_contracts(path: &Path) -> AdoptedContracts {
    // As for config.toml, opening a FIFO would block under the lock.
    match fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return AdoptedContracts::Read {
                revision: MISSING_REVISION.into(),
                contracts: Vec::new(),
            };
        }
        Ok(metadata) if metadata.is_file() => {}
        _ => return AdoptedContracts::Unreadable,
    }
    let mut bytes = Vec::new();
    let read = File::open(path).and_then(|file| {
        file.take(MAX_ADOPTED_CONTRACTS_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
    });
    if read.is_err() || bytes.len() > MAX_ADOPTED_CONTRACTS_BYTES {
        return AdoptedContracts::Unreadable;
    }
    match serde_json::from_slice::<StoredAdoptedContracts>(&bytes) {
        Ok(stored)
            if stored.format == ADOPTED_CONTRACTS_FORMAT
                && stored.contracts.len() <= MAX_ADOPTED_CONTRACTS =>
        {
            AdoptedContracts::Read {
                revision: revision_for(&bytes),
                contracts: stored.contracts,
            }
        }
        _ => AdoptedContracts::Unreadable,
    }
}

/// Call under the config lock. Adds the `contracts` the file lacks and
/// returns the state as written, or as found when nothing was missing or it
/// cannot be read. Fails without writing past either bound.
fn record_adopted_contracts(
    path: &Path,
    contracts: &[serde_json::Value],
) -> Result<AdoptedContracts> {
    let found = read_adopted_contracts(path);
    let AdoptedContracts::Read {
        contracts: recorded,
        ..
    } = &found
    else {
        return Ok(found);
    };
    let mut next = recorded.clone();
    for contract in contracts {
        if !next.contains(contract) {
            next.push(contract.clone());
        }
    }
    if next.len() == recorded.len() {
        return Ok(found);
    }
    let bytes = serde_json::to_vec(&StoredAdoptedContracts {
        format: ADOPTED_CONTRACTS_FORMAT,
        contracts: next.clone(),
    })
    .context("serializing adopted adapter contracts")?;
    if next.len() > MAX_ADOPTED_CONTRACTS || bytes.len() > MAX_ADOPTED_CONTRACTS_BYTES {
        bail!("{} is full", path.display());
    }
    let parent = path.parent().context("state path has no parent")?;
    // The config directory's startup sweep removes this name if a killed
    // host leaves it behind.
    let temporary = parent.join(format!(".config-{}.tmp", Uuid::new_v4()));
    let (mut file, _cleanup) = create_temporary(&temporary)?;
    let write_result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    write_result?;
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    if let Err(error) = sync_parent(parent) {
        eprintln!(
            "{NAMESPACE}: warning: contract state was saved but directory sync failed: {error:#}"
        );
    }
    Ok(AdoptedContracts::Read {
        revision: revision_for(&bytes),
        contracts: next,
    })
}

fn compile_validated_adapters(config: &ConfigSnapshot) -> Result<AdapterTemplates> {
    compile_adapters(config)
}

/// The origins automatic capture may record: the distinct origins of the
/// config's adapters. A site is on exactly when it has an adapter. An adapter
/// document that does not validate yields no origins, so capture stays off.
/// The host itself reads `ConfigStore::capture_origins`, which reuses the
/// document already compiled for the current revision.
#[cfg(test)]
pub(crate) fn capture_origins(config: &ConfigSnapshot) -> Vec<String> {
    compile_validated_adapters(config)
        .map(|document| document_origins(&document))
        .unwrap_or_default()
}

fn document_origins(document: &AdapterTemplates) -> Vec<String> {
    let mut origins = Vec::new();
    for adapter in document.adapters() {
        if !origins.iter().any(|origin| origin == adapter.origin()) {
            origins.push(adapter.origin().to_owned());
        }
    }
    origins
}

pub(crate) fn adapter_policy_valid(config: &ConfigSnapshot) -> bool {
    compile_validated_adapters(config).is_ok()
}

fn adapter_warnings(document: &AdapterTemplates) -> Vec<AdapterWarning> {
    #[cfg(test)]
    ADAPTER_OVERLAP_COUNT.set(ADAPTER_OVERLAP_COUNT.get() + 1);
    document
        .overlap_report()
        .into_iter()
        .take(MAX_ADAPTER_WARNINGS)
        .map(|pair| AdapterWarning {
            first_adapter: pair.first.0 as u32,
            first_rule: pair.first.1 as u32,
            second_adapter: pair.second.0 as u32,
            second_rule: pair.second.1 as u32,
        })
        .collect()
}

fn compile_adapters(config: &ConfigSnapshot) -> Result<AdapterTemplates> {
    #[cfg(test)]
    ADAPTER_COMPILE_COUNT.set(ADAPTER_COMPILE_COUNT.get() + 1);
    let json =
        serde_json::to_value(&config.adapters).context("serializing the adapter configuration")?;
    let value: AdapterValue =
        serde_json::from_value(json).context("reading the adapter configuration")?;
    AdapterTemplates::from_value(&value)
        .map_err(|error| anyhow::anyhow!("adapter configuration is invalid: {error}"))
}

/// Artifact IDs are record keys. Alias events are not implemented until
/// M1.5b, so an existing adapter's ID template is immutable for now. Compare
/// the ID-template multisets for each origin and type: removing a member makes
/// the next set a subset, adding one makes the current set a subset, and a
/// replacement is the ID edit that must be refused. This is intentionally
/// stricter than the eventual activity-aware migration.
pub(crate) fn validate_adapter_transition(
    current: &ConfigSnapshot,
    next: &ConfigSnapshot,
) -> Result<()> {
    validate_adapter_transition_with_history(current, next, &[])
}

fn validate_adapter_transition_with_history(
    current: &ConfigSnapshot,
    next: &ConfigSnapshot,
    retired: &[serde_json::Value],
) -> Result<()> {
    validate_adapter_history(retired)?;
    let current_is_valid = adapter_policy_valid(current);
    if current_is_valid {
        validate_active_id_transition(current, next)?;
    }

    let current_contracts = if current_is_valid {
        adapter_contracts(current)
    } else {
        individually_valid_adapter_contracts(current)
    };
    let (removed, added) =
        adapter_contract_delta_between(current_contracts, adapter_contracts(next));
    for candidate in &added {
        let Some((origin, id)) = adapter_contract_identity(candidate) else {
            continue;
        };
        let Some(artifact_type) = adapter_contract_type(candidate) else {
            continue;
        };
        for previous in retired.iter().chain(&removed) {
            let Some((previous_origin, previous_id)) = adapter_contract_identity(previous) else {
                continue;
            };
            let Some(previous_type) = adapter_contract_type(previous) else {
                continue;
            };
            if origin == previous_origin
                && (id != previous_id || artifact_type != previous_type)
                && adapter_contracts_overlap(previous, candidate)?
            {
                bail!(
                    "the artifact type or id template for adapter {artifact_type:?} on {origin:?} cannot change until artifact migrations are available"
                );
            }
        }
    }
    Ok(())
}

fn validate_active_id_transition(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<()> {
    let current_ids = adapter_id_counts(current);
    let next_ids = adapter_id_counts(next);
    for ((origin, artifact_type), old) in current_ids {
        let Some(new) = next_ids.get(&(origin, artifact_type)) else {
            continue;
        };
        let old_is_subset = old
            .iter()
            .all(|(id, count)| new.get(id).is_some_and(|next| next >= count));
        let new_is_subset = new
            .iter()
            .all(|(id, count)| old.get(id).is_some_and(|current| current >= count));
        if !old_is_subset && !new_is_subset {
            bail!(
                "the id template for adapter {artifact_type:?} on {origin:?} cannot change until artifact migrations are available"
            );
        }
    }
    Ok(())
}

/// Keep the recognition portion of an adapter with its durable ID. Display,
/// field, and edge metadata cannot affect which artifact record a URL names.
fn adapter_contract(value: &serde_json::Value) -> Option<serde_json::Value> {
    const KEYS: &[&str] = &[
        "type",
        "origin",
        "match",
        "id",
        "modes",
        "scope",
        "unscoped",
        "path_case",
    ];
    let source = value.as_object()?;
    let mut contract = serde_json::Map::new();
    for key in KEYS {
        if let Some(entry) = source.get(*key) {
            contract.insert((*key).to_owned(), entry.clone());
        }
    }
    let contract = serde_json::Value::Object(contract);
    adapter_contract_identity(&contract)?;
    contract.get("match")?.as_array()?;
    Some(contract)
}

fn adapter_contracts(config: &ConfigSnapshot) -> Vec<serde_json::Value> {
    config
        .adapters
        .artifacts
        .iter()
        .filter_map(adapter_contract)
        .collect()
}

fn adapter_contract_identity(value: &serde_json::Value) -> Option<(&str, &str)> {
    Some((value.get("origin")?.as_str()?, value.get("id")?.as_str()?))
}

fn adapter_contract_type(value: &serde_json::Value) -> Option<&str> {
    value.get("type")?.as_str()
}

/// Pair unchanged contracts first, then pair recognition edits that retain the
/// same origin, type, and ID. What remains is a real retirement or introduction.
fn adapter_contract_delta_between(
    current: Vec<serde_json::Value>,
    next: Vec<serde_json::Value>,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let mut current_matched = vec![false; current.len()];
    let mut next_matched = vec![false; next.len()];

    for (old_index, old) in current.iter().enumerate() {
        if let Some((new_index, _)) = next
            .iter()
            .enumerate()
            .find(|(index, new)| !next_matched[*index] && *new == old)
        {
            current_matched[old_index] = true;
            next_matched[new_index] = true;
        }
    }
    for (old_index, old) in current.iter().enumerate() {
        if current_matched[old_index] {
            continue;
        }
        if let Some((new_index, _)) = next.iter().enumerate().find(|(index, new)| {
            !next_matched[*index]
                && adapter_contract_identity(old) == adapter_contract_identity(new)
                && adapter_contract_type(old) == adapter_contract_type(new)
        }) {
            current_matched[old_index] = true;
            next_matched[new_index] = true;
        }
    }

    let removed = current
        .into_iter()
        .enumerate()
        .filter_map(|(index, contract)| (!current_matched[index]).then_some(contract))
        .collect();
    let added = next
        .into_iter()
        .enumerate()
        .filter_map(|(index, contract)| (!next_matched[index]).then_some(contract))
        .collect();
    (removed, added)
}

fn validate_adapter_history(history: &[serde_json::Value]) -> Result<()> {
    for contract in history {
        if adapter_contract(contract).as_ref() != Some(contract) {
            bail!("adapter ID history is invalid");
        }
        let config = ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts: vec![contract.clone()],
            },
            ..empty_config()
        };
        compile_adapters(&config).context("adapter ID history is invalid")?;
    }
    Ok(())
}

/// Preserve every contract an older host can validate independently, even
/// when another entry or a future document format makes the whole policy
/// inert. This is only a conservative record-key freeze: these contracts are
/// never used for capture until the complete document validates.
fn individually_valid_adapter_contracts(config: &ConfigSnapshot) -> Vec<serde_json::Value> {
    adapter_contracts(config)
        .into_iter()
        .filter(|contract| validate_adapter_history(std::slice::from_ref(contract)).is_ok())
        .collect()
}

fn adapter_contracts_overlap(
    previous: &serde_json::Value,
    candidate: &serde_json::Value,
) -> Result<bool> {
    let config = ConfigSnapshot {
        adapters: AdapterDocument {
            format: 1,
            artifacts: vec![previous.clone(), candidate.clone()],
        },
        ..empty_config()
    };
    Ok(!compile_adapters(&config)?.overlap_report().is_empty())
}

fn updated_adapter_history(
    history: &[serde_json::Value],
    current: &ConfigSnapshot,
    next: &ConfigSnapshot,
) -> Vec<serde_json::Value> {
    let active = adapter_contracts(next);
    let mut updated: Vec<_> = history
        .iter()
        // An active adapter only supersedes the exact historical contract.
        // A disjoint or narrowed adapter may deliberately reuse the same ID,
        // but it does not protect routes that only the historical contract
        // recognized from being reintroduced later under a different ID.
        .filter(|contract| !active.contains(contract))
        .cloned()
        .collect();
    // Recognition edits that retain an ID also retire the old shape. Keep it
    // for the same reason as a wholly removed adapter: a later update must not
    // be able to assign that old route a new record key. If the whole current
    // document is invalid, retain the contracts this host can still validate
    // independently; they remain inert and are used only as migration guards.
    for contract in individually_valid_adapter_contracts(current) {
        if active.contains(&contract) {
            continue;
        }
        if !updated.contains(&contract) {
            updated.push(contract);
        }
    }
    updated
}

fn adapter_id_counts(config: &ConfigSnapshot) -> BTreeMap<(&str, &str), BTreeMap<&str, usize>> {
    let mut groups = BTreeMap::new();
    for adapter in &config.adapters.artifacts {
        let (Some(identity), Some(id)) = (adapter_identity(adapter), adapter["id"].as_str()) else {
            continue;
        };
        *groups
            .entry(identity)
            .or_insert_with(BTreeMap::new)
            .entry(id)
            .or_default() += 1;
    }
    groups
}

fn adapter_identity(value: &serde_json::Value) -> Option<(&str, &str)> {
    Some((value.get("origin")?.as_str()?, value.get("type")?.as_str()?))
}

/// Validate a config the host itself persisted, including that its notes
/// folder can be opened. Only call this for a root that is already trusted.
fn validate_persisted(config: &ConfigSnapshot) -> Result<()> {
    validate_base(config)?;
    if let Some(storage) = &config.storage {
        Vault::open(storage).context("home folder is unavailable")?;
    }
    Ok(())
}

fn validate_storage(storage: &StorageConfig) -> Result<()> {
    if storage.root.len() > MAX_ROOT_PATH_BYTES {
        bail!("storage.root exceeds the 4096-byte path limit");
    }
    if has_unsafe_display_chars(&storage.root) {
        bail!("home folder path contains control or bidirectional formatting characters");
    }
    let root = Path::new(&storage.root);
    if !root.is_absolute() {
        bail!("storage.root must be an existing absolute folder selected by the user");
    }
    // The knowledge base is its own user-selected folder, read only by the
    // summary agent. It may hold or sit inside the home folder.
    if let Some(knowledge_base) = &storage.knowledge_base_dir {
        if knowledge_base.len() > MAX_ROOT_PATH_BYTES {
            bail!("storage.knowledge_base_dir exceeds the 4096-byte path limit");
        }
        if has_unsafe_display_chars(knowledge_base) {
            bail!(
                "knowledge base folder path contains control or bidirectional formatting characters"
            );
        }
        if !Path::new(knowledge_base).is_absolute() {
            bail!(
                "storage.knowledge_base_dir must be an existing absolute folder selected by the user"
            );
        }
    }
    if storage.profile != "neutral" {
        bail!("storage.profile must be neutral until the Obsidian preset is implemented");
    }

    let mut paths = vec![
        Path::new(&storage.log_dir),
        Path::new(&storage.pages_dir),
        Path::new(&storage.later_dir),
        Path::new(&storage.artifacts_dir),
    ];
    if let Some(summaries_dir) = &storage.summaries_dir {
        paths.push(Path::new(summaries_dir));
    }
    for path in &paths {
        let value = path.as_os_str().to_string_lossy();
        if value.len() > MAX_LOCATION_PATH_BYTES {
            bail!("content location exceeds the 512-byte path limit");
        }
        if has_unsafe_display_chars(&value) {
            bail!("content location contains control or bidirectional formatting characters");
        }
        if path.as_os_str().is_empty() || !path.components().all(|part| {
            matches!(part, Component::Normal(component) if component.len() <= MAX_PATH_COMPONENT_BYTES)
        }) {
            bail!("content locations must be nonempty relative paths without . or .. components");
        }
    }
    for left in 0..paths.len() {
        for right in (left + 1)..paths.len() {
            let l = lower_components(paths[left]);
            let r = lower_components(paths[right]);
            if l.starts_with(&r) || r.starts_with(&l) {
                bail!("log, pages, later, artifacts, and summaries locations must not overlap");
            }
        }
    }
    Ok(())
}

pub(crate) fn has_unsafe_display_chars(value: &str) -> bool {
    value.chars().any(|ch| {
        ch.is_control()
            || matches!(
                ch,
                '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
            )
    })
}

/// The shape a summary harness entry needs. Setup also found the binary on
/// the service's `PATH` and probed it.
fn validate_summarizer(agent: &AgentConfig) -> Result<()> {
    if agent.binary.len() > MAX_ROOT_PATH_BYTES
        || has_unsafe_display_chars(&agent.binary)
        || !Path::new(&agent.binary).is_absolute()
    {
        bail!("the summary harness binary must be an absolute path");
    }
    if agent.version.as_deref().is_some_and(|version| {
        version.len() > MAX_VERSION_BYTES || has_unsafe_display_chars(version)
    }) {
        bail!("the summary harness version is not valid");
    }
    if !SUMMARY_TIMEOUT_SECS.contains(&agent.timeout_secs) {
        bail!("the summary harness timeout must be 60 to 3600 seconds");
    }
    if agent.probed_at.len() > MAX_PROBED_AT_BYTES
        || time::OffsetDateTime::parse(
            &agent.probed_at,
            &time::format_description::well_known::Rfc3339,
        )
        .is_err()
    {
        bail!("the summary harness probe time must be an RFC 3339 timestamp");
    }
    Ok(())
}

/// Path components in canonical caseless form, NFD(fold(NFD(name))), so two
/// names a case-insensitive APFS volume treats as one folder compare equal.
/// `to_lowercase` is not enough: it keeps final sigma distinct from sigma.
pub(crate) fn lower_components(path: &Path) -> Vec<String> {
    let fold = icu_casemap::CaseMapper::new();
    let nfd = icu_normalizer::DecomposingNormalizer::new_nfd();
    path.components()
        .map(|part| {
            let name = part.as_os_str().to_string_lossy();
            nfd.normalize(&fold.fold_string(&nfd.normalize(&name)))
                .into_owned()
        })
        .collect()
}

struct TempFileCleanup<'a> {
    path: &'a Path,
    armed: bool,
}

impl TempFileCleanup<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFileCleanup<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

#[cfg(not(windows))]
fn sync_parent(parent: &Path) -> Result<()> {
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn sync_parent(_parent: &Path) -> Result<()> {
    // Windows directory handles are not opened for fsync by std::fs::File.
    // The temporary file itself was synced before the atomic rename.
    Ok(())
}

/// config.toml bodies as a user or an older build would have written them.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::vault::selected_root_identity;

    fn quoted(value: &str) -> String {
        toml::Value::String(value.to_owned()).to_string()
    }

    /// A config with storage under `root`. `keys` go at the top level and
    /// `tables` after `[storage]`.
    /// The storage keys default to the fixed layout; `storage_extra` may set
    /// any of them instead, as an older build's file would.
    pub(crate) fn write(path: &Path, root: &Path, storage_extra: &str, keys: &str, tables: &str) {
        let identity = selected_root_identity(root).unwrap();
        let mut layout = String::new();
        for (key, value) in [
            ("log_dir", "logs"),
            ("pages_dir", "notes"),
            ("later_dir", "later"),
            ("artifacts_dir", "artifacts"),
            ("summaries_dir", "summaries"),
        ] {
            if !storage_extra
                .lines()
                .any(|line| line.split('=').next().map(str::trim) == Some(key))
            {
                layout.push_str(&format!("{key} = \"{value}\"\n"));
            }
        }
        let body = format!(
            "root_picker_confirmed = true\nroot_identity = {}\n{keys}\n[storage]\nroot = {}\nprofile = \"neutral\"\n{layout}{storage_extra}\n{tables}",
            quoted(&identity),
            quoted(&root.to_string_lossy()),
        );
        fs::write(path, body).unwrap();
    }

    /// The storage table an older build wrote: `log/`, `pages/`, and
    /// `later/`, with `storage_extra` adding the keys later versions added.
    pub(crate) fn write_legacy(path: &Path, root: &Path, storage_extra: &str) {
        write(
            path,
            root,
            &format!(
                "log_dir = \"log\"\npages_dir = \"pages\"\nlater_dir = \"later\"\n{storage_extra}"
            ),
            "",
            "",
        );
    }

    /// A home folder and a config.toml with a summary harness set up.
    /// Returns the config path and the home folder.
    pub(crate) fn ready(folder: &Path) -> (PathBuf, PathBuf) {
        let root = folder.join("notes");
        fs::create_dir(&root).unwrap();
        let path = folder.join("config.toml");
        write(
            &path,
            &root,
            "summaries_dir = \"summaries\"",
            "",
            &agent_table(Path::new(&absent_binary("claude"))),
        );
        (path, root)
    }

    /// An absolute harness path on this platform for tests that never read
    /// or run it. Unix keeps the path a real install would have.
    pub(crate) fn absent_binary(name: &str) -> String {
        if cfg!(windows) {
            format!("C:\\Program Files\\{name}\\{name}.exe")
        } else {
            format!("/usr/local/bin/{name}")
        }
    }

    /// A Claude Code `[summarizer]` table with the default time limit.
    pub(crate) fn agent_table(binary: &Path) -> String {
        summarizer_table("claude", binary, 600)
    }

    pub(crate) fn summarizer_table(harness: &str, binary: &Path, timeout_secs: u32) -> String {
        format!(
            "[summarizer]\nharness = \"{harness}\"\nbinary = {}\nversion = \"2.1.284\"\ntimeout_secs = {timeout_secs}\nprobed_at = \"2026-10-27T10:00:00Z\"\n",
            quoted(&binary.to_string_lossy()),
        )
    }

    /// The v5 harness entry and checked record a native setup wrote.
    pub(crate) const V5_HARNESS_TABLES: &str = "[agent]\ndefault = \"claude-code\"\n[agent.harnesses.claude-code]\nbinary = \"/usr/local/bin/claude\"\nargs = [\"-p\", \"{prompt}\"]\nenv_allow = [\"HOME\", \"PATH\"]\ntimeout_secs = 120\n\n[harness_record]\nharness_id = \"claude-code\"\nreal_path = \"/usr/local/bin/claude\"\nsize = 1\nmtime_ns = \"1\"\nversion = \"2.1.284\"\nhelp_sha256 = \"00\"\nconfirmed_flags = [\"-p\"]\nprobe_passed_at = \"2026-09-28T00:00:00Z\"\n";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn loaded(path: &Path) -> ConfigStore {
        let mut store = ConfigStore::for_test(path.to_path_buf());
        store.refresh().unwrap();
        store
    }

    #[test]
    fn an_uncompilable_recorded_contract_makes_its_origins_note_identity_unavailable() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write(
            &path,
            &root,
            "",
            "",
            concat!(
                "[[adapter_id_history]]\ntype = \"work.item\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/items/{id:int}\" }]\nid = \"work:{host}/{id}\"\n\n",
                // Parseable, but no host can compile this shape.
                "[[adapter_id_history]]\ntype = \"work.other\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/items/{n:nope}\" }]\nid = \"other:{host}/{n}\"\n\n",
                // Without a valid origin it never compiled anywhere.
                "[[adapter_id_history]]\ntype = \"bad.origin\"\norigin = \"not an origin\"\nmatch = [{ path = \"/{n:int}\" }]\nid = \"bad:{host}/{n}\"\n\n",
            ),
        );
        let store = loaded(&path);
        assert!(store.config_issue().is_none());
        assert_eq!(
            store.note_key_rule("https://work.example/items/7"),
            NoteKeyRule::Unavailable
        );
        assert_eq!(
            store.note_key_rule("https://elsewhere.example/items/7"),
            NoteKeyRule::Uncovered
        );
    }

    #[test]
    fn retired_contracts_name_note_keys_but_overlapping_ids_are_ambiguous() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        let history = |artifact_type: &str, path: &str, id: &str| {
            format!(
                "[[adapter_id_history]]\ntype = {artifact_type:?}\norigin = \"https://work.example\"\nmatch = [{{ path = {path:?} }}]\nid = {id:?}\n\n"
            )
        };
        fixtures::write(
            &path,
            &root,
            "",
            "",
            &format!(
                "{}{}{}",
                history("work.doc", "/docs/{id:int}", "doc:{host}/{id}"),
                history("work.item", "/items/{id:int}", "work:{host}/{id}"),
                // An overlapping shape with another ID, as two once-active
                // overlapping adapters would leave behind.
                history("work.other", "/items/{n:int}", "other:{host}/{n}"),
            ),
        );
        let store = loaded(&path);
        assert!(store.config_issue().is_none());
        assert!(
            store
                .adapter_match("https://work.example/docs/8", None)
                .is_err()
        );
        assert_eq!(
            store.note_key_rule("https://work.example/docs/8"),
            NoteKeyRule::Artifact("doc:work.example/8".into())
        );
        assert_eq!(
            store.note_key_rule("https://work.example/items/7"),
            NoteKeyRule::Ambiguous
        );
        assert_eq!(
            store.note_key_rule("https://work.example/boards/1"),
            NoteKeyRule::Unrecognized
        );
        assert_eq!(
            store.note_key_rule("https://elsewhere.example/items/8"),
            NoteKeyRule::Uncovered
        );
    }

    /// A repair rewrites a config read_disk_state could not use. It must
    /// carry forward every `[[adapter_id_history]]` entry this host can
    /// validate: those entries keep removed adapters' IDs from being reused
    /// and keep their note keys (DESIGN.md §6.2).
    mod repair_keeps_adapter_id_history {
        use super::*;

        const HISTORY: &str = "[[adapter_id_history]]\ntype = \"work.item\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/items/{id:int}\" }]\nid = \"work:{host}/{id}\"\n\n";
        // Parseable, but no host can compile this shape.
        const DAMAGED: &str = "[[adapter_id_history]]\ntype = \"work.other\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/items/{n:nope}\" }]\nid = \"other:{host}/{n}\"\n\n";

        fn repair(edit: impl Fn(String) -> String) -> (String, ConfigStore) {
            let folder = tempfile::tempdir().unwrap();
            let root = folder.path().join("home");
            fs::create_dir(&root).unwrap();
            let path = folder.path().join("config.toml");
            fixtures::write(&path, &root, "", "", &format!("{HISTORY}{DAMAGED}"));
            fs::write(&path, edit(fs::read_to_string(&path).unwrap())).unwrap();
            let mut store = ConfigStore::for_test(path.clone());
            store.refresh().unwrap();
            assert!(store.config_issue().is_some(), "fixture must need repair");
            let revision = store.revision().to_owned();
            let identity = selected_root_identity(&root).unwrap();
            let storage = layout::fixed(&StorageConfig {
                root: root.to_string_lossy().into_owned(),
                profile: "neutral".into(),
                log_dir: String::new(),
                pages_dir: String::new(),
                later_dir: String::new(),
                artifacts_dir: String::new(),
                summaries_dir: None,
                knowledge_base_dir: None,
            });
            let next = ConfigSnapshot {
                storage: Some(storage),
                ..empty_config()
            };
            store
                .update_with_root_identity(next, &revision, Some(&identity))
                .unwrap()
                .unwrap();
            (fs::read_to_string(&path).unwrap(), loaded(&path))
        }

        fn assert_kept(repaired: &str, store: &ConfigStore) {
            assert_eq!(
                repaired.matches("[[adapter_id_history]]").count(),
                1,
                "{repaired}"
            );
            assert!(repaired.contains("work:{host}/{id}"), "{repaired}");
            assert!(store.config_issue().is_none());
            assert_eq!(
                store.note_key_rule("https://work.example/items/7"),
                NoteKeyRule::Artifact("work:work.example/7".into())
            );
        }

        #[test]
        fn an_unconfirmed_folder_repair_keeps_valid_history() {
            let (repaired, store) =
                repair(|text| text.replace("root_picker_confirmed = true\n", ""));
            assert_kept(&repaired, &store);
        }

        #[test]
        fn an_invalid_settings_repair_keeps_valid_history() {
            let (repaired, store) = repair(|text| format!("strip_params = [\"*\"]\n{text}"));
            assert_kept(&repaired, &store);
        }

        #[test]
        fn a_newer_version_repair_keeps_valid_history() {
            let (repaired, store) = repair(|text| format!("future_key = 1\n{text}"));
            assert_kept(&repaired, &store);
        }
    }

    /// Host-state fixtures for contracts that leave config.toml by hand.
    mod adopted_contracts {
        use super::*;

        const ITEM: &str = "[[artifacts]]\ntype = \"work.item\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/items/{id:int}\" }]\nid = \"work:{host}/{id}\"\n";
        const DOC: &str = "[[artifacts]]\ntype = \"work.doc\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/docs/{id:int}\" }]\nid = \"doc:{host}/{id}\"\n";
        const ITEM_7: &str = "https://work.example/items/7";
        const DOC_8: &str = "https://work.example/docs/8";
        const BOARD: &str = "https://work.example/boards/1";
        const ELSEWHERE: &str = "https://elsewhere.example/items/8";

        /// Write config.toml by hand, as a user editing it would.
        fn write_by_hand(folder: &Path, adapters: &[&str]) -> PathBuf {
            let root = folder.join("notes");
            fs::create_dir_all(&root).unwrap();
            let path = folder.join("config.toml");
            fixtures::write(
                &path,
                &root,
                "",
                "",
                &format!("[adapters]\nformat = 1\n\n{}", adapters.join("\n")),
            );
            path
        }

        /// A refresh under the config lock, as every note request makes.
        fn adopted(path: &Path) -> ConfigStore {
            let mut store = ConfigStore::for_test(path.to_path_buf());
            let lock = store.lock_current().unwrap();
            store.refresh().unwrap();
            drop(lock);
            store
        }

        fn state_path(folder: &Path) -> PathBuf {
            adopted_contracts_path(folder)
        }

        fn entries(folder: &Path) -> Vec<serde_json::Value> {
            let state: serde_json::Value =
                serde_json::from_slice(&fs::read(state_path(folder)).unwrap()).unwrap();
            state["contracts"].as_array().unwrap().clone()
        }

        #[test]
        fn a_hand_removed_contract_keeps_naming_note_keys() {
            let folder = tempfile::tempdir().unwrap();
            let path = write_by_hand(folder.path(), &[ITEM, DOC]);
            adopted(&path);
            assert_eq!(entries(folder.path()).len(), 2);

            write_by_hand(folder.path(), &[ITEM]);
            let store = loaded(&path);
            assert!(store.adapter_match(DOC_8, None).is_err());
            assert_eq!(
                store.note_key_rule(DOC_8),
                NoteKeyRule::Artifact("doc:work.example/8".into())
            );
            // The origin is still decided by contracts alone.
            assert_eq!(store.note_key_rule(BOARD), NoteKeyRule::Unrecognized);
            assert_eq!(store.note_key_rule(ELSEWHERE), NoteKeyRule::Uncovered);
            // Never written into config.toml, and never capture authority.
            assert!(
                !fs::read_to_string(&path)
                    .unwrap()
                    .contains("adapter_id_history")
            );
            assert_eq!(store.capture_origins(), vec!["https://work.example"]);
            assert!(!store.adapter_matches(DOC_8, None).unwrap());
        }

        #[test]
        fn only_a_refresh_under_the_config_lock_records_contracts() {
            let folder = tempfile::tempdir().unwrap();
            let path = write_by_hand(folder.path(), &[ITEM, DOC]);
            let mut store = loaded(&path);
            store.refresh().unwrap();
            assert!(!state_path(folder.path()).exists());
            // A contract nobody adopted under the lock cannot have keyed a
            // note, so its removal leaves nothing to keep.
            write_by_hand(folder.path(), &[ITEM]);
            let store = adopted(&path);
            assert_eq!(store.note_key_rule(DOC_8), NoteKeyRule::Unrecognized);
            assert_eq!(entries(folder.path()).len(), 1);
        }

        #[test]
        fn a_re_added_contract_is_current_again_and_recorded_once() {
            let folder = tempfile::tempdir().unwrap();
            let path = write_by_hand(folder.path(), &[ITEM, DOC]);
            adopted(&path);
            write_by_hand(folder.path(), &[ITEM]);
            adopted(&path);
            write_by_hand(folder.path(), &[DOC, ITEM]);
            let store = adopted(&path);
            assert_eq!(entries(folder.path()).len(), 2);
            assert!(store.note_key_rules.documents.is_empty());
            assert!(store.adapter_matches(DOC_8, None).unwrap());

            write_by_hand(folder.path(), &[ITEM]);
            let store = adopted(&path);
            assert_eq!(entries(folder.path()).len(), 2);
            assert_eq!(
                store.note_key_rule(DOC_8),
                NoteKeyRule::Artifact("doc:work.example/8".into())
            );
        }

        #[test]
        fn a_settings_removal_is_kept_once_by_adapter_id_history() {
            let folder = tempfile::tempdir().unwrap();
            let path = write_by_hand(folder.path(), &[ITEM, DOC]);
            let mut store = adopted(&path);
            let before = fs::read(state_path(folder.path())).unwrap();
            let mut next = store.snapshot().clone();
            next.adapters
                .artifacts
                .retain(|adapter| adapter["type"] != "work.doc");
            let revision = store.revision().to_owned();
            store.update(next, &revision).unwrap().unwrap();
            assert_eq!(store.retired_adapter_contracts.len(), 1);
            assert_eq!(fs::read(state_path(folder.path())).unwrap(), before);

            for store in [store, adopted(&path)] {
                assert_eq!(store.note_key_rules.documents.len(), 1);
                assert_eq!(
                    store.note_key_rule(DOC_8),
                    NoteKeyRule::Artifact("doc:work.example/8".into())
                );
            }
        }

        #[test]
        fn a_settings_save_records_the_contracts_it_writes() {
            let folder = tempfile::tempdir().unwrap();
            let path = write_by_hand(folder.path(), &[ITEM]);
            let mut store = loaded(&path);
            let mut next = store.snapshot().clone();
            next.adapters.artifacts.push(adapter(
                "https://work.example",
                "work.doc",
                "/docs/{id:int}",
                "doc:{host}/{id}",
            ));
            let revision = store.revision().to_owned();
            store.update(next, &revision).unwrap().unwrap();
            assert_eq!(entries(folder.path()).len(), 2);

            write_by_hand(folder.path(), &[ITEM]);
            assert_eq!(
                loaded(&path).note_key_rule(DOC_8),
                NoteKeyRule::Artifact("doc:work.example/8".into())
            );
        }

        #[test]
        fn hosts_sharing_the_state_file_keep_each_others_contracts() {
            let folder = tempfile::tempdir().unwrap();
            let path = write_by_hand(folder.path(), &[ITEM]);
            let mut first = adopted(&path);
            write_by_hand(folder.path(), &[DOC]);
            adopted(&path);
            assert_eq!(entries(folder.path()).len(), 2);

            write_by_hand(folder.path(), &[]);
            let lock = first.lock_current().unwrap();
            first.refresh().unwrap();
            drop(lock);
            assert_eq!(
                first.note_key_rule(ITEM_7),
                NoteKeyRule::Artifact("work:work.example/7".into())
            );
            assert_eq!(
                first.note_key_rule(DOC_8),
                NoteKeyRule::Artifact("doc:work.example/8".into())
            );
        }

        /// The state file can change while config.toml does not, when
        /// another host records into it or it is damaged.
        #[test]
        fn a_changed_state_file_is_reread_without_a_config_change() {
            let folder = tempfile::tempdir().unwrap();
            let path = write_by_hand(folder.path(), &[ITEM]);
            let mut store = adopted(&path);
            assert_eq!(store.note_key_rule(DOC_8), NoteKeyRule::Unrecognized);
            fs::write(state_path(folder.path()), b"not json").unwrap();
            store.refresh().unwrap();
            assert_eq!(store.note_key_rule(DOC_8), NoteKeyRule::Unavailable);
        }

        #[test]
        fn unreadable_state_makes_unrecognized_note_identity_unavailable() {
            let oversized = format!(
                "{{\"format\":1,\"contracts\":[],\"pad\":\"{}\"}}",
                "x".repeat(MAX_ADOPTED_CONTRACTS_BYTES)
            );
            for (damage, bytes) in [
                ("garbage", Some(b"{\"format\":1,\"contr".to_vec())),
                (
                    "newer format",
                    Some(b"{\"format\":2,\"contracts\":[]}".to_vec()),
                ),
                (
                    "unknown key",
                    Some(b"{\"format\":1,\"contracts\":[],\"x\":1}".to_vec()),
                ),
                ("oversized", Some(oversized.into_bytes())),
                ("directory", None),
            ] {
                let folder = tempfile::tempdir().unwrap();
                let path = write_by_hand(folder.path(), &[ITEM, DOC]);
                adopted(&path);
                let state = state_path(folder.path());
                fs::remove_file(&state).unwrap();
                match &bytes {
                    Some(bytes) => fs::write(&state, bytes).unwrap(),
                    None => fs::create_dir(&state).unwrap(),
                }
                write_by_hand(folder.path(), &[ITEM]);

                let mut store = adopted(&path);
                // The contract that left config.toml cannot be read back,
                // so no unrecognized URL anywhere may choose a key.
                for url in [DOC_8, BOARD, ELSEWHERE] {
                    assert_eq!(
                        store.note_key_rule(url),
                        NoteKeyRule::Unavailable,
                        "{damage}: {url}"
                    );
                }
                // The current policy still decides the pages it recognizes.
                assert!(store.adapter_match(ITEM_7, None).is_ok(), "{damage}");
                // The damaged file is evidence: never overwrite it.
                match &bytes {
                    Some(bytes) => assert_eq!(&fs::read(&state).unwrap(), bytes, "{damage}"),
                    None => assert!(state.is_dir(), "{damage}"),
                }

                // Removing it forgets what it held, like a config repair.
                match &bytes {
                    Some(_) => fs::remove_file(&state).unwrap(),
                    None => fs::remove_dir(&state).unwrap(),
                }
                let lock = store.lock_current().unwrap();
                store.refresh().unwrap();
                drop(lock);
                assert_eq!(
                    store.note_key_rule(DOC_8),
                    NoteKeyRule::Unrecognized,
                    "{damage}"
                );
                assert_eq!(
                    store.note_key_rule(ELSEWHERE),
                    NoteKeyRule::Uncovered,
                    "{damage}"
                );
                assert_eq!(entries(folder.path()).len(), 1, "{damage}");
            }
        }

        #[test]
        fn a_damaged_entry_makes_only_its_origin_unavailable() {
            let folder = tempfile::tempdir().unwrap();
            let path = write_by_hand(folder.path(), &[]);
            let damaged = json!({
                "type": "work.other",
                "origin": "https://work.example",
                "match": [{"path": "/items/{n:nope}"}],
                "id": "other:{host}/{n}",
            });
            fs::write(
                state_path(folder.path()),
                serde_json::to_vec(&json!({"format": 1, "contracts": [damaged]})).unwrap(),
            )
            .unwrap();
            let store = loaded(&path);
            assert_eq!(store.note_key_rule(ITEM_7), NoteKeyRule::Unavailable);
            assert_eq!(store.note_key_rule(ELSEWHERE), NoteKeyRule::Uncovered);

            // An entry that names no HTTP(S) origin cannot be attributed to
            // a page's origin.
            for origin in [
                None,
                Some("mailto:x@work.example"),
                Some("ftp://work.example"),
            ] {
                let mut entry = json!({"type": "work.other"});
                if let Some(origin) = origin {
                    entry["origin"] = json!(origin);
                }
                fs::write(
                    state_path(folder.path()),
                    serde_json::to_vec(&json!({"format": 1, "contracts": [entry]})).unwrap(),
                )
                .unwrap();
                let store = loaded(&path);
                assert_eq!(
                    store.note_key_rule(ELSEWHERE),
                    NoteKeyRule::Unavailable,
                    "{origin:?}"
                );
            }
        }

        #[test]
        fn the_state_file_stops_growing_at_its_bound() {
            let folder = tempfile::tempdir().unwrap();
            let path = write_by_hand(folder.path(), &[ITEM]);
            let full: Vec<_> = (0..MAX_ADOPTED_CONTRACTS)
                .map(|n| {
                    adapter(
                        &format!("https://site{n}.example"),
                        "work.item",
                        "/items/{id:int}",
                        "work:{host}/{id}",
                    )
                })
                .collect();
            let bytes = serde_json::to_vec(&json!({"format": 1, "contracts": full})).unwrap();
            fs::write(state_path(folder.path()), &bytes).unwrap();
            let store = adopted(&path);
            assert_eq!(fs::read(state_path(folder.path())).unwrap(), bytes);
            assert_eq!(
                store.note_key_rule("https://site3.example/items/4"),
                NoteKeyRule::Artifact("work:site3.example/4".into())
            );
            assert!(store.adapter_matches(ITEM_7, None).unwrap());
        }
    }

    fn adapter(origin: &str, artifact_type: &str, path: &str, id: &str) -> serde_json::Value {
        json!({
            "type": artifact_type,
            "origin": origin,
            "match": [{"path": path}],
            "id": id,
        })
    }

    fn adapter_config(artifacts: Vec<serde_json::Value>) -> ConfigSnapshot {
        ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts,
            },
            ..empty_config()
        }
    }

    /// The settings page's refusal: its state and message.
    fn refusal(store: &ConfigStore) -> (AgentState, String) {
        let status = store.agent_status();
        assert_ne!(status.state, AgentState::Ready, "{status:?}");
        (
            status.state,
            status.message.expect("a refusal has a message"),
        )
    }

    #[test]
    fn v5_storage_without_knowledge_base_dir_loads_as_null_and_follows_root() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let root = dir.path().join("notes");
        fs::create_dir(&root).unwrap();
        fixtures::write(&path, &root, "summaries_dir = \"summaries\"", "", "");
        let store = loaded(&path);
        assert_eq!(store.config_issue(), None);
        assert_eq!(
            store
                .snapshot()
                .storage
                .as_ref()
                .unwrap()
                .knowledge_base_dir,
            None
        );
        assert_eq!(store.knowledge_base_dir().unwrap(), root);
        assert_eq!(store.knowledge_base_identity(), store.root_identity());
    }

    #[test]
    fn v5_config_without_vocabulary_loads_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let root = dir.path().join("notes");
        fs::create_dir(&root).unwrap();
        fixtures::write(&path, &root, "summaries_dir = \"summaries\"", "", "");
        let mut store = loaded(&path);
        assert_eq!(store.config_issue(), None);
        let vocabulary = &store.snapshot().summary_vocabulary;
        assert_eq!(vocabulary.document_types, DEFAULT_DOCUMENT_TYPES);
        assert_eq!(vocabulary.categories, DEFAULT_CATEGORIES);
        assert_eq!(empty_config().summary_vocabulary, *vocabulary);
        // A plain update saves an edited vocabulary.
        let mut next = store.snapshot().clone();
        next.summary_vocabulary.document_types = vec!["memo".into(), "report".into()];
        next.summary_vocabulary.categories.push("legal".into());
        let revision = store.revision().to_owned();
        store.update(next.clone(), &revision).unwrap().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[summary_vocabulary]"), "{text}");
        let reloaded = loaded(&path);
        assert_eq!(reloaded.config_issue(), None);
        assert_eq!(
            reloaded.snapshot().summary_vocabulary,
            next.summary_vocabulary
        );
    }

    #[test]
    fn duplicate_vocabulary_entry_is_invalid_config() {
        let valid = empty_config();
        validate_base(&valid).unwrap();
        let invalid = |edit: &dyn Fn(&mut SummaryVocabulary)| {
            let mut config = empty_config();
            edit(&mut config.summary_vocabulary);
            validate_base(&config).is_err()
        };
        assert!(invalid(&|v| v.document_types.push("Report".into())));
        assert!(invalid(&|v| v.categories.push("NEWS".into())));
        assert!(invalid(&|v| v.document_types.clear()));
        assert!(invalid(&|v| v.categories.clear()));
        assert!(invalid(&|v| v.document_types.push(String::new())));
        assert!(invalid(&|v| v.categories.push("tab\there".into())));
        assert!(invalid(&|v| v.categories.push("x".repeat(65))));
        assert!(invalid(&|v| {
            v.document_types = (0..65).map(|n| format!("type {n}")).collect();
        }));
        // The limits are inclusive, and a value may be in both lists.
        assert!(!invalid(&|v| {
            v.document_types = (0..64).map(|n| format!("type {n}")).collect();
            v.categories.push("é".repeat(64));
            v.categories.push("report".into());
        }));

        // A config.toml with a duplicate loads as needing repair.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let root = dir.path().join("notes");
        fs::create_dir(&root).unwrap();
        fixtures::write(
            &path,
            &root,
            "",
            "",
            "[summary_vocabulary]\ndocument_types = [\"report\", \"REPORT\"]\ncategories = [\"news\"]\n",
        );
        let store = loaded(&path);
        assert!(store.config_issue().is_some());
        assert_eq!(
            store.snapshot().summary_vocabulary,
            valid.summary_vocabulary
        );
    }

    #[test]
    fn relative_knowledge_base_dir_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = ConfigSnapshot {
            storage: Some(StorageConfig {
                root: dir.path().to_string_lossy().into_owned(),
                profile: "neutral".into(),
                log_dir: "logs".into(),
                pages_dir: "notes".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: Some("summaries".into()),
                knowledge_base_dir: Some("kb".into()),
            }),
            ..empty_config()
        };
        assert!(validate_base(&config).is_err());
        let storage = config.storage.as_mut().unwrap();
        storage.knowledge_base_dir = Some(format!("/{}", "k".repeat(MAX_ROOT_PATH_BYTES)));
        assert!(validate_base(&config).is_err());
        let storage = config.storage.as_mut().unwrap();
        storage.knowledge_base_dir = Some("/kb\u{202e}".into());
        assert!(validate_base(&config).is_err());
        let storage = config.storage.as_mut().unwrap();
        // The knowledge base may be, hold, or sit inside the home folder.
        storage.knowledge_base_dir = Some(storage.root.clone());
        validate_base(&config).unwrap();
    }

    #[test]
    fn knowledge_base_dir_needs_its_own_picker_identity_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let root = dir.path().join("notes");
        let knowledge_base = dir.path().join("kb");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&knowledge_base).unwrap();
        fixtures::write(&path, &root, "summaries_dir = \"summaries\"", "", "");
        let mut store = loaded(&path);
        let mut next = store.snapshot().clone();
        next.storage.as_mut().unwrap().knowledge_base_dir =
            Some(knowledge_base.to_string_lossy().into_owned());
        let revision = store.revision().to_owned();
        // The home folder's identity does not vouch for another folder.
        let root_identity = store.root_identity().unwrap().to_owned();
        assert!(
            store
                .update_with_grants(next.clone(), &revision, None, Some(&root_identity), None)
                .is_err()
        );
        assert!(store.update(next.clone(), &revision).is_err());

        let identity = selected_root_identity(&knowledge_base).unwrap();
        store
            .update_with_grants(next.clone(), &revision, None, Some(&identity), None)
            .unwrap()
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("knowledge_base_identity"), "{text}");
        let mut reloaded = loaded(&path);
        assert_eq!(reloaded.snapshot(), &next);
        assert_eq!(reloaded.knowledge_base_dir().unwrap(), knowledge_base);
        assert_eq!(reloaded.knowledge_base_identity(), Some(identity.as_str()));

        // An unrelated save keeps the stored identity without a new grant.
        let mut unrelated = next.clone();
        unrelated.capture_enabled = true;
        let revision = reloaded.revision().to_owned();
        reloaded.update(unrelated, &revision).unwrap().unwrap();
        assert_eq!(reloaded.knowledge_base_identity(), Some(identity.as_str()));

        // Following the home folder again drops the host-only identity.
        let mut follow = reloaded.snapshot().clone();
        follow.storage.as_mut().unwrap().knowledge_base_dir = None;
        let revision = reloaded.revision().to_owned();
        reloaded.update(follow, &revision).unwrap().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("knowledge_base"), "{text}");
        assert_eq!(reloaded.knowledge_base_dir().unwrap(), root);
    }

    #[test]
    fn a_missing_knowledge_base_folder_does_not_block_unrelated_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let root = dir.path().join("notes");
        let knowledge_base = dir.path().join("kb");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&knowledge_base).unwrap();
        fixtures::write(&path, &root, "summaries_dir = \"summaries\"", "", "");
        let mut store = loaded(&path);
        let mut next = store.snapshot().clone();
        next.storage.as_mut().unwrap().knowledge_base_dir =
            Some(knowledge_base.to_string_lossy().into_owned());
        let identity = selected_root_identity(&knowledge_base).unwrap();
        let revision = store.revision().to_owned();
        store
            .update_with_grants(next.clone(), &revision, None, Some(&identity), None)
            .unwrap()
            .unwrap();

        fs::remove_dir(&knowledge_base).unwrap();
        let mut reloaded = loaded(&path);
        assert_eq!(reloaded.config_issue(), None);
        let mut paused = reloaded.snapshot().clone();
        paused.capture_enabled = !paused.capture_enabled;
        let revision = reloaded.revision().to_owned();
        reloaded.update(paused.clone(), &revision).unwrap().unwrap();
        assert_eq!(reloaded.snapshot(), &paused);
        assert_eq!(reloaded.knowledge_base_identity(), Some(identity.as_str()));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("knowledge_base_identity"), "{text}");

        // A fresh selection still has to match the folder on disk.
        let revision = reloaded.revision().to_owned();
        assert!(
            reloaded
                .update_with_grants(paused, &revision, None, Some(&identity), None)
                .is_err()
        );
    }

    #[test]
    fn m1_config_without_new_keys_loads_not_set_up() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        // An M1 file: storage without artifacts_dir or summaries_dir, and
        // no agent keys. It loads as an older layout, then moves to the
        // fixed one; with no folders created yet there is nothing to rename.
        fixtures::write_legacy(&path, folder.path(), "");
        let body = fs::read_to_string(&path).unwrap();
        let body = body
            .lines()
            .filter(|line| !line.starts_with("artifacts_dir") && !line.starts_with("summaries_dir"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, format!("capture_enabled = false\n{body}")).unwrap();
        let mut store = loaded(&path);
        let storage = store.snapshot().storage.clone().unwrap();
        assert_eq!(storage.summaries_dir, None);
        assert_eq!(storage.artifacts_dir, "artifacts");
        assert_eq!(store.config_issue(), Some(LAYOUT_PENDING));
        assert!(!store.configured());
        store.migrate_layout().unwrap();
        assert_eq!(store.config_issue(), None);
        assert!(store.configured());
        let snapshot = store.snapshot();
        assert!(layout::is_fixed(snapshot.storage.as_ref().unwrap()));
        assert_eq!(snapshot.agent, None);
        assert_eq!(snapshot.adapters.format, 1);
        assert!(snapshot.adapters.artifacts.is_empty());
        assert_eq!(store.adapter_issue(), None);
        assert_eq!(
            refusal(&store),
            (AgentState::NotSetUp, SET_UP_HARNESS.into())
        );
    }

    #[test]
    fn old_config_gets_a_non_overlapping_artifact_directory() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write_legacy(&path, folder.path(), "");
        let body = fs::read_to_string(&path)
            .unwrap()
            .replace("pages_dir = \"pages\"", "pages_dir = \"artifacts\"")
            .replace("artifacts_dir = \"artifacts\"\n", "");
        fs::write(&path, body).unwrap();

        let store = loaded(&path);
        assert_eq!(store.config_issue(), Some(LAYOUT_PENDING));
        let storage = store.snapshot().storage.as_ref().unwrap();
        assert_eq!(storage.pages_dir, "artifacts");
        assert_eq!(storage.artifacts_dir, "brauser-artifacts");
    }

    #[test]
    fn old_config_with_an_empty_location_reports_the_usual_issue() {
        // An empty location must not stall the artifact-name search; the
        // file should fail validation exactly as it does with artifacts_dir.
        let issue = |storage_extra: &'static str, line: &'static str| {
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let folder = tempfile::tempdir().unwrap();
                let path = folder.path().join("config.toml");
                fixtures::write_legacy(&path, folder.path(), storage_extra);
                if !storage_extra.contains("artifacts_dir") {
                    let body = fs::read_to_string(&path)
                        .unwrap()
                        .replace("artifacts_dir = \"artifacts\"\n", "");
                    fs::write(&path, body).unwrap();
                }
                let (field, _) = line.split_once(" = ").unwrap();
                let body = fs::read_to_string(&path)
                    .unwrap()
                    .replace(line, &format!("{field} = \"\""));
                fs::write(&path, body).unwrap();
                let _ = sender.send(loaded(&path).config_issue().map(str::to_owned));
            });
            receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("loading with an emptied {line} hung"))
        };
        for line in [
            "log_dir = \"log\"",
            "pages_dir = \"pages\"",
            "later_dir = \"later\"",
            "summaries_dir = \"summaries\"",
        ] {
            let with_artifacts = issue(
                "artifacts_dir = \"artifacts\"\nsummaries_dir = \"summaries\"",
                line,
            );
            assert!(
                with_artifacts.is_some(),
                "an emptied {line} should be rejected"
            );
            let without_artifacts = issue("summaries_dir = \"summaries\"", line);
            assert_eq!(without_artifacts, with_artifacts, "{line}");
        }
    }

    #[test]
    fn adapter_config_needs_no_site_entry_but_requires_shared_origin_scope() {
        let jira = adapter(
            "https://acme.atlassian.net",
            "jira.issue",
            "/browse/{key:jira_key}",
            "jira:{host}/{key}",
        );
        let without_site = ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts: vec![jira],
            },
            ..empty_config()
        };
        // An adapter turns its site on; there is no separate site entry.
        validate(&without_site).unwrap();
        assert_eq!(
            capture_origins(&without_site),
            vec!["https://acme.atlassian.net".to_owned()]
        );

        let github = adapter(
            "https://github.com",
            "github.issue",
            "/{owner:name|lower}/{repo:name}/issues/{number:int}",
            "github.issue:{host}/{owner}/{repo}/{number}",
        );
        let error = validate(&adapter_config(vec![github.clone()]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("shared_origin_unscoped"), "{error}");
        let mut acknowledged = github;
        acknowledged["unscoped"] = json!("acknowledged");
        validate(&adapter_config(vec![acknowledged])).unwrap();
    }

    #[test]
    fn adapter_overlap_warnings_name_both_rules() {
        let config = adapter_config(vec![
            adapter(
                "https://work.example",
                "work.issue",
                "/issues/{id:int}",
                "work.issue:{host}/{id}",
            ),
            adapter(
                "https://work.example",
                "work.ticket",
                "/issues/{id:int}",
                "work.ticket:{host}/{id}",
            ),
        ]);
        assert_eq!(
            validate_adapters(&config).unwrap(),
            vec![AdapterWarning {
                first_adapter: 0,
                first_rule: 0,
                second_adapter: 1,
                second_rule: 0,
            }]
        );
    }

    #[test]
    fn unchanged_refresh_and_matching_reuse_compiled_adapter_policy() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut store = ConfigStore::for_test(path.clone());
        let config = adapter_config(vec![
            adapter(
                "https://work.example",
                "work.issue",
                "/issues/{id:int}",
                "work.issue:{host}/{id}",
            ),
            adapter(
                "https://work.example",
                "work.ticket",
                "/issues/{id:int}",
                "work.ticket:{host}/{id}",
            ),
        ]);
        store.update(config, MISSING_REVISION).unwrap().unwrap();
        let mut store = loaded(&path);

        ADAPTER_COMPILE_COUNT.set(0);
        ADAPTER_OVERLAP_COUNT.set(0);
        store.refresh().unwrap();
        assert!(
            store
                .adapter_matches("https://work.example/issues/42", Some("Issue 42"))
                .unwrap()
        );
        assert!(
            store
                .adapter_matches("https://work.example/issues/43", Some("Issue 43"))
                .unwrap()
        );

        assert_eq!(ADAPTER_COMPILE_COUNT.get(), 0);
        assert_eq!(ADAPTER_OVERLAP_COUNT.get(), 0);

        assert_eq!(store.adapter_warnings().len(), 1);
        assert_eq!(ADAPTER_COMPILE_COUNT.get(), 0);
        assert_eq!(ADAPTER_OVERLAP_COUNT.get(), 1);
        store.refresh().unwrap();
        assert_eq!(store.adapter_warnings().len(), 1);
        assert_eq!(ADAPTER_COMPILE_COUNT.get(), 0);
        assert_eq!(ADAPTER_OVERLAP_COUNT.get(), 1);
    }

    #[test]
    fn existing_adapter_id_template_cannot_change_before_migrations_exist() {
        let current = adapter_config(vec![adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/{id}",
        )]);
        let next = adapter_config(vec![adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.ticket:{host}/{id}",
        )]);
        validate(&next).unwrap();
        let error = validate_adapter_transition(&current, &next)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot change until artifact migrations"),
            "{error}"
        );
        validate_adapter_transition(&current, &current).unwrap();
    }

    #[test]
    fn id_transition_matches_the_id_among_same_type_siblings() {
        let first = adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/{id}",
        );
        let second = adapter(
            "https://work.example",
            "work.issue",
            "/tickets/{id:int}",
            "work.issue:{host}/ticket/{id}",
        );
        let current = adapter_config(vec![first.clone(), second.clone()]);
        validate(&current).unwrap();

        // Adapter order is policy precedence, not adapter identity. Reordering
        // two same-type adapters does not change either durable ID contract.
        let reordered = adapter_config(vec![second, first]);
        validate(&reordered).unwrap();
        validate_adapter_transition(&current, &reordered).unwrap();
    }

    #[test]
    fn removing_one_of_two_same_type_adapters_keeps_the_survivor() {
        let survivor = adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/{id}",
        );
        let removed = adapter(
            "https://work.example",
            "work.issue",
            "/tickets/{id:int}",
            "work.issue:{host}/ticket/{id}",
        );
        let current = adapter_config(vec![survivor.clone(), removed]);
        let next = adapter_config(vec![survivor.clone()]);
        validate(&current).unwrap();
        validate(&next).unwrap();

        validate_adapter_transition(&current, &next).unwrap();

        let changed = adapter_config(vec![
            survivor,
            adapter(
                "https://work.example",
                "work.issue",
                "/tickets/{id:int}",
                "work.issue:{host}/changed/{id}",
            ),
        ]);
        validate(&changed).unwrap();
        assert!(validate_adapter_transition(&current, &changed).is_err());
    }

    #[test]
    fn removed_adapter_cannot_return_with_a_different_id_template() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut store = ConfigStore::for_test(path.clone());
        let original = adapter_config(vec![adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/{id}",
        )]);
        let revision = store
            .update(original.clone(), MISSING_REVISION)
            .unwrap()
            .unwrap();
        let remapped = adapter_config(vec![adapter(
            "https://work.example",
            "work.ticket",
            "/issues/{id:int}",
            "work.ticket:{host}/{id}",
        )]);
        let direct_error = store
            .update(remapped.clone(), &revision)
            .unwrap_err()
            .to_string();
        assert!(
            direct_error.contains("cannot change until artifact migrations"),
            "{direct_error}"
        );
        let type_only = adapter_config(vec![adapter(
            "https://work.example",
            "work.ticket",
            "/issues/{id:int}",
            "work.issue:{host}/{id}",
        )]);
        let type_error = store.update(type_only, &revision).unwrap_err().to_string();
        assert!(
            type_error.contains("cannot change until artifact migrations"),
            "{type_error}"
        );

        let removed = adapter_config(Vec::new());
        let revision = store.update(removed, &revision).unwrap().unwrap();
        assert!(
            fs::read_to_string(&store.path)
                .unwrap()
                .contains("[[adapter_id_history]]")
        );
        drop(store);
        let mut restarted = loaded(&path);

        let error = restarted
            .update(remapped, &revision)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot change until artifact migrations"),
            "{error}"
        );

        let disjoint = adapter_config(vec![adapter(
            "https://work.example",
            "work.ticket",
            "/tickets/{id:int}",
            "work.ticket:{host}/{id}",
        )]);
        let revision = restarted.update(disjoint, &revision).unwrap().unwrap();
        restarted.update(original, &revision).unwrap().unwrap();
    }

    #[test]
    fn recognition_edits_do_not_release_old_routes_for_a_new_id() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut store = ConfigStore::for_test(path);
        let original = adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/{id}",
        );
        let moved = adapter(
            "https://work.example",
            "work.issue",
            "/tickets/{id:int}",
            "work.issue:{host}/{id}",
        );
        let revision = store
            .update(adapter_config(vec![original.clone()]), MISSING_REVISION)
            .unwrap()
            .unwrap();
        let revision = store
            .update(adapter_config(vec![moved.clone()]), &revision)
            .unwrap()
            .unwrap();

        let remapped = adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/changed/{id}",
        );
        let error = store
            .update(adapter_config(vec![moved, remapped]), &revision)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot change until artifact migrations"),
            "{error}"
        );
    }

    #[test]
    fn disjoint_adapter_reusing_an_id_does_not_erase_route_history() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut store = ConfigStore::for_test(path);
        let original = adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/{id}",
        );
        let disjoint = adapter(
            "https://work.example",
            "work.issue",
            "/tickets/{id:int}",
            "work.issue:{host}/{id}",
        );
        let revision = store
            .update(adapter_config(vec![original]), MISSING_REVISION)
            .unwrap()
            .unwrap();
        let revision = store
            .update(adapter_config(Vec::new()), &revision)
            .unwrap()
            .unwrap();
        let revision = store
            .update(adapter_config(vec![disjoint]), &revision)
            .unwrap()
            .unwrap();
        let revision = store
            .update(adapter_config(Vec::new()), &revision)
            .unwrap()
            .unwrap();

        let remapped = adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/changed/{id}",
        );
        let error = store
            .update(adapter_config(vec![remapped]), &revision)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot change until artifact migrations"),
            "{error}"
        );
    }

    #[test]
    fn unsupported_adapter_format_is_preserved_and_only_suspends_adapter_policy() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write(
            &path,
            folder.path(),
            "",
            "capture_enabled = false\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            "[adapters]\nformat = 2\n\n[[artifacts]]\ntype = \"work.issue\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/issues/{id:int}\" }]\nid = \"work.issue:{host}/{id}\"\n",
        );
        let before = fs::read(&path).unwrap();
        let mut store = loaded(&path);
        assert_eq!(store.config_issue(), None);
        assert!(
            store.configured(),
            "notes remain configured for repair and use"
        );
        assert_eq!(store.snapshot().adapters.format, 2);
        assert_eq!(store.snapshot().adapters.artifacts.len(), 1);
        assert!(
            store
                .adapter_issue()
                .is_some_and(|issue| issue.contains("unsupported_format"))
        );
        assert!(store.adapter_warnings().is_empty());
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "loading must not rewrite config"
        );
    }

    #[test]
    fn unknown_adapter_table_keys_only_suspend_adapter_policy() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write(
            &path,
            folder.path(),
            "",
            "capture_enabled = false\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            "[adapters]\nformat = 1\nfuture_mode = \"semantic\"\n",
        );
        let before = fs::read(&path).unwrap();
        let mut store = loaded(&path);
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "loading must not rewrite config"
        );

        assert_eq!(store.config_issue(), None);
        assert!(store.configured(), "notes remain configured and usable");
        assert!(
            store
                .adapter_issue()
                .is_some_and(|issue| issue.contains("future_mode"))
        );

        let revision = store.revision().to_owned();
        let next = ConfigSnapshot {
            strip_params: vec!["utm_*".into()],
            ..store.snapshot().clone()
        };
        store.update(next, &revision).unwrap().unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("future_mode = \"semantic\""), "{after}");

        let mut reloaded = loaded(&path);
        let revision = reloaded.revision().to_owned();
        let reset = reloaded.snapshot().clone();
        reloaded.update(reset, &revision).unwrap().unwrap();
        let repaired = fs::read_to_string(&path).unwrap();
        assert!(!repaired.contains("future_mode"), "{repaired}");
        assert_eq!(reloaded.adapter_issue(), None);
    }

    #[test]
    fn unknown_adapter_table_keys_refuse_partial_adapter_replacement() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write(
            &path,
            folder.path(),
            "",
            "capture_enabled = false\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            "[adapters]\nformat = 1\nfuture_mode = \"semantic\"\n\n[[artifacts]]\ntype = \"work.issue\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/issues/{id:int}\" }]\nid = \"work.issue:{host}/{id}\"\n",
        );
        let before = fs::read(&path).unwrap();
        let mut store = loaded(&path);
        let revision = store.revision().to_owned();
        let mut partial_replacement = store.snapshot().clone();
        partial_replacement.adapters.artifacts[0]["match"] =
            json!([{ "path": "/tickets/{id:int}" }]);

        let error = store
            .update(partial_replacement, &revision)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("resetting the whole adapter document"),
            "{error}"
        );
        assert_eq!(fs::read(&path).unwrap(), before);

        let reset = ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts: Vec::new(),
            },
            ..store.snapshot().clone()
        };
        store.update(reset, &revision).unwrap().unwrap();
        let repaired = fs::read_to_string(&path).unwrap();
        assert!(!repaired.contains("future_mode"), "{repaired}");
        assert!(!repaired.contains("[[artifacts]]"), "{repaired}");
        assert_eq!(store.adapter_issue(), None);
    }

    #[test]
    fn unrelated_update_preserves_then_repairs_an_unsupported_adapter_document() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write(
            &path,
            folder.path(),
            "",
            "capture_enabled = false\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            "[adapters]\nformat = 2\n\n[[artifacts]]\ntype = \"work.issue\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/issues/{id:int}\" }]\nid = \"work.issue:{host}/{id}\"\n",
        );
        let mut store = loaded(&path);
        let revision = store.revision().to_owned();
        let next = ConfigSnapshot {
            strip_params: vec!["utm_*".into()],
            ..store.snapshot().clone()
        };

        store.update(next, &revision).unwrap().unwrap();

        let mut reloaded = loaded(&path);
        assert_eq!(reloaded.snapshot().adapters.format, 2);
        assert_eq!(reloaded.snapshot().strip_params, ["utm_*"]);
        assert!(reloaded.adapter_issue().is_some());

        let revision = reloaded.revision().to_owned();
        let repaired = ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts: Vec::new(),
            },
            ..reloaded.snapshot().clone()
        };
        reloaded.update(repaired, &revision).unwrap().unwrap();

        let repaired = loaded(&path);
        assert_eq!(repaired.snapshot().adapters.format, 1);
        assert!(repaired.snapshot().adapters.artifacts.is_empty());
        assert_eq!(repaired.adapter_issue(), None);
    }

    #[test]
    fn resetting_an_invalid_document_preserves_its_known_id_contracts() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut store = ConfigStore::for_test(path.clone());
        let original = adapter_config(vec![adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/{id}",
        )]);
        store.update(original, MISSING_REVISION).unwrap().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replacen("format = 1", "format = 2", 1)).unwrap();
        store.refresh().unwrap();
        assert!(store.adapter_issue().is_some());

        let reset = ConfigSnapshot {
            adapters: AdapterDocument {
                format: 1,
                artifacts: Vec::new(),
            },
            ..store.snapshot().clone()
        };
        let revision = store.revision().to_owned();
        let revision = store.update(reset, &revision).unwrap().unwrap();
        let remapped = adapter_config(vec![adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/changed/{id}",
        )]);

        let error = store.update(remapped, &revision).unwrap_err().to_string();
        assert!(
            error.contains("cannot change until artifact migrations"),
            "{error}"
        );
    }

    #[test]
    fn adapter_document_round_trips_as_adapters_header_and_artifact_tables() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        let mut store = ConfigStore::for_test(path.clone());
        let mut config = adapter_config(vec![adapter(
            "https://work.example",
            "work.issue",
            "/issues/{id:int}",
            "work.issue:{host}/{id}",
        )]);
        config.storage = Some(StorageConfig {
            root: root.to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "logs".into(),
            pages_dir: "notes".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: Some("summaries".into()),
            knowledge_base_dir: None,
        });
        let identity = selected_root_identity(&root).unwrap();
        store
            .update_with_root_identity(config.clone(), MISSING_REVISION, Some(&identity))
            .unwrap()
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[adapters]\nformat = 1"), "{text}");
        assert!(text.contains("[[artifacts]]"), "{text}");
        let reloaded = loaded(&path);
        assert_eq!(reloaded.snapshot(), &config);
        assert_eq!(reloaded.adapter_issue(), None);
    }

    #[test]
    fn v5_config_with_retired_privacy_keys_loads_and_drops_them_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let root = dir.path().join("notes");
        fs::create_dir(&root).unwrap();
        fixtures::write(
            &path,
            &root,
            "",
            "agent_denylist = [\"bank.example\"]\nagent_denylist_confirmed = true\nlog_incognito = true",
            "",
        );
        let mut store = loaded(&path);
        assert_eq!(store.config_issue(), None);
        assert!(store.configured());
        assert_eq!(backup_count(dir.path()), 0);
        let rev = store.revision().to_owned();
        store
            .update(store.snapshot().clone(), &rev)
            .unwrap()
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("agent_denylist") && !text.contains("log_incognito"));
    }

    #[test]
    fn v5_sites_key_is_accepted_and_dropped_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let root = dir.path().join("notes");
        fs::create_dir(&root).unwrap();
        fixtures::write(
            &path,
            &root,
            "",
            "capture_enabled = true\nsites = [{ origin = \"https://work.example\", path_prefix = \"/allowed\" }]",
            "",
        );
        let mut store = loaded(&path);
        assert_eq!(store.config_issue(), None);
        assert!(store.configured());
        assert!(store.snapshot().capture_enabled);
        assert_eq!(backup_count(dir.path()), 0);
        let rev = store.revision().to_owned();
        store
            .update(store.snapshot().clone(), &rev)
            .unwrap()
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("sites"), "{text}");
        assert!(!text.contains("path_prefix"), "{text}");
    }

    #[test]
    fn capture_enabled_needs_only_a_notes_folder() {
        let dir = tempfile::tempdir().unwrap();
        let storage = StorageConfig {
            root: dir.path().to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "logs".into(),
            pages_dir: "notes".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: Some("summaries".into()),
            knowledge_base_dir: None,
        };
        let enabled = ConfigSnapshot {
            storage: Some(storage),
            capture_enabled: true,
            ..empty_config()
        };
        validate_base(&enabled).unwrap();
        validate(&enabled).unwrap();
        let no_folder = ConfigSnapshot {
            capture_enabled: true,
            ..empty_config()
        };
        let error = validate_base(&no_folder).unwrap_err().to_string();
        assert!(error.contains("home folder"), "{error}");
    }

    #[test]
    fn capture_origins_are_the_distinct_adapter_origins() {
        let config = adapter_config(vec![
            adapter(
                "https://work.example",
                "work.issue",
                "/issues/{id:int}",
                "work.issue:{host}/{id}",
            ),
            adapter(
                "https://work.example",
                "work.page",
                "/pages/{id:int}",
                "work.page:{host}/{id}",
            ),
            adapter(
                "https://other.example",
                "other.issue",
                "/issues/{id:int}",
                "other.issue:{host}/{id}",
            ),
        ]);
        let mut origins = capture_origins(&config);
        origins.sort();
        assert_eq!(
            origins,
            vec![
                "https://other.example".to_owned(),
                "https://work.example".to_owned()
            ]
        );
        assert!(capture_origins(&empty_config()).is_empty());
    }

    #[test]
    fn invalid_summarizer_table_disables_ai_but_keeps_capture() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write(
            &path,
            folder.path(),
            "summaries_dir = \"summaries\"",
            "",
            "[summarizer]\nharness = \"claude\"\nbinary = \"claude\"\ntimeout_secs = 600\nprobed_at = \"2026-10-27T10:00:00Z\"\n",
        );
        let mut store = loaded(&path);
        assert_eq!(store.config_issue(), None);
        assert!(store.configured());
        assert_eq!(store.snapshot().agent, None);
        let status = store.agent_status();
        assert_eq!(status.state, AgentState::HarnessProblem);
        assert_eq!(status.message.as_deref(), Some(AGENT_ENTRY_INVALID));

        // Saving other settings keeps the bad entry on disk for the user.
        let next = store.snapshot().clone();
        let revision = store.revision().to_owned();
        store.update(next, &revision).unwrap().unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("binary = \"claude\""));
        assert_eq!(
            loaded(&path).agent_status().message.as_deref(),
            Some(AGENT_ENTRY_INVALID)
        );
    }

    #[test]
    fn rejects_generated_content_directories_overlapping_pages() {
        let folder = tempfile::tempdir().unwrap();
        let mut storage = StorageConfig {
            root: folder.path().to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: "pages".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: Some("pages/summaries".into()),
            knowledge_base_dir: None,
        };
        assert!(validate_storage(&storage).is_err());
        storage.summaries_dir = Some("log".into());
        assert!(validate_storage(&storage).is_err());
        storage.summaries_dir = Some("../summaries".into());
        assert!(validate_storage(&storage).is_err());
        storage.summaries_dir = Some("summaries".into());
        validate_storage(&storage).unwrap();
        storage.artifacts_dir = "pages/artifacts".into();
        assert!(validate_storage(&storage).is_err());
    }

    #[test]
    fn summaries_overlap_is_case_insensitive() {
        let folder = tempfile::tempdir().unwrap();
        let storage = StorageConfig {
            root: folder.path().to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: "pages".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: Some("Pages".into()),
            knowledge_base_dir: None,
        };
        assert!(validate_storage(&storage).is_err());
    }

    #[test]
    fn overlap_uses_unicode_case_folding_and_normalization() {
        // A case-insensitive APFS volume treats final and medial sigma as one
        // letter, and composed and decomposed accents as one name.
        let folder = tempfile::tempdir().unwrap();
        let mut storage = StorageConfig {
            root: folder.path().to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: "ας".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: Some("ασ".into()),
            knowledge_base_dir: None,
        };
        assert!(validate_storage(&storage).is_err());
        storage.pages_dir = "pages".into();
        storage.log_dir = "Stra\u{df}e".into();
        storage.summaries_dir = Some("STRASSE/summaries".into());
        assert!(validate_storage(&storage).is_err());
        storage.log_dir = "caf\u{e9}".into();
        storage.summaries_dir = Some("CAFE\u{301}".into());
        assert!(validate_storage(&storage).is_err());
        storage.summaries_dir = Some("cafe".into());
        validate_storage(&storage).unwrap();
    }

    fn claude(binary: &str) -> AgentConfig {
        AgentConfig {
            harness: HarnessId::Claude,
            binary: binary.to_owned(),
            version: Some("2.1.284".into()),
            timeout_secs: DEFAULT_SUMMARY_TIMEOUT_SECS,
            probed_at: "2026-10-27T10:00:00Z".into(),
        }
    }

    fn absent_claude() -> AgentConfig {
        claude(&fixtures::absent_binary("claude"))
    }

    fn assert_ready(store: &ConfigStore, version: Option<&str>) {
        let status = store.agent_status();
        assert_eq!(status.state, AgentState::Ready, "{status:?}");
        assert_eq!(status.harness_version.as_deref(), version);
        assert_eq!(status.message, None);
    }

    #[test]
    fn v5_agent_and_harness_record_tables_load_as_not_set_up() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let root = folder.path().join("notes");
        fs::create_dir(&root).unwrap();
        fixtures::write(
            &path,
            &root,
            "summaries_dir = \"summaries\"",
            "",
            fixtures::V5_HARNESS_TABLES,
        );
        let mut store = loaded(&path);
        assert_eq!(store.config_issue(), None);
        assert!(store.configured());
        assert_eq!(store.snapshot().agent, None);
        assert_eq!(
            refusal(&store),
            (AgentState::NotSetUp, SET_UP_HARNESS.into())
        );
        assert_eq!(backup_count(folder.path()), 0);

        let revision = store.revision().to_owned();
        store
            .update(store.snapshot().clone(), &revision)
            .unwrap()
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("[agent"), "{text}");
        assert!(!text.contains("harness_record"), "{text}");
        assert!(!text.contains("[summarizer"), "{text}");
        assert_eq!(backup_count(folder.path()), 0);
        assert_eq!(loaded(&path).agent_status().state, AgentState::NotSetUp);
    }

    #[test]
    fn summarizer_table_round_trips_as_wire_agent() {
        let folder = tempfile::tempdir().unwrap();
        let (path, _) = fixtures::ready(folder.path());
        let mut store = loaded(&path);
        assert_eq!(store.config_issue(), None);
        assert_eq!(store.snapshot().agent, Some(absent_claude()));
        assert_ready(&store, Some("2.1.284"));

        // An unrelated save carries the table forward unchanged.
        let mut next = store.snapshot().clone();
        next.strip_params.push("campaign".into());
        let revision = store.revision().to_owned();
        store.update(next, &revision).unwrap().unwrap();
        let reloaded = loaded(&path);
        assert_eq!(reloaded.snapshot().agent, Some(absent_claude()));
        assert_ready(&reloaded, Some("2.1.284"));

        // A version setup could not read is stored as absent.
        let commit = HarnessCommit {
            summarizer: StoredSummarizer::from(&AgentConfig {
                version: None,
                harness: HarnessId::Codex,
                ..absent_claude()
            }),
        };
        let mut store = reloaded;
        let next = ConfigSnapshot {
            agent: Some(commit.summarizer.agent()),
            ..store.snapshot().clone()
        };
        let revision = store.revision().to_owned();
        store
            .update_with_grants(next, &revision, None, None, Some(commit.clone()))
            .unwrap()
            .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[summarizer]"), "{text}");
        assert!(text.contains("harness = \"codex\""), "{text}");
        assert!(!text.contains("version"), "{text}");
        let reloaded = loaded(&path);
        assert_eq!(reloaded.snapshot().agent, Some(commit.summarizer.agent()));
        assert_ready(&reloaded, None);
    }

    #[test]
    fn harness_commit_must_match_the_snapshot() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write(
            &path,
            folder.path(),
            "summaries_dir = \"summaries\"",
            "",
            "",
        );
        let mut store = loaded(&path);
        let revision = store.revision().to_owned();
        let before = fs::read(&path).unwrap();
        let next = ConfigSnapshot {
            agent: Some(absent_claude()),
            ..store.snapshot().clone()
        };
        let other = HarnessCommit {
            summarizer: StoredSummarizer::from(&AgentConfig {
                timeout_secs: 900,
                ..absent_claude()
            }),
        };
        assert!(
            store
                .update_with_grants(next, &revision, None, None, Some(other))
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn update_without_commit_cannot_set_a_harness() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write(
            &path,
            folder.path(),
            "summaries_dir = \"summaries\"",
            "",
            "",
        );
        let mut store = loaded(&path);
        let revision = store.revision().to_owned();
        let before = fs::read(&path).unwrap();
        let next = ConfigSnapshot {
            agent: Some(absent_claude()),
            ..store.snapshot().clone()
        };
        assert!(store.update(next, &revision).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);

        // Nor change the one setup committed.
        let ready = folder.path().join("ready");
        fs::create_dir(&ready).unwrap();
        let (path, _) = fixtures::ready(&ready);
        let mut store = loaded(&path);
        let revision = store.revision().to_owned();
        let next = ConfigSnapshot {
            agent: Some(AgentConfig {
                timeout_secs: 900,
                ..absent_claude()
            }),
            ..store.snapshot().clone()
        };
        assert!(store.update(next, &revision).is_err());
    }

    #[test]
    fn removing_harness_leaves_ai_not_set_up() {
        let folder = tempfile::tempdir().unwrap();
        let (path, _) = fixtures::ready(folder.path());
        let mut store = loaded(&path);
        let next = ConfigSnapshot {
            agent: None,
            ..store.snapshot().clone()
        };
        let revision = store.revision().to_owned();
        store.update(next, &revision).unwrap().unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(!contents.contains("[summarizer"));
        assert_eq!(
            refusal(&loaded(&path)),
            (AgentState::NotSetUp, SET_UP_HARNESS.into())
        );
    }

    #[test]
    fn summarizer_entry_needs_an_absolute_binary_bounded_timeout_and_probe_time() {
        validate_summarizer(&absent_claude()).unwrap();
        for bad in [
            AgentConfig {
                binary: "claude".into(),
                ..absent_claude()
            },
            AgentConfig {
                binary: format!("{}\u{202e}", fixtures::absent_binary("claude")),
                ..absent_claude()
            },
            AgentConfig {
                version: Some("2.1\n284".into()),
                ..absent_claude()
            },
            AgentConfig {
                timeout_secs: 59,
                ..absent_claude()
            },
            AgentConfig {
                timeout_secs: 3601,
                ..absent_claude()
            },
            AgentConfig {
                probed_at: "yesterday".into(),
                ..absent_claude()
            },
        ] {
            assert!(validate_summarizer(&bad).is_err(), "{bad:?} was accepted");
        }
        for timeout_secs in [60, 3600] {
            validate_summarizer(&AgentConfig {
                timeout_secs,
                ..absent_claude()
            })
            .unwrap();
        }
    }

    #[test]
    fn v5_unconfirmed_denylist_key_no_longer_blocks_a_committed_harness() {
        let folder = tempfile::tempdir().unwrap();
        let (path, root) = fixtures::ready(folder.path());
        fixtures::write(
            &path,
            &root,
            "summaries_dir = \"summaries\"",
            "agent_denylist_confirmed = false",
            &fixtures::agent_table(Path::new(&fixtures::absent_binary("claude"))),
        );
        let store = loaded(&path);
        assert_eq!(store.config_issue(), None);
        assert!(store.snapshot().agent.is_some());
        assert_ready(&store, Some("2.1.284"));
    }

    #[test]
    fn agent_status_refuses_without_harness_or_before_the_layout_moves() {
        let folder = tempfile::tempdir().unwrap();
        let (path, root) = fixtures::ready(folder.path());
        fixtures::write(&path, &root, "", "", "");
        assert_eq!(
            refusal(&loaded(&path)),
            (AgentState::NotSetUp, SET_UP_HARNESS.into())
        );

        // An M1 file has no summaries folder. That is an older layout, which
        // is never used until it has been moved to the fixed one.
        fixtures::write(
            &path,
            &root,
            "summaries_dir = \"my-summaries\"",
            "",
            &fixtures::agent_table(Path::new(&fixtures::absent_binary("claude"))),
        );
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("summaries_dir = \"my-summaries\"\n", ""),
        )
        .unwrap();
        let mut store = loaded(&path);
        assert_eq!(
            store.snapshot().storage.as_ref().unwrap().summaries_dir,
            None
        );
        assert_eq!(
            refusal(&store),
            (
                AgentState::NotSetUp,
                "configuration is unavailable or needs repair".into()
            )
        );
        store.migrate_layout().unwrap();
        assert_ready(&store, Some("2.1.284"));
    }

    #[test]
    fn capture_remains_off_until_site_authorization_exists() {
        let config = ConfigSnapshot {
            storage: None,
            capture_enabled: true,
            ..empty_config()
        };
        assert!(validate(&config).is_err());
    }

    #[test]
    fn rejects_overlapping_content_locations() {
        let folder = tempfile::tempdir().unwrap();
        let config = ConfigSnapshot {
            storage: Some(StorageConfig {
                root: folder.path().to_string_lossy().into_owned(),
                profile: "neutral".into(),
                log_dir: "notes".into(),
                pages_dir: "notes/pages".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: None,
                knowledge_base_dir: None,
            }),
            capture_enabled: false,
            ..empty_config()
        };
        assert!(validate(&config).is_err());
    }

    #[test]
    fn empty_config_round_trips_through_an_atomic_update() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut store = ConfigStore::for_test(path.clone());
        let revision = store
            .update(empty_config(), MISSING_REVISION)
            .unwrap()
            .unwrap();
        let contents = fs::read_to_string(path).unwrap();
        let restored: StoredConfig = toml::from_str(&contents).unwrap();
        assert_eq!(ConfigSnapshot::from(restored), empty_config());
        assert_eq!(revision, revision_for(contents.as_bytes()));
    }

    #[test]
    fn stale_revision_does_not_replace_config() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut first = ConfigStore::for_test(path.clone());
        let mut second = ConfigStore::for_test(path.clone());
        let committed = first
            .update(empty_config(), MISSING_REVISION)
            .unwrap()
            .unwrap();
        let before = fs::read(&path).unwrap();

        assert_eq!(
            second.update(empty_config(), MISSING_REVISION).unwrap(),
            None
        );
        assert_eq!(second.revision(), committed);
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn two_writers_can_commit_in_revision_order() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut first = ConfigStore::for_test(path.clone());
        let mut second = ConfigStore::for_test(path);
        let first_revision = first
            .update(empty_config(), MISSING_REVISION)
            .unwrap()
            .unwrap();
        assert_eq!(
            second.update(empty_config(), MISSING_REVISION).unwrap(),
            None
        );

        let next = ConfigSnapshot {
            storage: Some(StorageConfig {
                root: folder.path().to_string_lossy().into_owned(),
                profile: "neutral".into(),
                log_dir: "logs".into(),
                pages_dir: "notes".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: Some("summaries".into()),
                knowledge_base_dir: None,
            }),
            capture_enabled: false,
            ..empty_config()
        };
        let root_identity = selected_root_identity(folder.path()).unwrap();
        let second_revision = second
            .update_with_root_identity(next.clone(), &first_revision, Some(&root_identity))
            .unwrap()
            .unwrap();
        assert_ne!(first_revision, second_revision);
        first.refresh().unwrap();
        assert_eq!(first.snapshot(), &next);
        assert_eq!(first.revision(), second_revision);
    }

    #[test]
    fn legacy_same_path_requires_and_accepts_fresh_selection() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let next = ConfigSnapshot {
            storage: Some(StorageConfig {
                root: folder.path().to_string_lossy().into_owned(),
                profile: "neutral".into(),
                log_dir: "logs".into(),
                pages_dir: "notes".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: Some("summaries".into()),
                knowledge_base_dir: None,
            }),
            ..empty_config()
        };
        // Old M1 files recorded picker confirmation but not folder identity.
        fs::write(
            &path,
            toml::to_string(&StoredConfig::from_snapshot(
                &next,
                None,
                None,
                &AgentView::default(),
                Vec::new(),
                BTreeMap::new(),
            ))
            .unwrap(),
        )
        .unwrap();
        let mut store = ConfigStore::for_test(path);
        store.refresh().unwrap();
        assert!(store.config_issue().is_some());
        assert!(store.snapshot().storage.is_none());
        let revision = store.revision().to_owned();
        assert!(store.update(next.clone(), &revision).is_err());
        let identity = selected_root_identity(folder.path()).unwrap();
        store
            .update_with_root_identity(next.clone(), &revision, Some(&identity))
            .unwrap()
            .unwrap();
        store.refresh().unwrap();
        assert!(store.config_issue().is_none());
        assert_eq!(store.snapshot(), &next);
    }

    #[test]
    fn replacing_selected_directory_requires_reselection() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("selected");
        let moved = base.path().join("moved");
        fs::create_dir(&root).unwrap();
        let path = base.path().join("config.toml");
        let mut store = ConfigStore::for_test(path);
        let config = ConfigSnapshot {
            storage: Some(StorageConfig {
                root: root.to_string_lossy().into_owned(),
                profile: "neutral".into(),
                log_dir: "logs".into(),
                pages_dir: "notes".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: Some("summaries".into()),
                knowledge_base_dir: None,
            }),
            ..empty_config()
        };
        let identity = selected_root_identity(&root).unwrap();
        store
            .update_with_root_identity(config.clone(), MISSING_REVISION, Some(&identity))
            .unwrap()
            .unwrap();
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        store.refresh().unwrap();
        assert!(store.config_issue().is_some());
        assert!(!store.configured());
        // Only the folder needs reselection; the settings are kept for repair.
        assert_eq!(store.snapshot(), &config);
        let revision = store.revision().to_owned();
        // The stored identity no longer matches, so the same path alone is
        // not enough to authorize the replacement folder.
        assert!(store.update(config.clone(), &revision).is_err());
        let new_identity = selected_root_identity(&root).unwrap();
        assert_ne!(identity, new_identity);
        store
            .update_with_root_identity(config, &revision, Some(&new_identity))
            .unwrap()
            .unwrap();
        store.refresh().unwrap();
        assert!(store.config_issue().is_none());
        // The file was valid; only its identity was stale, so nothing to back up.
        assert_eq!(backup_count(base.path()), 0);
    }

    #[test]
    fn remounted_or_missing_folder_keeps_settings_without_a_backup() {
        // An external or network volume can disappear, or come back with a
        // new device number. Neither may wipe the saved settings.
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("selected");
        let away = base.path().join("away");
        fs::create_dir(&root).unwrap();
        let path = base.path().join("config.toml");
        let mut store = ConfigStore::for_test(path.clone());
        let config = ConfigSnapshot {
            storage: Some(StorageConfig {
                root: root.to_string_lossy().into_owned(),
                profile: "neutral".into(),
                log_dir: "logs".into(),
                pages_dir: "notes".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: Some("summaries".into()),
                knowledge_base_dir: None,
            }),
            capture_enabled: true,
            ..empty_config()
        };
        let identity = selected_root_identity(&root).unwrap();
        store
            .update_with_root_identity(config.clone(), MISSING_REVISION, Some(&identity))
            .unwrap()
            .unwrap();
        let saved = fs::read(&path).unwrap();

        // Unmounted: the folder is gone. Capture stops, settings stay.
        fs::rename(&root, &away).unwrap();
        store.refresh().unwrap();
        assert!(store.config_issue().is_some());
        assert!(!store.configured());
        assert_eq!(store.snapshot(), &config);
        assert_eq!(store.root_identity(), Some(identity.as_str()));

        // Remounted with the same identity: no repair is needed at all.
        fs::rename(&away, &root).unwrap();
        store.refresh().unwrap();
        assert!(store.config_issue().is_none());
        assert!(store.configured());
        assert_eq!(fs::read(&path).unwrap(), saved);
        assert_eq!(backup_count(base.path()), 0);
    }

    #[test]
    fn unreadable_config_degrades_to_an_issue_at_startup() {
        // A directory (or FIFO, or unreadable file) at the config path must
        // not stop the host: it serves invalid_config errors instead.
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        fs::create_dir(&path).unwrap();
        let mut store = ConfigStore::load_from(path);
        assert!(store.config_issue().is_some());
        assert!(!store.configured());
        assert_eq!(store.snapshot(), &empty_config());
        assert!(store.refresh().is_err());
        assert!(store.config_issue().is_some());
    }

    #[test]
    fn a_config_that_becomes_unreadable_turns_inert_on_refresh() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut store = ConfigStore::for_test(path.clone());
        store.refresh().unwrap();
        assert!(store.config_issue().is_none());
        fs::create_dir(&path).unwrap();
        assert!(store.refresh().is_err());
        assert!(store.config_issue().is_some());
        assert!(!store.configured());
        assert_eq!(store.snapshot(), &empty_config());
        assert_eq!(store.revision(), UNREADABLE_REVISION);
        fs::remove_dir(&path).unwrap();
        store.refresh().unwrap();
        assert!(store.config_issue().is_none());
    }

    #[test]
    fn oversized_config_past_the_hash_cap_can_be_repaired() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let oversized = vec![b'#'; MAX_HASHED_CONFIG_BYTES as usize + 10];
        fs::write(&path, &oversized).unwrap();
        let mut store = ConfigStore::for_test(path.clone());
        store.refresh().unwrap();
        assert!(store.config_issue().is_some());
        let revision = store.revision().to_owned();
        store
            .update(empty_config(), &revision)
            .unwrap()
            .expect("repair should commit");
        store.refresh().unwrap();
        assert!(store.config_issue().is_none());
        assert_eq!(backup_count(folder.path()), 1);
        let backup = fs::read_dir(folder.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|entry| {
                entry
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("config-invalid-")
            })
            .unwrap();
        assert_eq!(fs::read(backup).unwrap(), oversized);
    }

    fn backup_count(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config-invalid-")
            })
            .count()
    }

    #[test]
    fn oversized_json_is_rejected_before_persistence() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        let mut store = ConfigStore::for_test(path.clone());
        let large = ConfigSnapshot {
            storage: Some(StorageConfig {
                root: "x".repeat(MAX_CONFIG_BYTES),
                profile: "neutral".into(),
                log_dir: "logs".into(),
                pages_dir: "notes".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: Some("summaries".into()),
                knowledge_base_dir: None,
            }),
            capture_enabled: false,
            ..empty_config()
        };
        assert!(store.update(large, MISSING_REVISION).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn case_insensitive_overlap_uses_path_components() {
        let folder = tempfile::tempdir().unwrap();
        let storage = StorageConfig {
            root: folder.path().to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "Notes/Pages".into(),
            pages_dir: "notes".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: None,
            knowledge_base_dir: None,
        };
        assert!(validate_storage(&storage).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn backslash_paths_are_checked_as_components_on_windows() {
        let folder = tempfile::tempdir().unwrap();
        let storage = StorageConfig {
            root: folder.path().to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "Notes\\Pages".into(),
            pages_dir: "notes".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: None,
            knowledge_base_dir: None,
        };
        assert!(validate_storage(&storage).is_err());
    }

    #[test]
    fn startup_sweep_removes_only_stale_config_temporaries() {
        use crate::vault::tests::age_file;
        use std::time::Duration;

        let folder = tempfile::tempdir().unwrap();
        let old = STALE_TEMPORARY_AGE + Duration::from_secs(60);
        let stale = folder
            .path()
            .join(format!(".config-{}.tmp", Uuid::new_v4()));
        let fresh = folder
            .path()
            .join(format!(".config-{}.tmp", Uuid::new_v4()));
        let other = folder.path().join(".config-existing.tmp");
        let config = folder.path().join("config.toml");
        for path in [&stale, &fresh, &other, &config] {
            fs::write(path, "content").unwrap();
        }
        for path in [&stale, &other, &config] {
            age_file(path, old);
        }
        let store = ConfigStore::for_test(config.clone());
        store.sweep_stale_temporaries();
        assert!(!stale.exists());
        for path in [&fresh, &other, &config] {
            assert!(path.exists(), "{} was removed", path.display());
        }
    }

    /// `strip_params` entries that remove a query parameter an adapter rule
    /// reads (brwr-f4s). Stripping it would give distinct artifacts, such as
    /// a Jira board and every issue modal on it, one canonical URL.
    mod strip_params_readers {
        use super::*;

        const JIRA: &str = "[[artifacts]]\ntype = \"jira.issue\"\norigin = \"https://acme.atlassian.net\"\nmatch = [{ path = \"/browse/{key:jira_key}\" }, { path = \"/jira/{*}\", query = { selectedIssue = \"{key:jira_key}\" } }]\nid = \"jira:{host}/{key}\"\n";
        const SEARCH: &str = "[[artifacts]]\ntype = \"work.list\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/lists/{id:int}\", absent = [\"q\"] }]\nid = \"list:{host}/{id}\"\n";

        fn jira_adapter() -> serde_json::Value {
            json!({
                "type": "jira.issue",
                "origin": "https://acme.atlassian.net",
                "match": [
                    { "path": "/browse/{key:jira_key}" },
                    { "path": "/jira/{*}", "query": { "selectedIssue": "{key:jira_key}" } },
                ],
                "id": "jira:{host}/{key}",
            })
        }

        /// config.toml with `keys` at the top level and `adapters` after it.
        fn written(folder: &Path, keys: &str, adapters: &[&str]) -> PathBuf {
            let root = folder.join("notes");
            fs::create_dir_all(&root).unwrap();
            let path = folder.join("config.toml");
            fixtures::write(
                &path,
                &root,
                "",
                keys,
                &format!("[adapters]\nformat = 1\n\n{}", adapters.join("\n")),
            );
            path
        }

        fn refused(store: &mut ConfigStore, next: ConfigSnapshot) -> String {
            let path = store.path.clone();
            let before = fs::read(&path).unwrap();
            let revision = store.revision().to_owned();
            let error = match store.update(next, &revision) {
                Err(error) => error,
                Ok(result) => panic!("a conflicting save was accepted: {result:?}"),
            };
            assert!(
                error.downcast_ref::<StripParamsRefusal>().is_some(),
                "{error:#}"
            );
            assert_eq!(fs::read(&path).unwrap(), before, "a refusal wrote config");
            error.to_string()
        }

        fn with_strip(store: &ConfigStore, entry: &str) -> ConfigSnapshot {
            let mut next = store.snapshot().clone();
            next.strip_params.push(entry.into());
            next
        }

        #[test]
        fn adding_an_entry_an_adapter_reads_is_refused() {
            let folder = tempfile::tempdir().unwrap();
            let path = written(folder.path(), "", &[JIRA]);
            let mut store = loaded(&path);
            assert!(store.config_issue().is_none(), "{:?}", store.config_issue());
            let next = with_strip(&store, "selectedIssue");
            let message = refused(&mut store, next);
            assert!(message.contains("\"selectedIssue\""), "{message}");
            assert!(message.contains("jira.issue"), "{message}");
            assert!(message.contains("https://acme.atlassian.net"), "{message}");
        }

        #[test]
        fn prefix_and_case_variants_of_a_read_parameter_are_refused() {
            let folder = tempfile::tempdir().unwrap();
            let path = written(folder.path(), "", &[JIRA, SEARCH]);
            let mut store = loaded(&path);
            // Stripping compares keys ASCII case-insensitively, by prefix
            // for a trailing *.
            for entry in ["selected*", "SELECTEDISSUE", "selectedissue", "q", "Q*"] {
                let next = with_strip(&store, entry);
                let message = refused(&mut store, next);
                assert!(message.contains(&format!("{entry:?}")), "{message}");
            }
            // An absent key is read too: stripping it would merge the list
            // with its filtered views.
            let next = with_strip(&store, "q");
            assert!(refused(&mut store, next).contains("work.list"));
            // Entries no rule reads still save.
            for entry in ["campaign", "selectedIssues", "sel", "q_*"] {
                let next = with_strip(&store, entry);
                let revision = store.revision().to_owned();
                store.update(next, &revision).unwrap().unwrap();
            }
        }

        #[test]
        fn adding_an_adapter_that_reads_a_stripped_parameter_is_refused() {
            let folder = tempfile::tempdir().unwrap();
            let path = written(
                folder.path(),
                "strip_params = [\"utm_*\", \"selected*\"]",
                &[],
            );
            let mut store = loaded(&path);
            assert!(store.config_issue().is_none(), "{:?}", store.config_issue());
            let mut next = store.snapshot().clone();
            next.adapters.artifacts.push(jira_adapter());
            let message = refused(&mut store, next);
            assert!(message.contains("\"selectedIssue\""), "{message}");
            assert!(message.contains("\"selected*\""), "{message}");
            assert!(message.contains("jira.issue"), "{message}");

            // Adding a reader of that parameter to an existing adapter is
            // refused the same way.
            let path = written(
                folder.path(),
                "strip_params = [\"selectedIssue\"]",
                &[
                    "[[artifacts]]\ntype = \"jira.issue\"\norigin = \"https://acme.atlassian.net\"\nmatch = [{ path = \"/browse/{key:jira_key}\" }]\nid = \"jira:{host}/{key}\"\n",
                ],
            );
            let mut store = loaded(&path);
            let mut next = store.snapshot().clone();
            next.adapters.artifacts[0] = jira_adapter();
            refused(&mut store, next);
        }

        #[test]
        fn an_existing_conflict_still_loads_saves_and_is_reported() {
            let folder = tempfile::tempdir().unwrap();
            let path = written(
                folder.path(),
                "capture_enabled = true\nstrip_params = [\"utm_*\", \"selectedIssue\", \"fbclid\"]",
                &[JIRA],
            );
            let mut store = loaded(&path);
            assert!(store.config_issue().is_none(), "{:?}", store.config_issue());
            assert!(
                store.adapter_issue().is_none(),
                "{:?}",
                store.adapter_issue()
            );
            assert_eq!(store.capture_origins(), vec!["https://acme.atlassian.net"]);
            let conflicts = strip_param_conflicts(store.snapshot());
            assert_eq!(conflicts.len(), 1, "{conflicts:?}");
            assert_eq!(conflicts[0].entry, "selectedIssue");
            assert_eq!(conflicts[0].param, "selectedIssue");
            assert_eq!(conflicts[0].artifact_type, "jira.issue");

            // A new rule reading the same parameter is a new conflict, even
            // beside an existing one on the same adapter.
            let mut next = store.snapshot().clone();
            next.adapters.artifacts[0]["match"]
                .as_array_mut()
                .unwrap()
                .push(json!({ "path": "/projects/{*}", "query": { "selectedIssue": "{key:jira_key}" } }));
            let message = refused(&mut store, next);
            assert!(message.contains("\"selectedIssue\""), "{message}");

            // Unrelated saves keep working: the capture switch, a new
            // unrelated entry, reordering, and an edit to the conflicting
            // adapter that reads nothing new.
            let mut next = store.snapshot().clone();
            next.capture_enabled = false;
            next.strip_params.push("campaign".into());
            next.strip_params.swap(0, 1);
            next.adapters.artifacts[0]["title"] = json!("[{key:jira_key}] {title} - Jira");
            let revision = store.revision().to_owned();
            store.update(next, &revision).unwrap().unwrap();
            let store = loaded(&path);
            assert_eq!(
                store.snapshot().strip_params,
                ["selectedIssue", "utm_*", "fbclid", "campaign"]
            );
            assert_eq!(strip_param_conflicts(store.snapshot()).len(), 1);
        }

        #[test]
        fn stopping_stripping_removes_only_that_entry_and_clears_the_conflict() {
            let folder = tempfile::tempdir().unwrap();
            let path = written(
                folder.path(),
                "strip_params = [\"utm_*\", \"selectedIssue\", \"fbclid\"]",
                &[JIRA],
            );
            let mut store = loaded(&path);
            let mut next = store.snapshot().clone();
            next.strip_params.retain(|entry| entry != "selectedIssue");
            let revision = store.revision().to_owned();
            store.update(next, &revision).unwrap().unwrap();
            let store = loaded(&path);
            assert_eq!(store.snapshot().strip_params, ["utm_*", "fbclid"]);
            assert_eq!(store.snapshot().adapters.artifacts.len(), 1);
            assert!(strip_param_conflicts(store.snapshot()).is_empty());
        }

        #[test]
        fn retired_contracts_count_as_readers() {
            // Removed through settings: kept in [[adapter_id_history]].
            let folder = tempfile::tempdir().unwrap();
            let path = written(folder.path(), "", &[JIRA]);
            let mut store = loaded(&path);
            let mut next = store.snapshot().clone();
            next.adapters.artifacts.clear();
            next.capture_enabled = false;
            let revision = store.revision().to_owned();
            store.update(next, &revision).unwrap().unwrap();
            assert_eq!(store.retired_adapter_contracts.len(), 1);
            let mut store = loaded(&path);
            let next = with_strip(&store, "selectedIssue");
            let message = refused(&mut store, next);
            assert!(message.contains("removed adapter"), "{message}");
            assert!(message.contains("jira.issue"), "{message}");

            // Removed from config.toml by hand: kept in the adopted state.
            let folder = tempfile::tempdir().unwrap();
            let path = written(folder.path(), "", &[JIRA, SEARCH]);
            let mut store = ConfigStore::for_test(path.clone());
            let lock = store.lock_current().unwrap();
            store.refresh().unwrap();
            drop(lock);
            written(folder.path(), "", &[SEARCH]);
            let mut store = loaded(&path);
            assert!(store.retired_adapter_contracts.is_empty());
            let next = with_strip(&store, "selected*");
            let message = refused(&mut store, next);
            assert!(message.contains("removed adapter"), "{message}");
        }

        #[test]
        fn removing_a_conflicting_adapter_is_not_a_new_conflict() {
            let folder = tempfile::tempdir().unwrap();
            let path = written(folder.path(), "strip_params = [\"selectedIssue\"]", &[JIRA]);
            let mut store = loaded(&path);
            let mut next = store.snapshot().clone();
            next.adapters.artifacts.clear();
            next.capture_enabled = false;
            let revision = store.revision().to_owned();
            store.update(next, &revision).unwrap().unwrap();
            // Restoring the same contract from history reads nothing new.
            let mut store = loaded(&path);
            let mut next = store.snapshot().clone();
            next.adapters.artifacts.push(jira_adapter());
            let revision = store.revision().to_owned();
            store.update(next, &revision).unwrap().unwrap();
        }
    }

    #[test]
    fn temporary_name_collision_preserves_existing_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(".config-existing.tmp");
        fs::write(&path, "existing").unwrap();
        assert!(create_temporary(&path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "existing");
    }
}

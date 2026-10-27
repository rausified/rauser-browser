//! Short-lived, process-local grants for folder selection. Only the native
//! host can mint these grants; the extension cannot authorize a path or
//! redirect writes by sending an arbitrary config. Each grant names the
//! setting it was chosen for, so a notes-folder selection cannot set the
//! knowledge base folder or the reverse. Other config changes need only the
//! revision check, and only harness setup sets a harness entry.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use brauser_protocol::{ConfigSnapshot, FolderPurpose};
use uuid::Uuid;

use crate::brand::APP_NAME;
use crate::config;
use crate::dialog;
use crate::vault::selected_root_identity;

const TOKEN_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING_GRANTS: usize = 32;

pub struct FolderSelection {
    pub path: String,
    pub picker_token: String,
}

struct PickerGrant {
    purpose: FolderPurpose,
    root: String,
    identity: String,
    revision: String,
    expires_at: Instant,
}

/// What `authorize_update` checked.
#[derive(Debug, Default)]
pub struct Authorized {
    pub selected_identity: Option<String>,
    pub knowledge_base_identity: Option<String>,
}

#[derive(Default)]
pub struct ConsentAuthority {
    picker_grants: HashMap<String, PickerGrant>,
}

impl ConsentAuthority {
    pub fn new() -> Self {
        Self::default()
    }

    /// Blocks this connection until the user closes the picker, which runs
    /// in a child process (see `dialog`).
    pub fn choose_folder(
        &mut self,
        revision: &str,
        purpose: FolderPurpose,
    ) -> Result<Option<FolderSelection>> {
        let noun = folder_noun(purpose);
        let Some(selected) = dialog::pick_folder(&format!("Choose {APP_NAME} {noun}"))? else {
            return Ok(None);
        };
        let canonical = fs::canonicalize(&selected)
            .with_context(|| format!("resolving selected folder {}", selected.display()))?;
        if !canonical.is_dir() {
            bail!("selected {noun} is not a directory");
        }
        let root = canonical
            .to_str()
            .with_context(|| format!("selected {noun} path cannot be represented as UTF-8"))?
            .to_owned();
        let identity = selected_root_identity(Path::new(&root))?;
        let picker_token = self.mint_picker(purpose, root.clone(), identity, revision);
        Ok(Some(FolderSelection {
            path: root,
            picker_token,
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
        knowledge_base_token: Option<&str>,
    ) -> Result<Authorized> {
        config::validate_update(current, next)?;
        let selected_identity = self.check_picker(current, revision, next, picker_token)?;
        let knowledge_base_identity =
            self.check_knowledge_base(current, revision, next, knowledge_base_token)?;
        check_agent_fields(current, next)?;
        Ok(Authorized {
            selected_identity,
            knowledge_base_identity,
        })
    }

    /// Call only after ConfigStore::update returns a committed revision.
    pub fn consume_update(
        &mut self,
        _current: &ConfigSnapshot,
        _revision: &str,
        _next: &ConfigSnapshot,
        picker_token: Option<&str>,
        knowledge_base_token: Option<&str>,
    ) {
        for token in [picker_token, knowledge_base_token].into_iter().flatten() {
            self.picker_grants.remove(token);
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
        self.check_grant(FolderPurpose::NotesFolder, token, revision, root)
            .map(Some)
    }

    /// The knowledge base folder needs a grant minted for that purpose,
    /// unless it is unset (it follows the notes folder) or unchanged.
    pub(crate) fn check_knowledge_base(
        &self,
        current: &ConfigSnapshot,
        revision: &str,
        next: &ConfigSnapshot,
        token: Option<&str>,
    ) -> Result<Option<String>> {
        let knowledge_base = |config: &ConfigSnapshot| {
            config
                .storage
                .as_ref()
                .and_then(|storage| storage.knowledge_base_dir.as_deref())
                .map(str::to_owned)
        };
        let old = knowledge_base(current);
        let Some(new) = knowledge_base(next) else {
            return Ok(None);
        };
        if Some(&new) == old.as_ref() && token.is_none() {
            return Ok(None);
        }
        let token = token.context("a native selection of the knowledge base folder is required")?;
        self.check_grant(FolderPurpose::KnowledgeBase, token, revision, &new)
            .map(Some)
    }

    fn check_grant(
        &self,
        purpose: FolderPurpose,
        token: &str,
        revision: &str,
        path: &str,
    ) -> Result<String> {
        let noun = folder_noun(purpose);
        let Some(grant) = self.picker_grants.get(token) else {
            bail!("folder selection token is unknown or already used");
        };
        if grant.purpose != purpose {
            bail!("this folder selection was not made for the {noun}");
        }
        if grant.expires_at <= Instant::now() || grant.revision != revision || grant.root != path {
            bail!("folder selection is stale or does not match the proposed {noun}");
        }
        if selected_root_identity(Path::new(path))? != grant.identity {
            bail!("selected {noun} changed; choose it again");
        }
        Ok(grant.identity.clone())
    }

    fn mint_picker(
        &mut self,
        purpose: FolderPurpose,
        root: String,
        identity: String,
        revision: &str,
    ) -> String {
        self.prune();
        let token = Uuid::new_v4().to_string();
        self.picker_grants.insert(
            token.clone(),
            PickerGrant {
                purpose,
                root,
                identity,
                revision: revision.to_owned(),
                expires_at: Instant::now() + TOKEN_LIFETIME,
            },
        );
        token
    }

    fn prune(&mut self) {
        let now = Instant::now();
        self.picker_grants.retain(|_, grant| grant.expires_at > now);
        if self.picker_grants.len() >= MAX_PENDING_GRANTS {
            self.picker_grants.clear();
        }
    }
}

fn folder_noun(purpose: FolderPurpose) -> &'static str {
    match purpose {
        FolderPurpose::NotesFolder => "notes folder",
        FolderPurpose::KnowledgeBase => "knowledge base folder",
    }
}

/// Only harness setup (`harness_setup::set_up`) sets a harness entry. The
/// extension may echo it unchanged or remove it.
fn check_agent_fields(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<()> {
    let trusted_current = config::validate_base(current).is_ok();
    if next.agent.is_some() && (!trusted_current || next.agent != current.agent) {
        bail!("the AI harness can only be set by native harness setup");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use brauser_protocol::{AdapterDocument, StorageConfig};

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
            knowledge_base_dir: None,
        };
        let snapshot = ConfigSnapshot {
            storage: Some(storage.clone()),
            capture_enabled: false,
            strip_params: Vec::new(),
            ..crate::config::empty_config()
        };
        let mut authority = ConsentAuthority::new();
        let identity = selected_root_identity(folder.path()).unwrap();
        let token = authority.mint_picker(
            FolderPurpose::NotesFolder,
            storage.root,
            identity.clone(),
            "revision",
        );
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
            agent: Some(agent()),
            ..crate::config::empty_config()
        }
    }

    fn authorize(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<Option<String>> {
        ConsentAuthority::new()
            .authorize_update(current, "revision", next, None, None)
            .map(|authorized| authorized.selected_identity)
    }

    #[test]
    fn update_accepts_unchanged_agent_echo() {
        let current = set_up();
        let next = current.clone();
        assert_eq!(authorize(&current, &next).unwrap(), None);
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
        assert_eq!(authorize(&current, &next).unwrap(), None);
    }

    #[test]
    fn unrelated_update_can_preserve_an_invalid_adapter_document() {
        let mut current = crate::config::empty_config();
        current.adapters.format = 2;
        let mut next = current.clone();
        next.strip_params.push("campaign".into());

        assert!(crate::config::validate(&next).is_err());
        assert_eq!(authorize(&current, &next).unwrap(), None);
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
        assert!(authorize(&current, &next).is_err());

        let current = set_up();
        let next = ConfigSnapshot {
            agent: Some(brauser_protocol::AgentConfig {
                binary: crate::config::fixtures::absent_binary("other-claude"),
                ..agent()
            }),
            ..current.clone()
        };
        assert!(authorize(&current, &next).is_err());
    }

    #[test]
    fn update_allows_agent_removal_without_token() {
        let current = set_up();
        let next = ConfigSnapshot {
            agent: None,
            ..current.clone()
        };
        assert_eq!(authorize(&current, &next).unwrap(), None);
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
            knowledge_base_dir: None,
        }
    }

    #[test]
    fn summaries_dir_change_needs_no_token() {
        let folder = tempfile::tempdir().unwrap();
        let with = |summaries: Option<&str>| ConfigSnapshot {
            storage: Some(storage_at(folder.path(), summaries)),
            ..crate::config::empty_config()
        };
        assert_eq!(
            authorize(&with(None), &with(Some("summaries"))).unwrap(),
            None
        );
        assert_eq!(
            authorize(&with(Some("summaries")), &with(Some("ai"))).unwrap(),
            None
        );
    }

    fn with_knowledge_base(root: &Path, knowledge_base: Option<&Path>) -> ConfigSnapshot {
        let mut storage = storage_at(root, None);
        storage.knowledge_base_dir = knowledge_base.map(|path| path.to_string_lossy().into_owned());
        ConfigSnapshot {
            storage: Some(storage),
            ..crate::config::empty_config()
        }
    }

    fn mint(authority: &mut ConsentAuthority, purpose: FolderPurpose, path: &Path) -> String {
        let identity = selected_root_identity(path).unwrap();
        authority.mint_picker(
            purpose,
            path.to_string_lossy().into_owned(),
            identity,
            "revision",
        )
    }

    #[test]
    fn notes_folder_token_cannot_authorize_the_knowledge_base() {
        let root = tempfile::tempdir().unwrap();
        let knowledge_base = tempfile::tempdir().unwrap();
        let current = with_knowledge_base(root.path(), None);
        let next = with_knowledge_base(root.path(), Some(knowledge_base.path()));
        let mut authority = ConsentAuthority::new();
        let token = mint(
            &mut authority,
            FolderPurpose::NotesFolder,
            knowledge_base.path(),
        );
        let error = authority
            .authorize_update(&current, "revision", &next, None, Some(&token))
            .unwrap_err();
        assert!(error.to_string().contains("knowledge base"), "{error:#}");
        assert!(
            authority
                .authorize_update(&current, "revision", &next, None, None)
                .is_err()
        );

        let token = mint(
            &mut authority,
            FolderPurpose::KnowledgeBase,
            knowledge_base.path(),
        );
        let authorized = authority
            .authorize_update(&current, "revision", &next, None, Some(&token))
            .unwrap();
        assert_eq!(
            authorized.knowledge_base_identity,
            Some(selected_root_identity(knowledge_base.path()).unwrap())
        );
        assert_eq!(authorized.selected_identity, None);
    }

    #[test]
    fn knowledge_base_token_cannot_change_the_notes_folder() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let current = with_knowledge_base(root.path(), None);
        let next = with_knowledge_base(other.path(), None);
        let mut authority = ConsentAuthority::new();
        let token = mint(&mut authority, FolderPurpose::KnowledgeBase, other.path());
        let error = authority
            .authorize_update(&current, "revision", &next, Some(&token), None)
            .unwrap_err();
        assert!(error.to_string().contains("notes folder"), "{error:#}");
        // A knowledge base token sent in its own slot authorizes nothing else.
        assert!(
            authority
                .authorize_update(&current, "revision", &next, None, Some(&token))
                .is_err()
        );
    }

    #[test]
    fn stale_or_changed_knowledge_base_selection_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let knowledge_base = parent.path().join("kb");
        fs::create_dir(&knowledge_base).unwrap();
        let current = with_knowledge_base(root.path(), None);
        let next = with_knowledge_base(root.path(), Some(&knowledge_base));
        let mut authority = ConsentAuthority::new();
        let token = mint(
            &mut authority,
            FolderPurpose::KnowledgeBase,
            &knowledge_base,
        );

        let stale = authority
            .authorize_update(&current, "other-revision", &next, None, Some(&token))
            .unwrap_err();
        assert!(stale.to_string().contains("stale"), "{stale:#}");

        // Keep the original directory alive so the replacement gets a new inode.
        fs::rename(&knowledge_base, parent.path().join("kb-old")).unwrap();
        fs::create_dir(&knowledge_base).unwrap();
        let changed = authority
            .authorize_update(&current, "revision", &next, None, Some(&token))
            .unwrap_err();
        assert!(
            changed.to_string().contains("choose it again"),
            "{changed:#}"
        );
    }

    #[test]
    fn unchanged_or_unset_knowledge_base_needs_no_token() {
        let root = tempfile::tempdir().unwrap();
        let knowledge_base = tempfile::tempdir().unwrap();
        let set = with_knowledge_base(root.path(), Some(knowledge_base.path()));
        let unset = with_knowledge_base(root.path(), None);
        assert_eq!(authorize(&set, &set).unwrap(), None);
        assert_eq!(authorize(&set, &unset).unwrap(), None);
        assert_eq!(authorize(&unset, &unset).unwrap(), None);
    }
}

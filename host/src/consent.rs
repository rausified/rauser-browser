//! Short-lived, process-local grants for folder selection. Only the native
//! host can mint these grants; the extension cannot authorize a path or
//! redirect writes by sending an arbitrary config. Other config changes need
//! only the revision check, and only harness setup sets a harness entry.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use brauser_protocol::ConfigSnapshot;
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
    root: String,
    identity: String,
    revision: String,
    expires_at: Instant,
}

/// What `authorize_update` checked.
#[derive(Debug, Default)]
pub struct Authorized {
    pub selected_identity: Option<String>,
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

    /// Validate grants immediately before the revision-checked config write.
    /// Grants are removed only after the write succeeds.
    pub fn authorize_update(
        &self,
        current: &ConfigSnapshot,
        revision: &str,
        next: &ConfigSnapshot,
        picker_token: Option<&str>,
    ) -> Result<Authorized> {
        config::validate_update(current, next)?;
        let selected_identity = self.check_picker(current, revision, next, picker_token)?;
        check_agent_fields(current, next)?;
        Ok(Authorized { selected_identity })
    }

    /// Call only after ConfigStore::update returns a committed revision.
    pub fn consume_update(
        &mut self,
        _current: &ConfigSnapshot,
        _revision: &str,
        _next: &ConfigSnapshot,
        picker_token: Option<&str>,
    ) {
        if let Some(token) = picker_token {
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

    fn prune(&mut self) {
        let now = Instant::now();
        self.picker_grants.retain(|_, grant| grant.expires_at > now);
        if self.picker_grants.len() >= MAX_PENDING_GRANTS {
            self.picker_grants.clear();
        }
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
            agent: Some(agent()),
            ..crate::config::empty_config()
        }
    }

    fn authorize(current: &ConfigSnapshot, next: &ConfigSnapshot) -> Result<Option<String>> {
        ConsentAuthority::new()
            .authorize_update(current, "revision", next, None)
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
}

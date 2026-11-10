//! Exact config preparation for the dormant re-key transaction. This is not
//! an exception available to ordinary config updates or adapter repair.
use super::*;
use crate::relearn::PlanRequest;

pub(crate) struct Replacement {
    pub before: String,
    pub after: String,
    pub storage: StorageConfig,
    pub root_identity: String,
}

impl ConfigStore {
    /// The transaction caller holds config then capture locks. Rendering has
    /// no side effects and deliberately bypasses only the frozen-ID check.
    pub(crate) fn prepare_relearn(
        &self,
        request: &PlanRequest,
        guard: &ConfigLock,
    ) -> Result<Replacement> {
        self.require_lock(guard)?;
        let current = read_disk_state(&self.path)?;
        anyhow::ensure!(
            current.revision == request.expected_revision && current.issue.is_none(),
            "configuration changed or needs repair"
        );
        let before = read_exact(&self.path)?;
        anyhow::ensure!(
            revision_for(before.as_bytes()) == current.revision,
            "configuration changed during preparation"
        );
        let replacement = render(&before, request)?;
        let mut next = current.config.clone();
        next.adapters.artifacts = replacement_artifacts(&current.config, request);
        let adopted_path = adopted_contracts_path(self.dir()?);
        validate_strip_params_readers(
            &current.config,
            &next,
            &recorded_readers(
                &current.retired_adapter_contracts,
                &read_adopted_contracts(&adopted_path),
                &current.config,
                &next,
                &adopted_path,
            )?,
        )?;
        Ok(replacement)
    }

    pub(crate) fn relearn_config_path(&self) -> &Path {
        &self.path
    }
}

fn replacement_artifacts(
    current: &ConfigSnapshot,
    request: &PlanRequest,
) -> Vec<serde_json::Value> {
    // Match the reviewed projection's order exactly: unaffected origins,
    // followed by the replacement document for this origin.
    current
        .adapters
        .artifacts
        .iter()
        .filter(|a| a["origin"].as_str() != Some(request.origin.as_str()))
        .cloned()
        .chain(request.proposed.artifacts.clone())
        .collect()
}

/// Re-derive after bytes from before bytes during recovery, rather than
/// trusting a prepared replacement that changes unrelated settings.
pub(crate) fn render(before: &str, request: &PlanRequest) -> Result<Replacement> {
    anyhow::ensure!(
        before.len() <= MAX_CONFIG_BYTES,
        "oversized prepared configuration"
    );
    anyhow::ensure!(
        revision_for(before.as_bytes()) == request.expected_revision,
        "prepared configuration revision mismatch"
    );
    let mut stored: StoredConfig = toml::from_str(before)?;
    let current: ConfigSnapshot = toml::from_str::<StoredConfig>(before)?.into();
    validate(&current)?;
    anyhow::ensure!(
        stored.root_picker_confirmed,
        "home folder lacks picker confirmation"
    );
    task_folders::validate(&stored.task_folders)?;
    anyhow::ensure!(
        stored.adapters.extensions.is_empty(),
        "unsupported adapter keys require repair"
    );
    anyhow::ensure!(
        request.proposed.format == 1
            && !request.proposed.artifacts.is_empty()
            && request
                .proposed
                .artifacts
                .iter()
                .all(|a| a["origin"].as_str() == Some(request.origin.as_str()))
            && current
                .adapters
                .artifacts
                .iter()
                .any(|a| a["origin"].as_str() == Some(request.origin.as_str())),
        "replacement must name exactly one existing origin"
    );
    let mut next = current.clone();
    next.adapters.artifacts = replacement_artifacts(&current, request);
    validate_update(&current, &next)?;
    let history = updated_adapter_history(&stored.adapter_id_history, &current, &next);
    validate_adapter_history_budget(&stored.adapter_id_history, &history)?;
    stored.artifacts = next.adapters.artifacts;
    stored.adapter_id_history = history;
    let storage = current.storage.context("choose a home folder")?;
    layout::require_fixed(&storage)?;
    let root_identity = stored
        .root_identity
        .clone()
        .context("home folder lacks identity")?;
    let after = toml::to_string_pretty(&stored)?;
    anyhow::ensure!(
        after.len() <= MAX_CONFIG_BYTES,
        "replacement configuration exceeds 64 KiB"
    );
    Ok(Replacement {
        before: before.into(),
        after,
        storage,
        root_identity,
    })
}

pub(crate) fn read_exact(path: &Path) -> Result<String> {
    let meta = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= MAX_CONFIG_BYTES as u64,
        "configuration must be a bounded regular file"
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_CONFIG_BYTES,
        "configuration grew beyond its limit"
    );
    Ok(String::from_utf8(bytes)?)
}

/// Unlike ordinary config saves, a directory-sync failure is an error: the
/// transaction must retain its prepare record until recovery proves durability.
pub(crate) fn publish(
    path: &Path,
    bytes: &str,
    checkpoint: &dyn Fn(&str) -> Result<()>,
    sync_config_parent: &dyn Fn(&Path) -> Result<()>,
) -> Result<()> {
    let parent = path.parent().context("config path has no parent")?;
    let temporary = parent.join(format!(".config-{}.tmp", Uuid::new_v4()));
    let (mut file, _cleanup) = create_temporary(&temporary)?;
    let result = file
        .write_all(bytes.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    result?;
    checkpoint("config_file_synced")?;
    fs::rename(&temporary, path)?;
    checkpoint("config_published")?;
    sync_config_parent(parent)?;
    checkpoint("config_synced")?;
    Ok(())
}

pub(crate) fn sync_config_parent(parent: &Path) -> Result<()> {
    sync_parent(parent)
}

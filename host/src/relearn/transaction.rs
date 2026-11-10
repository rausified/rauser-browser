//! Dormant durable apply prerequisite. No CLI/native path can prepare one.
//! Activation requires universal recovery before ordinary access AND replay
//! integration in every reader. A config lock alone is not that integration.
use std::collections::BTreeSet;
use std::io::{Read, Write};

use anyhow::{Context, Result, ensure};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};

use super::aliases::{Action, Event, Resolution};
use super::journal::{self, Decision, RecordRequest, Store};
use super::{Identity, hash, plan_locked};
use crate::config::{ConfigStore, relearn as config_bytes};
use crate::vault::{open_selected_root, sync_directory};

const PENDING: &str = "apply-pending.json";
const HISTORY: &str = "rekey-history.jsonl";
const MAX_HISTORY: usize = 64 * 1024 * 1024;
const MAX_ENTRY: usize = 4 * 1024 * 1024;
const MAX_PREPARE: usize = 8 * 1024 * 1024;
const MAX_OPERATIONS: usize = 4096;
const MAX_EVENTS: usize = 32_768;

/// Ordinary note access consumes authoritative history, never a review or
/// cache. The caller has refreshed config under this live lock. Until the
/// universal recovery gate is enabled, pending publication must refuse here;
/// this reader cannot decide which side of an interrupted apply to expose.
pub(crate) fn note_resolution(
    config: &ConfigStore,
    guard: &crate::config::ConfigLock,
) -> Result<Resolution> {
    config.require_lock(guard)?;
    if let Some(store) = Store::open(config.dir()?, false)? {
        ensure!(
            store.read(PENDING, MAX_PREPARE)?.is_none(),
            "re-key publication is pending; note access requires recovery"
        );
    }
    let storage = config
        .snapshot()
        .storage
        .as_ref()
        .context("choose a home folder")?;
    let identity = config
        .root_identity()
        .context("home folder identity is unavailable")?;
    let Some(dir) = history_dir(storage, Some(identity), false)? else {
        return Ok(Resolution::default());
    };
    let entries = replay(&read_history(&dir)?, &storage.root, identity)?;
    Resolution::from_events(
        &entries
            .into_iter()
            .flat_map(|entry| entry.payload.events)
            .collect::<Vec<_>>(),
    )
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    version: u32,
    payload: Applied,
    checksum: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Applied {
    sequence: usize,
    previous_checksum: String,
    operation_id: String,
    decision_checksum: String,
    request_revision: String,
    plan_revision: String,
    home_root: String,
    home_identity: String,
    before_hash: String,
    after_hash: String,
    events: Vec<Event>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    version: u32,
    payload: Prepared,
    checksum: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    config_path: String,
    request: RecordRequest,
    before: String,
    after: String,
    offset: usize,
    prefix_hash: String,
    append: String,
}

/// The input carries a review, never aliases or file paths. It must exactly
/// match an existing approval AND still pass a freshly recomputed plan.
fn commit(config: &ConfigStore, request: &RecordRequest) -> Result<Entry> {
    commit_with(config, request, &|_| Ok(()))
}

fn commit_with(
    config: &ConfigStore,
    request: &RecordRequest,
    checkpoint: &dyn Fn(&str) -> Result<()>,
) -> Result<Entry> {
    require_directory_sync()?;
    let config_lock = config.lock_for_read()?;
    let _capture_lock = crate::capture::lock_capture(config.dir()?)?;
    recover_locked(config)?;
    let decision = approval(config, request)?;
    let current = config.reopen_locked(&config_lock)?;
    let storage = current
        .snapshot()
        .storage
        .as_ref()
        .context("choose a home folder")?;
    ensure!(
        storage.root == decision.payload.home_root
            && current.root_identity() == Some(decision.payload.home_identity.as_str()),
        "reviewed home folder changed"
    );
    // An exact retry acknowledges committed history, even after the old
    // config revision ceased to be current. It grants no new write authority.
    if let Some(dir) = history_dir(storage, current.root_identity(), false)? {
        let bytes = read_history(&dir)?;
        if let Some(entry) = replay(&bytes, &storage.root, &decision.payload.home_identity)?
            .into_iter()
            .find(|entry| entry.payload.operation_id == request.operation_id)
        {
            ensure!(
                entry.payload.request_revision == hash(&serde_json::to_vec(request)?)
                    && entry.payload.decision_checksum == decision.checksum,
                "applied operation ID reused with a different decision"
            );
            return Ok(entry);
        }
    }
    let plan = plan_locked(config, &request.review, &config_lock)?;
    ensure!(
        plan.plan_revision == request.expected_plan_revision,
        "review evidence changed; review a fresh plan before applying"
    );
    journal::validate_decision(Decision::Approve, &plan)?;
    Resolution::from_plan(&plan)?;
    let replacement = config.prepare_relearn(&request.review, &config_lock)?;
    ensure!(
        replacement.storage.root == decision.payload.home_root
            && replacement.root_identity == decision.payload.home_identity,
        "reviewed home folder changed during preparation"
    );
    let dir = history_dir(&replacement.storage, Some(&replacement.root_identity), true)?
        .context("missing history directory")?;
    let prefix = read_history(&dir)?;
    let entries = replay(
        &prefix,
        &replacement.storage.root,
        &replacement.root_identity,
    )?;
    let payload = Applied {
        sequence: entries.len() + 1,
        previous_checksum: entries
            .last()
            .map(|entry| entry.checksum.clone())
            .unwrap_or_default(),
        operation_id: request.operation_id.clone(),
        decision_checksum: decision.checksum,
        request_revision: hash(&serde_json::to_vec(request)?),
        plan_revision: plan.plan_revision.clone(),
        home_root: replacement.storage.root,
        home_identity: replacement.root_identity,
        before_hash: hash(replacement.before.as_bytes()),
        after_hash: hash(replacement.after.as_bytes()),
        events: events_for(&serde_json::to_value(&plan)?, &request.operation_id)?,
    };
    let entry = Entry {
        version: 1,
        checksum: hash(&serde_json::to_vec(&payload)?),
        payload,
    };
    let mut append = serde_json::to_string(&entry)?;
    append.push('\n');
    let prepared = Prepared {
        config_path: config
            .relearn_config_path()
            .to_str()
            .context("non-UTF-8 config path")?
            .into(),
        request: request.clone(),
        before: replacement.before,
        after: replacement.after,
        offset: prefix.len(),
        prefix_hash: hash(&prefix),
        append,
    };
    let pending = Pending {
        version: 1,
        checksum: hash(&serde_json::to_vec(&prepared)?),
        payload: prepared,
    };
    let raw = serde_json::to_vec(&pending)?;
    ensure!(
        raw.len() <= MAX_PREPARE,
        "apply preparation exceeds its size limit"
    );
    // Recheck config, owned content, contracts and all evidence after rendering.
    // Local editors need not honor Brauser's locks.
    ensure!(
        plan_locked(config, &request.review, &config_lock)?.plan_revision == plan.plan_revision,
        "review evidence changed during preparation"
    );
    let store = Store::open(config.dir()?, true)?.context("missing rekey directory")?;
    validate(config, &pending, &prefix)?;
    checkpoint("before_prepare")?;
    store.publish_prepare(PENDING, &raw)?;
    checkpoint("prepared")?;
    finish(
        config,
        &store,
        &pending,
        &raw,
        checkpoint,
        &sync_directory,
        &config_bytes::sync_config_parent,
    )?;
    Ok(entry)
}

fn approval(config: &ConfigStore, request: &RecordRequest) -> Result<journal::Entry> {
    ensure!(
        request.decision == Decision::Approve,
        "only approvals can be applied"
    );
    let entry = journal::inspect_locked(config)?
        .into_iter()
        .find(|entry| entry.payload.request.operation_id == request.operation_id)
        .context("record a reviewed approval before applying")?;
    ensure!(
        entry.payload.request_revision == hash(&serde_json::to_vec(request)?)
            && entry.payload.request.decision == Decision::Approve,
        "operation ID does not match the recorded approval"
    );
    Ok(entry)
}

/// Explicit lock ownership here avoids hidden lock acquisition in refresh or
/// reopen. The future universal recovery gate must call recover_locked while
/// holding config then capture locks, before serving any ordinary request.
fn recover(config: &ConfigStore) -> Result<()> {
    let _config_lock = config.lock_for_read()?;
    let _capture_lock = crate::capture::lock_capture(config.dir()?)?;
    recover_locked(config)
}

fn recover_locked(config: &ConfigStore) -> Result<()> {
    let Some(store) = Store::open(config.dir()?, false)? else {
        return Ok(());
    };
    let Some(raw) = store.read(PENDING, MAX_PREPARE)? else {
        return Ok(());
    };
    require_directory_sync()?;
    let pending: Pending = serde_json::from_slice(&raw).context("invalid apply prepare record")?;
    finish(
        config,
        &store,
        &pending,
        &raw,
        &|_| Ok(()),
        &sync_directory,
        &config_bytes::sync_config_parent,
    )
}

// Ordinary file publication uses best-effort directory sync on Windows. This
// multi-file transaction requires that ordering, so cannot use its no-op seam.
fn require_directory_sync() -> Result<()> {
    ensure!(
        !cfg!(windows),
        "durable re-key transactions require directory sync support; unavailable on Windows"
    );
    Ok(())
}

/// Validate every participant before even repairing a torn history suffix.
fn validate(
    config: &ConfigStore,
    pending: &Pending,
    bytes: &[u8],
) -> Result<config_bytes::Replacement> {
    let (replacement, approval) = validate_config(config, pending)?;
    validate_append(pending, bytes, &replacement, &approval)?;
    Ok(replacement)
}

// Authenticate the recorded home before opening any path from preparation.
fn validate_config(
    config: &ConfigStore,
    pending: &Pending,
) -> Result<(config_bytes::Replacement, journal::Entry)> {
    ensure!(
        pending.version == 1 && pending.checksum == hash(&serde_json::to_vec(&pending.payload)?),
        "apply prepare checksum or version mismatch"
    );
    let p = &pending.payload;
    ensure!(
        std::path::Path::new(&p.config_path) == config.relearn_config_path(),
        "apply prepare belongs to another config file"
    );
    let replacement = config_bytes::render(&p.before, &p.request.review)?;
    ensure!(
        p.after == replacement.after,
        "prepared replacement changes unreviewed configuration"
    );
    let current = config_bytes::read_exact(config.relearn_config_path())?;
    ensure!(
        current == p.before || current == p.after,
        "configuration differs from prepared before and after bytes; retain files for repair"
    );
    let approval = approval(config, &p.request)?;
    ensure!(
        approval.payload.home_root == replacement.storage.root
            && approval.payload.home_identity == replacement.root_identity,
        "prepared home differs from the recorded approval"
    );
    Ok((replacement, approval))
}

fn validate_append(
    pending: &Pending,
    bytes: &[u8],
    replacement: &config_bytes::Replacement,
    approval: &journal::Entry,
) -> Result<()> {
    let p = &pending.payload;
    let append = p.append.as_bytes();
    ensure!(
        !append.is_empty()
            && append.len() <= MAX_ENTRY
            && append.ends_with(b"\n")
            && append.iter().filter(|b| **b == b'\n').count() == 1
            && p.offset <= bytes.len()
            && p.offset <= MAX_HISTORY.saturating_sub(append.len()),
        "invalid apply history append"
    );
    let prefix = &bytes[..p.offset];
    ensure!(
        hash(prefix) == p.prefix_hash && append.starts_with(&bytes[p.offset..]),
        "home history differs from the prepared prefix or append; retain files for repair"
    );
    let mut complete = prefix.to_vec();
    complete.extend_from_slice(append);
    let entries = replay(
        &complete,
        &replacement.storage.root,
        &replacement.root_identity,
    )?;
    let entry = entries.last().context("missing prepared history entry")?;
    ensure!(
        entry.payload.operation_id == p.request.operation_id
            && entry.payload.decision_checksum == approval.checksum
            && entry.payload.request_revision == hash(&serde_json::to_vec(&p.request)?)
            && entry.payload.plan_revision == p.request.expected_plan_revision
            && entry.payload.before_hash == hash(p.before.as_bytes())
            && entry.payload.after_hash == hash(p.after.as_bytes())
            && entry.payload.events == events_for(&approval.payload.plan, &p.request.operation_id)?,
        "prepared history does not match its reviewed decision"
    );
    Ok(())
}

fn finish(
    config: &ConfigStore,
    store: &Store,
    pending: &Pending,
    raw: &[u8],
    checkpoint: &dyn Fn(&str) -> Result<()>,
    sync_dir: &dyn Fn(&Dir) -> std::io::Result<()>,
    sync_config_parent: &dyn Fn(&std::path::Path) -> Result<()>,
) -> Result<()> {
    let (replacement, approval) = validate_config(config, pending)?;
    let dir = history_dir(
        &replacement.storage,
        Some(&replacement.root_identity),
        false,
    )?
    .context("prepared history directory disappeared")?;
    let bytes = read_history(&dir)?;
    validate_append(pending, &bytes, &replacement, &approval)?;
    // A failed prepare-directory sync can leave a visible pending record.
    // Recovery must persist that entry before publishing either participant;
    // otherwise a second crash could lose the only roll-forward authority.
    sync_dir(&store.dir)?;
    checkpoint("prepare_synced")?;
    let p = &pending.payload;
    let tail = bytes.len() - p.offset;
    let mut options = OpenOptions::new();
    options.append(true);
    if dir
        .symlink_metadata(HISTORY)
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        options.create_new(true);
    }
    let mut file = dir.open_with(HISTORY, &options)?;
    file.write_all(&p.append.as_bytes()[tail..])?;
    checkpoint("history_appended")?;
    file.sync_all()?;
    sync_dir(&dir)?;
    checkpoint("history_synced")?;
    drop(file);
    config_bytes::publish(
        config.relearn_config_path(),
        &p.after,
        checkpoint,
        sync_config_parent,
    )?;
    ensure!(
        store.read(PENDING, MAX_PREPARE)?.as_deref() == Some(raw),
        "apply prepare changed during publication; retain files for repair"
    );
    store.dir.remove_file(PENDING)?;
    sync_dir(&store.dir)?;
    checkpoint("prepare_removed")?;
    Ok(())
}

fn history_dir(
    storage: &brauser_protocol::StorageConfig,
    identity: Option<&str>,
    create: bool,
) -> Result<Option<Dir>> {
    crate::layout::require_fixed(storage)?;
    let (root, _) = open_selected_root(std::path::Path::new(&storage.root), identity)?;
    match root.symlink_metadata(crate::layout::LOGS_DIR) {
        Ok(meta) => ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "history requires a real logs directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Ok(None);
            }
            root.create_dir(crate::layout::LOGS_DIR)?;
        }
        Err(error) => return Err(error.into()),
    }
    if create {
        sync_directory(&root)?;
    }
    Ok(Some(root.open_dir(crate::layout::LOGS_DIR)?))
}

fn read_history(dir: &Dir) -> Result<Vec<u8>> {
    let meta = match dir.symlink_metadata(HISTORY) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= MAX_HISTORY as u64,
        "home re-key history must be a bounded regular file"
    );
    let mut bytes = Vec::new();
    dir.open(HISTORY)?
        .take(MAX_HISTORY as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_HISTORY,
        "home re-key history grew beyond its limit"
    );
    Ok(bytes)
}

fn replay(bytes: &[u8], home: &str, identity: &str) -> Result<Vec<Entry>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    ensure!(
        bytes.len() <= MAX_HISTORY && bytes.ends_with(b"\n"),
        "torn home re-key history without matching prepare"
    );
    let mut entries: Vec<Entry> = Vec::new();
    let mut ids = BTreeSet::new();
    let mut events = Vec::new();
    for line in bytes[..bytes.len() - 1].split(|b| *b == b'\n') {
        ensure!(
            !line.is_empty() && line.len() < MAX_ENTRY && entries.len() < MAX_OPERATIONS,
            "home re-key history exceeds its limits"
        );
        let entry: Entry = serde_json::from_slice(line)?;
        let p = &entry.payload;
        ensure!(
            entry.version == 1
                && p.sequence == entries.len() + 1
                && p.previous_checksum == entries.last().map(|e| e.checksum.as_str()).unwrap_or("")
                && entry.checksum == hash(&serde_json::to_vec(p)?)
                && p.home_root == home
                && p.home_identity == identity
                && uuid::Uuid::parse_str(&p.operation_id)?.to_string() == p.operation_id
                && ids.insert(p.operation_id.clone())
                && [
                    &p.decision_checksum,
                    &p.request_revision,
                    &p.plan_revision,
                    &p.before_hash,
                    &p.after_hash
                ]
                .into_iter()
                .all(|s| s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
            "home re-key history checksum, identity or sequence mismatch"
        );
        for (index, event) in p.events.iter().enumerate() {
            ensure!(
                event.action == Action::Confirm
                    && event.event_id == format!("{}:{index}", p.operation_id)
                    && !event.from.artifact_id.is_empty()
                    && !event.to.artifact_id.is_empty(),
                "invalid applied alias event"
            );
        }
        events.extend(p.events.iter().cloned());
        ensure!(
            events.len() <= MAX_EVENTS,
            "home re-key history has too many aliases"
        );
        entries.push(entry);
    }
    Resolution::from_events(&events)?;
    Ok(entries)
}

/// Read only the mapping columns of an already host-derived approval. Event
/// IDs are operation-scoped: a later move back may have the same source key.
fn events_for(plan: &serde_json::Value, operation: &str) -> Result<Vec<Event>> {
    #[derive(Deserialize)]
    struct Row {
        source: brauser_protocol::RecordKey,
        targets: Vec<Identity>,
        issues: Vec<String>,
        classifications: Vec<String>,
    }
    let rows: Vec<Row> = serde_json::from_value(plan["identities"].clone())?;
    let mut targets = BTreeSet::new();
    let mut moving = BTreeSet::new();
    for row in &rows {
        ensure!(
            row.source.kind == brauser_protocol::RecordKeyKind::Artifact
                && row.issues.is_empty()
                && row.targets.len() == 1
                && !row
                    .classifications
                    .iter()
                    .any(|c| c == "split" || c == "merged"),
            "approval contains unresolved identity mappings"
        );
        ensure!(
            targets.insert(row.targets[0].artifact_id.clone()),
            "simultaneous re-key merge"
        );
        if row.source.value != row.targets[0].artifact_id {
            moving.insert(row.source.value.as_str());
        }
    }
    let mut events = Vec::new();
    for row in &rows {
        let target = &row.targets[0];
        if row.source.value == target.artifact_id {
            continue;
        }
        ensure!(
            !moving.contains(target.artifact_id.as_str()),
            "simultaneous re-key reuse"
        );
        events.push(Event {
            event_id: format!("{operation}:{}", events.len()),
            action: Action::Confirm,
            from: Identity {
                artifact_type: target.artifact_type.clone(),
                artifact_id: row.source.value.clone(),
            },
            to: target.clone(),
        });
    }
    Ok(events)
}

#[cfg(all(test, not(windows)))]
mod tests;

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn publication_and_pending_recovery_refuse_without_directory_sync() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let config = ConfigStore::load_from(path.clone());
        let request = RecordRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            decision: Decision::Approve,
            expected_plan_revision: String::new(),
            review: super::super::PlanRequest {
                origin: "https://work.example".into(),
                expected_revision: config.revision().into(),
                proposed: brauser_protocol::AdapterDocument {
                    format: 1,
                    artifacts: Vec::new(),
                },
            },
        };
        assert!(
            commit(&config, &request)
                .unwrap_err()
                .to_string()
                .contains("unavailable on Windows")
        );
        assert!(!path.exists());
        assert!(!temp.path().join("rekey").exists());
        recover(&config).unwrap();
        let store = Store::open(temp.path(), true).unwrap().unwrap();
        // Check platform support before interpreting or changing pending bytes.
        store.publish_prepare(PENDING, b"{}").unwrap();
        let error = recover(&config).unwrap_err();
        assert!(error.to_string().contains("unavailable on Windows"));
        assert_eq!(store.read(PENDING, MAX_PREPARE).unwrap().unwrap(), b"{}");
        assert!(!path.exists());
    }
}

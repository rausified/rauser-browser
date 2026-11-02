//! Durable reviewed decisions, deliberately separate from identity application.
//! Callers hold the config and capture locks. No journal entry is an alias.
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use cap_std::fs::{Dir, DirBuilder, OpenOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{Plan, PlanRequest, hash, plan_locked};
use crate::{config::ConfigStore, vault::sync_directory};

const JOURNAL: &str = "decisions.jsonl";
const PENDING: &str = "pending.json";
const MAX_ENTRY: usize = 4 * 1024 * 1024;
const MAX_JOURNAL: usize = 64 * 1024 * 1024;
const MAX_OPERATIONS: usize = 4096;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Reject,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordRequest {
    pub operation_id: String,
    pub decision: Decision,
    pub review: PlanRequest,
    pub expected_plan_revision: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    version: u32,
    pub payload: Payload,
    pub checksum: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub sequence: usize,
    pub previous_checksum: String,
    pub request: RecordRequest,
    pub request_revision: String,
    pub home_root: String,
    pub home_identity: String,
    /// Stored as structured JSON so the complete simultaneous mapping and all
    /// conflict classifications survive. This never enters pairwise alias replay.
    pub plan: Value,
}

#[derive(Debug, Serialize)]
pub struct Receipt {
    pub operation_id: String,
    pub sequence: usize,
    pub decision: Decision,
    pub plan_revision: String,
    pub checksum: String,
    pub applied: bool,
}

impl Entry {
    fn receipt(&self) -> Receipt {
        Receipt {
            operation_id: self.payload.request.operation_id.clone(),
            sequence: self.payload.sequence,
            decision: self.payload.request.decision,
            plan_revision: self.payload.request.expected_plan_revision.clone(),
            checksum: self.checksum.clone(),
            applied: false,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    version: u32,
    offset: usize,
    prefix_hash: String,
    append: String,
    append_hash: String,
}

fn request_revision(request: &RecordRequest) -> Result<String> {
    Ok(hash(&serde_json::to_vec(request)?))
}

fn validate_request(request: &RecordRequest) -> Result<()> {
    let id = Uuid::parse_str(&request.operation_id).context("invalid operation UUID")?;
    ensure!(
        id.to_string() == request.operation_id,
        "operation UUID must use canonical lowercase form"
    );
    ensure!(
        request.expected_plan_revision.len() == 64
            && request
                .expected_plan_revision
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid plan revision"
    );
    Ok(())
}

pub(super) fn validate_decision(decision: Decision, plan: &Plan) -> Result<()> {
    if decision == Decision::Approve {
        ensure!(
            plan.identities.iter().all(|row| {
                row.issues.is_empty()
                    && row.targets.len() == 1
                    && !row.classifications.contains("split")
                    && !row.classifications.contains("merged")
                    && row.source.kind == brauser_protocol::RecordKeyKind::Artifact
            }),
            "review has unresolved, split, merge or key-reuse decisions; record a rejection or revise the proposal"
        );
    }
    Ok(())
}

/// Record one freshly revalidated decision. A retry of an already recorded
/// request returns its original receipt even if the vault has since changed.
/// That is acknowledgement of a decision, never permission to apply it.
pub fn record(config: &ConfigStore, request: &RecordRequest) -> Result<Receipt> {
    record_with(config, request, &|_| Ok(()))
}

fn record_with(
    config: &ConfigStore,
    request: &RecordRequest,
    checkpoint: &dyn Fn(&str) -> Result<()>,
) -> Result<Receipt> {
    validate_request(request)?;
    let _config_lock = config.lock_current()?;
    let _capture_lock = crate::capture::lock_capture(config.dir()?)?;
    let revision = request_revision(request)?;
    // Inspect an existing journal before revalidation, without creating one
    // for a stale or refused review. Recovery publishes only a past decision.
    if let Some(store) = Store::open(config.dir()?, false)? {
        let entries = store.recover()?;
        if let Some(entry) = entries
            .iter()
            .find(|e| e.payload.request.operation_id == request.operation_id)
        {
            ensure!(
                entry.payload.request_revision == revision,
                "operation ID reused with a different payload"
            );
            return Ok(entry.receipt());
        }
    }
    let plan = plan_locked(config, &request.review)?;
    ensure!(
        plan.plan_revision == request.expected_plan_revision,
        "review evidence changed; review a fresh plan before recording"
    );
    validate_decision(request.decision, &plan)?;
    let current = config.reopen();
    ensure!(
        current.revision() == plan.config_revision,
        "configuration changed during review"
    );
    let store = Store::open(config.dir()?, true)?.context("missing journal directory")?;
    let bytes = store.read(JOURNAL, MAX_JOURNAL)?.unwrap_or_default();
    let entries = replay(&bytes)?;
    ensure!(
        entries.len() < MAX_OPERATIONS,
        "re-learning journal is full; retain it and request a supported archive procedure"
    );
    let payload = Payload {
        sequence: entries.len() + 1,
        previous_checksum: entries
            .last()
            .map(|e| e.checksum.clone())
            .unwrap_or_default(),
        request: request.clone(),
        request_revision: revision,
        home_root: current
            .snapshot()
            .storage
            .as_ref()
            .context("missing home folder")?
            .root
            .clone(),
        home_identity: current
            .root_identity()
            .context("missing home folder identity")?
            .into(),
        plan: serde_json::to_value(plan)?,
    };
    let entry = Entry {
        version: 1,
        checksum: hash(&serde_json::to_vec(&payload)?),
        payload,
    };
    let mut append = serde_json::to_vec(&entry)?;
    append.push(b'\n');
    ensure!(
        append.len() <= MAX_ENTRY && bytes.len() + append.len() <= MAX_JOURNAL,
        "re-learning journal exceeds its size limit"
    );
    let pending = Pending {
        version: 1,
        offset: bytes.len(),
        prefix_hash: hash(&bytes),
        append_hash: hash(&append),
        append: String::from_utf8(append)?,
    };
    checkpoint("before_prepare")?;
    store.prepare(&pending)?;
    checkpoint("prepared")?;
    store.finish(&pending, &bytes, checkpoint)?;
    Ok(entry.receipt())
}

/// Inspection also completes an exact prepared append after a crash. It does
/// not require the old home folder to be mounted and never opens its contents.
pub fn inspect(config: &ConfigStore) -> Result<Vec<Entry>> {
    let _config_lock = config.lock_current()?;
    let _capture_lock = crate::capture::lock_capture(config.dir()?)?;
    match Store::open(config.dir()?, false)? {
        Some(store) => store.recover(),
        None => Ok(Vec::new()),
    }
}

struct Store {
    dir: Dir,
}

impl Store {
    fn open(parent: &Path, create: bool) -> Result<Option<Self>> {
        let root = Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
        match root.symlink_metadata("rekey") {
            Ok(meta) => {
                ensure!(
                    meta.is_dir() && !meta.file_type().is_symlink(),
                    "rekey journal must be a real directory"
                );
                check_private(&meta)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                let builder = DirBuilder::new();
                #[cfg(unix)]
                let builder = {
                    use cap_std::fs::DirBuilderExt;
                    let mut builder = builder;
                    builder.mode(0o700);
                    builder
                };
                root.create_dir_with("rekey", &builder)?;
            }
            Err(error) => return Err(error.into()),
        }
        // A previous creator may have failed its parent-directory sync. A
        // retry must persist this entry even when the directory already exists.
        sync_directory(&root)?;
        Ok(Some(Self {
            dir: root.open_dir("rekey")?,
        }))
    }

    fn read(&self, name: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        let meta = match self.dir.symlink_metadata(name) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= limit as u64,
            "invalid or oversized re-learning journal file: {name}"
        );
        check_private(&meta)?;
        let mut bytes = Vec::new();
        self.dir
            .open(name)?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= limit,
            "re-learning journal file grew beyond its limit"
        );
        Ok(Some(bytes))
    }

    fn prepare(&self, pending: &Pending) -> Result<()> {
        let name = format!(".prepare-{}.tmp", Uuid::new_v4());
        let mut file = self
            .dir
            .open_with(&name, private_options().write(true).create_new(true))?;
        let cleanup = crate::vault::TempFileCleanup::new(&self.dir, &name);
        file.write_all(&serde_json::to_vec(pending)?)?;
        file.sync_all()?;
        drop(file);
        self.dir
            .hard_link(&name, &self.dir, PENDING)
            .context("publishing re-learning prepare record")?;
        sync_directory(&self.dir)?;
        drop(cleanup);
        Ok(())
    }

    fn recover(&self) -> Result<Vec<Entry>> {
        let mut bytes = self.read(JOURNAL, MAX_JOURNAL)?.unwrap_or_default();
        if let Some(raw) = self.read(PENDING, MAX_ENTRY * 2 + 4096)? {
            let pending: Pending =
                serde_json::from_slice(&raw).context("invalid re-learning prepare record")?;
            self.finish(&pending, &bytes, &|_| Ok(()))?;
            bytes = self
                .read(JOURNAL, MAX_JOURNAL)?
                .context("recovered journal missing")?;
        }
        replay(&bytes)
    }

    fn finish(
        &self,
        pending: &Pending,
        bytes: &[u8],
        checkpoint: &dyn Fn(&str) -> Result<()>,
    ) -> Result<()> {
        let append = pending.append.as_bytes();
        ensure!(
            pending.version == 1
                && !append.is_empty()
                && append.len() <= MAX_ENTRY
                && append.ends_with(b"\n")
                && append.iter().filter(|byte| **byte == b'\n').count() == 1
                && hash(append) == pending.append_hash
                && pending.offset <= bytes.len()
                && pending.offset + append.len() <= MAX_JOURNAL,
            "invalid re-learning prepare identity"
        );
        let prefix = &bytes[..pending.offset];
        ensure!(
            hash(prefix) == pending.prefix_hash,
            "re-learning journal prefix changed; retain files for repair"
        );
        let mut complete = prefix.to_vec();
        complete.extend_from_slice(append);
        // Validate the entire future journal before touching even a torn tail.
        replay(&complete)?;
        let tail = &bytes[pending.offset..];
        ensure!(
            append.starts_with(tail),
            "re-learning journal tail differs from prepared bytes; retain files for repair"
        );
        let mut options = private_options();
        options.append(true);
        if self
            .dir
            .symlink_metadata(JOURNAL)
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            options.create_new(true);
        }
        let mut file = self.dir.open_with(JOURNAL, &options)?;
        file.write_all(&append[tail.len()..])?;
        checkpoint("appended")?;
        file.sync_all()?;
        sync_directory(&self.dir)?;
        checkpoint("synced")?;
        self.dir.remove_file(PENDING)?;
        sync_directory(&self.dir)?;
        Ok(())
    }
}

fn private_options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    let options = {
        use cap_std::fs::OpenOptionsExt;
        let mut options = options;
        options.mode(0o600);
        options
    };
    options
}

fn check_private(meta: &cap_std::fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        ensure!(
            meta.uid() == rustix::process::getuid().as_raw() && meta.mode() & 0o077 == 0,
            "re-learning state must be private and owned by the current user"
        );
    }
    #[cfg(not(unix))]
    let _ = meta;
    Ok(())
}

fn replay(bytes: &[u8]) -> Result<Vec<Entry>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    ensure!(
        bytes.len() <= MAX_JOURNAL && bytes.ends_with(b"\n"),
        "torn re-learning journal without a matching prepare record"
    );
    let mut entries: Vec<Entry> = Vec::new();
    let mut ids = BTreeSet::new();
    for line in bytes[..bytes.len() - 1].split(|b| *b == b'\n') {
        ensure!(
            !line.is_empty() && line.len() < MAX_ENTRY && entries.len() < MAX_OPERATIONS,
            "invalid re-learning journal length"
        );
        let entry: Entry =
            serde_json::from_slice(line).context("malformed re-learning journal entry")?;
        validate_request(&entry.payload.request)?;
        ensure!(
            entry.version == 1
                && entry.payload.sequence == entries.len() + 1
                && entry.payload.previous_checksum
                    == entries.last().map(|e| e.checksum.as_str()).unwrap_or("")
                && entry.checksum == hash(&serde_json::to_vec(&entry.payload)?)
                && entry.payload.request_revision == request_revision(&entry.payload.request)?
                && entry.payload.plan["plan_revision"].as_str()
                    == Some(entry.payload.request.expected_plan_revision.as_str())
                && entry.payload.plan["config_revision"].as_str()
                    == Some(entry.payload.request.review.expected_revision.as_str())
                && entry.payload.plan["origin"].as_str()
                    == Some(entry.payload.request.review.origin.as_str())
                && entry.payload.plan["proposed_revision"].as_str()
                    == Some(
                        hash(&serde_json::to_vec(&entry.payload.request.review.proposed)?).as_str()
                    )
                && entry.payload.plan["review_only"] == true
                && Path::new(&entry.payload.home_root).is_absolute()
                && !entry.payload.home_identity.is_empty()
                && ids.insert(entry.payload.request.operation_id.clone()),
            "re-learning journal identity, checksum or sequence mismatch"
        );
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests;

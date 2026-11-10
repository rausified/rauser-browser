use super::*;
use brauser_protocol::{AdapterDocument, RecordKey, RecordKeyKind};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

const ORIGIN: &str = "https://work.example";

struct Fixture {
    _temp: tempfile::TempDir,
    config: ConfigStore,
    path: PathBuf,
    home: PathBuf,
    request: RecordRequest,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let identity = crate::vault::selected_root_identity(&home).unwrap();
        let path = temp.path().join("config.toml");
        let raw = json!({
            "root_picker_confirmed":true, "root_identity":identity,
            "knowledge_base_identity":identity,
            "capture_enabled":true,
            "storage":{"root":home.to_str().unwrap(), "profile":"neutral", "log_dir":"logs",
                "pages_dir":"notes", "later_dir":"later", "artifacts_dir":"artifacts", "summaries_dir":"summaries",
                "knowledge_base_dir":home.to_str().unwrap()},
            "task_folders":[{"id":uuid::Uuid::new_v4().to_string(), "path":home.to_str().unwrap(), "identity":identity}],
            "commands":[{"name":"review", "instruction":"Keep this literal command"}],
            "summarizer":{"future_harness":"preserve this opaque table"},
            "adapters":{"format":1},
            "artifacts":[adapter("old:{host}/{id}"),
                {"type":"other.item", "origin":"https://other.example", "match":[{"path":"/items/{id:int}"}], "id":"other:{host}/{id}"}]
        });
        fs::write(&path, toml::to_string(&raw).unwrap()).unwrap();
        let config = ConfigStore::load_from(path.clone());
        assert!(config.configured(), "{:?}", config.config_issue());
        assert!(
            config.adapter_document().is_ok(),
            "{:?}",
            config.adapter_issue()
        );
        let vault = crate::vault::Vault::open_checked(
            config.snapshot().storage.as_ref().unwrap(),
            config.root_identity(),
        )
        .unwrap();
        crate::note::save_keyed_note_with_provenance(
            &vault,
            &crate::url_provenance::StoredUrl {
                url: format!("{ORIGIN}/items/1"),
                stripped_params: Some(vec![]),
            },
            &RecordKey {
                kind: RecordKeyKind::Artifact,
                value: "old:work.example/1".into(),
            },
            "Title",
            "Preserve physical note old:work.example/1",
            "missing",
        )
        .unwrap();
        fs::write(
            home.join("notes/user-owned.md"),
            "Preserve unrelated Markdown exactly",
        )
        .unwrap();
        let review = super::super::PlanRequest {
            origin: ORIGIN.into(),
            expected_revision: config.revision().into(),
            proposed: AdapterDocument {
                format: 1,
                artifacts: vec![adapter("new:{host}/{id}")],
            },
        };
        let plan = super::super::plan(&config, &review).unwrap();
        let request = RecordRequest {
            operation_id: uuid::Uuid::new_v4().to_string(),
            decision: Decision::Approve,
            expected_plan_revision: plan.plan_revision,
            review,
        };
        journal::record(&config, &request).unwrap();
        Self {
            _temp: temp,
            config,
            path,
            home,
            request,
        }
    }

    fn pending_path(&self) -> PathBuf {
        self.path.parent().unwrap().join("rekey").join(PENDING)
    }
    fn history_path(&self) -> PathBuf {
        self.home.join("logs").join(HISTORY)
    }
    fn decisions(&self) -> Vec<u8> {
        fs::read(self.path.parent().unwrap().join("rekey/decisions.jsonl")).unwrap()
    }
    fn pending(&self) -> Pending {
        serde_json::from_slice(&fs::read(self.pending_path()).unwrap()).unwrap()
    }
    fn prepare(&self) {
        assert!(commit_with(&self.config, &self.request, &fail_at("prepared")).is_err());
        assert!(self.pending_path().exists());
    }
    fn notes(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files: Vec<_> = fs::read_dir(self.home.join("notes"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (path.clone(), fs::read(path).unwrap())
            })
            .collect();
        files.sort();
        files
    }
}

fn adapter(id: &str) -> serde_json::Value {
    json!({"type":"work.item", "origin":ORIGIN, "match":[{"path":"/items/{id:int}"}], "id":id})
}
fn fail_at(phase: &'static str) -> impl Fn(&str) -> Result<()> {
    move |current| {
        ensure!(current != phase, "simulated crash at {phase}");
        Ok(())
    }
}
fn checksum(pending: &mut Pending) {
    pending.checksum = hash(&serde_json::to_vec(&pending.payload).unwrap());
}

#[test]
fn exact_approval_publishes_bound_history_and_preserves_host_fields_and_physical_notes() {
    let f = Fixture::new();
    let before: toml::Table = toml::from_str(&fs::read_to_string(&f.path).unwrap()).unwrap();
    let notes = f.notes();
    let decisions = f.decisions();
    let entry = commit(&f.config, &f.request).unwrap();
    let history = fs::read(f.history_path()).unwrap();
    let after: toml::Table = toml::from_str(&fs::read_to_string(&f.path).unwrap()).unwrap();
    for key in [
        "root_picker_confirmed",
        "root_identity",
        "knowledge_base_identity",
        "storage",
        "capture_enabled",
        "commands",
        "summarizer",
        "task_folders",
    ] {
        assert_eq!(before[key], after[key], "changed {key}");
    }
    assert_eq!(
        after["artifacts"].as_array().unwrap()[0],
        before["artifacts"].as_array().unwrap()[1]
    );
    assert_eq!(
        after["artifacts"].as_array().unwrap()[1]["id"].as_str(),
        Some("new:{host}/{id}")
    );
    assert!(
        after["adapter_id_history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"].as_str() == Some("old:{host}/{id}"))
    );
    assert_eq!(f.notes(), notes);
    assert_eq!(f.decisions(), decisions);
    assert!(!f.pending_path().exists());
    let events = &entry.payload.events;
    assert_eq!(events.len(), 1);
    let resolution = Resolution::from_events(events).unwrap();
    assert_eq!(
        resolution.canonical("old:work.example/1"),
        "new:work.example/1"
    );
    assert_eq!(
        commit(&f.config, &f.request).unwrap().checksum,
        entry.checksum
    );
    recover(&f.config).unwrap();
    assert_eq!(fs::read(f.history_path()).unwrap(), history);
    assert!(!journal::record(&f.config, &f.request).unwrap().applied);
}

#[test]
fn every_interruption_boundary_rolls_forward_or_leaves_an_unprepared_operation_untouched() {
    for phase in [
        "before_prepare",
        "prepared",
        "prepare_synced",
        "history_appended",
        "history_synced",
        "config_file_synced",
        "config_published",
        "config_synced",
        "prepare_removed",
    ] {
        let f = Fixture::new();
        let before = fs::read(&f.path).unwrap();
        let notes = f.notes();
        let decisions = f.decisions();
        let after = config_bytes::render(std::str::from_utf8(&before).unwrap(), &f.request.review)
            .unwrap()
            .after;
        assert!(
            commit_with(&f.config, &f.request, &fail_at(phase)).is_err(),
            "{phase}"
        );
        recover(&f.config).unwrap();
        recover(&f.config).unwrap();
        assert!(!f.pending_path().exists(), "{phase}");
        if phase == "before_prepare" {
            assert_eq!(fs::read(&f.path).unwrap(), before);
            assert!(!f.history_path().exists());
        } else {
            assert_eq!(fs::read_to_string(&f.path).unwrap(), after, "{phase}");
            assert_eq!(commit(&f.config, &f.request).unwrap().payload.sequence, 1);
        }
        assert_eq!(f.notes(), notes);
        assert_eq!(f.decisions(), decisions);
    }
}

#[test]
fn recovery_persists_prepare_before_touching_history_or_config() {
    let f = Fixture::new();
    f.prepare();
    let raw = fs::read(f.pending_path()).unwrap();
    let pending = f.pending();
    let store = Store::open(f.config.dir().unwrap(), false)
        .unwrap()
        .unwrap();
    // Fail the recovery durability barrier, retaining the prepare and both
    // original participants even though the prepare is readable.
    let error = finish(
        &f.config,
        &store,
        &pending,
        &raw,
        &|_| Ok(()),
        &|_| Err(std::io::Error::other("prepare directory sync failed")),
        &config_bytes::sync_config_parent,
    )
    .unwrap_err();
    assert!(error.to_string().contains("prepare directory sync failed"));
    assert_eq!(fs::read(f.pending_path()).unwrap(), raw);
    assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.before);
    assert!(!f.history_path().exists());
    recover(&f.config).unwrap();
    recover(&f.config).unwrap();
    assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.after);
    assert_eq!(
        fs::read(f.history_path()).unwrap(),
        pending.payload.append.as_bytes()
    );
    assert!(!f.pending_path().exists());
}

#[test]
fn history_directory_sync_failure_retains_prepare_before_config_publication() {
    let f = Fixture::new();
    f.prepare();
    let raw = fs::read(f.pending_path()).unwrap();
    let pending = f.pending();
    let notes = f.notes();
    let decisions = f.decisions();
    let store = Store::open(f.config.dir().unwrap(), false)
        .unwrap()
        .unwrap();
    let calls = std::cell::Cell::new(0);
    let error = finish(
        &f.config,
        &store,
        &pending,
        &raw,
        &|_| Ok(()),
        &|dir| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                return Err(std::io::Error::other("history directory sync failed"));
            }
            sync_directory(dir)
        },
        &config_bytes::sync_config_parent,
    )
    .unwrap_err();
    assert!(error.to_string().contains("history directory sync failed"));
    assert_eq!(calls.get(), 2);
    assert_eq!(fs::read(f.pending_path()).unwrap(), raw);
    assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.before);
    assert_eq!(
        fs::read(f.history_path()).unwrap(),
        pending.payload.append.as_bytes()
    );
    recover(&f.config).unwrap();
    recover(&f.config).unwrap();
    assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.after);
    assert_eq!(
        fs::read(f.history_path()).unwrap(),
        pending.payload.append.as_bytes()
    );
    assert!(!f.pending_path().exists());
    assert_eq!(f.notes(), notes);
    assert_eq!(f.decisions(), decisions);
}

#[test]
fn config_directory_sync_failure_retains_prepare_after_config_rename() {
    let f = Fixture::new();
    f.prepare();
    let raw = fs::read(f.pending_path()).unwrap();
    let pending = f.pending();
    let notes = f.notes();
    let decisions = f.decisions();
    let store = Store::open(f.config.dir().unwrap(), false)
        .unwrap()
        .unwrap();
    let calls = std::cell::Cell::new(0);
    let error = finish(
        &f.config,
        &store,
        &pending,
        &raw,
        &|_| Ok(()),
        &sync_directory,
        &|parent| {
            assert_eq!(parent, f.path.parent().unwrap());
            calls.set(calls.get() + 1);
            anyhow::bail!("config directory sync failed");
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("config directory sync failed"));
    assert_eq!(calls.get(), 1);
    assert_eq!(fs::read(f.pending_path()).unwrap(), raw);
    assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.after);
    assert_eq!(
        fs::read(f.history_path()).unwrap(),
        pending.payload.append.as_bytes()
    );
    recover(&f.config).unwrap();
    recover(&f.config).unwrap();
    assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.after);
    assert_eq!(
        fs::read(f.history_path()).unwrap(),
        pending.payload.append.as_bytes()
    );
    assert!(!f.pending_path().exists());
    assert_eq!(f.notes(), notes);
    assert_eq!(f.decisions(), decisions);
}

#[test]
fn cleanup_directory_sync_failure_can_recover_a_reappearing_prepare() {
    let f = Fixture::new();
    f.prepare();
    let raw = fs::read(f.pending_path()).unwrap();
    let pending = f.pending();
    let notes = f.notes();
    let decisions = f.decisions();
    let store = Store::open(f.config.dir().unwrap(), false)
        .unwrap()
        .unwrap();
    let calls = std::cell::Cell::new(0);
    let error = finish(
        &f.config,
        &store,
        &pending,
        &raw,
        &|_| Ok(()),
        &|dir| {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                return Err(std::io::Error::other("cleanup directory sync failed"));
            }
            sync_directory(dir)
        },
        &config_bytes::sync_config_parent,
    )
    .unwrap_err();
    assert!(error.to_string().contains("cleanup directory sync failed"));
    assert_eq!(calls.get(), 3);
    assert!(!f.pending_path().exists());
    assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.after);
    assert_eq!(
        fs::read(f.history_path()).unwrap(),
        pending.payload.append.as_bytes()
    );
    recover(&f.config).unwrap();
    // An unsynced removal may be lost after a crash. Model that directory
    // entry returning, then verify recovery does not append a second entry.
    store.publish_prepare(PENDING, &raw).unwrap();
    recover(&f.config).unwrap();
    recover(&f.config).unwrap();
    assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.after);
    assert_eq!(
        fs::read(f.history_path()).unwrap(),
        pending.payload.append.as_bytes()
    );
    assert!(!f.pending_path().exists());
    assert_eq!(f.notes(), notes);
    assert_eq!(f.decisions(), decisions);
}

#[test]
fn every_partial_history_suffix_recovers_the_exact_single_append() {
    let f = Fixture::new();
    f.prepare();
    let raw = fs::read(f.pending_path()).unwrap();
    let pending = f.pending();
    let append = pending.payload.append.as_bytes();
    // Include boundaries within UTF-8/JSON strings, and the final newline.
    for length in [0, 1, 17, append.len() / 2, append.len() - 1, append.len()] {
        if f.pending_path().exists() {
            fs::remove_file(f.pending_path()).unwrap();
        }
        Store::open(f.config.dir().unwrap(), true)
            .unwrap()
            .unwrap()
            .publish_prepare(PENDING, &raw)
            .unwrap();
        fs::write(&f.path, &pending.payload.before).unwrap();
        fs::write(f.history_path(), &append[..length]).unwrap();
        recover(&f.config).unwrap();
        assert_eq!(fs::read(f.history_path()).unwrap(), append);
        assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.after);
    }
}

#[test]
fn recovery_retains_a_complete_prefix_and_refuses_a_changed_prefix() {
    for change_prefix in [false, true] {
        let f = Fixture::new();
        // Two decisions are recorded while the same plan is current. Build
        // the second prepared state directly: production cannot prepare it
        // after the first config changed. This isolates nonempty-prefix recovery.
        let mut request = f.request.clone();
        request.operation_id = uuid::Uuid::new_v4().to_string();
        let decision = journal::record(&f.config, &request).unwrap();
        f.prepare();
        let mut pending = f.pending();
        recover(&f.config).unwrap();
        let prefix = fs::read(f.history_path()).unwrap();
        let first: Entry = serde_json::from_slice(&prefix).unwrap();
        let mut next: Entry = serde_json::from_slice(&prefix).unwrap();
        next.payload.sequence = 2;
        next.payload.previous_checksum = first.checksum;
        next.payload.operation_id = request.operation_id.clone();
        next.payload.decision_checksum = decision.checksum;
        next.payload.request_revision = hash(&serde_json::to_vec(&request).unwrap());
        next.payload.events[0].event_id = format!("{}:0", request.operation_id);
        next.checksum = hash(&serde_json::to_vec(&next.payload).unwrap());
        pending.payload.request = request;
        pending.payload.offset = prefix.len();
        pending.payload.prefix_hash = hash(&prefix);
        pending.payload.append = format!("{}\n", serde_json::to_string(&next).unwrap());
        checksum(&mut pending);
        Store::open(f.config.dir().unwrap(), true)
            .unwrap()
            .unwrap()
            .publish_prepare(PENDING, &serde_json::to_vec(&pending).unwrap())
            .unwrap();
        let mut torn = prefix.clone();
        torn.extend_from_slice(&pending.payload.append.as_bytes()[..43]);
        if change_prefix {
            torn[0] = b'x';
        }
        fs::write(f.history_path(), &torn).unwrap();
        let result = recover(&f.config);
        if change_prefix {
            assert!(result.is_err());
            assert_eq!(fs::read(f.history_path()).unwrap(), torn);
            assert!(f.pending_path().exists());
        } else {
            result.unwrap();
            let complete = fs::read(f.history_path()).unwrap();
            assert!(complete.starts_with(&prefix));
            assert_eq!(&complete[prefix.len()..], pending.payload.append.as_bytes());
            assert!(!f.pending_path().exists());
        }
    }
}

#[test]
fn oversized_participants_refuse_before_reading_or_replacing_them() {
    for participant in ["history", "config", "pending"] {
        let f = Fixture::new();
        f.prepare();
        let path = match participant {
            "history" => f.history_path(),
            "config" => f.path.clone(),
            "pending" => f.pending_path(),
            _ => unreachable!(),
        };
        let limit = match participant {
            "history" => MAX_HISTORY,
            "config" => 64 * 1024,
            "pending" => MAX_PREPARE,
            _ => unreachable!(),
        };
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        file.set_len(limit as u64 + 1).unwrap();
        drop(file);
        assert!(recover(&f.config).is_err(), "{participant}");
        assert_eq!(fs::metadata(path).unwrap().len(), limit as u64 + 1);
        assert!(f.pending_path().exists());
    }
}

#[test]
fn stale_content_or_config_and_missing_or_changed_approval_never_prepare() {
    for change in [
        "note",
        "config",
        "missing_approval",
        "changed_request",
        "rejected",
    ] {
        let f = Fixture::new();
        let mut request = f.request.clone();
        match change {
            "note" => {
                let path = &f.notes()[0].0;
                let mut body = fs::read(path).unwrap();
                body.extend_from_slice(b"\nexternal edit");
                fs::write(path, body).unwrap();
            }
            "config" => {
                let mut body = fs::read(&f.path).unwrap();
                body.extend_from_slice(b"\n# external edit\n");
                fs::write(&f.path, body).unwrap();
            }
            "missing_approval" => {
                fs::remove_file(f.path.parent().unwrap().join("rekey/decisions.jsonl")).unwrap();
            }
            "changed_request" => {
                request.review.proposed.artifacts[0]["id"] = json!("different:{id}");
            }
            "rejected" => request.decision = Decision::Reject,
            _ => unreachable!(),
        }
        let before = fs::read(&f.path).unwrap();
        let notes = f.notes();
        assert!(commit(&f.config, &request).is_err(), "{change}");
        assert!(!f.pending_path().exists(), "{change}");
        assert!(!f.history_path().exists(), "{change}");
        assert_eq!(fs::read(&f.path).unwrap(), before);
        assert_eq!(f.notes(), notes);
    }
}

#[test]
fn unexpected_config_prevents_even_repair_of_a_matching_torn_history() {
    let f = Fixture::new();
    f.prepare();
    let pending = f.pending();
    let torn = &pending.payload.append.as_bytes()[..37];
    fs::write(f.history_path(), torn).unwrap();
    let changed = format!("{}\n# unrelated edit\n", pending.payload.before);
    fs::write(&f.path, &changed).unwrap();
    assert!(recover(&f.config).is_err());
    assert_eq!(fs::read(f.history_path()).unwrap(), torn);
    assert_eq!(fs::read_to_string(&f.path).unwrap(), changed);
    assert!(f.pending_path().exists());
}

#[test]
fn published_config_does_not_authorize_repair_of_corrupt_history() {
    let f = Fixture::new();
    f.prepare();
    let pending = f.pending();
    let prepare = fs::read(f.pending_path()).unwrap();
    let notes = f.notes();
    let decisions = f.decisions();
    fs::write(&f.path, &pending.payload.after).unwrap();
    let mut corrupt = pending.payload.append.as_bytes()[..37].to_vec();
    corrupt[0] = b'x';
    fs::write(f.history_path(), &corrupt).unwrap();
    for _ in 0..2 {
        assert!(recover(&f.config).is_err());
        assert_eq!(fs::read_to_string(&f.path).unwrap(), pending.payload.after);
        assert_eq!(fs::read(f.history_path()).unwrap(), corrupt);
        assert_eq!(fs::read(f.pending_path()).unwrap(), prepare);
        assert_eq!(f.notes(), notes);
        assert_eq!(f.decisions(), decisions);
    }
}

#[test]
fn unrelated_history_or_unprepared_torn_history_is_never_truncated_or_adopted() {
    for prepared in [false, true] {
        let f = Fixture::new();
        if prepared {
            f.prepare();
        } else {
            fs::create_dir_all(f.home.join("logs")).unwrap();
        }
        let unrelated = b"unrelated data that must remain exact\n";
        fs::write(f.history_path(), unrelated).unwrap();
        let before = fs::read(&f.path).unwrap();
        let result = if prepared {
            recover(&f.config)
        } else {
            commit(&f.config, &f.request).map(|_| ())
        };
        assert!(result.is_err());
        assert_eq!(fs::read(f.history_path()).unwrap(), unrelated);
        assert_eq!(fs::read(&f.path).unwrap(), before);
    }
}

#[test]
fn recovery_refuses_a_replaced_home_without_touching_either_folder() {
    let f = Fixture::new();
    f.prepare();
    let original = f.home.with_file_name("original-home");
    fs::rename(&f.home, &original).unwrap();
    fs::create_dir(&f.home).unwrap();
    let before = fs::read(&f.path).unwrap();
    assert!(recover(&f.config).is_err());
    assert!(fs::read_dir(&f.home).unwrap().next().is_none());
    assert!(!original.join("logs").join(HISTORY).exists());
    assert_eq!(fs::read(&f.path).unwrap(), before);
}

#[test]
fn corrupt_or_rebound_prepare_never_mutates_config_or_history() {
    for change in [
        "checksum", "path", "after", "request", "offset", "prefix", "append", "version",
    ] {
        let f = Fixture::new();
        f.prepare();
        let mut pending = f.pending();
        match change {
            "checksum" => pending.checksum = "wrong".into(),
            "path" => pending.payload.config_path.push_str(".other"),
            "after" => pending.payload.after.push_str("\n# unreviewed bytes\n"),
            "request" => pending.payload.request.operation_id = uuid::Uuid::new_v4().to_string(),
            "offset" => pending.payload.offset = usize::MAX,
            "prefix" => pending.payload.prefix_hash = hash(b"other prefix"),
            "append" => pending.payload.append.push('x'),
            "version" => pending.version = 2,
            _ => unreachable!(),
        }
        if change != "checksum" {
            checksum(&mut pending);
        }
        fs::write(f.pending_path(), serde_json::to_vec(&pending).unwrap()).unwrap();
        let before = fs::read(&f.path).unwrap();
        assert!(recover(&f.config).is_err(), "{change}");
        assert_eq!(fs::read(&f.path).unwrap(), before);
        assert!(!f.history_path().exists());
        assert!(f.pending_path().exists());
    }
}

#[test]
fn replay_requires_checksum_chain_order_home_identity_and_operation_scoped_events() {
    let f = Fixture::new();
    let entry = commit(&f.config, &f.request).unwrap();
    let original = serde_json::to_value(&entry).unwrap();
    for change in [
        "checksum",
        "sequence",
        "previous_checksum",
        "home_identity",
        "home_root",
        "operation_id",
        "action",
        "event_id",
        "type",
    ] {
        let mut value = original.clone();
        match change {
            "checksum" => value["checksum"] = json!("wrong"),
            "sequence" => value["payload"]["sequence"] = json!(2),
            "previous_checksum" => value["payload"]["previous_checksum"] = json!("wrong"),
            "home_identity" => value["payload"]["home_identity"] = json!("another folder"),
            "home_root" => value["payload"]["home_root"] = json!("/another/home"),
            "operation_id" => value["payload"]["operation_id"] = json!("invalid"),
            "action" => value["payload"]["events"][0]["action"] = json!("unalias"),
            "event_id" => value["payload"]["events"][0]["event_id"] = json!("source-key-reused"),
            "type" => value["payload"]["events"][0]["to"]["artifact_type"] = json!("other.item"),
            _ => unreachable!(),
        }
        let mut changed: Entry = serde_json::from_value(value).unwrap();
        if change != "checksum" {
            changed.checksum = hash(&serde_json::to_vec(&changed.payload).unwrap());
        }
        let bytes = format!("{}\n", serde_json::to_string(&changed).unwrap());
        assert!(
            replay(
                bytes.as_bytes(),
                &entry.payload.home_root,
                &entry.payload.home_identity
            )
            .is_err(),
            "{change}"
        );
    }
    let bytes = fs::read(f.history_path()).unwrap();
    let mut duplicate = bytes.clone();
    duplicate.extend(&bytes);
    assert!(
        replay(
            &duplicate,
            &entry.payload.home_root,
            &entry.payload.home_identity
        )
        .is_err()
    );
    assert!(
        replay(
            &bytes[..bytes.len() - 1],
            &entry.payload.home_root,
            &entry.payload.home_identity
        )
        .is_err()
    );
}

#[test]
fn history_chain_replays_move_back_without_conflicting_event_id_reuse() {
    let f = Fixture::new();
    let first = commit(&f.config, &f.request).unwrap();
    let mut second: Entry = serde_json::from_value(serde_json::to_value(&first).unwrap()).unwrap();
    second.payload.sequence = 2;
    second.payload.previous_checksum = first.checksum.clone();
    second.payload.operation_id = uuid::Uuid::new_v4().to_string();
    let event = &mut second.payload.events[0];
    std::mem::swap(&mut event.from, &mut event.to);
    event.event_id = format!("{}:0", second.payload.operation_id);
    second.checksum = hash(&serde_json::to_vec(&second.payload).unwrap());
    let bytes = format!(
        "{}\n{}\n",
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
    let entries = replay(
        bytes.as_bytes(),
        &first.payload.home_root,
        &first.payload.home_identity,
    )
    .unwrap();
    let events: Vec<_> = entries.into_iter().flat_map(|e| e.payload.events).collect();
    let resolution = Resolution::from_events(&events).unwrap();
    assert_eq!(
        resolution.canonical("new:work.example/1"),
        "old:work.example/1"
    );
    assert_eq!(
        resolution.canonical("old:work.example/1"),
        "old:work.example/1"
    );
    assert_ne!(
        resolution.history_revision("old:work.example/1"),
        Resolution::from_events(&first.payload.events)
            .unwrap()
            .history_revision("old:work.example/1")
    );
}

#[test]
fn rendering_refuses_another_origin_and_ordinary_saves_still_freeze_ids() {
    let mut f = Fixture::new();
    let before = fs::read_to_string(&f.path).unwrap();
    let mut request = f.request.review.clone();
    request.proposed.artifacts[0]["origin"] = json!("https://other.example");
    assert!(config_bytes::render(&before, &request).is_err());
    let mut next = f.config.snapshot().clone();
    next.adapters.artifacts[0]["id"] = json!("new:{host}/{id}");
    let revision = f.config.revision().to_owned();
    assert!(f.config.update(next, &revision).is_err());
    assert_eq!(fs::read_to_string(&f.path).unwrap(), before);
    assert!(!f.pending_path().exists());
}

#[cfg(unix)]
#[test]
fn symlink_participants_refuse_without_modifying_their_targets() {
    for participant in ["history", "config", "pending", "logs"] {
        let f = Fixture::new();
        f.prepare();
        let target = f.path.parent().unwrap().join("unrelated-target");
        let before = fs::read(&f.path).unwrap();
        let link = match participant {
            "history" => {
                fs::write(&target, b"unrelated history").unwrap();
                f.history_path()
            }
            "config" => {
                fs::copy(&f.path, &target).unwrap();
                fs::remove_file(&f.path).unwrap();
                f.path.clone()
            }
            "pending" => {
                fs::rename(f.pending_path(), &target).unwrap();
                f.pending_path()
            }
            "logs" => {
                fs::rename(f.home.join("logs"), &target).unwrap();
                f.home.join("logs")
            }
            _ => unreachable!(),
        };
        std::os::unix::fs::symlink(&target, link).unwrap();
        let target_bytes = fs::read(&target).ok();
        assert!(recover(&f.config).is_err(), "{participant}");
        assert_eq!(fs::read(&target).ok(), target_bytes, "{participant}");
        assert_eq!(fs::read(&f.path).unwrap(), before);
    }
}

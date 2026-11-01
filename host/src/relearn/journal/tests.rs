use super::*;
use crate::note::save_keyed_note_with_provenance;
use crate::url_provenance::StoredUrl;
use crate::vault::Vault;
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
}
fn adapter(path: &str, id: &str) -> Value {
    json!({"type":"work.item", "origin":ORIGIN, "match":[{"path":path}], "id":id})
}
fn standard() -> Value {
    adapter("/items/{id:int}", "old:{host}/{id}")
}
fn proposed() -> Vec<Value> {
    vec![adapter("/items/{id:int}", "new:{host}/{id}")]
}
impl Fixture {
    fn new(adapters: Vec<Value>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let path = temp.path().join("config.toml");
        let raw = json!({
            "root_picker_confirmed":true,
            "root_identity":crate::vault::selected_root_identity(&home).unwrap(),
            "capture_enabled":true,
            "storage":{"root":home.to_str().unwrap(),"profile":"neutral", "log_dir":"logs",
                "pages_dir":"notes","later_dir":"later","artifacts_dir":"artifacts","summaries_dir":"summaries"},
            "adapters":{"format":1},"artifacts":adapters
        });
        fs::write(&path, toml::to_string(&raw).unwrap()).unwrap();
        let config = ConfigStore::load_from(path.clone());
        assert!(config.configured());
        Self {
            _temp: temp,
            config,
            path,
            home,
        }
    }
    fn note(&self, id: &str, route: &str) {
        let vault = Vault::open_checked(
            self.config.snapshot().storage.as_ref().unwrap(),
            self.config.root_identity(),
        )
        .unwrap();
        save_keyed_note_with_provenance(
            &vault,
            &StoredUrl {
                url: format!("{ORIGIN}{route}"),
                stripped_params: Some(vec![]),
            },
            &RecordKey {
                kind: RecordKeyKind::Artifact,
                value: id.into(),
            },
            "Title",
            "Retain this exact body",
            "missing",
        )
        .unwrap();
    }
    fn log(&self, id: &str, route: &str) {
        let store = crate::capture::CaptureStore::open_in(
            self.config.snapshot().storage.as_ref().unwrap(),
            self.config.root_identity(),
            self.config.dir().unwrap(),
        )
        .unwrap();
        let event = brauser_protocol::WhyEvent {
            event_id: Uuid::new_v4().to_string(),
            artifact_id: id.into(),
            url: format!("{ORIGIN}{route}"),
            text: "Context".into(),
            occurred_at: crate::capture::fixtures::event_time()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            incognito: false,
        };
        assert!(matches!(
            store.record_why(
                crate::capture::CapturePolicy {
                    enabled: true,
                    origins: &[ORIGIN.into()],
                    strip_params: &[]
                },
                &event
            ),
            crate::capture::CaptureOutcome::Persisted { .. }
        ));
    }
    fn request(&self, proposed: Vec<Value>, decision: Decision) -> RecordRequest {
        let review = PlanRequest {
            origin: ORIGIN.into(),
            expected_revision: self.config.revision().into(),
            proposed: AdapterDocument {
                format: 1,
                artifacts: proposed,
            },
        };
        let plan = crate::relearn::plan(&self.config, &review).unwrap();
        RecordRequest {
            operation_id: Uuid::new_v4().to_string(),
            decision,
            review,
            expected_plan_revision: plan.plan_revision,
        }
    }
    fn state(&self) -> PathBuf {
        self.path.parent().unwrap().join("rekey")
    }
    fn bytes(&self) -> Vec<u8> {
        fs::read(self.state().join(JOURNAL)).unwrap()
    }
    fn pending(&self) -> Pending {
        serde_json::from_slice(&fs::read(self.state().join(PENDING)).unwrap()).unwrap()
    }
}
fn basic() -> Fixture {
    let f = Fixture::new(vec![standard()]);
    f.note("old:work.example/1", "/items/1");
    f
}
fn fail_at(phase: &'static str) -> impl Fn(&str) -> Result<()> {
    move |current| {
        ensure!(current != phase, "simulated crash at {phase}");
        Ok(())
    }
}
fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn records_revalidated_plan_with_all_revisions_without_applying_anything() {
    let f = basic();
    let request = f.request(proposed(), Decision::Approve);
    let config_bytes = fs::read(&f.path).unwrap();
    let note = fs::read_dir(f.home.join("notes"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let note_bytes = fs::read(&note).unwrap();
    let receipt = record(&f.config, &request).unwrap();
    assert!(!receipt.applied);
    let entries = inspect(&f.config).unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(
        entry.payload.plan["plan_revision"],
        request.expected_plan_revision
    );
    assert_eq!(
        entry.payload.plan["config_revision"],
        request.review.expected_revision
    );
    for field in [
        "contracts_revision",
        "evidence_revision",
        "proposed_revision",
    ] {
        assert_eq!(entry.payload.plan[field].as_str().unwrap().len(), 64);
    }
    assert_eq!(
        entry.payload.plan["identities"][0]["source"]["value"],
        "old:work.example/1"
    );
    assert_eq!(
        entry.payload.plan["identities"][0]["targets"][0]["artifact_id"],
        "new:work.example/1"
    );
    assert_eq!(fs::read(&f.path).unwrap(), config_bytes);
    assert_eq!(fs::read(note).unwrap(), note_bytes);
    assert!(!f.config.cache_dir().unwrap().exists());
    assert!(
        !f.path
            .parent()
            .unwrap()
            .join("adopted-contracts.json")
            .exists()
    );
    let mut next = f.config.snapshot().clone();
    next.adapters = request.review.proposed;
    assert!(crate::config::validate_adapter_transition(f.config.snapshot(), &next).is_err());
}

#[test]
fn exact_retry_returns_original_receipt_even_after_new_evidence() {
    let f = basic();
    let request = f.request(proposed(), Decision::Approve);
    let first = record(&f.config, &request).unwrap();
    let bytes = f.bytes();
    f.note("old:work.example/2", "/items/2");
    let restarted = ConfigStore::load_from(f.path.clone());
    let second = record(&restarted, &request).unwrap();
    assert_eq!(first.checksum, second.checksum);
    assert_eq!(f.bytes(), bytes);
    let mut conflict = request.clone();
    conflict.decision = Decision::Reject;
    assert!(
        record(&restarted, &conflict)
            .unwrap_err()
            .to_string()
            .contains("different payload")
    );
    conflict = request.clone();
    conflict.review.proposed.artifacts[0]["title"] = json!("{title} - Changed");
    assert!(record(&restarted, &conflict).is_err());
    assert_eq!(f.bytes(), bytes);
}

#[test]
fn concurrent_host_stores_serialize_distinct_decisions_and_duplicate_retries() {
    let f = basic();
    let request = f.request(proposed(), Decision::Approve);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for index in 0..6 {
            let mut request = request.clone();
            if index > 2 {
                request.operation_id = Uuid::new_v4().to_string();
            }
            let path = f.path.clone();
            handles.push(
                scope.spawn(move || record(&ConfigStore::load_from(path), &request).unwrap()),
            );
        }
        let receipts: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(receipts[0].checksum, receipts[1].checksum);
        assert_eq!(receipts[0].checksum, receipts[2].checksum);
    });
    let entries = inspect(&f.config).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries
            .iter()
            .map(|e| e.payload.sequence)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
}

#[test]
fn stale_note_config_contracts_and_proposal_refuse_before_preparation() {
    for change in ["note", "log", "config", "contracts", "proposal", "revision"] {
        let f = basic();
        let mut request = f.request(proposed(), Decision::Approve);
        match change {
            "log" => f.log("old:work.example/1", "/items/1"),
            "note" => {
                let path = fs::read_dir(f.home.join("notes"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                let mut bytes = fs::read(&path).unwrap();
                bytes.extend_from_slice(b"\nExternal edit\n");
                fs::write(path, bytes).unwrap();
            }
            "config" => {
                let bytes = fs::read_to_string(&f.path).unwrap();
                fs::write(&f.path, format!("# changed\n{bytes}")).unwrap();
            }
            "contracts" => {
                // Adopted state participates in the contracts revision, even
                // if it introduces no evidence for this page.
                let mut current = ConfigStore::load_from(f.path.clone());
                let _lock = current.lock_current().unwrap();
                current.refresh().unwrap();
            }
            "proposal" => {
                request.review.proposed.artifacts[0]["title"] = json!("{title} - Different");
            }
            _ => {
                request.expected_plan_revision = "0".repeat(64);
            }
        }
        assert!(record(&f.config, &request).is_err(), "{change}");
        assert!(!f.state().exists(), "{change} published state");
    }
}

#[test]
fn every_crash_checkpoint_recovers_once_and_retains_old_identity() {
    for phase in ["before_prepare", "prepared", "appended", "synced"] {
        let f = basic();
        let request = f.request(proposed(), Decision::Approve);
        let note_path = fs::read_dir(f.home.join("notes"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let note_before = fs::read(&note_path).unwrap();
        let config_before = fs::read(&f.path).unwrap();
        assert!(record_with(&f.config, &request, &fail_at(phase)).is_err());
        let restarted = ConfigStore::load_from(f.path.clone());
        let entries = inspect(&restarted).unwrap();
        assert_eq!(entries.len(), usize::from(phase != "before_prepare"));
        let receipt = record(&restarted, &request).unwrap();
        assert_eq!(receipt.sequence, 1);
        assert_eq!(inspect(&restarted).unwrap().len(), 1);
        assert!(!f.state().join(PENDING).exists());
        assert_eq!(fs::read(&note_path).unwrap(), note_before);
        assert_eq!(fs::read(&f.path).unwrap(), config_before);
        assert!(
            f.config.snapshot().adapters.artifacts[0]["id"]
                .as_str()
                .unwrap()
                .starts_with("old:")
        );
    }
}

#[test]
fn representative_torn_prefixes_including_mid_utf8_recover_exact_prepared_bytes() {
    let f = basic();
    let mut next = proposed();
    next[0]["title"] = json!("{title} — 工具");
    let request = f.request(next, Decision::Approve);
    assert!(record_with(&f.config, &request, &fail_at("prepared")).is_err());
    let pending_raw = fs::read(f.state().join(PENDING)).unwrap();
    let pending = f.pending();
    let utf8_middle = pending.append.find('工').unwrap() + 1;
    // UTF-8 byte boundaries need not be character boundaries when a write tears.
    for count in [
        0,
        1,
        pending.append.len() / 2,
        utf8_middle,
        pending.append.len() - 1,
        pending.append.len(),
    ] {
        private_write(&f.state().join(PENDING), &pending_raw);
        private_write(
            &f.state().join(JOURNAL),
            &pending.append.as_bytes()[..count],
        );
        assert_eq!(inspect(&f.config).unwrap().len(), 1);
        assert_eq!(f.bytes(), pending.append.as_bytes());
    }
}

#[test]
fn recovery_does_not_need_still_current_evidence_or_a_mounted_home() {
    let f = basic();
    let request = f.request(proposed(), Decision::Approve);
    assert!(record_with(&f.config, &request, &fail_at("prepared")).is_err());
    fs::rename(&f.home, f.home.with_file_name("unmounted")).unwrap();
    fs::write(&f.path, "# temporarily broken configuration\n").unwrap();
    let restarted = ConfigStore::load_from(f.path.clone());
    let entries = inspect(&restarted).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].payload.home_root, f.home.to_str().unwrap());
    assert!(!entries[0].receipt().applied);
    assert_eq!(record(&restarted, &request).unwrap().sequence, 1);
}

#[test]
fn a_second_prepared_operation_preserves_the_first_committed_prefix() {
    let f = basic();
    let first = f.request(proposed(), Decision::Approve);
    record(&f.config, &first).unwrap();
    let bytes = f.bytes();
    let second = f.request(proposed(), Decision::Reject);
    assert!(record_with(&f.config, &second, &fail_at("prepared")).is_err());
    let pending = f.pending();
    let mut torn = bytes.clone();
    torn.extend_from_slice(&pending.append.as_bytes()[..200]);
    private_write(&f.state().join(JOURNAL), &torn);
    let entries = inspect(&f.config).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].payload.previous_checksum, entries[0].checksum);
    assert!(f.bytes().starts_with(&bytes));
    assert_eq!(record(&f.config, &second).unwrap().sequence, 2);
}

#[test]
fn changed_prefix_tail_or_prepare_never_get_repaired_by_guessing() {
    for change in ["prefix", "tail", "hash", "version", "offset", "payload"] {
        let f = basic();
        record(&f.config, &f.request(proposed(), Decision::Approve)).unwrap();
        let first = f.bytes();
        let request = f.request(proposed(), Decision::Reject);
        assert!(record_with(&f.config, &request, &fail_at("prepared")).is_err());
        let mut pending = f.pending();
        match change {
            "prefix" => {
                let mut bad = first.clone();
                bad[0] = b'!';
                private_write(&f.state().join(JOURNAL), &bad);
            }
            "tail" => {
                let mut bad = first.clone();
                bad.extend_from_slice(b"wrong");
                private_write(&f.state().join(JOURNAL), &bad);
            }
            "hash" => {
                pending.append_hash = "0".repeat(64);
            }
            "version" => {
                pending.version = 2;
            }
            "offset" => {
                pending.offset = usize::MAX;
            }
            _ => {
                pending.append = pending
                    .append
                    .replace("new:work.example/1", "bad:work.example/1");
                pending.append_hash = hash(pending.append.as_bytes());
            }
        }
        private_write(
            &f.state().join(PENDING),
            &serde_json::to_vec(&pending).unwrap(),
        );
        let before = f.bytes();
        assert!(inspect(&f.config).is_err(), "{change}");
        assert_eq!(f.bytes(), before);
        assert!(f.state().join(PENDING).exists());
    }
}

#[test]
fn unprepared_torn_and_tampered_complete_entries_refuse() {
    let f = basic();
    record(&f.config, &f.request(proposed(), Decision::Approve)).unwrap();
    let complete = f.bytes();
    for bad in [
        complete[..complete.len() - 1].to_vec(),
        complete
            .iter()
            .copied()
            .chain(complete.iter().copied())
            .collect(),
        b"{}\n".to_vec(),
    ] {
        private_write(&f.state().join(JOURNAL), &bad);
        assert!(inspect(&f.config).is_err());
        assert_eq!(f.bytes(), bad);
    }
}

#[test]
fn rejects_ambiguous_splits_merges_and_swaps_without_alias_flattening() {
    let cases = [
        (
            vec![adapter("/items/{id:int}/{_}", "old:{host}/{id}")],
            vec![
                ("old:work.example/1", "/items/1/10"),
                ("old:work.example/1", "/items/1/20"),
            ],
            vec![adapter("/items/{_}/{part:int}", "new:{host}/{part}")],
            "split",
        ),
        (
            vec![standard()],
            vec![
                ("old:work.example/1", "/items/1"),
                ("old:work.example/2", "/items/2"),
            ],
            vec![adapter("/items/(1|2)", "new:{host}/one")],
            "merged",
        ),
        (
            vec![
                adapter("/a/{id:int}", "old:{host}/a/{id}"),
                adapter("/b/{id:int}", "old:{host}/b/{id}"),
            ],
            vec![
                ("old:work.example/a/1", "/a/1"),
                ("old:work.example/b/1", "/b/1"),
            ],
            vec![
                adapter("/a/{id:int}", "old:{host}/b/{id}"),
                adapter("/b/{id:int}", "old:{host}/a/{id}"),
            ],
            "simultaneous_key_reuse",
        ),
    ];
    for (old, notes, new, issue) in cases {
        let f = Fixture::new(old);
        // Log both routes, including a split whose notes can only retain one
        // canonical address. These are real capture markers, not supplied IDs.
        for (id, route) in notes {
            f.log(id, route);
        }
        let mut request = f.request(new, Decision::Approve);
        assert!(record(&f.config, &request).is_err(), "{issue}");
        assert!(!f.state().exists());
        request.decision = Decision::Reject;
        record(&f.config, &request).unwrap();
        let entry = inspect(&f.config).unwrap().remove(0);
        assert_eq!(entry.payload.request.decision, Decision::Reject);
        assert!(
            entry.payload.plan["identities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| {
                    ["issues", "classifications"]
                        .iter()
                        .any(|key| row[key].as_array().unwrap().contains(&json!(issue)))
                })
        );
        assert!(!entry.receipt().applied);
    }
}

#[test]
fn inspection_of_absent_journal_creates_no_rekey_directory() {
    let f = basic();
    assert!(inspect(&f.config).unwrap().is_empty());
    assert!(!f.state().exists());
}

#[test]
fn unpublished_prepare_temporary_is_never_promoted_into_a_decision() {
    let f = basic();
    let request = f.request(proposed(), Decision::Approve);
    assert!(record_with(&f.config, &request, &fail_at("prepared")).is_err());
    // A crash before the no-clobber hard link leaves only a temporary.
    let temporary = f.state().join(format!(".prepare-{}.tmp", Uuid::new_v4()));
    fs::rename(f.state().join(PENDING), &temporary).unwrap();
    assert!(inspect(&f.config).unwrap().is_empty());
    assert!(temporary.exists());
    assert_eq!(record(&f.config, &request).unwrap().sequence, 1);
    assert_eq!(inspect(&f.config).unwrap().len(), 1);
}

#[test]
fn oversized_or_nonregular_state_refuses_without_replacement() {
    let f = basic();
    record(&f.config, &f.request(proposed(), Decision::Approve)).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(f.state().join(JOURNAL))
        .unwrap()
        .set_len(MAX_JOURNAL as u64 + 1)
        .unwrap();
    assert!(inspect(&f.config).is_err());
    fs::remove_file(f.state().join(JOURNAL)).unwrap();
    fs::create_dir(f.state().join(JOURNAL)).unwrap();
    assert!(inspect(&f.config).is_err());
    assert!(f.state().join(JOURNAL).is_dir());
}

#[cfg(unix)]
#[test]
fn symlinked_journal_directory_or_files_never_supply_replay_state() {
    use std::os::unix::fs::symlink;
    for name in ["rekey", JOURNAL, PENDING] {
        let f = basic();
        record(&f.config, &f.request(proposed(), Decision::Approve)).unwrap();
        let target = if name == "rekey" {
            f.state()
        } else {
            f.state().join(name)
        };
        let outside = f.path.parent().unwrap().join("other");
        if target.exists() {
            fs::rename(&target, &outside).unwrap();
        } else {
            private_write(&outside, b"{}");
        }
        symlink(&outside, &target).unwrap();
        assert!(inspect(&f.config).is_err(), "{name}");
    }
}

#[test]
fn invalid_operation_ids_never_create_journal_state() {
    let f = basic();
    let mut request = f.request(proposed(), Decision::Approve);
    for id in [
        "../../other",
        "not-a-uuid",
        "ABCDEF00-0000-0000-0000-000000000001",
    ] {
        request.operation_id = id.into();
        assert!(record(&f.config, &request).is_err());
        assert!(!f.state().exists());
    }
}

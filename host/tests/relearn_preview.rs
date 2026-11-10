//! Exercise the read-only projected-history boundary through the real binary.
use brauser_host::{note, url_provenance::StoredUrl, vault::Vault};
use brauser_protocol::{ActivityEvent, RecordKey, RecordKeyKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    _temp: tempfile::TempDir,
    config: PathBuf,
    home: PathBuf,
    request: Value,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir(&home).unwrap();
        let config = temp.path().join("config.toml");
        let raw = json!({"root_picker_confirmed":true,"root_identity":folder_identity(&home),
            "storage":{"root":home.to_str().unwrap(),"profile":"neutral","log_dir":"logs","pages_dir":"notes","later_dir":"later","artifacts_dir":"artifacts","summaries_dir":"summaries"},
            "adapters":{"format":1},"artifacts":[{"type":"work.item","origin":"https://work.example","match":[{"path":"/items/{id:int}"}],"id":"old:{host}/{id}"}]});
        let bytes = toml::to_string(&raw).unwrap();
        fs::write(&config, &bytes).unwrap();
        let request = json!({"origin":"https://work.example","expected_revision":format!("sha256:{}",hash(bytes.as_bytes())),
            "proposed":{"format":1,"artifacts":[{"type":"work.item","origin":"https://work.example","match":[{"path":"/items/{id:int}"}],"id":"new:{host}/{id}"}]}});
        let mut storage = raw["storage"].clone();
        storage["knowledge_base_dir"] = Value::Null;
        let vault = Vault::open(&serde_json::from_value(storage).unwrap()).unwrap();
        let key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "old:work.example/1".into(),
        };
        let url = "https://work.example/items/1";
        note::save_keyed_note_with_provenance(
            &vault,
            &StoredUrl {
                url: url.into(),
                stripped_params: Some(vec![]),
            },
            &key,
            "Keep",
            "Original note body",
            "missing",
        )
        .unwrap();
        fs::create_dir_all(home.join("summaries")).unwrap();
        fs::write(home.join("summaries").join(format!("{}.summary.md", brauser_host::vault::note_filename(&key, url).unwrap().strip_suffix(".md").unwrap())),format!(
            "---\nbrauser:\n  kind: summary\n  canonical_url: {url:?}\n  artifact_id: {:?}\n  stripped_params: []\n---\n\nOriginal summary body\n",key.value)).unwrap();
        let activity = ActivityEvent {
            focus_session: None,
            event_id: "00000000-0000-4000-8000-000000000001".into(),
            artifact_id: key.value,
            artifact_type: "work.item".into(),
            url: url.into(),
            title: Some("One".into()),
            occurred_at: "2026-11-02T10:00:00Z".into(),
            incognito: false,
            opens: 1,
            focused_seconds: 137,
            background_seconds: 11,
            modes: vec![],
            trails: vec![],
            refs: vec![],
        };
        fs::create_dir_all(home.join("logs")).unwrap();
        fs::write(home.join("logs/day.md"),format!("<!-- brauser:daily-log v1 -->\n<!-- brauser:visit id={} url={} at={} kind=activity data={} -->\n",
            hash(activity.event_id.as_bytes()),hash(url.as_bytes()),activity.occurred_at,
            hex::encode(serde_json::to_vec(&activity).unwrap()))).unwrap();
        Self {
            _temp: temp,
            config,
            home,
            request,
        }
    }
    fn run(&self, extra: &[&str], request: &Value) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_brauser"))
            .args(["adapters", "relearn-preview", "--config"])
            .arg(&self.config)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes());
        child.wait_with_output().unwrap()
    }
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn folder_identity(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(path).unwrap();
        format!("unix:{:016x}:{:016x}", m.dev(), m.ino())
    }
    #[cfg(windows)]
    {
        let handle = cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())
            .unwrap()
            .into_std_file();
        let m = winapi_util::file::information(&handle).unwrap();
        format!(
            "windows:{:08x}:{:016x}",
            m.volume_serial_number(),
            m.file_index()
        )
    }
}
fn inventory(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = vec![];
    for entry in fs::read_dir(path).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            files.extend(inventory(&p));
        } else {
            files.push((p.clone(), fs::read(p).unwrap()));
        }
    }
    files.sort();
    files
}

fn shifted_capture_fixture() -> Fixture {
    let mut f = Fixture::new();
    // This fixture records only activities under the two-capture rule.
    fs::remove_dir_all(f.home.join("notes")).unwrap();
    fs::remove_dir_all(f.home.join("summaries")).unwrap();
    let mut raw: Value = toml::from_str(&fs::read_to_string(&f.config).unwrap()).unwrap();
    raw["artifacts"][0]["match"] = json!([{"path":"/items/{id:jira_key}/{_}"}]);
    raw["artifacts"][0]["id"] = json!("old:{host}/{id}");
    let bytes = toml::to_string(&raw).unwrap();
    fs::write(&f.config, &bytes).unwrap();
    f.request["expected_revision"] = json!(format!("sha256:{}", hash(bytes.as_bytes())));
    f.request["proposed"]["artifacts"] = raw["artifacts"].clone();
    f.request["proposed"]["artifacts"][0]["match"] = json!([{"path":"/items/{_}/{id:jira_key}"}]);
    f.request["proposed"]["artifacts"][0]["id"] = json!("new:{host}/{id}");

    let mut log = String::from("<!-- brauser:daily-log v1 -->\n");
    for (n, (first, second, refs)) in [
        ("A-1", "B-2", vec![]),
        ("B-2", "C-3", vec!["B-2"]), // A recorded self-reference.
        ("X-4", "Y-5", vec!["B-2", "B-2", "X-4", "UNKNOWN-9"]),
    ]
    .into_iter()
    .enumerate()
    {
        let event = ActivityEvent {
            focus_session: None,
            event_id: format!("00000000-0000-4000-8000-{:012}", n + 1),
            artifact_id: format!("old:work.example/{first}"),
            artifact_type: "work.item".into(),
            url: format!("https://work.example/items/{first}/{second}"),
            title: Some(first.into()),
            occurred_at: "2026-11-02T10:00:00Z".into(),
            incognito: false,
            opens: 1,
            focused_seconds: 10,
            background_seconds: 0,
            modes: vec![],
            trails: vec![],
            refs: refs
                .into_iter()
                .map(|value| brauser_protocol::ActivityRef {
                    artifact_type: "work.item".into(),
                    value: value.into(),
                })
                .collect(),
        };
        log.push_str(&format!(
            "<!-- brauser:visit id={} url={} at={} kind=activity data={} -->\n",
            hash(event.event_id.as_bytes()),
            hash(event.url.as_bytes()),
            event.occurred_at,
            hex::encode(serde_json::to_vec(&event).unwrap())
        ));
    }
    fs::write(f.home.join("logs/day.md"), log).unwrap();
    f
}

fn assert_shifted_mentions(report: &Value) {
    let projection = &report["projection"];
    let edges = projection["edges"].as_array().unwrap();
    assert!(
        edges
            .iter()
            .any(|edge| edge["source"] == "new:work.example/Y-5"
                && edge["target"] == "new:work.example/C-3"
                && edge["kind"] == "mentions"
                && edge["weight"] == 1),
        "{edges:?}"
    );
    assert!(
        edges.iter().all(
            |edge| edge["target"] != "new:work.example/B-2" && edge["source"] != edge["target"]
        ),
        "{edges:?}"
    );
    assert_eq!(edges.len(), 2, "duplicate and self mentions add no edges");
    assert!(
        edges
            .iter()
            .any(|edge| edge["source"] == "new:work.example/Y-5"
                && edge["target"] == "old:work.example/UNKNOWN-9"
                && edge["weight"] == 1)
    );
    // Unknown targets stay unlinked; they must not borrow an existing row.
    for (id, count) in [("B-2", 0), ("C-3", 1), ("Y-5", 1)] {
        let artifact = projection["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["artifact_id"] == format!("new:work.example/{id}"))
            .unwrap();
        assert_eq!(artifact["linked_count"], count, "{artifact}");
    }
}

#[test]
fn real_cli_preview_resolves_refs_before_shifting_id_captures() {
    let f = shifted_capture_fixture();
    let before = inventory(&f.home);
    let config = fs::read(&f.config).unwrap();
    let output = f.run(&[], &f.request);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_shifted_mentions(&serde_json::from_slice(&output.stdout).unwrap());
    assert_eq!(inventory(&f.home), before);
    assert_eq!(fs::read(&f.config).unwrap(), config);
}

#[test]
fn real_cli_projects_existing_history_without_changing_any_config_or_content() {
    let f = Fixture::new();
    let before = inventory(&f.home);
    let config = fs::read(&f.config).unwrap();
    let first = f.run(&[], &f.request);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let report: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["review_only"], true);
    assert_eq!(
        report["plan"]["identities"][0]["source"]["value"],
        "old:work.example/1"
    );
    assert_eq!(
        report["projection"]["artifacts"][0]["artifact_id"],
        "new:work.example/1"
    );
    assert_eq!(report["projection"]["artifacts"][0]["focused_seconds"], 137);
    assert_eq!(
        report["projection"]["recent"][0]["artifact_id"],
        "new:work.example/1"
    );
    assert_eq!(report["projection"]["worklog"]["artifact_count"], 1);
    assert_eq!(
        f.run(&[], &f.request).stdout,
        first.stdout,
        "deterministic across processes"
    );
    assert_eq!(inventory(&f.home), before);
    assert_eq!(fs::read(&f.config).unwrap(), config);
    for path in ["cache", "rekey", "adopted-contracts.json"] {
        assert!(!f.config.parent().unwrap().join(path).exists(), "{path}");
    }
}
#[test]
fn real_preview_refuses_stale_foreign_alias_inputs_apply_flags_and_damaged_history() {
    let f = Fixture::new();
    let before = inventory(&f.home);
    let mut stale = f.request.clone();
    stale["expected_revision"] = json!("stale");
    let mut forged = f.request.clone();
    forged["aliases"] = json!([]);
    let mut foreign = f.request.clone();
    foreign["proposed"]["artifacts"][0]["origin"] = json!("https://other.example");
    for (extra, request) in [
        (&[][..], stale),
        (&[][..], forged),
        (&[][..], foreign),
        (&["--apply"][..], f.request.clone()),
    ] {
        let output = f.run(extra, &request);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    assert_eq!(inventory(&f.home), before);
    let log = f.home.join("logs/day.md");
    let mut bytes = fs::read(&log).unwrap();
    bytes.extend_from_slice(b"<!-- brauser:visit id=torn");
    fs::write(&log, &bytes).unwrap();
    let output = f.run(&[], &f.request);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("damaged owned log marker"));
    assert_eq!(fs::read(log).unwrap(), bytes);
}

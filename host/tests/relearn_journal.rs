//! Real binary checks for the developer-only journal boundary.
use brauser_host::{note, url_provenance::StoredUrl, vault::Vault};
use brauser_protocol::{RecordKey, RecordKeyKind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn configured_home() -> (tempfile::TempDir, std::path::PathBuf, Vault) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let config_path = temp.path().join("config.toml");
    let raw = json!({"root_picker_confirmed":true,"root_identity":folder_identity(&home),
        "storage":{"root":home.to_str().unwrap(),"profile":"neutral","log_dir":"logs","pages_dir":"notes","later_dir":"later","artifacts_dir":"artifacts","summaries_dir":"summaries"},
        "adapters":{"format":1},"artifacts":[{"type":"work.item","origin":"https://work.example","match":[{"path":"/items/{id:int}"}],"id":"old:{host}/{id}"}]});
    fs::write(&config_path, toml::to_string(&raw).unwrap()).unwrap();
    let mut storage_json = raw["storage"].clone();
    storage_json["knowledge_base_dir"] = Value::Null;
    let storage = serde_json::from_value(storage_json).unwrap();
    (temp, config_path, Vault::open(&storage).unwrap())
}

fn folder_identity(home: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(home).unwrap();
        format!("unix:{:016x}:{:016x}", meta.dev(), meta.ino())
    }
    #[cfg(windows)]
    {
        let handle = cap_std::fs::Dir::open_ambient_dir(home, cap_std::ambient_authority())
            .unwrap()
            .into_std_file();
        let info = winapi_util::file::information(&handle).unwrap();
        format!(
            "windows:{:08x}:{:016x}",
            info.volume_serial_number(),
            info.file_index()
        )
    }
}

fn command(config: &Path, name: &str, extra: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_brauser"))
        .args(["adapters", name, "--config"])
        .arg(config)
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(input);
    child.wait_with_output().unwrap()
}
fn success(config: &Path, name: &str, request: &Value) -> Value {
    let output = command(config, name, &[], request.to_string().as_bytes());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn separate_processes_plan_record_retry_and_inspect_without_changing_identity() {
    let (temp, config_path, vault) = configured_home();
    let home = temp.path().join("home");
    // A confirmed fixture uses the platform identity format of the picker.
    note::save_keyed_note_with_provenance(
        &vault,
        &StoredUrl {
            url: "https://work.example/items/1".into(),
            stripped_params: Some(vec![]),
        },
        &RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "old:work.example/1".into(),
        },
        "Title",
        "Keep this note",
        "missing",
    )
    .unwrap();
    let config_before = fs::read(&config_path).unwrap();
    let note_path = fs::read_dir(home.join("notes"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let note_before = fs::read(&note_path).unwrap();
    let revision = format!("sha256:{}", hex::encode(Sha256::digest(&config_before)));
    let review = json!({"origin":"https://work.example","expected_revision":revision,
        "proposed":{"format":1,"artifacts":[{"type":"work.item","origin":"https://work.example","match":[{"path":"/items/{id:int}"}],"id":"new:{host}/{id}"}]}});
    let plan = success(&config_path, "relearn-plan", &review);
    let request = json!({"operation_id":"00000000-0000-0000-0000-000000000001","decision":"approve","review":review,"expected_plan_revision":plan["plan_revision"]});
    let first = success(&config_path, "relearn-record", &request);
    let retry = success(&config_path, "relearn-record", &request);
    assert_eq!(first, retry);
    assert_eq!(first["applied"], false);
    let journal = success(&config_path, "relearn-journal", &json!(null));
    assert_eq!(journal["entries"].as_array().unwrap().len(), 1);
    assert_eq!(journal["entries"][0]["payload"]["plan"], plan);
    assert_eq!(fs::read(&config_path).unwrap(), config_before);
    assert_eq!(fs::read(note_path).unwrap(), note_before);
    assert!(!temp.path().join("cache").exists());
    assert!(!temp.path().join("adopted-contracts.json").exists());
    assert_eq!(fs::read_dir(home.join("notes")).unwrap().count(), 1);
}

#[test]
fn torn_why_marker_cannot_produce_or_approve_an_empty_plan() {
    let (temp, config_path, _vault) = configured_home();
    let config_before = fs::read(&config_path).unwrap();
    let review = json!({"origin":"https://work.example",
        "expected_revision":format!("sha256:{}", hex::encode(Sha256::digest(&config_before))),
        "proposed":{"format":1,"artifacts":[{"type":"work.item","origin":"https://work.example","match":[{"path":"/items/{id:int}"}],"id":"new:{host}/{id}"}]}});
    let empty = success(&config_path, "relearn-plan", &review);
    assert_eq!(empty["identities"], json!([]));
    let event = brauser_protocol::WhyEvent {
        event_id: "00000000-0000-0000-0000-000000000001".into(),
        artifact_id: "old:work.example/1".into(),
        url: "https://work.example/items/1".into(),
        text: "Keep this context".into(),
        occurred_at: "2026-11-02T10:00:00Z".into(),
        incognito: false,
    };
    let marker = format!(
        "<!-- brauser:visit id={} url={} at={} kind=why data={} -->\n",
        hex::encode(Sha256::digest(event.event_id.as_bytes())),
        hex::encode(Sha256::digest(event.url.as_bytes())),
        event.occurred_at,
        hex::encode(serde_json::to_vec(&event).unwrap())
    );
    let logs = temp.path().join("home/logs");
    fs::create_dir_all(&logs).unwrap();
    let path = logs.join("2026-11-02.md");
    fs::write(&path, format!("<!-- brauser:daily-log v1 -->\n{marker}")).unwrap();
    assert_eq!(
        success(&config_path, "relearn-plan", &review)["identities"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let damaged = format!(
        "<!-- brauser:daily-log v1 -->\n{}",
        &marker[..marker.len() - 40]
    );
    fs::write(&path, &damaged).unwrap();
    let decision = json!({"operation_id":"00000000-0000-0000-0000-000000000002",
        "decision":"approve","review":review,"expected_plan_revision":empty["plan_revision"]});
    for (name, input) in [("relearn-plan", &review), ("relearn-record", &decision)] {
        let output = command(&config_path, name, &[], input.to_string().as_bytes());
        assert!(!output.status.success(), "{name} accepted damaged history");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("damaged owned log marker"));
    }
    assert_eq!(fs::read(&config_path).unwrap(), config_before);
    assert_eq!(fs::read_to_string(path).unwrap(), damaged);
    assert!(!temp.path().join("cache").exists());
    assert!(!temp.path().join("rekey").exists());
}

#[test]
fn forged_inputs_and_apply_flags_fail_before_opening_configuration() {
    for (extra,input) in [
        (vec!["--apply"],b"{}".as_slice()),
        (vec![],br#"{"operation_id":"00000000-0000-0000-0000-000000000001","decision":"approve","review":{},"expected_plan_revision":"bad","aliases":[]}"#.as_slice())
    ] {
        let temp = tempfile::tempdir().unwrap();
        let output = command(&temp.path().join("config.toml"),"relearn-record",&extra,input);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(),0);
    }
}

//! Exercise the real, read-only developer CLI with an isolated support root.
//! These are ordinary child processes, with no sockets or real harnesses.
use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

fn run(input: &Value, extra: &[&str]) -> Output {
    let root = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_brauser"))
        .args(["adapters", "aliases-preview"])
        .args(extra)
        .current_dir(root.path())
        .env("HOME", root.path())
        .env("USERPROFILE", root.path())
        .env("APPDATA", root.path())
        .env("LOCALAPPDATA", root.path())
        .env("XDG_CONFIG_HOME", root.path())
        .env("XDG_CACHE_HOME", root.path())
        .env("BRAUSER_TEST_CONFIG_PATH", root.path().join("config.toml"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A usage error may have already closed stdin.
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes());
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        fs::read_dir(root.path()).unwrap().count(),
        0,
        "preview must not create config, cache, logs or notes"
    );
    output
}

fn request() -> Value {
    json!({"before": {"format": 1, "artifacts": [{
        "type": "work.item", "origin": "https://work.example",
        "match": [{"path": "/items/{id:int}"}], "id": "work:{host}/{id}"
    }]}, "events": [{
        "event_id": "00000000-0000-0000-0000-000000000001", "action": "confirm",
        "from": {"url": "https://work.example/items/1", "artifact_id": "work:work.example/1"},
        "to": {"url": "https://work.example/items/2", "artifact_id": "work:work.example/2"}
    }]})
}

#[test]
fn the_binary_previews_aliases_without_creating_any_support_or_vault_files() {
    let output = run(&request(), &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["review_only"], true);
    assert_eq!(
        result["groups"][0]["canonical"]["artifact_id"],
        "work:work.example/2"
    );
    assert_eq!(result["groups"][0]["members"].as_array().unwrap().len(), 2);
    assert!(output.stderr.is_empty());
}

#[test]
fn a_forged_endpoint_fails_without_partial_json_output() {
    let mut request = request();
    request["events"][0]["to"]["artifact_id"] = json!("work:work.example/other");
    let output = run(&request, &[]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("endpoint ID does not match host recognition")
    );
}

#[test]
fn unexpected_arguments_fail_before_reading_requests() {
    let output = run(&request(), &["--apply"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("takes no arguments"));
}

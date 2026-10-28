//! Real CLI boundary checks: rejected commands never open config or vault.
use std::io::Write;
use std::process::{Command, Stdio};

fn rejected(args: &[&str], input: &[u8]) -> String {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    let mut child = Command::new(env!("CARGO_BIN_EXE_brauser"))
        .args(["adapters", "relearn-plan", "--config"])
        .arg(&config)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(input);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn an_apply_flag_is_rejected_before_reading_configuration() {
    assert!(rejected(&["--apply"], b"{}").contains("unexpected relearn-plan arguments"));
}

#[test]
fn forged_or_oversized_requests_cannot_supply_an_impact_plan() {
    assert!(
        rejected(&[], br#"{"origin":"https://work.example","evidence":[]}"#)
            .contains("invalid re-learning request")
    );
    assert!(rejected(&[], &vec![b' '; 4 * 1024 * 1024 + 1]).contains("request is too large"));
}

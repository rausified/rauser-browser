//! Test the process-level config seam without changing this test process's
//! environment or reading the user's config. Production probes only print paths.
use std::process::Command;
#[cfg(feature = "scripted-dialogs")]
use std::process::Stdio;

const HOST: &str = env!("CARGO_BIN_EXE_brauser");

#[test]
fn config_path_override_is_only_available_in_scripted_builds() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("isolated.toml");
    let output = Command::new(HOST)
        .arg("--config-path")
        .env("BRAUSER_TEST_CONFIG_PATH", &config)
        .output()
        .unwrap();
    assert!(output.status.success());
    let reported = std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    #[cfg(feature = "scripted-dialogs")]
    assert_eq!(reported, config);
    #[cfg(not(feature = "scripted-dialogs"))]
    {
        let default = Command::new(HOST)
            .arg("--config-path")
            .env_remove("BRAUSER_TEST_CONFIG_PATH")
            .output()
            .unwrap();
        assert!(default.status.success());
        assert_eq!(
            reported.to_string_lossy(),
            String::from_utf8_lossy(&default.stdout).trim()
        );
        assert_ne!(reported, config);
    }
}

#[test]
#[cfg(feature = "scripted-dialogs")]
fn scripted_host_reads_only_the_injected_config() {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("isolated.toml");
    // A distinctive, invalid config proves serve() really loaded this file.
    // No chosen home means startup cannot touch a vault or start a harness.
    std::fs::write(&config, "not valid TOML {{{").unwrap();
    let before = std::fs::read(&config).unwrap();
    let mut child = Command::new(HOST)
        .arg("serve")
        .env("BRAUSER_TEST_CONFIG_PATH", &config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = serde_json::json!({
        "type": "get_config", "protocol_version": brauser_protocol::PROTOCOL_VERSION,
        "request_id": "isolation",
    })
    .to_string();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&(request.len() as u32).to_ne_bytes())
        .unwrap();
    stdin.write_all(request.as_bytes()).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let length = u32::from_ne_bytes(output.stdout[..4].try_into().unwrap()) as usize;
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout[4..4 + length]).unwrap();
    assert_eq!(reply["type"], "config_result");
    assert!(
        reply["config_issue"]
            .as_str()
            .is_some_and(|issue| issue.contains("Configuration is invalid")),
        "{reply}"
    );
    assert!(reply["config"]["storage"].is_null(), "{reply}");
    assert_eq!(
        reply["revision"],
        format!("sha256:{:x}", Sha256::digest(&before))
    );
    assert_eq!(std::fs::read(&config).unwrap(), before);
}

#[test]
#[cfg(feature = "scripted-dialogs")]
fn scripted_cache_uses_injected_config_directory() {
    // Run the assertion in a separate process so parallel tests never share
    // environment overrides. The parent only starts a child with a temp path.
    const EXPECTED_CACHE: &str = "BRAUSER_TEST_EXPECT_CACHE";
    if let Some(expected) = std::env::var_os(EXPECTED_CACHE) {
        let store = brauser_host::config::ConfigStore::load().unwrap();
        assert_eq!(
            store.cache_dir().unwrap(),
            std::path::PathBuf::from(expected)
        );
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "scripted_cache_uses_injected_config_directory"])
        .env(
            "BRAUSER_TEST_CONFIG_PATH",
            directory.path().join("config.toml"),
        )
        .env(EXPECTED_CACHE, directory.path().join("cache"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
#[cfg(feature = "scripted-dialogs")]
fn scripted_config_override_fails_closed_for_empty_or_relative_paths() {
    for path in ["", "config.toml"] {
        let output = Command::new(HOST)
            .arg("serve")
            .env("BRAUSER_TEST_CONFIG_PATH", path)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("absolute config file"));
    }
}

use brauser_protocol::{Request, Response};

mod common;
use common::{config, update};

fn with_agent(agent: &str) -> String {
    update(&config(r#""storage":null,"#).replace(r#""agent":null"#, &format!(r#""agent":{agent}"#)))
}

#[test]
fn agent_config_requires_every_key_and_rejects_unknown_fields() {
    let agent = r#"{"harness":"claude","binary":"/usr/local/bin/claude","version":"2.1.284","timeout_secs":600,"probed_at":"2026-10-27T10:00:00Z"}"#;
    assert!(serde_json::from_str::<Request>(&with_agent(agent)).is_ok());
    let unknown_version = agent.replace(r#""version":"2.1.284""#, r#""version":null"#);
    assert!(serde_json::from_str::<Request>(&with_agent(&unknown_version)).is_ok());
    for key in [
        r#""binary":"/usr/local/bin/claude","#,
        r#""version":"2.1.284","#,
        r#""timeout_secs":600,"#,
    ] {
        assert!(
            serde_json::from_str::<Request>(&with_agent(&agent.replace(key, ""))).is_err(),
            "{key}"
        );
    }
    let extra = agent.replace(
        r#""timeout_secs":600"#,
        r#""timeout_secs":600,"args":["-p"]"#,
    );
    assert!(serde_json::from_str::<Request>(&with_agent(&extra)).is_err());
}

#[test]
fn agent_config_rejects_the_v5_harness_names() {
    for harness in ["claude_code", "claude-code"] {
        let agent = format!(
            r#"{{"harness":"{harness}","binary":"/usr/local/bin/claude","version":null,"timeout_secs":600,"probed_at":"2026-10-27T10:00:00Z"}}"#
        );
        assert!(
            serde_json::from_str::<Request>(&with_agent(&agent)).is_err(),
            "{harness}"
        );
    }
}

#[test]
fn v6_retires_check_agent() {
    let check = r#"{"type":"check_agent","protocol_version":8,"request_id":"r1"}"#;
    assert!(serde_json::from_str::<Request>(check).is_err());
    let checked = r#"{"type":"agent_checked","protocol_version":8,"request_id":"r1","harness_id":"claude-code","harness_version":"2.1.284"}"#;
    assert!(serde_json::from_str::<Response>(checked).is_err());
}

#[test]
fn set_up_harness_requires_every_key() {
    let full = r#"{"type":"set_up_harness","protocol_version":8,"request_id":"r1","expected_revision":"missing","harness":"claude","timeout_secs":null,"summaries_dir":null}"#;
    assert!(serde_json::from_str::<Request>(full).is_ok());
    let timed = full.replace(r#""timeout_secs":null"#, r#""timeout_secs":900"#);
    assert!(serde_json::from_str::<Request>(&timed).is_ok());
    for key in [
        r#","expected_revision":"missing""#,
        r#","harness":"claude""#,
        r#","timeout_secs":null"#,
        r#","summaries_dir":null"#,
    ] {
        assert!(
            serde_json::from_str::<Request>(&full.replace(key, "")).is_err(),
            "{key}"
        );
    }
    // The extension never names a binary or environment names.
    for extra in [r#","binary":"/bin/sh""#, r#","env_names":["HOME"]"#] {
        let with_extra = full.replace(
            r#","summaries_dir":null"#,
            &format!("{extra},\"summaries_dir\":null"),
        );
        assert!(
            serde_json::from_str::<Request>(&with_extra).is_err(),
            "{extra}"
        );
    }
    let v5 = full.replace(r#""harness":"claude""#, r#""harness_id":"claude-code""#);
    assert!(serde_json::from_str::<Request>(&v5).is_err());
}

#[test]
fn v6_retires_confirm_harness_setup_and_offer_ids() {
    let confirm = r#"{"type":"confirm_harness_setup","protocol_version":8,"request_id":"r1","expected_revision":"missing","offer_id":"o1","env_names":["HOME"],"summaries_dir":null}"#;
    assert!(serde_json::from_str::<Request>(confirm).is_err());
    let body = config(r#""storage":null,"#);
    let confirmed = format!(
        r#"{{"type":"harness_setup_confirmed","protocol_version":8,"request_id":"r1","harness_token":"h1","config":{body},"summary":"Set up"}}"#
    );
    assert!(serde_json::from_str::<Response>(&confirmed).is_err());
    let discover = r#"{"type":"discover_harnesses","protocol_version":8,"request_id":"r1"}"#;
    assert!(serde_json::from_str::<Request>(discover).is_ok());
    let with_revision = r#"{"type":"discover_harnesses","protocol_version":8,"request_id":"r1","expected_revision":"missing"}"#;
    assert!(serde_json::from_str::<Request>(with_revision).is_err());
}

#[test]
fn harnesses_discovered_requires_nullable_keys() {
    let found = r#"{"harness":"codex","binary":"/usr/local/bin/codex","version":null,"problem":"codex --version output was not recognized"}"#;
    let message = |found: &str| {
        format!(
            r#"{{"type":"harnesses_discovered","protocol_version":8,"request_id":"r1","harnesses":[{found}]}}"#
        )
    };
    assert!(serde_json::from_str::<Response>(&message(found)).is_ok());
    let missing = r#"{"harness":"claude","binary":null,"version":null,"problem":null}"#;
    assert!(serde_json::from_str::<Response>(&message(missing)).is_ok());
    for key in [r#""binary":"/usr/local/bin/codex","#, r#""version":null,"#] {
        assert!(
            serde_json::from_str::<Response>(&message(&found.replace(key, ""))).is_err(),
            "{key}"
        );
    }
    let with_refusal = found.replace(r#""problem""#, r#""refusal":null,"problem""#);
    assert!(serde_json::from_str::<Response>(&message(&with_refusal)).is_err());
    let v5 =
        r#"{"type":"harnesses_discovered","protocol_version":8,"request_id":"r1","offers":[]}"#;
    assert!(serde_json::from_str::<Response>(v5).is_err());
}

#[test]
fn open_path_and_agent_env_messages_round_trip() {
    for (target, run_id) in [
        ("codex_thread", r#""00000000-0000-4000-8000-000000000001""#),
        ("agent_env", "null"),
    ] {
        let request = format!(
            r#"{{"type":"open_path","protocol_version":8,"request_id":"r","target":"{target}","run_id":{run_id}}}"#
        );
        let parsed: Request = serde_json::from_str(&request).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), request);
    }
    // There is no summary target, and run_id is required.
    let summary = r#"{"type":"open_path","protocol_version":8,"request_id":"r","target":"summary","run_id":null}"#;
    assert!(serde_json::from_str::<Request>(summary).is_err());
    let no_run =
        r#"{"type":"open_path","protocol_version":8,"request_id":"r","target":"agent_env"}"#;
    assert!(serde_json::from_str::<Request>(no_run).is_err());
    let opened = r#"{"type":"path_opened","protocol_version":8,"request_id":"r"}"#;
    assert!(serde_json::from_str::<Response>(opened).is_ok());

    let get = r#"{"type":"get_agent_env","protocol_version":8,"request_id":"r"}"#;
    assert!(serde_json::from_str::<Request>(get).is_ok());
    let listed = r#"{"type":"agent_env_listed","protocol_version":8,"request_id":"r","names":["FOO"],"problem":null}"#;
    let parsed: Response = serde_json::from_str(listed).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), listed);
    assert!(serde_json::from_str::<Response>(&listed.replace(r#","problem":null"#, "")).is_err());
}

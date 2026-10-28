use brauser_protocol::{Request, Response};

mod common;
use common::{config, update};

fn storage(summaries: Option<&str>) -> String {
    let summaries = summaries.map_or(String::new(), |value| {
        format!(r#","summaries_dir":{value}"#)
    });
    format!(
        r#""storage":{{"root":"/tmp/notes","profile":"neutral","log_dir":"log","pages_dir":"pages","later_dir":"later","artifacts_dir":"artifacts"{summaries},"knowledge_base_dir":null}},"#
    )
}

#[test]
fn omitted_storage_is_not_interpreted_as_explicit_null() {
    assert!(serde_json::from_str::<Request>(&update(&config(""))).is_err());
    assert!(serde_json::from_str::<Request>(&update(&config(r#""storage":null,"#))).is_ok());
}

#[test]
fn config_updates_require_a_revision_precondition() {
    let missing_revision = format!(
        r#"{{"type":"update_config","protocol_version":8,"request_id":"r1","picker_token":null,"config":{}}}"#,
        config(r#""storage":null,"#)
    );
    assert!(serde_json::from_str::<Request>(&missing_revision).is_err());
}

#[test]
fn v6_retires_config_confirmation() {
    let with_consent = update(&config(r#""storage":null,"#)).replace(
        r#""picker_token":null"#,
        r#""picker_token":null,"consent_token":null"#,
    );
    assert!(serde_json::from_str::<Request>(&with_consent).is_err());
    let confirm = format!(
        r#"{{"type":"confirm_config","protocol_version":8,"request_id":"r1","expected_revision":"missing","picker_token":null,"config":{}}}"#,
        config(r#""storage":null,"#)
    );
    assert!(serde_json::from_str::<Request>(&confirm).is_err());
}

#[test]
fn config_responses_always_report_the_revision() {
    for kind in ["config_result", "config_updated"] {
        let extra = if kind == "config_result" {
            r#","config_issue":null,"agent_status":{"state":"not_set_up","harness_version":null,"message":null},"adapter_issue":null,"adapter_warnings":[],"adopted_state":{"issue":null,"revision":"missing","warnings":[],"full":false}"#
        } else {
            ""
        };
        let body = config(r#""storage":null,"#);
        let missing = format!(
            r#"{{"type":"{kind}","protocol_version":8,"request_id":"r1"{extra},"config":{body}}}"#
        );
        assert!(serde_json::from_str::<Response>(&missing).is_err());

        let present = format!(
            r#"{{"type":"{kind}","protocol_version":8,"request_id":"r1","revision":"missing"{extra},"config":{body}}}"#
        );
        assert!(serde_json::from_str::<Response>(&present).is_ok());
    }
}

#[test]
fn v6_config_requires_summaries_dir_key() {
    assert!(serde_json::from_str::<Request>(&update(&config(&storage(None)))).is_err());
    assert!(serde_json::from_str::<Request>(&update(&config(&storage(Some("null"))))).is_ok());
    assert!(
        serde_json::from_str::<Request>(&update(&config(&storage(Some(r#""summaries""#))))).is_ok()
    );
}

#[test]
fn v6_config_requires_knowledge_base_dir_key() {
    let complete = storage(Some("null"));
    assert!(serde_json::from_str::<Request>(&update(&config(&complete))).is_ok());
    let chosen = complete.replace(
        r#""knowledge_base_dir":null"#,
        r#""knowledge_base_dir":"/tmp/kb""#,
    );
    assert!(serde_json::from_str::<Request>(&update(&config(&chosen))).is_ok());
    let omitted = complete.replace(r#","knowledge_base_dir":null"#, "");
    assert!(serde_json::from_str::<Request>(&update(&config(&omitted))).is_err());
}

#[test]
fn update_config_requires_knowledge_base_token_key() {
    let plain = update(&config(r#""storage":null,"#));
    let omitted = plain.replace(r#","knowledge_base_token":null"#, "");
    assert!(serde_json::from_str::<Request>(&omitted).is_err());
    let with_token = plain.replace(
        r#""knowledge_base_token":null"#,
        r#""knowledge_base_token":"t""#,
    );
    assert!(serde_json::from_str::<Request>(&with_token).is_ok());
}

#[test]
fn choose_folder_requires_purpose() {
    let base = r#"{"type":"choose_folder","protocol_version":8,"request_id":"r1""#;
    assert!(serde_json::from_str::<Request>(&format!("{base}}}")).is_err());
    for purpose in ["notes_folder", "knowledge_base"] {
        let request = format!(r#"{base},"purpose":"{purpose}"}}"#);
        assert!(
            serde_json::from_str::<Request>(&request).is_ok(),
            "{purpose} was refused"
        );
    }
    let unknown = format!(r#"{base},"purpose":"summaries"}}"#);
    assert!(serde_json::from_str::<Request>(&unknown).is_err());
}

#[test]
fn v6_config_requires_artifacts_dir_key() {
    let complete = storage(Some("null"));
    assert!(serde_json::from_str::<Request>(&update(&config(&complete))).is_ok());
    assert!(
        serde_json::from_str::<Request>(&update(&config(
            &complete.replace(r#","artifacts_dir":"artifacts""#, "")
        )))
        .is_err()
    );
}

fn config_with_extra(key: &str) -> String {
    let value = if key == "agent_denylist" {
        r#"["bank.example"]"#
    } else {
        "false"
    };
    let body = config(r#""storage":null,"#).replacen('{', &format!(r#"{{"{key}":{value},"#), 1);
    format!(
        r#"{{"type":"config_result","protocol_version":8,"request_id":"r1","revision":"missing","config_issue":null,"agent_status":{{"state":"not_set_up","harness_version":null,"message":null}},"adapter_issue":null,"adapter_warnings":[],"adopted_state":{{"issue":null,"revision":"missing","warnings":[],"full":false}},"config":{body}}}"#
    )
}

#[test]
fn v6_config_rejects_retired_privacy_keys() {
    assert!(serde_json::from_str::<Response>(&config_with_extra("capture_enabled_x")).is_err());
    let plain = config_with_extra("x").replace(r#""x":false,"#, "");
    assert!(serde_json::from_str::<Response>(&plain).is_ok(), "{plain}");
    for key in [
        "agent_denylist",
        "agent_denylist_confirmed",
        "log_incognito",
    ] {
        assert!(serde_json::from_str::<Response>(&config_with_extra(key)).is_err());
    }
}

#[test]
fn v6_config_rejects_sites() {
    let plain = update(&config(r#""storage":null,"#));
    assert!(serde_json::from_str::<Request>(&plain).is_ok(), "{plain}");
    let with_sites = plain.replace(
        r#""capture_enabled":false,"#,
        r#""capture_enabled":false,"sites":[{"origin":"https://work.example","path_prefix":"/"}],"#,
    );
    assert_ne!(with_sites, plain);
    assert!(serde_json::from_str::<Request>(&with_sites).is_err());
    let empty_sites = plain.replace(
        r#""capture_enabled":false,"#,
        r#""capture_enabled":false,"sites":[],"#,
    );
    assert!(serde_json::from_str::<Request>(&empty_sites).is_err());
}

#[test]
fn v6_config_requires_agent_and_adapter_keys() {
    for key in [
        r#""agent":null,"#,
        r#","adapters":{"format":1,"artifacts":[]}"#,
    ] {
        let without = config(r#""storage":null,"#).replace(key, "");
        assert_ne!(
            without,
            config(r#""storage":null,"#),
            "{key} was not removed"
        );
        assert!(
            serde_json::from_str::<Request>(&update(&without)).is_err(),
            "config without {key} was accepted"
        );
    }
}

#[test]
fn config_requires_summary_vocabulary() {
    let plain = config(r#""storage":null,"#);
    assert!(
        serde_json::from_str::<Request>(&update(&plain)).is_ok(),
        "{plain}"
    );
    let vocabulary = r#""summary_vocabulary":{"document_types":["report"],"categories":["news"]},"#;
    for (from, to) in [
        (vocabulary, ""),
        (r#""document_types":["report"],"#, ""),
        (r#","categories":["news"]"#, ""),
        (
            r#""categories":["news"]"#,
            r#""categories":["news"],"tags":[]"#,
        ),
    ] {
        let changed = plain.replace(from, to);
        assert_ne!(changed, plain, "{from} was not replaced");
        assert!(
            serde_json::from_str::<Request>(&update(&changed)).is_err(),
            "{changed}"
        );
    }
}

#[test]
fn update_config_refuses_a_harness_token() {
    let plain = update(&config(r#""storage":null,"#));
    assert!(serde_json::from_str::<Request>(&plain).is_ok());
    let with_token = plain.replace(
        r#""picker_token":null"#,
        r#""picker_token":null,"harness_token":null"#,
    );
    assert!(serde_json::from_str::<Request>(&with_token).is_err());
}

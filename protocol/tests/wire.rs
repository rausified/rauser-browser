use brauser_protocol::{Request, Response};

// Every agent and adapter key is required on the wire, like `storage`.
const AGENT_AND_ADAPTERS: &str = r#""agent":null,"adapters":{"format":1,"artifacts":[]}"#;

fn config(storage: &str) -> String {
    format!(
        r#"{{{storage}"capture_enabled":false,"sites":[],"strip_params":["utm_*","fbclid","gclid"],{AGENT_AND_ADAPTERS}}}"#
    )
}

fn update(config: &str) -> String {
    format!(
        r#"{{"type":"update_config","protocol_version":6,"request_id":"r1","expected_revision":"missing","picker_token":null,"consent_token":null,"harness_token":null,"config":{config}}}"#
    )
}

fn storage(summaries: Option<&str>) -> String {
    let summaries = summaries.map_or(String::new(), |value| {
        format!(r#","summaries_dir":{value}"#)
    });
    format!(
        r#""storage":{{"root":"/tmp/notes","profile":"neutral","log_dir":"log","pages_dir":"pages","later_dir":"later","artifacts_dir":"artifacts"{summaries}}},"#
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
        r#"{{"type":"update_config","protocol_version":6,"request_id":"r1","picker_token":null,"consent_token":null,"harness_token":null,"config":{}}}"#,
        config(r#""storage":null,"#)
    );
    assert!(serde_json::from_str::<Request>(&missing_revision).is_err());
}

#[test]
fn config_responses_always_report_the_revision() {
    for kind in ["config_result", "config_updated"] {
        let extra = if kind == "config_result" {
            r#","config_issue":null,"agent_status":{"state":"not_set_up","harness_version":null,"message":null},"adapter_issue":null,"adapter_warnings":[]"#
        } else {
            ""
        };
        let body = config(r#""storage":null,"#);
        let missing = format!(
            r#"{{"type":"{kind}","protocol_version":6,"request_id":"r1"{extra},"config":{body}}}"#
        );
        assert!(serde_json::from_str::<Response>(&missing).is_err());

        let present = format!(
            r#"{{"type":"{kind}","protocol_version":6,"request_id":"r1","revision":"missing"{extra},"config":{body}}}"#
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
        r#"{{"type":"config_result","protocol_version":6,"request_id":"r1","revision":"missing","config_issue":null,"agent_status":{{"state":"not_set_up","harness_version":null,"message":null}},"adapter_issue":null,"adapter_warnings":[],"config":{body}}}"#
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
fn update_config_requires_harness_token_key() {
    let without = update(&config(r#""storage":null,"#)).replace(r#""harness_token":null,"#, "");
    assert!(serde_json::from_str::<Request>(&without).is_err());
}

fn with_agent(agent: &str) -> String {
    update(&config(r#""storage":null,"#).replace(r#""agent":null"#, &format!(r#""agent":{agent}"#)))
}

#[test]
fn agent_config_rejects_unknown_fields() {
    let agent = r#"{"harness_id":"claude-code","adapter":"claude_code","binary":"/usr/local/bin/claude","args":["-p","{prompt}"],"env_allow":["HOME"],"timeout_secs":120}"#;
    assert!(serde_json::from_str::<Request>(&with_agent(agent)).is_ok());
    let extra = agent.replace(
        r#""timeout_secs":120"#,
        r#""timeout_secs":120,"stdin":"page""#,
    );
    assert!(serde_json::from_str::<Request>(&with_agent(&extra)).is_err());
}

#[test]
fn harness_adapter_rejects_hyphenated_value() {
    let agent = r#"{"harness_id":"claude-code","adapter":"claude-code","binary":"/usr/local/bin/claude","args":[],"env_allow":[],"timeout_secs":120}"#;
    assert!(serde_json::from_str::<Request>(&with_agent(agent)).is_err());
}

#[test]
fn check_agent_carries_no_url() {
    let plain = r#"{"type":"check_agent","protocol_version":6,"request_id":"r1"}"#;
    assert!(serde_json::from_str::<Request>(plain).is_ok());
    let with_url = r#"{"type":"check_agent","protocol_version":6,"request_id":"r1","url":null}"#;
    assert!(serde_json::from_str::<Request>(with_url).is_err());
}

#[test]
fn confirm_harness_setup_requires_every_key() {
    let full = r#"{"type":"confirm_harness_setup","protocol_version":6,"request_id":"r1","expected_revision":"missing","offer_id":"o1","env_names":["HOME"],"summaries_dir":null}"#;
    assert!(serde_json::from_str::<Request>(full).is_ok());
    for key in [
        r#","offer_id":"o1""#,
        r#","env_names":["HOME"]"#,
        r#","summaries_dir":null"#,
    ] {
        assert!(
            serde_json::from_str::<Request>(&full.replace(key, "")).is_err(),
            "{key}"
        );
    }
    // The extension never names a binary; an extra path key is refused.
    let with_binary = full.replace(
        r#""offer_id":"o1""#,
        r#""offer_id":"o1","binary":"/bin/sh""#,
    );
    assert!(serde_json::from_str::<Request>(&with_binary).is_err());
}

#[test]
fn harnesses_discovered_offers_require_nullable_keys() {
    let offer = r#"{"offer_id":null,"adapter":"codex","harness_id":"codex","binary":"/usr/local/bin/codex","real_path":null,"version":null,"args":[],"env_required":["HOME","PATH"],"env_optional":[{"name":"OPENAI_API_KEY","present":false}],"refusal":"codex --version output was not recognized"}"#;
    let message = |offer: &str| {
        format!(
            r#"{{"type":"harnesses_discovered","protocol_version":6,"request_id":"r1","offers":[{offer}]}}"#
        )
    };
    assert!(serde_json::from_str::<Response>(&message(offer)).is_ok());
    for key in [
        r#""offer_id":null,"#,
        r#""real_path":null,"#,
        r#""version":null,"#,
    ] {
        assert!(
            serde_json::from_str::<Response>(&message(&offer.replace(key, ""))).is_err(),
            "{key}"
        );
    }
    let with_help = offer.replace(r#""args":[]"#, r#""args":[],"help":"usage""#);
    assert!(serde_json::from_str::<Response>(&message(&with_help)).is_err());
}

#[test]
fn harness_setup_confirmed_carries_token_config_and_summary() {
    let body = config(r#""storage":null,"#);
    let present = format!(
        r#"{{"type":"harness_setup_confirmed","protocol_version":6,"request_id":"r1","harness_token":"h1","config":{body},"summary":"Set up"}}"#
    );
    assert!(serde_json::from_str::<Response>(&present).is_ok());
    let without = present.replace(r#""harness_token":"h1","#, "");
    assert!(serde_json::from_str::<Response>(&without).is_err());
    let discover = r#"{"type":"discover_harnesses","protocol_version":6,"request_id":"r1","expected_revision":"missing"}"#;
    assert!(serde_json::from_str::<Request>(discover).is_ok());
}

#[test]
fn activity_and_why_messages_require_complete_bounded_shapes() {
    let event = r#"{"event_id":"00000000-0000-4000-8000-000000000001","artifact_id":"work:example/7","artifact_type":"work.item","url":"https://work.example/items/7","title":"Item 7","occurred_at":"2026-10-16T10:00:00+00:00","incognito":false,"opens":1,"focused_seconds":42,"background_seconds":5,"modes":[{"mode":"view","url":"https://work.example/items/7"}],"trails":[{"from_artifact_id":null,"from_url":null,"transition":"forward_back"}],"refs":[]}"#;
    let request = format!(
        r#"{{"type":"record_activity","protocol_version":6,"request_id":"r1","event":{event}}}"#
    );
    assert!(serde_json::from_str::<Request>(&request).is_ok());
    assert!(serde_json::from_str::<Request>(&request.replace(r#","opens":1"#, "")).is_err());
    assert!(
        serde_json::from_str::<Request>(&request.replace(
            r#""transition":"forward_back""#,
            r#""transition":"forward_back","page_text":"secret""#,
        ))
        .is_err()
    );

    let why = r#"{"type":"record_why","protocol_version":6,"request_id":"r2","event":{"event_id":"00000000-0000-4000-8000-000000000002","artifact_id":"work:example/7","url":"https://work.example/items/7","occurred_at":"2026-10-16T10:02:00+00:00","incognito":false,"text":"Release blocker"}}"#;
    assert!(serde_json::from_str::<Request>(why).is_ok());
    assert!(
        serde_json::from_str::<Request>(&why.replace(r#","text":"Release blocker""#, "")).is_err()
    );

    for kind in ["activity_recorded", "why_recorded"] {
        let response = format!(
            r#"{{"type":"{kind}","protocol_version":6,"request_id":"r1","event_id":"e1","outcome":"persisted","reason":null,"relative_path":"log/day.md"}}"#
        );
        assert!(serde_json::from_str::<Response>(&response).is_ok());
    }
}

#[test]
fn capture_batch_requires_typed_items_and_correlated_results() {
    let visit = r#"{"event_id":"00000000-0000-4000-8000-000000000001","url":"https://work.example/items/7","title":"Item 7","occurred_at":"2026-10-16T10:00:00+00:00","incognito":false}"#;
    let request = format!(
        r#"{{"type":"record_batch","protocol_version":6,"request_id":"r1","batch_id":"00000000-0000-4000-8000-000000000099","events":[{{"kind":"visit","visit":{visit},"activity":null}}],"overflow":{{"count":2,"first_occurred_at":"2026-10-16T08:00:00+00:00","last_occurred_at":"2026-10-16T09:00:00+00:00"}}}}"#
    );
    assert!(serde_json::from_str::<Request>(&request).is_ok());
    assert!(
        serde_json::from_str::<Request>(
            &request.replace(r#""activity":null"#, r#""activity":{visit}"#)
        )
        .is_err()
    );

    let response = r#"{"type":"batch_recorded","protocol_version":6,"request_id":"r1","batch_id":"00000000-0000-4000-8000-000000000099","results":[{"event_id":"00000000-0000-4000-8000-000000000001","outcome":"persisted","reason":null,"relative_path":"log/day.md"}],"overflow_recorded":true}"#;
    assert!(serde_json::from_str::<Response>(response).is_ok());
    assert!(
        serde_json::from_str::<Response>(&response.replace(r#","overflow_recorded":true"#, ""))
            .is_err()
    );
}

#[test]
fn artifact_queries_and_note_keys_have_complete_shapes() {
    for request in [
        r#"{"type":"get_artifact","protocol_version":6,"request_id":"r1","url":"https://work.example/items/7","title":"Item 7"}"#,
        r#"{"type":"list_artifacts","protocol_version":6,"request_id":"r2","artifact_type":null,"title_query":null,"limit":200}"#,
        r#"{"type":"worklog","protocol_version":6,"request_id":"r3","start_date":"2026-10-12","end_date":"2026-10-17"}"#,
        r#"{"type":"refresh_artifacts","protocol_version":6,"request_id":"r4"}"#,
        r#"{"type":"use_artifact_note","protocol_version":6,"request_id":"r5","url":"https://work.example/items/7","source_url":"https://work.example/board?selected=7","expected_revision":"sha256:abc"}"#,
    ] {
        assert!(
            serde_json::from_str::<Request>(request).is_ok(),
            "{request}"
        );
    }

    let save = r#"{"type":"save_note","protocol_version":6,"request_id":"r6","url":"https://work.example/items/7","title":"Item","body":"text","expected_revision":"missing","expected_key":{"kind":"artifact","value":"work:work.example/7"}}"#;
    assert!(serde_json::from_str::<Request>(save).is_ok());
    assert!(
        serde_json::from_str::<Request>(&save.replace(
            ",\"expected_key\":{\"kind\":\"artifact\",\"value\":\"work:work.example/7\"}",
            ""
        ))
        .is_err()
    );
}

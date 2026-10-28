use brauser_protocol::{AdoptedState, Request, Response};
use brauser_protocol::{HarnessId, RunKind, UncheckedReason};
use brauser_protocol::{ScopeChoiceView, ScopeValues, SiteDrafter, UnscopedChoice};
use serde_json::{Value, json};

// Every agent, vocabulary and adapter key is required on the wire, like `storage`.
const AGENT_AND_ADAPTERS: &str = concat!(
    r#""agent":null,"#,
    r#""summary_vocabulary":{"document_types":["report"],"categories":["news"]},"#,
    r#""adapters":{"format":1,"artifacts":[]}"#,
);

fn config(storage: &str) -> String {
    format!(
        r#"{{{storage}"capture_enabled":false,"strip_params":["utm_*","fbclid","gclid"],{AGENT_AND_ADAPTERS}}}"#
    )
}

fn update(config: &str) -> String {
    format!(
        r#"{{"type":"update_config","protocol_version":8,"request_id":"r1","expected_revision":"missing","picker_token":null,"knowledge_base_token":null,"config":{config}}}"#
    )
}

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
fn activity_and_why_messages_require_complete_bounded_shapes() {
    let event = r#"{"event_id":"00000000-0000-4000-8000-000000000001","artifact_id":"work:example/7","artifact_type":"work.item","url":"https://work.example/items/7","title":"Item 7","occurred_at":"2026-10-16T10:00:00+00:00","incognito":false,"opens":1,"focused_seconds":42,"background_seconds":5,"modes":[{"mode":"view","url":"https://work.example/items/7"}],"trails":[{"from_artifact_id":null,"from_url":null,"transition":"forward_back"}],"refs":[]}"#;
    let request = format!(
        r#"{{"type":"record_activity","protocol_version":8,"request_id":"r1","event":{event}}}"#
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

    let why = r#"{"type":"record_why","protocol_version":8,"request_id":"r2","event":{"event_id":"00000000-0000-4000-8000-000000000002","artifact_id":"work:example/7","url":"https://work.example/items/7","occurred_at":"2026-10-16T10:02:00+00:00","incognito":false,"text":"Release blocker"}}"#;
    assert!(serde_json::from_str::<Request>(why).is_ok());
    assert!(
        serde_json::from_str::<Request>(&why.replace(r#","text":"Release blocker""#, "")).is_err()
    );

    for kind in ["activity_recorded", "why_recorded"] {
        let response = format!(
            r#"{{"type":"{kind}","protocol_version":8,"request_id":"r1","event_id":"e1","outcome":"persisted","reason":null,"relative_path":"log/day.md"}}"#
        );
        assert!(serde_json::from_str::<Response>(&response).is_ok());
    }
}

#[test]
fn capture_batch_requires_typed_items_and_correlated_results() {
    let visit = r#"{"event_id":"00000000-0000-4000-8000-000000000001","url":"https://work.example/items/7","title":"Item 7","occurred_at":"2026-10-16T10:00:00+00:00","incognito":false}"#;
    let request = format!(
        r#"{{"type":"record_batch","protocol_version":8,"request_id":"r1","batch_id":"00000000-0000-4000-8000-000000000099","events":[{{"kind":"visit","visit":{visit},"activity":null}}],"overflow":{{"count":2,"first_occurred_at":"2026-10-16T08:00:00+00:00","last_occurred_at":"2026-10-16T09:00:00+00:00"}}}}"#
    );
    assert!(serde_json::from_str::<Request>(&request).is_ok());
    assert!(
        serde_json::from_str::<Request>(
            &request.replace(r#""activity":null"#, r#""activity":{visit}"#)
        )
        .is_err()
    );

    let response = r#"{"type":"batch_recorded","protocol_version":8,"request_id":"r1","batch_id":"00000000-0000-4000-8000-000000000099","results":[{"event_id":"00000000-0000-4000-8000-000000000001","outcome":"persisted","reason":null,"relative_path":"log/day.md"}],"overflow_recorded":true}"#;
    assert!(serde_json::from_str::<Response>(response).is_ok());
    assert!(
        serde_json::from_str::<Response>(&response.replace(r#","overflow_recorded":true"#, ""))
            .is_err()
    );
}

#[test]
fn standalone_overflow_batches_round_trip_without_a_url_or_path() {
    let value = json!({
        "type": "record_batch", "protocol_version": 8, "request_id": "overflow-only",
        "batch_id": "00000000-0000-4000-8000-000000000099", "events": [],
        "overflow": { "count": 2, "first_occurred_at": "2026-10-28T08:00:00+00:00",
            "last_occurred_at": "2026-10-28T09:00:00+00:00" }
    });
    let request: Request = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), value);
    let mut forged = value;
    forged["overflow"]["url"] = json!("file:///tmp/unowned");
    assert!(serde_json::from_value::<Request>(forged).is_err());
}

#[test]
fn v8_artifact_incoming_trails_are_required_bounded_and_known() {
    let trail = json!({
        "source": { "artifact_id": "work:example/1", "artifact_type": "work.item",
            "title": "Source", "url": "https://work.example/items/1" },
        "transition": "link", "last_seen": "2026-10-28"
    });
    let artifact = json!({
        "artifact_id": "work:example/7", "artifact_type": "work.item",
        "title": "Target", "url": "https://work.example/items/7",
        "first_seen": "2026-10-28", "last_seen": "2026-10-28",
        "focused_seconds": 42, "background_seconds": 5, "opens": 1,
        "days_opened": 1, "last_mode": null, "why": null, "linked_count": 1,
        "within": null, "contained": [], "incoming_trails": [trail.clone()]
    });
    assert!(schema_subset::valid("ArtifactView", &artifact));
    let parsed: brauser_protocol::ArtifactView = serde_json::from_value(artifact.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), artifact);
    let mut missing = artifact.clone();
    missing.as_object_mut().unwrap().remove("incoming_trails");
    assert!(!schema_subset::valid("ArtifactView", &missing));
    assert!(serde_json::from_value::<brauser_protocol::ArtifactView>(missing).is_err());
    let mut bounded = artifact.clone();
    bounded["incoming_trails"] = json!(vec![trail.clone(); 5]);
    assert!(schema_subset::valid("ArtifactView", &bounded));
    bounded["incoming_trails"] = json!(vec![trail; 6]);
    assert!(!schema_subset::valid("ArtifactView", &bounded));
    for transition in [
        "forward_back",
        "unknown",
        "mentions",
        "within",
        "co-focused",
    ] {
        let mut invalid = artifact.clone();
        invalid["incoming_trails"][0]["transition"] = json!(transition);
        assert!(
            !schema_subset::valid("ArtifactView", &invalid),
            "{transition}"
        );
    }
}

#[test]
fn artifact_queries_and_note_keys_have_complete_shapes() {
    for request in [
        r#"{"type":"get_artifact","protocol_version":8,"request_id":"r1","url":"https://work.example/items/7","title":"Item 7"}"#,
        r#"{"type":"list_artifacts","protocol_version":8,"request_id":"r2","artifact_type":null,"title_query":null,"limit":200}"#,
        r#"{"type":"worklog","protocol_version":8,"request_id":"r3","start_date":"2026-10-12","end_date":"2026-10-17"}"#,
        r#"{"type":"refresh_artifacts","protocol_version":8,"request_id":"r4"}"#,
        r#"{"type":"use_artifact_note","protocol_version":8,"request_id":"r5","url":"https://work.example/items/7","source_url":"https://work.example/board?selected=7","expected_revision":"sha256:abc"}"#,
    ] {
        assert!(
            serde_json::from_str::<Request>(request).is_ok(),
            "{request}"
        );
    }

    let save = r#"{"type":"save_note","protocol_version":8,"request_id":"r6","url":"https://work.example/items/7","title":"Item","body":"text","expected_revision":"missing","expected_key":{"kind":"artifact","value":"work:work.example/7"}}"#;
    assert!(serde_json::from_str::<Request>(save).is_ok());
    assert!(
        serde_json::from_str::<Request>(&save.replace(
            ",\"expected_key\":{\"kind\":\"artifact\",\"value\":\"work:work.example/7\"}",
            ""
        ))
        .is_err()
    );
}

const RUN_VIEW: &str = r#"{"run_id":"00000000-0000-4000-8000-000000000001","state":"running","harness":"codex","record_key":{"kind":"url","value":"https://a.example/x"},"written_record_key":null,"url":"https://a.example/x","title":"T","cwd":null,"created_at":"2026-10-26T14:00:00-07:00","started_at":null,"finished_at":null,"error":null,"error_kind":null,"thread_id":null,"session_id":null,"summary_path":null,"stream_bytes":0,"can_retry_save":false,"answer":null}"#;

fn run_fetched(view: &str) -> String {
    format!(
        r#"{{"type":"run_fetched","protocol_version":8,"request_id":"r","run":{view},"stream_text":"x","next_offset":1,"more":false}}"#
    )
}

#[test]
fn start_run_requires_nullable_selection_harness_record_key_and_requested_at() {
    let ok = r#"{"type":"start_run","protocol_version":8,"request_id":"r","url":"https://a.example/x","title":"T","content":"body","selection":null,"harness":null,"record_key":null,"requested_at":"2026-10-26T14:00:00-07:00"}"#;
    assert!(serde_json::from_str::<Request>(ok).is_ok());
    assert!(serde_json::from_str::<Request>(&ok.replace(r#","record_key":null"#, "")).is_err());
    assert!(
        serde_json::from_str::<Request>(
            &ok.replace(r#","requested_at":"2026-10-26T14:00:00-07:00""#, "")
        )
        .is_err()
    );
    for key in [r#","selection":null"#, r#","harness":null"#] {
        assert!(
            serde_json::from_str::<Request>(&ok.replace(key, "")).is_err(),
            "{key}"
        );
    }
    let chosen = ok
        .replace(r#""selection":null"#, r#""selection":"quote""#)
        .replace(r#""harness":null"#, r#""harness":"claude""#)
        .replace(
            r#""record_key":null"#,
            r#""record_key":{"kind":"artifact","value":"work:a/7"}"#,
        );
    assert!(serde_json::from_str::<Request>(&chosen).is_ok(), "{chosen}");
    let unknown_harness = ok.replace(r#""harness":null"#, r#""harness":"claude_code""#);
    assert!(serde_json::from_str::<Request>(&unknown_harness).is_err());
}

#[test]
fn run_view_round_trips_every_state_and_error_kind() {
    for state in ["queued", "running", "succeeded", "failed", "canceled"] {
        let view = RUN_VIEW.replace(r#""state":"running""#, &format!(r#""state":"{state}""#));
        let message = run_fetched(&view);
        let parsed: Response = serde_json::from_str(&message).expect(state);
        let encoded = serde_json::to_string(&parsed).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&encoded).unwrap(), parsed);
        assert!(encoded.contains(&format!(r#""state":"{state}""#)));
    }
    for kind in [
        "harness_failed",
        "timed_out",
        "canceled",
        "service_restarted",
        "invalid_output",
        "save_failed",
    ] {
        let view = RUN_VIEW
            .replace(r#""state":"running""#, r#""state":"failed""#)
            .replace(r#""error":null"#, r#""error":"it failed""#)
            .replace(r#""error_kind":null"#, &format!(r#""error_kind":"{kind}""#));
        let parsed: Response = serde_json::from_str(&run_fetched(&view)).expect(kind);
        let encoded = serde_json::to_string(&parsed).unwrap();
        assert!(encoded.contains(&format!(r#""error_kind":"{kind}""#)));
    }
    assert!(
        serde_json::from_str::<Response>(&run_fetched(
            &RUN_VIEW.replace(r#""state":"running""#, r#""state":"paused""#)
        ))
        .is_err()
    );
    for key in [
        r#","written_record_key":null"#,
        r#","cwd":null"#,
        r#","started_at":null"#,
        r#","finished_at":null"#,
        r#","error":null"#,
        r#","error_kind":null"#,
        r#","thread_id":null"#,
        r#","session_id":null"#,
        r#","summary_path":null"#,
        r#","stream_bytes":0"#,
        r#","can_retry_save":false"#,
        r#","answer":null"#,
    ] {
        assert!(
            serde_json::from_str::<Response>(&run_fetched(&RUN_VIEW.replace(key, ""))).is_err(),
            "{key}"
        );
    }
    let run = |kind: &str, extra: &str| {
        format!(
            r#"{{"type":"{kind}","protocol_version":8,"request_id":"r","run":{RUN_VIEW}{extra}}}"#
        )
    };
    assert!(serde_json::from_str::<Response>(&run("run_started", r#","existing":false"#)).is_ok());
    assert!(serde_json::from_str::<Response>(&run("run_started", "")).is_err());
    assert!(serde_json::from_str::<Response>(&run("run_updated", "")).is_ok());
    let listed = format!(
        r#"{{"type":"runs_listed","protocol_version":8,"request_id":"r","runs":[{RUN_VIEW}]}}"#
    );
    assert!(serde_json::from_str::<Response>(&listed).is_ok());
}

#[test]
fn get_run_requires_since_offset() {
    let ok = r#"{"type":"get_run","protocol_version":8,"request_id":"r","run_id":"00000000-0000-4000-8000-000000000001","since_offset":0}"#;
    assert!(serde_json::from_str::<Request>(ok).is_ok());
    assert!(serde_json::from_str::<Request>(&ok.replace(r#","since_offset":0"#, "")).is_err());
    for kind in ["cancel_run", "retry_save"] {
        let request = format!(
            r#"{{"type":"{kind}","protocol_version":8,"request_id":"r","run_id":"00000000-0000-4000-8000-000000000001"}}"#
        );
        assert!(serde_json::from_str::<Request>(&request).is_ok(), "{kind}");
    }
    let list = r#"{"type":"list_runs","protocol_version":8,"request_id":"r"}"#;
    assert!(serde_json::from_str::<Request>(list).is_ok());
}

#[test]
fn service_unavailable_error_code_round_trips() {
    let message = r#"{"type":"error","protocol_version":8,"request_id":"r","code":"service_unavailable","message":"The Brauser service is not installed."}"#;
    let parsed: Response = serde_json::from_str(message).unwrap();
    let Response::Error(ref error) = parsed else {
        panic!("not an error response");
    };
    assert_eq!(error.code, brauser_protocol::ErrorCode::ServiceUnavailable);
    assert_eq!(serde_json::to_string(&parsed).unwrap(), message);
}

#[test]
fn service_ready_requires_build_id() {
    let hello = r#"{"type":"service_hello","protocol_version":8,"request_id":"r"}"#;
    assert!(serde_json::from_str::<Request>(hello).is_ok());
    let ready = r#"{"type":"service_ready","protocol_version":8,"request_id":"r","build_id":"b1"}"#;
    assert!(serde_json::from_str::<Response>(ready).is_ok());
    assert!(serde_json::from_str::<Response>(&ready.replace(r#","build_id":"b1""#, "")).is_err());
}

#[test]
fn run_view_answer_round_trips_and_rejects_an_unknown_importance() {
    let answer = r#""answer":{"document_type":"report","category":"news","importance":"high","importance_reason":"New."}"#;
    let view = RUN_VIEW.replace(r#""answer":null"#, answer);
    let parsed: Response = serde_json::from_str(&run_fetched(&view)).unwrap();
    let encoded = serde_json::to_string(&parsed).unwrap();
    assert!(encoded.contains(answer), "{encoded}");
    for bad in [
        view.replace(r#""importance":"high""#, r#""importance":"critical""#),
        view.replace(r#""importance":"high""#, r#""importance":"High""#),
        view.replace(r#","importance_reason":"New.""#, ""),
        view.replace(
            r#""importance_reason":"New.""#,
            r#""importance_reason":"New.","x":1"#,
        ),
    ] {
        assert!(
            serde_json::from_str::<Response>(&run_fetched(&bad)).is_err(),
            "{bad}"
        );
    }
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

const SUMMARY_VIEW: &str = r#"{"summarized_at":"2026-10-26T14:05:00-07:00","document_type":"specification","category":"technical specification","importance":"high","summary_path":"summaries/a.summary.md","harness":"codex"}"#;

const LIST_ITEM: &str = r#"{"local_date":"2026-10-26","artifact_id":"web:a.example/x","artifact_type":"web.page","title":"T","url":"https://a.example/x","focused_seconds":0,"background_seconds":0,"opens":0,"why":null,"summary":null}"#;

fn listed(item: &str) -> String {
    format!(
        r#"{{"type":"artifact_list_result","protocol_version":8,"request_id":"r","items":[{item}],"artifact_types":["web.page"],"truncated":false}}"#
    )
}

#[test]
fn artifact_list_item_requires_a_nullable_summary() {
    let with_summary =
        LIST_ITEM.replace(r#""summary":null"#, &format!(r#""summary":{SUMMARY_VIEW}"#));
    for item in [LIST_ITEM.to_owned(), with_summary.clone()] {
        let response = listed(&item);
        let parsed: Response = serde_json::from_str(&response).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), response);
    }
    for bad in [
        LIST_ITEM.replace(r#","summary":null"#, ""),
        with_summary.replace(r#""importance":"high""#, r#""importance":"critical""#),
        with_summary.replace(r#""harness":"codex""#, r#""harness":"claude_code""#),
        with_summary.replace(r#","harness":"codex""#, ""),
        with_summary.replace(r#""harness":"codex""#, r#""harness":"codex","reason":"x""#),
    ] {
        assert!(
            serde_json::from_str::<Response>(&listed(&bad)).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn artifact_result_requires_a_nullable_summary() {
    let unrecognized = format!(
        r#"{{"type":"artifact_result","protocol_version":8,"request_id":"r","recognized":false,"artifact":null,"summary":{SUMMARY_VIEW}}}"#
    );
    let parsed: Response = serde_json::from_str(&unrecognized).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), unrecognized);
    let none = r#"{"type":"artifact_result","protocol_version":8,"request_id":"r","recognized":false,"artifact":null,"summary":null}"#;
    assert!(serde_json::from_str::<Response>(none).is_ok());
    assert!(serde_json::from_str::<Response>(&none.replace(r#","summary":null"#, "")).is_err());
}

#[test]
fn open_summary_requires_url_and_a_nullable_summary_path() {
    let request = r#"{"type":"open_summary","protocol_version":8,"request_id":"r","url":"https://example.com/a","summary_path":null}"#;
    let parsed: Request = serde_json::from_str(request).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), request);
    let by_path = request.replace("null", r#""summaries/a.summary.md""#);
    let parsed: Request = serde_json::from_str(&by_path).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), by_path);
    for bad in [
        request.replace(r#","url":"https://example.com/a""#, ""),
        request.replace(r#""url":"https://example.com/a""#, r#""url":null"#),
        request.replace(r#","summary_path":null"#, ""),
        request.replace('}', r#","path":"summaries/a.summary.md"}"#),
    ] {
        assert!(serde_json::from_str::<Request>(&bad).is_err(), "{bad}");
    }
    let opened = r#"{"type":"summary_opened","protocol_version":8,"request_id":"r","relative_path":"summaries/a.summary.md"}"#;
    let parsed: Response = serde_json::from_str(opened).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), opened);
    assert!(
        serde_json::from_str::<Response>(
            &opened.replace(r#","relative_path":"summaries/a.summary.md""#, "")
        )
        .is_err()
    );
}

#[test]
fn adopted_state_and_its_forget_request_round_trip() {
    let request = r#"{"type":"forget_adopted_contracts","protocol_version":8,"request_id":"r","expected_revision":"sha256:abc"}"#;
    let parsed: Request = serde_json::from_str(request).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), request);
    // The revision is required, as for update_config.
    assert!(
        serde_json::from_str::<Request>(
            r#"{"type":"forget_adopted_contracts","protocol_version":8,"request_id":"r"}"#
        )
        .is_err()
    );

    let state = r#"{"issue":null,"revision":"sha256:abc","warnings":["full"],"full":true}"#;
    let parsed: AdoptedState = serde_json::from_str(state).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), state);
    // full is required, as are issue (even when null) and the rest.
    assert!(
        serde_json::from_str::<AdoptedState>(
            r#"{"issue":null,"revision":"missing","warnings":[]}"#
        )
        .is_err()
    );
    // issue is required even when null, and no other key is allowed.
    assert!(
        serde_json::from_str::<AdoptedState>(
            r#"{"revision":"missing","warnings":[],"full":false}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<AdoptedState>(
            r#"{"issue":null,"revision":"missing","warnings":[],"full":false,"extra":1}"#
        )
        .is_err()
    );
}

fn shapes(prefix: &str, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|n| json!({ "shape": format!("/{prefix}{n}/{{num}}"), "count": 1 }))
            .collect(),
    )
}

fn draft_site(visited: usize, links: usize) -> Value {
    json!({
        "type": "draft_site", "protocol_version": 8, "request_id": "r1",
        "origin": "https://work.example",
        "visited": shapes("v", visited), "links": shapes("l", links),
    })
}

/// A request that serde reads and the schema accepts, written back unchanged.
fn assert_request_round_trips(definition: &str, value: &Value) -> Request {
    assert!(
        schema_subset::valid(definition, value),
        "schema refused {value}"
    );
    let parsed: Request = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(&serde_json::to_value(&parsed).unwrap(), value);
    parsed
}

fn assert_response_round_trips(definition: &str, value: &Value) -> Response {
    assert!(
        schema_subset::valid(definition, value),
        "schema refused {value}"
    );
    let parsed: Response = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(&serde_json::to_value(&parsed).unwrap(), value);
    parsed
}

fn preview_row(source: &str) -> Value {
    json!({
        "source": source, "shape": "/issue/{num}", "url": "https://work.example/issue/1",
        "adapter": 0, "rule": 0, "artifact_id": "work.issue:work.example/1",
    })
}

fn site_drafted() -> Value {
    json!({
        "type": "site_drafted", "protocol_version": 8, "request_id": "r1",
        "revision": "sha256:00", "drafter": "structural",
        "adapters": [{
            "adapter": {
                "type": "work.issue", "origin": "https://work.example",
                "match": [{ "path": "/issue/{num:int}" }], "id": "work.issue:{host}/{num}",
            },
            "scope": "pending",
            "tenant": { "segment": 0, "capture": "org" },
            "rules": [{
                "rule": 0, "checked": false, "unchecked_reason": "link_only",
                "visited": 0, "links": 3, "example": "https://work.example/issue/1",
            }],
        }],
        "dropped": [
            { "shape": "/search", "adapter": null, "rule": null, "reason": "probe" },
            { "shape": null, "adapter": 0, "rule": 1, "reason": "draft_overlap" },
        ],
        "preview": [preview_row("visited"), preview_row("link"), {
            "source": "probe", "shape": "/", "url": "https://work.example/",
            "adapter": null, "rule": null, "artifact_id": null,
        }],
        "taken_types": ["jira.issue"],
        "remapped": 0,
    })
}

fn check_site_draft(scope: Value) -> Value {
    json!({
        "type": "check_site_draft", "protocol_version": 8, "request_id": "r2",
        "origin": "https://work.example", "expected_revision": "sha256:00",
        "adapters": [
            { "type": "work.issue", "origin": "https://work.example",
              "match": [{ "path": "/issue/{num:int}" }], "id": "work.issue:{host}/{num}" },
            { "type": "work.page", "origin": "https://work.example",
              "match": [{ "path": "/page/{num:int}" }], "id": "work.page:{host}/{num}" },
            { "type": "work.doc", "origin": "https://work.example",
              "match": [{ "path": "/doc/{num:int}" }], "id": "work.doc:{host}/{num}" },
        ],
        "provenance": [["drafted"], ["user_written"], ["drafted"]],
        "scope": scope,
        "visited": shapes("v", 2), "links": shapes("l", 1),
    })
}

#[test]
fn draft_site_round_trips() {
    let request = draft_site(21, 50);
    match assert_request_round_trips("DraftSiteRequest", &request) {
        Request::DraftSite(value) => {
            assert_eq!(value.origin, "https://work.example");
            assert_eq!((value.visited.len(), value.links.len()), (21, 50));
            assert_eq!(value.links[0].count, 1);
        }
        other => panic!("not draft_site: {other:?}"),
    }
    // Only the origin and shapes cross: a URL or title key is unknown.
    for (key, extra) in [
        ("url", json!("https://work.example/issue/1")),
        ("title", json!("T")),
    ] {
        let mut with_extra = request.clone();
        with_extra[key] = extra;
        assert!(
            serde_json::from_value::<Request>(with_extra).is_err(),
            "{key}"
        );
    }
    let mut no_links = request.clone();
    no_links.as_object_mut().unwrap().remove("links");
    assert!(serde_json::from_value::<Request>(no_links).is_err());
}

#[test]
fn draft_site_rejects_more_than_50_link_shapes() {
    assert!(schema_subset::valid(
        "DraftSiteRequest",
        &draft_site(21, 50)
    ));
    assert!(!schema_subset::valid(
        "DraftSiteRequest",
        &draft_site(21, 51)
    ));
    assert!(!schema_subset::valid(
        "DraftSiteRequest",
        &draft_site(22, 0)
    ));
    // Each shape and count is bounded too.
    let mut long = draft_site(1, 0);
    long["visited"][0]["shape"] = json!("/".repeat(2049));
    assert!(!schema_subset::valid("DraftSiteRequest", &long));
    for count in [0, 1_000_001] {
        let mut bad = draft_site(1, 0);
        bad["visited"][0]["count"] = json!(count);
        assert!(!schema_subset::valid("DraftSiteRequest", &bad), "{count}");
    }
}

#[test]
fn site_drafted_rejects_unknown_fields() {
    let drafted = site_drafted();
    match assert_response_round_trips("SiteDrafted", &drafted) {
        Response::SiteDrafted(value) => {
            assert_eq!(value.drafter, SiteDrafter::Structural);
            assert_eq!(value.adapters[0].tenant.as_ref().unwrap().capture, "org");
            assert_eq!(value.adapters[0].rules[0].links, 3);
        }
        other => panic!("not site_drafted: {other:?}"),
    }
    for pointer in [
        "",
        "/adapters/0",
        "/adapters/0/tenant",
        "/adapters/0/rules/0",
        "/dropped/0",
        "/preview/0",
    ] {
        let mut extra = drafted.clone();
        extra.pointer_mut(pointer).unwrap()["extra"] = json!(1);
        assert!(
            serde_json::from_value::<Response>(extra).is_err(),
            "extra key accepted at {pointer:?}"
        );
    }
    let mut missing = drafted.clone();
    missing.as_object_mut().unwrap().remove("remapped");
    assert!(serde_json::from_value::<Response>(missing).is_err());
    let mut reason = drafted.clone();
    reason["dropped"][0]["reason"] = json!("because");
    assert!(serde_json::from_value::<Response>(reason).is_err());
    let mut many = drafted.clone();
    many["adapters"] = Value::Array(vec![drafted["adapters"][0].clone(); 9]);
    assert!(!schema_subset::valid("SiteDrafted", &many));
}

#[test]
fn check_site_draft_round_trips() {
    let request = check_site_draft(json!([null, null, null]));
    match assert_request_round_trips("CheckSiteDraftRequest", &request) {
        Request::CheckSiteDraft(value) => {
            assert_eq!(value.adapters.len(), 3);
            assert_eq!(value.provenance.len(), 3);
            assert_eq!(value.expected_revision, "sha256:00");
        }
        other => panic!("not check_site_draft: {other:?}"),
    }
    // The title entry never crosses, and there is no merged-scope field.
    for key in ["title", "unscoped"] {
        let mut extra = request.clone();
        extra[key] = json!(null);
        assert!(serde_json::from_value::<Request>(extra).is_err(), "{key}");
    }
    let mut provenance = request.clone();
    provenance["provenance"][0][0] = json!("agent");
    assert!(serde_json::from_value::<Request>(provenance).is_err());
    let mut rules = request.clone();
    rules["provenance"][0] = Value::Array(vec![json!("drafted"); 17]);
    assert!(!schema_subset::valid("CheckSiteDraftRequest", &rules));

    let checked = json!({
        "type": "site_draft_checked", "protocol_version": 8, "request_id": "r2",
        "revision": "sha256:00",
        "rules": [
            [{ "verdict": "kept", "reason": null }],
            [{ "verdict": "warned", "reason": "draft_overlap" }],
            [{ "verdict": "dropped", "reason": "draft_matches_probe" }],
        ],
        "scope": [
            { "state": "not_shared", "resolved": "not_shared", "reason": null },
            { "state": "pending", "resolved": null, "reason": "draft_scope_pending" },
            { "state": "unscoped_only", "resolved": "unscoped", "reason": null },
        ],
        "preview": [preview_row("visited")],
    });
    assert_response_round_trips("SiteDraftChecked", &checked);
    let mut verdict = checked.clone();
    verdict["rules"][0][0]["verdict"] = json!("maybe");
    assert!(serde_json::from_value::<Response>(verdict).is_err());
    let mut extra = checked.clone();
    extra["scope"][0]["values"] = json!([]);
    assert!(serde_json::from_value::<Response>(extra).is_err());
}

#[test]
fn check_site_draft_scope_choice_round_trips_and_rejects_an_empty_values_list() {
    let request = check_site_draft(json!([{ "values": ["acme", "beta"] }, "unscoped", null]));
    match assert_request_round_trips("CheckSiteDraftRequest", &request) {
        Request::CheckSiteDraft(value) => assert_eq!(
            value.scope,
            vec![
                Some(ScopeChoiceView::ScopeValues(ScopeValues {
                    values: vec!["acme".to_owned(), "beta".to_owned()],
                })),
                Some(ScopeChoiceView::UnscopedChoice(UnscopedChoice::Unscoped)),
                None,
            ]
        ),
        other => panic!("not check_site_draft: {other:?}"),
    }
    let empty = check_site_draft(json!([{ "values": [] }, null, null]));
    assert!(!schema_subset::valid("CheckSiteDraftRequest", &empty));
    let long = check_site_draft(json!([{ "values": ["a".repeat(129)] }, null, null]));
    assert!(!schema_subset::valid("CheckSiteDraftRequest", &long));
    let many = check_site_draft(json!([{ "values": vec!["a"; 65] }, null, null]));
    assert!(!schema_subset::valid("CheckSiteDraftRequest", &many));
    for bad in [
        json!(["scoped", null, null]),
        json!([{ "values": ["acme"], "unscoped": true }, null, null]),
        json!([{}, null, null]),
        json!([true, null, null]),
    ] {
        let request = check_site_draft(bad.clone());
        assert!(
            serde_json::from_value::<Request>(request).is_err(),
            "accepted scope {bad}"
        );
    }
}

#[test]
fn unknown_drafter_value_is_rejected() {
    let mut unknown = site_drafted();
    unknown["drafter"] = json!("llm");
    assert!(serde_json::from_value::<Response>(unknown.clone()).is_err());
    assert!(!schema_subset::valid("SiteDrafted", &unknown));
}

// --- Capture Site agent drafting (additive v7) -----------------------------

fn start_site_draft(visited: usize, links: usize) -> Value {
    json!({
        "type": "start_site_draft", "protocol_version": 7, "request_id": "r1",
        "origin": "https://work.example",
        "visited": shapes("v", visited), "links": shapes("l", links),
        "harness": null, "requested_at": "2026-10-28T09:00:00-07:00",
    })
}

fn agent_table(path: &str) -> Value {
    json!({
        "type": "work.doc", "origin": "https://work.example",
        "match": [{ "path": path }], "id": "work.doc:{host}/{num}",
    })
}

fn run_view() -> Value {
    serde_json::from_str(RUN_VIEW).unwrap()
}

fn run_fetched_value(view: Value) -> Value {
    json!({
        "type": "run_fetched", "protocol_version": 7, "request_id": "r",
        "run": view, "stream_text": "x", "next_offset": 1, "more": false,
    })
}

#[test]
fn start_site_draft_round_trips() {
    let request = start_site_draft(21, 50);
    match assert_request_round_trips("StartSiteDraftRequest", &request) {
        Request::StartSiteDraft(value) => {
            assert_eq!(value.origin, "https://work.example");
            assert_eq!((value.visited.len(), value.links.len()), (21, 50));
            assert_eq!(value.harness, None);
            assert_eq!(value.requested_at, "2026-10-28T09:00:00-07:00");
        }
        other => panic!("not start_site_draft: {other:?}"),
    }
    let mut chosen = request.clone();
    chosen["harness"] = json!("claude");
    match assert_request_round_trips("StartSiteDraftRequest", &chosen) {
        Request::StartSiteDraft(value) => assert_eq!(value.harness, Some(HarnessId::Claude)),
        other => panic!("not start_site_draft: {other:?}"),
    }
    // Every key is required; harness is nullable, never omitted.
    for key in ["origin", "visited", "links", "harness", "requested_at"] {
        let mut missing = request.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<Request>(missing).is_err(), "{key}");
    }
}

#[test]
fn start_site_draft_rejects_more_than_50_link_shapes_and_unknown_fields() {
    assert!(!schema_subset::valid(
        "StartSiteDraftRequest",
        &start_site_draft(21, 51)
    ));
    assert!(!schema_subset::valid(
        "StartSiteDraftRequest",
        &start_site_draft(22, 0)
    ));
    let mut long = start_site_draft(0, 0);
    long["origin"] = json!(format!("https://{}", "a".repeat(2041)));
    assert!(!schema_subset::valid("StartSiteDraftRequest", &long));
    // Only the origin and shapes cross: no URL, title, page text, structural
    // draft, agent input, run kind, or record key.
    for (key, extra) in [
        ("url", json!("https://work.example/issue/1")),
        ("title", json!("T")),
        ("content", json!("body")),
        ("draft", json!({ "adapters": [] })),
        ("input", json!("planted")),
        ("kind", json!("site_draft")),
        ("record_key", json!(null)),
    ] {
        let mut with_extra = start_site_draft(1, 1);
        with_extra[key] = extra;
        assert!(
            serde_json::from_value::<Request>(with_extra.clone()).is_err(),
            "{key}"
        );
        assert!(
            !schema_subset::valid("StartSiteDraftRequest", &with_extra),
            "{key}"
        );
    }
    let mut harness = start_site_draft(1, 1);
    harness["harness"] = json!("claude_code");
    assert!(serde_json::from_value::<Request>(harness).is_err());
}

#[test]
fn start_run_kind_is_optional_and_bounded_to_the_run_kinds() {
    let ok: Value = serde_json::from_str(r#"{"type":"start_run","protocol_version":8,"request_id":"r","url":"https://a.example/x","title":"T","content":"body","selection":null,"harness":null,"record_key":null,"requested_at":"2026-10-26T14:00:00-07:00"}"#).unwrap();
    match serde_json::from_value::<Request>(ok.clone()).unwrap() {
        Request::StartRun(value) => assert_eq!(value.kind, None),
        other => panic!("not start_run: {other:?}"),
    }
    for (kind, expected) in [
        ("summary", RunKind::Summary),
        ("site_draft", RunKind::SiteDraft),
    ] {
        let mut with_kind = ok.clone();
        with_kind["kind"] = json!(kind);
        let parsed: Request = serde_json::from_value(with_kind.clone()).unwrap();
        match &parsed {
            Request::StartRun(value) => assert_eq!(value.kind, Some(expected)),
            other => panic!("not start_run: {other:?}"),
        }
        assert_eq!(serde_json::to_value(&parsed).unwrap(), with_kind);
    }
    let mut unknown = ok.clone();
    unknown["kind"] = json!("chat");
    assert!(serde_json::from_value::<Request>(unknown).is_err());
}

#[test]
fn run_view_without_kind_or_site_draft_still_parses() {
    // An older service's record has neither key, and a new view without them
    // writes neither back.
    let message = run_fetched_value(run_view());
    let parsed = assert_response_round_trips("RunFetched", &message);
    let Response::RunFetched(fetched) = parsed else {
        panic!("not run_fetched");
    };
    assert_eq!(fetched.run.kind, None);
    assert_eq!(fetched.run.site_draft, None);

    let mut view = run_view();
    view["kind"] = json!("site_draft");
    view["site_draft"] = json!(null);
    assert!(schema_subset::valid("RunView", &view));
    let parsed: Response = serde_json::from_value(run_fetched_value(view.clone())).unwrap();
    let Response::RunFetched(fetched) = parsed else {
        panic!("not run_fetched");
    };
    assert_eq!(fetched.run.kind, Some(RunKind::SiteDraft));
    assert_eq!(fetched.run.site_draft, None);

    view["kind"] = json!("summary");
    view["site_draft"] = json!({ "adapters": [agent_table("/docs/{num:int}")] });
    let parsed = assert_response_round_trips("RunFetched", &run_fetched_value(view.clone()));
    let Response::RunFetched(fetched) = parsed else {
        panic!("not run_fetched");
    };
    assert_eq!(fetched.run.site_draft.unwrap().adapters.len(), 1);

    let mut unknown = view.clone();
    unknown["kind"] = json!("chat");
    assert!(serde_json::from_value::<Response>(run_fetched_value(unknown.clone())).is_err());
    assert!(!schema_subset::valid("RunView", &unknown));
    let mut extra = view.clone();
    extra["site_draft"]["answer"] = json!("raw");
    assert!(serde_json::from_value::<Response>(run_fetched_value(extra)).is_err());
}

#[test]
fn run_view_site_draft_rejects_more_than_8_adapters() {
    let mut view = run_view();
    let tables = |count: usize| Value::Array(vec![agent_table("/docs/{num:int}"); count]);
    for count in [1, 8] {
        view["site_draft"] = json!({ "adapters": tables(count) });
        assert!(schema_subset::valid("RunView", &view), "{count}");
    }
    for count in [0, 9] {
        view["site_draft"] = json!({ "adapters": tables(count) });
        assert!(!schema_subset::valid("RunView", &view), "{count}");
    }
    // Each entry is a table; serde reads any JSON value there, so the
    // service's validator and the host's check enforce it at runtime.
    view["site_draft"] = json!({ "adapters": ["/docs/{num:int}"] });
    assert!(!schema_subset::valid("RunView", &view));
}

#[test]
fn draft_site_with_and_without_run_id_round_trips() {
    let without = draft_site(1, 1);
    match assert_request_round_trips("DraftSiteRequest", &without) {
        Request::DraftSite(value) => assert_eq!(value.run_id, None),
        other => panic!("not draft_site: {other:?}"),
    }
    let mut with = draft_site(1, 1);
    with["run_id"] = json!("00000000-0000-4000-8000-000000000001");
    match assert_request_round_trips("DraftSiteRequest", &with) {
        Request::DraftSite(value) => assert_eq!(
            value.run_id.as_deref(),
            Some("00000000-0000-4000-8000-000000000001")
        ),
        other => panic!("not draft_site: {other:?}"),
    }
    // An explicit null means no run, as a missing key does.
    let mut null = draft_site(1, 1);
    null["run_id"] = json!(null);
    assert!(schema_subset::valid("DraftSiteRequest", &null));
    match serde_json::from_value::<Request>(null).unwrap() {
        Request::DraftSite(value) => assert_eq!(value.run_id, None),
        other => panic!("not draft_site: {other:?}"),
    }
    for bad in [json!("short"), json!("0".repeat(37)), json!(7)] {
        let mut request = draft_site(1, 1);
        request["run_id"] = bad.clone();
        assert!(!schema_subset::valid("DraftSiteRequest", &request), "{bad}");
    }
    let mut number = draft_site(1, 1);
    number["run_id"] = json!(7);
    assert!(serde_json::from_value::<Request>(number).is_err());
}

#[test]
fn site_drafted_accepts_agent_drafter_and_unseen_reason() {
    let mut drafted = site_drafted();
    drafted["drafter"] = json!("agent");
    drafted["adapters"][0]["rules"][0]["unchecked_reason"] = json!("unseen");
    match assert_response_round_trips("SiteDrafted", &drafted) {
        Response::SiteDrafted(value) => {
            assert_eq!(value.drafter, SiteDrafter::Agent);
            assert_eq!(
                value.adapters[0].rules[0].unchecked_reason,
                Some(UncheckedReason::Unseen)
            );
        }
        other => panic!("not site_drafted: {other:?}"),
    }
}

mod schema_subset {
    use serde_json::Value;

    const KEYWORDS: &[&str] = &[
        "$ref",
        "oneOf",
        "const",
        "enum",
        "type",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "description",
        // Not checked here (no regex in this subset); the host checks a
        // requested_at's form and window at runtime.
        "pattern",
    ];

    pub fn schema() -> Value {
        serde_json::from_str(include_str!("../schema.json")).unwrap()
    }

    /// Whether `value` satisfies `#/$defs/<definition>`.
    pub fn valid(definition: &str, value: &Value) -> bool {
        let root = schema();
        let shape = &root["$defs"][definition];
        assert!(shape.is_object(), "no definition {definition}");
        matches(&root, shape, value)
    }

    fn number(shape: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
        shape.get(key).map(|bound| bound.as_u64().unwrap())
    }

    fn matches(root: &Value, shape: &Value, value: &Value) -> bool {
        let shape = shape.as_object().unwrap();
        for key in shape.keys() {
            assert!(
                KEYWORDS.contains(&key.as_str()),
                "unsupported keyword {key}"
            );
        }
        if let Some(reference) = shape.get("$ref") {
            let name = reference
                .as_str()
                .unwrap()
                .strip_prefix("#/$defs/")
                .unwrap();
            return matches(root, &root["$defs"][name], value);
        }
        if let Some(alternatives) = shape.get("oneOf") {
            let passing = alternatives
                .as_array()
                .unwrap()
                .iter()
                .filter(|alternative| matches(root, alternative, value))
                .count();
            return passing == 1;
        }
        if shape.get("const").is_some_and(|constant| value != constant) {
            return false;
        }
        if shape
            .get("enum")
            .is_some_and(|values| !values.as_array().unwrap().contains(value))
        {
            return false;
        }
        match shape.get("type").and_then(Value::as_str) {
            None => true,
            Some("null") => value.is_null(),
            Some("boolean") => value.is_boolean(),
            Some("integer") => value.as_u64().is_some_and(|n| {
                number(shape, "minimum").is_none_or(|min| n >= min)
                    && number(shape, "maximum").is_none_or(|max| n <= max)
            }),
            Some("string") => value.as_str().is_some_and(|text| {
                let length = text.chars().count() as u64;
                number(shape, "minLength").is_none_or(|min| length >= min)
                    && number(shape, "maxLength").is_none_or(|max| length <= max)
            }),
            Some("array") => value.as_array().is_some_and(|items| {
                let length = items.len() as u64;
                number(shape, "minItems").is_none_or(|min| length >= min)
                    && number(shape, "maxItems").is_none_or(|max| length <= max)
                    && shape
                        .get("items")
                        .is_none_or(|item| items.iter().all(|entry| matches(root, item, entry)))
            }),
            Some("object") => value.as_object().is_some_and(|object| {
                let properties = shape.get("properties").and_then(Value::as_object);
                let required = shape
                    .get("required")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                let closed = shape.get("additionalProperties") == Some(&Value::Bool(false));
                required
                    .iter()
                    .all(|key| object.contains_key(key.as_str().unwrap()))
                    && object.iter().all(|(key, entry)| {
                        match properties.and_then(|known| known.get(key)) {
                            Some(property) => matches(root, property, entry),
                            None => !closed,
                        }
                    })
            }),
            Some(other) => panic!("unsupported type {other}"),
        }
    }
}

#[test]
fn capture_site_preflight_requires_revision_and_document_and_round_trips() {
    let request = json!({
        "type": "check_capture_site", "protocol_version": 8, "request_id": "preflight",
        "expected_revision": "revision", "adapters": {"format": 1, "artifacts": []}
    });
    let parsed: Request = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), request);
    for field in ["expected_revision", "adapters"] {
        let mut invalid = request.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Request>(invalid).is_err());
    }
    let mut with_url = request;
    with_url["url"] = "https://example.com/private".into();
    assert!(serde_json::from_value::<Request>(with_url).is_err());
    let reply = json!({
        "type": "capture_site_checked", "protocol_version": 8, "request_id": "preflight",
        "revision": "revision"
    });
    let parsed: Response = serde_json::from_value(reply.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), reply);
    let mut missing = reply;
    missing.as_object_mut().unwrap().remove("revision");
    assert!(serde_json::from_value::<Response>(missing).is_err());
}

#[test]
fn preserved_note_requests_and_bounded_metadata_round_trip() {
    for value in [
        json!({"type":"list_preserved_notes","protocol_version":8,"request_id":"r","url":"https://example.com/page"}),
        json!({"type":"view_preserved_note","protocol_version":8,"request_id":"r","url":"https://example.com/page","key":{"kind":"artifact","value":"item:7"}}),
    ] {
        let request: Request = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), value);
        let mut unknown = value;
        unknown["path"] = json!("/arbitrary/file");
        assert!(serde_json::from_value::<Request>(unknown).is_err());
    }
    let note = json!({"key":{"kind":"artifact","value":"item:7"},"canonical_url":"https://example.com/page","title":"Seven","revision":"v"});
    let mut value = json!({"type":"preserved_notes_listed","protocol_version":8,"request_id":"r","notes":[note.clone()]});
    let response: Response = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), value);
    assert!(schema_subset::valid("PreservedNotesListed", &value));
    value["notes"] = json!(vec![note; 25]);
    assert!(!schema_subset::valid("PreservedNotesListed", &value));
}

#[test]
fn site_restore_requires_origin_and_revision_without_page_content() {
    let request = json!({"type":"get_site_restore", "protocol_version":8, "request_id":"r", "origin":"https://work.example", "expected_revision":"v"});
    assert_request_round_trips("GetSiteRestoreRequest", &request);
    for field in ["origin", "expected_revision"] {
        let mut invalid = request.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Request>(invalid).is_err());
    }
    for field in ["url", "title", "body", "adapters"] {
        let mut invalid = request.clone();
        invalid[field] = json!("private");
        assert!(serde_json::from_value::<Request>(invalid).is_err());
    }
    let reply = json!({"type":"site_restore_loaded", "protocol_version":8, "request_id":"r", "origin":"https://work.example", "revision":"v", "adapters":{"format":1,"artifacts":[]}});
    assert!(schema_subset::valid("SiteRestoreLoaded", &reply));
    let parsed: Response = serde_json::from_value(reply.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), reply);
}

use brauser_protocol::Request;
use serde_json::json;

mod common;
use common::schema_subset;

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

#[test]
fn recent_reading_filter_is_optional_known_and_never_serializes_for_all_activity() {
    let request = json!({"type":"list_artifacts", "protocol_version":8, "request_id":"reading",
        "artifact_type":null, "title_query":null, "limit":32});
    common::assert_request_round_trips("ListArtifactsRequest", &request);
    for reading in ["limited", "whole", "unknown"] {
        let mut filtered = request.clone();
        filtered["summary_reading"] = json!(reading);
        common::assert_request_round_trips("ListArtifactsRequest", &filtered);
    }
    for invalid in [
        json!(null),
        json!("partial"),
        json!("page_only"),
        json!("all"),
        json!(true),
    ] {
        let mut filtered = request.clone();
        filtered["summary_reading"] = invalid;
        assert!(!schema_subset::valid("ListArtifactsRequest", &filtered));
        // Serde treats optional null as absent; schema validation still refuses
        // null on the wire, like the protocol's other additive enum fields.
        if !filtered["summary_reading"].is_null() {
            assert!(serde_json::from_value::<Request>(filtered).is_err());
        }
    }
}

#[test]
fn worklog_choices_are_a_bounded_independent_read_only_query() {
    let request = json!({"type":"worklog_choices", "protocol_version":8, "request_id":"choose", "expected_revision":"sha256:config",
        "start_date":"2026-11-01", "end_date":"2026-11-02"});
    common::assert_request_round_trips("WorklogChoicesRequest", &request);
    assert!(schema_subset::valid("WorklogChoicesRequest", &request));
    let row = json!({"artifact_id":"doc:example/1", "local_date":"2026-11-01",
        "text":"Release plan — focused 2 min", "url":"https://example.com/1"});
    let mut response = json!({"type":"worklog_choices_result", "protocol_version":8, "request_id":"choose",
        "start_date":"2026-11-01", "end_date":"2026-11-02", "choices":[row.clone()], "truncated":false,
        "gaps":[], "gaps_truncated":false});
    common::assert_response_round_trips("WorklogChoicesResult", &response);
    assert!(schema_subset::valid("WorklogChoicesResult", &response));
    response["choices"] = json!(vec![row.clone(); 200]);
    assert!(schema_subset::valid("WorklogChoicesResult", &response));
    response["choices"] = json!(vec![row; 201]);
    assert!(!schema_subset::valid("WorklogChoicesResult", &response));
    let mut invalid = request;
    invalid["selected"] = json!(["doc:example/1"]);
    assert!(serde_json::from_value::<Request>(invalid).is_err());
}

#[test]
fn worklog_choices_paging_is_opt_in_and_has_bounded_snapshot_metadata() {
    let request = json!({"type":"worklog_choices", "protocol_version":8, "request_id":"page", "expected_revision":"revision",
        "start_date":"2026-11-01", "end_date":"2026-11-01", "page":{"offset":0}});
    common::assert_request_round_trips("WorklogChoicesRequest", &request);
    let mut next = request.clone();
    next["page"] = json!({"offset":200,"snapshot":"a".repeat(64)});
    common::assert_request_round_trips("WorklogChoicesRequest", &next);
    for invalid in [
        json!({"offset":-1}),
        json!({"offset":0,"snapshot":"short"}),
        json!({"offset":0,"snapshot":"a".repeat(65)}),
        json!({"offset":0,"path":"/home"}),
    ] {
        next["page"] = invalid;
        assert!(!schema_subset::valid("WorklogChoicesRequest", &next));
    }
    let mut response = json!({"type":"worklog_choices_result", "protocol_version":8, "request_id":"page",
        "start_date":"2026-11-01", "end_date":"2026-11-01", "choices":[], "truncated":true,
        "gaps":[], "gaps_truncated":false, "page":{"snapshot":"a".repeat(64),"offset":0,"next_offset":200,"omitted":false}});
    common::assert_response_round_trips("WorklogChoicesResult", &response);
    response["page"]["next_offset"] = json!(null);
    common::assert_response_round_trips("WorklogChoicesResult", &response);
    response["page"]["next_offset"] = json!(0);
    assert!(!schema_subset::valid("WorklogChoicesResult", &response));
}

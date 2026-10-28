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

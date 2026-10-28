use brauser_protocol::{Request, Response};
use serde_json::json;

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

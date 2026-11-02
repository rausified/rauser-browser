//! Auto-summary's wire additions: `start_auto_summary`, its
//! `auto_summary_skipped` answer, and the config's `auto_summary` settings
//! (additive within protocol version 8; host and extension deploy together).

use brauser_protocol::{AutoSummarySkipReason, ConfigSnapshot, Request, Response};
use serde_json::{Value, json};

mod common;
use common::{assert_request_round_trips, assert_response_round_trips, config, schema_subset};

fn start_auto_summary() -> Value {
    json!({
        "type": "start_auto_summary", "protocol_version": 8, "request_id": "r",
        "url": "https://a.example/x", "title": "T", "content": "body",
        "requested_at": "2026-11-02T14:00:00-07:00",
    })
}

#[test]
fn start_auto_summary_round_trips() {
    let parsed = assert_request_round_trips("StartAutoSummaryRequest", &start_auto_summary());
    let Request::StartAutoSummary(value) = &parsed else {
        panic!("{parsed:?}");
    };
    assert_eq!(value.url, "https://a.example/x");
    assert_eq!(value.capture_truncated, None);
}

#[test]
fn automatic_capture_truncation_is_an_optional_boolean() {
    let mut value = start_auto_summary();
    for flag in [true, false] {
        value["capture_truncated"] = json!(flag);
        let Request::StartAutoSummary(request) =
            assert_request_round_trips("StartAutoSummaryRequest", &value)
        else {
            panic!("expected start_auto_summary");
        };
        assert_eq!(request.capture_truncated, Some(flag));
    }
    for invalid in [json!("true"), json!(1), json!({})] {
        value["capture_truncated"] = invalid;
        assert!(!schema_subset::valid("StartAutoSummaryRequest", &value));
        assert!(serde_json::from_value::<Request>(value.clone()).is_err());
    }
}

#[test]
fn start_auto_summary_rejects_host_only_fields() {
    for (key, extra) in [
        ("record_key", json!(null)),
        ("kind", json!("summary")),
        ("harness", json!(null)),
        ("selection", json!(null)),
        ("instruction", json!("x")),
    ] {
        let mut value = start_auto_summary();
        value[key] = extra;
        assert!(
            serde_json::from_value::<Request>(value.clone()).is_err(),
            "{key} accepted"
        );
        assert!(
            !schema_subset::valid("StartAutoSummaryRequest", &value),
            "{key}"
        );
    }
}

#[test]
fn auto_summary_skipped_round_trips_every_reason() {
    for (wire, reason) in [
        ("disabled", AutoSummarySkipReason::Disabled),
        ("not_set_up", AutoSummarySkipReason::NotSetUp),
        ("not_an_artifact", AutoSummarySkipReason::NotAnArtifact),
        (
            "already_summarized",
            AutoSummarySkipReason::AlreadySummarized,
        ),
    ] {
        let value = json!({
            "type": "auto_summary_skipped", "protocol_version": 8,
            "request_id": "r", "reason": wire,
        });
        let parsed = assert_response_round_trips("AutoSummarySkippedResponse", &value);
        let Response::AutoSummarySkipped(skipped) = parsed else {
            panic!("{parsed:?}");
        };
        assert_eq!(skipped.reason, reason);
    }
    let later = json!({
        "type": "auto_summary_skipped", "protocol_version": 8,
        "request_id": "r", "reason": "later",
    });
    assert!(serde_json::from_value::<Response>(later.clone()).is_err());
    assert!(!schema_subset::valid("AutoSummarySkippedResponse", &later));
}

#[test]
fn config_snapshot_requires_auto_summary() {
    let mut value: Value = serde_json::from_str(&config(r#""storage":null,"#)).unwrap();
    let parsed: ConfigSnapshot = serde_json::from_value(value.clone()).unwrap();
    assert!(parsed.auto_summary.enabled);
    assert_eq!(parsed.auto_summary.dwell_secs, 0);
    assert!(schema_subset::valid("ConfigSnapshot", &value));

    value["auto_summary"]["dwell_secs"] = json!(3601);
    assert!(!schema_subset::valid("ConfigSnapshot", &value));
    value["auto_summary"]["dwell_secs"] = json!(3600);
    assert!(schema_subset::valid("ConfigSnapshot", &value));

    value.as_object_mut().unwrap().remove("auto_summary");
    assert!(serde_json::from_value::<ConfigSnapshot>(value.clone()).is_err());
    assert!(!schema_subset::valid("ConfigSnapshot", &value));
}

//! Session-scoped worklog sharing is additive and never changes old queries.
mod common;
use brauser_protocol::Request;
use common::schema_subset;
use serde_json::json;

#[test]
fn focus_worklog_catalog_and_selection_round_trip_with_explicit_bounds() {
    let id = "10000000-0000-4000-8000-000000000001";
    let mut request = json!({"type":"worklog_focus_choices", "protocol_version":8,
        "request_id":"focus", "expected_revision":"sha256:config", "start_date":"2026-11-03",
        "end_date":"2026-11-03", "focus_session_id":null});
    common::assert_request_round_trips("WorklogFocusChoicesRequest", &request);
    request["focus_session_id"] = json!(id);
    common::assert_request_round_trips("WorklogFocusChoicesRequest", &request);
    assert!(schema_subset::valid("WorklogFocusChoicesRequest", &request));
    request["path"] = json!("/arbitrary/path");
    assert!(serde_json::from_value::<Request>(request).is_err());
    let session = json!({"id":id,"name":"Release","revision":2});
    let mut response = json!({"type":"worklog_focus_choices_result", "protocol_version":8,
        "request_id":"focus", "start_date":"2026-11-03", "end_date":"2026-11-03",
        "focus_session_id":id, "sessions":[session], "sessions_truncated":false,
        "selected_session":session, "name_changed":true,
        "choices":[{"artifact_id":"work:example/A","local_date":"2026-11-03",
            "text":"Release — focused 2 min","url":"https://work.example/A"}],
        "truncated":false,"gaps":[],"gaps_truncated":false});
    common::assert_response_round_trips("WorklogFocusChoicesResult", &response);
    assert!(schema_subset::valid("WorklogFocusChoicesResult", &response));
    response["sessions"] = json!(vec![session; 65]);
    assert!(!schema_subset::valid(
        "WorklogFocusChoicesResult",
        &response
    ));
    response["sessions"] = json!([]);
    response["focus_session_id"] = json!(null);
    response["selected_session"] = json!(null);
    response["choices"] = json!([]);
    response["name_changed"] = json!(false);
    common::assert_response_round_trips("WorklogFocusChoicesResult", &response);
    assert!(schema_subset::valid("WorklogFocusChoicesResult", &response));
}

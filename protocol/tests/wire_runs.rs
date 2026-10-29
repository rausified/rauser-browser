use brauser_protocol::{Request, Response, RunCursor, RunKind};
use serde_json::{Value, json};

mod common;
use common::{assert_request_round_trips, assert_response_round_trips, schema_subset};

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
        "type": "run_fetched", "protocol_version": 8, "request_id": "r",
        "run": view, "stream_text": "x", "next_offset": 1, "more": false,
    })
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
        ("task", RunKind::Task),
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
fn list_runs_without_kind_round_trips_unchanged() {
    // Today's list_runs shape: no kind, before or more key anywhere.
    let request_value = json!({
        "type": "list_runs", "protocol_version": 8, "request_id": "r",
    });
    let parsed = assert_request_round_trips("ListRunsRequest", &request_value);
    let Request::ListRuns(request) = parsed else {
        panic!("not list_runs");
    };
    assert_eq!(request.kind, None);
    assert_eq!(request.before, None);
    let encoded = serde_json::to_value(Request::ListRuns(request)).unwrap();
    let object = encoded.as_object().unwrap();
    for key in ["kind", "before"] {
        assert!(!object.contains_key(key), "{key}");
    }

    let listed_value = json!({
        "type": "runs_listed", "protocol_version": 8, "request_id": "r",
        "runs": [run_view()],
    });
    let parsed = assert_response_round_trips("RunsListed", &listed_value);
    let Response::RunsListed(listed) = parsed else {
        panic!("not runs_listed");
    };
    assert_eq!(listed.more, None);
    let encoded = serde_json::to_value(Response::RunsListed(listed)).unwrap();
    assert!(!encoded.as_object().unwrap().contains_key("more"));
}

#[test]
fn list_runs_with_kind_and_cursor_round_trips() {
    let value = json!({
        "type": "list_runs", "protocol_version": 8, "request_id": "r",
        "kind": "task",
        "before": {
            "created_at": "2026-10-28T11:34:00-07:00",
            "run_id": "00000000-0000-4000-8000-000000000001",
        },
    });
    let parsed = assert_request_round_trips("ListRunsRequest", &value);
    let Request::ListRuns(request) = parsed else {
        panic!("not list_runs");
    };
    assert_eq!(request.kind, Some(RunKind::Task));
    assert_eq!(
        request.before,
        Some(RunCursor {
            created_at: "2026-10-28T11:34:00-07:00".to_string(),
            run_id: "00000000-0000-4000-8000-000000000001".to_string(),
        })
    );

    // kind alone (no before) also round-trips: the first page of a kind.
    let mut kind_only = value.clone();
    kind_only.as_object_mut().unwrap().remove("before");
    let parsed = assert_request_round_trips("ListRunsRequest", &kind_only);
    let Request::ListRuns(request) = parsed else {
        panic!("not list_runs");
    };
    assert_eq!(request.kind, Some(RunKind::Task));
    assert_eq!(request.before, None);

    // An unknown kind is rejected at the schema layer, as RunKind elsewhere.
    let mut unknown_kind = value.clone();
    unknown_kind["kind"] = json!("chat");
    assert!(serde_json::from_value::<Request>(unknown_kind).is_err());
}

#[test]
fn runs_listed_more_is_optional() {
    let base = json!({
        "type": "runs_listed", "protocol_version": 8, "request_id": "r",
        "runs": [],
    });
    let parsed = assert_response_round_trips("RunsListed", &base);
    let Response::RunsListed(listed) = parsed else {
        panic!("not runs_listed");
    };
    assert_eq!(listed.more, None);

    for more in [true, false] {
        let mut with_more = base.clone();
        with_more["more"] = json!(more);
        let parsed = assert_response_round_trips("RunsListed", &with_more);
        let Response::RunsListed(listed) = parsed else {
            panic!("not runs_listed");
        };
        assert_eq!(listed.more, Some(more));
    }
}

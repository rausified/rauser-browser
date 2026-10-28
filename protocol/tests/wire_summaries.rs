use brauser_protocol::{Request, Response};

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

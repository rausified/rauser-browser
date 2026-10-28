use brauser_protocol::{HarnessId, Request, Response, SiteDrafter, UncheckedReason};
use brauser_protocol::{ScopeChoiceView, ScopeValues, UnscopedChoice};
use serde_json::{Value, json};

mod common;
use common::{assert_request_round_trips, assert_response_round_trips, schema_subset};

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
        "type": "start_site_draft", "protocol_version": 8, "request_id": "r1",
        "origin": "https://work.example",
        "visited": shapes("v", visited), "links": shapes("l", links),
        "harness": null, "requested_at": "2026-10-28T09:00:00-07:00",
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

//! Related open tabs (`brwr-sm2`): the optional `open_tabs` list on
//! `start_run` and `start_auto_summary` (additive within protocol version 8;
//! the host and extension deploy together).

use brauser_protocol::Request;
use serde_json::{Value, json};

mod common;
use common::{assert_request_round_trips, schema_subset};

fn start_run() -> Value {
    json!({
        "type": "start_run", "protocol_version": 8, "request_id": "r",
        "url": "https://a.example/x", "title": "T", "content": "body",
        "selection": null, "harness": null, "record_key": null,
        "requested_at": "2026-11-10T14:00:00-08:00",
    })
}

fn start_auto_summary() -> Value {
    json!({
        "type": "start_auto_summary", "protocol_version": 8, "request_id": "r",
        "url": "https://a.example/x", "title": "T", "content": "body",
        "requested_at": "2026-11-10T14:00:00-08:00",
    })
}

fn tab(index: usize) -> Value {
    json!({ "url": format!("https://b.example/{index}"), "title": format!("Tab {index}") })
}

#[test]
fn open_tabs_are_optional_on_both_summary_starts_and_round_trip() {
    for (definition, mut value) in [
        ("StartRunRequest", start_run()),
        ("StartAutoSummaryRequest", start_auto_summary()),
    ] {
        let legacy = assert_request_round_trips(definition, &value);
        match &legacy {
            Request::StartRun(request) => assert_eq!(request.open_tabs, None),
            Request::StartAutoSummary(request) => assert_eq!(request.open_tabs, None),
            other => panic!("{other:?}"),
        }
        value["open_tabs"] = json!([tab(1), { "url": "https://c.example/", "title": "" }]);
        let parsed = assert_request_round_trips(definition, &value);
        let tabs = match parsed {
            Request::StartRun(request) => request.open_tabs,
            Request::StartAutoSummary(request) => request.open_tabs,
            other => panic!("{other:?}"),
        }
        .unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].url, "https://b.example/1");
        assert_eq!(tabs[0].title, "Tab 1");
        // An empty list is still a list, written back as given.
        value["open_tabs"] = json!([]);
        assert_request_round_trips(definition, &value);
    }
}

#[test]
fn open_tabs_are_bounded_by_the_schema() {
    let all: Vec<Value> = (0..20).map(tab).collect();
    let mut value = start_run();
    value["open_tabs"] = json!(all);
    assert!(schema_subset::valid("StartRunRequest", &value));
    let mut more = all.clone();
    more.push(tab(20));
    value["open_tabs"] = json!(more);
    assert!(!schema_subset::valid("StartRunRequest", &value), "21 tabs");

    for (entry, why) in [
        (json!({ "url": "", "title": "T" }), "empty url"),
        (
            json!({ "url": format!("https://b.example/{}", "a".repeat(2048)), "title": "T" }),
            "long url",
        ),
        (
            json!({ "url": "https://b.example/", "title": "t".repeat(301) }),
            "long title",
        ),
    ] {
        value["open_tabs"] = json!([entry]);
        assert!(!schema_subset::valid("StartRunRequest", &value), "{why}");
    }
    value["open_tabs"] = json!([{ "url": "https://b.example/", "title": "t".repeat(300) }]);
    assert!(schema_subset::valid("StartRunRequest", &value));
}

#[test]
fn open_tabs_carry_titles_and_addresses_only() {
    for (definition, mut value) in [
        ("StartRunRequest", start_run()),
        ("StartAutoSummaryRequest", start_auto_summary()),
    ] {
        for entry in [
            json!({ "url": "https://b.example/", "title": "T", "content": "page text" }),
            json!({ "url": "https://b.example/" }),
            json!({ "title": "T" }),
            json!("https://b.example/"),
        ] {
            value["open_tabs"] = json!([entry]);
            assert!(!schema_subset::valid(definition, &value), "{entry}");
            assert!(
                serde_json::from_value::<Request>(value.clone()).is_err(),
                "{entry}"
            );
        }
        value["open_tabs"] = json!({ "url": "https://b.example/", "title": "T" });
        assert!(serde_json::from_value::<Request>(value.clone()).is_err());
    }
}

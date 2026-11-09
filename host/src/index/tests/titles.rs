use super::*;

/// The adapter document for `activity`'s `work.item` events, with a title
/// template and one generic title (adapter-templates.md §9.4).
fn work_document() -> adapter_templates::Document {
    let document: adapter_templates::Value = serde_json::from_value(serde_json::json!({
        "format": 1,
        "artifacts": [{
            "type": "work.item",
            "origin": "https://example.com",
            "match": [{"path": "/items/{id:token}"}],
            "id": "work:{host}/{id}",
            "title": "{title} - Acme",
            "generic_titles": ["Sign in"],
        }],
    }))
    .unwrap();
    adapter_templates::Document::from_value(&document).unwrap()
}

fn titled(n: u32, title: Option<&str>, at: &str) -> ActivityEvent {
    let mut event = activity(&format!("00000000-0000-4000-8000-{n:012}"), "A1", "x", at);
    event.title = title.map(str::to_owned);
    event
}

/// An extension that sends a generic title, whether raw (`Sign in - Acme`),
/// extracted (`Sign in`) or the host placeholder, never replaces the
/// artifact's real name, while a later real title still does. The host
/// drops the title itself, so this holds with no extension-side check.
#[test]
fn a_generic_activity_title_never_replaces_the_artifact_name() {
    let document = work_document();
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let events = [
        titled(1, Some("Plan"), "2026-10-15T09:00:00Z"),
        titled(2, Some("Sign in - Acme"), "2026-10-15T10:00:00Z"),
        titled(3, Some("SIGN IN"), "2026-10-15T11:00:00Z"),
        titled(4, Some("example.com/items/A1"), "2026-10-15T12:00:00Z"),
    ];
    for event in &events {
        index.record_activity(event, &[]).unwrap();
    }
    let id = "work:example.com/A1";
    assert_eq!(index.artifact(id).unwrap().unwrap().title, "Plan");

    index
        .record_activity(&titled(5, Some("Plan v2"), "2026-10-15T13:00:00Z"), &[])
        .unwrap();
    assert_eq!(index.artifact(id).unwrap().unwrap().title, "Plan v2");
}

/// An artifact first seen only through a generic title is "Untitled".
#[test]
fn an_artifact_seen_only_behind_a_gate_is_untitled() {
    let document = work_document();
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    index
        .record_activity(
            &titled(1, Some("Sign in - Acme"), "2026-10-15T09:00:00Z"),
            &[],
        )
        .unwrap();
    assert_eq!(
        index
            .artifact("work:example.com/A1")
            .unwrap()
            .unwrap()
            .title,
        "Untitled"
    );
}

/// Without an adapter document the host cannot judge a title, so it keeps it.
#[test]
fn without_adapters_a_title_is_kept() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    index
        .record_activity(
            &titled(1, Some("Sign in - Acme"), "2026-10-15T09:00:00Z"),
            &[],
        )
        .unwrap();
    assert_eq!(
        index
            .artifact("work:example.com/A1")
            .unwrap()
            .unwrap()
            .title,
        "Sign in - Acme"
    );
}

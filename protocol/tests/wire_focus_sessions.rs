use brauser_protocol::{ActivityEvent, FocusSession};
use serde_json::json;

#[test]
fn optional_focus_snapshot_round_trips_and_absence_keeps_old_payloads_exact() {
    let old = json!({
        "event_id":"00000000-0000-4000-8000-000000000001", "artifact_id":"work:example/1",
        "artifact_type":"work.item", "url":"https://example.com/1", "title":null,
        "occurred_at":"2026-11-03T10:00:00Z", "incognito":false, "opens":1,
        "focused_seconds":0, "background_seconds":0, "modes":[], "trails":[], "refs":[]
    });
    let bytes = serde_json::to_vec(&old).unwrap();
    let mut event: ActivityEvent = serde_json::from_slice(&bytes).unwrap();
    assert!(event.focus_session.is_none());
    assert_eq!(serde_json::to_value(&event).unwrap(), old);
    event.focus_session = Some(FocusSession {
        id: "00000000-0000-4000-8000-000000000002".into(),
        name: "Release 😀".into(),
        revision: 2,
    });
    let tagged = serde_json::to_value(&event).unwrap();
    assert_eq!(
        serde_json::from_value::<ActivityEvent>(tagged.clone()).unwrap(),
        event
    );
    let mut forged = tagged;
    forged["focus_session"]["page_text"] = json!("not allowed");
    assert!(serde_json::from_value::<ActivityEvent>(forged).is_err());
}

use brauser_protocol::{Request, Response};
use serde_json::json;

mod common;
use common::schema_subset;

#[test]
fn preserved_note_requests_and_bounded_metadata_round_trip() {
    for value in [
        json!({"type":"list_preserved_notes","protocol_version":8,"request_id":"r","url":"https://example.com/page"}),
        json!({"type":"view_preserved_note","protocol_version":8,"request_id":"r","url":"https://example.com/page","key":{"kind":"artifact","value":"item:7"}}),
    ] {
        let request: Request = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), value);
        let mut unknown = value;
        unknown["path"] = json!("/arbitrary/file");
        assert!(serde_json::from_value::<Request>(unknown).is_err());
    }
    let note = json!({"key":{"kind":"artifact","value":"item:7"},"canonical_url":"https://example.com/page","title":"Seven","revision":"v"});
    let mut value = json!({"type":"preserved_notes_listed","protocol_version":8,"request_id":"r","notes":[note.clone()]});
    let response: Response = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(response).unwrap(), value);
    assert!(schema_subset::valid("PreservedNotesListed", &value));
    value["notes"] = json!(vec![note; 25]);
    assert!(!schema_subset::valid("PreservedNotesListed", &value));
}

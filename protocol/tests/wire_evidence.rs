use brauser_protocol::{Request, Response};
use serde_json::json;
mod common;
use common::{assert_request_round_trips, assert_response_round_trips, schema_subset};

#[test]
fn selected_text_request_and_receipt_round_trip_with_host_owned_attribution() {
    let request = json!({"type":"save_evidence","protocol_version":8,"request_id":"r","url":"https://work.example/items/7","selection":"A decision 🚀\r\n"});
    assert_request_round_trips("SaveEvidenceRequest", &request);
    for field in [
        "key",
        "path",
        "canonical_url",
        "captured_at",
        "content",
        "harness",
    ] {
        let mut forged = request.clone();
        forged[field] = json!("forged");
        assert!(serde_json::from_value::<Request>(forged.clone()).is_err());
        assert!(!schema_subset::valid("SaveEvidenceRequest", &forged));
    }
    let response = json!({"type":"evidence_saved","protocol_version":8,"request_id":"r","relative_path":"notes/evidence/card.md","key":{"kind":"artifact","value":"work:7"},"canonical_url":"https://work.example/items/7","captured_at":"2026-11-03T00:00:00Z","selection_sha256":"a".repeat(64),"already_saved":false,"warnings":[]});
    assert_response_round_trips("EvidenceSaved", &response);
    let mut extra = response;
    extra["selection"] = json!("never echoed");
    assert!(serde_json::from_value::<Response>(extra).is_err());
    let mut oversized = request;
    oversized["selection"] = json!("a".repeat(32769));
    assert!(!schema_subset::valid("SaveEvidenceRequest", &oversized));
}

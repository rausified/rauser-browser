//! The adapter fingerprint (`adapters_fingerprint`): what the index records
//! as the document it was derived under. It must change exactly when the
//! document's content does (brwr-8dlu).

use super::*;

fn document(raw: serde_json::Value) -> Document {
    let value: adapter_templates::Value = serde_json::from_value(raw).unwrap();
    Document::from_value(&value).unwrap()
}

/// One adapter that uses every adapter key the fingerprint input can hold
/// (synthetic hosts only), written once in each key order.
fn every_key(generic: &str) -> serde_json::Value {
    serde_json::json!({
        "format": 1,
        "artifacts": [{
            "type": "work.item",
            "origin": "https://work.example.com",
            "match": [{"path": "/items/{id:int}", "query": {"view": "full"}}],
            "id": "work:{host}/{id}",
            "title": "{title} - Example Work",
            "generic_titles": [generic],
            "refs": [{"type": "jira.issue", "find": "jira_key"}],
            "fields": {"status": ".status"},
        }],
    })
}

/// The bytes are pinned, so any change to the input form (the canonical
/// encoder, the prefix, a field the document starts writing) fails here
/// and re-dirties every user's index only if someone updates this value on
/// purpose. Update it only with a note in the PR that existing indexes
/// refold once. The value is also what Python gives for
/// `sha256("adapters content/v1\n" + json.dumps(doc, sort_keys=True,
/// separators=(",", ":"), ensure_ascii=False))`, so the input is plain
/// canonical JSON that another tool can reproduce.
#[test]
fn the_fingerprint_of_a_fixed_document_is_pinned() {
    assert_eq!(
        adapters_fingerprint(&document(every_key("Sign in"))),
        "0db61aad19a52e3c5e4743aafa96c7f663d83508241c6b00d5a843fc199ee047"
    );
    assert_eq!(
        adapters_fingerprint(&Document::default()),
        adapters_fingerprint(&document(serde_json::json!({"format": 1, "artifacts": []})))
    );
}

/// Key order is not content: the same document written in another order
/// keeps its fingerprint, so a reordered config never refolds the index.
#[test]
fn key_order_does_not_change_the_fingerprint() {
    let reordered: serde_json::Value = serde_json::from_str(
        r#"{"artifacts": [{"fields": {"status": ".status"},
            "refs": [{"find": "jira_key", "type": "jira.issue"}],
            "generic_titles": ["Sign in"], "title": "{title} - Example Work",
            "id": "work:{host}/{id}",
            "match": [{"query": {"view": "full"}, "path": "/items/{id:int}"}],
            "origin": "https://work.example.com", "type": "work.item"}],
          "format": 1}"#,
    )
    .unwrap();
    assert_eq!(
        adapters_fingerprint(&document(reordered)),
        adapters_fingerprint(&document(every_key("Sign in")))
    );
}

/// Content is: one changed generic title, one added key, or one removed
/// adapter each changes the fingerprint.
#[test]
fn a_content_change_changes_the_fingerprint() {
    let base = adapters_fingerprint(&document(every_key("Sign in")));
    assert_ne!(adapters_fingerprint(&document(every_key("Log in"))), base);
    let mut added = every_key("Sign in");
    added["artifacts"][0]["path_case"] = serde_json::json!("insensitive");
    assert_ne!(adapters_fingerprint(&document(added)), base);
    assert_ne!(
        adapters_fingerprint(&document(serde_json::json!({"format": 1, "artifacts": []}))),
        base
    );
}

/// The fingerprint never reads `Debug`: two documents whose adapters print
/// the same way but whose content differs still differ, and the input is
/// exactly the canonical text.
#[test]
fn the_fingerprint_hashes_the_canonical_text_not_debug() {
    let doc = document(every_key("Sign in"));
    let expected = sha256_hex(format!("adapters content/v1\n{}", doc.canonical_json()).as_bytes());
    assert_eq!(adapters_fingerprint(&doc), expected);
    // Generic titles are stored folded, so `Sign in` and `SIGN IN` print
    // alike; their source content differs, and so do their fingerprints.
    let upper = document(every_key("SIGN IN"));
    assert_eq!(format!("{doc:?}"), format!("{upper:?}"));
    assert_ne!(adapters_fingerprint(&doc), adapters_fingerprint(&upper));
}

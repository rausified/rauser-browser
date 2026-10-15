//! URL preparation and path matching (§5): any string is a URL to evaluate
//! against a fixed document. Results must respect the §5.1 and §8.1 caps.

#![no_main]

use std::sync::LazyLock;

use adapter_templates::{Document, MAX_ID_BYTES, Value, prepare_segments};
use libfuzzer_sys::fuzz_target;

/// Rules of every capture shape, including `@tail`, optional captures,
/// alternations, query conditions, and `absent`.
const DOC: &str = r#"{"format": 1, "artifacts": [
  {"type": "f.int", "origin": "https://fuzz.example.com",
   "match": [{"path": "/i/{id:int}/{mode?:(edit|view)}"}],
   "id": "f.int:{host}/{id}", "path_case": "insensitive", "modes": {"edit": "edit", "view": "view"}},
  {"type": "f.tail", "origin": "https://fuzz.example.com",
   "match": [{"path": "/j/{id:int@tail}"}], "id": "f.tail:{host}/{id}"},
  {"type": "f.hex", "origin": "https://fuzz.example.com",
   "match": [{"path": "/h/{id:hex(4,64)}/{*}"}], "id": "f.hex:{host}/{id}"},
  {"type": "f.key", "origin": "https://fuzz.example.com",
   "match": [{"path": "/t/{org:name|lower}/{id:jira_key}"}], "id": "f.key:{host}/{org}/{id}"},
  {"type": "f.uuid", "origin": "https://fuzz.example.com",
   "match": [{"path": "/u/{id:uuid}", "query": {"tab": "{_}"}, "absent": ["x"]}], "id": "f.uuid:{host}/{id}"},
  {"type": "f.token", "origin": "https://fuzz.example.com",
   "match": [{"path": "/q", "query": {"id": "{id:token}", "k": "view"}}], "id": "f.token:{host}/{id}"}
]}"#;

static DOCUMENT: LazyLock<Document> = LazyLock::new(|| {
    let value: Value = serde_json::from_str(DOC).expect("fixture is JSON");
    Document::from_value(&value).expect("fixture is valid")
});

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Some(segments) = prepare_segments(text) {
        assert!(segments.len() <= 64);
    }
    for url in [text.to_owned(), format!("https://fuzz.example.com{text}")] {
        if let Ok(found) = DOCUMENT.evaluate(&url, None) {
            assert!(found.artifact_id.len() <= MAX_ID_BYTES);
        }
    }
});

//! The document parser and validator (§3–§10.3): any JSON value, and any
//! string as a ctype, either validates or returns an error, never panics.
//! A document that validates must also evaluate without panicking.

#![no_main]

use adapter_templates::{CType, Document, Value};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(ctype) = CType::parse(text) {
        let _ = ctype.canonical(text);
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if let Ok(doc) = Document::from_value(&value) {
        let _ = doc.overlap_report();
        for adapter in doc.adapters() {
            let url = format!("{}/a/1", adapter.origin());
            let _ = doc.evaluate(&url, Some("a - 1"));
        }
    }
});

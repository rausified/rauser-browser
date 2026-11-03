//! Throwaway: evaluates `brauser adapters eval`-shaped JSON Lines with the
//! Rust adapter-templates crate (same calls as host/src/adapters.rs), so the
//! Vidcast vectors can be checked without building the whole host.
use std::io::{self, BufRead, Write};
use adapter_templates::{Document, Value};
use serde_json::{json, Value as Json};

fn main() {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        if line.trim().is_empty() { continue; }
        let req: Json = serde_json::from_str(&line).unwrap();
        let doc: Value = serde_json::from_value(req["doc"].clone()).unwrap();
        let url = req["url"].as_str().unwrap();
        let title = req.get("title").and_then(|t| t.as_str());
        let result = match Document::from_value(&doc) {
            Err(e) => json!({"error": {"code": e.code.as_str(), "adapter": e.adapter, "rule": e.rule, "entry": e.entry}}),
            Ok(doc) => match doc.evaluate(url, title) {
                Err(reason) => json!({"match": false, "reason": reason.as_str()}),
                Ok(m) => json!({
                    "match": true, "adapter": m.adapter, "rule": m.rule, "type": m.artifact_type,
                    "artifact_id": m.artifact_id, "mode": m.mode.map(|x| x.as_str()),
                    "title": m.title.as_ref().map(|t| t.title.clone()),
                }),
            },
        };
        writeln!(out, "{result}").unwrap();
    }
}

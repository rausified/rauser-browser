//! Reads the same JSON Lines requests as `brauser adapters eval` ({"doc", "url", "title"?})
//! and prints validation errors or match summaries with the Rust engine.
use adapter_templates::{Document, Value};
use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        if line.trim().is_empty() { continue; }
        let req: serde_json::Value = serde_json::from_str(&line).unwrap();
        let doc: Value = serde_json::from_value(req["doc"].clone()).unwrap();
        let url = req["url"].as_str().unwrap();
        let title = req.get("title").and_then(|t| t.as_str());
        if std::env::var_os("OVERLAP").is_some() {
            if let Ok(d) = Document::from_value(&doc) {
                let r: Vec<_> = d.overlap_report().iter().map(|p| (p.first, p.second)).collect();
                let all: Vec<_> = d.overlapping_rules().iter().map(|p| (p.first, p.second)).collect();
                writeln!(out, "{}", serde_json::json!({"overlap_report": r, "overlapping_rules": all})).unwrap();
            }
            return;
        }
        let result = match Document::from_value(&doc) {
            Err(e) => serde_json::json!({"error": format!("{e:?}")}),
            Ok(d) => match d.evaluate(url, title) {
                Err(no) => serde_json::json!({"match": false, "reason": no.as_str()}),
                Ok(m) => serde_json::json!({
                    "match": true, "adapter": m.adapter, "rule": m.rule,
                    "type": m.artifact_type, "artifact_id": m.artifact_id,
                    "mode": m.mode.map(|x| x.as_str()),
                    "title": m.title.as_ref().map(|t| t.title.clone()),
                    "title_entry": m.title.as_ref().and_then(|t| t.entry),
                }),
            },
        };
        writeln!(out, "{result}").unwrap();
    }
}

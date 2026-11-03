// Throwaway: JSON Lines {"doc","url","title"?} on stdin -> host-shaped result per line.
use adapter_templates::{Document, Value};
use serde_json::{Value as Json, json};
use std::io::BufRead;
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--overlap") {
        let mut line = String::new(); std::io::stdin().read_line(&mut line).unwrap();
        let doc: Value = serde_json::from_str(&line).unwrap();
        let d = Document::from_value(&doc).expect("valid");
        let pairs = d.overlapping_rules();
        println!("adapters={} overlapping_rule_pairs={}", d.adapters().len(), pairs.len());
        for p in pairs { println!("  {:?} ~ {:?}", p.first, p.second); }
        return;
    }
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        if line.trim().is_empty() { continue; }
        let req: Json = serde_json::from_str(&line).unwrap();
        let doc: Value = serde_json::from_value(req["doc"].clone()).unwrap();
        let out = match Document::from_value(&doc) {
            Err(e) => json!({"error": {"code": e.code.as_str(), "adapter": e.adapter, "rule": e.rule, "entry": e.entry}}),
            Ok(d) => match d.evaluate(req["url"].as_str().unwrap(), req["title"].as_str()) {
                Err(r) => json!({"match": false, "reason": r.as_str()}),
                Ok(m) => json!({"match": true, "type": m.artifact_type, "artifact_id": m.artifact_id,
                    "title": m.title.as_ref().map(|t| t.title.clone()),
                    "refs": m.title.as_ref().map(|t| t.refs.iter().map(|r| r.value.clone()).collect::<Vec<_>>())}),
            },
        };
        println!("{out}");
    }
}

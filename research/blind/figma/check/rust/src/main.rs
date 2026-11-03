//! Runs research/blind/figma/vectors.json through the host's Rust engine
//! (adapter-templates crate) and prints overlap warnings (§10.3).
//! Usage: cargo run -- <doc.json> <vectors.json>
use adapter_templates::{Document, Value};
use serde_json::Value as Json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let vectors: Json = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let doc = match Document::from_value(&doc) {
        Ok(d) => d,
        Err(e) => { println!("VALIDATION ERROR {:?}", e); std::process::exit(1) }
    };
    println!("document validates");
    println!("overlap_report: {:?}", doc.overlap_report());
    let (mut pass, mut fail) = (0, 0);
    for v in vectors["positive"].as_array().unwrap() {
        let e = &v["expect"];
        let r = doc.evaluate(v["url"].as_str().unwrap(), v["title"].as_str());
        let mut problems = Vec::new();
        match &r {
            Err(reason) => problems.push(format!("no match ({})", reason.as_str())),
            Ok(m) => {
                if m.artifact_type != e["type"].as_str().unwrap() { problems.push(format!("type {}", m.artifact_type)); }
                if m.artifact_id != e["artifact_id"].as_str().unwrap() { problems.push(format!("id {}", m.artifact_id)); }
                let mode = m.mode.map(|x| x.as_str());
                if mode != e["mode"].as_str() { problems.push(format!("mode {:?}", mode)); }
                if let Some(t) = e.get("title") {
                    let got = m.title.as_ref().map(|t| t.title.as_str());
                    if got != t.as_str() { problems.push(format!("title {:?}", got)); }
                }
                if let Some(te) = e.get("title_entry") {
                    let got = m.title.as_ref().and_then(|t| t.entry).map(|x| x as u64);
                    if got != te.as_u64() { problems.push(format!("title_entry {:?}", got)); }
                }
            }
        }
        if problems.is_empty() { pass += 1 } else { fail += 1; println!("FAIL {} {}", v["name"], problems.join("; ")); }
    }
    for v in vectors["negative"].as_array().unwrap() {
        match doc.evaluate(v["url"].as_str().unwrap(), None) {
            Ok(m) => { fail += 1; println!("FAIL negative matched {} -> {}", v["name"], m.artifact_id) }
            Err(_) => pass += 1,
        }
    }
    println!("rust engine: {pass} passed, {fail} failed");
    std::process::exit(if fail > 0 { 1 } else { 0 });
}

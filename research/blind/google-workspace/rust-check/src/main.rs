use adapter_templates::{Document, Value};
use serde_json::Value as Json;

fn main() {
    let dir = std::env::args().nth(1).expect("folder");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/doc.json")).unwrap()).unwrap();
    let doc = match Document::from_value(&doc) {
        Ok(d) => d,
        Err(e) => { println!("VALIDATION ERROR {:?} adapter {:?} rule {:?} entry {:?}", e.code, e.adapter, e.rule, e.entry); std::process::exit(1) }
    };
    println!("validated: {} adapters", doc.adapters().len());
    let overlaps = doc.overlap_report();
    println!("overlap warnings: {}", overlaps.len());
    for p in &overlaps { println!("  may overlap: {:?} vs {:?}", p.first, p.second); }
    let vectors: Json = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/vectors.json")).unwrap()).unwrap();
    let cases = vectors["cases"].as_array().unwrap();
    let mut fail = 0;
    for c in cases {
        let url = c["url"].as_str().unwrap();
        let title = c.get("title").and_then(|t| t.as_str());
        let e = &c["expect"];
        let mut bad = Vec::new();
        match doc.evaluate(url, title) {
            Err(reason) => if e["match"].as_bool().unwrap() { bad.push(format!("no match ({})", reason.as_str())) },
            Ok(m) => {
                if !e["match"].as_bool().unwrap() { bad.push(format!("unexpected match {}", m.artifact_id)) } else {
                    if m.artifact_type != e["type"].as_str().unwrap() { bad.push(format!("type {}", m.artifact_type)) }
                    if m.artifact_id != e["artifact_id"].as_str().unwrap() { bad.push(format!("id {}", m.artifact_id)) }
                    if m.mode.map(|x| x.as_str()) != e["mode"].as_str() { bad.push(format!("mode {:?}", m.mode.map(|x| x.as_str()))) }
                    if let Some(t) = e.get("title").and_then(|t| t.as_str()) {
                        let got = m.title.as_ref().map(|x| x.title.as_str());
                        if got != Some(t) { bad.push(format!("title {got:?}")) }
                    }
                }
            }
        }
        if !bad.is_empty() { fail += 1; println!("FAIL {}: {}", c["name"], bad.join("; ")); }
    }
    println!("{}/{} vectors pass", cases.len() - fail, cases.len());
    std::process::exit(if fail > 0 { 1 } else { 0 });
}

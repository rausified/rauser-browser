use adapter_templates::{Document, Value};
use serde_json::Value as J;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let doc_text = std::fs::read_to_string(&args[1]).unwrap();
    let raw: J = serde_json::from_str(&doc_text).unwrap();
    let doc = match Document::from_value(&serde_json::from_str::<Value>(&doc_text).unwrap()) {
        Ok(d) => d,
        Err(e) => { println!("VALIDATION ERROR {e:?}"); std::process::exit(1) }
    };
    println!("validated (rust): {} adapters", raw["artifacts"].as_array().unwrap().len());
    let vectors: J = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let (mut pass, mut total) = (0, 0);
    for c in vectors["cases"].as_array().unwrap() {
        total += 1;
        let d = if let Some(o) = c["origin_override"].as_str() {
            let mut r = raw.clone();
            for a in r["artifacts"].as_array_mut().unwrap() { a["origin"] = J::String(o.into()); }
            Document::from_value(&serde_json::from_str::<Value>(&r.to_string()).unwrap()).unwrap()
        } else { Document::from_value(&serde_json::from_str::<Value>(&doc_text).unwrap()).unwrap() };
        let _ = &doc;
        let e = &c["expect"];
        let r = d.evaluate(c["url"].as_str().unwrap(), c["title"].as_str());
        let mut bad = vec![];
        match (&r, e["match"].as_bool().unwrap()) {
            (Ok(m), true) => {
                if let Some(t) = e["type"].as_str() { if m.artifact_type != t { bad.push(format!("type={}", m.artifact_type)); } }
                if let Some(t) = e["rule"].as_u64() { if m.rule as u64 != t { bad.push(format!("rule={}", m.rule)); } }
                if let Some(t) = e["artifact_id"].as_str() { if m.artifact_id != t { bad.push(format!("id={}", m.artifact_id)); } }
                if let Some(t) = e.get("title") {
                    let got = m.title.as_ref().map(|t| t.title.clone());
                    let want = t.as_str().map(String::from);
                    if got != want { bad.push(format!("title={got:?}")); }
                }
                if e.get("title_entry").is_some() {
                    let got = m.title.as_ref().and_then(|t| t.entry);
                    if got.map(|v| v as u64) != e["title_entry"].as_u64() { bad.push(format!("title_entry={got:?}")); }
                }
            }
            (Err(_), false) => {}
            (Ok(m), false) => bad.push(format!("unexpected match {}", m.artifact_id)),
            (Err(n), true) => bad.push(format!("no match {n:?}")),
        }
        if bad.is_empty() { pass += 1 } else { println!("FAIL {} {}", c["id"], bad.join(" ")); }
    }
    println!("{pass}/{total} vectors pass (rust)");
    if pass != total { std::process::exit(1) }
}

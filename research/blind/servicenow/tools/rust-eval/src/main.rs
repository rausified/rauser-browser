use adapter_templates::{Document, Value};
use serde_json::Value as Json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let doc = match Document::from_value(&doc) {
        Ok(d) => d,
        Err(e) => { println!("VALIDATION ERROR {:?}", e); std::process::exit(1) }
    };
    println!("document valid: {} adapters", doc.adapters().len());
    let vec: Json = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let mut fail = 0;
    let pos = vec["positives"].as_array().unwrap();
    let neg = vec["negatives"].as_array().unwrap();
    for v in pos {
        let title = v.get("title").and_then(|t| t.as_str());
        match doc.evaluate(v["url"].as_str().unwrap(), title) {
            Ok(m) => {
                let ok_id = m.artifact_type == v["type"].as_str().unwrap() && m.artifact_id == v["id"].as_str().unwrap();
                let ok_title = title.is_none() || m.title.as_ref().map(|t| t.title.as_str()) == v["expected_title"].as_str();
                if !ok_id || !ok_title { fail += 1; println!("FAIL+ {} {} {:?}", v["url"], m.artifact_id, m.title.map(|t| t.title)); }
            }
            Err(r) => { fail += 1; println!("FAIL+ {} no match {}", v["url"], r.as_str()); }
        }
    }
    for v in neg {
        if let Ok(m) = doc.evaluate(v["url"].as_str().unwrap(), None) { fail += 1; println!("FAIL- {} {}", v["url"], m.artifact_id); }
    }
    println!("positives {}, negatives {}, failures {}", pos.len(), neg.len(), fail);
    std::process::exit(if fail > 0 { 1 } else { 0 });
}

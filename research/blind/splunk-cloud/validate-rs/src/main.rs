use adapter_templates::{Document, Value};
use serde_json::{Value as Json, json};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let doc_json = std::fs::read_to_string(&args[1]).expect("doc json");
    let vectors: Json = serde_json::from_str(&std::fs::read_to_string(&args[2]).expect("vectors")).unwrap();
    let value: Value = serde_json::from_str(&doc_json).expect("doc value");
    let doc = match Document::from_value(&value) {
        Ok(doc) => doc,
        Err(e) => {
            println!("{}", json!({"validation_error": format!("{e:?}")}));
            std::process::exit(2);
        }
    };
    println!("{}", json!({"valid": true, "adapters": doc.adapters().len()}));
    for pair in doc.overlap_report() {
        let (a, b) = (&doc.adapters()[pair.first.0], &doc.adapters()[pair.second.0]);
        println!("{}", json!({"overlap": {
            "first": [a.artifact_type(), pair.first.1], "second": [b.artifact_type(), pair.second.1]}}));
    }
    for case in vectors["cases"].as_array().unwrap() {
        let url = case["url"].as_str().unwrap();
        let title = case.get("title").and_then(Json::as_str);
        let out = match doc.evaluate(url, title) {
            Err(reason) => json!({"match": false, "reason": reason.as_str()}),
            Ok(m) => json!({
                "match": true, "type": m.artifact_type, "artifact_id": m.artifact_id,
                "mode": m.mode.map(|x| x.as_str()),
                "within": m.within.map(|w| json!({"type": w.artifact_type, "artifact_id": w.artifact_id})),
                "title": m.title.map(|t| t.title),
            }),
        };
        println!("{}", json!({"case": case["name"], "result": out}));
    }
}

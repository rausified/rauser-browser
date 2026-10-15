//! Title matching (§9) and ref scanning (§10.1): the first line of the input
//! is a title template, the rest is the tab title. Matching must not panic,
//! and every match must respect the §9.1 and §10.1 caps.

#![no_main]

use adapter_templates::{BaseType, Document, MAX_REFS, Value, scan_refs};
use libfuzzer_sys::fuzz_target;

fn text(s: &str) -> Value {
    Value::String(s.to_owned())
}

fn document(template: &str) -> Value {
    let adapter = Value::Table(vec![
        ("type".into(), text("f.item")),
        ("origin".into(), text("https://fuzz.example.com")),
        (
            "match".into(),
            Value::Array(vec![Value::Table(vec![(
                "path".into(),
                text("/o/{owner:name|lower}/{id:int}"),
            )])]),
        ),
        ("id".into(), text("f:{host}/{owner}/{id}")),
        (
            "title".into(),
            Value::Array(vec![text(template), text("{title} - {~}")]),
        ),
        (
            "alias".into(),
            Value::Table(vec![
                ("from".into(), text("tab_title")),
                ("capture".into(), text("id")),
            ]),
        ),
        (
            "refs".into(),
            Value::Array(vec![Value::Table(vec![
                ("type".into(), text("f.item")),
                ("find".into(), text("jira_key")),
            ])]),
        ),
    ]);
    Value::Table(vec![
        ("format".into(), Value::Int(1)),
        ("artifacts".into(), Value::Array(vec![adapter])),
    ])
}

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };
    let (template, title) = input.split_once('\n').unwrap_or((input, ""));
    for find in [
        BaseType::Int,
        BaseType::Hex,
        BaseType::JiraKey,
        BaseType::Uuid,
    ] {
        let _ = scan_refs(find, title);
    }
    let Ok(doc) = Document::from_value(&document(template)) else {
        return;
    };
    let url = "https://fuzz.example.com/o/acme/7";
    if let Ok(found) = doc.evaluate(url, Some(title)) {
        let matched = found.title.expect("a title was given");
        assert!(matched.title.chars().count() <= 512);
        assert!(matched.refs.len() <= MAX_REFS);
    }
});

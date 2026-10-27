//! The structural drafter (DESIGN §5.1): any bytes, split into lines, are
//! URL shapes. A line starting with `>` is a link shape, any other a visited
//! one, and lines `UrlShape::parse` refuses are dropped, as the host drops
//! them. Whatever the input, every rule `draft_site` returns must pass
//! `check_draft_rules` again, and no rule may match the origin root.

#![no_main]

use std::sync::LazyLock;

use adapter_templates::shape::UrlShape;
use adapter_templates::structural::{DraftedAdapter, ShapeInput, draft_site};
use adapter_templates::{
    Document, DraftPolicy, Fill, Provenance, RuleVerdict, ScopeState, SharedOrigin, Value,
    check_draft_rules,
};
use libfuzzer_sys::fuzz_target;
use serde_json::Value as Json;

const CATALOG: &str = include_str!("../../../protocol/capture-site/catalog.json");

/// A plain origin and a shared one with a tenant position.
const ORIGINS: [&str; 2] = ["https://fuzz.example.com", "https://github.com"];

/// The host catalog, parsed the way the host parses it.
static POLICY: LazyLock<DraftPolicy> = LazyLock::new(|| {
    let c: Json = serde_json::from_str(CATALOG).expect("catalog is JSON");
    let strings = |v: &Json| -> Vec<String> {
        v.as_array()
            .expect("list")
            .iter()
            .map(|s| s.as_str().expect("string").to_owned())
            .collect()
    };
    let fill = |k: &str| c["fill"][k].as_str().expect("fill").to_owned();
    let segment = |v: &Json| usize::try_from(v.as_u64().expect("segment")).expect("usize");
    DraftPolicy {
        probe_paths: strings(&c["probe_paths"]),
        search_keys: strings(&c["search_keys"]),
        paging_keys: strings(&c["paging_keys"]),
        blocked_route_words: strings(&c["blocked_route_words"]),
        mode_words: c["mode_words"]
            .as_object()
            .expect("mode_words")
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().expect("mode").to_owned()))
            .collect(),
        fill: Fill {
            num: fill("num"),
            key: fill("key"),
            slug: fill("slug"),
            uuid: fill("uuid"),
            hex32: fill("hex32"),
            hex40: fill("hex40"),
            hex64: fill("hex64"),
            tailhex32: fill("tailhex32"),
            id: fill("id"),
        },
        starter_types: strings(&c["starter_types"]),
        shared: c["shared_origins"]
            .as_object()
            .expect("shared_origins")
            .iter()
            .map(|(origin, e)| SharedOrigin {
                origin: origin.clone(),
                tenant: e["tenant"].as_object().map(|t| {
                    (
                        segment(&t["segment"]),
                        t["capture"].as_str().expect("capture").to_owned(),
                    )
                }),
                account_index: e["account_index"]
                    .as_array()
                    .expect("account_index")
                    .iter()
                    .map(|a| {
                        (
                            a["literal"].as_str().expect("literal").to_owned(),
                            segment(&a["segment"]),
                        )
                    })
                    .collect(),
            })
            .collect(),
    }
});

static EMPTY: LazyLock<Document> = LazyLock::new(|| {
    let doc = Value::Table(vec![
        ("format".into(), Value::Int(1)),
        ("artifacts".into(), Value::Array(Vec::new())),
    ]);
    Document::from_value(&doc).expect("empty document")
});

/// The adapter alone as a document, matching without scope as the draft
/// checks do while the scope is the review's to choose.
fn evaluable(adapter: &DraftedAdapter) -> Document {
    let Value::Table(mut table) = adapter.adapter.clone() else {
        panic!("a drafted adapter is a table");
    };
    match adapter.scope {
        ScopeState::NotShared => {}
        ScopeState::Pending { .. } | ScopeState::UnscopedOnly => {
            table.push(("unscoped".into(), Value::String("acknowledged".into())));
        }
        // An empty existing document has no scope to copy.
        ScopeState::Copied(_) => panic!("scope copied from an empty document"),
    }
    let doc = Value::Table(vec![
        ("format".into(), Value::Int(1)),
        ("artifacts".into(), Value::Array(vec![Value::Table(table)])),
    ]);
    Document::from_value(&doc).expect("a drafted adapter validates")
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut input = ShapeInput::default();
    for line in text.lines() {
        let (list, shape) = match line.strip_prefix('>') {
            Some(link) => (&mut input.links, link),
            None => (&mut input.visited, line),
        };
        if let Ok(parsed) = UrlShape::parse(shape) {
            list.push((parsed, 1));
        }
    }
    for origin in ORIGINS {
        let site = draft_site(origin, &EMPTY, &input, &POLICY, &[]);
        let mut accepted: Vec<Document> = Vec::new();
        for adapter in &site.adapters {
            let count = adapter
                .adapter
                .get("match")
                .and_then(Value::as_array)
                .map_or(0, <[Value]>::len);
            assert!(count > 0, "an adapter with no rule is removed");
            assert_eq!(count, adapter.rules.len());
            let provenance = vec![Provenance::Drafted; count];
            let report =
                check_draft_rules(&EMPTY, &adapter.adapter, &provenance, &POLICY, &accepted)
                    .unwrap_or_else(|e| panic!("returned adapter refused: {e:?}"));
            assert!(
                report.rules.iter().all(|v| *v == RuleVerdict::Kept),
                "returned rules fail the draft checks: {:?}",
                report.rules
            );
            accepted.extend(report.document);
            let doc = evaluable(adapter);
            assert!(
                doc.evaluate(&format!("{origin}/"), None).is_err(),
                "a drafted rule matches the root"
            );
        }
    }
});

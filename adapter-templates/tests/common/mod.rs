//! Helpers shared by the structural and agent draft integration tests.

#![allow(dead_code)]

use adapter_templates::shape::UrlShape;
use adapter_templates::structural::{DraftedAdapter, SiteDraft};
use adapter_templates::{Document, DraftPolicy, Fill, ScopeState, SharedOrigin, Value};
use serde_json::Value as Json;

pub const SHAPES: &str = include_str!("../../../protocol/capture-site/shapes.json");
pub const CATALOG: &str = include_str!("../../../protocol/capture-site/catalog.json");

pub const TOOLS: &str = "https://tools.example.com";

/// The host catalog as a [`DraftPolicy`], built field by field.
pub fn policy() -> DraftPolicy {
    let catalog: Json = serde_json::from_str(CATALOG).expect("catalog.json parses");
    let strings = |v: &Json| -> Vec<String> {
        v.as_array()
            .expect("string list")
            .iter()
            .map(|s| s.as_str().expect("string").to_owned())
            .collect()
    };
    let fill = |class: &str| {
        catalog["fill"][class]
            .as_str()
            .expect("fill value")
            .to_owned()
    };
    let segment = |v: &Json| usize::try_from(v.as_u64().expect("segment")).expect("usize");
    DraftPolicy {
        probe_paths: strings(&catalog["probe_paths"]),
        search_keys: strings(&catalog["search_keys"]),
        paging_keys: strings(&catalog["paging_keys"]),
        blocked_route_words: strings(&catalog["blocked_route_words"]),
        secret_query_keys: strings(&catalog["secret_query_keys"]),
        secret_query_key_parts: strings(&catalog["secret_query_key_parts"]),
        secret_query_key_suffixes: strings(&catalog["secret_query_key_suffixes"]),
        mode_words: catalog["mode_words"]
            .as_object()
            .expect("mode_words")
            .iter()
            .map(|(word, mode)| (word.clone(), mode.as_str().expect("mode").to_owned()))
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
        starter_types: strings(&catalog["starter_types"]),
        shared: catalog["shared_origins"]
            .as_object()
            .expect("shared_origins")
            .iter()
            .map(|(origin, entry)| SharedOrigin {
                origin: origin.clone(),
                tenant: entry["tenant"].as_object().map(|t| {
                    (
                        segment(&t["segment"]),
                        t["capture"].as_str().expect("capture").to_owned(),
                    )
                }),
                account_index: entry["account_index"]
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
}

/// A config document holding `adapters`.
pub fn document(adapters: Vec<Value>) -> Document {
    let doc = Value::Table(vec![
        ("format".into(), Value::Int(1)),
        ("artifacts".into(), Value::Array(adapters)),
    ]);
    Document::from_value(&doc).unwrap_or_else(|e| panic!("valid document: {e:?}"))
}

pub fn empty() -> Document {
    document(Vec::new())
}

pub fn value(json: Json) -> Value {
    serde_json::from_value(json).expect("value")
}

pub fn shapes(list: &[&str]) -> Vec<(UrlShape, u32)> {
    list.iter()
        .map(|s| (UrlShape::parse(s).unwrap_or_else(|e| panic!("{s}: {e}")), 1))
        .collect()
}

/// Each rule as `path` or `path?key=template&...`.
pub fn rules(adapter: &DraftedAdapter) -> Vec<String> {
    adapter
        .adapter
        .get("match")
        .and_then(Value::as_array)
        .expect("match")
        .iter()
        .map(|rule| {
            let mut text = rule
                .get("path")
                .and_then(Value::as_str)
                .expect("path")
                .to_owned();
            for (index, (key, template)) in rule
                .get("query")
                .and_then(Value::as_table)
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                text.push(if index == 0 { '?' } else { '&' });
                text.push_str(key);
                text.push('=');
                text.push_str(template.as_str().unwrap_or("<non-string>"));
            }
            text
        })
        .collect()
}

/// The tenants the recorded fixture URLs name, for a draft whose only
/// anchor is the tenant the review scopes.
pub const FIXTURE_TENANTS: &[&str] = &["acme", "acme-corp"];

/// The drafted adapter table as it would be evaluated: the checked table,
/// unscoped while the scope is the review's to choose, so it matches without
/// scope as the draft checks do, or with the copied scope. A pending draft
/// anchored only by its tenant cannot be unscoped (adapter-templates §4.1),
/// so it is scoped to the fixtures' tenants instead.
pub fn evaluable(adapter: &DraftedAdapter) -> Value {
    let Value::Table(mut table) = adapter.adapter.clone() else {
        panic!("adapter table");
    };
    match &adapter.scope {
        ScopeState::NotShared => {}
        ScopeState::Pending { capture } => {
            let mut unscoped = table.clone();
            unscoped.push(("unscoped".into(), Value::String("acknowledged".into())));
            let valid = Document::from_value(&Value::Table(vec![
                ("format".into(), Value::Int(1)),
                (
                    "artifacts".into(),
                    Value::Array(vec![Value::Table(unscoped.clone())]),
                ),
            ]))
            .is_ok();
            if valid {
                table = unscoped;
            } else {
                let tenants = FIXTURE_TENANTS.iter().map(|t| Value::String((*t).into()));
                table.push((
                    "scope".into(),
                    Value::Table(vec![(capture.clone(), Value::Array(tenants.collect()))]),
                ));
            }
        }
        ScopeState::UnscopedOnly => {
            table.push(("unscoped".into(), Value::String("acknowledged".into())));
        }
        ScopeState::Copied(scope) => table.push((
            "scope".into(),
            Value::Table(
                scope
                    .iter()
                    .map(|(name, values)| {
                        (
                            name.clone(),
                            Value::Array(values.iter().cloned().map(Value::String).collect()),
                        )
                    })
                    .collect(),
            ),
        )),
    }
    Value::Table(table)
}

/// The whole draft as one document, for the engine.
pub fn drafted_document(site: &SiteDraft) -> Document {
    document(site.adapters.iter().map(evaluable).collect())
}

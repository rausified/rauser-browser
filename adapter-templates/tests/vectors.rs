//! Runs every shared vector whose `impls` includes `"rust"`
//! (`protocol/adapter-templates/vectors.json`) and fails on any mismatch.
//!
//! Cases marked `pending` are parsed but skipped, and so are the ids listed
//! in `tests/disputed.txt`. Every mismatch is reported, not just the first.

use std::collections::HashSet;
use std::time::Instant;

use adapter_templates::{
    BaseType, CType, Document, DraftOutcome, DraftPolicy, Fill, Match, NoMatch, Provenance, Ref,
    ResolvedScope, RuleVerdict, Scope, ScopeChoice, ScopeState, SharedOrigin, Value, check_draft,
    check_draft_rules, prepare_segments, resolve_scope_choice, scan_refs,
};
use serde_json::{Map, Value as Json, json};

const VECTORS: &str = include_str!("../../protocol/adapter-templates/vectors.json");
const DISPUTED: &str = include_str!("disputed.txt");
/// The host-owned drafting catalog; `draft_rules` cases judge against it.
const CATALOG: &str = include_str!("../../protocol/capture-site/catalog.json");

/// Replaces `$fixture` references and `$build` strings.
fn resolve(v: &Json, fixtures: &Map<String, Json>) -> Json {
    match v {
        Json::Object(o) => {
            if let (1, Some(Json::String(name))) = (o.len(), o.get("$fixture")) {
                return fixtures
                    .get(name)
                    .unwrap_or_else(|| panic!("unknown fixture {name}"))
                    .clone();
            }
            if let (1, Some(Json::Array(parts))) = (o.len(), o.get("$build")) {
                return Json::String(build(parts));
            }
            Json::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), resolve(v, fixtures)))
                    .collect(),
            )
        }
        Json::Array(a) => Json::Array(a.iter().map(|v| resolve(v, fixtures)).collect()),
        other => other.clone(),
    }
}

fn build(parts: &[Json]) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            Json::String(s) => out.push_str(s),
            Json::Object(o) if o.contains_key("repeat") => {
                let s = o["repeat"].as_str().expect("repeat string");
                let n = o["times"].as_u64().expect("times") as usize;
                out.push_str(&s.repeat(n));
            }
            Json::Object(o) if o.contains_key("utf16") => {
                let units: Vec<u16> = o["utf16"]
                    .as_array()
                    .expect("utf16 array")
                    .iter()
                    .map(|u| u.as_u64().expect("code unit") as u16)
                    .collect();
                out.push_str(&String::from_utf16_lossy(&units));
            }
            other => panic!("bad builder part {other}"),
        }
    }
    out
}

fn document(v: &Json) -> Result<Document, adapter_templates::ValidationError> {
    let value: Value = serde_json::from_value(v.clone()).expect("document deserializes");
    Document::from_value(&value)
}

fn validation_json(e: &adapter_templates::ValidationError) -> Json {
    json!({"code": e.code.as_str(), "adapter": e.adapter, "rule": e.rule, "entry": e.entry})
}

fn str_field<'a>(case: &'a Json, key: &str) -> &'a str {
    case[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is a string"))
}

fn pair(v: &Json) -> (usize, usize) {
    let a = v.as_array().expect("pair");
    (
        a[0].as_u64().unwrap() as usize,
        a[1].as_u64().unwrap() as usize,
    )
}

fn strings(v: &Json) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap().to_owned()).collect())
        .unwrap_or_default()
}

/// The catalog as the host parses it (`host/src/capture_site.rs`).
fn catalog_policy() -> DraftPolicy {
    let c: Json = serde_json::from_str(CATALOG).expect("catalog parses");
    let fill = |k: &str| str_field(&c["fill"], k).to_owned();
    DraftPolicy {
        probe_paths: strings(&c["probe_paths"]),
        search_keys: strings(&c["search_keys"]),
        paging_keys: strings(&c["paging_keys"]),
        blocked_route_words: strings(&c["blocked_route_words"]),
        secret_query_keys: strings(&c["secret_query_keys"]),
        secret_query_key_parts: strings(&c["secret_query_key_parts"]),
        secret_query_key_suffixes: strings(&c["secret_query_key_suffixes"]),
        mode_words: c["mode_words"]
            .as_object()
            .expect("mode_words")
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
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
                        t["segment"].as_u64().unwrap() as usize,
                        t["capture"].as_str().unwrap().to_owned(),
                    )
                }),
                account_index: e["account_index"]
                    .as_array()
                    .expect("account_index")
                    .iter()
                    .map(|a| {
                        (
                            a["literal"].as_str().unwrap().to_owned(),
                            a["segment"].as_u64().unwrap() as usize,
                        )
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn scope_json(scope: &Scope) -> Json {
    Json::Object(
        scope
            .iter()
            .map(|(k, vs)| (k.clone(), json!(vs)))
            .collect::<Map<String, Json>>(),
    )
}

fn run_draft_rules(case: &Json) -> Result<Json, String> {
    let existing = document(&case["existing"]).map_err(|e| format!("existing invalid: {e}"))?;
    let draft: Value = serde_json::from_value(case["draft"].clone()).map_err(|e| e.to_string())?;
    let rules = case["draft"]["match"].as_array().map_or(0, Vec::len);
    let provenance: Vec<Provenance> = match case.get("provenance") {
        None => vec![Provenance::Drafted; rules],
        Some(p) => strings(p)
            .iter()
            .map(|p| match p.as_str() {
                "drafted" => Ok(Provenance::Drafted),
                "user_written" => Ok(Provenance::UserWritten),
                other => Err(format!("bad provenance {other}")),
            })
            .collect::<Result<_, _>>()?,
    };
    let accepted = case
        .get("accepted")
        .and_then(Json::as_array)
        .map(|a| a.iter().map(document).collect::<Result<Vec<_>, _>>())
        .transpose()
        .map_err(|e| format!("accepted invalid: {e}"))?
        .unwrap_or_default();
    let mut policy = catalog_policy();
    if let Some(extra) = case.get("extra_probe_paths") {
        policy.probe_paths.extend(strings(extra));
    }
    let report = match check_draft_rules(&existing, &draft, &provenance, &policy, &accepted) {
        Ok(report) => report,
        Err(DraftOutcome::DraftError(code)) => return Ok(json!({"draft_error": code.as_str()})),
        Err(DraftOutcome::Invalid(e)) => return Ok(json!({"error": validation_json(&e)})),
        Err(DraftOutcome::Ok { .. }) => return Err("refusal carried Ok".into()),
    };
    let scope = match &report.scope {
        ScopeState::NotShared => json!("not_shared"),
        ScopeState::UnscopedOnly => json!("unscoped_only"),
        ScopeState::Pending { capture } => json!({"pending": capture}),
        ScopeState::Copied(s) => json!({"copied": scope_json(s)}),
    };
    let verdicts: Vec<Json> = report
        .rules
        .iter()
        .map(|v| match v {
            RuleVerdict::Kept => json!("kept"),
            RuleVerdict::Dropped(c) => json!({"dropped": c.as_str()}),
            RuleVerdict::Warned(c) => json!({"warned": c.as_str()}),
        })
        .collect();
    let survivors = report
        .rules
        .iter()
        .filter(|v| !matches!(v, RuleVerdict::Dropped(_)))
        .count();
    let in_document = report
        .document
        .as_ref()
        .map_or(0, |d| d.adapters()[0].rule_count());
    if survivors != in_document {
        return Err(format!(
            "{survivors} surviving rules but {in_document} in the document"
        ));
    }
    let mut out = json!({"scope": scope, "rules": verdicts});
    if report.scope_anchored {
        out.as_object_mut()
            .unwrap()
            .insert("scope_anchored".into(), json!(true));
    }
    if let Some(choice) = case.get("choice") {
        let choice = match choice {
            Json::Null => ScopeChoice::None,
            Json::String(s) if s == "unscoped" => ScopeChoice::Unscoped,
            Json::Object(o) if o.len() == 1 && o.contains_key("values") => {
                ScopeChoice::Values(strings(&o["values"]))
            }
            other => return Err(format!("bad choice {other}")),
        };
        let resolved =
            match resolve_scope_choice(&report.scope, &choice, &policy, report.scope_anchored) {
                Ok(ResolvedScope::NotShared) => json!("not_shared"),
                Ok(ResolvedScope::Unscoped) => json!("unscoped"),
                Ok(ResolvedScope::Scoped(s)) => json!({"scoped": scope_json(&s)}),
                Err(code) => json!({"scope_error": code.as_str()}),
            };
        out.as_object_mut()
            .unwrap()
            .insert("resolved".into(), resolved);
    }
    Ok(out)
}

/// The README invariants on every match.
fn check_invariants(m: &Match) -> Result<(), String> {
    for (name, v) in &m.captures {
        let bad = |c: char| {
            !c.is_ascii()
                || matches!(c, '/' | ':' | '{' | '}' | '%')
                || c.is_whitespace()
                || c.is_control()
        };
        if v.chars().any(bad) {
            return Err(format!("capture {name} = {v:?} breaks the value alphabet"));
        }
    }
    if m.artifact_id.len() > 512 {
        return Err("artifact_id over 512 bytes".into());
    }
    let (scheme, rest) = m
        .artifact_id
        .split_once(':')
        .ok_or("artifact_id has no scheme")?;
    if scheme.is_empty() || rest.split('/').any(str::is_empty) {
        return Err(format!(
            "artifact_id {} does not split into parts",
            m.artifact_id
        ));
    }
    Ok(())
}

fn run_evaluate(case: &Json, doc: &Document) -> Result<Json, String> {
    let url = str_field(case, "url");
    let title = case.get("title").map(|t| t.as_str().expect("title string"));
    let start = Instant::now();
    let result = doc.evaluate(url, title);
    let elapsed = start.elapsed();
    if let Some(budget) = case.get("budget_ms").and_then(Json::as_u64)
        && elapsed.as_millis() > u128::from(budget)
    {
        return Err(format!("took {elapsed:?}, budget {budget} ms"));
    }
    Ok(match result {
        Err(reason) => json!({"match": false, "reason": reason.as_str()}),
        Ok(m) => {
            check_invariants(&m)?;
            let captures: Map<String, Json> = m
                .captures
                .iter()
                .map(|(k, v)| (k.clone(), Json::String(v.clone())))
                .collect();
            let mut out = json!({
                "match": true,
                "adapter": m.adapter,
                "rule": m.rule,
                "type": m.artifact_type,
                "captures": captures,
                "artifact_id": m.artifact_id,
                "mode": m.mode.map(|x| x.as_str()),
            });
            if let Some(w) = &m.within {
                out.as_object_mut().unwrap().insert(
                    "within".into(),
                    json!({"type": w.artifact_type, "artifact_id": w.artifact_id}),
                );
            }
            if let Some(t) = &m.title {
                let refs: Vec<Json> = t
                    .refs
                    .iter()
                    .map(|r| json!({"type": r.artifact_type, "value": r.value}))
                    .collect();
                let o = out.as_object_mut().unwrap();
                o.insert("title".into(), json!(t.title));
                o.insert("title_entry".into(), json!(t.entry));
                o.insert("alias_id".into(), json!(t.alias_id));
                o.insert("refs".into(), Json::Array(refs));
            }
            out
        }
    })
}

fn run_case(case: &Json) -> Result<Json, String> {
    let kind = str_field(case, "kind");
    let doc = || document(&case["doc"]).map_err(|e| format!("doc invalid: {e}"));
    Ok(match kind {
        "validate" => match document(&case["doc"]) {
            Ok(_) => json!({"ok": true}),
            Err(e) => json!({"error": validation_json(&e)}),
        },
        "evaluate" => return run_evaluate(case, &doc()?),
        "capture" => {
            let ctype =
                CType::parse(str_field(case, "ctype")).map_err(|e| format!("ctype: {e}"))?;
            json!({"canonical": ctype.canonical(str_field(case, "value"))})
        }
        "segments" => json!({"segments": prepare_segments(str_field(case, "path"))}),
        "refs" => {
            let find = BaseType::from_name(str_field(case, "find")).ok_or("bad find")?;
            json!({"values": scan_refs(find, str_field(case, "text"))})
        }
        "refs_resolve" => {
            let refs: Vec<Ref> = case["refs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| Ref {
                    artifact_type: str_field(r, "type").to_owned(),
                    value: str_field(r, "value").to_owned(),
                })
                .collect();
            let self_id = case.get("self_id").and_then(Json::as_str);
            let resolved: Vec<Json> = doc()?
                .resolve_refs(&refs, self_id)
                .into_iter()
                .map(|r| json!({"type": r.artifact_type, "value": r.value, "artifact_id": r.artifact_id}))
                .collect();
            json!({"resolved": resolved})
        }
        "meta_alias" => {
            json!({"alias_id": doc()?.meta_alias(str_field(case, "url"), str_field(case, "content"))})
        }
        "overlap" => {
            let o = doc()?.may_overlap(pair(&case["a"]), pair(&case["b"]));
            json!({"overlap": if o { "may_overlap" } else { "disjoint" }})
        }
        "overlap_report" => {
            let pairs: Vec<Json> = doc()?
                .overlap_report()
                .into_iter()
                .map(|p| json!({"first": [p.first.0, p.first.1], "second": [p.second.0, p.second.1]}))
                .collect();
            json!({"pairs": pairs})
        }
        "draft" => {
            let existing =
                document(&case["existing"]).map_err(|e| format!("existing invalid: {e}"))?;
            let draft: Value =
                serde_json::from_value(case["draft"].clone()).map_err(|e| e.to_string())?;
            let strings = |key: &str| -> Vec<String> {
                case[key]
                    .as_array()
                    .map(|a| a.iter().map(|s| s.as_str().unwrap().to_owned()).collect())
                    .unwrap_or_default()
            };
            let (probes, keys) = (strings("probes"), strings("search_keys"));
            let probes: Vec<&str> = probes.iter().map(String::as_str).collect();
            let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
            match check_draft(&existing, &draft, &probes, &keys) {
                DraftOutcome::Ok { scope, .. } => {
                    let scope = scope.map(|s| {
                        s.into_iter()
                            .map(|(k, vs)| (k, json!(vs)))
                            .collect::<Map<String, Json>>()
                    });
                    json!({"ok": true, "scope": scope})
                }
                DraftOutcome::DraftError(code) => json!({"draft_error": code.as_str()}),
                DraftOutcome::Invalid(e) => json!({"error": validation_json(&e)}),
            }
        }
        "draft_rules" => return run_draft_rules(case),
        other => return Err(format!("unknown kind {other}")),
    })
}

#[test]
fn every_rust_vector_passes() {
    let file: Json = serde_json::from_str(VECTORS).expect("vectors.json parses");
    assert_eq!(file["vectors_format"], 1);
    let fixtures = file["fixtures"].as_object().expect("fixtures");
    let disputed: HashSet<&str> = DISPUTED
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let (mut passed, mut skipped, mut failures) = (0, 0, Vec::new());
    for raw in file["cases"].as_array().expect("cases") {
        let case = resolve(raw, fixtures);
        let id = str_field(&case, "id").to_owned();
        let rust = case["impls"]
            .as_array()
            .is_some_and(|i| i.iter().any(|x| x == "rust"));
        if !rust {
            continue;
        }
        if case.get("pending").is_some() || disputed.contains(id.as_str()) {
            skipped += 1;
            continue;
        }
        match run_case(&case) {
            Ok(got) if got == case["expect"] => passed += 1,
            Ok(got) => failures.push(format!(
                "{id}\n  expected {}\n  got      {got}",
                case["expect"]
            )),
            Err(e) => failures.push(format!("{id}\n  error: {e}")),
        }
    }
    eprintln!(
        "vectors: {passed} passed, {skipped} skipped, {} failed",
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "{} vector failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // Keep the reason enum in sync with the README's list.
    assert_eq!(NoMatch::IdTooLong.as_str(), "id_too_long");
}

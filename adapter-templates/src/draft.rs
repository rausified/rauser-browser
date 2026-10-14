//! Tier 2 draft checks (§10.4), in the README order.
//!
//! A draft is one adapter table proposed by an agent. It is judged against
//! the existing document: it may not use the keys the agent cannot know, it
//! inherits scope on a shared origin, and it must not match the origin root,
//! a probe URL, or anything an existing adapter already recognizes.

use crate::ctype::CType;
use crate::document::{Document, QueryCond, SHARED_ORIGINS};
use crate::error::{DraftErrorCode, ValidationError};
use crate::eval::{Prepared, match_rule};
use crate::overlap::rules_may_overlap;
use crate::path::Segment;
use crate::value::Value;

const FORBIDDEN_KEYS: &[&str] = &["title", "scope", "unscoped", "alias", "fields", "path_case"];

pub type Scope = Vec<(String, Vec<String>)>;

#[derive(Clone, Debug)]
pub enum DraftOutcome {
    /// The draft is acceptable. `scope` is the copied scope, if any, and
    /// `document` is the validated one-adapter document, for the preview.
    Ok {
        scope: Option<Scope>,
        document: Document,
    },
    DraftError(DraftErrorCode),
    /// The draft (with any copied scope) failed validation as a one-adapter
    /// document, so `adapter` is 0 (SQ-V10).
    Invalid(ValidationError),
}

/// Scopes compare as maps of sets (SQ-V5).
fn same_scope(a: &Scope, b: &Scope) -> bool {
    let covers = |x: &Scope, y: &Scope| {
        x.iter().all(|(k, vs)| {
            y.iter().find(|(k2, _)| k2 == k).is_some_and(|(_, ws)| {
                vs.iter().all(|v| ws.contains(v)) && ws.iter().all(|w| vs.contains(w))
            })
        })
    };
    covers(a, b) && covers(b, a)
}

fn scope_value(scope: &Scope) -> Value {
    Value::Table(
        scope
            .iter()
            .map(|(k, vs)| {
                let list = vs.iter().map(|v| Value::String(v.clone())).collect();
                (k.clone(), Value::Array(list))
            })
            .collect(),
    )
}

/// Checks a draft adapter against `existing`. `probes` are URLs the draft
/// must not match; the origin root is always probed. `search_keys` are query
/// keys whose values are search input and so cannot identify an artifact.
pub fn check_draft(
    existing: &Document,
    draft: &Value,
    probes: &[&str],
    search_keys: &[&str],
) -> DraftOutcome {
    use DraftOutcome::{DraftError, Invalid};

    // 1. Keys the agent cannot know.
    if let Some(table) = draft.as_table()
        && table
            .iter()
            .any(|(k, _)| FORBIDDEN_KEYS.contains(&k.as_str()))
    {
        return DraftError(DraftErrorCode::ForbiddenKey);
    }

    // 2. Scope source on a shared origin. Only scoped adapters count (SQ-V17).
    let origin = draft.get("origin").and_then(Value::as_str);
    let mut scope = None;
    if let Some(origin) = origin.filter(|o| SHARED_ORIGINS.contains(o)) {
        let mut sources = existing
            .adapters
            .iter()
            .filter(|a| a.origin == origin)
            .filter_map(|a| a.scope.as_ref());
        let Some(first) = sources.next() else {
            return DraftError(DraftErrorCode::NoScopeSource);
        };
        if sources.any(|s| !same_scope(first, s)) {
            return DraftError(DraftErrorCode::ScopeConflict);
        }
        scope = Some(first.clone());
    }

    // 3. Validation as a one-adapter document with the copied scope.
    let mut adapter = draft.clone();
    if let (Some(s), Value::Table(t)) = (&scope, &mut adapter) {
        t.push(("scope".to_owned(), scope_value(s)));
    }
    let doc_value = Value::Table(vec![
        ("format".to_owned(), Value::Int(1)),
        ("artifacts".to_owned(), Value::Array(vec![adapter])),
    ]);
    let doc = match Document::from_value(&doc_value) {
        Ok(d) => d,
        Err(e) => return Invalid(e),
    };
    let a = &doc.adapters[0];

    // 4. Per rule: a literal before the first capture, then `{*}` not second.
    // Neither `{_}` nor a plain alternation is a capture or a literal (SQ28).
    for rule in &a.rules {
        let segs = &rule.path.segments;
        let first_capture = segs
            .iter()
            .position(|s| matches!(s, Segment::Capture { .. }))
            .unwrap_or(segs.len());
        if !segs[..first_capture]
            .iter()
            .any(|s| matches!(s, Segment::Literal(_)))
        {
            return DraftError(DraftErrorCode::NoLeadingLiteral);
        }
        if segs.get(1) == Some(&Segment::Rest) {
            return DraftError(DraftErrorCode::RestSecond);
        }
    }

    // 5. The ID uses at least one typed capture.
    if !a
        .id
        .names()
        .any(|n| matches!(a.id_ctype(n), Some(CType::Typed { .. })))
    {
        return DraftError(DraftErrorCode::IdNeedsTypedCapture);
    }

    // 6. No ID capture comes from a search box.
    let from_search = a.rules.iter().any(|rule| {
        rule.query.iter().any(|(key, cond)| {
            matches!(cond, QueryCond::Capture { name, .. } if a.id.names().any(|n| n == name))
                && search_keys.contains(&key.as_str())
        })
    });
    if from_search {
        return DraftError(DraftErrorCode::SearchCapture);
    }

    // 7. Probes, matched by rule without scope (SQ-V3).
    let root = format!("{}/", a.origin);
    for probe in std::iter::once(root.as_str()).chain(probes.iter().copied()) {
        let Ok(p) = Prepared::new(probe) else {
            continue;
        };
        let Some(segments) = p.segments.as_deref().filter(|_| p.origin == a.origin) else {
            continue;
        };
        if a.rules
            .iter()
            .any(|r| match_rule(a, r, segments, &p.query).is_some())
        {
            return DraftError(DraftErrorCode::MatchesProbe);
        }
    }

    // 8. Overlap with any existing rule on the origin.
    for rule in &a.rules {
        for b in &existing.adapters {
            if b.rules.iter().any(|rb| rules_may_overlap(a, rule, b, rb)) {
                return DraftError(DraftErrorCode::Overlap);
            }
        }
    }

    DraftOutcome::Ok {
        scope,
        document: doc,
    }
}

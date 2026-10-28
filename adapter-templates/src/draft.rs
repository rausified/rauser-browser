//! Tier 2 draft checks (§10.4), in the README order.
//!
//! A draft is one adapter table proposed by a drafter. It is judged against
//! the existing document: it may not use the keys a drafter cannot know, it
//! inherits scope on a shared origin, and it must not match the origin root,
//! a probe URL, or anything an existing adapter already recognizes.
//!
//! [`check_draft`] refuses the whole draft at the first failure.
//! [`check_draft_rules`] judges each rule on its own (DESIGN §5.1): a failing
//! drafted rule is dropped, a failing user-written rule only warns, and a
//! shared origin with no agreeing scope leaves the scope pending for the
//! review to choose with [`resolve_scope_choice`].

use crate::ctype::{BaseType, CType};
use crate::document::{Adapter, Document, MAX_SCOPE_VALUES, QueryCond, Rule, SHARED_ORIGINS};
use crate::error::{DraftErrorCode, ValidationError};
use crate::eval::{Prepared, match_rule};
use crate::overlap::rules_may_overlap;
use crate::path::{Segment, is_scope_anchor};
use crate::policy::DraftPolicy;
use crate::value::Value;

const FORBIDDEN_KEYS: &[&str] = &["title", "scope", "unscoped", "alias", "fields", "path_case"];

/// The `unscoped` value of the stand-in where scope can only be unscoped.
const UNSCOPED: &str = "acknowledged";
/// The tenant value of the stand-in scope while scope is pending. It is a
/// canonical `name|lower` value, and it never reaches a saved document: the
/// saved scope comes from [`resolve_scope_choice`].
const PENDING_STAND_IN: &str = "example";

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

/// Who wrote a rule. Drafted rules fail closed; a rule the user wrote by
/// hand is trusted config (DESIGN §4.2), so its failures only warn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    Drafted,
    UserWritten,
}

/// The scope a draft on its origin gets (DESIGN §5.1, adapter-templates §6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeState {
    /// Not a shared origin: no scope applies.
    NotShared,
    /// Every scoped adapter on the shared origin agrees; the draft takes
    /// that scope and the review cannot change it.
    Copied(Scope),
    /// A shared origin with a tenant position but no agreeing scope: the
    /// review must choose values for `capture`, or unscoped.
    Pending { capture: String },
    /// A shared origin whose URLs name no tenant: only unscoped is offered.
    UnscopedOnly,
}

/// One rule's result, in rule order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleVerdict {
    Kept,
    /// A drafted rule failed this check and is not saved.
    Dropped(DraftErrorCode),
    /// A user-written rule failed this check; it is kept and the review
    /// shows the warning.
    Warned(DraftErrorCode),
}

/// The per-rule result of [`check_draft_rules`].
#[derive(Clone, Debug)]
pub struct DraftRulesReport {
    pub scope: ScopeState,
    /// One verdict per rule of the draft's `match`, in order.
    pub rules: Vec<RuleVerdict>,
    /// The surviving (kept and warned) rules validated as a one-adapter
    /// document, with the copied scope. While scope is pending it carries a
    /// stand-in scope of the tenant capture, so a rule that cannot be scoped
    /// by the tenant is caught here, and where scope can only be unscoped it
    /// carries `unscoped`. A preview should match its rules without scope, as
    /// the checks do; the saved scope comes from [`resolve_scope_choice`].
    /// `None` when no rule survives.
    pub document: Option<Document>,
    /// Some surviving rule is anchored only by the scope's tenant capture
    /// (step 4): it needs that scope, so [`resolve_scope_choice`] refuses
    /// `unscoped` for it.
    pub scope_anchored: bool,
}

/// The review's scope choice for a shared origin. It travels next to the
/// checked table, never inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeChoice {
    /// Scope the tenant capture to these values.
    Values(Vec<String>),
    Unscoped,
    /// No choice: right only where the scope is not the review's to make.
    None,
}

/// The scope a saved draft carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedScope {
    NotShared,
    Scoped(Scope),
    Unscoped,
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

/// §10.4: an identifier capture is `int`, `hex`, `uuid`, `jira_key`, or
/// `token`. A `name` capture (a host-supplied tenant or namespace) is not an
/// identifier, and neither is a capture of an alternation.
fn is_identifier_capture(s: &Segment) -> bool {
    matches!(
        s,
        Segment::Capture {
            ctype: CType::Typed { base, .. },
            ..
        } if *base != BaseType::Name
    )
}

/// Step 1: a key a drafter cannot know.
fn has_forbidden_key(draft: &Value) -> bool {
    draft
        .as_table()
        .is_some_and(|t| t.iter().any(|(k, _)| FORBIDDEN_KEYS.contains(&k.as_str())))
}

/// Step 2: the scope every scoped adapter on a shared origin agrees on.
/// Only scoped adapters count (SQ-V17).
fn scope_source(existing: &Document, origin: &str) -> Result<Scope, DraftErrorCode> {
    let mut sources = existing
        .adapters
        .iter()
        .filter(|a| a.origin == origin)
        .filter_map(|a| a.scope.as_ref());
    let Some(first) = sources.next() else {
        return Err(DraftErrorCode::NoScopeSource);
    };
    if sources.any(|s| !same_scope(first, s)) {
        return Err(DraftErrorCode::ScopeConflict);
    }
    Ok(first.clone())
}

/// Step 3: validates an adapter table as a one-adapter document.
fn one_adapter_document(adapter: Value) -> Result<Document, ValidationError> {
    Document::from_value(&Value::Table(vec![
        ("format".to_owned(), Value::Int(1)),
        ("artifacts".to_owned(), Value::Array(vec![adapter])),
    ]))
}

/// The segments of `rule` before its first identifier capture.
fn before_first_identifier(rule: &Rule) -> &[Segment] {
    let segs = &rule.path.segments;
    let first = segs
        .iter()
        .position(is_identifier_capture)
        .unwrap_or(segs.len());
    &segs[..first]
}

/// A required path capture that a `scope` key of `a` restricts to a list.
/// It anchors a template as a literal does (adapter-templates §4.1, §6:
/// the listed values pin it). An `unscoped` adapter has no such capture.
fn scope_restricted(a: &Adapter, s: &Segment) -> bool {
    is_scope_anchor(s, |name| {
        a.scope
            .as_ref()
            .is_some_and(|scope| scope.iter().any(|(key, _)| key == name))
    })
}

/// Step 4: an anchor before the first identifier capture, then `{*}` not
/// second (SQ-V18). The anchor is a literal, or a required capture a scope
/// key restricts (on a shared origin, the scoped tenant): the same rule
/// that lets a hand-written template anchor on a scoped capture (§4.1), so
/// the drafter keeps no tenant rule of its own. A draft is still stricter
/// than §4.1 where its inputs are page-controlled: neither `{_}`, a plain
/// alternation (SQ28) nor a fixed-shape capture anchors it, and a `name`
/// capture or a captured alternation is a capture but not an identifier.
fn rule_structure(a: &Adapter, rule: &Rule) -> Option<DraftErrorCode> {
    if !before_first_identifier(rule)
        .iter()
        .any(|s| matches!(s, Segment::Literal(_)) || scope_restricted(a, s))
    {
        return Some(DraftErrorCode::NoLeadingLiteral);
    }
    if rule.path.segments.get(1) == Some(&Segment::Rest) {
        return Some(DraftErrorCode::RestSecond);
    }
    None
}

/// Whether `rule`'s only anchor before its first identifier is the scoped
/// capture: without that scope it would fail step 4, so it cannot be saved
/// `unscoped` (see [`DraftRulesReport::scope_anchored`]).
fn anchored_by_scope(rule: &Rule) -> bool {
    !before_first_identifier(rule)
        .iter()
        .any(|s| matches!(s, Segment::Literal(_)))
}

/// Step 5: the ID uses at least one identifier capture; a `name` capture
/// alone does not count (§10.4).
fn id_uses_identifier(a: &Adapter) -> bool {
    a.id.names().any(
        |n| matches!(a.id_ctype(n), Some(CType::Typed { base, .. }) if *base != BaseType::Name),
    )
}

/// Whether a capture the ID uses is bound from one of `keys`.
fn id_capture_from<S: AsRef<str>>(a: &Adapter, rule: &Rule, keys: &[S]) -> bool {
    id_capture_where(a, rule, |key| keys.iter().any(|k| k.as_ref() == key))
}

/// Whether a capture the ID uses is bound from a query key `listed` accepts.
fn id_capture_where(a: &Adapter, rule: &Rule, listed: impl Fn(&str) -> bool) -> bool {
    rule.query.iter().any(|(key, cond)| {
        matches!(cond, QueryCond::Capture { name, .. } if a.id.names().any(|n| n == name))
            && listed(key)
    })
}

/// Whether the rule matches the origin root or a probe URL, by rule and
/// without scope (SQ-V3).
fn rule_matches_probe<'p>(
    a: &Adapter,
    rule: &Rule,
    probes: impl IntoIterator<Item = &'p str>,
) -> bool {
    let matches = |probe: &str| {
        let Ok(p) = Prepared::new(probe) else {
            return false;
        };
        p.segments
            .as_deref()
            .filter(|_| p.origin == a.origin)
            .is_some_and(|segments| match_rule(a, rule, segments, &p.query).is_some())
    };
    matches(&format!("{}/", a.origin)) || probes.into_iter().any(matches)
}

/// Whether the rule may overlap any rule of `doc` on its origin.
fn rule_overlaps(a: &Adapter, rule: &Rule, doc: &Document) -> bool {
    doc.adapters
        .iter()
        .any(|b| b.rules.iter().any(|rb| rules_may_overlap(a, rule, b, rb)))
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
    if has_forbidden_key(draft) {
        return DraftError(DraftErrorCode::ForbiddenKey);
    }

    // 2. Scope source on a shared origin.
    let origin = draft.get("origin").and_then(Value::as_str);
    let mut scope = None;
    if let Some(origin) = origin.filter(|o| SHARED_ORIGINS.contains(o)) {
        match scope_source(existing, origin) {
            Ok(s) => scope = Some(s),
            Err(code) => return DraftError(code),
        }
    }

    // 3. Validation as a one-adapter document with the copied scope.
    let mut adapter = draft.clone();
    if let (Some(s), Value::Table(t)) = (&scope, &mut adapter) {
        t.push(("scope".to_owned(), scope_value(s)));
    }
    let doc = match one_adapter_document(adapter) {
        Ok(d) => d,
        Err(e) => return Invalid(e),
    };
    let a = &doc.adapters[0];

    // 4. Per rule: leading literal, then rest position.
    if let Some(code) = a.rules.iter().find_map(|r| rule_structure(a, r)) {
        return DraftError(code);
    }

    // 5. The ID uses an identifier capture.
    if !id_uses_identifier(a) {
        return DraftError(DraftErrorCode::IdNeedsTypedCapture);
    }

    // 6. No ID capture comes from a search box.
    if a.rules.iter().any(|r| id_capture_from(a, r, search_keys)) {
        return DraftError(DraftErrorCode::SearchCapture);
    }

    // 7. Probes, matched by rule without scope (SQ-V3).
    if a.rules
        .iter()
        .any(|r| rule_matches_probe(a, r, probes.iter().copied()))
    {
        return DraftError(DraftErrorCode::MatchesProbe);
    }

    // 8. Overlap with any existing rule on the origin.
    if a.rules.iter().any(|r| rule_overlaps(a, r, existing)) {
        return DraftError(DraftErrorCode::Overlap);
    }

    DraftOutcome::Ok {
        scope,
        document: doc,
    }
}

/// The scope state of a draft on `origin` (step 2 of [`check_draft_rules`]).
/// A shared origin whose catalog entry has no tenant position offers only
/// unscoped, whatever its existing adapters are scoped by: no drafted rule
/// binds a tenant there. Agreeing adapters' scope is copied only when it
/// restricts the tenant capture alone; otherwise the scope is pending.
pub fn scope_state(existing: &Document, origin: Option<&str>, policy: &DraftPolicy) -> ScopeState {
    let Some(origin) = origin.filter(|o| SHARED_ORIGINS.contains(o)) else {
        return ScopeState::NotShared;
    };
    let Some((_, capture)) = policy.shared_origin(origin).and_then(|s| s.tenant.as_ref()) else {
        return ScopeState::UnscopedOnly;
    };
    // A copied scope must restrict only the tenant capture drafted rules
    // bind; one keyed otherwise (`scope.org` where rules bind `{owner}`)
    // would leave every rule `scope_capture_not_bound`.
    match scope_source(existing, origin) {
        Ok(scope) if scope.iter().all(|(key, _)| key == capture) => ScopeState::Copied(scope),
        _ => ScopeState::Pending {
            capture: capture.clone(),
        },
    }
}

/// The draft table with `match` replaced by `rules` and the scope applied:
/// the copied scope, or a stand-in while the choice is the review's (see
/// [`DraftRulesReport::document`]). A table that already carries `scope` or `unscoped` (a
/// user-written adapter) keeps its own.
fn with_rules(draft: &Value, rules: Vec<Value>, scope: &ScopeState) -> Value {
    let Value::Table(table) = draft else {
        return draft.clone();
    };
    let mut table: Vec<(String, Value)> = table
        .iter()
        .map(|(k, v)| match k.as_str() {
            "match" => (k.clone(), Value::Array(rules.clone())),
            _ => (k.clone(), v.clone()),
        })
        .collect();
    let has_own = table.iter().any(|(k, _)| k == "scope" || k == "unscoped");
    match scope {
        _ if has_own => {}
        ScopeState::NotShared => {}
        ScopeState::Copied(s) => table.push(("scope".to_owned(), scope_value(s))),
        ScopeState::Pending { capture } => {
            let stand_in = vec![(capture.clone(), vec![PENDING_STAND_IN.to_owned()])];
            table.push(("scope".to_owned(), scope_value(&stand_in)))
        }
        ScopeState::UnscopedOnly => {
            table.push(("unscoped".to_owned(), Value::String(UNSCOPED.to_owned())))
        }
    }
    Value::Table(table)
}

/// Whether a path literal or alternative is a blocked route word, as
/// [`DraftPolicy::is_blocked_route_word`] judges it: ASCII case is folded,
/// so an insensitive adapter cannot slip one through, and `sign-in` or
/// `reset-password` are caught by their parts.
fn blocked_route(rule: &Rule, policy: &DraftPolicy) -> bool {
    let blocked = |word: &str| policy.is_blocked_route_word(word);
    rule.path.segments.iter().any(|s| match s {
        Segment::Literal(l) => blocked(l),
        Segment::Alternation(alts)
        | Segment::Capture {
            ctype: CType::Alternation(alts),
            ..
        } => alts.iter().any(|w| blocked(w)),
        _ => false,
    })
}

/// Step 4 of [`check_draft_rules`] for one validated rule: the first check
/// it fails.
fn rule_check(
    a: &Adapter,
    rule: &Rule,
    policy: &DraftPolicy,
    existing: &Document,
    accepted: &[Document],
) -> Option<DraftErrorCode> {
    if let Some(code) = rule_structure(a, rule) {
        return Some(code);
    }
    if blocked_route(rule, policy) {
        return Some(DraftErrorCode::BlockedRoute);
    }
    if id_capture_from(a, rule, &policy.search_keys) {
        return Some(DraftErrorCode::SearchCapture);
    }
    if id_capture_from(a, rule, &policy.paging_keys) {
        return Some(DraftErrorCode::PagingCapture);
    }
    if id_capture_where(a, rule, |key| policy.is_secret_query_key(key)) {
        return Some(DraftErrorCode::SecretCapture);
    }
    let probes: Vec<String> = policy
        .probe_paths
        .iter()
        .map(|path| format!("{}{path}", a.origin))
        .collect();
    if rule_matches_probe(a, rule, probes.iter().map(String::as_str)) {
        return Some(DraftErrorCode::MatchesProbe);
    }
    if rule_overlaps(a, rule, existing) || accepted.iter().any(|d| rule_overlaps(a, rule, d)) {
        return Some(DraftErrorCode::Overlap);
    }
    None
}

/// Judges each rule of a drafted adapter table (DESIGN §5.1).
///
/// `draft` is the checked table: only the Tier 0 keys, before the review's
/// scope, unscoped and title overlays, which this function never sees.
/// `provenance` names who wrote each rule; a rule without an entry counts as
/// drafted. `accepted` holds the earlier-ranked drafted adapters, so a later
/// rule that may overlap one is dropped. A caller also puts the origin's
/// recorded note-only contracts (retired and hand-removed adapters) there:
/// they freeze their routes' IDs, so a drafted rule that may overlap one
/// could never be saved and is dropped as `draft_overlap` too.
///
/// The checks, in order:
///
/// 1. A forbidden key refuses the whole draft when any rule is drafted. When
///    every rule is user-written, each rule is warned instead.
/// 2. The scope state (see [`ScopeState`]).
/// 3. Each rule is validated as a one-rule adapter. A rule that fails is
///    dropped as `draft_invalid_rule` whoever wrote it, since it can never
///    be saved; an adapter-level failure refuses the whole draft.
/// 4. Per rule: leading literal, rest position, blocked route words, ID
///    captures from search, paging or secret keys, the root and probe URLs
///    (matched without scope, SQ-V3), then overlap with `existing` and
///    `accepted`.
///    The surviving rules are then validated together in order, and one that
///    conflicts with an earlier survivor is dropped as `draft_invalid_rule`.
/// 5. The ID must use an identifier capture. This refuses the whole draft
///    when any rule is drafted, and warns every survivor otherwise.
///
/// A user-written rule's failure in steps 1, 4 and 5 is a warning and the
/// rule is kept. The first failing check decides each verdict.
pub fn check_draft_rules(
    existing: &Document,
    draft: &Value,
    provenance: &[Provenance],
    policy: &DraftPolicy,
    accepted: &[Document],
) -> Result<DraftRulesReport, DraftOutcome> {
    use DraftOutcome::{DraftError, Invalid};

    let rule_values: &[Value] = draft
        .get("match")
        .and_then(Value::as_array)
        .unwrap_or_default();
    let written = |i: usize| provenance.get(i).copied().unwrap_or(Provenance::Drafted);
    let all_user_written = !rule_values.is_empty()
        && (0..rule_values.len()).all(|i| written(i) == Provenance::UserWritten);
    let mut rules = vec![RuleVerdict::Kept; rule_values.len()];
    let fail = |rules: &mut [RuleVerdict], i: usize, code: DraftErrorCode| {
        if rules[i] == RuleVerdict::Kept {
            rules[i] = match written(i) {
                Provenance::Drafted => RuleVerdict::Dropped(code),
                Provenance::UserWritten => RuleVerdict::Warned(code),
            };
        }
    };

    // 1. Forbidden keys.
    if has_forbidden_key(draft) {
        if !all_user_written {
            return Err(DraftError(DraftErrorCode::ForbiddenKey));
        }
        for i in 0..rules.len() {
            fail(&mut rules, i, DraftErrorCode::ForbiddenKey);
        }
    }

    // 2. Scope state.
    let scope = scope_state(
        existing,
        draft.get("origin").and_then(Value::as_str),
        policy,
    );

    // 3. Each rule as a one-rule adapter. With no rule list to split, the
    // whole table is validated so its error is reported.
    if rule_values.is_empty() {
        one_adapter_document(with_rules(draft, Vec::new(), &scope)).map_err(Invalid)?;
    }
    let mut alone = Vec::with_capacity(rule_values.len());
    for (i, rule) in rule_values.iter().enumerate() {
        match one_adapter_document(with_rules(draft, vec![rule.clone()], &scope)) {
            Ok(doc) => alone.push(Some(doc)),
            Err(e) if e.rule.is_some() => {
                rules[i] = RuleVerdict::Dropped(DraftErrorCode::InvalidRule);
                alone.push(None);
            }
            Err(e) => return Err(Invalid(e)),
        }
    }

    // 4. Per-rule checks, then the survivors together.
    let mut by_scope = vec![false; rule_values.len()];
    for (i, doc) in alone.iter().enumerate() {
        let Some(doc) = doc else { continue };
        let a = &doc.adapters[0];
        let rule = &a.rules[0];
        by_scope[i] = rule_structure(a, rule).is_none() && anchored_by_scope(rule);
        if let Some(code) = rule_check(a, rule, policy, existing, accepted) {
            fail(&mut rules, i, code);
        }
    }
    let mut survivors: Vec<Value> = Vec::new();
    let mut document = None;
    for (i, rule) in rule_values.iter().enumerate() {
        if matches!(rules[i], RuleVerdict::Dropped(_)) {
            continue;
        }
        let mut next = survivors.clone();
        next.push(rule.clone());
        match one_adapter_document(with_rules(draft, next.clone(), &scope)) {
            Ok(doc) => {
                survivors = next;
                document = Some(doc);
            }
            Err(_) => rules[i] = RuleVerdict::Dropped(DraftErrorCode::InvalidRule),
        }
    }

    // 5. The ID uses an identifier capture.
    if let Some(doc) = &document
        && !id_uses_identifier(&doc.adapters[0])
    {
        if !all_user_written {
            return Err(DraftError(DraftErrorCode::IdNeedsTypedCapture));
        }
        for i in 0..rules.len() {
            fail(&mut rules, i, DraftErrorCode::IdNeedsTypedCapture);
        }
    }

    let scope_anchored = rules
        .iter()
        .zip(&by_scope)
        .any(|(verdict, by_scope)| *by_scope && !matches!(verdict, RuleVerdict::Dropped(_)));
    Ok(DraftRulesReport {
        scope,
        rules,
        document,
        scope_anchored,
    })
}

/// Judges the review's scope choice against the scope state, after
/// [`check_draft_rules`]; it never changes a rule verdict. Pending scope
/// needs non-empty, canonical `name|lower` values (at most 64) that are not
/// blocked route words (a workspace named `auth` or `invite` is a sign-in
/// or capability route), or unscoped. Unscoped-only accepts only unscoped,
/// and copied or not-shared scope accepts no choice: a copied scope is the
/// host's copy. When `scope_anchored` (a kept rule is anchored only by the
/// scoped tenant, [`DraftRulesReport::scope_anchored`]), `unscoped` is
/// `draft_scope_choice` too: without the scope that rule has no anchor.
pub fn resolve_scope_choice(
    state: &ScopeState,
    choice: &ScopeChoice,
    policy: &DraftPolicy,
    scope_anchored: bool,
) -> Result<ResolvedScope, DraftErrorCode> {
    match (state, choice) {
        (_, ScopeChoice::Unscoped) if scope_anchored => Err(DraftErrorCode::ScopeChoice),
        (ScopeState::NotShared, ScopeChoice::None) => Ok(ResolvedScope::NotShared),
        (ScopeState::Copied(scope), ScopeChoice::None) => Ok(ResolvedScope::Scoped(scope.clone())),
        (ScopeState::Pending { .. } | ScopeState::UnscopedOnly, ScopeChoice::Unscoped) => {
            Ok(ResolvedScope::Unscoped)
        }
        (ScopeState::Pending { .. }, ScopeChoice::None) => Err(DraftErrorCode::ScopePending),
        (ScopeState::Pending { capture }, ScopeChoice::Values(values)) => {
            if values.is_empty() {
                return Err(DraftErrorCode::ScopePending);
            }
            let tenant = CType::Typed {
                base: BaseType::Name,
                bounds: BaseType::Name.default_bounds().unwrap_or((1, 100)),
                case: Some(crate::ctype::CaseMod::Lower),
                tail: false,
            };
            let mut distinct: Vec<String> = Vec::new();
            for value in values {
                if tenant.canonical_whole(value).as_deref() != Some(value.as_str())
                    || policy.is_blocked_route_word(value)
                {
                    return Err(DraftErrorCode::ScopeChoice);
                }
                if !distinct.contains(value) {
                    distinct.push(value.clone());
                }
            }
            if distinct.len() > MAX_SCOPE_VALUES {
                return Err(DraftErrorCode::ScopeChoice);
            }
            Ok(ResolvedScope::Scoped(vec![(capture.clone(), distinct)]))
        }
        _ => Err(DraftErrorCode::ScopeChoice),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::test_catalog as catalog;
    use serde_json::{Value as Json, json};

    const TOOLS: &str = "https://tools.example.com";

    fn value(v: Json) -> Value {
        serde_json::from_value(v).unwrap()
    }

    fn doc(v: Json) -> Document {
        Document::from_value(&value(v)).unwrap()
    }

    fn empty() -> Document {
        Document::default()
    }

    fn github_scoped(owner: &str) -> Json {
        json!({
            "type": "github.issue",
            "origin": "https://github.com",
            "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}"}],
            "scope": {"owner": [owner]},
            "id": "github.issue:{host}/{owner}/{repo}/{number}"
        })
    }

    fn github_project() -> Json {
        json!({
            "type": "github.project",
            "origin": "https://github.com",
            "match": [{"path": "/orgs/{owner:name|lower}/projects/{number:int}"}],
            "id": "github.project:{host}/{owner}/{number}"
        })
    }

    fn tools(rules: Json) -> Json {
        json!({"type": "x.item", "origin": TOOLS, "match": rules, "id": "x:{host}/{id}"})
    }

    fn check(existing: &Document, draft: Json, provenance: &[Provenance]) -> DraftRulesReport {
        check_with(existing, draft, provenance, &catalog(), &[])
    }

    fn check_with(
        existing: &Document,
        draft: Json,
        provenance: &[Provenance],
        policy: &DraftPolicy,
        accepted: &[Document],
    ) -> DraftRulesReport {
        match check_draft_rules(existing, &value(draft), provenance, policy, accepted) {
            Ok(report) => report,
            Err(e) => panic!("whole draft refused: {e:?}"),
        }
    }

    fn refused(existing: &Document, draft: Json, provenance: &[Provenance]) -> DraftOutcome {
        match check_draft_rules(existing, &value(draft), provenance, &catalog(), &[]) {
            Ok(report) => panic!("draft accepted: {report:?}"),
            Err(e) => e,
        }
    }

    fn kept_rules(report: &DraftRulesReport) -> usize {
        report
            .document
            .as_ref()
            .map_or(0, |d| d.adapters[0].rules.len())
    }

    use DraftErrorCode as C;
    use Provenance::{Drafted as D, UserWritten as U};
    use RuleVerdict::{Dropped, Kept, Warned};

    #[test]
    fn draft_rules_drop_only_the_failing_rule() {
        let r = check(
            &empty(),
            tools(json!([{"path": "/items/{id:int}"}, {"path": "/(a|b)/{id:int}"}])),
            &[D, D],
        );
        assert_eq!(r.scope, ScopeState::NotShared);
        assert_eq!(r.rules, vec![Kept, Dropped(C::NoLeadingLiteral)]);
        assert_eq!(kept_rules(&r), 1);
    }

    #[test]
    fn draft_rules_pending_scope_on_shared_origin_without_scoped_adapters() {
        let r = check(&empty(), github_project(), &[D]);
        assert_eq!(
            r.scope,
            ScopeState::Pending {
                capture: "owner".into()
            }
        );
        assert_eq!(r.rules, vec![Kept]);
        // Unscoped adapters on the origin are not scope sources (SQ-V17).
        let unscoped = doc(json!({"format": 1, "artifacts": [{
            "type": "github.issue",
            "origin": "https://github.com",
            "unscoped": "acknowledged",
            "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}"}],
            "id": "github.issue:{host}/{owner}/{repo}/{number}"
        }]}));
        let r = check(&unscoped, github_project(), &[D]);
        assert!(matches!(r.scope, ScopeState::Pending { .. }));
        // A rule the tenant cannot scope is dropped while scope is pending.
        let gist = json!({
            "type": "github.gist",
            "origin": "https://github.com",
            "match": [{"path": "/gists/{id:hex}"}],
            "id": "gist:{host}/{id}"
        });
        let r = check(&empty(), gist, &[D]);
        assert_eq!(r.rules, vec![Dropped(C::InvalidRule)]);
        assert!(r.document.is_none());
    }

    #[test]
    fn draft_rules_unscoped_only_where_no_tenant_position() {
        let draft = json!({
            "type": "gdoc.sheet",
            "origin": "https://docs.google.com",
            "match": [{"path": "/spreadsheets/d/{id:token(20,64)}"}],
            "id": "gsheet:{host}/{id}"
        });
        let r = check(&empty(), draft, &[D]);
        assert_eq!(r.scope, ScopeState::UnscopedOnly);
        assert_eq!(r.rules, vec![Kept]);
    }

    #[test]
    fn draft_rules_copy_agreeing_scope() {
        let existing = doc(json!({"format": 1, "artifacts": [
            github_scoped("acme-corp"), github_scoped("acme-corp")
        ]}));
        let r = check(&existing, github_project(), &[D]);
        let scope = vec![("owner".to_owned(), vec!["acme-corp".to_owned()])];
        assert_eq!(r.scope, ScopeState::Copied(scope.clone()));
        assert_eq!(r.rules, vec![Kept]);
        let document = r.document.expect("survivors");
        assert_eq!(document.adapters[0].scope(), Some(scope.as_slice()));
    }

    #[test]
    fn draft_rules_conflicting_scopes_leave_scope_pending() {
        let existing = doc(json!({"format": 1, "artifacts": [
            github_scoped("acme-corp"), github_scoped("acme-labs")
        ]}));
        let r = check(&existing, github_project(), &[D]);
        assert_eq!(
            r.scope,
            ScopeState::Pending {
                capture: "owner".into()
            }
        );
        assert_eq!(r.rules, vec![Kept]);
    }

    #[test]
    fn draft_rules_leave_scope_pending_when_agreeing_scope_cannot_bind_the_tenant() {
        // Adapters agree on `scope.org`, but drafted rules bind the
        // catalog's `{owner}`: copying `org` would leave every rule
        // `scope_capture_not_bound`, so the choice is the review's.
        let org_scoped = json!({
            "type": "github.issue",
            "origin": "https://github.com",
            "scope": {"org": ["acme-corp"]},
            "match": [{"path": "/{org:name|lower}/{repo:name|lower}/issues/{number:int}"}],
            "id": "github.issue:{host}/{org}/{repo}/{number}"
        });
        let existing = doc(json!({"format": 1, "artifacts": [org_scoped]}));
        let r = check(&existing, github_project(), &[D]);
        assert_eq!(
            r.scope,
            ScopeState::Pending {
                capture: "owner".into()
            }
        );
        assert_eq!(r.rules, vec![Kept]);
    }

    #[test]
    fn draft_rules_drop_search_parameter_ids() {
        let r = check(
            &empty(),
            tools(json!([
                {"path": "/items/{id:int}"},
                {"path": "/items", "query": {"q": "{id:int}"}}
            ])),
            &[D, D],
        );
        assert_eq!(r.rules, vec![Kept, Dropped(C::SearchCapture)]);
    }

    #[test]
    fn draft_rules_drop_paging_parameter_ids() {
        let r = check(
            &empty(),
            tools(json!([
                {"path": "/items/{id:int}"},
                {"path": "/items", "query": {"page": "{id:int}"}}
            ])),
            &[D, D],
        );
        assert_eq!(r.rules, vec![Kept, Dropped(C::PagingCapture)]);
    }

    #[test]
    fn draft_rules_drop_blocked_route_words() {
        let r = check(
            &empty(),
            tools(json!([
                {"path": "/items/{id:int}"},
                {"path": "/share/{id:int}"},
                {"path": "/x/(view|login)/{id:int}"}
            ])),
            &[D, D, D],
        );
        assert_eq!(
            r.rules,
            vec![Kept, Dropped(C::BlockedRoute), Dropped(C::BlockedRoute)]
        );
    }

    #[test]
    fn draft_rules_drop_root_and_probe_matches() {
        let mut policy = catalog();
        policy.probe_paths.push("/reports/view?rid=5".into());
        let r = check_with(
            &empty(),
            tools(json!([
                {"path": "/reports/view", "query": {"rid": "{id:int}"}},
                {"path": "/items/{id:int}"}
            ])),
            &[D, D],
            &policy,
            &[],
        );
        assert_eq!(r.rules, vec![Dropped(C::MatchesProbe), Kept]);
    }

    #[test]
    fn draft_rules_drop_rest_after_first_segment() {
        let r = check(
            &empty(),
            tools(json!([
                {"path": "/items/{id:int}"},
                {"path": "/x/{*}", "query": {"item": "{id:int}"}}
            ])),
            &[D, D],
        );
        assert_eq!(r.rules, vec![Kept, Dropped(C::RestSecond)]);
    }

    fn jira_existing() -> Document {
        doc(json!({"format": 1, "artifacts": [{
            "type": "jira.issue",
            "origin": "https://acme.atlassian.net",
            "match": [
                {"path": "/browse/{key:jira_key}"},
                {"path": "/jira/{*}", "query": {"selectedIssue": "{key:jira_key}"}}
            ],
            "id": "jira:{host}/{key}"
        }]}))
    }

    #[test]
    fn draft_rules_overlap_drops_only_overlapping_rules() {
        let draft = json!({
            "type": "jira.filter",
            "origin": "https://acme.atlassian.net",
            "match": [{"path": "/jira/boards/{id:int}"}, {"path": "/reports/{id:int}"}],
            "id": "jf:{host}/{id}"
        });
        let r = check(&jira_existing(), draft, &[D, D]);
        assert_eq!(r.rules, vec![Dropped(C::Overlap), Kept]);
    }

    #[test]
    fn overlap_between_drafted_kinds_keeps_the_higher_ranked_rule() {
        let first = doc(json!({"format": 1, "artifacts": [
            tools(json!([{"path": "/items/{id:int}"}]))
        ]}));
        let draft = json!({
            "type": "x.other",
            "origin": TOOLS,
            "match": [{"path": "/items/{n:int}"}, {"path": "/other/{n:int}"}],
            "id": "xo:{host}/{n}"
        });
        let r = check_with(&empty(), draft.clone(), &[D, D], &catalog(), &[first]);
        assert_eq!(r.rules, vec![Dropped(C::Overlap), Kept]);
        let alone = check(&empty(), draft, &[D, D]);
        assert_eq!(alone.rules, vec![Kept, Kept]);
    }

    #[test]
    fn draft_rules_user_written_failures_only_warn() {
        let r = check(
            &empty(),
            tools(json!([
                {"path": "/share/{id:int}"},
                {"path": "/items", "query": {"q": "{id:int}"}},
                {"path": "/share/{id:int}/x"}
            ])),
            &[U, U, D],
        );
        assert_eq!(
            r.rules,
            vec![
                Warned(C::BlockedRoute),
                Warned(C::SearchCapture),
                Dropped(C::BlockedRoute)
            ]
        );
        assert_eq!(kept_rules(&r), 2);
    }

    #[test]
    fn draft_rules_forbidden_key_rejects_whole_draft_when_any_rule_is_drafted() {
        let mut draft = tools(json!([{"path": "/items/{id:int}"}, {"path": "/other/{id:int}"}]));
        draft["title"] = json!("{title} - Tools");
        assert!(matches!(
            refused(&empty(), draft.clone(), &[U, D]),
            DraftOutcome::DraftError(C::ForbiddenKey)
        ));
        // A missing provenance entry counts as drafted.
        assert!(matches!(
            refused(&empty(), draft, &[U]),
            DraftOutcome::DraftError(C::ForbiddenKey)
        ));
    }

    #[test]
    fn draft_rules_all_user_written_adapter_forbidden_key_only_warns() {
        let mut draft = tools(json!([{"path": "/items/{id:int}"}, {"path": "/share/{id:int}"}]));
        draft["path_case"] = json!("insensitive");
        let r = check(&empty(), draft, &[U, U]);
        assert_eq!(
            r.rules,
            vec![Warned(C::ForbiddenKey), Warned(C::ForbiddenKey)]
        );
        assert_eq!(kept_rules(&r), 2);
    }

    #[test]
    fn draft_rules_invalid_rule_is_dropped_even_when_user_written() {
        let rules = json!([{"path": "/items/{id:int}"}, {"path": "/other/{id:int"}]);
        for provenance in [[D, D], [U, U]] {
            let r = check(&empty(), tools(rules.clone()), &provenance);
            assert_eq!(r.rules, vec![Kept, Dropped(C::InvalidRule)]);
        }
        // Rules valid alone but conflicting together keep the earlier one.
        let r = check(
            &empty(),
            tools(json!([{"path": "/items/{id:int}"}, {"path": "/other/{id:token}"}])),
            &[D, D],
        );
        assert_eq!(r.rules, vec![Kept, Dropped(C::InvalidRule)]);
        assert_eq!(kept_rules(&r), 1);
    }

    #[test]
    fn draft_rules_adapter_level_invalid_rejects_draft() {
        let mut draft = tools(json!([{"path": "/items/{id:int}"}]));
        draft["type"] = json!("X");
        let DraftOutcome::Invalid(e) = refused(&empty(), draft, &[D]) else {
            panic!("expected a validation error");
        };
        assert_eq!(e.code, crate::ErrorCode::BadAdapterType);
        assert_eq!((e.adapter, e.rule), (Some(0), None));
    }

    fn pending() -> ScopeState {
        ScopeState::Pending {
            capture: "owner".into(),
        }
    }

    fn owner(values: &[&str]) -> Scope {
        vec![(
            "owner".to_owned(),
            values.iter().map(|v| (*v).to_owned()).collect(),
        )]
    }

    #[test]
    fn resolve_scope_choice_accepts_values_or_unscoped_for_pending_scope() {
        let values = ScopeChoice::Values(vec!["acme-corp".into(), "acme-labs".into()]);
        assert_eq!(
            resolve_scope_choice(&pending(), &values, &catalog(), false),
            Ok(ResolvedScope::Scoped(owner(&["acme-corp", "acme-labs"])))
        );
        assert_eq!(
            resolve_scope_choice(&pending(), &ScopeChoice::Unscoped, &catalog(), false),
            Ok(ResolvedScope::Unscoped)
        );
    }

    #[test]
    fn resolve_scope_choice_refuses_missing_or_empty_scope_when_pending() {
        for choice in [ScopeChoice::None, ScopeChoice::Values(vec![])] {
            assert_eq!(
                resolve_scope_choice(&pending(), &choice, &catalog(), false),
                Err(C::ScopePending)
            );
        }
        for bad in ["Acme", "", "a/b", "a b"] {
            let choice = ScopeChoice::Values(vec![bad.into()]);
            assert_eq!(
                resolve_scope_choice(&pending(), &choice, &catalog(), false),
                Err(C::ScopeChoice),
                "{bad:?}"
            );
        }
        let too_many = ScopeChoice::Values((0..65).map(|i| format!("org{i}")).collect());
        assert_eq!(
            resolve_scope_choice(&pending(), &too_many, &catalog(), false),
            Err(C::ScopeChoice)
        );
    }

    #[test]
    fn resolve_scope_choice_allows_only_unscoped_where_no_tenant_position() {
        let state = ScopeState::UnscopedOnly;
        assert_eq!(
            resolve_scope_choice(&state, &ScopeChoice::Unscoped, &catalog(), false),
            Ok(ResolvedScope::Unscoped)
        );
        for choice in [ScopeChoice::None, ScopeChoice::Values(vec!["acme".into()])] {
            assert_eq!(
                resolve_scope_choice(&state, &choice, &catalog(), false),
                Err(C::ScopeChoice)
            );
        }
    }

    #[test]
    fn resolve_scope_choice_keeps_copied_scope_fixed() {
        let state = ScopeState::Copied(owner(&["acme-corp"]));
        assert_eq!(
            resolve_scope_choice(&state, &ScopeChoice::None, &catalog(), false),
            Ok(ResolvedScope::Scoped(owner(&["acme-corp"])))
        );
        for choice in [
            ScopeChoice::Unscoped,
            ScopeChoice::Values(vec!["other".into()]),
        ] {
            assert_eq!(
                resolve_scope_choice(&state, &choice, &catalog(), false),
                Err(C::ScopeChoice)
            );
        }
        let state = ScopeState::NotShared;
        assert_eq!(
            resolve_scope_choice(&state, &ScopeChoice::None, &catalog(), false),
            Ok(ResolvedScope::NotShared)
        );
        assert_eq!(
            resolve_scope_choice(&state, &ScopeChoice::Unscoped, &catalog(), false),
            Err(C::ScopeChoice)
        );
    }

    #[test]
    fn scope_choice_never_changes_rule_verdicts() {
        let mut draft = github_project();
        draft["match"] = json!([
            {"path": "/orgs/{owner:name|lower}/projects/{number:int}"},
            {"path": "/orgs/{owner:name|lower}/share/{number:int}"}
        ]);
        let first = check(&empty(), draft.clone(), &[D, D]);
        for choice in [
            ScopeChoice::Unscoped,
            ScopeChoice::Values(vec!["acme-corp".into()]),
            ScopeChoice::None,
        ] {
            let _ = resolve_scope_choice(&first.scope, &choice, &catalog(), false);
            let again = check(&empty(), draft.clone(), &[D, D]);
            assert_eq!(again.rules, first.rules);
            assert_eq!(again.scope, first.scope);
        }
        assert_eq!(first.rules, vec![Kept, Dropped(C::BlockedRoute)]);
    }

    #[test]
    fn draft_rules_id_without_identifier_rejects_draft() {
        let draft = json!({
            "type": "x.kind",
            "origin": TOOLS,
            "match": [{"path": "/x/{kind:(a|b)}"}],
            "id": "x:{host}/{kind}"
        });
        assert!(matches!(
            refused(&empty(), draft, &[D]),
            DraftOutcome::DraftError(C::IdNeedsTypedCapture)
        ));
    }

    #[test]
    fn notion_workspace_pages_are_anchored_by_the_scoped_tenant() {
        let draft = json!({
            "type": "notion.page",
            "origin": "https://www.notion.so",
            "match": [{"path": "/{workspace:name|lower}/{id:hex(32)@tail}"}],
            "id": "notion:{host}/{workspace}/{id}"
        });
        let r = check(&empty(), draft, &[D]);
        assert_eq!(
            r.scope,
            ScopeState::Pending {
                capture: "workspace".into()
            }
        );
        // The tenant capture the review scopes is the rule's anchor
        // (adapter-templates §4.1, §6), as it is for a hand-written adapter.
        assert_eq!(r.rules, vec![Kept]);
        assert!(r.document.is_some());
        assert!(r.scope_anchored);
        // Without the scope the rule has no anchor, so unscoped is refused.
        assert_eq!(
            resolve_scope_choice(
                &r.scope,
                &ScopeChoice::Unscoped,
                &catalog(),
                r.scope_anchored
            ),
            Err(C::ScopeChoice)
        );
        assert_eq!(
            resolve_scope_choice(
                &r.scope,
                &ScopeChoice::Values(vec!["acme".into()]),
                &catalog(),
                r.scope_anchored
            ),
            Ok(ResolvedScope::Scoped(vec![(
                "workspace".into(),
                vec!["acme".into()]
            )]))
        );
    }

    #[test]
    fn only_a_scope_restricted_capture_anchors_a_draft() {
        // A rule with a route word does not depend on its scope.
        let linear = json!({
            "type": "linear.issue",
            "origin": "https://linear.app",
            "match": [{"path": "/{workspace:name|lower}/issue/{key:jira_key}"}],
            "id": "linear.issue:{host}/{workspace}/{key}"
        });
        let r = check(&empty(), linear, &[D]);
        assert_eq!(r.rules, vec![Kept]);
        assert!(!r.scope_anchored);
        assert_eq!(
            resolve_scope_choice(
                &r.scope,
                &ScopeChoice::Unscoped,
                &catalog(),
                r.scope_anchored
            ),
            Ok(ResolvedScope::Unscoped)
        );
        // Off a shared origin a `name` capture is not scoped and anchors
        // nothing, and neither does a fixed-shape capture.
        for (path, id) in [
            ("/{space:name|lower}/{id:hex(32)}", "x:{host}/{space}/{id}"),
            ("/{id:hex(32)@tail}", "x:{host}/{id}"),
        ] {
            let draft = json!({
                "type": "x.item", "origin": TOOLS,
                "match": [{"path": path}], "id": id
            });
            let r = check(&empty(), draft, &[D]);
            assert_eq!(r.rules, vec![Dropped(C::NoLeadingLiteral)], "{path}");
            assert!(!r.scope_anchored);
        }
        // A copied scope anchors its capture too.
        let existing = doc(json!({"format": 1, "artifacts": [github_scoped("acme-corp")]}));
        let draft = json!({
            "type": "github.thing",
            "origin": "https://github.com",
            "match": [{"path": "/{owner:name|lower}/{number:int}"}],
            "id": "github.thing:{host}/{owner}/{number}"
        });
        let r = check(&existing, draft, &[D]);
        assert!(matches!(r.scope, ScopeState::Copied(_)));
        assert_eq!(r.rules, vec![Kept]);
        assert!(r.scope_anchored);
    }
}

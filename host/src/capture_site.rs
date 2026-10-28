//! Capture Site drafting for sites no starter covers (DESIGN §5.1).
//!
//! The probe URLs, query-key lists, route and mode words, placeholder fill
//! values, starter types and shared-origin positions are host-owned data in
//! `protocol/capture-site/catalog.json`. The extension's own copies are
//! pinned to the same file by its tests.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use adapter_templates::shape::UrlShape;
use adapter_templates::structural::{self, DraftedAdapter, DropReason, ShapeInput};
use adapter_templates::{
    Document, DraftErrorCode, DraftOutcome, DraftPolicy, Fill, Provenance, ResolvedScope,
    RuleVerdict, SHARED_ORIGINS, ScopeChoice, ScopeState, SharedOrigin, Value as AdapterValue,
    check_draft_rules, resolve_scope_choice, scope_state,
};
use brauser_protocol::{
    CheckSiteDraftRequest, DraftDropReason, DraftScopeState, DraftSiteRequest, DraftTenantView,
    DraftedAdapterView, DraftedRuleView, DroppedView, ErrorCode, PROTOCOL_VERSION, PreviewRowView,
    PreviewSource, ResolvedScopeKind, Response, RuleProvenance, RuleVerdictKind, RuleVerdictView,
    ScopeChoiceView, ScopeVerdictView, ShapeCount, SiteDraftChecked, SiteDrafted, SiteDrafter,
    UncheckedReason,
};
use serde::Deserialize;

use crate::config::{ConfigLock, ConfigStore, NoteKeyRule};
use crate::native::error;

const CATALOG: &str = include_str!("../../protocol/capture-site/catalog.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    format: u32,
    probe_paths: Vec<String>,
    search_keys: Vec<String>,
    paging_keys: Vec<String>,
    blocked_route_words: Vec<String>,
    secret_query_keys: Vec<String>,
    secret_query_key_parts: Vec<String>,
    secret_query_key_suffixes: Vec<String>,
    mode_words: BTreeMap<String, String>,
    fill: FillFile,
    starter_types: Vec<String>,
    shared_origins: BTreeMap<String, SharedOriginFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FillFile {
    num: String,
    key: String,
    slug: String,
    uuid: String,
    hex32: String,
    hex40: String,
    hex64: String,
    tailhex32: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedOriginFile {
    tenant: Option<TenantFile>,
    account_index: Vec<AccountIndexFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantFile {
    segment: usize,
    capture: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountIndexFile {
    literal: String,
    segment: usize,
}

fn parse(source: &str) -> Result<DraftPolicy, String> {
    let file: CatalogFile = serde_json::from_str(source).map_err(|error| error.to_string())?;
    if file.format != 1 {
        return Err(format!("unsupported catalog format {}", file.format));
    }
    let FillFile {
        num,
        key,
        slug,
        uuid,
        hex32,
        hex40,
        hex64,
        tailhex32,
        id,
    } = file.fill;
    Ok(DraftPolicy {
        probe_paths: file.probe_paths,
        search_keys: file.search_keys,
        paging_keys: file.paging_keys,
        blocked_route_words: file.blocked_route_words,
        secret_query_keys: file.secret_query_keys,
        secret_query_key_parts: file.secret_query_key_parts,
        secret_query_key_suffixes: file.secret_query_key_suffixes,
        mode_words: file.mode_words.into_iter().collect(),
        fill: Fill {
            num,
            key,
            slug,
            uuid,
            hex32,
            hex40,
            hex64,
            tailhex32,
            id,
        },
        starter_types: file.starter_types,
        shared: file
            .shared_origins
            .into_iter()
            .map(|(origin, entry)| SharedOrigin {
                origin,
                tenant: entry.tenant.map(|t| (t.segment, t.capture)),
                account_index: entry
                    .account_index
                    .into_iter()
                    .map(|a| (a.literal, a.segment))
                    .collect(),
            })
            .collect(),
    })
}

/// The host-owned drafting catalog, parsed once. The file is compiled in,
/// and the tests below parse it, so a malformed catalog never ships.
pub fn catalog() -> &'static DraftPolicy {
    static CATALOG_POLICY: OnceLock<DraftPolicy> = OnceLock::new();
    CATALOG_POLICY.get_or_init(|| match parse(CATALOG) {
        Ok(policy) => policy,
        Err(error) => panic!("protocol/capture-site/catalog.json is invalid: {error}"),
    })
}

/// Request and response bounds, mirrored from `protocol/schema.json`. The
/// host enforces every request bound at runtime.
const MAX_ORIGIN_CHARS: usize = 2_048;
const MAX_VISITED_SHAPES: usize = 21;
const MAX_LINK_SHAPES: usize = 50;
const MAX_SHAPE_COUNT: u32 = 1_000_000;
const MAX_CHECKED_ADAPTERS: usize = structural::MAX_DRAFT_ADAPTERS;
const MAX_CHECKED_RULES: usize = adapter_templates::MAX_RULES;
const MAX_SCOPE_VALUES: usize = 64;
const MAX_SCOPE_VALUE_CHARS: usize = 128;
const MAX_DROPPED_ROWS: usize = 128;
const MAX_PREVIEW_ROWS: usize = 128;
const MAX_TAKEN_TYPES: usize = 160;
const MAX_TYPE_CHARS: usize = 64;

/// Whether `origin` is exactly the serialized origin of an HTTP(S) URL with
/// a host and no credentials: no path, query, fragment, trailing slash,
/// default port or uppercase that the browser's own origin would not show.
fn exact_origin(origin: &str) -> bool {
    if origin.is_empty() || origin.chars().count() > MAX_ORIGIN_CHARS {
        return false;
    }
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.origin().ascii_serialization() == origin
}

/// Parses the request's shapes. `None` refuses the whole request: a list
/// over its bound, a count out of range, or a value that is not a shape in
/// its canonical form, such as a raw URL or path.
fn shape_input(visited: &[ShapeCount], links: &[ShapeCount]) -> Option<ShapeInput> {
    fn parse(list: &[ShapeCount], bound: usize) -> Option<Vec<(UrlShape, u32)>> {
        if list.len() > bound {
            return None;
        }
        list.iter()
            .map(|entry| {
                if !(1..=MAX_SHAPE_COUNT).contains(&entry.count) {
                    return None;
                }
                let shape = UrlShape::parse(&entry.shape).ok()?;
                (shape.to_string() == entry.shape).then_some((shape, entry.count))
            })
            .collect()
    }
    Some(ShapeInput {
        visited: parse(visited, MAX_VISITED_SHAPES)?,
        links: parse(links, MAX_LINK_SHAPES)?,
    })
}

/// Takes the config lock and re-reads config.toml, as `get_artifact` does.
/// Drafting is judged against the current adapter document, so a config or
/// adapter document that needs repair refuses.
fn locked_document(config: &mut ConfigStore) -> Result<ConfigLock, (ErrorCode, &'static str)> {
    let lock = config
        .lock_current()
        .map_err(|_| (ErrorCode::Internal, "configuration is unavailable"))?;
    if config.refresh().is_err()
        || config.config_issue().is_some()
        || config.adapter_issue().is_some()
        || config.adapter_document().is_err()
    {
        return Err((
            ErrorCode::InvalidConfig,
            "configuration or adapter policy is unavailable or needs repair",
        ));
    }
    Ok(lock)
}

/// The types a drafted adapter may not take: the config's adapter types,
/// the catalog's starter types and every recorded contract's type. The
/// config's and the starters' come first, so a bound never hides them.
fn taken_types(config: &ConfigStore, existing: &Document) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    let mut current: Vec<String> = existing
        .adapters()
        .iter()
        .map(|a| a.artifact_type().to_owned())
        .chain(catalog().starter_types.iter().cloned())
        .collect();
    current.sort();
    for artifact_type in current.into_iter().chain(config.recorded_adapter_types()) {
        if seen.insert(artifact_type.clone()) {
            ordered.push(artifact_type);
        }
    }
    ordered
}

/// The adapter's JSON form for the wire, keys in table order.
fn to_json(value: &AdapterValue) -> serde_json::Value {
    match value {
        AdapterValue::Null => serde_json::Value::Null,
        AdapterValue::Bool(b) => serde_json::Value::Bool(*b),
        AdapterValue::Int(i) => serde_json::Value::from(*i),
        AdapterValue::Float(f) => serde_json::Number::from_f64(*f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        AdapterValue::String(s) => serde_json::Value::String(s.clone()),
        AdapterValue::Array(items) => serde_json::Value::Array(items.iter().map(to_json).collect()),
        AdapterValue::Table(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), to_json(value)))
                .collect(),
        ),
    }
}

fn scope_state_view(state: &ScopeState) -> DraftScopeState {
    match state {
        ScopeState::NotShared => DraftScopeState::NotShared,
        ScopeState::Copied(_) => DraftScopeState::Copied,
        ScopeState::Pending { .. } => DraftScopeState::Pending,
        ScopeState::UnscopedOnly => DraftScopeState::UnscopedOnly,
    }
}

fn draft_error_reason(code: DraftErrorCode) -> DraftDropReason {
    match code {
        DraftErrorCode::ForbiddenKey => DraftDropReason::DraftForbiddenKey,
        DraftErrorCode::NoScopeSource => DraftDropReason::DraftNoScopeSource,
        DraftErrorCode::ScopeConflict => DraftDropReason::DraftScopeConflict,
        DraftErrorCode::NoLeadingLiteral => DraftDropReason::DraftNoLeadingLiteral,
        DraftErrorCode::RestSecond => DraftDropReason::DraftRestSecond,
        DraftErrorCode::IdNeedsTypedCapture => DraftDropReason::DraftIdNeedsTypedCapture,
        DraftErrorCode::SearchCapture => DraftDropReason::DraftSearchCapture,
        DraftErrorCode::MatchesProbe => DraftDropReason::DraftMatchesProbe,
        DraftErrorCode::Overlap => DraftDropReason::DraftOverlap,
        DraftErrorCode::BlockedRoute => DraftDropReason::DraftBlockedRoute,
        DraftErrorCode::PagingCapture => DraftDropReason::DraftPagingCapture,
        DraftErrorCode::SecretCapture => DraftDropReason::DraftSecretCapture,
        DraftErrorCode::InvalidRule => DraftDropReason::DraftInvalidRule,
        DraftErrorCode::ScopePending => DraftDropReason::DraftScopePending,
        DraftErrorCode::ScopeChoice => DraftDropReason::DraftScopeChoice,
    }
}

fn drop_reason(reason: DropReason) -> DraftDropReason {
    match reason {
        DropReason::Root => DraftDropReason::Root,
        DropReason::Probe => DraftDropReason::Probe,
        DropReason::SearchOrPaging => DraftDropReason::SearchOrPaging,
        DropReason::SecretQuery => DraftDropReason::SecretQuery,
        DropReason::BlockedRoute => DraftDropReason::BlockedRoute,
        DropReason::ParamName => DraftDropReason::ParamName,
        DropReason::AlreadyRecognized => DraftDropReason::AlreadyRecognized,
        DropReason::NoRouteWord => DraftDropReason::NoRouteWord,
        DropReason::NoIdentifier => DraftDropReason::NoIdentifier,
        DropReason::Limit => DraftDropReason::Limit,
        DropReason::Check(code) => draft_error_reason(code),
    }
}

fn adapter_view(drafted: &DraftedAdapter) -> DraftedAdapterView {
    DraftedAdapterView {
        adapter: to_json(&drafted.adapter),
        scope: scope_state_view(&drafted.scope),
        tenant: drafted
            .tenant
            .as_ref()
            .map(|(segment, capture)| DraftTenantView {
                segment: u32::try_from(*segment).unwrap_or(u32::MAX),
                capture: capture.clone(),
            }),
        rules: drafted
            .rules
            .iter()
            .map(|rule| {
                let unchecked_reason = if rule.link_only {
                    Some(UncheckedReason::LinkOnly)
                } else if rule.token_only {
                    Some(UncheckedReason::TokenOnly)
                } else {
                    None
                };
                DraftedRuleView {
                    rule: u32::try_from(rule.rule).unwrap_or(u32::MAX),
                    checked: unchecked_reason.is_none(),
                    unchecked_reason,
                    visited: rule.visited,
                    links: rule.links,
                    example: rule.example.clone(),
                }
            })
            .collect(),
    }
}

/// One adapter as the preview evaluates it: its surviving rules matched
/// without scope, as the draft checks match them (SQ-V3). `rules` maps a
/// rule of `document` back to its index in the checked table.
struct PreviewAdapter {
    document: Document,
    rules: Vec<usize>,
}

/// The preview form of a checked table: only the rules in `keep`, any
/// `scope` or `unscoped` removed, and `unscoped` on a shared origin so the
/// rules match without scope. `None` when nothing survives or the table
/// does not validate.
fn preview_adapter(table: &AdapterValue, keep: Vec<usize>) -> Option<PreviewAdapter> {
    let AdapterValue::Table(entries) = table else {
        return None;
    };
    let rules = table.get("match").and_then(AdapterValue::as_array)?;
    let kept: Vec<AdapterValue> = keep.iter().filter_map(|&i| rules.get(i).cloned()).collect();
    if kept.is_empty() || kept.len() != keep.len() {
        return None;
    }
    let mut entries: Vec<(String, AdapterValue)> = entries
        .iter()
        .filter(|(key, _)| key != "scope" && key != "unscoped")
        .map(|(key, value)| match key.as_str() {
            "match" => (key.clone(), AdapterValue::Array(kept.clone())),
            _ => (key.clone(), value.clone()),
        })
        .collect();
    let shared = table
        .get("origin")
        .and_then(AdapterValue::as_str)
        .is_some_and(|origin| SHARED_ORIGINS.contains(&origin));
    if shared {
        entries.push((
            "unscoped".to_owned(),
            AdapterValue::String("acknowledged".to_owned()),
        ));
    }
    let document = Document::from_value(&AdapterValue::Table(vec![
        ("format".to_owned(), AdapterValue::Int(1)),
        (
            "artifacts".to_owned(),
            AdapterValue::Array(vec![AdapterValue::Table(entries)]),
        ),
    ]))
    .ok()?;
    Some(PreviewAdapter {
        document,
        rules: keep,
    })
}

/// The match preview: every visited and link shape filled to an
/// illustrative URL, then every probe URL, with the first adapter rule that
/// recognizes it without scope. No row holds a real URL.
fn preview(
    origin: &str,
    input: &ShapeInput,
    adapters: &[Option<PreviewAdapter>],
) -> Vec<PreviewRowView> {
    let policy = catalog();
    let row = |source: PreviewSource, shape: String, url: String| {
        let found = adapters.iter().enumerate().find_map(|(index, adapter)| {
            let adapter = adapter.as_ref()?;
            let found = adapter.document.evaluate(&url, None).ok()?;
            Some((
                index,
                adapter.rules.get(found.rule).copied()?,
                found.artifact_id,
            ))
        });
        PreviewRowView {
            source,
            shape,
            url,
            adapter: found
                .as_ref()
                .map(|(a, _, _)| u32::try_from(*a).unwrap_or(u32::MAX)),
            rule: found
                .as_ref()
                .map(|(_, r, _)| u32::try_from(*r).unwrap_or(u32::MAX)),
            artifact_id: found.map(|(_, _, id)| id),
        }
    };
    let shapes = input
        .visited
        .iter()
        .map(|(shape, _)| (PreviewSource::Visited, shape))
        .chain(
            input
                .links
                .iter()
                .map(|(shape, _)| (PreviewSource::Link, shape)),
        );
    shapes
        .map(|(source, shape)| row(source, shape.to_string(), shape.fill(origin, policy)))
        .chain(policy.probe_paths.iter().map(|path| {
            row(
                PreviewSource::Probe,
                path.clone(),
                format!("{origin}{path}"),
            )
        }))
        .take(MAX_PREVIEW_ROWS)
        .collect()
}

/// How many already-recognized artifacts the preview's shapes would move to
/// a drafted adapter: a filled shape a survivor matches that the current
/// policy, or a recorded contract's note key, already names. The drafter
/// judges every rule against the current policy and the recorded contracts
/// (filter and overlap check), so this is 0; `draft_site` refuses a draft
/// where it is not rather than offer rules the save would refuse.
fn remapped(config: &ConfigStore, existing: &Document, rows: &[PreviewRowView]) -> u32 {
    let count = rows
        .iter()
        .filter(|row| row.source != PreviewSource::Probe && row.adapter.is_some())
        .filter(|row| {
            existing.evaluate(&row.url, None).is_ok()
                || matches!(config.note_key_rule(&row.url), NoteKeyRule::Artifact(_))
        })
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// `draft_site`: the structural draft for one exact origin from private URL
/// shapes, bound to the config revision it was drafted against (DESIGN
/// §5.1). It reads the config and writes nothing.
pub fn draft_site(value: DraftSiteRequest, config: &mut ConfigStore) -> Response {
    let id = value.request_id.as_str();
    if !exact_origin(&value.origin) {
        return error(
            id,
            ErrorCode::InvalidRequest,
            "origin must be an exact HTTP(S) origin",
        );
    }
    let Some(input) = shape_input(&value.visited, &value.links) else {
        return error(
            id,
            ErrorCode::InvalidRequest,
            "shapes are invalid or over their bounds",
        );
    };
    let _lock = match locked_document(config) {
        Ok(lock) => lock,
        Err((code, message)) => return error(id, code, message),
    };
    let Ok(existing) = config.adapter_document() else {
        return error(
            id,
            ErrorCode::InvalidConfig,
            "adapter policy is unavailable",
        );
    };
    let taken = taken_types(config, existing);
    let recorded = config.recorded_contract_documents();
    let draft =
        structural::draft_site(&value.origin, existing, recorded, &input, catalog(), &taken);
    let previewed: Vec<Option<PreviewAdapter>> = draft
        .adapters
        .iter()
        .map(|a| preview_adapter(&a.adapter, a.rules.iter().map(|r| r.rule).collect()))
        .collect();
    let rows = preview(&value.origin, &input, &previewed);
    let remapped = remapped(config, existing, &rows);
    // The remap invariant (DESIGN §5.1): a drafted rule never re-keys an
    // artifact the current policy or a recorded contract already names.
    if remapped != 0 {
        return error(
            id,
            ErrorCode::Internal,
            "the draft would move already captured pages to a new kind of page",
        );
    }
    Response::SiteDrafted(SiteDrafted {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id,
        revision: config.revision().to_owned(),
        drafter: SiteDrafter::Structural,
        adapters: draft.adapters.iter().map(adapter_view).collect(),
        dropped: draft
            .dropped
            .iter()
            .take(MAX_DROPPED_ROWS)
            .map(|d| DroppedView {
                shape: Some(d.shape.clone()),
                adapter: None,
                rule: None,
                reason: drop_reason(d.reason),
            })
            .collect(),
        preview: rows,
        taken_types: taken
            .into_iter()
            .filter(|t| !t.is_empty() && t.chars().count() <= MAX_TYPE_CHARS)
            .take(MAX_TAKEN_TYPES)
            .collect(),
        remapped,
    })
}

fn scope_choice(view: Option<&ScopeChoiceView>) -> ScopeChoice {
    match view {
        None => ScopeChoice::None,
        Some(ScopeChoiceView::ScopeValues(scope)) => ScopeChoice::Values(scope.values.clone()),
        Some(ScopeChoiceView::UnscopedChoice(_)) => ScopeChoice::Unscoped,
    }
}

fn scope_choice_in_bounds(view: Option<&ScopeChoiceView>) -> bool {
    match view {
        Some(ScopeChoiceView::ScopeValues(scope)) => {
            (1..=MAX_SCOPE_VALUES).contains(&scope.values.len())
                && scope
                    .values
                    .iter()
                    .all(|v| v.chars().count() <= MAX_SCOPE_VALUE_CHARS)
        }
        None | Some(ScopeChoiceView::UnscopedChoice(_)) => true,
    }
}

/// The checked tables, converted, when every bound holds: at most 8 tables
/// on the request's origin, one provenance list and one scope choice each,
/// and one provenance per rule (at most 16).
fn checked_tables(value: &CheckSiteDraftRequest) -> Option<Vec<(AdapterValue, Vec<Provenance>)>> {
    let n = value.adapters.len();
    if n > MAX_CHECKED_ADAPTERS || value.provenance.len() != n || value.scope.len() != n {
        return None;
    }
    if !value
        .scope
        .iter()
        .all(|s| scope_choice_in_bounds(s.as_ref()))
    {
        return None;
    }
    value
        .adapters
        .iter()
        .zip(&value.provenance)
        .map(|(table, provenance)| {
            let object = table.as_object()?;
            if object.get("origin").and_then(serde_json::Value::as_str) != Some(&value.origin) {
                return None;
            }
            let rules = object.get("match").and_then(serde_json::Value::as_array);
            if provenance.len() > MAX_CHECKED_RULES
                || rules.is_some_and(|rules| rules.len() != provenance.len())
            {
                return None;
            }
            let table: AdapterValue = serde_json::from_value(table.clone()).ok()?;
            let provenance = provenance
                .iter()
                .map(|p| match p {
                    RuleProvenance::Drafted => Provenance::Drafted,
                    RuleProvenance::UserWritten => Provenance::UserWritten,
                })
                .collect();
            Some((table, provenance))
        })
        .collect()
}

fn rule_verdict_view(verdict: RuleVerdict) -> RuleVerdictView {
    match verdict {
        RuleVerdict::Kept => RuleVerdictView {
            verdict: RuleVerdictKind::Kept,
            reason: None,
        },
        RuleVerdict::Dropped(code) => RuleVerdictView {
            verdict: RuleVerdictKind::Dropped,
            reason: Some(code.as_str().to_owned()),
        },
        RuleVerdict::Warned(code) => RuleVerdictView {
            verdict: RuleVerdictKind::Warned,
            reason: Some(code.as_str().to_owned()),
        },
    }
}

/// `check_site_draft`: re-checks the review's selection against the
/// current config before the save. Each checked table is judged by
/// `check_draft_rules` with its provenance and the earlier tables as
/// accepted, then its scope choice by `resolve_scope_choice`, which refuses
/// `unscoped` for a table with a rule anchored only by its tenant. The choice
/// travels beside the table and is never merged into it, so a table that
/// itself carries `title`, `scope` or `unscoped` with a drafted rule is
/// still refused as `draft_forbidden_key`. It writes nothing.
pub fn check_site_draft(value: CheckSiteDraftRequest, config: &mut ConfigStore) -> Response {
    let id = value.request_id.as_str();
    if !exact_origin(&value.origin) {
        return error(
            id,
            ErrorCode::InvalidRequest,
            "origin must be an exact HTTP(S) origin",
        );
    }
    let Some(input) = shape_input(&value.visited, &value.links) else {
        return error(
            id,
            ErrorCode::InvalidRequest,
            "shapes are invalid or over their bounds",
        );
    };
    let Some(tables) = checked_tables(&value) else {
        return error(
            id,
            ErrorCode::InvalidRequest,
            "checked adapters are invalid or over their bounds",
        );
    };
    let _lock = match locked_document(config) {
        Ok(lock) => lock,
        Err((code, message)) => return error(id, code, message),
    };
    if config.revision() != value.expected_revision {
        return error(
            id,
            ErrorCode::Conflict,
            "configuration changed; draft the site again",
        );
    }
    let Ok(existing) = config.adapter_document() else {
        return error(
            id,
            ErrorCode::InvalidConfig,
            "adapter policy is unavailable",
        );
    };
    let policy = catalog();
    let state = scope_state(existing, Some(&value.origin), policy);
    // Recorded contracts freeze their routes' IDs, so a rule that may
    // overlap one is judged as overlapping an accepted adapter.
    let mut accepted: Vec<Document> = config.recorded_contract_documents().to_vec();
    let mut rules = Vec::with_capacity(tables.len());
    let mut scope = Vec::with_capacity(tables.len());
    let mut previewed = Vec::with_capacity(tables.len());
    for ((table, provenance), choice) in tables.iter().zip(&value.scope) {
        let (verdicts, scope_anchored) =
            match check_draft_rules(existing, table, provenance, policy, &accepted) {
                Ok(report) => {
                    accepted.extend(report.document);
                    (report.rules, report.scope_anchored)
                }
                Err(outcome) => {
                    let code = match outcome {
                        DraftOutcome::DraftError(code) => code,
                        DraftOutcome::Invalid(_) | DraftOutcome::Ok { .. } => {
                            DraftErrorCode::InvalidRule
                        }
                    };
                    (vec![RuleVerdict::Dropped(code); provenance.len()], false)
                }
            };
        let keep: Vec<usize> = verdicts
            .iter()
            .enumerate()
            .filter(|(_, v)| !matches!(v, RuleVerdict::Dropped(_)))
            .map(|(i, _)| i)
            .collect();
        previewed.push(preview_adapter(table, keep));
        rules.push(verdicts.into_iter().map(rule_verdict_view).collect());
        // A rule anchored only by the scoped tenant cannot be saved unscoped.
        let resolved = resolve_scope_choice(
            &state,
            &scope_choice(choice.as_ref()),
            policy,
            scope_anchored,
        );
        scope.push(ScopeVerdictView {
            state: scope_state_view(&state),
            resolved: resolved.as_ref().ok().map(|r| match r {
                ResolvedScope::NotShared => ResolvedScopeKind::NotShared,
                ResolvedScope::Scoped(_) => ResolvedScopeKind::Scoped,
                ResolvedScope::Unscoped => ResolvedScopeKind::Unscoped,
            }),
            reason: resolved.err().map(|code| code.as_str().to_owned()),
        });
    }
    Response::SiteDraftChecked(SiteDraftChecked {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id.clone(),
        revision: config.revision().to_owned(),
        rules,
        scope,
        preview: preview(&value.origin, &input, &previewed),
    })
}

#[cfg(test)]
mod tests {
    use super::{CATALOG, catalog, parse};
    use std::collections::BTreeSet;

    #[test]
    fn catalog_parses_and_lists_every_shared_origin() {
        let listed: BTreeSet<&str> = catalog().shared.iter().map(|s| s.origin.as_str()).collect();
        let expected: BTreeSet<&str> = adapter_templates::SHARED_ORIGINS.iter().copied().collect();
        assert_eq!(listed, expected);
        assert_eq!(
            listed.len(),
            catalog().shared.len(),
            "no origin is listed twice"
        );
    }

    #[test]
    fn catalog_probe_paths_include_root_search_auth_and_capability_routes() {
        let probes = &catalog().probe_paths;
        for path in [
            "/",
            "/search?q=x",
            "/login",
            "/dashboard",
            "/oauth",
            "/auth",
            "/signin",
            "/invite",
            "/reset",
            "/password",
            "/magic",
            "/token",
            "/share",
            "/unsubscribe",
            "/?jql=x",
        ] {
            assert!(probes.iter().any(|p| p == path), "missing probe {path}");
        }
    }

    #[test]
    fn catalog_refuses_an_unknown_format_or_key() {
        let bumped = CATALOG.replacen("\"format\": 1", "\"format\": 2", 1);
        assert!(parse(&bumped).unwrap_err().contains("format 2"));
        let extra = CATALOG.replacen("{", "{\"extra\": true,", 1);
        assert!(parse(&extra).is_err());
    }

    #[test]
    fn catalog_mode_words_and_fill_values_convert() {
        let policy = catalog();
        assert!(
            policy
                .mode_words
                .contains(&("files".to_owned(), "review".to_owned()))
        );
        assert_eq!(policy.fill.num, "123456");
        assert_eq!(policy.fill.key, "ABC-123");
        assert_eq!(policy.fill.slug, "example-page");
        let github = policy.shared_origin("https://github.com").unwrap();
        assert_eq!(github.tenant, Some((0, "owner".to_owned())));
        let docs = policy.shared_origin("https://docs.google.com").unwrap();
        assert_eq!(docs.tenant, None);
        assert!(docs.account_index.iter().all(|(literal, _)| literal == "u"));
    }
}

#[cfg(test)]
mod handler_tests {
    use super::{catalog, check_site_draft, draft_site};
    use crate::config::{ConfigStore, fixtures};
    use crate::frame::MAX_OUTBOUND_BYTES;
    use brauser_protocol::{
        CheckSiteDraftRequest, DraftDropReason, DraftScopeState, DraftSiteRequest, ErrorCode,
        PROTOCOL_VERSION, PreviewSource, ResolvedScopeKind, Response, RuleProvenance,
        RuleVerdictKind, ScopeChoiceView, ScopeValues, ShapeCount, SiteDraftChecked, SiteDrafted,
        SiteDrafter, UnscopedChoice,
    };
    use serde_json::{Value as Json, json};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    const TOOLS: &str = "https://tools.example.com";
    const NOTION: &str = "https://www.notion.so";
    const LINEAR: &str = "https://linear.app";
    const FIGMA: &str = "https://www.figma.com";
    const TOOLS_ITEMS: &str = "[[artifacts]]\ntype = \"tools.item\"\norigin = \"https://tools.example.com\"\nmatch = [{ path = \"/items/{id:int}\" }]\nid = \"tools.item:{host}/{id}\"\n";

    struct Setup {
        folder: tempfile::TempDir,
        path: PathBuf,
        config: ConfigStore,
    }

    /// A configured home folder with `tables` after `[storage]`.
    fn setup(tables: &str) -> Setup {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        fixtures::write(&path, &root, "", "", tables);
        let mut config = ConfigStore::for_test(path.clone());
        config.refresh().unwrap();
        Setup {
            folder,
            path,
            config,
        }
    }

    fn adapters(tables: &[&str]) -> String {
        format!("[adapters]\nformat = 1\n\n{}", tables.join("\n"))
    }

    fn counts(shapes: &[&str]) -> Vec<ShapeCount> {
        shapes
            .iter()
            .map(|shape| ShapeCount {
                shape: (*shape).to_owned(),
                count: 1,
            })
            .collect()
    }

    fn request(origin: &str, visited: &[&str], links: &[&str]) -> DraftSiteRequest {
        DraftSiteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "draft-1".into(),
            origin: origin.into(),
            visited: counts(visited),
            links: counts(links),
        }
    }

    fn drafted(response: Response) -> SiteDrafted {
        match response {
            Response::SiteDrafted(drafted) => drafted,
            other => panic!("expected site_drafted, got {other:?}"),
        }
    }

    fn checked(response: Response) -> SiteDraftChecked {
        match response {
            Response::SiteDraftChecked(checked) => checked,
            other => panic!("expected site_draft_checked, got {other:?}"),
        }
    }

    fn code(response: Response) -> ErrorCode {
        match response {
            Response::Error(error) => error.code,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    fn check(
        origin: &str,
        revision: &str,
        tables: Vec<Json>,
        provenance: Vec<Vec<RuleProvenance>>,
        scope: Vec<Option<ScopeChoiceView>>,
    ) -> CheckSiteDraftRequest {
        CheckSiteDraftRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "check-1".into(),
            origin: origin.into(),
            expected_revision: revision.into(),
            adapters: tables,
            provenance,
            scope,
            visited: Vec::new(),
            links: Vec::new(),
        }
    }

    fn drafted_rules(table: &Json) -> Vec<RuleProvenance> {
        vec![RuleProvenance::Drafted; table["match"].as_array().unwrap().len()]
    }

    fn values(list: &[&str]) -> Option<ScopeChoiceView> {
        Some(ScopeChoiceView::ScopeValues(ScopeValues {
            values: list.iter().map(|v| (*v).to_owned()).collect(),
        }))
    }

    fn verdicts(checked: &SiteDraftChecked) -> Vec<Vec<(RuleVerdictKind, Option<&str>)>> {
        checked
            .rules
            .iter()
            .map(|rules| {
                rules
                    .iter()
                    .map(|r| (r.verdict, r.reason.as_deref()))
                    .collect()
            })
            .collect()
    }

    /// Every file under `dir` with its contents.
    fn tree(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(next) = pending.pop() {
            for entry in fs::read_dir(&next).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files.insert(path.clone(), Vec::new());
                    pending.push(path);
                } else {
                    files.insert(path.clone(), fs::read(&path).unwrap());
                }
            }
        }
        files
    }

    #[test]
    fn draft_site_returns_revision_bound_draft_for_unknown_origin() {
        let mut s = setup("");
        let site = drafted(draft_site(
            request(TOOLS, &["/items/{num}", "/items/{num}/edit"], &[]),
            &mut s.config,
        ));
        assert_eq!(site.request_id, "draft-1");
        assert_eq!(site.revision, s.config.revision());
        assert_eq!(site.drafter, SiteDrafter::Structural);
        assert_eq!(site.adapters.len(), 1, "{site:?}");
        let adapter = &site.adapters[0];
        assert_eq!(adapter.adapter["type"], "tools.items");
        assert_eq!(adapter.adapter["origin"], TOOLS);
        assert_eq!(adapter.scope, DraftScopeState::NotShared);
        assert_eq!(adapter.tenant, None);
        assert!(!adapter.rules.is_empty());
        assert!(adapter.rules.iter().all(|r| r.checked && r.visited > 0));
        // Only Tier 0 keys: never the review's overlays.
        let keys: Vec<&str> = adapter
            .adapter
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for key in keys {
            assert!(
                ["type", "origin", "match", "id", "modes", "refs"].contains(&key),
                "{key}"
            );
        }
        let row = &site.preview[0];
        assert_eq!(row.source, PreviewSource::Visited);
        assert_eq!(row.url, "https://tools.example.com/items/123456");
        assert_eq!(row.adapter, Some(0));
        assert_eq!(row.rule, Some(0));
        assert_eq!(
            row.artifact_id.as_deref(),
            Some("tools.items:tools.example.com/123456")
        );
    }

    #[test]
    fn draft_site_never_adds_or_changes_an_origin() {
        let mut s = setup(&adapters(&[TOOLS_ITEMS]));
        let before = fs::read(&s.path).unwrap();
        let snapshot = s.config.snapshot().clone();
        let site = drafted(draft_site(
            request(
                "https://work.example",
                &["/docs/{num}", "/boards/{num}"],
                &["/teams/{num}"],
            ),
            &mut s.config,
        ));
        assert!(!site.adapters.is_empty());
        for adapter in &site.adapters {
            assert_eq!(adapter.adapter["origin"], "https://work.example");
        }
        assert!(
            site.preview
                .iter()
                .all(|row| row.url.starts_with("https://work.example/"))
        );
        assert_eq!(fs::read(&s.path).unwrap(), before);
        assert_eq!(s.config.snapshot(), &snapshot);

        // Only an exact, credential-free HTTP(S) origin is drafted for.
        for origin in [
            "https://tools.example.com/",
            "https://tools.example.com/items",
            "https://TOOLS.example.com",
            "https://tools.example.com:443",
            "https://user:pass@tools.example.com",
            "https://user@tools.example.com",
            "ftp://tools.example.com",
            "file:///tmp",
            "tools.example.com",
            "",
        ] {
            assert_eq!(
                code(draft_site(
                    request(origin, &["/items/{num}"], &[]),
                    &mut s.config
                )),
                ErrorCode::InvalidRequest,
                "{origin}"
            );
        }
        // A non-default port is part of the origin.
        drafted(draft_site(
            request("http://localhost:8080", &["/items/{num}"], &[]),
            &mut s.config,
        ));
    }

    #[test]
    fn draft_site_refuses_invalid_shapes_and_raw_urls() {
        let mut s = setup("");
        for shape in [
            "https://tools.example.com/items/42",
            "//tools.example.com/items/{num}",
            "/items/42",
            "/items/Release-Plan",
            "/Items/{num}",
            "/items/{nope}",
            "/items/{num}?q=hello",
            "items/{num}",
            "",
        ] {
            assert_eq!(
                code(draft_site(request(TOOLS, &[shape], &[]), &mut s.config)),
                ErrorCode::InvalidRequest,
                "visited {shape}"
            );
            assert_eq!(
                code(draft_site(
                    request(TOOLS, &["/items/{num}"], &[shape]),
                    &mut s.config
                )),
                ErrorCode::InvalidRequest,
                "link {shape}"
            );
        }
        for count in [0, 1_000_001] {
            let mut bad = request(TOOLS, &["/items/{num}"], &[]);
            bad.visited[0].count = count;
            assert_eq!(
                code(draft_site(bad, &mut s.config)),
                ErrorCode::InvalidRequest
            );
        }
        let many: Vec<String> = (0..51)
            .map(|i| format!("/items/{{num}}/{}", word(i)))
            .collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            code(draft_site(request(TOOLS, &many[..22], &[]), &mut s.config)),
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            code(draft_site(request(TOOLS, &[], &many), &mut s.config)),
            ErrorCode::InvalidRequest
        );
        drafted(draft_site(
            request(TOOLS, &many[..21], &many[..50]),
            &mut s.config,
        ));
    }

    #[test]
    fn draft_site_refuses_while_adapter_policy_needs_repair() {
        let mut s = setup("[adapters]\nformat = 2\n");
        assert!(s.config.adapter_issue().is_some());
        assert_eq!(
            code(draft_site(
                request(TOOLS, &["/items/{num}"], &[]),
                &mut s.config
            )),
            ErrorCode::InvalidConfig
        );
        let revision = s.config.revision().to_owned();
        let table = json!({
            "type": "tools.items",
            "origin": TOOLS,
            "match": [{"path": "/items/{num:int}"}],
            "id": "tools.items:{host}/{num}"
        });
        assert_eq!(
            code(check_site_draft(
                check(
                    TOOLS,
                    &revision,
                    vec![table],
                    vec![vec![RuleProvenance::Drafted]],
                    vec![None]
                ),
                &mut s.config
            )),
            ErrorCode::InvalidConfig
        );

        // A config.toml that does not parse refuses too.
        fs::write(&s.path, "not = [valid").unwrap();
        assert_eq!(
            code(draft_site(
                request(TOOLS, &["/items/{num}"], &[]),
                &mut s.config
            )),
            ErrorCode::InvalidConfig
        );
    }

    #[test]
    fn draft_site_counts_recorded_contract_types_as_taken() {
        // A removed adapter's contract still names note keys, so its type
        // stays taken even on another origin.
        let mut s = setup(
            "[[adapter_id_history]]\ntype = \"tools.items\"\norigin = \"https://old.example\"\nmatch = [{ path = \"/items/{id:int}\" }]\nid = \"tools.items:{host}/{id}\"\n",
        );
        let site = drafted(draft_site(
            request(TOOLS, &["/items/{num}"], &[]),
            &mut s.config,
        ));
        assert_eq!(site.adapters.len(), 1, "{site:?}");
        assert_eq!(site.adapters[0].adapter["type"], "tools.items2");
        assert_eq!(site.adapters[0].adapter["id"], "tools.items2:{host}/{num}");
        assert!(site.taken_types.iter().any(|t| t == "tools.items"));
        for starter in &catalog().starter_types {
            assert!(site.taken_types.contains(starter), "{starter}");
        }
    }

    const TOOLS_ISSUES_HISTORY: &str = "[[adapter_id_history]]\ntype = \"tools.issues\"\norigin = \"https://tools.example.com\"\nmatch = [{ path = \"/issues/{id:int}\" }]\nid = \"tools.issues:{host}/{id}\"\n";

    /// Saves every drafted adapter, all rules as drafted, through the
    /// revision-checked config update the panel's save ends with.
    fn save_drafted(s: &mut Setup, site: &SiteDrafted) {
        let mut next = s.config.snapshot().clone();
        next.adapters
            .artifacts
            .extend(site.adapters.iter().map(|a| a.adapter.clone()));
        let revision = s.config.revision().to_owned();
        s.config
            .update(next, &revision)
            .expect("the drafted adapters save")
            .expect("no conflict");
    }

    /// The user removed a site, then captures it again: the retired
    /// contract still names its artifacts, so its routes are already
    /// captured instead of re-drafted under a new type the save refuses.
    #[test]
    fn draft_site_lists_a_retired_adapters_routes_as_already_captured() {
        let mut s = setup(TOOLS_ISSUES_HISTORY);
        let site = drafted(draft_site(
            request(TOOLS, &["/issues/{num}", "/docs/{num}"], &["/issues/{num}"]),
            &mut s.config,
        ));
        assert_eq!(site.remapped, 0);
        let types: Vec<&Json> = site.adapters.iter().map(|a| &a.adapter["type"]).collect();
        assert_eq!(types, [&json!("tools.docs")], "{site:#?}");
        assert!(
            site.dropped
                .iter()
                .any(|d| d.shape.as_deref() == Some("/issues/{num}")
                    && d.reason == DraftDropReason::AlreadyRecognized),
            "{:?}",
            site.dropped
        );
        // The re-check judges a rule over the retired routes as overlapping.
        let revision = site.revision.clone();
        let issues = json!({
            "type": "tools.issues2",
            "origin": TOOLS,
            "match": [{"path": "/issues/{num:int}"}, {"path": "/tasks/{num:int}"}],
            "id": "tools.issues2:{host}/{num}"
        });
        let response = checked(check_site_draft(
            check(
                TOOLS,
                &revision,
                vec![issues.clone()],
                vec![drafted_rules(&issues)],
                vec![None],
            ),
            &mut s.config,
        ));
        assert_eq!(
            verdicts(&response),
            [vec![
                (RuleVerdictKind::Dropped, Some("draft_overlap")),
                (RuleVerdictKind::Kept, None)
            ]]
        );
        // What the review offers saves.
        save_drafted(&mut s, &site);
    }

    /// A contract removed from config.toml by hand is recorded in the
    /// host's adopted-contract state and counts the same way.
    #[test]
    fn draft_site_lists_a_hand_removed_adapters_routes_as_already_captured() {
        let mut s = setup(&adapters(&[TOOLS_ITEMS]));
        {
            let _lock = s.config.lock_current().unwrap();
            s.config.refresh().unwrap();
        }
        let root = s.folder.path().join("notes");
        fixtures::write(&s.path, &root, "", "", "");
        {
            let _lock = s.config.lock_current().unwrap();
            s.config.refresh().unwrap();
        }
        assert!(s.config.snapshot().adapters.artifacts.is_empty());
        let site = drafted(draft_site(
            request(TOOLS, &["/items/{num}", "/docs/{num}"], &[]),
            &mut s.config,
        ));
        assert_eq!(site.remapped, 0);
        let types: Vec<&Json> = site.adapters.iter().map(|a| &a.adapter["type"]).collect();
        assert_eq!(types, [&json!("tools.docs")], "{site:#?}");
        assert!(
            site.dropped
                .iter()
                .any(|d| d.shape.as_deref() == Some("/items/{num}")
                    && d.reason == DraftDropReason::AlreadyRecognized),
            "{:?}",
            site.dropped
        );
        save_drafted(&mut s, &site);
    }

    /// Every drafted adapter crosses the wire as JSON and passes the
    /// re-check unchanged, including a shape that repeats a query name with
    /// two classes and one that carries a token.
    #[test]
    fn every_drafted_adapter_round_trips_through_json_and_the_re_check() {
        let mut s = setup("");
        let visited = [
            "/view?id={id}&id={num}",
            "/doc/{num}?rev={id}&rev={num}",
            "/files/{num}?token={id}",
            "/items/{num}",
            "/wiki/pages/{uuid}?access_token={hex64}",
        ];
        let links = ["/tasks/{num}?sig={hex32}", "/boards/{key}"];
        let site = drafted(draft_site(request(TOOLS, &visited, &links), &mut s.config));
        assert!(site.adapters.len() >= 3, "{site:#?}");
        assert!(
            site.dropped
                .iter()
                .any(|d| d.shape.as_deref() == Some("/view?id={id}&id={num}")),
            "{:?}",
            site.dropped
        );
        let tables: Vec<Json> = site.adapters.iter().map(|a| a.adapter.clone()).collect();
        for table in &tables {
            let text = table.to_string();
            for secret in ["token=", "access_token", "sig=", "rev="] {
                assert!(!text.contains(secret), "{text}");
            }
        }
        let provenance = tables.iter().map(drafted_rules).collect();
        let response = checked(check_site_draft(
            check(
                TOOLS,
                &site.revision,
                tables.clone(),
                provenance,
                vec![None; tables.len()],
            ),
            &mut s.config,
        ));
        for rules in verdicts(&response) {
            assert!(
                rules.iter().all(|(v, _)| *v == RuleVerdictKind::Kept),
                "{rules:?}"
            );
        }
        save_drafted(&mut s, &site);
    }

    /// On a shared origin a blocked word at the tenant position is a route,
    /// and the review cannot scope a draft to one.
    #[test]
    fn shared_origin_blocked_words_are_never_tenants() {
        let mut s = setup("");
        let site = drafted(draft_site(
            request(
                LINEAR,
                &["/auth/callback/{id}", "/invite/accept/{hex32}"],
                &["/share/doc/{uuid}"],
            ),
            &mut s.config,
        ));
        assert!(site.adapters.is_empty(), "{site:#?}");
        assert!(
            site.dropped
                .iter()
                .all(|d| d.reason == DraftDropReason::BlockedRoute),
            "{:?}",
            site.dropped
        );
        let (revision, table) = linear_draft(&mut s);
        let response = checked(check_site_draft(
            check(
                LINEAR,
                &revision,
                vec![table.clone()],
                vec![drafted_rules(&table)],
                vec![values(&["acme", "auth"])],
            ),
            &mut s.config,
        ));
        assert_eq!(response.scope[0].resolved, None);
        assert_eq!(
            response.scope[0].reason.as_deref(),
            Some("draft_scope_choice")
        );
    }

    #[test]
    fn draft_site_preview_lists_every_shape_and_probe() {
        let mut s = setup("");
        let site = drafted(draft_site(
            request(
                TOOLS,
                &["/items/{num}", "/search?q={slug}"],
                &["/docs/{uuid}"],
            ),
            &mut s.config,
        ));
        let probes = &catalog().probe_paths;
        assert_eq!(site.preview.len(), 3 + probes.len());
        let sources: Vec<PreviewSource> = site.preview.iter().map(|r| r.source).collect();
        assert_eq!(
            &sources[..3],
            [
                PreviewSource::Visited,
                PreviewSource::Visited,
                PreviewSource::Link
            ]
        );
        assert_eq!(site.preview[0].shape, "/items/{num}");
        assert_eq!(site.preview[1].shape, "/search?q={slug}");
        assert_eq!(
            site.preview[1].url,
            "https://tools.example.com/search?q=example-page"
        );
        assert_eq!(site.preview[1].adapter, None, "a search is never drafted");
        assert_eq!(site.preview[2].shape, "/docs/{uuid}");
        assert!(site.preview[2].adapter.is_some(), "{site:?}");
        for (row, probe) in site.preview[3..].iter().zip(probes) {
            assert_eq!(row.source, PreviewSource::Probe);
            assert_eq!(&row.shape, probe);
            assert_eq!(row.url, format!("{TOOLS}{probe}"));
            assert_eq!(row.adapter, None, "{probe}");
            assert_eq!(row.artifact_id, None);
        }
        // A link-only rule starts unchecked.
        let docs = site
            .adapters
            .iter()
            .find(|a| a.adapter["type"] == "tools.docs")
            .unwrap();
        assert!(docs.rules.iter().all(|r| !r.checked && r.links == 1));
        assert!(
            site.dropped
                .iter()
                .any(|d| d.shape.as_deref() == Some("/search?q={slug}"))
        );
    }

    #[test]
    fn draft_site_reports_zero_remapped_after_overlap_removal() {
        let mut s = setup(&adapters(&[TOOLS_ITEMS]));
        let site = drafted(draft_site(
            request(TOOLS, &["/items/{num}", "/docs/{num}"], &[]),
            &mut s.config,
        ));
        assert_eq!(site.remapped, 0);
        assert!(
            site.dropped
                .iter()
                .any(|d| d.shape.as_deref() == Some("/items/{num}")
                    && d.reason == DraftDropReason::AlreadyRecognized),
            "{:?}",
            site.dropped
        );
        let types: Vec<&Json> = site.adapters.iter().map(|a| &a.adapter["type"]).collect();
        assert_eq!(types, [&json!("tools.docs")], "{site:#?}");
        // The recognized shape's preview row names no drafted rule.
        assert_eq!(site.preview[0].adapter, None);
    }

    /// Task 13: drafting additions on a configured origin lists a shape that
    /// an owner-scoped existing adapter recognizes only without scope as
    /// already captured, and drafts only the new kind of page.
    #[test]
    fn draft_site_on_a_configured_origin_lists_scoped_recognized_shapes_as_already_captured() {
        const GITHUB_ISSUES: &str = "[[artifacts]]\ntype = \"github.issue\"\norigin = \"https://github.com\"\nmatch = [{ path = \"/{owner:name|lower}/{repo:name|lower}/issues/{number:int}\" }]\nscope = { owner = [\"acme-corp\"] }\nid = \"github.issue:{host}/{owner}/{repo}/{number}\"\n";
        let mut s = setup(&adapters(&[GITHUB_ISSUES]));
        let snapshot = s.config.snapshot().clone();
        let site = drafted(draft_site(
            request(
                "https://github.com",
                &[
                    "/{slug}/{slug}/discussions/{num}",
                    "/{slug}/{slug}/issues/{num}",
                ],
                &[],
            ),
            &mut s.config,
        ));
        assert!(
            site.dropped.iter().any(
                |d| d.shape.as_deref() == Some("/{slug}/{slug}/issues/{num}")
                    && d.reason == DraftDropReason::AlreadyRecognized
            ),
            "{:?}",
            site.dropped
        );
        assert_eq!(site.adapters.len(), 1, "{site:#?}");
        let adapter = &site.adapters[0];
        assert_ne!(adapter.adapter["type"], "github.issue");
        assert_eq!(adapter.scope, DraftScopeState::Copied);
        let rules = adapter.adapter["match"].as_array().unwrap();
        assert!(
            rules
                .iter()
                .all(|rule| rule["path"].as_str().unwrap().contains("/discussions/")),
            "{rules:?}"
        );
        // The existing definition is untouched.
        assert_eq!(s.config.snapshot(), &snapshot);
    }

    #[test]
    fn draft_site_writes_nothing_to_home_or_config_dir() {
        let mut s = setup(&adapters(&[TOOLS_ITEMS]));
        // Any locked refresh records the adopted contracts and creates the
        // lock file; take one first, as an earlier request would have.
        {
            let _lock = s.config.lock_current().unwrap();
            s.config.refresh().unwrap();
        }
        let before = tree(s.folder.path());
        drafted(draft_site(
            request(TOOLS, &["/docs/{num}"], &["/boards/{num}"]),
            &mut s.config,
        ));
        let revision = s.config.revision().to_owned();
        checked(check_site_draft(
            check(
                TOOLS,
                &revision,
                vec![json!({
                    "type": "tools.docs",
                    "origin": TOOLS,
                    "match": [{"path": "/docs/{num:int}"}],
                    "id": "tools.docs:{host}/{num}"
                })],
                vec![vec![RuleProvenance::Drafted]],
                vec![None],
            ),
            &mut s.config,
        ));
        assert_eq!(tree(s.folder.path()), before);
    }

    /// A distinct 20-letter route word for `i`.
    fn word(i: usize) -> String {
        let mut n = i;
        let mut out = String::new();
        for _ in 0..20 {
            out.push(char::from(b'a' + (n % 26) as u8));
            n /= 26;
        }
        out
    }

    /// The longest shape the shaper can build: 32 segments of 20-letter
    /// route words ending in a UUID, and 8 query pairs.
    fn long_shape(i: usize) -> String {
        let mut shape = String::new();
        for _ in 0..31 {
            shape.push('/');
            shape.push_str(&word(i));
        }
        shape.push_str("/{uuid}?");
        let pairs: Vec<String> = (0..8)
            .map(|p| format!("{}={{uuid}}", word(i * 8 + p)))
            .collect();
        shape.push_str(&pairs.join("&"));
        shape
    }

    #[test]
    fn draft_site_response_fits_outbound_cap_at_limits() {
        let mut s = setup("");
        let origin = format!("https://{}.example", vec!["a".repeat(63); 30].join("."));
        assert!(super::exact_origin(&origin), "{}", origin.len());
        let shapes: Vec<String> = (0..71).map(long_shape).collect();
        let shapes: Vec<&str> = shapes.iter().map(String::as_str).collect();
        let site = draft_site(
            request(&origin, &shapes[..21], &shapes[21..]),
            &mut s.config,
        );
        let size = serde_json::to_vec(&site).unwrap().len();
        let site = drafted(site);
        assert_eq!(site.preview.len(), 71 + catalog().probe_paths.len());
        assert!(size < MAX_OUTBOUND_BYTES, "{size}");

        // A re-check at the limits: 8 adapters of 16 rules, every shape.
        let tables: Vec<Json> = (0..8)
            .map(|a| {
                let rules: Vec<Json> = (0..16)
                    .map(|r| json!({"path": format!("/{}/{}/{{n:int}}", word(a), word(100 + r))}))
                    .collect();
                json!({
                    "type": format!("long.k{}", word(a)),
                    "origin": origin,
                    "match": rules,
                    "id": format!("long.k{}:{{host}}/{{n}}", word(a))
                })
            })
            .collect();
        let mut request = check(
            &origin,
            &site.revision,
            tables,
            vec![vec![RuleProvenance::UserWritten; 16]; 8],
            vec![None; 8],
        );
        request.visited = counts(&shapes[..21]);
        request.links = counts(&shapes[21..]);
        let response = check_site_draft(request, &mut s.config);
        let size = serde_json::to_vec(&response).unwrap().len();
        let response = checked(response);
        assert_eq!(response.rules.len(), 8);
        assert!(response.rules.iter().all(|r| r.len() == 16));
        assert!(size < MAX_OUTBOUND_BYTES, "{size}");
    }

    #[test]
    fn check_site_draft_rejects_drafted_rule_that_now_overlaps() {
        let mut s = setup("");
        let site = drafted(draft_site(
            request(TOOLS, &["/items/{num}"], &[]),
            &mut s.config,
        ));
        let table = site.adapters[0].adapter.clone();
        // An adapter for the same pages is saved before the review saves.
        fs::write(
            &s.path,
            format!(
                "{}\n{}",
                fs::read_to_string(&s.path).unwrap(),
                adapters(&[TOOLS_ITEMS])
            ),
        )
        .unwrap();
        s.config.refresh().unwrap();
        let revision = s.config.revision().to_owned();
        assert_ne!(revision, site.revision);
        let response = checked(check_site_draft(
            check(
                TOOLS,
                &revision,
                vec![table.clone()],
                vec![drafted_rules(&table)],
                vec![None],
            ),
            &mut s.config,
        ));
        assert_eq!(
            verdicts(&response),
            [vec![(RuleVerdictKind::Dropped, Some("draft_overlap"))]]
        );
        assert_eq!(response.revision, revision);
    }

    #[test]
    fn check_site_draft_user_written_rule_only_warns() {
        let mut s = setup(&adapters(&[TOOLS_ITEMS]));
        let revision = s.config.revision().to_owned();
        let table = json!({
            "type": "tools.things",
            "origin": TOOLS,
            "match": [{"path": "/items/{n:int}"}, {"path": "/things/{n:int}"}],
            "id": "tools.things:{host}/{n}"
        });
        let response = checked(check_site_draft(
            check(
                TOOLS,
                &revision,
                vec![table.clone()],
                vec![vec![RuleProvenance::UserWritten, RuleProvenance::Drafted]],
                vec![None],
            ),
            &mut s.config,
        ));
        assert_eq!(
            verdicts(&response),
            [vec![
                (RuleVerdictKind::Warned, Some("draft_overlap")),
                (RuleVerdictKind::Kept, None)
            ]]
        );
        let response = checked(check_site_draft(
            check(
                TOOLS,
                &revision,
                vec![table.clone()],
                vec![drafted_rules(&table)],
                vec![None],
            ),
            &mut s.config,
        ));
        assert_eq!(
            verdicts(&response),
            [vec![
                (RuleVerdictKind::Dropped, Some("draft_overlap")),
                (RuleVerdictKind::Kept, None)
            ]]
        );
        // The preview shows only surviving rules, by their table index.
        let response = checked(check_site_draft(
            CheckSiteDraftRequest {
                visited: counts(&["/items/{num}", "/things/{num}"]),
                ..check(
                    TOOLS,
                    &revision,
                    vec![table.clone()],
                    vec![drafted_rules(&table)],
                    vec![None],
                )
            },
            &mut s.config,
        ));
        assert_eq!(response.preview[0].adapter, None);
        assert_eq!(response.preview[1].adapter, Some(0));
        assert_eq!(response.preview[1].rule, Some(1));
        assert_eq!(
            response.preview[1].artifact_id.as_deref(),
            Some("tools.things:tools.example.com/123456")
        );
    }

    #[test]
    fn check_site_draft_stale_revision_is_conflict() {
        let mut s = setup("");
        let site = drafted(draft_site(
            request(TOOLS, &["/items/{num}"], &[]),
            &mut s.config,
        ));
        let table = site.adapters[0].adapter.clone();
        assert_eq!(
            code(check_site_draft(
                check(
                    TOOLS,
                    "stale",
                    vec![table.clone()],
                    vec![drafted_rules(&table)],
                    vec![None]
                ),
                &mut s.config,
            )),
            ErrorCode::Conflict
        );
        // The draft's own revision still checks.
        checked(check_site_draft(
            check(
                TOOLS,
                &site.revision,
                vec![table.clone()],
                vec![drafted_rules(&table)],
                vec![None],
            ),
            &mut s.config,
        ));
    }

    #[test]
    fn check_site_draft_refuses_requests_outside_its_bounds() {
        let mut s = setup("");
        let revision = s.config.revision().to_owned();
        let table = json!({
            "type": "tools.items",
            "origin": TOOLS,
            "match": [{"path": "/items/{n:int}"}],
            "id": "tools.items:{host}/{n}"
        });
        let other_origin = json!({
            "type": "tools.items",
            "origin": "https://other.example",
            "match": [{"path": "/items/{n:int}"}],
            "id": "tools.items:{host}/{n}"
        });
        let one = || vec![vec![RuleProvenance::Drafted]];
        let too_many_values: Vec<String> = (0..65).map(word).collect();
        let too_many_values: Vec<&str> = too_many_values.iter().map(String::as_str).collect();
        for request in [
            check(TOOLS, &revision, vec![table.clone()], vec![], vec![None]),
            check(TOOLS, &revision, vec![table.clone()], one(), vec![]),
            check(
                TOOLS,
                &revision,
                vec![table.clone()],
                vec![vec![RuleProvenance::Drafted; 2]],
                vec![None],
            ),
            check(TOOLS, &revision, vec![other_origin], one(), vec![None]),
            check(TOOLS, &revision, vec![json!("table")], one(), vec![None]),
            check(
                TOOLS,
                &revision,
                vec![table.clone()],
                one(),
                vec![values(&[])],
            ),
            check(
                TOOLS,
                &revision,
                vec![table.clone()],
                one(),
                vec![values(&[&"a".repeat(129)])],
            ),
            check(
                TOOLS,
                &revision,
                vec![table.clone()],
                one(),
                vec![values(&too_many_values)],
            ),
            check(
                TOOLS,
                &revision,
                vec![table.clone(); 9],
                vec![vec![RuleProvenance::Drafted]; 9],
                vec![None; 9],
            ),
            check(
                "https://tools.example.com/",
                &revision,
                vec![table.clone()],
                one(),
                vec![None],
            ),
            CheckSiteDraftRequest {
                visited: counts(&["https://tools.example.com/items/1"]),
                ..check(TOOLS, &revision, vec![table.clone()], one(), vec![None])
            },
        ] {
            assert_eq!(
                code(check_site_draft(request.clone(), &mut s.config)),
                ErrorCode::InvalidRequest,
                "{request:?}"
            );
        }
    }

    /// Linear's tenant is the first segment; the draft has no agreeing scope
    /// to copy, so the review chooses it.
    fn linear_draft(s: &mut Setup) -> (String, Json) {
        let site = drafted(draft_site(
            request(LINEAR, &["/acme/issue/{key}"], &[]),
            &mut s.config,
        ));
        assert_eq!(site.adapters.len(), 1, "{site:?}");
        let adapter = &site.adapters[0];
        assert_eq!(adapter.scope, DraftScopeState::Pending);
        assert_eq!(
            adapter.tenant.as_ref().map(|t| t.capture.as_str()),
            Some("workspace")
        );
        assert!(adapter.adapter.get("scope").is_none());
        assert!(adapter.adapter.get("unscoped").is_none());
        (site.revision, adapter.adapter.clone())
    }

    #[test]
    fn check_site_draft_accepts_pending_scope_with_chosen_scope() {
        let mut s = setup("");
        let (revision, table) = linear_draft(&mut s);
        let response = checked(check_site_draft(
            CheckSiteDraftRequest {
                visited: counts(&["/acme/issue/{key}"]),
                ..check(
                    LINEAR,
                    &revision,
                    vec![table.clone()],
                    vec![drafted_rules(&table)],
                    vec![values(&["acme"])],
                )
            },
            &mut s.config,
        ));
        assert!(
            response.rules[0]
                .iter()
                .all(|r| r.verdict == RuleVerdictKind::Kept),
            "{response:?}"
        );
        let scope = &response.scope[0];
        assert_eq!(scope.state, DraftScopeState::Pending);
        assert_eq!(scope.resolved, Some(ResolvedScopeKind::Scoped));
        assert_eq!(scope.reason, None);
        // The preview matches without scope.
        assert_eq!(response.preview[0].adapter, Some(0));

        // A value that is not a canonical tenant name is refused.
        let response = checked(check_site_draft(
            check(
                LINEAR,
                &revision,
                vec![table.clone()],
                vec![drafted_rules(&table)],
                vec![values(&["Acme Corp"])],
            ),
            &mut s.config,
        ));
        assert_eq!(response.scope[0].resolved, None);
        assert_eq!(
            response.scope[0].reason.as_deref(),
            Some("draft_scope_choice")
        );
        assert!(
            response.rules[0]
                .iter()
                .all(|r| r.verdict == RuleVerdictKind::Kept),
            "rule verdicts never depend on the scope choice"
        );
    }

    #[test]
    fn notion_workspace_pages_draft_with_the_tenant_as_anchor_and_refuse_unscoped() {
        let mut s = setup("");
        let site = drafted(draft_site(
            request(NOTION, &["/acme/{tailhex32}", "/acme/{hex32}"], &[]),
            &mut s.config,
        ));
        assert_eq!(site.adapters.len(), 1, "{site:?}");
        let adapter = &site.adapters[0];
        assert_eq!(adapter.scope, DraftScopeState::Pending);
        assert_eq!(adapter.adapter["type"], json!("notion.page"));
        assert_eq!(
            adapter.adapter["match"],
            json!([{"path": "/{workspace:name|lower}/{hex:hex(32)@tail}"}])
        );
        let table = adapter.adapter.clone();
        let run = |s: &mut Setup, choice: Option<ScopeChoiceView>| {
            checked(check_site_draft(
                check(
                    NOTION,
                    &site.revision,
                    vec![table.clone()],
                    vec![drafted_rules(&table)],
                    vec![choice],
                ),
                &mut s.config,
            ))
        };
        let scoped = run(&mut s, values(&["acme"]));
        assert!(
            scoped.rules[0]
                .iter()
                .all(|r| r.verdict == RuleVerdictKind::Kept),
            "{scoped:?}"
        );
        assert_eq!(scoped.scope[0].resolved, Some(ResolvedScopeKind::Scoped));
        // The workspace is the rule's only anchor, so it cannot be unscoped.
        let unscoped = run(
            &mut s,
            Some(ScopeChoiceView::UnscopedChoice(UnscopedChoice::Unscoped)),
        );
        assert_eq!(unscoped.scope[0].resolved, None);
        assert_eq!(
            unscoped.scope[0].reason.as_deref(),
            Some("draft_scope_choice")
        );
        assert!(
            unscoped.rules[0]
                .iter()
                .all(|r| r.verdict == RuleVerdictKind::Kept),
            "rule verdicts never depend on the scope choice"
        );
    }

    #[test]
    fn check_site_draft_accepts_unscoped_choice_on_unscoped_only_origin() {
        let mut s = setup("");
        let site = drafted(draft_site(
            request(FIGMA, &["/file/{id}/{slug}", "/file/{id}"], &[]),
            &mut s.config,
        ));
        assert_eq!(site.adapters.len(), 1, "{site:?}");
        assert_eq!(site.adapters[0].scope, DraftScopeState::UnscopedOnly);
        let table = site.adapters[0].adapter.clone();
        let run = |s: &mut Setup, choice: Option<ScopeChoiceView>| {
            checked(check_site_draft(
                check(
                    FIGMA,
                    &site.revision,
                    vec![table.clone()],
                    vec![drafted_rules(&table)],
                    vec![choice],
                ),
                &mut s.config,
            ))
            .scope
            .remove(0)
        };
        let unscoped = run(
            &mut s,
            Some(ScopeChoiceView::UnscopedChoice(UnscopedChoice::Unscoped)),
        );
        assert_eq!(unscoped.state, DraftScopeState::UnscopedOnly);
        assert_eq!(unscoped.resolved, Some(ResolvedScopeKind::Unscoped));
        assert_eq!(unscoped.reason, None);
        for choice in [None, values(&["acme"])] {
            let refused = run(&mut s, choice);
            assert_eq!(refused.resolved, None);
            assert_eq!(refused.reason.as_deref(), Some("draft_scope_choice"));
        }
    }

    #[test]
    fn check_site_draft_refuses_pending_scope_without_a_choice() {
        let mut s = setup("");
        let (revision, table) = linear_draft(&mut s);
        let response = checked(check_site_draft(
            check(
                LINEAR,
                &revision,
                vec![table.clone()],
                vec![drafted_rules(&table)],
                vec![None],
            ),
            &mut s.config,
        ));
        assert_eq!(response.scope[0].state, DraftScopeState::Pending);
        assert_eq!(response.scope[0].resolved, None);
        assert_eq!(
            response.scope[0].reason.as_deref(),
            Some("draft_scope_pending")
        );
        // Not-shared scope accepts no choice.
        let site = drafted(draft_site(
            request(TOOLS, &["/items/{num}"], &[]),
            &mut s.config,
        ));
        let tools = site.adapters[0].adapter.clone();
        let response = checked(check_site_draft(
            check(
                TOOLS,
                &site.revision,
                vec![tools.clone()],
                vec![drafted_rules(&tools)],
                vec![values(&["acme"])],
            ),
            &mut s.config,
        ));
        assert_eq!(response.scope[0].state, DraftScopeState::NotShared);
        assert_eq!(
            response.scope[0].reason.as_deref(),
            Some("draft_scope_choice")
        );
    }

    #[test]
    fn check_site_draft_still_refuses_a_table_carrying_scope_or_title() {
        let mut s = setup("");
        let (revision, table) = linear_draft(&mut s);
        for (key, value) in [
            ("scope", json!({"workspace": ["acme"]})),
            ("unscoped", json!("acknowledged")),
            ("title", json!([{"template": "{title} · Linear"}])),
        ] {
            let mut carrying = table.clone();
            carrying
                .as_object_mut()
                .unwrap()
                .insert(key.to_owned(), value);
            let response = checked(check_site_draft(
                check(
                    LINEAR,
                    &revision,
                    vec![carrying.clone()],
                    vec![drafted_rules(&carrying)],
                    vec![values(&["acme"])],
                ),
                &mut s.config,
            ));
            assert!(
                response.rules[0]
                    .iter()
                    .all(|r| r.verdict == RuleVerdictKind::Dropped
                        && r.reason.as_deref() == Some("draft_forbidden_key")),
                "{key}: {response:?}"
            );
        }
    }
}

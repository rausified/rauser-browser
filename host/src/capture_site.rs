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
#[cfg(unix)]
use brauser_protocol::{
    GetRunRequest, RecordKey, RecordKeyKind, Request, RunKind, RunState, StartRunRequest,
    StartSiteDraftRequest,
};
use serde::Deserialize;

use crate::config::{ConfigLock, ConfigStore, NoteKeyRule};
use crate::native::error;
#[cfg(unix)]
use crate::service::client::ServiceLink;

/// Where there is no Brauser service (Windows), the only service link is
/// none: run messages answer `service_unavailable`.
#[cfg(not(unix))]
pub enum ServiceLink {}

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
pub(crate) fn exact_origin(origin: &str) -> bool {
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

/// Check a starter review without exporting host-owned identity history.
/// The save rechecks this independently; this message never writes a policy.
pub fn check_capture_site(
    value: brauser_protocol::CheckCaptureSiteRequest,
    config: &mut ConfigStore,
) -> Response {
    let id = value.request_id.as_str();
    let _lock = match locked_document(config) {
        Ok(lock) => lock,
        Err((code, message)) => return error(id, code, message),
    };
    if config.revision() != value.expected_revision {
        return error(
            id,
            ErrorCode::Conflict,
            "Settings changed. Open Capture Site again.",
        );
    }
    if let Err(reason) = config.check_capture_site(&value.adapters) {
        return error(id, ErrorCode::InvalidConfig, &reason.to_string());
    }
    Response::CaptureSiteChecked(brauser_protocol::CaptureSiteChecked {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id,
        revision: config.revision().to_owned(),
    })
}

/// The refusal while the adopted-contract record cannot vouch for
/// `origin` (`ConfigStore::adopted_contracts_unknown_for`): drafting against
/// an incomplete set of recorded contracts would offer rules the save
/// refuses.
fn adopted_record_refusal(config: &ConfigStore, id: &str, origin: &str) -> Option<Response> {
    config.adopted_contracts_unknown_for(origin).then(|| {
        error(
            id,
            ErrorCode::InvalidConfig,
            "Brauser's record of the adapters it has used cannot be read for this site, so a draft cannot be checked against adapters removed from config.toml by hand. Choose Forget recorded adapters in Brauser settings, or fix the file, then draft again.",
        )
    })
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
                } else if rule.unseen {
                    Some(UncheckedReason::Unseen)
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

/// A request's refusal, boxed: a `Response` is large.
type Refusal = Box<Response>;

/// Checks what every drafting request carries: an exact origin and
/// shapes within their bounds. `Err` is the request's refusal.
fn request_input(
    id: &str,
    origin: &str,
    visited: &[ShapeCount],
    links: &[ShapeCount],
) -> Result<ShapeInput, Refusal> {
    if !exact_origin(origin) {
        return Err(Box::new(error(
            id,
            ErrorCode::InvalidRequest,
            "origin must be an exact HTTP(S) origin",
        )));
    }
    shape_input(visited, links).ok_or_else(|| {
        Box::new(error(
            id,
            ErrorCode::InvalidRequest,
            "shapes are invalid or over their bounds",
        ))
    })
}

/// Takes the config lock for drafting `origin` and applies the refusals
/// every draft shares (`locked_document`, `adopted_record_refusal`).
fn lock_for_drafting(
    config: &mut ConfigStore,
    id: &str,
    origin: &str,
) -> Result<ConfigLock, Refusal> {
    let lock =
        locked_document(config).map_err(|(code, message)| Box::new(error(id, code, message)))?;
    if let Some(refusal) = adopted_record_refusal(config, id, origin) {
        return Err(Box::new(refusal));
    }
    Ok(lock)
}

fn policy_unavailable(id: &str) -> Response {
    error(
        id,
        ErrorCode::InvalidConfig,
        "adapter policy is unavailable",
    )
}

/// Drafts under the config lock with `judge` against the current adapter
/// document, the recorded contracts and the taken types, then builds the
/// response from what it returns.
fn judged_response(
    request_id: String,
    origin: &str,
    input: &ShapeInput,
    config: &mut ConfigStore,
    drafter: SiteDrafter,
    judge: impl FnOnce(&Document, &[Document], &[String]) -> structural::SiteDraft,
) -> Response {
    let _lock = match lock_for_drafting(config, &request_id, origin) {
        Ok(lock) => lock,
        Err(refusal) => return *refusal,
    };
    let Ok(existing) = config.adapter_document() else {
        return policy_unavailable(&request_id);
    };
    let taken = taken_types(config, existing);
    let draft = judge(existing, config.recorded_contract_documents(), &taken);
    site_drafted_response(
        config, existing, origin, input, request_id, drafter, &draft, taken,
    )
}

/// The structural draft's response (`draft_site` without a `run_id`).
fn structural_response(
    value: DraftSiteRequest,
    input: &ShapeInput,
    config: &mut ConfigStore,
) -> Response {
    let origin = value.origin;
    judged_response(
        value.request_id,
        &origin,
        input,
        config,
        SiteDrafter::Structural,
        |existing, recorded, taken| {
            structural::draft_site(&origin, existing, recorded, input, catalog(), taken)
        },
    )
}

/// The `site_drafted` response for `draft`, structural or agent, with its
/// match preview, under the remap invariant. Call with the config lock held.
#[allow(clippy::too_many_arguments)]
fn site_drafted_response(
    config: &ConfigStore,
    existing: &Document,
    origin: &str,
    input: &ShapeInput,
    request_id: String,
    drafter: SiteDrafter,
    draft: &structural::SiteDraft,
    taken: Vec<String>,
) -> Response {
    let previewed: Vec<Option<PreviewAdapter>> = draft
        .adapters
        .iter()
        .map(|a| preview_adapter(&a.adapter, a.rules.iter().map(|r| r.rule).collect()))
        .collect();
    let rows = preview(origin, input, &previewed);
    let remapped = remapped(config, existing, &rows);
    // The remap invariant (DESIGN §5.1): a drafted rule never re-keys an
    // artifact the current policy or a recorded contract already names.
    if remapped != 0 {
        return error(
            &request_id,
            ErrorCode::Internal,
            "the draft would move already captured pages to a new kind of page",
        );
    }
    let index = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
    // An agent draft refused whole is one row naming no shape, adapter or
    // rule. A dropped agent rule names the agent's table and rule (the
    // index in the run's answer, not in `adapters`), or no rule when its
    // whole table was refused.
    let refused = draft.rejected.map(|code| DroppedView {
        shape: None,
        adapter: None,
        rule: None,
        reason: draft_error_reason(code),
    });
    let shapes = draft.dropped.iter().map(|d| DroppedView {
        shape: Some(d.shape.clone()),
        adapter: None,
        rule: None,
        reason: drop_reason(d.reason),
    });
    let rules = draft.dropped_rules.iter().map(|d| DroppedView {
        shape: None,
        adapter: Some(index(d.table)),
        rule: d.rule.map(index),
        reason: draft_error_reason(d.reason),
    });
    Response::SiteDrafted(SiteDrafted {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        revision: config.revision().to_owned(),
        drafter,
        adapters: draft.adapters.iter().map(adapter_view).collect(),
        dropped: refused
            .into_iter()
            .chain(shapes)
            .chain(rules)
            .take(MAX_DROPPED_ROWS)
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

/// The length of a service run id (a hyphenated UUID).
const RUN_ID_CHARS: usize = 36;

/// `draft_site`: the draft for one exact origin from private URL shapes,
/// bound to the config revision it was drafted against (DESIGN §5.1). It
/// reads the config and writes nothing.
///
/// Without a `run_id` it is the structural draft. With one, it judges that
/// site-draft run's answer: the tables the service validated, fetched from
/// the service, never from the extension, and held to the same per-rule
/// checks as a structural draft (`structural::check_agent_draft`).
pub fn draft_site(
    value: DraftSiteRequest,
    config: &mut ConfigStore,
    service: Option<&ServiceLink>,
) -> Response {
    let id = value.request_id.as_str();
    let input = match request_input(id, &value.origin, &value.visited, &value.links) {
        Ok(input) => input,
        Err(refusal) => return *refusal,
    };
    let Some(run_id) = value.run_id.clone() else {
        return structural_response(value, &input, config);
    };
    if run_id.chars().count() != RUN_ID_CHARS {
        return error(id, ErrorCode::InvalidRequest, "run_id is not a run id");
    }
    // Fetched before the config lock is taken: the service is not asked
    // anything while the lock is held.
    let answer = match agent_answer(id, &run_id, &value.origin, service) {
        Ok(answer) => answer,
        Err(refusal) => return *refusal,
    };
    let tables: Option<Vec<AdapterValue>> = answer
        .into_iter()
        .map(|table| serde_json::from_value(table).ok())
        .collect();
    let origin = value.origin;
    judged_response(
        value.request_id,
        &origin,
        &input,
        config,
        SiteDrafter::Agent,
        |existing, recorded, taken| match tables {
            Some(tables) => structural::check_agent_draft(
                &origin,
                existing,
                recorded,
                &input,
                catalog(),
                taken,
                &tables,
            ),
            // A table that does not read as an adapter-templates value is a
            // table that does not parse: the whole draft is refused.
            None => structural::SiteDraft {
                rejected: Some(DraftErrorCode::InvalidRule),
                ..structural::SiteDraft::default()
            },
        },
    )
}

/// The validated tables of the site-draft run `run_id` for `origin`, from
/// the service. A run of another kind, origin or id, an unfinished run
/// (`conflict`), or a run that ended without tables is refused.
#[cfg(unix)]
fn agent_answer(
    id: &str,
    run_id: &str,
    origin: &str,
    service: Option<&ServiceLink>,
) -> Result<Vec<serde_json::Value>, Refusal> {
    let request = Request::GetRun(GetRunRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: id.to_owned(),
        run_id: run_id.to_owned(),
        since_offset: 0,
    });
    // The stream page is the agent's progress text; only the validated
    // tables are judged.
    let run = match crate::native::forward(&request, service) {
        Response::RunFetched(fetched) => fetched.run,
        refusal @ Response::Error(_) => return Err(Box::new(refusal)),
        _ => {
            return Err(Box::new(error(
                id,
                ErrorCode::Internal,
                "The Brauser service sent an unexpected reply.",
            )));
        }
    };
    if run.run_id != run_id
        || run.kind != Some(RunKind::SiteDraft)
        || run.url != origin
        || run.record_key.kind != RecordKeyKind::Url
        || run.record_key.value != origin
    {
        return Err(Box::new(error(
            id,
            ErrorCode::InvalidRequest,
            "that run is not an AI draft for this site",
        )));
    }
    match (run.state, run.site_draft) {
        (RunState::Succeeded, Some(answer)) => Ok(answer.adapters),
        (RunState::Queued | RunState::Running, _) => Err(Box::new(error(
            id,
            ErrorCode::Conflict,
            "the AI draft is not ready",
        ))),
        _ => Err(Box::new(error(
            id,
            ErrorCode::InvalidRequest,
            "that run ended without an AI draft",
        ))),
    }
}

#[cfg(not(unix))]
fn agent_answer(
    id: &str,
    _run_id: &str,
    _origin: &str,
    _service: Option<&ServiceLink>,
) -> Result<Vec<serde_json::Value>, Refusal> {
    Err(Box::new(error(
        id,
        ErrorCode::ServiceUnavailable,
        "The Brauser service is not supported on Windows.",
    )))
}

/// `start_site_draft`: starts, or joins, the agent drafter's run for one
/// exact origin. Only the origin and shapes come from the extension: the
/// host re-runs the structural drafter on its own config and builds the
/// agent's input from that draft, then forwards a `start_run` whose kind,
/// title, content and record key are all the host's.
#[cfg(unix)]
pub fn start_site_draft(
    value: StartSiteDraftRequest,
    config: &mut ConfigStore,
    service: Option<&ServiceLink>,
) -> Response {
    let id = value.request_id.as_str();
    let input = match request_input(id, &value.origin, &value.visited, &value.links) {
        Ok(input) => input,
        Err(refusal) => return *refusal,
    };
    if crate::capture::parse_event_time(&value.requested_at)
        .and_then(crate::capture::check_event_window)
        .is_err()
    {
        return error(
            id,
            ErrorCode::InvalidRequest,
            "requested_at must be a current RFC 3339 timestamp",
        );
    }
    // Drafted under the lock, which is released before forwarding.
    let structural: Vec<serde_json::Value> = {
        let _lock = match lock_for_drafting(config, id, &value.origin) {
            Ok(lock) => lock,
            Err(refusal) => return *refusal,
        };
        let Ok(existing) = config.adapter_document() else {
            return policy_unavailable(id);
        };
        let taken = taken_types(config, existing);
        let draft = structural::draft_site(
            &value.origin,
            existing,
            config.recorded_contract_documents(),
            &input,
            catalog(),
            &taken,
        );
        draft.adapters.iter().map(|a| to_json(&a.adapter)).collect()
    };
    let pairs = |list: &[ShapeCount]| -> Vec<(String, u32)> {
        list.iter()
            .map(|entry| (entry.shape.clone(), entry.count))
            .collect()
    };
    let content = crate::service::prompt::site_draft_input(
        &value.origin,
        &pairs(&value.visited),
        &pairs(&value.links),
        &structural,
    );
    let host = url::Url::parse(&value.origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    let title = format!("Capture Site: {host}");
    if let Err(message) =
        crate::service::record::check_start_run(&value.origin, &title, &content, None)
    {
        return error(id, ErrorCode::InvalidRequest, message);
    }
    let request = Request::StartRun(StartRunRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id.clone(),
        url: value.origin.clone(),
        title,
        content,
        selection: None,
        harness: value.harness,
        record_key: Some(RecordKey {
            kind: RecordKeyKind::Url,
            value: value.origin,
        }),
        requested_at: value.requested_at,
        kind: Some(RunKind::SiteDraft),
    });
    crate::native::forward(&request, service)
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
    if let Some(refusal) = adopted_record_refusal(config, id, &value.origin) {
        return refusal;
    }
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
    use super::{catalog, check_site_draft};
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

    /// `draft_site` with no service link: every structural request.
    fn draft_site(value: DraftSiteRequest, config: &mut ConfigStore) -> Response {
        super::draft_site(value, config, None)
    }

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
            run_id: None,
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
    fn draft_site_with_a_run_id_and_no_service_is_service_unavailable() {
        // Judging an agent run needs its answer from the service; drafting
        // structurally instead would show a basic draft as the AI's.
        let mut s = setup("");
        let mut with_run = request(TOOLS, &["/items/{num}"], &[]);
        with_run.run_id = Some("00000000-0000-4000-8000-000000000001".into());
        assert_eq!(
            code(draft_site(with_run, &mut s.config)),
            ErrorCode::ServiceUnavailable
        );
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

    /// A damaged adopted-contract record cannot say which adapters removed
    /// by hand still name artifacts, so drafting the affected origin waits
    /// for its repair rather than offer rules the save would refuse.
    #[test]
    fn draft_site_refuses_an_origin_the_adopted_record_cannot_vouch_for() {
        let mut s = setup("");
        let state = crate::config::adopted_contracts_path(s.folder.path());
        let damaged = json!({
            "type": "tools.other",
            "origin": TOOLS,
            "match": [{"path": "/items/{n:nope}"}],
            "id": "other:{host}/{n}",
        });
        fs::write(
            &state,
            serde_json::to_vec(&json!({"format": 1, "contracts": [damaged]})).unwrap(),
        )
        .unwrap();
        let table = json!({
            "type": "tools.items",
            "origin": TOOLS,
            "match": [{"path": "/items/{num:int}"}],
            "id": "tools.items:{host}/{num}"
        });
        let refused = |s: &mut Setup| {
            let response = draft_site(request(TOOLS, &["/items/{num}"], &[]), &mut s.config);
            let Response::Error(error) = response else {
                panic!("expected a refusal, got {response:?}")
            };
            assert_eq!(error.code, ErrorCode::InvalidConfig);
            assert!(
                error.message.contains("Forget recorded adapters"),
                "{}",
                error.message
            );
            let revision = s.config.revision().to_owned();
            assert_eq!(
                code(check_site_draft(
                    check(
                        TOOLS,
                        &revision,
                        vec![table.clone()],
                        vec![vec![RuleProvenance::Drafted]],
                        vec![None]
                    ),
                    &mut s.config
                )),
                ErrorCode::InvalidConfig
            );
        };
        refused(&mut s);
        // Other origins still draft.
        drafted(draft_site(
            request("https://elsewhere.example", &["/items/{num}"], &[]),
            &mut s.config,
        ));

        // An unreadable record vouches for no origin.
        fs::write(&state, b"{").unwrap();
        refused(&mut s);
        let response = draft_site(
            request("https://elsewhere.example", &["/items/{num}"], &[]),
            &mut s.config,
        );
        assert_eq!(code(response), ErrorCode::InvalidConfig);
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

    /// The structural response, pinned before `draft_site` was split into
    /// `structural_response` and `site_drafted_response`: refactoring it
    /// for agent drafts must not change a byte of it (the revision is the
    /// temp config's, so it is compared separately).
    #[test]
    fn structural_draft_site_response_is_unchanged_by_the_refactor() {
        let mut s = setup(&adapters(&[TOOLS_ITEMS]));
        let response = draft_site(
            request(
                TOOLS,
                &[
                    "/items/{num}",
                    "/docs/{num}",
                    "/docs/{num}/edit",
                    "/search?q={slug}",
                    "/wiki/{slug}/{uuid}",
                ],
                &["/boards/{num}", "/teams/{key}", "/invite/{hex32}"],
            ),
            &mut s.config,
        );
        let mut json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["revision"], s.config.revision());
        json["revision"] = json!("<revision>");
        let pinned: Json = serde_json::from_str(include_str!(
            "../tests/fixtures/capture-site-structural-response.json"
        ))
        .unwrap();
        assert_eq!(
            json,
            pinned,
            "{}",
            serde_json::to_string_pretty(&json).unwrap()
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

    /// `start_site_draft` and `draft_site` with a `run_id`, against a fake
    /// service on a short `/tmp` socket. No launchd, no real service and no
    /// harness is ever involved.
    #[cfg(unix)]
    mod agent {
        use super::*;
        use crate::service::client::fake::{self, FakeService};
        use crate::service::client::{Launch, ServiceLink};
        use adapter_templates::Value as AdapterValue;
        use brauser_protocol::{DraftedRuleView, DroppedView};
        use brauser_protocol::{
            HarnessId, RecordKey, RecordKeyKind, Request, RunFetched, RunKind, RunStarted,
            RunState, RunView, SiteDraftAnswerView, StartSiteDraftRequest, UncheckedReason,
        };
        use std::sync::Arc;
        use std::time::Duration;

        const BUILD: &str = "this-build";
        const RUN_ID: &str = "00000000-0000-4000-8000-0000000000aa";

        fn now() -> String {
            time::OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        }

        fn never(_: bool) -> Launch {
            panic!("the service must not be started")
        }

        fn link(socket: PathBuf) -> ServiceLink {
            ServiceLink {
                restart_lock: socket.with_file_name("restart.lock"),
                socket,
                launch: Box::new(never),
                wait: Duration::from_secs(2),
                build_id: BUILD.into(),
                disk_build: || BUILD.to_owned(),
            }
        }

        /// A succeeded site-draft run for `origin` holding `tables`.
        fn site_run(origin: &str, tables: Vec<Json>) -> RunView {
            let key = RecordKey {
                kind: RecordKeyKind::Url,
                value: origin.into(),
            };
            RunView {
                run_id: RUN_ID.into(),
                state: RunState::Succeeded,
                url: origin.into(),
                title: "Capture Site: tools.example.com".into(),
                written_record_key: Some(key.clone()),
                kind: Some(RunKind::SiteDraft),
                site_draft: Some(SiteDraftAnswerView { adapters: tables }),
                ..fake::run_view(key)
            }
        }

        /// A fake service whose get_run answers `run`, and whose start_run
        /// answers a queued run for the forwarded request.
        fn service(dir: &tempfile::TempDir, run: RunView) -> (FakeService, ServiceLink) {
            let socket = fake::socket_in(dir);
            let service = FakeService::listen(
                &socket,
                Arc::new(move |request, _| match request {
                    Request::ServiceHello(_) => fake::ready(request, BUILD),
                    Request::GetRun(value) => Response::RunFetched(RunFetched {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id.clone(),
                        run: run.clone(),
                        stream_text: "agent text".into(),
                        next_offset: 10,
                        more: false,
                    }),
                    Request::StartRun(value) => Response::RunStarted(RunStarted {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id.clone(),
                        run: RunView {
                            kind: value.kind,
                            url: value.url.clone(),
                            ..fake::run_view(value.record_key.clone().unwrap())
                        },
                        existing: false,
                    }),
                    _ => fake::runs_listed(request),
                }),
            );
            (service, link(socket))
        }

        fn tools_key() -> RecordKey {
            RecordKey {
                kind: RecordKeyKind::Url,
                value: TOOLS.into(),
            }
        }

        fn start(origin: &str, visited: &[&str], links: &[&str]) -> StartSiteDraftRequest {
            StartSiteDraftRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "site-1".into(),
                origin: origin.into(),
                visited: counts(visited),
                links: counts(links),
                harness: Some(HarnessId::Claude),
                requested_at: now(),
            }
        }

        fn agent_request(origin: &str, visited: &[&str], links: &[&str]) -> DraftSiteRequest {
            DraftSiteRequest {
                run_id: Some(RUN_ID.into()),
                ..request(origin, visited, links)
            }
        }

        fn table(kind: &str, paths: &[&str], capture: &str) -> Json {
            json!({
                "type": kind,
                "origin": TOOLS,
                "match": paths.iter().map(|p| json!({"path": p})).collect::<Vec<_>>(),
                "id": format!("{kind}:{{host}}/{{{capture}}}"),
            })
        }

        /// The one forwarded start_run (after the service_hello).
        fn forwarded(service: &FakeService) -> brauser_protocol::StartRunRequest {
            let requests = service.requests();
            assert_eq!(requests.len(), 2, "{requests:?}");
            match &requests[1] {
                Request::StartRun(value) => value.clone(),
                other => panic!("expected start_run, got {other:?}"),
            }
        }

        #[test]
        fn start_site_draft_builds_the_input_from_the_hosts_own_structural_draft() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup(&adapters(&[TOOLS_ITEMS]));
            let visited = ["/docs/{num}", "/docs/{num}/edit", "/items/{num}"];
            let links = ["/boards/{num}"];
            let structural = drafted(draft_site(request(TOOLS, &visited, &links), &mut s.config));
            assert!(!structural.adapters.is_empty());
            let dir = fake::short_dir();
            let (service, link) = service(&dir, fake::run_view(tools_key()));
            let response = super::super::start_site_draft(
                start(TOOLS, &visited, &links),
                &mut s.config,
                Some(&link),
            );
            assert!(matches!(response, Response::RunStarted(_)), "{response:?}");
            let sent = forwarded(&service);
            let pairs = |list: &[&str]| -> Vec<(String, u32)> {
                list.iter().map(|shape| ((*shape).to_owned(), 1)).collect()
            };
            let tables: Vec<Json> = structural
                .adapters
                .iter()
                .map(|a| a.adapter.clone())
                .collect();
            assert_eq!(
                sent.content,
                crate::service::prompt::site_draft_input(
                    TOOLS,
                    &pairs(&visited),
                    &pairs(&links),
                    &tables
                )
            );
            for shape in visited.iter().chain(&links) {
                assert!(sent.content.contains(shape), "{shape}");
            }
            // The host's structural tables, not anything the extension sent.
            assert!(sent.content.contains("tools.docs"), "{}", sent.content);
            // No URL other than the origin: shapes and templates only.
            for (at, _) in sent.content.match_indices("http") {
                assert!(
                    sent.content[at..].starts_with(TOOLS),
                    "{}",
                    &sent.content[at..]
                );
            }
        }

        #[test]
        fn start_site_draft_forwards_kind_site_draft_with_origin_only() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup("");
            let dir = fake::short_dir();
            let (service, link) = service(&dir, fake::run_view(tools_key()));
            let request = start(TOOLS, &["/docs/{num}"], &[]);
            let requested_at = request.requested_at.clone();
            let started = match super::super::start_site_draft(request, &mut s.config, Some(&link))
            {
                Response::RunStarted(started) => started,
                other => panic!("expected run_started, got {other:?}"),
            };
            assert_eq!(started.request_id, "site-1");
            assert_eq!(started.run.kind, Some(RunKind::SiteDraft));
            let sent = forwarded(&service);
            assert_eq!(sent.kind, Some(RunKind::SiteDraft));
            assert_eq!(sent.url, TOOLS);
            assert_eq!(sent.title, "Capture Site: tools.example.com");
            assert_eq!(sent.selection, None);
            assert_eq!(sent.harness, Some(HarnessId::Claude));
            assert_eq!(sent.requested_at, requested_at);
            assert_eq!(
                sent.record_key,
                Some(RecordKey {
                    kind: RecordKeyKind::Url,
                    value: TOOLS.into(),
                })
            );
        }

        /// Checked before the service is reached: with no link at all a bad
        /// request is still `invalid_request`, not `service_unavailable`.
        #[test]
        fn start_site_draft_refuses_invalid_shapes_raw_urls_and_stale_requested_at() {
            let mut s = setup("");
            let refused = |s: &mut Setup, request: StartSiteDraftRequest| {
                code(super::super::start_site_draft(request, &mut s.config, None))
            };
            for origin in ["https://tools.example.com/", "tools.example.com", ""] {
                assert_eq!(
                    refused(&mut s, start(origin, &["/docs/{num}"], &[])),
                    ErrorCode::InvalidRequest,
                    "{origin}"
                );
            }
            for shape in [
                "https://tools.example.com/docs/42",
                "/docs/42",
                "/Docs/{num}",
            ] {
                assert_eq!(
                    refused(&mut s, start(TOOLS, &[shape], &[])),
                    ErrorCode::InvalidRequest,
                    "visited {shape}"
                );
                assert_eq!(
                    refused(&mut s, start(TOOLS, &["/docs/{num}"], &[shape])),
                    ErrorCode::InvalidRequest,
                    "link {shape}"
                );
            }
            let many: Vec<String> = (0..51)
                .map(|i| format!("/docs/{{num}}/{}", word(i)))
                .collect();
            let many: Vec<&str> = many.iter().map(String::as_str).collect();
            assert_eq!(
                refused(&mut s, start(TOOLS, &[], &many)),
                ErrorCode::InvalidRequest
            );
            for requested_at in ["yesterday", "2001-01-01T00:00:00Z"] {
                let mut stale = start(TOOLS, &["/docs/{num}"], &[]);
                stale.requested_at = requested_at.into();
                assert_eq!(
                    refused(&mut s, stale),
                    ErrorCode::InvalidRequest,
                    "{requested_at}"
                );
            }
        }

        #[test]
        fn start_site_draft_refuses_while_adapter_policy_needs_repair() {
            let mut s = setup("[adapters]\nformat = 2\n");
            assert!(s.config.adapter_issue().is_some());
            assert_eq!(
                code(super::super::start_site_draft(
                    start(TOOLS, &["/docs/{num}"], &[]),
                    &mut s.config,
                    None
                )),
                ErrorCode::InvalidConfig
            );
        }

        #[test]
        fn start_site_draft_without_the_service_is_service_unavailable() {
            let mut s = setup("");
            assert_eq!(
                code(super::super::start_site_draft(
                    start(TOOLS, &["/docs/{num}"], &[]),
                    &mut s.config,
                    None
                )),
                ErrorCode::ServiceUnavailable
            );
        }

        #[test]
        fn agent_draft_site_returns_agent_drafter_bound_to_the_current_revision() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup("");
            let dir = fake::short_dir();
            let tables = vec![
                table("tools.docs", &["/docs/{n:int}"], "n"),
                table("tools.boards", &["/boards/{n:int}"], "n"),
            ];
            let (service, link) = service(&dir, site_run(TOOLS, tables));
            let site = drafted(super::super::draft_site(
                agent_request(TOOLS, &["/docs/{num}"], &[]),
                &mut s.config,
                Some(&link),
            ));
            assert_eq!(site.request_id, "draft-1");
            assert_eq!(site.drafter, SiteDrafter::Agent);
            assert_eq!(site.revision, s.config.revision());
            assert_eq!(site.adapters.len(), 2, "{site:#?}");
            let docs = &site.adapters[0];
            assert_eq!(docs.adapter["type"], "tools.docs");
            assert!(docs.rules[0].checked);
            assert_eq!(docs.rules[0].visited, 1);
            // No page the user saw matches the boards rule: it starts
            // unchecked, suggested by AI.
            let boards = &site.adapters[1];
            assert!(!boards.rules[0].checked);
            assert_eq!(
                boards.rules[0].unchecked_reason,
                Some(UncheckedReason::Unseen)
            );
            let requests = service.requests();
            assert!(
                matches!(
                    &requests[1],
                    Request::GetRun(get) if get.run_id == RUN_ID && get.since_offset == 0
                ),
                "{requests:?}"
            );
        }

        #[test]
        fn agent_draft_site_refuses_a_summary_run_another_origin_or_an_unfinished_run() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup("");
            let good = site_run(TOOLS, vec![table("tools.docs", &["/docs/{n:int}"], "n")]);
            let other = "https://other.example.com";
            let cases: Vec<(&str, RunView, ErrorCode)> = vec![
                (
                    "summary",
                    RunView {
                        kind: None,
                        site_draft: None,
                        ..good.clone()
                    },
                    ErrorCode::InvalidRequest,
                ),
                (
                    "another origin",
                    RunView {
                        url: other.into(),
                        record_key: RecordKey {
                            kind: RecordKeyKind::Url,
                            value: other.into(),
                        },
                        ..good.clone()
                    },
                    ErrorCode::InvalidRequest,
                ),
                (
                    "another run",
                    RunView {
                        run_id: "00000000-0000-4000-8000-0000000000bb".into(),
                        ..good.clone()
                    },
                    ErrorCode::InvalidRequest,
                ),
                (
                    "queued",
                    RunView {
                        state: RunState::Queued,
                        site_draft: None,
                        ..good.clone()
                    },
                    ErrorCode::Conflict,
                ),
                (
                    "running",
                    RunView {
                        state: RunState::Running,
                        site_draft: None,
                        ..good.clone()
                    },
                    ErrorCode::Conflict,
                ),
                (
                    "failed",
                    RunView {
                        state: RunState::Failed,
                        site_draft: None,
                        error_kind: Some(brauser_protocol::RunErrorKind::InvalidOutput),
                        ..good.clone()
                    },
                    ErrorCode::InvalidRequest,
                ),
                (
                    "succeeded without tables",
                    RunView {
                        site_draft: None,
                        ..good.clone()
                    },
                    ErrorCode::InvalidRequest,
                ),
            ];
            for (name, run, expected) in cases {
                let dir = fake::short_dir();
                let (_service, link) = service(&dir, run);
                assert_eq!(
                    code(super::super::draft_site(
                        agent_request(TOOLS, &["/docs/{num}"], &[]),
                        &mut s.config,
                        Some(&link),
                    )),
                    expected,
                    "{name}"
                );
            }
            // A malformed run id is refused before the service is reached.
            let dir = fake::short_dir();
            let link = link(fake::socket_in(&dir));
            let mut bad = agent_request(TOOLS, &["/docs/{num}"], &[]);
            bad.run_id = Some("not-a-run".into());
            assert_eq!(
                code(super::super::draft_site(bad, &mut s.config, Some(&link))),
                ErrorCode::InvalidRequest
            );
        }

        #[test]
        fn agent_draft_site_preview_lists_every_shape_and_probe() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup("");
            let dir = fake::short_dir();
            let (_service, link) = service(
                &dir,
                site_run(
                    TOOLS,
                    vec![
                        table("tools.items", &["/items/{n:int}"], "n"),
                        table("tools.docs", &["/docs/{u:uuid}"], "u"),
                    ],
                ),
            );
            let site = drafted(super::super::draft_site(
                agent_request(
                    TOOLS,
                    &["/items/{num}", "/search?q={slug}"],
                    &["/docs/{uuid}"],
                ),
                &mut s.config,
                Some(&link),
            ));
            let probes = &catalog().probe_paths;
            assert_eq!(site.preview.len(), 3 + probes.len());
            assert_eq!(site.preview[0].shape, "/items/{num}");
            assert_eq!(site.preview[0].adapter, Some(0));
            assert_eq!(
                site.preview[0].artifact_id.as_deref(),
                Some("tools.items:tools.example.com/123456")
            );
            assert_eq!(site.preview[1].adapter, None);
            assert_eq!(site.preview[2].source, PreviewSource::Link);
            assert_eq!(site.preview[2].adapter, Some(1));
            for (row, probe) in site.preview[3..].iter().zip(probes) {
                assert_eq!(row.source, PreviewSource::Probe);
                assert_eq!(row.url, format!("{TOOLS}{probe}"));
                assert_eq!(row.adapter, None, "{probe}");
            }
            // A link-only agent rule starts unchecked, as a structural one.
            assert!(!site.adapters[1].rules[0].checked);
            assert_eq!(
                site.adapters[1].rules[0].unchecked_reason,
                Some(UncheckedReason::LinkOnly)
            );
        }

        #[test]
        fn agent_draft_site_reports_zero_remapped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup(&adapters(&[TOOLS_ITEMS]));
            let dir = fake::short_dir();
            let (_service, link) = service(
                &dir,
                site_run(
                    TOOLS,
                    vec![table(
                        "tools.docs",
                        &["/items/{n:int}", "/docs/{n:int}"],
                        "n",
                    )],
                ),
            );
            let site = drafted(super::super::draft_site(
                agent_request(TOOLS, &["/items/{num}", "/docs/{num}"], &[]),
                &mut s.config,
                Some(&link),
            ));
            assert_eq!(site.remapped, 0);
            // The rule over the configured route is dropped as an overlap,
            // so the recognized page keeps its kind.
            assert!(
                site.dropped.iter().any(|d| d.shape.is_none()
                    && d.adapter == Some(0)
                    && d.rule == Some(0)
                    && d.reason == DraftDropReason::DraftOverlap),
                "{:?}",
                site.dropped
            );
            assert_eq!(site.adapters.len(), 1);
            assert_eq!(
                site.adapters[0].adapter["match"],
                json!([{"path": "/docs/{n:int}"}])
            );
            assert_eq!(site.preview[0].adapter, None);
        }

        #[test]
        fn agent_draft_site_rejects_a_forbidden_key_with_one_dropped_row() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup("");
            let mut titled = table("tools.docs", &["/docs/{n:int}"], "n");
            titled["title"] = json!([{"template": "{title}"}]);
            let dir = fake::short_dir();
            let (_service, link) = service(&dir, site_run(TOOLS, vec![titled]));
            let site = drafted(super::super::draft_site(
                agent_request(TOOLS, &["/docs/{num}"], &[]),
                &mut s.config,
                Some(&link),
            ));
            assert_eq!(site.drafter, SiteDrafter::Agent);
            assert!(site.adapters.is_empty());
            assert_eq!(site.dropped.len(), 1, "{:?}", site.dropped);
            let row = &site.dropped[0];
            assert_eq!(
                (row.shape.as_deref(), row.adapter, row.rule),
                (None, None, None)
            );
            assert_eq!(row.reason, DraftDropReason::DraftForbiddenKey);
            assert!(site.preview.iter().all(|r| r.adapter.is_none()));
        }

        #[test]
        fn agent_draft_site_writes_nothing_to_home_or_config_dir() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup(&adapters(&[TOOLS_ITEMS]));
            {
                let _lock = s.config.lock_current().unwrap();
                s.config.refresh().unwrap();
            }
            let before = tree(s.folder.path());
            let dir = fake::short_dir();
            let (_service, link) = service(
                &dir,
                site_run(TOOLS, vec![table("tools.docs", &["/docs/{n:int}"], "n")]),
            );
            drafted(super::super::draft_site(
                agent_request(TOOLS, &["/docs/{num}"], &["/boards/{num}"]),
                &mut s.config,
                Some(&link),
            ));
            assert!(matches!(
                super::super::start_site_draft(
                    start(TOOLS, &["/docs/{num}"], &["/boards/{num}"]),
                    &mut s.config,
                    Some(&link),
                ),
                Response::RunStarted(_)
            ));
            assert_eq!(tree(s.folder.path()), before);
        }

        #[test]
        fn agent_draft_site_response_fits_outbound_cap_at_limits() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup("");
            let origin = format!("https://{}.example", vec!["a".repeat(63); 30].join("."));
            let shapes: Vec<String> = (0..71).map(long_shape).collect();
            let shapes: Vec<&str> = shapes.iter().map(String::as_str).collect();
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
            let dir = fake::short_dir();
            let (_service, link) = service(&dir, site_run(&origin, tables));
            let response = super::super::draft_site(
                agent_request(&origin, &shapes[..21], &shapes[21..]),
                &mut s.config,
                Some(&link),
            );
            let size = serde_json::to_vec(&response).unwrap().len();
            let site = drafted(response);
            assert_eq!(site.adapters.len(), 8, "{:?}", site.dropped);
            assert!(site.adapters.iter().all(|a| a.rules.len() == 16));
            assert_eq!(site.preview.len(), 71 + catalog().probe_paths.len());
            assert!(size < MAX_OUTBOUND_BYTES, "{size}");
        }

        /// The save does not change for an agent draft: the same selection
        /// gets the same verdicts whichever drafter offered it.
        #[test]
        fn check_site_draft_judges_an_agent_selection_exactly_as_a_structural_one() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let mut s = setup("");
            let visited = ["/docs/{num}", "/docs/{num}/edit"];
            let structural = drafted(draft_site(request(TOOLS, &visited, &[]), &mut s.config));
            let tables: Vec<Json> = structural
                .adapters
                .iter()
                .map(|a| a.adapter.clone())
                .collect();
            let dir = fake::short_dir();
            let (_service, link) = service(&dir, site_run(TOOLS, tables));
            let agent = drafted(super::super::draft_site(
                agent_request(TOOLS, &visited, &[]),
                &mut s.config,
                Some(&link),
            ));
            assert_eq!(agent.drafter, SiteDrafter::Agent);
            let judge = |s: &mut Setup, site: &SiteDrafted| {
                let tables: Vec<Json> = site.adapters.iter().map(|a| a.adapter.clone()).collect();
                let provenance = tables.iter().map(drafted_rules).collect();
                let scope = vec![None; tables.len()];
                let mut request = check(TOOLS, &site.revision, tables, provenance, scope);
                request.visited = counts(&visited);
                checked(check_site_draft(request, &mut s.config))
            };
            let from_structural = judge(&mut s, &structural);
            let from_agent = judge(&mut s, &agent);
            assert_eq!(
                serde_json::to_value(&from_agent).unwrap(),
                serde_json::to_value(&from_structural).unwrap()
            );
            assert_eq!(agent.adapters, structural.adapters);
            assert_eq!(agent.preview, structural.preview);
        }

        // Step 7.7 end to end through the host (DESIGN §12.2 step 7.7). Each
        // case replays the shared agent answer in
        // `host/tests/fixtures/site-draft-answers/` (the answers the crate
        // judges in `adapter-templates/tests/agent_draft.rs` and the service
        // passes on unchanged) as a succeeded site-draft run, then asks
        // `draft_site` with its `run_id`, so the decision shown is the
        // host's: `dropped`, `adapters`, `checked` and `preview` of the
        // `site_drafted` it answers.

        macro_rules! fixture {
            ($case:literal) => {
                include_str!(concat!(
                    "../tests/fixtures/site-draft-answers/",
                    $case,
                    ".json"
                ))
            };
        }

        /// The answer's tables, as the service hands them on.
        fn answer(text: &str) -> Vec<Json> {
            let answer: Json = serde_json::from_str(text).expect("fixture answer parses");
            let keys: Vec<&String> = answer.as_object().expect("answer object").keys().collect();
            assert_eq!(keys, ["adapters"], "an answer's only key is adapters");
            answer["adapters"].as_array().expect("adapters").clone()
        }

        /// `draft_site` with the run id of a succeeded site-draft run holding
        /// `tables`. Every
        /// preview row names a rule the response keeps.
        fn judge(s: &mut Setup, tables: &[Json], visited: &[&str], links: &[&str]) -> SiteDrafted {
            let dir = fake::short_dir();
            let (service, link) = service(&dir, site_run(TOOLS, tables.to_vec()));
            let site = drafted(super::super::draft_site(
                agent_request(TOOLS, visited, links),
                &mut s.config,
                Some(&link),
            ));
            assert_eq!(site.drafter, SiteDrafter::Agent);
            assert_eq!(site.revision, s.config.revision());
            assert_eq!(site.remapped, 0);
            assert_eq!(
                site.preview.len(),
                visited.len() + links.len() + catalog().probe_paths.len()
            );
            for row in &site.preview {
                if let (Some(adapter), Some(rule)) = (row.adapter, row.rule) {
                    let adapter = &site.adapters[adapter as usize];
                    assert!(
                        adapter.rules.iter().any(|r| r.rule == rule),
                        "{row:?} names a kept rule"
                    );
                } else {
                    assert_eq!(
                        (row.adapter, row.rule, &row.artifact_id),
                        (None, None, &None)
                    );
                }
            }
            // The tables were the service's, fetched with the run id.
            let requests = service.requests();
            assert!(
                matches!(&requests[1], Request::GetRun(get) if get.run_id == RUN_ID),
                "{requests:?}"
            );
            site
        }

        /// A rule as `path` or `path?key=template&...`.
        fn rule_text(rule: &Json) -> String {
            let mut text = rule["path"].as_str().expect("path").to_owned();
            if let Some(query) = rule.get("query").and_then(Json::as_object) {
                for (index, (key, template)) in query.iter().enumerate() {
                    text.push(if index == 0 { '?' } else { '&' });
                    text.push_str(key);
                    text.push('=');
                    text.push_str(template.as_str().unwrap_or("<non-string>"));
                }
            }
            text
        }

        fn table_rules(table: &Json) -> Vec<String> {
            table["match"]
                .as_array()
                .expect("match")
                .iter()
                .map(rule_text)
                .collect()
        }

        /// Every dropped agent rule as `(rule text, reason)`, from the rows
        /// that name the agent's table (and rule; a whole dropped table names
        /// each of its rules). Shape rows and a whole-draft refusal are not
        /// agent rules.
        fn dropped_rules(site: &SiteDrafted, tables: &[Json]) -> Vec<(String, String)> {
            site.dropped
                .iter()
                .filter_map(|d| {
                    assert_eq!(d.shape, None, "an agent draft drops no shapes: {d:?}");
                    let texts = table_rules(&tables[d.adapter? as usize]);
                    let texts = match d.rule {
                        Some(rule) => vec![texts[rule as usize].clone()],
                        None => texts,
                    };
                    Some(
                        texts
                            .into_iter()
                            .map(move |t| (t, format!("{:?}", d.reason))),
                    )
                })
                .flatten()
                .collect()
        }

        /// Each expected `(rule, reason)` pair is reported, and nothing else
        /// is dropped.
        fn assert_dropped(
            site: &SiteDrafted,
            tables: &[Json],
            expected: &[(&str, DraftDropReason)],
        ) {
            let mut got = dropped_rules(site, tables);
            let mut want: Vec<(String, String)> = expected
                .iter()
                .map(|(text, reason)| ((*text).to_owned(), format!("{reason:?}")))
                .collect();
            got.sort();
            want.sort();
            assert_eq!(got, want);
        }

        /// The text of every kept rule.
        fn all_kept(site: &SiteDrafted) -> Vec<String> {
            site.adapters
                .iter()
                .flat_map(|a| {
                    let texts = table_rules(&a.adapter);
                    a.rules
                        .iter()
                        .map(move |r| texts[r.rule as usize].clone())
                        .collect::<Vec<_>>()
                })
                .collect()
        }

        /// The kept rule with this text.
        fn kept<'s>(site: &'s SiteDrafted, text: &str) -> &'s DraftedRuleView {
            site.adapters
                .iter()
                .find_map(|a| {
                    let texts = table_rules(&a.adapter);
                    a.rules.iter().find(|r| texts[r.rule as usize] == text)
                })
                .unwrap_or_else(|| panic!("{text} kept: {site:#?}"))
        }

        /// The visited rule is kept and checked: backed by a visited shape,
        /// with no unchecked reason.
        fn assert_visited_rule_kept_and_checked(site: &SiteDrafted, text: &str) {
            assert!(
                !site.dropped.iter().any(|d| d.adapter.is_none()),
                "{:?}",
                site.dropped
            );
            let rule = kept(site, text);
            assert!(
                rule.checked && rule.unchecked_reason.is_none(),
                "{text}: {rule:?}"
            );
            assert!(rule.visited > 0, "{text}: {rule:?}");
        }

        /// The type of the first checked rule that evaluates `url`, matched
        /// without scope as the preview matches.
        fn checked_type(site: &SiteDrafted, url: &str) -> Option<String> {
            site.adapters.iter().find_map(|a| {
                let keep = a
                    .rules
                    .iter()
                    .filter(|r| r.checked)
                    .map(|r| r.rule as usize);
                let table: AdapterValue = serde_json::from_value(a.adapter.clone()).unwrap();
                let preview = super::super::preview_adapter(&table, keep.collect())?;
                Some(preview.document.evaluate(url, None).ok()?.artifact_type)
            })
        }

        /// No checked rule, and no preview row, captures any of `urls`.
        fn assert_not_captured(site: &SiteDrafted, urls: &[&str]) {
            for url in urls {
                assert_eq!(checked_type(site, url), None, "{url}");
            }
        }

        #[test]
        fn host_agent_planted_links_toward_rest_after_first_segment_are_dropped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!(
                "agent_planted_links_toward_rest_after_first_segment_are_dropped"
            ));
            let planted = [
                "/{slug}",
                "/{slug}/{slug}",
                "/{slug}/{slug}/{slug}",
                "/docs/{slug}/{slug}",
                "/docs/{slug}/{slug}/{slug}",
                "/{num}/{slug}/{slug}",
                "/{key}/{slug}/{slug}/{slug}",
                "/docs/{num}/{slug}/{slug}",
            ];
            let site = judge(&mut setup(""), &tables, &["/docs/{num}"], &planted);
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            assert_dropped(
                &site,
                &tables,
                &[
                    ("/{n:int}/page/{*}", DraftDropReason::DraftNoLeadingLiteral),
                    ("/docs/{*}", DraftDropReason::DraftRestSecond),
                ],
            );
            assert_eq!(all_kept(&site), ["/docs/{n:int}"]);
            assert_not_captured(
                &site,
                &[
                    "https://tools.example.com/anything",
                    "https://tools.example.com/docs/a/b/c",
                    "https://tools.example.com/42/a/b",
                ],
            );
            // Only the visited row is captured; no planted link is.
            assert!(site.preview[1..].iter().all(|r| r.adapter.is_none()));
        }

        #[test]
        fn host_agent_planted_root_match_is_dropped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!("agent_planted_root_match_is_dropped"));
            let site = judge(
                &mut setup(""),
                &tables,
                &["/docs/{num}"],
                &["/", "/?id={num}", "//{num}", "/{num}"],
            );
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            assert_dropped(
                &site,
                &tables,
                &[
                    ("/{n:int}/index", DraftDropReason::DraftNoLeadingLiteral),
                    ("/{n:int}/{_}/view", DraftDropReason::DraftNoLeadingLiteral),
                    (
                        "/{m:int}/view?id={n:int}",
                        DraftDropReason::DraftNoLeadingLiteral,
                    ),
                ],
            );
            assert_not_captured(
                &site,
                &[
                    "https://tools.example.com/",
                    "https://tools.example.com/?id=42",
                    "https://tools.example.com/42",
                ],
            );
            assert!(site.preview[1..].iter().all(|r| r.adapter.is_none()));
        }

        #[test]
        fn host_agent_planted_search_page_is_dropped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!("agent_planted_search_page_is_dropped"));
            let planted = [
                "/search?q={slug}",
                "/?q={slug}",
                "/search/{num}",
                "/find?q={num}",
                "/docs?query={id}",
                "/query/{key}",
                "/filter/{uuid}",
            ];
            let site = judge(&mut setup(""), &tables, &["/docs/{num}"], &planted);
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            assert_dropped(
                &site,
                &tables,
                &[
                    ("/search/{n:int}", DraftDropReason::DraftBlockedRoute),
                    ("/find?q={n:int}", DraftDropReason::DraftSearchCapture),
                    (
                        "/docs?query={t:token(1,128)}",
                        DraftDropReason::DraftSearchCapture,
                    ),
                ],
            );
            assert_not_captured(
                &site,
                &[
                    "https://tools.example.com/search/42",
                    "https://tools.example.com/find?q=42",
                    "https://tools.example.com/docs?query=abc",
                ],
            );
            assert!(site.preview[1..].iter().all(|r| r.adapter.is_none()));
        }

        #[test]
        fn host_agent_planted_secret_query_parameter_id_is_dropped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!(
                "agent_planted_secret_query_parameter_id_is_dropped"
            ));
            let planted = [
                "/files/{num}?token={id}",
                "/doc/{num}?access_token={hex64}",
                "/view?sig={hex32}",
                "/cb?code={id}",
            ];
            let site = judge(&mut setup(""), &tables, &["/docs/{num}"], &planted);
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            assert_dropped(
                &site,
                &tables,
                &[
                    ("/view?sig={h:hex(32)}", DraftDropReason::DraftSecretCapture),
                    (
                        "/cb?code={t:token(1,128)}",
                        DraftDropReason::DraftSecretCapture,
                    ),
                    (
                        "/files/{_}?token={t:token(1,128)}",
                        DraftDropReason::DraftSecretCapture,
                    ),
                ],
            );
            // No surviving ID, kept rule or previewed artifact carries a
            // secret.
            for adapter in &site.adapters {
                let id = adapter.adapter["id"].as_str().unwrap();
                assert!(!id.contains("{h}") && !id.contains("{t}"), "{id}");
            }
            for rule in all_kept(&site) {
                for key in ["token", "access_token", "sig", "code"] {
                    assert!(!rule.contains(&format!("{key}=")), "{rule}");
                }
            }
            assert!(site.preview[1..].iter().all(|r| r.adapter.is_none()));
        }

        #[test]
        fn host_agent_planted_auth_or_capability_route_is_dropped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!(
                "agent_planted_auth_or_capability_route_is_dropped"
            ));
            let planted = [
                "/invite/{hex32}",
                "/reset/{uuid}",
                "/password/reset/{hex64}",
                "/verify/{id}",
                "/confirm/{num}",
            ];
            let site = judge(&mut setup(""), &tables, &["/docs/{num}"], &planted);
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            assert_dropped(
                &site,
                &tables,
                &[
                    ("/invite/{h:hex(32)}", DraftDropReason::DraftBlockedRoute),
                    ("/reset/{u:uuid}", DraftDropReason::DraftBlockedRoute),
                    (
                        "/password/reset/{h:hex(64)}",
                        DraftDropReason::DraftBlockedRoute,
                    ),
                    (
                        "/verify/{t:token(1,128)}",
                        DraftDropReason::DraftBlockedRoute,
                    ),
                    ("/confirm/{n:int}", DraftDropReason::DraftBlockedRoute),
                ],
            );
            assert_eq!(all_kept(&site), ["/docs/{n:int}"]);
            assert!(site.preview[1..].iter().all(|r| r.adapter.is_none()));
        }

        #[test]
        fn host_agent_planted_search_or_paging_parameter_id_is_dropped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!(
                "agent_planted_search_or_paging_parameter_id_is_dropped"
            ));
            let planted = [
                "/items?page={num}",
                "/items?cursor={id}",
                "/items?offset={num}&sort={slug}",
                "/items?q={key}",
                "/items?jql={slug}&start={num}",
                "/items?tab={num}&view={uuid}",
            ];
            let site = judge(
                &mut setup(""),
                &tables,
                &["/docs/{num}", "/docs/{num}?page={num}"],
                &planted,
            );
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            assert_dropped(
                &site,
                &tables,
                &[
                    ("/items?page={n:int}", DraftDropReason::DraftPagingCapture),
                    (
                        "/items?cursor={t:token(1,128)}",
                        DraftDropReason::DraftPagingCapture,
                    ),
                    ("/items?q={k:jira_key}", DraftDropReason::DraftSearchCapture),
                    ("/items?tab={n:int}", DraftDropReason::DraftPagingCapture),
                ],
            );
            assert_eq!(all_kept(&site), ["/docs/{n:int}"]);
            assert!(site.preview[2..].iter().all(|r| r.adapter.is_none()));
        }

        #[test]
        fn host_agent_planted_id_less_rule_is_dropped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!("agent_planted_id_less_rule_is_dropped"));
            let planted = [
                "/about/team",
                "/pages/{slug}",
                "/docs/{slug}/{slug}/{slug}",
                "/docs/list?owner={slug}",
                "/docs?{param}={num}",
            ];
            let site = judge(&mut setup(""), &tables, &["/docs/{num}"], &planted);
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            // Each ID-less table is dropped whole: one row naming the table
            // and no rule.
            let whole = |table: u32| DroppedView {
                shape: None,
                adapter: Some(table),
                rule: None,
                reason: DraftDropReason::DraftIdNeedsTypedCapture,
            };
            assert_eq!(site.dropped, [whole(1), whole(2)]);
            assert_eq!(all_kept(&site), ["/docs/{n:int}"]);
            assert!(site.preview[1..].iter().all(|r| r.adapter.is_none()));
        }

        #[test]
        fn host_agent_planted_alternative_on_visited_rule_starts_unchecked_or_is_dropped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!(
                "agent_planted_alternative_on_visited_rule_starts_unchecked_or_is_dropped"
            ));
            let site = judge(
                &mut setup(""),
                &tables,
                &["/wiki/doc/{num}"],
                &["/wiki/page/{num}", "/wiki/doc/{num}/{slug}/{slug}"],
            );
            assert_visited_rule_kept_and_checked(&site, "/wiki/doc/{n:int}");
            // Outcome-based: whether the widened rule is dropped or starts
            // unchecked, no checked rule captures the planted link-only page,
            // and the visited page is still the checked first table's.
            assert_eq!(
                checked_type(&site, "https://tools.example.com/wiki/page/42"),
                None
            );
            assert_eq!(
                checked_type(&site, "https://tools.example.com/wiki/doc/42").as_deref(),
                Some("tools.doc")
            );
            for rule in site.adapters.iter().flat_map(|a| &a.rules) {
                assert_eq!(rule.checked, rule.unchecked_reason.is_none(), "{rule:?}");
            }
            // The crate drops it as an overlap with the first table.
            assert_dropped(
                &site,
                &tables,
                &[("/wiki/(doc|page)/{n:int}", DraftDropReason::DraftOverlap)],
            );
            assert_eq!(site.preview[0].adapter, Some(0));
            assert_eq!(site.preview[1].adapter, None);
        }

        #[test]
        fn host_agent_planted_blocked_route_word_is_dropped() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!("agent_planted_blocked_route_word_is_dropped"));
            // The expected words come from the host's catalog, never a copy.
            let mut expected: Vec<String> = catalog()
                .blocked_route_words
                .iter()
                .map(|w| format!("/{w}/{{n:int}}"))
                .collect();
            expected.push("/tools/(docs|share)/{n:int}".to_owned());
            let mut planted: Vec<String> = tables[1..].iter().flat_map(table_rules).collect();
            planted.sort();
            expected.sort();
            assert_eq!(planted, expected, "the fixture plants every catalog word");

            let site = judge(
                &mut setup(""),
                &tables,
                &["/docs/{num}"],
                &["/invite/{hex32}", "/share/{key}"],
            );
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            let want: Vec<(&str, DraftDropReason)> = expected
                .iter()
                .map(|r| (r.as_str(), DraftDropReason::DraftBlockedRoute))
                .collect();
            assert_dropped(&site, &tables, &want);
            assert_eq!(all_kept(&site), ["/docs/{n:int}"]);
            assert!(site.preview[1..].iter().all(|r| r.adapter.is_none()));
        }

        #[test]
        fn host_agent_draft_overlapping_existing_adapter_loses_only_overlapping_rules() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!(
                "agent_draft_overlapping_existing_adapter_loses_only_overlapping_rules"
            ));
            let history = "[[artifacts]]\ntype = \"tools.history\"\norigin = \"https://tools.example.com\"\nmatch = [{ path = \"/docs/{n:int}/history\" }]\nid = \"tools.history:{host}/{n}\"\n";
            let visited = [
                "/docs/{num}",
                "/docs/{num}/{slug}/{slug}",
                "/docs/{num}/history",
                "/tickets/{key}",
            ];
            let site = judge(&mut setup(&adapters(&[history])), &tables, &visited, &[]);
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            assert_dropped(
                &site,
                &tables,
                &[("/docs/{n:int}/history", DraftDropReason::DraftOverlap)],
            );
            let mut all = all_kept(&site);
            all.sort();
            assert_eq!(all, ["/docs/{n:int}", "/tickets/{k:jira_key}"]);
            assert_visited_rule_kept_and_checked(&site, "/tickets/{k:jira_key}");
            // The configured page keeps its kind: the draft does not name it.
            let row = &site.preview[2];
            assert_eq!(row.shape, "/docs/{num}/history");
            assert_eq!(row.adapter, None);
        }

        /// A whole-draft refusal through the host: no adapters, one dropped
        /// row naming no shape, adapter or rule, and no preview row naming an
        /// agent rule.
        fn assert_rejected_before_the_preview(site: &SiteDrafted, reason: DraftDropReason) {
            assert!(site.adapters.is_empty(), "{site:#?}");
            assert_eq!(
                site.dropped,
                [DroppedView {
                    shape: None,
                    adapter: None,
                    rule: None,
                    reason,
                }]
            );
            assert!(
                site.preview
                    .iter()
                    .all(|r| r.adapter.is_none() && r.rule.is_none() && r.artifact_id.is_none()),
                "{:?}",
                site.preview
            );
        }

        /// The answer, and each forbidden table beside the clean visited
        /// table, is refused whole; without them the same answer is kept.
        fn assert_each_table_rejected(tables: &[Json]) {
            let visited = ["/docs/{num}", "/notes/{num}", "/wiki/{num}", "/items/{num}"];
            let site = judge(&mut setup(""), tables, &visited, &[]);
            assert_rejected_before_the_preview(&site, DraftDropReason::DraftForbiddenKey);
            for table in &tables[1..] {
                let pair = [tables[0].clone(), table.clone()];
                let site = judge(&mut setup(""), &pair, &visited, &[]);
                assert_rejected_before_the_preview(&site, DraftDropReason::DraftForbiddenKey);
            }
            let clean = judge(&mut setup(""), &tables[..1], &visited, &[]);
            assert_visited_rule_kept_and_checked(&clean, "/docs/{n:int}");
            assert_eq!(clean.preview[0].adapter, Some(0));
        }

        #[test]
        fn host_agent_draft_with_tier1_fields_is_rejected_before_the_preview() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!(
                "agent_draft_with_tier1_fields_is_rejected_before_the_preview"
            ));
            for (table, key) in tables[1..].iter().zip(["fields", "alias", "path_case"]) {
                assert!(table.get(key).is_some(), "the fixture plants {key}");
            }
            assert_each_table_rejected(&tables);
        }

        #[test]
        fn host_agent_draft_with_a_title_template_is_rejected_before_the_preview() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!(
                "agent_draft_with_a_title_template_is_rejected_before_the_preview"
            ));
            assert!(tables[1].get("title").is_some());
            assert_each_table_rejected(&tables);
        }

        #[test]
        fn host_agent_draft_with_a_scope_is_rejected_before_the_preview() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let tables = answer(fixture!(
                "agent_draft_with_a_scope_is_rejected_before_the_preview"
            ));
            assert!(tables[1].get("scope").is_some());
            assert!(tables[2].get("unscoped").is_some());
            assert_each_table_rejected(&tables);
        }

        /// The service fails the prose answer (after its repair turn) as
        /// `invalid_output`, with no tables; the host refuses that run, so no
        /// `site_drafted` is ever built from it.
        #[test]
        fn host_unparseable_agent_draft_is_discarded_without_being_shown() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let text = include_str!(
                "../tests/fixtures/site-draft-answers/unparseable_agent_draft_is_discarded_without_being_shown.txt"
            );
            assert!(crate::site_draft_answer::parse_site_draft_answer(text, TOOLS).is_err());
            let mut s = setup("");
            let failed = RunView {
                state: RunState::Failed,
                error_kind: Some(brauser_protocol::RunErrorKind::InvalidOutput),
                site_draft: None,
                ..site_run(TOOLS, Vec::new())
            };
            let dir = fake::short_dir();
            let (_service, link) = service(&dir, failed);
            let response = super::super::draft_site(
                agent_request(TOOLS, &["/docs/{num}"], &[]),
                &mut s.config,
                Some(&link),
            );
            assert!(
                !matches!(response, Response::SiteDrafted(_)),
                "{response:?}"
            );
            assert_eq!(code(response), ErrorCode::InvalidRequest);
        }

        /// Borderline (Review Focus 7): an agent rule that passes every check
        /// but also matches visited shapes the structural pass dropped as
        /// search or paging pages, which are not artifacts and not on the
        /// probe list. The preview row is the only place the review shows
        /// the rule captures them too, so every one is listed with that rule.
        #[test]
        fn agent_preview_lists_every_non_artifact_shape_a_borderline_draft_matches() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let borderline = ["/docs/{slug}?page={num}", "/docs/{slug}?q={slug}"];
            let elsewhere = "/notes/{slug}?sort={slug}";
            let visited = ["/docs/{id}", borderline[0], borderline[1], elsewhere];
            let mut s = setup("");
            let structural = drafted(draft_site(request(TOOLS, &visited, &[]), &mut s.config));
            for shape in borderline.iter().chain([&elsewhere]) {
                assert!(
                    structural
                        .dropped
                        .iter()
                        .any(|d| d.shape.as_deref() == Some(*shape)
                            && d.reason == DraftDropReason::SearchOrPaging),
                    "{shape}: {:?}",
                    structural.dropped
                );
                assert!(!catalog().probe_paths.iter().any(|p| p == shape), "{shape}");
            }
            // The rule passes every check; its token ID starts it unchecked.
            let tables = vec![table("tools.docs", &["/docs/{t:token(1,128)}"], "t")];
            let site = judge(&mut s, &tables, &visited, &[]);
            assert!(site.dropped.is_empty(), "{:?}", site.dropped);
            assert_eq!(site.adapters.len(), 1);
            let rule = &site.adapters[0].rules[0];
            assert_eq!(rule.unchecked_reason, Some(UncheckedReason::TokenOnly));
            assert_eq!(rule.visited, 3, "{rule:?}");
            for shape in borderline {
                let row = site
                    .preview
                    .iter()
                    .find(|r| r.shape == shape)
                    .unwrap_or_else(|| panic!("{shape} previewed"));
                assert_eq!(row.source, PreviewSource::Visited);
                assert_eq!((row.adapter, row.rule), (Some(0), Some(0)), "{row:?}");
                assert_eq!(
                    row.artifact_id.as_deref(),
                    Some("tools.docs:tools.example.com/example-page"),
                    "{row:?}"
                );
            }
            let row = site.preview.iter().find(|r| r.shape == elsewhere).unwrap();
            assert_eq!(row.adapter, None, "{row:?}");
        }

        /// The point of the feature (DESIGN §5.1): a visited page the
        /// structural pass could not draft is covered by a valid anchored
        /// agent rule, which passes the same checks and starts checked.
        #[test]
        fn host_agent_can_add_a_tier0_rule_the_structural_pass_missed_and_it_passes_the_same_checks()
         {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let visited = ["/docs/{num}", "/{uuid}/view/{num}"];
            let mut s = setup("");
            let structural = drafted(draft_site(request(TOOLS, &visited, &[]), &mut s.config));
            assert!(
                structural
                    .dropped
                    .iter()
                    .any(|d| d.shape.as_deref() == Some("/{uuid}/view/{num}")
                        && d.reason == DraftDropReason::NoRouteWord),
                "{:?}",
                structural.dropped
            );
            assert_eq!(structural.preview[1].adapter, None);
            let tables = answer(fixture!(
                "agent_can_add_a_tier0_rule_the_structural_pass_missed_and_it_passes_the_same_checks"
            ));
            let site = judge(&mut s, &tables, &visited, &[]);
            assert!(site.dropped.is_empty(), "{:?}", site.dropped);
            assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
            assert_visited_rule_kept_and_checked(&site, "/{_}/view/{n:int}");
            let row = &site.preview[1];
            assert_eq!(row.shape, "/{uuid}/view/{num}");
            assert_eq!((row.adapter, row.rule), (Some(1), Some(0)));
            assert_eq!(checked_type(&site, &row.url).as_deref(), Some("tools.view"));
            assert_eq!(checked_type(&site, "https://tools.example.com/"), None);
            // The save judges the agent's selection as it would any draft.
            let chosen: Vec<Json> = site.adapters.iter().map(|a| a.adapter.clone()).collect();
            let provenance = chosen.iter().map(drafted_rules).collect();
            let scope = vec![None; chosen.len()];
            let mut check_request = check(TOOLS, &site.revision, chosen, provenance, scope);
            check_request.visited = counts(&visited);
            let verdicts = checked(check_site_draft(check_request, &mut s.config));
            assert!(
                verdicts
                    .rules
                    .iter()
                    .flatten()
                    .all(|r| r.verdict == RuleVerdictKind::Kept),
                "{verdicts:?}"
            );
        }
    }
}

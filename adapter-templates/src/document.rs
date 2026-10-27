//! The validated adapter document and its checks (§3-§10.3).
//!
//! Validation follows the check order in `protocol/adapter-templates/README.md`
//! exactly and reports the first failure. Once built, a [`Document`] is
//! immutable, and every template in it is known to be well formed.

use std::collections::HashSet;

use url::Url;

use crate::ctype::{BaseType, CType, is_id_literal_byte};
use crate::error::{ErrorCode, Field, ValidationError};
use crate::eval::MAX_ID_BYTES;
use crate::path::{PathTemplate, Segment, is_anchor, is_name, parse_capture};
use crate::title::{MAX_TITLE_CHARS, Part, TitleTemplate};
use crate::value::Value;

pub const FORMAT: i64 = 1;
pub const MAX_ADAPTERS: usize = 64;
pub const MAX_RULES: usize = 16;
const MAX_QUERY: usize = 8;
const MAX_ABSENT: usize = 8;
const MAX_TITLES: usize = 4;
const MAX_SCOPE_VALUES: usize = 64;
const MAX_REF_SPECS: usize = 8;
const MAX_TYPE: usize = 32;
const MAX_SCHEME: usize = 32;

/// Origins that host many unrelated tenants (§6). Host-owned; later releases
/// may extend it.
pub const SHARED_ORIGINS: &[&str] = &[
    "https://github.com",
    "https://docs.google.com",
    "https://www.notion.so",
    "https://www.figma.com",
    "https://linear.app",
];

const ADAPTER_KEYS: &[&str] = &[
    "type",
    "origin",
    "match",
    "id",
    "title",
    "alias",
    "refs",
    "modes",
    "scope",
    "unscoped",
    "path_case",
    "fields",
    "within",
];
const RULE_KEYS: &[&str] = &["path", "query", "absent", "mode"];
const REF_TYPES: &[BaseType] = &[
    BaseType::JiraKey,
    BaseType::Int,
    BaseType::Hex,
    BaseType::Uuid,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    View,
    Edit,
    Review,
}

impl Mode {
    fn parse(s: &str) -> Option<Mode> {
        Some(match s {
            "view" => Mode::View,
            "edit" => Mode::Edit,
            "review" => Mode::Review,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::View => "view",
            Mode::Edit => "edit",
            Mode::Review => "review",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum QueryCond {
    Capture { name: String, ctype: CType },
    Literal(String),
    Presence,
}

#[derive(Clone, Debug)]
pub(crate) struct Rule {
    pub(crate) path: PathTemplate,
    pub(crate) query: Vec<(String, QueryCond)>,
    pub(crate) absent: Vec<String>,
    pub(crate) mode: Option<Mode>,
}

impl Rule {
    /// Every named capture, path first, with its optionality.
    pub(crate) fn captures(&self) -> Vec<(&str, &CType, bool)> {
        let mut out: Vec<(&str, &CType, bool)> = self.path.captures().collect();
        for (_, cond) in &self.query {
            if let QueryCond::Capture { name, ctype } = cond {
                out.push((name, ctype, false));
            }
        }
        out
    }

    pub(crate) fn capture(&self, name: &str) -> Option<(&CType, bool)> {
        self.captures()
            .into_iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, c, o)| (c, o))
    }

    /// Where each capture of [`Rule::captures`] is written, in the same order:
    /// its path segment's offset, or the start of its query value.
    fn capture_sites(&self) -> Vec<(&str, Field, usize)> {
        let mut out: Vec<(&str, Field, usize)> = self
            .path
            .capture_offsets()
            .map(|(n, at)| (n, Field::Path, at))
            .collect();
        for (key, cond) in &self.query {
            if let QueryCond::Capture { name, .. } = cond {
                out.push((name, Field::Query(key.clone()), 0));
            }
        }
        out
    }

    /// Where the first capture with this name is written.
    fn site(&self, name: &str) -> (Field, Option<usize>) {
        self.capture_sites()
            .into_iter()
            .find(|(n, _, _)| *n == name)
            .map_or((Field::Path, None), |(_, f, at)| (f, Some(at)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IdPart {
    Capture(String),
    Literal(String),
}

#[derive(Clone, Debug)]
pub(crate) struct IdTemplate {
    pub(crate) scheme: String,
    pub(crate) parts: Vec<IdPart>,
    /// The character offset of each part in the template text.
    offsets: Vec<usize>,
}

impl IdTemplate {
    /// `scheme ":" "{host}" { "/" id_part }`. An `id_syntax` error carries the
    /// character offset where the text stops fitting: 0 for the scheme, else
    /// the start of the offending piece.
    fn parse(text: &str) -> Result<IdTemplate, usize> {
        let (scheme, rest) = text.split_once(':').ok_or(0usize)?;
        let mut sb = scheme.bytes();
        let scheme_ok = scheme.len() <= MAX_SCHEME
            && sb.next().is_some_and(|b| b.is_ascii_lowercase())
            && sb.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.');
        if !scheme_ok {
            return Err(0);
        }
        // The scheme is ASCII, so byte and character offsets agree up to here.
        let mut at = scheme.len() + 1;
        let rest = rest.strip_prefix("{host}").ok_or(at)?;
        at += "{host}".len();
        let mut parts = Vec::new();
        let mut offsets = Vec::new();
        if !rest.is_empty() {
            let rest = rest.strip_prefix('/').ok_or(at)?;
            for part in rest.split('/') {
                at += 1;
                if let Some(inner) = part.strip_prefix('{') {
                    let name = inner.strip_suffix('}').filter(|n| is_name(n)).ok_or(at)?;
                    parts.push(IdPart::Capture(name.to_owned()));
                } else if !part.is_empty() && part.bytes().all(is_id_literal_byte) {
                    parts.push(IdPart::Literal(part.to_owned()));
                } else {
                    return Err(at);
                }
                offsets.push(at);
                at += part.chars().count();
            }
        }
        Ok(IdTemplate {
            scheme: scheme.to_owned(),
            parts,
            offsets,
        })
    }

    /// Whether two templates build the same ID from the same captures: equal
    /// scheme and parts, ignoring where each part sat in its text.
    pub(crate) fn same_template(&self, other: &IdTemplate) -> bool {
        self.scheme == other.scheme && self.parts == other.parts
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.names_at().map(|(n, _)| n)
    }

    /// Each capture with its character offset in the template text.
    fn names_at(&self) -> impl Iterator<Item = (&str, usize)> {
        self.parts
            .iter()
            .zip(&self.offsets)
            .filter_map(|(p, &at)| match p {
                IdPart::Capture(n) => Some((n.as_str(), at)),
                IdPart::Literal(_) => None,
            })
    }

    /// Builds an ID, or `None` once it passes 512 bytes (§8.1). `value`
    /// supplies each capture; every name is bound by validation. The template
    /// itself has no length cap, so stopping at the cap keeps the work per
    /// visit bounded by the ID limit rather than the config (§10.2).
    pub(crate) fn build<'a>(
        &self,
        host: &str,
        value: impl Fn(&str) -> Option<&'a str>,
    ) -> Option<String> {
        let mut id = format!("{}:{}", self.scheme, host);
        for part in &self.parts {
            if id.len() > MAX_ID_BYTES {
                return None;
            }
            id.push('/');
            match part {
                IdPart::Capture(n) => id.push_str(value(n).unwrap_or_default()),
                IdPart::Literal(l) => id.push_str(l),
            }
        }
        (id.len() <= MAX_ID_BYTES).then_some(id)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Alias {
    TabTitle {
        capture: String,
    },
    Meta {
        name: String,
        capture: String,
        ctype: CType,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct RefSpec {
    pub(crate) artifact_type: String,
    pub(crate) find: BaseType,
}

/// The container an adapter's artifacts sit within (§8.3): its type and an ID
/// template built from captures the adapter's ID or scope already uses.
#[derive(Clone, Debug)]
pub struct Container {
    pub(crate) artifact_type: String,
    pub(crate) id: IdTemplate,
}

impl Container {
    pub fn artifact_type(&self) -> &str {
        &self.artifact_type
    }
}

#[derive(Clone, Debug)]
pub struct Adapter {
    pub(crate) artifact_type: String,
    pub(crate) origin: String,
    /// `{host}` for IDs: the origin's host, with any non-default port.
    pub(crate) host: String,
    pub(crate) insensitive: bool,
    pub(crate) rules: Vec<Rule>,
    pub(crate) id: IdTemplate,
    pub(crate) titles: Vec<TitleTemplate>,
    pub(crate) alias: Option<Alias>,
    pub(crate) refs: Vec<RefSpec>,
    pub(crate) modes: Vec<(String, Mode)>,
    pub(crate) scope: Option<Vec<(String, Vec<String>)>>,
    pub(crate) fields: Vec<(String, String)>,
    pub(crate) within: Option<Container>,
}

impl Adapter {
    pub fn artifact_type(&self) -> &str {
        &self.artifact_type
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// The Tier 1 field selectors, unchanged by this format.
    pub fn fields(&self) -> &[(String, String)] {
        &self.fields
    }

    pub fn scope(&self) -> Option<&[(String, Vec<String>)]> {
        self.scope.as_deref()
    }

    /// The container this adapter's artifacts sit within, if declared.
    pub fn within(&self) -> Option<&Container> {
        self.within.as_ref()
    }

    /// The ctype of a capture the ID uses, from the first rule.
    pub(crate) fn id_ctype(&self, name: &str) -> Option<&CType> {
        self.rules.first()?.capture(name).map(|(c, _)| c)
    }
}

/// A validated adapter document.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pub(crate) adapters: Vec<Adapter>,
}

impl Document {
    /// Validates a config value in the README check order.
    pub fn from_value(doc: &Value) -> Result<Document, ValidationError> {
        validate_document(doc)
    }

    pub fn adapters(&self) -> &[Adapter] {
        &self.adapters
    }
}

impl TryFrom<&Value> for Document {
    type Error = ValidationError;

    fn try_from(v: &Value) -> Result<Self, Self::Error> {
        Document::from_value(v)
    }
}

/// An error builder that carries the adapter index.
#[derive(Clone, Copy)]
struct At {
    adapter: Option<usize>,
    rule: Option<usize>,
    entry: Option<usize>,
}

impl At {
    fn doc() -> At {
        At {
            adapter: None,
            rule: None,
            entry: None,
        }
    }

    fn adapter(a: usize) -> At {
        At {
            adapter: Some(a),
            ..At::doc()
        }
    }

    fn rule(self, r: usize) -> At {
        At {
            rule: Some(r),
            ..self
        }
    }

    fn entry(self, e: usize) -> At {
        At {
            entry: Some(e),
            ..self
        }
    }

    fn err(self, code: ErrorCode) -> ValidationError {
        self.err_at(code, None)
    }

    fn err_at(self, code: ErrorCode, position: Option<usize>) -> ValidationError {
        ValidationError {
            code,
            adapter: self.adapter,
            rule: self.rule,
            entry: self.entry,
            field: None,
            position,
        }
    }

    /// An error inside one template or value, at a character offset in it.
    fn err_in(self, code: ErrorCode, field: Field, position: Option<usize>) -> ValidationError {
        ValidationError {
            field: Some(field),
            ..self.err_at(code, position)
        }
    }

    /// An error about a capture, located where `rule` writes it.
    fn err_capture(self, code: ErrorCode, rule: &Rule, name: &str) -> ValidationError {
        let (field, position) = rule.site(name);
        self.err_in(code, field, position)
    }
}

type VResult<T> = Result<T, ValidationError>;

fn validate_document(doc: &Value) -> VResult<Document> {
    let at = At::doc();
    let Some(table) = doc.as_table() else {
        return Err(at.err(ErrorCode::WrongType));
    };
    let artifacts = doc.get("artifacts");
    match doc.get("format") {
        None => {
            if artifacts
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
            {
                return Err(at.err(ErrorCode::MissingFormat));
            }
        }
        // An integer by value (SQ-V20): `1.0` is format 1, and `2.0` or an
        // integer past i64 is a format this build does not know.
        Some(Value::Int(n)) if *n >= 1 => {
            if *n != FORMAT {
                return Err(at.err(ErrorCode::UnsupportedFormat));
            }
        }
        Some(Value::Float(f)) if f.is_finite() && f.fract() == 0.0 && *f >= 1.0 => {
            if *f != FORMAT as f64 {
                return Err(at.err(ErrorCode::UnsupportedFormat));
            }
        }
        Some(_) => return Err(at.err(ErrorCode::InvalidFormat)),
    }
    if table.iter().any(|(k, _)| k != "format" && k != "artifacts") {
        return Err(at.err(ErrorCode::UnknownKey));
    }
    let list = match artifacts {
        None => &[][..],
        Some(v) => v.as_array().ok_or(at.err(ErrorCode::WrongType))?,
    };
    if list.len() > MAX_ADAPTERS {
        return Err(at.err(ErrorCode::TooManyAdapters));
    }
    let adapters = list
        .iter()
        .enumerate()
        .map(|(i, a)| validate_adapter(a, At::adapter(i)))
        .collect::<VResult<Vec<_>>>()?;
    Ok(Document { adapters })
}

fn is_adapter_type(s: &str) -> bool {
    let mut b = s.bytes();
    s.len() <= MAX_TYPE
        && b.next().is_some_and(|c| c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'_')
}

/// SQ15: `http(s)`, equal to its own WHATWG origin serialization, and a host
/// that is an IP address or an ASCII hostname.
fn check_origin(origin: &str) -> Option<String> {
    let url = Url::parse(origin).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.origin().ascii_serialization() != origin {
        return None;
    }
    if let Some(url::Host::Domain(d)) = url.host()
        && !d.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_')
        })
    {
        return None;
    }
    origin.split_once("://").map(|(_, host)| host.to_owned())
}

fn validate_adapter(v: &Value, at: At) -> VResult<Adapter> {
    // 1. Keys and value types.
    let Some(table) = v.as_table() else {
        return Err(at.err(ErrorCode::WrongType));
    };
    if table
        .iter()
        .any(|(k, _)| !ADAPTER_KEYS.contains(&k.as_str()))
    {
        return Err(at.err(ErrorCode::UnknownKey));
    }
    let (Some(ty), Some(origin), Some(rules_v), Some(id_v)) =
        (v.get("type"), v.get("origin"), v.get("match"), v.get("id"))
    else {
        return Err(at.err(ErrorCode::MissingKey));
    };
    let wrong = || at.err(ErrorCode::WrongType);
    let ty = ty.as_str().ok_or_else(wrong)?;
    let origin = origin.as_str().ok_or_else(wrong)?;
    let rules_v = rules_v.as_array().ok_or_else(wrong)?;
    let id_text = id_v.as_str().ok_or_else(wrong)?;
    let path_case = v
        .get("path_case")
        .map(|p| p.as_str().ok_or_else(wrong))
        .transpose()?;
    let unscoped = v
        .get("unscoped")
        .map(|p| p.as_str().ok_or_else(wrong))
        .transpose()?;
    let modes_v = v
        .get("modes")
        .map(|p| p.as_table().ok_or_else(wrong))
        .transpose()?;
    let scope_v = v
        .get("scope")
        .map(|p| p.as_table().ok_or_else(wrong))
        .transpose()?;
    // `within` is exactly `{type, id}`, both strings (§8.3).
    let within_v = v
        .get("within")
        .map(|w| {
            let t = w.as_table().filter(|t| t.len() == 2).ok_or_else(wrong)?;
            let ty = w.get("type").and_then(Value::as_str);
            let id = w.get("id").and_then(Value::as_str);
            match (t.iter().all(|(k, _)| k == "type" || k == "id"), ty, id) {
                (true, Some(ty), Some(id)) => Ok((ty, id)),
                _ => Err(wrong()),
            }
        })
        .transpose()?;
    let modes = match modes_v {
        None => None,
        Some(t) => Some(
            t.iter()
                .map(|(k, m)| Ok((k.as_str(), m.as_str().ok_or_else(wrong)?)))
                .collect::<VResult<Vec<_>>>()?,
        ),
    };
    let scope = match scope_v {
        None => None,
        Some(t) => Some(
            t.iter()
                .map(|(k, list)| {
                    let list = list.as_array().ok_or_else(wrong)?;
                    let values = list
                        .iter()
                        .map(|s| s.as_str().map(str::to_owned).ok_or_else(wrong))
                        .collect::<VResult<Vec<_>>>()?;
                    Ok((k.clone(), values))
                })
                .collect::<VResult<Vec<_>>>()?,
        ),
    };

    // 2. type, origin, path_case.
    if !is_adapter_type(ty) {
        return Err(at.err(ErrorCode::BadAdapterType));
    }
    if ty == "web.page" {
        return Err(at.err(ErrorCode::ReservedAdapterType));
    }
    let host = check_origin(origin).ok_or(at.err(ErrorCode::BadOrigin))?;
    let insensitive = match path_case {
        None | Some("sensitive") => false,
        Some("insensitive") => true,
        Some(_) => return Err(at.err(ErrorCode::BadPathCase)),
    };

    // 3. Rules.
    if rules_v.is_empty() {
        return Err(at.err(ErrorCode::NoRules));
    }
    if rules_v.len() > MAX_RULES {
        return Err(at.err(ErrorCode::TooManyRules));
    }
    let rules = rules_v
        .iter()
        .enumerate()
        .map(|(r, rv)| validate_rule(rv, at.rule(r)))
        .collect::<VResult<Vec<_>>>()?;

    // 4. Same-named captures share one ctype; `mode` is exempt (SQ-V13).
    for (r, rule) in rules.iter().enumerate() {
        for (name, ctype, _) in rule.captures() {
            if name == "mode" {
                continue;
            }
            let earlier = rules[..r].iter().find_map(|e| e.capture(name));
            if earlier.is_some_and(|(c, _)| c != ctype) {
                return Err(at
                    .rule(r)
                    .err_capture(ErrorCode::CaptureTypeConflict, rule, name));
            }
        }
    }

    // 5. Modes (§5.4).
    for (r, rule) in rules.iter().enumerate() {
        if rule
            .capture("mode")
            .is_some_and(|(c, _)| !c.is_alternation())
        {
            return Err(at
                .rule(r)
                .err_capture(ErrorCode::ModeNotAlternation, rule, "mode"));
        }
    }
    for (r, rule) in rules.iter().enumerate() {
        if let Some((CType::Alternation(lits), _)) = rule.capture("mode") {
            let mapped = |l: &String| {
                modes
                    .as_ref()
                    .is_some_and(|m| m.iter().any(|(k, _)| k == l))
            };
            if !lits.iter().all(mapped) {
                return Err(at
                    .rule(r)
                    .err_capture(ErrorCode::ModeMissingMapping, rule, "mode"));
            }
        }
    }
    // Every value is checked, but only keys some rule's `mode` alternation
    // can produce are kept (unused keys are ignored, SQ18), so the lookup at
    // evaluation is bounded by the rule limits, not by the table's size.
    let used = |k: &str| {
        rules.iter().any(|rule| {
            matches!(rule.capture("mode"), Some((CType::Alternation(lits), _))
                if lits.iter().any(|l| l == k))
        })
    };
    let mut kept = Vec::new();
    for (k, m) in modes.unwrap_or_default() {
        let mode = Mode::parse(m).ok_or(at.err(ErrorCode::BadModesValue))?;
        if used(k) {
            kept.push((k.to_owned(), mode));
        }
    }
    let modes = kept;

    // 6. ID (§8.1).
    let in_id = |code, p| at.err_in(code, Field::Id, Some(p));
    let id = IdTemplate::parse(id_text).map_err(|p| in_id(ErrorCode::IdSyntax, p))?;
    if id.scheme == "web" {
        return Err(in_id(ErrorCode::IdReservedScheme, 0));
    }
    if let Some((_, p)) = id.names_at().find(|(n, _)| *n == "mode") {
        return Err(in_id(ErrorCode::ModeInId, p));
    }
    // No rule can bind a reserved name (SQ-V19).
    if let Some((_, p)) = id.names_at().find(|(n, _)| *n == "host" || *n == "title") {
        return Err(in_id(ErrorCode::IdUnknownCapture, p));
    }
    for (r, rule) in rules.iter().enumerate() {
        for (name, p) in id.names_at() {
            let code = match rule.capture(name) {
                None => ErrorCode::IdUnknownCapture,
                Some((_, true)) => ErrorCode::IdOptionalCapture,
                Some(_) => continue,
            };
            return Err(at.rule(r).err_in(code, Field::Id, Some(p)));
        }
    }
    let scoped = |n: &str| {
        scope
            .as_ref()
            .is_some_and(|s| s.iter().any(|(k, _)| k == n))
    };
    for (r, rule) in rules.iter().enumerate() {
        for (name, _, _) in rule.captures() {
            if name != "mode" && !scoped(name) && !id.names().any(|n| n == name) {
                return Err(at.rule(r).err_capture(ErrorCode::ExtraCapture, rule, name));
            }
        }
    }

    // 7. Scope (§6).
    let shared = SHARED_ORIGINS.contains(&origin);
    if scope.is_some() && unscoped.is_some() {
        return Err(at.err(ErrorCode::ScopeAndUnscoped));
    }
    if unscoped.is_some_and(|u| u != "acknowledged") {
        return Err(at.err(ErrorCode::BadUnscoped));
    }
    if unscoped.is_some() && !shared {
        return Err(at.err(ErrorCode::UnscopedNotShared));
    }
    if let Some(scope) = &scope {
        if scope.is_empty() || scope.iter().any(|(_, l)| l.is_empty()) {
            return Err(at.err(ErrorCode::ScopeEmptyList));
        }
        if scope.iter().any(|(_, l)| l.len() > MAX_SCOPE_VALUES) {
            return Err(at.err(ErrorCode::TooManyScopeValues));
        }
        for (r, rule) in rules.iter().enumerate() {
            for (name, _) in scope {
                if !matches!(rule.capture(name), Some((_, false))) {
                    return Err(at.rule(r).err(ErrorCode::ScopeCaptureNotBound));
                }
            }
        }
        // A value must be one some URL could produce (SQ-V22): canonical for
        // at least one rule's binding. Only `mode` may differ between rules.
        for (name, values) in scope {
            let producible = |v: &String| {
                rules.iter().any(|rule| {
                    rule.capture(name)
                        .is_some_and(|(c, _)| c.canonical_whole(v).as_deref() == Some(v.as_str()))
                })
            };
            if !values.iter().all(producible) {
                return Err(at.err(ErrorCode::ScopeBadValue));
            }
        }
    }
    if shared && scope.is_none() && unscoped.is_none() {
        return Err(at.err(ErrorCode::SharedOriginUnscoped));
    }
    // 7.9. Anchors (§4.1): each rule needs one among its required path
    // segments. A required path capture named by a scope key anchors too:
    // the scope list pins it the way a literal would. A query binding never
    // anchors, and an unscoped adapter gets no scope anchor.
    for (r, rule) in rules.iter().enumerate() {
        let anchored = rule.path.segments[..rule.path.required].iter().any(|s| {
            is_anchor(s)
                || matches!(s, Segment::Capture { name, optional: false, .. } if scoped(name))
        });
        if !anchored {
            return Err(at
                .rule(r)
                .err_in(ErrorCode::NoLiteralSegment, Field::Path, Some(0)));
        }
    }

    // 8. Title (§9.1).
    let titles = match v.get("title") {
        None => Vec::new(),
        Some(t) => validate_titles(t, &rules, at)?,
    };

    // 9. Alias (§9.3).
    let alias = match v.get("alias") {
        None => None,
        Some(a) => Some(validate_alias(a, &rules, &id, &titles, at)?),
    };

    // 10. Refs (§10.1).
    let refs = match v.get("refs") {
        None => Vec::new(),
        Some(r) => validate_refs(r, at)?,
    };

    // 11. Tier 1 fields.
    let fields = match v.get("fields") {
        None => Vec::new(),
        Some(f) => f
            .as_table()
            .ok_or_else(wrong)?
            .iter()
            .map(|(k, s)| Ok((k.clone(), s.as_str().ok_or_else(wrong)?.to_owned())))
            .collect::<VResult<Vec<_>>>()?,
    };

    // 12. Container (§8.3). Its captures are the ID's and scope's, so step 6's
    // extra_capture is unchanged.
    let within = match within_v {
        None => None,
        Some((wty, wid)) => {
            if !is_adapter_type(wty) || wty == "web.page" {
                return Err(at.err(ErrorCode::WithinBadType));
            }
            if wty == ty {
                return Err(at.err(ErrorCode::WithinSelf));
            }
            let in_within = |code, p| at.err_in(code, Field::Within, Some(p));
            let cid =
                IdTemplate::parse(wid).map_err(|p| in_within(ErrorCode::WithinIdSyntax, p))?;
            if cid.scheme == "web" {
                return Err(in_within(ErrorCode::WithinIdReservedScheme, 0));
            }
            // Its own ID would give the artifact and its container one
            // record key, which is no container at all.
            if cid.same_template(&id) {
                return Err(at.err(ErrorCode::WithinSelf));
            }
            for (r, rule) in rules.iter().enumerate() {
                for (name, p) in cid.names_at() {
                    let known = name != "mode"
                        && rule.capture(name).is_some()
                        && (scoped(name) || id.names().any(|n| n == name));
                    if !known {
                        return Err(at.rule(r).err_in(
                            ErrorCode::WithinUnknownCapture,
                            Field::Within,
                            Some(p),
                        ));
                    }
                }
            }
            Some(Container {
                artifact_type: wty.to_owned(),
                id: cid,
            })
        }
    };

    Ok(Adapter {
        artifact_type: ty.to_owned(),
        origin: origin.to_owned(),
        host,
        insensitive,
        rules,
        id,
        titles,
        alias,
        refs,
        modes,
        scope,
        fields,
        within,
    })
}

fn validate_rule(v: &Value, at: At) -> VResult<Rule> {
    let wrong = || at.err(ErrorCode::WrongType);
    let Some(table) = v.as_table() else {
        return Err(wrong());
    };
    if table.iter().any(|(k, _)| !RULE_KEYS.contains(&k.as_str())) {
        return Err(at.err(ErrorCode::UnknownKey));
    }
    let path_text = v
        .get("path")
        .ok_or(at.err(ErrorCode::MissingKey))?
        .as_str()
        .ok_or_else(wrong)?;
    let query_v = v
        .get("query")
        .map(|q| q.as_table().ok_or_else(wrong))
        .transpose()?;
    let mut query_v = query_v
        .unwrap_or_default()
        .iter()
        .map(|(k, q)| Ok((k.as_str(), q.as_str().ok_or_else(wrong)?)))
        .collect::<VResult<Vec<_>>>()?;
    // Conditions are checked in ascending key order by code point (SQ-V21),
    // which a JSON-parsed object in the worker can reproduce and document
    // order cannot.
    query_v.sort_by(|a, b| a.0.cmp(b.0));
    let absent_v = v
        .get("absent")
        .map(|a| a.as_array().ok_or_else(wrong))
        .transpose()?;
    let absent = absent_v
        .unwrap_or_default()
        .iter()
        .map(|k| k.as_str().map(str::to_owned).ok_or_else(wrong))
        .collect::<VResult<Vec<_>>>()?;
    let mode_text = v
        .get("mode")
        .map(|m| m.as_str().ok_or_else(wrong))
        .transpose()?;

    let path =
        PathTemplate::parse(path_text).map_err(|(c, p)| at.err_in(c, Field::Path, Some(p)))?;

    if query_v.len() > MAX_QUERY {
        return Err(at.err(ErrorCode::TooManyQuery));
    }
    let mut query = Vec::with_capacity(query_v.len());
    for (key, value) in query_v {
        let in_value = |(c, p)| at.err_in(c, Field::Query(key.to_owned()), Some(p));
        if key.is_empty() {
            return Err(in_value((ErrorCode::BadQueryKey, 0)));
        }
        query.push((key.to_owned(), parse_query_value(value).map_err(in_value)?));
    }

    if absent.len() > MAX_ABSENT {
        return Err(at.err(ErrorCode::TooManyAbsent));
    }
    for (i, key) in absent.iter().enumerate() {
        let in_entry = |c| at.err_in(c, Field::Absent(i), Some(0));
        if key.is_empty() {
            return Err(in_entry(ErrorCode::BadQueryKey));
        }
        if query.iter().any(|(k, _)| k == key) || absent[..i].contains(key) {
            return Err(in_entry(ErrorCode::QueryKeyConflict));
        }
    }

    let mode = mode_text
        .map(|m| Mode::parse(m).ok_or(at.err(ErrorCode::BadMode)))
        .transpose()?;

    let rule = Rule {
        path,
        query,
        absent,
        mode,
    };
    let mut seen = HashSet::new();
    if let Some((_, field, p)) = rule
        .capture_sites()
        .into_iter()
        .find(|(n, _, _)| !seen.insert(*n))
    {
        return Err(at.err_in(ErrorCode::DuplicateCapture, field, Some(p)));
    }
    Ok(rule)
}

/// SQ12: `{_}` is presence, a braced value is a capture, anything else a literal.
/// Errors carry a character offset: 0 for a capture, else the first character
/// a literal may not contain.
fn parse_query_value(value: &str) -> Result<QueryCond, (ErrorCode, usize)> {
    if value == "{_}" {
        return Ok(QueryCond::Presence);
    }
    if value.len() >= 2 && value.starts_with('{') && value.ends_with('}') {
        let content = &value[1..value.len() - 1];
        if content == "*" {
            return Err((ErrorCode::BadQueryValue, 0));
        }
        return match parse_capture(content) {
            Ok((_, _, true)) => Err((ErrorCode::OptionalInQuery, 0)),
            Ok((name, ctype, false)) => Ok(QueryCond::Capture { name, ctype }),
            Err(ErrorCode::PathSyntax) => Err((ErrorCode::BadQueryValue, 0)),
            Err(e) => Err((e, 0)),
        };
    }
    // §5.1's controls are C0 and DEL only; C1 characters are ordinary text.
    let bad = value
        .chars()
        .position(|c| matches!(c, '{' | '}' | '\0'..='\x1f' | '\x7f'));
    if value.is_empty() || bad.is_some() {
        return Err((ErrorCode::BadQueryValue, bad.unwrap_or(0)));
    }
    Ok(QueryCond::Literal(value.to_owned()))
}

/// The ctype of a capture as any rule binds it; same-named captures share one
/// ctype, except `mode`, whose first binding is used.
fn find_ctype<'a>(rules: &'a [Rule], name: &str) -> Option<&'a CType> {
    rules.iter().find_map(|r| r.capture(name).map(|(c, _)| c))
}

fn validate_titles(t: &Value, rules: &[Rule], at: At) -> VResult<Vec<TitleTemplate>> {
    let entries: Vec<&Value> = match t {
        Value::String(_) => vec![t],
        Value::Array(list) => {
            if list.is_empty() {
                return Err(at.err(ErrorCode::EmptyTitleList));
            }
            if list.len() > MAX_TITLES {
                return Err(at.err(ErrorCode::TooManyTitles));
            }
            list.iter().collect()
        }
        _ => return Err(at.err(ErrorCode::WrongType)),
    };
    let mut out = Vec::with_capacity(entries.len());
    for (e, entry) in entries.into_iter().enumerate() {
        let at = at.entry(e);
        let text = entry.as_str().ok_or(at.err(ErrorCode::WrongType))?;
        let in_title = |c, p| at.err_in(c, Field::Title, Some(p));
        // The cap comes first, so parsing and every later check see at most
        // 512 scalars; the position is the first scalar past it.
        if text.chars().nth(MAX_TITLE_CHARS).is_some() {
            return Err(in_title(ErrorCode::TitleTooLong, MAX_TITLE_CHARS));
        }
        let mut tpl = TitleTemplate::parse(text).map_err(|(c, p)| in_title(c, p))?;
        let caps: Vec<_> = tpl
            .captures()
            .map(|(i, n, b, bounds)| (i, n.to_owned(), b, bounds))
            .collect();
        let mut ctypes = Vec::with_capacity(caps.len());
        // A `mode` capture is known but never agrees: it is an alternation
        // (§9.1), so it is a type mismatch rather than an unknown name.
        for (i, name, _, _) in &caps {
            let ctype = find_ctype(rules, name)
                .ok_or_else(|| in_title(ErrorCode::TitleUnknownCapture, tpl.offsets[*i]))?;
            ctypes.push((*i, ctype));
        }
        for ((i, _, base, bounds), (_, ctype)) in caps.iter().zip(&ctypes) {
            let agrees = match ctype {
                CType::Typed {
                    base: b,
                    bounds: eb,
                    ..
                } => b == base && bounds.is_none_or(|tb| *b != BaseType::Uuid && tb == *eb),
                CType::Alternation(_) => false,
            };
            if !agrees {
                return Err(in_title(ErrorCode::TitleTypeMismatch, tpl.offsets[*i]));
            }
        }
        if let Some(i) = tpl.swallows() {
            return Err(in_title(ErrorCode::TitleCaptureSwallows, tpl.offsets[i]));
        }
        for (i, ctype) in ctypes {
            tpl.ctypes[i] = Some(ctype.clone());
        }
        out.push(tpl);
    }
    Ok(out)
}

fn validate_alias(
    a: &Value,
    rules: &[Rule],
    id: &IdTemplate,
    titles: &[TitleTemplate],
    at: At,
) -> VResult<Alias> {
    let Some(table) = a.as_table() else {
        return Err(at.err(ErrorCode::WrongType));
    };
    let in_alias = |c| at.err_in(c, Field::Alias, Some(0));
    let bad = || in_alias(ErrorCode::BadAlias);
    let keys: Vec<&str> = table.iter().map(|(k, _)| k.as_str()).collect();
    let string = |k: &str| a.get(k).and_then(Value::as_str);
    let has_keys =
        |want: &[&str]| keys.len() == want.len() && want.iter().all(|k| keys.contains(k));
    let alias = match string("from") {
        Some("tab_title") if has_keys(&["from", "capture"]) => Alias::TabTitle {
            capture: string("capture").ok_or_else(bad)?.to_owned(),
        },
        Some("meta") if has_keys(&["from", "name", "value"]) => {
            let name = string("name").filter(|n| !n.is_empty()).ok_or_else(bad)?;
            let value = string("value").ok_or_else(bad)?;
            let content = value
                .strip_prefix('{')
                .and_then(|v| v.strip_suffix('}'))
                .ok_or_else(bad)?;
            let (capture, ctype, optional) = parse_capture(content).map_err(|_| bad())?;
            if optional {
                return Err(bad());
            }
            Alias::Meta {
                name: name.to_owned(),
                capture,
                ctype,
            }
        }
        _ => return Err(bad()),
    };
    let capture = match &alias {
        Alias::TabTitle { capture } | Alias::Meta { capture, .. } => capture,
    };
    if !id.names().any(|n| n == capture) {
        return Err(in_alias(ErrorCode::AliasCaptureNotId));
    }
    match &alias {
        Alias::Meta { capture, ctype, .. } => {
            if find_ctype(rules, capture) != Some(ctype) {
                return Err(in_alias(ErrorCode::AliasTypeMismatch));
            }
        }
        Alias::TabTitle { capture } => {
            let in_title = titles.iter().any(|t| {
                t.parts
                    .iter()
                    .any(|p| matches!(p, Part::Capture { name, .. } if name == capture))
            });
            if !in_title {
                return Err(in_alias(ErrorCode::AliasCaptureNotInTitle));
            }
        }
    }
    Ok(alias)
}

fn validate_refs(r: &Value, at: At) -> VResult<Vec<RefSpec>> {
    let list = r.as_array().ok_or(at.err(ErrorCode::WrongType))?;
    if list.len() > MAX_REF_SPECS {
        return Err(at.err(ErrorCode::TooManyRefs));
    }
    let mut out = Vec::with_capacity(list.len());
    for (e, entry) in list.iter().enumerate() {
        let at = at.entry(e);
        let bad = || at.err(ErrorCode::BadRef);
        let table = entry.as_table().ok_or_else(bad)?;
        if table.len() != 2 {
            return Err(bad());
        }
        let ty = entry.get("type").and_then(Value::as_str).ok_or_else(bad)?;
        let find = entry.get("find").and_then(Value::as_str).ok_or_else(bad)?;
        if !is_adapter_type(ty) {
            return Err(bad());
        }
        let find = BaseType::from_name(find)
            .filter(|b| REF_TYPES.contains(b))
            .ok_or(at.err(ErrorCode::BadRefFind))?;
        out.push(RefSpec {
            artifact_type: ty.to_owned(),
            find,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(json: &str) -> Result<Document, ValidationError> {
        Document::from_value(&serde_json::from_str(json).unwrap())
    }

    #[test]
    fn a_minimal_adapter_validates() {
        let d = doc(r#"{"format": 1, "artifacts": [{"type": "acme.item",
            "origin": "https://tools.example.com",
            "match": [{"path": "/items/{id:int}"}], "id": "acme:{host}/{id}"}]}"#)
        .unwrap();
        assert_eq!(d.adapters()[0].host, "tools.example.com");
    }

    #[test]
    fn format_is_decided_first() {
        let e = doc(r#"{"format": 2, "bogus": true}"#).unwrap_err();
        assert_eq!(e.code, ErrorCode::UnsupportedFormat);
        let e = doc(r#"{"format": 1, "bogus": true}"#).unwrap_err();
        assert_eq!(e.code, ErrorCode::UnknownKey);
    }

    fn one(adapter: &str) -> Result<Document, ValidationError> {
        doc(&format!(
            r#"{{"format": 1, "artifacts": [{{"type": "acme.item",
            "origin": "https://tools.example.com", {adapter}}}]}}"#
        ))
    }

    fn located(adapter: &str) -> (ErrorCode, Option<Field>, Option<usize>) {
        let e = one(adapter).unwrap_err();
        (e.code, e.field, e.position)
    }

    #[test]
    fn errors_point_into_the_template_they_concern() {
        use ErrorCode::*;
        let m = r#""match": [{"path": "/x/{id:int}"}]"#;
        assert_eq!(
            located(&format!(r#"{m}, "id": "a:{{host}}/{{id}}/X""#)),
            (IdSyntax, Some(Field::Id), Some(14))
        );
        assert_eq!(
            located(&format!(r#"{m}, "id": "a:{{host}}/{{nope}}""#)),
            (IdUnknownCapture, Some(Field::Id), Some(9))
        );
        assert_eq!(
            located(r#""match": [{"path": "/x/{*}/{id:int}"}], "id": "a:{host}/{id}""#),
            (RestNotLast, Some(Field::Path), Some(3))
        );
        assert_eq!(
            located(
                r#""match": [{"path": "/x/{id:int}", "query": {"k": "a{b"}}], "id": "a:{host}/{id}""#
            ),
            (BadQueryValue, Some(Field::Query("k".into())), Some(1))
        );
        assert_eq!(
            located(
                r#""match": [{"path": "/x/{id:int}", "absent": ["a", "a"]}], "id": "a:{host}/{id}""#
            ),
            (QueryKeyConflict, Some(Field::Absent(1)), Some(0))
        );
        assert_eq!(
            located(r#""match": [{"path": "/x/{id:int}/{id:int}"}], "id": "a:{host}/{id}""#),
            (DuplicateCapture, Some(Field::Path), Some(12))
        );
        assert_eq!(
            located(r#""match": [{"path": "/x/{id:int}/{n:int}"}], "id": "a:{host}/{id}""#),
            (ExtraCapture, Some(Field::Path), Some(12))
        );
        assert_eq!(
            located(&format!(
                r#"{m}, "id": "a:{{host}}/{{id}}", "title": "{{id:int}}{{~}}{{title}}""#
            )),
            (TitleUnboundedCapture, Some(Field::Title), Some(8))
        );
        assert_eq!(
            located(&format!(
                r#"{m}, "id": "a:{{host}}/{{id}}", "title": "{{title}} {{no:int}}""#
            )),
            (TitleUnknownCapture, Some(Field::Title), Some(8))
        );
        assert_eq!(
            located(&format!(
                r#"{m}, "id": "a:{{host}}/{{id}}", "alias": {{"from": "x"}}"#
            )),
            (BadAlias, Some(Field::Alias), Some(0))
        );
    }

    #[test]
    fn format_is_read_by_value() {
        let code = |f: &str| doc(&format!(r#"{{"format": {f}}}"#)).err().map(|e| e.code);
        assert_eq!(code("1.0"), None);
        assert_eq!(code("2.0"), Some(ErrorCode::UnsupportedFormat));
        assert_eq!(code("1e300"), Some(ErrorCode::UnsupportedFormat));
        assert_eq!(
            code("9223372036854775808"),
            Some(ErrorCode::UnsupportedFormat)
        );
        assert_eq!(code("1.5"), Some(ErrorCode::InvalidFormat));
        assert_eq!(code("0"), Some(ErrorCode::InvalidFormat));
    }

    #[test]
    fn scope_values_need_one_rule_that_produces_them() {
        let adapter = |values: &str| {
            one(&format!(
                r#""match": [{{"path": "/x/{{id:int}}/{{mode:(view)}}"}},
                    {{"path": "/y/{{id:int}}/{{mode:(files)}}"}}],
                "modes": {{"view": "view", "files": "review"}},
                "scope": {{"mode": [{values}]}}, "id": "a:{{host}}/{{id}}""#
            ))
        };
        assert!(adapter(r#""files""#).is_ok());
        assert_eq!(
            adapter(r#""files", "edit""#).unwrap_err().code,
            ErrorCode::ScopeBadValue
        );
    }

    #[test]
    fn a_mode_title_capture_is_a_type_mismatch() {
        let e = one(
            r#""match": [{"path": "/x/{id:int}/{mode:(view)}"}], "modes": {"view": "view"},
            "id": "a:{host}/{id}", "title": "{title} {mode:name}""#,
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::TitleTypeMismatch);
    }

    #[test]
    fn query_conditions_are_checked_in_key_order() {
        let e = one(
            r#""match": [{"path": "/x/{id:int}", "query": {"z": "{host:int}", "7": "{_:int}"}}],
            "id": "a:{host}/{id}""#,
        )
        .unwrap_err();
        assert_eq!(e.field, Some(Field::Query("7".into())));
    }

    #[test]
    fn origins_must_be_their_own_serialization() {
        for bad in [
            "https://Tools.example.com",
            "https://tools.example.com:443",
            "https://*.example.com",
            "https://tools.example.com/",
        ] {
            assert!(check_origin(bad).is_none(), "{bad}");
        }
        assert_eq!(
            check_origin("http://[::1]:8080").as_deref(),
            Some("[::1]:8080")
        );
    }
}

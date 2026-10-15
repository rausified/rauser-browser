//! Evaluation (§5, §6, §8, §9, §10.1): from a URL and tab title to an
//! artifact ID, mode, title, alias, and refs.
//!
//! Every step is a single forward pass over its input. Rules are tried in
//! config order, and the first rule whose path and query match decides the
//! result.

use std::fmt;

use url::Url;

use crate::ctype::BaseType;
use crate::document::{Adapter, Alias, Document, IdPart, Mode, QueryCond, Rule};
use crate::path::prepare_segments;
use crate::title::prepare_title;

pub const MAX_URL_CHARS: usize = 2048;
pub const MAX_ID_BYTES: usize = 512;
pub const MAX_REFS: usize = 20;
const UUID_LEN: usize = 36;

/// Why a URL is not an artifact, in the order the reasons are decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoMatch {
    UrlTooLong,
    UrlInvalid,
    NoAdapter,
    PathRejected,
    NoRule,
    OutOfScope,
    IdTooLong,
}

impl NoMatch {
    pub fn as_str(self) -> &'static str {
        match self {
            NoMatch::UrlTooLong => "url_too_long",
            NoMatch::UrlInvalid => "url_invalid",
            NoMatch::NoAdapter => "no_adapter",
            NoMatch::PathRejected => "path_rejected",
            NoMatch::NoRule => "no_rule",
            NoMatch::OutOfScope => "out_of_scope",
            NoMatch::IdTooLong => "id_too_long",
        }
    }
}

impl fmt::Display for NoMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A value a ref scan found, before resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub artifact_type: String,
    pub value: String,
}

/// A ref after resolution; `artifact_id` is `None` when it stays unresolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRef {
    pub artifact_type: String,
    pub value: String,
    pub artifact_id: Option<String>,
}

/// The title-derived part of a match (§9, §10.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleMatch {
    /// The extracted title, or the whole prepared title for the fallback.
    pub title: String,
    /// The matching entry, or `None` for the fallback.
    pub entry: Option<usize>,
    /// The suggested tab-title alias ID.
    pub alias_id: Option<String>,
    pub refs: Vec<Ref>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub adapter: usize,
    pub rule: usize,
    pub artifact_type: String,
    /// Every bound capture's canonical value: path captures in template
    /// order, then query captures.
    pub captures: Vec<(String, String)>,
    pub artifact_id: String,
    pub mode: Option<Mode>,
    /// Present when a title was given.
    pub title: Option<TitleMatch>,
}

impl Match {
    pub fn capture(&self, name: &str) -> Option<&str> {
        self.captures
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A URL prepared once for every rule (§5.1).
pub(crate) struct Prepared {
    pub(crate) origin: String,
    pub(crate) segments: Option<Vec<String>>,
    pub(crate) query: Vec<(String, String)>,
}

impl Prepared {
    pub(crate) fn new(raw: &str) -> Result<Prepared, NoMatch> {
        if raw.chars().count() > MAX_URL_CHARS {
            return Err(NoMatch::UrlTooLong);
        }
        let url = Url::parse(raw).map_err(|_| NoMatch::UrlInvalid)?;
        // Only an http(s) URL is ever an artifact. A `blob:` URL carries its
        // creator's origin but addresses an in-memory object, not a page.
        let origin = url.origin();
        if !matches!(url.scheme(), "http" | "https") || !origin.is_tuple() {
            return Err(NoMatch::NoAdapter);
        }
        let query = url::form_urlencoded::parse(url.query().unwrap_or_default().as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        Ok(Prepared {
            origin: origin.ascii_serialization(),
            segments: prepare_segments(url.path()),
            query,
        })
    }
}

/// Matches one rule, without scope. Returns the bound captures.
pub(crate) fn match_rule(
    adapter: &Adapter,
    rule: &Rule,
    segments: &[String],
    query: &[(String, String)],
) -> Option<Vec<(String, String)>> {
    let mut caps = Vec::new();
    if !rule.path.matches(segments, adapter.insensitive, &mut caps) {
        return None;
    }
    if rule
        .absent
        .iter()
        .any(|key| query.iter().any(|(k, _)| k == key))
    {
        return None;
    }
    for (key, cond) in &rule.query {
        let mut values = query.iter().filter(|(k, _)| k == key).map(|(_, v)| v);
        let (Some(value), None) = (values.next(), values.next()) else {
            return None;
        };
        match cond {
            QueryCond::Presence => {
                if value.is_empty() {
                    return None;
                }
            }
            QueryCond::Literal(lit) => {
                if lit != value {
                    return None;
                }
            }
            QueryCond::Capture { name, ctype } => {
                caps.push((name.clone(), ctype.canonical(value)?))
            }
        }
    }
    Some(caps)
}

fn lookup<'a>(caps: &'a [(String, String)], name: &str) -> Option<&'a str> {
    caps.iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// The ID with one capture replaced, for aliases; `None` when it equals the
/// primary ID, is too long, or names a scoped capture with a value outside
/// the scope: an out-of-scope URL is not an artifact (§6), so neither is its
/// ID as an alias.
fn alias_id(
    adapter: &Adapter,
    caps: &[(String, String)],
    name: &str,
    value: &str,
    primary: &str,
) -> Option<String> {
    let out_of_scope = adapter
        .scope
        .iter()
        .flatten()
        .any(|(scoped, values)| scoped == name && !values.iter().any(|v| v == value));
    if out_of_scope {
        return None;
    }
    let id = adapter.id.build(&adapter.host, |n| {
        if n == name {
            Some(value)
        } else {
            lookup(caps, n)
        }
    })?;
    (id != primary).then_some(id)
}

impl Document {
    /// Evaluates a URL and an optional tab title.
    pub fn evaluate(&self, url: &str, title: Option<&str>) -> Result<Match, NoMatch> {
        let prepared = Prepared::new(url)?;
        let same_origin: Vec<(usize, &Adapter)> = self
            .adapters
            .iter()
            .enumerate()
            .filter(|(_, a)| a.origin == prepared.origin)
            .collect();
        if same_origin.is_empty() {
            return Err(NoMatch::NoAdapter);
        }
        let segments = prepared.segments.as_deref().ok_or(NoMatch::PathRejected)?;
        let (index, adapter, r, caps) = same_origin
            .iter()
            .find_map(|&(i, a)| {
                a.rules.iter().enumerate().find_map(|(r, rule)| {
                    match_rule(a, rule, segments, &prepared.query).map(|caps| (i, a, r, caps))
                })
            })
            .ok_or(NoMatch::NoRule)?;

        // Scope is checked on the first matching rule, and is terminal (SQ4).
        if let Some(scope) = &adapter.scope {
            let in_scope = scope.iter().all(|(name, values)| {
                lookup(&caps, name).is_some_and(|v| values.iter().any(|s| s == v))
            });
            if !in_scope {
                return Err(NoMatch::OutOfScope);
            }
        }
        let artifact_id = adapter
            .id
            .build(&adapter.host, |n| lookup(&caps, n))
            .ok_or(NoMatch::IdTooLong)?;
        let rule = &adapter.rules[r];
        let mode = match lookup(&caps, "mode") {
            Some(m) => adapter
                .modes
                .iter()
                .find(|(k, _)| k == m)
                .map(|(_, mode)| *mode),
            None => rule.mode,
        };
        let title = title.map(|t| match_title(adapter, &caps, &artifact_id, t));
        Ok(Match {
            adapter: index,
            rule: r,
            artifact_type: adapter.artifact_type.clone(),
            captures: caps,
            artifact_id,
            mode,
            title,
        })
    }

    /// Resolves refs (§10.1). A ref resolves when exactly one adapter has
    /// the target type and its ID has exactly one placeholder besides
    /// `{host}`. A ref that resolves to `self_id` is dropped.
    pub fn resolve_refs(&self, refs: &[Ref], self_id: Option<&str>) -> Vec<ResolvedRef> {
        let mut out = Vec::with_capacity(refs.len());
        for r in refs {
            let artifact_id = self.resolve_one(r);
            if artifact_id.is_some() && artifact_id.as_deref() == self_id {
                continue;
            }
            out.push(ResolvedRef {
                artifact_type: r.artifact_type.clone(),
                value: r.value.clone(),
                artifact_id,
            });
        }
        out
    }

    fn resolve_one(&self, r: &Ref) -> Option<String> {
        let mut of_type = self
            .adapters
            .iter()
            .filter(|a| a.artifact_type == r.artifact_type);
        let (Some(adapter), None) = (of_type.next(), of_type.next()) else {
            return None;
        };
        let mut names = adapter
            .id
            .parts
            .iter()
            .filter(|p| matches!(p, IdPart::Capture(_)));
        let (Some(IdPart::Capture(name)), None) = (names.next(), names.next()) else {
            return None;
        };
        let value = adapter.id_ctype(name)?.canonical_whole(&r.value)?;
        adapter.id.build(&adapter.host, |_| Some(value.as_str()))
    }

    /// The suggested alias ID from a `meta` alias (§9.3), given the page URL
    /// and the element's `content`.
    pub fn meta_alias(&self, url: &str, content: &str) -> Option<String> {
        let m = self.evaluate(url, None).ok()?;
        let adapter = &self.adapters[m.adapter];
        let Some(Alias::Meta { capture, ctype, .. }) = &adapter.alias else {
            return None;
        };
        let value = ctype.canonical_whole(content)?;
        alias_id(adapter, &m.captures, capture, &value, &m.artifact_id)
    }

    /// The `<meta>` element name a matched adapter reads, if any.
    pub fn meta_alias_name(&self, adapter: usize) -> Option<&str> {
        match &self.adapters.get(adapter)?.alias {
            Some(Alias::Meta { name, .. }) => Some(name),
            _ => None,
        }
    }
}

fn match_title(
    adapter: &Adapter,
    caps: &[(String, String)],
    primary: &str,
    raw: &str,
) -> TitleMatch {
    let prepared = prepare_title(raw);
    let alias_capture = match &adapter.alias {
        Some(Alias::TabTitle { capture }) => Some(capture.as_str()),
        _ => None,
    };
    let mut found = None;
    'entries: for (e, tpl) in adapter.titles.iter().enumerate() {
        let Some((title, tcaps)) = tpl.matches(&prepared) else {
            continue;
        };
        let mut alias = None;
        for (name, value) in &tcaps {
            match lookup(caps, name) {
                Some(bound) if bound != value => {
                    if alias_capture == Some(name.as_str()) {
                        alias = Some(value.clone());
                    } else {
                        continue 'entries;
                    }
                }
                _ => {}
            }
        }
        let alias_id = alias.and_then(|v| {
            alias_id(
                adapter,
                caps,
                alias_capture.unwrap_or_default(),
                &v,
                primary,
            )
        });
        found = Some((title, Some(e), alias_id));
        break;
    }
    let (title, entry, alias_id) = found.unwrap_or_else(|| (prepared.iter().collect(), None, None));
    // Specs that share a `find` share one scan of the title.
    let mut scans: Vec<(BaseType, Vec<String>)> = Vec::new();
    let mut refs: Vec<Ref> = Vec::new();
    'specs: for spec in &adapter.refs {
        let values = match scans.iter().position(|(f, _)| *f == spec.find) {
            Some(i) => &scans[i].1,
            None => {
                scans.push((spec.find, scan_refs(spec.find, &title)));
                &scans[scans.len() - 1].1
            }
        };
        for value in values {
            let r = Ref {
                artifact_type: spec.artifact_type.clone(),
                value: value.clone(),
            };
            if !refs.contains(&r) {
                if refs.len() == MAX_REFS {
                    break 'specs;
                }
                refs.push(r);
            }
        }
    }
    TitleMatch {
        title,
        entry,
        alias_id,
        refs,
    }
}

/// A ref boundary: anything but ASCII `[A-Za-z0-9_]`.
fn is_boundary(c: char) -> bool {
    !(c.is_ascii_alphanumeric() || c == '_')
}

/// Scans text for ref values (§10.1): the distinct canonical values, in
/// first-seen order, at most 20. Candidates start only at a boundary, so
/// each character is scanned a bounded number of times.
pub fn scan_refs(find: BaseType, text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let run = |from: usize, pred: fn(&char) -> bool| {
        from + chars[from..].iter().take_while(|c| pred(c)).count()
    };
    let mut out: Vec<String> = Vec::new();
    for p in 0..n {
        if out.len() == MAX_REFS {
            break;
        }
        if p > 0 && !is_boundary(chars[p - 1]) {
            continue;
        }
        let end = match find {
            BaseType::JiraKey => {
                if !chars[p].is_ascii_uppercase() {
                    continue;
                }
                let dash = run(p + 1, |c| {
                    c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_'
                });
                if chars.get(dash) != Some(&'-') {
                    continue;
                }
                run(dash + 1, char::is_ascii_digit)
            }
            BaseType::Int => run(p, char::is_ascii_digit),
            BaseType::Hex => run(p, char::is_ascii_hexdigit),
            BaseType::Uuid => p + UUID_LEN,
            BaseType::Token | BaseType::Name => return out,
        };
        if end > n || end == p || chars.get(end).is_some_and(|&c| !is_boundary(c)) {
            continue;
        }
        let candidate: String = chars[p..end].iter().collect();
        let bounds = find.default_bounds().unwrap_or((0, 0));
        if let Some(value) = find.canonical(bounds, &candidate)
            && !out.contains(&value)
        {
            out.push(value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_need_boundaries_and_uppercase() {
        assert_eq!(
            scan_refs(BaseType::JiraKey, "See OPS-12 and ENG_2-7"),
            ["OPS-12", "ENG_2-7"]
        );
        assert!(scan_refs(BaseType::JiraKey, "covid-19 xOPS-1 OPS-1x").is_empty());
        assert_eq!(scan_refs(BaseType::Int, "#12 and 012, 12"), ["12"]);
        assert_eq!(
            scan_refs(BaseType::Uuid, "{0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D}"),
            ["0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d"]
        );
    }

    #[test]
    fn a_huge_id_template_costs_no_more_than_the_id_cap() {
        // About 2 MB of literal parts: the template validates, but every ID
        // it builds is over 512 bytes. Building the whole string took about
        // 35 ms per visit in a debug build; stopping at the cap takes
        // microseconds.
        let id = format!("acme:{{host}}/{{id}}{}", "/a".repeat(1_000_000));
        let json = serde_json::json!({"format": 1, "artifacts": [{
            "type": "acme.item", "origin": "https://acme.example.com",
            "match": [{"path": "/items/{id:int}"}], "id": id,
            "title": "{title} {id:int}", "alias": {"from": "tab_title", "capture": "id"},
        }]});
        let doc = Document::from_value(&serde_json::from_value(json).unwrap()).unwrap();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(
                doc.evaluate("https://acme.example.com/items/7", Some("x 8")),
                Err(NoMatch::IdTooLong)
            );
        }
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
    }
}

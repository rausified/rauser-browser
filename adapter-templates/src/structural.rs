//! The structural drafter (DESIGN §5.1): a deterministic, agent-free draft
//! of one adapter per kind of page, built only from private URL shapes.
//!
//! The steps run in the order DESIGN §5.1 gives them. This module holds the
//! first three:
//!
//! 1. **Filter** drops the root, the probe paths, blocked route words,
//!    search and paging identifiers, `{param}` names and every shape an
//!    existing adapter on the origin already recognizes, and marks the
//!    shared-origin tenant and account-index positions.
//! 2. **Group** groups what is left into kinds, keyed by the route words and
//!    placeholder classes up to the last identifier.
//! 3. **Keep** keeps a kind only when a route word comes before its first
//!    identifier.
//!
//! Every collection is sorted by shape string before it is grouped, so the
//! order of the input never changes the output. Link shapes come from the
//! page and are untrusted: they never change how visited shapes group, and
//! what only links support always forms a rule of its own.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use crate::document::Document;
use crate::eval::{Prepared, match_rule};
use crate::policy::{DraftPolicy, SharedOrigin};
use crate::shape::{QueryName, ShapeClass, ShapePart, UrlShape};

/// The drafter's input: shapes with their counts. `visited` holds the
/// current page and the origin's sample, `links` the click-only link shapes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShapeInput {
    pub visited: Vec<(UrlShape, u32)>,
    pub links: Vec<(UrlShape, u32)>,
}

/// Why a shape, or every shape of a kind, is not drafted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DropReason {
    /// The origin root.
    Root,
    /// One of the catalog's probe paths.
    Probe,
    /// Its only identifiers are search or paging parameters.
    SearchOrPaging,
    /// It uses a blocked route word.
    BlockedRoute,
    /// Its only identifiers are under `{param}` names.
    ParamName,
    /// An existing adapter on the origin already recognizes it.
    AlreadyRecognized,
    /// No route word comes before its first identifier.
    NoRouteWord,
    /// It has no identifier at all.
    NoIdentifier,
    /// It falls outside the rule or adapter limits.
    Limit,
}

impl DropReason {
    pub fn as_str(self) -> &'static str {
        match self {
            DropReason::Root => "root",
            DropReason::Probe => "probe",
            DropReason::SearchOrPaging => "search_or_paging",
            DropReason::BlockedRoute => "blocked_route",
            DropReason::ParamName => "param_name",
            DropReason::AlreadyRecognized => "already_recognized",
            DropReason::NoRouteWord => "no_route_word",
            DropReason::NoIdentifier => "no_identifier",
            DropReason::Limit => "limit",
        }
    }
}

/// A shape the drafter did not use, and why.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Dropped {
    pub shape: String,
    pub reason: DropReason,
}

/// Where a shape came from. Visited sorts first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Source {
    Visited,
    Link,
}

/// One position of a drafted path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Part {
    Empty,
    Literal(String),
    /// Route words that differ in one position of a kind, sorted.
    Alternation(Vec<String>),
    Class(ShapeClass),
    /// The shared origin's tenant position, whatever the shaper kept there.
    Tenant,
    /// `{_}`: an account index or a short trailing slug.
    Anon,
    /// `{*}`: a folded trailing run of slugs.
    Rest,
}

impl Part {
    /// Whether this position holds an identifier: any placeholder class but
    /// a slug, which is title-like text.
    fn is_identifier(&self) -> bool {
        matches!(self, Part::Class(class) if *class != ShapeClass::Slug)
    }

    /// The part as it groups: `{hex32}` and `{tailhex32}` are one class.
    fn grouped(&self) -> Part {
        match self {
            Part::Class(ShapeClass::Hex32) => Part::Class(ShapeClass::TailHex32),
            other => other.clone(),
        }
    }
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Part::Empty => Ok(()),
            Part::Literal(word) => f.write_str(word),
            Part::Alternation(words) => write!(f, "({})", words.join("|")),
            Part::Class(class) => write!(f, "{class}"),
            Part::Tenant => f.write_str("{tenant}"),
            Part::Anon => f.write_str("{_}"),
            Part::Rest => f.write_str("{*}"),
        }
    }
}

/// The text of a path and its query identifiers, such as
/// `/wiki/(blog|pages)/{num}?rev={num}`.
fn render(path: &[Part], query: &[(String, ShapeClass)]) -> String {
    let mut out = String::new();
    for part in path {
        let _ = write!(out, "/{part}");
    }
    for (index, (name, class)) in query.iter().enumerate() {
        let _ = write!(out, "{}{name}={class}", if index == 0 { '?' } else { '&' });
    }
    out
}

/// A shape that passed the filter.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Tagged {
    /// The shape as the extension sent it.
    shape: String,
    source: Source,
    count: u32,
    /// The path with the tenant and account-index positions marked.
    path: Vec<Part>,
    /// The identifier query pairs, sorted by name.
    query: Vec<(String, ShapeClass)>,
}

/// One rule of a kind, before it is mapped to a template.
#[derive(Clone, Debug, PartialEq, Eq)]
struct KindRule {
    path: Vec<Part>,
    query: Vec<(String, ShapeClass)>,
    /// Summed counts of the visited and link shapes behind the rule.
    visited: u32,
    links: u32,
    /// The shapes behind the rule, sorted.
    shapes: Vec<String>,
}

impl KindRule {
    /// Backed only by link shapes, so it starts unchecked in the review.
    fn link_only(&self) -> bool {
        self.visited == 0
    }

    fn text(&self) -> String {
        render(&self.path, &self.query)
    }
}

/// One kind of page: shapes that share route words and placeholder classes
/// up to the last identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Kind {
    /// The path up to the last identifier, with the kind's first route word
    /// at the varying position.
    base: Vec<Part>,
    query: Vec<(String, ShapeClass)>,
    /// The one path position whose route word differs within the kind.
    varying: Option<usize>,
    /// The rules, visited-backed first, then by text.
    rules: Vec<KindRule>,
}

impl Kind {
    /// Every shape behind the kind, sorted.
    fn shapes(&self) -> Vec<String> {
        let mut shapes: Vec<String> = self
            .rules
            .iter()
            .flat_map(|r| r.shapes.iter().cloned())
            .collect();
        shapes.sort();
        shapes.dedup();
        shapes
    }
}

/// Sorts drops and removes repeats, such as one shape both visited and
/// linked.
fn tidy(mut dropped: Vec<Dropped>) -> Vec<Dropped> {
    dropped.sort();
    dropped.dedup();
    dropped
}

/// Whether the shape is one of the probe paths: the same literal path and
/// the same query names.
fn is_probe(shape: &UrlShape, policy: &DraftPolicy) -> bool {
    policy.probe_paths.iter().any(|probe| {
        let (path, query) = probe.split_once('?').unwrap_or((probe, ""));
        let segments: Vec<&str> = path.split('/').skip(1).collect();
        let same_path = segments.len() == shape.path.len()
            && segments
                .iter()
                .zip(&shape.path)
                .all(|(segment, part)| match part {
                    ShapePart::Empty => segment.is_empty(),
                    ShapePart::Literal(word) => word == segment,
                    ShapePart::Placeholder(_) => false,
                });
        let mut probe_keys: Vec<&str> = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| pair.split_once('=').map_or(pair, |(key, _)| key))
            .collect();
        let mut shape_keys: Vec<&str> = shape
            .query
            .iter()
            .map(|(name, _)| match name {
                QueryName::Literal(word) => word.as_str(),
                QueryName::Param => "{param}",
            })
            .collect();
        probe_keys.sort_unstable();
        shape_keys.sort_unstable();
        same_path && probe_keys == shape_keys
    })
}

/// Whether an existing adapter on `origin` recognizes the filled shape,
/// matching each rule without scope as the draft overlap check does: an
/// owner-scoped adapter would not recognize the filler owner otherwise.
fn recognized(shape: &UrlShape, origin: &str, existing: &Document, policy: &DraftPolicy) -> bool {
    let Ok(url) = Prepared::new(&shape.fill(origin, policy)) else {
        return false;
    };
    let Some(segments) = url.segments.as_deref() else {
        return false;
    };
    existing
        .adapters
        .iter()
        .filter(|a| a.origin == origin && a.origin == url.origin)
        .any(|a| {
            a.rules
                .iter()
                .any(|rule| match_rule(a, rule, segments, &url.query).is_some())
        })
}

/// Step 1, Filter. Returns the shapes that can be drafted, sorted by shape
/// and source with repeats merged, and the drops with their reasons.
#[cfg_attr(not(test), expect(dead_code, reason = "draft_site runs the steps"))]
fn filter(
    origin: &str,
    existing: &Document,
    input: &ShapeInput,
    policy: &DraftPolicy,
) -> (Vec<Tagged>, Vec<Dropped>) {
    let mut merged: BTreeMap<(String, Source), (&UrlShape, u32)> = BTreeMap::new();
    let sources = [
        (Source::Visited, &input.visited),
        (Source::Link, &input.links),
    ];
    for (source, shapes) in sources {
        for (shape, count) in shapes {
            let entry = merged
                .entry((shape.to_string(), source))
                .or_insert((shape, 0));
            entry.1 = entry.1.saturating_add(*count);
        }
    }

    let shared = policy.shared_origin(origin);
    let tenant = shared.and_then(|s| s.tenant.as_ref()).map(|(at, _)| *at);
    let is_listed = |word: &str, list: &[String]| list.iter().any(|w| w == word);

    let mut tagged = Vec::new();
    let mut dropped = Vec::new();
    for ((text, source), (shape, count)) in merged {
        let drop = |reason| Dropped {
            shape: text.clone(),
            reason,
        };
        if shape.path == [ShapePart::Empty] && shape.query.is_empty() {
            dropped.push(drop(DropReason::Root));
            continue;
        }
        if is_probe(shape, policy) {
            dropped.push(drop(DropReason::Probe));
            continue;
        }

        let mut path: Vec<Part> = shape
            .path
            .iter()
            .map(|part| match part {
                ShapePart::Empty => Part::Empty,
                ShapePart::Literal(word) => Part::Literal(word.clone()),
                ShapePart::Placeholder(class) => Part::Class(*class),
            })
            .collect();
        for (literal, at) in shared.iter().flat_map(|s| &s.account_index) {
            if path.get(*at) == Some(&Part::Literal(literal.clone())) && *at + 1 < path.len() {
                path[*at + 1] = Part::Anon;
            }
        }
        if let Some(at) = tenant
            && path.get(at).is_some_and(|part| *part != Part::Empty)
        {
            path[at] = Part::Tenant;
        }

        // The tenant is a name, not a route word, so it is never blocked.
        let blocked = path.iter().any(
            |part| matches!(part, Part::Literal(word) if is_listed(word, &policy.blocked_route_words)),
        );
        if blocked {
            dropped.push(drop(DropReason::BlockedRoute));
            continue;
        }

        let mut query = Vec::new();
        let (mut search_or_paging, mut param) = (false, false);
        for (name, class) in &shape.query {
            match name {
                QueryName::Param => param = true,
                QueryName::Literal(word)
                    if is_listed(word, &policy.search_keys)
                        || is_listed(word, &policy.paging_keys) =>
                {
                    search_or_paging = true
                }
                // A slug value is title-like text, never an identifier.
                QueryName::Literal(_) if *class == ShapeClass::Slug => {}
                QueryName::Literal(word) => query.push((word.clone(), *class)),
            }
        }
        query.sort();
        query.dedup();
        let has_identifier = !query.is_empty() || path.iter().any(Part::is_identifier);
        if !has_identifier && search_or_paging {
            dropped.push(drop(DropReason::SearchOrPaging));
            continue;
        }
        if !has_identifier && param {
            dropped.push(drop(DropReason::ParamName));
            continue;
        }

        if recognized(shape, origin, existing, policy) {
            dropped.push(drop(DropReason::AlreadyRecognized));
            continue;
        }

        tagged.push(Tagged {
            shape: text,
            source,
            count,
            path,
            query,
        });
    }
    (tagged, tidy(dropped))
}

/// Folds a trailing run of slugs after the last identifier: into one `{*}`
/// when at least two segments come before it, otherwise each into `{_}`.
fn fold_trailing_slugs(path: &mut Vec<Part>) {
    let run = path
        .iter()
        .rev()
        .take_while(|part| **part == Part::Class(ShapeClass::Slug))
        .count();
    if run == 0 {
        return;
    }
    let start = path.len() - run;
    if start >= 2 {
        path.truncate(start);
        path.push(Part::Rest);
    } else {
        for part in &mut path[start..] {
            *part = Part::Anon;
        }
    }
}

/// A shape split for grouping: the key path up to and including the last
/// path identifier (the whole path when the identifiers are query-only) and
/// the ending after it.
struct Split<'t> {
    tagged: &'t Tagged,
    key: Vec<Part>,
    ending: Vec<Part>,
}

impl<'t> Split<'t> {
    fn new(tagged: &'t Tagged) -> Split<'t> {
        let mut path = tagged.path.clone();
        fold_trailing_slugs(&mut path);
        let cut = path
            .iter()
            .rposition(Part::is_identifier)
            .map_or(path.len(), |last| last + 1);
        let ending = path.split_off(cut);
        Split {
            tagged,
            key: path,
            ending,
        }
    }

    /// The key as it groups: classes merged and query classes merged.
    fn grouped(&self) -> (Vec<Part>, Vec<(String, Part)>) {
        (
            self.key.iter().map(Part::grouped).collect(),
            self.tagged
                .query
                .iter()
                .map(|(name, class)| (name.clone(), Part::Class(*class).grouped()))
                .collect(),
        )
    }
}

/// A kind while shapes join it.
struct Builder<'t> {
    base: Vec<Part>,
    query: Vec<(String, Part)>,
    varying: Option<usize>,
    members: Vec<Split<'t>>,
}

impl Builder<'_> {
    /// Whether a shape with this grouped key belongs to the kind: the same
    /// key, or one that differs in a single route word, at the kind's
    /// varying position once it has one.
    fn admits(&self, key: &[Part], query: &[(String, Part)]) -> Option<Option<usize>> {
        if key.len() != self.base.len() || query != self.query {
            return None;
        }
        let differ: Vec<usize> = (0..key.len()).filter(|&i| key[i] != self.base[i]).collect();
        match (differ.as_slice(), self.varying) {
            ([], varying) => Some(varying),
            ([at], _)
                if self.varying.is_none_or(|v| v == *at)
                    && matches!(key[*at], Part::Literal(_))
                    && matches!(self.base[*at], Part::Literal(_)) =>
            {
                Some(Some(*at))
            }
            _ => None,
        }
    }
}

/// Step 2, Group. Visited shapes form kinds first, in shape order; link
/// shapes then join those kinds or form their own, so a link can never
/// change how visited shapes group.
#[cfg_attr(not(test), expect(dead_code, reason = "draft_site runs the steps"))]
fn group(tagged: Vec<Tagged>) -> Vec<Kind> {
    let mut splits: Vec<Split<'_>> = tagged.iter().map(Split::new).collect();
    splits.sort_by(|a, b| {
        (a.tagged.source, &a.tagged.shape).cmp(&(b.tagged.source, &b.tagged.shape))
    });

    let mut builders: Vec<Builder<'_>> = Vec::new();
    for split in splits {
        let (key, query) = split.grouped();
        let joined = builders
            .iter()
            .enumerate()
            .find_map(|(i, b)| b.admits(&key, &query).map(|varying| (i, varying)));
        match joined {
            Some((i, varying)) => {
                builders[i].varying = varying;
                builders[i].members.push(split);
            }
            None => builders.push(Builder {
                base: key,
                query,
                varying: None,
                members: vec![split],
            }),
        }
    }

    let mut kinds: Vec<Kind> = builders.into_iter().map(build_kind).collect();
    kinds.sort_by_key(|kind| render(&kind.base, &kind.query));
    kinds
}

/// The grouped key with each merged hex position resolved for `members`:
/// `{tailhex32}` when any of them has one there, so the rule matches both
/// forms, and `{hex32}` otherwise.
fn resolve_classes(
    key: &[Part],
    query: &[(String, Part)],
    members: &[&Split<'_>],
) -> (Vec<Part>, Vec<(String, ShapeClass)>) {
    let tail = Part::Class(ShapeClass::TailHex32);
    let resolve = |grouped: &Part, tailed: bool| match grouped {
        Part::Class(ShapeClass::TailHex32) if !tailed => Part::Class(ShapeClass::Hex32),
        other => other.clone(),
    };
    let path = key
        .iter()
        .enumerate()
        .map(|(at, part)| resolve(part, members.iter().any(|m| m.key[at] == tail)))
        .collect();
    let query = query
        .iter()
        .map(|(name, part)| {
            let tailed = members.iter().any(|m| {
                m.tagged
                    .query
                    .iter()
                    .any(|(n, c)| n == name && *c == ShapeClass::TailHex32)
            });
            let class = match resolve(part, tailed) {
                Part::Class(class) => class,
                _ => ShapeClass::Id,
            };
            (name.clone(), class)
        })
        .collect();
    (path, query)
}

/// A rule of a kind: its ending, then either whether its words are
/// link-only (an alternation) or its single word (a split).
type RuleBucket<'k> = (&'k [Part], bool, Option<String>);

/// Turns a finished kind into rules: one per ending, and per route word at
/// the varying position unless a literal comes before it, in which case the
/// words form one alternation. What only links support is a rule of its own.
fn build_kind(builder: Builder<'_>) -> Kind {
    let Builder {
        base,
        query,
        varying,
        members,
    } = builder;
    let word = |m: &Split<'_>| -> Option<String> {
        varying.and_then(|at| match &m.key[at] {
            Part::Literal(word) => Some(word.clone()),
            _ => None,
        })
    };
    let alternation = varying.is_some_and(|at| {
        base[..at]
            .iter()
            .any(|part| matches!(part, Part::Literal(_)))
    });

    // Words with visited support, per ending.
    let mut visited_words: BTreeMap<&[Part], Vec<String>> = BTreeMap::new();
    for m in &members {
        if m.tagged.source == Source::Visited
            && let Some(w) = word(m)
        {
            visited_words.entry(&m.ending).or_default().push(w);
        }
    }

    // One bucket per rule.
    let mut buckets: BTreeMap<RuleBucket<'_>, Vec<&Split<'_>>> = BTreeMap::new();
    for m in &members {
        let bucket = match word(m) {
            Some(w) if alternation => {
                let backed = visited_words
                    .get(m.ending.as_slice())
                    .is_some_and(|ws| ws.contains(&w));
                (m.ending.as_slice(), !backed, None)
            }
            w => (m.ending.as_slice(), false, w),
        };
        buckets.entry(bucket).or_default().push(m);
    }

    let mut rules: Vec<KindRule> = buckets
        .into_iter()
        .map(|((ending, _, single), backing)| {
            let (mut path, rule_query) = resolve_classes(&base, &query, &backing);
            if let Some(at) = varying {
                let mut words: Vec<String> = match single {
                    Some(w) => vec![w],
                    None => backing.iter().filter_map(|m| word(m)).collect(),
                };
                words.sort();
                words.dedup();
                path[at] = match words.len() {
                    1 => Part::Literal(words.remove(0)),
                    _ => Part::Alternation(words),
                };
            }
            path.extend(ending.iter().cloned());
            let sum = |source: Source| {
                backing
                    .iter()
                    .filter(|m| m.tagged.source == source)
                    .fold(0u32, |n, m| n.saturating_add(m.tagged.count))
            };
            let mut shapes: Vec<String> = backing.iter().map(|m| m.tagged.shape.clone()).collect();
            shapes.sort();
            shapes.dedup();
            KindRule {
                path,
                query: rule_query,
                visited: sum(Source::Visited),
                links: sum(Source::Link),
                shapes,
            }
        })
        .collect();
    rules.sort_by_key(|rule| (rule.link_only(), rule.text()));

    // The kind's base keeps its first route word at the varying position.
    let all: Vec<&Split<'_>> = members.iter().collect();
    let (mut kind_base, kind_query) = resolve_classes(&base, &query, &all);
    if let Some(at) = varying {
        kind_base[at] = base[at].clone();
    }
    Kind {
        base: kind_base,
        query: kind_query,
        varying,
        rules,
    }
}

/// Step 3, Keep. A kind is a candidate when a route word comes before its
/// first path identifier, or, when its identifiers are only in the query,
/// when its path has any route word. The tenant position is never a route
/// word.
#[cfg_attr(not(test), expect(dead_code, reason = "draft_site runs the steps"))]
fn keep(kinds: Vec<Kind>, shared: Option<&SharedOrigin>) -> (Vec<Kind>, Vec<Dropped>) {
    let tenant = shared.and_then(|s| s.tenant.as_ref()).map(|(at, _)| *at);
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for kind in kinds {
        let first = kind.base.iter().position(Part::is_identifier);
        let reason = if first.is_none() && kind.query.is_empty() {
            Some(DropReason::NoIdentifier)
        } else {
            let before = first.unwrap_or(kind.base.len());
            let route_word = kind.base[..before].iter().enumerate().any(|(at, part)| {
                Some(at) != tenant && matches!(part, Part::Literal(_) | Part::Alternation(_))
            });
            (!route_word).then_some(DropReason::NoRouteWord)
        };
        match reason {
            Some(reason) => dropped.extend(
                kind.shapes()
                    .into_iter()
                    .map(|shape| Dropped { shape, reason }),
            ),
            None => kept.push(kind),
        }
    }
    (kept, tidy(dropped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::test_catalog;
    use crate::value::Value;
    use serde_json::{Value as Json, json};

    const TOOLS: &str = "https://tools.example.com";
    const GITHUB: &str = "https://github.com";
    const DOCS: &str = "https://docs.google.com";

    fn shapes(list: &[&str]) -> Vec<(UrlShape, u32)> {
        list.iter()
            .map(|s| (UrlShape::parse(s).unwrap(), 1))
            .collect()
    }

    fn input(visited: &[&str], links: &[&str]) -> ShapeInput {
        ShapeInput {
            visited: shapes(visited),
            links: shapes(links),
        }
    }

    fn doc(adapters: Json) -> Document {
        let v: Value = serde_json::from_value(json!({"format": 1, "artifacts": adapters})).unwrap();
        Document::from_value(&v).unwrap()
    }

    /// Filter, Group and Keep, with every drop.
    fn run(origin: &str, existing: &Document, input: &ShapeInput) -> (Vec<Kind>, Vec<Dropped>) {
        let policy = test_catalog();
        let (tagged, mut dropped) = filter(origin, existing, input, &policy);
        let (kinds, more) = keep(group(tagged), policy.shared_origin(origin));
        dropped.extend(more);
        (kinds, tidy(dropped))
    }

    /// Each kind's rule texts.
    fn texts(kinds: &[Kind]) -> Vec<Vec<String>> {
        kinds
            .iter()
            .map(|k| k.rules.iter().map(KindRule::text).collect())
            .collect()
    }

    fn drops(dropped: &[Dropped]) -> Vec<(&str, DropReason)> {
        dropped
            .iter()
            .map(|d| (d.shape.as_str(), d.reason))
            .collect()
    }

    #[test]
    fn root_probe_search_and_paging_shapes_are_filtered() {
        let (kinds, dropped) = run(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/",
                    "/?q={slug}",
                    "/search?q={slug}",
                    "/issues?page={num}",
                    "/issues?sort={slug}&cursor={id}",
                    "/browse/{key}?page={num}",
                ],
                &[],
            ),
        );
        assert_eq!(texts(&kinds), [["/browse/{key}"]]);
        assert_eq!(
            drops(&dropped),
            [
                ("/", DropReason::Root),
                ("/?q={slug}", DropReason::Probe),
                ("/issues?page={num}", DropReason::SearchOrPaging),
                (
                    "/issues?sort={slug}&cursor={id}",
                    DropReason::SearchOrPaging
                ),
                ("/search?q={slug}", DropReason::Probe),
            ]
        );
    }

    #[test]
    fn blocked_route_word_shapes_are_filtered() {
        let (kinds, dropped) = run(
            TOOLS,
            &Document::default(),
            &input(
                &["/invite/{id}", "/reset/{hex32}", "/wiki/{num}"],
                &["/oauth/{slug}/{num}", "/share/doc/{uuid}"],
            ),
        );
        assert_eq!(texts(&kinds), [["/wiki/{num}"]]);
        assert_eq!(
            drops(&dropped),
            [
                ("/invite/{id}", DropReason::BlockedRoute),
                ("/oauth/{slug}/{num}", DropReason::BlockedRoute),
                ("/reset/{hex32}", DropReason::BlockedRoute),
                ("/share/doc/{uuid}", DropReason::BlockedRoute),
            ]
        );
    }

    #[test]
    fn param_names_are_dropped() {
        let (kinds, dropped) = run(
            TOOLS,
            &Document::default(),
            &input(&["/doc?{param}={num}", "/doc/{num}?{param}={uuid}"], &[]),
        );
        // Only the pair goes when the shape has another identifier.
        assert_eq!(texts(&kinds), [["/doc/{num}"]]);
        assert_eq!(
            drops(&dropped),
            [("/doc?{param}={num}", DropReason::ParamName)]
        );
    }

    #[test]
    fn recognized_shapes_are_filtered() {
        let existing = doc(json!([
            {
                "type": "jira.issue",
                "origin": TOOLS,
                "match": [{"path": "/browse/{key:jira_key}"}],
                "id": "jira.issue:{host}/{key}"
            },
            {
                "type": "other.project",
                "origin": "https://other.example.com",
                "match": [{"path": "/projects/{n:int}"}],
                "id": "other.project:{host}/{n}"
            }
        ]));
        let (kinds, dropped) = run(
            TOOLS,
            &existing,
            &input(&["/browse/{key}", "/projects/{num}"], &["/browse/{key}"]),
        );
        // An adapter on another origin recognizes nothing here.
        assert_eq!(texts(&kinds), [["/projects/{num}"]]);
        assert_eq!(
            drops(&dropped),
            [("/browse/{key}", DropReason::AlreadyRecognized)]
        );
    }

    #[test]
    fn shapes_a_scoped_existing_adapter_would_recognize_are_filtered_as_already_recognized() {
        let existing = doc(json!([{
            "type": "github.issue",
            "origin": GITHUB,
            "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}"}],
            "scope": {"owner": ["acme-corp"]},
            "id": "github.issue:{host}/{owner}/{repo}/{number}"
        }]));
        let shape = UrlShape::parse("/{slug}/{slug}/issues/{num}").unwrap();
        let filled = shape.fill(GITHUB, &test_catalog());
        assert!(
            existing.evaluate(&filled, None).is_err(),
            "the scope hides the filler owner from evaluate"
        );
        let (kinds, dropped) = run(
            GITHUB,
            &existing,
            &input(&["/{slug}/{slug}/issues/{num}"], &[]),
        );
        assert!(kinds.is_empty());
        assert_eq!(
            drops(&dropped),
            [("/{slug}/{slug}/issues/{num}", DropReason::AlreadyRecognized)]
        );
    }

    #[test]
    fn account_index_becomes_anonymous() {
        let (kinds, dropped) = run(
            DOCS,
            &Document::default(),
            &input(&["/document/u/{num}/d/{id}/edit", "/u/{num}/d/{id}"], &[]),
        );
        assert!(dropped.is_empty(), "{dropped:?}");
        assert_eq!(
            texts(&kinds),
            [["/document/u/{_}/d/{id}/edit"], ["/u/{_}/d/{id}"]]
        );
    }

    #[test]
    fn hex32_and_tailhex32_group_as_one_class() {
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(&["/p/{hex32}", "/p/{tailhex32}", "/q/{hex32}"], &[]),
        );
        // `/p/` and `/q/` are one kind split by route word; each rule takes
        // the tail form only where its own shapes have it.
        assert_eq!(texts(&kinds), [["/p/{tailhex32}", "/q/{hex32}"]]);
        assert_eq!(kinds[0].rules[0].visited, 2);
    }

    #[test]
    fn endings_after_last_identifier_share_a_kind() {
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/x/pull/{num}",
                    "/x/pull/{num}/files",
                    "/x/pull/{num}/commits",
                ],
                &[],
            ),
        );
        assert_eq!(
            texts(&kinds),
            [[
                "/x/pull/{num}",
                "/x/pull/{num}/commits",
                "/x/pull/{num}/files"
            ]]
        );
    }

    #[test]
    fn trailing_slug_run_folds_only_after_two_segments() {
        let policy = test_catalog();
        let shapes = input(
            &[
                "/wiki/{num}/{slug}",
                "/wiki/{num}/{slug}/{slug}",
                "/{num}/{slug}",
                "/a/{slug}?id={num}",
            ],
            &[],
        );
        let (tagged, _) = filter(TOOLS, &Document::default(), &shapes, &policy);
        let kinds = group(tagged);
        assert_eq!(
            texts(&kinds),
            [
                vec!["/a/{_}?id={num}"],
                vec!["/wiki/{num}/{*}"],
                vec!["/{num}/{_}"],
            ]
        );
        assert_eq!(kinds[1].rules[0].visited, 2, "both runs fold into one rule");
    }

    #[test]
    fn single_position_route_word_difference_becomes_alternation_after_a_literal() {
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(&["/wiki/pages/{num}", "/wiki/blog/{num}"], &[]),
        );
        assert_eq!(texts(&kinds), [["/wiki/(blog|pages)/{num}"]]);
    }

    #[test]
    fn route_word_difference_without_leading_literal_splits_rules() {
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/pages/{num}",
                    "/blog/{num}",
                    "/wiki/pages/{uuid}",
                    "/docs/blog/{uuid}",
                ],
                &[],
            ),
        );
        // Two positions differing is two kinds.
        assert_eq!(
            texts(&kinds),
            [
                vec!["/blog/{num}", "/pages/{num}"],
                vec!["/docs/blog/{uuid}"],
                vec!["/wiki/pages/{uuid}"],
            ]
        );
    }

    #[test]
    fn link_only_alternative_never_widens_visited_rule() {
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(
                &["/wiki/pages/{num}"],
                &[
                    "/wiki/blog/{num}",
                    "/wiki/pages/{num}",
                    "/wiki/pages/{num}/{slug}/{slug}",
                ],
            ),
        );
        assert_eq!(
            texts(&kinds),
            [[
                "/wiki/pages/{num}",
                "/wiki/blog/{num}",
                "/wiki/pages/{num}/{*}"
            ]]
        );
        let rules = &kinds[0].rules;
        assert_eq!((rules[0].visited, rules[0].links), (1, 1));
        assert!(!rules[0].link_only());
        assert!(rules[1].link_only() && rules[2].link_only());

        // A link that sorts first cannot regroup visited shapes.
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(&["/b/x/{num}", "/b/y/{num}"], &["/a/x/{num}"]),
        );
        assert_eq!(texts(&kinds), [["/a/x/{num}"], ["/b/(x|y)/{num}"]]);
        assert!(kinds[0].rules[0].link_only());
    }

    #[test]
    fn kind_without_route_word_before_identifier_is_not_a_candidate() {
        let (kinds, dropped) = run(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/{num}/edit",
                    "/{uuid}/view/{num}",
                    "/about/{slug}",
                    "/team",
                ],
                &[],
            ),
        );
        assert!(kinds.is_empty());
        assert_eq!(
            drops(&dropped),
            [
                ("/about/{slug}", DropReason::NoIdentifier),
                ("/team", DropReason::NoIdentifier),
                ("/{num}/edit", DropReason::NoRouteWord),
                ("/{uuid}/view/{num}", DropReason::NoRouteWord),
            ]
        );
    }

    #[test]
    fn query_only_identifier_needs_any_path_route_word() {
        let (kinds, dropped) = run(
            TOOLS,
            &Document::default(),
            &input(&["/viewpage?pageid={num}", "/?pageid={num}"], &[]),
        );
        assert_eq!(texts(&kinds), [["/viewpage?pageid={num}"]]);
        assert_eq!(
            drops(&dropped),
            [("/?pageid={num}", DropReason::NoRouteWord)]
        );
    }

    #[test]
    fn tenant_position_is_not_a_route_word() {
        let (kinds, dropped) = run(
            GITHUB,
            &Document::default(),
            &input(
                &[
                    "/acme/{num}",
                    "/acme/{slug}/issues/{num}",
                    "/other/{slug}/issues/{num}",
                    "/share/{slug}/issues/{num}",
                ],
                &[],
            ),
        );
        // Tenants group as one position; a tenant named like a blocked
        // route word is still a tenant.
        assert_eq!(texts(&kinds), [["/{tenant}/{slug}/issues/{num}"]]);
        assert_eq!(kinds[0].rules[0].visited, 3);
        assert_eq!(drops(&dropped), [("/acme/{num}", DropReason::NoRouteWord)]);

        // Off a shared origin the same first segment is a route word.
        let (kinds, _) = run(TOOLS, &Document::default(), &input(&["/acme/{num}"], &[]));
        assert_eq!(texts(&kinds), [["/acme/{num}"]]);
    }

    #[test]
    fn filter_group_and_keep_ignore_input_order() {
        let visited = [
            "/wiki/pages/{num}",
            "/wiki/blog/{num}",
            "/x/{num}/files",
            "/x/{num}",
            "/",
        ];
        let links = ["/wiki/news/{num}", "/x/{num}/{slug}/{slug}"];
        let forward = run(TOOLS, &Document::default(), &input(&visited, &links));
        let mut v = visited;
        let mut l = links;
        v.reverse();
        l.reverse();
        let backward = run(TOOLS, &Document::default(), &input(&v, &l));
        assert_eq!(forward, backward);
    }
}

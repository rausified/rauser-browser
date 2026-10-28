//! The structural drafter (DESIGN §5.1): a deterministic, agent-free draft
//! of one adapter per kind of page, built only from private URL shapes.
//!
//! The steps run in the order DESIGN §5.1 gives them:
//!
//! 1. **Filter** drops the root, the probe paths, blocked route words
//!    (checked before the tenant is marked, so the tenant position cannot
//!    hide one), search, paging and secret identifiers, `{param}` names, a
//!    query name repeated with different classes, and every shape an
//!    existing adapter or a recorded contract on the origin already
//!    recognizes, and marks the shared-origin tenant and account-index
//!    positions.
//! 2. **Group** groups what is left into kinds, keyed by the route words and
//!    placeholder classes up to the last identifier.
//! 3. **Keep** keeps a kind only when a route word comes before its first
//!    identifier.
//! 4. **Map** turns each kind into an adapter table with only the Tier 0
//!    keys `type`, `origin`, `match`, `id` and `modes`.
//! 5. **Name** gives each kind a unique `<site>.<word>` type and ID scheme.
//! 6. **Rank** orders the rules, visited before link-only, and keeps at most
//!    16 rules per adapter and 8 adapters. Kinds are named in rank order, so
//!    the best-supported kind gets the type without a numeric suffix.
//!
//! [`draft_site`] then judges every adapter with [`check_draft_rules`],
//! earlier-ranked adapters first, and reports each dropped shape with its
//! reason. [`draft_kinds`] is the same draft before those checks.
//!
//! Every collection is sorted by shape string before it is grouped, so the
//! order of the input never changes the output. Link shapes come from the
//! page and are untrusted: they never change how visited shapes group, nor
//! the type, ID scheme or rank of a kind with visited support, and what only
//! links support always forms a rule of its own.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use crate::document::{Document, MAX_RULES};
use crate::draft::{
    DraftOutcome, Provenance, RuleVerdict, ScopeState, check_draft_rules, scope_state,
};
use crate::error::DraftErrorCode;
use crate::eval::{Prepared, match_rule};
use crate::policy::{DraftPolicy, SharedOrigin};
use crate::shape::{QueryName, ShapeClass, ShapePart, UrlShape};
use crate::value::Value;

/// Most adapters one draft proposes (DESIGN §5.1).
pub const MAX_DRAFT_ADAPTERS: usize = 8;

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
    /// Its only identifiers are query parameters that carry a secret or a
    /// capability, such as `token` or `sig`.
    SecretQuery,
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
    /// Its drafted rule failed this draft check.
    Check(DraftErrorCode),
}

impl DropReason {
    pub fn as_str(self) -> &'static str {
        match self {
            DropReason::Root => "root",
            DropReason::Probe => "probe",
            DropReason::SearchOrPaging => "search_or_paging",
            DropReason::SecretQuery => "secret_query",
            DropReason::BlockedRoute => "blocked_route",
            DropReason::ParamName => "param_name",
            DropReason::AlreadyRecognized => "already_recognized",
            DropReason::NoRouteWord => "no_route_word",
            DropReason::NoIdentifier => "no_identifier",
            DropReason::Limit => "limit",
            DropReason::Check(code) => code.as_str(),
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
    /// The route word every visited shape has at the varying position, when
    /// only links differ there. It names the kind as it would be named
    /// without those links, so a planted link cannot change the type or ID
    /// scheme of visited pages.
    visited_word: Option<String>,
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

/// Whether an existing adapter or a recorded contract on `origin`
/// recognizes the filled shape, matching each rule without scope as the
/// draft overlap check does: an owner-scoped adapter would not recognize the
/// filler owner otherwise.
fn recognized(
    shape: &UrlShape,
    origin: &str,
    existing: &Document,
    recorded: &[Document],
    policy: &DraftPolicy,
) -> bool {
    let Ok(url) = Prepared::new(&shape.fill(origin, policy)) else {
        return false;
    };
    let Some(segments) = url.segments.as_deref() else {
        return false;
    };
    std::iter::once(existing)
        .chain(recorded)
        .flat_map(|doc| &doc.adapters)
        .filter(|a| a.origin == origin && a.origin == url.origin)
        .any(|a| {
            a.rules
                .iter()
                .any(|rule| match_rule(a, rule, segments, &url.query).is_some())
        })
}

/// Step 1, Filter. Returns the shapes that can be drafted, sorted by shape
/// and source with repeats merged, and the drops with their reasons.
fn filter(
    origin: &str,
    existing: &Document,
    recorded: &[Document],
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
        // Every literal the shaper kept is checked, the tenant position too:
        // a shared origin's `/auth/callback/{id}` is a sign-in route, not a
        // workspace named `auth`.
        let blocked = shape.path.iter().any(
            |part| matches!(part, ShapePart::Literal(word) if policy.is_blocked_route_word(word)),
        );
        if blocked {
            dropped.push(drop(DropReason::BlockedRoute));
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

        let mut query = Vec::new();
        let (mut search_or_paging, mut secret, mut param) = (false, false, false);
        for (name, class) in &shape.query {
            match name {
                QueryName::Param => param = true,
                QueryName::Literal(word)
                    if is_listed(word, &policy.search_keys)
                        || is_listed(word, &policy.paging_keys) =>
                {
                    search_or_paging = true
                }
                // A token, signature or one-time code is never an ID, even
                // where the value has a fixed shape.
                QueryName::Literal(word) if policy.is_secret_query_key(word) => secret = true,
                // A slug value is title-like text, never an identifier.
                QueryName::Literal(_) if *class == ShapeClass::Slug => {}
                QueryName::Literal(word) => query.push((word.clone(), *class)),
            }
        }
        query.sort();
        query.dedup();
        // A name repeated with different classes would be two conditions on
        // one key, which no adapter table can hold, so the name is dropped.
        let repeated: BTreeSet<String> = query
            .windows(2)
            .filter(|pair| pair[0].0 == pair[1].0)
            .map(|pair| pair[0].0.clone())
            .collect();
        query.retain(|(name, _)| !repeated.contains(name));
        let has_identifier = !query.is_empty() || path.iter().any(Part::is_identifier);
        if !has_identifier && secret {
            dropped.push(drop(DropReason::SecretQuery));
            continue;
        }
        if !has_identifier && search_or_paging {
            dropped.push(drop(DropReason::SearchOrPaging));
            continue;
        }
        if !has_identifier && param {
            dropped.push(drop(DropReason::ParamName));
            continue;
        }
        if !has_identifier && !repeated.is_empty() {
            dropped.push(drop(DropReason::NoIdentifier));
            continue;
        }

        if recognized(shape, origin, existing, recorded, policy) {
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

/// A rule of a kind: its ending, whether it is link-only (its alternation
/// words, or a tail form, have no visited support), and its single word
/// when the kind splits per word.
type RuleBucket<'k> = (&'k [Part], bool, Option<String>);

/// A position holding `{tailhex32}`: a path index or a query name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Tail {
    Path(usize),
    Query(String),
}

type Tails = BTreeSet<Tail>;

/// The positions where a shape has the tail form.
fn tails(m: &Split<'_>) -> Tails {
    let tail = Part::Class(ShapeClass::TailHex32);
    let path = m
        .key
        .iter()
        .enumerate()
        .filter(|(_, part)| **part == tail)
        .map(|(at, _)| Tail::Path(at));
    let query = m
        .tagged
        .query
        .iter()
        .filter(|(_, class)| *class == ShapeClass::TailHex32)
        .map(|(name, _)| Tail::Query(name.clone()));
    path.chain(query).collect()
}

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
    // The rule's link-only flag and single word, before any tail check.
    let flag_and_word = |m: &Split<'_>| -> (bool, Option<String>) {
        match word(m) {
            Some(w) if alternation => {
                let backed = visited_words
                    .get(m.ending.as_slice())
                    .is_some_and(|ws| ws.contains(&w));
                (!backed, None)
            }
            w => (false, w),
        }
    };
    // Where each visited-backed bucket's visited shapes have the tail form.
    let mut visited_tails: BTreeMap<RuleBucket<'_>, Tails> = BTreeMap::new();
    for m in &members {
        if m.tagged.source == Source::Visited {
            let (link_only, single) = flag_and_word(m);
            visited_tails
                .entry((m.ending.as_slice(), link_only, single))
                .or_default()
                .extend(tails(m));
        }
    }
    let mut buckets: BTreeMap<RuleBucket<'_>, Vec<&Split<'_>>> = BTreeMap::new();
    for m in &members {
        let ending = m.ending.as_slice();
        let (link_only, single) = flag_and_word(m);
        // A link with the tail form where the bucket's visited shapes have
        // none would widen their rule, so it goes into a link-only one.
        let widens = m.tagged.source == Source::Link
            && visited_tails
                .get(&(ending, link_only, single.clone()))
                .is_some_and(|visited| !tails(m).is_subset(visited));
        buckets
            .entry((ending, link_only || widens, single))
            .or_default()
            .push(m);
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
    let visited_words: BTreeSet<Option<String>> = members
        .iter()
        .filter(|m| m.tagged.source == Source::Visited)
        .map(word)
        .collect();
    let visited_word = match visited_words.len() {
        1 => visited_words.into_iter().next().flatten(),
        _ => None,
    };
    Kind {
        base: kind_base,
        query: kind_query,
        varying,
        visited_word,
        rules,
    }
}

/// Step 3, Keep. A kind is a candidate when a route word comes before its
/// first path identifier, or, when its identifiers are only in the query,
/// when its path has any route word. The tenant position is never a route
/// word.
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

/// One kept rule of a drafted adapter, as the review shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftedRule {
    /// The rule's index in the adapter's `match`.
    pub rule: usize,
    /// Backed only by link shapes: starts unchecked, "seen only in links on
    /// this page".
    pub link_only: bool,
    /// Its only identifier is a `token` capture: starts unchecked, "loose:
    /// its ID has no fixed shape".
    pub token_only: bool,
    /// Summed counts of the visited and link shapes behind the rule.
    pub visited: u32,
    pub links: u32,
    /// An illustrative URL the rule matches, filled from the catalog's fixed
    /// examples. It is never a URL the user visited.
    pub example: String,
}

/// One drafted adapter.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftedAdapter {
    /// The checked table: only `type`, `origin`, `match`, `id` and an
    /// optional `modes`, never the review's scope, unscoped or title overlays.
    pub adapter: Value,
    pub scope: ScopeState,
    /// `(path segment, capture name)` of the shared-origin tenant the rules
    /// bind, for the review to prefill scope from the current tab.
    pub tenant: Option<(usize, String)>,
    /// One entry per rule of `match`, in order.
    pub rules: Vec<DraftedRule>,
}

/// A site's draft: the adapters, best supported first, and every shape the
/// drafter did not use, sorted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SiteDraft {
    pub adapters: Vec<DraftedAdapter>,
    pub dropped: Vec<Dropped>,
}

/// The capture type a placeholder class maps to.
fn capture_type(class: ShapeClass) -> &'static str {
    match class {
        ShapeClass::Num => "int",
        ShapeClass::Key => "jira_key",
        ShapeClass::Uuid => "uuid",
        ShapeClass::Hex32 => "hex(32)",
        ShapeClass::Hex40 => "hex(40)",
        ShapeClass::Hex64 => "hex(64)",
        ShapeClass::TailHex32 => "hex(32)@tail",
        ShapeClass::Id => "token(1,128)",
        ShapeClass::Slug => "name",
    }
}

/// The capture name a placeholder class starts from.
fn capture_base(class: ShapeClass) -> &'static str {
    match class {
        ShapeClass::Num => "num",
        ShapeClass::Key => "key",
        ShapeClass::Uuid => "uuid",
        ShapeClass::Hex32 | ShapeClass::Hex40 | ShapeClass::Hex64 | ShapeClass::TailHex32 => "hex",
        ShapeClass::Id => "token",
        ShapeClass::Slug => "space",
    }
}

/// A `uuid` or `hexN` identifier: as distinctive as a route word.
fn is_fixed_shape(class: ShapeClass) -> bool {
    matches!(
        class,
        ShapeClass::Uuid
            | ShapeClass::Hex32
            | ShapeClass::Hex40
            | ShapeClass::Hex64
            | ShapeClass::TailHex32
    )
}

/// `base`, or `base_2`, `base_3`… when that is taken. `mode` is never used:
/// a capture of that name maps modes.
fn unique_name(base: &str, used: &mut Vec<String>) -> String {
    let mut name = base.to_owned();
    let mut n = 2;
    while used.contains(&name) || name == "mode" {
        name = format!("{base}_{n}");
        n += 1;
    }
    used.push(name.clone());
    name
}

/// One mapped rule.
#[derive(Clone, Debug)]
struct MappedRule {
    path: String,
    /// Query conditions, sorted by key.
    query: Vec<(String, String)>,
    /// The `(word, mode)` pairs its mode capture uses.
    modes: Vec<(String, String)>,
    visited: u32,
    links: u32,
    /// The shapes behind the rule, sorted.
    shapes: Vec<String>,
}

impl MappedRule {
    fn link_only(&self) -> bool {
        self.visited == 0
    }

    /// The canonical rule text, such as `/viewpage?pageid={num:int}`.
    fn text(&self) -> String {
        let mut out = self.path.clone();
        for (index, (key, cond)) in self.query.iter().enumerate() {
            let _ = write!(out, "{}{key}={cond}", if index == 0 { '?' } else { '&' });
        }
        out
    }

    fn value(&self) -> Value {
        let mut table = vec![("path".to_owned(), Value::String(self.path.clone()))];
        if !self.query.is_empty() {
            let query = self
                .query
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            table.push(("query".to_owned(), Value::Table(query)));
        }
        Value::Table(table)
    }

    /// The fill of its first shape.
    fn example(&self, origin: &str, policy: &DraftPolicy) -> String {
        self.shapes
            .first()
            .and_then(|shape| UrlShape::parse(shape).ok())
            .map(|shape| shape.fill(origin, policy))
            .unwrap_or_default()
    }
}

/// One kind mapped to an adapter, before it is checked.
#[derive(Clone, Debug)]
struct Candidate {
    /// The last route word before the identifier, for the type.
    word: String,
    /// Set by [`name`].
    artifact_type: String,
    /// The ID's captures in path order, then query identifiers by key.
    id_names: Vec<String>,
    tenant: Option<(usize, String)>,
    /// Every identifier the ID uses is a `token`.
    token_only: bool,
    rules: Vec<MappedRule>,
}

impl Candidate {
    fn visited(&self) -> u32 {
        self.rules
            .iter()
            .fold(0u32, |n, r| n.saturating_add(r.visited))
    }

    fn links(&self) -> u32 {
        self.rules
            .iter()
            .fold(0u32, |n, r| n.saturating_add(r.links))
    }

    fn id(&self) -> String {
        let mut id = format!("{}:{{host}}", self.artifact_type);
        for name in &self.id_names {
            let _ = write!(id, "/{{{name}}}");
        }
        id
    }

    /// The Tier 0 adapter table.
    fn table(&self, origin: &str) -> Value {
        let mut table = vec![
            ("type".to_owned(), Value::String(self.artifact_type.clone())),
            ("origin".to_owned(), Value::String(origin.to_owned())),
            (
                "match".to_owned(),
                Value::Array(self.rules.iter().map(MappedRule::value).collect()),
            ),
            ("id".to_owned(), Value::String(self.id())),
        ];
        let modes: BTreeMap<&str, &str> = self
            .rules
            .iter()
            .flat_map(|r| r.modes.iter().map(|(w, m)| (w.as_str(), m.as_str())))
            .collect();
        if !modes.is_empty() {
            let modes = modes
                .into_iter()
                .map(|(w, m)| (w.to_owned(), Value::String(m.to_owned())))
                .collect();
            table.push(("modes".to_owned(), Value::Table(modes)));
        }
        Value::Table(table)
    }

    fn drafted(&self, origin: &str, scope: ScopeState, policy: &DraftPolicy) -> DraftedAdapter {
        DraftedAdapter {
            adapter: self.table(origin),
            scope,
            tenant: self.tenant.clone(),
            rules: self
                .rules
                .iter()
                .enumerate()
                .map(|(rule, r)| DraftedRule {
                    rule,
                    link_only: r.link_only(),
                    token_only: self.token_only,
                    visited: r.visited,
                    links: r.links,
                    example: r.example(origin, policy),
                })
                .collect(),
        }
    }

    /// Every shape behind its rules, each with `reason`.
    fn drop_all(&self, reason: DropReason, dropped: &mut Vec<Dropped>) {
        for rule in &self.rules {
            drop_rule(rule, reason, dropped);
        }
    }
}

fn drop_rule(rule: &MappedRule, reason: DropReason, dropped: &mut Vec<Dropped>) {
    dropped.extend(rule.shapes.iter().map(|shape| Dropped {
        shape: shape.clone(),
        reason,
    }));
}

/// Step 4, Map. Positions up to the last identifier are the kind's and map
/// the same way in every rule, so every rule binds the ID's captures:
///
/// - the shared-origin tenant becomes `{<capture>:name|lower}`;
/// - a `{slug}` after the tenant and before the first identifier becomes a
///   namespace, `{space:name}`; any other `{slug}` becomes `{_}`;
/// - each identifier becomes a typed capture. When the kind has a fixed-shape
///   identifier (`uuid` or `hexN`), only those enter the ID and the other
///   identifiers become `{_}`;
/// - an alternation of mode words, or else a mode word in the ending, becomes
///   `{mode:(…)}` and enters `modes`.
///
/// Query identifiers are already off the search and paging lists, so each
/// becomes a query condition. Rules that map to the same text merge.
///
/// Same-named captures share one type within an adapter, so a kind whose
/// rules disagree on `hex(32)` and `hex(32)@tail` maps to one candidate per
/// set of capture types instead of widening a rule. A link-only tail rule
/// that shadows a visited hex rule is then dropped by the overlap check.
fn map(kind: &Kind, shared: Option<&SharedOrigin>, policy: &DraftPolicy) -> Vec<Candidate> {
    let tenant_capture = shared.and_then(|s| s.tenant.as_ref()).map(|(_, c)| c);
    let tenant_at = kind.base.iter().position(|p| *p == Part::Tenant);
    let first_id = kind.base.iter().position(Part::is_identifier);
    let path_classes = kind.base.iter().filter_map(|p| match p {
        Part::Class(class) => Some(*class),
        _ => None,
    });
    let fixed = path_classes
        .chain(kind.query.iter().map(|(_, class)| *class))
        .any(is_fixed_shape);
    let in_id = |class: ShapeClass| !fixed || is_fixed_shape(class);

    let mut used: Vec<String> = tenant_capture.into_iter().cloned().collect();
    let mut id_names = Vec::new();
    let mut id_classes = Vec::new();
    let mut names: Vec<Option<String>> = Vec::with_capacity(kind.base.len());
    for (at, part) in kind.base.iter().enumerate() {
        let name = match part {
            Part::Tenant => tenant_capture.cloned(),
            Part::Class(ShapeClass::Slug)
                if tenant_at.is_some_and(|t| at > t) && first_id.is_none_or(|f| at < f) =>
            {
                Some(unique_name(capture_base(ShapeClass::Slug), &mut used))
            }
            Part::Class(class) if part.is_identifier() && in_id(*class) => {
                id_classes.push(*class);
                Some(unique_name(capture_base(*class), &mut used))
            }
            _ => None,
        };
        id_names.extend(name.clone());
        names.push(name);
    }
    let mut query_names: Vec<Option<String>> = Vec::with_capacity(kind.query.len());
    for (_, class) in &kind.query {
        let name = in_id(*class).then(|| {
            id_classes.push(*class);
            unique_name(capture_base(*class), &mut used)
        });
        id_names.extend(name.clone());
        query_names.push(name);
    }

    // The last route word before the ID's first path identifier, or in the
    // whole path when the ID's identifiers are only in the query. The
    // varying position holds several words, so it names nothing, unless
    // only links vary there: then it names the kind with the visited word.
    let before = (0..kind.base.len())
        .find(|&at| kind.base[at].is_identifier() && names[at].is_some())
        .unwrap_or(kind.base.len());
    let word = kind.base[..before]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, part)| match part {
            _ if Some(at) == kind.varying => kind.visited_word.clone(),
            Part::Literal(word) => Some(word.clone()),
            _ => None,
        })
        .unwrap_or_default();

    let mode_of = |word: &str| {
        policy
            .mode_words
            .iter()
            .find(|(w, _)| w == word)
            .map(|(w, m)| (w.clone(), m.clone()))
    };
    // Rules by the types of their ID captures, then by text.
    let mut partitions: BTreeMap<Vec<&'static str>, BTreeMap<String, MappedRule>> = BTreeMap::new();
    for rule in &kind.rules {
        let mut signature = Vec::new();
        let mut modes: Vec<(String, String)> = Vec::new();
        let mut segments: Vec<String> = Vec::with_capacity(rule.path.len());
        for (at, part) in rule.path.iter().enumerate() {
            let in_key = at < kind.base.len();
            let named = names.get(at).and_then(Option::as_deref).filter(|_| in_key);
            let segment = match part {
                Part::Empty => String::new(),
                Part::Literal(word) if !in_key && modes.is_empty() && mode_of(word).is_some() => {
                    modes.extend(mode_of(word));
                    format!("{{mode:({word})}}")
                }
                Part::Literal(word) => word.clone(),
                Part::Alternation(words)
                    if modes.is_empty() && words.iter().all(|w| mode_of(w).is_some()) =>
                {
                    modes.extend(words.iter().filter_map(|w| mode_of(w)));
                    format!("{{mode:({})}}", words.join("|"))
                }
                Part::Alternation(words) => format!("({})", words.join("|")),
                Part::Tenant => match named {
                    Some(name) => format!("{{{name}:name|lower}}"),
                    None => "{_}".to_owned(),
                },
                Part::Class(class) => match named {
                    Some(name) => {
                        signature.push(capture_type(*class));
                        format!("{{{name}:{}}}", capture_type(*class))
                    }
                    None => "{_}".to_owned(),
                },
                Part::Anon => "{_}".to_owned(),
                Part::Rest => "{*}".to_owned(),
            };
            segments.push(segment);
        }
        // A trailing slash matches without its empty segment (§5.1).
        if segments.len() > 1 && segments.last().is_some_and(String::is_empty) {
            segments.pop();
        }
        let query = rule
            .query
            .iter()
            .zip(&query_names)
            .map(|((key, class), name)| {
                let cond = match name {
                    Some(name) => {
                        signature.push(capture_type(*class));
                        format!("{{{name}:{}}}", capture_type(*class))
                    }
                    None => "{_}".to_owned(),
                };
                (key.clone(), cond)
            })
            .collect();
        let mapped = MappedRule {
            path: format!("/{}", segments.join("/")),
            query,
            modes,
            visited: rule.visited,
            links: rule.links,
            shapes: rule.shapes.clone(),
        };
        match partitions
            .entry(signature)
            .or_default()
            .entry(mapped.text())
        {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(mapped);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let r = slot.get_mut();
                r.visited = r.visited.saturating_add(mapped.visited);
                r.links = r.links.saturating_add(mapped.links);
                r.shapes.extend(mapped.shapes);
                r.shapes.sort();
                r.shapes.dedup();
            }
        }
    }

    let token_only = !id_classes.is_empty() && id_classes.iter().all(|c| *c == ShapeClass::Id);
    partitions
        .into_values()
        .map(|rules| Candidate {
            word: word.clone(),
            artifact_type: String::new(),
            id_names: id_names.clone(),
            tenant: tenant_at.zip(tenant_capture.cloned()),
            token_only,
            rules: rules.into_values().collect(),
        })
        .collect()
}

/// Reduces a label to lowercase letters and digits, cut to 12 characters.
/// An empty label, or one that starts with a digit or with `web`, becomes
/// `fallback`, so no scheme is ever `web` or starts like one.
fn label(raw: &str, fallback: &str) -> String {
    let reduced: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .take(12)
        .collect();
    if reduced.is_empty()
        || reduced.starts_with(|c: char| c.is_ascii_digit())
        || reduced.starts_with("web")
    {
        fallback.to_owned()
    } else {
        reduced
    }
}

/// Step 5, Name. Each type is `<site>.<word>`, with a number appended when
/// the config, a starter, `taken` or an earlier kind already has it. The ID
/// scheme is the type, so IDs never collide across kinds.
fn name(
    candidates: &mut [Candidate],
    origin: &str,
    existing: &Document,
    policy: &DraftPolicy,
    taken: &[String],
) {
    let host = url::Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
    let site = label(host.split('.').next().unwrap_or_default(), "site");
    let mut used: BTreeSet<String> = taken
        .iter()
        .chain(&policy.starter_types)
        .cloned()
        .chain(existing.adapters.iter().map(|a| a.artifact_type.clone()))
        .collect();
    for candidate in candidates {
        let base = format!("{site}.{}", label(&candidate.word, "page"));
        let mut artifact_type = base.clone();
        let mut n = 2u32;
        while used.contains(&artifact_type) {
            artifact_type = format!("{base}{n}");
            n += 1;
        }
        used.insert(artifact_type.clone());
        candidate.artifact_type = artifact_type;
    }
}

/// Step 6, Rank. Rules backed by a visited shape come first by visit count,
/// then link-only rules by link count, ties by rule text; at most 16 rules
/// per adapter. Adapters order by their visit count, then, for link-only
/// adapters only, link count, then first rule; at most 8. Links never
/// reorder adapters with visits, so they never change which visited kind
/// gets a type without a numeric suffix. What does not fit is dropped as
/// `limit`.
fn rank(mut candidates: Vec<Candidate>) -> (Vec<Candidate>, Vec<Dropped>) {
    let mut dropped = Vec::new();
    for candidate in &mut candidates {
        candidate.rules.sort_by_cached_key(|r| {
            let support = if r.link_only() { r.links } else { r.visited };
            (r.link_only(), Reverse(support), r.text())
        });
        if candidate.rules.len() > MAX_RULES {
            for rule in candidate.rules.split_off(MAX_RULES) {
                drop_rule(&rule, DropReason::Limit, &mut dropped);
            }
        }
    }
    candidates.sort_by_cached_key(|c| {
        let links = if c.visited() == 0 { c.links() } else { 0 };
        (
            Reverse(c.visited()),
            Reverse(links),
            c.rules.first().map(MappedRule::text),
        )
    });
    if candidates.len() > MAX_DRAFT_ADAPTERS {
        for candidate in candidates.split_off(MAX_DRAFT_ADAPTERS) {
            candidate.drop_all(DropReason::Limit, &mut dropped);
        }
    }
    (candidates, tidy(dropped))
}

/// Filter, Group, Keep, Map, Rank and Name.
fn candidates(
    origin: &str,
    existing: &Document,
    recorded: &[Document],
    input: &ShapeInput,
    policy: &DraftPolicy,
    taken: &[String],
) -> (Vec<Candidate>, Vec<Dropped>) {
    let shared = policy.shared_origin(origin);
    let (tagged, mut dropped) = filter(origin, existing, recorded, input, policy);
    let (kinds, unkept) = keep(group(tagged), shared);
    dropped.extend(unkept);
    let mapped = kinds.iter().flat_map(|k| map(k, shared, policy)).collect();
    let (mut ranked, limited) = rank(mapped);
    dropped.extend(limited);
    name(&mut ranked, origin, existing, policy, taken);
    (ranked, tidy(dropped))
}

/// The structural draft before the draft checks: the seam an agent drafter
/// (`brwr-f45`) shares, since [`check_draft_rules`] judges any adapter table.
/// Each adapter carries the origin's scope state. `recorded` is as for
/// [`draft_site`].
pub fn draft_kinds(
    origin: &str,
    existing: &Document,
    recorded: &[Document],
    input: &ShapeInput,
    policy: &DraftPolicy,
    taken: &[String],
) -> SiteDraft {
    let scope = scope_state(existing, Some(origin), policy);
    let (candidates, dropped) = candidates(origin, existing, recorded, input, policy, taken);
    SiteDraft {
        adapters: candidates
            .iter()
            .map(|c| c.drafted(origin, scope.clone(), policy))
            .collect(),
        dropped,
    }
}

/// Drafts the site at `origin` from private URL shapes (DESIGN §5.1).
///
/// `recorded` holds the note-only contracts the host still records (retired
/// `[[adapter_id_history]]` entries and contracts removed from config.toml
/// by hand). They name artifacts and freeze their routes' IDs, so a shape
/// one recognizes is dropped as `already_recognized`, and a drafted rule
/// that may overlap one is dropped as `draft_overlap`: no drafted rule
/// re-maps an artifact a recorded contract names.
///
/// `taken` holds artifact types the draft may not use, beyond the starters
/// and `existing`'s types. Each adapter is judged by [`check_draft_rules`]
/// as drafted, against `existing`, the recorded contracts and the adapters
/// accepted before it in rank order. A dropped rule's shapes are reported
/// with the check's code, and an adapter left with no rule is removed.
pub fn draft_site(
    origin: &str,
    existing: &Document,
    recorded: &[Document],
    input: &ShapeInput,
    policy: &DraftPolicy,
    taken: &[String],
) -> SiteDraft {
    let (candidates, mut dropped) = candidates(origin, existing, recorded, input, policy, taken);
    let mut accepted: Vec<Document> = recorded.to_vec();
    let mut adapters = Vec::new();
    for mut candidate in candidates {
        let table = candidate.table(origin);
        let provenance = vec![Provenance::Drafted; candidate.rules.len()];
        let report = match check_draft_rules(existing, &table, &provenance, policy, &accepted) {
            Ok(report) => report,
            Err(outcome) => {
                let code = match outcome {
                    DraftOutcome::DraftError(code) => code,
                    DraftOutcome::Invalid(_) | DraftOutcome::Ok { .. } => {
                        DraftErrorCode::InvalidRule
                    }
                };
                candidate.drop_all(DropReason::Check(code), &mut dropped);
                continue;
            }
        };
        let mut kept = Vec::new();
        for (rule, verdict) in candidate.rules.drain(..).zip(&report.rules) {
            match verdict {
                RuleVerdict::Dropped(code) => {
                    drop_rule(&rule, DropReason::Check(*code), &mut dropped)
                }
                RuleVerdict::Kept | RuleVerdict::Warned(_) => kept.push(rule),
            }
        }
        if kept.is_empty() {
            continue;
        }
        candidate.rules = kept;
        accepted.extend(report.document);
        adapters.push(candidate.drafted(origin, report.scope, policy));
    }
    SiteDraft {
        adapters,
        dropped: tidy(dropped),
    }
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
        let (tagged, mut dropped) = filter(origin, existing, &[], input, &policy);
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
        let (tagged, _) = filter(TOOLS, &Document::default(), &[], &shapes, &policy);
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
    fn link_only_tail_hex_never_widens_visited_hex_rule() {
        // A planted link with the tail form keeps the visited rule at
        // `{hex32}` and goes into a link-only rule of its own.
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(&["/p/{hex32}"], &["/p/{tailhex32}"]),
        );
        assert_eq!(texts(&kinds), [["/p/{hex32}", "/p/{tailhex32}"]]);
        let rules = &kinds[0].rules;
        assert_eq!((rules[0].visited, rules[0].links), (1, 0));
        assert!(!rules[0].link_only());
        assert_eq!((rules[1].visited, rules[1].links), (0, 1));
        assert!(rules[1].link_only());

        // The same holds in the query, after a route-word alternation, and
        // per route word.
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/doc?id={hex32}",
                    "/w/a/{hex32}",
                    "/w/b/{hex32}",
                    "/a/{hex32}",
                    "/b/{hex32}",
                ],
                &["/doc?id={tailhex32}", "/w/a/{tailhex32}", "/a/{tailhex32}"],
            ),
        );
        assert_eq!(
            texts(&kinds),
            [
                vec!["/a/{hex32}", "/b/{hex32}", "/a/{tailhex32}"],
                vec!["/doc?id={hex32}", "/doc?id={tailhex32}"],
                vec!["/w/(a|b)/{hex32}", "/w/a/{tailhex32}"],
            ]
        );
        for kind in &kinds {
            for rule in &kind.rules {
                assert_eq!(rule.link_only(), rule.text().contains("tailhex32"));
            }
        }

        // A link with the narrower form still backs a visited tail rule.
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(&["/p/{tailhex32}"], &["/p/{hex32}"]),
        );
        assert_eq!(texts(&kinds), [["/p/{tailhex32}"]]);
        assert_eq!((kinds[0].rules[0].visited, kinds[0].rules[0].links), (1, 1));
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
        // Tenants group as one position. A blocked route word at the
        // tenant position is still a blocked route, never a tenant.
        assert_eq!(texts(&kinds), [["/{tenant}/{slug}/issues/{num}"]]);
        assert_eq!(kinds[0].rules[0].visited, 2);
        assert_eq!(
            drops(&dropped),
            [
                ("/acme/{num}", DropReason::NoRouteWord),
                ("/share/{slug}/issues/{num}", DropReason::BlockedRoute),
            ]
        );

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

    // --- Map, Name, Rank and draft_site ---

    fn counted(list: &[(&str, u32)]) -> Vec<(UrlShape, u32)> {
        list.iter()
            .map(|(s, n)| (UrlShape::parse(s).unwrap(), *n))
            .collect()
    }

    fn site(origin: &str, existing: &Document, input: &ShapeInput, taken: &[&str]) -> SiteDraft {
        let taken: Vec<String> = taken.iter().map(|t| (*t).to_owned()).collect();
        draft_site(origin, existing, &[], input, &test_catalog(), &taken)
    }

    fn json_of(v: &Value) -> Json {
        match v {
            Value::Null => Json::Null,
            Value::Bool(b) => json!(b),
            Value::Int(i) => json!(i),
            Value::Float(f) => json!(f),
            Value::String(s) => json!(s),
            Value::Array(a) => Json::Array(a.iter().map(json_of).collect()),
            Value::Table(t) => {
                Json::Object(t.iter().map(|(k, v)| (k.clone(), json_of(v))).collect())
            }
        }
    }

    fn str_of<'v>(v: &'v Value, key: &str) -> &'v str {
        v.get(key).and_then(Value::as_str).unwrap()
    }

    /// Each rule as `path?key=value&…`.
    fn rule_texts(adapter: &Value) -> Vec<String> {
        adapter
            .get("match")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|rule| {
                let mut text = str_of(rule, "path").to_owned();
                for (i, (k, v)) in rule
                    .get("query")
                    .and_then(Value::as_table)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    let _ = write!(
                        text,
                        "{}{k}={}",
                        if i == 0 { '?' } else { '&' },
                        v.as_str().unwrap()
                    );
                }
                text
            })
            .collect()
    }

    fn all_rule_texts(draft: &SiteDraft) -> Vec<String> {
        let mut texts: Vec<String> = draft
            .adapters
            .iter()
            .flat_map(|a| rule_texts(&a.adapter))
            .collect();
        texts.sort();
        texts
    }

    fn types(draft: &SiteDraft) -> Vec<&str> {
        draft
            .adapters
            .iter()
            .map(|a| str_of(&a.adapter, "type"))
            .collect()
    }

    fn ids(draft: &SiteDraft) -> Vec<&str> {
        draft
            .adapters
            .iter()
            .map(|a| str_of(&a.adapter, "id"))
            .collect()
    }

    #[test]
    fn placeholders_map_to_capture_types() {
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/n/{num}",
                    "/k/{key}",
                    "/u/{uuid}",
                    "/h/{hex32}",
                    "/f/{hex40}",
                    "/s/{hex64}",
                    "/t/{tailhex32}",
                    "/i/{id}",
                ],
                &[],
            ),
            &[],
        );
        assert!(draft.dropped.is_empty(), "{:?}", draft.dropped);
        // `/h/` and `/t/` are one kind, but their captures differ in type,
        // so each is an adapter of its own.
        assert_eq!(draft.adapters.len(), 8);
        assert_eq!(
            all_rule_texts(&draft),
            [
                "/f/{hex:hex(40)}",
                "/h/{hex:hex(32)}",
                "/i/{token:token(1,128)}",
                "/k/{key:jira_key}",
                "/n/{num:int}",
                "/s/{hex:hex(64)}",
                "/t/{hex:hex(32)@tail}",
                "/u/{uuid:uuid}",
            ]
        );
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(&["/q?x={uuid}"], &[]),
            &[],
        );
        assert_eq!(all_rule_texts(&draft), ["/q?x={uuid:uuid}"]);
    }

    #[test]
    fn link_only_tail_rule_never_shares_an_adapter_with_a_visited_hex_rule() {
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(&["/p/{hex32}"], &["/p/{tailhex32}"]),
            &[],
        );
        assert_eq!(all_rule_texts(&draft), ["/p/{hex:hex(32)}"]);
        assert_eq!(
            drops(&draft.dropped),
            [("/p/{tailhex32}", DropReason::Check(DraftErrorCode::Overlap))]
        );
        // Visited tail and bare forms share one tail rule.
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(&["/p/{hex32}", "/p/{tailhex32}"], &[]),
            &[],
        );
        assert_eq!(all_rule_texts(&draft), ["/p/{hex:hex(32)@tail}"]);
        assert_eq!(draft.adapters[0].rules[0].visited, 2);
    }

    #[test]
    fn tenant_position_becomes_lower_name_capture_even_when_literal() {
        let shapes = input(&["/acme/issues/{num}"], &[]);
        let draft = site(GITHUB, &Document::default(), &shapes, &[]);
        assert_eq!(draft.adapters.len(), 1);
        let a = &draft.adapters[0];
        assert_eq!(
            rule_texts(&a.adapter),
            ["/{owner:name|lower}/issues/{num:int}"]
        );
        assert_eq!(
            str_of(&a.adapter, "id"),
            "github.issues:{host}/{owner}/{num}"
        );
        assert_eq!(a.tenant, Some((0, "owner".to_owned())));
        assert_eq!(
            a.scope,
            ScopeState::Pending {
                capture: "owner".to_owned()
            }
        );

        // Agreeing scoped adapters on the origin give their scope.
        let existing = doc(json!([{
            "type": "github.pr",
            "origin": GITHUB,
            "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}"}],
            "scope": {"owner": ["acme-corp"]},
            "id": "github.pr:{host}/{owner}/{repo}/{number}"
        }]));
        let draft = site(GITHUB, &existing, &shapes, &[]);
        assert_eq!(
            draft.adapters[0].scope,
            ScopeState::Copied(vec![("owner".to_owned(), vec!["acme-corp".to_owned()])])
        );
    }

    #[test]
    fn namespace_slug_after_tenant_becomes_name_capture() {
        let draft = site(
            GITHUB,
            &Document::default(),
            &input(
                &["/acme/{slug}/pull/{num}", "/acme/{slug}/{slug}/tree/{num}"],
                &[],
            ),
            &[],
        );
        assert_eq!(
            all_rule_texts(&draft),
            [
                "/{owner:name|lower}/{space:name}/pull/{num:int}",
                "/{owner:name|lower}/{space:name}/{space_2:name}/tree/{num:int}",
            ]
        );
        let mut got = ids(&draft);
        got.sort_unstable();
        assert_eq!(
            got,
            [
                "github.pull:{host}/{owner}/{space}/{num}",
                "github.tree:{host}/{owner}/{space}/{space_2}/{num}",
            ]
        );

        // Off a shared origin there is no tenant, so no namespace either.
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(&["/team/{slug}/pull/{num}"], &[]),
            &[],
        );
        assert_eq!(all_rule_texts(&draft), ["/team/{_}/pull/{num:int}"]);
        assert_eq!(ids(&draft), ["tools.pull:{host}/{num}"]);
    }

    #[test]
    fn other_non_trailing_slugs_become_anonymous() {
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/wiki/{slug}/pages/{num}/{slug}/history",
                    "/p/{num}/{slug}/c/{key}",
                ],
                &[],
            ),
            &[],
        );
        assert_eq!(
            all_rule_texts(&draft),
            [
                "/p/{num:int}/{_}/c/{key:jira_key}",
                "/wiki/{_}/pages/{num:int}/{_}/history",
            ]
        );
        let mut got = ids(&draft);
        got.sort_unstable();
        assert_eq!(
            got,
            ["tools.p:{host}/{num}/{key}", "tools.pages:{host}/{num}"]
        );
    }

    #[test]
    fn query_identifier_becomes_condition_unless_listed() {
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/viewpage?pageid={num}&page={num}&sort={slug}",
                    "/browse/{key}?q={num}",
                ],
                &[],
            ),
            &[],
        );
        let rules: Vec<Json> = draft
            .adapters
            .iter()
            .flat_map(|a| {
                a.adapter
                    .get("match")
                    .and_then(Value::as_array)
                    .unwrap()
                    .iter()
                    .map(json_of)
            })
            .collect();
        assert!(
            rules.contains(&json!({"path": "/viewpage", "query": {"pageid": "{num:int}"}})),
            "{rules:?}"
        );
        assert!(
            rules.contains(&json!({"path": "/browse/{key:jira_key}"})),
            "{rules:?}"
        );
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn fixed_shape_identifier_wins_over_num() {
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/sites/{slug}/layouts/{num}/doc?sourcedoc={uuid}",
                    "/d/{num}/{hex32}",
                    "/v/{uuid}?rev={num}",
                ],
                &[],
            ),
            &[],
        );
        assert_eq!(
            all_rule_texts(&draft),
            [
                "/d/{_}/{hex:hex(32)}",
                "/sites/{_}/layouts/{_}/doc?sourcedoc={uuid:uuid}",
                "/v/{uuid:uuid}?rev={_}",
            ]
        );
        let mut got = ids(&draft);
        got.sort_unstable();
        assert_eq!(
            got,
            [
                "tools.d:{host}/{hex}",
                "tools.layouts:{host}/{uuid}",
                "tools.v:{host}/{uuid}",
            ]
        );
    }

    #[test]
    fn mode_word_ending_maps_modes() {
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/x/pull/{num}",
                    "/x/pull/{num}/files",
                    "/x/pull/{num}/commits",
                    "/d/edit/{num}",
                    "/d/view/{num}",
                ],
                &[],
            ),
            &[],
        );
        let by_type = |t: &str| {
            draft
                .adapters
                .iter()
                .find(|a| str_of(&a.adapter, "type") == t)
                .unwrap()
                .adapter
                .clone()
        };
        let pull = by_type("tools.pull");
        assert_eq!(
            rule_texts(&pull),
            [
                "/x/pull/{num:int}",
                "/x/pull/{num:int}/commits",
                "/x/pull/{num:int}/{mode:(files)}",
            ]
        );
        assert_eq!(
            json_of(pull.get("modes").unwrap()),
            json!({"files": "review"})
        );
        let d = by_type("tools.d");
        assert_eq!(rule_texts(&d), ["/d/{mode:(edit|view)}/{num:int}"]);
        assert_eq!(
            json_of(d.get("modes").unwrap()),
            json!({"edit": "edit", "view": "view"})
        );
    }

    #[test]
    fn types_are_site_dot_word_with_label_fallbacks() {
        let cases: &[(&str, &[&str], &str)] = &[
            (TOOLS, &["/wiki/{num}"], "tools.wiki"),
            (
                "https://my-tools.example.com",
                &["/merge_requests/{num}"],
                "mytools.mergerequest",
            ),
            (
                "https://averyveryverylonghost.example.com",
                &["/configurations/{num}"],
                "averyveryver.configuratio",
            ),
            ("https://123.example.com", &["/wiki/{num}"], "site.wiki"),
            ("https://127.0.0.1", &["/wiki/{num}"], "site.wiki"),
            (
                "https://webmail.example.com",
                &["/webhooks/{num}"],
                "site.page",
            ),
            // The alternation is not a single route word; the literal before it is.
            (
                TOOLS,
                &["/wiki/pages/{num}", "/wiki/blog/{num}"],
                "tools.wiki",
            ),
        ];
        for (origin, visited, want) in cases {
            let draft = site(origin, &Document::default(), &input(visited, &[]), &[]);
            assert_eq!(types(&draft), [*want], "{origin} {visited:?}");
            assert_eq!(
                ids(&draft)[0].split_once(':').unwrap().0,
                *want,
                "the scheme is the type"
            );
        }
    }

    #[test]
    fn taken_types_get_numeric_suffix_and_ids_never_collide() {
        let shapes = ShapeInput {
            visited: counted(&[("/wiki/{num}", 2), ("/wiki/{num}/x/{key}", 1)]),
            links: Vec::new(),
        };
        let draft = site(TOOLS, &Document::default(), &shapes, &[]);
        assert_eq!(types(&draft), ["tools.wiki", "tools.wiki2"]);
        let draft = site(TOOLS, &Document::default(), &shapes, &["tools.wiki"]);
        assert_eq!(types(&draft), ["tools.wiki2", "tools.wiki3"]);
        assert_eq!(
            ids(&draft),
            ["tools.wiki2:{host}/{num}", "tools.wiki3:{host}/{num}/{key}"]
        );

        // A starter's type and an existing adapter's type are taken too.
        let draft = site(
            "https://jira.example.com",
            &Document::default(),
            &input(&["/issue/{num}"], &[]),
            &[],
        );
        assert_eq!(types(&draft), ["jira.issue2"]);
        let existing = doc(json!([{
            "type": "tools.wiki",
            "origin": "https://other.example.com",
            "match": [{"path": "/w/{n:int}"}],
            "id": "tools.wiki:{host}/{n}"
        }]));
        let draft = site(TOOLS, &existing, &input(&["/wiki/{num}"], &[]), &[]);
        assert_eq!(types(&draft), ["tools.wiki2"]);
    }

    #[test]
    fn no_drafted_scheme_is_web() {
        for origin in [
            "https://web.example.com",
            "https://webapp.example.com",
            "https://www.example.com",
        ] {
            let draft = site(
                origin,
                &Document::default(),
                &input(&["/web/{num}", "/w/x/{key}"], &[]),
                &[],
            );
            assert!(!draft.adapters.is_empty());
            for a in &draft.adapters {
                let scheme = str_of(&a.adapter, "id").split_once(':').unwrap().0;
                assert!(!scheme.starts_with("web"), "{origin}: {scheme}");
                assert_ne!(str_of(&a.adapter, "type"), "web.page");
            }
        }
        let draft = site(
            "https://web.example.com",
            &Document::default(),
            &input(&["/web/{num}"], &[]),
            &[],
        );
        assert_eq!(types(&draft), ["site.page"]);
    }

    #[test]
    fn drafted_adapters_use_only_tier0_keys() {
        let drafts = [
            site(
                GITHUB,
                &Document::default(),
                &input(
                    &["/acme/{slug}/pull/{num}", "/acme/{slug}/pull/{num}/files"],
                    &[],
                ),
                &[],
            ),
            site(
                TOOLS,
                &Document::default(),
                &input(
                    &["/d/edit/{num}", "/d/view/{num}", "/v?id={uuid}"],
                    &["/w/{num}"],
                ),
                &[],
            ),
            site(
                DOCS,
                &Document::default(),
                &input(&["/document/u/{num}/d/{id}/edit"], &[]),
                &[],
            ),
        ];
        for draft in &drafts {
            assert!(!draft.adapters.is_empty());
            for a in &draft.adapters {
                for (key, _) in a.adapter.as_table().unwrap() {
                    assert!(
                        ["type", "origin", "match", "id", "modes"].contains(&key.as_str()),
                        "{key}"
                    );
                }
                for rule in a.adapter.get("match").and_then(Value::as_array).unwrap() {
                    for (key, _) in rule.as_table().unwrap() {
                        assert!(["path", "query"].contains(&key.as_str()), "{key}");
                    }
                }
            }
        }
        assert_eq!(drafts[2].adapters[0].scope, ScopeState::UnscopedOnly);
        assert_eq!(drafts[2].adapters[0].tenant, None);
        assert_eq!(
            all_rule_texts(&drafts[2]),
            ["/document/u/{_}/d/{token:token(1,128)}/{mode:(edit)}"]
        );
    }

    #[test]
    fn visited_rules_rank_before_link_only_rules() {
        let shapes = ShapeInput {
            visited: counted(&[
                ("/wiki/pages/{num}", 1),
                ("/wiki/pages/{num}/history", 2),
                ("/b/{key}", 1),
            ]),
            links: counted(&[
                ("/wiki/blog/{num}", 9),
                ("/wiki/pages/{num}/{slug}/{slug}", 3),
                ("/a/{uuid}", 100),
            ]),
        };
        let draft = site(TOOLS, &Document::default(), &shapes, &[]);
        // The planted `/wiki/blog/{num}` link does not rename the visited
        // kind: it is named as it would be without the link.
        assert_eq!(types(&draft), ["tools.pages", "tools.b", "tools.a"]);
        let wiki = &draft.adapters[0];
        assert_eq!(
            rule_texts(&wiki.adapter),
            [
                "/wiki/pages/{num:int}/history",
                "/wiki/pages/{num:int}",
                "/wiki/blog/{num:int}",
                "/wiki/pages/{num:int}/{*}",
            ]
        );
        let counts: Vec<(usize, u32, u32, bool)> = wiki
            .rules
            .iter()
            .map(|r| (r.rule, r.visited, r.links, r.link_only))
            .collect();
        assert_eq!(
            counts,
            [
                (0, 2, 0, false),
                (1, 1, 0, false),
                (2, 0, 9, true),
                (3, 0, 3, true)
            ]
        );
        assert!(draft.adapters[2].rules[0].link_only);
    }

    #[test]
    fn at_most_16_rules_and_8_adapters() {
        // Twenty endings of one kind; the four least visited go.
        let endings: Vec<String> = (0..20u8)
            .map(|i| format!("/x/{{num}}/e{}", char::from(b'a' + i)))
            .collect();
        let visited: Vec<(&str, u32)> = endings
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), 100 - i as u32))
            .collect();
        let draft = site(
            TOOLS,
            &Document::default(),
            &ShapeInput {
                visited: counted(&visited),
                links: Vec::new(),
            },
            &[],
        );
        assert_eq!(draft.adapters.len(), 1);
        assert_eq!(draft.adapters[0].rules.len(), 16);
        let limited: Vec<&str> = draft
            .dropped
            .iter()
            .filter(|d| d.reason == DropReason::Limit)
            .map(|d| d.shape.as_str())
            .collect();
        assert_eq!(
            limited,
            ["/x/{num}/eq", "/x/{num}/er", "/x/{num}/es", "/x/{num}/et"]
        );

        // Ten kinds of different lengths; the two least visited go.
        let paths: Vec<String> = (1..=10)
            .map(|n| format!("{}/{{num}}", "/k".repeat(n)))
            .collect();
        let visited: Vec<(&str, u32)> = paths
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), 20 - i as u32))
            .collect();
        let draft = site(
            TOOLS,
            &Document::default(),
            &ShapeInput {
                visited: counted(&visited),
                links: Vec::new(),
            },
            &[],
        );
        assert_eq!(draft.adapters.len(), 8);
        let limited: Vec<&str> = draft
            .dropped
            .iter()
            .filter(|d| d.reason == DropReason::Limit)
            .map(|d| d.shape.as_str())
            .collect();
        // Drops sort by shape text.
        assert_eq!(limited, [paths[9].as_str(), paths[8].as_str()]);
    }

    #[test]
    fn link_only_and_token_only_rules_are_flagged() {
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(&["/doc/{id}", "/wiki/{num}"], &["/wiki/{num}/history"]),
            &[],
        );
        let flags: Vec<(String, bool, bool)> = draft
            .adapters
            .iter()
            .flat_map(|a| {
                let texts = rule_texts(&a.adapter);
                a.rules
                    .iter()
                    .map(move |r| (texts[r.rule].clone(), r.link_only, r.token_only))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            flags,
            [
                // Equal visits: links never reorder visited kinds.
                ("/doc/{token:token(1,128)}".to_owned(), false, true),
                ("/wiki/{num:int}".to_owned(), false, false),
                ("/wiki/{num:int}/history".to_owned(), true, false),
            ]
        );
        // The example is the illustrative fill, never a visited URL.
        assert_eq!(
            draft.adapters[0].rules[0].example,
            format!("{TOOLS}/doc/example123")
        );
    }

    #[test]
    fn draft_is_deterministic_for_permuted_input() {
        let visited = [
            ("/wiki/pages/{num}", 3),
            ("/wiki/blog/{num}", 1),
            ("/x/{num}/files", 2),
            ("/x/{num}", 2),
            ("/doc?id={uuid}", 1),
            ("/", 5),
            ("/t/{id}", 1),
        ];
        let links = [
            ("/wiki/news/{num}", 4),
            ("/x/{num}/{slug}/{slug}", 1),
            ("/p/{hex32}", 1),
            ("/p/{tailhex32}", 1),
        ];
        let forward = site(
            TOOLS,
            &Document::default(),
            &ShapeInput {
                visited: counted(&visited),
                links: counted(&links),
            },
            &["tools.x"],
        );
        let mut v = visited;
        let mut l = links;
        v.reverse();
        l.rotate_left(1);
        let backward = site(
            TOOLS,
            &Document::default(),
            &ShapeInput {
                visited: counted(&v),
                links: counted(&l),
            },
            &["tools.x"],
        );
        assert_eq!(forward, backward);
        assert!(forward.adapters.len() >= 4);
    }

    #[test]
    fn an_adapter_left_with_no_rule_is_removed_with_reasons() {
        // The filler `123456` is too long for the existing token, so the
        // filter keeps the shape, but the overlap check still drops it.
        let existing = doc(json!([{
            "type": "tools.short",
            "origin": TOOLS,
            "match": [{"path": "/wiki/{t:token(1,5)}"}],
            "id": "tools.short:{host}/{t}"
        }]));
        let shapes = ShapeInput {
            visited: counted(&[
                ("/wiki/{num}", 1),
                ("/x/{num}", 2),
                ("/x/{id}", 1),
                ("/y/{key}", 1),
            ]),
            links: Vec::new(),
        };
        let draft = site(TOOLS, &existing, &shapes, &[]);
        // `/x/{id}` loses to the higher-ranked `/x/{num}` it may overlap.
        assert_eq!(
            all_rule_texts(&draft),
            ["/x/{num:int}", "/y/{key:jira_key}"]
        );
        assert_eq!(
            drops(&draft.dropped),
            [
                ("/wiki/{num}", DropReason::Check(DraftErrorCode::Overlap)),
                ("/x/{id}", DropReason::Check(DraftErrorCode::Overlap)),
            ]
        );
        assert_eq!(
            DropReason::Check(DraftErrorCode::Overlap).as_str(),
            "draft_overlap"
        );
        for a in &draft.adapters {
            assert!(!a.rules.is_empty());
        }

        // Before the checks every rule is still there: `/wiki/` and `/x/`
        // are one kind split by route word, and `/x/{id}` is its own adapter.
        let kinds = draft_kinds(TOOLS, &existing, &[], &shapes, &test_catalog(), &[]);
        assert_eq!(kinds.adapters.len(), 3);
        let mut texts = all_rule_texts(&kinds);
        texts.sort();
        assert_eq!(
            texts,
            [
                "/wiki/{num:int}",
                "/x/{num:int}",
                "/x/{token:token(1,128)}",
                "/y/{key:jira_key}",
            ]
        );
        assert!(kinds.dropped.is_empty());
    }

    /// A contract recorded for the origin (a removed adapter) recognizes a
    /// shape: it is already captured, never re-drafted under a new type.
    #[test]
    fn shapes_a_recorded_contract_recognizes_are_already_recognized() {
        let recorded = doc(json!([{
            "type": "tools.issues",
            "origin": TOOLS,
            "match": [{"path": "/issues/{n:int}"}],
            "id": "tools.issues:{host}/{n}"
        }]));
        let shapes = input(&["/issues/{num}", "/docs/{num}"], &[]);
        let taken = ["tools.issues".to_owned()];
        let draft = draft_site(
            TOOLS,
            &Document::default(),
            std::slice::from_ref(&recorded),
            &shapes,
            &test_catalog(),
            &taken,
        );
        assert_eq!(types(&draft), ["tools.docs"]);
        assert_eq!(
            drops(&draft.dropped),
            [("/issues/{num}", DropReason::AlreadyRecognized)]
        );

        // A drafted rule that may overlap a recorded contract, though the
        // filler is not one it recognizes, is dropped by the overlap check.
        let narrow = doc(json!([{
            "type": "tools.short",
            "origin": TOOLS,
            "match": [{"path": "/wiki/{t:token(1,5)}"}],
            "id": "tools.short:{host}/{t}"
        }]));
        let draft = draft_site(
            TOOLS,
            &Document::default(),
            std::slice::from_ref(&narrow),
            &input(&["/wiki/{num}"], &[]),
            &test_catalog(),
            &[],
        );
        assert!(draft.adapters.is_empty(), "{draft:?}");
        assert_eq!(
            drops(&draft.dropped),
            [("/wiki/{num}", DropReason::Check(DraftErrorCode::Overlap))]
        );
    }

    /// A planted link one route word away from a visited shape never changes
    /// the visited kind's type or ID scheme.
    #[test]
    fn a_link_one_word_away_never_renames_a_visited_kind() {
        let alone = site(
            TOOLS,
            &Document::default(),
            &input(&["/projects/{num}"], &[]),
            &[],
        );
        let planted = site(
            TOOLS,
            &Document::default(),
            &input(&["/projects/{num}"], &["/zzz/{num}"]),
            &[],
        );
        assert_eq!(types(&alone), ["tools.projects"]);
        assert_eq!(ids(&alone), ["tools.projects:{host}/{num}"]);
        assert_eq!(types(&planted)[0], "tools.projects");
        assert_eq!(ids(&planted)[0], "tools.projects:{host}/{num}");
        let visited = &planted.adapters[0];
        assert_eq!(
            rule_texts(&visited.adapter)[visited.rules.iter().position(|r| !r.link_only).unwrap()],
            "/projects/{num:int}"
        );
        // After a literal the link is a link-only alternative of the kind.
        let alone = site(
            TOOLS,
            &Document::default(),
            &input(&["/wiki/pages/{num}"], &[]),
            &[],
        );
        let planted = site(
            TOOLS,
            &Document::default(),
            &input(&["/wiki/pages/{num}"], &["/wiki/zzz/{num}"]),
            &[],
        );
        assert_eq!(types(&alone), ["tools.pages"]);
        assert_eq!(types(&planted), ["tools.pages"]);
        assert_eq!(ids(&planted), ids(&alone));
        // Visited shapes that differ there still name nothing by that word.
        let both = site(
            TOOLS,
            &Document::default(),
            &input(
                &["/wiki/pages/{num}", "/wiki/blog/{num}"],
                &["/wiki/zzz/{num}"],
            ),
            &[],
        );
        assert_eq!(types(&both), ["tools.wiki"]);
    }

    /// `?id=1&id=abc` shapes to `id={id}&id={num}`: one key cannot hold two
    /// conditions, so the name is dropped, and the shape with it when it has
    /// no other identifier.
    #[test]
    fn a_query_name_repeated_with_different_classes_is_dropped() {
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(
                &["/view?id={id}&id={num}", "/doc/{num}?rev={id}&rev={num}"],
                &[],
            ),
            &[],
        );
        assert_eq!(all_rule_texts(&draft), ["/doc/{num:int}"]);
        assert_eq!(
            drops(&draft.dropped),
            [("/view?id={id}&id={num}", DropReason::NoIdentifier)]
        );
        for a in &draft.adapters {
            for rule in a.adapter.get("match").and_then(Value::as_array).unwrap() {
                let keys: Vec<&str> = rule
                    .get("query")
                    .and_then(Value::as_table)
                    .unwrap_or_default()
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .collect();
                let distinct: BTreeSet<&str> = keys.iter().copied().collect();
                assert_eq!(keys.len(), distinct.len(), "{keys:?}");
            }
        }
    }

    /// Tokens, signatures and one-time codes in the query never become ID
    /// captures, visited or planted, and a fixed-shape secret never demotes
    /// the path identifier.
    #[test]
    fn secret_query_keys_never_become_identifiers() {
        for (shape, rule, id) in [
            (
                "/files/{num}?token={id}",
                "/files/{num:int}",
                "tools.files:{host}/{num}",
            ),
            (
                "/doc/{num}?access_token={hex64}",
                "/doc/{num:int}",
                "tools.doc:{host}/{num}",
            ),
            (
                "/doc/{num}?session_id={uuid}",
                "/doc/{num:int}",
                "tools.doc:{host}/{num}",
            ),
        ] {
            for (visited, links) in [(&[shape][..], &[][..]), (&[][..], &[shape][..])] {
                let draft = site(TOOLS, &Document::default(), &input(visited, links), &[]);
                assert_eq!(all_rule_texts(&draft), [rule], "{shape}");
                assert_eq!(ids(&draft), [id], "{shape}");
            }
        }
        let draft = site(
            TOOLS,
            &Document::default(),
            &input(
                &[
                    "/view?sig={hex32}",
                    "/s?amz-signature={hex64}",
                    "/cb?code={id}&state={id}",
                    "/open?url_sig={id}",
                    "/share?token={uuid}",
                ],
                &[],
            ),
            &[],
        );
        assert!(draft.adapters.is_empty(), "{draft:?}");
        assert_eq!(
            drops(&draft.dropped),
            [
                ("/cb?code={id}&state={id}", DropReason::SecretQuery),
                ("/open?url_sig={id}", DropReason::SecretQuery),
                ("/s?amz-signature={hex64}", DropReason::SecretQuery),
                ("/share?token={uuid}", DropReason::BlockedRoute),
                ("/view?sig={hex32}", DropReason::SecretQuery),
            ]
        );
        assert_eq!(DropReason::SecretQuery.as_str(), "secret_query");
    }

    /// Hyphenated, underscored and plural forms of blocked route words are
    /// blocked too, visited or planted.
    #[test]
    fn blocked_route_word_variants_are_filtered() {
        let variants = [
            "/invitations/{hex32}",
            "/accept-invite/{hex32}",
            "/reset-password/{uuid}",
            "/sign-in/{id}",
            "/magic-link/{id}",
            "/tokens/{num}",
            "/auth_callback/{id}",
            "/sso/{id}",
            "/saml/{id}",
            "/join/{num}",
            "/sign_up/{id}",
        ];
        for (visited, links) in [(&variants[..], &[][..]), (&[][..], &variants[..])] {
            let (kinds, dropped) = run(TOOLS, &Document::default(), &input(visited, links));
            assert!(kinds.is_empty(), "{:?}", texts(&kinds));
            assert!(
                dropped.iter().all(|d| d.reason == DropReason::BlockedRoute),
                "{dropped:?}"
            );
            assert_eq!(dropped.len(), variants.len(), "{dropped:?}");
        }
        // Route words that only contain a blocked word stay route words.
        let (kinds, _) = run(
            TOOLS,
            &Document::default(),
            &input(&["/shared-drives/{num}", "/homework/{num}"], &[]),
        );
        assert_eq!(texts(&kinds), [["/homework/{num}", "/shared-drives/{num}"]]);
    }

    /// On a shared origin a blocked word at the tenant position is a
    /// sign-in, invite or sharing route, never a workspace.
    #[test]
    fn a_blocked_word_at_the_tenant_position_is_blocked() {
        const LINEAR: &str = "https://linear.app";
        let shapes = [
            "/auth/callback/{id}",
            "/invite/accept/{hex32}",
            "/share/doc/{uuid}",
        ];
        for (visited, links) in [(&shapes[..], &[][..]), (&[][..], &shapes[..])] {
            let draft = site(LINEAR, &Document::default(), &input(visited, links), &[]);
            assert!(draft.adapters.is_empty(), "{draft:?}");
            assert!(
                draft
                    .dropped
                    .iter()
                    .all(|d| d.reason == DropReason::BlockedRoute),
                "{:?}",
                draft.dropped
            );
        }
        let draft = site(
            GITHUB,
            &Document::default(),
            &input(&["/login/oauth/{id}", "/acme/app/issues/{num}"], &[]),
            &[],
        );
        assert_eq!(
            drops(&draft.dropped),
            [("/login/oauth/{id}", DropReason::BlockedRoute)]
        );
        assert_eq!(draft.adapters.len(), 1);
    }
}

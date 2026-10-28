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
//! [`check_agent_draft`] judges an agent drafter's tables (`brwr-f45`) with
//! the same check loop, so an agent rule is held to exactly the checks a
//! structural rule is, and [`describe_rules`] gives any table's rules the
//! review fields the structural review shows.
//!
//! Every collection is sorted by shape string before it is grouped, so the
//! order of the input never changes the output. Link shapes come from the
//! page and are untrusted: they never change how visited shapes group, nor
//! the type, ID scheme or rank of a kind with visited support, and what only
//! links support always forms a rule of its own.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use crate::ctype::{BaseType, CType};
use crate::document::{Adapter, Document, MAX_RULES, QueryCond, Rule};
use crate::draft::{
    DraftOutcome, DraftRulesReport, Provenance, RuleVerdict, ScopeState, check_draft_rules,
    has_forbidden_key, scope_state,
};
use crate::error::DraftErrorCode;
use crate::eval::{Prepared, match_rule};
use crate::path::Segment;
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

/// Step 3, Keep. A kind is a candidate when an anchor comes before its first
/// path identifier, or, when its identifiers are only in the query, when its
/// path has any anchor. An anchor is a route word or, on a shared origin
/// with a tenant position, the tenant: the drafter scopes that capture, and a
/// scope-restricted capture anchors a template as a literal does
/// (adapter-templates §4.1, §6). The tenant is never a route word, so it
/// never names a kind, and [`check_draft_rules`] then decides whether the
/// scope it gets really restricts it.
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
            let anchored = kind.base[..before].iter().enumerate().any(|(at, part)| {
                Some(at) == tenant || matches!(part, Part::Literal(_) | Part::Alternation(_))
            });
            (!anchored).then_some(DropReason::NoRouteWord)
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
    /// No visited or link shape matches it: starts unchecked, "suggested by
    /// AI: no page you visited or linked here matches it". Always `false`
    /// for structural rules, whose kinds come from shapes.
    pub unseen: bool,
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
    /// Set when [`check_agent_draft`] refused the whole agent draft before
    /// any rule was judged, so none of it is shown: a forbidden key, a table
    /// for another origin, or a table that does not parse. Always `None`
    /// from [`draft_site`].
    pub rejected: Option<DraftErrorCode>,
    /// The agent rules a check dropped, in table and rule order. Always
    /// empty from [`draft_site`], which reports dropped shapes instead.
    pub dropped_rules: Vec<DroppedRule>,
}

/// An agent rule a check dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DroppedRule {
    /// The index of the agent's table.
    pub table: usize,
    /// The rule's index in that table's `match`, or `None` when a
    /// table-level check dropped every rule of the table.
    pub rule: Option<usize>,
    pub reason: DraftErrorCode,
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
                    unseen: false,
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
    namespaces(kind, policy)
        .into_iter()
        .flat_map(|(group, bind)| map_group(&group, bind, shared, policy))
        .collect()
}

/// Whether the kind has a `uuid` or `hexN` identifier, so only those enter
/// the ID.
fn has_fixed_shape(kind: &Kind) -> bool {
    kind.base
        .iter()
        .filter_map(|p| match p {
            Part::Class(class) => Some(*class),
            _ => None,
        })
        .chain(kind.query.iter().map(|(_, class)| *class))
        .any(is_fixed_shape)
}

/// A varying position the ID binds, and the words its capture admits.
#[derive(Clone, Debug)]
struct Bind {
    at: usize,
    words: Vec<String>,
}

/// The route words a rule has at the varying position.
fn words_at(rule: &KindRule, at: usize) -> BTreeSet<String> {
    match rule.path.get(at) {
        Some(Part::Literal(word)) => BTreeSet::from([word.clone()]),
        Some(Part::Alternation(words)) => words.iter().cloned().collect(),
        _ => BTreeSet::new(),
    }
}

/// Step 4a, Namespaces. Route words that differ at the varying position
/// name namespaces of the kind's identifiers: equal numbers under
/// `/foo/discussions/7` and `/bar/discussions/7` are two pages. So the
/// kind's rules group by the words they hold there:
///
/// - a rule holding one word is in that word's group, whose ID is the one
///   the kind would have with that word alone;
/// - rules holding the same several words, an alternation after a route
///   word (step 2), form one group whose ID binds the position as
///   `{space:(…)}`, and so do visited words seen with the same visited
///   rules. Same-named captures share one type, so a group never mixes
///   word sets, and no visited rule is widened to a word it was not seen
///   with;
/// - a bound capture is never a draft's anchor (adapter-templates §10.4),
///   so words bind only when another route word anchors the rules.
///
/// A rule's words are visited words or link-only words, never a mix (step
/// 2), so a planted link never changes the groups or IDs of visited pages.
/// Mode words name views of one page, so a kind whose words are all mode
/// words stays one group. Each group is returned with the position its ID
/// binds, if any.
fn namespaces(kind: &Kind, policy: &DraftPolicy) -> Vec<(Kind, Option<Bind>)> {
    let Some(at) = kind.varying else {
        return vec![(kind.clone(), None)];
    };
    let is_mode = |w: &String| policy.mode_words.iter().any(|(m, _)| m == w);
    let all: BTreeSet<String> = kind.rules.iter().flat_map(|r| words_at(r, at)).collect();
    if all.len() < 2 || all.iter().all(is_mode) {
        return vec![(kind.clone(), None)];
    }
    let fixed = has_fixed_shape(kind);
    let first_id = kind
        .base
        .iter()
        .position(
            |p| matches!(p, Part::Class(c) if p.is_identifier() && (!fixed || is_fixed_shape(*c))),
        )
        .unwrap_or(kind.base.len());
    let anchored_elsewhere = at > first_id
        || kind.base[..first_id]
            .iter()
            .enumerate()
            .any(|(i, p)| i != at && matches!(p, Part::Literal(_)));
    let mut groups: BTreeMap<Vec<String>, Vec<KindRule>> = BTreeMap::new();
    for rule in &kind.rules {
        groups
            .entry(words_at(rule, at).into_iter().collect())
            .or_default()
            .push(rule.clone());
    }
    // Words seen with the same visited rules, but for the word, bind as one
    // group, so one kind still covers the site's namespaces. Their link-only
    // rules come along and start unchecked.
    if anchored_elsewhere {
        let mask = |rule: &KindRule| {
            let mut path = rule.path.clone();
            path[at] = Part::Rest;
            render(&path, &rule.query)
        };
        let mut by_profile: BTreeMap<BTreeSet<String>, Vec<String>> = BTreeMap::new();
        for (words, rules) in &groups {
            if let [word] = words.as_slice()
                && !is_mode(word)
            {
                let profile: BTreeSet<String> =
                    rules.iter().filter(|r| !r.link_only()).map(mask).collect();
                if !profile.is_empty() {
                    by_profile.entry(profile).or_default().push(word.clone());
                }
            }
        }
        // One alternation holds at most 16 words (adapter-templates §4.1).
        for words in by_profile
            .into_values()
            .filter(|w| (2..=crate::ctype::MAX_ALTERNATIVES).contains(&w.len()))
        {
            let rules: Vec<KindRule> = words
                .iter()
                .flat_map(|w| groups.remove(std::slice::from_ref(w)).unwrap_or_default())
                .collect();
            groups.entry(words).or_default().extend(rules);
        }
    }
    groups
        .into_iter()
        .map(|(words, rules)| {
            let (word, bind) = match words.as_slice() {
                [word] => (Some(word.clone()), None),
                _ if anchored_elsewhere && !words.iter().all(is_mode) => {
                    (None, Some(Bind { at, words }))
                }
                _ => (None, None),
            };
            let group = Kind {
                rules,
                visited_word: word,
                ..kind.clone()
            };
            (group, bind)
        })
        .collect()
}

/// [`map`] for one group of [`namespaces`]; `bind` is the varying position
/// when the ID binds it.
fn map_group(
    kind: &Kind,
    bind: Option<Bind>,
    shared: Option<&SharedOrigin>,
    policy: &DraftPolicy,
) -> Vec<Candidate> {
    let tenant_capture = shared.and_then(|s| s.tenant.as_ref()).map(|(_, c)| c);
    let tenant_at = kind.base.iter().position(|p| *p == Part::Tenant);
    let first_id = kind.base.iter().position(Part::is_identifier);
    let fixed = has_fixed_shape(kind);
    let in_id = |class: ShapeClass| !fixed || is_fixed_shape(class);
    let bound = |at: usize| bind.as_ref().is_some_and(|b| b.at == at);

    let mut used: Vec<String> = tenant_capture.into_iter().cloned().collect();
    let mut id_names = Vec::new();
    let mut id_classes = Vec::new();
    let mut names: Vec<Option<String>> = Vec::with_capacity(kind.base.len());
    for (at, part) in kind.base.iter().enumerate() {
        let name = match part {
            _ if bound(at) => Some(unique_name(capture_base(ShapeClass::Slug), &mut used)),
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
                _ if bound(at) => {
                    let words = bind.as_ref().map(|b| b.words.join("|")).unwrap_or_default();
                    format!("{{{}:({words})}}", named.unwrap_or("_"))
                }
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

/// Step 5, Name. Each type is `<site>.<word>`, `<site>` being the host's
/// first label after a leading `www`, with a number appended when
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
    // `www.notion.so` is named for `notion`, not `www`.
    let mut labels = host.split('.');
    let first = labels.next().unwrap_or_default();
    let first = match labels.next() {
        Some(next) if first == "www" && host.matches('.').count() >= 2 => next,
        _ => first,
    };
    let site = label(first, "site");
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
        ..SiteDraft::default()
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
    let tables: Vec<Value> = candidates.iter().map(|c| c.table(origin)).collect();
    let reports = check_in_rank_order(existing, recorded, policy, &tables);
    let mut adapters = Vec::new();
    for (mut candidate, report) in candidates.into_iter().zip(reports) {
        let report = match report {
            Ok(report) => report,
            Err(code) => {
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
        adapters.push(candidate.drafted(origin, report.scope, policy));
    }
    SiteDraft {
        adapters,
        dropped: tidy(dropped),
        ..SiteDraft::default()
    }
}

/// The check loop [`draft_site`] and [`check_agent_draft`] share, so
/// structural and agent rules cannot drift apart: each table is judged by
/// [`check_draft_rules`] with every rule drafted, against `existing`, the
/// recorded contracts and the surviving rules of the tables before it. A
/// table refused whole gets its code; a table that fails validation as an
/// adapter is `draft_invalid_rule`.
fn check_in_rank_order(
    existing: &Document,
    recorded: &[Document],
    policy: &DraftPolicy,
    tables: &[Value],
) -> Vec<Result<DraftRulesReport, DraftErrorCode>> {
    let mut accepted: Vec<Document> = recorded.to_vec();
    tables
        .iter()
        .map(|table| {
            let rules = table
                .get("match")
                .and_then(Value::as_array)
                .map_or(0, <[Value]>::len);
            let provenance = vec![Provenance::Drafted; rules];
            let report = check_draft_rules(existing, table, &provenance, policy, &accepted)
                .map_err(|outcome| match outcome {
                    DraftOutcome::DraftError(code) => code,
                    DraftOutcome::Invalid(_) | DraftOutcome::Ok { .. } => {
                        DraftErrorCode::InvalidRule
                    }
                })?;
            accepted.extend(report.document.clone());
            Ok(report)
        })
        .collect()
}

/// Whether an agent table has the form a draft needs before any check: a
/// table for `origin` whose `type` and `id` are strings, whose ID scheme is
/// its type, and whose `match` holds 1 to 16 rules.
fn agent_table_form(table: &Value, origin: &str) -> bool {
    let text = |key: &str| table.get(key).and_then(Value::as_str);
    let rules = table.get("match").and_then(Value::as_array);
    table.as_table().is_some()
        && only_tier0_keys(table)
        && text("origin") == Some(origin)
        && text("type").is_some_and(|t| {
            text("id")
                .and_then(|id| id.split_once(':'))
                .is_some_and(|(scheme, _)| scheme == t)
        })
        && rules.is_some_and(|r| (1..=MAX_RULES).contains(&r.len()))
}

/// Agent table keys: the Tier 0 keys of a checked table (adapter-templates.md §10.4).
const TIER0_KEYS: &[&str] = &["type", "origin", "match", "id", "modes", "refs"];

/// The longest artifact type a document accepts.
const MAX_TYPE_LEN: usize = 32;

/// True unless `table` is a table with a key outside [`TIER0_KEYS`]; a
/// non-table is left to the form check.
fn only_tier0_keys(table: &Value) -> bool {
    table
        .as_table()
        .is_none_or(|t| t.iter().all(|(k, _)| TIER0_KEYS.contains(&k.as_str())))
}

/// The table with `type` set to `artifact_type`, and the ID scheme and any
/// ref to its own old type with it.
fn renamed(table: &Value, artifact_type: &str) -> Value {
    let Value::Table(entries) = table else {
        return table.clone();
    };
    let old = table
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Value::Table(
        entries
            .iter()
            .map(|(key, value)| match (key.as_str(), value) {
                ("type", _) => (key.clone(), Value::String(artifact_type.to_owned())),
                ("id", Value::String(id)) => {
                    let rest = id.split_once(':').map_or("", |(_, rest)| rest);
                    (
                        key.clone(),
                        Value::String(format!("{artifact_type}:{rest}")),
                    )
                }
                ("refs", Value::Array(refs)) => (
                    key.clone(),
                    Value::Array(
                        refs.iter()
                            .map(|r| retarget_ref(r, old, artifact_type))
                            .collect(),
                    ),
                ),
                _ => (key.clone(), value.clone()),
            })
            .collect(),
    )
}

/// `r` with its `type` moved from `old` to `new` when it names `old`.
fn retarget_ref(r: &Value, old: &str, new: &str) -> Value {
    match r {
        Value::Table(entries) if r.get("type").and_then(Value::as_str) == Some(old) => {
            Value::Table(
                entries
                    .iter()
                    .map(|(k, v)| {
                        if k == "type" {
                            (k.clone(), Value::String(new.to_owned()))
                        } else {
                            (k.clone(), v.clone())
                        }
                    })
                    .collect(),
            )
        }
        _ => r.clone(),
    }
}

/// The table with only the rules at `keep` in its `match`.
fn with_match(table: &Value, keep: &[usize]) -> Value {
    let Value::Table(entries) = table else {
        return table.clone();
    };
    let rules = table
        .get("match")
        .and_then(Value::as_array)
        .unwrap_or_default();
    let kept: Vec<Value> = keep.iter().filter_map(|&i| rules.get(i).cloned()).collect();
    Value::Table(
        entries
            .iter()
            .map(|(key, value)| match key.as_str() {
                "match" => (key.clone(), Value::Array(kept.clone())),
                _ => (key.clone(), value.clone()),
            })
            .collect(),
    )
}

/// The shared-origin tenant every rule of `document` binds at the catalog's
/// tenant position, for the review to prefill scope from the current tab.
fn bound_tenant(
    document: Option<&Document>,
    origin: &str,
    policy: &DraftPolicy,
) -> Option<(usize, String)> {
    let (at, capture) = policy.shared_origin(origin)?.tenant.as_ref()?;
    let adapter = document?.adapters.first()?;
    adapter
        .rules
        .iter()
        .all(|rule| {
            matches!(
                rule.path.segments.get(*at),
                Some(Segment::Capture { name, optional: false, .. }) if name == capture
            )
        })
        .then(|| (*at, capture.clone()))
}

/// Judges an agent's adapter tables for `origin` (DESIGN §5.1, step 7.7).
///
/// The tables are untrusted: the service only checked their form. They are
/// judged as drafted rules against the same `existing` document, recorded
/// contracts and policy as [`draft_site`], so an agent rule never gets more
/// trust than a structural one:
///
/// 1. **Whole-draft refusals**, before any rule is judged: any table with a
///    key a drafter may not set is `draft_forbidden_key`; no tables, more
///    than 8, a table for another origin, one that is not an adapter table,
///    whose ID scheme is not its type, or with no rule or more than 16 is
///    `draft_invalid_rule`, and so is a table that fails validation as an
///    adapter. A refusal returns no adapters, with `rejected` set: the
///    agent ignored the rules, so none of its draft is shown.
/// 2. **Types** are made unique against `taken`, the starters, `existing`
///    and earlier tables with the structural Name step's numeric suffix,
///    and the ID scheme follows the type.
/// 3. **Per-table checks** through the loop [`draft_site`] uses, in the
///    agent's table order. A dropped rule is reported in `dropped_rules`
///    with its code, a table refused whole with `rule: None`, and a table
///    left with no rule is removed.
/// 4. **Structural refusals**: a kept rule that captures an input shape
///    the structural Keep step refuses for a blocked route word in any
///    path segment is dropped as `draft_blocked_route`, whatever capture,
///    `{_}`, alternation or `{*}` let it reach that page.
/// 5. **Review fields** come from [`describe_rules`] on the kept rules,
///    and a rule that also captures a link shape none of its visited
///    shapes is starts `link_only`, so it is never checked by default.
pub fn check_agent_draft(
    origin: &str,
    existing: &Document,
    recorded: &[Document],
    input: &ShapeInput,
    policy: &DraftPolicy,
    taken: &[String],
    tables: &[Value],
) -> SiteDraft {
    let reject = |code| SiteDraft {
        rejected: Some(code),
        ..SiteDraft::default()
    };
    // `within` is a container link the review never shows; any other key
    // outside the Tier 0 set fails the form check below.
    if tables
        .iter()
        .any(|t| has_forbidden_key(t) || t.get("within").is_some())
    {
        return reject(DraftErrorCode::ForbiddenKey);
    }
    if tables.is_empty()
        || tables.len() > MAX_DRAFT_ADAPTERS
        || !tables.iter().all(|t| agent_table_form(t, origin))
    {
        return reject(DraftErrorCode::InvalidRule);
    }
    let mut used: BTreeSet<String> = taken
        .iter()
        .chain(&policy.starter_types)
        .cloned()
        .chain(existing.adapters.iter().map(|a| a.artifact_type.clone()))
        .collect();
    let tables: Vec<Value> = tables
        .iter()
        .map(|table| {
            let base = table
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut artifact_type = base.to_owned();
            let mut n = 2u32;
            while used.contains(&artifact_type) {
                // Keep the suffixed type within the type length limit.
                let suffix = n.to_string();
                let keep = base.len().min(MAX_TYPE_LEN.saturating_sub(suffix.len()));
                artifact_type = format!("{}{suffix}", &base[..keep]);
                n += 1;
            }
            used.insert(artifact_type.clone());
            renamed(table, &artifact_type)
        })
        .collect();
    let reports = check_in_rank_order(existing, recorded, policy, &tables);
    let refused = refused_shapes(origin, existing, recorded, input, policy);
    let mut site = SiteDraft::default();
    for (index, (table, report)) in tables.iter().zip(reports).enumerate() {
        let report = match report {
            Ok(report) => report,
            Err(DraftErrorCode::InvalidRule) => return reject(DraftErrorCode::InvalidRule),
            Err(code) => {
                site.dropped_rules.push(DroppedRule {
                    table: index,
                    rule: None,
                    reason: code,
                });
                continue;
            }
        };
        let mut keep = Vec::new();
        let rules = table
            .get("match")
            .and_then(Value::as_array)
            .unwrap_or_default();
        for (rule, verdict) in report.rules.iter().enumerate() {
            // A kept rule that captures a page the structural Keep step
            // refuses for a blocked route word is dropped as one.
            let refused = || {
                let document = rules
                    .get(rule)
                    .and_then(|r| matching_document(table, r, policy))?;
                refused
                    .iter()
                    .any(|url| matches_url(&document, url))
                    .then_some(DraftErrorCode::BlockedRoute)
            };
            let code = match verdict {
                RuleVerdict::Dropped(code) => Some(*code),
                RuleVerdict::Kept | RuleVerdict::Warned(_) => refused(),
            };
            match code {
                Some(reason) => site.dropped_rules.push(DroppedRule {
                    table: index,
                    rule: Some(rule),
                    reason,
                }),
                None => keep.push(rule),
            }
        }
        if keep.is_empty() {
            continue;
        }
        let adapter = with_match(table, &keep);
        let mut rules = describe_rules(&adapter, input, policy);
        // A rule that also captures a link shape none of its visited shapes
        // is starts unchecked as link-only: the agent may not widen capture
        // to pages only the page's links suggested (Decision 4).
        for (rule, uncovered) in rules
            .iter_mut()
            .zip(uncovered_links(&adapter, input, policy))
        {
            rule.link_only |= uncovered;
        }
        site.adapters.push(DraftedAdapter {
            tenant: bound_tenant(report.document.as_ref(), origin, policy),
            adapter,
            scope: report.scope,
            rules,
        });
    }
    site
}

/// The rule's table as a one-rule document the engine can match, without
/// its own scope: plain, else `unscoped` on a shared origin, else with a
/// stand-in tenant scope where the rule needs one to validate. The matching
/// itself ignores scope, as the draft checks do (SQ-V3).
fn matching_document(table: &Value, rule: &Value, policy: &DraftPolicy) -> Option<Document> {
    let Value::Table(entries) = table else {
        return None;
    };
    let base: Vec<(String, Value)> = entries
        .iter()
        .filter(|(key, _)| key != "scope" && key != "unscoped")
        .map(|(key, value)| match key.as_str() {
            "match" => (key.clone(), Value::Array(vec![rule.clone()])),
            _ => (key.clone(), value.clone()),
        })
        .collect();
    let tenant = table
        .get("origin")
        .and_then(Value::as_str)
        .and_then(|origin| policy.shared_origin(origin))
        .and_then(|shared| shared.tenant.as_ref())
        .map(|(_, capture)| capture.clone());
    let mut forms = vec![base.clone()];
    let mut unscoped = base.clone();
    unscoped.push((
        "unscoped".to_owned(),
        Value::String("acknowledged".to_owned()),
    ));
    forms.push(unscoped);
    if let Some(capture) = tenant {
        let mut scoped = base;
        let values = Value::Array(vec![Value::String("example".to_owned())]);
        scoped.push(("scope".to_owned(), Value::Table(vec![(capture, values)])));
        forms.push(scoped);
    }
    forms.into_iter().find_map(|form| {
        Document::from_value(&Value::Table(vec![
            ("format".to_owned(), Value::Int(1)),
            (
                "artifacts".to_owned(),
                Value::Array(vec![Value::Table(form)]),
            ),
        ]))
        .ok()
    })
}

/// An illustrative value of a capture type from the catalog's fixed
/// examples, when one fits.
fn fill_value(ctype: &CType, policy: &DraftPolicy) -> Option<String> {
    let fill = &policy.fill;
    let candidates: Vec<&str> = match ctype {
        CType::Alternation(words) => return words.first().cloned(),
        CType::Typed { base, tail, .. } => match base {
            BaseType::Int => vec![&fill.num, "1"],
            BaseType::Hex if *tail => vec![&fill.tailhex32],
            BaseType::Hex => vec![&fill.hex32, &fill.hex40, &fill.hex64, "0"],
            BaseType::Token => vec![&fill.id, "x"],
            BaseType::Name => vec![&fill.slug, "x"],
            BaseType::JiraKey => vec![&fill.key],
            BaseType::Uuid => vec![&fill.uuid],
        },
    };
    candidates
        .into_iter()
        .find_map(|value| ctype.canonical_whole(value))
}

/// The rule's own illustrative URL: literals as written, the first word of
/// an alternation, each capture filled from the catalog's examples, and no
/// optional segment. Empty when no fill matches the rule.
fn fill_rule(adapter: &Adapter, rule: &Rule, policy: &DraftPolicy) -> String {
    let mut url = adapter.origin.clone();
    let mut path = String::new();
    for segment in &rule.path.segments {
        let part = match segment {
            Segment::Literal(word) => Some(word.clone()),
            Segment::Alternation(words) => words.first().cloned(),
            Segment::Capture { optional: true, .. } | Segment::Rest => continue,
            Segment::Capture { ctype, .. } => fill_value(ctype, policy),
            Segment::Anonymous => Some(policy.fill.slug.clone()),
        };
        let Some(part) = part else {
            return String::new();
        };
        path.push('/');
        path.push_str(&part);
    }
    if path.is_empty() {
        path.push('/');
    }
    url.push_str(&path);
    for (index, (key, cond)) in rule.query.iter().enumerate() {
        let value = match cond {
            QueryCond::Presence => Some("1".to_owned()),
            QueryCond::Literal(value) => Some(value.clone()),
            QueryCond::Capture { ctype, .. } => fill_value(ctype, policy),
        };
        let Some(value) = value else {
            return String::new();
        };
        let _ = write!(url, "{}{key}={value}", if index == 0 { '?' } else { '&' });
    }
    let matches = Prepared::new(&url).is_ok_and(|p| {
        p.origin == adapter.origin
            && p.segments
                .as_deref()
                .is_some_and(|segments| match_rule(adapter, rule, segments, &p.query).is_some())
    });
    if matches { url } else { String::new() }
}

/// Every identifier capture the adapter's ID uses is a `token`.
fn id_token_only(adapter: &Adapter) -> bool {
    let mut identifiers = adapter
        .id
        .names()
        .filter_map(|name| match adapter.id_ctype(name) {
            Some(CType::Typed { base, .. }) if *base != BaseType::Name => Some(*base),
            _ => None,
        })
        .peekable();
    identifiers.peek().is_some() && identifiers.all(|base| base == BaseType::Token)
}

/// Whether the first adapter of `document` matches the prepared URL,
/// ignoring scope.
fn matches_prepared(document: &Document, prepared: &Prepared) -> bool {
    document.adapters.first().is_some_and(|adapter| {
        prepared.origin == adapter.origin
            && prepared.segments.as_deref().is_some_and(|segments| {
                adapter
                    .rules
                    .iter()
                    .any(|rule| match_rule(adapter, rule, segments, &prepared.query).is_some())
            })
    })
}

/// Whether the first adapter of `document` matches `url`, ignoring scope.
fn matches_url(document: &Document, url: &str) -> bool {
    Prepared::new(url).is_ok_and(|prepared| matches_prepared(document, &prepared))
}

/// The input shapes the structural Keep step refuses for a blocked route
/// word anywhere in the path (the [`filter`] verdict), each filled with the
/// catalog's examples: no agent rule may capture one. A shape refused only
/// for its search, paging or secret query is not here: a path rule ignores
/// the query, so a borderline rule may still match it, and the preview
/// lists it as the structural review does (DESIGN §12.2 step 7.7).
fn refused_shapes(
    origin: &str,
    existing: &Document,
    recorded: &[Document],
    input: &ShapeInput,
    policy: &DraftPolicy,
) -> Vec<String> {
    let (_, dropped) = filter(origin, existing, recorded, input, policy);
    let refused: BTreeSet<String> = dropped
        .into_iter()
        .filter(|d| d.reason == DropReason::BlockedRoute)
        .map(|d| d.shape)
        .collect();
    input
        .visited
        .iter()
        .chain(&input.links)
        .filter(|(shape, _)| refused.contains(&shape.to_string()))
        .map(|(shape, _)| shape.fill(origin, policy))
        .collect()
}

/// For each rule of `table`, whether it captures a link shape that is not
/// also one of the visited shapes it captures: a page only the page's links
/// suggested, which the structural drafter would have kept out of a visited
/// rule. Shapes count toward the first rule that matches, as in
/// [`describe_rules`].
fn uncovered_links(table: &Value, input: &ShapeInput, policy: &DraftPolicy) -> Vec<bool> {
    let origin = table
        .get("origin")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let rules = table
        .get("match")
        .and_then(Value::as_array)
        .unwrap_or_default();
    let documents: Vec<Option<Document>> = rules
        .iter()
        .map(|rule| matching_document(table, rule, policy))
        .collect();
    let first_rule = |shape: &UrlShape| {
        let url = shape.fill(origin, policy);
        documents
            .iter()
            .position(|d| d.as_ref().is_some_and(|d| matches_url(d, &url)))
    };
    let mut visited: Vec<BTreeSet<String>> = vec![BTreeSet::new(); rules.len()];
    for (shape, _) in &input.visited {
        if let Some(index) = first_rule(shape) {
            visited[index].insert(shape.to_string());
        }
    }
    let mut uncovered = vec![false; rules.len()];
    for (shape, _) in &input.links {
        if let Some(index) = first_rule(shape)
            && !visited[index].contains(&shape.to_string())
        {
            uncovered[index] = true;
        }
    }
    uncovered
}

/// The review fields of every rule of an adapter table, from the shapes it
/// matches: each visited and link shape is filled with the catalog's
/// examples and matched against the rule without scope. `visited` and
/// `links` sum the matching shapes' counts; a rule only link shapes match
/// is `link_only`, one no shape matches is `unseen`; `token_only` is set
/// when every identifier the ID uses is a `token`; and `example` is the
/// fill of the first matching shape in shape order, else the rule's own
/// fill. A rule that does not validate gets zero counts and `unseen`.
///
/// For a structural table this agrees with the drafter's own counts.
pub fn describe_rules(table: &Value, input: &ShapeInput, policy: &DraftPolicy) -> Vec<DraftedRule> {
    let origin = table
        .get("origin")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let rules = table
        .get("match")
        .and_then(Value::as_array)
        .unwrap_or_default();
    // Each rule as a one-rule document, `None` when it does not validate.
    let documents: Vec<Option<Document>> = rules
        .iter()
        .map(|rule| matching_document(table, rule, policy))
        .collect();
    let first_rule = |prepared: &Prepared| {
        documents.iter().position(|document| {
            document
                .as_ref()
                .is_some_and(|document| matches_prepared(document, prepared))
        })
    };
    let mut described: Vec<DraftedRule> = (0..rules.len())
        .map(|rule| DraftedRule {
            rule,
            link_only: false,
            token_only: false,
            visited: 0,
            links: 0,
            unseen: true,
            example: String::new(),
        })
        .collect();
    // The first matching shape of each rule, in shape order.
    let mut first: Vec<Option<(String, String)>> = vec![None; rules.len()];
    // A page is captured by the first rule that matches it, as evaluation
    // does, so each shape counts toward that rule only.
    for (from_links, shapes) in [(false, &input.visited), (true, &input.links)] {
        for (shape, count) in shapes {
            let url = shape.fill(origin, policy);
            let Ok(prepared) = Prepared::new(&url) else {
                continue;
            };
            let Some(index) = first_rule(&prepared) else {
                continue;
            };
            let entry = &mut described[index];
            if from_links {
                entry.links = entry.links.saturating_add(*count);
            } else {
                entry.visited = entry.visited.saturating_add(*count);
            }
            let text = shape.to_string();
            if first[index]
                .as_ref()
                .is_none_or(|(best, _)| text.as_str() < best.as_str())
            {
                first[index] = Some((text, url));
            }
        }
    }
    for ((entry, document), first) in described.iter_mut().zip(&documents).zip(first) {
        let Some(adapter) = document.as_ref().and_then(|d| d.adapters.first()) else {
            continue;
        };
        entry.token_only = id_token_only(adapter);
        entry.unseen = first.is_none();
        entry.link_only = entry.visited == 0 && !entry.unseen;
        entry.example = match first {
            Some((_, url)) => url,
            None => adapter
                .rules
                .first()
                .map(|rule| fill_rule(adapter, rule, policy))
                .unwrap_or_default(),
        };
    }
    described
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
    fn tenant_position_anchors_but_is_not_a_route_word() {
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
        // Tenants group as one position. The scoped tenant anchors a kind
        // with no route word (adapter-templates §4.1, §6), so `/acme/{num}`
        // is kept for the per-rule checks to judge. A blocked route word at
        // the tenant position is still a blocked route, never a tenant.
        assert_eq!(
            texts(&kinds),
            [["/{tenant}/{num}"], ["/{tenant}/{slug}/issues/{num}"]]
        );
        assert_eq!(kinds[1].rules[0].visited, 2);
        assert_eq!(
            drops(&dropped),
            [("/share/{slug}/issues/{num}", DropReason::BlockedRoute)]
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
        // kind: it is named as it would be without the link, and its other
        // namespace is a link-only kind of its own.
        assert_eq!(
            types(&draft),
            ["tools.pages", "tools.b", "tools.a", "tools.blog"]
        );
        let wiki = &draft.adapters[0];
        assert_eq!(
            rule_texts(&wiki.adapter),
            [
                "/wiki/pages/{num:int}/history",
                "/wiki/pages/{num:int}",
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
            [(0, 2, 0, false), (1, 1, 0, false), (2, 0, 3, true)]
        );
        assert!(draft.adapters[2].rules[0].link_only);
        assert_eq!(
            rule_texts(&draft.adapters[3].adapter),
            ["/wiki/blog/{num:int}"]
        );
        assert!(draft.adapters[3].rules[0].link_only);
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
        // are one kind split by route word into two adapters, since the
        // word is their only anchor, and `/x/{id}` is its own adapter.
        let kinds = draft_kinds(TOOLS, &existing, &[], &shapes, &test_catalog(), &[]);
        assert_eq!(kinds.adapters.len(), 4);
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
        // It is a namespace of its own, so it never shares their IDs.
        assert_eq!(types(&alone), ["tools.pages"]);
        assert_eq!(types(&planted), ["tools.pages", "tools.zzz"]);
        assert_eq!(ids(&planted)[0], ids(&alone)[0]);
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
        // The ID binds the visited words; the link's word is its own kind.
        assert_eq!(types(&both), ["tools.wiki", "tools.zzz"]);
        assert_eq!(ids(&both)[0], "tools.wiki:{host}/{space}/{num}");
        assert_eq!(
            rule_texts(&both.adapters[0].adapter),
            ["/wiki/{space:(blog|pages)}/{num:int}"]
        );
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

    // --- check_agent_draft and describe_rules ---

    /// The loop `draft_site` ran before [`check_in_rank_order`] was shared,
    /// kept verbatim as the reference.
    fn draft_site_reference(
        origin: &str,
        existing: &Document,
        recorded: &[Document],
        input: &ShapeInput,
        policy: &DraftPolicy,
        taken: &[String],
    ) -> SiteDraft {
        let (candidates, mut dropped) =
            candidates(origin, existing, recorded, input, policy, taken);
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
            ..SiteDraft::default()
        }
    }

    fn table(v: Json) -> Value {
        serde_json::from_value(v).unwrap()
    }

    /// A Tier 0 table on `origin` with these rule paths.
    fn tier0(origin: &str, artifact_type: &str, paths: &[&str], id: &[&str]) -> Value {
        let id: String = id.iter().map(|n| format!("/{{{n}}}")).collect();
        table(json!({
            "type": artifact_type,
            "origin": origin,
            "match": paths.iter().map(|p| json!({"path": p})).collect::<Vec<_>>(),
            "id": format!("{artifact_type}:{{host}}{id}"),
        }))
    }

    fn agent(
        origin: &str,
        existing: &Document,
        recorded: &[Document],
        input: &ShapeInput,
        taken: &[&str],
        tables: &[Value],
    ) -> SiteDraft {
        let taken: Vec<String> = taken.iter().map(|t| (*t).to_owned()).collect();
        check_agent_draft(
            origin,
            existing,
            recorded,
            input,
            &test_catalog(),
            &taken,
            tables,
        )
    }

    /// The structural inputs the shared loop is pinned on: drops of every
    /// kind, overlaps with existing and recorded contracts, shared origins.
    fn structural_cases() -> Vec<(&'static str, Document, Vec<Document>, ShapeInput)> {
        let history = doc(json!([{
            "type": "tools.history",
            "origin": TOOLS,
            "match": [{"path": "/docs/{n:int}/history"}],
            "id": "tools.history:{host}/{n}",
        }]));
        vec![
            (
                TOOLS,
                Document::default(),
                Vec::new(),
                input(
                    &[
                        "/docs/{num}",
                        "/d/{id}",
                        "/x/{num}/files",
                        "/",
                        "/search/{num}",
                    ],
                    &["/wiki/page/{num}", "/docs/{num}/{slug}/{slug}", "/{slug}"],
                ),
            ),
            (
                TOOLS,
                history.clone(),
                Vec::new(),
                input(
                    &["/docs/{num}", "/docs/{num}/{slug}/{slug}", "/tickets/{key}"],
                    &[],
                ),
            ),
            (
                TOOLS,
                Document::default(),
                vec![history],
                input(
                    &["/docs/{num}/history", "/docs/{num}"],
                    &["/docs/{num}/{slug}/{slug}"],
                ),
            ),
            (
                GITHUB,
                Document::default(),
                Vec::new(),
                input(
                    &["/acme/{slug}/issues/{num}", "/acme/{slug}/wiki/{num}"],
                    &["/acme/{slug}/pull/{num}/files"],
                ),
            ),
            (
                DOCS,
                Document::default(),
                Vec::new(),
                input(
                    &["/document/d/{id}/edit", "/document/u/{num}/d/{id}/edit"],
                    &[],
                ),
            ),
        ]
    }

    #[test]
    fn draft_site_results_are_unchanged_by_the_shared_check_loop() {
        let policy = test_catalog();
        for (origin, existing, recorded, input) in structural_cases() {
            let taken = vec!["tools.docs".to_owned()];
            assert_eq!(
                draft_site(origin, &existing, &recorded, &input, &policy, &taken),
                draft_site_reference(origin, &existing, &recorded, &input, &policy, &taken),
                "{origin}"
            );
        }
    }

    #[test]
    fn agent_tables_get_the_same_verdicts_as_identical_structural_tables() {
        for (origin, existing, recorded, input) in structural_cases() {
            let policy = test_catalog();
            let unchecked = draft_kinds(origin, &existing, &recorded, &input, &policy, &[]);
            let tables: Vec<Value> = unchecked
                .adapters
                .iter()
                .map(|a| a.adapter.clone())
                .collect();
            let structural = draft_site(origin, &existing, &recorded, &input, &policy, &[]);
            let agent = agent(origin, &existing, &recorded, &input, &[], &tables);
            assert_eq!(agent.rejected, None, "{origin}");
            assert_eq!(agent.adapters.len(), structural.adapters.len(), "{origin}");
            for (a, s) in agent.adapters.iter().zip(&structural.adapters) {
                assert_eq!(json_of(&a.adapter), json_of(&s.adapter), "{origin}");
                assert_eq!(a.scope, s.scope, "{origin}");
                assert_eq!(a.tenant, s.tenant, "{origin}");
                let flags = |rules: &[DraftedRule]| -> Vec<(usize, bool, bool, u32, u32)> {
                    rules
                        .iter()
                        .map(|r| (r.rule, r.link_only, r.token_only, r.visited, r.links))
                        .collect()
                };
                assert_eq!(flags(&a.rules), flags(&s.rules), "{origin}");
            }
            // Every rule the structural pass dropped by a check, the agent
            // pass drops with the same code.
            let mut structural_codes: Vec<DraftErrorCode> = structural
                .dropped
                .iter()
                .filter_map(|d| match d.reason {
                    DropReason::Check(code) => Some(code),
                    _ => None,
                })
                .collect();
            let mut agent_codes: Vec<DraftErrorCode> =
                agent.dropped_rules.iter().map(|d| d.reason).collect();
            structural_codes.sort();
            structural_codes.dedup();
            agent_codes.sort();
            agent_codes.dedup();
            assert_eq!(agent_codes, structural_codes, "{origin}");
        }
    }

    #[test]
    fn agent_draft_with_a_forbidden_key_in_any_table_is_rejected_whole() {
        let good = tier0(TOOLS, "tools.docs", &["/docs/{n:int}"], &["n"]);
        let input = input(&["/docs/{num}"], &[]);
        for (key, val) in [
            ("title", json!("{n}")),
            ("scope", json!({"n": ["1"]})),
            ("unscoped", json!("acknowledged")),
            ("alias", json!({"capture": "n"})),
            ("fields", json!({"a": "{n}"})),
            ("path_case", json!("insensitive")),
        ] {
            let Value::Table(mut entries) = tier0(TOOLS, "tools.wiki", &["/wiki/{n:int}"], &["n"])
            else {
                unreachable!()
            };
            entries.push((key.to_owned(), table(val)));
            let bad = Value::Table(entries);
            for tables in [[good.clone(), bad.clone()], [bad.clone(), good.clone()]] {
                let site = agent(TOOLS, &Document::default(), &[], &input, &[], &tables);
                assert_eq!(site.rejected, Some(DraftErrorCode::ForbiddenKey), "{key}");
                assert!(
                    site.adapters.is_empty() && site.dropped_rules.is_empty(),
                    "{key}"
                );
            }
        }
    }

    #[test]
    fn agent_draft_for_another_origin_is_rejected_whole() {
        let input = input(&["/docs/{num}"], &[]);
        let tables = [
            tier0(TOOLS, "tools.docs", &["/docs/{n:int}"], &["n"]),
            tier0(
                "https://other.example.com",
                "other.docs",
                &["/docs/{n:int}"],
                &["n"],
            ),
        ];
        let site = agent(TOOLS, &Document::default(), &[], &input, &[], &tables);
        assert_eq!(site.rejected, Some(DraftErrorCode::InvalidRule));
        assert!(site.adapters.is_empty());
    }

    #[test]
    fn agent_draft_that_does_not_parse_is_rejected_whole() {
        let input = input(&["/docs/{num}"], &[]);
        let good = tier0(TOOLS, "tools.docs", &["/docs/{n:int}"], &["n"]);
        let too_many: Vec<Value> = (0..=MAX_DRAFT_ADAPTERS)
            .map(|i| {
                tier0(
                    TOOLS,
                    &format!("tools.d{i}"),
                    &[&format!("/d{i}/{{n:int}}")],
                    &["n"],
                )
            })
            .collect();
        let too_many_rules = tier0(
            TOOLS,
            "tools.many",
            &(0..=MAX_RULES)
                .map(|i| format!("/m{i}/{{n:int}}"))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            &["n"],
        );
        for tables in [
            vec![good.clone(), table(json!(["not", "a", "table"]))],
            vec![
                good.clone(),
                table(
                    json!({"type": "tools.x", "origin": TOOLS, "match": "/x/{n:int}", "id": "tools.x:{host}/{n}"}),
                ),
            ],
            vec![
                good.clone(),
                table(
                    json!({"type": "tools.x", "origin": TOOLS, "match": [{"path": "/x/{n:int}"}], "id": "tools.x:{host}/{n}", "format": 1}),
                ),
            ],
            vec![good.clone(), too_many_rules],
            too_many,
            Vec::new(),
        ] {
            let site = agent(TOOLS, &Document::default(), &[], &input, &[], &tables);
            assert_eq!(
                site.rejected,
                Some(DraftErrorCode::InvalidRule),
                "{tables:?}"
            );
            assert!(site.adapters.is_empty());
        }
    }

    #[test]
    fn agent_rule_overlapping_an_earlier_agent_table_is_dropped_as_overlap() {
        let input = input(&["/docs/{num}"], &[]);
        let tables = [
            tier0(TOOLS, "tools.docs", &["/docs/{n:int}"], &["n"]),
            tier0(
                TOOLS,
                "tools.other",
                &["/docs/{m:int}", "/other/{m:int}"],
                &["m"],
            ),
        ];
        let site = agent(TOOLS, &Document::default(), &[], &input, &[], &tables);
        assert_eq!(
            site.dropped_rules,
            [DroppedRule {
                table: 1,
                rule: Some(0),
                reason: DraftErrorCode::Overlap,
            }]
        );
        assert_eq!(rule_texts(&site.adapters[1].adapter), ["/other/{m:int}"]);
    }

    #[test]
    fn agent_rule_overlapping_a_recorded_contract_is_dropped_as_overlap() {
        let recorded = doc(json!([{
            "type": "tools.history",
            "origin": TOOLS,
            "match": [{"path": "/docs/{n:int}/history"}],
            "id": "tools.history:{host}/{n}",
        }]));
        let input = input(&["/docs/{num}", "/docs/{num}/history"], &[]);
        let tables = [tier0(
            TOOLS,
            "tools.docs",
            &["/docs/{n:int}", "/docs/{n:int}/history"],
            &["n"],
        )];
        let site = agent(
            TOOLS,
            &Document::default(),
            &[recorded],
            &input,
            &[],
            &tables,
        );
        assert_eq!(
            site.dropped_rules,
            [DroppedRule {
                table: 0,
                rule: Some(1),
                reason: DraftErrorCode::Overlap,
            }]
        );
        assert_eq!(rule_texts(&site.adapters[0].adapter), ["/docs/{n:int}"]);
    }

    #[test]
    fn agent_types_that_are_taken_get_a_numeric_suffix_and_the_id_scheme_follows() {
        let existing = doc(json!([{
            "type": "tools.wiki",
            "origin": TOOLS,
            "match": [{"path": "/wiki/{n:int}"}],
            "id": "tools.wiki:{host}/{n}",
        }]));
        let tables = [
            tier0(TOOLS, "tools.docs", &["/docs/{n:int}"], &["n"]),
            tier0(TOOLS, "tools.docs", &["/doc/{n:int}"], &["n"]),
            tier0(TOOLS, "tools.wiki", &["/w/{n:int}"], &["n"]),
            tier0(TOOLS, "jira.issue", &["/browse/{k:jira_key}"], &["k"]),
            tier0(TOOLS, "tools.free", &["/free/{n:int}"], &["n"]),
        ];
        let input = input(&["/docs/{num}"], &[]);
        let site = agent(TOOLS, &existing, &[], &input, &["tools.docs"], &tables);
        assert_eq!(site.rejected, None, "{site:?}");
        assert_eq!(
            types(&site),
            [
                "tools.docs2",
                "tools.docs3",
                "tools.wiki2",
                "jira.issue2",
                "tools.free"
            ]
        );
        assert_eq!(
            ids(&site),
            [
                "tools.docs2:{host}/{n}",
                "tools.docs3:{host}/{n}",
                "tools.wiki2:{host}/{n}",
                "jira.issue2:{host}/{k}",
                "tools.free:{host}/{n}",
            ]
        );
    }

    #[test]
    fn agent_id_scheme_that_differs_from_its_type_is_invalid() {
        let input = input(&["/docs/{num}"], &[]);
        for id in [
            "other.docs:{host}/{n}",
            "{host}/{n}",
            "tools.docs2:{host}/{n}",
        ] {
            let tables = [table(json!({
                "type": "tools.docs",
                "origin": TOOLS,
                "match": [{"path": "/docs/{n:int}"}],
                "id": id,
            }))];
            let site = agent(TOOLS, &Document::default(), &[], &input, &[], &tables);
            assert_eq!(site.rejected, Some(DraftErrorCode::InvalidRule), "{id}");
            assert!(site.adapters.is_empty());
        }
    }

    #[test]
    fn agent_rule_matching_no_seen_shape_starts_unchecked_as_unseen() {
        let input = input(&["/docs/{num}"], &["/wiki/{num}"]);
        let tables = [tier0(
            TOOLS,
            "tools.docs",
            &["/docs/{n:int}", "/tickets/{n:int}", "/wiki/{n:int}"],
            &["n"],
        )];
        let site = agent(TOOLS, &Document::default(), &[], &input, &[], &tables);
        let rules = &site.adapters[0].rules;
        assert_eq!(
            rules
                .iter()
                .map(|r| (r.rule, r.link_only, r.unseen, r.visited, r.links))
                .collect::<Vec<_>>(),
            [
                (0, false, false, 1, 0),
                (1, false, true, 0, 0),
                (2, true, false, 0, 1),
            ]
        );
        // The unseen rule's example is its own fill, never a visited URL.
        assert_eq!(rules[1].example, format!("{TOOLS}/tickets/123456"));
        assert_eq!(rules[0].example, format!("{TOOLS}/docs/123456"));
    }

    /// Whether a rule the review starts checked captures `url`.
    fn checked_rule_matches(site: &SiteDraft, url: &str) -> bool {
        let policy = test_catalog();
        site.adapters.iter().any(|adapter| {
            let rules = adapter
                .adapter
                .get("match")
                .and_then(Value::as_array)
                .unwrap_or_default();
            adapter.rules.iter().any(|r| {
                !(r.link_only || r.token_only || r.unseen)
                    && matching_document(&adapter.adapter, &rules[r.rule], &policy)
                        .is_some_and(|document| matches_url(&document, url))
            })
        })
    }

    #[test]
    fn agent_rule_widened_over_planted_link_shapes_is_dropped_or_starts_unchecked() {
        let policy = test_catalog();
        // (rule path, visited shapes, link shapes, the drop expected)
        type Case = (
            &'static str,
            &'static [&'static str],
            &'static [&'static str],
            Option<DraftErrorCode>,
        );
        let cases: [Case; 3] = [
            (
                "/wiki/{n:int}/{*}",
                &["/wiki/{num}"],
                &["/wiki/{num}/share/{slug}", "/wiki/{num}/invite/{hex32}"],
                Some(DraftErrorCode::BlockedRoute),
            ),
            (
                "/wiki/{_}/{n:int}",
                &["/wiki/doc/{num}"],
                &["/wiki/share/{num}", "/wiki/reset/{num}"],
                Some(DraftErrorCode::BlockedRoute),
            ),
            (
                "/wiki/(doc|page)/{n:int}",
                &["/wiki/doc/{num}"],
                &["/wiki/page/{num}"],
                None,
            ),
        ];
        for (path, visited, links, drop) in cases {
            let input = input(visited, links);
            // Only the widened table: no narrow rule before it to overlap.
            let tables = [tier0(TOOLS, "tools.wiki", &[path], &["n"])];
            let site = agent(TOOLS, &Document::default(), &[], &input, &[], &tables);
            assert_eq!(site.rejected, None, "{path}: {site:?}");
            for (shape, _) in &input.links {
                let url = shape.fill(TOOLS, &policy);
                assert!(
                    !checked_rule_matches(&site, &url),
                    "{path}: a checked rule captures the planted {url}: {site:?}"
                );
            }
            match drop {
                Some(code) => {
                    assert!(site.adapters.is_empty(), "{path}: {site:?}");
                    assert_eq!(
                        site.dropped_rules,
                        [DroppedRule {
                            table: 0,
                            rule: Some(0),
                            reason: code
                        }],
                        "{path}"
                    );
                }
                None => {
                    let rule = &site.adapters[0].rules[0];
                    assert!(rule.link_only, "{path}: {rule:?}");
                    assert_eq!((rule.visited, rule.links), (1, 1), "{path}");
                }
            }
        }
        // A link shape the rule's own visited shapes cover leaves it checked.
        let input = input(&["/wiki/{num}"], &["/wiki/{num}"]);
        let tables = [tier0(TOOLS, "tools.wiki", &["/wiki/{n:int}"], &["n"])];
        let site = agent(TOOLS, &Document::default(), &[], &input, &[], &tables);
        let rule = &site.adapters[0].rules[0];
        assert!(!(rule.link_only || rule.unseen), "{rule:?}");
        assert_eq!((rule.visited, rule.links), (1, 1));
    }

    #[test]
    fn agent_link_only_and_token_only_rules_are_flagged_like_structural_ones() {
        let input = input(&["/doc/{id}", "/wiki/{num}"], &["/wiki/{num}/history"]);
        let structural = site(TOOLS, &Document::default(), &input, &[]);
        let tables = [
            tier0(TOOLS, "tools.doc", &["/doc/{t:token(1,128)}"], &["t"]),
            tier0(
                TOOLS,
                "tools.wiki",
                &["/wiki/{n:int}", "/wiki/{n:int}/history"],
                &["n"],
            ),
        ];
        let site = agent(TOOLS, &Document::default(), &[], &input, &[], &tables);
        let flags = |d: &SiteDraft| -> Vec<(bool, bool, bool)> {
            d.adapters
                .iter()
                .flat_map(|a| {
                    a.rules
                        .iter()
                        .map(|r| (r.link_only, r.token_only, r.unseen))
                })
                .collect()
        };
        assert_eq!(
            flags(&site),
            [
                (false, true, false),
                (false, false, false),
                (true, false, false)
            ]
        );
        assert_eq!(flags(&site), flags(&structural));
        assert_eq!(
            site.adapters[0].rules[0].example,
            format!("{TOOLS}/doc/example123")
        );
    }

    #[test]
    fn describe_rules_agrees_with_structural_counts() {
        let policy = test_catalog();
        let input = ShapeInput {
            visited: counted(&[
                ("/docs/{num}", 3),
                ("/docs/{num}/history", 2),
                ("/d/{id}", 1),
                ("/x/{num}/files", 4),
            ]),
            links: counted(&[("/docs/{num}/{slug}/{slug}", 5), ("/wiki/page/{num}", 2)]),
        };
        let draft = draft_site(TOOLS, &Document::default(), &[], &input, &policy, &[]);
        assert!(draft.adapters.len() >= 3, "{draft:?}");
        for adapter in &draft.adapters {
            assert_eq!(
                describe_rules(&adapter.adapter, &input, &policy),
                adapter.rules,
                "{}",
                str_of(&adapter.adapter, "type")
            );
        }
    }

    #[test]
    fn agent_shared_origin_tables_get_the_hosts_scope_state_and_tenant() {
        let input = input(&["/acme/{slug}/wiki/{num}"], &[]);
        let tables = [table(json!({
            "type": "github.page",
            "origin": GITHUB,
            "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/wiki/{n:int}"}],
            "id": "github.page:{host}/{owner}/{repo}/{n}",
        }))];
        let pending = agent(GITHUB, &Document::default(), &[], &input, &[], &tables);
        assert_eq!(pending.rejected, None, "{pending:?}");
        assert_eq!(
            pending.adapters[0].scope,
            ScopeState::Pending {
                capture: "owner".to_owned()
            }
        );
        assert_eq!(pending.adapters[0].tenant, Some((0, "owner".to_owned())));
        assert!(!pending.adapters[0].rules[0].unseen);

        let existing = doc(json!([{
            "type": "github.issue",
            "origin": GITHUB,
            "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}"}],
            "scope": {"owner": ["acme"]},
            "id": "github.issue:{host}/{owner}/{repo}/{number}",
        }]));
        let copied = agent(GITHUB, &existing, &[], &input, &[], &tables);
        assert_eq!(
            copied.adapters[0].scope,
            ScopeState::Copied(vec![("owner".to_owned(), vec!["acme".to_owned()])])
        );
        assert_eq!(copied.adapters[0].tenant, Some((0, "owner".to_owned())));
        // The checked table never carries the scope: the review overlays it.
        assert!(copied.adapters[0].adapter.get("scope").is_none());
    }
}

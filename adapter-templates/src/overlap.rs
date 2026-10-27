//! The conservative overlap check (§10.3).
//!
//! Two rules "may overlap" unless some structural fact proves no URL can
//! match both. Each path position is reduced to a set (anything, a list of
//! literals, or a type's alphabet and length range), so the check can report
//! an overlap no URL produces but never misses one.

use crate::ctype::CType;
use crate::document::{Adapter, Document, QueryCond, Rule};
use crate::path::{PathTemplate, Segment};

/// Past this many segments no URL is prepared (§5.1 step 6).
const MAX_COUNT: usize = 64;

/// The values one position can take.
enum Set<'a> {
    Any,
    Literals(Vec<&'a str>),
    Typed(&'a CType),
}

fn ctype_set(ctype: &CType) -> Set<'_> {
    match ctype {
        CType::Alternation(lits) => Set::Literals(lits.iter().map(String::as_str).collect()),
        // `@tail` ignores the text before the last `-` (SQ-V7).
        CType::Typed { tail: true, .. } => Set::Any,
        CType::Typed { .. } => Set::Typed(ctype),
    }
}

fn segment_set(seg: &Segment) -> Set<'_> {
    match seg {
        Segment::Literal(l) => Set::Literals(vec![l.as_str()]),
        Segment::Alternation(lits) => Set::Literals(lits.iter().map(String::as_str).collect()),
        Segment::Capture { ctype, .. } => ctype_set(ctype),
        Segment::Anonymous | Segment::Rest => Set::Any,
    }
}

/// The set at position `i` of a URL with `n` segments.
fn position_set(t: &PathTemplate, i: usize) -> Set<'_> {
    let (r, o) = (t.required, t.optional);
    if i < r || (i < r + o && !t.rest) {
        segment_set(&t.segments[i])
    } else {
        // An optional that fails falls through to the rest, and the rest
        // takes anything.
        Set::Any
    }
}

fn intersects(a: &Set<'_>, b: &Set<'_>, fold: bool) -> bool {
    let typed = |c: &CType, lit: &str| c.canonical(lit).is_some();
    match (a, b) {
        (Set::Any, _) | (_, Set::Any) => true,
        (Set::Literals(x), Set::Literals(y)) => x.iter().any(|l| {
            y.iter().any(|m| {
                if fold {
                    l.eq_ignore_ascii_case(m)
                } else {
                    l == m
                }
            })
        }),
        // Type validity does not depend on ASCII case, so folding cannot
        // change membership.
        (Set::Literals(x), Set::Typed(c)) | (Set::Typed(c), Set::Literals(x)) => {
            x.iter().any(|l| typed(c, l))
        }
        (Set::Typed(c), Set::Typed(d)) => {
            let (
                CType::Typed {
                    base: b1,
                    bounds: n1,
                    ..
                },
                CType::Typed {
                    base: b2,
                    bounds: n2,
                    ..
                },
            ) = (c, d)
            else {
                return true;
            };
            let (l1, h1) = b1.value_len(*n1);
            let (l2, h2) = b2.value_len(*n2);
            b1.alphabet_mask() & b2.alphabet_mask() != 0 && l1.max(l2) <= h1.min(h2)
        }
    }
}

fn count_range(t: &PathTemplate) -> (usize, usize) {
    let max = if t.rest {
        MAX_COUNT
    } else {
        t.required + t.optional
    };
    (t.required, max.min(MAX_COUNT))
}

fn paths_may_overlap(a: &PathTemplate, b: &PathTemplate, fold: bool) -> bool {
    let (lo_a, hi_a) = count_range(a);
    let (lo_b, hi_b) = count_range(b);
    // Beyond both templates' lengths every position is `Any` for both, so
    // one count past the longer template decides every larger count.
    let hi = hi_a
        .min(hi_b)
        .min(a.segments.len().max(b.segments.len()) + 1);
    (lo_a.max(lo_b)..=hi)
        .any(|n| (0..n).all(|i| intersects(&position_set(a, i), &position_set(b, i), fold)))
}

/// SQ-V6: only a key required in one rule and absent in the other, or two
/// conditions on one key that cannot share a value, prove disjointness.
fn queries_disjoint(a: &Rule, b: &Rule) -> bool {
    let required_absent = |x: &Rule, y: &Rule| x.query.iter().any(|(k, _)| y.absent.contains(k));
    if required_absent(a, b) || required_absent(b, a) {
        return true;
    }
    a.query.iter().any(|(key, ca)| {
        b.query.iter().any(|(k, cb)| {
            k == key
                && match (ca, cb) {
                    (QueryCond::Literal(x), QueryCond::Literal(y)) => x != y,
                    (QueryCond::Capture { ctype: x, .. }, QueryCond::Capture { ctype: y, .. }) => {
                        !intersects(&ctype_set(x), &ctype_set(y), false)
                    }
                    _ => false,
                }
        })
    })
}

/// Whether two rules could match one URL. Different origins never overlap.
pub(crate) fn rules_may_overlap(a: &Adapter, ra: &Rule, b: &Adapter, rb: &Rule) -> bool {
    a.origin == b.origin
        && paths_may_overlap(&ra.path, &rb.path, a.insensitive || b.insensitive)
        && !queries_disjoint(ra, rb)
}

/// A pair of rules from different adapters that may overlap; `first` wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlapPair {
    pub first: (usize, usize),
    pub second: (usize, usize),
}

impl Document {
    /// Whether rule `a` and rule `b`, each `(adapter, rule)`, may overlap.
    /// Panics on an index out of range.
    pub fn may_overlap(&self, a: (usize, usize), b: (usize, usize)) -> bool {
        let (aa, ab) = (&self.adapters[a.0], &self.adapters[b.0]);
        rules_may_overlap(aa, &aa.rules[a.1], ab, &ab.rules[b.1])
    }

    /// The overlap warnings (§10.3): every pair of rules from different
    /// adapters on one origin that may overlap, ordered by `first` and then
    /// `second`, except a declared fall-through. When the earlier adapter
    /// declares `within` the later adapter's type (§8.3), the later one is its
    /// container's catch-all and the overlap is intended, so it is not
    /// reported. The reverse order, where the container would win, still is.
    pub fn overlap_report(&self) -> Vec<OverlapPair> {
        self.pairs(true)
    }

    /// Every pair of rules from different adapters on one origin that may
    /// overlap, including declared fall-throughs, in `overlap_report` order.
    /// For checks that must see every shared URL, such as route history.
    pub fn overlapping_rules(&self) -> Vec<OverlapPair> {
        self.pairs(false)
    }

    fn pairs(&self, skip_declared: bool) -> Vec<OverlapPair> {
        let mut out = Vec::new();
        for (i, a) in self.adapters.iter().enumerate() {
            for (j, b) in self.adapters.iter().enumerate().skip(i + 1) {
                if skip_declared && sits_within(a, b) {
                    continue;
                }
                for (ri, ra) in a.rules.iter().enumerate() {
                    for (rj, rb) in b.rules.iter().enumerate() {
                        if rules_may_overlap(a, ra, b, rb) {
                            out.push(OverlapPair {
                                first: (i, ri),
                                second: (j, rj),
                            });
                        }
                    }
                }
            }
        }
        out.sort_by_key(|pair| (pair.first, pair.second));
        out
    }
}

/// Whether `contained` declares `within` the type of `container` on its origin.
fn sits_within(contained: &Adapter, container: &Adapter) -> bool {
    contained.origin == container.origin
        && contained
            .within
            .as_ref()
            .is_some_and(|within| within.artifact_type == container.artifact_type)
}

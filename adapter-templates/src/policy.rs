//! Host-owned drafting policy (DESIGN §5.1, adapter-templates.md §10.4).
//!
//! The probe URLs, query-key lists, blocked route words, mode words,
//! placeholder fill values, starter types and shared-origin tenant and
//! account-index positions live in one host-owned data file,
//! `protocol/capture-site/catalog.json`. The host parses it and hands the
//! drafter this plain struct, so this crate needs no JSON parser.

/// Everything the structural drafter and the draft checks read from the
/// host catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftPolicy {
    /// Probe paths, each starting with `/` and optionally carrying a query,
    /// in the order the review lists them. No drafted rule may match one.
    pub probe_paths: Vec<String>,
    /// Query keys that carry a search; never an identifier.
    pub search_keys: Vec<String>,
    /// Query keys that carry paging, sorting or view state; never an
    /// identifier.
    pub paging_keys: Vec<String>,
    /// Route words a drafted rule may not use as a literal.
    pub blocked_route_words: Vec<String>,
    /// `(route word, mode)` pairs, sorted by route word.
    pub mode_words: Vec<(String, String)>,
    /// Example values for each shape placeholder class.
    pub fill: Fill,
    /// Artifact types reserved for starters; a draft never takes one.
    pub starter_types: Vec<String>,
    /// Shared origins (adapter-templates.md §6), sorted by origin.
    pub shared: Vec<SharedOrigin>,
}

/// Fixed example values for the URL-shape placeholder classes. A filled
/// shape is an illustrative URL, never a real one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub num: String,
    pub key: String,
    pub slug: String,
    pub uuid: String,
    pub hex32: String,
    pub hex40: String,
    pub hex64: String,
    pub tailhex32: String,
    pub id: String,
}

/// One shared origin's drafting positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedOrigin {
    pub origin: String,
    /// `(zero-based path segment, capture name)` where the URL names the
    /// tenant, or `None` where it cannot (only `unscoped` is offered).
    pub tenant: Option<(usize, String)>,
    /// `(literal, zero-based path segment of the literal)`: when that
    /// segment equals the literal, the segment after it is an account
    /// index such as Google's `/u/<n>/`, which never enters an ID.
    pub account_index: Vec<(String, usize)>,
}

impl DraftPolicy {
    /// The shared-origin entry for `origin`, if it is a shared origin.
    pub fn shared_origin(&self, origin: &str) -> Option<&SharedOrigin> {
        self.shared.iter().find(|entry| entry.origin == origin)
    }
}

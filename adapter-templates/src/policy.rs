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

/// The host catalog, parsed the way the host parses it, for unit tests.
#[cfg(test)]
pub(crate) fn test_catalog() -> DraftPolicy {
    use serde_json::Value as Json;

    const CATALOG: &str = include_str!("../../protocol/capture-site/catalog.json");
    let strings = |v: &Json| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect()
    };
    let c: Json = serde_json::from_str(CATALOG).unwrap();
    let fill = |k: &str| c["fill"][k].as_str().unwrap().to_owned();
    DraftPolicy {
        probe_paths: strings(&c["probe_paths"]),
        search_keys: strings(&c["search_keys"]),
        paging_keys: strings(&c["paging_keys"]),
        blocked_route_words: strings(&c["blocked_route_words"]),
        mode_words: c["mode_words"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect(),
        fill: Fill {
            num: fill("num"),
            key: fill("key"),
            slug: fill("slug"),
            uuid: fill("uuid"),
            hex32: fill("hex32"),
            hex40: fill("hex40"),
            hex64: fill("hex64"),
            tailhex32: fill("tailhex32"),
            id: fill("id"),
        },
        starter_types: strings(&c["starter_types"]),
        shared: c["shared_origins"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(origin, e)| SharedOrigin {
                origin: origin.clone(),
                tenant: e["tenant"].as_object().map(|t| {
                    (
                        t["segment"].as_u64().unwrap() as usize,
                        t["capture"].as_str().unwrap().to_owned(),
                    )
                }),
                account_index: e["account_index"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| {
                        (
                            a["literal"].as_str().unwrap().to_owned(),
                            a["segment"].as_u64().unwrap() as usize,
                        )
                    })
                    .collect(),
            })
            .collect(),
    }
}

//! Capture Site drafting for sites no starter covers (DESIGN §5.1).
//!
//! The probe URLs, query-key lists, route and mode words, placeholder fill
//! values, starter types and shared-origin positions are host-owned data in
//! `protocol/capture-site/catalog.json`. The extension's own copies are
//! pinned to the same file by its tests.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use adapter_templates::{DraftPolicy, Fill, SharedOrigin};
use serde::Deserialize;

const CATALOG: &str = include_str!("../../protocol/capture-site/catalog.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    format: u32,
    probe_paths: Vec<String>,
    search_keys: Vec<String>,
    paging_keys: Vec<String>,
    blocked_route_words: Vec<String>,
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

//! Step 7.6 drafter cases (DESIGN §5.1, §12.2 step 7.6) on real recorded
//! URLs. The shapes come from the shared shape fixture
//! (`protocol/capture-site/shapes.json`) and the drafting policy from the
//! host catalog (`protocol/capture-site/catalog.json`), parsed here by hand
//! the way the host parses it. Each draft is then judged by evaluating the
//! original real URL with the Rust engine; a draft whose scope is still the
//! review's to choose is evaluated unscoped, which matches without scope as
//! the draft checks do.

use std::collections::{BTreeMap, BTreeSet};

use adapter_templates::structural::{
    DraftedAdapter, DropReason, ShapeInput, SiteDraft, draft_site,
};
use adapter_templates::{
    Document, DraftErrorCode, Provenance, RuleVerdict, ScopeState, Value, check_draft_rules,
};
use serde_json::{Value as Json, json};

mod common;

use common::{
    SHAPES, TOOLS, document, drafted_document, empty, evaluable, policy, rules, shapes, value,
};

const GITHUB: &str = "https://github.com";
const NOTION: &str = "https://www.notion.so";
const WIKI: &str = "https://wiki.example";

fn draft(origin: &str, existing: &Document, visited: &[&str], links: &[&str]) -> SiteDraft {
    let input = ShapeInput {
        visited: shapes(visited),
        links: shapes(links),
    };
    draft_site(origin, existing, &[], &input, &policy(), &[])
}

/// The adapter's `type`.
fn kind(adapter: &DraftedAdapter) -> &str {
    adapter
        .adapter
        .get("type")
        .and_then(Value::as_str)
        .expect("type")
}

/// The adapter's `id` template.
fn id_template(adapter: &DraftedAdapter) -> &str {
    adapter
        .adapter
        .get("id")
        .and_then(Value::as_str)
        .expect("id")
}

fn all_rules(site: &SiteDraft) -> Vec<String> {
    site.adapters.iter().flat_map(rules).collect()
}

fn drops(site: &SiteDraft) -> Vec<(&str, DropReason)> {
    site.dropped
        .iter()
        .map(|d| (d.shape.as_str(), d.reason))
        .collect()
}

fn reason_of(site: &SiteDraft, shape: &str) -> Option<DropReason> {
    site.dropped
        .iter()
        .find(|d| d.shape == shape)
        .map(|d| d.reason)
}

/// `(note, url, shape)` for every accept row of the shape fixture that has a
/// plain URL and shape.
fn fixture_rows() -> Vec<(String, String, String)> {
    let fixture: Json = serde_json::from_str(SHAPES).expect("shapes.json parses");
    fixture["accept"]
        .as_array()
        .expect("accept rows")
        .iter()
        .filter_map(|row| {
            Some((
                row["note"].as_str()?.to_owned(),
                row["url"].as_str()?.to_owned(),
                row["shape"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// The shape fixture's row with this note.
fn fixture_row(note: &str) -> (String, String) {
    fixture_rows()
        .into_iter()
        .find(|(n, _, _)| n == note)
        .map(|(_, url, shape)| (url, shape))
        .unwrap_or_else(|| panic!("no fixture row {note}"))
}

fn origin_of(url: &str) -> String {
    url::Url::parse(url)
        .expect("fixture URL")
        .origin()
        .ascii_serialization()
}

#[test]
fn starter_recorded_urls_draft_to_matching_adapters_one_per_kind() {
    // The recorded URL forms of every shipped starter, grouped by origin, as
    // the panel would send them: each origin's own shapes, visited.
    let mut by_origin: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for (note, url, shape) in fixture_rows() {
        if note.contains("(docs/adapters/") {
            by_origin
                .entry(origin_of(&url))
                .or_default()
                .push((note, url, shape));
        }
    }
    let starters: BTreeSet<&str> = by_origin
        .values()
        .flatten()
        .filter_map(|(note, _, _)| note.split("(docs/adapters/").nth(1))
        .collect();
    assert_eq!(
        starters.len(),
        4,
        "every starter has a recorded URL: {starters:?}"
    );

    let policy = policy();
    let mut failures = Vec::new();
    for (origin, rows) in &by_origin {
        let visited: Vec<&str> = rows.iter().map(|(_, _, shape)| shape.as_str()).collect();
        let site = draft(origin, &empty(), &visited, &[]);
        let doc = drafted_document(&site);

        let types: Vec<&str> = site.adapters.iter().map(kind).collect();
        let unique: BTreeSet<&str> = types.iter().copied().collect();
        assert_eq!(unique.len(), types.len(), "{origin}: types {types:?}");
        for adapter in &site.adapters {
            let scheme = id_template(adapter).split(':').next().unwrap_or_default();
            assert_ne!(scheme, "web", "{origin}: {}", kind(adapter));
            assert_eq!(scheme, kind(adapter), "{origin}: scheme is the type");
            assert!(
                !policy.starter_types.iter().any(|s| s == kind(adapter)),
                "{origin}: a draft never takes a starter type"
            );
        }

        // Each real URL is matched, by exactly one adapter.
        let mut matched: BTreeMap<usize, Vec<(String, String)>> = BTreeMap::new();
        for (note, url, _) in rows {
            match doc.evaluate(url, None) {
                Ok(m) => {
                    let owners = site
                        .adapters
                        .iter()
                        .filter(|a| document(vec![evaluable(a)]).evaluate(url, None).is_ok())
                        .count();
                    if owners != 1 {
                        failures.push(format!("{note}: {owners} adapters match {url}"));
                    }
                    assert!(
                        m.artifact_id.starts_with(&format!("{}:", m.artifact_type)),
                        "{note}: {}",
                        m.artifact_id
                    );
                    matched
                        .entry(m.adapter)
                        .or_default()
                        .push((note.clone(), m.artifact_id));
                }
                Err(e) => failures.push(format!("{note}: {url} not matched: {e:?}")),
            }
        }

        // One adapter per kind: every adapter is backed by a recorded URL,
        // and the URLs of one recorded artifact that one adapter matches
        // agree on its ID.
        for index in 0..site.adapters.len() {
            if !matched.contains_key(&index) {
                failures.push(format!(
                    "{origin}: {} matches no recorded URL",
                    kind(&site.adapters[index])
                ));
            }
        }
        for (index, ids) in &matched {
            let same_artifact = |note: &str| {
                // Recorded GitHub pull-request sub-routes all name PR 42 of
                // acme/widget; SharePoint's recorded forms all name one
                // document per host.
                note.contains("github.pr.md") || note.contains("sharepoint.doc.md")
            };
            let artifact: BTreeSet<&str> = ids
                .iter()
                .filter(|(note, _)| same_artifact(note))
                .map(|(_, id)| id.as_str())
                .collect();
            if artifact.len() > 1 {
                failures.push(format!(
                    "{origin}: {} gives one artifact several IDs: {artifact:?}",
                    kind(&site.adapters[*index])
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn github_pull_request_and_files_tab_are_one_adapter_keyed_by_owner_repo_number() {
    let site = draft(
        GITHUB,
        &empty(),
        &["/acme/{slug}/pull/{num}", "/acme/{slug}/pull/{num}/files"],
        &[],
    );
    assert_eq!(site.adapters.len(), 1, "{site:?}");
    let pr = &site.adapters[0];
    assert_eq!(kind(pr), "github.pull");
    assert_eq!(
        rules(pr),
        [
            "/{owner:name|lower}/{space:name}/pull/{num:int}",
            "/{owner:name|lower}/{space:name}/pull/{num:int}/{mode:(files)}",
        ]
    );
    assert_eq!(id_template(pr), "github.pull:{host}/{owner}/{space}/{num}");
    assert_eq!(
        pr.scope,
        ScopeState::Pending {
            capture: "owner".into()
        }
    );
    assert_eq!(pr.tenant, Some((0, "owner".into())));
    assert!(pr.rules.iter().all(|r| !r.link_only && !r.token_only));

    // The overview and its files tab are one artifact, in review mode on
    // the files tab.
    let doc = drafted_document(&site);
    let overview = doc
        .evaluate("https://github.com/Acme/Widget-App/pull/42", None)
        .expect("overview");
    let files = doc
        .evaluate("https://github.com/acme/Widget-App/pull/42/files", None)
        .expect("files");
    assert_eq!(
        overview.artifact_id,
        "github.pull:github.com/acme/Widget-App/42"
    );
    assert_eq!(files.artifact_id, overview.artifact_id);
    assert_eq!(overview.mode, None);
    assert_eq!(files.mode.map(|m| m.as_str()), Some("review"));

    // A lowercase short repository name stays a literal in the shape, so its
    // draft is bound to that one repository and is never wider.
    let site = draft(
        GITHUB,
        &empty(),
        &["/acme/widget/pull/{num}", "/acme/widget/pull/{num}/files"],
        &[],
    );
    assert_eq!(site.adapters.len(), 1, "{site:?}");
    assert_eq!(
        rules(&site.adapters[0]),
        [
            "/{owner:name|lower}/widget/pull/{num:int}",
            "/{owner:name|lower}/widget/pull/{num:int}/{mode:(files)}",
        ]
    );
    let doc = drafted_document(&site);
    assert!(
        doc.evaluate("https://github.com/acme/widget/pull/42/files", None)
            .is_ok()
    );
    assert!(
        doc.evaluate("https://github.com/acme/gadget/pull/42", None)
            .is_err()
    );
}

#[test]
fn title_prefixed_and_bare_hex_pages_share_one_tail_id_behind_a_route_word() {
    let (prefixed_url, prefixed) =
        fixture_row("Non-shared title-prefixed page behind a route word");
    let (bare_url, bare) = fixture_row("Non-shared bare hex page behind a route word");
    let site = draft(WIKI, &empty(), &[&prefixed, &bare], &[]);
    assert_eq!(site.adapters.len(), 1, "{site:?}");
    let page = &site.adapters[0];
    assert_eq!(rules(page), ["/p/{hex:hex(32)@tail}"]);
    assert_eq!(id_template(page), "wiki.p:{host}/{hex}");
    assert_eq!(page.rules[0].visited, 2);
    let doc = drafted_document(&site);
    let a = doc.evaluate(&prefixed_url, None).expect("prefixed");
    let b = doc.evaluate(&bare_url, None).expect("bare");
    assert_eq!(
        a.artifact_id,
        "wiki.p:wiki.example/0123456789abcdef0123456789abcdef"
    );
    assert_eq!(a.artifact_id, b.artifact_id);
}

#[test]
fn notion_workspace_pages_draft_behind_the_scoped_workspace() {
    let (prefixed_url, prefixed) = fixture_row("Notion title-prefixed workspace page");
    let (bare_url, bare) = fixture_row("Notion bare workspace page");
    let site = draft(NOTION, &empty(), &[&prefixed, &bare], &[]);
    assert!(site.dropped.is_empty(), "{site:?}");
    // One `hex(32)@tail` rule behind the workspace tenant, which the review
    // scopes and which is then the rule's anchor (adapter-templates §4.1,
    // §6). `www` does not name the kind.
    let [adapter] = &site.adapters[..] else {
        panic!("one adapter: {site:?}")
    };
    assert_eq!(kind(adapter), "notion.page");
    assert_eq!(
        rules(adapter),
        ["/{workspace:name|lower}/{hex:hex(32)@tail}"]
    );
    assert_eq!(
        adapter.scope,
        ScopeState::Pending {
            capture: "workspace".into()
        }
    );
    assert_eq!(adapter.tenant, Some((0, "workspace".to_owned())));
    let report = check_draft_rules(
        &empty(),
        &adapter.adapter,
        &[Provenance::Drafted],
        &policy(),
        &[],
    )
    .expect("judged per rule");
    assert_eq!(report.rules, [RuleVerdict::Kept]);
    assert!(report.scope_anchored, "unscoped would leave no anchor");
    // Both forms are one page under one ID, scoped to the workspace.
    let doc = drafted_document(&site);
    let a = doc.evaluate(&prefixed_url, None).expect("prefixed matches");
    let b = doc.evaluate(&bare_url, None).expect("bare matches");
    assert_eq!(a.artifact_id, b.artifact_id);
    // Scoped to the workspace, another workspace is not recorded. A page
    // with no workspace segment has its ID where the tenant goes, so it is
    // still not drafted (adapter-templates §13).
    let Value::Table(mut scoped) = adapter.adapter.clone() else {
        panic!("adapter table")
    };
    scoped.push(("scope".into(), value(json!({"workspace": ["acme"]}))));
    let scoped = document(vec![Value::Table(scoped)]);
    assert!(scoped.evaluate(&prefixed_url, None).is_ok());
    let other = prefixed_url.replacen("/acme/", "/elsewhere/", 1);
    assert!(scoped.evaluate(&other, None).is_err(), "{other}");
    let rootless = draft(NOTION, &empty(), &["/{hex32}"], &[]);
    assert!(rootless.adapters.is_empty(), "{rootless:?}");
    assert_eq!(drops(&rootless), [("/{hex32}", DropReason::NoIdentifier)]);
}

#[test]
fn sharepoint_sourcedoc_uuid_not_15_is_the_id() {
    let mut failures = Vec::new();
    for note in [
        "SharePoint site document (docs/adapters/sharepoint.doc.md)",
        "SharePoint personal document (docs/adapters/sharepoint.doc.md)",
        "SharePoint sharing form (docs/adapters/sharepoint.doc.md)",
        "SharePoint braced sourcedoc",
        "SharePoint bare sourcedoc",
    ] {
        let (url, shape) = fixture_row(note);
        let origin = origin_of(&url);
        let site = draft(&origin, &empty(), &[&shape], &[]);
        assert_eq!(site.adapters.len(), 1, "{note}: {site:?}");
        let adapter = &site.adapters[0];
        let id = id_template(adapter);
        assert!(id.ends_with(":{host}/{uuid}"), "{note}: {id}");
        for rule in rules(adapter) {
            assert!(!rule.contains("{num"), "{note}: {rule} captures 15");
            assert!(rule.contains("sourcedoc={uuid:uuid}"), "{note}: {rule}");
        }
        let host = url::Url::parse(&url)
            .unwrap()
            .host_str()
            .unwrap()
            .to_owned();
        match drafted_document(&site).evaluate(&url, None) {
            Ok(m) => {
                let expected = format!(
                    "{}:{host}/0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
                    kind(adapter)
                );
                if m.artifact_id != expected {
                    failures.push(format!("{note}: {} != {expected}", m.artifact_id));
                }
                if m.artifact_id.contains("/15") {
                    failures.push(format!("{note}: {}", m.artifact_id));
                }
            }
            Err(e) => failures.push(format!("{note}: {url} not matched: {e:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn google_docs_account_index_never_enters_an_id() {
    let (plain_url, plain) = fixture_row("Google Docs document");
    let (indexed_url, indexed) = fixture_row("Google Docs document under an account index");
    let origin = origin_of(&plain_url);
    let site = draft(&origin, &empty(), &[&plain, &indexed], &[]);
    assert!(!site.adapters.is_empty(), "{site:?}");
    let doc = drafted_document(&site);
    let a = doc.evaluate(&plain_url, None).expect("plain");
    let b = doc.evaluate(&indexed_url, None).expect("indexed");
    for m in [&a, &b] {
        assert_eq!(
            m.captures.iter().filter(|(_, v)| v == "0").count(),
            0,
            "{m:?}"
        );
        assert!(
            m.artifact_id.ends_with(":docs.google.com/1aBcD23eFgH"),
            "{m:?}"
        );
    }
    for adapter in &site.adapters {
        assert_eq!(adapter.scope, ScopeState::UnscopedOnly);
        for rule in rules(adapter) {
            if rule.contains("/u/") {
                assert!(rule.contains("/u/{_}/"), "{rule}");
            }
        }
        let id = id_template(adapter);
        assert!(id.ends_with(":{host}/{token}"), "{id}");
    }
    // The account index is anonymous, so other indexes match too, with the
    // same ID.
    let other = doc
        .evaluate(
            "https://docs.google.com/document/u/3/d/1aBcD23eFgH/edit",
            None,
        )
        .expect("other account");
    assert_eq!(other.artifact_id, b.artifact_id);
}

/// Every rule a link-only shape adds is flagged link-only, and the visited
/// rule keeps its exact form.
fn assert_visited_rule_unchanged(site: &SiteDraft, visited_rule: &str) {
    let mut found = false;
    for adapter in &site.adapters {
        for (text, rule) in rules(adapter).iter().zip(&adapter.rules) {
            if text == visited_rule {
                assert!(!rule.link_only, "{text}");
                found = true;
            } else {
                assert!(rule.link_only, "{text} is backed only by planted links");
                assert_eq!(rule.visited, 0, "{text}");
            }
        }
    }
    assert!(found, "visited rule {visited_rule} kept: {site:?}");
}

#[test]
fn planted_links_toward_rest_after_first_segment_are_dropped() {
    let planted = [
        "/{slug}",
        "/{slug}/{slug}",
        "/{slug}/{slug}/{slug}",
        "/docs/{slug}/{slug}",
        "/docs/{slug}/{slug}/{slug}",
        "/{num}/{slug}/{slug}",
        "/{key}/{slug}/{slug}/{slug}",
    ];
    // A trailing run after two segments may fold, but only into a rule of
    // its own that starts unchecked.
    let fold = "/docs/{num}/{slug}/{slug}";
    let links: Vec<&str> = planted.iter().copied().chain([fold]).collect();
    let site = draft(TOOLS, &empty(), &["/docs/{num}"], &links);
    assert_visited_rule_unchanged(&site, "/docs/{num:int}");
    assert_eq!(all_rules(&site), ["/docs/{num:int}", "/docs/{num:int}/{*}"]);
    for rule in all_rules(&site) {
        let segments: Vec<&str> = rule.split('?').next().unwrap().split('/').collect();
        assert!(
            !segments.iter().take(3).any(|s| *s == "{*}"),
            "{rule} rests right after the first segment"
        );
    }
    for shape in planted {
        assert!(
            reason_of(&site, shape).is_some(),
            "{shape} is dropped: {:?}",
            drops(&site)
        );
    }
    let doc = drafted_document(&site);
    for url in [
        "https://tools.example.com/anything",
        "https://tools.example.com/anything/else",
        "https://tools.example.com/docs/a/b/c",
        "https://tools.example.com/42/a/b",
    ] {
        assert!(doc.evaluate(url, None).is_err(), "{url}");
    }
}

#[test]
fn planted_root_match_is_dropped() {
    let planted = ["/", "/?id={num}", "//{num}", "/{num}"];
    let site = draft(TOOLS, &empty(), &["/docs/{num}"], &planted);
    assert_visited_rule_unchanged(&site, "/docs/{num:int}");
    assert_eq!(reason_of(&site, "/"), Some(DropReason::Root));
    for shape in planted {
        assert!(reason_of(&site, shape).is_some(), "{shape}: {site:?}");
    }
    let doc = drafted_document(&site);
    for url in [
        "https://tools.example.com/",
        "https://tools.example.com/?id=42",
        "https://tools.example.com/42",
    ] {
        assert!(doc.evaluate(url, None).is_err(), "{url}");
    }
}

#[test]
fn planted_search_page_is_dropped() {
    let planted = [
        "/search?q={slug}",
        "/?q={slug}",
        "/search/{num}",
        "/find?q={num}",
        "/docs?query={id}",
        "/query/{key}",
        "/filter/{uuid}",
    ];
    let site = draft(TOOLS, &empty(), &["/docs/{num}"], &planted);
    assert_visited_rule_unchanged(&site, "/docs/{num:int}");
    for shape in planted {
        let reason = reason_of(&site, shape);
        assert!(
            matches!(
                reason,
                Some(
                    DropReason::Probe
                        | DropReason::SearchOrPaging
                        | DropReason::BlockedRoute
                        | DropReason::NoRouteWord
                )
            ),
            "{shape}: {reason:?}"
        );
    }
    assert_eq!(
        reason_of(&site, "/search/{num}"),
        Some(DropReason::BlockedRoute)
    );
    assert_eq!(
        reason_of(&site, "/find?q={num}"),
        Some(DropReason::SearchOrPaging)
    );
    let doc = drafted_document(&site);
    for url in [
        "https://tools.example.com/search?q=x",
        "https://tools.example.com/?q=x",
        "https://tools.example.com/search/42",
        "https://tools.example.com/find?q=42",
    ] {
        assert!(doc.evaluate(url, None).is_err(), "{url}");
    }
}

#[test]
fn planted_secret_query_parameter_id_is_dropped() {
    // A signed or shared link's token is never an ID: the pair is dropped,
    // and the shape too when it has no other identifier.
    let planted = [
        "/files/{num}?token={id}",
        "/doc/{num}?access_token={hex64}",
        "/view?sig={hex32}",
        "/cb?code={id}",
    ];
    let site = draft(TOOLS, &empty(), &["/docs/{num}"], &planted);
    assert_visited_rule_unchanged(&site, "/docs/{num:int}");
    for shape in ["/view?sig={hex32}", "/cb?code={id}"] {
        assert_eq!(
            reason_of(&site, shape),
            Some(DropReason::SecretQuery),
            "{shape}: {:?}",
            drops(&site)
        );
    }
    for rule in all_rules(&site) {
        for key in ["token", "access_token", "sig", "code"] {
            assert!(!rule.contains(&format!("{key}=")), "{rule}");
        }
    }
    for adapter in &site.adapters {
        let id = adapter.adapter.get("id").and_then(Value::as_str).unwrap();
        assert!(!id.contains("{hex}") && !id.contains("{token}"), "{id}");
    }
}

#[test]
fn planted_auth_or_capability_route_is_dropped() {
    let planted = [
        "/invite/{hex32}",
        "/reset/{uuid}",
        "/password/reset/{hex64}",
        "/verify/{id}",
        "/confirm/{num}",
        "/share/{key}",
        "/magic/{hex40}",
        "/token/{id}",
        "/oauth/{id}",
        "/auth/{id}",
        "/signin/{num}",
        "/signup/{num}",
        "/logout/{num}",
        "/unsubscribe/{uuid}",
        "/docs/{num}/share/{id}",
        "/docs/invite/{uuid}",
        "/invitations/{hex32}",
        "/accept-invite/{hex32}",
        "/reset-password/{uuid}",
        "/sign-in/{id}",
        "/magic-link/{id}",
        "/tokens/{num}",
        "/auth/callback/{id}",
        "/sso/{id}",
    ];
    let site = draft(TOOLS, &empty(), &["/docs/{num}"], &planted);
    assert_visited_rule_unchanged(&site, "/docs/{num:int}");
    for shape in planted {
        assert_eq!(
            reason_of(&site, shape),
            Some(DropReason::BlockedRoute),
            "{shape}: {:?}",
            drops(&site)
        );
    }
    let blocked = policy().blocked_route_words;
    for rule in all_rules(&site) {
        for word in &blocked {
            assert!(
                !rule
                    .split(['/', '?', '&', '(', ')', '|'])
                    .any(|s| s == word),
                "{rule}"
            );
        }
    }
}

#[test]
fn planted_search_or_paging_parameter_id_is_dropped() {
    let planted = [
        "/items?page={num}",
        "/items?cursor={id}",
        "/items?offset={num}&sort={slug}",
        "/items?q={key}",
        "/items?jql={slug}&start={num}",
        "/items?tab={num}&view={uuid}",
    ];
    let site = draft(
        TOOLS,
        &empty(),
        &["/docs/{num}", "/docs/{num}?page={num}"],
        &planted,
    );
    for shape in planted {
        assert_eq!(
            reason_of(&site, shape),
            Some(DropReason::SearchOrPaging),
            "{shape}: {:?}",
            drops(&site)
        );
    }
    // Where the shape has another identifier only the paging pair goes, so
    // the paged form is the same rule as the plain one.
    assert_eq!(all_rules(&site), ["/docs/{num:int}"]);
    let p = policy();
    for rule in all_rules(&site) {
        for key in p.search_keys.iter().chain(&p.paging_keys) {
            assert!(!rule.contains(&format!("?{key}=")), "{rule}");
            assert!(!rule.contains(&format!("&{key}=")), "{rule}");
        }
    }
    let doc = drafted_document(&site);
    assert!(
        doc.evaluate("https://tools.example.com/items?page=2", None)
            .is_err()
    );
}

#[test]
fn planted_id_less_rule_is_dropped() {
    let planted = [
        "/about/team",
        "/pages/{slug}",
        "/docs/{slug}/{slug}/{slug}",
        "/docs/list?owner={slug}",
        "/docs?{param}={num}",
    ];
    let site = draft(TOOLS, &empty(), &["/docs/{num}"], &planted);
    assert_visited_rule_unchanged(&site, "/docs/{num:int}");
    for shape in planted {
        let reason = reason_of(&site, shape);
        assert!(
            matches!(
                reason,
                Some(DropReason::NoIdentifier | DropReason::ParamName)
            ),
            "{shape}: {reason:?}"
        );
    }
    assert_eq!(
        reason_of(&site, "/about/team"),
        Some(DropReason::NoIdentifier)
    );
    assert_eq!(
        reason_of(&site, "/docs?{param}={num}"),
        Some(DropReason::ParamName)
    );
    for adapter in &site.adapters {
        assert!(
            id_template(adapter).contains("{num}"),
            "{}",
            id_template(adapter)
        );
    }
}

#[test]
fn planted_alternative_on_visited_rule_is_a_separate_unchecked_rule() {
    let site = draft(
        TOOLS,
        &empty(),
        &["/wiki/doc/{num}"],
        &["/wiki/page/{num}", "/wiki/doc/{num}/{slug}/{slug}"],
    );
    // The planted route word is a namespace of its own: a link-only kind,
    // whose IDs never meet the visited pages'.
    assert_eq!(site.adapters.len(), 2, "{site:?}");
    let planted = &site.adapters[1];
    assert_eq!(rules(planted), ["/wiki/page/{num:int}"]);
    assert!(planted.rules[0].link_only);
    assert_ne!(id_template(planted), id_template(&site.adapters[0]));
    let adapter = &site.adapters[0];
    let texts = rules(adapter);
    assert_eq!(texts[0], "/wiki/doc/{num:int}", "visited rule ranks first");
    assert!(!adapter.rules[0].link_only);
    assert_eq!(adapter.rules[0].visited, 1);
    assert!(texts.len() > 1, "{texts:?}");
    for (text, rule) in texts.iter().zip(&adapter.rules).skip(1) {
        assert!(rule.link_only, "{text} starts unchecked");
        assert_eq!(rule.visited, 0);
        assert!(rule.links > 0);
        assert!(!text.starts_with("/wiki/doc/{num:int}") || text.len() > texts[0].len());
    }
    // The planted alternative is never an alternation on the visited rule.
    assert!(
        texts
            .iter()
            .all(|t| !t.contains("(doc|page)") && !t.contains("(page|doc)"))
    );
    let doc = drafted_document(&site);
    let visited = doc
        .evaluate("https://tools.example.com/wiki/doc/42", None)
        .expect("visited");
    assert_eq!(visited.rule, 0);
}

#[test]
fn token_only_rule_starts_unchecked() {
    let site = draft(TOOLS, &empty(), &["/d/{id}", "/tickets/{num}"], &[]);
    let flags: BTreeMap<String, bool> = site
        .adapters
        .iter()
        .flat_map(|a| {
            rules(a)
                .into_iter()
                .zip(a.rules.iter().map(|r| r.token_only))
        })
        .collect();
    assert_eq!(
        flags,
        BTreeMap::from([
            ("/d/{token:token(1,128)}".to_owned(), true),
            ("/tickets/{num:int}".to_owned(), false),
        ])
    );
    for adapter in &site.adapters {
        assert!(adapter.rules.iter().all(|r| !r.link_only));
    }
}

#[test]
fn draft_overlapping_existing_adapter_loses_only_overlapping_rules() {
    let existing = document(vec![value(json!({
        "type": "tools.history",
        "origin": TOOLS,
        "match": [{"path": "/docs/{n:int}/history"}],
        "id": "tools.history:{host}/{n}",
    }))]);
    let site = draft(
        TOOLS,
        &existing,
        &[
            "/docs/{num}",
            "/docs/{num}/{slug}/{slug}",
            "/docs/{num}/history",
            "/tickets/{key}",
        ],
        &[],
    );
    assert_eq!(
        reason_of(&site, "/docs/{num}/history"),
        Some(DropReason::AlreadyRecognized)
    );
    assert_eq!(
        reason_of(&site, "/docs/{num}/{slug}/{slug}"),
        Some(DropReason::Check(DraftErrorCode::Overlap)),
        "{:?}",
        drops(&site)
    );
    let mut kept = all_rules(&site);
    kept.sort();
    assert_eq!(kept, ["/docs/{num:int}", "/tickets/{key:jira_key}"]);
    // The existing adapter is untouched and still owns its pages.
    assert!(site.adapters.iter().all(|a| kind(a) != "tools.history"));
    let mut both: Vec<Value> = vec![value(json!({
        "type": "tools.history",
        "origin": TOOLS,
        "match": [{"path": "/docs/{n:int}/history"}],
        "id": "tools.history:{host}/{n}",
    }))];
    both.extend(site.adapters.iter().map(evaluable));
    let doc = document(both);
    let history = doc
        .evaluate("https://tools.example.com/docs/42/history", None)
        .expect("history");
    assert_eq!(history.artifact_type, "tools.history");
    assert!(doc.overlap_report().is_empty());
}

/// The fuzz target's invariant (`fuzz/fuzz_targets/structural_draft.rs`) on
/// a fixed corpus: every recorded shape, visited and then as links, on a
/// plain and a shared origin.
#[test]
fn drafted_rules_pass_the_draft_checks_again_and_never_match_the_root() {
    let corpus: Vec<String> = fixture_rows()
        .into_iter()
        .map(|(_, _, shape)| shape)
        .chain(
            [
                "/docs/{num}",
                "/docs/{num}/{slug}/{slug}",
                "/wiki/page/{num}",
                "/d/{id}",
                "/?id={num}",
                "/search/{num}",
                "/items?page={num}",
            ]
            .map(str::to_owned),
        )
        .collect();
    let corpus: Vec<&str> = corpus.iter().map(String::as_str).collect();
    let policy = policy();
    let mut drafted = 0;
    for origin in [TOOLS, GITHUB] {
        for (visited, links) in [(&corpus[..], &[][..]), (&corpus[..1], &corpus[..])] {
            let site = draft(origin, &empty(), visited, links);
            let mut accepted = Vec::new();
            for adapter in &site.adapters {
                drafted += adapter.rules.len();
                let provenance = vec![Provenance::Drafted; adapter.rules.len()];
                let report =
                    check_draft_rules(&empty(), &adapter.adapter, &provenance, &policy, &accepted)
                        .unwrap_or_else(|e| panic!("{origin}: {} refused: {e:?}", kind(adapter)));
                assert!(
                    report.rules.iter().all(|v| *v == RuleVerdict::Kept),
                    "{origin}: {:?} {:?}",
                    rules(adapter),
                    report.rules
                );
                accepted.extend(report.document);
                let doc = document(vec![evaluable(adapter)]);
                assert!(doc.evaluate(&format!("{origin}/"), None).is_err());
            }
        }
    }
    assert!(drafted > 10, "the corpus drafts rules: {drafted}");
}

/// Route words that differ before the kind's word are namespaces: equal
/// numbers under two of them are two pages, so the ID binds the namespace
/// when another route word anchors the rule, and the kind splits per word
/// when that word is the only anchor.
#[test]
fn equal_numbers_under_two_route_words_get_different_ids() {
    let urls = [
        "https://tools.example.com/foo/discussions/7",
        "https://tools.example.com/bar/discussions/7",
    ];
    let site = draft(
        TOOLS,
        &empty(),
        &["/foo/discussions/{num}", "/bar/discussions/{num}"],
        &[],
    );
    assert!(site.dropped.is_empty(), "{site:?}");
    let [adapter] = &site.adapters[..] else {
        panic!("one adapter covers both namespaces: {site:?}")
    };
    assert_eq!(kind(adapter), "tools.discussions");
    assert_eq!(
        id_template(adapter),
        "tools.discussions:{host}/{space}/{num}"
    );
    let doc = drafted_document(&site);
    let ids: Vec<String> = urls
        .iter()
        .map(|url| doc.evaluate(url, None).expect("matched").artifact_id)
        .collect();
    assert_eq!(
        ids,
        [
            "tools.discussions:tools.example.com/foo/7",
            "tools.discussions:tools.example.com/bar/7"
        ]
    );

    // After a literal the words alternate, and the ID still binds them.
    let site = draft(
        TOOLS,
        &empty(),
        &["/wiki/pages/{num}", "/wiki/blog/{num}"],
        &[],
    );
    let doc = drafted_document(&site);
    let a = doc.evaluate("https://tools.example.com/wiki/pages/7", None);
    let b = doc.evaluate("https://tools.example.com/wiki/blog/7", None);
    assert_ne!(a.expect("pages").artifact_id, b.expect("blog").artifact_id);

    // With no other route word, each word is its own kind.
    let site = draft(TOOLS, &empty(), &["/pages/{num}", "/blog/{num}"], &[]);
    let mut types: Vec<&str> = site.adapters.iter().map(kind).collect();
    types.sort_unstable();
    assert_eq!(types, ["tools.blog", "tools.pages"], "{site:?}");
    let doc = drafted_document(&site);
    let a = doc.evaluate("https://tools.example.com/pages/7", None);
    let b = doc.evaluate("https://tools.example.com/blog/7", None);
    assert_ne!(a.expect("pages").artifact_id, b.expect("blog").artifact_id);

    // A link under another namespace never shares the visited pages' IDs,
    // and never changes their ID scheme.
    let alone = draft(TOOLS, &empty(), &["/foo/discussions/{num}"], &[]);
    let planted = draft(
        TOOLS,
        &empty(),
        &["/foo/discussions/{num}"],
        &["/bar/discussions/{num}"],
    );
    assert_eq!(
        id_template(&alone.adapters[0]),
        "tools.discussions:{host}/{num}"
    );
    assert_eq!(
        id_template(&planted.adapters[0]),
        id_template(&alone.adapters[0])
    );
    let doc = drafted_document(&planted);
    let a = doc.evaluate(urls[0], None).expect("visited");
    let b = doc.evaluate(urls[1], None).expect("link");
    assert_ne!(a.artifact_id, b.artifact_id);
}

/// One alternation holds at most 16 words, so more namespaces than that
/// never bind into one invalid rule: each stays a kind of its own.
#[test]
fn more_namespaces_than_one_alternation_holds_split_per_word() {
    let shapes: Vec<String> = (b'a'..=b'q')
        .map(|c| format!("/w{}/discussions/{{num}}", char::from(c)))
        .collect();
    let visited: Vec<&str> = shapes.iter().map(String::as_str).collect();
    let site = draft(TOOLS, &empty(), &visited, &[]);
    assert!(
        site.dropped.iter().all(|d| d.reason == DropReason::Limit),
        "{:?}",
        site.dropped
    );
    assert!(!site.adapters.is_empty());
    for adapter in &site.adapters {
        assert!(
            !id_template(adapter).contains("{space}"),
            "{}",
            id_template(adapter)
        );
    }
}

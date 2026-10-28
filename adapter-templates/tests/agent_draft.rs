//! Step 7.7 agent drafter cases (DESIGN §5.1, §12.2 step 7.7) at the crate
//! layer. Each case feeds the planted input of the mirrored step 7.6 case
//! in `tests/structural.rs` and an agent answer that follows the plant into
//! [`check_agent_draft`], and asserts the code each planted rule is dropped
//! with. The answers are the shared fixtures in
//! `host/tests/fixtures/site-draft-answers/`, which the service and host
//! layers replay too. An agent rule never goes through the structural Keep
//! step: it is judged by `check_draft_rules`, so its outcome is a
//! [`DraftErrorCode`], and `rule_check`'s order decides which one.

use adapter_templates::structural::{
    DraftedAdapter, DroppedRule, ShapeInput, SiteDraft, check_agent_draft,
};
use adapter_templates::{Document, DraftErrorCode, Value};
use serde_json::{Value as Json, json};

mod common;

use common::{CATALOG, TOOLS, document, drafted_document, empty, policy, rules, shapes, value};

macro_rules! fixture {
    ($case:literal) => {
        include_str!(concat!(
            "../../host/tests/fixtures/site-draft-answers/",
            $case,
            ".json"
        ))
    };
}

/// The answer's tables, as the service hands them on.
fn answer(text: &str) -> Vec<Value> {
    let answer: Json = serde_json::from_str(text).expect("fixture answer parses");
    let keys: Vec<&String> = answer.as_object().expect("answer object").keys().collect();
    assert_eq!(keys, ["adapters"], "an answer's only key is adapters");
    answer["adapters"]
        .as_array()
        .expect("adapters")
        .iter()
        .cloned()
        .map(value)
        .collect()
}

fn input(visited: &[&str], links: &[&str]) -> ShapeInput {
    ShapeInput {
        visited: shapes(visited),
        links: shapes(links),
    }
}

fn agent(existing: &Document, input: &ShapeInput, tables: &[Value]) -> SiteDraft {
    check_agent_draft(TOOLS, existing, &[], input, &policy(), &[], tables)
}

/// A rule as `path` or `path?key=template&...`.
fn rule_text(rule: &Value) -> String {
    let mut text = rule
        .get("path")
        .and_then(Value::as_str)
        .expect("path")
        .to_owned();
    for (index, (key, template)) in rule
        .get("query")
        .and_then(Value::as_table)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        text.push(if index == 0 { '?' } else { '&' });
        text.push_str(key);
        text.push('=');
        text.push_str(template.as_str().unwrap_or("<non-string>"));
    }
    text
}

/// The agent table's rules, as text.
fn table_rules(table: &Value) -> Vec<String> {
    table
        .get("match")
        .and_then(Value::as_array)
        .expect("match")
        .iter()
        .map(rule_text)
        .collect()
}

/// Every dropped rule as `(rule text, code)`. A whole dropped table names
/// each of its rules.
fn dropped(site: &SiteDraft, tables: &[Value]) -> Vec<(String, DraftErrorCode)> {
    site.dropped_rules
        .iter()
        .flat_map(
            |DroppedRule {
                 table,
                 rule,
                 reason,
             }| {
                let texts = table_rules(&tables[*table]);
                let texts: Vec<String> = match rule {
                    Some(rule) => vec![texts[*rule].clone()],
                    None => texts,
                };
                texts.into_iter().map(move |text| (text, *reason))
            },
        )
        .collect()
}

fn all_kept(site: &SiteDraft) -> Vec<String> {
    site.adapters.iter().flat_map(rules).collect()
}

/// The kept rule with this text, and its adapter.
fn kept<'s>(site: &'s SiteDraft, text: &str) -> (&'s DraftedAdapter, usize) {
    site.adapters
        .iter()
        .find_map(|a| rules(a).iter().position(|t| t == text).map(|i| (a, i)))
        .unwrap_or_else(|| panic!("{text} kept: {site:?}"))
}

/// The visited rule is kept and checked: backed by a visited shape, and
/// neither link-only, token-only nor unseen.
fn assert_visited_rule_kept_and_checked(site: &SiteDraft, text: &str) {
    assert_eq!(site.rejected, None, "{site:?}");
    let (adapter, index) = kept(site, text);
    let rule = &adapter.rules[index];
    assert!(rule.visited > 0, "{text}: {rule:?}");
    assert!(
        !rule.link_only && !rule.token_only && !rule.unseen,
        "{text}: {rule:?}"
    );
}

/// Each expected `(rule, code)` pair is reported, and nothing else is
/// dropped.
fn assert_dropped(site: &SiteDraft, tables: &[Value], expected: &[(&str, DraftErrorCode)]) {
    let mut got = dropped(site, tables);
    let mut want: Vec<(String, DraftErrorCode)> = expected
        .iter()
        .map(|(text, code)| ((*text).to_owned(), *code))
        .collect();
    got.sort();
    want.sort();
    assert_eq!(got, want);
}

const PLANTED_REST: &[&str] = &[
    "/{slug}",
    "/{slug}/{slug}",
    "/{slug}/{slug}/{slug}",
    "/docs/{slug}/{slug}",
    "/docs/{slug}/{slug}/{slug}",
    "/{num}/{slug}/{slug}",
    "/{key}/{slug}/{slug}/{slug}",
    "/docs/{num}/{slug}/{slug}",
];

#[test]
fn agent_planted_links_toward_rest_after_first_segment_are_dropped() {
    let tables = answer(fixture!(
        "agent_planted_links_toward_rest_after_first_segment_are_dropped"
    ));
    let site = agent(&empty(), &input(&["/docs/{num}"], PLANTED_REST), &tables);
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    // The case map's bare `/{*}` has no literal at all, which the template
    // grammar refuses (`no_literal_segment`, so `draft_invalid_rule`); the
    // fixture's rest rule keeps a literal, after its identifier.
    assert_dropped(
        &site,
        &tables,
        &[
            ("/{n:int}/page/{*}", DraftErrorCode::NoLeadingLiteral),
            ("/docs/{*}", DraftErrorCode::RestSecond),
        ],
    );
    assert_eq!(all_kept(&site), ["/docs/{n:int}"]);
    let doc = drafted_document(&site);
    for url in [
        "https://tools.example.com/anything",
        "https://tools.example.com/docs/a/b/c",
        "https://tools.example.com/42/a/b",
    ] {
        assert!(doc.evaluate(url, None).is_err(), "{url}");
    }
}

#[test]
fn agent_planted_root_match_is_dropped() {
    let tables = answer(fixture!("agent_planted_root_match_is_dropped"));
    let site = agent(
        &empty(),
        &input(&["/docs/{num}"], &["/", "/?id={num}", "//{num}", "/{num}"]),
        &tables,
    );
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    // The case map's `/`, `/{n:int}` and `/?id={n:int}` have no literal at
    // all, which the template grammar refuses before any draft check (they
    // are dropped as `draft_invalid_rule`, below). The fixture's rules start
    // at the root with an identifier and keep a literal after it.
    assert_dropped(
        &site,
        &tables,
        &[
            ("/{n:int}/index", DraftErrorCode::NoLeadingLiteral),
            ("/{n:int}/{_}/view", DraftErrorCode::NoLeadingLiteral),
            ("/{m:int}/view?id={n:int}", DraftErrorCode::NoLeadingLiteral),
        ],
    );
    let bare = [value(json!({
        "type": "tools.bare",
        "origin": TOOLS,
        "match": [
            {"path": "/"},
            {"path": "/{n:int}"},
            {"path": "/", "query": {"id": "{n:int}"}},
            {"path": "/bare/{n:int}"},
        ],
        "id": "tools.bare:{host}/{n}",
    }))];
    let literal_less = agent(&empty(), &input(&["/docs/{num}"], &[]), &bare);
    assert_dropped(
        &literal_less,
        &bare,
        &[
            ("/", DraftErrorCode::InvalidRule),
            ("/{n:int}", DraftErrorCode::InvalidRule),
            ("/?id={n:int}", DraftErrorCode::InvalidRule),
        ],
    );
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
fn agent_planted_search_page_is_dropped() {
    let tables = answer(fixture!("agent_planted_search_page_is_dropped"));
    let planted = [
        "/search?q={slug}",
        "/?q={slug}",
        "/search/{num}",
        "/find?q={num}",
        "/docs?query={id}",
        "/query/{key}",
        "/filter/{uuid}",
    ];
    let site = agent(&empty(), &input(&["/docs/{num}"], &planted), &tables);
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    assert_dropped(
        &site,
        &tables,
        &[
            ("/search/{n:int}", DraftErrorCode::BlockedRoute),
            ("/find?q={n:int}", DraftErrorCode::SearchCapture),
            (
                "/docs?query={t:token(1,128)}",
                DraftErrorCode::SearchCapture,
            ),
        ],
    );
    let doc = drafted_document(&site);
    for url in [
        "https://tools.example.com/search/42",
        "https://tools.example.com/find?q=42",
        "https://tools.example.com/docs?query=abc",
    ] {
        assert!(doc.evaluate(url, None).is_err(), "{url}");
    }
}

#[test]
fn agent_planted_secret_query_parameter_id_is_dropped() {
    let tables = answer(fixture!(
        "agent_planted_secret_query_parameter_id_is_dropped"
    ));
    let planted = [
        "/files/{num}?token={id}",
        "/doc/{num}?access_token={hex64}",
        "/view?sig={hex32}",
        "/cb?code={id}",
    ];
    let site = agent(&empty(), &input(&["/docs/{num}"], &planted), &tables);
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    assert_dropped(
        &site,
        &tables,
        &[
            ("/view?sig={h:hex(32)}", DraftErrorCode::SecretCapture),
            ("/cb?code={t:token(1,128)}", DraftErrorCode::SecretCapture),
            (
                "/files/{_}?token={t:token(1,128)}",
                DraftErrorCode::SecretCapture,
            ),
        ],
    );
    // The case map's `/files/{n:int}` beside an ID built from `t` alone
    // would be an unused capture (`extra_capture`), so the fixture leaves
    // that segment anonymous.
    // No surviving ID contains a secret capture.
    for adapter in &site.adapters {
        let id = adapter.adapter.get("id").and_then(Value::as_str).unwrap();
        assert!(!id.contains("{h}") && !id.contains("{t}"), "{id}");
    }
    for rule in all_kept(&site) {
        for key in ["token", "access_token", "sig", "code"] {
            assert!(!rule.contains(&format!("{key}=")), "{rule}");
        }
    }
}

#[test]
fn agent_planted_auth_or_capability_route_is_dropped() {
    let tables = answer(fixture!(
        "agent_planted_auth_or_capability_route_is_dropped"
    ));
    let planted = [
        "/invite/{hex32}",
        "/reset/{uuid}",
        "/password/reset/{hex64}",
        "/verify/{id}",
        "/confirm/{num}",
    ];
    let site = agent(&empty(), &input(&["/docs/{num}"], &planted), &tables);
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    assert_dropped(
        &site,
        &tables,
        &[
            ("/invite/{h:hex(32)}", DraftErrorCode::BlockedRoute),
            ("/reset/{u:uuid}", DraftErrorCode::BlockedRoute),
            ("/password/reset/{h:hex(64)}", DraftErrorCode::BlockedRoute),
            ("/verify/{t:token(1,128)}", DraftErrorCode::BlockedRoute),
            ("/confirm/{n:int}", DraftErrorCode::BlockedRoute),
        ],
    );
}

#[test]
fn agent_planted_search_or_paging_parameter_id_is_dropped() {
    let tables = answer(fixture!(
        "agent_planted_search_or_paging_parameter_id_is_dropped"
    ));
    let planted = [
        "/items?page={num}",
        "/items?cursor={id}",
        "/items?offset={num}&sort={slug}",
        "/items?q={key}",
        "/items?jql={slug}&start={num}",
        "/items?tab={num}&view={uuid}",
    ];
    let site = agent(
        &empty(),
        &input(&["/docs/{num}", "/docs/{num}?page={num}"], &planted),
        &tables,
    );
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    assert_dropped(
        &site,
        &tables,
        &[
            ("/items?page={n:int}", DraftErrorCode::PagingCapture),
            (
                "/items?cursor={t:token(1,128)}",
                DraftErrorCode::PagingCapture,
            ),
            ("/items?q={k:jira_key}", DraftErrorCode::SearchCapture),
            ("/items?tab={n:int}", DraftErrorCode::PagingCapture),
        ],
    );
    assert_eq!(all_kept(&site), ["/docs/{n:int}"]);
}

#[test]
fn agent_planted_id_less_rule_is_dropped() {
    let tables = answer(fixture!("agent_planted_id_less_rule_is_dropped"));
    let planted = [
        "/about/team",
        "/pages/{slug}",
        "/docs/{slug}/{slug}/{slug}",
        "/docs/list?owner={slug}",
        "/docs?{param}={num}",
    ];
    let site = agent(&empty(), &input(&["/docs/{num}"], &planted), &tables);
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    // Step 5 of `check_draft_rules` is table-level for drafted rules, so
    // each ID-less table is dropped whole.
    assert_eq!(
        site.dropped_rules,
        [
            DroppedRule {
                table: 1,
                rule: None,
                reason: DraftErrorCode::IdNeedsTypedCapture,
            },
            DroppedRule {
                table: 2,
                rule: None,
                reason: DraftErrorCode::IdNeedsTypedCapture,
            },
        ]
    );
    assert_eq!(all_kept(&site), ["/docs/{n:int}"]);
}

#[test]
fn agent_planted_alternative_on_visited_rule_starts_unchecked_or_is_dropped() {
    let tables = answer(fixture!(
        "agent_planted_alternative_on_visited_rule_starts_unchecked_or_is_dropped"
    ));
    let site = agent(
        &empty(),
        &input(
            &["/wiki/doc/{num}"],
            &["/wiki/page/{num}", "/wiki/doc/{num}/{slug}/{slug}"],
        ),
        &tables,
    );
    assert_visited_rule_kept_and_checked(&site, "/wiki/doc/{n:int}");
    // The widened rule (`(doc|page)`; the case map's braced `{(doc|page)}`
    // is not template syntax) may overlap the first table, so it is dropped
    // as `draft_overlap`, never offered beside it.
    assert_dropped(
        &site,
        &tables,
        &[("/wiki/(doc|page)/{n:int}", DraftErrorCode::Overlap)],
    );
    // Outcome-based: no checked rule evaluates the planted link-only page,
    // and the visited page still evaluates to the checked first table.
    let checked: Vec<Value> = site
        .adapters
        .iter()
        .filter_map(|a| {
            let keep: Vec<Value> = a
                .adapter
                .get("match")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .zip(&a.rules)
                .filter(|(_, r)| !r.link_only && !r.token_only && !r.unseen)
                .map(|(rule, _)| rule.clone())
                .collect();
            let Value::Table(entries) = &a.adapter else {
                return None;
            };
            (!keep.is_empty()).then(|| {
                Value::Table(
                    entries
                        .iter()
                        .map(|(k, v)| match k.as_str() {
                            "match" => (k.clone(), Value::Array(keep.clone())),
                            _ => (k.clone(), v.clone()),
                        })
                        .collect(),
                )
            })
        })
        .collect();
    let doc = document(checked);
    assert!(
        doc.evaluate("https://tools.example.com/wiki/page/42", None)
            .is_err()
    );
    let visited = doc
        .evaluate("https://tools.example.com/wiki/doc/42", None)
        .expect("visited");
    assert_eq!(visited.artifact_type, "tools.doc");
}

#[test]
fn agent_planted_blocked_route_word_is_dropped() {
    let tables = answer(fixture!("agent_planted_blocked_route_word_is_dropped"));
    // The expected words come from the catalog, never a copy.
    let catalog: Json = serde_json::from_str(CATALOG).unwrap();
    let words: Vec<String> = catalog["blocked_route_words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_owned())
        .collect();
    let mut expected: Vec<String> = words.iter().map(|w| format!("/{w}/{{n:int}}")).collect();
    // A plain alternation is written `(docs|share)`; the case map's braced
    // `{(docs|share)}` is not template syntax.
    expected.push("/tools/(docs|share)/{n:int}".to_owned());
    let mut planted: Vec<String> = tables[1..].iter().flat_map(table_rules).collect();
    planted.sort();
    expected.sort();
    assert_eq!(planted, expected, "the fixture plants every catalog word");

    let site = agent(
        &empty(),
        &input(&["/docs/{num}"], &["/invite/{hex32}", "/share/{key}"]),
        &tables,
    );
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    let want: Vec<(&str, DraftErrorCode)> = expected
        .iter()
        .map(|r| (r.as_str(), DraftErrorCode::BlockedRoute))
        .collect();
    assert_dropped(&site, &tables, &want);
}

#[test]
fn agent_draft_overlapping_existing_adapter_loses_only_overlapping_rules() {
    let tables = answer(fixture!(
        "agent_draft_overlapping_existing_adapter_loses_only_overlapping_rules"
    ));
    let history = value(json!({
        "type": "tools.history",
        "origin": TOOLS,
        "match": [{"path": "/docs/{n:int}/history"}],
        "id": "tools.history:{host}/{n}",
    }));
    let existing = document(vec![history.clone()]);
    let site = agent(
        &existing,
        &input(
            &[
                "/docs/{num}",
                "/docs/{num}/{slug}/{slug}",
                "/docs/{num}/history",
                "/tickets/{key}",
            ],
            &[],
        ),
        &tables,
    );
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    assert_dropped(
        &site,
        &tables,
        &[("/docs/{n:int}/history", DraftErrorCode::Overlap)],
    );
    let mut kept = all_kept(&site);
    kept.sort();
    assert_eq!(kept, ["/docs/{n:int}", "/tickets/{k:jira_key}"]);
    let mut both = vec![history];
    both.extend(site.adapters.iter().map(|a| a.adapter.clone()));
    let doc = document(both);
    let page = doc
        .evaluate("https://tools.example.com/docs/42/history", None)
        .expect("history");
    assert_eq!(page.artifact_type, "tools.history");
}

/// A whole-draft refusal: no adapters, no per-rule drops, only the code.
fn assert_rejected(site: &SiteDraft, code: DraftErrorCode) {
    assert_eq!(site.rejected, Some(code), "{site:?}");
    assert!(site.adapters.is_empty(), "{site:?}");
    assert!(site.dropped_rules.is_empty(), "{site:?}");
}

/// Each forbidden table refuses the draft alone, beside the clean visited
/// table, and so does the whole answer.
fn assert_each_table_rejected(tables: &[Value], code: DraftErrorCode) {
    let input = input(&["/docs/{num}"], &[]);
    assert_rejected(&agent(&empty(), &input, tables), code);
    for table in &tables[1..] {
        let pair = [tables[0].clone(), table.clone()];
        assert_rejected(&agent(&empty(), &input, &pair), code);
        let reversed = [table.clone(), tables[0].clone()];
        assert_rejected(&agent(&empty(), &input, &reversed), code);
    }
    // Without the forbidden tables the same answer is accepted.
    let clean = agent(&empty(), &input, &tables[..1]);
    assert_visited_rule_kept_and_checked(&clean, "/docs/{n:int}");
}

#[test]
fn agent_draft_with_tier1_fields_is_rejected_before_the_preview() {
    let tables = answer(fixture!(
        "agent_draft_with_tier1_fields_is_rejected_before_the_preview"
    ));
    for (table, key) in tables[1..].iter().zip(["fields", "alias", "path_case"]) {
        assert!(table.get(key).is_some(), "the fixture plants {key}");
    }
    assert_each_table_rejected(&tables, DraftErrorCode::ForbiddenKey);
}

#[test]
fn agent_draft_with_a_title_template_is_rejected_before_the_preview() {
    let tables = answer(fixture!(
        "agent_draft_with_a_title_template_is_rejected_before_the_preview"
    ));
    assert!(tables[1].get("title").is_some());
    assert_each_table_rejected(&tables, DraftErrorCode::ForbiddenKey);
}

#[test]
fn agent_draft_with_a_scope_is_rejected_before_the_preview() {
    let tables = answer(fixture!(
        "agent_draft_with_a_scope_is_rejected_before_the_preview"
    ));
    assert!(tables[1].get("scope").is_some());
    assert!(tables[2].get("unscoped").is_some());
    assert_each_table_rejected(&tables, DraftErrorCode::ForbiddenKey);
}

#[test]
fn unparseable_agent_draft_is_discarded_without_being_shown() {
    // The service discards the fixture's prose answer before the host sees
    // it; here a table that does not parse as an adapter refuses the draft.
    let docs = value(json!({
        "type": "tools.docs",
        "origin": TOOLS,
        "match": [{"path": "/docs/{n:int}"}],
        "id": "tools.docs:{host}/{n}",
    }));
    let input = input(&["/docs/{num}"], &[]);
    for broken in [
        json!("/tickets/{k:jira_key}"),
        json!({"type": "tools.tickets", "origin": TOOLS, "match": "/tickets/{k:jira_key}", "id": "tools.tickets:{host}/{k}"}),
        json!({"type": "tools.tickets", "origin": TOOLS, "match": [{"path": "/tickets/{k:jira_key}"}]}),
        json!({"type": "tools.tickets", "origin": TOOLS, "match": [], "id": "tools.tickets:{host}/{k}"}),
        json!({"type": "Tools Tickets", "origin": TOOLS, "match": [{"path": "/tickets/{k:jira_key}"}], "id": "Tools Tickets:{host}/{k}"}),
    ] {
        let tables = [docs.clone(), value(broken.clone())];
        assert_rejected(
            &agent(&empty(), &input, &tables),
            DraftErrorCode::InvalidRule,
        );
    }
}

#[test]
fn agent_can_add_a_tier0_rule_the_structural_pass_missed_and_it_passes_the_same_checks() {
    use adapter_templates::structural::{DropReason, draft_site};

    let input = input(&["/docs/{num}", "/{uuid}/view/{num}"], &[]);
    let structural = draft_site(TOOLS, &empty(), &[], &input, &policy(), &[]);
    assert!(
        structural
            .dropped
            .iter()
            .any(|d| d.shape == "/{uuid}/view/{num}" && d.reason == DropReason::NoRouteWord),
        "{structural:?}"
    );
    let tables = answer(fixture!(
        "agent_can_add_a_tier0_rule_the_structural_pass_missed_and_it_passes_the_same_checks"
    ));
    let site = agent(&empty(), &input, &tables);
    assert!(site.dropped_rules.is_empty(), "{site:?}");
    assert_visited_rule_kept_and_checked(&site, "/docs/{n:int}");
    assert_visited_rule_kept_and_checked(&site, "/{_}/view/{n:int}");
    let doc = drafted_document(&site);
    let page = doc
        .evaluate(
            "https://tools.example.com/123e4567-e89b-12d3-a456-426614174000/view/7",
            None,
        )
        .expect("the missed page is captured");
    assert_eq!(page.artifact_type, "tools.view");
    assert!(doc.evaluate("https://tools.example.com/", None).is_err());
}

/// Agent tables carry only the Tier 0 keys. `within` (a container link the
/// review never shows) is a forbidden key; any other key fails the table's
/// form. Either refuses the whole draft before the preview.
#[test]
fn agent_draft_with_a_container_or_unknown_key_is_rejected_before_the_preview() {
    let clean = json!({
        "type": "tools.docs", "origin": TOOLS, "match": [{ "path": "/docs/{n:int}" }],
        "id": "tools.docs:{host}/{n}",
    });
    let mut within = clean.clone();
    within["type"] = json!("tools.page");
    within["id"] = json!("tools.page:{host}/{n}");
    within["match"] = json!([{ "path": "/page/{n:int}" }]);
    within["within"] = json!({ "type": "tools.docs", "id": "tools.docs:{host}/{n}" });
    let mut unknown = clean.clone();
    unknown["type"] = json!("tools.page");
    unknown["id"] = json!("tools.page:{host}/{n}");
    unknown["match"] = json!([{ "path": "/page/{n:int}" }]);
    unknown["priority"] = json!(1);
    let forbidden: Vec<Value> = [clean.clone(), within].into_iter().map(value).collect();
    assert_each_table_rejected(&forbidden, DraftErrorCode::ForbiddenKey);
    let malformed: Vec<Value> = [clean, unknown].into_iter().map(value).collect();
    assert_each_table_rejected(&malformed, DraftErrorCode::InvalidRule);
}

/// A taken type gets a numeric suffix that still fits the 32-character type
/// limit, and a ref to the agent's own type follows the rename.
#[test]
fn a_renamed_agent_type_fits_the_limit_and_keeps_its_self_refs() {
    let long = "tools.abcdefghijklmnopqrstuvwxyz"; // 32 characters
    assert_eq!(long.len(), 32);
    let table = value(json!({
        "type": long, "origin": TOOLS, "match": [{ "path": "/docs/{n:int}" }],
        "id": format!("{long}:{{host}}/{{n}}"),
        "refs": [{ "type": long, "find": "jira_key" }],
    }));
    let input = input(&["/docs/{num}"], &[]);
    let site = check_agent_draft(
        TOOLS,
        &empty(),
        &[],
        &input,
        &policy(),
        &[long.to_owned()],
        &[table],
    );
    assert_eq!(site.rejected, None, "{site:?}");
    let shown = &site.adapters[0].adapter;
    let renamed = shown.get("type").and_then(Value::as_str).unwrap();
    assert_ne!(renamed, long);
    assert!(renamed.len() <= 32, "{renamed}");
    let refs = shown.get("refs").and_then(Value::as_array).unwrap();
    assert_eq!(refs[0].get("type").and_then(Value::as_str), Some(renamed));
    // The draft also validates as a document.
    let _ = drafted_document(&site);
}

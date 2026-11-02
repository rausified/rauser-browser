//! Golden fixtures for the SQLite index migration (plan T2).
//!
//! Each directory under `host/tests/fixtures/index/` holds a frozen daily-log
//! tree (`log/`), an optional confirmed adapter document (`adapters.json`),
//! the read calls to make (`queries.json`) and what the pre-SQLite index
//! answered (`golden.json`): the relearn revision and evidence, every read
//! API's output and a canonical dump of the five public `index.db` tables.
//!
//! The goldens were captured from the hand-written snapshot code before the
//! rusqlite store replaced it. Later work keeps these tests green without
//! editing the goldens; `bless_golden_fixtures` exists only to add a new
//! fixture (it needs `BRAUSER_BLESS_GOLDENS=1` as well as `--ignored`), and
//! rewriting an existing golden needs a stated reason.

use super::*;
use crate::url_provenance::UrlProvenance;
use brauser_protocol::{ActivityMode, ActivityRef, ActivityTrail};
use serde_json::{Value, json};

/// Every fixture directory, in the order the tests check them.
pub(super) const FIXTURES: &[&str] = &[
    "hostile",
    "case-collision",
    "containers",
    "legacy-m1",
    "summaries",
    "conflicts",
    "backdated",
    "equal-instants",
    "overflow",
    "year",
];

/// Fixture directories that are not golden read fixtures: `v8` is a cache a
/// format 8 build wrote, which the migration tests (`migration.rs`) load.
pub(super) const MIGRATION_FIXTURES: &[&str] = &["v8"];

/// An ID no fixture records, so every read's "not found" answer is pinned.
const UNRECORDED: &str = "work:example.com/never-recorded";

/// The legacy column names and order of each public table. Columns a later
/// schema adds (such as a dirty generation) are deliberately not dumped.
const PUBLIC_TABLES: &[(&str, &[&str])] = &[
    (
        "artifacts",
        &[
            "artifact_id",
            "artifact_type",
            "site",
            "title",
            "url",
            "first_seen",
            "last_seen",
            "focused_secs",
            "background_secs",
            "opens",
            "days_opened",
            "last_mode",
            "why",
        ],
    ),
    (
        "visits",
        &[
            "event_id",
            "artifact_id",
            "occurred_at",
            "local_date",
            "focused_secs",
            "background_secs",
            "opens",
            "mode",
            "title",
            "url",
        ],
    ),
    (
        "edges",
        &[
            "source_artifact_id",
            "target_artifact_id",
            "kind",
            "weight",
            "last_seen",
        ],
    ),
    (
        "url_provenance",
        &["event_id", "artifact_id", "role", "url", "stripped_params"],
    ),
    ("dirty_artifacts", &["artifact_id"]),
];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Queries {
    /// Every artifact ID the log names (activity, why, legacy or summary),
    /// plus one that it never does.
    artifacts: Vec<String>,
    recent: Vec<RecentQuery>,
    /// Inclusive local-date ranges, at most seven days each.
    worklog: Vec<(String, String)>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecentQuery {
    artifact_type: Option<String>,
    title_query: Option<String>,
    limit: usize,
}

pub(super) fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/index")
}

pub(super) fn read_json(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

pub(super) fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A fresh root holding a copy of the fixture's logs, an empty cache and an
/// index over them under the fixture's adapter document.
struct Loaded {
    index: ArtifactIndex,
    queries: Queries,
    golden: Value,
    // Dropped after `index`, which holds no handle into either.
    _root: TempDir,
    _cache: TempDir,
}

fn load(name: &str) -> Loaded {
    let dir = fixtures_root().join(name);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    copy_tree(&dir.join("log"), &root.path().join("log"));
    let adapters_path = dir.join("adapters.json");
    let adapters = adapters_path.exists().then(|| {
        let value: adapter_templates::Value =
            serde_json::from_value(read_json(&adapters_path)).unwrap();
        adapter_templates::Document::from_value(&value).unwrap()
    });
    let index = index(&root, &cache).with_adapters(adapters.as_ref());
    let queries = serde_json::from_value(read_json(&dir.join("queries.json"))).unwrap();
    let golden_path = dir.join("golden.json");
    let golden = if golden_path.exists() {
        read_json(&golden_path)
    } else {
        Value::Null
    };
    Loaded {
        index,
        queries,
        golden,
        _root: root,
        _cache: cache,
    }
}

fn outcome<T: Serialize>(result: Result<T>) -> Value {
    match result {
        Ok(value) => json!({ "ok": value }),
        Err(error) => json!({ "error": format!("{error:#}") }),
    }
}

fn stored_url(stored: &StoredUrl) -> Value {
    json!({ "url": stored.url, "stripped_params": stored.stripped_params })
}

fn relearn(index: &ArtifactIndex) -> Value {
    outcome(index.relearn_evidence().map(|(revision, evidence)| {
        let evidence = evidence
            .iter()
            .map(|(key, url)| json!({ "record_key": key, "url": stored_url(url) }))
            .collect::<Vec<_>>();
        json!({ "revision": revision, "evidence": evidence })
    }))
}

fn reads(index: &ArtifactIndex, queries: &Queries) -> Value {
    let mut cards = serde_json::Map::new();
    for id in &queries.artifacts {
        let card = index.artifact_with_summary(id);
        // `artifact` is the card without its summary, from its own catch-up.
        let alone = index.artifact(id);
        if let (Ok((view, _)), Ok(alone)) = (&card, &alone) {
            assert_eq!(view, alone, "{id}: artifact differs from its card");
        }
        let evidence = index
            .artifact_url_evidence(id)
            .map(|urls| urls.iter().map(stored_url).collect::<Vec<_>>());
        cards.insert(
            id.clone(),
            json!({
                "card": outcome(card.map(|(view, summary)| json!({
                    "artifact": view,
                    "summary": summary,
                }))),
                "summary_for": outcome(index.summary_for(id)),
                "latest_why": outcome(index.latest_why(id)),
                "url_evidence": outcome(evidence),
                "urls": outcome(index.artifact_urls(id)),
            }),
        );
    }
    let mut recent = serde_json::Map::new();
    for query in &queries.recent {
        let key = format!(
            "type={:?} query={:?} limit={}",
            query.artifact_type, query.title_query, query.limit
        );
        let result = index.recent(
            query.artifact_type.as_deref(),
            query.title_query.as_deref(),
            query.limit,
        );
        recent.insert(
            key,
            outcome(result.map(|(items, types, truncated)| {
                json!({ "items": items, "types": types, "truncated": truncated })
            })),
        );
    }
    let mut worklog = serde_json::Map::new();
    for (start, end) in &queries.worklog {
        let (start_date, end_date) = (parse_date(start).unwrap(), parse_date(end).unwrap());
        let report = index.worklog_report(start_date, end_date);
        if let (Ok((text, count, _, _)), Ok(short)) =
            (&report, &index.worklog(start_date, end_date))
        {
            assert_eq!((text, count), (&short.0, &short.1), "{start}..{end}");
        }
        worklog.insert(
            format!("{start}..{end}"),
            outcome(report.map(|(text, count, gaps, gaps_truncated)| {
                json!({
                    "text": text,
                    "count": count,
                    "gaps": gaps,
                    "gaps_truncated": gaps_truncated,
                })
            })),
        );
    }
    json!({ "artifacts": cards, "recent": recent, "worklog": worklog })
}

/// Every public table's legacy columns, each ordered by all of them.
fn tables(database: &Path) -> Value {
    use rusqlite::types::ValueRef;
    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let mut dump = serde_json::Map::new();
    for (table, columns) in PUBLIC_TABLES {
        let list = columns.join(", ");
        let order = (1..=columns.len())
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = connection
            .prepare(&format!("SELECT {list} FROM {table} ORDER BY {order}"))
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                (0..columns.len())
                    .map(|n| {
                        Ok(match row.get_ref(n)? {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(value) => json!(value),
                            ValueRef::Real(value) => json!(value),
                            ValueRef::Text(value) => json!(String::from_utf8_lossy(value)),
                            ValueRef::Blob(value) => json!({ "blob": hex::encode(value) }),
                        })
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        dump.insert((*table).to_owned(), json!(rows));
    }
    // Close before the temporary directory is removed (Windows).
    connection.close().map_err(|(_, error)| error).unwrap();
    Value::Object(dump)
}

/// The table dump after the index first catches up with the fixture's logs.
fn caught_up_tables(index: &ArtifactIndex) -> Value {
    index.reconcile_from_logs().unwrap();
    tables(&index.database_path())
}

fn assert_golden(fixture: &str, section: &str, actual: &Value, golden: &Value) {
    let expected = &golden[section];
    assert!(
        !expected.is_null(),
        "{fixture}: golden.json has no {section}"
    );
    if let (Value::Object(actual), Value::Object(expected)) = (actual, expected) {
        let keys = |map: &serde_json::Map<String, Value>| map.keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(actual), keys(expected), "{fixture} {section}: keys");
        for (key, value) in actual {
            assert_golden_value(&format!("{fixture} {section}.{key}"), value, &expected[key]);
        }
    } else {
        assert_golden_value(&format!("{fixture} {section}"), actual, expected);
    }
}

fn assert_golden_value(context: &str, actual: &Value, expected: &Value) {
    if let (Value::Object(actual), Value::Object(expected)) = (actual, expected)
        && actual.keys().eq(expected.keys())
    {
        for (key, value) in actual {
            assert_golden_value(&format!("{context}.{key}"), value, &expected[key]);
        }
        return;
    }
    assert!(
        actual == expected,
        "{context} differs from its golden\n actual: {}\n golden: {}",
        serde_json::to_string_pretty(actual).unwrap(),
        serde_json::to_string_pretty(expected).unwrap()
    );
}

#[test]
fn relearn_revision_is_golden() {
    for fixture in FIXTURES {
        let loaded = load(fixture);
        assert_golden(fixture, "relearn", &relearn(&loaded.index), &loaded.golden);
        // Relearn reads only the logs; it never creates the cache.
        assert!(!loaded.index.database_path().exists(), "{fixture}");
    }
}

#[test]
fn read_apis_match_golden() {
    for fixture in FIXTURES {
        let loaded = load(fixture);
        let actual = reads(&loaded.index, &loaded.queries);
        assert_golden(fixture, "reads", &actual, &loaded.golden);
    }
}

#[test]
fn public_tables_match_golden() {
    for fixture in FIXTURES {
        let loaded = load(fixture);
        let actual = caught_up_tables(&loaded.index);
        assert_golden(fixture, "tables", &actual, &loaded.golden);
    }
}

#[test]
fn every_fixture_directory_is_checked() {
    let mut found = fs::read_dir(fixtures_root())
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    found.sort();
    let mut listed = FIXTURES
        .iter()
        .chain(MIGRATION_FIXTURES)
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    listed.sort();
    assert_eq!(found, listed);
}

/// The environment variable that must be set to `1` before
/// `bless_golden_fixtures` rewrites anything. `#[ignore]` alone is not
/// enough: any `--ignored`/`--include-ignored` run would otherwise recapture
/// the goldens from the code under test and hide a parity regression.
const BLESS_ENV: &str = "BRAUSER_BLESS_GOLDENS";

fn bless_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

#[test]
fn blessing_needs_the_explicit_env_var() {
    assert!(!bless_requested(None));
    assert!(!bless_requested(Some("".as_ref())));
    assert!(!bless_requested(Some("0".as_ref())));
    assert!(!bless_requested(Some("true".as_ref())));
    assert!(bless_requested(Some("1".as_ref())));
}

/// Write every fixture's logs, adapter document and queries, then capture
/// its goldens from the code under test. Run only to add a fixture:
/// `BRAUSER_BLESS_GOLDENS=1 cargo test -p brauser --lib bless_golden_fixtures -- --ignored`.
/// Without the env var it does nothing, so a broad `--ignored` run cannot
/// silently rewrite the goldens.
#[test]
#[ignore = "rewrites the committed golden fixtures; also needs BRAUSER_BLESS_GOLDENS=1"]
fn bless_golden_fixtures() {
    if !bless_requested(std::env::var_os(BLESS_ENV).as_deref()) {
        eprintln!("bless_golden_fixtures: {BLESS_ENV}=1 is not set; leaving the goldens untouched");
        return;
    }
    for fixture in build_fixtures() {
        let dir = fixtures_root().join(fixture.name);
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        for (path, text) in &fixture.logs {
            let path = dir.join("log").join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        if let Some(adapters) = &fixture.adapters {
            write_pretty(&dir.join("adapters.json"), adapters);
        }
        write_pretty(
            &dir.join("queries.json"),
            &serde_json::to_value(&fixture.queries).unwrap(),
        );
        // Each section from its own fresh cache, as the tests check them.
        let read_outputs = {
            let loaded = load(fixture.name);
            reads(&loaded.index, &loaded.queries)
        };
        let golden = json!({
            "relearn": relearn(&load(fixture.name).index),
            "reads": read_outputs,
            "tables": caught_up_tables(&load(fixture.name).index),
        });
        write_pretty(&dir.join("golden.json"), &golden);
    }
}

fn write_pretty(path: &Path, value: &Value) {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    fs::write(path, text).unwrap();
}

// ---------------------------------------------------------------------------
// Fixture builders. Markers are written exactly as capture writes them; the
// presentation line is included only where parsing needs it (M1 visits).

struct Fixture {
    name: &'static str,
    logs: Vec<(String, String)>,
    adapters: Option<Value>,
    queries: Queries,
}

const STRIP: &[&str] = &["utm_*"];

fn strip() -> Vec<String> {
    STRIP.iter().map(|value| (*value).to_owned()).collect()
}

fn uuid(fixture: u32, n: u32) -> String {
    format!("00000000-0000-4000-8{fixture:03}-{n:012}")
}

fn act(id: String, artifact: &str, title: Option<&str>, at: &str) -> ActivityEvent {
    ActivityEvent {
        event_id: id,
        artifact_id: format!("work:example.com/{artifact}"),
        artifact_type: "work.item".into(),
        url: format!("https://example.com/items/{artifact}?utm_source=golden"),
        title: title.map(str::to_owned),
        occurred_at: at.into(),
        incognito: false,
        opens: 1,
        focused_seconds: 125,
        background_seconds: 61,
        modes: vec![ActivityMode {
            mode: "view".into(),
            url: format!("https://example.com/items/{artifact}"),
        }],
        trails: Vec::new(),
        refs: Vec::new(),
    }
}

fn why(id: String, artifact_id: &str, url: &str, at: &str, text: &str) -> WhyEvent {
    WhyEvent {
        event_id: id,
        artifact_id: artifact_id.into(),
        url: url.into(),
        occurred_at: at.into(),
        incognito: false,
        text: text.into(),
    }
}

fn marker(kind: &str, id: &str, url: &str, at: &str, data: &[u8], stripped: &str) -> String {
    format!(
        "{MARKER_PREFIX}{} url={} at={at} kind={kind} data={}{stripped} -->\n",
        sha256_hex(id.as_bytes()),
        sha256_hex(url.as_bytes()),
        hex::encode(data)
    )
}

/// A delivered activity as capture logs it: the normalized payload plus its
/// names-only stripped-parameter evidence.
fn activity_line(raw: &ActivityEvent) -> String {
    let strip = strip();
    let event = normalized_activity(raw, &strip).unwrap();
    let provenance = UrlProvenance::activity(raw, &strip).unwrap();
    marker(
        "activity",
        &event.event_id,
        &event.url,
        &event.occurred_at,
        &serde_json::to_vec(&event).unwrap(),
        &provenance.marker(),
    )
}

fn why_line(event: &WhyEvent) -> String {
    let provenance = UrlProvenance::page(&event.url, &strip()).unwrap();
    marker(
        "why",
        &event.event_id,
        &event.url,
        &event.occurred_at,
        &serde_json::to_vec(event).unwrap(),
        &provenance.marker(),
    )
}

fn summary_line(event: &SummaryEvent) -> String {
    marker(
        "summary",
        &event.event_id,
        &event.url,
        &event.occurred_at,
        &serde_json::to_vec(event).unwrap(),
        "",
    )
}

/// An M1 visit: its presentation line, then its data-free marker.
fn legacy_line(marker_seed: &str, at: &str, title: &str, url: &str) -> String {
    format!(
        "\n- {at} — {title} — <{url}>\n{MARKER_PREFIX}{} url={} at={at} -->\n",
        sha256_hex(marker_seed.as_bytes()),
        sha256_hex(url.as_bytes()),
    )
}

fn overflow_line(id: &str, overflow: &OverflowMarker) -> String {
    marker(
        "overflow",
        id,
        "",
        &overflow.last_occurred_at,
        &serde_json::to_vec(overflow).unwrap(),
        "",
    )
}

fn log(lines: &[String]) -> String {
    format!("{LOG_HEADER}\n{}", lines.concat())
}

fn day_log(date: &str) -> String {
    format!("{}/{}/{date}.md", &date[..4], &date[5..7])
}

fn recent(artifact_type: Option<&str>, title_query: Option<&str>, limit: usize) -> RecentQuery {
    RecentQuery {
        artifact_type: artifact_type.map(str::to_owned),
        title_query: title_query.map(str::to_owned),
        limit,
    }
}

/// The recent calls every fixture makes: everything, a truncated page and
/// an empty page.
fn standard_recent() -> Vec<RecentQuery> {
    vec![
        recent(None, None, 50),
        recent(None, None, 1),
        recent(None, None, 0),
        recent(Some("work.item"), None, 50),
        recent(Some("no.such.type"), None, 50),
    ]
}

fn ids(mut ids: Vec<String>) -> Vec<String> {
    ids.push(UNRECORDED.into());
    ids.sort();
    ids.dedup();
    ids
}

fn work(id: &str) -> String {
    format!("work:example.com/{id}")
}

fn range(start: &str, end: &str) -> (String, String) {
    (start.into(), end.into())
}

fn build_fixtures() -> Vec<Fixture> {
    vec![
        hostile(),
        case_collision(),
        containers(),
        legacy_m1(),
        summaries(),
        conflicts(),
        backdated(),
        equal_instants(),
        overflow(),
        year(),
    ]
}

fn hostile() -> Fixture {
    let at = "2026-10-15T12:00:00+00:00";
    let markup = act(
        uuid(1, 1),
        "ABC",
        Some(
            "---\n![fetch](https://attacker.invalid/x) [break] <script>x</script> `code` | pipe # h",
        ),
        at,
    );
    let formatting = act(
        uuid(1, 2),
        "FMT",
        Some("\u{202E}evil\u{200B}zero\u{FEFF}width Ärger ÉCOLE straße"),
        "2026-10-15T13:00:00+00:00",
    );
    let long_title = "L".repeat(2048);
    let long = act(
        uuid(1, 3),
        "LONG",
        Some(&long_title),
        "2026-10-15T14:00:00+00:00",
    );
    let mut untitled = act(uuid(1, 4), "NOTITLE", None, "2026-10-15T15:00:00+00:00");
    untitled.modes[0].mode = "edit".into();
    let context = why(
        uuid(1, 5),
        &markup.artifact_id,
        "https://example.com/items/ABC?utm_medium=why",
        "2026-10-15T12:30:00+00:00",
        "[x](javascript:alert(1)) **bold** \u{202E}rtl\n---\nnext",
    );
    Fixture {
        name: "hostile",
        logs: vec![(
            day_log("2026-10-15"),
            log(&[
                activity_line(&markup),
                why_line(&context),
                activity_line(&formatting),
                activity_line(&long),
                activity_line(&untitled),
            ]),
        )],
        adapters: None,
        queries: Queries {
            artifacts: ids(vec![
                work("ABC"),
                work("FMT"),
                work("LONG"),
                work("NOTITLE"),
            ]),
            recent: [
                standard_recent(),
                vec![
                    recent(None, Some("ärger"), 50),
                    recent(None, Some("ÉCOLE"), 50),
                    recent(None, Some("STRASSE"), 50),
                    recent(None, Some("  "), 50),
                    recent(None, Some("![fetch]"), 50),
                    recent(None, Some("untitled"), 50),
                ],
            ]
            .concat(),
            worklog: vec![range("2026-10-15", "2026-10-15")],
        },
    }
}

fn case_collision() -> Fixture {
    let upper = act(uuid(2, 1), "ABC", Some("Upper"), "2026-10-15T10:00:00Z");
    let lower = act(uuid(2, 2), "abc", Some("Lower"), "2026-10-15T11:00:00Z");
    let mut mixed = act(uuid(2, 3), "Abc", Some("Mixed"), "2026-10-16T09:00:00Z");
    mixed.artifact_id = "work:Example.com/Abc".into();
    let again = act(
        uuid(2, 4),
        "abc",
        Some("Lower again"),
        "2026-10-16T10:00:00Z",
    );
    Fixture {
        name: "case-collision",
        logs: vec![
            (
                day_log("2026-10-15"),
                log(&[activity_line(&upper), activity_line(&lower)]),
            ),
            (
                day_log("2026-10-16"),
                log(&[activity_line(&mixed), activity_line(&again)]),
            ),
        ],
        adapters: None,
        queries: Queries {
            artifacts: ids(vec![
                work("ABC"),
                work("abc"),
                "work:Example.com/Abc".into(),
            ]),
            recent: [
                standard_recent(),
                vec![
                    recent(None, Some("LOWER"), 50),
                    recent(None, Some("upper"), 50),
                ],
            ]
            .concat(),
            worklog: vec![range("2026-10-15", "2026-10-16")],
        },
    }
}

const REPO_ID: &str = "github.repo:github.com/acme-corp/widgets";

fn adapters_document() -> Value {
    let container = json!({
        "type": "github.repo",
        "id": "github.repo:{host}/{owner}/{repo}",
    });
    json!({
        "format": 1,
        "artifacts": [
            {
                "type": "github.pr",
                "origin": "https://github.com",
                "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}/{mode?:(files|commits|checks)}/{*}"}],
                "scope": {"owner": ["acme-corp"]},
                "modes": {"files": "review", "commits": "view", "checks": "view"},
                "id": "github.pr:{host}/{owner}/{repo}/{number}",
                "within": container,
            },
            {
                "type": "github.issue",
                "origin": "https://github.com",
                "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}"}],
                "scope": {"owner": ["acme-corp"]},
                "id": "github.issue:{host}/{owner}/{repo}/{number}",
                "within": container,
            },
            {
                "type": "github.repo",
                "origin": "https://github.com",
                "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/{*}"}],
                "scope": {"owner": ["acme-corp"]},
                "id": "github.repo:{host}/{owner}/{repo}",
            },
            {
                "type": "jira.issue",
                "origin": "https://acme.atlassian.net",
                "match": [{"path": "/browse/{key:jira_key}"}],
                "id": "jira:{host}/{key}",
            },
        ],
    })
}

fn github(
    n: u32,
    (artifact_type, artifact_id): (&str, String),
    url: String,
    title: &str,
    at: &str,
    (focused, background): (u32, u32),
) -> ActivityEvent {
    let mut event = act(uuid(3, n), "X", Some(title), at);
    event.artifact_type = artifact_type.into();
    event.artifact_id = artifact_id;
    event.url = url;
    event.modes.clear();
    event.focused_seconds = focused;
    event.background_seconds = background;
    event
}

fn pr(n: u32, number: u32, title: &str, at: &str, measures: (u32, u32)) -> ActivityEvent {
    github(
        n,
        (
            "github.pr",
            format!("github.pr:github.com/acme-corp/widgets/{number}"),
        ),
        format!("https://github.com/acme-corp/widgets/pull/{number}"),
        title,
        at,
        measures,
    )
}

fn containers() -> Fixture {
    let issue_id = "github.issue:github.com/acme-corp/widgets/3".to_owned();
    let issue_url = "https://github.com/acme-corp/widgets/issues/3".to_owned();
    let jira_id = "jira:acme.atlassian.net/PROJ-1".to_owned();
    let mut jira = github(
        1,
        ("jira.issue", jira_id.clone()),
        "https://acme.atlassian.net/browse/PROJ-1".into(),
        "PROJ-1 Export SBOM",
        "2026-10-14T09:00:00Z",
        (300, 0),
    );
    jira.refs.push(ActivityRef {
        artifact_type: "jira.issue".into(),
        value: "PROJ-1".into(),
    });
    let mut first = pr(
        2,
        1,
        "PROJ-1 Fix SBOM export",
        "2026-10-14T10:00:00Z",
        (125, 61),
    );
    first.refs = ["PROJ-1", "PROJ-1", "UTF-8"]
        .into_iter()
        .map(|value| ActivityRef {
            artifact_type: "jira.issue".into(),
            value: value.into(),
        })
        .collect();
    first.modes.push(ActivityMode {
        mode: "review".into(),
        url: "https://github.com/acme-corp/widgets/pull/1/files?utm_campaign=x".into(),
    });
    let issue = github(
        3,
        ("github.issue", issue_id.clone()),
        issue_url.clone(),
        "Pagination is broken",
        "2026-10-15T09:00:00Z",
        (200, 10),
    );
    let mut second = pr(4, 2, "Paginate exports", "2026-10-15T11:00:00Z", (400, 20));
    second.trails = vec![
        ActivityTrail {
            from_artifact_id: Some(issue_id.clone()),
            from_url: Some(issue_url.clone()),
            transition: "link".into(),
        },
        ActivityTrail {
            from_artifact_id: Some(jira_id.clone()),
            from_url: Some("https://acme.atlassian.net/browse/PROJ-1?utm_source=t".into()),
            transition: "forward_back".into(),
        },
        ActivityTrail {
            from_artifact_id: None,
            from_url: None,
            transition: "typed".into(),
        },
    ];
    let repo = github(
        5,
        ("github.repo", REPO_ID.into()),
        "https://github.com/acme-corp/widgets".into(),
        "acme-corp/widgets",
        "2026-10-16T08:00:00Z",
        (50, 5),
    );
    let first_again = pr(
        6,
        1,
        "PROJ-1 Fix SBOM export (v2)",
        "2026-10-16T12:00:00Z",
        (90, 0),
    );
    let outside = github(
        7,
        ("github.pr", "github.pr:github.com/other-org/tools/9".into()),
        "https://github.com/other-org/tools/pull/9".into(),
        "Out of scope",
        "2026-10-16T13:00:00Z",
        (30, 0),
    );
    let repo_why = why(
        uuid(3, 8),
        REPO_ID,
        "https://github.com/acme-corp/widgets",
        "2026-10-15T08:00:00Z",
        "Tracking the export work",
    );
    Fixture {
        name: "containers",
        logs: vec![
            (
                day_log("2026-10-14"),
                log(&[activity_line(&jira), activity_line(&first)]),
            ),
            (
                day_log("2026-10-15"),
                log(&[
                    why_line(&repo_why),
                    activity_line(&issue),
                    activity_line(&second),
                ]),
            ),
            (
                day_log("2026-10-16"),
                log(&[
                    activity_line(&repo),
                    activity_line(&first_again),
                    activity_line(&outside),
                ]),
            ),
        ],
        adapters: Some(adapters_document()),
        queries: Queries {
            artifacts: ids(vec![
                jira_id,
                first.artifact_id.clone(),
                second.artifact_id.clone(),
                issue_id,
                REPO_ID.into(),
                outside.artifact_id.clone(),
            ]),
            recent: [
                standard_recent(),
                vec![
                    recent(Some("github.pr"), None, 50),
                    recent(Some("github.repo"), None, 50),
                    recent(Some("github.pr"), Some("sbom"), 1),
                ],
            ]
            .concat(),
            worklog: vec![
                range("2026-10-14", "2026-10-16"),
                range("2026-10-15", "2026-10-15"),
            ],
        },
    }
}

fn legacy_m1() -> Fixture {
    let docs = "https://docs.example.org/guide";
    let news = "https://news.example.org/item?id=7";
    let docs_id = format!("web:docs.example.org/{}", sha256_hex(docs.as_bytes()));
    let news_id = format!("web:news.example.org/{}", sha256_hex(news.as_bytes()));
    let work_item = act(uuid(4, 1), "M1", Some("Measured"), "2026-10-19T12:00:00Z");
    Fixture {
        name: "legacy-m1",
        logs: vec![
            (
                day_log("2026-10-18"),
                log(&[
                    legacy_line("docs-1", "2026-10-18T09:00:00Z", "The guide", docs),
                    legacy_line(
                        "news-1",
                        "2026-10-18T10:00:00Z",
                        r"A \*starred\* title",
                        news,
                    ),
                    // No presentation line: never a visit. Since d322188 (merged
                    // from main) strict relearn review refuses it as a damaged
                    // owned marker, so this fixture's relearn golden is that
                    // error; indexing still skips it.
                    format!(
                        "{MARKER_PREFIX}{} url={} at=2026-10-18T11:00:00Z -->\n",
                        sha256_hex(b"orphan"),
                        sha256_hex(docs.as_bytes())
                    ),
                ]),
            ),
            (
                day_log("2026-10-19"),
                log(&[
                    legacy_line("docs-2", "2026-10-19T09:00:00Z", "The guide, revised", docs),
                    activity_line(&work_item),
                ]),
            ),
            (
                day_log("2026-10-20"),
                log(&[legacy_line(
                    "docs-3",
                    "2026-10-20T09:00:00Z",
                    "The guide",
                    docs,
                )]),
            ),
        ],
        adapters: None,
        queries: Queries {
            artifacts: ids(vec![docs_id, news_id, work("M1")]),
            recent: [
                standard_recent(),
                vec![
                    recent(Some("web.page"), None, 50),
                    recent(None, Some("*starred*"), 50),
                ],
            ]
            .concat(),
            worklog: vec![
                range("2026-10-18", "2026-10-20"),
                range("2026-10-20", "2026-10-20"),
            ],
        },
    }
}

fn summary_event(
    n: u32,
    url: &str,
    at: &str,
    title: &str,
    key: RecordKey,
    kind: &str,
) -> SummaryEvent {
    let artifact_id = crate::summary::summary_artifact_id(&key, kind, url).unwrap();
    SummaryEvent {
        overview: None,
        event_id: uuid(5, n),
        artifact_id,
        artifact_type: kind.into(),
        url: url.into(),
        title: title.into(),
        occurred_at: at.into(),
        document_type: "discussion".into(),
        category: "reading".into(),
        importance: brauser_protocol::ImportanceLevel::High,
        summary_path: format!("summaries/s{n}.summary.md"),
        record_key: key,
        harness: brauser_protocol::HarnessId::Claude,
    }
}

fn summaries() -> Fixture {
    let page = "https://news.example.org/item?id=42";
    let page_key = crate::vault::url_record_key(page).unwrap();
    let joined = summary_event(
        1,
        page,
        "2026-10-21T18:00:00Z",
        "A good discussion",
        page_key.clone(),
        "web.page",
    );
    let mut later = summary_event(
        2,
        page,
        "2026-10-22T09:00:00Z",
        "A better discussion",
        page_key,
        "web.page",
    );
    later.importance = brauser_protocol::ImportanceLevel::Low;
    later.harness = brauser_protocol::HarnessId::Codex;
    let work_key = RecordKey {
        kind: RecordKeyKind::Artifact,
        value: work("SUM"),
    };
    let measured = act(
        uuid(5, 3),
        "SUM",
        Some("Summarized work"),
        "2026-10-21T10:00:00Z",
    );
    let tie_a = summary_event(
        4,
        "https://example.com/items/SUM",
        "2026-10-21T12:00:00Z",
        "First at noon",
        work_key.clone(),
        "work.item",
    );
    let tie_b = summary_event(
        5,
        "https://example.com/items/SUM",
        "2026-10-21T12:00:00Z",
        "Second at noon",
        work_key,
        "work.item",
    );
    let only_key = RecordKey {
        kind: RecordKeyKind::Artifact,
        value: work("ONLY"),
    };
    let only = summary_event(
        6,
        "https://example.com/items/ONLY",
        "2026-10-22T08:00:00Z",
        "Summary without activity",
        only_key,
        "work.item",
    );
    Fixture {
        name: "summaries",
        logs: vec![
            (
                day_log("2026-10-20"),
                log(&[legacy_line(
                    "hn",
                    "2026-10-20T10:00:00Z",
                    "A good discussion",
                    page,
                )]),
            ),
            (
                day_log("2026-10-21"),
                log(&[
                    activity_line(&measured),
                    summary_line(&tie_a),
                    summary_line(&tie_b),
                    summary_line(&joined),
                ]),
            ),
            (
                day_log("2026-10-22"),
                log(&[summary_line(&only), summary_line(&later)]),
            ),
        ],
        adapters: None,
        queries: Queries {
            artifacts: ids(vec![joined.artifact_id.clone(), work("SUM"), work("ONLY")]),
            recent: [
                standard_recent(),
                vec![
                    recent(Some("web.page"), None, 50),
                    recent(None, Some("discussion"), 50),
                ],
            ]
            .concat(),
            worklog: vec![
                range("2026-10-20", "2026-10-22"),
                range("2026-10-22", "2026-10-22"),
            ],
        },
    }
}

fn conflicts() -> Fixture {
    let original = act(
        uuid(6, 1),
        "CON",
        Some("First body"),
        "2026-10-15T10:00:00Z",
    );
    let mut rival = original.clone();
    rival.title = Some("Second body".into());
    rival.focused_seconds = 999;
    let duplicate = act(
        uuid(6, 2),
        "DUP",
        Some("Same body twice"),
        "2026-10-15T11:00:00Z",
    );
    let other = act(uuid(6, 3), "OK", Some("Unrelated"), "2026-10-16T09:00:00Z");
    Fixture {
        name: "conflicts",
        logs: vec![
            (
                day_log("2026-10-15"),
                log(&[activity_line(&original), activity_line(&duplicate)]),
            ),
            (
                day_log("2026-10-16"),
                log(&[
                    activity_line(&rival),
                    activity_line(&duplicate),
                    activity_line(&other),
                ]),
            ),
        ],
        adapters: None,
        queries: Queries {
            artifacts: ids(vec![work("CON"), work("DUP"), work("OK")]),
            recent: standard_recent(),
            worklog: vec![range("2026-10-15", "2026-10-16")],
        },
    }
}

fn backdated() -> Fixture {
    let old = act(
        uuid(7, 1),
        "BACK",
        Some("Old title"),
        "2026-10-12T10:00:00Z",
    );
    let newest = act(
        uuid(7, 2),
        "BACK",
        Some("Newest title"),
        "2026-10-20T10:00:00Z",
    );
    let backdated = act(
        uuid(7, 3),
        "BACK",
        Some("Backdated title"),
        "2026-10-10T10:00:00Z",
    );
    let untitled = act(uuid(7, 4), "BACK", None, "2026-10-21T10:00:00Z");
    let evening = act(
        uuid(7, 5),
        "EVE",
        Some("Evening"),
        "2026-10-19T23:30:00-07:00",
    );
    let early_why = why(
        uuid(7, 6),
        &work("EARLY"),
        "https://example.com/items/EARLY",
        "2026-10-09T10:00:00Z",
        "Context before any activity",
    );
    let early = act(uuid(7, 7), "EARLY", Some("Early"), "2026-10-12T11:00:00Z");
    let late_why = why(
        uuid(7, 8),
        &work("BACK"),
        "https://example.com/items/BACK",
        "2026-10-11T10:00:00Z",
        "Older why written later",
    );
    let current_why = why(
        uuid(7, 9),
        &work("BACK"),
        "https://example.com/items/BACK",
        "2026-10-19T10:00:00Z",
        "Current why",
    );
    let orphan_why = why(
        uuid(7, 10),
        &work("WHYONLY"),
        "https://example.com/items/WHYONLY",
        "2026-10-20T12:00:00Z",
        "Saved before activity was delivered",
    );
    Fixture {
        name: "backdated",
        logs: vec![
            (
                day_log("2026-10-12"),
                log(&[activity_line(&old), activity_line(&early)]),
            ),
            (
                day_log("2026-10-19"),
                log(&[why_line(&current_why), activity_line(&evening)]),
            ),
            (
                day_log("2026-10-20"),
                log(&[
                    activity_line(&newest),
                    activity_line(&backdated),
                    why_line(&early_why),
                    why_line(&late_why),
                    why_line(&orphan_why),
                ]),
            ),
            (day_log("2026-10-21"), log(&[activity_line(&untitled)])),
        ],
        adapters: None,
        queries: Queries {
            artifacts: ids(vec![
                work("BACK"),
                work("EVE"),
                work("EARLY"),
                work("WHYONLY"),
            ]),
            recent: [standard_recent(), vec![recent(None, Some("title"), 50)]].concat(),
            worklog: vec![
                range("2026-10-09", "2026-10-15"),
                range("2026-10-16", "2026-10-22"),
                range("2026-10-19", "2026-10-19"),
                range("2026-10-20", "2026-10-20"),
            ],
        },
    }
}

fn equal_instants() -> Fixture {
    let at = "2026-10-15T23:00:00-07:00";
    let same_utc = "2026-10-16T06:00:00Z";
    let nested = act(uuid(8, 1), "TIE", Some("In 2026/10/15.md"), at);
    let dashed = act(uuid(8, 2), "TIE", Some("In 2026/10-b.md"), at);
    let top = act(uuid(8, 3), "TIE", Some("In 2026-10-15.md"), at);
    let utc = act(uuid(8, 4), "TIE", Some("Same instant in UTC"), same_utc);
    let line_one = act(uuid(8, 5), "LINE", Some("Line one"), "2026-10-15T09:00:00Z");
    let line_two = act(uuid(8, 6), "LINE", Some("Line two"), "2026-10-15T09:00:00Z");
    let why_a = why(
        uuid(8, 7),
        &work("TIE"),
        "https://example.com/items/TIE",
        at,
        "Why in the nested log",
    );
    let why_b = why(
        uuid(8, 8),
        &work("TIE"),
        "https://example.com/items/TIE",
        same_utc,
        "Why in the dashed log",
    );
    Fixture {
        name: "equal-instants",
        logs: vec![
            (
                "2026/10/15.md".into(),
                log(&[
                    activity_line(&nested),
                    why_line(&why_a),
                    activity_line(&line_one),
                    activity_line(&line_two),
                ]),
            ),
            (
                "2026/10-b.md".into(),
                log(&[activity_line(&dashed), why_line(&why_b)]),
            ),
            ("2026-10-15.md".into(), log(&[activity_line(&top)])),
            ("2026/10/2026-10-16.md".into(), log(&[activity_line(&utc)])),
        ],
        adapters: None,
        queries: Queries {
            artifacts: ids(vec![work("TIE"), work("LINE")]),
            recent: standard_recent(),
            worklog: vec![
                range("2026-10-15", "2026-10-16"),
                range("2026-10-16", "2026-10-16"),
            ],
        },
    }
}

fn overflow() -> Fixture {
    let spanning = OverflowMarker {
        count: 4,
        first_occurred_at: "2026-10-20T23:00:00-07:00".into(),
        last_occurred_at: "2026-10-22T01:00:00-07:00".into(),
    };
    let overlap = OverflowMarker {
        count: 2,
        first_occurred_at: "2026-10-21T01:00:00-07:00".into(),
        last_occurred_at: "2026-10-21T02:00:00-07:00".into(),
    };
    let measured = act(
        uuid(9, 1),
        "GAP",
        Some("Around the gap"),
        "2026-10-21T09:00:00-07:00",
    );
    Fixture {
        name: "overflow",
        logs: vec![
            (
                day_log("2026-10-21"),
                log(&[
                    overflow_line(&uuid(9, 2), &overlap),
                    activity_line(&measured),
                ]),
            ),
            (
                day_log("2026-10-22"),
                log(&[overflow_line(&uuid(9, 3), &spanning)]),
            ),
        ],
        adapters: None,
        queries: Queries {
            artifacts: ids(vec![work("GAP")]),
            recent: standard_recent(),
            worklog: vec![
                range("2026-10-19", "2026-10-25"),
                range("2026-10-20", "2026-10-20"),
                range("2026-10-21", "2026-10-21"),
                range("2026-10-23", "2026-10-24"),
            ],
        },
    }
}

fn year() -> Fixture {
    let mut logs = Vec::new();
    let mut date = Date::from_calendar_date(2025, time::Month::January, 1).unwrap();
    let mut n = 0;
    while date.year() == 2025 {
        let day = date.to_string();
        n += 1;
        let mut daily = act(
            uuid(10, n),
            "DAILY",
            Some("Daily standup notes"),
            &format!("{day}T09:00:00Z"),
        );
        daily.focused_seconds = 60 + u32::from(date.ordinal()) % 7;
        daily.background_seconds = 0;
        // Small bodies keep a year of logs and its goldens compact.
        daily.url = "https://example.com/items/DAILY".into();
        daily.modes.clear();
        let mut lines = vec![activity_line(&daily)];
        if date.weekday() == time::Weekday::Monday {
            n += 1;
            let mut weekly = act(
                uuid(10, n),
                "WEEKLY",
                Some("Weekly planning"),
                &format!("{day}T15:00:00Z"),
            );
            weekly.url = "https://example.com/items/WEEKLY".into();
            weekly.modes.clear();
            lines.push(activity_line(&weekly));
        }
        logs.push((day_log(&day), log(&lines)));
        date = date.next_day().unwrap();
    }
    Fixture {
        name: "year",
        logs,
        adapters: None,
        queries: Queries {
            artifacts: ids(vec![work("DAILY"), work("WEEKLY")]),
            recent: [standard_recent(), vec![recent(None, Some("weekly"), 50)]].concat(),
            worklog: vec![
                range("2025-01-01", "2025-01-07"),
                range("2025-06-10", "2025-06-16"),
                range("2025-12-25", "2025-12-31"),
            ],
        },
    }
}

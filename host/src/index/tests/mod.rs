//! Index tests, split by area, with the helpers more than one area uses.

use super::*;
use brauser_protocol::{ActivityMode, ActivityRef, ActivityTrail};
use std::fs::File;
use tempfile::TempDir;

mod catch_up;
mod containers;
mod database;
mod gaps;
mod golden;
mod identity;
mod migration;
pub(super) mod mp_harness;
mod ordering;
mod presentation;
mod reads;
mod regeneration;
mod scan;
pub(crate) mod sql;
mod store;
mod summaries;
mod views;
mod watermark;

fn storage(root: &Path) -> StorageConfig {
    StorageConfig {
        root: root.to_string_lossy().into_owned(),
        profile: "neutral".into(),
        log_dir: "log".into(),
        pages_dir: "pages".into(),
        later_dir: "later".into(),
        artifacts_dir: "artifacts".into(),
        summaries_dir: None,
        knowledge_base_dir: None,
    }
}

fn activity(id: &str, native_id: &str, title: &str, at: &str) -> ActivityEvent {
    ActivityEvent {
        event_id: id.into(),
        artifact_id: format!("work:example.com/{native_id}"),
        artifact_type: "work.item".into(),
        url: format!("https://example.com/items/{native_id}?utm_source=test"),
        title: Some(title.into()),
        occurred_at: at.into(),
        incognito: false,
        opens: 1,
        focused_seconds: 125,
        background_seconds: 61,
        modes: vec![ActivityMode {
            mode: "view".into(),
            url: format!("https://example.com/items/{native_id}"),
        }],
        trails: Vec::<ActivityTrail>::new(),
        refs: Vec::<ActivityRef>::new(),
    }
}

fn index(root: &TempDir, cache: &TempDir) -> ArtifactIndex {
    ArtifactIndex::open_in(&storage(root.path()), None, cache.path()).unwrap()
}

/// Run SQLite's own integrity checks on `path`, then one query whose
/// rows are printed one per line, values separated by spaces.
fn sqlite_check(path: &Path, query: &str) -> String {
    let mut text = sql::integrity(path);
    text.push('\n');
    for row in sql::sql_rows(path, query) {
        let values = row.iter().map(sql::display).collect::<Vec<_>>();
        text.push_str(&values.join(" "));
        text.push('\n');
    }
    text
}

fn activity_marker(event: &ActivityEvent) -> String {
    format!(
        "{MARKER_PREFIX}{} url={} at={} kind=activity data={} -->\n",
        sha256_hex(event.event_id.as_bytes()),
        sha256_hex(event.url.as_bytes()),
        event.occurred_at,
        hex::encode(serde_json::to_vec(event).unwrap())
    )
}

fn write_log(root: &Path, name: &str, events: &[&ActivityEvent]) -> PathBuf {
    let dir = root.join("log");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let markers = events.iter().map(|event| activity_marker(event));
    fs::write(
        &path,
        format!("{LOG_HEADER}\n{}", markers.collect::<String>()),
    )
    .unwrap();
    path
}

const GITHUB_REPO_ID: &str = "github.repo:github.com/acme-corp/widgets";
/// Ruling 1: the PR and issue starters inside the repo, then the repo
/// catch-all, all scoped to one owner. `within` is optional so a test
/// can index under the document before containers existed.
fn github_document(within: bool) -> adapter_templates::Document {
    scoped_github_document(within, &["acme-corp"])
}

fn scoped_github_document(within: bool, owners: &[&str]) -> adapter_templates::Document {
    let container = serde_json::json!({
        "type": "github.repo",
        "id": "github.repo:{host}/{owner}/{repo}",
    });
    let mut pr = serde_json::json!({
        "type": "github.pr",
        "origin": "https://github.com",
        "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}/{mode?:(files|commits|checks)}/{*}"}],
        "scope": {"owner": owners},
        "modes": {"files": "review", "commits": "view", "checks": "view"},
        "id": "github.pr:{host}/{owner}/{repo}/{number}",
    });
    let mut issue = serde_json::json!({
        "type": "github.issue",
        "origin": "https://github.com",
        "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}"}],
        "scope": {"owner": owners},
        "id": "github.issue:{host}/{owner}/{repo}/{number}",
    });
    if within {
        pr["within"] = container.clone();
        issue["within"] = container;
    }
    let repo = serde_json::json!({
        "type": "github.repo",
        "origin": "https://github.com",
        "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/{*}"}],
        "scope": {"owner": owners},
        "id": "github.repo:{host}/{owner}/{repo}",
    });
    let document: adapter_templates::Value = serde_json::from_value(serde_json::json!({
        "format": 1,
        "artifacts": [pr, issue, repo],
    }))
    .unwrap();
    adapter_templates::Document::from_value(&document).unwrap()
}

/// A GitHub activity event with `opens = 1` and the given measures.
fn github_activity(
    n: u32,
    (artifact_type, artifact_id): (&str, &str),
    url: &str,
    at: &str,
    (focused, background): (u32, u32),
) -> ActivityEvent {
    let mut event = activity(&format!("00000000-0000-4000-8000-{n:012}"), "X", "Page", at);
    event.artifact_type = artifact_type.into();
    event.artifact_id = artifact_id.into();
    event.url = url.into();
    event.modes.clear();
    event.focused_seconds = focused;
    event.background_seconds = background;
    event
}

fn github_pr(n: u32, number: u32, at: &str) -> ActivityEvent {
    github_activity(
        n,
        (
            "github.pr",
            &format!("github.pr:github.com/acme-corp/widgets/{number}"),
        ),
        &format!("https://github.com/acme-corp/widgets/pull/{number}"),
        at,
        (125, 61),
    )
}

fn github_file(root: &Path, kind: &str) -> String {
    let file = artifact_files(root)
        .into_iter()
        .find(|path| path.to_string_lossy().contains(kind))
        .unwrap();
    fs::read_to_string(file).unwrap()
}

fn expected_view(event: &SummaryEvent) -> brauser_protocol::SummaryView {
    brauser_protocol::SummaryView {
        summarized_at: event.occurred_at.clone(),
        document_type: event.document_type.clone(),
        category: event.category.clone(),
        importance: event.importance,
        summary_path: event.summary_path.clone(),
        harness: event.harness,
    }
}

/// A summary attached to an adapter artifact, as `CaptureStore::record_summary`
/// writes it for an `Artifact` record key.
fn adapter_summary(artifact_id: &str, artifact_type: &str, url: &str, at: &str) -> SummaryEvent {
    SummaryEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        artifact_id: artifact_id.into(),
        artifact_type: artifact_type.into(),
        url: url.into(),
        title: "A good discussion".into(),
        occurred_at: at.into(),
        document_type: "discussion".into(),
        category: "reading".into(),
        importance: brauser_protocol::ImportanceLevel::High,
        summary_path: "summaries/a.summary.md".into(),
        record_key: brauser_protocol::RecordKey {
            kind: brauser_protocol::RecordKeyKind::Artifact,
            value: artifact_id.into(),
        },
        harness: brauser_protocol::HarnessId::Claude,
    }
}

fn artifact_files(root: &Path) -> Vec<PathBuf> {
    fn walk(path: &Path, output: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, output);
            } else if path.extension().and_then(|value| value.to_str()) == Some("md") {
                output.push(path);
            }
        }
    }
    let mut output = Vec::new();
    walk(&root.join("artifacts"), &mut output);
    output.sort();
    output
}

//! Read-time identity projection must leave every durable input untouched.
use super::tests::{activity, adapter_summary, github_document, github_pr, index, summary_marker};
use super::*;
use crate::relearn::{
    Identity,
    aliases::{Action, Event, Resolution},
};
use brauser_protocol::{ActivityRef, ActivityTrail};
use tempfile::TempDir;

fn alias(n: u128, action: Action, kind: &str, from: &str, to: &str) -> Event {
    Event {
        event_id: Uuid::from_u128(n).to_string(),
        action,
        from: Identity {
            artifact_type: kind.into(),
            artifact_id: from.into(),
        },
        to: Identity {
            artifact_type: kind.into(),
            artifact_id: to.into(),
        },
    }
}
fn mappings(pairs: &[(&str, &str, &str)]) -> Resolution {
    Resolution::from_events(
        &pairs
            .iter()
            .enumerate()
            .map(|(n, (kind, from, to))| alias(n as u128 + 1, Action::Confirm, kind, from, to))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}
fn logged(index: &ArtifactIndex, events: &[ActivityEvent], summaries: &[SummaryEvent]) {
    let logs = Path::new(&index.storage.root).join(&index.storage.log_dir);
    fs::create_dir_all(&logs).unwrap();
    let mut bytes = format!("{LOG_HEADER}\n");
    for event in events {
        bytes.push_str(&tests::activity_marker(event));
    }
    for summary in summaries {
        bytes.push_str(&summary_marker(summary));
    }
    fs::write(logs.join("day.md"), bytes).unwrap();
}
fn project(index: &ArtifactIndex, resolution: &Resolution) -> serde_json::Value {
    let (revision, _) = index.relearn_evidence().unwrap();
    serde_json::to_value(index.relearn_projection(resolution, &revision).unwrap()).unwrap()
}
fn document(raw: serde_json::Value) -> Document {
    Document::from_value(&serde_json::from_value(raw).unwrap()).unwrap()
}

#[test]
fn rekey_projection_joins_totals_and_keeps_original_summary_ownership_paths() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let a = activity(
        &Uuid::new_v4().to_string(),
        "A",
        "Old",
        "2026-11-02T10:00:00Z",
    );
    let b = activity(
        &Uuid::new_v4().to_string(),
        "B",
        "New",
        "2026-11-02T11:00:00Z",
    );
    let mut first = adapter_summary(&a.artifact_id, "work.item", &a.url, "2026-11-02T10:30:00Z");
    first.summary_path = "summaries/original-A.summary.md".into();
    let mut second = adapter_summary(&b.artifact_id, "work.item", &b.url, "2026-11-02T11:30:00Z");
    second.summary_path = "summaries/original-B.summary.md".into();
    logged(
        &index,
        &[a.clone(), b.clone()],
        &[first.clone(), second.clone()],
    );
    let raw = serde_json::to_vec(&index.relearn_state().unwrap().events).unwrap();
    let resolution = mappings(&[("work.item", &a.artifact_id, &b.artifact_id)]);
    let result = project(&index, &resolution);
    assert_eq!(result["artifacts"].as_array().unwrap().len(), 1);
    let row = &result["artifacts"][0];
    assert_eq!(row["artifact_id"], b.artifact_id);
    assert_eq!(row["focused_seconds"], 250);
    assert_eq!(row["background_seconds"], 122);
    assert_eq!(row["opens"], 2);
    assert_eq!(row["days_opened"], 1);
    let summaries = &result["summaries"][&b.artifact_id];
    assert_eq!(summaries[0]["summary_path"], first.summary_path);
    assert_eq!(summaries[1]["summary_path"], second.summary_path);
    assert_eq!(
        result["recent"][0]["summary"]["summary_path"],
        second.summary_path
    );
    assert_eq!(result["worklog"]["artifact_count"], 1);
    assert!(
        result["worklog"]["text"]
            .as_str()
            .unwrap()
            .contains("Opened 1 artifact")
    );
    assert_eq!(
        serde_json::to_vec(&index.relearn_state().unwrap().events).unwrap(),
        raw
    );
    let derived = derive_resolved(&index.relearn_state().unwrap(), None, &resolution);
    assert_eq!(derived.artifacts[&b.artifact_id].summaries[0], first);
    let rendered = render_artifact("artifacts", &derived.artifacts[&b.artifact_id]);
    assert!(rendered.contains("original-B.summary.md"));
    assert!(!root.path().join("artifacts").exists());
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn rekey_projection_uses_shared_chain_move_back_and_unalias_semantics() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let events = ["A", "B", "C"]
        .map(|id| activity(&Uuid::new_v4().to_string(), id, id, "2026-11-02T10:00:00Z"));
    logged(&index, &events, &[]);
    let (a, b, c) = (
        &events[0].artifact_id,
        &events[1].artifact_id,
        &events[2].artifact_id,
    );
    let mut history = vec![
        alias(1, Action::Confirm, "work.item", a, b),
        alias(2, Action::Confirm, "work.item", b, c),
    ];
    let result = project(&index, &Resolution::from_events(&history).unwrap());
    assert_eq!(result["artifacts"][0]["artifact_id"], *c);
    assert_eq!(result["artifacts"][0]["opens"], 3);
    history.push(alias(3, Action::Confirm, "work.item", b, a));
    let result = project(&index, &Resolution::from_events(&history).unwrap());
    assert_eq!(result["artifacts"][0]["artifact_id"], *a);
    history.push(alias(4, Action::Unalias, "work.item", a, b));
    let result = project(&index, &Resolution::from_events(&history).unwrap());
    assert_eq!(result["artifacts"].as_array().unwrap().len(), 2);
    assert_eq!(result["artifacts"][0]["artifact_id"], *a);
    assert_eq!(result["artifacts"][0]["opens"], 1);
    assert_eq!(result["artifacts"][1]["artifact_id"], *c);
    assert_eq!(result["artifacts"][1]["opens"], 2);
}

#[test]
fn rekey_projection_resolves_historical_refs_and_incoming_trails_without_self_edges() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let doc = document(serde_json::json!({"format":1,"artifacts":[{
        "type":"jira.issue","origin":"https://example.com","match":[{"path":"/browse/{key:jira_key}"}],"id":"jira:{host}/{key}"}]}));
    let index = index(&root, &cache).with_adapters(Some(&doc));
    let mut old = activity(
        &Uuid::new_v4().to_string(),
        "1",
        "Old",
        "2026-11-02T10:00:00Z",
    );
    old.artifact_type = "jira.issue".into();
    old.artifact_id = "jira:example.com/OLD-1".into();
    old.url = "https://example.com/browse/OLD-1".into();
    old.modes.clear();
    let mut other = activity(
        &Uuid::new_v4().to_string(),
        "2",
        "Other",
        "2026-11-02T11:00:00Z",
    );
    other.refs = vec![ActivityRef {
        artifact_type: "jira.issue".into(),
        value: "OLD-1".into(),
    }];
    other.trails = vec![ActivityTrail {
        from_artifact_id: Some(old.artifact_id.clone()),
        from_url: Some(old.url.clone()),
        transition: "link".into(),
    }];
    let mut new = old.clone();
    new.event_id = Uuid::new_v4().to_string();
    new.artifact_id = "jira:example.com/NEW-1".into();
    new.url = "https://example.com/browse/NEW-1".into();
    new.refs = other.refs.clone();
    new.trails = other.trails.clone();
    logged(&index, &[old.clone(), other.clone(), new.clone()], &[]);
    let resolution = mappings(&[("jira.issue", &old.artifact_id, &new.artifact_id)]);
    let result = project(&index, &resolution);
    let edges = result["edges"].as_array().unwrap();
    assert_eq!(edges.iter().filter(|e| e["kind"] == "mentions").count(), 1);
    assert!(edges.iter().any(|e| e["source"] == other.artifact_id
        && e["target"] == new.artifact_id
        && e["kind"] == "mentions"));
    assert!(edges.iter().all(|e| e["source"] != e["target"]));
    let other_row = result["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["artifact_id"] == other.artifact_id)
        .unwrap();
    assert_eq!(
        other_row["incoming_trails"][0]["source"]["artifact_id"],
        new.artifact_id
    );
}

#[test]
fn rekey_projection_preserves_container_rollups_and_counts_visits_once() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let doc = github_document(true);
    let index = index(&root, &cache).with_adapters(Some(&doc));
    let mut old = github_pr(1, 7, "2026-11-02T10:00:00Z");
    old.artifact_id = "old.pr:github.com/acme-corp/widgets/7".into();
    logged(&index, &[old.clone()], &[]);
    let resolution = mappings(&[
        (
            "github.pr",
            &old.artifact_id,
            "github.pr:github.com/acme-corp/widgets/7",
        ),
        (
            "github.repo",
            "old.repo:github.com/acme-corp/widgets",
            "github.repo:github.com/acme-corp/widgets",
        ),
    ]);
    let result = project(&index, &resolution);
    let rows = result["artifacts"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let pull = rows
        .iter()
        .find(|r| r["artifact_type"] == "github.pr")
        .unwrap();
    let repo = rows
        .iter()
        .find(|r| r["artifact_type"] == "github.repo")
        .unwrap();
    assert_eq!(
        pull["within"]["artifact_id"],
        "github.repo:github.com/acme-corp/widgets"
    );
    assert_eq!(repo["focused_seconds"], 125);
    assert_eq!(
        repo["contained"][0]["artifact_id"],
        "github.pr:github.com/acme-corp/widgets/7"
    );
    assert!(
        result["worklog"]["text"]
            .as_str()
            .unwrap()
            .contains("Opened 1 artifact")
    );
    // A recorded contract can still emit the old implicit container. Both
    // child recognition and the container endpoint must use the resolver.
    let old_doc = document(serde_json::json!({"format":1,"artifacts":[
        {"type":"github.pr","origin":"https://github.com",
         "match":[{"path":"/{owner:name|lower}/{repo:name|lower}/pull/{number:int}"}],
         "scope":{"owner":["acme-corp"]},
         "id":"old.pr:{host}/{owner}/{repo}/{number}",
         "within":{"type":"github.repo","id":"old.repo:{host}/{owner}/{repo}"}},
        {"type":"github.repo","origin":"https://github.com",
         "match":[{"path":"/{owner:name|lower}/{repo:name|lower}"}],
         "scope":{"owner":["acme-corp"]},
         "id":"old.repo:{host}/{owner}/{repo}"}
    ]}));
    assert_eq!(
        project(&index.clone().with_adapters(Some(&old_doc)), &resolution),
        result
    );
}

#[test]
fn rekey_projection_ignores_disposable_cache_and_keeps_url_summaries_separate() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let old = activity(
        &Uuid::new_v4().to_string(),
        "A",
        "Old",
        "2026-11-02T10:00:00Z",
    );
    let mut summary = adapter_summary("placeholder", "web.page", &old.url, "2026-11-02T11:00:00Z");
    summary.record_key = crate::vault::url_record_key(&old.url).unwrap();
    summary.artifact_id = crate::summary::web_page_artifact_id(&old.url).unwrap();
    logged(
        &index,
        std::slice::from_ref(&old),
        std::slice::from_ref(&summary),
    );
    let resolution = mappings(&[("work.item", &old.artifact_id, "new:example.com/A")]);
    let first = project(&index, &resolution);
    assert_eq!(first["artifacts"].as_array().unwrap().len(), 2);
    assert_eq!(
        first["summaries"][&summary.artifact_id][0]["summary_path"],
        summary.summary_path
    );
    fs::write(cache.path().join("index-state.json"), b"broken cache").unwrap();
    fs::write(cache.path().join("index.db"), b"obsolete snapshot").unwrap();
    assert_eq!(project(&index, &resolution), first);
    assert_eq!(
        fs::read(cache.path().join("index-state.json")).unwrap(),
        b"broken cache"
    );
    fs::remove_file(cache.path().join("index-state.json")).unwrap();
    fs::remove_file(cache.path().join("index.db")).unwrap();
    assert_eq!(project(&index, &resolution), first);
}

#[test]
fn rekey_projection_refuses_a_different_log_snapshot_from_the_review() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        &Uuid::new_v4().to_string(),
        "1",
        "Original",
        "2026-11-02T10:00:00Z",
    );
    logged(&index, std::slice::from_ref(&event), &[]);
    let (revision, _) = index.relearn_evidence().unwrap();
    let mut changed = event.clone();
    changed.focused_seconds += 10;
    logged(&index, &[changed], &[]);
    assert!(
        index
            .relearn_projection(&Resolution::default(), &revision)
            .is_err()
    );
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn rekey_projection_revision_binds_append_order_and_capture_gaps() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let first = activity(
        &Uuid::new_v4().to_string(),
        "1",
        "First",
        "2026-11-02T10:00:00Z",
    );
    let second = activity(
        &Uuid::new_v4().to_string(),
        "1",
        "Second",
        "2026-11-02T10:00:00Z",
    );
    logged(&index, &[first.clone(), second.clone()], &[]);
    let (revision, _) = index.relearn_evidence().unwrap();
    let payloads = serde_json::to_vec(&index.relearn_state().unwrap().events).unwrap();
    let before = project(&index, &Resolution::default());
    assert_eq!(before["artifacts"][0]["title"], "Second");
    logged(&index, &[second, first], &[]);
    assert_eq!(
        serde_json::to_vec(&index.relearn_state().unwrap().events).unwrap(),
        payloads
    );
    let after = project(&index, &Resolution::default());
    assert_eq!(after["artifacts"][0]["title"], "First");
    assert!(
        index
            .relearn_projection(&Resolution::default(), &revision)
            .is_err(),
        "identical event payloads in a different append order must invalidate the projection"
    );
    let (revision, _) = index.relearn_evidence().unwrap();
    let path = root.path().join("log/day.md");
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(
        tests::overflow_line(
            "gap",
            &OverflowMarker {
                count: 3,
                first_occurred_at: "2026-11-02T10:00:00Z".into(),
                last_occurred_at: "2026-11-02T10:00:00Z".into(),
            },
        )
        .as_bytes(),
    );
    fs::write(&path, bytes).unwrap();
    assert_eq!(
        serde_json::to_vec(&index.relearn_state().unwrap().events).unwrap(),
        payloads
    );
    assert!(
        index
            .relearn_projection(&Resolution::default(), &revision)
            .is_err(),
        "capture-gap-only changes must invalidate the projection"
    );
    assert_eq!(
        project(&index, &Resolution::default())["worklog"]["capture_gaps"][0]["count"],
        3
    );
}

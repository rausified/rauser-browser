//! The staged in-place rebuild, `brauser reindex` and the adapter change
//! (plan T10).

use super::golden::{FIXTURES, fixtures_root, read_json};
use super::*;

/// The public tables and `brauser_artifact_days`, each ordered by its key.
/// Generations, sequences and order keys are not part of it, so a rebuilt
/// store and one caught up incrementally compare equal.
fn canonical(index: &ArtifactIndex) -> Vec<(&'static str, Vec<Vec<sql::Value>>)> {
    [
        (
            "artifacts",
            "SELECT artifact_id, artifact_type, site, title, url, first_seen, last_seen,
               focused_secs, background_secs, opens, days_opened, last_mode, why
             FROM artifacts ORDER BY artifact_id",
        ),
        (
            "visits",
            "SELECT event_id, artifact_id, occurred_at, local_date, focused_secs,
               background_secs, opens, mode, title, url FROM visits ORDER BY event_id",
        ),
        (
            "edges",
            "SELECT source_artifact_id, target_artifact_id, kind, weight, last_seen FROM edges
             ORDER BY source_artifact_id, target_artifact_id, kind",
        ),
        (
            "url_provenance",
            "SELECT event_id, artifact_id, role, url, stripped_params FROM url_provenance
             ORDER BY event_id, role",
        ),
        (
            "brauser_artifact_days",
            "SELECT artifact_id, local_date, focused_secs, background_secs, opens,
               own_focused_secs, own_background_secs, own_opens, summaries
             FROM brauser_artifact_days ORDER BY artifact_id, local_date",
        ),
    ]
    .into_iter()
    .map(|(table, query)| (table, sql::sql_rows(&index.database_path(), query)))
    .collect()
}

fn stage_tables(cache: &TempDir) -> Vec<Vec<sql::Value>> {
    sql::sql_rows(
        &db::db_path(cache.path()),
        "SELECT name FROM sqlite_master WHERE name LIKE 'brauser_stage_%' ORDER BY name",
    )
}

fn meta(cache: &TempDir, key: &str) -> Vec<Vec<sql::Value>> {
    sql::sql_rows(
        &db::db_path(cache.path()),
        &format!("SELECT value FROM brauser_meta WHERE key = '{key}'"),
    )
}

fn text(value: &str) -> Vec<Vec<sql::Value>> {
    vec![vec![sql::Value::Text(value.into())]]
}

fn dirty_ids(cache: &TempDir) -> Vec<String> {
    sql::sql_rows(
        &db::db_path(cache.path()),
        "SELECT artifact_id FROM dirty_artifacts ORDER BY artifact_id",
    )
    .into_iter()
    .map(|row| match &row[0] {
        sql::Value::Text(id) => id.clone(),
        other => panic!("{other:?}"),
    })
    .collect()
}

fn fixture_adapters(dir: &Path) -> Option<adapter_templates::Document> {
    let path = dir.join("adapters.json");
    path.exists().then(|| {
        let value: adapter_templates::Value = serde_json::from_value(read_json(&path)).unwrap();
        adapter_templates::Document::from_value(&value).unwrap()
    })
}

/// Every file under `dir`, relative to it, sorted.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(base: &Path, path: &Path, output: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, output);
            } else {
                output.push(path.strip_prefix(base).unwrap().to_path_buf());
            }
        }
    }
    let mut output = Vec::new();
    walk(dir, dir, &mut output);
    output.sort();
    output
}

/// Three one-event daily logs of equal size, and that size.
fn three_logs(root: &TempDir) -> (Vec<ActivityEvent>, u64) {
    let events = (1..=3)
        .map(|day| {
            activity(
                &format!("00000000-0000-4000-8000-{:012}", 500 + day),
                &format!("P{day}"),
                &format!("Item {day}"),
                &format!("2026-10-1{day}T12:00:00Z"),
            )
        })
        .collect::<Vec<_>>();
    let mut size = 0;
    for (day, event) in events.iter().enumerate() {
        let path = write_log(root.path(), &format!("2026-10-1{}.md", day + 1), &[event]);
        size = fs::metadata(path).unwrap().len();
    }
    (events, size)
}

#[test]
fn incremental_equals_reindex() {
    for fixture in FIXTURES {
        let dir = fixtures_root().join(fixture);
        let adapters = fixture_adapters(&dir);
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let open = || index(&root, &cache).with_adapters(adapters.as_ref());
        let incremental = open();
        // The logs arrive a few at a time, each batch caught up in turn.
        let logs = files_under(&dir.join("log"));
        let batch = logs.len().div_ceil(6).max(1);
        for chunk in logs.chunks(batch) {
            for relative in chunk {
                let target = root.path().join("log").join(relative);
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::copy(dir.join("log").join(relative), target).unwrap();
            }
            incremental.reconcile_from_logs().unwrap();
        }
        let caught_up = canonical(&incremental);
        assert!(!caught_up[0].1.is_empty(), "{fixture}: no artifacts");
        incremental.reindex().unwrap();
        assert_eq!(canonical(&incremental), caught_up, "{fixture}: reindex");

        // A rebuild split over passes: one host stops after its first pass,
        // and a fresh one resumes and finishes it.
        SCAN_LIMIT.with(|value| value.set(1));
        let stopped = open();
        let finished = stopped
            .rebuild_pass(crate::index::rebuild::Finish::All)
            .unwrap();
        assert_eq!(finished, logs.len() == 1, "{fixture}");
        drop(stopped);
        SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
        open().reindex().unwrap();
        assert_eq!(canonical(&open()), caught_up, "{fixture}: resumed");
        assert!(stage_tables(&cache).is_empty(), "{fixture}");
    }
}

#[test]
fn rebuild_progress_resumes_after_a_crash_mid_pass() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let (_, size) = three_logs(&root);
    // One pass reads two of the three logs and commits them to the stage.
    SCAN_LIMIT.with(|value| value.set(size * 5 / 2));
    let first = index(&root, &cache);
    LOG_BYTES_READ.with(|count| count.set(0));
    let error = format!("{:#}", first.recent(None, None, 32).unwrap_err());
    assert!(error.contains("history is unavailable"), "{error}");
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 2 * size);
    // The progress lives in index.db, never in a JSON file, and nothing
    // derived is published before the rebuild finishes.
    assert_eq!(meta(&cache, "rebuild"), text("staging"));
    assert_eq!(stage_tables(&cache).len(), 4);
    assert!(!first.pending_path().exists());
    assert!(!first.state_path().exists());
    assert_eq!(
        sql::sql_rows(&first.database_path(), "SELECT count(*) FROM artifacts"),
        [[sql::Value::Integer(0)]]
    );
    // The host stops mid-rebuild; a new one resumes after the staged logs.
    drop(first);
    let resumed = index(&root, &cache);
    LOG_BYTES_READ.with(|count| count.set(0));
    assert_eq!(resumed.recent(None, None, 32).unwrap().0.len(), 3);
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), size);
    assert_eq!(meta(&cache, "rebuild"), text("none"));
    assert!(stage_tables(&cache).is_empty());
    assert_eq!(dirty_ids(&cache).len(), 3);

    // A reindex that stops after one pass resumes the same way.
    assert!(
        !resumed
            .rebuild_pass(crate::index::rebuild::Finish::All)
            .unwrap()
    );
    drop(resumed);
    let again = index(&root, &cache);
    LOG_BYTES_READ.with(|count| count.set(0));
    assert_eq!(again.reindex().unwrap().written, 3);
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), size);
    SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
}

/// A reindex stopped between passes, with the JSON cache still current and
/// no log changed since, resumes on the next read and the next pass rather
/// than leaving history unavailable until some log changes.
#[test]
fn a_stopped_reindex_resumes_on_reads_when_no_log_changed() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let (_, size) = three_logs(&root);
    let first = index(&root, &cache);
    assert_eq!(first.recent(None, None, 32).unwrap().0.len(), 3);
    assert!(first.state_path().exists());
    // `brauser reindex` commits one pass of two logs and is killed.
    SCAN_LIMIT.with(|value| value.set(size * 5 / 2));
    assert!(
        !first
            .rebuild_pass(crate::index::rebuild::Finish::All)
            .unwrap()
    );
    assert_eq!(meta(&cache, "rebuild"), text("staging"));
    SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
    drop(first);
    // The next read advances the staged rebuild to its finish.
    let reader = index(&root, &cache);
    assert_eq!(reader.recent(None, None, 32).unwrap().0.len(), 3);
    assert_eq!(meta(&cache, "rebuild"), text("none"));
    assert!(stage_tables(&cache).is_empty());

    // A regeneration pass resumes one the same way.
    SCAN_LIMIT.with(|value| value.set(size * 5 / 2));
    assert!(
        !reader
            .rebuild_pass(crate::index::rebuild::Finish::All)
            .unwrap()
    );
    SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
    reader.regenerate_dirty().unwrap();
    assert_eq!(meta(&cache, "rebuild"), text("none"));
    assert_eq!(reader.recent(None, None, 32).unwrap().0.len(), 3);
}

#[test]
fn previous_rows_stay_readable_while_a_rebuild_is_staged() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let (events, size) = three_logs(&root);
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 3);
    let before = canonical(&index);
    SCAN_LIMIT.with(|value| value.set(size * 3 / 2));
    fs::remove_file(root.path().join("log/2026-10-11.md")).unwrap();
    // Each call advances one pass; the published rows stay as they were.
    assert!(index.recent(None, None, 32).is_err());
    assert_eq!(meta(&cache, "rebuild"), text("staging"));
    assert_eq!(canonical(&index), before);
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 2);
    assert!(index.artifact(&events[0].artifact_id).unwrap().is_none());
    assert!(stage_tables(&cache).is_empty());
    SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
}

#[test]
fn a_reset_rebuild_dirties_only_changed_renders() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let (_, _) = three_logs(&root);
    assert_eq!(index.regenerate_dirty().unwrap().written, 3);
    // The user adds notes to one owned file; its frontmatter stays.
    let files = artifact_files(root.path());
    let edited = format!("{}\nMy notes.\n", fs::read_to_string(&files[1]).unwrap());
    fs::write(&files[1], &edited).unwrap();
    // An unrelated log is removed, which rebuilds the store from the rest.
    fs::remove_file(root.path().join("log/2026-10-11.md")).unwrap();
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.failed), (0, 0), "{report:?}");
    assert_eq!(fs::read_to_string(&files[1]).unwrap(), edited);
    assert!(dirty_ids(&cache).is_empty());
    // reindex, by contrast, rewrites every file.
    assert_eq!(index.reindex().unwrap().written, 1);
    assert_ne!(fs::read_to_string(&files[1]).unwrap(), edited);
}

#[test]
fn reindex_truncates_the_write_ahead_log() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    three_logs(&root);
    index.reindex().unwrap();
    // An idle reader keeps the WAL from being removed when the writers
    // close, so only the explicit checkpoint can empty it.
    let held = rusqlite::Connection::open_with_flags(
        index.database_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    held.query_row("SELECT count(*) FROM artifacts", [], |_| Ok(()))
        .unwrap();
    index.reindex().unwrap();
    let wal = cache.path().join("index.db-wal");
    assert_eq!(fs::metadata(&wal).map_or(0, |meta| meta.len()), 0);
    drop(held);
}

#[test]
fn adapter_change_dirties_only_changed_renders() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let other_pr = github_activity(
        2,
        ("github.pr", "github.pr:github.com/other-org/gadgets/3"),
        "https://github.com/other-org/gadgets/pull/3",
        "2026-10-15T11:00:00Z",
        (30, 0),
    );
    write_log(
        root.path(),
        "2026-10-15.md",
        &[&github_pr(1, 7, "2026-10-15T10:00:00Z"), &other_pr],
    );
    let before = scoped_github_document(true, &["acme-corp"]);
    let old = index(&root, &cache).with_adapters(Some(&before));
    assert_eq!(old.regenerate_dirty().unwrap().written, 3);
    // Regeneration (plan T12) records what it wrote; stand in for it.
    let mut conn = crate::index::store::open(cache.path()).unwrap();
    db::write_txn(&mut conn, |tx| {
        for group in rebuild_ids(tx)? {
            let events = crate::index::store::group_events(tx, &group)?;
            let derived = crate::index::store::fold_events(Some(&before), &events);
            if let Some(row) = derived.artifacts.get(&group) {
                tx.execute(
                    "UPDATE brauser_group_state SET render_sha256 = ?2 WHERE artifact_id = ?1",
                    rusqlite::params![
                        group,
                        crate::index::rebuild::render_digest("artifacts", row)
                    ],
                )?;
            }
        }
        Ok(())
    })
    .unwrap();
    drop(conn);
    // The other organization's repository becomes a container; nothing
    // the acme-corp files show changes.
    let after = scoped_github_document(true, &["acme-corp", "other-org"]);
    let new = index(&root, &cache).with_adapters(Some(&after));
    new.reconcile_from_logs().unwrap();
    assert_eq!(
        dirty_ids(&cache),
        ["github.repo:github.com/other-org/gadgets"]
    );
    let report = new.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged), (1, 0), "{report:?}");
    assert_eq!(artifact_files(root.path()).len(), 4);
}

fn rebuild_ids(tx: &rusqlite::Transaction<'_>) -> Result<Vec<String>> {
    Ok(tx
        .prepare("SELECT artifact_id FROM brauser_group_state")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

#[test]
fn the_rebuild_records_where_its_rows_came_from() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    three_logs(&root);
    index.reindex().unwrap();
    assert_eq!(meta(&cache, "root"), text(&index.storage.root));
    assert_eq!(meta(&cache, "root_identity"), text(&index.root_identity));
    assert_eq!(meta(&cache, "log_dir"), text("log"));
    assert_eq!(meta(&cache, "artifacts_dir"), text("artifacts"));
    assert_eq!(meta(&cache, "rebuild"), text("none"));
    assert_eq!(meta(&cache, "rebuild_id").len(), 1);
}

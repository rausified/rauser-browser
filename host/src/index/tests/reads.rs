//! Reads answer from `index.db` (plan T11): one read snapshot after a
//! stat-only freshness check, a catch-up only when the logs are ahead, and
//! the committed snapshot when that catch-up is busy.

use super::*;

fn summary(artifact_id: &str, at: &str) -> SummaryEvent {
    adapter_summary(artifact_id, "work.item", "https://example.com/items/A", at)
}

/// One artifact with activity, a why and a summary, recorded the way the
/// native host records them (the log first, then the index).
fn recorded(root: &TempDir, cache: &TempDir) -> (ArtifactIndex, ActivityEvent) {
    let index = index(root, cache);
    let event = activity(
        "00000000-0000-4000-8000-0000000000c1",
        "A",
        "Über Straße",
        "2026-10-15T10:00:00Z",
    );
    write_log(root.path(), "2026-10-15.md", &[&event]);
    index.record_activity(&event, &[]).unwrap();
    index
        .record_why(
            &WhyEvent {
                event_id: "00000000-0000-4000-8000-0000000000c2".into(),
                artifact_id: event.artifact_id.clone(),
                url: event.url.clone(),
                occurred_at: "2026-10-15T10:05:00Z".into(),
                incognito: false,
                text: "reviewing it".into(),
            },
            &[],
        )
        .unwrap();
    index
        .record_summary(&summary(&event.artifact_id, "2026-10-15T09:50:00Z"))
        .unwrap();
    (index, event)
}

fn every_read(index: &ArtifactIndex, id: &str) {
    let day = parse_date("2026-10-15").unwrap();
    let (card, card_summary) = index.artifact_with_summary(id).unwrap();
    assert!(card.is_some() && card_summary.is_some());
    assert!(index.summary_for(id).unwrap().is_some());
    assert_eq!(
        index.latest_why(id).unwrap().as_deref(),
        Some("reviewing it")
    );
    assert!(!index.artifact_url_evidence(id).unwrap().is_empty());
    assert_eq!(index.recent(None, None, 10).unwrap().0.len(), 1);
    assert_eq!(index.worklog_report(day, day).unwrap().1, 1);
}

#[test]
fn reads_answer_from_index_db_without_a_full_derive() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let (index, event) = recorded(&root, &cache);
    FULL_DERIVES.with(|count| count.set(0));
    every_read(&index, &event.artifact_id);
    assert_eq!(FULL_DERIVES.with(std::cell::Cell::get), 0);
}

#[test]
fn a_current_read_reads_no_log_and_commits_nothing() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let (index, event) = recorded(&root, &cache);
    // The logs are checkpointed in brauser_logs, so the stat-only check
    // finds nothing to catch up: no log byte is read and no write
    // transaction runs.
    LOG_BYTES_READ.with(|count| count.set(0));
    TRANSACTIONS.with(|count| count.set(0));
    every_read(&index, &event.artifact_id);
    assert_eq!(LOG_BYTES_READ.with(std::cell::Cell::get), 0);
    assert_eq!(TRANSACTIONS.with(std::cell::Cell::get), 0);
    assert_no_json_cache(cache.path());
}

#[test]
fn a_read_catches_up_logs_another_writer_appended() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let (_, event) = recorded(&root, &cache);
    // Another process appended to the log and has not indexed it.
    let later = activity(
        "00000000-0000-4000-8000-0000000000c3",
        "B",
        "Later",
        "2026-10-15T11:00:00Z",
    );
    write_log(root.path(), "2026-10-15.md", &[&event, &later]);
    let reader = ArtifactIndex::open_in(&storage(root.path()), None, cache.path()).unwrap();
    assert_eq!(
        reader.artifact(&later.artifact_id).unwrap().unwrap().title,
        "Later"
    );
    assert_eq!(reader.recent(None, None, 10).unwrap().0.len(), 2);
}

#[test]
fn a_busy_catch_up_serves_the_committed_snapshot() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let (index, event) = recorded(&root, &cache);
    let later = activity(
        "00000000-0000-4000-8000-0000000000c4",
        "B",
        "Later",
        "2026-10-15T11:00:00Z",
    );
    write_log(root.path(), "2026-10-15.md", &[&event, &later]);
    let busy = || Err(anyhow::Error::from(db::DbError::Busy));
    // The logs are ahead and the lock holder is ingesting them: the read
    // answers from what is committed instead of failing.
    let titles = index
        .read_with(busy, |tx| {
            Ok(query::recent(tx, None, None, 10)?
                .0
                .into_iter()
                .map(|item| item.title)
                .collect::<Vec<_>>())
        })
        .unwrap();
    assert_eq!(titles, ["Über Straße"]);
    // Any other catch-up failure is still reported.
    let failed = index.read_with(
        || bail!("artifact history is unavailable: test"),
        |_| Ok(()),
    );
    assert!(failed.is_err());
}

#[test]
fn a_busy_catch_up_never_serves_another_folders_snapshot() {
    let root = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    recorded(&root, &cache);
    // The cache is shared; a snapshot for another folder is not this one's.
    let elsewhere = index(&other, &cache);
    let busy = || Err(anyhow::Error::from(db::DbError::Busy));
    let error = elsewhere.read_with(busy, |_| Ok(())).unwrap_err();
    assert!(format!("{error:#}").contains("artifact history is unavailable"));
}

#[test]
fn title_queries_fold_case_as_rust_lowercases() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let (index, _) = recorded(&root, &cache);
    // SQLite's lower() folds ASCII only; brauser_fold matches Rust.
    assert_eq!(index.recent(None, Some("ÜBER"), 10).unwrap().0.len(), 1);
    assert_eq!(index.recent(None, Some("straße"), 10).unwrap().0.len(), 1);
    assert!(
        index
            .recent(None, Some("strasse"), 10)
            .unwrap()
            .0
            .is_empty()
    );
    assert!(
        index
            .recent(Some("web.page"), Some("über"), 10)
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn an_adapter_change_is_caught_up_before_a_read() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let pr = github_pr(1, 7, "2026-10-15T10:00:00Z");
    write_log(root.path(), "2026-10-15.md", &[&pr]);
    index.record_activity(&pr, &[]).unwrap();
    assert!(index.artifact(GITHUB_REPO_ID).unwrap().is_none());
    // The logs are unchanged, but containers follow the adapters.
    let adapted = index.with_adapters(Some(&github_document(true)));
    let repo = adapted.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!(repo.contained.len(), 1);
}

/// A read whose logs are ahead while another connection holds the write
/// lock serves the snapshot after about one busy timeout, not after the
/// schema check and the catch-up each waited out every retry.
#[test]
fn a_read_behind_a_held_write_lock_serves_the_snapshot_within_one_timeout() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let (index, event) = recorded(&root, &cache);
    let later = activity(
        "00000000-0000-4000-8000-0000000000c4",
        "B",
        "Later",
        "2026-10-15T11:00:00Z",
    );
    write_log(root.path(), "2026-10-15.md", &[&event, &later]);
    let holder = rusqlite::Connection::open(index.database_path()).unwrap();
    holder.execute("BEGIN IMMEDIATE", []).unwrap();
    let timeout = Duration::from_millis(1_000);
    let _short = BusyTimeout::set(timeout);
    let started = Instant::now();
    let waits_before = db::BEGIN_WAITS.with(std::cell::Cell::get);
    let (recent, _, _) = index.recent(None, None, 10).unwrap();
    let waited = started.elapsed();
    // The read's catch-up waits out the held lock exactly once.
    assert_eq!(db::BEGIN_WAITS.with(std::cell::Cell::get) - waits_before, 1);
    assert_eq!(
        recent
            .into_iter()
            .map(|item| item.title)
            .collect::<Vec<_>>(),
        ["Über Straße"]
    );
    // The count above is the exact check; this wall-clock bound only
    // catches a multiplied timeout, with room for a slow CI runner.
    assert!(waited < timeout * 4, "{waited:?}");
    holder.execute("ROLLBACK", []).unwrap();
    drop(holder);
    assert_eq!(index.recent(None, None, 10).unwrap().0.len(), 2);
}

/// A handle's reads of a current index share one open of `index.db`
/// (opening it and its `-wal`/`-shm` is slow where endpoint security scans
/// each open), and a database replaced underneath is never read through
/// the stale handle.
#[test]
fn a_handles_reads_share_one_connection_and_follow_a_replaced_file() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let (index, event) = recorded(&root, &cache);
    index.artifact(&event.artifact_id).unwrap().unwrap();
    let before = db::OPENS.with(std::cell::Cell::get);
    for _ in 0..3 {
        index.artifact(&event.artifact_id).unwrap().unwrap();
        index.summary_for(&event.artifact_id).unwrap();
        index.latest_why(&event.artifact_id).unwrap();
    }
    let opens = db::OPENS.with(std::cell::Cell::get) - before;
    if cfg!(unix) {
        assert_eq!(opens, 0, "reads reuse the handle's connection");
    } else {
        assert_eq!(opens, 9, "Windows opens per read and caches nothing");
    }

    // The database goes away underneath: the next read rebuilds it from the
    // log instead of answering from the deleted file.
    remove_database(&index);
    assert!(index.artifact(&event.artifact_id).unwrap().is_some());
    assert!(index.database_path().exists());
}

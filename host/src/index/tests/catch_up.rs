use super::*;

#[test]
fn checkpoints_skip_unchanged_logs_and_recover_completed_trailing_markers() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let first = activity(
        "00000000-0000-4000-8000-000000000001",
        "FIRST",
        "First",
        "2026-10-15T12:00:00Z",
    );
    let second = activity(
        "00000000-0000-4000-8000-000000000002",
        "SECOND",
        "Second",
        "2026-10-16T12:00:00Z",
    );
    let dir = root.path().join("log");
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("first.md"),
        format!("{LOG_HEADER}\n{}", activity_marker(&first)),
    )
    .unwrap();
    assert!(index.artifact(&first.artifact_id).unwrap().is_some());
    // A partial trailing marker is not lost when its completion arrives;
    // the entire changed file (including the previous prose) is reparsed.
    let marker = activity_marker(&second);
    let cut = marker.len() / 2;
    let path = dir.join("second.md");
    fs::write(&path, format!("{LOG_HEADER}\n{}", &marker[..cut])).unwrap();
    LOG_BYTES_READ.with(|count| count.set(0));
    assert!(index.artifact(&second.artifact_id).unwrap().is_none());
    assert_eq!(
        LOG_BYTES_READ.with(|count| count.get()),
        fs::metadata(&path).unwrap().len()
    );
    StdOpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&marker.as_bytes()[cut..])
        .unwrap();
    LOG_BYTES_READ.with(|count| count.set(0));
    assert_eq!(
        index.artifact(&second.artifact_id).unwrap().unwrap().opens,
        1
    );
    assert_eq!(
        LOG_BYTES_READ.with(|count| count.get()),
        fs::metadata(&path).unwrap().len()
    );
    assert!(
        read_state(&index.state_path())
            .unwrap()
            .unwrap()
            .dirty
            .contains(&second.artifact_id)
    );
    LOG_BYTES_READ.with(|count| count.set(0));
    assert_eq!(
        index.artifact(&second.artifact_id).unwrap().unwrap().opens,
        1
    );
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);
    // The old format has no ingestion checkpoints. Treat it as a cache
    // miss and rebuild every event, even if its old events map is valid.
    let mut old: serde_json::Value =
        serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
    old["version"] = 1.into();
    old["events"].as_object_mut().unwrap().clear();
    old.as_object_mut().unwrap().remove("logs");
    fs::write(index.state_path(), serde_json::to_vec(&old).unwrap()).unwrap();
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 2);
    assert_eq!(
        read_state(&index.state_path()).unwrap().unwrap().version,
        STATE_VERSION
    );
}

#[test]
fn catch_up_bounds_aggregate_reads_without_publishing_partial_history() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    index
        .record_activity(
            &activity(
                "00000000-0000-4000-8000-000000000001",
                "FIRST",
                "First",
                "2026-10-15T12:00:00Z",
            ),
            &[],
        )
        .unwrap();
    let old_state = fs::read(index.state_path()).unwrap();
    let old_db = fs::read(index.database_path()).unwrap();
    fs::create_dir(root.path().join("log")).unwrap();
    for day in 1..=3 {
        let mut file = File::create(root.path().join(format!("log/{day}.md"))).unwrap();
        file.write_all(format!("{LOG_HEADER}\n").as_bytes())
            .unwrap();
        file.set_len(24 * 1024 * 1024).unwrap();
    }
    LOG_BYTES_READ.with(|count| count.set(0));
    let error = index.recent(None, None, 32).unwrap_err().to_string();
    assert!(
        error.contains("catch-up reached its 64 MiB per-pass limit"),
        "{error}"
    );
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 48 * 1024 * 1024);
    assert_eq!(fs::read(index.state_path()).unwrap(), old_state);
    assert_eq!(fs::read(index.database_path()).unwrap(), old_db);
    // The next pass resumes after the logs already read.
    LOG_BYTES_READ.with(|count| count.set(0));
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 1);
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 24 * 1024 * 1024);
    assert!(!index.pending_path().exists());
}

/// Run `pass` until it succeeds. Every pass must stay within the
/// per-pass read bound and report history unavailable until it is whole.
fn bounded_passes<T>(limit: u64, mut pass: impl FnMut() -> Result<T>) -> (T, usize) {
    for passes in 1..=16 {
        LOG_BYTES_READ.with(|count| count.set(0));
        let result = pass();
        let read = LOG_BYTES_READ.with(|count| count.get());
        assert!(read <= limit, "pass {passes} read {read} of {limit} bytes");
        match result {
            Ok(value) => return (value, passes),
            Err(error) => {
                let error = format!("{error:#}");
                assert!(error.contains("history is unavailable"), "{error}");
            }
        }
    }
    panic!("catch-up never finished");
}

#[test]
fn full_rebuilds_finish_in_bounded_passes_once_history_exceeds_one_pass() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let events = (1..=6)
        .map(|day| {
            activity(
                &format!("00000000-0000-4000-8000-{day:012}"),
                &format!("N{day}"),
                &format!("Item {day}"),
                &format!("2026-10-1{day}T12:00:00Z"),
            )
        })
        .collect::<Vec<_>>();
    let log = |day: usize| format!("2026-10-1{day}.md");
    // Two logs fit in one pass; the whole history needs three.
    let size = fs::metadata(write_log(root.path(), &log(1), &[&events[0]]))
        .unwrap()
        .len();
    let limit = size * 5 / 2;
    SCAN_LIMIT.with(|value| value.set(limit));
    // History grows one day at a time, so each catch-up reads one log.
    for (day, event) in events.iter().enumerate().map(|(n, event)| (n + 1, event)) {
        write_log(root.path(), &log(day), &[event]);
        let (recent, passes) = bounded_passes(limit, || index.recent(None, None, 32));
        assert_eq!((recent.0.len(), passes), (day, 1));
    }

    // A deleted cache rebuilds in bounded passes and never publishes a
    // partial checkpoint on the way.
    fs::remove_file(index.state_path()).unwrap();
    let old_db = fs::read(index.database_path()).unwrap();
    let mut partial = Vec::new();
    let (recent, passes) = bounded_passes(limit, || {
        let result = index.recent(None, None, 32);
        if result.is_err() {
            partial.push((
                index.state_path().exists(),
                fs::read(index.database_path()).unwrap() == old_db,
            ));
        }
        result
    });
    assert_eq!((recent.0.len(), passes), (6, 3));
    assert_eq!(partial, [(false, true); 2]);
    LOG_BYTES_READ.with(|count| count.set(0));
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 6);
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);

    // The documented repair command finishes in one invocation.
    let report = index.reindex().unwrap();
    assert_eq!(report.written, 6);
    assert_eq!(artifact_files(root.path()).len(), 6);
    LOG_BYTES_READ.with(|count| count.set(0));
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 6);
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);

    // Duplicate reconciliation resumes its own rebuild.
    let (_, passes) = bounded_passes(limit, || index.reconcile_from_logs());
    assert_eq!(passes, 3);
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 6);

    // A deleted log resets and rebuilds the remaining five.
    fs::remove_file(root.path().join("log").join(log(1))).unwrap();
    let (recent, passes) = bounded_passes(limit, || index.recent(None, None, 32));
    assert_eq!((recent.0.len(), passes), (5, 3));
    // A truncated log does the same with the remaining four.
    fs::write(
        root.path().join("log").join(log(2)),
        format!("{LOG_HEADER}\n"),
    )
    .unwrap();
    let (report, _) = bounded_passes(limit, || index.regenerate_dirty());
    assert_eq!(report.failed, 0);
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 4);
    assert!(index.artifact(&events[1].artifact_id).unwrap().is_none());
}

#[test]
fn markdown_without_a_log_header_does_not_stall_a_bounded_rebuild() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-000000000001",
        "FIRST",
        "First",
        "2026-10-15T12:00:00Z",
    );
    let size = fs::metadata(write_log(root.path(), "2026-10-15.md", &[&event]))
        .unwrap()
        .len();
    // Two notes that are not logs fill the first pass and sort first.
    for name in ["2026-01-01.md", "2026-01-02.md"] {
        fs::write(
            root.path().join("log").join(name),
            "x".repeat(size as usize),
        )
        .unwrap();
    }
    let limit = size * 5 / 2;
    SCAN_LIMIT.with(|value| value.set(limit));
    let (recent, passes) = bounded_passes(limit, || index.recent(None, None, 32));
    assert_eq!((recent.0.len(), passes), (1, 2));
    LOG_BYTES_READ.with(|count| count.set(0));
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 1);
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);
}

#[test]
fn a_rebuild_split_over_passes_keeps_the_earliest_logged_body() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let original = activity(
        "00000000-0000-4000-8000-000000000010",
        "REUSED",
        "Original",
        "2026-10-20T12:00:00Z",
    );
    let mut backdated = original.clone();
    backdated.title = Some("Backdated".into());
    let other = activity(
        "00000000-0000-4000-8000-000000000011",
        "OTHER",
        "Other",
        "2026-10-25T12:00:00Z",
    );
    let size = fs::metadata(write_log(root.path(), "2026-10-20.md", &[&original]))
        .unwrap()
        .len();
    write_log(root.path(), "2026-10-25.md", &[&other]);
    // One log per pass, with no cache to keep a winner from.
    let limit = size * 3 / 2;
    SCAN_LIMIT.with(|value| value.set(limit));
    assert!(index.recent(None, None, 32).is_err());
    // An earlier-dated log lands between passes.
    write_log(root.path(), "2026-10-15.md", &[&backdated]);
    let (recent, _) = bounded_passes(limit, || index.recent(None, None, 32));
    assert_eq!(recent.0.len(), 2);
    let bounded = index
        .artifact(&original.artifact_id)
        .unwrap()
        .unwrap()
        .title;
    // The same logs rebuilt in one pass without a cache.
    SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
    fs::remove_file(index.state_path()).unwrap();
    let single = index
        .artifact(&original.artifact_id)
        .unwrap()
        .unwrap()
        .title;
    assert_eq!(
        (bounded.as_str(), single.as_str()),
        ("Backdated", "Backdated")
    );
}

#[test]
fn a_reset_catch_up_keeps_the_conflict_winner_the_cache_chose() {
    let original = activity(
        "00000000-0000-4000-8000-000000000010",
        "REUSED",
        "Original",
        "2026-10-20T12:00:00Z",
    );
    let mut backdated = original.clone();
    backdated.title = Some("Backdated".into());
    let unrelated = activity(
        "00000000-0000-4000-8000-000000000011",
        "OTHER",
        "Other",
        "2026-09-01T12:00:00Z",
    );
    // Same artifact and instant, so log position decides the latest
    // title: the winner's own position, not the rejected body's.
    let sibling = activity(
        "00000000-0000-4000-8000-000000000012",
        "REUSED",
        "Sibling",
        "2026-10-20T12:00:00Z",
    );
    // Whether the rebuild after the reset takes one pass or several.
    for bounded in [false, true] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let title = || {
            index
                .artifact(&original.artifact_id)
                .unwrap()
                .unwrap()
                .title
        };
        write_log(root.path(), "2026-09-01.md", &[&unrelated]);
        write_log(root.path(), "2026-10-18.md", &[&sibling]);
        write_log(root.path(), "2026-10-20.md", &[&original]);
        index.regenerate_dirty().unwrap();
        assert_eq!(title(), "Original");
        // A later delivery backdated into an earlier-dated log.
        let backdated_log = write_log(root.path(), "2026-10-15.md", &[&backdated]);
        index.regenerate_dirty().unwrap();
        assert_eq!(title(), "Original", "bounded={bounded}");
        index.reindex().unwrap();
        assert_eq!(title(), "Original", "bounded={bounded}");
        // One log per pass, so the two bodies meet in different passes.
        let limit = fs::metadata(&backdated_log).unwrap().len() * 3 / 2;
        if bounded {
            SCAN_LIMIT.with(|value| value.set(limit));
        }
        // Removing an unrelated log must not swap in the backdated body.
        fs::remove_file(root.path().join("log/2026-09-01.md")).unwrap();
        let (report, passes) = bounded_passes(MAX_SCAN_BYTES, || index.regenerate_dirty());
        assert_eq!(report.failed, 0);
        assert_eq!(passes, if bounded { 3 } else { 1 });
        assert_eq!(title(), "Original", "bounded={bounded}");
        let owned = artifact_files(root.path())
            .into_iter()
            .map(|path| fs::read_to_string(path).unwrap())
            .collect::<String>();
        assert!(owned.contains("Original"), "bounded={bounded}: {owned}");
        assert!(!owned.contains("Backdated"), "bounded={bounded}: {owned}");
        SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
    }
}

#[test]
fn reindex_rebuilds_database_and_files_from_log_markers() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-000000000010",
        "REBUILD",
        "Rebuilt",
        "2026-10-15T12:00:00Z",
    );
    let payload = hex::encode(serde_json::to_vec(&event).unwrap());
    let event_hash = sha256_hex(event.event_id.as_bytes());
    let url_hash = sha256_hex(event.url.as_bytes());
    let log = format!(
        "<!-- brauser:daily-log v1 -->\n\n- ignored presentation\n<!-- brauser:visit id={event_hash} url={url_hash} at={} kind=activity data={payload} -->\n<!-- brauser:visit id=broken -->\n",
        event.occurred_at,
    );
    let dir = root.path().join("log/2026/10");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("2026-10-15.md"), log).unwrap();

    let report = index.reindex().unwrap();
    assert_eq!(report.written, 1);
    let artifact = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert!(artifact.contains("Rebuilt"));
    let output = Command::new("python3").args([
            "-c", "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('select count(*) from visits').fetchone()[0])",
        ]).arg(index.database_path()).output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "1");
}

#[test]
fn a_renamed_log_folder_is_rescanned_and_its_artifacts_regenerate_in_place() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let old = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-000000000020",
        "MOVED",
        "Moved",
        "2026-10-15T12:00:00Z",
    );
    write_log(root.path(), "2026-10-15.md", &[&event]);
    assert_eq!(
        old.artifact(&event.artifact_id).unwrap().unwrap().title,
        "Moved"
    );
    old.regenerate_dirty().unwrap();
    // The layout migration renames log/ to logs/ and records the new
    // name; the cache that still names log/ must not hide the history.
    fs::rename(root.path().join("log"), root.path().join("logs")).unwrap();
    let mut renamed = storage(root.path());
    renamed.log_dir = "logs".into();
    let new = ArtifactIndex::open_in(&renamed, None, cache.path()).unwrap();
    assert_eq!(
        new.artifact(&event.artifact_id).unwrap().unwrap().title,
        "Moved"
    );
    let files = artifact_files(root.path());
    assert_eq!(files.len(), 1);
    let before = fs::read(&files[0]).unwrap();
    new.regenerate_dirty().unwrap();
    assert_eq!(artifact_files(root.path()), files);
    assert_eq!(fs::read(&files[0]).unwrap(), before);
}

#[test]
fn reconciling_a_duplicate_keeps_the_dirty_set_and_external_edits() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let events = (1..=3)
        .map(|n| {
            activity(
                &format!("00000000-0000-4000-8000-{n:012}"),
                &format!("N{n}"),
                &format!("Item {n}"),
                "2026-10-15T10:00:00Z",
            )
        })
        .collect::<Vec<_>>();
    let path = write_log(root.path(), "2026-10-15.md", &[&events[0], &events[1]]);
    assert_eq!(index.regenerate_dirty().unwrap().written, 2);
    let edited = artifact_files(root.path())
        .into_iter()
        .find(|path| fs::read_to_string(path).unwrap().contains("Item 1"))
        .unwrap();
    let kept = format!("{}\nmy edit\n", fs::read_to_string(&edited).unwrap());
    fs::write(&edited, &kept).unwrap();
    // A drained legacy visit or duplicate retry reconciles from the log.
    StdOpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(activity_marker(&events[2]).as_bytes())
        .unwrap();
    index.reconcile_from_logs().unwrap();
    assert_eq!(
        read_state(&index.state_path()).unwrap().unwrap().dirty,
        BTreeSet::from([events[2].artifact_id.clone()])
    );
    assert_eq!(index.regenerate_dirty().unwrap().written, 1);
    assert_eq!(fs::read_to_string(&edited).unwrap(), kept);
}

#[test]
fn a_valid_stale_cache_catches_up_events_appended_by_another_path() {
    // brwr-ee5: deliver A and keep its cache; B reaches the durable log
    // without an index update; the next delivery or query includes B.
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let a = activity(
        "00000000-0000-4000-8000-000000000001",
        "A",
        "A",
        "2026-10-15T10:00:00Z",
    );
    let b = activity(
        "00000000-0000-4000-8000-000000000002",
        "B",
        "B",
        "2026-10-15T11:00:00Z",
    );
    let c = activity(
        "00000000-0000-4000-8000-000000000003",
        "C",
        "C",
        "2026-10-16T11:00:00Z",
    );
    let path = write_log(root.path(), "2026-10-15.md", &[&a]);
    index.record_activity(&a, &[]).unwrap();
    let stale = fs::read(index.state_path()).unwrap();
    StdOpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(activity_marker(&b).as_bytes())
        .unwrap();
    fs::write(index.state_path(), &stale).unwrap();
    write_log(root.path(), "2026-10-16.md", &[&c]);
    index.record_activity(&c, &[]).unwrap();
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 3);
    // A restored stale cache on a later query (a restart) also catches up.
    fs::write(index.state_path(), &stale).unwrap();
    let restarted = ArtifactIndex::open_in(&storage(root.path()), None, cache.path()).unwrap();
    assert!(restarted.artifact(&b.artifact_id).unwrap().is_some());
}

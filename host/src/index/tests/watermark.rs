//! The `brauser_logs` watermark catch-up (`index/catchup.rs`, plan T9),
//! driven directly against `index.db` with a recording ingest sink.

use super::*;
use rusqlite::Connection;
use std::time::{Duration, SystemTime};

/// What one catch-up pass handed to the store: event ID, log, line.
type Seen = Vec<(String, PathBuf, u64)>;

fn connection(cache: &TempDir) -> Connection {
    db::open(cache.path(), db::Mode::ReadWrite)
        .unwrap()
        .unwrap()
}

/// One committed catch-up pass, the events it ingested and the log bytes
/// it read.
fn pass(index: &ArtifactIndex, conn: &mut Connection) -> (catchup::CatchUp, Seen, u64) {
    LOG_BYTES_READ.with(|count| count.set(0));
    let mut seen = Vec::new();
    let outcome = db::write_txn(conn, |tx| {
        catchup::catch_up(tx, &index.storage, &index.root_identity, &mut |_,
                                                                          event,
                                                                          (
            path,
            line,
        )| {
            seen.push((event_identity(&event).0, path, line));
            Ok(())
        })
    })
    .unwrap();
    (outcome, seen, LOG_BYTES_READ.with(|count| count.get()))
}

fn freshness(index: &ArtifactIndex, conn: &mut Connection) -> (catchup::Freshness, u64) {
    LOG_BYTES_READ.with(|count| count.set(0));
    let fresh = db::read_txn(conn, |tx| {
        catchup::freshness(tx, &index.storage, &index.root_identity)
    })
    .unwrap();
    (fresh, LOG_BYTES_READ.with(|count| count.get()))
}

fn done(logs_read: usize) -> catchup::CatchUp {
    catchup::CatchUp::Caught {
        logs_read,
        complete: true,
    }
}

fn numbered(n: u32, at: &str) -> ActivityEvent {
    activity(
        &format!("00000000-0000-4000-8000-{n:012}"),
        &format!("N{n:04}"),
        &format!("Item {n:04}"),
        at,
    )
}

fn append(path: &Path, text: &str) {
    StdOpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

fn len(path: &Path) -> u64 {
    fs::metadata(path).unwrap().len()
}

/// Every event a full parse of `path` finds, as catch-up reports them.
fn full_parse(path: &Path, relative: &str) -> Seen {
    parse_log_lines(&fs::read_to_string(path).unwrap())
        .into_iter()
        .map(|(line, event)| (event_identity(&event).0, PathBuf::from(relative), line))
        .collect()
}

fn logs_rows(cache: &TempDir) -> Vec<Vec<sql::Value>> {
    sql::sql_rows(
        &db::db_path(cache.path()),
        "SELECT path, purpose, bytes, parsed_bytes FROM brauser_logs ORDER BY path, purpose",
    )
}

fn meta(cache: &TempDir, key: &str) -> Vec<Vec<sql::Value>> {
    sql::sql_rows(
        &db::db_path(cache.path()),
        &format!("SELECT value FROM brauser_meta WHERE key = '{key}'"),
    )
}

/// A log of `count` events, large enough that its tail window (4 KiB) is
/// a small part of it.
fn long_log(root: &TempDir, name: &str, count: u32) -> (PathBuf, Vec<ActivityEvent>) {
    let events = (1..=count)
        .map(|n| numbered(n, "2026-10-15T12:00:00Z"))
        .collect::<Vec<_>>();
    let path = write_log(root.path(), name, &events.iter().collect::<Vec<_>>());
    assert!(len(&path) > 4 * catchup::TAIL_WINDOW);
    (path, events)
}

#[test]
fn unchanged_logs_are_never_reread_and_a_completed_trailing_marker_is_recovered() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    let first = numbered(1, "2026-10-15T12:00:00Z");
    let second = numbered(2, "2026-10-16T12:00:00Z");
    let first_log = write_log(root.path(), "first.md", &[&first]);
    let (outcome, seen, read) = pass(&index, &mut conn);
    assert_eq!(outcome, done(1));
    assert_eq!(seen, full_parse(&first_log, "first.md"));
    assert_eq!(read, len(&first_log));

    // A partial trailing marker is left unparsed, and only its log is read.
    let marker = activity_marker(&second);
    let cut = marker.len() / 2;
    let second_log = root.path().join("log/second.md");
    fs::write(&second_log, format!("{LOG_HEADER}\n{}", &marker[..cut])).unwrap();
    let (outcome, seen, read) = pass(&index, &mut conn);
    assert_eq!(outcome, done(1));
    assert!(seen.is_empty(), "{seen:?}");
    assert_eq!(read, len(&second_log));

    // Its completion is recovered from the watermark.
    append(&second_log, &marker[cut..]);
    let (outcome, seen, _) = pass(&index, &mut conn);
    assert_eq!(outcome, done(1));
    assert_eq!(seen, full_parse(&second_log, "second.md"));

    // Nothing changed: nothing is read, nothing ingested.
    let (outcome, seen, read) = pass(&index, &mut conn);
    assert_eq!((outcome, seen, read), (done(0), Vec::new(), 0));
    assert_eq!(
        logs_rows(&cache),
        [
            [
                sql::Value::Text("first.md".into()),
                sql::Value::Text("events".into()),
                sql::Value::Integer(len(&first_log) as i64),
                sql::Value::Integer(len(&first_log) as i64),
            ],
            [
                sql::Value::Text("second.md".into()),
                sql::Value::Text("events".into()),
                sql::Value::Integer(len(&second_log) as i64),
                sql::Value::Integer(len(&second_log) as i64),
            ],
        ]
    );
    drop(conn);
}

#[test]
fn a_pass_bound_stops_before_the_next_log_and_the_next_pass_resumes() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    let logs = (1..=3)
        .map(|day| {
            write_log(
                root.path(),
                &format!("2026-10-1{day}.md"),
                &[&numbered(day, &format!("2026-10-1{day}T12:00:00Z"))],
            )
        })
        .collect::<Vec<_>>();
    let size = len(&logs[0]);
    SCAN_LIMIT.with(|value| value.set(size * 5 / 2));
    let (outcome, seen, read) = pass(&index, &mut conn);
    assert_eq!(
        outcome,
        catchup::CatchUp::Caught {
            logs_read: 2,
            complete: false
        }
    );
    assert_eq!(seen.len(), 2);
    assert_eq!(read, size * 2);
    // Only the logs read are checkpointed, so the next pass resumes.
    assert_eq!(logs_rows(&cache).len(), 2);
    let (outcome, seen, read) = pass(&index, &mut conn);
    assert_eq!(outcome, done(1));
    assert_eq!(seen, full_parse(&logs[2], "2026-10-13.md"));
    assert_eq!(read, size);
    let (outcome, _, read) = pass(&index, &mut conn);
    assert_eq!((outcome, read), (done(0), 0));
    SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
    drop(conn);
}

#[test]
fn markdown_without_a_log_header_does_not_stall_catch_up() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    let event = numbered(1, "2026-10-15T12:00:00Z");
    let log = write_log(root.path(), "2026-10-15.md", &[&event]);
    let size = len(&log);
    // Two notes that are not logs fill the first pass and sort first.
    let note = root.path().join("log/2026-01-01.md");
    for name in ["2026-01-01.md", "2026-01-02.md"] {
        fs::write(
            root.path().join("log").join(name),
            activity_marker(&numbered(9, "2026-10-15T12:00:00Z"))
                .chars()
                .cycle()
                .take(size as usize)
                .collect::<String>(),
        )
        .unwrap();
    }
    SCAN_LIMIT.with(|value| value.set(size * 5 / 2));
    let (outcome, seen, _) = pass(&index, &mut conn);
    assert!(
        matches!(
            outcome,
            catchup::CatchUp::Caught {
                logs_read: 2,
                complete: false
            }
        ),
        "{outcome:?}"
    );
    assert!(seen.is_empty(), "a marker outside a log is never indexed");
    let (outcome, seen, _) = pass(&index, &mut conn);
    assert_eq!(outcome, done(1));
    assert_eq!(seen, full_parse(&log, "2026-10-15.md"));
    let (outcome, _, read) = pass(&index, &mut conn);
    assert_eq!((outcome, read), (done(0), 0));
    // A note that grows is reread whole and still never indexed.
    append(
        &note,
        &activity_marker(&numbered(8, "2026-10-15T12:00:00Z")),
    );
    let (outcome, seen, read) = pass(&index, &mut conn);
    assert_eq!((outcome, seen, read), (done(1), Vec::new(), len(&note)));
    SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
    drop(conn);
}

#[test]
fn an_append_reads_only_the_tail() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    let (path, events) = long_log(&root, "2026-10-15.md", 40);
    let other = write_log(
        root.path(),
        "2026-10-14.md",
        &[&numbered(99, "2026-10-14T12:00:00Z")],
    );
    let (_, seen, read) = pass(&index, &mut conn);
    assert_eq!(seen.len(), events.len() + 1);
    assert_eq!(read, len(&path) + len(&other));

    let before = len(&path);
    let added = numbered(41, "2026-10-15T13:00:00Z");
    let marker = activity_marker(&added);
    append(&path, &marker);
    let (outcome, seen, read) = pass(&index, &mut conn);
    assert_eq!(outcome, done(1));
    // Only the new event, at the line a full parse gives it.
    let full = full_parse(&path, "2026-10-15.md");
    assert_eq!(seen, full[full.len() - 1..]);
    assert_eq!(seen[0].0, added.event_id);
    // The appended bytes plus the guard window before them.
    assert_eq!(read, marker.len() as u64 + catchup::TAIL_WINDOW);
    assert_eq!(len(&path), before + marker.len() as u64);

    // Two appends in a row, the second in a later pass, keep counting lines.
    let later = numbered(42, "2026-10-15T14:00:00Z");
    append(
        &path,
        &format!("\n- presentation\n{}", activity_marker(&later)),
    );
    let (_, seen, _) = pass(&index, &mut conn);
    let full = full_parse(&path, "2026-10-15.md");
    assert_eq!(seen, full[full.len() - 1..]);
    drop(conn);
}

#[test]
fn a_same_size_rewrite_rereads_only_that_log() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    let kept = write_log(
        root.path(),
        "2026-10-14.md",
        &[&numbered(1, "2026-10-14T12:00:00Z")],
    );
    let rewritten = write_log(
        root.path(),
        "2026-10-15.md",
        &[&numbered(2, "2026-10-15T12:00:00Z")],
    );
    pass(&index, &mut conn);
    // The same size with different bytes and a newer modification time.
    let size = len(&rewritten);
    write_log(
        root.path(),
        "2026-10-15.md",
        &[&numbered(3, "2026-10-15T12:00:00Z")],
    );
    assert_eq!(len(&rewritten), size);
    let file = File::options().write(true).open(&rewritten).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    drop(file);
    let (outcome, seen, read) = pass(&index, &mut conn);
    assert_eq!(outcome, done(1));
    assert_eq!(read, size);
    assert_eq!(seen, full_parse(&rewritten, "2026-10-15.md"));
    assert!(len(&kept) > 0);
    drop(conn);
}

#[test]
fn a_tail_guard_mismatch_rereads_that_log_in_full() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    let (path, events) = long_log(&root, "2026-10-15.md", 40);
    pass(&index, &mut conn);
    // An in-place edit inside the hashed window (prose, same length),
    // then an append: the edit is noticed and the whole log reparsed.
    let mut bytes = fs::read(&path).unwrap();
    let at = bytes.len() - 100;
    bytes[at] = if bytes[at] == b'a' { b'b' } else { b'a' };
    fs::write(&path, &bytes).unwrap();
    append(
        &path,
        &activity_marker(&numbered(41, "2026-10-15T13:00:00Z")),
    );
    let (outcome, seen, read) = pass(&index, &mut conn);
    assert_eq!(outcome, done(1));
    assert_eq!(read, len(&path));
    // Every event again, not just the appended one.
    assert_eq!(seen, full_parse(&path, "2026-10-15.md"));
    assert!(seen.len() >= events.len(), "{}", seen.len());
    drop(conn);
}

#[test]
fn a_legacy_marker_in_the_tail_uses_its_prose_line() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    let (path, _) = long_log(&root, "2026-10-15.md", 40);
    pass(&index, &mut conn);
    let url = "https://example.com/legacy";
    let at = "2026-10-15T15:00:00Z";
    // The prose line lands in one pass and its marker in the next.
    append(&path, &format!("\n- {at} — Legacy page — <{url}>\n"));
    let (_, seen, _) = pass(&index, &mut conn);
    assert!(seen.is_empty(), "{seen:?}");
    append(
        &path,
        &format!(
            "{MARKER_PREFIX}{} url={} at={at} -->\n",
            sha256_hex(b"legacy"),
            sha256_hex(url.as_bytes())
        ),
    );
    let (_, seen, read) = pass(&index, &mut conn);
    let full = full_parse(&path, "2026-10-15.md");
    assert_eq!(seen, full[full.len() - 1..]);
    assert_eq!(seen[0].0, sha256_hex(b"legacy"));
    assert!(read < len(&path), "read {read} of {}", len(&path));
    drop(conn);
}

#[test]
fn overflow_markers_keep_their_earliest_position() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    let gap = OverflowMarker {
        count: 4,
        first_occurred_at: "2026-10-20T23:00:00-07:00".into(),
        last_occurred_at: "2026-10-22T01:00:00-07:00".into(),
    };
    let line = format!(
        "{MARKER_PREFIX}{} url={} at={} kind=overflow data={} -->\n",
        sha256_hex(b"gap"),
        sha256_hex(b""),
        gap.last_occurred_at,
        hex::encode(serde_json::to_vec(&gap).unwrap())
    );
    fs::create_dir_all(root.path().join("log")).unwrap();
    let later = root.path().join("log/2026-10-22.md");
    fs::write(&later, format!("{LOG_HEADER}\n{line}")).unwrap();
    pass(&index, &mut conn);
    let rows = || {
        sql::sql_rows(
            &db::db_path(cache.path()),
            "SELECT gap_id, marker, log_path, log_line, start_date, end_date FROM brauser_gaps",
        )
    };
    let expected = |path: &str, line: i64| {
        vec![vec![
            sql::Value::Text(sha256_hex(b"gap")),
            sql::Value::Text(serde_json::to_string(&gap).unwrap()),
            sql::Value::Text(path.into()),
            sql::Value::Integer(line),
            sql::Value::Text("2026-10-20".into()),
            sql::Value::Text("2026-10-22".into()),
        ]]
    };
    assert_eq!(rows(), expected("2026-10-22.md", 1));
    // A retry later in the same log keeps the first copy.
    append(&later, &line);
    pass(&index, &mut conn);
    assert_eq!(rows(), expected("2026-10-22.md", 1));
    // A copy in an earlier-sorting log, found later, takes its position.
    fs::create_dir_all(root.path().join("log/2026")).unwrap();
    fs::write(
        root.path().join("log/2026/10.md"),
        format!("{LOG_HEADER}\n\n{line}"),
    )
    .unwrap();
    pass(&index, &mut conn);
    assert_eq!(rows(), expected("2026/10.md", 2));
    drop(conn);
}

#[test]
fn a_shrunk_or_removed_log_requests_a_staged_rebuild() {
    for removed in [false, true] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let mut conn = connection(&cache);
        let first = write_log(
            root.path(),
            "2026-10-14.md",
            &[&numbered(1, "2026-10-14T12:00:00Z")],
        );
        write_log(
            root.path(),
            "2026-10-15.md",
            &[&numbered(2, "2026-10-15T12:00:00Z")],
        );
        pass(&index, &mut conn);
        if removed {
            fs::remove_file(&first).unwrap();
        } else {
            fs::write(&first, format!("{LOG_HEADER}\n")).unwrap();
        }
        // Appends elsewhere are left to the rebuild, never ingested here.
        write_log(
            root.path(),
            "2026-10-16.md",
            &[&numbered(3, "2026-10-16T12:00:00Z")],
        );
        assert_eq!(freshness(&index, &mut conn).0, catchup::Freshness::Behind);
        let (outcome, seen, _) = pass(&index, &mut conn);
        assert_eq!(outcome, catchup::CatchUp::Rebuilding, "removed={removed}");
        assert!(seen.is_empty());
        assert_eq!(
            meta(&cache, "rebuild"),
            [[sql::Value::Text("staging".into())]]
        );
        let (outcome, seen, read) = pass(&index, &mut conn);
        assert_eq!(
            (outcome, seen, read),
            (catchup::CatchUp::Rebuilding, Vec::new(), 0)
        );
        assert_eq!(
            freshness(&index, &mut conn),
            (catchup::Freshness::Rebuilding, 0)
        );
        drop(conn);
    }
}

#[test]
fn a_missing_log_folder_is_unavailable_once_it_was_read() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    // No folder and no watermark: nothing to do.
    assert_eq!(pass(&index, &mut conn).0, done(0));
    assert_eq!(freshness(&index, &mut conn).0, catchup::Freshness::Current);
    write_log(
        root.path(),
        "2026-10-14.md",
        &[&numbered(1, "2026-10-14T12:00:00Z")],
    );
    pass(&index, &mut conn);
    // A folder that vanished (an unmounted volume) never wipes history.
    fs::rename(root.path().join("log"), root.path().join("moved")).unwrap();
    let error = db::write_txn(&mut conn, |tx| {
        catchup::catch_up(tx, &index.storage, &index.root_identity, &mut |_, _, _| {
            Ok(())
        })
    })
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("history is unavailable"),
        "{error:#}"
    );
    assert!(meta(&cache, "rebuild").is_empty());
    assert_eq!(logs_rows(&cache).len(), 1);
    fs::rename(root.path().join("moved"), root.path().join("log")).unwrap();
    assert_eq!(pass(&index, &mut conn).0, done(0));
    drop(conn);
}

#[test]
fn freshness_is_a_stat_only_check_against_the_watermark() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    let path = write_log(
        root.path(),
        "2026-10-15.md",
        &[&numbered(1, "2026-10-15T12:00:00Z")],
    );
    assert_eq!(
        freshness(&index, &mut conn),
        (catchup::Freshness::Behind, 0)
    );
    pass(&index, &mut conn);
    assert_eq!(
        freshness(&index, &mut conn),
        (catchup::Freshness::Current, 0)
    );
    append(
        &path,
        &activity_marker(&numbered(2, "2026-10-15T13:00:00Z")),
    );
    assert_eq!(
        freshness(&index, &mut conn),
        (catchup::Freshness::Behind, 0)
    );
    pass(&index, &mut conn);
    assert_eq!(
        freshness(&index, &mut conn),
        (catchup::Freshness::Current, 0)
    );
    write_log(
        root.path(),
        "2026-10-16.md",
        &[&numbered(3, "2026-10-16T12:00:00Z")],
    );
    assert_eq!(
        freshness(&index, &mut conn),
        (catchup::Freshness::Behind, 0)
    );
    drop(conn);
}

#[test]
fn a_failed_ingest_commits_no_watermark() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut conn = connection(&cache);
    write_log(
        root.path(),
        "2026-10-15.md",
        &[&numbered(1, "2026-10-15T12:00:00Z")],
    );
    let error = db::write_txn(&mut conn, |tx| {
        catchup::catch_up(tx, &index.storage, &index.root_identity, &mut |_, _, _| {
            bail!("store refused")
        })
    })
    .unwrap_err();
    assert!(format!("{error:#}").contains("store refused"));
    assert!(logs_rows(&cache).is_empty());
    assert_eq!(pass(&index, &mut conn).1.len(), 1);
    drop(conn);
}

#[test]
fn portable_paths_use_forward_slashes() {
    assert_eq!(
        catchup::portable_path(&Path::new("2026").join("10").join("15.md")).as_deref(),
        Some("2026/10/15.md")
    );
    assert_eq!(catchup::portable_path(Path::new("../x.md")), None);
}

#[test]
fn reconcile_reads_only_changed_logs_and_marks_only_new_events_dirty() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let kept = write_log(
        root.path(),
        "2026-10-14.md",
        &[&numbered(1, "2026-10-14T12:00:00Z")],
    );
    let grown = write_log(
        root.path(),
        "2026-10-15.md",
        &[&numbered(2, "2026-10-15T12:00:00Z")],
    );
    assert_eq!(index.regenerate_dirty().unwrap().written, 2);
    // A duplicate replay with nothing new in the logs reads nothing.
    LOG_BYTES_READ.with(|count| count.set(0));
    index.reconcile_from_logs().unwrap();
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);
    // A drained legacy visit lands in one log: only that log is read, and
    // only its new artifact becomes dirty.
    let added = numbered(3, "2026-10-15T13:00:00Z");
    append(&grown, &activity_marker(&added));
    LOG_BYTES_READ.with(|count| count.set(0));
    index.reconcile_from_logs().unwrap();
    assert!(LOG_BYTES_READ.with(|count| count.get()) <= len(&grown));
    assert!(len(&kept) > 0);
    assert_eq!(
        read_state(&index.state_path()).unwrap().unwrap().dirty,
        BTreeSet::from([added.artifact_id.clone()])
    );
}

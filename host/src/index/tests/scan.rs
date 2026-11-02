//! `LogScan`: the log-only scan behind relearn evidence.

use super::*;

const A: &str = "00000000-0000-4000-8000-00000000000a";
const B: &str = "00000000-0000-4000-8000-00000000000b";

fn cached(event: &ActivityEvent) -> CachedEvent {
    CachedEvent {
        data: EventData::Activity(event.clone()),
        url_provenance: None,
    }
}

fn scan(root: &TempDir, strict: bool) -> Result<LogScan> {
    let cache = TempDir::new().unwrap();
    let index = index(root, &cache);
    LogScan::read(&index.storage, &index.root_identity, strict)
}

#[test]
fn a_log_scan_keeps_the_earliest_copy_and_records_conflicts_and_gaps() {
    let root = TempDir::new().unwrap();
    let first = activity(A, "1", "First", "2026-10-01T12:00:00Z");
    let mut rival = first.clone();
    rival.title = Some("Rival".into());
    let other = activity(B, "2", "Other", "2026-10-02T12:00:00Z");
    write_log(root.path(), "2026-10-01.md", &[&first, &other]);
    // A later log repeats one event verbatim and rivals the other.
    write_log(root.path(), "2026-10-02.md", &[&other, &rival]);
    let gap = OverflowMarker {
        count: 2,
        first_occurred_at: "2026-10-03T01:00:00Z".into(),
        last_occurred_at: "2026-10-03T02:00:00Z".into(),
    };
    let gap_line = format!(
        "{MARKER_PREFIX}{} url={} at={} kind=overflow data={} -->\n",
        sha256_hex(b"gap"),
        sha256_hex(b""),
        gap.last_occurred_at,
        hex::encode(serde_json::to_vec(&gap).unwrap())
    );
    fs::write(
        root.path().join("log/2026-10-03.md"),
        format!("{LOG_HEADER}\n{gap_line}{gap_line}"),
    )
    .unwrap();

    let scan = scan(&root, true).unwrap();
    assert!(scan.complete);
    assert!(
        scan.events
            == BTreeMap::from([
                (A.to_owned(), cached(&first)),
                (B.to_owned(), cached(&other))
            ])
    );
    assert_eq!(
        scan.positions,
        BTreeMap::from([
            (A.to_owned(), (PathBuf::from("2026-10-01.md"), 1)),
            (B.to_owned(), (PathBuf::from("2026-10-01.md"), 2)),
        ])
    );
    assert!(scan.conflicts[A] == vec![(cached(&rival), Some((PathBuf::from("2026-10-02.md"), 2)))]);
    assert_eq!(scan.conflicts.len(), 1);
    let gaps = scan
        .gaps
        .iter()
        .map(|(id, gap)| (id.clone(), gap.marker.count, gap.position.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        gaps,
        vec![(sha256_hex(b"gap"), 2, (PathBuf::from("2026-10-03.md"), 1))]
    );
}

#[test]
fn a_log_scan_skips_files_without_the_log_header_and_tolerates_missing_logs() {
    let root = TempDir::new().unwrap();
    let scan_empty = scan(&root, true).unwrap();
    assert!(scan_empty.complete && scan_empty.events.is_empty());
    let event = activity(A, "1", "First", "2026-10-01T12:00:00Z");
    fs::create_dir_all(root.path().join("log")).unwrap();
    fs::write(root.path().join("log/notes.md"), activity_marker(&event)).unwrap();
    assert!(scan(&root, true).unwrap().events.is_empty());
}

#[test]
fn a_log_scan_stops_at_the_pass_bound_and_relearn_refuses_it() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let first = activity(A, "1", "First", "2026-10-01T12:00:00Z");
    let second = activity(B, "2", "Second", "2026-10-02T12:00:00Z");
    let size = fs::metadata(write_log(root.path(), "2026-10-01.md", &[&first]))
        .unwrap()
        .len();
    write_log(root.path(), "2026-10-02.md", &[&second]);
    SCAN_LIMIT.with(|value| value.set(size * 3 / 2));
    let partial = scan(&root, true).unwrap();
    let refused = index(&root, &cache).relearn_evidence();
    SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
    assert!(!partial.complete);
    assert_eq!(partial.events.keys().collect::<Vec<_>>(), vec![A]);
    assert!(
        refused
            .unwrap_err()
            .to_string()
            .contains("exceeds the log scan limit")
    );
}

#[test]
fn relearn_evidence_hashes_the_scanned_events_and_never_touches_the_cache() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let cache_dir = cache.path().join("never");
    let first = activity(A, "1", "First", "2026-10-01T12:00:00Z");
    let second = activity(B, "2", "Second", "2026-10-02T12:00:00Z");
    write_log(root.path(), "2026-10-01.md", &[&first, &second]);
    let index = ArtifactIndex::open_in(&storage(root.path()), None, &cache_dir).unwrap();
    let (revision, evidence) = index.relearn_evidence().unwrap();
    let expected = BTreeMap::from([
        (A.to_owned(), cached(&first)),
        (B.to_owned(), cached(&second)),
    ]);
    assert_eq!(
        revision,
        sha256_hex(&serde_json::to_vec(&expected).unwrap())
    );
    let keys = evidence
        .iter()
        .map(|(key, url)| (key.value.clone(), url.url.clone()))
        .collect::<BTreeSet<_>>();
    assert!(keys.contains(&(first.artifact_id.clone(), first.url.clone())));
    assert!(keys.contains(&(second.artifact_id.clone(), second.url.clone())));
    assert!(!cache_dir.exists());
}

#[test]
fn relearn_evidence_refuses_conflicting_event_ids() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let first = activity(A, "1", "First", "2026-10-01T12:00:00Z");
    let mut rival = first.clone();
    rival.title = Some("Rival".into());
    write_log(root.path(), "2026-10-01.md", &[&first, &rival]);
    let error = index(&root, &cache).relearn_evidence().unwrap_err();
    assert!(error.to_string().contains("conflicting recorded event IDs"));
}

#[cfg(unix)]
#[test]
fn a_strict_log_scan_refuses_symlinked_history_and_a_lenient_one_skips_it() {
    let root = TempDir::new().unwrap();
    let event = activity(A, "1", "First", "2026-10-01T12:00:00Z");
    let saved = write_log(root.path(), "saved.md", &[&event]);
    let outside = root.path().join("saved.md");
    fs::rename(&saved, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.path().join("log/link.md")).unwrap();
    assert!(
        scan(&root, true)
            .err()
            .unwrap()
            .to_string()
            .contains("symlinked log history")
    );
    assert!(scan(&root, false).unwrap().events.is_empty());
}

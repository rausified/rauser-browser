use super::*;

#[test]
fn index_db_has_queryable_tables_and_capture_does_not_touch_artifact_files() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let hostile = "---\n![fetch](https://attacker.invalid/x) [break]";
    index
        .record_activity(
            &activity(
                "00000000-0000-4000-8000-000000000001",
                "ABC",
                hostile,
                "2026-10-15T12:00:00+00:00",
            ),
            &["utm_*".into()],
        )
        .unwrap();
    assert!(!root.path().join("artifacts").exists());

    let script = r#"import json, sqlite3, sys
c=sqlite3.connect(sys.argv[1])
tables=[r[0] for r in c.execute("select name from sqlite_schema where type='table' order by name")]
row=c.execute("select title, url from artifacts").fetchone()
print(json.dumps([tables,row]))"#;
    let output = Command::new("python3")
        .args(["-c", script])
        .arg(index.database_path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decoded: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        decoded[0],
        serde_json::json!([
            "artifacts",
            "dirty_artifacts",
            "edges",
            "url_provenance",
            "visits"
        ])
    );
    assert_eq!(decoded[1][0], hostile);
    assert_eq!(decoded[1][1], "https://example.com/items/ABC");
}

#[test]
fn stripped_url_evidence_survives_cache_rebuild_and_sqlite_queries() {
    use crate::capture::{CaptureOutcome, CapturePolicy, CaptureStore};
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let capture = CaptureStore::open_in(&index.storage, None, state.path()).unwrap();
    let mut event = activity(
        "00000000-0000-4000-8000-000000000333",
        "ABC",
        "Item",
        &crate::capture::fixtures::event_time()
            .format(&Rfc3339)
            .unwrap(),
    );
    event.url = "https://example.com/items/ABC?utm_source=private-value".into();
    event.modes = vec![ActivityMode {
        mode: "edit".into(),
        url: "https://example.com/items/ABC/edit?utm_campaign=other-private-value".into(),
    }];
    event.trails = vec![ActivityTrail {
        from_artifact_id: Some("source:1".into()),
        from_url: Some("https://example.com/source?utm_source=secret".into()),
        transition: "link".into(),
    }];
    let origins = ["https://example.com".into()];
    let strip = ["utm_*".into()];
    assert!(matches!(
        capture.record_activity(
            CapturePolicy {
                enabled: true,
                origins: &origins,
                strip_params: &strip
            },
            &event
        ),
        CaptureOutcome::Persisted { .. }
    ));
    index.record_activity(&event, &strip).unwrap();
    let evidence = index.artifact_url_evidence(&event.artifact_id).unwrap();
    assert_eq!(
        evidence,
        vec![
            StoredUrl {
                url: "https://example.com/items/ABC".into(),
                stripped_params: Some(vec!["utm_source".into()])
            },
            StoredUrl {
                url: "https://example.com/items/ABC/edit".into(),
                stripped_params: Some(vec!["utm_campaign".into()])
            },
        ]
    );
    let original = fs::read(index.state_path()).unwrap();
    assert!(!String::from_utf8_lossy(&original).contains("private-value"));
    index.reindex().unwrap();
    assert_eq!(
        index.artifact_url_evidence(&event.artifact_id).unwrap(),
        evidence
    );
    let script = r#"import sqlite3,sys,json
c=sqlite3.connect(sys.argv[1])
print(json.dumps(list(c.execute('select role,url,stripped_params from url_provenance order by role'))))"#;
    let output = Command::new("python3")
        .args(["-c", script])
        .arg(index.database_path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        rows,
        serde_json::json!([
            [
                "mode:0",
                "https://example.com/items/ABC/edit",
                "[\"utm_campaign\"]"
            ],
            ["trail:0", "https://example.com/source", "[\"utm_source\"]"],
            ["url", "https://example.com/items/ABC", "[\"utm_source\"]"],
        ])
    );
    // A v6 cache has no evidence. Upgrade schedules a gap rescan and does
    // not manufacture an empty list for an older stripped address.
    let mut old: serde_json::Value = serde_json::from_slice(&original).unwrap();
    old["version"] = 6.into();
    for event in old["events"].as_object_mut().unwrap().values_mut() {
        event.as_object_mut().unwrap().remove("url_provenance");
    }
    fs::write(index.state_path(), serde_json::to_vec(&old).unwrap()).unwrap();
    let (migrated, changed) = index.read_current_state().unwrap();
    assert!(changed);
    assert_eq!(migrated.version, STATE_VERSION);
    assert_eq!(serde_json::to_value(&migrated.logs).unwrap(), old["logs"]);
    assert!(migrated.gap_logs.is_empty());
    assert!(
        migrated
            .events
            .values()
            .all(|event| event.url_provenance.is_none())
    );
    // Malformed positional evidence is ignored as an unusable cache.
    let mut invalid: serde_json::Value = serde_json::from_slice(&original).unwrap();
    invalid["events"][&event.event_id]["url_provenance"]["modes"] = serde_json::json!([]);
    fs::write(index.state_path(), serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(index.read_current_state().is_none());
    assert_eq!(
        index.artifact_url_evidence(&event.artifact_id).unwrap(),
        evidence
    );
}

#[test]
fn a_logged_title_over_the_capture_limit_is_skipped_without_stopping_the_index() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    // Capture refuses titles over 2048 bytes; an edited or crafted log
    // line can still carry one inside an otherwise valid marker.
    let crafted = activity(
        "00000000-0000-4000-8000-0000000000a1",
        "CRAFTED",
        &"T".repeat(100_000),
        "2026-10-15T10:00:00Z",
    );
    let longest = activity(
        "00000000-0000-4000-8000-0000000000a2",
        "LONGEST",
        &"L".repeat(2048),
        "2026-10-15T11:00:00Z",
    );
    let just_over = activity(
        "00000000-0000-4000-8000-0000000000a4",
        "OVER",
        &"O".repeat(2049),
        "2026-10-15T11:30:00Z",
    );
    write_log(
        root.path(),
        "2026-10-15.md",
        &[&crafted, &longest, &just_over],
    );
    assert_eq!(index.reindex().unwrap().written, 1);
    assert!(index.artifact(&crafted.artifact_id).unwrap().is_none());
    assert!(index.artifact(&just_over.artifact_id).unwrap().is_none());
    assert!(index.artifact(&longest.artifact_id).unwrap().is_some());
    // Later deliveries keep updating index.db.
    let later = activity(
        "00000000-0000-4000-8000-0000000000a3",
        "LATER",
        "Later",
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&later, &[]).unwrap();
    assert_eq!(
        sqlite_check(
            &index.database_path(),
            "select title from artifacts order by title"
        ),
        format!("ok\n{}\nLater\n", "L".repeat(2048))
    );
}

#[test]
fn an_oversize_cached_row_is_left_out_of_index_db_without_stopping_publication() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    // Log parsing bounds titles, so only a hand-edited or otherwise
    // unexpected cache can hold an event whose rows exceed one SQLite
    // page. Publication must still survive it.
    let oversize = activity(
        "00000000-0000-4000-8000-0000000000b1",
        "OVERSIZE",
        &"T".repeat(100_000),
        "2026-10-15T10:00:00Z",
    );
    let good = activity(
        "00000000-0000-4000-8000-0000000000b2",
        "GOOD",
        "Good",
        "2026-10-15T11:00:00Z",
    );
    let mut state = index.empty_state();
    for event in [&oversize, &good] {
        state.ingest(EventData::Activity(event.clone()).into(), None, false);
    }
    index.publish_state_and_database(&state).unwrap();
    let later = activity(
        "00000000-0000-4000-8000-0000000000b3",
        "LATER",
        "Later",
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&later, &[]).unwrap();
    assert_eq!(
        sqlite_check(
            &index.database_path(),
            "select title from visits order by title"
        ),
        "ok\nGood\nLater\n"
    );
    assert_eq!(
        sqlite_check(&index.database_path(), "select count(*) from artifacts"),
        "ok\n2\n"
    );
    // Only index.db leaves the row out; the derived history keeps it.
    assert!(index.artifact(&oversize.artifact_id).unwrap().is_some());
    // A version 4 cache (from before log titles were bounded) drops
    // the event while it migrates, as a log rescan now would.
    let mut cached: serde_json::Value =
        serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
    cached["version"] = 4.into();
    fs::write(index.state_path(), serde_json::to_vec(&cached).unwrap()).unwrap();
    assert!(index.artifact(&oversize.artifact_id).unwrap().is_none());
    assert!(index.artifact(&good.artifact_id).unwrap().is_some());
    assert_eq!(
        read_state(&index.state_path()).unwrap().unwrap().version,
        STATE_VERSION
    );
}

#[test]
fn index_db_rows_up_to_the_local_payload_limit_are_readable_and_larger_ones_are_left_out() {
    // A table-leaf payload over U-35 = 65,501 bytes must spill to
    // overflow pages, which this writer does not emit. A one-column text
    // record has a 4-byte header here, so 65,497 bytes is the limit.
    let at_limit = "a".repeat(65_497);
    let over_limit = "b".repeat(65_498);
    let image = build_sqlite(&[SqlTable {
        name: "t",
        sql: "CREATE TABLE t(v TEXT)",
        rows: vec![
            vec![SqlValue::Text(at_limit.clone())],
            vec![SqlValue::Text(over_limit)],
            vec![SqlValue::Text("small".into())],
        ],
    }])
    .unwrap();
    assert_eq!(image.skipped_rows, 1);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("t.db");
    fs::write(&path, image.bytes).unwrap();
    assert_eq!(
        sqlite_check(&path, "select length(v) from t order by rowid"),
        "ok\n65497\n5\n"
    );
}

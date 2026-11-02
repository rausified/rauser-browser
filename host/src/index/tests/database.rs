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

    let db = index.database_path();
    let tables = sql::sql_rows(
        &db,
        "select name from sqlite_schema where type='table' order by name",
    )
    .into_iter()
    .map(|row| sql::display(&row[0]))
    .collect::<Vec<_>>();
    // The public tables are exactly the legacy five; every other table is
    // internal and says so by its prefix.
    assert_eq!(
        tables
            .iter()
            .filter(|name| !name.starts_with("brauser_"))
            .collect::<Vec<_>>(),
        [
            "artifacts",
            "dirty_artifacts",
            "edges",
            "url_provenance",
            "visits"
        ]
    );
    // The internal tables, with no stage left over once nothing rebuilds.
    assert_eq!(
        tables
            .iter()
            .filter(|name| name.starts_with("brauser_"))
            .collect::<Vec<_>>(),
        [
            "brauser_artifact_days",
            "brauser_events",
            "brauser_gaps",
            "brauser_group_state",
            "brauser_logs",
            "brauser_meta"
        ]
    );
    assert_no_json_cache(cache.path());
    assert_eq!(
        sql::sql_rows(&db, "select title, url from artifacts")[0],
        [
            sql::Value::Text(hostile.into()),
            sql::Value::Text("https://example.com/items/ABC".into())
        ]
    );
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
    // Only the parameter names are kept, never their values.
    let bodies = sql::sql_rows(&index.database_path(), "SELECT body FROM brauser_events");
    assert_eq!(bodies.len(), 1);
    assert!(!sql::display(&bodies[0][0]).contains("private-value"));
    index.reindex().unwrap();
    assert_eq!(
        index.artifact_url_evidence(&event.artifact_id).unwrap(),
        evidence
    );
    let rows = sql::sql_rows(
        &index.database_path(),
        "select role,url,stripped_params from url_provenance order by role",
    );
    assert_eq!(
        rows,
        [
            [
                "mode:0",
                "https://example.com/items/ABC/edit",
                "[\"utm_campaign\"]"
            ],
            ["trail:0", "https://example.com/source", "[\"utm_source\"]"],
            ["url", "https://example.com/items/ABC", "[\"utm_source\"]"],
        ]
        .map(|row| row.map(|value| sql::Value::Text(value.into())).to_vec())
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
fn an_oversize_row_is_stored_and_rendered_bounded() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    // Log parsing bounds titles, but a delivery's title is not: its rows
    // exceed one SQLite page, so SQLite stores them on overflow pages, and
    // its artifact file renders the title truncated at 2048 characters.
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
    index.record_activities([&oversize, &good], &[]).unwrap();
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
            "select length(title) from visits order by title"
        ),
        "ok\n4\n5\n100000\n"
    );
    assert_eq!(
        sqlite_check(&index.database_path(), "select count(*) from artifacts"),
        "ok\n3\n"
    );
    assert_eq!(
        index
            .artifact(&oversize.artifact_id)
            .unwrap()
            .unwrap()
            .title,
        oversize.title.clone().unwrap()
    );
    assert_eq!(index.regenerate_dirty().unwrap().written, 3);
    let rendered = artifact_files(root.path())
        .into_iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .find(|text| text.contains(&"T".repeat(2000)))
        .unwrap();
    let title = rendered
        .lines()
        .find(|line| line.starts_with("title: "))
        .unwrap();
    assert!(title.contains(&"T".repeat(2000)), "{title}");
    assert!(!title.contains(&"T".repeat(2049)), "{title}");
}

#[test]
fn no_legacy_writer_remains() {
    let modules = [
        ("mod.rs", include_str!("../mod.rs")),
        ("catchup.rs", include_str!("../catchup.rs")),
        ("db.rs", include_str!("../db.rs")),
        ("fold.rs", include_str!("../fold.rs")),
        ("legacy.rs", include_str!("../legacy.rs")),
        ("ord.rs", include_str!("../ord.rs")),
        ("query.rs", include_str!("../query.rs")),
        ("rebuild.rs", include_str!("../rebuild.rs")),
        ("regen.rs", include_str!("../regen.rs")),
        ("render.rs", include_str!("../render.rs")),
        ("scan.rs", include_str!("../scan.rs")),
        ("store.rs", include_str!("../store.rs")),
    ];
    // Spelled in pieces so this file never matches itself.
    let forbidden = [
        concat!("build_", "sqlite"),
        concat!("Sqlite", "Image"),
        concat!("Sql", "Table"),
        concat!("sqlite_", "snapshot"),
        concat!("SQLITE_", "PAGE_SIZE"),
        concat!("mod ", "snapshot"),
        concat!("publish_state_", "and_database"),
        concat!("Pending", "CatchUp"),
        concat!("save_", "pending"),
        concat!("struct Index", "State"),
        concat!("migrate_v", "4_state"),
        concat!("migrate_v", "5_state"),
        concat!("PUBLI", "CATIONS"),
    ];
    for (name, source) in modules {
        for pattern in forbidden {
            assert!(!source.contains(pattern), "{name} still has {pattern}");
        }
    }
    // The legacy JSON files are named only where the migration reads and
    // then removes them, and nothing there writes a file.
    let legacy_json = [
        concat!("index-state", ".json"),
        concat!("index-catch-up", ".json"),
    ];
    for (name, source) in modules {
        if name != "legacy.rs" {
            for file in legacy_json {
                assert!(!source.contains(file), "{name} names {file}");
            }
        }
    }
    let legacy = include_str!("../legacy.rs");
    for write in ["fs::write", "atomic_write", "File::create", ".create(true)"] {
        assert!(!legacy.contains(write), "legacy.rs writes through {write}");
    }
}

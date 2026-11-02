use super::*;

#[test]
fn an_event_id_cannot_replace_different_indexed_content() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let original = activity(
        "00000000-0000-4000-8000-000000000001",
        "7",
        "Original",
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&original, &[]).unwrap();

    let mut forged = original.clone();
    forged.title = Some("Forged retry".into());
    assert!(index.record_activity(&forged, &[]).is_err());

    assert_eq!(
        sql::sql_rows(&index.database_path(), "select title from artifacts")[0],
        [sql::Value::Text("Original".into())]
    );
}

#[test]
fn latest_metadata_uses_instant_order_across_offsets() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let early = activity(
        "00000000-0000-4000-8000-000000000999",
        "OFFSET",
        "Earlier",
        "2026-10-16T01:00:00Z",
    );
    let mut later = activity(
        "00000000-0000-4000-8000-000000000001",
        "OFFSET",
        "Later",
        "2026-10-15T23:00:00-07:00",
    );
    later.modes[0].mode = "edit".into();
    index.record_activities([&early, &later], &[]).unwrap();
    index.regenerate_dirty().unwrap();

    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert!(markdown.contains("title: \"Later\""));
    assert!(markdown.contains("  last_mode: \"edit\""));
}

#[test]
fn passive_activity_does_not_invent_an_open_or_erase_the_last_mode() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let opened = activity(
        "00000000-0000-4000-8000-000000000001",
        "CARRY",
        "Opened",
        "2026-10-15T23:00:00Z",
    );
    let mut carried = activity(
        "00000000-0000-4000-8000-000000000002",
        "CARRY",
        "Still open",
        "2026-10-16T00:00:00Z",
    );
    carried.opens = 0;
    carried.modes.clear();
    index.record_activities([&opened, &carried], &[]).unwrap();

    let derived = derive(&index.load_or_rebuild().unwrap(), None);
    let row = derived.artifacts.get(&opened.artifact_id).unwrap();
    assert_eq!(row.days.len(), 1);
    assert_eq!(row.last_mode.as_deref(), Some("view"));
    assert_eq!(row.title, "Still open");
}

#[test]
fn a_year_of_daily_opens_has_a_bounded_timeline() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let start = Date::from_calendar_date(2025, time::Month::October, 1).unwrap();
    let mut events = Vec::new();
    for day in 0..365 {
        let date = start + time::Duration::days(day);
        events.push(activity(
            &format!("00000000-0000-4000-8000-{day:012}"),
            "YEAR",
            "Year",
            &format!("{date}T12:00:00Z"),
        ));
    }
    index.record_activities(events.iter(), &[]).unwrap();
    index.regenerate_dirty().unwrap();
    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    let lines = markdown
        .lines()
        .filter(|line| line.starts_with("- "))
        .count();
    assert!(lines <= 48, "unbounded Timeline had {lines} lines");
    assert_eq!(
        markdown.lines().find(|line| line.starts_with("- ")),
        Some("- 2026-09-30 · focused 2 min · 1 min in background · opened 1×")
    );
    assert!(markdown.contains("- 2025-10"));
    assert!(markdown.contains("- 2026-09-30"));
    assert!(
        fs::metadata(index.database_path()).unwrap().len() > (5 * SQLITE_PAGE_SIZE) as u64,
        "the SQLite fixture did not exercise an interior B-tree page"
    );
    assert_eq!(
        sql::sql_rows(&index.database_path(), "select count(*) from visits"),
        [[sql::Value::Integer(365)]]
    );
}

#[test]
fn old_unknown_trail_transitions_preserve_activity_but_never_become_edges() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let source = activity(
        "00000000-0000-4000-8000-000000000021",
        "SOURCE",
        "Source",
        "2026-10-15T12:00:00Z",
    );
    let mut target = activity(
        "00000000-0000-4000-8000-000000000022",
        "TARGET",
        "Target",
        "2026-10-15T13:00:00Z",
    );
    target.trails = ["link", "\u{202e}unknown"]
        .into_iter()
        .map(|transition| ActivityTrail {
            from_artifact_id: Some(source.artifact_id.clone()),
            from_url: Some(source.url.clone()),
            transition: transition.into(),
        })
        .collect();
    let marker = |event: &ActivityEvent| {
        format!(
            "{MARKER_PREFIX}{} url={} at={} kind=activity data={} -->\n",
            sha256_hex(event.event_id.as_bytes()),
            sha256_hex(event.url.as_bytes()),
            event.occurred_at,
            hex::encode(serde_json::to_vec(event).unwrap()),
        )
    };
    let log = format!("{LOG_HEADER}\n{}{}", marker(&source), marker(&target));
    let logs = root.path().join("log/2026/10");
    fs::create_dir_all(&logs).unwrap();
    let log_path = logs.join("2026-10-15.md");
    fs::write(&log_path, &log).unwrap();

    // Simulate an ingestion cache from before transition validation.
    let mut state = index.empty_state();
    for event in [&source, &target] {
        state.events.insert(
            event.event_id.clone(),
            EventData::Activity(event.clone()).into(),
        );
        state.dirty.insert(event.artifact_id.clone());
    }
    index.publish_state_and_database(&state).unwrap();
    let check = || {
        let artifact = index.artifact(&target.artifact_id).unwrap().unwrap();
        assert_eq!(artifact.opens, 1);
        assert_eq!(artifact.focused_seconds, 125);
        assert_eq!(artifact.background_seconds, 61);
        assert_eq!(artifact.linked_count, 1);
        assert_eq!(artifact.incoming_trails.len(), 1);
        assert_eq!(artifact.incoming_trails[0].transition, "link");
        assert_eq!(
            sql::sql_rows(&index.database_path(), "select count(*) from visits"),
            [[sql::Value::Integer(2)]]
        );
        assert_eq!(
            sql::sql_rows(&index.database_path(), "select kind from edges"),
            [[sql::Value::Text("link".into())]]
        );
        // Do not rewrite historical payloads: their exact content detects
        // conflicting reuse of an event ID during replay and rebuilding.
        assert!(
            index.load_or_rebuild().unwrap().events[&target.event_id]
                == EventData::Activity(target.clone()).into()
        );
    };
    check();
    assert_eq!(index.reindex().unwrap().written, 2);
    check();
    assert_eq!(fs::read_to_string(log_path).unwrap(), log);

    // Legacy tolerance must keep the original size and paired-source caps.
    let mut malformed = target.clone();
    malformed.trails[0].transition = "x".repeat(65);
    assert!(parse_log(&marker(&malformed)).is_empty());
    malformed = target.clone();
    malformed.trails[0].from_url = None;
    assert!(parse_log(&marker(&malformed)).is_empty());
    malformed = target.clone();
    malformed.trails = vec![target.trails[0].clone(); 11];
    assert!(parse_log(&marker(&malformed)).is_empty());
}

#[test]
fn latest_metadata_follows_time_and_log_order_not_event_id_order() {
    // A passive later hour (no title after a reload, no mode) whose
    // random event ID sorts first must not hide the earlier hour's title
    // and mode, in either ID order.
    for (opened_id, passive_id) in [
        (
            "ffffffff-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000002",
        ),
        (
            "00000000-0000-4000-8000-000000000001",
            "ffffffff-0000-4000-8000-000000000002",
        ),
    ] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let mut opened = activity(opened_id, "SPEC", "Spec", "2026-10-15T10:00:00Z");
        opened.modes[0].mode = "edit".into();
        let mut passive = activity(passive_id, "SPEC", "ignored", "2026-10-15T11:00:00Z");
        passive.title = None;
        passive.opens = 0;
        passive.modes.clear();
        write_log(root.path(), "2026-10-15.md", &[&opened, &passive]);
        let view = index.artifact(&opened.artifact_id).unwrap().unwrap();
        assert_eq!(view.last_mode.as_deref(), Some("edit"), "{opened_id}");
        assert_eq!(view.title, "Spec", "{opened_id}");
    }

    // Context is saved with one-second timestamps, so two saves in the
    // same second keep the one appended last, whatever their IDs.
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-000000000001",
        "WHY",
        "Why",
        "2026-10-15T10:00:00Z",
    );
    index.record_activity(&event, &[]).unwrap();
    for (id, text) in [
        ("ffffffff-0000-4000-8000-000000000001", "first"),
        ("00000000-0000-4000-8000-000000000003", "second"),
    ] {
        let why = WhyEvent {
            event_id: id.into(),
            artifact_id: event.artifact_id.clone(),
            url: "https://example.com/items/WHY".into(),
            occurred_at: "2026-10-15T10:30:00Z".into(),
            incognito: false,
            text: text.into(),
        };
        index.record_why(&why, &[]).unwrap();
    }
    assert_eq!(
        index.latest_why(&event.artifact_id).unwrap().as_deref(),
        Some("second")
    );
    assert_eq!(
        index
            .artifact(&event.artifact_id)
            .unwrap()
            .unwrap()
            .why
            .as_deref(),
        Some("second")
    );
}

#[test]
fn equal_instants_across_logs_order_by_log_position_not_discovery() {
    let why = |id: &str, at: &str, text: &str| WhyEvent {
        event_id: id.into(),
        artifact_id: "work:example.com/TIE".into(),
        url: "https://example.com/items/TIE".into(),
        occurred_at: at.into(),
        incognito: false,
        text: text.into(),
    };
    let marker = |event: &WhyEvent| {
        format!(
            "{MARKER_PREFIX}{} url={} at={} kind=why data={} -->\n",
            sha256_hex(event.event_id.as_bytes()),
            sha256_hex(event.url.as_bytes()),
            event.occurred_at,
            hex::encode(serde_json::to_vec(event).unwrap())
        )
    };
    // One instant written with two offsets lands in two date files.
    let early = why(
        "00000000-0000-4000-8000-000000000001",
        "2026-10-15T23:30:00-02:00",
        "early path",
    );
    let late = why(
        "00000000-0000-4000-8000-000000000002",
        "2026-10-16T01:30:00Z",
        "late path",
    );
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let dir = root.path().join("log");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("2026-10-16.md"),
        format!("{LOG_HEADER}\n{}", marker(&late)),
    )
    .unwrap();
    assert_eq!(
        index.latest_why(&late.artifact_id).unwrap().as_deref(),
        Some("late path")
    );
    // The earlier-sorting file is discovered later.
    fs::write(
        dir.join("2026-10-15.md"),
        format!("{LOG_HEADER}\n{}", marker(&early)),
    )
    .unwrap();
    let incremental = index.latest_why(&late.artifact_id).unwrap();
    fs::remove_file(index.state_path()).unwrap();
    let rebuilt = index.latest_why(&late.artifact_id).unwrap();
    assert_eq!(incremental, rebuilt);
    assert_eq!(rebuilt.as_deref(), Some("late path"));
}

#[test]
fn a_torn_multibyte_line_keeps_the_rest_of_that_days_log() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let before = activity(
        "00000000-0000-4000-8000-000000000001",
        "BEFORE",
        "Before",
        "2026-10-15T10:00:00Z",
    );
    let after = activity(
        "00000000-0000-4000-8000-000000000002",
        "AFTER",
        "After",
        "2026-10-15T11:00:00Z",
    );
    let path = write_log(root.path(), "2026-10-15.md", &[&before]);
    // A crash tore an append inside an em dash; capture keeps appending.
    let mut file = StdOpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"- 10:30 \xe2\x80\n").unwrap();
    file.write_all(activity_marker(&after).as_bytes()).unwrap();
    drop(file);
    assert!(index.artifact(&before.artifact_id).unwrap().is_some());
    assert!(index.artifact(&after.artifact_id).unwrap().is_some());
    assert_eq!(index.reindex().unwrap().written, 2);
}

// This fixture removes POSIX read permission bits; Windows uses ACLs.
#[cfg(unix)]
#[test]
fn an_unreadable_log_names_its_path() {
    use std::os::unix::fs::PermissionsExt;
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-000000000001",
        "LOCKED",
        "Locked",
        "2026-10-15T10:00:00Z",
    );
    let path = write_log(root.path(), "2026-10-15.md", &[&event]);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let error = format!("{:#}", index.reindex().unwrap_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(error.contains("2026-10-15.md"), "{error}");
}

#[test]
fn a_reused_event_id_keeps_the_earliest_durable_event_and_the_rest_of_history() {
    let original = activity(
        "00000000-0000-4000-8000-000000000010",
        "REUSED",
        "Original",
        "2026-10-24T12:00:00Z",
    );
    let mut forged = original.clone();
    forged.title = Some("Forged".into());
    forged.occurred_at = "2026-10-25T12:00:00Z".into();
    let mut same_time = original.clone();
    same_time.title = Some("Also forged".into());
    // A later marker claiming an earlier time must not win either.
    let mut backdated = original.clone();
    backdated.title = Some("Backdated".into());
    backdated.occurred_at = "2026-10-23T12:00:00Z".into();
    let other = activity(
        "00000000-0000-4000-8000-000000000011",
        "OTHER",
        "Other",
        "2026-10-25T13:00:00Z",
    );
    // Full rebuilds and incremental catch-up agree on the winner.
    for incremental in [false, true] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        write_log(root.path(), "2026-10-24.md", &[&original]);
        if incremental {
            assert!(index.artifact(&original.artifact_id).unwrap().is_some());
        }
        write_log(
            root.path(),
            "2026-10-25.md",
            &[&forged, &same_time, &backdated, &other],
        );
        if !incremental {
            index.reindex().unwrap();
        }
        let view = index.artifact(&original.artifact_id).unwrap().unwrap();
        assert_eq!(view.title, "Original", "incremental={incremental}");
        assert!(index.artifact(&other.artifact_id).unwrap().is_some());
        assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 2);
        // A later delivery backdated into an earlier-sorting log never
        // replaces the body already indexed.
        write_log(root.path(), "2026-10-23.md", &[&backdated]);
        let view = index.artifact(&original.artifact_id).unwrap().unwrap();
        assert_eq!(view.title, "Original", "incremental={incremental}");
        assert!(index.record_activity(&backdated, &[]).is_err());
        let view = index.artifact(&original.artifact_id).unwrap().unwrap();
        assert_eq!(view.title, "Original", "incremental={incremental}");
        // Rescans keep the winner a valid cache already recorded.
        index.reconcile_from_logs().unwrap();
        let view = index.artifact(&original.artifact_id).unwrap().unwrap();
        assert_eq!(view.title, "Original", "incremental={incremental}");
        index.reindex().unwrap();
        let view = index.artifact(&original.artifact_id).unwrap().unwrap();
        assert_eq!(view.title, "Original", "incremental={incremental}");
    }
}

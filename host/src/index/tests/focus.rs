use super::*;
use brauser_protocol::FocusSession;

fn tagged(n: u32, native_id: &str, session: u32, name: &str, revision: u32) -> ActivityEvent {
    let mut event = activity(
        &format!("00000000-0000-4000-8000-{n:012}"),
        native_id,
        "Review",
        "2026-11-03T10:00:00Z",
    );
    event.focus_session = Some(FocusSession {
        id: format!("10000000-0000-4000-8000-{session:012}"),
        name: name.into(),
        revision,
    });
    event
}

#[test]
fn focus_worklog_groups_direct_activity_names_and_rebuilds_from_real_logs() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let events = [
        tagged(1, "A", 1, "Release", 1),
        tagged(2, "B", 1, "Release", 1),
        tagged(3, "A", 1, "Ship it", 2),
        tagged(4, "A", 2, "Incident", 1),
    ];
    write_log(
        root.path(),
        "2026-11-03.md",
        &events.iter().collect::<Vec<_>>(),
    );
    let day = parse_date("2026-11-03").unwrap();
    let index = index(&root, &cache);
    let report = index.worklog_report(day, day).unwrap();
    assert_eq!(
        report.1, 2,
        "session sections do not add artifacts to the range total"
    );
    let (main, groups) = report.0.split_once("Focus sessions").unwrap();
    assert_eq!(main.matches("work.item work:example.com/A:").count(), 1);
    assert!(groups.contains("Focus: Ship it (name changed in this range)"));
    assert!(groups.contains("Focus: Incident"));
    assert_eq!(groups.matches("work.item work:example.com/A:").count(), 2);
    assert_eq!(groups.matches("work.item work:example.com/B:").count(), 1);
    let clean_cache = TempDir::new().unwrap();
    let rebuilt = ArtifactIndex::open_in(&storage(root.path()), None, clean_cache.path()).unwrap();
    assert_eq!(report.0, rebuilt.worklog_report(day, day).unwrap().0);
    let yesterday = parse_date("2026-11-02").unwrap();
    assert!(
        !rebuilt
            .worklog_report(yesterday, yesterday)
            .unwrap()
            .0
            .contains("Focus:")
    );
}

#[test]
fn focus_groups_use_resolved_keys_in_sqlite_and_relearn_projection() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let event = tagged(1, "A", 1, "Release", 1);
    write_log(root.path(), "2026-11-03.md", &[&event]);
    let resolution = Resolution::from_events(&[crate::relearn::aliases::Event {
        event_id: Uuid::new_v4().to_string(),
        action: crate::relearn::aliases::Action::Confirm,
        from: crate::relearn::Identity {
            artifact_type: "work.item".into(),
            artifact_id: "work:example.com/A".into(),
        },
        to: crate::relearn::Identity {
            artifact_type: "work.item".into(),
            artifact_id: "work:example.com/B".into(),
        },
    }])
    .unwrap();
    let index = index(&root, &cache);
    let (revision, _) = index.relearn_evidence().unwrap();
    let projected =
        serde_json::to_value(index.relearn_projection(&resolution, &revision).unwrap()).unwrap();
    let text = projected["worklog"]["text"].as_str().unwrap();
    let groups = text.split_once("Focus sessions").unwrap().1;
    assert!(groups.contains("work:example.com/B:"));
    assert!(!groups.contains("work:example.com/A:"));
    // Projection and ordinary reads share the same group renderer.
    let plain = serde_json::to_value(
        index
            .relearn_projection(&Resolution::default(), &revision)
            .unwrap(),
    )
    .unwrap();
    let day = parse_date("2026-11-03").unwrap();
    assert_eq!(
        plain["worklog"]["text"].as_str().unwrap(),
        index.worklog_report(day, day).unwrap().0
    );
}

#[test]
fn focus_breakdown_is_bounded_and_reports_omissions() {
    let mut groups = crate::index::focus::FocusGroups::default();
    for n in 1..=300 {
        groups.add(
            &format!("work:example.com/{n}"),
            &tagged(n, "A", n, "Research 😀", 1),
        );
    }
    let mut text = "Opened 300 artifacts.".to_owned();
    groups.append(&mut text);
    assert!(text.len() <= MAX_WORKLOG_BYTES);
    assert!(text.ends_with("Focus-session breakdown shortened."));
    assert_eq!(text.matches("\nFocus:").count(), 64);
}

#[test]
fn a_full_base_worklog_still_reports_an_omitted_focus_breakdown() {
    let mut groups = crate::index::focus::FocusGroups::default();
    groups.add("work:example.com/A", &tagged(1, "A", 1, "Release", 1));
    let mut rows = BTreeMap::new();
    for n in 0..300 {
        rows.insert(
            format!("work:example.com/{n}"),
            super::super::query::WorklogRow {
                artifact: Some(("work.item".into(), "😀".repeat(500), None)),
                focused: 125,
                own_focused: 125,
                ..Default::default()
            },
        );
    }
    let (mut text, _) = super::super::query::worklog_text(rows);
    // The main renderer's cap is almost full; a session section must not
    // disappear merely because the main artifact list consumed its space.
    text.extend(std::iter::repeat_n(
        'x',
        MAX_WORKLOG_BYTES.saturating_sub(text.len()),
    ));
    groups.append(&mut text);
    assert!(text.len() <= MAX_WORKLOG_BYTES);
    assert!(text.contains("Focus-session breakdown shortened."));
}

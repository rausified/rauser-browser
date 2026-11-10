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

#[test]
fn selected_session_filters_before_aggregating_time_titles_urls_and_summaries() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let mut public = tagged(1, "A", 1, "Release", 1);
    public.title = Some("Public update".into());
    public.url.push_str("?view=public");
    let mut private = tagged(2, "A", 2, "Private", 1);
    private.title = Some("Private topic".into());
    private.url.push_str("?view=private");
    private.focused_seconds = 600;
    private.occurred_at = "2026-11-03T11:00:00Z".into();
    let mut untagged = private.clone();
    untagged.event_id = Uuid::new_v4().to_string();
    untagged.focus_session = None;
    untagged.title = Some("Unrelated work".into());
    let mut renamed = tagged(3, "A", 1, "Ship it", 2);
    renamed.title = Some("Public wrap-up".into());
    renamed.url.push_str("?view=wrap-up");
    renamed.occurred_at = "2026-11-03T12:00:00Z".into();
    // Reuse the same name for another ID; names are never selection keys.
    private.focus_session.as_mut().unwrap().name = "Ship it".into();
    write_log(
        root.path(),
        "2026-11-03.md",
        &[&public, &private, &untagged, &renamed],
    );
    let index = index(&root, &cache);
    let day = parse_date("2026-11-03").unwrap();
    let id = &public.focus_session.as_ref().unwrap().id;
    let list = index.worklog_focus_choices(day, day, None).unwrap();
    assert_eq!(list.sessions.len(), 2);
    assert!(list.choices.is_empty());
    assert!(list.selected_session.is_none());
    let selected = index.worklog_focus_choices(day, day, Some(id)).unwrap();
    assert_eq!(selected.selected_session.as_ref().unwrap().name, "Ship it");
    assert!(selected.name_changed);
    assert_eq!(selected.choices.len(), 1);
    let choice = &selected.choices[0];
    assert_eq!(choice.artifact_id, "work:example.com/A");
    assert!(
        choice.text.contains("Public wrap-up — focused 4 min"),
        "{}",
        choice.text
    );
    assert!(!choice.text.contains("Private"));
    assert!(!choice.text.contains("Unrelated"));
    assert_eq!(choice.url, renamed.url);
    assert_eq!(index.worklog_choices(day, day).unwrap().0.len(), 1);
    let rebuilt_cache = TempDir::new().unwrap();
    let rebuilt =
        ArtifactIndex::open_in(&storage(root.path()), None, rebuilt_cache.path()).unwrap();
    assert_eq!(
        selected.choices,
        rebuilt
            .worklog_focus_choices(day, day, Some(id))
            .unwrap()
            .choices
    );
}

#[test]
fn selected_session_is_date_scoped_and_excludes_summary_and_container_copies() {
    use super::containers::github_blob;
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let mut event = github_pr(1, 7, "2026-11-03T10:00:00Z");
    event.focus_session = tagged(1, "A", 1, "Release", 1).focus_session;
    let mut repo = github_blob(2, "2026-11-03T11:00:00Z");
    repo.focus_session = tagged(2, "A", 2, "Other", 1).focus_session;
    let mut tomorrow = event.clone();
    tomorrow.event_id = Uuid::new_v4().to_string();
    tomorrow.occurred_at = "2026-11-04T10:00:00Z".into();
    tomorrow.title = Some("Next day".into());
    index
        .record_activities([&event, &repo, &tomorrow], &[])
        .unwrap();
    index
        .record_summary(&super::summaries::web_summary(
            "https://news.example/a",
            "2026-11-03T12:00:00Z",
        ))
        .unwrap();
    let day = parse_date("2026-11-03").unwrap();
    let id = &event.focus_session.as_ref().unwrap().id;
    let selected = index.worklog_focus_choices(day, day, Some(id)).unwrap();
    assert_eq!(selected.choices.len(), 1);
    assert_eq!(selected.choices[0].artifact_id, event.artifact_id);
    assert!(!selected.choices[0].text.contains("Next day"));
    assert!(!selected.choices[0].text.contains("Summarized"));
    let week = index
        .worklog_focus_choices(day, day.next_day().unwrap(), Some(id))
        .unwrap();
    assert_eq!(week.choices.len(), 2);
    for selected_day in [
        parse_date("2026-11-02").unwrap(),
        parse_date("2026-11-05").unwrap(),
    ] {
        let empty = index
            .worklog_focus_choices(selected_day, selected_day, Some(id))
            .unwrap();
        assert!(empty.sessions.is_empty());
        assert!(empty.selected_session.is_none());
        assert!(empty.choices.is_empty());
    }
    let unknown = index
        .worklog_focus_choices(day, day, Some("10000000-0000-4000-8000-999999999999"))
        .unwrap();
    assert_eq!(unknown.sessions.len(), 2);
    assert!(unknown.selected_session.is_none());
    assert!(unknown.choices.is_empty());
}

#[test]
fn session_catalog_omission_never_limits_a_selected_sessions_entries() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let events = (1..=70)
        .map(|n| tagged(n, &format!("A{n}"), n, "Same name", 1))
        .collect::<Vec<_>>();
    index.record_activities(events.iter(), &[]).unwrap();
    let day = parse_date("2026-11-03").unwrap();
    let catalog = index.worklog_focus_choices(day, day, None).unwrap();
    assert_eq!(catalog.sessions.len(), 64);
    assert!(catalog.sessions_truncated);
    let missing = events
        .iter()
        .find(|event| {
            !catalog
                .sessions
                .iter()
                .any(|s| Some(s) == event.focus_session.as_ref())
        })
        .unwrap();
    let selected = index
        .worklog_focus_choices(day, day, Some(&missing.focus_session.as_ref().unwrap().id))
        .unwrap();
    assert!(selected.selected_session.is_some());
    assert_eq!(selected.choices.len(), 1);
    assert_eq!(selected.choices[0].artifact_id, missing.artifact_id);
    assert!(!selected.truncated);
}

#[test]
fn session_catalog_pages_all_tied_sessions_and_preserves_selected_direct_work() {
    use brauser_protocol::WorklogChoicesPageRequest;
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut events = (1..=140)
        .map(|n| tagged(n, &format!("A{n}"), n, "Same name", 1))
        .collect::<Vec<_>>();
    // One session with several snapshots must occupy exactly one position;
    // its highest revision supplies the name even on an older event.
    events.push(tagged(141, "A1", 1, "Renamed", 3));
    index.record_activities(events.iter(), &[]).unwrap();
    let day = parse_date("2026-11-03").unwrap();
    let mut request = WorklogChoicesPageRequest {
        offset: 0,
        snapshot: None,
    };
    let first = index
        .worklog_focus_choices_page(day, day, None, &request)
        .unwrap();
    assert_eq!(first.sessions.len(), 64);
    assert!(first.choices.is_empty());
    let first_page = first.sessions_page.as_ref().unwrap();
    let mut sessions = first.sessions.clone();
    request.snapshot = Some(first_page.snapshot.clone());
    request.offset = first_page.next_offset.unwrap();
    loop {
        let page = index
            .worklog_focus_choices_page(day, day, None, &request)
            .unwrap();
        sessions.extend(page.sessions);
        let info = page.sessions_page.unwrap();
        assert!(!info.omitted);
        assert_eq!(page.sessions_truncated, info.next_offset.is_some());
        let Some(next) = info.next_offset else { break };
        request.offset = next;
    }
    assert_eq!(sessions.len(), 140);
    assert_eq!(
        sessions
            .iter()
            .map(|s| &s.id)
            .collect::<BTreeSet<_>>()
            .len(),
        140
    );
    assert_eq!(
        sessions
            .iter()
            .find(|s| s.id.ends_with("000000000001"))
            .unwrap()
            .name,
        "Renamed"
    );
    let selected_id = &sessions.last().unwrap().id;
    let selected = index
        .worklog_focus_choices_page(day, day, Some(selected_id), &request)
        .unwrap();
    let legacy = index
        .worklog_focus_choices(day, day, Some(selected_id))
        .unwrap();
    assert_eq!(selected.choices, legacy.choices);
    assert_eq!(selected.selected_session, legacy.selected_session);
    assert_eq!(selected.choices.len(), 1);
    assert_eq!(
        selected.sessions_page.unwrap().snapshot,
        first_page.snapshot
    );
    request.offset = 140;
    assert!(
        index
            .worklog_focus_choices_page(day, day, None, &request)
            .unwrap()
            .sessions
            .is_empty()
    );
    request.offset = 141;
    assert!(
        index
            .worklog_focus_choices_page(day, day, None, &request)
            .err()
            .unwrap()
            .is::<StaleWorklogPage>()
    );
}

#[test]
fn session_continuations_bind_history_home_dates_and_survive_cache_rebuild() {
    use brauser_protocol::WorklogChoicesPageRequest;
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let events = (1..=70)
        .map(|n| tagged(n, "A", n, "Same", 1))
        .collect::<Vec<_>>();
    write_log(
        root.path(),
        "2026-11-03.md",
        &events.iter().collect::<Vec<_>>(),
    );
    let index = index(&root, &cache);
    let day = parse_date("2026-11-03").unwrap();
    let first = index
        .worklog_focus_choices_page(
            day,
            day,
            None,
            &WorklogChoicesPageRequest {
                offset: 0,
                snapshot: None,
            },
        )
        .unwrap();
    let info = first.sessions_page.unwrap();
    let request = WorklogChoicesPageRequest {
        offset: 64,
        snapshot: Some(info.snapshot),
    };
    let rebuilt_cache = TempDir::new().unwrap();
    let rebuilt =
        ArtifactIndex::open_in(&storage(root.path()), None, rebuilt_cache.path()).unwrap();
    let tail = rebuilt
        .worklog_focus_choices_page(day, day, None, &request)
        .unwrap();
    assert_eq!(tail.sessions.len(), 6);
    assert_eq!(
        tail.sessions,
        index
            .worklog_focus_choices_page(day, day, None, &request)
            .unwrap()
            .sessions
    );
    assert!(
        index
            .worklog_focus_choices_page(day.previous_day().unwrap(), day, None, &request)
            .is_err()
    );
    let other = TempDir::new().unwrap();
    write_log(
        other.path(),
        "2026-11-03.md",
        &events.iter().collect::<Vec<_>>(),
    );
    let other_cache = TempDir::new().unwrap();
    let other_index =
        ArtifactIndex::open_in(&storage(other.path()), None, other_cache.path()).unwrap();
    assert!(
        other_index
            .worklog_focus_choices_page(day, day, None, &request)
            .is_err()
    );
    // Changed measures in an existing session must invalidate a catalog,
    // even though the session list and names themselves are identical.
    index
        .record_activities([&tagged(71, "A", 1, "Same", 1)], &[])
        .unwrap();
    assert!(
        index
            .worklog_focus_choices_page(day, day, None, &request)
            .is_err()
    );
    for bad in [
        WorklogChoicesPageRequest {
            offset: 1,
            snapshot: None,
        },
        WorklogChoicesPageRequest {
            offset: 0,
            snapshot: Some("x".repeat(64)),
        },
    ] {
        assert!(
            index
                .worklog_focus_choices_page(day, day, None, &bad)
                .is_err()
        );
    }
}

#[test]
fn selected_session_choices_reuse_row_and_escaped_byte_bounds() {
    for (count, title) in [(205, "Brief".to_owned()), (200, "\"é".repeat(250))] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let events = (1..=count)
            .map(|n| {
                let mut event = tagged(n, &format!("A{n:04}"), 1, "Release", 1);
                event.title = Some(title.clone());
                event
            })
            .collect::<Vec<_>>();
        index.record_activities(events.iter(), &[]).unwrap();
        let day = parse_date("2026-11-03").unwrap();
        let selected = index
            .worklog_focus_choices(
                day,
                day,
                Some(&events[0].focus_session.as_ref().unwrap().id),
            )
            .unwrap();
        assert!(selected.truncated);
        assert!(selected.choices.len() <= 200);
        assert!(serde_json::to_vec(&selected.choices).unwrap().len() <= MAX_WORKLOG_BYTES);
        assert_eq!(
            selected.choices,
            index
                .worklog_focus_choices(
                    day,
                    day,
                    Some(&events[0].focus_session.as_ref().unwrap().id)
                )
                .unwrap()
                .choices
        );
    }
}

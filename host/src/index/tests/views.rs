use super::*;

#[test]
fn artifact_urls_include_mode_evidence_urls() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let edit_url = "https://docs.example/d/abc/edit";
    let view_url = "https://docs.example/d/abc/view";
    let event = ActivityEvent {
        focus_session: None,
        event_id: "00000000-0000-4000-8000-000000000003".into(),
        artifact_id: "gdoc:docs.example/abc".into(),
        artifact_type: "gdoc".into(),
        url: view_url.into(),
        title: Some("Doc".into()),
        occurred_at: "2026-10-16T12:00:00+00:00".into(),
        incognito: false,
        opens: 1,
        focused_seconds: 30,
        background_seconds: 0,
        modes: vec![ActivityMode {
            mode: "edit".into(),
            url: edit_url.into(),
        }],
        trails: Vec::new(),
        refs: Vec::new(),
    };
    index.record_activity(&event, &[]).unwrap();

    assert_eq!(
        index.artifact_urls("gdoc:docs.example/abc").unwrap(),
        vec![edit_url.to_string(), view_url.to_string()]
    );
}

#[test]
fn recent_and_worklog_are_measured_bounded_views_of_the_index() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let first = activity(
        "00000000-0000-4000-8000-000000000001",
        "ABC",
        "Release blocker",
        "2026-10-15T12:00:00-04:00",
    );
    let second = activity(
        "00000000-0000-4000-8000-000000000002",
        "XYZ",
        "Design notes",
        "2026-10-16T09:00:00-04:00",
    );
    index.record_activities([&first, &second], &[]).unwrap();
    index
        .record_why(
            &WhyEvent {
                event_id: "00000000-0000-4000-8000-000000000003".into(),
                artifact_id: first.artifact_id.clone(),
                url: first.url.clone(),
                occurred_at: "2026-10-16T10:00:00-04:00".into(),
                incognito: false,
                text: "blocking the release".into(),
            },
            &[],
        )
        .unwrap();

    let (recent, types, truncated) = index.recent(None, Some("release"), 200).unwrap();
    assert_eq!(types, ["work.item"]);
    assert!(!truncated);
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].local_date, "2026-10-15");
    assert_eq!(recent[0].focused_seconds, 125);
    assert_eq!(recent[0].why.as_deref(), Some("blocking the release"));

    let (text, count) = index
        .worklog(
            parse_date("2026-10-15").unwrap(),
            parse_date("2026-10-16").unwrap(),
        )
        .unwrap();
    assert_eq!(count, 2);
    assert!(text.starts_with("Opened 2 artifacts"));
    assert!(text.contains("Release blocker"));
    assert!(text.contains("Why: blocking the release"));
}

#[test]
fn incoming_trails_only_show_known_navigation_sources() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let source = activity(
        &Uuid::new_v4().to_string(),
        "SOURCE",
        "<img src=x onerror=alert(1)>",
        "2026-10-15T12:00:00Z",
    );
    index.record_activities([&source], &[]).unwrap();
    for transition in [
        "link",
        "typed",
        "auto_bookmark",
        "auto_subframe",
        "manual_subframe",
        "generated",
        "start_page",
        "form_submit",
        "reload",
        "keyword",
        "keyword_generated",
        "forward_back",
    ] {
        let mut target = activity(
            &Uuid::new_v4().to_string(),
            transition,
            "Target mentions SOURCE",
            "2026-10-16T13:00:00Z",
        );
        target.trails = vec![
            ActivityTrail {
                from_artifact_id: Some(source.artifact_id.clone()),
                from_url: Some(source.url.clone()),
                transition: transition.into(),
            },
            ActivityTrail {
                from_artifact_id: Some("work:example.com/MISSING".into()),
                from_url: Some("https://example.com/items/MISSING".into()),
                transition: "link".into(),
            },
            ActivityTrail {
                from_artifact_id: Some(target.artifact_id.clone()),
                from_url: Some(target.url.clone()),
                transition: "link".into(),
            },
            ActivityTrail {
                from_artifact_id: None,
                from_url: None,
                transition: "forward_back".into(),
            },
        ];
        index.record_activities([&target], &[]).unwrap();
        let incoming = index
            .artifact(&target.artifact_id)
            .unwrap()
            .unwrap()
            .incoming_trails;
        if transition == "forward_back" {
            assert!(incoming.is_empty(), "{transition}");
        } else {
            assert_eq!(incoming.len(), 1, "{transition}");
            assert_eq!(incoming[0].transition, transition);
            assert_eq!(incoming[0].source.artifact_id, source.artifact_id);
            assert_eq!(
                incoming[0].source.title,
                source.title.as_ref().unwrap().as_str()
            );
            assert_eq!(incoming[0].last_seen, "2026-10-16");
        }
    }
    assert!(
        index
            .artifact(&source.artifact_id)
            .unwrap()
            .unwrap()
            .incoming_trails
            .is_empty()
    );
}

#[test]
fn incoming_trails_are_bounded_sorted_and_deduplicated_after_rebuild() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut sources = Vec::new();
    let mut targets = Vec::new();
    for day in 10..18 {
        let at = format!("2026-10-{day}T12:00:00Z");
        let source = activity(
            &Uuid::new_v4().to_string(),
            &format!("S{day}"),
            "Source",
            &at,
        );
        let mut target = activity(&Uuid::new_v4().to_string(), "TARGET", "Target", &at);
        target.trails.push(ActivityTrail {
            from_artifact_id: Some(source.artifact_id.clone()),
            from_url: Some(source.url.clone()),
            transition: "link".into(),
        });
        sources.push(source);
        targets.push(target);
    }
    // Repeated source/transition observations are still one incoming row.
    let mut repeated = targets.last().unwrap().clone();
    repeated.event_id = Uuid::new_v4().to_string();
    targets.push(repeated);
    index
        .record_activities(sources.iter().chain(targets.iter()), &[])
        .unwrap();
    let check = || {
        let incoming = index
            .artifact("work:example.com/TARGET")
            .unwrap()
            .unwrap()
            .incoming_trails;
        assert_eq!(incoming.len(), MAX_INCOMING_TRAILS);
        assert_eq!(
            incoming
                .iter()
                .map(|trail| trail.last_seen.as_str())
                .collect::<Vec<_>>(),
            [
                "2026-10-17",
                "2026-10-16",
                "2026-10-15",
                "2026-10-14",
                "2026-10-13"
            ]
        );
        assert_eq!(incoming[0].source.artifact_id, "work:example.com/S17");
    };
    check();
    // Persist owned markers so reindex proves the same query from logs.
    let logs = root.path().join("log/2026/10");
    fs::create_dir_all(&logs).unwrap();
    let mut log = format!("{LOG_HEADER}\n");
    for event in sources.iter().chain(targets.iter()) {
        log.push_str(&format!(
            "{MARKER_PREFIX}{} url={} at={} kind=activity data={} -->\n",
            sha256_hex(event.event_id.as_bytes()),
            sha256_hex(event.url.as_bytes()),
            event.occurred_at,
            hex::encode(serde_json::to_vec(event).unwrap()),
        ));
    }
    fs::write(logs.join("2026-10-17.md"), log).unwrap();
    index.reindex().unwrap();
    check();
}

#[test]
fn linked_artifacts_are_distinct_and_unresolved_mentions_do_not_count() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let first = activity(
        "00000000-0000-4000-8000-000000000011",
        "ABC",
        "Source",
        "2026-10-15T12:00:00Z",
    );
    let mut second = activity(
        "00000000-0000-4000-8000-000000000012",
        "XYZ",
        "Target",
        "2026-10-15T13:00:00Z",
    );
    second.trails = ["link", "form_submit"]
        .into_iter()
        .map(|transition| ActivityTrail {
            from_artifact_id: Some(first.artifact_id.clone()),
            from_url: Some(first.url.clone()),
            transition: transition.into(),
        })
        .collect();
    second.refs.push(ActivityRef {
        artifact_type: "work.item".into(),
        value: "NOT-RECORDED".into(),
    });
    index.record_activities([&first, &second], &[]).unwrap();

    assert_eq!(
        index
            .artifact(&first.artifact_id)
            .unwrap()
            .unwrap()
            .linked_count,
        1
    );
    assert_eq!(
        index
            .artifact(&second.artifact_id)
            .unwrap()
            .unwrap()
            .linked_count,
        1
    );
}

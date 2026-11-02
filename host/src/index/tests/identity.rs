use super::*;

fn capture_ids_survive_reindex(kind: &str) {
    use crate::capture::{CaptureBatchEvent, CaptureOutcome, CapturePolicy, CaptureStore};

    let canonical = "abcdef01-2345-4678-9abc-def012345678";
    for raw_id in [
        canonical.to_owned(),
        canonical.to_uppercase(),
        canonical.to_uppercase().replace('-', ""),
    ] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let capture = CaptureStore::open_in(&storage(root.path()), None, cache.path()).unwrap();
        let origins = ["https://example.com".to_owned()];
        let policy = CapturePolicy {
            enabled: true,
            origins: &origins,
            strip_params: &[],
        };
        let event = activity(
            &raw_id,
            "ABC",
            "Keep this work",
            &time::OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        );
        let why = WhyEvent {
            event_id: raw_id.clone(),
            artifact_id: event.artifact_id.clone(),
            url: event.url.clone(),
            occurred_at: event.occurred_at.clone(),
            incognito: false,
            text: "Prepare the release".into(),
        };
        if kind == "why" {
            let seed = ActivityEvent {
                event_id: Uuid::new_v4().to_string(),
                ..event.clone()
            };
            assert!(matches!(
                capture.record_activity(policy, &seed),
                CaptureOutcome::Persisted { .. }
            ));
        }
        let outcome = match kind {
            "single" => capture.record_activity(policy, &event),
            "batch" => {
                let (reply_id, outcome) = capture
                    .record_batch(
                        policy,
                        &Uuid::new_v4().to_string(),
                        &[CaptureBatchEvent::Activity(&event)],
                        None,
                    )
                    .events
                    .remove(0);
                assert_eq!(reply_id, raw_id, "acknowledge the caller's spelling");
                outcome
            }
            "why" => capture.record_why(policy, &why),
            _ => unreachable!(),
        };
        let CaptureOutcome::Persisted { relative_path } = outcome else {
            panic!("{kind} {raw_id}: {outcome:?}");
        };
        let markdown = fs::read_to_string(root.path().join(relative_path)).unwrap();
        let index = index(&root, &cache);
        index.reindex().unwrap();
        let rebuilt = index.artifact(&event.artifact_id).unwrap().unwrap();
        if kind == "why" {
            assert_eq!(rebuilt.why.as_deref(), Some(why.text.as_str()));
        } else {
            assert_eq!(rebuilt.focused_seconds, event.focused_seconds);
            assert_eq!(rebuilt.opens, event.opens);
        }
        let marker_kind = if kind == "why" {
            " kind=why "
        } else {
            " kind=activity "
        };
        let marker = markdown
            .lines()
            .find(|line| line.starts_with(MARKER_PREFIX) && line.contains(marker_kind))
            .unwrap();
        let (prefix, data) = marker.split_once(" data=").unwrap();
        let mut payload: serde_json::Value =
            serde_json::from_slice(&hex::decode(data.split_whitespace().next().unwrap()).unwrap())
                .unwrap();
        assert_eq!(payload["event_id"], canonical, "{kind} {raw_id}");
        assert!(prefix.contains(&format!("id={}", sha256_hex(canonical.as_bytes()))));
        let expected_count = if kind == "why" { 2 } else { 1 };
        assert_eq!(parse_log(&markdown).len(), expected_count);

        // A different, valid UUID must still fail the marker's hash check.
        payload["event_id"] = Uuid::new_v4().to_string().into();
        let forged = format!(
            "{prefix} data={} -->",
            hex::encode(serde_json::to_vec(&payload).unwrap())
        );
        assert!(parse_marker_event(&forged, "").is_none());

        if kind == "why" {
            // The native host also indexes the original request after capture.
            index.record_why(&why, &[]).unwrap();
        } else {
            if kind == "batch" {
                index.record_activities([&event], &[]).unwrap();
            } else {
                index.record_activity(&event, &[]).unwrap();
            }
        }
        let state = index.load_current_state().unwrap().0;
        assert_eq!(
            state.events.len(),
            expected_count,
            "live updates must reuse the logged ID"
        );
        assert!(state.events.contains_key(canonical));
        index.reindex().unwrap();
        assert_eq!(
            index.artifact(&event.artifact_id).unwrap().unwrap(),
            rebuilt
        );
    }
}

#[test]
fn historical_noncanonical_uuid_markers_survive_reindex() {
    let canonical = "abcdef01-2345-4678-9abc-def012345678";
    for raw in [
        canonical.to_uppercase(),
        canonical.replace('-', ""),
        canonical.to_uppercase().replace('-', ""),
    ] {
        for why in [false, true] {
            let root = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let event = activity(
                &raw,
                "ABC",
                "Historical work",
                &crate::capture::fixtures::event_time()
                    .replace_minute(0)
                    .unwrap()
                    .replace_second(0)
                    .unwrap()
                    .format(&Rfc3339)
                    .unwrap(),
            );
            let context = WhyEvent {
                event_id: raw.clone(),
                artifact_id: event.artifact_id.clone(),
                url: event.url.clone(),
                occurred_at: event.occurred_at.clone(),
                incognito: false,
                text: "Prepare the release".into(),
            };
            let payload = if why {
                serde_json::to_value(&context).unwrap()
            } else {
                serde_json::to_value(&event).unwrap()
            };
            let kind = if why { "why" } else { "activity" };
            let marker = |payload: &serde_json::Value, id: &str, kind: &str| {
                format!(
                    "{MARKER_PREFIX}{} url={} at={} kind={kind} data={} -->",
                    sha256_hex(id.as_bytes()),
                    sha256_hex(event.url.as_bytes()),
                    event.occurred_at,
                    hex::encode(serde_json::to_vec(payload).unwrap())
                )
            };
            let historical = marker(&payload, canonical, kind);
            let parsed = parse_marker_event(&historical, "").expect("historical UUID marker");
            match &parsed.data {
                EventData::Activity(event) => assert_eq!(event.event_id, canonical),
                EventData::Why(event) => assert_eq!(event.event_id, canonical),
                _ => panic!("unexpected historical event"),
            }
            let mut forged = payload.clone();
            forged["event_id"] = Uuid::new_v4().to_string().into();
            assert!(parse_marker_event(&marker(&forged, canonical, kind), "").is_none());
            let seed = ActivityEvent {
                event_id: Uuid::new_v4().to_string(),
                ..event.clone()
            };
            let mut contents = format!("{LOG_HEADER}\n");
            if why {
                contents.push_str(&marker(
                    &serde_json::to_value(&seed).unwrap(),
                    &seed.event_id,
                    "activity",
                ));
                contents.push('\n');
            }
            contents.push_str(&historical);
            contents.push('\n');
            let date = OffsetDateTime::parse(&event.occurred_at, &Rfc3339)
                .unwrap()
                .date();
            let path = root.path().join(format!(
                "log/{}/{:02}/{date}.md",
                date.year(),
                u8::from(date.month())
            ));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &contents).unwrap();
            let index = index(&root, &cache);
            index.reindex().unwrap();
            let rebuilt = index.artifact(&event.artifact_id).unwrap().unwrap();
            if why {
                assert_eq!(rebuilt.why.as_deref(), Some(context.text.as_str()));
                index.record_why(&context, &[]).unwrap();
            } else {
                assert_eq!(rebuilt.focused_seconds, event.focused_seconds);
                assert_eq!(rebuilt.opens, event.opens);
                index.record_activity(&event, &[]).unwrap();
            }
            let state = index.load_current_state().unwrap().0;
            assert!(state.events.contains_key(canonical));
            assert_eq!(state.events.len(), if why { 2 } else { 1 });
            index.reindex().unwrap();
            assert_eq!(
                index.artifact(&event.artifact_id).unwrap().unwrap(),
                rebuilt
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
    }
}

#[test]
fn single_activity_ids_survive_reindex() {
    capture_ids_survive_reindex("single");
}

#[test]
fn batch_activity_ids_survive_reindex() {
    capture_ids_survive_reindex("batch");
}

#[test]
fn why_ids_survive_reindex() {
    capture_ids_survive_reindex("why");
}

#[test]
fn normalized_urls_at_the_capture_limit_survive_reindex() {
    use crate::capture::{CaptureBatchEvent, CaptureOutcome, CapturePolicy, CaptureStore};
    for batched in [false, true] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let capture = CaptureStore::open_in(&storage(root.path()), None, cache.path()).unwrap();
        let origins = ["https://example.com".to_owned()];
        let policy = CapturePolicy {
            enabled: true,
            origins: &origins,
            strip_params: &[],
        };
        let url = format!("https://example.com/{}", "ü".repeat(1362));
        assert!(url.len() < 8192);
        assert_eq!(canonical_url(&url, &[]).unwrap().len(), 8192);
        let mut event = activity(
            &Uuid::new_v4().to_string(),
            "ABC",
            "Unicode route",
            &time::OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        );
        event.url = url.clone();
        event.modes[0].url = url.clone();
        event.trails.push(ActivityTrail {
            transition: "link".into(),
            from_artifact_id: Some("work:example.com/source".into()),
            from_url: Some(url),
        });
        let outcome = if batched {
            capture
                .record_batch(
                    policy,
                    &Uuid::new_v4().to_string(),
                    &[CaptureBatchEvent::Activity(&event)],
                    None,
                )
                .events
                .remove(0)
                .1
        } else {
            capture.record_activity(policy, &event)
        };
        assert!(
            matches!(outcome, CaptureOutcome::Persisted { .. }),
            "{outcome:?}"
        );
        let index = index(&root, &cache);
        index.reindex().unwrap();
        let rebuilt = index.artifact(&event.artifact_id).unwrap().unwrap();
        assert_eq!(rebuilt.focused_seconds, event.focused_seconds);
        assert_eq!(rebuilt.url, canonical_url(&event.url, &[]).unwrap());
    }
}

#[test]
fn sub_minute_measures_remain_visible() {
    assert_eq!(measures(30, 20, 0), "focused <1 min · <1 min in background");
    assert_eq!(measures(1, 59, 0), "focused <1 min · <1 min in background");
    assert_eq!(measures(0, 0, 0), "seen");
    assert_eq!(
        measures(60, 119, 1),
        "focused 1 min · 1 min in background · opened 1×"
    );
    assert_eq!(measures(120, 0, 0), "focused 2 min");
}

#[test]
fn sub_minute_activity_is_visible_in_worklog_and_timeline() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut event = activity(
        "00000000-0000-4000-8000-000000000001",
        "ABC",
        "Brief review",
        "2026-10-15T12:00:00-04:00",
    );
    event.focused_seconds = 30;
    event.background_seconds = 20;
    index.record_activities([&event], &[]).unwrap();
    let day = parse_date("2026-10-15").unwrap();
    let (worklog, _) = index.worklog(day, day).unwrap();
    assert!(
        worklog.contains("focused <1 min · <1 min in background"),
        "{worklog}"
    );
    assert_eq!(index.regenerate_dirty().unwrap().written, 1);
    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert!(
        markdown.contains("2026-10-15 · focused <1 min · <1 min in background"),
        "{markdown}"
    );
    assert!(markdown.contains("  focused_min: 0\n"));
    assert!(markdown.contains("  background_min: 0\n"));
}

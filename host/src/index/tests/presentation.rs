use super::*;

#[test]
fn hostile_presentation_is_safe_in_logs_and_rebuilt_artifacts_without_changing_payloads() {
    use crate::capture::{CaptureBatchEvent, CaptureOutcome, CapturePolicy, CaptureStore};
    use brauser_protocol::VisitEvent;

    let hostile = "Report\u{202e}evil\u{202c}\u{200b}\u{200f}\u{2066}hidden\u{2069}\u{feff} %%comment%% ==marked== #tag ![embed](https://bad.invalid)";
    for batched in [false, true] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let config = storage(root.path());
        let capture = CaptureStore::open_in(&config, None, cache.path()).unwrap();
        let origins = ["https://example.com".to_owned()];
        let policy = || CapturePolicy {
            enabled: true,
            origins: &origins,
            strip_params: &[],
        };
        let event = activity(
            "00000000-0000-4000-8000-000000000001",
            "ABC",
            hostile,
            &time::OffsetDateTime::now_utc()
                .replace_minute(0)
                .unwrap()
                .replace_second(0)
                .unwrap()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        );
        let visit = VisitEvent {
            event_id: "00000000-0000-4000-8000-000000000002".into(),
            url: "https://example.com/old".into(),
            title: Some(hostile.into()),
            occurred_at: event.occurred_at.clone(),
            incognito: false,
        };
        if batched {
            let outcome = capture.record_batch(
                policy(),
                "00000000-0000-4000-8000-000000000004",
                &[
                    CaptureBatchEvent::Activity(&event),
                    CaptureBatchEvent::Visit(&visit),
                ],
                None,
            );
            assert_eq!(outcome.events.len(), 2);
            assert!(
                outcome
                    .events
                    .iter()
                    .all(|(_, result)| matches!(result, CaptureOutcome::Persisted { .. }))
            );
        } else {
            assert!(matches!(
                capture.record_activity(policy(), &event),
                CaptureOutcome::Persisted { .. }
            ));
            assert!(matches!(
                capture.record(policy(), &visit),
                CaptureOutcome::Persisted { .. }
            ));
        }
        let why = WhyEvent {
            event_id: "00000000-0000-4000-8000-000000000003".into(),
            artifact_id: event.artifact_id.clone(),
            url: event.url.clone(),
            occurred_at: event.occurred_at.clone(),
            incognito: false,
            text: hostile.into(),
        };
        assert!(matches!(
            capture.record_why(policy(), &why),
            CaptureOutcome::Persisted { .. }
        ));
        let day = &event.occurred_at[..10];
        let log = root
            .path()
            .join(format!("log/{}/{}/{day}.md", &day[..4], &day[5..7]));
        let original = fs::read_to_string(&log).unwrap();
        for (kind, payload) in [
            ("activity", serde_json::to_value(&event).unwrap()),
            ("why", serde_json::to_value(&why).unwrap()),
        ] {
            let expected = hex::encode(serde_json::to_vec(&payload).unwrap());
            assert!(original.contains(&format!(" kind={kind} data={expected} stripped=")));
        }
        let rows = original.lines().filter(|line| line.starts_with("- "));
        assert_eq!(rows.clone().count(), 3);
        for row in rows {
            assert!(!row.contains(['\u{202e}', '\u{200b}', '\u{200f}', '\u{2066}', '\u{feff}']));
            assert!(row.contains(r"\%\%comment\%\% \=\=marked\=\= \#tag \!\[embed\]"));
        }

        // Rebuild from the actual markers, not a second sanitized event.
        let decoded = parse_log(&original);
        assert_eq!(decoded.len(), 3);
        assert!(decoded.iter().any(|stored| matches!(&stored.data,
                EventData::Activity(value) if serde_json::to_value(value).unwrap() == serde_json::to_value(&event).unwrap()
            )));
        assert!(decoded.iter().any(|stored| matches!(&stored.data,
                EventData::Why(value) if serde_json::to_value(value).unwrap() == serde_json::to_value(&why).unwrap()
            )));
        let index = index(&root, &cache);
        assert_eq!(index.reindex().unwrap().written, 2);
        for path in artifact_files(root.path()) {
            let markdown = fs::read_to_string(path).unwrap();
            assert!(
                !markdown.contains(['\u{202e}', '\u{200b}', '\u{200f}', '\u{2066}', '\u{feff}'])
            );
            assert!(markdown.contains("Report"));
        }
        assert_eq!(fs::read_to_string(&log).unwrap(), original);
        assert_eq!(index.reindex().unwrap().unchanged, 2);
    }
}

#[test]
fn artifact_files_and_worklogs_neutralize_unicode_formatting() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let hostile = "Visible\u{202e}spoof\u{202c}\u{200b}\u{2069}\u{feff} end";
    let event = activity(
        "00000000-0000-4000-8000-000000000001",
        "ABC",
        hostile,
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&event, &[]).unwrap();
    index
        .record_why(
            &WhyEvent {
                event_id: "00000000-0000-4000-8000-000000000002".into(),
                artifact_id: event.artifact_id.clone(),
                url: event.url.clone(),
                occurred_at: event.occurred_at.clone(),
                incognito: false,
                text: format!("{hostile} %%hidden%% ==mark== #tag"),
            },
            &[],
        )
        .unwrap();
    assert_eq!(index.regenerate_dirty().unwrap().written, 1);
    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert!(!markdown.contains(['\u{202e}', '\u{202c}', '\u{200b}', '\u{2069}', '\u{feff}']));
    assert!(markdown.contains(r"\%\%hidden\%\% \=\=mark\=\= \#tag"));
    let (worklog, _) = index
        .worklog(
            parse_date("2026-10-15").unwrap(),
            parse_date("2026-10-15").unwrap(),
        )
        .unwrap();
    assert!(!worklog.contains(['\u{202e}', '\u{202c}', '\u{200b}', '\u{2069}', '\u{feff}']));
    assert!(worklog.contains("Visible spoof"));
}

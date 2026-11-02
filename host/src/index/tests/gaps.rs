use super::*;

fn overflow_line(id: &str, marker: &OverflowMarker) -> String {
    format!(
        "{MARKER_PREFIX}{} url={} at={} kind=overflow data={} -->\n",
        sha256_hex(id.as_bytes()),
        sha256_hex(b""),
        marker.last_occurred_at,
        hex::encode(serde_json::to_vec(marker).unwrap())
    )
}

#[test]
fn worklog_gaps_intersect_local_dates_without_prorating_or_summing_overlap() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let spanning = OverflowMarker {
        count: 4,
        first_occurred_at: "2026-10-20T23:00:00-07:00".into(),
        last_occurred_at: "2026-10-22T01:00:00-07:00".into(),
    };
    let overlap = OverflowMarker {
        count: 2,
        first_occurred_at: "2026-10-21T01:00:00-07:00".into(),
        last_occurred_at: "2026-10-21T02:00:00-07:00".into(),
    };
    fs::create_dir_all(root.path().join("log")).unwrap();
    let path = root.path().join("log/gaps.md");
    let marker = overflow_line("spanning", &spanning);
    let bytes = format!(
        "{LOG_HEADER}\n{marker}{marker}{}",
        overflow_line("overlap", &overlap)
    );
    fs::write(&path, &bytes).unwrap();
    let report = |start: &str, end: &str| {
        index
            .worklog_report(parse_date(start).unwrap(), parse_date(end).unwrap())
            .unwrap()
    };
    for day in ["2026-10-20", "2026-10-21", "2026-10-22"] {
        let (text, artifacts, gaps, truncated) = report(day, day);
        assert_eq!(artifacts, 0);
        assert!(text.starts_with("Opened 0 artifacts"));
        assert!(!truncated);
        assert_eq!(
            gaps[0], spanning,
            "full marker count, even for one intersecting day"
        );
        assert_eq!(gaps.len(), if day == "2026-10-21" { 2 } else { 1 });
    }
    assert!(report("2026-10-19", "2026-10-19").2.is_empty());
    assert!(report("2026-10-23", "2026-10-23").2.is_empty());
    let expected = report("2026-10-19", "2026-10-25");
    assert_eq!(expected.2, [spanning, overlap]);
    assert!(index.recent(None, None, 32).unwrap().0.is_empty());
    index.reindex().unwrap();
    assert_eq!(report("2026-10-19", "2026-10-25"), expected);
    assert_eq!(fs::read_to_string(path).unwrap(), bytes);
    // V7 checkpointed this same log while ignoring overflow entirely.
    let mut old: serde_json::Value =
        serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
    old["version"] = 7.into();
    old.as_object_mut().unwrap().remove("gaps");
    fs::write(index.state_path(), serde_json::to_vec(&old).unwrap()).unwrap();
    assert_eq!(report("2026-10-19", "2026-10-25"), expected);
    LOG_BYTES_READ.with(|value| value.set(0));
    assert_eq!(report("2026-10-19", "2026-10-25"), expected);
    assert_eq!(LOG_BYTES_READ.with(|value| value.get()), 0);
}

#[test]
fn real_attached_and_standalone_overflow_survive_retries_restart_and_reindex() {
    use crate::capture::{CaptureBatchEvent, CapturePolicy, CaptureStore};
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let capture = CaptureStore::open_in(&index.storage, None, state.path()).unwrap();
    let stamp = crate::capture::fixtures::event_time();
    let timestamp = stamp.format(&Rfc3339).unwrap();
    let origins = ["https://example.com".into()];
    let policy = CapturePolicy {
        enabled: true,
        origins: &origins,
        strip_params: &[],
    };
    let marker = OverflowMarker {
        count: 3,
        first_occurred_at: timestamp.clone(),
        last_occurred_at: timestamp.clone(),
    };
    let standalone = Uuid::new_v4().to_string();
    let attached = Uuid::new_v4().to_string();
    let mut event = activity(
        &Uuid::new_v4().to_string(),
        "actual",
        "Actual work",
        &timestamp,
    );
    // Attached markers include their anchor URL and provenance. The
    // anchor's log time can differ from the missing range's endpoints.
    // Query backslashes survive URL normalization and double in JSON.
    event.url.push_str(&format!("&large={}", "\\".repeat(7000)));
    event.occurred_at = (stamp + time::Duration::hours(1)).format(&Rfc3339).unwrap();
    for _ in 0..2 {
        assert!(
            capture
                .record_batch(policy, &standalone, &[], Some(&marker))
                .overflow_recorded
        );
        assert!(
            capture
                .record_batch(
                    policy,
                    &attached,
                    &[CaptureBatchEvent::Activity(&event)],
                    Some(&marker)
                )
                .overflow_recorded
        );
    }
    let expected = index.worklog_report(stamp.date(), stamp.date()).unwrap();
    assert_eq!(expected.1, 1);
    assert_eq!(expected.2, [marker.clone(), marker]);
    assert!(!expected.3);
    assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 1);
    index.reindex().unwrap();
    let reopened = ArtifactIndex::open_in(&index.storage, None, cache.path()).unwrap();
    assert_eq!(
        reopened.worklog_report(stamp.date(), stamp.date()).unwrap(),
        expected
    );
}

#[test]
fn gap_notices_are_bounded_and_invalid_or_unowned_markers_are_ignored() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let marker = OverflowMarker {
        count: 1,
        first_occurred_at: "2026-10-20T01:00:00+14:00".into(),
        last_occurred_at: "2026-10-20T01:00:00+14:00".into(),
    };
    let valid = overflow_line("valid", &marker);
    for bad in [
        valid.trim_end().trim_end_matches(" -->").to_owned(),
        valid.replace("at=2026-10-20", "at=invalid"),
        valid.replace("kind=overflow", "kind=activity"),
        overflow_line(
            "zero",
            &OverflowMarker {
                count: 0,
                ..marker.clone()
            },
        ),
        overflow_line(
            "backwards",
            &OverflowMarker {
                first_occurred_at: "2026-10-21T01:00:00+14:00".into(),
                ..marker.clone()
            },
        ),
    ] {
        assert!(parse_overflow_marker(bad.trim_end()).is_none(), "{bad}");
    }
    fs::create_dir_all(root.path().join("log")).unwrap();
    fs::write(root.path().join("log/unowned.md"), &valid).unwrap();
    let day = parse_date("2026-10-20").unwrap();
    assert!(index.worklog_report(day, day).unwrap().2.is_empty());
    let markers = (0..33)
        .map(|id| overflow_line(&id.to_string(), &marker))
        .collect::<String>();
    fs::write(
        root.path().join("log/owned.md"),
        format!("{LOG_HEADER}\n{markers}"),
    )
    .unwrap();
    let report = index.worklog_report(day, day).unwrap();
    assert_eq!(report.2.len(), 32);
    assert!(report.3);
    // Own offsets, not UTC (these markers are on October 19 in UTC).
    let utc_day = day.previous_day().unwrap();
    assert!(index.worklog_report(utc_day, utc_day).unwrap().2.is_empty());
}

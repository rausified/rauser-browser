use super::*;

fn page_summary(url: &str, occurred_at: &str) -> crate::summary::PageSummary {
    crate::summary::PageSummary {
        overview: None,
        read_scope: None,
        event_id: uuid::Uuid::new_v4().to_string(),
        url: url.into(),
        title: "A good discussion".into(),
        occurred_at: occurred_at.into(),
        document_type: "discussion".into(),
        category: "reading".into(),
        importance: brauser_protocol::ImportanceLevel::High,
        summary_path: "pages/hn-item.summary.md".into(),
        record_key: crate::vault::url_record_key(url).unwrap(),
        artifact_type: "web.page".into(),
        harness: brauser_protocol::HarnessId::Codex,
    }
}

fn rfc3339_seconds_ago(seconds: i64) -> String {
    (OffsetDateTime::now_utc() - time::Duration::seconds(seconds))
        .replace_nanosecond(0)
        .unwrap()
        .format(&Rfc3339)
        .unwrap()
}

#[test]
fn summary_event_appends_once_on_a_page_no_adapter_covers() {
    use crate::capture::{CapturePolicy, CaptureStore};

    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let config = storage(root.path());
    let capture = CaptureStore::open_in(&config, None, cache.path()).unwrap();
    let index = index(&root, &cache);
    let policy = CapturePolicy {
        enabled: true,
        origins: &[],
        strip_params: &[],
    };
    let url = "https://news.ycombinator.com/item?id=50251344";
    let summary = page_summary(url, &rfc3339_seconds_ago(5));

    assert!(matches!(
        capture.record_summary(policy, &summary).0,
        crate::capture::CaptureOutcome::Persisted { .. }
    ));

    let report = index.reindex().unwrap();
    assert_eq!(report.written, 1);
    let expected_id = crate::summary::web_page_artifact_id(url).unwrap();
    let artifact = index.artifact(&expected_id).unwrap().expect("indexed");
    assert_eq!(artifact.artifact_type, "web.page");
    assert_eq!(artifact.title, "A good discussion");

    let path = artifact_files(root.path()).pop().unwrap();
    let file = fs::read_to_string(&path).unwrap();
    assert!(file.contains("type: \"web.page\""));
    // The artifact file is nested 3 directories under the vault root
    // (artifacts/<type>/<site>/<file>.md); a root-relative summary_path
    // must resolve back up through all of them, or the Markdown link is
    // broken from the artifact file's own location.
    assert!(
        path.strip_prefix(root.path()).unwrap().components().count() == 4,
        "expected artifacts/<type>/<site>/<file>.md, got {path:?}"
    );
    assert!(
        file.contains(
            "Summary: [discussion, high importance](<../../../pages/hn-item.summary.md>)"
        )
    );
    assert!(file.contains("Summarized: A good discussion"));

    // Re-summarizing the same URL updates the one artifact, not a second.
    let mut again = page_summary(url, &rfc3339_seconds_ago(0));
    again.title = "An even better discussion".into();
    assert!(matches!(
        capture.record_summary(policy, &again).0,
        crate::capture::CaptureOutcome::Persisted { .. }
    ));
    index.reindex().unwrap();
    assert_eq!(artifact_files(root.path()).len(), 1);
    let updated = index.artifact(&expected_id).unwrap().expect("indexed");
    assert_eq!(updated.title, "An even better discussion");
}

#[test]
fn record_summary_returns_the_event_it_wrote() {
    use crate::capture::{CaptureOutcome, CapturePolicy, CaptureStore};

    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let capture = CaptureStore::open_in(&storage(root.path()), None, cache.path()).unwrap();
    let policy = CapturePolicy {
        enabled: true,
        origins: &[],
        strip_params: &[],
    };
    let url = "https://jira.example/browse/PROJ-1";
    let web = page_summary(url, &rfc3339_seconds_ago(5));
    let adapter = crate::summary::PageSummary {
        event_id: uuid::Uuid::new_v4().to_string(),
        record_key: brauser_protocol::RecordKey {
            kind: brauser_protocol::RecordKeyKind::Artifact,
            value: "jira:PROJ-1".into(),
        },
        artifact_type: "jira.issue".into(),
        harness: brauser_protocol::HarnessId::Claude,
        ..page_summary(url, &rfc3339_seconds_ago(4))
    };
    for summary in [web, adapter] {
        let (CaptureOutcome::Persisted { relative_path }, Some(event)) =
            capture.record_summary(policy, &summary)
        else {
            panic!("expected a persisted summary");
        };
        let log = fs::read_to_string(root.path().join(relative_path)).unwrap();
        let replayed = parse_log(&log)
            .into_iter()
            .filter_map(|replayed| match replayed.data {
                EventData::Summary(replayed) => Some(replayed),
                _ => None,
            })
            .find(|replayed| replayed.event_id == event.event_id)
            .expect("the written summary replays");
        assert_eq!(replayed, event);
        assert_eq!(event.record_key, summary.record_key);
        assert_eq!(event.harness, summary.harness);
    }
}

#[test]
fn the_artifact_summary_link_is_percent_encoded() {
    use crate::capture::{CapturePolicy, CaptureStore};

    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    // #82 fixed the folder names, so the reserved character sits in the
    // file name inside the fixed summaries folder.
    let config = crate::layout::fixed(&storage(root.path()));
    let capture = CaptureStore::open_in(&config, None, cache.path()).unwrap();
    let index = ArtifactIndex::open_in(&config, None, cache.path()).unwrap();
    let policy = CapturePolicy {
        enabled: true,
        origins: &[],
        strip_params: &[],
    };
    let url = "https://news.ycombinator.com/item?id=50251344";
    let mut summary = page_summary(url, &rfc3339_seconds_ago(5));
    summary.summary_path = format!("{}/hn item.summary.md", crate::layout::SUMMARIES_DIR);

    assert!(matches!(
        capture.record_summary(policy, &summary).0,
        crate::capture::CaptureOutcome::Persisted { .. }
    ));
    assert_eq!(index.reindex().unwrap().written, 1);

    let file = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert!(file.contains(
        "Summary: [discussion, high importance](<../../../summaries/hn%20item.summary.md>)"
    ));
}

#[test]
fn a_summary_joins_the_m1_legacy_visit_of_the_same_url() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    let artifact_id = crate::summary::web_page_artifact_id(url).unwrap();
    let url_hash = sha256_hex(url.as_bytes());

    let legacy_line = format!(
        "\n- 2026-10-20T10:00:00Z — A good discussion — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
        sha256_hex("legacy-marker".as_bytes()),
    );

    let event = SummaryEvent {
        overview: None,
        read_scope: None,
        event_id: uuid::Uuid::new_v4().to_string(),
        artifact_id: artifact_id.clone(),
        artifact_type: "web.page".into(),
        url: url.into(),
        title: "A good discussion".into(),
        occurred_at: "2026-10-26T18:00:00Z".into(),
        document_type: "discussion".into(),
        category: "reading".into(),
        importance: brauser_protocol::ImportanceLevel::High,
        summary_path: "pages/hn-item.summary.md".into(),
        record_key: crate::vault::url_record_key(url).unwrap(),
        harness: brauser_protocol::HarnessId::Codex,
    };
    let payload = hex::encode(serde_json::to_vec(&event).unwrap());
    let summary_line = format!(
        "<!-- brauser:visit id={} url={url_hash} at={} kind=summary data={payload} -->\n",
        sha256_hex(event.event_id.as_bytes()),
        event.occurred_at,
    );

    let dir = root.path().join("log/2026/10");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("2026-10-20.md"),
        format!("{LOG_HEADER}\n{legacy_line}"),
    )
    .unwrap();
    fs::write(
        dir.join("2026-10-26.md"),
        format!("{LOG_HEADER}\n{summary_line}"),
    )
    .unwrap();

    index.reindex().unwrap();
    assert_eq!(artifact_files(root.path()).len(), 1, "one shared artifact");
    let artifact = index.artifact(&artifact_id).unwrap().expect("indexed");
    assert_eq!(artifact.artifact_type, "web.page");
}

#[test]
fn forged_summary_markers_are_ignored() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    let url_hash = sha256_hex(url.as_bytes());
    let event = SummaryEvent {
        overview: None,
        read_scope: None,
        event_id: uuid::Uuid::new_v4().to_string(),
        artifact_id:
            "web:evil.example/0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
        artifact_type: "web.page".into(),
        url: url.into(),
        title: "A good discussion".into(),
        occurred_at: "2026-10-26T18:00:00Z".into(),
        document_type: "discussion".into(),
        category: "reading".into(),
        importance: brauser_protocol::ImportanceLevel::High,
        summary_path: "pages/hn-item.summary.md".into(),
        record_key: crate::vault::url_record_key(url).unwrap(),
        harness: brauser_protocol::HarnessId::Codex,
    };
    let payload = hex::encode(serde_json::to_vec(&event).unwrap());
    let line = format!(
        "<!-- brauser:visit id={} url={url_hash} at={} kind=summary data={payload} -->\n",
        sha256_hex(event.event_id.as_bytes()),
        event.occurred_at,
    );
    let dir = root.path().join("log/2026/10");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("2026-10-26.md"), format!("{LOG_HEADER}\n{line}")).unwrap();

    let report = index.reindex().unwrap();
    assert_eq!(report.written, 0);
    assert!(!root.path().join("artifacts").exists());
}

pub(super) fn web_summary(url: &str, at: &str) -> SummaryEvent {
    SummaryEvent {
        overview: None,
        read_scope: None,
        event_id: uuid::Uuid::new_v4().to_string(),
        artifact_id: crate::summary::web_page_artifact_id(url).unwrap(),
        artifact_type: "web.page".into(),
        url: url.into(),
        title: "A good discussion".into(),
        occurred_at: at.into(),
        document_type: "discussion".into(),
        category: "reading".into(),
        importance: brauser_protocol::ImportanceLevel::High,
        summary_path: "summaries/hn-item.summary.md".into(),
        record_key: crate::vault::url_record_key(url).unwrap(),
        harness: brauser_protocol::HarnessId::Codex,
    }
}

#[test]
fn recent_lists_a_summary_only_page_with_its_summary() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    let event = web_summary(url, "2026-10-15T12:00:00-07:00");
    index.record_summary(&event).unwrap();

    let (recent, types, truncated) = index.recent(None, None, 32).unwrap();
    assert_eq!(types, ["web.page"]);
    assert!(!truncated);
    assert_eq!(recent.len(), 1);
    let row = &recent[0];
    assert_eq!(row.local_date, "2026-10-15");
    assert_eq!(row.artifact_id, event.artifact_id);
    assert_eq!(row.artifact_type, "web.page");
    assert_eq!(row.title, "A good discussion");
    assert_eq!(
        (row.opens, row.focused_seconds, row.background_seconds),
        (0, 0, 0)
    );
    assert_eq!(row.summary, Some(expected_view(&event)));
    assert_eq!(
        row.summary.as_ref().unwrap().importance,
        brauser_protocol::ImportanceLevel::High
    );
}

#[test]
fn a_recent_row_carries_only_its_own_days_latest_summary() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let measured = activity(
        "00000000-0000-4000-8000-000000000031",
        "PROJ-1",
        "Ticket",
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&measured, &[]).unwrap();
    let summary = |at: &str, level| SummaryEvent {
        importance: level,
        ..adapter_summary(&measured.artifact_id, "work.item", &measured.url, at)
    };
    let early = summary(
        "2026-10-16T08:00:00Z",
        brauser_protocol::ImportanceLevel::Low,
    );
    let late = summary(
        "2026-10-16T09:00:00Z",
        brauser_protocol::ImportanceLevel::Medium,
    );
    // Recorded out of order: the later summary still wins.
    index.record_summary(&late).unwrap();
    index.record_summary(&early).unwrap();

    let (recent, _, _) = index.recent(None, None, 32).unwrap();
    let days = recent
        .iter()
        .map(|row| (row.local_date.as_str(), row.summary.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        days,
        [
            ("2026-10-16", Some(expected_view(&late))),
            ("2026-10-15", None)
        ]
    );
    assert_eq!(
        index.summary_for(&measured.artifact_id).unwrap(),
        Some(expected_view(&late))
    );
    // The panel's single-load lookup picks the same latest summary.
    let (artifact, latest) = index.artifact_with_summary(&measured.artifact_id).unwrap();
    assert_eq!(artifact.unwrap().artifact_id, measured.artifact_id);
    assert_eq!(latest, Some(expected_view(&late)));
    assert_eq!(index.summary_for("work:example.com/none").unwrap(), None);
    assert_eq!(
        index
            .artifact_with_summary("work:example.com/none")
            .unwrap(),
        (None, None)
    );
}

#[test]
fn recent_reading_filters_the_displayed_summary_before_counting_rows() {
    use crate::answer::ReadScope;
    use brauser_protocol::SummaryReadingFilter::{Limited, Unknown, Whole};

    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    // A full native response of nonmatching newer days. The limited
    // reading on an older day must remain discoverable even at limit one.
    for day in 1..=31 {
        for hour in [8, 9] {
            let mut event = web_summary(
                "https://example.com/whole",
                &format!("2026-10-{day:02}T{hour:02}:00:00Z"),
            );
            event.read_scope = Some(ReadScope::Whole);
            index.record_summary(&event).unwrap();
        }
    }
    let mut partial = web_summary("https://example.com/partial", "2026-09-30T08:00:00Z");
    partial.read_scope = Some(ReadScope::Partial);
    partial.title = "Focused analysis".into();
    let mut page_only = web_summary("https://example.com/page", "2026-09-29T08:00:00Z");
    page_only.read_scope = Some(ReadScope::PageOnly);
    let legacy = web_summary("https://example.com/legacy", "2026-09-28T08:00:00Z");
    for event in [&partial, &page_only, &legacy] {
        index.record_summary(event).unwrap();
    }
    index
        .record_activity(
            &activity(
                "00000000-0000-4000-8000-000000001101",
                "PROJ-1",
                "No summary",
                "2026-10-31T08:00:00Z",
            ),
            &[],
        )
        .unwrap();

    let (limited, types, truncated) = index.recent_with_reading(None, None, Limited, 1).unwrap();
    assert_eq!(
        limited
            .iter()
            .map(|row| row.artifact_id.as_str())
            .collect::<Vec<_>>(),
        [partial.artifact_id.as_str()]
    );
    assert!(truncated);
    assert!(types.contains(&"work.item".into())); // Type choices stay independent.
    let (limited, _, truncated) = index.recent_with_reading(None, None, Limited, 2).unwrap();
    assert_eq!(limited.len(), 2);
    assert!(!truncated); // Exactly the limit is not truncated.
    let (matching_title, _, truncated) = index
        .recent_with_reading(Some("web.page"), Some("FOCUSED"), Limited, 1)
        .unwrap();
    assert_eq!(matching_title.len(), 1);
    assert!(!truncated);
    let (unknown, _, truncated) = index.recent_with_reading(None, None, Unknown, 32).unwrap();
    assert_eq!(unknown.len(), 1); // Unsummarized activity is not unknown reading.
    assert_eq!(unknown[0].summary, Some(expected_view(&legacy)));
    assert!(!truncated);
    assert_eq!(
        index
            .recent_with_reading(None, None, Whole, 32)
            .unwrap()
            .0
            .len(),
        31
    );
    assert!(
        index
            .recent_with_reading(Some("work.item"), None, Limited, 32)
            .unwrap()
            .0
            .is_empty()
    );
    assert!(
        index
            .recent_with_reading(None, Some("No summary"), Unknown, 32)
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn recent_reading_uses_latest_renderable_scope_with_instant_and_log_order() {
    use crate::answer::ReadScope;
    use brauser_protocol::SummaryReadingFilter::{Limited, Unknown, Whole};
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut whole = web_summary("https://example.com/spec", "2026-10-16T09:00:00Z");
    whole.read_scope = Some(ReadScope::Whole);
    let mut partial = web_summary("https://example.com/spec", "2026-10-16T10:00:00+02:00");
    partial.read_scope = Some(ReadScope::Partial);
    index.record_summary(&whole).unwrap();
    index.record_summary(&partial).unwrap(); // Recorded later but earlier instant.
    assert!(
        index
            .recent_with_reading(None, None, Limited, 32)
            .unwrap()
            .0
            .is_empty()
    );
    assert_eq!(
        index.recent_with_reading(None, None, Whole, 32).unwrap().0[0].summary,
        Some(expected_view(&whole))
    );
    partial.event_id = uuid::Uuid::new_v4().to_string();
    partial.occurred_at = whole.occurred_at.clone();
    index.record_summary(&partial).unwrap(); // Same instant; later log entry wins.
    assert!(
        index
            .recent_with_reading(None, None, Whole, 32)
            .unwrap()
            .0
            .is_empty()
    );
    assert_eq!(
        index
            .recent_with_reading(None, None, Limited, 32)
            .unwrap()
            .0[0]
            .summary,
        Some(expected_view(&partial))
    );
    assert!(
        index
            .recent_with_reading(None, None, Unknown, 32)
            .unwrap()
            .0
            .is_empty()
    );
    let broken = web_summary(
        "https://example.com/spec",
        "2026-10-16T23:59:59 and later, in the evening, local",
    );
    index.record_summary(&broken).unwrap();
    assert_eq!(
        index
            .recent_with_reading(None, None, Limited, 32)
            .unwrap()
            .0[0]
            .summary,
        Some(expected_view(&partial))
    );
    let only_broken = web_summary(
        "https://example.com/broken",
        "2026-10-17T23:59:59 and later, in the evening, local",
    );
    index.record_summary(&only_broken).unwrap();
    assert!(
        index
            .recent_with_reading(None, None, Unknown, 32)
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn recent_reading_searches_a_year_of_summary_history_without_a_candidate_cap() {
    use crate::answer::ReadScope;
    use brauser_protocol::SummaryReadingFilter;
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let first_day = Date::from_calendar_date(2025, time::Month::November, 1).unwrap();
    for n in 0..365 {
        let day = first_day + time::Duration::days(n);
        let mut event = web_summary(
            "https://example.com/daily-report",
            &format!("{day}T10:00:00Z"),
        );
        event.read_scope = Some(ReadScope::Whole);
        index.record_summary(&event).unwrap();
    }
    let started = Instant::now();
    let (rows, _, truncated) = index
        .recent_with_reading(None, None, SummaryReadingFilter::Limited, 32)
        .unwrap();
    println!(
        "365-day single-artifact no-match reading filter: {:?}",
        started.elapsed()
    );
    assert!(rows.is_empty());
    assert!(!truncated);
    let mut older = web_summary("https://example.com/older-report", "2025-10-31T10:00:00Z");
    older.read_scope = Some(ReadScope::Partial);
    index.record_summary(&older).unwrap();
    let (rows, _, truncated) = index
        .recent_with_reading(None, None, SummaryReadingFilter::Limited, 32)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].summary, Some(expected_view(&older)));
    assert!(!truncated);
}

/// Two summaries of one record in the same second (the stamp has
/// whole-second resolution) order by when they were recorded, as the
/// summary file holds the later write, never by their random run IDs.
#[test]
fn same_second_summaries_keep_their_recorded_order() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    let at = "2026-10-16T08:00:00Z";
    let first = SummaryEvent {
        event_id: "ffffffff-ffff-4fff-bfff-ffffffffffff".into(),
        document_type: "first".into(),
        ..web_summary(url, at)
    };
    let second = SummaryEvent {
        event_id: "00000000-0000-4000-8000-000000000000".into(),
        document_type: "second".into(),
        ..web_summary(url, at)
    };
    index.record_summary(&first).unwrap();
    index.record_summary(&second).unwrap();

    let latest = Some(expected_view(&second));
    assert_eq!(index.summary_for(&first.artifact_id).unwrap(), latest);
    assert_eq!(
        index.artifact_with_summary(&first.artifact_id).unwrap().1,
        latest
    );
    let (recent, _, _) = index.recent(None, None, 32).unwrap();
    assert_eq!(recent[0].summary, latest);
    let day = Date::from_calendar_date(2026, time::Month::October, 16).unwrap();
    let (worklog, _) = index.worklog(day, day).unwrap();
    assert!(worklog.contains("second, high importance"), "{worklog}");
}

/// A latest summary with no view (an unparseable, over-long time) does
/// not hide an earlier one that renders.
#[test]
fn a_latest_summary_without_a_view_falls_back_to_the_newest_that_renders() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    let valid = web_summary(url, "2026-10-16T08:00:00Z");
    let broken = web_summary(url, "2026-10-16T23:59:59 and later, in the evening, local");
    assert_eq!(summary_view(&broken), None);
    assert_eq!(
        timestamp_order(&broken.occurred_at, &valid.occurred_at),
        std::cmp::Ordering::Greater
    );
    index.record_summary(&valid).unwrap();
    index.record_summary(&broken).unwrap();

    assert_eq!(
        index.summary_for(&valid.artifact_id).unwrap(),
        Some(expected_view(&valid))
    );
    let (_, latest) = index.artifact_with_summary(&valid.artifact_id).unwrap();
    assert_eq!(latest, Some(expected_view(&valid)));
    let (recent, _, _) = index.recent(None, None, 32).unwrap();
    let row = recent
        .iter()
        .find(|row| row.artifact_id == valid.artifact_id)
        .expect("the summarized page is listed");
    assert_eq!(row.summary, Some(expected_view(&valid)));
}

#[test]
fn a_summary_view_keeps_summarized_at_within_the_wire_bound() {
    let url = "https://news.ycombinator.com/item?id=50251344";
    let short = web_summary(url, "2026-10-26T14:05:00-07:00");
    assert_eq!(
        summary_view(&short).unwrap().summarized_at,
        "2026-10-26T14:05:00-07:00"
    );
    let long = web_summary(url, "2026-10-26T14:05:00.123456789-07:00");
    assert_eq!(
        summary_view(&long).unwrap().summarized_at,
        "2026-10-26T14:05:00-07:00"
    );
    // Too long and not a timestamp: no view rather than an empty time.
    let unparseable = web_summary(url, "2026-10-26 sometime in the afternoon, local");
    assert_eq!(summary_view(&unparseable), None);
}

#[test]
fn worklog_names_summaries_without_counting_them_as_opened() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    index
        .record_summary(&web_summary(url, "2026-10-15T12:00:00Z"))
        .unwrap();

    let (text, count) = index
        .worklog(
            parse_date("2026-10-15").unwrap(),
            parse_date("2026-10-15").unwrap(),
        )
        .unwrap();
    assert_eq!(count, 1);
    // A summary is a distinct, named action: it is neither an open nor
    // the generic "seen" wording of an artifact with no measured time.
    assert!(
            text.starts_with(
                "Opened 0 artifacts, focused on 0 left open from an earlier day, and summarized 1 page.\n"
            ),
            "{text}"
        );
    assert!(text.contains("\nSummarized: A good discussion — discussion, high importance"));
    assert!(!text.contains("seen"));
}

#[test]
fn an_m1_visit_and_a_later_summary_open_the_page_on_one_day() {
    // brwr-hnr: an M1 legacy visit on day A and a summary on day B are
    // one artifact opened on one day, and the summary is never an open.
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    let url_hash = sha256_hex(url.as_bytes());
    let legacy_line = format!(
        "\n- 2026-10-20T10:00:00Z — A good discussion — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
        sha256_hex("legacy-marker-hnr".as_bytes()),
    );
    let summary = web_summary(url, "2026-10-26T18:00:00Z");
    let dir = root.path().join("log/2026/10");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("2026-10-20.md"),
        format!("{LOG_HEADER}\n{legacy_line}"),
    )
    .unwrap();
    fs::write(
        dir.join("2026-10-26.md"),
        format!("{LOG_HEADER}\n{}", summary_marker(&summary)),
    )
    .unwrap();

    index.reindex().unwrap();
    let artifact = index
        .artifact(&summary.artifact_id)
        .unwrap()
        .expect("indexed");
    assert_eq!((artifact.opens, artifact.days_opened), (1, 1));
    assert_eq!(
        (artifact.first_seen.as_str(), artifact.last_seen.as_str()),
        ("2026-10-20", "2026-10-26")
    );
    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert!(
        markdown.contains("  opens: 1\n  days_opened: 1\n"),
        "{markdown}"
    );

    let worklog = |start: &str, end: &str| {
        index
            .worklog(parse_date(start).unwrap(), parse_date(end).unwrap())
            .unwrap()
    };
    let (both, count) = worklog("2026-10-20", "2026-10-26");
    assert_eq!(count, 1);
    assert_eq!(
        both,
        "Opened 1 artifact, focused on 0 left open from an earlier day, and summarized 1 page.\n\
             Summarized: A good discussion — discussion, high importance — opened 1×"
    );
    let (summary_day, _) = worklog("2026-10-26", "2026-10-26");
    assert_eq!(
        summary_day,
        "Opened 0 artifacts, focused on 0 left open from an earlier day, and summarized 1 page.\n\
             Summarized: A good discussion — discussion, high importance"
    );
    let (visit_day, _) = worklog("2026-10-20", "2026-10-20");
    assert!(
        visit_day
            .starts_with("Opened 1 artifact, and focused on 0 left open from an earlier day.\n"),
        "{visit_day}"
    );
}

#[test]
fn worklog_keeps_measures_when_a_summary_lands_on_an_already_measured_artifact() {
    // Exercises the future adapter-attached-summary case (spec §7.2's
    // "the adapter does recognize the page" branch, not wired by any
    // caller yet): the summary wording must not silently drop real
    // measured activity on the same artifact.
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-000000000020",
        "PROJ-1",
        "Ticket",
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&event, &[]).unwrap();
    index
        .record_summary(&SummaryEvent {
            overview: None,
            read_scope: None,
            event_id: uuid::Uuid::new_v4().to_string(),
            artifact_id: event.artifact_id.clone(),
            artifact_type: event.artifact_type.clone(),
            url: event.url.clone(),
            title: "Ticket".into(),
            occurred_at: "2026-10-15T13:00:00Z".into(),
            document_type: "ticket".into(),
            category: "work".into(),
            importance: brauser_protocol::ImportanceLevel::High,
            summary_path: "pages/proj-1.summary.md".into(),
            record_key: brauser_protocol::RecordKey {
                kind: brauser_protocol::RecordKeyKind::Artifact,
                value: event.artifact_id.clone(),
            },
            harness: brauser_protocol::HarnessId::Claude,
        })
        .unwrap();

    let (text, count) = index
        .worklog(
            parse_date("2026-10-15").unwrap(),
            parse_date("2026-10-15").unwrap(),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert!(
            text.starts_with(
                "Opened 1 artifact, focused on 0 left open from an earlier day, and summarized 1 page.\n"
            ),
            "{text}"
        );
    assert!(text.contains("Summarized: Ticket — ticket, high importance"));
    assert!(text.contains("focused 2 min"), "{text}");
}

pub(super) fn summary_marker(event: &SummaryEvent) -> String {
    format!(
        "<!-- brauser:visit id={} url={} at={} kind=summary data={} -->\n",
        sha256_hex(event.event_id.as_bytes()),
        sha256_hex(event.url.as_bytes()),
        event.occurred_at,
        hex::encode(serde_json::to_vec(event).unwrap()),
    )
}

fn timeline(markdown: &str) -> &str {
    let start = markdown.find("## Timeline\n\n").expect("a Timeline") + "## Timeline\n\n".len();
    &markdown[start..]
}

#[test]
fn timeline_interleaves_summaries_and_rollups_by_date() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let day1 = activity(
        "00000000-0000-4000-8000-000000000101",
        "MIX",
        "Item",
        "2026-10-01T12:00:00Z",
    );
    let day5 = ActivityEvent {
        focus_session: None,
        event_id: "00000000-0000-4000-8000-000000000105".into(),
        occurred_at: "2026-10-05T12:00:00Z".into(),
        ..day1.clone()
    };
    index.record_activities([&day1, &day5], &[]).unwrap();
    let mut on_day3 = adapter_summary(
        &day1.artifact_id,
        &day1.artifact_type,
        &day1.url,
        "2026-10-03T08:30:00Z",
    );
    on_day3.title = "Day three".into();
    let mut on_day5 = adapter_summary(
        &day1.artifact_id,
        &day1.artifact_type,
        &day1.url,
        "2026-10-05T07:00:00Z",
    );
    on_day5.title = "Day five".into();
    on_day5.importance = brauser_protocol::ImportanceLevel::Low;
    index.record_summary(&on_day3).unwrap();
    index.record_summary(&on_day5).unwrap();
    assert_eq!(index.regenerate_dirty().unwrap().written, 1);

    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert_eq!(
        timeline(&markdown),
        "- 2026-10-05 07:00 · Summarized: Day five — discussion, low importance\n\
             - 2026-10-05 · focused 2 min · 1 min in background · opened 1×\n\
             - 2026-10-03 08:30 · Summarized: Day three — discussion, high importance\n\
             - 2026-10-01 · focused 2 min · 1 min in background · opened 1×\n",
        "{markdown}"
    );
    // A summary-only day is not a day opened.
    assert!(
        markdown.contains("  opens: 2\n  days_opened: 2\n"),
        "{markdown}"
    );
    assert_eq!(
        index
            .artifact(&day1.artifact_id)
            .unwrap()
            .unwrap()
            .days_opened,
        2
    );
}

#[test]
fn timeline_merges_summaries_across_offsets_by_their_own_date() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let day5 = activity(
        "00000000-0000-4000-8000-000000000111",
        "TRAVEL",
        "Item",
        "2026-10-05T12:00:00Z",
    );
    let day6 = ActivityEvent {
        focus_session: None,
        event_id: "00000000-0000-4000-8000-000000000112".into(),
        occurred_at: "2026-10-06T12:00:00Z".into(),
        ..day5.clone()
    };
    index.record_activities([&day5, &day6], &[]).unwrap();
    // The later instant has the earlier local date.
    let mut late_evening = adapter_summary(
        &day5.artifact_id,
        &day5.artifact_type,
        &day5.url,
        "2026-10-05T23:30:00-07:00",
    );
    late_evening.title = "Late evening".into();
    let mut early_morning = adapter_summary(
        &day5.artifact_id,
        &day5.artifact_type,
        &day5.url,
        "2026-10-06T01:00:00+09:00",
    );
    early_morning.title = "Early morning".into();
    index.record_summary(&late_evening).unwrap();
    index.record_summary(&early_morning).unwrap();
    index.regenerate_dirty().unwrap();

    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert_eq!(
        timeline(&markdown),
        "- 2026-10-06 01:00 · Summarized: Early morning — discussion, high importance\n\
             - 2026-10-06 · focused 2 min · 1 min in background · opened 1×\n\
             - 2026-10-05 23:30 · Summarized: Late evening — discussion, high importance\n\
             - 2026-10-05 · focused 2 min · 1 min in background · opened 1×\n",
        "{markdown}"
    );
}

#[test]
fn a_timeline_merges_summaries_with_m1_legacy_visit_lines() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    let url_hash = sha256_hex(url.as_bytes());
    let legacy_line = format!(
        "\n- 2026-10-20T10:00:00Z — A good discussion — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
        sha256_hex("legacy-marker-merge".as_bytes()),
    );
    let summary = |at: &str| {
        let mut summary = adapter_summary(
            &crate::summary::web_page_artifact_id(url).unwrap(),
            "web.page",
            url,
            at,
        );
        summary.record_key = crate::vault::url_record_key(url).unwrap();
        summary
    };
    let before = summary("2026-10-19T09:00:00Z");
    let after = summary("2026-10-26T18:05:59Z");
    let dir = root.path().join("log/2026/10");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("2026-10-19.md"),
        format!("{LOG_HEADER}\n{}", summary_marker(&before)),
    )
    .unwrap();
    fs::write(
        dir.join("2026-10-20.md"),
        format!("{LOG_HEADER}\n{legacy_line}"),
    )
    .unwrap();
    fs::write(
        dir.join("2026-10-26.md"),
        format!("{LOG_HEADER}\n{}", summary_marker(&after)),
    )
    .unwrap();

    assert_eq!(index.reindex().unwrap().written, 1);
    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert_eq!(
        timeline(&markdown),
        "- 2026-10-26 18:05 · Summarized: A good discussion — discussion, high importance\n\
             - 2026-10-20 · opened 1×\n\
             - 2026-10-19 09:00 · Summarized: A good discussion — discussion, high importance\n",
        "{markdown}"
    );
}

#[test]
fn a_timeline_summary_line_has_the_title_and_no_seconds() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    let mut event = adapter_summary(
        &crate::summary::web_page_artifact_id(url).unwrap(),
        "web.page",
        url,
        "2026-10-20T09:15:42-07:00",
    );
    event.record_key = crate::vault::url_record_key(url).unwrap();
    index.record_summary(&event).unwrap();
    index.regenerate_dirty().unwrap();

    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    // The time stays in the event's own offset, not UTC.
    assert_eq!(
        timeline(&markdown),
        "- 2026-10-20 09:15 · Summarized: A good discussion — discussion, high importance\n",
        "{markdown}"
    );
}

#[test]
fn timeline_summary_fields_are_escaped_and_capped_at_twenty_lines() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://jira.example/browse/PROJ-9";
    for day in 1..=22 {
        let mut event = adapter_summary(
            "jira:jira.example/PROJ-9",
            "jira.issue",
            url,
            &format!("2026-09-{day:02}T10:00:00Z"),
        );
        event.title = "![x](https://e.example/a.png) `t`".into();
        event.document_type = "a`b".into();
        index.record_summary(&event).unwrap();
    }
    index.regenerate_dirty().unwrap();

    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    let lines = timeline(&markdown).lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 21, "{markdown}");
    assert_eq!(
        lines[0],
        format!(
            "- 2026-09-22 10:00 · Summarized: {} — {}, high importance",
            markdown::inline(
                "![x](https://e.example/a.png) `t`",
                MAX_RENDERED_FIELD_CHARS
            ),
            markdown::inline("a`b", MAX_RENDERED_FIELD_CHARS),
        )
    );
    assert!(!lines[0].contains('`'), "{}", lines[0]);
    assert!(lines[19].starts_with("- 2026-09-03 10:00 · Summarized:"));
    assert_eq!(lines[20], "- 2 earlier summaries");
}

#[test]
fn a_summary_on_an_adapter_artifact_without_activity_creates_that_row() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let summary = adapter_summary(
        "jira:jira.example/PROJ-1",
        "jira.issue",
        "https://jira.example/browse/PROJ-1",
        "2026-10-20T09:00:00Z",
    );
    index.record_summary(&summary).unwrap();
    let row = index
        .artifact(&summary.artifact_id)
        .unwrap()
        .expect("a row");
    assert_eq!(row.artifact_type, "jira.issue");
    assert_eq!(row.title, "A good discussion");
    // A summary is not measured activity.
    assert_eq!((row.opens, row.focused_seconds, row.days_opened), (0, 0, 0));
    index.regenerate_dirty().unwrap();
    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert!(
        markdown.contains("  opens: 0\n  days_opened: 0\n"),
        "{markdown}"
    );

    let later = ActivityEvent {
        focus_session: None,
        artifact_id: summary.artifact_id.clone(),
        artifact_type: "jira.issue".into(),
        url: "https://jira.example/browse/PROJ-1?focusedCommentId=2".into(),
        ..activity(
            "00000000-0000-4000-8000-000000000201",
            "PROJ-1",
            "PROJ-1 renamed",
            "2026-10-21T09:00:00Z",
        )
    };
    index.record_activity(&later, &[]).unwrap();
    index.regenerate_dirty().unwrap();
    assert_eq!(artifact_files(root.path()).len(), 1);
    let row = index.artifact(&summary.artifact_id).unwrap().unwrap();
    assert_eq!(row.artifact_type, "jira.issue");
    assert_eq!(row.title, "PROJ-1 renamed");
    assert_eq!(row.url, later.url);
    assert_eq!((row.opens, row.days_opened), (1, 1));
}

#[test]
fn an_adapter_summary_is_a_note_key_candidate() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://jira.example/browse/PROJ-1";
    let summary = adapter_summary(
        "jira:jira.example/PROJ-1",
        "jira.issue",
        url,
        "2026-10-20T09:00:00Z",
    );
    index.record_summary(&summary).unwrap();
    assert_eq!(index.artifact_urls(&summary.artifact_id).unwrap(), [url]);
}

#[test]
fn a_web_summary_url_stays_with_its_own_web_artifact() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let adapter_url = "https://jira.example/browse/PROJ-1";
    let web_url = "https://jira.example/browse/PROJ-1?view=print";
    let adapter = ActivityEvent {
        focus_session: None,
        artifact_id: "jira:jira.example/PROJ-1".into(),
        artifact_type: "jira.issue".into(),
        url: adapter_url.into(),
        modes: Vec::new(),
        ..activity(
            "00000000-0000-4000-8000-000000000301",
            "PROJ-1",
            "PROJ-1",
            "2026-10-20T09:00:00Z",
        )
    };
    index.record_activity(&adapter, &[]).unwrap();
    let web_id = crate::summary::web_page_artifact_id(web_url).unwrap();
    let mut web = adapter_summary(&web_id, "web.page", web_url, "2026-10-20T10:00:00Z");
    web.record_key = crate::vault::url_record_key(web_url).unwrap();
    index.record_summary(&web).unwrap();

    // The web summary's URL is indexed under its own `web:` artifact,
    // not the adapter artifact on the same host. This checks the index
    // side only; `resolve_note_context`'s `names_artifact` filter is
    // what keeps a URL from becoming another artifact's note key.
    assert_eq!(
        index.artifact_urls(&adapter.artifact_id).unwrap(),
        [adapter_url]
    );
    assert_eq!(index.artifact_urls(&web_id).unwrap(), [web_url]);
}

#[test]
fn reusing_a_summary_event_id_with_other_content_bails() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let first = adapter_summary(
        "jira:jira.example/PROJ-1",
        "jira.issue",
        "https://jira.example/browse/PROJ-1",
        "2026-10-20T09:00:00Z",
    );
    index.record_summary(&first).unwrap();
    // An identical retry is accepted.
    index.record_summary(&first).unwrap();
    let mut forged = first.clone();
    forged.title = "Something else".into();
    let error = index.record_summary(&forged).unwrap_err().to_string();
    assert!(error.contains("cannot be reused"), "{error}");
    assert_eq!(
        index.artifact(&first.artifact_id).unwrap().unwrap().title,
        "A good discussion"
    );
}

#[test]
fn saved_scopes_survive_incremental_indexing_reopen_and_cold_replay() {
    use crate::answer::ReadScope;
    use crate::capture::{CaptureOutcome, CapturePolicy, CaptureStore};
    use brauser_protocol::SummaryReadingFilter;
    for scope in [
        None,
        Some(ReadScope::Whole),
        Some(ReadScope::Partial),
        Some(ReadScope::PageOnly),
    ] {
        for adapter_key in [false, true] {
            let root = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let config = storage(root.path());
            let capture = CaptureStore::open_in(&config, None, cache.path()).unwrap();
            let mut summary = page_summary(
                "https://jira.example/browse/PROJ-1",
                &rfc3339_seconds_ago(5),
            );
            summary.read_scope = scope;
            summary.overview =
                Some("A viewer can look complete without the whole document.".into());
            if adapter_key {
                summary.record_key = brauser_protocol::RecordKey {
                    kind: brauser_protocol::RecordKeyKind::Artifact,
                    value: "jira:PROJ-1".into(),
                };
                summary.artifact_type = "jira.issue".into();
            }
            let policy = CapturePolicy {
                enabled: true,
                origins: &[],
                strip_params: &[],
            };
            let (CaptureOutcome::Persisted { .. }, Some(event)) =
                capture.record_summary(policy, &summary)
            else {
                panic!("persisted summary")
            };
            assert_eq!(event.read_scope, scope);
            let first = index(&root, &cache);
            first.record_summary(&event).unwrap();
            let expected = Some(expected_view(&event));
            let reading = match scope {
                None => SummaryReadingFilter::Unknown,
                Some(ReadScope::Whole) => SummaryReadingFilter::Whole,
                Some(ReadScope::Partial | ReadScope::PageOnly) => SummaryReadingFilter::Limited,
            };
            assert_eq!(
                first.recent_with_reading(None, None, reading, 1).unwrap().0[0].summary,
                expected
            );
            assert_eq!(
                first.artifact_with_summary(&event.artifact_id).unwrap().1,
                expected
            );
            drop(first);
            let reopened = index(&root, &cache);
            assert_eq!(reopened.summary_for(&event.artifact_id).unwrap(), expected);
            reopened.reindex().unwrap();
            assert_eq!(
                reopened
                    .artifact_with_summary(&event.artifact_id)
                    .unwrap()
                    .1,
                expected
            );
            assert_eq!(
                reopened.recent(None, None, 32).unwrap().0[0].summary,
                expected
            );
            // A genuinely empty cache must derive the same claim from the log.
            let cold_cache = TempDir::new().unwrap();
            let cold = index(&root, &cold_cache);
            cold.reindex().unwrap();
            assert_eq!(cold.summary_for(&event.artifact_id).unwrap(), expected);
            assert_eq!(
                cold.recent_with_reading(None, None, reading, 1).unwrap().0[0].summary,
                expected
            );
            let payload = serde_json::to_value(&event).unwrap();
            assert_eq!(payload.get("read_scope").is_some(), scope.is_some());
        }
    }
}

#[test]
fn malformed_recorded_scope_is_not_a_summary() {
    let event = web_summary(
        "https://news.ycombinator.com/item?id=1",
        "2026-10-26T18:00:00Z",
    );
    let mut payload = serde_json::to_value(&event).unwrap();
    payload["read_scope"] = "most".into();
    let invalid = summary_marker(&event).replace(
        &hex::encode(serde_json::to_vec(&event).unwrap()),
        &hex::encode(serde_json::to_vec(&payload).unwrap()),
    );
    assert!(
        !parse_log(&invalid)
            .iter()
            .any(|entry| matches!(entry.data, EventData::Summary(_)))
    );
}

/// Ported from origin/main 99714a7 onto format 9: the overview is stored in
/// the event body (absent when the event had none, so older bodies are
/// byte-identical), survives reopen without migration and survives reindex.
#[test]
fn old_and_new_summary_overviews_survive_cache_reopen_and_reindex() {
    for overview in [None, Some("The release plan and its tradeoffs.".to_owned())] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let url = "https://news.ycombinator.com/item?id=50251344";
        let mut event = web_summary(url, "2026-10-26T18:00:00Z");
        event.overview = overview;
        let dir = root.path().join("log/2026/10");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("2026-10-26.md"),
            format!("{LOG_HEADER}\n{}", summary_marker(&event)),
        )
        .unwrap();
        let body_query = format!(
            "SELECT body FROM brauser_events WHERE event_id = '{}'",
            event.event_id
        );
        let first = index(&root, &cache);
        first.reindex().unwrap();
        let body = sql::sql_rows(&first.database_path(), &body_query);
        let [row] = body.as_slice() else {
            panic!("one stored body: {body:?}")
        };
        let text = sql::display(&row[0]);
        // Absence keeps the pre-overview body shape.
        assert_eq!(
            text.contains("\"overview\""),
            event.overview.is_some(),
            "{text}"
        );
        drop(first);
        let reopened = index(&root, &cache);
        assert_eq!(
            reopened.summary_for(&event.artifact_id).unwrap(),
            Some(expected_view(&event))
        );
        assert_eq!(
            sql::sql_rows(&reopened.database_path(), &body_query),
            body,
            "stored bodies need no migration"
        );
        reopened.reindex().unwrap();
        assert_eq!(
            reopened.summary_for(&event.artifact_id).unwrap(),
            Some(expected_view(&event))
        );
        assert_eq!(
            reopened.recent(None, None, 32).unwrap().0[0].summary,
            Some(expected_view(&event))
        );
    }
}

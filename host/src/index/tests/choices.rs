//! Worklog choices: direct visits only, each day's own context.

use super::containers::github_blob;
use super::summaries::web_summary;
use super::*;

#[test]
fn worklog_choices_scope_context_and_measures_to_each_day() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let mut public = activity(
        &Uuid::new_v4().to_string(),
        "ABC",
        "Public update",
        "2026-10-15T12:00:00Z",
    );
    public.url = "https://work.example/items/ABC?view=public".into();
    let mut private = public.clone();
    private.event_id = Uuid::new_v4().to_string();
    private.title = Some("Private next-day topic".into());
    private.url = "https://work.example/items/ABC?view=private".into();
    private.occurred_at = "2026-10-16T12:00:00Z".into();
    index.record_activities([&public, &private], &[]).unwrap();
    let start = parse_date("2026-10-15").unwrap();
    let end = parse_date("2026-10-16").unwrap();
    let (choices, truncated, _, _) = index.worklog_choices(start, end).unwrap();
    assert!(!truncated);
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[0].local_date, "2026-10-16");
    assert!(choices[1].text.contains("Public update — focused 2 min"));
    assert!(!choices[1].text.contains("Private"));
    assert_eq!(choices[1].url, public.url);
    assert_eq!(choices[0].url, private.url);
    assert_eq!(
        index.worklog_choices(start, start).unwrap().0,
        vec![choices[1].clone()]
    );
}

#[test]
fn worklog_choices_include_summary_only_days_without_inventing_activity() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://news.ycombinator.com/item?id=50251344";
    index
        .record_summary(&web_summary(url, "2026-10-15T12:00:00Z"))
        .unwrap();
    let day = parse_date("2026-10-15").unwrap();
    let choices = index.worklog_choices(day, day).unwrap().0;
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].url, url);
    assert!(
        choices[0]
            .text
            .contains("Summarized: discussion, high importance")
    );
    assert!(!choices[0].text.contains("opened"));
    assert!(!choices[0].text.contains("seen"));
}

#[test]
fn worklog_choices_never_include_container_copies() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    index
        .record_activities(
            [
                &github_pr(1, 7, "2026-10-15T10:00:00Z"),
                &github_blob(2, "2026-10-15T11:00:00Z"),
                &github_pr(3, 8, "2026-10-16T10:00:00Z"),
            ],
            &[],
        )
        .unwrap();
    let d1 = parse_date("2026-10-15").unwrap();
    let d2 = d1.next_day().unwrap();
    let choices = index.worklog_choices(d1, d2).unwrap().0;
    let repo = choices
        .iter()
        .filter(|r| r.artifact_id == GITHUB_REPO_ID)
        .collect::<Vec<_>>();
    assert_eq!(
        repo.len(),
        1,
        "a child-only day must not invent a container choice"
    );
    assert_eq!(repo[0].local_date, "2026-10-15");
    assert!(repo[0].text.contains("focused <1 min"), "{}", repo[0].text);
    assert!(!repo[0].text.contains("focused 2 min"));
}

#[test]
fn worklog_choices_bound_rows_and_serialized_bytes_deterministically() {
    for (count, title) in [(205, "Brief".to_owned()), (200, "\"é".repeat(250))] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let events = (0..count)
            .map(|n| {
                activity(
                    &Uuid::new_v4().to_string(),
                    &format!("ABC{n:04}"),
                    &title,
                    "2026-10-15T12:00:00Z",
                )
            })
            .collect::<Vec<_>>();
        index.record_activities(events.iter(), &[]).unwrap();
        let day = parse_date("2026-10-15").unwrap();
        let (choices, truncated, _, _) = index.worklog_choices(day, day).unwrap();
        assert!(truncated);
        assert!(choices.len() <= 200);
        assert!(serde_json::to_vec(&choices).unwrap().len() <= MAX_WORKLOG_BYTES);
        if title == "Brief" {
            assert_eq!(choices.len(), 200);
        } else {
            assert!(choices.len() < 200);
        }
        assert_eq!(choices, index.worklog_choices(day, day).unwrap().0);
    }
}

fn first_page() -> brauser_protocol::WorklogChoicesPageRequest {
    brauser_protocol::WorklogChoicesPageRequest {
        offset: 0,
        snapshot: None,
    }
}

#[test]
fn worklog_pages_reach_every_busy_day_row_under_row_and_escaped_byte_bounds() {
    for title in ["Brief".to_owned(), "\"é".repeat(250)] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let events = (0..451)
            .map(|n| {
                activity(
                    &Uuid::new_v4().to_string(),
                    &format!("P{n:04}"),
                    &title,
                    "2026-10-15T12:00:00Z",
                )
            })
            .collect::<Vec<_>>();
        let log = write_log(
            root.path(),
            "2026-10-15.md",
            &events.iter().collect::<Vec<_>>(),
        );
        let original = fs::read(&log).unwrap();
        let day = parse_date("2026-10-15").unwrap();
        let mut request = first_page();
        let mut all = Vec::new();
        let mut pages = 0;
        loop {
            let result = index.worklog_choices_page(day, day, &request).unwrap();
            pages += 1;
            assert!(!result.page.omitted);
            assert!(!result.choices.is_empty());
            assert!(result.choices.len() <= 200);
            assert!(serde_json::to_vec(&result.choices).unwrap().len() <= MAX_WORKLOG_BYTES);
            if let Some(snapshot) = &request.snapshot {
                assert_eq!(&result.page.snapshot, snapshot);
            }
            all.extend(result.choices);
            let Some(offset) = result.page.next_offset else {
                break;
            };
            assert!(offset > request.offset);
            request = brauser_protocol::WorklogChoicesPageRequest {
                offset,
                snapshot: Some(result.page.snapshot),
            };
        }
        assert!(pages >= 3);
        assert_eq!(all.len(), events.len());
        assert_eq!(
            all.iter()
                .map(|row| row.artifact_id.clone())
                .collect::<Vec<_>>(),
            events
                .iter()
                .map(|event| event.artifact_id.clone())
                .collect::<Vec<_>>()
        );
        let before = index.worklog_choices_page(day, day, &request).unwrap();
        // Pagination depends on log replay, not generating hundreds of
        // artifact Markdown files. Exercise cache loss on the large fixture;
        // the small history fixture below also checks the real reindex path.
        remove_database(&index);
        let after = index.worklog_choices_page(day, day, &request).unwrap();
        assert_eq!(before.page, after.page);
        assert_eq!(before.choices, after.choices);
        assert_eq!(fs::read(log).unwrap(), original);
    }
}

#[test]
fn worklog_pages_refuse_inserted_deleted_and_edited_history_or_capture_gaps() {
    for change in ["insert", "delete", "edit", "gap"] {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let mut events = (0..3)
            .map(|n| {
                activity(
                    &Uuid::new_v4().to_string(),
                    &format!("P{n}"),
                    "Before",
                    "2026-10-15T12:00:00Z",
                )
            })
            .collect::<Vec<_>>();
        write_log(
            root.path(),
            "2026-10-15.md",
            &events.iter().collect::<Vec<_>>(),
        );
        let day = parse_date("2026-10-15").unwrap();
        let first = index.worklog_choices_page(day, day, &first_page()).unwrap();
        index.reindex().unwrap();
        assert_eq!(
            first.page,
            index
                .worklog_choices_page(day, day, &first_page())
                .unwrap()
                .page
        );
        let next = brauser_protocol::WorklogChoicesPageRequest {
            offset: 1,
            snapshot: Some(first.page.snapshot),
        };
        match change {
            "insert" => events.push(activity(
                &Uuid::new_v4().to_string(),
                "NEW",
                "New",
                "2026-10-15T12:00:00Z",
            )),
            "delete" => {
                events.pop();
            }
            "edit" => {
                events[0].title = Some("After".into());
                events[0].focused_seconds += 1;
            }
            "gap" => {
                fs::write(
                    root.path().join("log/gaps.md"),
                    format!(
                        "{LOG_HEADER}\n{}",
                        super::gaps::overflow_line(
                            "new-gap",
                            &OverflowMarker {
                                count: 2,
                                first_occurred_at: "2026-10-15T12:00:00Z".into(),
                                last_occurred_at: "2026-10-15T13:00:00Z".into(),
                            }
                        )
                    ),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        write_log(
            root.path(),
            "2026-10-15.md",
            &events.iter().collect::<Vec<_>>(),
        );
        index.reindex().unwrap();
        assert!(
            index
                .worklog_choices_page(day, day, &next)
                .err()
                .unwrap()
                .is::<StaleWorklogPage>(),
            "{change}"
        );
    }
}

#[test]
fn worklog_pages_bind_date_storage_scope_and_validate_continuations() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let day = parse_date("2026-10-15").unwrap();
    let page = index.worklog_choices_page(day, day, &first_page()).unwrap();
    assert!(page.choices.is_empty());
    assert_eq!(page.page.next_offset, None);
    let check = |offset, snapshot| brauser_protocol::WorklogChoicesPageRequest { offset, snapshot };
    for request in [
        check(1, None),
        check(0, Some("x".repeat(64))),
        check(0, Some("a".repeat(63))),
        check(1, Some(page.page.snapshot.clone())),
        check(0, Some("a".repeat(64))),
    ] {
        assert!(index.worklog_choices_page(day, day, &request).is_err());
    }
    let request = check(0, Some(page.page.snapshot.clone()));
    assert!(
        index
            .worklog_choices_page(day, day.next_day().unwrap(), &request)
            .is_err()
    );
    let other_root = TempDir::new().unwrap();
    let other_cache = TempDir::new().unwrap();
    let other = super::index(&other_root, &other_cache);
    assert!(other.worklog_choices_page(day, day, &request).is_err());
    let adapters = github_document(true);
    assert!(
        index
            .with_adapters(Some(&adapters))
            .worklog_choices_page(day, day, &request)
            .is_err()
    );
}

#[test]
fn worklog_page_snapshot_includes_notices_beyond_the_display_cap() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let day = parse_date("2026-10-15").unwrap();
    fs::create_dir_all(root.path().join("log")).unwrap();
    let path = root.path().join("log/gaps.md");
    let write = |last_count| {
        let markers = (0..33)
            .map(|n| {
                super::gaps::overflow_line(
                    &format!("gap-{n}"),
                    &OverflowMarker {
                        count: if n == 32 { last_count } else { 1 },
                        first_occurred_at: format!("2026-10-15T12:{n:02}:00Z"),
                        last_occurred_at: format!("2026-10-15T12:{n:02}:30Z"),
                    },
                )
            })
            .collect::<String>();
        fs::write(&path, format!("{LOG_HEADER}\n{markers}")).unwrap();
    };
    write(1);
    let first = index.worklog_choices_page(day, day, &first_page()).unwrap();
    assert_eq!(first.gaps.len(), 32);
    assert!(first.gaps_truncated);
    write(2);
    index.reindex().unwrap();
    let after = index.worklog_choices_page(day, day, &first_page()).unwrap();
    assert_eq!(first.gaps, after.gaps);
    assert_ne!(first.page.snapshot, after.page.snapshot);
    let old = brauser_protocol::WorklogChoicesPageRequest {
        offset: 0,
        snapshot: Some(first.page.snapshot),
    };
    assert!(index.worklog_choices_page(day, day, &old).is_err());
}

#[test]
fn worklog_pages_skip_oversized_rows_without_losing_normal_rows_or_looping() {
    let rows = (0..5)
        .map(|n| WorklogChoice {
            artifact_id: format!("doc:{n}"),
            local_date: "2026-10-15".into(),
            text: if n % 2 == 0 {
                "z".repeat(9000)
            } else {
                "Fits".into()
            },
            url: format!("https://example.com/{n}"),
        })
        .collect::<Vec<_>>();
    let (choices, page) =
        choice_page::bound_rows(rows.clone(), "a".repeat(64), &first_page()).unwrap();
    assert!(page.omitted);
    assert_eq!(page.next_offset, None);
    assert_eq!(
        choices
            .iter()
            .map(|row| row.artifact_id.as_str())
            .collect::<Vec<_>>(),
        ["doc:1", "doc:3"]
    );
    for offset in [4, 5] {
        let request = brauser_protocol::WorklogChoicesPageRequest {
            offset,
            snapshot: Some(page.snapshot.clone()),
        };
        let (last, page) =
            choice_page::bound_rows(rows.clone(), page.snapshot.clone(), &request).unwrap();
        assert!(last.is_empty());
        assert_eq!(page.omitted, offset == 4);
        assert_eq!(page.next_offset, None);
    }
    let all_oversized = rows
        .into_iter()
        .filter(|row| row.text.len() > 8192)
        .collect();
    let (choices, page) =
        choice_page::bound_rows(all_oversized, "a".repeat(64), &first_page()).unwrap();
    assert!(choices.is_empty());
    assert!(page.omitted);
    assert_eq!(page.next_offset, None);
}

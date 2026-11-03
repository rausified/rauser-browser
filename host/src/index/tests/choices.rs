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

use super::*;

#[test]
fn mentions_resolve_through_the_adapter_document() {
    let document: adapter_templates::Value = serde_json::from_value(serde_json::json!({
        "format": 1,
        "artifacts": [{
            "type": "jira.issue",
            "origin": "https://acme.atlassian.net",
            "match": [{"path": "/browse/{key:jira_key}"}],
            "id": "jira:{host}/{key}",
        }],
    }))
    .unwrap();
    let document = adapter_templates::Document::from_value(&document).unwrap();
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let mut issue = activity(
        "00000000-0000-4000-8000-000000000001",
        "PROJ-1",
        "Issue",
        "2026-10-15T10:00:00Z",
    );
    issue.artifact_id = "jira:acme.atlassian.net/PROJ-1".into();
    issue.artifact_type = "jira.issue".into();
    issue.url = "https://acme.atlassian.net/browse/PROJ-1".into();
    issue.modes.clear();
    // An artifact's title can name its own key; that is never a link.
    issue.refs.push(ActivityRef {
        artifact_type: "jira.issue".into(),
        value: "PROJ-1".into(),
    });
    let mut pull = activity(
        "00000000-0000-4000-8000-000000000002",
        "412",
        "PROJ-1 Fix SBOM export pagination",
        "2026-10-15T11:00:00Z",
    );
    pull.refs = ["PROJ-1", "PROJ-1", "UTF-8"]
        .into_iter()
        .map(|value| ActivityRef {
            artifact_type: "jira.issue".into(),
            value: value.into(),
        })
        .collect();
    index.record_activities([&issue, &pull], &[]).unwrap();
    for id in [&issue.artifact_id, &pull.artifact_id] {
        assert_eq!(index.artifact(id).unwrap().unwrap().linked_count, 1, "{id}");
    }
    let query = |query: &str| sql::display(&sql::sql_rows(&index.database_path(), query)[0][0]);
    let linked = "select count(*) from edges e join artifacts a on a.artifact_id = e.target_artifact_id where e.kind = 'mentions'";
    assert_eq!(query(linked), "1");
    // A repeated ref is one mention, not one per copy.
    assert_eq!(
        query("select max(weight) from edges where kind = 'mentions'"),
        "1"
    );
    assert_eq!(
        query(&format!(
            "select count(*) from edges where source_artifact_id = '{}'",
            issue.artifact_id
        )),
        "0"
    );
    // Without a document (or with an ambiguous one) a ref stays
    // unresolved, and the published snapshot follows the change.
    let unresolved = index.clone().with_adapters(None);
    assert_eq!(
        unresolved
            .artifact(&issue.artifact_id)
            .unwrap()
            .unwrap()
            .linked_count,
        0
    );
    assert_eq!(query(linked), "0");
}

const GITHUB_PR_ID: &str = "github.pr:github.com/acme-corp/widgets/7";

fn github_blob(n: u32, at: &str) -> ActivityEvent {
    github_activity(
        n,
        ("github.repo", GITHUB_REPO_ID),
        "https://github.com/acme-corp/widgets/blob/main/README.md",
        at,
        (30, 0),
    )
}

#[test]
fn contained_visits_roll_up_once_into_the_container() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let pr = github_pr(1, 7, "2026-10-15T10:00:00Z");
    let blob = github_blob(2, "2026-10-15T11:00:00Z");
    index.record_activities([&pr, &blob], &[]).unwrap();
    let repo = index.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!(
        (
            repo.focused_seconds,
            repo.background_seconds,
            repo.opens,
            repo.days_opened
        ),
        (155, 61, 2, 1)
    );
    let pull = index.artifact(GITHUB_PR_ID).unwrap().unwrap();
    assert_eq!(
        (pull.focused_seconds, pull.background_seconds, pull.opens),
        (125, 61, 1)
    );
    // Recent shows the repo as one row with its rolled-up day.
    let (items, _, _) = index.recent(None, None, 10).unwrap();
    let rows = items
        .iter()
        .map(|item| (item.artifact_id.as_str(), item.focused_seconds, item.opens))
        .collect::<Vec<_>>();
    assert_eq!(rows, [(GITHUB_REPO_ID, 155, 2), (GITHUB_PR_ID, 125, 1)]);
    // A contained delivery dirties its container's file too.
    assert_eq!(index.regenerate_dirty().unwrap().written, 2);
    let more = github_pr(3, 7, "2026-10-15T12:00:00Z");
    index.record_activity(&more, &[]).unwrap();
    assert_eq!(index.regenerate_dirty().unwrap().written, 2);
}

#[test]
fn container_days_are_a_union() {
    let document = github_document(true);
    let days = |events: &[ActivityEvent]| {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache).with_adapters(Some(&document));
        index.record_activities(events, &[]).unwrap();
        index.artifact(GITHUB_REPO_ID).unwrap().unwrap().days_opened
    };
    assert_eq!(
        days(&[
            github_pr(1, 7, "2026-10-15T10:00:00Z"),
            github_blob(2, "2026-10-16T10:00:00Z"),
        ]),
        2
    );
    assert_eq!(
        days(&[
            github_pr(1, 7, "2026-10-15T10:00:00Z"),
            github_pr(2, 8, "2026-10-15T11:00:00Z"),
        ]),
        1
    );
}

#[test]
fn a_container_never_visited_directly_has_a_usable_title_and_url() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    index
        .record_activity(&github_pr(1, 7, "2026-10-15T10:00:00Z"), &[])
        .unwrap();
    let repo = index.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!(repo.artifact_type, "github.repo");
    assert_eq!(repo.title, "acme-corp/widgets");
    assert_eq!(repo.url, "https://github.com/acme-corp/widgets");
    assert_eq!((repo.focused_seconds, repo.opens), (125, 1));
}

#[test]
fn container_title_ignores_sub_page_titles() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let pr = github_pr(1, 7, "2026-10-15T10:00:00Z");
    let mut blob = github_blob(2, "2026-10-15T11:00:00Z");
    blob.title = Some("widgets/README.md at main · acme-corp/widgets".into());
    index.record_activities([&pr, &blob], &[]).unwrap();
    let repo = index.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!(repo.title, "acme-corp/widgets");
    assert_eq!(repo.url, "https://github.com/acme-corp/widgets");
}

/// Ruling 17 (accepted limit): with no adapter declaring `within`
/// `github.repo`, the repository is not known to be a container, so its
/// record keeps the latest sub-page's title and URL.
#[test]
fn a_repo_without_any_within_declaration_keeps_the_page_title() {
    let document = github_document(false);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let mut blob = github_blob(1, "2026-10-15T11:00:00Z");
    blob.title = Some("widgets/README.md at main · acme-corp/widgets".into());
    index.record_activity(&blob, &[]).unwrap();
    let repo = index.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!(repo.title, "widgets/README.md at main · acme-corp/widgets");
    assert_eq!(
        repo.url,
        "https://github.com/acme-corp/widgets/blob/main/README.md"
    );
}

#[test]
fn artifact_view_names_its_container_and_contents() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let seven = github_pr(1, 7, "2026-10-15T10:00:00Z");
    let mut eight = github_pr(2, 8, "2026-10-15T11:00:00Z");
    eight.focused_seconds = 300;
    let mut nine = github_pr(3, 9, "2026-10-15T12:00:00Z");
    nine.focused_seconds = 125;
    let blob = github_blob(4, "2026-10-15T13:00:00Z");
    index
        .record_activities([&seven, &eight, &nine, &blob], &[])
        .unwrap();

    let pull = index.artifact(GITHUB_PR_ID).unwrap().unwrap();
    assert_eq!(
        pull.within,
        Some(ArtifactRefView {
            artifact_id: GITHUB_REPO_ID.into(),
            artifact_type: "github.repo".into(),
            title: "acme-corp/widgets".into(),
            url: "https://github.com/acme-corp/widgets".into(),
        })
    );
    assert!(pull.contained.is_empty());

    let repo = index.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!(repo.within, None);
    // Most focused first; equal focus breaks by artifact ID.
    let contained = repo
        .contained
        .iter()
        .map(|item| (item.artifact_id.as_str(), item.focused_seconds))
        .collect::<Vec<_>>();
    assert_eq!(
        contained,
        [
            ("github.pr:github.com/acme-corp/widgets/8", 300),
            (GITHUB_PR_ID, 125),
            ("github.pr:github.com/acme-corp/widgets/9", 125),
        ]
    );
    assert_eq!(repo.contained[0].artifact_type, "github.pr");
    assert_eq!(
        repo.contained[0].url,
        "https://github.com/acme-corp/widgets/pull/8"
    );
}

#[test]
fn artifact_view_lists_at_most_twenty_contained_artifacts() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let events = (1..=25)
        .map(|n| {
            let mut event = github_pr(n, n, "2026-10-15T10:00:00Z");
            event.focused_seconds = n;
            event
        })
        .collect::<Vec<_>>();
    index.record_activities(&events, &[]).unwrap();
    let repo = index.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!(repo.contained.len(), 20);
    assert_eq!(repo.contained[0].focused_seconds, 25);
    assert_eq!(repo.contained[19].focused_seconds, 6);
}

#[test]
fn events_that_no_longer_match_their_id_do_not_roll_up() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let mut stale = github_pr(1, 7, "2026-10-15T10:00:00Z");
    stale.url = "https://github.com/acme-corp/widgets/issues/7".into();
    index.record_activity(&stale, &[]).unwrap();
    assert!(index.artifact(GITHUB_REPO_ID).unwrap().is_none());
    assert!(index.artifact(GITHUB_PR_ID).unwrap().is_some());
}

#[test]
fn within_edges_link_child_to_container() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    index
        .record_activity(&github_pr(1, 7, "2026-10-15T10:00:00Z"), &[])
        .unwrap();
    assert_eq!(
        sqlite_check(
            &index.database_path(),
            &format!(
                "select kind, weight from edges where source_artifact_id = '{GITHUB_PR_ID}' and target_artifact_id = '{GITHUB_REPO_ID}'"
            ),
        ),
        "ok\nwithin 1\n"
    );
}

#[test]
fn worklog_day_totals_count_each_visit_once() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let d1 = Date::from_calendar_date(2026, time::Month::October, 15).unwrap();
    let d2 = d1.next_day().unwrap();
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
    // index.db keeps one visit row per logged event, so a day's total
    // is 155 focused seconds, not 280 with the PR counted again.
    assert_eq!(
        sqlite_check(
            &index.database_path(),
            "select sum(focused_secs), count(*) from visits where local_date = '2026-10-15'",
        ),
        "ok\n155 2\n"
    );
    let (text, rows) = index.worklog(d1, d1).unwrap();
    assert!(text.starts_with("Opened 2 artifacts,"), "{text}");
    assert_eq!(rows, 2);
    assert!(
        text.contains(&format!(
            "github.repo {GITHUB_REPO_ID}: acme-corp/widgets — focused 2 min"
        )),
        "{text}"
    );
    // A day with only a PR opened one artifact, though the repo it sits
    // within is still listed with the rolled-up time.
    let (text, rows) = index.worklog(d2, d2).unwrap();
    assert!(
        text.starts_with("Opened 1 artifact, and focused on 0 left open"),
        "{text}"
    );
    assert_eq!(rows, 2);
    assert!(text.contains(GITHUB_REPO_ID), "{text}");
}

fn github_summary(artifact: (&str, &str), url: &str, at: &str) -> SummaryEvent {
    SummaryEvent {
        title: "Fix the widget".into(),
        summary_path: "summaries/widget.summary.md".into(),
        ..adapter_summary(artifact.1, artifact.0, url, at)
    }
}

/// A summary of a page an adapter recognizes as an artifact within a
/// container attaches to that artifact (spec §7.2: the page's record
/// key), never to the container. Like measured activity it is not an
/// open, and unlike activity it is not rolled up: the container's
/// totals, days, last seen, Summary line, and Timeline do not change.
#[test]
fn a_summary_within_a_repo_attaches_to_the_contained_artifact() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    index
        .record_activity(&github_pr(1, 7, "2026-10-15T10:00:00Z"), &[])
        .unwrap();
    let summary = github_summary(
        ("github.pr", GITHUB_PR_ID),
        "https://github.com/acme-corp/widgets/pull/7",
        "2026-10-16T09:00:00Z",
    );
    index.record_summary(&summary).unwrap();

    let (pull, pull_summary) = index.artifact_with_summary(GITHUB_PR_ID).unwrap();
    let pull = pull.unwrap();
    assert_eq!(pull_summary, Some(expected_view(&summary)));
    assert_eq!(
        (pull.opens, pull.days_opened, pull.focused_seconds),
        (1, 1, 125)
    );
    assert_eq!(pull.last_seen, "2026-10-16");
    assert_eq!(
        pull.within
            .as_ref()
            .map(|within| within.artifact_id.as_str()),
        Some(GITHUB_REPO_ID)
    );

    let (repo, repo_summary) = index.artifact_with_summary(GITHUB_REPO_ID).unwrap();
    let repo = repo.unwrap();
    assert_eq!(repo_summary, None);
    assert_eq!(
        (
            repo.focused_seconds,
            repo.background_seconds,
            repo.opens,
            repo.days_opened
        ),
        (125, 61, 1, 1)
    );
    assert_eq!(
        (repo.first_seen.as_str(), repo.last_seen.as_str()),
        ("2026-10-15", "2026-10-15")
    );
    assert_eq!(repo.title, "acme-corp/widgets");

    // Recent: the summary's day lists only the PR, with its summary.
    let (items, _, _) = index.recent(None, None, 32).unwrap();
    let rows = items
        .iter()
        .map(|item| {
            (
                item.local_date.as_str(),
                item.artifact_id.as_str(),
                item.opens,
                item.summary.is_some(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [
            ("2026-10-16", GITHUB_PR_ID, 0, true),
            // Equal focus sorts by title: "Page" before "acme-corp/…".
            ("2026-10-15", GITHUB_PR_ID, 1, false),
            ("2026-10-15", GITHUB_REPO_ID, 1, false),
        ]
    );

    // The worklog counts it as a summary, not an open, and lists no
    // repository row for a day with nothing rolled up.
    let d2 = Date::from_calendar_date(2026, time::Month::October, 16).unwrap();
    let (text, rows) = index.worklog(d2, d2).unwrap();
    assert!(
        text.starts_with(
            "Opened 0 artifacts, focused on 0 left open from an earlier day, and summarized 1 page."
        ),
        "{text}"
    );
    assert_eq!(rows, 1);
    assert!(!text.contains(GITHUB_REPO_ID), "{text}");

    index.regenerate_dirty().unwrap();
    let pr_file = github_file(root.path(), "github-pr");
    assert!(pr_file.contains("Summarized: Fix the widget"), "{pr_file}");
    assert!(pr_file.contains("Summary: ["), "{pr_file}");
    let repo_file = github_file(root.path(), "github-repo");
    assert!(!repo_file.contains("Summar"), "{repo_file}");
    assert!(repo_file.contains("opens: 1\n"), "{repo_file}");
    assert!(repo_file.contains("days_opened: 1\n"), "{repo_file}");
}

/// A summary-only contained artifact (summarized before any visit was
/// delivered) creates no container record and adds nothing to one.
#[test]
fn a_summary_only_contained_artifact_does_not_inflate_its_container() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    let summary = github_summary(
        ("github.pr", GITHUB_PR_ID),
        "https://github.com/acme-corp/widgets/pull/7",
        "2026-10-16T09:00:00Z",
    );
    index.record_summary(&summary).unwrap();
    assert!(index.artifact(GITHUB_REPO_ID).unwrap().is_none());
    let pull = index.artifact(GITHUB_PR_ID).unwrap().unwrap();
    assert_eq!(
        (pull.opens, pull.days_opened, pull.focused_seconds),
        (0, 0, 0)
    );

    // With a repo visit as well, the container counts only that visit.
    index
        .record_activity(&github_blob(2, "2026-10-16T10:00:00Z"), &[])
        .unwrap();
    let repo = index.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!(
        (repo.focused_seconds, repo.opens, repo.days_opened),
        (30, 1, 1)
    );
    assert!(repo.contained.is_empty(), "{:?}", repo.contained);
    let (items, _, _) = index.recent(None, None, 32).unwrap();
    let repo_row = items
        .iter()
        .find(|item| item.artifact_id == GITHUB_REPO_ID)
        .unwrap();
    assert_eq!((repo_row.focused_seconds, repo_row.opens), (30, 1));
    assert_eq!(repo_row.summary, None);
}

/// A repository page the catch-all recognizes (a file view) is the
/// container's own record key (DESIGN.md §6.2), so its summary attaches
/// to the repository itself, which keeps its ID-derived title.
#[test]
fn a_summary_of_a_repo_page_attaches_to_the_repository() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    index
        .record_activity(&github_pr(1, 7, "2026-10-15T10:00:00Z"), &[])
        .unwrap();
    let summary = SummaryEvent {
        title: "widgets/README.md at main · acme-corp/widgets".into(),
        ..github_summary(
            ("github.repo", GITHUB_REPO_ID),
            "https://github.com/acme-corp/widgets/blob/main/README.md",
            "2026-10-16T09:00:00Z",
        )
    };
    index.record_summary(&summary).unwrap();
    let (repo, repo_summary) = index.artifact_with_summary(GITHUB_REPO_ID).unwrap();
    let repo = repo.unwrap();
    assert_eq!(repo_summary, Some(expected_view(&summary)));
    assert_eq!(repo.title, "acme-corp/widgets");
    assert_eq!(repo.url, "https://github.com/acme-corp/widgets");
    assert_eq!(
        (repo.focused_seconds, repo.opens, repo.days_opened),
        (125, 1, 1)
    );
    assert_eq!(repo.last_seen, "2026-10-16");
    let (_, pull_summary) = index.artifact_with_summary(GITHUB_PR_ID).unwrap();
    assert_eq!(pull_summary, None);
}

/// Index a PR and a repo page under the containing document and write
/// both files, then reopen under `after` and regenerate. Returns the
/// pass and the repo's file, which must then hold only its own visit.
fn regenerate_after_losing_roll_ups(
    after: &adapter_templates::Document,
) -> (RegenerationReport, String) {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let before = github_document(true);
    let old = index(&root, &cache).with_adapters(Some(&before));
    let pr = github_pr(1, 7, "2026-10-15T10:00:00Z");
    let mut blob = github_blob(2, "2026-10-15T11:00:00Z");
    blob.title = Some("widgets/README.md at main · acme-corp/widgets".into());
    blob.focused_seconds = 90;
    index_and_write(&old, &[&pr, &blob]);
    let new = index(&root, &cache).with_adapters(Some(after));
    let report = new.regenerate_dirty().unwrap();
    let repo = new.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!((repo.focused_seconds, repo.opens), (90, 1));
    let file = artifact_files(root.path())
        .into_iter()
        .find(|path| path.to_string_lossy().contains("github-repo"))
        .unwrap();
    (report, fs::read_to_string(file).unwrap())
}

fn index_and_write(index: &ArtifactIndex, events: &[&ActivityEvent]) {
    index
        .record_activities(events.iter().copied(), &[])
        .unwrap();
    assert_eq!(index.regenerate_dirty().unwrap().written, 2);
    let file = artifact_files(Path::new(&index.storage.root))
        .into_iter()
        .find(|path| path.to_string_lossy().contains("github-repo"))
        .unwrap();
    let rolled = fs::read_to_string(file).unwrap();
    assert!(rolled.contains("focused_min: 3\n"), "{rolled}");
    assert!(rolled.contains("title: \"acme-corp/widgets\""), "{rolled}");
}

fn assert_direct_only(report: &RegenerationReport, file: &str, title: &str) {
    assert_eq!((report.written, report.unchanged), (1, 0), "{report:?}");
    assert!(file.contains("focused_min: 1\n"), "{file}");
    assert!(file.contains("opens: 1\n"), "{file}");
    assert!(file.contains("days_opened: 1\n"), "{file}");
    assert!(
        file.contains("- 2026-10-15 · focused 1 min · opened 1×\n"),
        "{file}"
    );
    assert!(file.contains(&format!("title: \"{title}\"\n")), "{file}");
}

#[test]
fn removing_within_rewrites_the_container_with_only_its_direct_visits() {
    let (report, file) = regenerate_after_losing_roll_ups(&github_document(false));
    // No longer a container type, so its own latest title returns.
    assert_direct_only(
        &report,
        &file,
        "widgets/README.md at main · acme-corp/widgets",
    );
}

#[test]
fn narrowing_scope_rewrites_the_container_with_only_its_direct_visits() {
    // The repo page stays logged as the repo; the PR is now out of scope
    // and no longer rolls up.
    let (report, file) =
        regenerate_after_losing_roll_ups(&scoped_github_document(true, &["other-org"]));
    // github.repo is still a declared container type, so it keeps the
    // container's own name; only the roll-up goes.
    assert_direct_only(&report, &file, "acme-corp/widgets");
}

/// A container with only direct visits has no `within` edge, yet its
/// file is titled from its ID while some adapter declares its type, so
/// losing that declaration must rewrite it with its page title.
#[test]
fn losing_within_retitles_a_container_with_only_direct_visits() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let before = github_document(true);
    let old = index(&root, &cache).with_adapters(Some(&before));
    let mut blob = github_blob(2, "2026-10-15T11:00:00Z");
    blob.title = Some("widgets/README.md at main · acme-corp/widgets".into());
    old.record_activity(&blob, &[]).unwrap();
    assert_eq!(old.regenerate_dirty().unwrap().written, 1);
    let repo_file = || {
        let file = artifact_files(root.path())
            .into_iter()
            .find(|path| path.to_string_lossy().contains("github-repo"))
            .unwrap();
        fs::read_to_string(file).unwrap()
    };
    assert!(
        repo_file().contains("title: \"acme-corp/widgets\"\n"),
        "{}",
        repo_file()
    );
    let after = github_document(false);
    let new = index(&root, &cache).with_adapters(Some(&after));
    let report = new.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged), (1, 0), "{report:?}");
    assert!(
        repo_file().contains("title: \"widgets/README.md at main · acme-corp/widgets\"\n"),
        "{}",
        repo_file()
    );
}

/// A container known only from rolled-up activity has no record once
/// that roll-up goes, so its generated file goes too rather than keep
/// totals the index no longer has.
#[test]
fn losing_the_only_roll_up_removes_the_container_file() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let before = github_document(true);
    let old = index(&root, &cache).with_adapters(Some(&before));
    old.record_activity(&github_pr(1, 7, "2026-10-15T10:00:00Z"), &[])
        .unwrap();
    assert_eq!(old.regenerate_dirty().unwrap().written, 2);
    let is_repo = |path: &PathBuf| path.to_string_lossy().contains("github-repo");
    assert!(artifact_files(root.path()).iter().any(is_repo));
    let after = scoped_github_document(true, &["other-org"]);
    let new = index(&root, &cache).with_adapters(Some(&after));
    new.regenerate_dirty().unwrap();
    assert!(new.artifact(GITHUB_REPO_ID).unwrap().is_none());
    let files = artifact_files(root.path());
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(!files.iter().any(is_repo), "{files:?}");
    // The pass is complete: a second one has nothing left to do.
    let again = new.regenerate_dirty().unwrap();
    assert_eq!((again.written, again.conflicts), (0, 0), "{again:?}");
}

/// A container whose only activity was a roll-up, rebuilt by
/// `brauser reindex` (`clear_cache`: or after its cache was deleted)
/// under adapters that no longer make that roll-up. Returns the
/// container file's path and whether it is still there.
fn reindex_after_losing_the_only_roll_up(clear_cache: bool, replace: bool) -> (PathBuf, bool) {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let before = github_document(true);
    let old = index(&root, &cache).with_adapters(Some(&before));
    old.record_activity(&github_pr(1, 7, "2026-10-15T10:00:00Z"), &[])
        .unwrap();
    assert_eq!(old.regenerate_dirty().unwrap().written, 2);
    let repo = artifact_files(root.path())
        .into_iter()
        .find(|path| path.to_string_lossy().contains("github-repo"))
        .unwrap();
    if replace {
        fs::write(&repo, "my own notes\n").unwrap();
    }
    if clear_cache {
        for entry in fs::read_dir(cache.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                fs::remove_dir_all(path).unwrap();
            } else {
                fs::remove_file(path).unwrap();
            }
        }
    }
    let after = scoped_github_document(true, &["other-org"]);
    let new = index(&root, &cache).with_adapters(Some(&after));
    new.reindex().unwrap();
    assert!(new.artifact(GITHUB_REPO_ID).unwrap().is_none());
    let exists = repo.exists();
    if replace && exists {
        assert_eq!(fs::read_to_string(&repo).unwrap(), "my own notes\n");
    }
    (repo, exists)
}

#[test]
fn reindex_after_losing_the_only_roll_up_removes_the_container_file() {
    let (repo, exists) = reindex_after_losing_the_only_roll_up(false, false);
    assert!(!exists, "{repo:?}");
}

#[test]
fn reindex_without_a_cache_still_removes_a_vanished_container_file() {
    let (repo, exists) = reindex_after_losing_the_only_roll_up(true, false);
    assert!(!exists, "{repo:?}");
}

#[test]
fn a_cache_rebuild_still_removes_a_vanished_container_file() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let before = github_document(true);
    let old = index(&root, &cache).with_adapters(Some(&before));
    old.record_activity(&github_pr(1, 7, "2026-10-15T10:00:00Z"), &[])
        .unwrap();
    assert_eq!(old.regenerate_dirty().unwrap().written, 2);
    let is_repo = |path: &PathBuf| path.to_string_lossy().contains("github-repo");
    for entry in fs::read_dir(cache.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            fs::remove_file(path).unwrap();
        }
    }
    let after = scoped_github_document(true, &["other-org"]);
    let new = index(&root, &cache).with_adapters(Some(&after));
    new.regenerate_dirty().unwrap();
    let files = artifact_files(root.path());
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(!files.iter().any(is_repo), "{files:?}");
}

#[test]
fn reindex_leaves_a_replaced_vanished_container_file_alone() {
    for clear_cache in [false, true] {
        let (repo, exists) = reindex_after_losing_the_only_roll_up(clear_cache, true);
        assert!(exists, "{repo:?}");
    }
}

/// A vanished container's path no longer holding Brauser's file (the
/// user replaced it) is left alone.
#[test]
fn a_replaced_container_file_is_not_removed() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let before = github_document(true);
    let old = index(&root, &cache).with_adapters(Some(&before));
    old.record_activity(&github_pr(1, 7, "2026-10-15T10:00:00Z"), &[])
        .unwrap();
    assert_eq!(old.regenerate_dirty().unwrap().written, 2);
    let repo = artifact_files(root.path())
        .into_iter()
        .find(|path| path.to_string_lossy().contains("github-repo"))
        .unwrap();
    fs::write(&repo, "my own notes\n").unwrap();
    let after = scoped_github_document(true, &["other-org"]);
    let new = index(&root, &cache).with_adapters(Some(&after));
    new.regenerate_dirty().unwrap();
    assert_eq!(fs::read_to_string(&repo).unwrap(), "my own notes\n");
}

#[test]
fn adding_within_regenerates_container_files() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let before = github_document(false);
    let old = index(&root, &cache).with_adapters(Some(&before));
    write_log(
        root.path(),
        "2026-10-15.md",
        &[&github_pr(1, 7, "2026-10-15T10:00:00Z")],
    );
    assert_eq!(old.reindex().unwrap().written, 1);
    assert!(old.artifact(GITHUB_REPO_ID).unwrap().is_none());
    let after = github_document(true);
    let new = index(&root, &cache).with_adapters(Some(&after));
    let report = new.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged), (1, 0), "{report:?}");
    let files = artifact_files(root.path());
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(
        files
            .iter()
            .any(|path| path.to_string_lossy().contains("github-repo")),
        "{files:?}"
    );
}

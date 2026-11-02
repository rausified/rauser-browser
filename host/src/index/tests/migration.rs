use super::*;

/// A version 5 cache a #126 host published, holding a container with
/// only rolled-up activity, migrates in place: its container records
/// survive, and the container's file is regenerated with the rest.
#[test]
fn a_version_5_cache_with_containers_is_migrated() {
    let document = github_document(true);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache).with_adapters(Some(&document));
    index
        .record_activity(&github_pr(1, 7, "2026-10-15T10:00:00Z"), &[])
        .unwrap();
    assert_eq!(index.regenerate_dirty().unwrap().written, 2);
    let current = github_file(root.path(), "github-repo");
    let path = artifact_files(root.path())
        .into_iter()
        .find(|path| path.to_string_lossy().contains("github-repo"))
        .unwrap();
    // As a version 5 build rendered it.
    let stale = current.replace(
        "title: \"acme-corp/widgets\"\n",
        "title: \"acme-corp/widgets (v5)\"\n",
    );
    assert_ne!(stale, current);
    fs::write(&path, stale).unwrap();
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
    assert_eq!(state["containers"], serde_json::json!([GITHUB_REPO_ID]));
    assert!(state["container_files"][GITHUB_REPO_ID].is_string());
    state["version"] = 5.into();
    fs::write(index.state_path(), serde_json::to_vec(&state).unwrap()).unwrap();

    LOG_BYTES_READ.with(|count| count.set(0));
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged), (1, 1), "{report:?}");
    assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), current);
    let migrated = read_state(&index.state_path()).unwrap().unwrap();
    assert_eq!(migrated.version, STATE_VERSION);
    assert!(migrated.containers.contains(GITHUB_REPO_ID));
    assert!(migrated.container_files.contains_key(GITHUB_REPO_ID));
    let repo = index.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!((repo.focused_seconds, repo.opens), (125, 1));
}

#[test]
fn a_version_1_state_cache_is_rescanned() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let url = "https://news.ycombinator.com/item?id=50251344";
    let url_hash = sha256_hex(url.as_bytes());
    let event = SummaryEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        artifact_id: crate::summary::web_page_artifact_id(url).unwrap(),
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

    // A cache written by a build that predates `EventData::Summary`
    // (version 1) must never be trusted as-is: it cannot contain this
    // summary, so loading it unchanged would hide it until a manual
    // reindex. Write one by hand and confirm it is rescanned instead.
    let stale_cache_dir = cache.path();
    fs::create_dir_all(stale_cache_dir).unwrap();
    let stale_state = serde_json::json!({
        "version": 1,
        "root": root.path().to_string_lossy(),
        "root_identity": open_selected_root(root.path(), None).unwrap().1,
        "log_dir": "log",
        "artifacts_dir": "artifacts",
        "events": {},
        "dirty": [],
    });
    fs::write(
        stale_cache_dir.join("index-state.json"),
        serde_json::to_vec(&stale_state).unwrap(),
    )
    .unwrap();

    let index = index(&root, &cache);
    let report = index.regenerate_dirty().unwrap();
    assert_eq!(
        report.written, 1,
        "a v1 cache must be rescanned, not trusted"
    );
    assert_eq!(artifact_files(root.path()).len(), 1);
}

#[test]
fn a_version_5_state_cache_is_migrated() {
    assert_eq!(STATE_VERSION, 8);
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-000000000401",
        "V5",
        "Version five",
        "2026-10-20T12:00:00Z",
    );
    let other = activity(
        "00000000-0000-4000-8000-000000000402",
        "V5B",
        "Version five, rendered by a v5 build",
        "2026-10-20T13:00:00Z",
    );
    write_log(root.path(), "2026-10-20.md", &[&event, &other]);
    assert_eq!(index.reindex().unwrap().written, 2);
    let mut files = artifact_files(root.path());
    files.sort();
    let current = files
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>();
    let (stale_at, stale_text) = files
        .iter()
        .zip(&current)
        .find(|(_, text)| text.contains("rendered by a v5 build"))
        .unwrap();
    // A file as a version 5 build rendered it: PR 4 changed how every
    // artifact file escapes its title and Why line.
    fs::write(
        stale_at,
        stale_text.replace("Version five, rendered", "Version five rendered"),
    )
    .unwrap();
    // The clean cache a version 5 build left. It can hold no summary
    // (#59 had no caller).
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
    state["version"] = 5.into();
    fs::write(index.state_path(), serde_json::to_vec(&state).unwrap()).unwrap();

    // Migration rechecks old logs for gaps; only the file whose
    // rendering changed is rewritten.
    LOG_BYTES_READ.with(|count| count.set(0));
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged), (1, 1));
    assert!(LOG_BYTES_READ.with(|count| count.get()) > 0);
    for (path, text) in files.iter().zip(&current) {
        assert_eq!(&fs::read_to_string(path).unwrap(), text);
    }
    assert_eq!(
        read_state(&index.state_path()).unwrap().unwrap().version,
        STATE_VERSION
    );
    assert!(index.artifact(&event.artifact_id).unwrap().is_some());
    assert_eq!(index.regenerate_dirty().unwrap().written, 0);
}

#[test]
fn an_m1_legacy_visit_counts_one_open_with_a_dated_timeline_line() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://example.org/legacy-page";
    let artifact_id = crate::summary::web_page_artifact_id(url).unwrap();
    let url_hash = sha256_hex(url.as_bytes());
    let legacy_line = format!(
        "\n- 2026-10-20T10:00:00Z — Legacy page — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
        sha256_hex("legacy-marker-open".as_bytes()),
    );
    let dir = root.path().join("log/2026/10");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("2026-10-20.md"),
        format!("{LOG_HEADER}\n{legacy_line}"),
    )
    .unwrap();
    assert_eq!(index.reindex().unwrap().written, 1);
    let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
    assert!(
        markdown.contains("  opens: 1\n  days_opened: 1\n"),
        "{markdown}"
    );
    assert!(
        markdown.ends_with("## Timeline\n\n- 2026-10-20 · opened 1×\n"),
        "{markdown}"
    );
    assert_eq!(index.artifact(&artifact_id).unwrap().unwrap().opens, 1);
    assert_eq!(index.recent(None, None, 32).unwrap().0[0].opens, 1);
    let (worklog, _) = index
        .worklog(
            parse_date("2026-10-20").unwrap(),
            parse_date("2026-10-20").unwrap(),
        )
        .unwrap();
    assert!(worklog.starts_with("Opened 1 artifact,"), "{worklog}");
    assert_eq!(
        sqlite_check(&index.database_path(), "select opens from visits"),
        "ok\n1\n"
    );
}

#[test]
fn a_version_4_cache_migrates_so_existing_m1_artifact_files_gain_their_opens() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let url = "https://example.org/legacy-page";
    let url_hash = sha256_hex(url.as_bytes());
    let dir = root.path().join("log/2026/10");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
            dir.join("2026-10-20.md"),
            format!(
                "{LOG_HEADER}\n\n- 2026-10-20T10:00:00Z — Legacy page — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
                sha256_hex("legacy-marker-upgrade".as_bytes()),
            ),
        )
        .unwrap();
    index.reindex().unwrap();
    // Simulate the file and clean cache a version 4 build left: legacy
    // visits rendered no open and no Timeline line.
    let path = artifact_files(root.path()).pop().unwrap();
    let current = fs::read_to_string(&path).unwrap();
    let old = current
        .replace("  opens: 1\n", "  opens: 0\n")
        .replace("- 2026-10-20 · opened 1×\n", "");
    assert_ne!(old, current);
    fs::write(&path, &old).unwrap();
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
    state["version"] = 4.into();
    state["dirty"] = serde_json::json!([]);
    fs::write(index.state_path(), serde_json::to_vec(&state).unwrap()).unwrap();

    // The migration rechecks old checkpoints once for skipped gaps.
    LOG_BYTES_READ.with(|count| count.set(0));
    assert_eq!(index.regenerate_dirty().unwrap().written, 1);
    assert!(LOG_BYTES_READ.with(|count| count.get()) > 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), current);
}

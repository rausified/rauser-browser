use super::*;

#[test]
fn case_collisions_external_edits_and_unchanged_files_are_handled() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let first = activity(
        "00000000-0000-4000-8000-000000000001",
        "AbC",
        "![x](https://bad.invalid)",
        "2026-10-15T12:00:00Z",
    );
    let second = activity(
        "00000000-0000-4000-8000-000000000002",
        "abc",
        "Second",
        "2026-10-15T13:00:00Z",
    );
    index.record_activities([&first, &second], &[]).unwrap();
    let report = index.regenerate_dirty().unwrap();
    assert_eq!(report.written, 2);
    let files = artifact_files(root.path());
    assert_eq!(files.len(), 2);
    assert_ne!(files[0].file_name(), files[1].file_name());
    let first_path = files
        .iter()
        .find(|path| fs::read_to_string(path).unwrap().contains("AbC"))
        .unwrap();
    let original = fs::read_to_string(first_path).unwrap();
    // The title is YAML-escaped with no raw `![` (markdown::yaml_string).
    assert!(original.contains("title: \"\\u0021[x]"));
    assert!(!original.contains("\n![x]"));

    fs::write(first_path, format!("{original}\nexternal edit\n")).unwrap();
    let replacement = activity(
        "00000000-0000-4000-8000-000000000003",
        "AbC",
        "Safe",
        "2026-10-15T14:00:00Z",
    );
    index.record_activity(&replacement, &[]).unwrap();
    assert_eq!(index.regenerate_dirty().unwrap().written, 1);
    let replaced = fs::read_to_string(first_path).unwrap();
    assert!(!replaced.contains("external edit"));
    let modified = fs::metadata(first_path).unwrap().modified().unwrap();
    index.record_activity(&replacement, &[]).unwrap();
    assert_eq!(index.regenerate_dirty().unwrap().unchanged, 1);
    assert_eq!(
        fs::metadata(first_path).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn a_heavy_day_only_marks_the_artifact_dirty_until_regeneration() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let events = (0..96)
        .map(|hour| {
            activity(
                &format!("00000000-0000-4000-8000-{hour:012}"),
                "BUSY",
                "Busy day",
                &format!("2026-10-15T{:02}:00:00Z", hour % 24),
            )
        })
        .collect::<Vec<_>>();
    index.record_activities(events.iter(), &[]).unwrap();
    assert!(!root.path().join("artifacts").exists());
    let output = Command::new("python3")
            .args([
                "-c",
                "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('select count(*) from dirty_artifacts').fetchone()[0])",
            ])
            .arg(index.database_path())
            .output()
            .unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "1");
    assert_eq!(index.regenerate_dirty().unwrap().written, 1);
}

fn typed_activity(id: u32, native_id: &str, artifact_type: &str) -> ActivityEvent {
    let mut event = activity(
        &format!("00000000-0000-4000-8000-{id:012}"),
        native_id,
        native_id,
        "2026-10-15T12:00:00-07:00",
    );
    event.artifact_type = artifact_type.into();
    event
}

fn dirty_ids(index: &ArtifactIndex) -> Vec<String> {
    read_state(&index.state_path())
        .unwrap()
        .unwrap()
        .dirty
        .into_iter()
        .collect()
}

#[test]
fn an_unwritable_file_stays_dirty_while_the_pass_publishes_the_rest() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let events = [
        typed_activity(1, "A", "work.item"),
        typed_activity(2, "B", "blocked.item"),
        typed_activity(3, "C", "work.item"),
    ];
    index.record_activities(events.iter(), &[]).unwrap();
    // A file where B's type directory belongs makes B's write fail.
    fs::create_dir_all(root.path().join("artifacts")).unwrap();
    fs::write(root.path().join("artifacts/blocked-item"), "in the way").unwrap();
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.failed), (2, 1));
    assert_eq!(
        dirty_ids(&index),
        ["work:example.com/B"],
        "A's and C's completed writes are published; only B waits"
    );
    fs::remove_file(root.path().join("artifacts/blocked-item")).unwrap();
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged, report.failed), (1, 0, 0));
}

#[test]
fn a_pass_killed_midway_resumes_after_the_files_it_wrote() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let events = ["A", "B", "C", "D"]
        .iter()
        .zip(1..)
        .map(|(name, id)| typed_activity(id, name, "work.item"))
        .collect::<Vec<_>>();
    index.record_activities(events.iter(), &[]).unwrap();
    let (state, _) = index.load_current_state().unwrap();
    // Chrome kills a host whose port closed; a panic stands in for that
    // here, after the first file, with the first checkpoint due at once.
    let killed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        index.regenerate_state_with(state, false, std::time::Duration::ZERO, |written| {
            assert!(written < 1, "killed");
        })
    }));
    assert!(killed.is_err());
    assert_eq!(
        dirty_ids(&index),
        [
            "work:example.com/B",
            "work:example.com/C",
            "work:example.com/D"
        ]
    );
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged), (3, 0));
}

#[test]
fn a_pass_ending_on_a_checkpoint_does_not_publish_twice() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    index
        .record_activity(&typed_activity(1, "A", "work.item"), &[])
        .unwrap();
    let (state, _) = index.load_current_state().unwrap();
    PUBLICATIONS.with(|count| count.set(0));
    index
        .regenerate_state_with(state, false, Duration::ZERO, |_| {})
        .unwrap();
    assert_eq!(PUBLICATIONS.with(|count| count.get()), 1);
    assert!(dirty_ids(&index).is_empty());
    // Nothing dirty and nothing to repair: nothing to publish.
    let (state, _) = index.load_current_state().unwrap();
    PUBLICATIONS.with(|count| count.set(0));
    index
        .regenerate_state_with(state, false, REGENERATION_CHECKPOINT, |_| {})
        .unwrap();
    assert_eq!(PUBLICATIONS.with(|count| count.get()), 0);
}

#[test]
fn checkpoints_back_off_when_publishing_the_cache_is_slow() {
    let minimum = Duration::from_millis(500);
    assert_eq!(
        checkpoint_interval(minimum, Duration::from_millis(10)),
        minimum
    );
    assert_eq!(
        checkpoint_interval(minimum, Duration::from_millis(300)),
        Duration::from_millis(1200)
    );
}

#[test]
fn the_daily_pass_is_due_once_per_local_day() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let at = |text: &str| OffsetDateTime::parse(text, &Rfc3339).unwrap();
    assert!(index.daily_pass_due(at("2026-10-15T20:00:00Z")));
    index
        .record_activity(&typed_activity(1, "A", "work.item"), &[])
        .unwrap();
    index.regenerate_dirty().unwrap();
    // The pass ran today in the user's -07:00 offset (from their latest
    // activity), so 23:30 local the same day is not a new day, even
    // though UTC has already moved on.
    let marker = fs::read_to_string(cache.path().join("regenerated.json")).unwrap();
    let today = OffsetDateTime::now_utc()
        .to_offset(time::UtcOffset::from_whole_seconds(-7 * 3600).unwrap())
        .date();
    assert!(marker.contains(&today.to_string()), "{marker}");
    let local_late = today
        .with_hms(23, 30, 0)
        .unwrap()
        .assume_offset(time::UtcOffset::from_whole_seconds(-7 * 3600).unwrap());
    assert!(!index.daily_pass_due(local_late));
    assert!(index.daily_pass_due(local_late + time::Duration::hours(1)));
    // A pass for another home folder says nothing about this one.
    let other_root = TempDir::new().unwrap();
    let other = ArtifactIndex::open_in(&storage(other_root.path()), None, cache.path()).unwrap();
    assert!(other.daily_pass_due(local_late));
    let mut moved = storage(root.path());
    moved.artifacts_dir = "elsewhere".into();
    let moved = ArtifactIndex::open_in(&moved, None, cache.path()).unwrap();
    assert!(moved.daily_pass_due(local_late));
    fs::write(cache.path().join("regenerated.json"), "not json").unwrap();
    assert!(index.daily_pass_due(local_late));
}

#[test]
fn stale_artifact_and_index_temporaries_are_swept() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    index
        .record_activity(&typed_activity(1, "A", "work.item"), &[])
        .unwrap();
    index.regenerate_dirty().unwrap();
    let site = root.path().join("artifacts/work-item/example-com");
    let old = crate::vault::STALE_TEMPORARY_AGE + std::time::Duration::from_secs(60);
    let make = |dir: &Path, name: String, stale: bool| {
        let path = dir.join(name);
        fs::write(&path, "partial").unwrap();
        if stale {
            crate::vault::tests::age_file(&path, old);
        }
        path
    };
    let stale_artifact = make(
        &site,
        format!(".{NAMESPACE}-artifact-{}.tmp", Uuid::new_v4()),
        true,
    );
    let fresh_artifact = make(
        &site,
        format!(".{NAMESPACE}-artifact-{}.tmp", Uuid::new_v4()),
        false,
    );
    let user_file = make(&site, format!(".{NAMESPACE}-artifact-mine.tmp"), true);
    let stale_index = make(
        cache.path(),
        format!(".{NAMESPACE}-index-{}.tmp", Uuid::new_v4()),
        true,
    );
    let fresh_index = make(
        cache.path(),
        format!(".{NAMESPACE}-index-{}.tmp", Uuid::new_v4()),
        false,
    );
    assert_eq!(index.sweep_stale_temporaries().unwrap(), 2);
    assert!(!stale_artifact.exists());
    assert!(!stale_index.exists());
    assert!(fresh_artifact.exists());
    assert!(fresh_index.exists());
    assert!(user_file.exists());
    assert_eq!(artifact_files(root.path()).len(), 1);
}

#[test]
fn an_unowned_file_at_the_artifact_path_is_never_adopted() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-000000000099",
        "CONFLICT",
        "Conflict",
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&event, &[]).unwrap();
    let derived = derive(&index.load_or_rebuild().unwrap(), None);
    let artifact = derived.artifacts.get(&event.artifact_id).unwrap();
    let occupied = root.path().join(artifact_path("artifacts", artifact));
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, "someone else's file\n").unwrap();

    let report = index.regenerate_dirty().unwrap();
    assert_eq!(report.conflicts, 1);
    assert_eq!(
        report.conflict_paths,
        vec![
            artifact_path("artifacts", artifact)
                .to_string_lossy()
                .into_owned()
        ]
    );
    assert_eq!(
        fs::read_to_string(occupied).unwrap(),
        "someone else's file\n"
    );
}

#[test]
fn ownership_requires_one_matching_brauser_identity_block() {
    let id = "work:example.com/7";
    let owned = format!(
        "---\ntitle: \"Seven\"\nbrauser:\n  kind: artifact\n  artifact_id: {}\n---\n",
        serde_json::to_string(id).unwrap()
    );
    assert!(owned_artifact(owned.as_bytes(), id));

    let unrelated = format!(
        "---\ntitle: \"Seven\"\nother:\n  kind: artifact\n  artifact_id: {}\n---\n",
        serde_json::to_string(id).unwrap()
    );
    assert!(!owned_artifact(unrelated.as_bytes(), id));

    let duplicate = format!(
        "---\nbrauser:\n  kind: artifact\n  artifact_id: {}\nbrauser:\n  kind: artifact\n  artifact_id: {}\n---\n",
        serde_json::to_string(id).unwrap(),
        serde_json::to_string(id).unwrap()
    );
    assert!(!owned_artifact(duplicate.as_bytes(), id));
}

// This fixture removes POSIX directory write bits; Windows uses ACLs.
#[cfg(unix)]
#[test]
fn one_failing_artifact_path_does_not_block_the_others() {
    use std::os::unix::fs::PermissionsExt;
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let blocked = activity(
        "00000000-0000-4000-8000-000000000001",
        "AAA",
        "Blocked",
        "2026-10-15T10:00:00Z",
    );
    let mut open = activity(
        "00000000-0000-4000-8000-000000000002",
        "BBB",
        "Open",
        "2026-10-15T10:00:00Z",
    );
    open.artifact_id = "work:other.example/BBB".into();
    open.url = "https://other.example/items/BBB".into();
    open.modes[0].url = open.url.clone();
    index.record_activities([&blocked, &open], &[]).unwrap();
    let derived = derive(&index.load_or_rebuild().unwrap(), None);
    let relative = artifact_path("artifacts", &derived.artifacts[&blocked.artifact_id]);
    let site = root.path().join(relative.parent().unwrap());
    fs::create_dir_all(&site).unwrap();
    fs::set_permissions(&site, fs::Permissions::from_mode(0o555)).unwrap();
    let report = index.regenerate_dirty();
    fs::set_permissions(&site, fs::Permissions::from_mode(0o755)).unwrap();
    let report = report.unwrap();
    assert_eq!(report.written, 1);
    assert_eq!(report.failed, 1);
    assert_eq!(report.failed_paths.len(), 1);
    assert!(
        report.failed_paths[0].contains("aaa"),
        "{:?}",
        report.failed_paths
    );
    assert_eq!(
        read_state(&index.state_path()).unwrap().unwrap().dirty,
        BTreeSet::from([blocked.artifact_id.clone()])
    );
    assert_eq!(artifact_files(root.path()).len(), 1);
    // Once the path is writable again, the remaining artifact catches up.
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.failed), (1, 0));
}

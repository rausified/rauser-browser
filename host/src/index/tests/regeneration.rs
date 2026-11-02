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
    // A redelivery of an indexed body marks nothing, so the pass has
    // nothing to render and the file is not touched.
    index.record_activity(&replacement, &[]).unwrap();
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged), (0, 0), "{report:?}");
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
    FULL_DERIVES.with(|count| count.set(0));
    index.record_activities(events.iter(), &[]).unwrap();
    // A delivery re-derives only its group, never the whole history.
    assert_eq!(FULL_DERIVES.with(|count| count.get()), 0);
    assert!(!root.path().join("artifacts").exists());
    assert_eq!(
        sql::sql_rows(
            &index.database_path(),
            "select count(*) from dirty_artifacts"
        ),
        [[sql::Value::Integer(1)]]
    );
    // A second delivery to the same history derives nothing in full either.
    let late = activity(
        "00000000-0000-4000-8000-000000000999",
        "BUSY",
        "Busy day",
        "2026-10-16T09:00:00Z",
    );
    index.record_activity(&late, &[]).unwrap();
    assert_eq!(FULL_DERIVES.with(|count| count.get()), 0);
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

/// The artifacts `dirty_artifacts` marks, which regeneration clears one
/// artifact at a time.
fn dirty_ids(index: &ArtifactIndex) -> Vec<String> {
    sql::sql_rows(
        &index.database_path(),
        "select artifact_id from dirty_artifacts order by artifact_id",
    )
    .into_iter()
    .map(|row| match &row[0] {
        sql::Value::Text(id) => id.clone(),
        other => panic!("artifact_id is {other:?}"),
    })
    .collect()
}

fn generation_of(index: &ArtifactIndex, id: &str) -> Option<i64> {
    sql::sql_rows(
        &index.database_path(),
        &format!("select generation from dirty_artifacts where artifact_id = '{id}'"),
    )
    .into_iter()
    .next()
    .map(|row| match row[0] {
        sql::Value::Integer(generation) => generation,
        ref other => panic!("generation is {other:?}"),
    })
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

// Ported from the checkpoint tests (`a_pass_killed_midway_resumes_after_
// the_files_it_wrote`, `a_pass_ending_on_a_checkpoint_does_not_publish_twice`
// and `checkpoints_back_off_when_publishing_the_cache_is_slow`): with no
// cache-wide checkpoint left, each cleared artifact commits on its own.
#[test]
fn each_cleared_artifact_commits_alone() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let events = ["A", "B", "C", "D"]
        .iter()
        .zip(1..)
        .map(|(name, id)| typed_activity(id, name, "work.item"))
        .collect::<Vec<_>>();
    index.record_activities(events.iter(), &[]).unwrap();
    // Chrome kills a host whose port closed; a panic stands in for that
    // here, right after the first artifact's clear committed.
    let killed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        index.regenerate_with(|step, _| {
            assert!(step != RegenStep::Cleared, "killed");
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
    assert!(dirty_ids(&index).is_empty());
}

#[test]
fn a_crash_between_write_and_clear_leaves_it_dirty_and_the_rewrite_is_unchanged() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = typed_activity(1, "A", "work.item");
    index.record_activity(&event, &[]).unwrap();
    let killed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        index.regenerate_with(|step, _| {
            assert!(step != RegenStep::Written, "killed");
        })
    }));
    assert!(killed.is_err());
    let files = artifact_files(root.path());
    assert_eq!(files.len(), 1, "the file was written before the crash");
    assert_eq!(dirty_ids(&index), std::slice::from_ref(&event.artifact_id));
    let unrecorded = sql::sql_rows(
        &index.database_path(),
        "select file_path is null, render_sha256 is null from brauser_group_state",
    );
    assert_eq!(
        unrecorded,
        [[sql::Value::Integer(1), sql::Value::Integer(1)]]
    );
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged), (0, 1));
    assert!(dirty_ids(&index).is_empty());
    // The clear stores where the file is and what was rendered into it.
    let relative = files[0]
        .strip_prefix(root.path())
        .unwrap()
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    let rendered = Sha256::digest(fs::read(&files[0]).unwrap()).to_vec();
    assert_eq!(
        sql::sql_rows(
            &index.database_path(),
            "select file_path, render_sha256 from brauser_group_state",
        ),
        [[sql::Value::Text(relative), sql::Value::Blob(rendered)]]
    );
}

#[test]
fn a_redirty_between_render_and_clear_survives() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = typed_activity(1, "A", "work.item");
    index.record_activity(&event, &[]).unwrap();
    let first = generation_of(&index, &event.artifact_id).unwrap();
    let mut later = typed_activity(2, "A", "work.item");
    later.title = Some("Renamed".into());
    later.occurred_at = "2026-10-15T13:00:00-07:00".into();
    let report = index
        .regenerate_with(|step, _| {
            if step == RegenStep::Written {
                // Another delivery lands after the file was rendered and
                // written, before its clear.
                index.record_activity(&later, &[]).unwrap();
            }
        })
        .unwrap();
    assert_eq!(report.written, 1);
    let second = generation_of(&index, &event.artifact_id).expect("the new mark survives");
    assert!(second > first);
    assert!(!only_file_text(root.path()).contains("Renamed"));
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.unchanged), (1, 0));
    assert!(only_file_text(root.path()).contains("title: \"Renamed\""));
    assert!(dirty_ids(&index).is_empty());
}

fn only_file_text(root: &Path) -> String {
    let files = artifact_files(root);
    assert_eq!(files.len(), 1, "{files:?}");
    fs::read_to_string(&files[0]).unwrap()
}

#[test]
fn regeneration_holds_the_regen_lock() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    index
        .record_activity(&typed_activity(1, "A", "work.item"), &[])
        .unwrap();
    let lock = cache.path().join("index-regen.lock");
    let mut held = false;
    index
        .regenerate_with(|step, _| {
            if step == RegenStep::Written {
                let other = File::options().read(true).write(true).open(&lock).unwrap();
                held = matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock));
            }
        })
        .unwrap();
    assert!(held, "another handle could take index-regen.lock mid-pass");
    // The lock is released after the pass and never read or written.
    let other = File::options().read(true).write(true).open(&lock).unwrap();
    other.try_lock().unwrap();
    assert_eq!(fs::metadata(&lock).unwrap().len(), 0);
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
    let derived = stored_derive(&index);
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
    let derived = stored_derive(&index);
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
        dirty_set(&index),
        BTreeSet::from([blocked.artifact_id.clone()])
    );
    assert_eq!(artifact_files(root.path()).len(), 1);
    // Once the path is writable again, the remaining artifact catches up.
    let report = index.regenerate_dirty().unwrap();
    assert_eq!((report.written, report.failed), (1, 0));
}

/// The pass marker's offset is that of the latest activity across artifacts,
/// as `latest_at` records it, not that of the artifact whose newest event of
/// any kind (here a why, which never moves `latest_at`) is latest.
#[test]
fn the_pass_marker_offset_follows_the_latest_activity_not_a_later_why() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let a = activity(
        "00000000-0000-4000-8000-0000000000a1",
        "A",
        "A",
        "2026-10-02T06:00:00Z",
    );
    let b = activity(
        "00000000-0000-4000-8000-0000000000b1",
        "B",
        "B",
        "2026-10-02T02:00:00-07:00",
    );
    index.record_activities([&a, &b], &[]).unwrap();
    index
        .record_why(
            &WhyEvent {
                event_id: "00000000-0000-4000-8000-0000000000a2".into(),
                artifact_id: a.artifact_id.clone(),
                url: a.url.clone(),
                occurred_at: "2026-10-02T10:00:00Z".into(),
                incognito: false,
                text: "later context".into(),
            },
            &[],
        )
        .unwrap();
    index.regenerate_dirty().unwrap();
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(cache.path().join("regenerated.json")).unwrap()).unwrap();
    assert_eq!(marker["offset_seconds"], -7 * 3600, "{marker}");
}

/// Hold `<cache>/index-regen.lock` as another host's regenerator would.
fn hold_regen_lock(cache: &Path) -> File {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache.join(regen::REGEN_LOCK))
        .unwrap();
    lock.lock().unwrap();
    lock
}

#[test]
fn a_held_regen_lock_skips_the_close_pass() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-0000000000e1",
        "HELD",
        "Held",
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&event, &[]).unwrap();
    let held = hold_regen_lock(cache.path());
    // Another regenerator holds the lock: the pass returns at once, writes
    // nothing, records no pass and keeps the mark for the next pass.
    let started = Instant::now();
    assert_eq!(index.regenerate_dirty_unless_busy().unwrap(), None);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!root.path().join("artifacts").exists());
    assert!(!cache.path().join("regenerated.json").exists());
    assert_eq!(
        dirty_set(&index),
        BTreeSet::from([event.artifact_id.clone()])
    );
    drop(held);
    let report = index.regenerate_dirty_unless_busy().unwrap().unwrap();
    assert_eq!(report.written, 1);
    assert_eq!(artifact_files(root.path()).len(), 1);
    assert!(dirty_set(&index).is_empty());
    // A free lock is released again after the pass.
    drop(hold_regen_lock(cache.path()));
}

#[test]
fn an_explicit_pass_waits_for_a_held_regen_lock() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let event = activity(
        "00000000-0000-4000-8000-0000000000e2",
        "WAITS",
        "Waits",
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&event, &[]).unwrap();
    let held = hold_regen_lock(cache.path());
    let hold = Duration::from_millis(300);
    let release = std::thread::spawn(move || {
        std::thread::sleep(hold);
        drop(held);
    });
    // The settings refresh (and the reindex tail) wait for the holder.
    let started = Instant::now();
    let report = index.regenerate_dirty().unwrap();
    assert!(started.elapsed() >= hold);
    assert_eq!(report.written, 1);
    release.join().unwrap();
    assert!(dirty_set(&index).is_empty());
}

#[test]
fn sweep_keeps_wal_shm_journal_and_lock_files() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    index
        .record_activity(&typed_activity(1, "A", "work.item"), &[])
        .unwrap();
    index.regenerate_dirty().unwrap();
    let old = crate::vault::STALE_TEMPORARY_AGE + Duration::from_secs(60);
    let kept = [
        "index.db",
        "index.db-wal",
        "index.db-shm",
        "index.db-journal",
        regen::REGEN_LOCK,
        "index-open.lock",
        "regenerated.json",
    ]
    .map(|name| {
        let path = cache.path().join(name);
        if !path.exists() {
            fs::write(&path, "").unwrap();
        }
        crate::vault::tests::age_file(&path, old);
        path
    });
    let stale = cache
        .path()
        .join(format!(".{NAMESPACE}-index-{}.tmp", Uuid::new_v4()));
    fs::write(&stale, "partial").unwrap();
    crate::vault::tests::age_file(&stale, old);
    assert_eq!(index.sweep_stale_temporaries().unwrap(), 1);
    assert!(!stale.exists());
    for path in kept {
        assert!(path.is_file(), "{} was swept", path.display());
    }
    // The database still opens and answers.
    assert!(
        index
            .artifact(&typed_activity(1, "A", "work.item").artifact_id)
            .unwrap()
            .is_some()
    );
}

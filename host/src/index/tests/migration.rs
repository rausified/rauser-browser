//! Migration of a format 8 cache (plan T16): a legacy `index.db` and
//! `index-state.json` that the pre-change build (`48d80d6`) wrote, committed
//! as bytes under `tests/fixtures/index/v8/`. They replace the in-place
//! v1/v4/v5 JSON migrations; a pre-v8 cache now takes the fallback.

use super::golden::{copy_tree, fixtures_root};
use super::rebuild::canonical;
use super::*;
use std::sync::Barrier;

/// The fixture's clean work item, whose file a user may have edited.
const V8_CLEAN: &str = "work:example.com/A1";
const V8_CLEAN_FILE: &str = "artifacts/work-item/example-com/a1-481b108c117b.md";
/// Logged and caught up, but not yet regenerated: the v8 dirty set.
const V8_DIRTY: &str = "work:example.com/B1";
const V8_REPO_FILE: &str = "artifacts/github-repo/github-com/acme-corp-widgets-c737120cdb17.md";
const V8_PR_FILE: &str = "artifacts/github-pr/github-com/acme-corp-widgets-7-b8095322e3c0.md";
const V8_LEGACY_URL: &str = "https://example.org/legacy-page";

/// The v8 fixture copied into a fresh root and cache. The JSON cache names
/// the folder it was written for, so its root and identity are moved to
/// this one, as if the same user's host had written it here.
struct V8 {
    root: TempDir,
    cache: TempDir,
}

impl V8 {
    fn load() -> Self {
        let fixture = fixtures_root().join("v8");
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        copy_tree(&fixture.join("log"), &root.path().join("log"));
        copy_tree(&fixture.join("artifacts"), &root.path().join("artifacts"));
        fs::copy(
            fixture.join("cache/index.db"),
            cache.path().join("index.db"),
        )
        .unwrap();
        let v8 = Self { root, cache };
        fs::write(v8.state_path(), v8.legacy_state()).unwrap();
        v8
    }

    /// The fixture's `index-state.json`, moved to this root.
    fn legacy_state(&self) -> Vec<u8> {
        let fixture = fixtures_root().join("v8/cache/index-state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
        assert_eq!(state["version"], 8);
        state["root"] = self.root.path().to_string_lossy().into_owned().into();
        state["root_identity"] = open_selected_root(self.root.path(), None).unwrap().1.into();
        serde_json::to_vec(&state).unwrap()
    }

    fn state_path(&self) -> PathBuf {
        self.cache.path().join("index-state.json")
    }

    fn index(&self) -> ArtifactIndex {
        index(&self.root, &self.cache).with_adapters(Some(&github_document(true)))
    }

    fn file(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn db(&self) -> PathBuf {
        db::db_path(self.cache.path())
    }

    fn rows(&self, query: &str) -> Vec<Vec<sql::Value>> {
        sql::sql_rows(&self.db(), query)
    }

    fn legacy_import(&self) -> Vec<Vec<sql::Value>> {
        self.rows("SELECT value FROM brauser_meta WHERE key = 'legacy_import'")
    }
}

fn text(value: &str) -> sql::Value {
    sql::Value::Text(value.into())
}

fn dirty(v8: &V8) -> Vec<Vec<sql::Value>> {
    v8.rows("SELECT artifact_id FROM dirty_artifacts ORDER BY artifact_id")
}

/// The canonical dump a fresh cache rebuilt from the same logs gives, in a
/// scratch root so the fixture's own files are left alone.
fn reindexed(v8: &V8) -> Vec<(&'static str, Vec<Vec<sql::Value>>)> {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    copy_tree(&v8.root.path().join("log"), &root.path().join("log"));
    let fresh = index(&root, &cache).with_adapters(Some(&github_document(true)));
    fresh.reindex().unwrap();
    canonical(&fresh)
}

#[test]
fn v8_import_keeps_dirty_set_container_files_and_user_edits() {
    let v8 = V8::load();
    let clean = fs::read_to_string(v8.file(V8_CLEAN_FILE)).unwrap();
    let edited = format!("{clean}\nA note of my own.\n");
    fs::write(v8.file(V8_CLEAN_FILE), &edited).unwrap();
    let index = v8.index();

    // The first write-capable open migrates: the v8 dirty set is kept, and
    // nothing the v8 cache had already written is marked again.
    index.reconcile_from_logs().unwrap();
    assert_eq!(db::detect_legacy(&v8.db()).unwrap(), db::FileState::Ours);
    assert_eq!(v8.legacy_import(), vec![vec![text("done")]]);
    assert_eq!(dirty(&v8), vec![vec![text(V8_DIRTY)]]);
    // The container's file is where the v8 cache last wrote it, and each
    // clean artifact's render is the one the v8 state derives.
    assert_eq!(
        v8.rows(&format!(
            "SELECT file_path FROM brauser_group_state WHERE artifact_id = '{GITHUB_REPO_ID}'"
        )),
        vec![vec![text(V8_REPO_FILE)]]
    );
    assert_eq!(
        v8.rows(&format!(
            "SELECT render_sha256 FROM brauser_group_state WHERE artifact_id = '{V8_CLEAN}'"
        )),
        vec![vec![sql::Value::Blob(
            Sha256::digest(clean.as_bytes()).to_vec()
        )]]
    );
    assert_eq!(
        v8.rows(&format!(
            "SELECT render_sha256 FROM brauser_group_state WHERE artifact_id = '{V8_DIRTY}'"
        )),
        vec![vec![sql::Value::Null]]
    );

    let report = index.regenerate_dirty().unwrap();
    assert_eq!(
        (report.written, report.unchanged, report.conflicts),
        (1, 0, 0),
        "{report:?}"
    );
    assert_eq!(fs::read_to_string(v8.file(V8_CLEAN_FILE)).unwrap(), edited);
    assert_eq!(artifact_files(v8.root.path()).len(), 5);

    // Without the container adapter the repo derives no record: its file,
    // located through the imported `file_path`, is removed, and the seeded
    // renders keep every unaffected file (and its edits) as it is.
    let plain = super::index(&v8.root, &v8.cache);
    let pr = fs::read_to_string(v8.file(V8_PR_FILE)).unwrap();
    plain.reconcile_from_logs().unwrap();
    assert_eq!(dirty(&v8), vec![vec![text(GITHUB_REPO_ID)]]);
    let report = plain.regenerate_dirty().unwrap();
    assert_eq!(
        (report.written, report.conflicts, report.failed),
        (0, 0, 0),
        "{report:?}"
    );
    assert!(!v8.file(V8_REPO_FILE).exists());
    assert_eq!(fs::read_to_string(v8.file(V8_PR_FILE)).unwrap(), pr);
    assert_eq!(fs::read_to_string(v8.file(V8_CLEAN_FILE)).unwrap(), edited);
}

#[test]
fn v8_tables_equal_a_fresh_reindex() {
    let v8 = V8::load();
    let index = v8.index();
    assert_eq!(index.regenerate_dirty().unwrap().written, 1);
    assert_eq!(v8.legacy_import(), vec![vec![text("done")]]);
    let migrated = canonical(&index);
    assert_eq!(migrated, reindexed(&v8));
    index.reindex().unwrap();
    assert_eq!(canonical(&index), migrated);
}

/// A JSON cache that is not format 8 (or not JSON) imports nothing: after
/// the rebuild only artifacts whose file is missing are dirty, and every
/// existing file, edited or stale, is kept until its next real change. The
/// M1 visit still gains its open through the rebuild.
#[test]
fn unusable_json_dirties_only_missing_files() {
    for unusable in [Some(4), Some(7), None] {
        let v8 = V8::load();
        match unusable {
            Some(version) => {
                let mut state: serde_json::Value =
                    serde_json::from_slice(&fs::read(v8.state_path()).unwrap()).unwrap();
                state["version"] = version.into();
                fs::write(v8.state_path(), serde_json::to_vec(&state).unwrap()).unwrap();
            }
            None => fs::write(v8.state_path(), b"{\"version\": 8, \"truncated").unwrap(),
        }
        let clean = fs::read_to_string(v8.file(V8_CLEAN_FILE)).unwrap();
        let edited = format!("{clean}\nA note of my own.\n");
        fs::write(v8.file(V8_CLEAN_FILE), &edited).unwrap();
        let index = v8.index();

        let report = index.regenerate_dirty().unwrap();
        assert_eq!(
            (report.written, report.unchanged, report.conflicts),
            (1, 0, 0),
            "{unusable:?}: {report:?}"
        );
        assert_eq!(fs::read_to_string(v8.file(V8_CLEAN_FILE)).unwrap(), edited);
        assert_eq!(artifact_files(v8.root.path()).len(), 5);
        assert_eq!(v8.legacy_import(), vec![vec![text("done")]]);
        assert_eq!(
            v8.rows(&format!(
                "SELECT render_sha256 FROM brauser_group_state WHERE artifact_id = '{V8_CLEAN}'"
            )),
            vec![vec![sql::Value::Null]]
        );
        let legacy = crate::summary::web_page_artifact_id(V8_LEGACY_URL).unwrap();
        assert_eq!(index.artifact(&legacy).unwrap().unwrap().opens, 1);
        assert_eq!(canonical(&index), reindexed(&v8));
    }
}

#[test]
fn stray_sidecars_are_removed_before_the_legacy_file() {
    let v8 = V8::load();
    let stray = vec![0xA5_u8; 8192];
    for suffix in ["-wal", "-shm", "-journal"] {
        fs::write(v8.cache.path().join(format!("index.db{suffix}")), &stray).unwrap();
    }
    let index = v8.index();
    index.reconcile_from_logs().unwrap();
    assert!(sql::integrity_ok(&v8.db()));
    assert_eq!(db::detect_legacy(&v8.db()).unwrap(), db::FileState::Ours);
    assert!(!v8.cache.path().join("index.db-journal").exists());
    for suffix in ["-wal", "-shm"] {
        let path = v8.cache.path().join(format!("index.db{suffix}"));
        assert!(
            fs::read(&path).map_or(true, |bytes| bytes != stray),
            "{} kept the stray bytes",
            path.display()
        );
    }
    assert_eq!(dirty(&v8), vec![vec![text(V8_DIRTY)]]);
    assert!(index.artifact(V8_CLEAN).unwrap().is_some());
}

/// The legacy JSON files outlive every pass of the migration's rebuild and
/// are removed only once it commits. Each call advances one pass, as a
/// rebuild split over passes always has.
#[test]
fn legacy_json_files_are_removed_after_commit() {
    let v8 = V8::load();
    let pending = v8.cache.path().join("index-catch-up.json");
    fs::write(&pending, b"{\"rebuild\": true}").unwrap();
    let legacy = fs::read(v8.state_path()).unwrap();
    let index = v8.index();
    SCAN_LIMIT.with(|limit| limit.set(1));
    let mut passes = 0;
    let outcome = loop {
        passes += 1;
        assert!(passes < 10, "the migration never finished");
        match index.artifact(V8_DIRTY) {
            Ok(found) => break found,
            Err(error) => {
                assert!(
                    format!("{error:#}").contains(INCOMPLETE_CATCH_UP),
                    "{error:#}"
                );
                assert_eq!(fs::read(v8.state_path()).unwrap(), legacy);
                assert!(pending.exists());
                assert_ne!(v8.legacy_import(), vec![vec![text("done")]]);
            }
        }
    };
    SCAN_LIMIT.with(|limit| limit.set(MAX_SCAN_BYTES));
    assert!(passes >= 2, "{passes}");
    assert!(outcome.is_some());
    assert!(!pending.exists());
    // Until plan T15 the host keeps its own JSON cache, rewritten from the
    // store; it is never the legacy one again.
    assert_ne!(fs::read(v8.state_path()).ok(), Some(legacy));
    assert_eq!(v8.legacy_import(), vec![vec![text("done")]]);
    assert_eq!(dirty(&v8), vec![vec![text(V8_DIRTY)]]);
}

/// A pre-format-9 host that finds no cache it can use rebuilds from the
/// logs and renames its snapshot over `index.db`. The next format 9 open
/// sees the foreign file and migrates again; a delivery the orphaned store
/// took is recovered from the authoritative log.
#[test]
fn an_old_host_rename_over_is_recovered() {
    let v8 = V8::load();
    let index = v8.index();
    index.regenerate_dirty().unwrap();
    let later = activity(
        "00000000-0000-4000-8000-000000000804",
        "C1",
        "Gamma item",
        "2026-10-17T09:00:00Z",
    );
    write_log(v8.root.path(), "2026-10-17.md", &[&later]);
    index.record_activity(&later, &[]).unwrap();

    let snapshot = v8.cache.path().join(".old-host-snapshot");
    fs::copy(fixtures_root().join("v8/cache/index.db"), &snapshot).unwrap();
    fs::rename(&snapshot, v8.db()).unwrap();
    fs::write(v8.state_path(), v8.legacy_state()).unwrap();
    assert_eq!(db::detect_legacy(&v8.db()).unwrap(), db::FileState::Legacy);

    let found = index.artifact(&later.artifact_id).unwrap().unwrap();
    assert_eq!(found.opens, 1);
    assert_eq!(db::detect_legacy(&v8.db()).unwrap(), db::FileState::Ours);
    assert_eq!(v8.legacy_import(), vec![vec![text("done")]]);
    // The old host's dirty set comes back with it; the recovered delivery
    // is new to the store, so its artifact is dirty too.
    assert_eq!(
        dirty(&v8),
        vec![vec![text(V8_DIRTY)], vec![text(&later.artifact_id)]]
    );
    assert_eq!(canonical(&index), reindexed(&v8));
}

/// Two hosts opening the same legacy cache at once converge on one store.
#[test]
fn two_first_opens_converge() {
    let v8 = V8::load();
    let barrier = Barrier::new(2);
    let found = std::thread::scope(|scope| {
        let hosts = (0..2)
            .map(|_| {
                let index = v8.index();
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    index.artifact(V8_DIRTY)
                })
            })
            .collect::<Vec<_>>();
        hosts
            .into_iter()
            .map(|host| host.join().unwrap())
            .collect::<Vec<_>>()
    });
    for result in found {
        assert!(result.unwrap().is_some());
    }
    assert_eq!(v8.legacy_import(), vec![vec![text("done")]]);
    assert_eq!(dirty(&v8), vec![vec![text(V8_DIRTY)]]);
    assert_eq!(canonical(&v8.index()), reindexed(&v8));
}

/// A v8 cache derived under another adapter document: a directly visited
/// artifact whose type is a container type under this host's document (so
/// its file is now named for its ID) is dirty although nothing rolled up
/// into it, as a format 8 host's own adapter change marked it.
#[test]
fn an_import_under_another_document_dirties_adapter_dependent_files() {
    let v8 = V8::load();
    let gadgets = "github.repo:github.com/acme-corp/gadgets";
    let visit = github_activity(
        805,
        ("github.repo", gadgets),
        "https://github.com/acme-corp/gadgets",
        "2026-10-17T09:00:00Z",
        (10, 5),
    );
    write_log(v8.root.path(), "2026-10-17.md", &[&visit]);
    // The cached event exactly as the log holds it.
    let scratch = (TempDir::new().unwrap(), TempDir::new().unwrap());
    copy_tree(&v8.root.path().join("log"), &scratch.0.path().join("log"));
    let fresh = index(&scratch.0, &scratch.1);
    fresh.reindex().unwrap();
    let cached = fresh.store_state().unwrap().events[&visit.event_id].clone();
    let mut state: serde_json::Value = serde_json::from_slice(&v8.legacy_state()).unwrap();
    state["events"][&visit.event_id] = serde_json::to_value(&cached).unwrap();
    state["positions"][&visit.event_id] = serde_json::json!(["2026-10-17.md", 1]);
    state["adapters"] = "an-older-document".into();
    fs::write(v8.state_path(), serde_json::to_vec(&state).unwrap()).unwrap();

    v8.index().reconcile_from_logs().unwrap();
    assert_eq!(v8.legacy_import(), vec![vec![text("done")]]);
    let marked = dirty(&v8);
    assert!(marked.contains(&vec![text(gadgets)]), "{marked:?}");
    assert!(marked.contains(&vec![text(GITHUB_REPO_ID)]), "{marked:?}");
    assert!(!marked.contains(&vec![text(V8_CLEAN)]), "{marked:?}");
}

/// A host that read the migration as pending while another host finished
/// it resumes nothing: no second staged rebuild begins.
#[test]
fn a_migration_finished_by_another_host_is_not_rebuilt_again() {
    let v8 = V8::load();
    let (first, second) = (v8.index(), v8.index());
    let step = first.legacy_step().unwrap();
    assert_eq!(
        step,
        Legacy::Rebuild(crate::index::rebuild::Finish::Changed)
    );
    second.migrate_legacy(true).unwrap();
    let rebuild_id = || v8.rows("SELECT value FROM brauser_meta WHERE key = 'rebuild_id'");
    let finished = rebuild_id();

    SCAN_LIMIT.with(|limit| limit.set(1));
    let resumed = first.advance_legacy(step, false);
    SCAN_LIMIT.with(|limit| limit.set(MAX_SCAN_BYTES));
    resumed.unwrap();
    assert_eq!(rebuild_id(), finished);
    assert!(!first.store_rebuilding());
    assert_eq!(v8.legacy_import(), vec![vec![text("done")]]);
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

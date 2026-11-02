//! Regeneration (plan T12): the pass that renders each dirty artifact from
//! its group's events, plus artifact file replacement, removal, roll-up file
//! discovery and the temporary-file sweep.
//!
//! A pass holds `<cache>/index-regen.lock`, snapshots `dirty_artifacts`, and
//! then for each marked artifact re-derives its group in a read snapshot,
//! renders it, writes (or removes) its file, and commits a clear that is
//! conditional on the snapshotted generation. Each artifact commits alone,
//! so a pass killed midway keeps every clear it made, and a mark a delivery
//! renews meanwhile (a fresh generation) survives the clear.

use rusqlite::{Transaction, params};

use super::*;

/// The lock file only one regenerator holds. It is never read or written:
/// only the lock on it counts, and a crash releases it.
pub(super) const REGEN_LOCK: &str = "index-regen.lock";

/// Where a pass is, for each artifact: the test seam
/// [`ArtifactIndex::regenerate_with`] observes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegenStep {
    /// The file was written, found unchanged or removed, and its clear has
    /// not committed yet.
    Written,
    /// Its generation-conditional clear has committed.
    Cleared,
}

/// One `dirty_artifacts` mark and the file regeneration last wrote for it.
struct DirtyMark {
    artifact_id: String,
    generation: i64,
    file_path: Option<String>,
}

/// What a pass did with a marked artifact's file.
enum Settled {
    /// Written or already current: where, and the SHA-256 of what it holds.
    Rendered { path: String, sha256: Vec<u8> },
    /// No record derives any more, and its file (if any) is gone.
    Removed,
}

/// Whether a pass waits for another regenerator or skips (7vf).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// The explicit refresh and the reindex tail: their caller asked for
    /// current files.
    Block,
    /// The connection-close and daily passes: the holder is already
    /// regenerating, and whatever it leaves dirty waits for the next pass.
    Skip,
}

/// Take `<cache>/index-regen.lock` (std `File::lock`, never the database
/// file: Windows `LockFileEx` is mandatory). With [`Wait::Skip`], `None`
/// when another regenerator holds it. Released when the file is dropped.
fn regen_lock(cache_dir: &Path, wait: Wait) -> Result<Option<fs::File>> {
    fs::create_dir_all(cache_dir).context("creating artifact index cache directory")?;
    let mut options = StdOpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(cache_dir.join(REGEN_LOCK))
        .context("opening the artifact regeneration lock")?;
    match wait {
        Wait::Block => file
            .lock()
            .context("taking the artifact regeneration lock")?,
        Wait::Skip => match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Ok(None),
            Err(fs::TryLockError::Error(error)) => {
                return Err(error).context("taking the artifact regeneration lock");
            }
        },
    }
    Ok(Some(file))
}

/// `path` relative to the home folder with `/` separators, as stored in
/// `brauser_group_state.file_path`.
fn stored_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn dirty_marks(tx: &Transaction<'_>) -> Result<Vec<DirtyMark>> {
    let marks = tx
        .prepare_cached(
            "SELECT d.artifact_id, d.generation, g.file_path FROM dirty_artifacts d
               LEFT JOIN brauser_group_state g ON g.artifact_id = d.artifact_id
             ORDER BY d.artifact_id",
        )?
        .query_map([], |row| {
            Ok(DirtyMark {
                artifact_id: row.get(0)?,
                generation: row.get(1)?,
                file_path: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(marks)
}

/// Clear `mark` if no delivery renewed it since the snapshot, and then
/// record what its file now holds. Returns whether it was cleared.
fn clear(tx: &Transaction<'_>, mark: &DirtyMark, settled: &Settled) -> Result<bool> {
    let cleared = tx
        .prepare_cached("DELETE FROM dirty_artifacts WHERE artifact_id = ?1 AND generation = ?2")?
        .execute(params![mark.artifact_id, mark.generation])?
        == 1;
    if !cleared {
        return Ok(false);
    }
    match settled {
        Settled::Rendered { path, sha256 } => {
            tx.prepare_cached(
                "UPDATE brauser_group_state SET file_path = ?2, render_sha256 = ?3
                 WHERE artifact_id = ?1",
            )?
            .execute(params![mark.artifact_id, path, sha256])?;
        }
        // The state row stayed only so this pass could find the file.
        Settled::Removed => {
            tx.prepare_cached(
                "DELETE FROM brauser_group_state WHERE artifact_id = ?1
                   AND NOT EXISTS (SELECT 1 FROM artifacts WHERE artifact_id = ?1)",
            )?
            .execute([&mark.artifact_id])?;
        }
    }
    Ok(true)
}

/// The UTC offset of the latest activity across artifacts, for the pass
/// marker's local day. This is the greatest `latest_at` instant, as the
/// derive's `max(latest_at)` was; `latest_key` would also count a group's
/// why and child events, which never move its `latest_at`. On equal
/// instants the last artifact in ID order wins, as `Iterator::max` did.
fn newest_offset(tx: &Transaction<'_>) -> Result<Option<UtcOffset>> {
    let mut statement = tx.prepare_cached(
        "SELECT latest_at FROM brauser_group_state
         WHERE artifact_id IN (SELECT artifact_id FROM artifacts)
         ORDER BY artifact_id",
    )?;
    let mut rows = statement.query([])?;
    let mut newest: Option<OffsetDateTime> = None;
    while let Some(row) = rows.next()? {
        let Ok(at) = OffsetDateTime::parse(&row.get::<_, String>(0)?, &Rfc3339) else {
            continue;
        };
        if newest.is_none_or(|newest| at >= newest) {
            newest = Some(at);
        }
    }
    Ok(newest.map(OffsetDateTime::offset))
}

impl ArtifactIndex {
    /// The connection-close and daily pass (7vf): like `regenerate_dirty`,
    /// but `None` without reading anything when another host process holds
    /// `index-regen.lock`. That regenerator is already writing these files,
    /// and a mark it leaves behind waits for the next pass.
    pub fn regenerate_dirty_unless_busy(&self) -> Result<Option<RegenerationReport>> {
        let (root, _) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))?;
        // Lock order: the regen lock before any SQLite transaction.
        let Some(lock) = regen_lock(&self.cache_dir, Wait::Skip)? else {
            return Ok(None);
        };
        self.reconcile_from_logs()?;
        self.regenerate_locked(&root, &lock, |_, _| {}).map(Some)
    }

    /// One regeneration pass under `index-regen.lock`, waiting for another
    /// regenerator to finish first. `after` (a test seam) sees each
    /// artifact's steps.
    pub(super) fn regenerate_with(
        &self,
        after: impl FnMut(RegenStep, &str),
    ) -> Result<RegenerationReport> {
        let (root, _) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))?;
        // Lock order: the regen lock before any SQLite transaction.
        let lock = regen_lock(&self.cache_dir, Wait::Block)?.context("regeneration lock")?;
        self.regenerate_locked(&root, &lock, after)
    }

    /// The pass itself, for the holder of `_lock`. A container file to
    /// remove is located by `brauser_group_state.file_path`, which a
    /// rebuild also adopts vanished container files into.
    fn regenerate_locked(
        &self,
        root: &Dir,
        _lock: &fs::File,
        mut after: impl FnMut(RegenStep, &str),
    ) -> Result<RegenerationReport> {
        let adapters = self.adapters.as_deref();
        // An empty history has nothing to regenerate, and a pass never
        // creates `index.db` (it is opened only to commit ingested data).
        let mut conn = if self.nothing_to_commit()? {
            db::empty()?
        } else {
            store::open(&self.cache_dir)?
        };
        let marks = db::read_txn(&mut conn, dirty_marks)?;
        let mut report = RegenerationReport::default();
        let fail = |report: &mut RegenerationReport, path: String, error: anyhow::Error| {
            report.failed += 1;
            report
                .first_failure
                .get_or_insert_with(|| format!("{path}: {error:#}"));
            if report.failed_paths.len() < 64 {
                report.failed_paths.push(path);
            }
        };
        for mark in &marks {
            let id = mark.artifact_id.as_str();
            let row = db::read_txn(&mut conn, |tx| store::derived_row(tx, adapters, id))?;
            let settled = match &row {
                // A container whose only activity was a roll-up the adapters
                // no longer make: its file would keep totals the index no
                // longer has, so it is removed (only if still Brauser's).
                None => {
                    let path = mark.file_path.as_deref().map(PathBuf::from);
                    if let Some(path) = path
                        && let Err(error) = remove_artifact(root, &path, id)
                    {
                        fail(&mut report, path.to_string_lossy().into_owned(), error);
                        continue;
                    }
                    Settled::Removed
                }
                Some(artifact) => {
                    let relative = artifact_path(&self.storage.artifacts_dir, artifact);
                    let shown = || relative.to_string_lossy().into_owned();
                    let rendered = render_artifact(&self.storage.artifacts_dir, artifact);
                    match replace_artifact(root, &self.storage.artifacts_dir, artifact, &rendered) {
                        // A warning is post-publication work (temporary
                        // cleanup, directory sync); the file is in place, so
                        // it is written and not retried, as for page notes.
                        Ok(OwnedWrite::Written(warnings)) => {
                            for warning in warnings {
                                let (step, error) = match warning {
                                    PostPublishWarning::DirectorySync(error) => {
                                        ("syncing its folder", error)
                                    }
                                    PostPublishWarning::TemporaryCleanup(error) => {
                                        ("removing its temporary", error)
                                    }
                                };
                                tracing::warn!(
                                    path = %shown(),
                                    step,
                                    %error,
                                    "artifact file written, but a later step failed"
                                );
                            }
                            report.written += 1;
                        }
                        Ok(OwnedWrite::Unchanged) => report.unchanged += 1,
                        Ok(OwnedWrite::Conflict) => {
                            report.conflicts += 1;
                            if report.conflict_paths.len() < 64 {
                                report.conflict_paths.push(shown());
                            }
                            continue;
                        }
                        // One unwritable path (a locked file, a read-only
                        // folder) stays dirty for the next pass without
                        // blocking the rest.
                        Err(error) => {
                            fail(&mut report, shown(), error);
                            continue;
                        }
                    }
                    Settled::Rendered {
                        path: stored_path(&relative),
                        sha256: Sha256::digest(rendered.as_bytes()).to_vec(),
                    }
                }
            };
            after(RegenStep::Written, id);
            store::write(&mut conn, adapters, |tx| clear(tx, mark, &settled))?;
            after(RegenStep::Cleared, id);
        }
        self.record_pass(&mut conn);
        Ok(report)
    }

    /// Remember the local day this pass completed (`daily_pass_due`), in
    /// the offset of the newest group's latest activity. Best effort: a lost
    /// marker only means one extra pass.
    fn record_pass(&self, conn: &mut rusqlite::Connection) {
        let offset = match db::read_txn(conn, newest_offset) {
            Ok(offset) => offset.unwrap_or(UtcOffset::UTC),
            Err(error) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "could not read the latest activity for the artifact pass"
                );
                UtcOffset::UTC
            }
        };
        let marker = PassMarker {
            date: OffsetDateTime::now_utc()
                .to_offset(offset)
                .date()
                .to_string(),
            offset_seconds: offset.whole_seconds(),
            root: self.storage.root.clone(),
            root_identity: self.root_identity.clone(),
            artifacts_dir: self.storage.artifacts_dir.clone(),
        };
        if let Ok(bytes) = serde_json::to_vec(&marker)
            && fs::read(self.pass_marker_path()).ok().as_deref() != Some(bytes.as_slice())
            && let Err(error) = atomic_write(&self.pass_marker_path(), &bytes)
        {
            tracing::warn!(
                error = format!("{error:#}"),
                "could not record artifact pass"
            );
        }
    }
}

pub(super) fn replace_artifact(
    root: &Dir,
    artifacts_dir: &str,
    artifact: &ArtifactRow,
    contents: &str,
) -> Result<OwnedWrite> {
    // The multi-process harness's regeneration trace (7vf).
    #[cfg(test)]
    let _span = tests::mp_harness::SpanGuard::start(&artifact.artifact_id, None);
    replace_owned_file(
        root,
        &artifact_path(artifacts_dir, artifact),
        contents,
        "artifact",
        &|existing| owned_artifact(existing, &artifact.artifact_id),
    )
}

/// Remove the generated file at `relative` for `artifact_id`, an artifact
/// that no longer derives a record. A missing file, or one that is not
/// Brauser's file for that artifact (a symlink, a directory, or content
/// without its frontmatter), is left alone.
pub(super) fn remove_artifact(root: &Dir, relative: &Path, artifact_id: &str) -> Result<()> {
    let parent = relative.parent().context("artifact path has no parent")?;
    let filename = relative
        .file_name()
        .context("artifact path has no filename")?;
    let dir = match root.open_dir(parent) {
        Ok(dir) => dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("opening artifact directory"),
    };
    let metadata = match dir.symlink_metadata(filename) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("checking artifact file"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 4 * 1024 * 1024
    {
        return Ok(());
    }
    let mut existing = Vec::new();
    Read::by_ref(&mut dir.open(filename).context("opening existing artifact")?)
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut existing)?;
    if !owned_artifact(&existing, artifact_id) {
        return Ok(());
    }
    dir.remove_file(filename)
        .context("removing artifact file")?;
    sync_directory(&dir).context("syncing artifact directory")
}

/// Subdirectories of `dir`, not following symlinks.
pub(super) fn subdirectories(dir: &Dir) -> Result<Vec<Dir>> {
    let mut found = Vec::new();
    for entry in dir.entries().context("listing artifacts directory")? {
        let entry = entry.context("listing artifacts directory")?;
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            found.push(entry.open_dir().context("opening artifacts directory")?);
        }
    }
    Ok(found)
}

/// Subdirectories of `dir` with their names, not following symlinks.
pub(super) fn named_subdirectories(dir: &Dir) -> Result<Vec<(String, Dir)>> {
    let mut found = Vec::new();
    for entry in dir.entries().context("listing artifacts directory")? {
        let entry = entry.context("listing artifacts directory")?;
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        found.push((
            name,
            entry.open_dir().context("opening artifacts directory")?,
        ));
    }
    Ok(found)
}

/// How much of an artifact file `scan_rollup_files` reads: enough for the
/// frontmatter, whose title and URL are bounded well below this.
pub(super) const ROLLUP_SCAN_BYTES: u64 = 64 * 1024;

/// The artifact ID of an owned artifact file (`owned_artifact`) whose
/// frontmatter marks a roll-up (`brauser.rollup: true`), from its leading
/// bytes.
pub(super) fn rollup_artifact_id(head: &[u8]) -> Option<String> {
    // The read may end inside a multi-byte character after the
    // frontmatter, which is all that is read.
    let text = String::from_utf8_lossy(head);
    let frontmatter = text.strip_prefix("---\n")?.split_once("\n---\n")?.0;
    let mut in_brauser = false;
    let mut id = None;
    let mut rollups = 0;
    for line in frontmatter.lines() {
        if !line.starts_with(' ') {
            in_brauser = line == "brauser:";
            continue;
        }
        if !in_brauser {
            continue;
        }
        if let Some(value) = line.strip_prefix("  artifact_id: ") {
            id = serde_json::from_str::<String>(value).ok();
        } else if line == "  rollup: true" {
            rollups += 1;
        }
    }
    let id = id?;
    (rollups == 1 && owned_artifact(text.as_bytes(), &id)).then_some(id)
}

pub(super) fn sweep_temporaries_in(dir: &Dir, prefix: &str) -> Result<usize> {
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in dir.entries().context("listing temporaries")? {
        let entry = entry.context("listing temporaries")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_generated_temporary(name, prefix) {
            continue;
        }
        // Not following symlinks: a link is not ours, whatever its name.
        let Ok(metadata) = dir.symlink_metadata(name) else {
            continue;
        };
        let stale = metadata.is_file()
            && metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified.into_std()).ok())
                .is_some_and(|elapsed| elapsed >= STALE_TEMPORARY_AGE);
        if stale && dir.remove_file(name).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

pub(super) fn owned_artifact(bytes: &[u8], artifact_id: &str) -> bool {
    owned_frontmatter(bytes, "artifact", &[("artifact_id", artifact_id)], &[])
}

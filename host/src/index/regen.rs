//! Artifact file replacement, removal, roll-up file discovery and the
//! temporary-file sweep used by regeneration.

use super::*;

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

/// Each checkpoint rewrites the whole cache, so space them by at least four
/// times the last one's cost: for a large index, checkpoints then take at
/// most a fifth of the pass instead of dominating it.
pub(super) fn checkpoint_interval(minimum: Duration, last_publish: Duration) -> Duration {
    minimum.max(last_publish.saturating_mul(4))
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

/// Add each of `known`'s container files whose record `derived` no longer
/// has to `state`, dirty, so the next regeneration pass removes it (only if
/// it is still Brauser's file for that ID).
pub(super) fn adopt_vanished_container_files(
    state: &mut IndexState,
    derived: &Derived,
    known: BTreeMap<String, PathBuf>,
) {
    for (id, path) in known {
        if derived.artifacts.contains_key(&id) {
            continue;
        }
        state.dirty.insert(id.clone());
        state.container_files.entry(id).or_insert(path);
    }
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

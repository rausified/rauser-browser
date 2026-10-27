//! The fixed folder layout inside the home folder the user chose (DESIGN.md
//! §6.1), and the one-time rename that moves an older, configurable layout
//! onto it. The home holds only human-facing content; config, caches, intents,
//! and locks stay in the OS config and cache directories.

use std::fmt;
use std::io;
use std::path::Path;

use anyhow::{Context, Result};
use brauser_protocol::StorageConfig;
use cap_std::fs::Dir;

use crate::config::lower_components;
use crate::vault::sync_directory;

pub(crate) const NOTES_DIR: &str = "notes";
pub(crate) const LOGS_DIR: &str = "logs";
pub(crate) const ARTIFACTS_DIR: &str = "artifacts";
pub(crate) const SUMMARIES_DIR: &str = "summaries";
/// Reserved for read-later items (§5.4), which nothing writes yet.
pub(crate) const LATER_DIR: &str = "later";

/// `storage` with every content location at its fixed name.
pub(crate) fn fixed(storage: &StorageConfig) -> StorageConfig {
    StorageConfig {
        log_dir: LOGS_DIR.into(),
        pages_dir: NOTES_DIR.into(),
        later_dir: LATER_DIR.into(),
        artifacts_dir: ARTIFACTS_DIR.into(),
        summaries_dir: Some(SUMMARIES_DIR.into()),
        ..storage.clone()
    }
}

pub(crate) fn is_fixed(storage: &StorageConfig) -> bool {
    *storage == fixed(storage)
}

/// Brauser chooses every content folder's name; an update cannot.
pub(crate) fn require_fixed(storage: &StorageConfig) -> anyhow::Result<()> {
    if !is_fixed(storage) {
        anyhow::bail!(
            "content folders are fixed: notes, logs, artifacts, summaries, and later inside the home folder"
        );
    }
    Ok(())
}

/// Why the rename was refused. Nothing was changed by the check that found
/// it; the message tells the user what to move so the next start can finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal(pub String);

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// One folder to rename inside the home folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Move {
    pub from: String,
    pub to: &'static str,
}

/// The older location and the fixed name for each content folder.
fn pairs(storage: &StorageConfig) -> [(Option<&str>, &'static str); 5] {
    [
        (Some(storage.pages_dir.as_str()), NOTES_DIR),
        (Some(storage.log_dir.as_str()), LOGS_DIR),
        (Some(storage.artifacts_dir.as_str()), ARTIFACTS_DIR),
        (storage.summaries_dir.as_deref(), SUMMARIES_DIR),
        (Some(storage.later_dir.as_str()), LATER_DIR),
    ]
}

/// Read-only: the renames that move `storage`'s folders to the fixed layout
/// in `root`, or why they cannot be done. A folder that does not exist needs
/// nothing; a fixed name that already exists while its older folder does is
/// never replaced or merged into. That rule also makes an interrupted
/// migration resumable: a folder already renamed has no older folder left.
/// Unusual layouts are refused rather than guessed at.
pub(crate) fn plan(root: &Dir, storage: &StorageConfig) -> Result<Vec<Move>, Refusal> {
    let mut moves = Vec::new();
    let mut refusals = Vec::new();
    for (legacy, target) in pairs(storage) {
        let Some(legacy) = legacy.filter(|legacy| *legacy != target) else {
            continue;
        };
        match root.symlink_metadata(legacy) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                refusals.push(format!("{legacy}/ cannot be inspected"));
                continue;
            }
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                refusals.push(format!("{legacy} is not a plain folder"));
                continue;
            }
            Ok(_) => {}
        }
        if Path::new(legacy).components().count() != 1 {
            refusals.push(format!(
                "{legacy}/ is nested; move it to {target}/ at the top of the home folder"
            ));
            continue;
        }
        if lower_components(Path::new(legacy)) == lower_components(Path::new(target)) {
            refusals.push(format!("rename {legacy}/ to {target}/"));
            continue;
        }
        match root.symlink_metadata(target) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => moves.push(Move {
                from: legacy.to_owned(),
                to: target,
            }),
            Ok(_) => refusals.push(format!(
                "{target}/ already exists beside {legacy}/; move or rename {target}/ so {legacy}/ can become it"
            )),
            Err(_) => refusals.push(format!("{target}/ cannot be inspected")),
        }
    }
    if refusals.is_empty() {
        Ok(moves)
    } else {
        Err(Refusal(format!(
            "{} could not move its folders to the fixed layout (notes, logs, artifacts, summaries) in {}: {}. Nothing was changed. Fix this, then reload this page.",
            crate::brand::APP_NAME,
            storage.root,
            refusals.join("; ")
        )))
    }
}

/// Rename each planned folder without replacing anything at its new name.
/// Callers hold every writer's lock and have just run `plan` under it. A
/// failure part-way leaves earlier renames done; `plan` resumes from there.
pub(crate) fn apply(root: &Dir, moves: &[Move]) -> Result<()> {
    for step in moves {
        match rename_no_replace(root, &step.from, step.to) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(Refusal(format!(
                    "{}/ appeared while {} was moving {}/; move or rename it, then reload this page.",
                    step.to,
                    crate::brand::APP_NAME,
                    step.from
                ))
                .into());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("renaming {} to {}", step.from, step.to));
            }
        }
    }
    if !moves.is_empty() {
        sync_directory(root).context("syncing the home folder after renaming its folders")?;
    }
    Ok(())
}

#[cfg(unix)]
fn rename_no_replace(root: &Dir, from: &str, to: &str) -> io::Result<()> {
    use rustix::fs::{RenameFlags, renameat, renameat_with};
    match renameat_with(root, from, root, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        // A filesystem without RENAME_NOREPLACE. Every Brauser writer is
        // locked out, so only another program could race this check.
        Err(rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS) => {
            if root.symlink_metadata(to).is_ok() {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            renameat(root, from, root, to).map_err(io::Error::from)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn rename_no_replace(root: &Dir, from: &str, to: &str) -> io::Result<()> {
    // Windows never renames a folder over an existing one; check anyway so
    // the refusal names the folder.
    if root.symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    root.rename(from, root, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_std::ambient_authority;
    use std::fs;

    fn legacy(root: &Path) -> StorageConfig {
        StorageConfig {
            root: root.to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: "pages".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: Some("summaries".into()),
            knowledge_base_dir: None,
        }
    }

    fn open(root: &Path) -> Dir {
        Dir::open_ambient_dir(root, ambient_authority()).unwrap()
    }

    #[test]
    fn fixed_layout_keeps_root_profile_and_knowledge_base() {
        let mut storage = legacy(Path::new("/home"));
        storage.knowledge_base_dir = Some("/kb".into());
        let fixed = fixed(&storage);
        assert!(is_fixed(&fixed));
        assert!(!is_fixed(&storage));
        assert_eq!(fixed.root, "/home");
        assert_eq!(fixed.knowledge_base_dir.as_deref(), Some("/kb"));
        assert_eq!(
            (
                fixed.pages_dir.as_str(),
                fixed.log_dir.as_str(),
                fixed.artifacts_dir.as_str(),
                fixed.summaries_dir.as_deref(),
                fixed.later_dir.as_str()
            ),
            ("notes", "logs", "artifacts", Some("summaries"), "later")
        );
    }

    #[test]
    fn only_existing_older_folders_are_renamed() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join("pages")).unwrap();
        fs::write(home.path().join("pages/a.md"), "a").unwrap();
        let root = open(home.path());
        let moves = plan(&root, &legacy(home.path())).unwrap();
        assert_eq!(
            moves,
            [Move {
                from: "pages".into(),
                to: "notes"
            }]
        );
        apply(&root, &moves).unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join("notes/a.md")).unwrap(),
            "a"
        );
        assert!(!home.path().join("pages").exists());
        // Run again, as after a crash before config.toml was updated.
        assert_eq!(plan(&root, &legacy(home.path())).unwrap(), []);
    }

    #[test]
    fn an_existing_fixed_name_refuses_every_rename() {
        let home = tempfile::tempdir().unwrap();
        for name in ["pages", "log", "notes"] {
            fs::create_dir(home.path().join(name)).unwrap();
        }
        let root = open(home.path());
        let refusal = plan(&root, &legacy(home.path())).unwrap_err();
        assert!(
            refusal.0.contains("notes/ already exists beside pages/"),
            "{refusal}"
        );
        assert!(refusal.0.contains("Nothing was changed"));
        for name in ["pages", "log", "notes"] {
            assert!(home.path().join(name).is_dir());
        }
        assert!(!home.path().join("logs").exists());
    }

    #[test]
    fn unusual_layouts_are_refused_not_guessed() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("brauser/log")).unwrap();
        fs::write(home.path().join("Pages"), "a file, not a folder").unwrap();
        let mut storage = legacy(home.path());
        storage.log_dir = "brauser/log".into();
        storage.pages_dir = "Pages".into();
        let refusal = plan(&open(home.path()), &storage).unwrap_err();
        assert!(refusal.0.contains("brauser/log/ is nested"), "{refusal}");
        assert!(
            refusal.0.contains("Pages is not a plain folder"),
            "{refusal}"
        );

        let other = tempfile::tempdir().unwrap();
        fs::create_dir(other.path().join("Notes")).unwrap();
        let mut storage = legacy(other.path());
        storage.pages_dir = "Notes".into();
        let refusal = plan(&open(other.path()), &storage).unwrap_err();
        assert!(refusal.0.contains("rename Notes/ to notes/"), "{refusal}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_older_folder_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join("pages")).unwrap();
        let refusal = plan(&open(home.path()), &legacy(home.path())).unwrap_err();
        assert!(
            refusal.0.contains("pages is not a plain folder"),
            "{refusal}"
        );
    }

    #[test]
    fn a_target_created_after_planning_is_never_replaced() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join("pages")).unwrap();
        let root = open(home.path());
        let moves = plan(&root, &legacy(home.path())).unwrap();
        // Another program creates an empty notes/ between plan and apply.
        fs::create_dir(home.path().join("notes")).unwrap();
        fs::write(home.path().join("pages/a.md"), "a").unwrap();
        let error = apply(&root, &moves).unwrap_err();
        assert!(error.downcast_ref::<Refusal>().is_some(), "{error:#}");
        assert_eq!(
            fs::read_to_string(home.path().join("pages/a.md")).unwrap(),
            "a"
        );
        assert_eq!(fs::read_dir(home.path().join("notes")).unwrap().count(), 0);
    }
}

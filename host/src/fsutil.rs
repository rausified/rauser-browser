//! Shared helpers for every place in this crate that renames a file into
//! its final, published place: `config.rs`'s `publish_config` and its
//! `forget_adopted_contracts` backup, `capture.rs`'s batch-intent and log-
//! relocations publish, `diag.rs`'s log rotation, `index/mod.rs`'s
//! `atomic_write`, `service/record.rs`'s `atomic_write`, and `vault.rs`'s
//! two cap_std `Dir`-sandboxed note/page publishes. Every one of those
//! renames a file within one directory (a sibling temporary, or a sibling
//! backup/rotated name), so today the rename is always same-volume and
//! always takes the plain, atomic branch below.
//!
//! `layout.rs`'s one-time content-folder migration is the one rename in
//! this crate that does not go through here: it needs `RENAME_NOREPLACE`'s
//! atomic refuse-if-the-destination-exists guarantee, which a copy
//! fallback cannot give (a copy would silently overwrite it), and it is
//! always a rename within one root `Dir`, never a temp file from
//! elsewhere, so it is in no more danger of crossing volumes than this
//! module's own fallback copy would be.
//!
//! `every_other_rename_into_place_goes_through_the_shared_helper` (below)
//! statically checks that no other file under `host/src` calls
//! `fs::rename` or a `Dir::rename` directly, so a future writer that
//! bypasses these helpers fails the test suite instead of silently
//! joining the ones already found missing one (#377 review).

use std::fs;
use std::io;
use std::path::Path;

use cap_std::fs::Dir;

/// Put `temporary` at `path`. Tries a plain, atomic `fs::rename` first; a
/// temporary file created beside `path` (every caller's pattern today)
/// always takes this branch. Falls back to copying `temporary`'s bytes
/// onto `path` and then removing `temporary` only when the rename fails
/// with [`io::ErrorKind::CrossesDevices`] — a future caller whose temp
/// directory and target land on different volumes, the way a Windows
/// machine's default `TMP` on `C:` and a home folder on another drive
/// would (CI's own `TMP` now sits on the checkout's drive precisely so it
/// no longer exercises this path; see `cross_drive_rename_falls_back_to_copy`
/// below). The fallback is not atomic, the same way a plain `mv` across
/// filesystems is not: a reader could briefly see `path` partially
/// written. It is still complete and correct, because `fs::copy` only
/// returns once every byte has been copied, and every caller here already
/// `sync_all`s `temporary` before calling this.
pub(crate) fn rename_or_copy(temporary: &Path, path: &Path) -> io::Result<()> {
    match fs::rename(temporary, path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            fs::copy(temporary, path)?;
            fs::remove_file(temporary)
        }
        Err(error) => Err(error),
    }
}

/// [`rename_or_copy`], for the cap_std-sandboxed `Dir` API `vault.rs` uses
/// instead of plain `std::fs` (every path here stays relative to a capture
/// of an already-open directory, never an absolute path cap_std would have
/// to re-validate). `from_dir` and `to_dir` are the same `Dir` at every
/// call site today, the same way `temporary` and `path` are always
/// siblings for [`rename_or_copy`].
pub(crate) fn rename_or_copy_in_dir(
    from_dir: &Dir,
    temporary: &Path,
    to_dir: &Dir,
    to: &Path,
) -> io::Result<()> {
    match from_dir.rename(temporary, to_dir, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            from_dir.copy(temporary, to_dir, to)?;
            from_dir.remove_file(temporary)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_volume_rename_moves_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let temporary = dir.path().join("a.tmp");
        let path = dir.path().join("a");
        fs::write(&temporary, b"hello").unwrap();
        rename_or_copy(&temporary, &path).unwrap();
        assert!(!temporary.exists());
        assert_eq!(fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn same_volume_rename_in_dir_moves_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let cap_dir = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        cap_dir.write("a.tmp", b"hello").unwrap();
        rename_or_copy_in_dir(&cap_dir, Path::new("a.tmp"), &cap_dir, Path::new("a")).unwrap();
        assert!(!dir.path().join("a.tmp").exists());
        assert_eq!(fs::read(dir.path().join("a")).unwrap(), b"hello");
    }

    /// Every production-code rename-into-place must go through
    /// `rename_or_copy` or `rename_or_copy_in_dir` above, not a bare
    /// `fs::rename`/`Dir::rename` — `capture.rs`'s `publish_log_relocations`
    /// still called `fs::rename` directly until the #377 review caught it.
    /// This walks every other file under `host/src`, skips each file's own
    /// trailing `#[cfg(test)] mod tests { ... }` block (every file in this
    /// crate keeps its tests there, to the end of the file, so a test
    /// fixture's own direct rename is never checked), and fails if a bare
    /// rename call remains outside `layout.rs`'s documented exception (see
    /// the module doc comment above).
    #[test]
    fn every_other_rename_into_place_goes_through_the_shared_helper() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offending = Vec::new();
        visit_rust_files(&src_dir, &mut offending);
        assert!(
            offending.is_empty(),
            "these renames bypass fsutil::rename_or_copy/rename_or_copy_in_dir: {offending:#?}"
        );
    }

    /// Every `.rs` file under `dir`, recursively, except this module's own
    /// file (which legitimately calls `fs::rename`/`Dir::rename` directly,
    /// both in the helpers above and in this very test's probes),
    /// `layout.rs` (the documented folder-migration exception), and a
    /// whole file or directory devoted to tests — a `#[cfg(test)] mod
    /// tests;` external submodule, such as `index/tests/` or
    /// `relearn/journal/tests.rs` — which this function never descends
    /// into or reads at all, rather than trying to truncate test code out
    /// of it the way `check_file_for_bypassed_renames` does for a file
    /// that mixes production code with one trailing inline test module.
    fn visit_rust_files(dir: &Path, offending: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().and_then(|name| name.to_str()) == Some("tests") && path.is_dir() {
                continue;
            }
            if path.is_dir() {
                visit_rust_files(&path, offending);
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }
            if path.ends_with("fsutil.rs")
                || path.ends_with("layout.rs")
                || path.ends_with("tests.rs")
            {
                continue;
            }
            check_file_for_bypassed_renames(&path, offending);
        }
    }

    /// One of the two markers a file's trailing inline test module starts
    /// with, found in every file under `host/src` that has rename calls of
    /// its own to check (a handful of files put their tests in a separate
    /// `tests/` submodule instead, with no bare rename of their own to
    /// hide; those are scanned in full since nothing here would truncate
    /// them wrongly).
    const TEST_MODULE_MARKERS: &[&str] = &[
        "#[cfg(test)]\nmod tests",
        "#[cfg(test)]\npub(crate) mod tests {",
        "#[cfg(test)]\npub mod tests",
    ];

    fn check_file_for_bypassed_renames(path: &Path, offending: &mut Vec<String>) {
        // Windows checkouts use CRLF (core.autocrlf), so normalize before
        // looking for the `\n`-spelled test-module markers; otherwise every
        // file is scanned whole and test fixtures' renames are reported.
        let text = fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let production_text = TEST_MODULE_MARKERS
            .iter()
            .filter_map(|marker| text.find(marker))
            .min()
            .map_or(text.as_str(), |index| &text[..index]);
        for (zero_based_line, line) in production_text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            let bypasses = (line.contains("fs::rename(") || line.contains(".rename("))
                && !line.contains("rename_or_copy");
            if bypasses {
                offending.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    zero_based_line + 1,
                    line.trim()
                ));
            }
        }
    }

    /// The drive letter prefix of an absolute Windows path (`"C:"` from
    /// `C:\Users\...`), or `None` for a path with no drive prefix (a UNC
    /// path, or anything not absolute).
    #[cfg(windows)]
    fn drive_letter(path: &Path) -> Option<String> {
        let text = path.to_str()?;
        let bytes = text.as_bytes();
        (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic())
            .then(|| text[..2].to_ascii_uppercase())
    }

    /// A second volume beside the one `std::env::temp_dir()` is on, or
    /// `None` when there isn't one to find. `%USERPROFILE%\AppData\Local\
    /// Temp` is Windows' own default `TMP`/`TEMP`, on the system drive
    /// (`%SystemDrive%`, today `C:`) — the exact folder CI's "Put
    /// temporary folders on the runner's work drive" step
    /// (`.github/workflows/build-os.yml`) moves `TMP`/`TEMP` away from.
    /// On a GitHub-hosted `windows-latest` runner that always leaves it on
    /// a different volume from the job's own `D:`-based temp dir; on a
    /// single-drive machine (most developers' own), or one where CI's
    /// override has not taken effect, the two match and this returns
    /// `None`.
    #[cfg(windows)]
    fn other_volume_temp_root() -> Option<std::path::PathBuf> {
        let userprofile = std::env::var("USERPROFILE").ok()?;
        let candidate = Path::new(&userprofile)
            .join("AppData")
            .join("Local")
            .join("Temp");
        let here_drive = drive_letter(&std::env::temp_dir())?;
        let candidate_drive = drive_letter(&candidate)?;
        if here_drive.eq_ignore_ascii_case(&candidate_drive) || !candidate.is_dir() {
            return None;
        }
        Some(candidate)
    }

    /// Windows only: proves the copy+remove fallback above actually moves
    /// bytes across two real volumes, not just that `fs::rename` would
    /// fail there. CI's Windows job points `TMP`/`TEMP` at the checkout's
    /// own drive (build-os.yml, "Put temporary folders on the runner's
    /// work drive"), so nothing else in this workspace's test suite still
    /// renames across two volumes; this finds one by hand instead
    /// (`other_volume_temp_root`). Skips with a one-line reason, rather
    /// than failing, when the runner has only one volume to offer.
    #[test]
    #[cfg(windows)]
    fn cross_drive_rename_falls_back_to_copy() {
        let Some(other_root) = other_volume_temp_root() else {
            eprintln!(
                "skipping: no second volume beside {:?}; cross-drive rename not exercised",
                std::env::temp_dir()
            );
            return;
        };
        let source_dir = tempfile::Builder::new()
            .prefix("brauser-fsutil-src-")
            .tempdir_in(&other_root)
            .expect("creating a temp dir on the other volume");
        let target_dir = tempfile::tempdir().expect("creating the target temp dir");

        // Confirm this really is a cross-volume case: a plain fs::rename
        // between these two dirs must fail, or the fallback below would
        // not be exercised at all.
        let unused = source_dir.path().join("unused.tmp");
        fs::write(&unused, b"probe").unwrap();
        let probe = fs::rename(&unused, target_dir.path().join("unused"));
        assert_eq!(
            probe.unwrap_err().kind(),
            io::ErrorKind::CrossesDevices,
            "expected {:?} and {:?} to be on different volumes",
            source_dir.path(),
            target_dir.path(),
        );

        let temporary = source_dir.path().join("a.tmp");
        let path = target_dir.path().join("a");
        fs::write(&temporary, b"cross-drive").unwrap();
        rename_or_copy(&temporary, &path).expect("the copy+remove fallback should succeed");
        assert!(
            !temporary.exists(),
            "the source temp file should be removed"
        );
        assert_eq!(fs::read(&path).unwrap(), b"cross-drive");
    }
}

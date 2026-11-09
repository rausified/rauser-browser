//! Shared helper for Brauser's "write a temp file beside the target, then
//! put it in place" writers (`config.rs`'s `publish_config`, `capture.rs`'s
//! batch-intent publish, `index/mod.rs`'s `atomic_write`, and
//! `service/record.rs`'s `atomic_write`). Every one of those creates its
//! temporary file in the target's own parent directory, so today the
//! rename below is always same-volume and always the plain, atomic
//! `fs::rename` branch.

use std::fs;
use std::io;
use std::path::Path;

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

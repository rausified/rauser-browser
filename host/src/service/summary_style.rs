//! Optional user-written summary guidance. No template language or paths from
//! the extension: one fixed file in the picker-confirmed home, then one private
//! run snapshot. Instructions and priority live in the compiled SUMMARY.md.

use std::io::{self, Read};
use std::path::Path;

use anyhow::{Result, bail};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use rustix::fs::OFlags;

use crate::config::ConfigStore;
use crate::vault::open_selected_root;

pub const STYLE_NAME: &str = "SUMMARY_STYLE.md";
pub const MAX_STYLE_BYTES: usize = 8192;
pub const SNAPSHOT_NAME: &str = "summary-style.json";
const MAX_SNAPSHOT_BYTES: usize = MAX_STYLE_BYTES * 6 + 2;
pub const HOME_ERROR: &str = "Cannot read SUMMARY_STYLE.md in your Brauser home folder. Use a regular UTF-8 file of at most 8192 bytes with no control characters except newlines and tabs, or rename/remove it to use the default summary style. Symlinks are not supported.";
pub const SNAPSHOT_ERROR: &str = "The saved summary style cannot be read. Start a new summary.";

/// Called under the config lock at submission, never by a task or site draft.
pub fn from_home(config: &ConfigStore) -> Result<Option<String>, &'static str> {
    let read = || -> Result<Option<String>> {
        let storage = config
            .snapshot()
            .storage
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!(HOME_ERROR))?;
        let identity = config
            .root_identity()
            .ok_or_else(|| anyhow::anyhow!(HOME_ERROR))?;
        let (root, _) = open_selected_root(Path::new(&storage.root), Some(identity))?;
        match read_bounded(&root, STYLE_NAME, MAX_STYLE_BYTES)? {
            Some(bytes) => normalize(String::from_utf8(bytes)?),
            None => Ok(None),
        }
    };
    read().map_err(|_| HOME_ERROR)
}

/// The encoded snapshot is private run data, separate from strict status.json
/// so an older service can still recover process groups and inspect the run.
pub fn encode(style: Option<&str>) -> Result<Vec<u8>> {
    if let Some(text) = style {
        normalize(text.to_owned())?;
    }
    Ok(serde_json::to_vec(&style)?)
}

pub fn from_run(dir: &Path) -> Result<Option<String>, &'static str> {
    let read = || -> Result<Option<String>> {
        let root = Dir::open_ambient_dir(dir, cap_std::ambient_authority())?;
        let Some(bytes) = read_bounded(&root, SNAPSHOT_NAME, MAX_SNAPSHOT_BYTES)? else {
            // Legacy queued runs use defaults, regardless of today's home file.
            return Ok(None);
        };
        let style: Option<String> = serde_json::from_slice(&bytes)?;
        style.map(normalize).transpose().map(Option::flatten)
    };
    read().map_err(|_| SNAPSHOT_ERROR)
}

fn normalize(text: String) -> Result<Option<String>> {
    if text.len() > MAX_STYLE_BYTES {
        bail!("summary style exceeds the byte limit");
    }
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(&text)
        .replace("\r\n", "\n");
    if text
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        bail!("summary style contains a control character");
    }
    Ok((!text.trim().is_empty()).then_some(text))
}

/// No-follow and nonblocking open prevents a replacement symlink or FIFO
/// between inspection and open from escaping the decision or hanging setup.
fn read_bounded(dir: &Dir, name: &str, limit: usize) -> Result<Option<Vec<u8>>> {
    let inspected = match dir.symlink_metadata(name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !inspected.is_file() || inspected.len() > limit as u64 {
        bail!("summary style is not a bounded regular file");
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits() as i32);
    let mut file = dir.open_with(name, &options)?;
    let opened = file.metadata()?;
    use cap_std::fs::MetadataExt;
    if !opened.is_file()
        || opened.len() > limit as u64
        || (opened.dev(), opened.ino()) != (inspected.dev(), inspected.ino())
    {
        bail!("summary style changed while being read");
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        bail!("summary style exceeds the byte limit");
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::fixtures;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn home() -> (tempfile::TempDir, ConfigStore) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("home");
        fs::create_dir(&root).unwrap();
        let path = temp.path().join("config.toml");
        fixtures::write(&path, &root, "", "", "");
        let mut config = ConfigStore::for_test(path);
        config.refresh().unwrap();
        assert!(config.configured());
        (temp, config)
    }

    #[test]
    fn home_style_defaults_and_preserves_literal_text_with_normal_line_endings() {
        let (temp, config) = home();
        let path = temp.path().join("home").join(STYLE_NAME);
        assert_eq!(from_home(&config), Ok(None));
        fs::write(&path, "\u{feff} \r\n\t").unwrap();
        assert_eq!(from_home(&config), Ok(None));
        fs::write(
            &path,
            "\u{feff}Write in French.\r\nKeep {{page}} and {{#office}} literal.\t\"Yes\"",
        )
        .unwrap();
        assert_eq!(
            from_home(&config).unwrap().unwrap(),
            "Write in French.\nKeep {{page}} and {{#office}} literal.\t\"Yes\""
        );
        fs::write(&path, "x".repeat(MAX_STYLE_BYTES)).unwrap();
        assert_eq!(from_home(&config).unwrap().unwrap().len(), MAX_STYLE_BYTES);
    }

    #[test]
    fn invalid_home_style_is_actionable_without_echoing_its_contents() {
        let (temp, config) = home();
        let path = temp.path().join("home").join(STYLE_NAME);
        for bytes in [
            vec![b'x'; MAX_STYLE_BYTES + 1],
            vec![0xff],
            b"private-sentinel\0".to_vec(),
            b"lone\rreturn".to_vec(),
        ] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(from_home(&config), Err(HOME_ERROR));
        }
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(from_home(&config), Err(HOME_ERROR));
        fs::remove_dir(&path).unwrap();
        let other = temp.path().join("private.txt");
        fs::write(&other, "private-sentinel").unwrap();
        symlink(&other, &path).unwrap();
        assert_eq!(from_home(&config), Err(HOME_ERROR));
        fs::remove_file(&path).unwrap();
        symlink("missing", &path).unwrap();
        assert_eq!(from_home(&config), Err(HOME_ERROR));
    }

    #[test]
    fn replaced_home_cannot_supply_style_under_an_old_picker_identity() {
        let (temp, config) = home();
        let root = temp.path().join("home");
        fs::rename(&root, temp.path().join("old-home")).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join(STYLE_NAME), "Use this replacement").unwrap();
        assert_eq!(from_home(&config), Err(HOME_ERROR));
    }

    #[test]
    fn snapshots_are_bounded_validated_and_independent_of_the_live_file() {
        let (temp, config) = home();
        let live = temp.path().join("home").join(STYLE_NAME);
        fs::write(&live, "Emphasize decisions.").unwrap();
        let style = from_home(&config).unwrap();
        let run = temp.path().join("run");
        fs::create_dir(&run).unwrap();
        assert_eq!(from_run(&run), Ok(None));
        let path = run.join(SNAPSHOT_NAME);
        fs::write(&path, encode(style.as_deref()).unwrap()).unwrap();
        fs::write(&live, "Different language").unwrap();
        assert_eq!(from_run(&run), Ok(style.clone()));
        fs::remove_file(&live).unwrap();
        assert_eq!(from_run(&run), Ok(style));
        // Escaped input is bounded after decoding as well as before it.
        for bytes in [
            b"not JSON".to_vec(),
            b"{}".to_vec(),
            serde_json::to_vec(&"x".repeat(MAX_STYLE_BYTES + 1)).unwrap(),
            vec![b' '; MAX_SNAPSHOT_BYTES + 1],
            br#""bad\u0000style""#.to_vec(),
        ] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(from_run(&run), Err(SNAPSHOT_ERROR));
        }
        fs::remove_file(&path).unwrap();
        symlink(&live, &path).unwrap();
        assert_eq!(from_run(&run), Err(SNAPSHOT_ERROR));
        fs::remove_file(&path).unwrap();
        fs::write(&path, encode(None).unwrap()).unwrap();
        assert_eq!(from_run(&run), Ok(None));
    }
}

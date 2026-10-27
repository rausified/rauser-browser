//! The optional `agent.env` overlay: `KEY=VALUE` lines the service adds to
//! every run's environment. Values never leave [`AgentEnv`]; only names do.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, ErrorKind, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

/// The largest `agent.env` the service reads.
const MAX_BYTES: u64 = 64 * 1024;

/// The overlay read from `agent.env`. A missing file is an empty overlay.
pub struct AgentEnv {
    vars: Vec<(String, String)>,
}

/// Why `agent.env` was refused. No variant carries a value from the file.
#[derive(Debug)]
pub enum AgentEnvError {
    NotPrivate { mode: u32 },
    Symlink,
    Line { number: usize, reason: &'static str },
    Io(io::Error),
}

impl fmt::Display for AgentEnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPrivate { mode } => {
                write!(f, "agent.env must have mode 0600, not {mode:04o}")
            }
            Self::Symlink => f.write_str("agent.env is a symlink"),
            Self::Line { number, reason } => write!(f, "agent.env line {number}: {reason}"),
            Self::Io(error) => write!(f, "reading agent.env: {error}"),
        }
    }
}

impl std::error::Error for AgentEnvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for AgentEnvError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl AgentEnv {
    /// Read and validate `agent.env`. The file must be a regular file, not a
    /// symlink, with mode exactly 0600 and at most 64 KiB.
    pub fn read(path: &Path) -> Result<AgentEnv, AgentEnvError> {
        let link = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(AgentEnv { vars: Vec::new() });
            }
            Err(error) => return Err(error.into()),
        };
        if link.file_type().is_symlink() {
            return Err(AgentEnvError::Symlink);
        }
        check_regular_private(&link)?;

        let file = fs::File::open(path)?;
        let opened = file.metadata()?;
        // The path was replaced between the check and the open.
        if opened.dev() != link.dev() || opened.ino() != link.ino() {
            return Err(AgentEnvError::Symlink);
        }
        check_regular_private(&opened)?;

        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(too_large());
        }
        parse(&bytes).map(|vars| AgentEnv { vars })
    }

    /// The names the file sets, sorted. Values are never exposed.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.vars.iter().map(|(key, _)| key.clone()).collect();
        names.sort();
        names
    }

    /// `base` with every key the file sets replaced by the file's value.
    /// Base order is kept; the file's variables follow in file order.
    pub fn overlay(
        &self,
        base: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> Vec<(OsString, OsString)> {
        let mut merged: Vec<(OsString, OsString)> = base
            .into_iter()
            .filter(|(key, _)| !self.vars.iter().any(|(name, _)| key == name.as_str()))
            .collect();
        merged.extend(
            self.vars
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        );
        merged
    }
}

impl fmt::Debug for AgentEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentEnv")
            .field("names", &self.names())
            .finish_non_exhaustive()
    }
}

fn too_large() -> AgentEnvError {
    AgentEnvError::Io(io::Error::new(
        ErrorKind::InvalidData,
        "agent.env is larger than 64 KiB",
    ))
}

fn check_regular_private(metadata: &fs::Metadata) -> Result<(), AgentEnvError> {
    if !metadata.is_file() {
        return Err(AgentEnvError::Io(io::Error::new(
            ErrorKind::InvalidInput,
            "agent.env is not a regular file",
        )));
    }
    let mode = metadata.permissions().mode() & 0o7777;
    if mode != 0o600 {
        return Err(AgentEnvError::NotPrivate { mode });
    }
    if metadata.len() > MAX_BYTES {
        return Err(too_large());
    }
    Ok(())
}

fn parse(bytes: &[u8]) -> Result<Vec<(String, String)>, AgentEnvError> {
    let mut vars = Vec::new();
    let mut seen = HashSet::new();
    for (index, raw) in bytes.split(|&byte| byte == b'\n').enumerate() {
        let number = index + 1;
        let line_error = |reason| AgentEnvError::Line { number, reason };
        let line = std::str::from_utf8(raw).map_err(|_| line_error("not UTF-8"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| line_error("expected KEY=VALUE"))?;
        if !is_valid_key(key) {
            return Err(line_error("key must match [A-Z_][A-Z0-9_]*"));
        }
        if value.contains('\0') {
            return Err(line_error("value contains a NUL byte"));
        }
        if !seen.insert(key) {
            return Err(line_error("key repeats an earlier line"));
        }
        vars.push((key.to_owned(), value.to_owned()));
    }
    Ok(vars)
}

fn is_valid_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_uppercase() || first == b'_' => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_env(dir: &Path, contents: &str, mode: u32) -> std::path::PathBuf {
        let path = dir.join("agent.env");
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn os(key: &str, value: &str) -> (OsString, OsString) {
        (OsString::from(key), OsString::from(value))
    }

    #[test]
    fn missing_file_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        let env = AgentEnv::read(&temp.path().join("agent.env")).unwrap();
        assert!(env.names().is_empty());
        assert_eq!(env.overlay([os("HOME", "/h")]), vec![os("HOME", "/h")]);
    }

    #[test]
    fn overlay_replaces_base_values() {
        let temp = tempfile::tempdir().unwrap();
        let path = write_env(
            temp.path(),
            "# Bedrock\n\nAWS_PROFILE=work\nCLAUDE_CODE_USE_BEDROCK=1\nQUOTED=\"$HOME\"\n",
            0o600,
        );
        let env = AgentEnv::read(&path).unwrap();
        assert_eq!(
            env.names(),
            vec!["AWS_PROFILE", "CLAUDE_CODE_USE_BEDROCK", "QUOTED"]
        );
        let merged = env.overlay([os("HOME", "/h"), os("AWS_PROFILE", "default")]);
        assert_eq!(
            merged,
            vec![
                os("HOME", "/h"),
                os("AWS_PROFILE", "work"),
                os("CLAUDE_CODE_USE_BEDROCK", "1"),
                // No quote stripping and no `$` expansion.
                os("QUOTED", "\"$HOME\""),
            ]
        );
    }

    #[test]
    fn group_readable_file_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = write_env(temp.path(), "A=1\n", 0o640);
        match AgentEnv::read(&path) {
            Err(AgentEnvError::NotPrivate { mode }) => assert_eq!(mode, 0o640),
            other => panic!("expected NotPrivate, got {other:?}"),
        }
        // 0400 is not exactly 0600 either.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(matches!(
            AgentEnv::read(&path),
            Err(AgentEnvError::NotPrivate { mode: 0o400 })
        ));
    }

    #[test]
    fn symlinked_file_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real.env");
        std::fs::write(&real, "A=1\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = temp.path().join("agent.env");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(AgentEnv::read(&link), Err(AgentEnvError::Symlink)));
    }

    #[test]
    fn directory_and_oversized_files_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("agent.env");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(AgentEnv::read(&dir).is_err());

        let other = tempfile::tempdir().unwrap();
        let big = format!("A={}\n", "x".repeat(MAX_BYTES as usize));
        let path = write_env(other.path(), &big, 0o600);
        assert!(matches!(AgentEnv::read(&path), Err(AgentEnvError::Io(_))));
    }

    #[test]
    fn bad_key_reports_line_number() {
        let temp = tempfile::tempdir().unwrap();
        for (contents, number) in [
            ("A=1\n# ok\nlower=2\n", 3),
            ("A=1\n1ABC=2\n", 2),
            ("NOEQUALS\n", 1),
            ("=empty\n", 1),
            (" A=1\n", 1),
            ("A=1\nB=2\nA=3\n", 3),
        ] {
            let path = write_env(temp.path(), contents, 0o600);
            match AgentEnv::read(&path) {
                Err(AgentEnvError::Line { number: got, .. }) => {
                    assert_eq!(got, number, "{contents:?}")
                }
                other => panic!("expected Line error for {contents:?}, got {other:?}"),
            }
        }
        let path = write_env(temp.path(), "A=1\nB=2\nA=3\n", 0o600);
        match AgentEnv::read(&path) {
            Err(AgentEnvError::Line { reason, .. }) => assert!(reason.contains("repeat")),
            other => panic!("expected Line error, got {other:?}"),
        }
    }

    #[test]
    fn debug_never_prints_values() {
        let temp = tempfile::tempdir().unwrap();
        let path = write_env(temp.path(), "AWS_SECRET_ACCESS_KEY=hunter2secret\n", 0o600);
        let env = AgentEnv::read(&path).unwrap();
        let debug = format!("{env:?}");
        assert!(debug.contains("AWS_SECRET_ACCESS_KEY"), "{debug}");
        assert!(!debug.contains("hunter2secret"), "{debug}");
        let pretty = format!("{env:#?}");
        assert!(!pretty.contains("hunter2secret"), "{pretty}");

        // Errors never quote the offending line either.
        let path = write_env(temp.path(), "bad key=hunter2secret\n", 0o600);
        let error = AgentEnv::read(&path).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("hunter2secret"));
    }
}

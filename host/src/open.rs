//! Opening a link or revealing a file the way the user's desktop does. A
//! release macOS build runs `/usr/bin/open`; test builds never do, and only
//! append what they would have opened to the file named by `OPEN_LOG_ENV`.
//! Other platforms have no opener yet.

use std::ffi::OsString;
use std::io;

/// Opens `args` as `/usr/bin/open` would take them: a URL, or `-R <path>` to
/// reveal a file in Finder. A plain function so a test can pass a recorder.
pub type Opener = fn(args: &[OsString]) -> io::Result<()>;

/// The one message for an opener that returns `io::ErrorKind::Unsupported`,
/// whichever request asked to open something.
pub(crate) const UNSUPPORTED_MESSAGE: &str = "Opening is only supported on macOS.";

/// The opener this build uses.
pub fn default_opener() -> Opener {
    #[cfg(any(test, feature = "scripted-dialogs"))]
    {
        scripted_open
    }
    #[cfg(all(not(any(test, feature = "scripted-dialogs")), target_os = "macos"))]
    {
        system_open
    }
    #[cfg(all(not(any(test, feature = "scripted-dialogs")), not(target_os = "macos")))]
    {
        unsupported
    }
}

/// `/usr/bin/open <args>`; a non-zero exit is an error.
#[cfg(target_os = "macos")]
#[cfg_attr(any(test, feature = "scripted-dialogs"), allow(dead_code))]
fn system_open(args: &[OsString]) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let status = Command::new("/usr/bin/open")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("open exited with {status}")))
    }
}

#[cfg(not(target_os = "macos"))]
#[cfg_attr(any(test, feature = "scripted-dialogs"), allow(dead_code))]
fn unsupported(_args: &[OsString]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "opening is only supported on macOS",
    ))
}

#[cfg(any(test, feature = "scripted-dialogs"))]
fn scripted_open(args: &[OsString]) -> io::Result<()> {
    scripted_open_to(
        std::env::var_os(crate::brand::OPEN_LOG_ENV).as_deref(),
        args,
    )
}

/// Append `args`, space-separated, as one line to `log`. Without a log the
/// test build refuses rather than open anything.
#[cfg(any(test, feature = "scripted-dialogs"))]
fn scripted_open_to(log: Option<&std::ffi::OsStr>, args: &[OsString]) -> io::Result<()> {
    use std::io::Write;
    let Some(log) = log.filter(|log| std::path::Path::new(log).is_absolute()) else {
        return Err(io::Error::other("open is disabled in test builds"));
    };
    let line = args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    if line.contains('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an open argument contains a newline",
        ));
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn scripted_opener_without_log_env_refuses() {
        let error = scripted_open_to(None, &args(&["codex://threads/t"])).unwrap_err();
        assert_eq!(error.to_string(), "open is disabled in test builds");
        // A relative log path is refused the same way.
        let error = scripted_open_to(Some("open.log".as_ref()), &args(&["x"])).unwrap_err();
        assert_eq!(error.to_string(), "open is disabled in test builds");
    }

    #[test]
    fn scripted_opener_appends_one_line_per_open() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("open.log");
        scripted_open_to(Some(log.as_os_str()), &args(&["codex://threads/t-1"])).unwrap();
        scripted_open_to(Some(log.as_os_str()), &args(&["-R", "/tmp/agent.env"])).unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "codex://threads/t-1\n-R /tmp/agent.env\n"
        );
        assert!(scripted_open_to(Some(log.as_os_str()), &args(&["a\nb"])).is_err());
    }
}

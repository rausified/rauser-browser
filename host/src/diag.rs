//! Local diagnostics: structured logs and spans for the native host, the
//! service, and the CLI (DESIGN.md §15).
//!
//! Each process writes JSON lines to its own size-capped file in the log
//! directory (`host.jsonl`, `service.jsonl`, `cli.jsonl`), rotated to `.1`
//! through `.3`. Every line carries the root `process` span (`proc`, `pid`,
//! `build`), so lines from many short-lived host processes stay apart, and
//! the spans the event happened in. A span's close line carries `time.busy`
//! and `time.idle`; outcome events carry an explicit `elapsed_ms`. The
//! extension's `request_id` and the service's `run_id` are span fields, so
//! one request can be followed from the extension, through the host, into
//! the service and its run.
//!
//! Warnings and errors are also mirrored to stderr as plain text: Chrome's
//! log for the host, launchd's `service.log` for the service.
//!
//! Nothing here leaves the computer. Never log page text, note text, a
//! prompt, an answer, a harness's output, or any `agent.env` value; log a
//! URL only through [`url_origin`].
//!
//! `BRAUSER_LOG` sets the filter (`EnvFilter` syntax, default `info`);
//! `BRAUSER_LOG_DIR` moves the directory. Test and `scripted-dialogs`
//! builds write no file unless `BRAUSER_LOG_DIR` is set, so tests never
//! touch the real log directory.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, filter::LevelFilter};

use crate::brand::APP_NAME;

/// The filter variable, in `EnvFilter` syntax (`debug`, `brauser_host=trace`).
pub const LOG_FILTER_ENV: &str = "BRAUSER_LOG";
/// Moves the log directory; required for any file in test builds.
pub const LOG_DIR_ENV: &str = "BRAUSER_LOG_DIR";
/// A log file is rotated once it reaches this size.
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Rotated copies kept beside the live file (`.1` is the newest).
pub const KEPT_ROTATIONS: usize = 3;

/// Which process is logging; names its file and its `proc` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Process {
    /// The native messaging host Chrome starts per connection.
    Host,
    /// The per-user service that owns agent runs.
    Service,
    /// Command-line tools (`reindex`, `adapters`).
    Cli,
}

impl Process {
    pub fn name(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Service => "service",
            Self::Cli => "cli",
        }
    }

    pub fn file_name(self) -> String {
        format!("{}.jsonl", self.name())
    }
}

/// Keeps the root `process` span entered for the life of the process.
pub struct Guard {
    _entered: tracing::span::EnteredSpan,
}

/// Install the subscriber and panic hook. Returns `None` when a subscriber
/// is already installed (a second call, or a test harness's own).
/// Never fails the process: a log directory that cannot be opened leaves
/// stderr only, with one line saying why.
pub fn init(process: Process) -> Option<Guard> {
    let filter = || {
        EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .with_env_var(LOG_FILTER_ENV)
            .from_env_lossy()
    };
    let (file, file_error) = match log_dir() {
        Some(dir) => match RotatingFile::open(&dir, &process.file_name()) {
            Ok(file) => (Some(file), None),
            Err(error) => (None, Some(format!("{}: {error}", dir.display()))),
        },
        None => (None, None),
    };
    let file_layer = file.map(|file| {
        tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(false)
            .with_span_list(true)
            .with_span_events(FmtSpan::CLOSE)
            .with_writer(file)
            .with_filter(filter())
    });
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .without_time()
        .with_writer(io::stderr)
        .with_filter(LevelFilter::WARN);
    tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .ok()?;
    install_panic_hook();
    if let Some(error) = file_error {
        tracing::warn!(%error, "{APP_NAME} diagnostics file unavailable; logging to stderr only");
    }
    let root = tracing::info_span!(
        "process",
        proc = process.name(),
        pid = std::process::id(),
        build = crate::BUILD_LABEL,
    );
    Some(Guard {
        _entered: root.entered(),
    })
}

/// The log directory: `BRAUSER_LOG_DIR`, else `~/Library/Logs/Brauser` on
/// macOS and the per-user local data directory's `logs` elsewhere. Test and
/// `scripted-dialogs` builds have no default.
pub fn log_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(LOG_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if cfg!(any(test, feature = "scripted-dialogs")) {
        return None;
    }
    default_log_dir()
}

fn default_log_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
        Some(home.join("Library").join("Logs").join(APP_NAME))
    } else {
        directories::ProjectDirs::from("", "", APP_NAME)
            .map(|dirs| dirs.data_local_dir().join("logs"))
    }
}

/// Log panics as `error` events (with the spans they happened in) before
/// the default hook prints them.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()));
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        tracing::error!(panic = %payload, location = location.as_deref(), "panic");
        previous(info);
    }));
}

/// A URL reduced to its scheme and host (`https://example.com`), the only
/// form of a URL diagnostics record. Anything unparseable is `"invalid"`.
pub fn url_origin(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => match parsed.host_str() {
            Some(host) => format!("{}://{host}", parsed.scheme()),
            None => format!("{}:", parsed.scheme()),
        },
        Err(_) => "invalid".to_owned(),
    }
}

/// Whole milliseconds since `start`, for `elapsed_ms` fields.
pub fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// A size-capped, append-only file shared by every writer in the process.
/// Other processes appending to the same name (several native hosts) may
/// race a rotation; the worst case is one rotation too many, never a lost
/// live file.
#[derive(Clone)]
pub struct RotatingFile {
    inner: Arc<Mutex<Rotating>>,
}

struct Rotating {
    path: PathBuf,
    file: File,
    written: u64,
}

impl RotatingFile {
    pub fn open(dir: &Path, name: &str) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(name);
        let (file, written) = open_append(&path)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Rotating {
                path,
                file,
                written,
            })),
        })
    }
}

fn open_append(path: &Path) -> io::Result<(File, u64)> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    let written = file.metadata()?.len();
    Ok((file, written))
}

fn rotated(path: &Path, index: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

impl Rotating {
    fn rotate(&mut self) -> io::Result<()> {
        // Another process may have rotated first; follow the live file when
        // it is already small again instead of rotating it a second time.
        if let Ok(metadata) = fs::metadata(&self.path)
            && metadata.len() < MAX_FILE_BYTES
        {
            (self.file, self.written) = open_append(&self.path)?;
            return Ok(());
        }
        for index in (1..KEPT_ROTATIONS).rev() {
            let _ = fs::rename(rotated(&self.path, index), rotated(&self.path, index + 1));
        }
        let _ = fs::rename(&self.path, rotated(&self.path, 1));
        (self.file, self.written) = open_append(&self.path)?;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.written >= MAX_FILE_BYTES {
            inner.rotate()?;
        }
        // One write per event line keeps O_APPEND lines from interleaving.
        inner.file.write_all(buf)?;
        inner.written += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RotatingFile {
    type Writer = RotatingFile;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Captures what a block logs, as the JSON lines the log file would get,
/// so tests can check fields and that no content leaks. Captures `debug`
/// and up, so a leak at a level the default filter hides still fails.
#[cfg(test)]
pub(crate) mod test_capture {
    use std::cell::RefCell;
    use std::io;
    use std::sync::Once;

    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::fmt::format::FmtSpan;

    thread_local! {
        /// This thread's capture, while one runs.
        static CAPTURED: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    }

    /// Appends to this thread's capture; other threads' lines are dropped.
    struct ThreadWriter;

    impl io::Write for ThreadWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            CAPTURED.with(|captured| {
                if let Some(captured) = captured.borrow_mut().as_mut() {
                    captured.extend_from_slice(buf);
                }
            });
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// One global subscriber for the whole test binary. A scoped one
    /// (`with_default`) is not enough: a callsite that another test thread
    /// registers while no capture is live can cache "never" and hide its
    /// events from a capture that starts a moment later.
    fn install() {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .json()
                .with_current_span(false)
                .with_span_list(true)
                .with_span_events(FmtSpan::CLOSE)
                .with_max_level(LevelFilter::DEBUG)
                .with_writer(|| ThreadWriter)
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
        });
    }

    /// Run `body` on this thread, capturing what it logs as the log file
    /// would get it; returns its result, the lines, and their text.
    pub(crate) fn capture<T>(body: impl FnOnce() -> T) -> (T, Vec<serde_json::Value>, String) {
        install();
        CAPTURED.with(|captured| *captured.borrow_mut() = Some(Vec::new()));
        let result = body();
        let bytes = CAPTURED
            .with(|captured| captured.borrow_mut().take())
            .unwrap_or_default();
        let text = String::from_utf8(bytes).expect("log lines are UTF-8");
        let lines = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("each log line is JSON"))
            .collect();
        (result, lines, text)
    }

    /// Every captured event whose message is `message`.
    pub(crate) fn events<'a>(
        lines: &'a [serde_json::Value],
        message: &str,
    ) -> Vec<&'a serde_json::Value> {
        lines
            .iter()
            .filter(|line| line["fields"]["message"] == message)
            .collect()
    }

    /// The innermost span named `name` that `line` happened in, with its
    /// fields.
    pub(crate) fn span<'a>(
        line: &'a serde_json::Value,
        name: &str,
    ) -> Option<&'a serde_json::Value> {
        line["spans"]
            .as_array()?
            .iter()
            .rev()
            .find(|span| span["name"] == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_keeps_only_scheme_and_host() {
        assert_eq!(
            url_origin("https://user:pw@example.com:8443/a/b?q=secret#frag"),
            "https://example.com"
        );
        assert_eq!(url_origin("about:blank"), "about:");
        assert_eq!(url_origin("not a url"), "invalid");
    }

    #[test]
    fn test_builds_have_no_default_log_dir() {
        // SAFETY of the assertion: tests never set BRAUSER_LOG_DIR globally.
        if std::env::var_os(LOG_DIR_ENV).is_none() {
            assert_eq!(log_dir(), None);
        }
    }

    #[test]
    fn rotates_at_the_cap_and_keeps_three() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = RotatingFile::open(dir.path(), "x.jsonl").unwrap();
        let line = vec![b'a'; 1024 * 1024];
        for _ in 0..(5 * 5 + 3) {
            file.write_all(&line).unwrap();
        }
        let live = dir.path().join("x.jsonl");
        assert!(fs::metadata(&live).unwrap().len() <= MAX_FILE_BYTES);
        for index in 1..=KEPT_ROTATIONS {
            assert!(rotated(&live, index).exists(), "missing .{index}");
        }
        assert!(!rotated(&live, KEPT_ROTATIONS + 1).exists());
    }

    #[test]
    fn follows_a_rotation_done_by_another_process() {
        let dir = tempfile::tempdir().unwrap();
        let mut first = RotatingFile::open(dir.path(), "x.jsonl").unwrap();
        let mut second = RotatingFile::open(dir.path(), "x.jsonl").unwrap();
        let line = vec![b'a'; MAX_FILE_BYTES as usize];
        first.write_all(&line).unwrap();
        second.inner.lock().unwrap().written = MAX_FILE_BYTES;
        first.write_all(b"x").unwrap();
        // `second` sees a small live file and reopens instead of rotating.
        second.write_all(b"y").unwrap();
        let live = dir.path().join("x.jsonl");
        assert_eq!(fs::read(&live).unwrap(), b"xy");
        assert!(!rotated(&live, 2).exists());
    }

    #[test]
    #[cfg(unix)]
    fn log_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        RotatingFile::open(dir.path(), "x.jsonl").unwrap();
        let mode = fs::metadata(dir.path().join("x.jsonl"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

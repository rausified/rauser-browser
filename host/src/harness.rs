//! AI harness detection (spec §6.1). The service's harness setup calls
//! [`detect`] with the search path and child environment it runs harnesses
//! with, so tests never read the host's own `PATH` or environment.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use brauser_protocol::HarnessId;

use crate::brand::APP_NAME;

pub const UNSUPPORTED_PLATFORM: &str = "AI harness setup is not supported on this platform yet";

/// The setup probe's prompt. Neither harness has a max-tokens flag, so the
/// "one-token probe" is approximated by a prompt whose answer is one word.
pub const PROBE_PROMPT: &str = "Reply with the single word OK.";

const STDERR_CAP: usize = 64 * 1024;
/// How much stderr a failed check writes to the host's own log.
const STDERR_LOG_BYTES: usize = 2 * 1024;

pub const VERSION_LIMITS: Limits = Limits {
    timeout: Duration::from_secs(10),
    stdout_cap: 4 * 1024,
    stderr_cap: STDERR_CAP,
};

/// Where harness detection looks.
#[derive(Debug, Clone)]
pub struct HarnessEnv {
    pub search_path: OsString,
    pub home: Option<PathBuf>,
}

impl HarnessEnv {
    /// The search a run with this child environment gets: its `PATH` plus
    /// `$HOME/.local/bin`, as the runners look their programs up.
    pub fn from_run_env(run_env: &[(OsString, OsString)]) -> Self {
        let get = |name: &str| {
            run_env
                .iter()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let home = get("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_absolute());
        Self {
            search_path: search_path(get("PATH").as_deref(), home.as_deref()),
            home,
        }
    }
}

/// The absolute entries of `PATH` plus `$HOME/.local/bin`, first occurrence
/// only. No other install location is added, so a test `PATH` finds only
/// what the test put there.
pub(crate) fn search_path(path: Option<&OsStr>, home: Option<&Path>) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let listed = path.map(|path| std::env::split_paths(path).collect::<Vec<_>>());
    for dir in listed
        .unwrap_or_default()
        .into_iter()
        .chain(home.map(|home| home.join(".local").join("bin")))
    {
        if dir.is_absolute() && !dirs.contains(&dir) && std::env::join_paths([&dir]).is_ok() {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adapter {
    ClaudeCode,
    Codex,
}

impl Adapter {
    pub const ALL: [Self; 2] = [Self::ClaudeCode, Self::Codex];

    pub fn harness_id(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    pub(crate) fn program(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
        }
    }

    // Only the service (macOS) names a harness in its messages.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

impl From<Adapter> for HarnessId {
    fn from(value: Adapter) -> Self {
        match value {
            Adapter::ClaudeCode => Self::Claude,
            Adapter::Codex => Self::Codex,
        }
    }
}

impl From<HarnessId> for Adapter {
    fn from(value: HarnessId) -> Self {
        match value {
            HarnessId::Claude => Self::ClaudeCode,
            HarnessId::Codex => Self::Codex,
        }
    }
}

/// A harness binary's resolved real path, size, and modification time. Both
/// harnesses update themselves in place, so any change means a new binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryIdentity {
    pub real_path: String,
    pub size: u64,
    pub mtime_ns: String,
    /// `dev:ino` on Unix.
    pub file_id: Option<String>,
}

pub fn identity(path: &Path) -> Result<BinaryIdentity> {
    let real = fs::canonicalize(path).with_context(|| format!("resolving {}", path.display()))?;
    let metadata =
        fs::symlink_metadata(&real).with_context(|| format!("reading {}", real.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a regular file", real.display());
    }
    let mtime_ns = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .context("harness modification time predates 1970")?
        .as_nanos()
        .to_string();
    #[cfg(unix)]
    let file_id = {
        use std::os::unix::fs::MetadataExt;
        Some(format!("{}:{}", metadata.dev(), metadata.ino()))
    };
    #[cfg(not(unix))]
    let file_id = None;
    Ok(BinaryIdentity {
        real_path: real
            .to_str()
            .context("harness path cannot be represented as UTF-8")?
            .to_owned(),
        size: metadata.len(),
        mtime_ns,
        file_id,
    })
}

/// The first executable file with this name, like a shell's lookup.
pub(crate) fn find_on_path(search_path: &OsStr, program: &str) -> Option<PathBuf> {
    std::env::split_paths(search_path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|path| fs::metadata(path).is_ok_and(|metadata| executable(&metadata)))
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
}

/// One supported harness as [`detect`] found it. `binary` is where it was
/// found on the search path, `None` when it was not; `problem` is
/// host-authored text saying why a harness that was found cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub adapter: Adapter,
    pub binary: Option<PathBuf>,
    pub version: Option<String>,
    pub problem: Option<String>,
}

/// Look for each supported harness on `env`'s search path, Codex first, and
/// read its version. Only `--version` runs, with exactly `run_env`: the
/// service's environment and the agent.env overlay a run gets. Any
/// well-formed version is accepted.
pub fn detect(env: &HarnessEnv, run_env: &[(OsString, OsString)]) -> Vec<Detected> {
    [Adapter::Codex, Adapter::ClaudeCode]
        .into_iter()
        .map(|adapter| {
            let Some(binary) = find_on_path(&env.search_path, adapter.program()) else {
                return Detected {
                    adapter,
                    binary: None,
                    version: None,
                    problem: None,
                };
            };
            let (version, problem) = match installed_version(adapter, &binary, run_env) {
                Ok(version) => (Some(version), None),
                Err(problem) => (None, Some(problem)),
            };
            Detected {
                adapter,
                binary: Some(binary),
                version,
                problem,
            }
        })
        .collect()
}

/// The program a run with `run_env` starts for `adapter`: found on the same
/// search [`detect`] uses for that environment, and passing the same install
/// check, so a run never starts a program setup did not look at. The error is
/// host-authored text.
#[cfg(unix)] // Only the Unix-only service uses this.
pub(crate) fn resolve_for_run(
    adapter: Adapter,
    run_env: &[(OsString, OsString)],
) -> Result<PathBuf, String> {
    let program = adapter.program();
    let found = find_on_path(&HarnessEnv::from_run_env(run_env).search_path, program)
        .ok_or_else(|| format!("{program} is not on the service's PATH"))?;
    let real =
        fs::canonicalize(&found).map_err(|_| format!("{} could not be read", found.display()))?;
    check_install(&real).map_err(|problem| format!("{program} cannot be used: {problem}"))?;
    Ok(real)
}

/// Check the install `found` resolves to, then run its `--version`.
fn installed_version(
    adapter: Adapter,
    found: &Path,
    run_env: &[(OsString, OsString)],
) -> Result<String, String> {
    let real =
        fs::canonicalize(found).map_err(|_| format!("{} could not be read", found.display()))?;
    check_install(&real)?;
    let command = format!("{} --version", adapter.program());
    let output = run_bounded(&real, &["--version".to_owned()], run_env, VERSION_LIMITS).map_err(
        |error| {
            eprintln!("{APP_NAME}: {command} could not run: {error:#}");
            format!("{command} could not run")
        },
    )?;
    log_stderr(&command, &output.stderr);
    match output.status {
        RunStatus::Exited(0) => {}
        RunStatus::Exited(code) => return Err(format!("{command} failed (exit {code})")),
        RunStatus::Signaled => return Err(format!("{command} failed (killed by a signal)")),
        RunStatus::TimedOut => return Err(format!("{command} timed out")),
        RunStatus::OutputTooLarge => return Err(format!("{command} produced too much output")),
    }
    match adapter {
        Adapter::ClaudeCode => parse_claude_version(&output.stdout),
        Adapter::Codex => parse_codex_version(&output.stdout),
    }
    .ok_or_else(|| format!("{command} output was not recognized"))
}

/// The real path must be an executable regular file that neither it nor
/// its directory lets another user replace.
#[cfg(unix)]
fn check_install(real: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let unreadable = |path: &Path| format!("{} could not be read", path.display());
    let metadata = fs::symlink_metadata(real).map_err(|_| unreadable(real))?;
    if !executable(&metadata) || metadata.file_type().is_symlink() {
        return Err(format!("{} is not an executable file", real.display()));
    }
    let parent = real.parent().ok_or_else(|| unreadable(real))?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|_| unreadable(parent))?;
    let uid = rustix::process::getuid().as_raw();
    for (path, metadata) in [(real, &metadata), (parent, &parent_metadata)] {
        if metadata.mode() & 0o022 != 0 {
            return Err(format!("{} is writable by other users", path.display()));
        }
        if metadata.uid() != uid && metadata.uid() != 0 {
            return Err(format!("{} is owned by another user", path.display()));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_install(_real: &Path) -> Result<(), String> {
    Err(UNSUPPORTED_PLATFORM.to_owned())
}

fn log_stderr(command: &str, stderr: &[u8]) {
    if !stderr.is_empty() {
        let shown = &stderr[..stderr.len().min(STDERR_LOG_BYTES)];
        eprintln!(
            "{APP_NAME}: {command} stderr: {}",
            String::from_utf8_lossy(shown)
        );
    }
}

/// Exactly `<N.N.N> (Claude Code)`, returning `N.N.N`.
pub fn parse_claude_version(stdout: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(stdout).ok()?.trim();
    let version = text.strip_suffix(" (Claude Code)")?;
    numeric_version(version).then(|| version.to_owned())
}

/// Exactly `codex-cli <N.N.N>`, returning `N.N.N`.
pub fn parse_codex_version(stdout: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(stdout).ok()?.trim();
    let version = text.strip_prefix("codex-cli ")?;
    numeric_version(version).then(|| version.to_owned())
}

fn numeric_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            (1..=9).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Duration,
    pub stdout_cap: usize,
    pub stderr_cap: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Exited(i32),
    Signaled,
    /// The deadline passed, or a process outside the group kept a pipe open.
    TimedOut,
    /// stdout or stderr passed its cap; the process group was killed.
    OutputTooLarge,
}

#[derive(Debug)]
pub struct RunOutput {
    pub status: RunStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run `real_path` directly (no shell) with exactly `env` and no stdin,
/// with capped output and a deadline. On timeout or overflow the whole
/// process group is killed; it is also killed after a normal exit so no
/// background child outlives the run. Only `--version` runs this way.
#[cfg(unix)]
pub fn run_bounded(
    real_path: &Path,
    argv: &[String],
    env: &[(OsString, OsString)],
    limits: Limits,
) -> Result<RunOutput> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    use rustix::process::{Pid, Signal, kill_process_group};

    const POLL: Duration = Duration::from_millis(10);
    const DRAIN_GRACE: Duration = Duration::from_secs(2);

    let mut command = Command::new(real_path);
    command
        .args(argv)
        .env_clear()
        .envs(env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {}", real_path.display()))?;
    let group = Pid::from_child(&child);
    let kill_group = || {
        let _ = kill_process_group(group, Signal::KILL);
    };

    let overflow = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::channel();
    fn drain<R: std::io::Read + Send + 'static>(
        stream: Option<R>,
        index: usize,
        cap: usize,
        overflow: &Arc<AtomicBool>,
        sender: &mpsc::Sender<(usize, Vec<u8>)>,
    ) {
        let overflow = Arc::clone(overflow);
        let sender = sender.clone();
        thread::spawn(move || {
            let captured = stream.map(|stream| read_capped(stream, cap, &overflow));
            let _ = sender.send((index, captured.unwrap_or_default()));
        });
    }
    drain(
        child.stdout.take(),
        0,
        limits.stdout_cap,
        &overflow,
        &sender,
    );
    drain(
        child.stderr.take(),
        1,
        limits.stderr_cap,
        &overflow,
        &sender,
    );
    drop(sender);

    let deadline = Instant::now() + limits.timeout;
    let mut status = loop {
        match child.try_wait() {
            Ok(Some(exit)) => {
                break match exit.code() {
                    Some(code) => RunStatus::Exited(code),
                    None => RunStatus::Signaled,
                };
            }
            Ok(None) => {}
            Err(error) => {
                kill_group();
                let _ = child.wait();
                return Err(error).context("waiting for the AI harness");
            }
        }
        if overflow.load(Ordering::SeqCst) {
            break RunStatus::OutputTooLarge;
        }
        if Instant::now() >= deadline {
            break RunStatus::TimedOut;
        }
        thread::sleep(POLL);
    };
    kill_group();
    let _ = child.wait();

    let mut outputs = [Vec::new(), Vec::new()];
    let drain_deadline = Instant::now() + DRAIN_GRACE;
    for _ in 0..outputs.len() {
        let wait = drain_deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(wait) {
            Ok((index, captured)) => outputs[index] = captured,
            Err(_) => {
                status = RunStatus::TimedOut;
                break;
            }
        }
    }
    if overflow.load(Ordering::SeqCst) {
        status = RunStatus::OutputTooLarge;
    }
    let [stdout, stderr] = outputs;
    Ok(RunOutput {
        status,
        stdout,
        stderr,
    })
}

#[cfg(not(unix))]
pub fn run_bounded(
    _real_path: &Path,
    _argv: &[String],
    _env: &[(OsString, OsString)],
    _limits: Limits,
) -> Result<RunOutput> {
    bail!(UNSUPPORTED_PLATFORM)
}

/// Read up to `cap` bytes. Past it, flag the overflow and stop reading so
/// the caller kills the process group.
#[cfg(unix)]
fn read_capped(
    mut stream: impl std::io::Read,
    cap: usize,
    overflow: &std::sync::atomic::AtomicBool,
) -> Vec<u8> {
    let mut captured = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if captured.len() + read > cap {
                    captured.extend_from_slice(&chunk[..cap - captured.len()]);
                    overflow.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                captured.extend_from_slice(&chunk[..read]);
            }
        }
    }
    captured
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unix paths, so `/usr/bin` is absolute.
    #[cfg(unix)]
    #[test]
    fn search_path_keeps_absolute_entries_and_adds_local_bin() {
        let joined =
            std::env::join_paths(["/usr/bin", "relative/bin", "", "/bin", "/usr/bin"]).unwrap();
        let path = search_path(Some(&joined), Some(Path::new("/home/user")));
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(
            dirs,
            [
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                Path::new("/home/user").join(".local").join("bin"),
            ]
        );
        assert!(search_path(None, None).is_empty());
    }

    #[test]
    fn claude_version_must_parse_exactly() {
        assert_eq!(
            parse_claude_version(b"2.1.284 (Claude Code)\n").as_deref(),
            Some("2.1.284")
        );
        for bad in [
            &b"2.1.284"[..],
            b"2.1 (Claude Code)",
            b"2.1.x (Claude Code)",
            b"v2.1.284 (Claude Code)",
            b"note\n2.1.284 (Claude Code)",
            b"2.1.284 (Claude Code) beta",
            b"",
        ] {
            assert_eq!(parse_claude_version(bad), None, "{bad:?}");
        }
        // Codex's form, kept from the removed reviewed-version test.
        assert_eq!(
            parse_codex_version(b"codex-cli 0.144.4\n").as_deref(),
            Some("0.144.4")
        );
        for bad in ["0.144.4", "codex-cli 0.144", "codex-cli 0.144.4 beta"] {
            assert_eq!(parse_codex_version(bad.as_bytes()), None, "{bad}");
        }
    }
}

/// Fake harnesses for process tests here and in `service::setup`. Each is a
/// `#!/bin/sh` script in a temp dir that logs its argv, `$0`, and
/// environment names to files there.
#[cfg(all(test, unix))]
pub(crate) mod fake {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    pub(crate) const PASSING_BODY: &str =
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"OK"}'"#;
    /// Names /bin/sh itself exports into `env`'s output.
    const SHELL_NAMES: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_"];

    pub(crate) struct Fake {
        root: tempfile::TempDir,
    }

    impl Fake {
        pub(crate) fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            for dir in ["bin", "log", "home"] {
                let path = root.path().join(dir);
                fs::create_dir(&path).unwrap();
                // Tests must not depend on the invoking shell's umask. The
                // fake binary directory models a secure installation even on
                // builders whose cooperative umask would otherwise make a
                // freshly created directory group-writable.
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self { root }
        }

        pub(crate) fn path(&self, name: &str) -> PathBuf {
            self.root.path().join(name)
        }

        /// Write a fake harness at `bin/<name>`, answering `--version` with
        /// `version` and anything else with `body`.
        pub(crate) fn install(&self, name: &str, version: &str, body: &str) -> PathBuf {
            let script = self.path("bin").join(name);
            self.write_script(&script, name, version, body);
            script
        }

        pub(crate) fn write_script(&self, script: &Path, name: &str, version: &str, body: &str) {
            let log = self.path("log").join(name);
            fs::create_dir_all(&log).unwrap();
            let text = format!(
                r#"#!/bin/sh
log='{log}'
for arg in "$@"; do printf '[%s]' "$arg"; done >> "$log/argv.log"
echo >> "$log/argv.log"
printf '%s\n' "$0" >> "$log/argv0.log"
env | cut -d= -f1 | sort | tr '\n' ' ' >> "$log/env.log"
echo >> "$log/env.log"
case "$1" in
  --version) echo '{version}'; exit 0 ;;
esac
{body}
"#,
                log = log.display(),
            );
            fs::write(script, text).unwrap();
            fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
        }

        pub(crate) fn claude(&self, body: &str) -> PathBuf {
            self.install("claude", "2.1.284 (Claude Code)", body)
        }

        pub(crate) fn log(&self, name: &str, file: &str) -> Option<String> {
            fs::read_to_string(self.path("log").join(name).join(file)).ok()
        }

        /// Each run's environment names, without the ones /bin/sh adds.
        pub(crate) fn env_names(&self, name: &str) -> Vec<Vec<String>> {
            self.log(name, "env.log")
                .unwrap_or_default()
                .lines()
                .map(|line| {
                    line.split_whitespace()
                        .filter(|name| !SHELL_NAMES.contains(name))
                        .map(str::to_owned)
                        .collect()
                })
                .collect()
        }
    }
}

/// Process tests, using the fake harnesses in [`fake`].
#[cfg(all(test, unix))]
mod process_tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::fake::{Fake, PASSING_BODY};
    use super::*;

    /// Search only the fake bin directory, with a run `PATH` that lets the
    /// fake scripts find `env`, `cut`, and `sort`.
    fn env_for(fake: &Fake) -> (HarnessEnv, Vec<(OsString, OsString)>) {
        let env = HarnessEnv {
            search_path: fake.path("bin").into_os_string(),
            home: Some(fake.path("home")),
        };
        let run_env = vec![
            ("HOME".into(), fake.path("home").into_os_string()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ];
        (env, run_env)
    }

    fn detected_claude(fake: &Fake) -> Detected {
        let (env, run_env) = env_for(fake);
        detect(&env, &run_env)
            .into_iter()
            .find(|found| found.adapter == Adapter::ClaudeCode)
            .unwrap()
    }

    #[test]
    fn detect_uses_only_injected_search_path() {
        let fake = Fake::new();
        let script = fake.claude(PASSING_BODY);
        let (env, run_env) = env_for(&fake);
        let found = detect(&env, &run_env);
        assert_eq!(
            found.iter().map(|found| found.adapter).collect::<Vec<_>>(),
            [Adapter::Codex, Adapter::ClaudeCode]
        );
        assert_eq!(found[0].binary, None);
        assert_eq!(found[1].binary.as_ref(), Some(&script));
        assert_eq!(found[1].version.as_deref(), Some("2.1.284"));

        // No search path means nothing is found, even with a real harness
        // installed for this user. Relative entries are never searched.
        for search_path in ["", "bin"] {
            let empty = HarnessEnv {
                search_path: OsString::from(search_path),
                ..env.clone()
            };
            assert!(
                detect(&empty, &run_env)
                    .iter()
                    .all(|found| found.binary.is_none())
            );
        }
    }

    #[test]
    fn discover_follows_symlink_to_real_path_identity() {
        let fake = Fake::new();
        let real_dir = fake.path("versions");
        fs::create_dir(&real_dir).unwrap();
        fs::set_permissions(&real_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let real = real_dir.join("2.1.284");
        fake.write_script(&real, "claude", "2.1.284 (Claude Code)", PASSING_BODY);
        let link = fake.path("bin").join("claude");
        symlink(&real, &link).unwrap();

        let found = detected_claude(&fake);
        assert_eq!(found.binary.as_ref(), Some(&link));
        assert_eq!(found.version.as_deref(), Some("2.1.284"));
        let identity = super::identity(&link).unwrap();
        assert_eq!(identity, super::identity(&real).unwrap());
        assert_eq!(
            PathBuf::from(&identity.real_path),
            fs::canonicalize(&real).unwrap()
        );
        // The host runs the canonical path it checked, never the link.
        let argv0 = fake.log("claude", "argv0.log").unwrap();
        assert_eq!(
            argv0.lines().collect::<Vec<_>>(),
            [identity.real_path.as_str()]
        );
    }

    #[test]
    fn refuses_world_writable_binary_or_dir() {
        for (binary_mode, dir_mode) in [
            (0o777, 0o755),
            (0o775, 0o755),
            (0o755, 0o777),
            (0o755, 0o775),
        ] {
            let fake = Fake::new();
            let script = fake.claude(PASSING_BODY);
            fs::set_permissions(&script, fs::Permissions::from_mode(binary_mode)).unwrap();
            fs::set_permissions(fake.path("bin"), fs::Permissions::from_mode(dir_mode)).unwrap();
            let found = detected_claude(&fake);
            let problem = found.problem.clone().unwrap();
            assert!(problem.ends_with("is writable by other users"), "{problem}");
            assert_eq!(found.version, None);
            assert_eq!(fake.log("claude", "argv.log"), None);
            fs::set_permissions(fake.path("bin"), fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn discover_refuses_unparseable_version() {
        let fake = Fake::new();
        fake.install("claude", "2.1.284", PASSING_BODY);
        let found = detected_claude(&fake);
        assert_eq!(
            found.problem.as_deref(),
            Some("claude --version output was not recognized")
        );
        assert_eq!(found.version, None);
        assert_eq!(
            fake.log("claude", "argv.log").as_deref(),
            Some("[--version]\n")
        );
    }

    #[test]
    fn version_check_passes_the_service_env_through() {
        let fake = Fake::new();
        fake.claude(PASSING_BODY);
        let (env, mut run_env) = env_for(&fake);
        run_env.extend([
            ("AWS_PROFILE".into(), "work".into()),
            ("CLAUDE_CODE_USE_BEDROCK".into(), "1".into()),
        ]);
        let claude = detect(&env, &run_env)
            .into_iter()
            .find(|found| found.adapter == Adapter::ClaudeCode)
            .unwrap();
        assert_eq!(claude.problem, None);
        assert_eq!(claude.version.as_deref(), Some("2.1.284"));
        assert_eq!(
            fake.log("claude", "argv.log").as_deref(),
            Some("[--version]\n")
        );
        assert_eq!(
            fake.env_names("claude"),
            [["AWS_PROFILE", "CLAUDE_CODE_USE_BEDROCK", "HOME", "PATH"]]
        );
    }
}

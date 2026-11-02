//! Which provider variables the user's login shell sets that a harness run
//! does not see. The service runs under launchd, which reads no shell start-up
//! file, so a harness set up for Bedrock, Vertex or an API key in
//! `~/.zshenv` or `~/.zshrc` fails its probe with a login error instead
//! (2026-11-02: Claude Code reported "Not logged in" because
//! `CLAUDE_CODE_USE_BEDROCK` lived only in `~/.zshenv`). After a failed probe,
//! setup names the missing variables so the user knows what to put in
//! `agent.env`.
//!
//! Only variable names leave this module. Values are read from the shell's
//! output and dropped at once; nothing here is logged.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long the login shell may take to start and print its environment.
/// Interactive start-up files are slow (a zsh with plugins took 4 to 7
/// seconds on 2026-11-02), and this runs only after a failed probe.
pub const SHELL_TIMEOUT: Duration = Duration::from_secs(20);
/// The most shell output read; the rest is ignored.
const MAX_OUTPUT_BYTES: u64 = 1 << 20;
/// Printed before `env -0`, so start-up files that print can be skipped.
const MARKER: &str = "BRAUSER-SHELL-ENV-7c1f";
/// Name prefixes of the variables harnesses read for their provider and
/// login: Anthropic, Claude Code, AWS (Bedrock), OpenAI, Codex, and Google
/// (Vertex).
const PROVIDER_PREFIXES: &[&str] = &[
    "ANTHROPIC_",
    "CLAUDE_CODE_",
    "AWS_",
    "OPENAI_",
    "CODEX_",
    "GOOGLE_",
    "CLOUD_ML_",
];
/// How many names the hint lists before "and N more".
const NAMES_SHOWN: usize = 3;

/// How setup asks for the login shell's variable names: given the run
/// environment, the names, or None when they cannot be read.
pub type ShellNames = dyn Fn(&[(OsString, OsString)]) -> Option<Vec<String>>;

/// The variable names the user's login shell exports, or None when the
/// shell cannot be found or does not answer in time. Tests and the
/// scripted-dialogs smoke build never start the user's real shell.
pub fn login_shell_names(run_env: &[(OsString, OsString)]) -> Option<Vec<String>> {
    if cfg!(any(test, feature = "scripted-dialogs")) {
        return None;
    }
    let shell = login_shell(run_env)?;
    names_from_shell(&shell, run_env, SHELL_TIMEOUT)
}

/// The provider variable names in `shell_names` that `run_env` lacks, sorted
/// and without repeats.
pub fn missing_provider_names(
    shell_names: &[String],
    run_env: &[(OsString, OsString)],
) -> Vec<String> {
    let mut missing: Vec<String> = shell_names
        .iter()
        .filter(|name| {
            PROVIDER_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .filter(|name| {
            !run_env
                .iter()
                .any(|(key, _)| key == OsStr::new(name.as_str()))
        })
        .cloned()
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

/// The sentence setup appends to a failed probe, or None when nothing is
/// missing.
pub fn hint(missing: &[String]) -> Option<String> {
    let (shown, rest) = missing.split_at(missing.len().min(NAMES_SHOWN));
    let names = match (shown, rest.len()) {
        ([], _) => return None,
        ([one], 0) => one.clone(),
        (shown, 0) => {
            let (last, first) = shown.split_last().expect("at least two names");
            format!("{} and {last}", first.join(", "))
        }
        (shown, more) => format!("{} and {more} more", shown.join(", ")),
    };
    let them = if missing.len() == 1 { "it" } else { "them" };
    Some(format!(
        "Your login shell sets {names}, which {}'s service does not see; add {them} to agent.env",
        crate::app_name!(),
    ))
}

/// The user's login shell: `SHELL` from the run environment, or else the
/// account's own shell. It must be an absolute path listed in `/etc/shells`.
fn login_shell(run_env: &[(OsString, OsString)]) -> Option<PathBuf> {
    let get = |name: &str| {
        run_env
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    let from_env = get("SHELL").map(PathBuf::from);
    let from_account = || get("USER").and_then(|user| account_shell(&user.to_string_lossy()));
    let shell = from_env.or_else(from_account)?;
    let allowed = fs::read_to_string("/etc/shells").ok()?;
    (shell.is_absolute() && listed_shell(&allowed, &shell)).then_some(shell)
}

/// Whether `/etc/shells` (its text, `allowed`) lists `shell`.
fn listed_shell(allowed: &str, shell: &Path) -> bool {
    allowed
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .any(|line| Path::new(line) == shell)
}

/// The account's shell, from Directory Services on macOS or `/etc/passwd`
/// elsewhere.
fn account_shell(user: &str) -> Option<PathBuf> {
    if user.is_empty() || user.contains(['/', ':', '\n']) {
        return None;
    }
    if cfg!(target_os = "macos") {
        let output = Command::new("/usr/bin/dscl")
            .args([".", "-read", &format!("/Users/{user}"), "UserShell"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        return text
            .lines()
            .find_map(|line| line.strip_prefix("UserShell:"))
            .map(|shell| PathBuf::from(shell.trim()));
    }
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.len() == 7 && fields[0] == user).then(|| PathBuf::from(fields[6]))
    })
}

/// Start `shell` as an interactive login shell with exactly `run_env`, have
/// it print `env -0` after [`MARKER`], and return the names it printed. The
/// shell is killed at `timeout`.
pub(crate) fn names_from_shell(
    shell: &Path,
    run_env: &[(OsString, OsString)],
    timeout: Duration,
) -> Option<Vec<String>> {
    let script = format!("printf '%s\\0' {MARKER}; exec /usr/bin/env -0");
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .env_clear()
        .envs(run_env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(MAX_OUTPUT_BYTES)
            .read_to_end(&mut output)
            .map(|_| output)
    });
    let deadline = Instant::now() + timeout;
    let exited = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let output = reader.join().ok()?.ok()?;
    if !exited {
        return None;
    }
    Some(parse_names(&output))
}

/// The variable names in `env -0` output after [`MARKER`]. Each value is
/// dropped as soon as its name is split off.
fn parse_names(output: &[u8]) -> Vec<String> {
    let mut entries = output.split(|byte| *byte == 0);
    if !entries.any(|entry| entry.ends_with(MARKER.as_bytes())) {
        return Vec::new();
    }
    entries
        .filter_map(|entry| {
            let name = &entry[..entry.iter().position(|byte| *byte == b'=')?];
            let name = std::str::from_utf8(name).ok()?;
            is_name(name).then(|| name.to_owned())
        })
        .collect()
}

/// Whether `name` is a plain shell variable name.
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    /// A fake login shell: a `#!/bin/sh` script that runs `body` before the
    /// `-c` command it is given (the last argument).
    fn fake_shell(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-shell");
        fs::write(
            &path,
            format!("#!/bin/sh\n{body}\nfor last; do :; done\neval \"$last\"\n"),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn names_only_provider_variables_the_run_lacks() {
        let shell = names(&[
            "CLAUDE_CODE_USE_BEDROCK",
            "ANTHROPIC_MODEL",
            "AWS_REGION",
            "PATH",
            "EDITOR",
            "ANTHROPIC_MODEL",
        ]);
        let run = env(&[("AWS_REGION", "us-east-1"), ("PATH", "/usr/bin")]);
        assert_eq!(
            missing_provider_names(&shell, &run),
            names(&["ANTHROPIC_MODEL", "CLAUDE_CODE_USE_BEDROCK"])
        );
    }

    #[test]
    fn the_hint_lists_three_names_then_a_count() {
        assert_eq!(hint(&[]), None);
        assert_eq!(
            hint(&names(&["CLAUDE_CODE_USE_BEDROCK"])).unwrap(),
            format!(
                "Your login shell sets CLAUDE_CODE_USE_BEDROCK, which {}'s service does not see; add it to agent.env",
                crate::app_name!()
            )
        );
        assert!(
            hint(&names(&["A_1", "A_2"]))
                .unwrap()
                .starts_with("Your login shell sets A_1 and A_2, which")
        );
        assert!(
            hint(&names(&["A_1", "A_2", "A_3", "A_4", "A_5"]))
                .unwrap()
                .starts_with("Your login shell sets A_1, A_2, A_3 and 2 more, which")
        );
    }

    #[test]
    fn reads_names_from_the_shell_and_never_returns_values() {
        let dir = tempfile::tempdir().unwrap();
        // Start-up output before the marker is skipped; the shell's exports
        // and the run environment both appear.
        let shell = fake_shell(
            dir.path(),
            "echo 'Welcome back'\nexport CLAUDE_CODE_USE_BEDROCK=1\nexport ANTHROPIC_MODEL='secret value'",
        );
        let run = env(&[("PATH", "/usr/bin:/bin"), ("HOME", "/nonexistent")]);
        let found = names_from_shell(&shell, &run, Duration::from_secs(5)).unwrap();
        assert!(
            found.contains(&"CLAUDE_CODE_USE_BEDROCK".to_owned()),
            "{found:?}"
        );
        assert!(found.contains(&"ANTHROPIC_MODEL".to_owned()), "{found:?}");
        assert!(found.contains(&"PATH".to_owned()), "{found:?}");
        assert!(
            found.iter().all(|name| !name.contains("secret")),
            "{found:?}"
        );
        assert_eq!(
            missing_provider_names(&found, &run),
            names(&["ANTHROPIC_MODEL", "CLAUDE_CODE_USE_BEDROCK"])
        );
    }

    #[test]
    fn a_shell_that_hangs_is_killed_and_gives_no_names() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "exec sleep 30");
        let started = Instant::now();
        let found = names_from_shell(
            &shell,
            &env(&[("PATH", "/usr/bin:/bin")]),
            Duration::from_millis(300),
        );
        assert_eq!(found, None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_shell_that_prints_no_marker_gives_no_names() {
        assert!(parse_names(b"PATH=/bin\0HOME=/x\0").is_empty());
        let mut output = b"noise\0".to_vec();
        output.extend_from_slice(MARKER.as_bytes());
        output.extend_from_slice(b"\0GOOD=1\0bad name=2\0=3\0");
        assert_eq!(parse_names(&output), names(&["GOOD"]));
    }

    #[test]
    fn only_shells_in_etc_shells_are_used() {
        let allowed = "# List of acceptable shells\n/bin/bash\n/bin/zsh\n";
        assert!(listed_shell(allowed, Path::new("/bin/zsh")));
        assert!(!listed_shell(allowed, Path::new("/tmp/zsh")));
        assert!(!listed_shell(
            allowed,
            Path::new("# List of acceptable shells")
        ));
    }

    #[test]
    fn tests_never_start_the_real_login_shell() {
        let run = env(&[
            ("SHELL", "/bin/sh"),
            ("USER", "nobody"),
            ("PATH", "/usr/bin:/bin"),
        ]);
        assert_eq!(login_shell_names(&run), None);
    }
}

//! Test-only guard for tests that need local Unix-socket IPC.
//!
//! Some development sandboxes (Codex's `workspace-write` sandbox on Linux,
//! for one) refuse to bind or connect a Unix socket even in a temporary
//! directory, failing with EPERM or EACCES. Every test that binds or
//! connects a Unix socket, or starts a service that does, calls
//! [`ipc_denied`] first and returns early when it answers `true`:
//!
//! ```ignore
//! if crate::ipc_test_guard::ipc_denied() {
//!     return;
//! }
//! ```
//!
//! One probe per test process binds and connects a socket in a fresh
//! directory under `/tmp` and caches the result. A permission denial skips
//! the test with a one-line reason on stderr, unless
//! `BRAUSER_REQUIRE_IPC_TESTS=1` is set, which CI always sets: then the test
//! panics, so CI can never skip socket coverage. Any other probe error
//! panics too. `BRAUSER_TEST_SIMULATE_IPC_DENIED=1` makes the probe report a
//! denial without trying, to exercise both paths on a machine that allows
//! IPC. This module exists only in test and `scripted-dialogs` builds, so no
//! product build reads either variable.

use std::io::{self, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Set to `1` to make a denied probe fail the test instead of skipping it.
pub const REQUIRE_IPC_TESTS_ENV: &str = "BRAUSER_REQUIRE_IPC_TESTS";
/// Set to `1` to make the probe report a denial without binding anything.
pub const SIMULATE_IPC_DENIED_ENV: &str = "BRAUSER_TEST_SIMULATE_IPC_DENIED";

/// What the probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// A socket could be bound and connected.
    Allowed,
    /// Binding or connecting failed with EPERM or EACCES.
    Denied(String),
    /// Anything else went wrong; never a reason to skip.
    Failed(String),
}

/// What a guarded test does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Run,
    Skip(String),
    Fail(String),
}

/// Decide from a probe result and whether `BRAUSER_REQUIRE_IPC_TESTS=1`.
pub fn decide(probe: &Probe, require: bool) -> Decision {
    match probe {
        Probe::Allowed => Decision::Run,
        Probe::Denied(why) if require => Decision::Fail(format!(
            "local IPC is denied ({why}) but {REQUIRE_IPC_TESTS_ENV}=1 forbids skipping \
             socket tests; run them where Unix sockets are allowed"
        )),
        Probe::Denied(why) => Decision::Skip(format!(
            "skipping: local IPC denied by sandbox ({why}); set {REQUIRE_IPC_TESTS_ENV}=1 to fail instead"
        )),
        Probe::Failed(why) => Decision::Fail(format!("the local IPC probe failed: {why}")),
    }
}

/// Classify a socket error: only a permission denial may skip.
fn classify(step: &str, err: &io::Error) -> Probe {
    let why = format!("{step}: {err}");
    if err.kind() == io::ErrorKind::PermissionDenied {
        Probe::Denied(why)
    } else {
        Probe::Failed(why)
    }
}

fn flag(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

fn probe_now() -> Probe {
    if flag(SIMULATE_IPC_DENIED_ENV) {
        return Probe::Denied(format!("simulated by {SIMULATE_IPC_DENIED_ENV}=1"));
    }
    // Short and directly under /tmp, like the tests' own socket paths, so
    // `sun_path` length is never the reason a bind fails.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = PathBuf::from(format!("/tmp/bipc-{}-{nanos}", std::process::id()));
    if let Err(err) = std::fs::create_dir(&dir) {
        return Probe::Failed(format!("creating {}: {err}", dir.display()));
    }
    let socket = dir.join("p.sock");
    let result = match UnixListener::bind(&socket) {
        Err(err) => classify("bind", &err),
        Ok(_listener) => match UnixStream::connect(&socket) {
            Err(err) => classify("connect", &err),
            Ok(_) => Probe::Allowed,
        },
    };
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// The cached probe result for this test process.
pub fn probe() -> &'static Probe {
    static PROBE: OnceLock<Probe> = OnceLock::new();
    PROBE.get_or_init(probe_now)
}

/// `true` when the calling test should return early because local IPC is
/// denied. Panics when the denial must not be skipped or the probe failed.
pub fn ipc_denied() -> bool {
    match decide(probe(), flag(REQUIRE_IPC_TESTS_ENV)) {
        Decision::Run => false,
        Decision::Skip(why) => {
            let name = std::thread::current()
                .name()
                .unwrap_or("this test")
                .to_owned();
            // Straight to stderr, not `eprintln!`: the test harness
            // captures the print macros of a passing test, and a skip must
            // stay visible in the gate's output.
            let _ = writeln!(io::stderr(), "{name}: {why}");
            true
        }
        Decision::Fail(why) => panic!("{why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn denied() -> Probe {
        Probe::Denied("bind: Operation not permitted".into())
    }

    #[test]
    fn an_allowed_probe_runs_the_test() {
        assert_eq!(decide(&Probe::Allowed, false), Decision::Run);
        assert_eq!(decide(&Probe::Allowed, true), Decision::Run);
    }

    #[test]
    fn a_denial_skips_unless_required() {
        let Decision::Skip(why) = decide(&denied(), false) else {
            panic!("a denial without the flag must skip");
        };
        assert!(
            why.starts_with("skipping: local IPC denied by sandbox"),
            "{why}"
        );
        assert!(matches!(decide(&denied(), true), Decision::Fail(_)));
    }

    #[test]
    fn any_other_probe_error_fails() {
        let failed = Probe::Failed("bind: Address already in use".into());
        assert!(matches!(decide(&failed, false), Decision::Fail(_)));
        assert!(matches!(decide(&failed, true), Decision::Fail(_)));
    }

    #[test]
    fn only_permission_errors_count_as_denied() {
        for errno in [rustix::io::Errno::PERM, rustix::io::Errno::ACCESS] {
            let err = io::Error::from_raw_os_error(errno.raw_os_error());
            assert!(matches!(classify("bind", &err), Probe::Denied(_)), "{err}");
        }
        for errno in [
            rustix::io::Errno::ADDRINUSE,
            rustix::io::Errno::NAMETOOLONG,
            rustix::io::Errno::NOENT,
        ] {
            let err = io::Error::from_raw_os_error(errno.raw_os_error());
            assert!(matches!(classify("bind", &err), Probe::Failed(_)), "{err}");
        }
    }
}

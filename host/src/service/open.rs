//! `open_path` and `get_agent_env`: open a run's Codex thread, reveal
//! `agent.env` in Finder, and list the names `agent.env` sets. Values from
//! `agent.env` never leave the service.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use brauser_protocol::{ErrorCode, OpenTarget};
use uuid::Uuid;

use super::agent_env::AgentEnv;
use super::paths::ServicePaths;
use super::record::RunStore;
use crate::open::Opener;

/// The first line of an `agent.env` the service creates.
pub const AGENT_ENV_HEADER: &str =
    "# KEY=VALUE lines added to every Brauser agent run. Settings shows names, never values.\n";
/// The most names [`list_agent_env`] returns, and the longest one it shows.
const MAX_NAMES: usize = 64;
const MAX_NAME_BYTES: usize = 256;
const MAX_THREAD_ID_BYTES: usize = 128;

/// Open `target` with `opener`. `codex_thread` needs the run's id and opens
/// `codex://threads/<id>`; `agent_env` takes no run, creates the file (mode
/// 0600, the header only) when it is missing, and reveals it in Finder.
pub fn open_target(
    paths: &ServicePaths,
    store: &RunStore,
    target: OpenTarget,
    run_id: Option<Uuid>,
    opener: Opener,
) -> Result<(), (ErrorCode, String)> {
    let what = match target {
        OpenTarget::CodexThread => "the Codex thread",
        OpenTarget::AgentEnv => "agent.env",
    };
    let args = match target {
        OpenTarget::CodexThread => {
            let Some(run_id) = run_id else {
                return Err(invalid("run_id is required to open a Codex thread"));
            };
            let run = store
                .status(run_id)
                .map_err(|_| invalid("There is no such run."))?;
            let Some(thread) = run.thread_id else {
                return Err((
                    ErrorCode::NotConfigured,
                    "This run has no Codex thread to open.".to_owned(),
                ));
            };
            // The id came from the harness; only a plain token goes into a URL.
            if !is_thread_id(&thread) {
                return Err(invalid("The run's Codex thread id is not valid."));
            }
            vec![OsString::from(format!("codex://threads/{thread}"))]
        }
        OpenTarget::AgentEnv => {
            if run_id.is_some() {
                return Err(invalid("run_id must be null to reveal agent.env"));
            }
            let path = paths.agent_env();
            create_agent_env(&path).map_err(|error| {
                (
                    ErrorCode::Internal,
                    format!("Could not create agent.env: {error}"),
                )
            })?;
            vec![OsString::from("-R"), path.into_os_string()]
        }
    };
    opener(&args).map_err(|error| match error.kind() {
        ErrorKind::Unsupported => invalid("Opening is only supported on macOS."),
        _ => (
            ErrorCode::Internal,
            format!("Could not open {what}: {error}"),
        ),
    })
}

/// The names `agent.env` sets, sorted, and why the file cannot be used or
/// why not every name is listed. Never a value.
pub fn list_agent_env(paths: &ServicePaths) -> (Vec<String>, Option<String>) {
    match AgentEnv::read(&paths.agent_env()) {
        Ok(env) => {
            let all = env.names();
            let total = all.len();
            let names: Vec<String> = all
                .into_iter()
                .filter(|name| name.len() <= MAX_NAME_BYTES)
                .take(MAX_NAMES)
                .collect();
            let problem = (names.len() < total)
                .then(|| format!("agent.env sets {total} names; {} are listed.", names.len()));
            (names, problem)
        }
        // The error names a mode or a line number, never a value.
        Err(error) => (Vec::new(), Some(error.to_string())),
    }
}

/// `^[A-Za-z0-9_-]{1,128}$`.
fn is_thread_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_THREAD_ID_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Create `agent.env` with mode 0600 holding only the header. An existing
/// path, whatever it is, is left alone: revealing it is how the user fixes it.
fn create_agent_env(path: &Path) -> io::Result<()> {
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error),
    };
    // The umask may have narrowed the mode further; agent.env must be exact.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(AGENT_ENV_HEADER.as_bytes())?;
    file.sync_all()
}

fn invalid(message: &str) -> (ErrorCode, String) {
    (ErrorCode::InvalidRequest, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    use brauser_protocol::{HarnessId, RecordKey, RecordKeyKind, RunState};

    use crate::service::record::RunStatusFile;

    thread_local! {
        static OPENED: RefCell<Vec<Vec<OsString>>> = const { RefCell::new(Vec::new()) };
    }

    /// Records each call on this test's thread.
    fn recording(args: &[OsString]) -> io::Result<()> {
        OPENED.with(|opened| opened.borrow_mut().push(args.to_vec()));
        Ok(())
    }

    fn opened() -> Vec<Vec<String>> {
        OPENED.with(|opened| {
            opened
                .borrow()
                .iter()
                .map(|args| {
                    args.iter()
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect()
                })
                .collect()
        })
    }

    struct World {
        _temp: tempfile::TempDir,
        support: PathBuf,
        paths: ServicePaths,
        store: RunStore,
    }

    fn world() -> World {
        let temp = tempfile::tempdir().unwrap();
        let support = temp.path().join("support");
        let paths = ServicePaths::at(support.clone());
        paths.ensure_private().unwrap();
        let store = RunStore::open(paths.runs()).unwrap();
        World {
            _temp: temp,
            support,
            paths,
            store,
        }
    }

    impl World {
        /// A finished Codex run with `thread_id`.
        fn run(&self, thread_id: Option<&str>) -> Uuid {
            let id = Uuid::new_v4();
            let status = RunStatusFile {
                version: 1,
                run_id: id.to_string(),
                state: RunState::Succeeded,
                harness: HarnessId::Codex,
                service_pid: None,
                process_group: None,
                heartbeat_at: None,
                thread_id: thread_id.map(str::to_owned),
                session_id: None,
                cwd: None,
                start_key: RecordKey {
                    kind: RecordKeyKind::Url,
                    value: "https://example.com/a".into(),
                },
                written_key: None,
                url: "https://example.com/a".into(),
                title: "A".into(),
                requested_at: "2026-10-26T10:00:00Z".into(),
                created_at: "2026-10-26T10:00:00Z".into(),
                started_at: None,
                finished_at: None,
                error: None,
                error_kind: None,
                summary_path: None,
                stream_truncated: false,
                run_timeout_secs: None,
                vocabulary: None,
                answered_at: None,
            };
            self.store.create(&status, "page").unwrap();
            id
        }

        fn open(
            &self,
            target: OpenTarget,
            run_id: Option<Uuid>,
        ) -> Result<(), (ErrorCode, String)> {
            open_target(&self.paths, &self.store, target, run_id, recording)
        }

        fn write_agent_env(&self, contents: &str, mode: u32) {
            let path = self.paths.agent_env();
            std::fs::write(&path, contents).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    #[test]
    fn codex_thread_opens_the_deep_link() {
        let world = world();
        let id = world.run(Some("019a-thread_ID-1"));
        world.open(OpenTarget::CodexThread, Some(id)).unwrap();
        assert_eq!(opened(), vec![vec!["codex://threads/019a-thread_ID-1"]]);
    }

    #[test]
    fn run_without_thread_is_not_opened() {
        let world = world();
        let id = world.run(None);
        let (code, message) = world.open(OpenTarget::CodexThread, Some(id)).unwrap_err();
        assert_eq!(code, ErrorCode::NotConfigured);
        assert!(message.contains("no Codex thread"), "{message}");
        // No run id, or a run that does not exist, is a bad request.
        let (code, _) = world.open(OpenTarget::CodexThread, None).unwrap_err();
        assert_eq!(code, ErrorCode::InvalidRequest);
        let (code, _) = world
            .open(OpenTarget::CodexThread, Some(Uuid::new_v4()))
            .unwrap_err();
        assert_eq!(code, ErrorCode::InvalidRequest);
        assert!(opened().is_empty());
    }

    #[test]
    fn malformed_thread_id_is_refused() {
        let world = world();
        let long = "a".repeat(129);
        for bad in [
            "",
            "a/b",
            "../x",
            "t?x=1",
            "t id",
            "t\nid",
            "té",
            long.as_str(),
        ] {
            let id = world.run(Some(bad));
            let (code, _) = world.open(OpenTarget::CodexThread, Some(id)).unwrap_err();
            assert_eq!(code, ErrorCode::InvalidRequest, "{bad:?}");
        }
        let longest = world.run(Some(&"a".repeat(128)));
        world.open(OpenTarget::CodexThread, Some(longest)).unwrap();
        assert_eq!(opened().len(), 1);
    }

    #[test]
    fn reveal_creates_a_private_agent_env_and_selects_it() {
        let world = world();
        world.open(OpenTarget::AgentEnv, None).unwrap();
        let path = world.support.join("agent.env");
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(metadata.is_file());
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), AGENT_ENV_HEADER);
        // The header alone is an empty overlay.
        assert!(AgentEnv::read(&path).unwrap().names().is_empty());
        let path_text = path.to_string_lossy().into_owned();
        assert_eq!(opened(), vec![vec!["-R".to_owned(), path_text.clone()]]);

        // An existing file is revealed as it is, never rewritten.
        world.write_agent_env("FOO=bar\n", 0o644);
        world.open(OpenTarget::AgentEnv, None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "FOO=bar\n");
        assert_eq!(opened()[1], vec!["-R".to_owned(), path_text]);

        // agent_env takes no run.
        let (code, _) = world
            .open(OpenTarget::AgentEnv, Some(Uuid::new_v4()))
            .unwrap_err();
        assert_eq!(code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn opener_failure_is_reported() {
        fn failing(_: &[OsString]) -> io::Result<()> {
            Err(io::Error::other("no opener"))
        }
        fn unsupported(_: &[OsString]) -> io::Result<()> {
            Err(io::Error::new(ErrorKind::Unsupported, "unsupported"))
        }
        let world = world();
        let id = world.run(Some("t-1"));
        let (code, message) = open_target(
            &world.paths,
            &world.store,
            OpenTarget::CodexThread,
            Some(id),
            failing,
        )
        .unwrap_err();
        assert_eq!(code, ErrorCode::Internal);
        assert!(message.contains("no opener"), "{message}");
        let (code, _) = open_target(
            &world.paths,
            &world.store,
            OpenTarget::AgentEnv,
            None,
            unsupported,
        )
        .unwrap_err();
        assert_eq!(code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn agent_env_listing_shows_names_never_values() {
        let world = world();
        assert_eq!(list_agent_env(&world.paths), (Vec::new(), None));
        world.write_agent_env("ZED=secret-z\n# comment\nALPHA=secret-a\n", 0o600);
        let (names, problem) = list_agent_env(&world.paths);
        assert_eq!(names, vec!["ALPHA", "ZED"]);
        assert_eq!(problem, None);
        let shown = format!("{names:?}");
        assert!(!shown.contains("secret"), "{shown}");

        // More names than the wire carries: the first 64, and a note.
        let many: String = (0..70).map(|n| format!("K{n:02}=v\n")).collect();
        world.write_agent_env(&many, 0o600);
        let (names, problem) = list_agent_env(&world.paths);
        assert_eq!(names.len(), 64);
        assert_eq!(names[0], "K00");
        assert!(problem.unwrap().contains("70"));
    }

    #[test]
    fn bad_agent_env_mode_is_a_problem_not_an_error() {
        let world = world();
        world.write_agent_env("TOKEN=secret-value\n", 0o644);
        let (names, problem) = list_agent_env(&world.paths);
        assert!(names.is_empty());
        let problem = problem.unwrap();
        assert!(problem.contains("0600"), "{problem}");
        assert!(!problem.contains("secret"), "{problem}");
        // A bad line names its number, never its text.
        world.write_agent_env("TOKEN=secret-value\nsecret-line\n", 0o600);
        let problem = list_agent_env(&world.paths).1.unwrap();
        assert!(problem.contains("line 2"), "{problem}");
        assert!(!problem.contains("secret"), "{problem}");
    }

    #[test]
    fn reveal_refuses_a_support_dir_it_cannot_write() {
        let world = world();
        let paths = ServicePaths::at(world.support.join("missing").join("support"));
        let (code, _) =
            open_target(&paths, &world.store, OpenTarget::AgentEnv, None, recording).unwrap_err();
        assert_eq!(code, ErrorCode::Internal);
        assert!(opened().is_empty());
    }
}

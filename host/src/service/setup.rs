//! Harness setup in the service (spec §6.1): detect `codex` and `claude` on
//! the service's own `PATH`, then run one fixed probe prompt through the
//! harness's real invocation, in the knowledge base folder with the service
//! environment plus `agent.env`, and commit the harness only when the probe
//! answers. There is no native dialog and no token: the extension names a
//! harness, never a path or arguments.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use brauser_protocol::{
    AgentConfig, ConfigSnapshot, ErrorCode, HarnessFound, SetUpHarnessRequest, StorageConfig,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use super::harness_api::{HarnessRegistry, HarnessRunner, ProcessSlot, RunSpec, TurnOutcome};
use super::server::checked_knowledge_base;
use crate::config::{
    self, ConfigStore, DEFAULT_SUMMARIES_DIR, DEFAULT_SUMMARY_TIMEOUT_SECS, HarnessCommit,
    SUMMARY_TIMEOUT_SECS, StoredSummarizer,
};
use crate::harness::{self, Adapter, Detected, HarnessEnv, PROBE_PROMPT};

/// How long the probe may take, including a harness's own startup.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_PROBLEM_BYTES: usize = 512;
const MAX_BINARY_BYTES: usize = 4096;
const UNSAFE_PATH: &str = "the harness path cannot be shown safely; move it to a plain path";
const CHANGED: &str = "configuration changed; read it again before setting up a summary harness";
/// The probe's page: there is none.
const NO_PAGE: &str = "/dev/null";

/// Both supported harnesses, Codex first, as the service finds them with
/// `env`'s search path and `run_env`.
pub fn discover(env: &HarnessEnv, run_env: &[(OsString, OsString)]) -> Vec<HarnessFound> {
    harness::detect(env, run_env)
        .into_iter()
        .map(|detected| {
            let (binary, problem) = shown(&detected);
            HarnessFound {
                harness: detected.adapter.into(),
                binary,
                version: detected.version,
                problem: problem.map(|text| bounded(text, MAX_PROBLEM_BYTES)),
            }
        })
        .collect()
}

/// The binary as settings may show it, and the problem to report: a path
/// that is not plain text cannot be set up.
fn shown(detected: &Detected) -> (Option<String>, Option<String>) {
    let Some(path) = &detected.binary else {
        return (None, detected.problem.clone());
    };
    match path.to_str() {
        Some(text) if !config::has_unsafe_display_chars(text) && text.len() <= MAX_BINARY_BYTES => {
            (Some(text.to_owned()), detected.problem.clone())
        }
        _ => {
            let escaped = path.to_string_lossy().escape_debug().to_string();
            (
                Some(bounded(escaped, MAX_BINARY_BYTES)),
                Some(
                    detected
                        .problem
                        .clone()
                        .unwrap_or_else(|| UNSAFE_PATH.to_owned()),
                ),
            )
        }
    }
}

/// Send [`PROBE_PROMPT`] through `runner` as a run would, and require the
/// answer OK (see [`answer_is_ok`]). The run is killed at `timeout`.
pub fn probe(runner: &dyn HarnessRunner, spec: &RunSpec, timeout: Duration) -> Result<(), String> {
    let slot = ProcessSlot::default();
    let (finished, deadline) = mpsc::channel::<()>();
    let timer = {
        let slot = slot.clone();
        thread::spawn(move || {
            let expired = deadline.recv_timeout(timeout) == Err(mpsc::RecvTimeoutError::Timeout);
            if expired {
                slot.kill();
            }
            expired
        })
    };
    let outcome = match runner.start(spec, &slot) {
        Ok(mut session) => {
            let outcome = session.turn(&spec.prompt, &mut |_| {});
            session.close();
            Ok(outcome)
        }
        Err(error) => Err(format!("{error:#}")),
    };
    drop(finished);
    if timer.join().unwrap_or(false) {
        return Err(format!("timed out after {} seconds", timeout.as_secs()));
    }
    match outcome? {
        TurnOutcome::Answered { final_text } if answer_is_ok(&final_text) => Ok(()),
        TurnOutcome::Answered { .. } => Err("the answer was not OK".to_owned()),
        TurnOutcome::Failed { reason } => Err(reason),
    }
}

/// Whether `answer` is the word OK, ignoring case, surrounding whitespace,
/// and surrounding punctuation or quotes ("OK.", "\"ok\"", "**OK**").
/// Words that merely contain "ok" ("token", "broken") are not OK.
fn answer_is_ok(answer: &str) -> bool {
    answer
        .trim()
        .trim_matches(|c: char| {
            c.is_ascii_punctuation()
                || c.is_whitespace()
                || "\u{2018}\u{2019}\u{201c}\u{201d}".contains(c)
        })
        .eq_ignore_ascii_case("ok")
}

/// Set up `request.harness` and return the committed configuration and its
/// revision. Detection and the probe run with no lock held; the commit then
/// takes the config lock and checks `expected_revision` again, so a save
/// during the probe is a conflict. `env` is the run environment: the
/// service's own overlaid with `agent.env`.
pub fn set_up(
    config: &mut ConfigStore,
    registry: &HarnessRegistry,
    request: &SetUpHarnessRequest,
    env: Vec<(OsString, OsString)>,
) -> Result<(ConfigSnapshot, String), (ErrorCode, String)> {
    let refuse = |code, message: &str| (code, message.to_owned());
    if config.refresh().is_err() {
        return Err(refuse(
            ErrorCode::InvalidConfig,
            "configuration file is unavailable",
        ));
    }
    // Checked now so a stale page does not wait out a probe; the commit
    // checks it again.
    if request.expected_revision != config.revision() {
        return Err(refuse(ErrorCode::Conflict, CHANGED));
    }
    // Setup rewrites the whole file from this snapshot, so it must be the
    // file's own content, not a stand-in for one that needs repair.
    if config.config_issue().is_some() {
        return Err(refuse(
            ErrorCode::InvalidConfig,
            "configuration needs repair before setting up a summary harness",
        ));
    }
    let current = config.snapshot().clone();
    let Some(storage) = &current.storage else {
        return Err(refuse(
            ErrorCode::NotConfigured,
            "Choose a notes folder first",
        ));
    };
    let timeout_secs = request.timeout_secs.unwrap_or(DEFAULT_SUMMARY_TIMEOUT_SECS);
    if !SUMMARY_TIMEOUT_SECS.contains(&timeout_secs) {
        return Err(refuse(
            ErrorCode::InvalidRequest,
            "timeout_secs must be 60 to 3600",
        ));
    }
    let knowledge_base =
        checked_knowledge_base(config).map_err(|(code, message)| refuse(code, message))?;

    let adapter = Adapter::from(request.harness);
    let name = adapter.display_name();
    let detected = harness::detect(&HarnessEnv::from_run_env(&env), &env)
        .into_iter()
        .find(|detected| detected.adapter == adapter)
        .ok_or_else(|| (ErrorCode::Internal, format!("{name} is not supported")))?;
    let binary = match shown(&detected) {
        (None, _) => {
            return Err((
                ErrorCode::InvalidConfig,
                format!("{name} was not found on the service's PATH"),
            ));
        }
        (Some(_), Some(problem)) => {
            return Err((
                ErrorCode::InvalidConfig,
                format!(
                    "{name} cannot be used: {}",
                    bounded(problem, MAX_PROBLEM_BYTES)
                ),
            ));
        }
        (Some(binary), None) => binary,
    };
    let agent = |probed_at: String| AgentConfig {
        harness: request.harness,
        binary: binary.clone(),
        version: detected.version.clone(),
        timeout_secs,
        probed_at,
    };
    let after = |agent: AgentConfig| ConfigSnapshot {
        storage: Some(StorageConfig {
            summaries_dir: Some(
                request
                    .summaries_dir
                    .clone()
                    .or_else(|| storage.summaries_dir.clone())
                    .unwrap_or_else(|| DEFAULT_SUMMARIES_DIR.to_owned()),
            ),
            ..storage.clone()
        }),
        agent: Some(agent),
        ..current.clone()
    };
    let now = || {
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| refuse(ErrorCode::Internal, "could not record the probe time"))
    };
    // Refuse a summaries folder that would not save before spending a probe.
    config::validate_update(&current, &after(agent(now()?)))
        .map_err(|error| (ErrorCode::InvalidConfig, error.to_string()))?;
    let runner = registry.get(Some(request.harness)).ok_or_else(|| {
        (
            ErrorCode::Internal,
            format!("the service cannot run {name}"),
        )
    })?;
    let spec = RunSpec {
        run_id: Uuid::new_v4(),
        harness: request.harness,
        title: concat!(crate::app_name!(), " setup test").to_owned(),
        cwd: knowledge_base,
        page_path: PathBuf::from(NO_PAGE),
        prompt: PROBE_PROMPT.to_owned(),
        env,
    };
    let probe_runner = runner.for_probe();
    probe(
        probe_runner.as_deref().unwrap_or(runner),
        &spec,
        PROBE_TIMEOUT,
    )
    .map_err(|reason| {
        (
            ErrorCode::InvalidConfig,
            format!(
                "{name} failed its test run ({}); summaries stay off",
                bounded(reason, MAX_PROBLEM_BYTES)
            ),
        )
    })?;

    let agent = agent(now()?);
    let commit = HarnessCommit {
        summarizer: StoredSummarizer::from(&agent),
    };
    match config.update_with_grants(
        after(agent),
        &request.expected_revision,
        None,
        None,
        Some(commit),
    ) {
        Ok(Some(revision)) => Ok((config.snapshot().clone(), revision)),
        Ok(None) => Err(refuse(ErrorCode::Conflict, CHANGED)),
        Err(error) => Err((
            ErrorCode::Internal,
            format!("could not save configuration: {error:#}"),
        )),
    }
}

/// Truncate host-authored text at a character boundary.
fn bounded(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Setup against fake harnesses: `#!/bin/sh` scripts in a temp dir found
/// through an injected `PATH`, and the fake runner. No real harness is ever
/// found or run.
#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Instant;

    use brauser_protocol::{AgentState, HarnessId, PROTOCOL_VERSION};

    use super::*;
    use crate::brand::FAKE_HARNESS_ENV;
    use crate::config::fixtures;
    use crate::harness::fake::Fake;

    struct Setup {
        fake: Fake,
        path: PathBuf,
        config: ConfigStore,
        /// The fake runner's script; it answers the probe.
        runner: PathBuf,
    }

    /// A notes folder, fake `codex` and `claude` that answer `--version`,
    /// and a fake runner script with `body`.
    fn setup(body: &str) -> Setup {
        let fake = Fake::new();
        fake.install("codex", "codex-cli 0.144.4", "exit 1");
        fake.claude("exit 1");
        let notes = fake.path("notes");
        fs::create_dir(&notes).unwrap();
        let path = fake.path("config.toml");
        fixtures::write(&path, &notes, "", "", "");
        let runner = fake.path("runner");
        fs::write(&runner, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&runner, fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = ConfigStore::for_test(path.clone());
        config.refresh().unwrap();
        Setup {
            fake,
            path,
            config,
            runner,
        }
    }

    impl Setup {
        /// The run environment: the fake bin directory on `PATH`.
        fn run_env(&self) -> Vec<(OsString, OsString)> {
            vec![
                ("HOME".into(), self.fake.path("home").into()),
                ("PATH".into(), self.fake.path("bin").into()),
                ("PROBE_MARK".into(), "probe-env".into()),
            ]
        }

        fn registry(&self) -> HarnessRegistry {
            let runner = self.runner.clone();
            HarnessRegistry::from_lookup(move |name| {
                (name == FAKE_HARNESS_ENV).then(|| runner.clone().into_os_string())
            })
        }

        fn run(
            &mut self,
            harness: HarnessId,
        ) -> Result<(ConfigSnapshot, String), (ErrorCode, String)> {
            self.config.refresh().unwrap();
            let request = SetUpHarnessRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "r1".into(),
                expected_revision: self.config.revision().to_owned(),
                harness,
                timeout_secs: None,
                summaries_dir: None,
            };
            let registry = self.registry();
            let env = self.run_env();
            set_up(&mut self.config, &registry, &request, env)
        }
    }

    fn found(harness: HarnessId, binary: &Path, version: &str) -> HarnessFound {
        HarnessFound {
            harness,
            binary: Some(binary.to_string_lossy().into_owned()),
            version: Some(version.to_owned()),
            problem: None,
        }
    }

    #[test]
    fn discover_reports_both_harnesses_with_versions_from_the_service_path() {
        let setup = setup("echo OK");
        let env = setup.run_env();
        let bin = setup.fake.path("bin");
        assert_eq!(
            discover(&HarnessEnv::from_run_env(&env), &env),
            [
                found(HarnessId::Codex, &bin.join("codex"), "0.144.4"),
                found(HarnessId::Claude, &bin.join("claude"), "2.1.284"),
            ]
        );
        // Only `--version` ran.
        assert_eq!(
            setup.fake.log("codex", "argv.log").as_deref(),
            Some("[--version]\n")
        );

        // A harness that is not on the service's PATH is still reported.
        fs::remove_file(bin.join("codex")).unwrap();
        let listed = discover(&HarnessEnv::from_run_env(&env), &env);
        assert_eq!(
            listed[0],
            HarnessFound {
                harness: HarnessId::Codex,
                binary: None,
                version: None,
                problem: None,
            }
        );
        assert_eq!(listed[1].harness, HarnessId::Claude);

        // A version that cannot be read is a problem, not a missing harness.
        setup.fake.install("claude", "Claude Code", "exit 1");
        let listed = discover(&HarnessEnv::from_run_env(&env), &env);
        assert_eq!(listed[1].version, None);
        assert_eq!(
            listed[1].problem.as_deref(),
            Some("claude --version output was not recognized")
        );
    }

    #[test]
    fn unreviewed_codex_version_is_not_refused() {
        let setup = setup("echo OK");
        setup.fake.install("codex", "codex-cli 0.130.0", "exit 1");
        let env = setup.run_env();
        let codex = &discover(&HarnessEnv::from_run_env(&env), &env)[0];
        assert_eq!(codex.version.as_deref(), Some("0.130.0"));
        assert_eq!(codex.problem, None);
    }

    #[test]
    fn passing_probe_commits_agent_and_reports_ready() {
        let mut setup = setup(
            r#"/bin/cat > "$0.log"; pwd -P >> "$0.log"; echo "$1 $PROBE_MARK" >> "$0.log"; echo ' OK.'"#,
        );
        let before = setup.config.revision().to_owned();
        let (config, revision) = setup.run(HarnessId::Codex).unwrap();
        let agent = config.agent.clone().unwrap();
        assert_eq!(agent.harness, HarnessId::Codex);
        assert_eq!(
            agent.binary,
            setup.fake.path("bin").join("codex").to_string_lossy()
        );
        assert_eq!(agent.version.as_deref(), Some("0.144.4"));
        assert_eq!(agent.timeout_secs, 600);
        OffsetDateTime::parse(&agent.probed_at, &Rfc3339).unwrap();
        assert_eq!(
            config.storage.as_ref().unwrap().summaries_dir.as_deref(),
            Some("summaries")
        );
        assert_ne!(revision, before);
        // The probe ran the fixed prompt in the knowledge base folder (the
        // notes folder here), with no page and the run environment.
        let log = fs::read_to_string(setup.runner.with_extension("log")).unwrap();
        let notes = fs::canonicalize(setup.fake.path("notes")).unwrap();
        assert_eq!(
            log,
            format!("{PROBE_PROMPT}\n{}\n/dev/null probe-env\n", notes.display())
        );

        let mut reloaded = ConfigStore::for_test(setup.path.clone());
        reloaded.refresh().unwrap();
        assert_eq!(reloaded.revision(), revision);
        assert_eq!(reloaded.snapshot().agent, Some(agent));
        let status = reloaded.agent_status();
        assert_eq!(status.state, AgentState::Ready);
        assert_eq!(status.harness_version.as_deref(), Some("0.144.4"));
        assert!(
            fs::read_to_string(&setup.path)
                .unwrap()
                .contains("[summarizer]")
        );
    }

    #[test]
    fn failing_probe_leaves_config_unchanged() {
        for (body, reason) in [
            (
                "echo 'Invalid API key' >&2; exit 3",
                "harness exited with status 3: Invalid API key",
            ),
            ("echo 'Sorry.'", "the answer was not OK"),
            // Words that merely contain "ok" are not OK.
            ("echo 'token'", "the answer was not OK"),
            ("echo 'Look, it is broken'", "the answer was not OK"),
        ] {
            let mut setup = setup(body);
            let before = fs::read(&setup.path).unwrap();
            let (code, message) = setup.run(HarnessId::Claude).unwrap_err();
            assert_eq!(code, ErrorCode::InvalidConfig);
            assert_eq!(
                message,
                format!("Claude Code failed its test run ({reason}); summaries stay off")
            );
            assert_eq!(fs::read(&setup.path).unwrap(), before);
            assert_eq!(setup.config.agent_status().state, AgentState::NotSetUp);
        }
    }

    #[test]
    fn probe_answer_must_be_exactly_ok() {
        for answer in ["OK", "ok", " OK.\n", "\"OK\"", "'Ok!'", "**OK**", "`OK`"] {
            assert!(answer_is_ok(answer), "{answer:?}");
        }
        for answer in [
            "", "token", "look", "broken", "OK then", "not ok", "O K", "...",
        ] {
            assert!(!answer_is_ok(answer), "{answer:?}");
        }
        let mut setup = setup("echo '\"OK.\"'");
        setup.run(HarnessId::Claude).unwrap();
    }

    #[test]
    fn missing_or_unusable_harness_is_refused_before_any_probe() {
        let mut setup = setup("echo OK > \"$0.probe\"");
        fs::remove_file(setup.fake.path("bin").join("codex")).unwrap();
        assert_eq!(
            setup.run(HarnessId::Codex).unwrap_err(),
            (
                ErrorCode::InvalidConfig,
                "Codex was not found on the service's PATH".to_owned()
            )
        );
        setup.fake.install("claude", "unknown", "exit 1");
        assert_eq!(
            setup.run(HarnessId::Claude).unwrap_err(),
            (
                ErrorCode::InvalidConfig,
                "Claude Code cannot be used: claude --version output was not recognized".to_owned()
            )
        );
        assert!(!setup.runner.with_extension("probe").exists());
    }

    #[test]
    fn stale_revision_after_probe_is_a_conflict() {
        // The probe saves another change to config.toml while it runs.
        let mut setup = setup("");
        fs::write(
            &setup.runner,
            format!(
                "#!/bin/sh\nprintf '\\n# edited during the probe\\n' >> '{}'\necho OK\n",
                setup.path.display()
            ),
        )
        .unwrap();
        let (code, _) = setup.run(HarnessId::Claude).unwrap_err();
        assert_eq!(code, ErrorCode::Conflict);
        let text = fs::read_to_string(&setup.path).unwrap();
        assert!(text.ends_with("# edited during the probe\n"), "{text}");
        assert!(!text.contains("[summarizer"), "{text}");

        // A request already stale never probes.
        let request = SetUpHarnessRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "r2".into(),
            expected_revision: "stale".into(),
            harness: HarnessId::Claude,
            timeout_secs: None,
            summaries_dir: None,
        };
        fs::write(&setup.runner, "#!/bin/sh\n: > \"$0.ran\"\necho OK\n").unwrap();
        let registry = setup.registry();
        let env = setup.run_env();
        let (code, _) = set_up(&mut setup.config, &registry, &request, env).unwrap_err();
        assert_eq!(code, ErrorCode::Conflict);
        assert!(!setup.runner.with_extension("ran").exists());
    }

    #[test]
    fn timeout_outside_the_bounds_is_refused() {
        let mut setup = setup("echo OK");
        for timeout_secs in [59, 3601] {
            let request = SetUpHarnessRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "r1".into(),
                expected_revision: setup.config.revision().to_owned(),
                harness: HarnessId::Claude,
                timeout_secs: Some(timeout_secs),
                summaries_dir: None,
            };
            let registry = setup.registry();
            let env = setup.run_env();
            let (code, _) = set_up(&mut setup.config, &registry, &request, env).unwrap_err();
            assert_eq!(code, ErrorCode::InvalidRequest, "{timeout_secs}");
        }
    }

    #[test]
    fn probe_is_killed_at_its_timeout() {
        let setup = setup("exec /bin/sleep 30");
        let registry = setup.registry();
        let spec = RunSpec {
            run_id: Uuid::new_v4(),
            harness: HarnessId::Claude,
            title: "t".into(),
            cwd: setup.fake.path("notes"),
            page_path: PathBuf::from(NO_PAGE),
            prompt: PROBE_PROMPT.into(),
            env: setup.run_env(),
        };
        let started = Instant::now();
        let error = probe(
            registry.get(Some(HarnessId::Claude)).unwrap(),
            &spec,
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert_eq!(error, "timed out after 2 seconds");
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}

//! The per-user Brauser service that owns agent runs. Everything here is
//! plain POSIX and builds on any Unix; only starting it through launchd
//! (`client::launchd_kickstart`) is macOS-only. Windows has no service.

pub mod agent_env;
pub mod child;
pub mod claude;
pub mod client;
pub mod codex;
pub mod command_runner;
pub mod finisher;
pub mod harness_api;
pub mod maintenance;
pub mod open;
pub mod paths;
pub mod prompt;
pub mod queue;
pub mod record;
pub mod server;
pub mod setup;
pub mod supervisor;

use std::time::Duration;

use anyhow::{Result, bail};

use crate::brand::NAMESPACE;
use crate::config::ConfigStore;
use finisher::{KindFinisher, SummaryFinisher};
use harness_api::HarnessRegistry;
use paths::ServicePaths;
use record::RunStore;
use server::ServeExit;
use supervisor::{DEFAULT_RUN_TIMEOUT, ServiceOptions, Supervisor};

/// The `brauser service` subcommand (launchd runs it on macOS).
pub const COMMAND: &str = "service";
/// How long the service stays up with nothing to do.
pub const DEFAULT_IDLE_EXIT: Duration = Duration::from_secs(600);

/// Run the service until it is idle. A second instance exits with status 0.
pub fn run(mut args: impl Iterator<Item = String>) -> Result<()> {
    if let Some(extra) = args.next() {
        bail!("unexpected argument to {COMMAND}: {extra}");
    }
    let config_path = ConfigStore::default_path()?;
    let config = ConfigStore::load()?;
    let paths = ServicePaths::for_config(&config)?;
    let options = options(&paths, &config);
    let exit = server::serve(&paths, config, &options, || {
        let store = RunStore::open(paths.runs())?;
        // It reads config.toml again at each save, so a summary follows
        // the configuration current when its run finishes. A site-draft run
        // goes to its own finisher, which writes nothing.
        let finisher = KindFinisher::new(SummaryFinisher::new(config_path.clone()));
        Ok(Supervisor::new(
            store,
            HarnessRegistry::from_environment(),
            Box::new(finisher),
            options.clone(),
        ))
    })?;
    if exit == ServeExit::AlreadyRunning {
        eprintln!("{NAMESPACE} service: another service process is already running");
    }
    Ok(())
}

fn options(paths: &ServicePaths, config: &ConfigStore) -> ServiceOptions {
    #[allow(unused_mut)]
    let mut options = ServiceOptions {
        run_timeout: DEFAULT_RUN_TIMEOUT,
        forced_run_timeout: None,
        idle_exit: DEFAULT_IDLE_EXIT,
        service_pid: std::process::id(),
        run_env: server::env_source(paths),
        run_cwd: server::cwd_check(config.reopen()),
        opener: crate::open::default_opener(),
    };
    #[cfg(feature = "scripted-dialogs")]
    {
        use crate::brand::{RUN_TIMEOUT_SECS_ENV, SERVICE_IDLE_SECS_ENV};
        if let Some(idle) = seconds_from_env(SERVICE_IDLE_SECS_ENV) {
            options.idle_exit = idle;
        }
        // It also beats a run's own `timeout_secs`.
        if let Some(timeout) = seconds_from_env(RUN_TIMEOUT_SECS_ENV) {
            options.run_timeout = timeout;
            options.forced_run_timeout = Some(timeout);
        }
    }
    options
}

/// A positive, finite number of seconds (fractions allowed) from `name`.
#[cfg(feature = "scripted-dialogs")]
fn seconds_from_env(name: &str) -> Option<Duration> {
    parse_seconds(&std::env::var(name).ok()?)
}

#[cfg(any(test, feature = "scripted-dialogs"))]
fn parse_seconds(text: &str) -> Option<Duration> {
    let seconds: f64 = text.trim().parse().ok()?;
    (seconds.is_finite() && seconds > 0.0).then(|| Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seam_seconds_are_positive_and_finite() {
        assert_eq!(parse_seconds("2"), Some(Duration::from_secs(2)));
        assert_eq!(parse_seconds(" 0.5 "), Some(Duration::from_millis(500)));
        for bad in ["0", "-1", "inf", "NaN", "", "soon"] {
            assert_eq!(parse_seconds(bad), None, "{bad}");
        }
    }
}

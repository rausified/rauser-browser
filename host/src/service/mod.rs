//! The per-user Brauser service that owns agent runs (macOS only).

pub mod agent_env;
pub mod client;
pub mod command_runner;
pub mod harness_api;
pub mod maintenance;
pub mod paths;
pub mod queue;
pub mod record;
pub mod server;
pub mod supervisor;

use std::time::Duration;

use anyhow::{Result, bail};

use crate::brand::NAMESPACE;
use crate::config::ConfigStore;
use harness_api::HarnessRegistry;
use paths::ServicePaths;
use record::RunStore;
use server::ServeExit;
use supervisor::{DEFAULT_RUN_TIMEOUT, ServiceOptions, Supervisor, VaultCheckFinisher};

/// The `brauser service` subcommand launchd runs.
pub const COMMAND: &str = "service";
/// How long the service stays up with nothing to do.
pub const DEFAULT_IDLE_EXIT: Duration = Duration::from_secs(600);

/// Run the service until it is idle. A second instance exits with status 0.
pub fn run(mut args: impl Iterator<Item = String>) -> Result<()> {
    if let Some(extra) = args.next() {
        bail!("unexpected argument to {COMMAND}: {extra}");
    }
    let config = ConfigStore::load()?;
    let paths = ServicePaths::for_config(&config)?;
    let options = options(&paths);
    let exit = server::serve(&paths, config, &options, || {
        let store = RunStore::open(paths.runs())?;
        let finisher = VaultCheckFinisher::new(ConfigStore::load()?);
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

fn options(paths: &ServicePaths) -> ServiceOptions {
    // Queued runs resumed after a restart get the same environment a new
    // run would; a bad agent.env falls back to the service environment and
    // each new start_run reports the problem.
    let resume_env = server::run_env(paths).unwrap_or_else(|error| {
        eprintln!("{NAMESPACE} service: {error}");
        std::env::vars_os().collect()
    });
    #[allow(unused_mut)]
    let mut options = ServiceOptions {
        run_timeout: DEFAULT_RUN_TIMEOUT,
        idle_exit: DEFAULT_IDLE_EXIT,
        service_pid: std::process::id(),
        resume_env,
    };
    #[cfg(feature = "scripted-dialogs")]
    {
        use crate::brand::{RUN_TIMEOUT_SECS_ENV, SERVICE_IDLE_SECS_ENV};
        if let Some(idle) = seconds_from_env(SERVICE_IDLE_SECS_ENV) {
            options.idle_exit = idle;
        }
        if let Some(timeout) = seconds_from_env(RUN_TIMEOUT_SECS_ENV) {
            options.run_timeout = timeout;
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

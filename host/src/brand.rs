//! Product naming for the host. The extension's copy is `extension/brand.ts`.

/// Lowercase namespace for markers, frontmatter keys, temp files, and the binary.
/// A macro so `concat!` can build compile-time marker strings from it.
#[macro_export]
macro_rules! namespace {
    () => {
        "brauser"
    };
}

/// The user-facing product name, as a macro for compile-time messages.
#[macro_export]
macro_rules! app_name {
    () => {
        "Brauser"
    };
}

pub const APP_NAME: &str = app_name!();
pub const NAMESPACE: &str = namespace!();
pub const BLOCK_START: &str = concat!("<!-- ", namespace!(), ":start -->");
pub const BLOCK_END: &str = concat!("<!-- ", namespace!(), ":end -->");
pub const FRONTMATTER_KEY: &str = concat!(namespace!(), ":");
/// Names the scripted-answer directory in `scripted-dialogs` test builds.
#[cfg(feature = "scripted-dialogs")]
pub const SCRIPTED_DIALOGS_ENV: &str = "BRAUSER_SCRIPTED_DIALOGS";

/// The launchd label of the per-user Brauser service. The extension's copy is
/// `SERVICE_LABEL` in `extension/brand.ts`.
pub const SERVICE_LABEL: &str = concat!("com.rauser.", namespace!(), ".service");
/// File names inside the service's support directory.
pub const SOCKET_NAME: &str = "service.sock";
pub const AGENT_ENV_NAME: &str = "agent.env";
pub const RUNS_DIR: &str = "runs";

/// Test-only overrides for the service; never read by a release build.
#[cfg(any(test, feature = "scripted-dialogs"))]
pub const FAKE_HARNESS_ENV: &str = "BRAUSER_FAKE_HARNESS";
#[cfg(any(test, feature = "scripted-dialogs"))]
pub const SERVICE_SPAWN_ENV: &str = "BRAUSER_SERVICE_SPAWN";
#[cfg(any(test, feature = "scripted-dialogs"))]
pub const SERVICE_IDLE_SECS_ENV: &str = "BRAUSER_SERVICE_IDLE_SECS";
#[cfg(any(test, feature = "scripted-dialogs"))]
pub const RUN_TIMEOUT_SECS_ENV: &str = "BRAUSER_RUN_TIMEOUT_SECS";

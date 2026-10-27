#![forbid(unsafe_code)]

// Scripted dialogs replace the user's answer, so a release host must never
// contain them. Enabling the feature in an optimized build fails to compile.
#[cfg(all(feature = "scripted-dialogs", not(debug_assertions)))]
compile_error!("the scripted-dialogs feature is only for debug test builds");

/// This host build, `"<version>+<commit>"`, the commit marked `-dirty` for
/// changed host sources (`build_id.rs`, stamped by `build.rs`). Hello reports
/// it as `host_version`, and the service's `build_id()` starts with it, so
/// the page headers and the service staleness check name one build.
pub const BUILD_LABEL: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("BRAUSER_BUILD_ID"));

pub mod adapters;
// Only the service, which is Unix-only, reads answers and writes summaries.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) mod answer;
pub mod brand;
#[cfg(test)]
mod build_id;
pub mod capture;
pub mod config;
pub mod consent;
pub mod dialog;
pub mod frame;
pub mod harness;
pub mod index;
// Tests that need Unix sockets skip under a sandbox that denies them, unless
// BRAUSER_REQUIRE_IPC_TESTS=1 (CI). Test and scripted-dialogs builds only.
#[cfg(all(unix, any(test, feature = "scripted-dialogs")))]
#[doc(hidden)]
pub mod ipc_test_guard;
mod layout;
pub(crate) mod markdown;
#[cfg(test)]
mod markdown_corpus;
pub mod native;
pub mod note;
pub(crate) mod open;
mod presentation;
// The service is plain POSIX; only its launchd glue is macOS-only.
#[cfg(unix)]
pub mod service;
mod summary;
pub mod vault;

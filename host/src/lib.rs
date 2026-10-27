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
pub mod brand;
#[cfg(test)]
mod build_id;
pub mod capture;
pub mod config;
pub mod consent;
pub mod dialog;
pub mod frame;
pub mod harness;
pub mod harness_setup;
pub mod index;
mod layout;
pub mod native;
pub mod note;
mod presentation;
// The service is plain POSIX; only its launchd glue is macOS-only.
#[cfg(unix)]
pub mod service;
mod summary;
pub mod vault;

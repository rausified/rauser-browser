#![forbid(unsafe_code)]

// Scripted dialogs replace the user's answer, so a release host must never
// contain them. Enabling the feature in an optimized build fails to compile.
#[cfg(all(feature = "scripted-dialogs", not(debug_assertions)))]
compile_error!("the scripted-dialogs feature is only for debug test builds");

pub mod adapters;
pub mod brand;
pub mod capture;
pub mod config;
pub mod consent;
pub mod dialog;
pub mod frame;
pub mod harness;
pub mod harness_setup;
pub mod index;
pub mod native;
pub mod note;
mod presentation;
#[cfg(target_os = "macos")]
pub mod service;
mod summary;
pub mod vault;

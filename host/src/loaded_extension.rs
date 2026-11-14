//! The extension build Chrome last said hello with, for
//! `scripts/dev/deploy-live`, which cannot see inside Chrome (brwr-x622).
//!
//! Each `hello` that carries `extension_build` (the manifest `version_name`
//! Chrome has loaded) replaces [`LOADED_EXTENSION_FILE`] beside
//! `config.toml` (the support directory deploy-live reads) whole, through a
//! sibling temporary file and a rename (no fsync: losing it to a crash only
//! means "unavailable"). The record is a diagnostic: a build
//! that fails [`valid_build`] is not recorded, and a failed write never
//! fails the hello.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The record's name in the support directory (`LOADED_FILE` in
/// scripts/dev/deploy-live).
pub const LOADED_EXTENSION_FILE: &str = "loaded-extension.json";

/// The longest build the host records, matching the schema's `maxLength`.
pub const MAX_BUILD_CHARS: usize = 128;

/// Whether `build` is worth recording: 1 to [`MAX_BUILD_CHARS`] characters
/// from the `version_name` alphabet (`[0-9A-Za-z .+()_-]`, the schema's
/// pattern), since it is extension-supplied text that deploy-live prints to
/// a terminal: no control, bidi, zero-width or other format characters.
pub fn valid_build(build: &str) -> bool {
    !build.is_empty()
        && build.len() <= MAX_BUILD_CHARS
        && build
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" .+()_-".contains(&b))
}

/// Replace the record in `dir` with `build`, said hello at `now`.
pub fn record(dir: &Path, build: &str, now: OffsetDateTime) -> io::Result<()> {
    let body = serde_json::json!({
        "version": 1,
        "extension_build": build,
        "said_hello_unix": now.unix_timestamp(),
        "said_hello_at": now.format(&Rfc3339).map_err(io::Error::other)?,
    });
    let path = dir.join(LOADED_EXTENSION_FILE);
    let temporary = dir.join(format!(
        ".{LOADED_EXTENSION_FILE}.{}.{}",
        std::process::id(),
        now.unix_timestamp_nanos()
    ));
    let written = (|| {
        let mut file = fs::File::create_new(&temporary)?;
        file.write_all(format!("{body}\n").as_bytes())?;
        // No fsync: this runs before every hello is answered, and a record
        // lost to a crash only makes deploy-live say "unavailable". The
        // rename alone keeps it whole.
        crate::fsutil::rename_or_copy(&temporary, &path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(unix: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(unix).unwrap()
    }

    fn read(dir: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(dir.join(LOADED_EXTENSION_FILE)).unwrap()).unwrap()
    }

    #[test]
    fn records_the_build_and_when_it_said_hello() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), "0.1.0+abcdef012345", at(1_800_000_000)).unwrap();
        let value = read(dir.path());
        assert_eq!(value["version"], 1);
        assert_eq!(value["extension_build"], "0.1.0+abcdef012345");
        assert_eq!(value["said_hello_unix"], 1_800_000_000);
        // RFC 3339 for the same instant as the fixed test clock.
        let said = OffsetDateTime::parse(value["said_hello_at"].as_str().unwrap(), &Rfc3339);
        assert_eq!(said.unwrap(), at(1_800_000_000));
    }

    #[test]
    fn a_later_hello_replaces_the_record_whole_and_leaves_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), "0.1.0+old000000000", at(1_800_000_000)).unwrap();
        record(dir.path(), "0.1.0+new000000000", at(1_800_000_060)).unwrap();
        let value = read(dir.path());
        assert_eq!(value["extension_build"], "0.1.0+new000000000");
        assert_eq!(value["said_hello_unix"], 1_800_000_060);
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [LOADED_EXTENSION_FILE]);
    }

    #[test]
    fn a_build_is_bounded_text() {
        assert!(valid_build("0.1.0+abcdef012345-dirty"));
        assert!(valid_build(&"x".repeat(MAX_BUILD_CHARS)));
        assert!(valid_build("0.1.0 (fe01752470be)"));
        assert!(valid_build("0.1.0 (test_build-2.x)"));
        // Only the version_name alphabet: no bidi overrides, zero-width or
        // other format characters, terminal escapes, or non-ASCII letters.
        for bad in [
            "",
            "a\nb",
            "a\u{7}",
            "0.1.0 (\u{202e}cba)",
            "0.1.0\u{200b}",
            "0.1.0\u{2066}x",
            "\u{1b}[31m0.1.0",
            "0.1.0 (café)",
            "0.1.0/../x",
            &"x".repeat(MAX_BUILD_CHARS + 1),
        ] {
            assert!(!valid_build(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_missing_directory_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        assert!(record(&dir.path().join("gone"), "b", at(1_800_000_000)).is_err());
    }
}

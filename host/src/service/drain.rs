//! The deploy pause: while `scripts/dev/deploy-live --drain` waits for the
//! runs already admitted to finish, it writes [`DRAIN_FILE`] in the support
//! directory, and the service refuses every new run until the time it names
//! (`start_run`, which every run kind goes through). Runs already queued or
//! running carry on. The deploy removes the file when it ends.
//!
//! The marker only ever pauses, so it fails open: a missing, unreadable or
//! malformed file pauses nothing, and the pause never lasts past
//! [`DRAIN_MAX`] after the file was last written, so one a killed deploy
//! left behind cannot hold runs up for long. A marker dated more than
//! [`DRAIN_SKEW`] in the future pauses nothing: its mtime cannot bound it.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

/// The marker's name in the support directory (`DRAIN_FILE` in
/// scripts/dev/deploy-live).
pub const DRAIN_FILE: &str = "deploy-drain.json";

/// The longest a marker pauses runs, from its mtime (`DRAIN_MAX` in
/// scripts/dev/deploy-live).
pub const DRAIN_MAX: Duration = Duration::from_secs(3600);

/// How far in the future a marker's mtime may be (clock adjustments) and
/// still pause runs.
pub const DRAIN_SKEW: Duration = Duration::from_secs(300);

/// What a paused `start_run` answers. Fixed text: it never names the file.
pub const DRAIN_REFUSAL: &str =
    "Brauser is being updated and is not starting new runs; try again in a few minutes";

/// The marker's fields the service reads; the deploy writes others too.
#[derive(Deserialize)]
struct Marker {
    until_unix: u64,
}

/// Whether the marker at `path` ([`super::paths::ServicePaths::drain`])
/// pauses new runs at `now`.
pub fn paused(path: &Path, now: SystemTime) -> bool {
    let (text, modified) = match fs::read_to_string(path)
        .and_then(|text| Ok((text, fs::metadata(path)?.modified()?)))
    {
        Ok(found) => found,
        Err(error) if error.kind() == ErrorKind::NotFound => return false,
        Err(error) => {
            tracing::warn!(error = %error, "deploy pause marker unreadable; not pausing");
            return false;
        }
    };
    let Ok(marker) = serde_json::from_str::<Marker>(&text) else {
        tracing::warn!("deploy pause marker malformed; not pausing");
        return false;
    };
    let until = UNIX_EPOCH + Duration::from_secs(marker.until_unix);
    // A future mtime (a clock stepped back, or a copied file) cannot bound
    // the pause; capping from min(mtime, now) would not either, since now
    // moves with every check. Fail open past a small skew.
    if modified > now + DRAIN_SKEW {
        tracing::warn!("deploy pause marker dated in the future; not pausing");
        return false;
    }
    let cap = modified + DRAIN_MAX;
    now < until.min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn write(dir: &Path, text: &str) {
        fs::write(dir.join(DRAIN_FILE), text).unwrap();
    }

    #[test]
    fn no_marker_pauses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!paused(&dir.path().join(DRAIN_FILE), SystemTime::now()));
    }

    #[test]
    fn a_marker_pauses_until_its_time() {
        let dir = tempfile::tempdir().unwrap();
        let until = now_secs() + 600;
        write(
            dir.path(),
            &format!(r#"{{"version":1,"until_unix":{until},"by":"deploy-live","pid":7}}"#),
        );
        assert!(paused(&dir.path().join(DRAIN_FILE), SystemTime::now()));
        assert!(paused(&dir.path().join(DRAIN_FILE), at(until - 1)));
        assert!(!paused(&dir.path().join(DRAIN_FILE), at(until)));
    }

    #[test]
    fn a_marker_never_pauses_past_the_cap_from_its_mtime() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r#"{"until_unix":99999999999}"#);
        let written = fs::metadata(dir.path().join(DRAIN_FILE))
            .unwrap()
            .modified()
            .unwrap();
        assert!(paused(
            &dir.path().join(DRAIN_FILE),
            written + DRAIN_MAX - Duration::from_secs(1)
        ));
        assert!(!paused(&dir.path().join(DRAIN_FILE), written + DRAIN_MAX));
    }

    #[test]
    fn a_marker_dated_in_the_future_pauses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), r#"{"until_unix":99999999999}"#);
        let path = dir.path().join(DRAIN_FILE);
        let now = at(now_secs());
        let future = now + Duration::from_secs(10 * 3600);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(future)
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), future);
        // Neither now nor any later check before the mtime is paused.
        for later in [0, 1, 3600, 9 * 3600] {
            assert!(!paused(&path, now + Duration::from_secs(later)), "{later}");
        }
        // Within the skew it still pauses, capped from its mtime.
        let near = now + DRAIN_SKEW;
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(near)
            .unwrap();
        assert!(paused(&path, now));
        assert!(paused(&path, near + DRAIN_MAX - Duration::from_secs(1)));
        assert!(!paused(&path, near + DRAIN_MAX));
    }

    #[test]
    fn a_malformed_or_unreadable_marker_pauses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        for text in [
            "",
            "{",
            "[]",
            r#"{"until_unix":"soon"}"#,
            r#"{"until_unix":-1}"#,
        ] {
            write(dir.path(), text);
            assert!(
                !paused(&dir.path().join(DRAIN_FILE), SystemTime::now()),
                "{text:?}"
            );
        }
        fs::remove_file(dir.path().join(DRAIN_FILE)).unwrap();
        fs::create_dir(dir.path().join(DRAIN_FILE)).unwrap();
        assert!(!paused(&dir.path().join(DRAIN_FILE), SystemTime::now()));
    }
}

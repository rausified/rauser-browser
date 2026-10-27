//! Build identity: the checkout's commit, marked "-dirty" when tracked files
//! differ from it. `host/build.rs` includes this file to stamp
//! `BRAUSER_BUILD_ID`; the extension's copy is `extension/build-id.mjs`.
//! Keep the two in step.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Commits are abbreviated to a fixed length so both builds print the same.
pub const COMMIT_LENGTH: usize = 12;

/// The sources built into the host, relative to `host/`. Cargo reruns the
/// build script only when one changes, so the dirty mark covers only these;
/// a whole-tree status would go stale in a cached build.
pub const SOURCES: &[&str] = &[
    "build.rs",
    "src",
    "Cargo.toml",
    "../Cargo.toml",
    "../Cargo.lock",
    "../rust-toolchain.toml",
    "../protocol",
    "../adapter-templates",
];

/// Formats `git rev-parse --short HEAD` and `git status --porcelain` output,
/// each `None` when git failed. A status git could not report is not
/// claimed clean.
pub fn build_id(commit: Option<&str>, status: Option<&str>) -> String {
    match commit.map(str::trim) {
        None | Some("") => "unknown".to_owned(),
        Some(commit) if status == Some("") => commit.to_owned(),
        Some(commit) => format!("{commit}-dirty"),
    }
}

pub fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The build identity of the host package at `host_dir`.
pub fn git_build_id(host_dir: &Path) -> String {
    let short = format!("--short={COMMIT_LENGTH}");
    let commit = git(host_dir, &["rev-parse", &short, "HEAD"]);
    if commit.is_none() {
        return build_id(None, None);
    }
    let mut status = vec!["status", "--porcelain", "--untracked-files=no", "--"];
    status.extend_from_slice(SOURCES);
    build_id(commit.as_deref(), git(host_dir, &status).as_deref())
}

/// What to watch for `path`: the path itself, or else the nearest directory
/// it would appear in. Cargo reruns every build for a missing path, and a
/// packed branch has no loose ref until its next commit writes one.
pub fn watch_path(path: &Path) -> PathBuf {
    path.ancestors()
        .find(|ancestor| ancestor.exists())
        .unwrap_or(path)
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::{COMMIT_LENGTH, build_id, git, git_build_id, watch_path};
    use std::fs;
    use std::path::Path;

    #[test]
    fn build_id_formats_the_commit_and_a_dirty_suffix() {
        assert_eq!(build_id(Some("abc1234def56\n"), Some("")), "abc1234def56");
        assert_eq!(
            build_id(Some("abc1234def56"), Some(" M host/src/native.rs\n")),
            "abc1234def56-dirty"
        );
        assert_eq!(build_id(None, None), "unknown");
        assert_eq!(build_id(Some(""), Some("")), "unknown");
        assert_eq!(build_id(Some("abc1234def56"), None), "abc1234def56-dirty");
    }

    fn run(dir: &Path, args: &[&str]) {
        assert!(git(dir, args).is_some(), "git {args:?} failed");
    }

    #[test]
    fn git_build_id_marks_only_host_sources_dirty() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let host = root.join("host");
        fs::create_dir_all(host.join("src")).unwrap();
        fs::create_dir_all(root.join("protocol")).unwrap();
        fs::write(host.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("protocol/schema.json"), "{}\n").unwrap();
        fs::write(root.join("README.md"), "one\n").unwrap();
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "test@example.com"],
            &["config", "user.name", "Test"],
            &["config", "commit.gpgsign", "false"],
            &["add", "."],
            &["commit", "-q", "-m", "first"],
        ] {
            run(root, args);
        }
        let commit = git(root, &["rev-parse", "HEAD"]).unwrap()[..COMMIT_LENGTH].to_owned();
        assert_eq!(git_build_id(&host), commit);

        // Changes Cargo never rebuilds the host for must not mark it dirty,
        // or a cached build would report a different identity than a fresh one.
        fs::write(root.join("README.md"), "two\n").unwrap();
        fs::write(host.join("src/scratch.rs"), "\n").unwrap();
        assert_eq!(git_build_id(&host), commit);

        fs::write(root.join("protocol/schema.json"), "{ }\n").unwrap();
        assert_eq!(git_build_id(&host), format!("{commit}-dirty"));
        run(root, &["checkout", "-q", "--", "protocol"]);
        fs::write(host.join("src/main.rs"), "fn main() { }\n").unwrap();
        assert_eq!(git_build_id(&host), format!("{commit}-dirty"));
    }

    #[test]
    fn git_build_id_outside_a_checkout_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        // Not a repository, unless the temp directory sits inside one.
        if git(dir.path(), &["rev-parse", "HEAD"]).is_none() {
            assert_eq!(git_build_id(dir.path()), "unknown");
        }
    }

    #[test]
    fn watch_path_falls_back_to_the_directory_a_missing_ref_would_appear_in() {
        let dir = tempfile::tempdir().unwrap();
        let heads = dir.path().join("refs/heads");
        fs::create_dir_all(&heads).unwrap();
        fs::write(heads.join("main"), "ref\n").unwrap();
        assert_eq!(watch_path(&heads.join("main")), heads.join("main"));
        // A packed branch has no loose ref until its next commit writes one.
        assert_eq!(watch_path(&heads.join("feat/packed")), heads);
    }
}

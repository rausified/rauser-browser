// Stamps BRAUSER_BUILD_ID, the checkout's commit plus "-dirty" for tracked
// changes, for hello's host_version (src/build_id.rs).
use std::path::Path;
use std::process::Command;

#[path = "src/build_id.rs"]
mod build_id;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
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

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let short = format!("--short={}", build_id::COMMIT_LENGTH);
    let commit = git(dir, &["rev-parse", &short, "HEAD"]);
    let status = commit
        .as_ref()
        .and(git(dir, &["status", "--porcelain", "--untracked-files=no"]));
    println!(
        "cargo:rustc-env=BRAUSER_BUILD_ID={}",
        build_id::build_id(commit.as_deref(), status.as_deref())
    );

    // Rerun when HEAD moves or the sources built into the host change.
    // Watching the index instead would rebuild after every `git status`.
    println!("cargo:rerun-if-changed=build.rs");
    for path in [
        "src",
        "Cargo.toml",
        "../Cargo.lock",
        "../protocol",
        "../adapter-templates",
        "../macos-alert",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let head = git(dir, &["rev-parse", "--git-path", "HEAD"]);
    let branch = git(dir, &["symbolic-ref", "-q", "HEAD"]);
    let packed = git(dir, &["rev-parse", "--git-path", "packed-refs"]);
    let reference = branch.and_then(|name| git(dir, &["rev-parse", "--git-path", name.trim()]));
    for path in [head, reference, packed].into_iter().flatten() {
        // --git-path answers relative to the working directory it ran in.
        // Cargo reruns every build for a missing path, and a branch ref may
        // live only in packed-refs.
        let path = dir.join(path.trim());
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

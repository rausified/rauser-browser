// Stamps BRAUSER_BUILD_ID, the checkout's commit plus "-dirty" for changed
// host sources, for hello's host_version (src/build_id.rs).
use std::path::Path;

#[path = "src/build_id.rs"]
mod build_id;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    println!(
        "cargo:rustc-env=BRAUSER_BUILD_ID={}",
        build_id::git_build_id(dir)
    );

    // Rerun when HEAD moves or a source the dirty mark covers changes.
    // Watching the index instead would rebuild after every `git status`.
    for path in build_id::SOURCES {
        println!("cargo:rerun-if-changed={path}");
    }
    let git_path = |name: &str| build_id::git(dir, &["rev-parse", "--git-path", name]);
    let head = git_path("HEAD");
    let packed = git_path("packed-refs");
    let reference =
        build_id::git(dir, &["symbolic-ref", "-q", "HEAD"]).and_then(|name| git_path(name.trim()));
    for path in [head, reference, packed].into_iter().flatten() {
        // --git-path answers relative to the working directory it ran in.
        let path = build_id::watch_path(&dir.join(path.trim()));
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

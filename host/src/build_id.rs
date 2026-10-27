//! Build identity: the checkout's commit, marked "-dirty" when tracked files
//! differ from it. `host/build.rs` includes this file to stamp
//! `BRAUSER_BUILD_ID`; the extension's copy is `extension/build-id.mjs`.
//! Keep the two in step.

/// Commits are abbreviated to a fixed length so both builds print the same.
pub const COMMIT_LENGTH: usize = 12;

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

#[cfg(test)]
mod tests {
    use super::build_id;

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
}

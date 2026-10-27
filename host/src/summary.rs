//! Summaries (spec §7): the Brauser-owned summary file and the `summary`
//! daily-log event that records it.
//!
//! The file side names, owns and writes one summary per record key, beside
//! the page note: `summaries/<note stem>.summary.md`, whose leading
//! frontmatter `brauser:` block is `kind: summary`, `canonical_url`, then
//! `url_id` or `artifact_id` (DESIGN.md §5.2, §6.6). `owned_summary` reads
//! only that block, and `Vault::write_summary` replaces a file only when it
//! passes, so a note, another record's summary, or the user's own file is a
//! conflict and is never adopted. This module never invokes a harness.
//!
//! The event side is storage-layer support for "Summarize & log" (§7.2): a
//! page with no matching adapter becomes a `web.page` artifact, keyed by its
//! own URL, and `summary_path` names the summary file written above.

use anyhow::Result;
use brauser_protocol::{RecordKey, RecordKeyKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::vault::{note_filename, owned_frontmatter};

pub const MAX_TITLE_BYTES: usize = 2048;
pub const MAX_DOCUMENT_TYPE_BYTES: usize = 64;
pub const MAX_CATEGORY_BYTES: usize = 64;
pub const MAX_SUMMARY_PATH_BYTES: usize = 1024;

/// What a caller supplies to record a summary. The host derives the
/// `web.page` artifact identity from `url`; callers never supply one.
#[derive(Clone, Debug)]
pub struct PageSummary {
    pub event_id: String,
    pub url: String,
    pub title: String,
    pub occurred_at: String,
    pub document_type: String,
    pub category: String,
    pub importance: String,
    pub summary_path: String,
}

/// The durable, indexed form of a `PageSummary`: the marker payload stored in
/// the daily log and replayed by `ArtifactIndex`. `url` and `artifact_id`
/// hold the normalized URL and the identity derived from it, not whatever a
/// caller originally supplied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryEvent {
    pub event_id: String,
    pub artifact_id: String,
    pub artifact_type: String,
    pub url: String,
    pub title: String,
    pub occurred_at: String,
    pub document_type: String,
    pub category: String,
    pub importance: String,
    pub summary_path: String,
}

pub(crate) fn validate_page_summary(summary: &PageSummary) -> Result<(), &'static str> {
    if summary.title.trim().is_empty() || summary.title.len() > MAX_TITLE_BYTES {
        return Err("invalid_summary_title");
    }
    if summary.document_type.trim().is_empty()
        || summary.document_type.len() > MAX_DOCUMENT_TYPE_BYTES
    {
        return Err("invalid_summary_document_type");
    }
    if summary.category.trim().is_empty() || summary.category.len() > MAX_CATEGORY_BYTES {
        return Err("invalid_summary_category");
    }
    if !matches!(summary.importance.as_str(), "high" | "medium" | "low") {
        return Err("invalid_summary_importance");
    }
    if summary.summary_path.is_empty()
        || summary.summary_path.len() > MAX_SUMMARY_PATH_BYTES
        || summary.summary_path.starts_with('/')
        || summary.summary_path.split('/').any(|part| part == "..")
        || !summary.summary_path.ends_with(".summary.md")
        // The rendered link wraps this in `<...>` (CommonMark's bracketed
        // destination form), which forbids line breaks and unescaped `<`/
        // `>`; `#`/`?` are not special to CommonMark there, but some
        // Markdown viewers (DESIGN.md's Obsidian/VS Code preview bar) treat
        // a link destination as a URI reference and would resolve a
        // trailing fragment or query instead of opening the file. Reject
        // rather than escape: a vault-relative path has no legitimate
        // reason to need any of these.
        || summary
            .summary_path
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | '#' | '?'))
    {
        return Err("invalid_summary_path");
    }
    Ok(())
}

/// Re-validate a `SummaryEvent` replayed from the log: everything
/// `validate_page_summary` checks, plus the identity invariant that only
/// exists once a URL has been normalized and hashed.
pub(crate) fn validate_summary_event(event: &SummaryEvent) -> bool {
    let as_summary = PageSummary {
        event_id: event.event_id.clone(),
        url: event.url.clone(),
        title: event.title.clone(),
        occurred_at: event.occurred_at.clone(),
        document_type: event.document_type.clone(),
        category: event.category.clone(),
        importance: event.importance.clone(),
        summary_path: event.summary_path.clone(),
    };
    if validate_page_summary(&as_summary).is_err() {
        return false;
    }
    event.artifact_type == "web.page"
        && web_page_artifact_id(&event.url).as_deref() == Ok(event.artifact_id.as_str())
}

/// The `web:<host>/<sha256(url)>` identity `host/src/index.rs`'s legacy
/// visits already use for an unrecognized page. Summaries reuse it exactly
/// so a page that later gains both a summary and a legacy visit share one
/// artifact.
pub(crate) fn web_page_artifact_id(normalized_url: &str) -> Result<String, &'static str> {
    let parsed = Url::parse(normalized_url).map_err(|_| "invalid_summary_url")?;
    let host = parsed.host_str().ok_or("invalid_summary_url")?;
    let hash = hex::encode(Sha256::digest(normalized_url.as_bytes()));
    Ok(format!("web:{host}/{hash}"))
}

/// The summary file's name: the page note's stem plus `.summary.md`, so a
/// summary and a note for one record share a stem and never a name. A URL key
/// must match `canonical_url`, exactly as for the note.
#[cfg_attr(not(test), allow(dead_code))]
pub fn summary_filename(key: &RecordKey, canonical_url: &str) -> Result<String> {
    let note = note_filename(key, canonical_url)?;
    let stem = note.strip_suffix(".md").unwrap_or(&note);
    Ok(format!("{stem}.summary.md"))
}

/// True only for a summary file of `key`: its leading frontmatter holds one
/// `brauser:` block with `kind: summary` and this record's `url_id` or
/// `artifact_id`, and not the other key. Body text is never read, so agent
/// text cannot change a file's identity. `canonical_url` is not part of the
/// check: an artifact's summary keeps one name across its URLs.
#[cfg_attr(not(test), allow(dead_code))]
pub fn owned_summary(bytes: &[u8], key: &RecordKey) -> bool {
    let (id, forbidden) = match key.kind {
        RecordKeyKind::Url => ("url_id", "artifact_id"),
        RecordKeyKind::Artifact => ("artifact_id", "url_id"),
    };
    owned_frontmatter(bytes, "summary", &[(id, key.value.as_str())], &[forbidden])
}

// PR 3 dependency: render_summary lands after crate::answer exists (Task 45 remainder)

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> PageSummary {
        PageSummary {
            event_id: "11111111-1111-1111-1111-111111111111".into(),
            url: "https://news.ycombinator.com/item?id=50251344".into(),
            title: "A good discussion".into(),
            occurred_at: "2026-10-26T18:00:00Z".into(),
            document_type: "discussion".into(),
            category: "reading".into(),
            importance: "medium".into(),
            summary_path: "pages/news-ycombinator-com.summary.md".into(),
        }
    }

    #[test]
    fn a_well_formed_summary_is_valid() {
        assert!(validate_page_summary(&summary()).is_ok());
    }

    #[test]
    fn an_unknown_importance_is_rejected() {
        let mut bad = summary();
        bad.importance = "urgent".into();
        assert_eq!(
            validate_page_summary(&bad),
            Err("invalid_summary_importance")
        );
    }

    #[test]
    fn a_summary_path_outside_the_vault_is_rejected() {
        let mut bad = summary();
        bad.summary_path = "../../etc/passwd.summary.md".into();
        assert_eq!(validate_page_summary(&bad), Err("invalid_summary_path"));
    }

    #[test]
    fn a_summary_path_without_the_summary_suffix_is_rejected() {
        let mut bad = summary();
        bad.summary_path = "pages/news-ycombinator-com.md".into();
        assert_eq!(validate_page_summary(&bad), Err("invalid_summary_path"));
    }

    #[test]
    fn a_summary_path_with_a_link_breaking_character_is_rejected() {
        for path in [
            "pages/>evil.summary.md",
            "pages/<script>.summary.md",
            "pages/evil\n.summary.md",
            "pages/evil#frag.summary.md",
            "pages/evil?query.summary.md",
        ] {
            let mut bad = summary();
            bad.summary_path = path.into();
            assert_eq!(
                validate_page_summary(&bad),
                Err("invalid_summary_path"),
                "{path} should be rejected"
            );
        }
    }

    #[test]
    fn web_page_artifact_id_is_stable_for_the_same_url() {
        let first = web_page_artifact_id("https://news.ycombinator.com/item?id=50251344").unwrap();
        let second = web_page_artifact_id("https://news.ycombinator.com/item?id=50251344").unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("web:news.ycombinator.com/"));
    }

    #[test]
    fn web_page_artifact_id_differs_by_full_url_not_just_host() {
        let a = web_page_artifact_id("https://news.ycombinator.com/item?id=1").unwrap();
        let b = web_page_artifact_id("https://news.ycombinator.com/item?id=2").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_summary_event_with_a_forged_artifact_id_is_invalid() {
        let id = web_page_artifact_id("https://news.ycombinator.com/item?id=50251344").unwrap();
        let event = SummaryEvent {
            event_id: summary().event_id,
            artifact_id: "web:evil.example/0000".into(),
            artifact_type: "web.page".into(),
            url: "https://news.ycombinator.com/item?id=50251344".into(),
            title: summary().title,
            occurred_at: summary().occurred_at,
            document_type: summary().document_type,
            category: summary().category,
            importance: summary().importance,
            summary_path: summary().summary_path,
        };
        assert_ne!(event.artifact_id, id);
        assert!(!validate_summary_event(&event));
    }

    use crate::brand::NAMESPACE;
    use crate::layout;
    use crate::markdown::LinkStyle;
    use crate::vault::{OwnedWrite, Vault, note_filename, url_record_key};
    use brauser_protocol::{RecordKey, RecordKeyKind, StorageConfig};
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    const PAGE_URL: &str = "https://e.example/docs/a";

    fn fixed_storage(root: &Path) -> StorageConfig {
        StorageConfig {
            root: root.to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: layout::LOGS_DIR.into(),
            pages_dir: layout::NOTES_DIR.into(),
            later_dir: layout::LATER_DIR.into(),
            artifacts_dir: layout::ARTIFACTS_DIR.into(),
            summaries_dir: Some(layout::SUMMARIES_DIR.into()),
            knowledge_base_dir: None,
        }
    }

    fn fixed_vault() -> (tempfile::TempDir, Vault) {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open(&fixed_storage(root.path())).unwrap();
        (root, vault)
    }

    fn artifact_key(value: &str) -> RecordKey {
        RecordKey {
            kind: RecordKeyKind::Artifact,
            value: value.into(),
        }
    }

    /// A summary file's leading frontmatter, built by hand in the PR 4
    /// identity layout: `kind: summary`, `canonical_url`, then the record key.
    fn summary_fixture(canonical_url: &str, key_lines: &str) -> String {
        format!(
            "---\ntitle: \"T\"\nbrauser:\n  kind: summary\n  canonical_url: {}\n{key_lines}---\n\nAgent text.\n",
            serde_json::to_string(canonical_url).unwrap()
        )
    }

    fn url_id_line(value: &str) -> String {
        format!("  url_id: {}\n", serde_json::to_string(value).unwrap())
    }

    fn artifact_id_line(value: &str) -> String {
        format!("  artifact_id: {}\n", serde_json::to_string(value).unwrap())
    }

    /// The vault-relative path with `/` separators, as the summary event
    /// stores it on every platform.
    fn slash_path(path: &Path) -> String {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    #[test]
    fn summary_name_never_equals_the_note_name() {
        let (_root, vault) = fixed_vault();
        let url_key = url_record_key(PAGE_URL).unwrap();
        let jira = artifact_key("jira:acme.example/PROJ-1");
        for key in [&url_key, &jira] {
            let note = note_filename(key, PAGE_URL).unwrap();
            let summary = summary_filename(key, PAGE_URL).unwrap();
            assert_ne!(summary, note);
            assert_eq!(
                summary.strip_suffix(".summary.md"),
                note.strip_suffix(".md"),
                "the summary shares the note's stem"
            );
            let summary_path = vault.summary_relative_path(key, PAGE_URL).unwrap();
            let note_path = vault.note_relative_path(key, PAGE_URL).unwrap();
            assert_ne!(summary_path, note_path);
            assert_eq!(
                summary_path,
                Path::new(layout::SUMMARIES_DIR).join(&summary)
            );
        }
        // A URL key must still match its canonical URL, as for notes.
        assert!(summary_filename(&url_key, "https://e.example/other").is_err());
    }

    #[test]
    fn summary_paths_pass_the_summary_event_path_check() {
        let (_root, vault) = fixed_vault();
        let tricky_url = "https://e.example/a#b/c?d=<e>&f=#g";
        let tricky_canonical = {
            let mut parsed = url::Url::parse(tricky_url).unwrap();
            parsed.set_fragment(None);
            parsed.to_string()
        };
        let cases = [
            (url_record_key(PAGE_URL).unwrap(), PAGE_URL.to_owned()),
            (url_record_key(&tricky_canonical).unwrap(), tricky_canonical),
            (
                artifact_key("jira:acme.example/PROJ-1"),
                PAGE_URL.to_owned(),
            ),
            (artifact_key("x:e.example/#?<>"), PAGE_URL.to_owned()),
        ];
        for (key, canonical_url) in cases {
            let path = vault.summary_relative_path(&key, &canonical_url).unwrap();
            for title in ["Plain", "Issue #12: why? <maybe>"] {
                let mut candidate = summary();
                candidate.url = canonical_url.clone();
                candidate.title = title.into();
                candidate.summary_path = slash_path(&path);
                assert_eq!(
                    validate_page_summary(&candidate),
                    Ok(()),
                    "{} for {key:?}",
                    candidate.summary_path
                );
            }
        }
    }

    #[test]
    fn the_note_link_follows_the_profile_link_style() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let note = note_filename(&key, PAGE_URL).unwrap();
        let stem = note.strip_suffix(".md").unwrap();
        assert_eq!(
            vault
                .note_link(&key, PAGE_URL, LinkStyle::Relative)
                .unwrap(),
            format!("[Page note](../{}/{note})", layout::NOTES_DIR)
        );
        assert_eq!(
            vault
                .note_link(&key, PAGE_URL, LinkStyle::Wikilink)
                .unwrap(),
            format!("[[{stem}|Page note]]")
        );
    }

    #[test]
    fn owned_summary_reads_only_the_summary_identity_block() {
        let url_key = url_record_key(PAGE_URL).unwrap();
        let jira = artifact_key("jira:acme.example/PROJ-1");
        let ours = summary_fixture(PAGE_URL, &url_id_line(&url_key.value));
        assert!(owned_summary(ours.as_bytes(), &url_key));
        assert!(!owned_summary(ours.as_bytes(), &jira));
        let adapter = summary_fixture(PAGE_URL, &artifact_id_line(&jira.value));
        assert!(owned_summary(adapter.as_bytes(), &jira));
        assert!(!owned_summary(adapter.as_bytes(), &url_key));
        // Both keys at once is neither record's summary.
        let both = summary_fixture(
            PAGE_URL,
            &format!(
                "{}{}",
                url_id_line(&url_key.value),
                artifact_id_line(&jira.value)
            ),
        );
        assert!(!owned_summary(both.as_bytes(), &url_key));
        assert!(!owned_summary(both.as_bytes(), &jira));
        // Agent text in the body cannot make a file a summary.
        let forged = format!(
            "---\ntitle: \"T\"\n---\n\n{}",
            summary_fixture(PAGE_URL, &url_id_line(&url_key.value))
        );
        assert!(!owned_summary(forged.as_bytes(), &url_key));
    }

    #[test]
    fn write_summary_never_adopts_a_note_or_foreign_summary() {
        let (root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let path = root
            .path()
            .join(vault.summary_relative_path(&key, PAGE_URL).unwrap());
        let ours = |body: &str| {
            summary_fixture(PAGE_URL, &url_id_line(&key.value)).replace("Agent text.", body)
        };

        assert!(matches!(
            vault.write_summary(&key, PAGE_URL, &ours("one")).unwrap(),
            OwnedWrite::Written
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), ours("one"));
        assert!(matches!(
            vault.write_summary(&key, PAGE_URL, &ours("one")).unwrap(),
            OwnedWrite::Unchanged
        ));
        assert!(matches!(
            vault.write_summary(&key, PAGE_URL, &ours("two")).unwrap(),
            OwnedWrite::Written
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), ours("two"));

        let other = url_record_key("https://e.example/other").unwrap();
        let jira = artifact_key("jira:acme.example/PROJ-1");
        let occupants = [
            // The page note for the same record, which has no kind line.
            format!(
                "---\ntitle: \"T\"\nurl: \"{PAGE_URL}\"\nbrauser:\n  canonical_url: \"{PAGE_URL}\"\n{}  created: 2026-01-01T00:00:00Z\n---\n\nThe user's note.\n",
                url_id_line(&key.value)
            ),
            // The same layout with an explicit page kind.
            summary_fixture(PAGE_URL, &url_id_line(&key.value))
                .replace("kind: summary", "kind: page"),
            // Another record's summary.
            summary_fixture("https://e.example/other", &url_id_line(&other.value)),
            // An adapter artifact's summary.
            summary_fixture(PAGE_URL, &artifact_id_line(&jira.value)),
            // This record's key and an artifact key at once.
            summary_fixture(
                PAGE_URL,
                &format!(
                    "{}{}",
                    url_id_line(&key.value),
                    artifact_id_line(&jira.value)
                ),
            ),
            "The user's own Markdown.\n".to_owned(),
        ];
        for occupant in occupants {
            fs::write(&path, &occupant).unwrap();
            assert!(
                matches!(
                    vault.write_summary(&key, PAGE_URL, &ours("three")).unwrap(),
                    OwnedWrite::Conflict
                ),
                "adopted {occupant:?}"
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), occupant);
        }

        // An artifact-keyed summary is owned by its artifact key alone.
        let jira_path = root
            .path()
            .join(vault.summary_relative_path(&jira, PAGE_URL).unwrap());
        let jira_summary = summary_fixture(PAGE_URL, &artifact_id_line(&jira.value));
        fs::write(
            &jira_path,
            summary_fixture(PAGE_URL, &url_id_line(&key.value)),
        )
        .unwrap();
        assert!(matches!(
            vault.write_summary(&jira, PAGE_URL, &jira_summary).unwrap(),
            OwnedWrite::Conflict
        ));
        fs::remove_file(&jira_path).unwrap();
        assert!(matches!(
            vault.write_summary(&jira, PAGE_URL, &jira_summary).unwrap(),
            OwnedWrite::Written
        ));
        // Nothing was ever written to the notes folder.
        assert!(!root.path().join(layout::NOTES_DIR).exists());
    }

    #[test]
    fn a_vault_without_a_summaries_folder_has_no_summary_paths() {
        let root = tempfile::tempdir().unwrap();
        let mut storage = fixed_storage(root.path());
        storage.summaries_dir = None;
        let vault = Vault::open(&storage).unwrap();
        let key = url_record_key(PAGE_URL).unwrap();
        assert!(vault.summary_relative_path(&key, PAGE_URL).is_err());
        assert!(
            vault
                .note_link(&key, PAGE_URL, LinkStyle::Relative)
                .is_err()
        );
        assert!(vault.write_summary(&key, PAGE_URL, "x").is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn stale_summary_temporaries_are_swept() {
        let (root, vault) = fixed_vault();
        let summaries = root.path().join(layout::SUMMARIES_DIR);
        let notes = root.path().join(layout::NOTES_DIR);
        fs::create_dir_all(&summaries).unwrap();
        fs::create_dir_all(&notes).unwrap();
        let old = crate::vault::STALE_TEMPORARY_AGE + Duration::from_secs(60);
        let uuid = || uuid::Uuid::new_v4();
        let stale = summaries.join(format!(".{NAMESPACE}-summary-{}.tmp", uuid()));
        let stale_note = notes.join(format!(".{NAMESPACE}-{}.tmp", uuid()));
        let fresh = summaries.join(format!(".{NAMESPACE}-summary-{}.tmp", uuid()));
        // Names this host never writes in the summaries folder stay.
        let kept = [
            summaries.join(format!(".{NAMESPACE}-artifact-{}.tmp", uuid())),
            summaries.join(format!(".{NAMESPACE}-{}.tmp", uuid())),
            summaries.join(format!(".{NAMESPACE}-summary-notes.tmp")),
            summaries.join("page.summary.md"),
        ];
        for path in kept.iter().chain([&stale, &stale_note, &fresh]) {
            fs::write(path, "content").unwrap();
        }
        for path in kept.iter().chain([&stale, &stale_note]) {
            crate::vault::tests::age_file(path, old);
        }

        assert_eq!(vault.sweep_stale_temporaries().unwrap(), 2);
        assert!(!stale.exists());
        assert!(!stale_note.exists());
        for path in kept.iter().chain([&fresh]) {
            assert!(path.exists(), "{} was removed", path.display());
        }
    }

    #[test]
    fn the_sweep_tolerates_a_missing_summaries_folder() {
        let (_root, vault) = fixed_vault();
        assert_eq!(vault.sweep_stale_temporaries().unwrap(), 0);
    }
}

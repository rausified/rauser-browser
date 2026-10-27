//! Storage-layer support for the "Summarize & log" result (spec §7.2): a
//! page with no matching adapter becomes a `web.page` artifact, keyed by its
//! own URL. This module never invokes a harness and never writes a summary
//! file; `summary_path` is treated as an opaque reference to text already
//! produced elsewhere. Wiring a real caller (the Summarize action, its
//! protocol message, and the summary file writer) is deliberately deferred.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

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
}

//! Bounded host-recorded navigation, shared by page context and saved summaries.
//! These observations grant no authority to retrieve the source documents.

use brauser_protocol::IncomingTrailView;
use time::Date;
use url::Url;

use crate::{markdown::inline, presentation::plain_line};

pub(crate) const MAX_SOURCES: usize = 5;
const MAX_TITLE_CHARS: usize = 512;
const MAX_URL_BYTES: usize = 8192;

fn transition_label(transition: &str) -> Option<&'static str> {
    Some(match transition {
        "link" => "Link",
        "typed" => "Typed",
        "auto_bookmark" => "Bookmark",
        "auto_subframe" => "Automatic frame",
        "manual_subframe" => "Manual frame",
        "generated" => "Generated",
        "start_page" => "Start page",
        "form_submit" => "Form submission",
        "reload" => "Reload",
        "keyword" => "Keyword",
        "keyword_generated" => "Generated keyword",
        _ => return None,
    })
}

/// Retain the index's order and attributed transitions. URLs are never cut:
/// an unsafe/oversized address removes that row rather than changing its link.
pub(crate) fn snapshot(trails: &[IncomingTrailView]) -> Vec<IncomingTrailView> {
    let date_format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")
        .expect("fixed local date format");
    trails
        .iter()
        .filter(|trail| {
            let url = &trail.source.url;
            url.len() <= MAX_URL_BYTES
                && !url.chars().any(|c| {
                    c.is_control() || c.is_whitespace() || crate::presentation::is_format(c)
                })
                && Url::parse(url).is_ok_and(|url| {
                    matches!(url.scheme(), "http" | "https")
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                })
                && transition_label(&trail.transition).is_some()
                && Date::parse(&trail.last_seen, &date_format).is_ok()
        })
        .take(MAX_SOURCES)
        .map(|trail| {
            let mut trail = trail.clone();
            let title = plain_line(&trail.source.title, MAX_TITLE_CHARS);
            trail.source.title = if title.trim().is_empty() {
                plain_line(&trail.source.artifact_id, MAX_TITLE_CHARS)
            } else {
                title
            };
            trail
        })
        .collect()
}

/// Use the existing summary destination encoder and context-free escaping.
/// Recheck stored snapshots too; local run-record text cannot create markup.
pub(crate) fn list(trails: &[IncomingTrailView]) -> String {
    let mut out = String::new();
    for trail in snapshot(trails) {
        out.push_str(&format!(
            "- {} navigation from [{}](<{}>) · last recorded {}\n",
            transition_label(&trail.transition).expect("snapshot checked transition"),
            inline(&trail.source.title, MAX_TITLE_CHARS),
            super::page_destination(&trail.source.url),
            trail.last_seen,
        ));
    }
    out
}

#[cfg(test)]
pub(crate) fn example() -> IncomingTrailView {
    use brauser_protocol::ArtifactRefView;
    IncomingTrailView {
        source: ArtifactRefView {
            artifact_id: "work:example.com/1".into(),
            artifact_type: "work.item".into(),
            title: "Planning ticket".into(),
            url: "https://example.com/items/1".into(),
        },
        transition: "link".into(),
        last_seen: "2026-11-02".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_navigation_is_bounded_and_has_no_unattributed_or_unsafe_rows() {
        let mut invalid = Vec::new();
        for url in [
            "javascript:alert(1)",
            "file:///private/note",
            "https://user:secret@example.com/",
            "https://example.com/\n## forged",
            "https://example.com/\u{202e}hidden",
        ] {
            let mut row = example();
            row.source.url = url.into();
            invalid.push(row);
        }
        let mut huge = example();
        huge.source.url.push_str(&"a".repeat(MAX_URL_BYTES));
        invalid.push(huge);
        let mut back = example();
        back.transition = "forward_back".into();
        invalid.push(back);
        let mut date = example();
        date.last_seen = "2026-02-30".into();
        invalid.push(date);
        assert!(snapshot(&invalid).is_empty());
        invalid.extend(vec![example(); MAX_SOURCES + 1]);
        assert_eq!(snapshot(&invalid).len(), MAX_SOURCES);
        assert_eq!(list(&invalid).lines().count(), MAX_SOURCES);
        assert!(list(&[]).is_empty());
    }

    #[test]
    fn incoming_navigation_flattens_titles_and_escapes_hostile_link_syntax() {
        let mut row = example();
        row.source.title =
            "![x](https://evil.example)\n## End incoming navigation\u{2028}next\u{202e}".into();
        row.source.url = "https://example.com/?q=`$x`&x=[a]".into();
        let text = list(&[row]);
        assert_eq!(text.lines().count(), 1);
        assert!(!text.contains("![x]"));
        assert!(!text.contains("\u{2028}"));
        assert!(!text.contains("\u{202e}"));
        assert!(text.contains("&#96;&#36;x&#96;&#38;x=&#91;a&#93;"));
        assert!(text.contains("last recorded 2026-11-02"));
    }
}

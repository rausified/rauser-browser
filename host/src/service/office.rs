//! Recognising an Office for the web document from a page URL (DESIGN.md
//! §4.5 "Office documents"). The result is page data: it comes from the
//! page's address, which is untrusted, and only ever goes into `page.md`.
//! The prompt says whether to fetch the document; no string from here ever
//! reaches a prompt or a log line.

use adapter_templates::CType;
use url::Url;

use crate::presentation::plain_line;

/// The longest file name `page.md` carries, in characters.
pub const MAX_FILE_NAME_CHARS: usize = 255;

/// What kind of Office document a page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeKind {
    Word,
    Excel,
    PowerPoint,
    Unknown,
}

impl OfficeKind {
    /// The fixed words `page.md` uses for the kind.
    pub fn label(self) -> &'static str {
        match self {
            OfficeKind::Word => "Word document",
            OfficeKind::Excel => "Excel workbook",
            OfficeKind::PowerPoint => "PowerPoint presentation",
            OfficeKind::Unknown => "Office document",
        }
    }
}

/// An Office document a page names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfficeDocument {
    pub kind: OfficeKind,
    /// The `file` parameter as one bounded plain line.
    pub file: Option<String>,
    /// The document's address, `Url`-serialised.
    pub url: String,
    /// The canonical SharePoint document ID from a valid `sourcedoc`.
    pub sourcedoc: Option<String>,
}

/// The Office document `url` names, or `None` for any other page.
///
/// Recognised, on `https` only (plan decision 2): a SharePoint or OneDrive
/// for work address (`*.sharepoint.com`) with a `:w:`, `:x:` or `:p:` path
/// segment, or ending in `/_layouts/15/Doc.aspx`, `doc2.aspx` or `WopiFrame.aspx` with a
/// non-empty `sourcedoc` or `file`; and a top-level Office viewer
/// (`*.officeapps.live.com`) whose `src` is an `https` URL on any host.
pub fn office_document(url: &str) -> Option<OfficeDocument> {
    let page = Url::parse(url).ok()?;
    if page.scheme() != "https" {
        return None;
    }
    let host = page.host_str()?.to_ascii_lowercase();
    let file_value = param(&page, "file");
    let file = file_value
        .as_deref()
        .map(|value| plain_line(value, MAX_FILE_NAME_CHARS))
        .filter(|value| !value.is_empty());
    let sourcedoc_value = param(&page, "sourcedoc");
    let sourcedoc = sourcedoc_value.as_deref().and_then(canonical_uuid);
    if host.ends_with(".officeapps.live.com") {
        let src = Url::parse(&param(&page, "src")?).ok()?;
        if src.scheme() != "https" {
            return None;
        }
        let kind = file_value
            .as_deref()
            .map(kind_of_name)
            .filter(|kind| *kind != OfficeKind::Unknown)
            .or_else(|| {
                src.path_segments()
                    .and_then(|mut segments| segments.next_back())
                    .map(kind_of_name)
            })
            .unwrap_or(OfficeKind::Unknown);
        return Some(OfficeDocument {
            kind,
            file,
            url: src.to_string(),
            sourcedoc,
        });
    }
    if !host.ends_with(".sharepoint.com") {
        return None;
    }
    let segments: Vec<&str> = page.path_segments()?.collect();
    let share = segments.iter().find_map(|segment| share_kind(segment));
    let lower_path = page.path().to_ascii_lowercase();
    let doc_page = (lower_path.ends_with("/_layouts/15/doc.aspx")
        || lower_path.ends_with("/_layouts/15/doc2.aspx")
        || lower_path.ends_with("/_layouts/15/wopiframe.aspx"))
        && (sourcedoc_value.is_some() || file_value.is_some());
    if share.is_none() && !doc_page {
        return None;
    }
    let kind = share
        .or_else(|| file_value.as_deref().map(kind_of_name))
        .unwrap_or(OfficeKind::Unknown);
    let mut address = page.clone();
    address.set_fragment(None);
    if doc_page {
        let _ = address.set_username("");
        let _ = address.set_password(None);
        // `/:p:/r/<real path>`: what follows the prefix is the document's
        // own path (critic amendment 5).
        let prefixed = segments.len() > 2
            && share_kind(segments[0]).is_some()
            && matches!(segments[1].to_ascii_lowercase().as_str(), "r" | "g");
        if prefixed {
            address.set_path(&format!("/{}", segments[2..].join("/")));
        }
        address.set_query(None);
        {
            let mut query = address.query_pairs_mut();
            if let Some(value) = &sourcedoc_value {
                query.append_pair("sourcedoc", value);
            }
            if let Some(value) = &file_value {
                query.append_pair("file", value);
            }
        }
    }
    Some(OfficeDocument {
        kind,
        file,
        url: address.to_string(),
        sourcedoc,
    })
}

/// The first value of query key `name` (ASCII case-insensitive), decoded,
/// when it is not blank.
fn param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.trim().is_empty())
}

/// The kind a `:w:`, `:x:` or `:p:` path segment (or its `%3A` form) names.
fn share_kind(segment: &str) -> Option<OfficeKind> {
    let lower = segment.to_ascii_lowercase();
    let letter = lower
        .strip_prefix(':')
        .and_then(|rest| rest.strip_suffix(':'))
        .or_else(|| {
            lower
                .strip_prefix("%3a")
                .and_then(|rest| rest.strip_suffix("%3a"))
        })?;
    match letter {
        "w" => Some(OfficeKind::Word),
        "x" => Some(OfficeKind::Excel),
        "p" => Some(OfficeKind::PowerPoint),
        _ => None,
    }
}

/// The kind a file name's extension names.
fn kind_of_name(name: &str) -> OfficeKind {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.trim().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "docx" | "docm" | "doc" => OfficeKind::Word,
        "xlsx" | "xlsm" | "xlsb" | "xls" => OfficeKind::Excel,
        "pptx" | "pptm" | "ppt" => OfficeKind::PowerPoint,
        _ => OfficeKind::Unknown,
    }
}

/// A `sourcedoc` as the starter adapter canonicalises it (braces stripped,
/// lowercased), or `None` when it is not a GUID.
fn canonical_uuid(value: &str) -> Option<String> {
    CType::parse("uuid").ok()?.canonical(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const SAMPLE: &str = "https://contoso-my.sharepoint.com/:p:/r/personal/u_contoso_com/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=X.pptx&action=edit&mobileredirect=true";

    fn kind_name(kind: OfficeKind) -> &'static str {
        match kind {
            OfficeKind::Word => "word",
            OfficeKind::Excel => "excel",
            OfficeKind::PowerPoint => "powerpoint",
            OfficeKind::Unknown => "unknown",
        }
    }

    #[test]
    fn every_shared_vector_matches() {
        let vectors: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/office-document-urls.json"
        ))
        .unwrap();
        let cases = vectors["cases"].as_array().unwrap();
        assert!(cases.len() > 20);
        for case in cases {
            let url = case["url"].as_str().unwrap();
            let found = office_document(url);
            if !case["office"].as_bool().unwrap() {
                assert_eq!(found, None, "{url}");
                continue;
            }
            let found = found.unwrap_or_else(|| panic!("not recognised: {url}"));
            assert_eq!(
                kind_name(found.kind),
                case["kind"].as_str().unwrap(),
                "{url}"
            );
            assert_eq!(found.file.as_deref(), case["file"].as_str(), "{url}");
            assert_eq!(
                found.sourcedoc.as_deref(),
                case["sourcedoc"].as_str(),
                "{url}"
            );
            assert_eq!(found.url, case["document_url"].as_str().unwrap(), "{url}");
        }
    }

    #[test]
    fn kind_comes_from_the_share_segment_before_the_file_extension() {
        let found = office_document(
            "https://contoso.sharepoint.com/:w:/r/sites/s/_layouts/15/Doc.aspx?file=X.pptx",
        )
        .unwrap();
        assert_eq!(found.kind, OfficeKind::Word);
        let found = office_document(
            "https://contoso.sharepoint.com/sites/s/_layouts/15/Doc.aspx?file=Book.XLSM",
        )
        .unwrap();
        assert_eq!(found.kind, OfficeKind::Excel);
        let found = office_document(
            "https://contoso.sharepoint.com/sites/s/_layouts/15/Doc.aspx?file=a.txt",
        )
        .unwrap();
        assert_eq!(found.kind, OfficeKind::Unknown);
    }

    #[test]
    fn document_address_keeps_only_sourcedoc_and_file() {
        let found = office_document(SAMPLE).unwrap();
        assert_eq!(
            found.url,
            "https://contoso-my.sharepoint.com/personal/u_contoso_com/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=X.pptx"
        );
        // Order is sourcedoc, then file, whatever the page's order; the
        // fragment and any credentials go.
        let found = office_document(
            "https://me:pw@contoso.sharepoint.com/sites/s/_layouts/15/Doc.aspx?wdOrigin=a&file=B.docx&x=1&sourcedoc=%7BAAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE%7D#top",
        )
        .unwrap();
        assert_eq!(
            found.url,
            "https://contoso.sharepoint.com/sites/s/_layouts/15/Doc.aspx?sourcedoc=%7BAAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE%7D&file=B.docx"
        );
        for bad in [' ', '\n', '\r', '\t'] {
            assert!(!found.url.contains(bad));
        }
    }

    #[test]
    fn file_name_is_one_bounded_plain_line() {
        let long = "a".repeat(400);
        let found = office_document(&format!(
            "https://contoso.sharepoint.com/sites/s/_layouts/15/Doc.aspx?file=%0D%0A{long}.pptx"
        ))
        .unwrap();
        let file = found.file.unwrap();
        assert_eq!(file.chars().count(), MAX_FILE_NAME_CHARS);
        assert!(!file.contains('\n') && !file.contains('\r'));
        // A name that flattens to nothing is no name, but the page is still
        // recognised by its sourcedoc.
        let found = office_document(
            "https://contoso.sharepoint.com/sites/s/_layouts/15/Doc.aspx?file=%E2%80%8B&sourcedoc=%7BAAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE%7D",
        )
        .unwrap();
        assert_eq!(found.file, None);
    }

    #[test]
    fn kind_labels_are_fixed() {
        assert_eq!(OfficeKind::Word.label(), "Word document");
        assert_eq!(OfficeKind::Excel.label(), "Excel workbook");
        assert_eq!(OfficeKind::PowerPoint.label(), "PowerPoint presentation");
        assert_eq!(OfficeKind::Unknown.label(), "Office document");
    }
}

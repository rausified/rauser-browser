//! The host's own section in a page run's `page.md` (DESIGN.md §4.5): an
//! Office document or a Miro board recognised from the page's address, whose
//! content page extraction rarely reads. The run record's `office_section`
//! flag (named before Miro boards joined) says that `page.md` was written
//! with this section, so a page cannot have forged it; only the kind
//! reaches a prompt, as a fixed paragraph.

use super::miro::{MiroBoard, miro_board};
use super::office::{OfficeDocument, office_document};

/// What the host's section in `page.md` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageSection {
    Office(OfficeDocument),
    Miro(MiroBoard),
}

/// Which fixed prompt paragraph a page run gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    Office,
    Miro,
}

impl PageSection {
    /// The section `url` gets, if any. Office documents are recognised
    /// first; the two never overlap (different hosts).
    pub fn for_url(url: &str) -> Option<Self> {
        office_document(url)
            .map(Self::Office)
            .or_else(|| miro_board(url).map(Self::Miro))
    }

    pub fn kind(&self) -> SectionKind {
        match self {
            Self::Office(_) => SectionKind::Office,
            Self::Miro(_) => SectionKind::Miro,
        }
    }

    pub fn office(&self) -> Option<&OfficeDocument> {
        match self {
            Self::Office(document) => Some(document),
            Self::Miro(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_and_miro_pages_get_their_own_section_and_others_none() {
        let office = PageSection::for_url(
            "https://acme.sharepoint.com/:w:/r/sites/x/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=A.docx",
        )
        .unwrap();
        assert_eq!(office.kind(), SectionKind::Office);
        assert!(office.office().is_some());
        let miro = PageSection::for_url("https://miro.com/app/board/uXjVM0uuBLo=/").unwrap();
        assert_eq!(miro.kind(), SectionKind::Miro);
        assert!(miro.office().is_none());
        assert_eq!(PageSection::for_url("https://example.com/"), None);
        assert_eq!(
            PageSection::for_url("https://miro.com/app/dashboard/"),
            None
        );
    }
}

//! Recognising a Miro board from a page URL (DESIGN.md §4.5 "Miro boards").
//! Miro draws a board on a canvas, so page extraction reads only Miro's own
//! interface text. The result is page data, as for an Office document: it
//! comes from the page's address and only ever goes into `page.md`; no
//! string from here reaches a prompt or a log line.

use adapter_templates::CType;
use url::Url;

/// A Miro board a page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MiroBoard {
    /// The board ID, as matched: 12 token characters ending in base64
    /// padding, such as `uXjVQ7rTn2c=` (docs/adapters/miro.board.md).
    pub board_id: String,
    /// The board's canonical address, `https://miro.com/app/board/<id>/`.
    pub url: String,
}

/// The Miro board `url` names, or `None` for any other page.
///
/// Recognised: `https://miro.com/app/board/<id>` with an optional trailing
/// slash and any query or fragment, where `<id>` is a `token(12)` value
/// (the `miro.board` starter's capture). Only the exact origin: no other
/// host, port or scheme, and nothing below the board path.
pub fn miro_board(url: &str) -> Option<MiroBoard> {
    let page = Url::parse(url).ok()?;
    if page.scheme() != "https"
        || page.host_str() != Some("miro.com")
        || page.port().is_some()
        || !page.username().is_empty()
        || page.password().is_some()
    {
        return None;
    }
    let mut segments: Vec<&str> = page.path_segments()?.collect();
    if segments.last() == Some(&"") {
        segments.pop();
    }
    let [app, board, raw] = segments.as_slice() else {
        return None;
    };
    if *app != "app" || *board != "board" {
        return None;
    }
    // A board ID holds only token characters and `=` padding, so `%3D` is
    // the one escape that can name a board; any other `%` fails the type.
    let decoded = raw.replace("%3D", "=").replace("%3d", "=");
    let board_id = CType::parse("token(12)").ok()?.canonical(&decoded)?;
    Some(MiroBoard {
        url: format!("https://miro.com/app/board/{board_id}/"),
        board_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn every_shared_vector_matches() {
        let vectors: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/miro-board-urls.json"))
                .unwrap();
        let cases = vectors["cases"].as_array().unwrap();
        assert!(cases.len() > 15);
        for case in cases {
            let url = case["url"].as_str().unwrap();
            let found = miro_board(url);
            if !case["board"].as_bool().unwrap() {
                assert_eq!(found, None, "{url}");
                continue;
            }
            let found = found.unwrap_or_else(|| panic!("not recognised: {url}"));
            assert_eq!(found.board_id, case["board_id"].as_str().unwrap(), "{url}");
            assert_eq!(found.url, case["board_url"].as_str().unwrap(), "{url}");
        }
    }
}

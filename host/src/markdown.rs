//! Markdown and YAML safety helpers shared by every host-written file
//! (DESIGN.md §5.2 step 6, §6.3). Single-line fields are escaped with
//! `inline`; multi-line agent text keeps its Markdown but has every construct
//! that can make a viewer fetch a remote resource turned into an inert code
//! span by `neutralize_remote`. Links follow the profile's link style.
//!
//! The neutralizer is a hand-written scanner over chars: the host has no
//! regex crate.

use std::path::{Component, Path};

/// A YAML scalar as a JSON string, which YAML reads as a double-quoted
/// scalar with the same escapes.
pub(crate) fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

/// One line of escaped Markdown, at most `limit` characters before escaping.
pub(crate) fn inline(value: &str, limit: usize) -> String {
    crate::presentation::markdown_line(value, limit)
}

/// The link syntax a profile writes. The neutral profile uses relative
/// Markdown links; the obsidian preset uses wikilinks.
// The summary writer (PR 4) is the first caller of the profile-dependent
// helpers.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkStyle {
    Relative,
    Wikilink,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn link_style(profile: &str) -> LinkStyle {
    if profile == "obsidian" {
        LinkStyle::Wikilink
    } else {
        LinkStyle::Relative
    }
}

/// A relative Markdown link destination from a file in `from_dir` to `to`,
/// both relative to the same root. Shared leading folders are skipped, the
/// rest of `from_dir` is climbed with `../`, and every segment of `to` is
/// percent-encoded except RFC 3986 unreserved characters.
pub(crate) fn relative_link(from_dir: &Path, to: &Path) -> String {
    let from = segments(from_dir);
    let to = segments(to);
    let shared = from
        .iter()
        .zip(&to)
        .take(to.len().saturating_sub(1))
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec!["..".to_owned(); from.len() - shared];
    parts.extend(to[shared..].iter().map(|segment| percent_encode(segment)));
    parts.join("/")
}

fn segments(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_owned()),
            _ => None,
        })
        .collect()
}

fn percent_encode(segment: &str) -> String {
    if segment == ".." {
        return segment.to_owned();
    }
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// An Obsidian wikilink that cannot close early, alias, or point at a
/// heading or block: `[ ] | # ^` and control characters are removed.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn wikilink(target: &str, label: Option<&str>) -> String {
    fn clean(value: &str) -> String {
        value
            .chars()
            .filter(|ch| !ch.is_control() && !matches!(ch, '[' | ']' | '|' | '#' | '^'))
            .collect::<String>()
            .trim()
            .to_owned()
    }
    let target = clean(target);
    match label.map(clean).filter(|label| !label.is_empty()) {
        Some(label) => format!("[[{target}|{label}]]"),
        None => format!("[[{target}]]"),
    }
}

/// Agent Markdown with every remote-resource construct made inert: images
/// (`![..](..)`, `![..][..]`, `![..]`), link reference definitions, and raw
/// HTML (`<` followed by a letter, `/` or `!`) become code spans, so the URL
/// stays visible but nothing is fetched. Fenced code blocks and existing
/// inline code spans are left alone.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn neutralize_remote(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<(char, usize)> = None;
    for (index, raw) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let (line, ending) = match raw.strip_suffix('\r') {
            Some(line) => (line, "\r"),
            None => (raw, ""),
        };
        match fence {
            Some(open) => {
                if closes_fence(line, open) {
                    fence = None;
                }
                out.push_str(line);
            }
            None => {
                if let Some(open) = opens_fence(line) {
                    fence = Some(open);
                    out.push_str(line);
                } else if let Some(indent) = reference_definition(line) {
                    out.push_str(&line[..indent]);
                    out.push_str(&code_span(&line[indent..]));
                } else {
                    neutralize_line(line, &mut out);
                }
            }
        }
        out.push_str(ending);
    }
    out
}

/// Up to three spaces of indentation, then the rest of the line.
fn block_start(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    (indent <= 3).then(|| (indent, &line[indent..]))
}

fn opens_fence(line: &str) -> Option<(char, usize)> {
    let (_, rest) = block_start(line)?;
    let marker = rest.chars().next().filter(|ch| matches!(ch, '`' | '~'))?;
    let length = rest.chars().take_while(|ch| *ch == marker).count();
    if length < 3 {
        return None;
    }
    // A backtick fence's info string cannot contain a backtick; such a line
    // is an inline code span instead.
    if marker == '`' && rest[length..].contains('`') {
        return None;
    }
    Some((marker, length))
}

fn closes_fence(line: &str, (marker, length): (char, usize)) -> bool {
    let Some((_, rest)) = block_start(line) else {
        return false;
    };
    let run = rest.chars().take_while(|ch| *ch == marker).count();
    run >= length && rest[run * marker.len_utf8()..].trim().is_empty()
}

/// The indentation of a link reference definition line (`[label]: dest`,
/// whitespace optional),
/// or `None`. Footnote definitions (`[^1]:`) are not link references.
fn reference_definition(line: &str) -> Option<usize> {
    let (indent, rest) = block_start(line)?;
    let label = rest.strip_prefix('[')?;
    if label.starts_with('^') {
        return None;
    }
    let close = closing_bracket(label)?;
    if close == 0 {
        return None;
    }
    // CommonMark allows no whitespace before the destination, so any
    // non-blank text after the colon counts.
    let after = label[close + 1..].strip_prefix(':')?;
    (!after.trim().is_empty()).then_some(indent)
}

/// The byte offset of the `]` closing a label that starts just after `[`,
/// honoring backslash escapes and nested brackets.
fn closing_bracket(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (offset, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '[' => depth += 1,
            ']' if depth == 0 => return Some(offset),
            ']' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The byte offset of the `)` closing a destination that starts just after
/// `(`, honoring backslash escapes and nested parentheses.
fn closing_paren(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (offset, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '(' => depth += 1,
            ')' if depth == 0 => return Some(offset),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn neutralize_line(line: &str, out: &mut String) {
    let mut at = 0;
    while at < line.len() {
        let rest = &line[at..];
        let ch = rest.chars().next().expect("non-empty rest");
        let taken = match ch {
            '\\' => {
                // An escaped character is literal text, never a construct.
                let next = rest[1..].chars().next().map_or(0, char::len_utf8);
                out.push_str(&rest[..1 + next]);
                1 + next
            }
            '`' => {
                let run = rest.chars().take_while(|ch| *ch == '`').count();
                let span = closing_backticks(&rest[run..], run).map_or(run, |end| run + end + run);
                out.push_str(&rest[..span]);
                span
            }
            '!' if rest[1..].starts_with('[') => match image_length(rest) {
                Some(length) => {
                    out.push_str(&code_span(&rest[..length]));
                    length
                }
                None => {
                    out.push('!');
                    1
                }
            },
            '<' if rest[1..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!')) =>
            {
                // A tag can continue onto the next line, so an unclosed one is
                // wrapped through the end of this line: its `<` is then inert.
                let length = rest.find('>').map_or(rest.len(), |end| end + 1);
                out.push_str(&code_span(&rest[..length]));
                length
            }
            _ => {
                out.push(ch);
                ch.len_utf8()
            }
        };
        at += taken;
    }
}

/// The offset in `text` of a backtick run of exactly `length`, closing a
/// code span.
fn closing_backticks(text: &str, length: usize) -> Option<usize> {
    let mut offset = 0;
    while offset < text.len() {
        let rest = &text[offset..];
        let run = rest.chars().take_while(|ch| *ch == '`').count();
        if run == length {
            return Some(offset);
        }
        if run > 0 {
            offset += run;
        } else {
            offset += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    None
}

/// The byte length of an image construct at the start of `text` (which
/// starts with `![`): inline `![..](..)`, full or collapsed reference
/// `![..][..]`, or shortcut `![..]`.
fn image_length(text: &str) -> Option<usize> {
    let label_end = 2 + closing_bracket(&text[2..])?;
    let after = &text[label_end + 1..];
    let extra = if let Some(destination) = after.strip_prefix('(') {
        closing_paren(destination).map_or(0, |end| end + 2)
    } else if let Some(reference) = after.strip_prefix('[') {
        closing_bracket(reference).map_or(0, |end| end + 2)
    } else {
        0
    };
    Some(label_end + 1 + extra)
}

/// `content` as an inline code span whose fence is one backtick longer than
/// the longest backtick run inside, padded when the content starts or ends
/// with a backtick.
fn code_span(content: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in content.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    let pad = if content.starts_with('`') || content.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{content}{pad}{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_remote_construct_becomes_an_inert_code_span_with_its_url_visible() {
        for (input, expected) in [
            (
                "see ![x](https://e.example/a.png) now",
                "see `![x](https://e.example/a.png)` now",
            ),
            (
                "![x](https://e.example/a.png \"t\")",
                "`![x](https://e.example/a.png \"t\")`",
            ),
            (
                "![x][r]\n\n[r]: https://e.example/b.png",
                "`![x][r]`\n\n`[r]: https://e.example/b.png`",
            ),
            ("![x][]", "`![x][]`"),
            ("![x]", "`![x]`"),
            (
                "<img src=https://e.example/c.png>",
                "`<img src=https://e.example/c.png>`",
            ),
            ("<IFRAME src=x></IFRAME>", "`<IFRAME src=x>``</IFRAME>`"),
        ] {
            assert_eq!(neutralize_remote(input), expected);
        }
        for tag in [
            "video", "audio", "object", "embed", "link", "style", "svg", "picture", "source",
        ] {
            assert!(neutralize_remote(&format!("<{tag} src=u>")).starts_with('`'));
        }
    }

    #[test]
    fn backticks_inside_a_construct_get_a_longer_fence() {
        assert_eq!(neutralize_remote("![`a`](u)"), "``![`a`](u)``");
    }

    #[test]
    fn ordinary_markdown_is_left_alone() {
        for text in [
            "[a](https://x.example)",
            "a < b and c > d",
            "```\n<img src=u>\n```",
            "`<img>`",
        ] {
            assert_eq!(neutralize_remote(text), text);
        }
    }

    #[test]
    fn relative_links_climb_and_percent_encode() {
        assert_eq!(
            relative_link(Path::new("summaries"), Path::new("pages/a b.md")),
            "../pages/a%20b.md"
        );
        assert_eq!(
            relative_link(
                Path::new("log/2026/10"),
                Path::new("summaries/x.summary.md")
            ),
            "../../../summaries/x.summary.md"
        );
    }

    #[test]
    fn the_obsidian_profile_writes_wikilinks_and_others_relative_links() {
        assert_eq!(link_style("obsidian"), LinkStyle::Wikilink);
        assert_eq!(link_style("neutral"), LinkStyle::Relative);
    }

    #[test]
    fn link_reference_definitions_and_unclosed_tags_are_wrapped() {
        assert_eq!(
            neutralize_remote("  [r]:https://e.example/b.png"),
            "  `[r]:https://e.example/b.png`"
        );
        assert_eq!(neutralize_remote("[^1]: a note"), "[^1]: a note");
        assert_eq!(
            neutralize_remote("<img\nsrc=https://e.example/c.png>"),
            "`<img`\nsrc=https://e.example/c.png>"
        );
        assert_eq!(neutralize_remote("\\![x](u) \\<img>"), "\\![x](u) \\<img>");
    }

    #[test]
    fn wikilinks_cannot_break_out() {
        assert_eq!(wikilink("a]]|b#^", None), "[[ab]]");
    }
}

//! Markdown and YAML safety helpers shared by every host-written file
//! (DESIGN.md §5.2 step 6, §6.3). Single-line fields are escaped with
//! `inline`; multi-line agent text keeps its Markdown but has every construct
//! that can make a viewer fetch a remote resource turned into an inert code
//! span by `neutralize_remote`. Links follow the profile's link style.
//!
//! The neutralizer is a hand-written scanner over chars: the host has no
//! regex crate.

use std::collections::HashMap;
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
/// HTML (`<` followed by a letter, `/`, `!` or `?`) become code spans, so the
/// URL stays visible but nothing is fetched. URI autolinks fetch nothing and
/// stay links. Column-0 fenced code blocks and inline code spans that close
/// on their own line are left alone.
///
/// The scan is per line, so whatever could pair across lines in a viewer is
/// made literal instead: a backtick run with no closer on its line is
/// backslash-escaped, an image label that does not close on its line keeps an
/// escaped `!`, and a link destination or title (raw text in a viewer, so a
/// backtick there is no fence) is escaped through its end, or through the end
/// of the paragraph when it does not end on its line. A generated span never
/// touches another backtick. The trade-offs are cosmetic: a multi-line code
/// span shows its backticks, an indented code block or an indented fence
/// gains visible backticks around a tag, and email autolinks are wrapped.
///
/// Every search is answered from per-line tables, so the scan is linear.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn neutralize_remote(text: &str) -> String {
    let mut scan = Scan {
        out: String::with_capacity(text.len() + text.len() / 8),
        fence: None,
        trust_fences: true,
        escape_paragraph: false,
    };
    for (line, ending) in lines(text) {
        scan.line(line);
        scan.out.push_str(ending);
    }
    scan.out
}

/// Lines with their endings; CommonMark ends a line at `\r\n`, `\r` or `\n`.
fn lines(text: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = text;
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        match rest.find(['\r', '\n']) {
            Some(end) => {
                let length = if rest[end..].starts_with("\r\n") {
                    2
                } else {
                    1
                };
                let item = (&rest[..end], &rest[end..end + length]);
                rest = &rest[end + length..];
                Some(item)
            }
            None => {
                done = true;
                Some((rest, ""))
            }
        }
    })
}

#[derive(Clone, Copy)]
struct Fence {
    marker: char,
    length: usize,
    indent: usize,
}

struct Scan {
    out: String,
    fence: Option<Fence>,
    /// False once an indented fence may have ended before its closer (a list
    /// item's fence ends with the item). A later fence marker could then be
    /// the viewer's closer, so no later line is hidden as code.
    trust_fences: bool,
    /// A link destination or title is still open: escape until the
    /// paragraph ends.
    escape_paragraph: bool,
}

impl Scan {
    fn line(&mut self, line: &str) {
        if let Some(open) = self.fence {
            if closes_fence(line, open) {
                self.fence = None;
                self.out.push_str(line);
                return;
            }
            if open.indent == 0 {
                self.out.push_str(line);
                return;
            }
            // Only a column-0 fence hides its lines. An indented one may
            // belong to a list item and end with it, so its lines are
            // neutralized like any other.
            if !line.trim().is_empty() && leading_spaces(line) < open.indent {
                self.fence = None;
                self.trust_fences = false;
            } else {
                self.text_line(line);
                return;
            }
        }
        if let Some(open) = opens_fence(line).filter(|_| self.trust_fences) {
            self.fence = Some(open);
            self.escape_paragraph = false;
            self.out.push_str(line);
            return;
        }
        self.text_line(line);
    }

    fn text_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            self.escape_paragraph = false;
            self.out.push_str(line);
        } else if self.escape_paragraph {
            escape_text(line, &mut self.out);
        } else if let Some(indent) = reference_definition(line) {
            self.out.push_str(&line[..indent]);
            push_code(&mut self.out, &code_span(&line[indent..]));
        } else {
            neutralize_line(line, &mut self.out, &mut self.escape_paragraph);
        }
    }
}

fn leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Up to three spaces of indentation, then the rest of the line.
fn block_start(line: &str) -> Option<(usize, &str)> {
    let indent = leading_spaces(line);
    (indent <= 3).then(|| (indent, &line[indent..]))
}

fn opens_fence(line: &str) -> Option<Fence> {
    let (indent, rest) = block_start(line)?;
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
    Some(Fence {
        marker,
        length,
        indent,
    })
}

fn closes_fence(line: &str, open: Fence) -> bool {
    let Some((_, rest)) = block_start(line) else {
        return false;
    };
    let run = rest.chars().take_while(|ch| *ch == open.marker).count();
    run >= open.length && rest[run * open.marker.len_utf8()..].trim().is_empty()
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

/// Per-line answers to the scanner's forward searches, built in one pass so
/// no position is rescanned.
struct LineTables {
    /// Unescaped `[` offset to its matching `]`, nesting honored.
    brackets: HashMap<usize, usize>,
    /// Unescaped `(` offset to its matching `)`, nesting honored.
    parens: HashMap<usize, usize>,
    /// Backtick run length to the start offsets of runs of that length.
    runs: HashMap<usize, Vec<usize>>,
}

impl LineTables {
    fn new(line: &str) -> Self {
        let mut tables = Self {
            brackets: HashMap::new(),
            parens: HashMap::new(),
            runs: HashMap::new(),
        };
        let (mut open_brackets, mut open_parens) = (Vec::new(), Vec::new());
        let mut escaped = false;
        let mut run: Option<(usize, usize)> = None;
        for (offset, ch) in line.char_indices() {
            if ch == '`' {
                run = Some(run.map_or((offset, 1), |(start, length)| (start, length + 1)));
            } else if let Some((start, length)) = run.take() {
                tables.runs.entry(length).or_default().push(start);
            }
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '[' => open_brackets.push(offset),
                ']' => {
                    if let Some(open) = open_brackets.pop() {
                        tables.brackets.insert(open, offset);
                    }
                }
                '(' => open_parens.push(offset),
                ')' => {
                    if let Some(open) = open_parens.pop() {
                        tables.parens.insert(open, offset);
                    }
                }
                _ => {}
            }
        }
        if let Some((start, length)) = run {
            tables.runs.entry(length).or_default().push(start);
        }
        tables
    }

    /// The offset of the first backtick run of exactly `length` starting at
    /// or after `from`: the closer of a code span.
    fn closing_run(&self, length: usize, from: usize) -> Option<usize> {
        let starts = self.runs.get(&length)?;
        starts
            .get(starts.partition_point(|start| *start < from))
            .copied()
    }

    /// The byte length of an image construct at `at` (which starts with
    /// `![`): inline `![..](..)`, full or collapsed reference `![..][..]`, or
    /// shortcut `![..]`. `None` when the label does not close on the line.
    fn image_length(&self, line: &str, at: usize) -> Option<usize> {
        let label_end = *self.brackets.get(&(at + 1))?;
        let next = label_end + 1;
        let end = match line[next..].chars().next() {
            Some('(') => self.parens.get(&next).map_or(label_end, |end| *end),
            Some('[') => self.brackets.get(&next).map_or(label_end, |end| *end),
            _ => label_end,
        };
        Some(end + 1 - at)
    }
}

/// Whether `ch` after `<` can open raw HTML: a tag, closing tag,
/// declaration, comment, CDATA section, or processing instruction.
fn opens_tag(ch: char) -> bool {
    ch.is_ascii_alphabetic() || matches!(ch, '/' | '!' | '?')
}

/// The byte length of a URI autolink (`<scheme:...>`) at the start of
/// `text`. One holding a backtick is not counted, so a fence can never hide
/// inside it.
fn autolink_length(text: &str) -> Option<usize> {
    let body = text.strip_prefix('<')?;
    let colon = body.find(':')?;
    let scheme = &body[..colon];
    if !(2..=32).contains(&scheme.len())
        || !scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
        || !scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-'))
    {
        return None;
    }
    for (offset, ch) in body[colon + 1..].char_indices() {
        match ch {
            '>' => return Some(1 + colon + 1 + offset + 1),
            '<' | '`' | ' ' => return None,
            ch if ch.is_ascii_control() => return None,
            _ => {}
        }
    }
    None
}

/// The byte length of an inline link's destination and title after its `(`,
/// through the closing `)`, when they end on this line. `None` when they do
/// not, or when they do not parse; either way the caller escapes onward.
fn link_tail(text: &str) -> Option<usize> {
    let skip_space = |from: usize| {
        from + text[from..].len() - text[from..].trim_start_matches([' ', '\t']).len()
    };
    let chars = |from: usize| {
        text[from..]
            .char_indices()
            .map(move |(o, ch)| (from + o, ch))
    };
    let mut at = skip_space(0);
    match text[at..].chars().next()? {
        ')' => return Some(at + 1),
        '<' => {
            let mut escaped = false;
            let mut end = None;
            for (offset, ch) in chars(at + 1) {
                match ch {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '<' => return None,
                    '>' => {
                        end = Some(offset + 1);
                        break;
                    }
                    _ => {}
                }
            }
            at = end?;
        }
        _ => {
            let (mut depth, mut escaped) = (0usize, false);
            let mut end = text.len();
            for (offset, ch) in chars(at) {
                match ch {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    ' ' | '\t' => {
                        end = offset;
                        break;
                    }
                    ch if ch.is_ascii_control() => return None,
                    '(' => depth += 1,
                    ')' if depth == 0 => {
                        end = offset;
                        break;
                    }
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            if depth != 0 {
                return None;
            }
            at = end;
        }
    }
    let after_destination = at;
    at = skip_space(at);
    let next = text[at..].chars().next()?;
    if next == ')' {
        return Some(at + 1);
    }
    let close = match next {
        '"' => '"',
        '\'' => '\'',
        '(' => ')',
        _ => return None,
    };
    if at == after_destination {
        return None;
    }
    let mut escaped = false;
    let mut end = None;
    for (offset, ch) in chars(at + 1) {
        match ch {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            ch if ch == close => {
                end = Some(offset + 1);
                break;
            }
            '(' if next == '(' => return None,
            _ => {}
        }
    }
    at = skip_space(end?);
    text[at..].starts_with(')').then_some(at + 1)
}

/// The byte length of a reference label after its `[`, through the `]`,
/// when it ends on this line within CommonMark's 999-character limit.
fn reference_label(text: &str) -> Option<usize> {
    let mut escaped = false;
    for (offset, ch) in text.char_indices().take(1000) {
        match ch {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '[' => return None,
            ']' => return Some(offset + 1),
            _ => {}
        }
    }
    None
}

/// `text` with every character that could open a code span, an image, or
/// raw HTML backslash-escaped. Escapes are decoded inside link destinations
/// and titles, so a link that forms keeps its meaning.
fn escape_text(text: &str, out: &mut String) {
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let ch = rest.chars().next().expect("non-empty rest");
        let next = rest[ch.len_utf8()..].chars().next();
        let taken = match ch {
            '\\' => {
                let length = 1 + next.map_or(0, char::len_utf8);
                out.push_str(&rest[..length]);
                length
            }
            '`' => {
                out.push_str("\\`");
                1
            }
            '!' if next == Some('[') => {
                out.push_str("\\!");
                1
            }
            '<' if next.is_some_and(opens_tag) => match autolink_length(rest) {
                Some(length) => {
                    out.push_str(&rest[..length]);
                    length
                }
                None => {
                    out.push_str("\\<");
                    1
                }
            },
            _ => {
                out.push(ch);
                ch.len_utf8()
            }
        };
        at += taken;
    }
}

/// Appends text that starts with a backtick fence, a space away from any
/// backtick already at the end of `out` so the two runs cannot merge.
fn push_code(out: &mut String, code: &str) {
    if out.ends_with('`') && code.starts_with('`') {
        out.push(' ');
    }
    out.push_str(code);
}

fn neutralize_line(line: &str, out: &mut String, escape: &mut bool) {
    let tables = LineTables::new(line);
    // Adjacent constructs share one span.
    let mut pending = String::new();
    let flush = |pending: &mut String, out: &mut String| {
        if !pending.is_empty() {
            push_code(out, &code_span(pending));
            pending.clear();
        }
    };
    let mut at = 0;
    while at < line.len() {
        let rest = &line[at..];
        let ch = rest.chars().next().expect("non-empty rest");
        let next = rest[ch.len_utf8()..].chars().next();
        match ch {
            '!' if next == Some('[') => {
                if let Some(length) = tables.image_length(line, at) {
                    pending.push_str(&rest[..length]);
                    at += length;
                    continue;
                }
            }
            '<' if next.is_some_and(opens_tag) && autolink_length(rest).is_none() => {
                // A tag can continue onto the next line, so an unclosed one is
                // wrapped through the end of this line: its `<` is then inert.
                let length = rest.find('>').map_or(rest.len(), |end| end + 1);
                pending.push_str(&rest[..length]);
                at += length;
                continue;
            }
            _ => {}
        }
        flush(&mut pending, out);
        at += match ch {
            '\\' => {
                // An escaped character is literal text, never a construct.
                let length = 1 + next.map_or(0, char::len_utf8);
                out.push_str(&rest[..length]);
                length
            }
            '`' => {
                let run = rest.chars().take_while(|ch| *ch == '`').count();
                match tables.closing_run(run, at + run) {
                    Some(close) => {
                        push_code(out, &line[at..close + run]);
                        close + run - at
                    }
                    None => {
                        // No closer on this line: a viewer could pair it with
                        // a later line's backticks, so it is made literal.
                        out.push_str(&"\\`".repeat(run));
                        run
                    }
                }
            }
            // Only reached when the label does not close on this line.
            '!' if next == Some('[') => {
                out.push_str("\\!");
                1
            }
            '<' if next.is_some_and(opens_tag) => {
                let length = autolink_length(rest).expect("only autolinks reach here");
                out.push_str(&rest[..length]);
                length
            }
            ']' if matches!(next, Some('(' | '[')) => {
                out.push_str(&rest[..2]);
                let tail = &rest[2..];
                let length = if next == Some('(') {
                    link_tail(tail)
                } else {
                    reference_label(tail)
                };
                match length {
                    Some(length) => {
                        escape_text(&tail[..length], out);
                        2 + length
                    }
                    None => {
                        *escape = true;
                        escape_text(tail, out);
                        rest.len()
                    }
                }
            }
            _ => {
                out.push(ch);
                ch.len_utf8()
            }
        };
    }
    flush(&mut pending, out);
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
            // Adjacent constructs share one span.
            ("<IFRAME src=x></IFRAME>", "`<IFRAME src=x></IFRAME>`"),
            // A literal backtick next to a construct is escaped, or kept a
            // space away, so it cannot join the span's fence.
            (
                "`<img src=https://e.example/c.png>",
                "\\` `<img src=https://e.example/c.png>`",
            ),
            (
                "\\`<img src=https://e.example/c.png>`",
                "\\` `<img src=https://e.example/c.png>`\\`",
            ),
            // An image label that does not close on its line keeps its `!`
            // escaped; the rest becomes an ordinary link.
            (
                "![x\ny](https://e.example/a.png)",
                "\\![x\ny](https://e.example/a.png)",
            ),
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

    /// Panics if a raw-HTML or image opener survives outside a code span.
    /// Code spans are matched over whole paragraphs, as CommonMark does, so
    /// a span the neutralizer meant to close on one line but that a viewer
    /// pairs differently is caught. Fences and link titles are not modelled;
    /// those cases have hand-checked expectations below.
    fn assert_inert(output: &str) {
        let normalized = output.replace("\r\n", "\n").replace('\r', "\n");
        for paragraph in normalized.split("\n\n") {
            let chars: Vec<char> = paragraph.chars().collect();
            let run_at = |at: usize| chars[at..].iter().take_while(|ch| **ch == '`').count();
            let mut at = 0;
            while at < chars.len() {
                match chars[at] {
                    '\\' if chars.get(at + 1).is_some_and(char::is_ascii_punctuation) => at += 2,
                    '`' => {
                        let run = run_at(at);
                        let mut close = at + run;
                        let mut found = None;
                        while close < chars.len() {
                            if chars[close] == '`' {
                                let length = run_at(close);
                                if length == run {
                                    found = Some(close + length);
                                    break;
                                }
                                close += length;
                            } else {
                                close += 1;
                            }
                        }
                        at = found.unwrap_or(at + run);
                    }
                    '<' if chars.get(at + 1).is_some_and(|next| {
                        next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?')
                    }) =>
                    {
                        let rest: String = chars[at..].iter().collect();
                        let length = autolink_length(&rest)
                            .unwrap_or_else(|| panic!("live tag in {output:?}"));
                        at += rest[..length].chars().count();
                    }
                    '!' if chars.get(at + 1) == Some(&'[') => panic!("live image in {output:?}"),
                    _ => at += 1,
                }
            }
        }
    }

    #[test]
    fn adversarial_inputs_leave_nothing_live() {
        for input in [
            "`<img src=https://e.example/c.png>",
            "\\`<img src=https://e.example/c.png>`",
            "`x`![a](https://e/a.png)`",
            "![a](https://e/a.png)`",
            "x`![a](https://e/a.png)``",
            "<a><b>`",
            "`<a><b>``",
            "``<img src=u>`",
            "![x\ny](https://e.example/a.png)",
            "![a\nb](https://e/a.png)",
            "`foo\n<img src=https://e.example/c.png>`",
            "`a\n![x](https://e/a.png)\nb`",
            "<img src=a>`x`<img src=b>",
            "`x`<img src=a>`y`",
            "<a title='`'>`<img src=u>`",
            "![x](u ')`')<img src=y>`",
            "<IFRAME src=x></IFRAME>",
            "![a](u)![b](v)<img src=w>",
            "![[a]](u)",
            "[![a](u)](v)",
            "a\r<img src=u>\r`b",
        ] {
            assert_inert(&neutralize_remote(input));
        }
    }

    #[test]
    fn link_destinations_and_titles_cannot_swallow_a_fence() {
        // A link's destination and title are raw text in a viewer, so a
        // backtick there is not a code-span fence. Inside them backticks and
        // openers are backslash-escaped, which the viewer decodes back.
        for (input, expected) in [
            (
                "[a](u '`')<img src=https://e/x.png>`",
                "[a](u '\\`')`<img src=https://e/x.png>`\\`",
            ),
            (
                "[a](`)<img src=https://e/x.png>`",
                "[a](\\`)`<img src=https://e/x.png>`\\`",
            ),
            ("[a](<img src=x>)", "[a](\\<img src=x>)"),
            ("[a][`]<img src=x>`", "[a][\\`]`<img src=x>`\\`"),
            // A title still open at the end of the line keeps everything
            // escaped until the paragraph ends.
            (
                "[a](u \"\n`\")<img src=https://e/x.png>`\n\n`ok`",
                "[a](u \"\n\\`\")\\<img src=https://e/x.png>\\`\n\n`ok`",
            ),
            (
                "[a](https://x.example \"t\") and `code`",
                "[a](https://x.example \"t\") and `code`",
            ),
        ] {
            assert_eq!(neutralize_remote(input), expected, "{input:?}");
        }
    }

    #[test]
    fn only_a_column_zero_fence_hides_its_lines() {
        for (input, expected) in [
            // A list item's fence ends with the item; its lines are neutralized.
            (
                "- a\n\n  ```\n<img src=https://e.example/c.png>",
                "- a\n\n  ```\n`<img src=https://e.example/c.png>`",
            ),
            (
                "  ```\n  <img src=u>\n  ```",
                "  ```\n  `<img src=u>`\n  ```",
            ),
            // Once an indented fence may have ended early, no later fence is
            // trusted, since its marker could be the viewer's closer.
            (
                "1. a\n\n  ```\nx\n```\n<img src=y>",
                "1. a\n\n  ```\nx\n\\`\\`\\`\n`<img src=y>`",
            ),
            // A lone CR ends a line.
            (
                "```\n```\r<img src=https://e/a.png>",
                "```\n```\r`<img src=https://e/a.png>`",
            ),
            ("```\r<img src=u>\r```", "```\r<img src=u>\r```"),
        ] {
            assert_eq!(neutralize_remote(input), expected, "{input:?}");
        }
    }

    #[test]
    fn uri_autolinks_stay_links() {
        assert_eq!(
            neutralize_remote("see <https://x.example/a?b=c>"),
            "see <https://x.example/a?b=c>"
        );
        // Email autolinks are not recognized, so they are wrapped.
        assert_eq!(neutralize_remote("<a@b.example>"), "`<a@b.example>`");
    }

    #[test]
    fn pathological_inputs_scan_in_linear_time() {
        let started = std::time::Instant::now();
        for input in [
            "![".repeat(65_536),
            format!("{}]", "![".repeat(65_536)),
            "![a](".repeat(32_768),
            (1..400).map(|length| "`".repeat(length) + "x").collect(),
        ] {
            assert_inert(&neutralize_remote(&input));
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn wikilinks_cannot_break_out() {
        assert_eq!(wikilink("a]]|b#^", None), "[[ab]]");
    }
}

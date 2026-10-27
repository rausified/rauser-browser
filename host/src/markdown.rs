//! Markdown and YAML safety helpers shared by every host-written file
//! (DESIGN.md §5.2 step 6, §6.3). Single-line fields are escaped with
//! `inline`; multi-line agent text keeps most of its Markdown, with `<`,
//! image `!`s, backticks, `$`, fence-length `~` runs and indentation that
//! would make code escaped by `neutralize_remote`, so no CommonMark, GFM or
//! Obsidian embed construct fetches a remote resource and no code span,
//! fence or math reaches Mermaid, Dataview or a math engine. Links follow
//! the profile's link style.

use std::path::{Component, Path};

/// A YAML scalar as a JSON string, which YAML reads as a double-quoted
/// scalar with the same escapes. `<` is written as the escape `\u003c`, and
/// a `!` before `[` as `\u0021`, so a YAML reader (Obsidian's properties,
/// the ownership check) gets the value unchanged while the raw text holds no
/// `<` and no `![`. A viewer without front matter support renders the block
/// as Markdown body text, and there the same guarantee as
/// `neutralize_remote` holds.
pub(crate) fn yaml_string(value: &str) -> String {
    let json = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into());
    // JSON writes `<` and `!` only as themselves, and only inside the string.
    json.replace('<', "\\u003c").replace("![", "\\u0021[")
}

/// One line of escaped Markdown, at most `limit` characters before escaping.
/// On top of `markdown_line`'s backslash escapes, every backtick, `$` and `~`
/// is written as a numeric entity, as in `neutralize_remote`, so no code span
/// (a Dataview inline query), math, or fence can form wherever the line is
/// placed, at the start of a line or a list item included. Each space from
/// the fourth of a run on is written as `&#32;`, so a line placed at a line
/// start, such as `-     x`, cannot make an indented code block in a list
/// item (`markdown_line` has already turned tabs into spaces and trimmed
/// both ends). Since `markdown_line` escapes every backslash, the backslash
/// it puts before a backtick is dropped for the entity.
pub(crate) fn inline(value: &str, limit: usize) -> String {
    let line = crate::presentation::markdown_line(value, limit);
    let mut out = String::with_capacity(line.len() + line.len() / 4);
    let mut spaces = 0usize;
    for ch in line.chars() {
        spaces = if ch == ' ' { spaces + 1 } else { 0 };
        match ch {
            ' ' if spaces > 3 => out.push_str("&#32;"),
            '`' => {
                out.pop();
                out.push_str("&#96;");
            }
            '$' => out.push_str("&#36;"),
            '~' => out.push_str("&#126;"),
            _ => out.push(ch),
        }
    }
    out
}

/// The link syntax a profile writes. The neutral profile uses relative
/// Markdown links; the obsidian preset uses wikilinks.
// The summary writer (PR 4) is the first caller of the profile-dependent
// helpers.
#[cfg_attr(not(any(test, unix)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkStyle {
    Relative,
    Wikilink,
}

#[cfg_attr(not(any(test, unix)), allow(dead_code))]
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

/// The longest label written as plain text in place of a target-less
/// wikilink.
const MAX_WIKILINK_LABEL_CHARS: usize = 200;

/// An Obsidian wikilink that cannot close early, alias, or point at a
/// heading or block: `[ ] | # ^` and control characters are removed. A
/// viewer without wikilinks (VS Code preview) parses the inside as Markdown,
/// so `<` and `>` are removed too, which no Obsidian file name can contain,
/// and so are backticks and `$`, so no code span (a Dataview inline query in
/// Obsidian) or math forms there either. Entities are not used, since
/// Obsidian would not decode them in a link target. A `~` cannot open a fence
/// after `[[`, so it stays. When nothing of the target is left, the label is
/// written as escaped plain text instead (`Untitled` without one).
#[cfg_attr(not(any(test, unix)), allow(dead_code))]
pub(crate) fn wikilink(target: &str, label: Option<&str>) -> String {
    fn clean(value: &str) -> String {
        value
            .chars()
            .filter(|ch| {
                !ch.is_control()
                    && !matches!(ch, '[' | ']' | '|' | '#' | '^' | '<' | '>' | '`' | '$')
            })
            .collect::<String>()
            .trim()
            .to_owned()
    }
    let target = clean(target);
    if target.is_empty() {
        // An empty target would render as a broken or self link.
        return inline(label.unwrap_or(""), MAX_WIKILINK_LABEL_CHARS);
    }
    match label.map(clean).filter(|label| !label.is_empty()) {
        Some(label) => format!("[[{target}|{label}]]"),
        None => format!("[[{target}]]"),
    }
}

/// Agent Markdown with nothing that can make a viewer fetch a remote
/// resource or hand text to another engine, by a context-free escape:
///
/// - every `<` becomes `&lt;`. An entity is only ever text in CommonMark and
///   GFM, so no tag, autolink, comment, CDATA section, processing
///   instruction or declaration can form, in any context;
/// - every `!` directly before `[` gets a backslash (`\!`), unless an odd run
///   of backslashes already escapes it. An escaped `!` cannot start an image,
///   so inline, full, collapsed and shortcut reference images and Obsidian
///   `![[embeds]]` cannot form;
/// - every backtick becomes `&#96;`, every `$` `&#36;`, every tab `&#9;`,
///   and every form feed and vertical tab `&#32;`; every `~` in a run of
///   three or more becomes `&#126;`, and every space from the fourth of a run
///   on becomes `&#32;`. So no code span,
///   fenced block, indented code block or math can form, in any container.
///   Obsidian hands those to other engines: Mermaid (whose v11
///   `@{ img: ... }` loads a remote image), Dataview (`= ...` and `$= ...`
///   inline queries in code spans, also run inside code blocks by default,
///   and `dataview`/`dataviewjs` fences), math and plugin blocks. Runs of
///   one or two `~` stay, so strikethrough still renders. A backslash that
///   escapes a backtick, `$` or `~` written as an entity is dropped, since
///   the entity already shows the plain character, and one before
///   whitespace written as an entity is doubled, since it would otherwise
///   escape the entity's `&`; numeric entities are used rather than
///   backslashes because agent backslashes cannot undo them;
/// - a `!` at the very end is escaped as `\!`, a trailing run of `~` or of
///   spaces is written as entities whatever its length, and a trailing odd
///   run of backslashes gets one more, so the output can be joined with
///   another neutralized fragment or with host text without re-forming `![`,
///   a fence, or indented code, or cancelling a following `\!`. Text written
///   directly before the output must likewise not end in an unescaped `!`,
///   an odd run of backslashes, a `~`, a space or a tab; both
///   `neutralize_remote` and `inline` output satisfy that;
/// - nothing else changes. Links, emphasis, strikethrough, lists indented
///   up to three spaces, and tables still render.
///
/// The escape does not depend on where a character falls, so it needs no
/// model of a viewer's parser. An earlier scanner turned each construct into
/// a code span, which meant mirroring how each viewer finds fences, list
/// items, table cells, code spans, link titles and autolink literals; review
/// kept finding inputs where a real renderer paired things differently and
/// rendered a tag live. Agent prose loses code formatting by design: code
/// shows as plain text with its backticks, and a list nested four or more
/// spaces deep, or by tabs, is flattened into its parent's text
/// (DESIGN.md §5.2 step 6).
#[cfg_attr(not(any(test, unix)), allow(dead_code))]
pub(crate) fn neutralize_remote(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut backslashes = 0usize;
    let mut at = 0;
    while at < chars.len() {
        let ch = chars[at];
        let escaped = !backslashes.is_multiple_of(2);
        if matches!(ch, '~' | ' ') {
            let length = chars[at..].iter().take_while(|next| **next == ch).count();
            let at_end = at + length == chars.len();
            for position in 0..length {
                let entity = match ch {
                    '~' if at_end || length >= 3 => "&#126;",
                    ' ' if at_end || position >= 3 => "&#32;",
                    _ => {
                        out.push(ch);
                        continue;
                    }
                };
                // An escaped `~` loses its backslash. A backslash before a
                // space is literal, but before the entity it would escape the
                // `&`, so it is doubled to stay visible.
                if position == 0 && escaped {
                    if ch == '~' {
                        out.pop();
                    } else {
                        out.push('\\');
                    }
                }
                out.push_str(entity);
            }
            at += length;
            backslashes = 0;
            continue;
        }
        match ch {
            '<' => out.push_str("&lt;"),
            // A `!` at the end is escaped too, so a `[` written after the
            // output cannot make an image of it.
            '!' if matches!(chars.get(at + 1), Some('[') | None) && !escaped => out.push_str("\\!"),
            '`' | '$' => {
                if escaped {
                    out.pop();
                }
                out.push_str(if ch == '`' { "&#96;" } else { "&#36;" });
            }
            // Form feed and vertical tab, which some renderers treat as blank
            // and others decode to U+FFFD, are written as spaces. As with a
            // trailing space, an escaping backslash is doubled.
            '\t' | '\u{b}' | '\u{c}' => {
                if escaped {
                    out.push('\\');
                }
                out.push_str(if ch == '\t' { "&#9;" } else { "&#32;" });
            }
            _ => out.push(ch),
        }
        backslashes = if ch == '\\' { backslashes + 1 } else { 0 };
        at += 1;
    }
    // A trailing odd run of backslashes would escape the first character
    // written after the output, such as the `\` of a following `\![`, so it
    // gets one more. A trailing backslash is literal either way.
    if !backslashes.is_multiple_of(2) {
        out.push('\\');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lt_becomes_an_entity_and_every_image_bang_is_escaped() {
        for (input, expected) in [
            // Images in every form, Obsidian embeds included.
            (
                "see ![x](https://e.example/a.png) now",
                "see \\![x](https://e.example/a.png) now",
            ),
            (
                "![x](https://e.example/a.png \"t\")",
                "\\![x](https://e.example/a.png \"t\")",
            ),
            (
                "![x][r]\n\n[r]: https://e.example/b.png",
                "\\![x][r]\n\n[r]: https://e.example/b.png",
            ),
            ("![x][]", "\\![x][]"),
            ("![x]", "\\![x]"),
            ("![x](<u>)", "\\![x](&lt;u>)"),
            (
                "![[https://e.example/x.png]]",
                "\\![[https://e.example/x.png]]",
            ),
            ("![`a`](u)", "\\![&#96;a&#96;](u)"),
            ("[![a](u)](v)", "[\\![a](u)](v)"),
            ("!![x](u)", "!\\![x](u)"),
            ("![[a]](u)", "\\![[a]](u)"),
            (
                "![a](u)![b](v)<img src=w>",
                "\\![a](u)\\![b](v)&lt;img src=w>",
            ),
            // Multi-line constructs.
            (
                "![x\ny](https://e.example/a.png)",
                "\\![x\ny](https://e.example/a.png)",
            ),
            (
                "<img\nsrc=https://e.example/c.png>",
                "&lt;img\nsrc=https://e.example/c.png>",
            ),
            ("`a\n![x](u)\nb`", "&#96;a\n\\![x](u)\nb&#96;"),
            // Tags in any case, comments, CDATA, processing instructions,
            // declarations and autolinks.
            ("<IFRAME src=x></IFRAME>", "&lt;IFRAME src=x>&lt;/IFRAME>"),
            ("<!-- c --><img src=u>", "&lt;!-- c -->&lt;img src=u>"),
            ("<!--\n<img src=u>\n-->", "&lt;!--\n&lt;img src=u>\n-->"),
            ("<![CDATA[<img src=u>]]>", "&lt;\\![CDATA[&lt;img src=u>]]>"),
            ("<?php x ?>", "&lt;?php x ?>"),
            ("<!DOCTYPE html>", "&lt;!DOCTYPE html>"),
            ("see <https://x.example/a>", "see &lt;https://x.example/a>"),
            ("<a@b.example>", "&lt;a@b.example>"),
            ("a < b and c > d", "a &lt; b and c > d"),
            // Next to backticks, in what would have been code or fences of
            // any indent, table cells and reference definitions, the rule is
            // the same.
            (
                "`<img src=https://e.example/c.png>",
                "&#96;&lt;img src=https://e.example/c.png>",
            ),
            ("`<img>`", "&#96;&lt;img>&#96;"),
            ("``<img src=u>`", "&#96;&#96;&lt;img src=u>&#96;"),
            ("`x`![a](u)`", "&#96;x&#96;\\![a](u)&#96;"),
            (
                "<a title='`'>`<img src=u>`",
                "&lt;a title='&#96;'>&#96;&lt;img src=u>&#96;",
            ),
            (
                "```\n<img src=u>\n```",
                "&#96;&#96;&#96;\n&lt;img src=u>\n&#96;&#96;&#96;",
            ),
            (
                "  ```\n  <img src=u>\n  ```",
                "  &#96;&#96;&#96;\n  &lt;img src=u>\n  &#96;&#96;&#96;",
            ),
            (
                "- a\n  ```\n  x\n```\n<img src=y>\n```\n<!--\n",
                "- a\n  &#96;&#96;&#96;\n  x\n&#96;&#96;&#96;\n&lt;img src=y>\n&#96;&#96;&#96;\n&lt;!--\n",
            ),
            (
                "- a\n  ~~~\n  x\n ~~~\n![x](u)",
                "- a\n  &#126;&#126;&#126;\n  x\n &#126;&#126;&#126;\n\\![x](u)",
            ),
            (
                "| a | b |\n|---|---|\n| <b|<img src=x> | c |",
                "| a | b |\n|---|---|\n| &lt;b|&lt;img src=x> | c |",
            ),
            (
                "| `x|y` ![a](https://e/x.png) |",
                "| &#96;x|y&#96; \\![a](https://e/x.png) |",
            ),
            (
                "  [r]:https://e.example/b.png",
                "  [r]:https://e.example/b.png",
            ),
            ("[r]: <img src=x>|y", "[r]: &lt;img src=x>|y"),
            (
                "[r]:\n  https://e.example/b.png \"t\"\n\n![x][r]",
                "[r]:\n  https://e.example/b.png \"t\"\n\n\\![x][r]",
            ),
            ("[r]:\n<img src=x>|y", "[r]:\n&lt;img src=x>|y"),
            (
                "[a](u '`')<img src=x>`",
                "[a](u '&#96;')&lt;img src=x>&#96;",
            ),
            ("[a](<img src=x>)", "[a](&lt;img src=x>)"),
            ("a\r<img src=u>\r`b", "a\r&lt;img src=u>\r&#96;b"),
            // An escaped `!` stays escaped; one after an escaped backslash
            // gets its own escape. An escaped `<` shows its entity.
            ("\\![x](u) \\<img>", "\\![x](u) \\&lt;img>"),
            ("\\\\![x](u)", "\\\\\\![x](u)"),
            ("\\\\\\![x](u)", "\\\\\\![x](u)"),
        ] {
            assert_eq!(neutralize_remote(input), expected, "{input:?}");
        }
    }

    /// Whether the character at byte `at` of `text` follows an odd run of
    /// backslashes, so it is backslash-escaped.
    fn escaped_at(text: &str, at: usize) -> bool {
        text[..at]
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'\\')
            .count()
            % 2
            == 1
    }

    #[test]
    fn the_end_of_the_output_cannot_join_with_what_follows() {
        for (input, expected) in [
            // A trailing `!` is escaped, so a following `[` cannot make an
            // image of it.
            ("x!", "x\\!"),
            ("!", "\\!"),
            ("x\\\\!", "x\\\\\\!"),
            ("x\\!", "x\\!"),
            // A trailing odd run of backslashes gets one more, so it cannot
            // escape a following `\`.
            ("x\\", "x\\\\"),
            ("\\", "\\\\"),
            ("x\\\\\\", "x\\\\\\\\"),
            ("x\\\\", "x\\\\"),
            // A trailing run of `~` or spaces is escaped whole, so a run
            // that follows cannot extend it into a fence or indented code.
            ("x~", "x&#126;"),
            ("~~s~~", "~~s&#126;&#126;"),
            ("x ", "x&#32;"),
            ("x\n  ", "x\n&#32;&#32;"),
            ("x\t", "x&#9;"),
        ] {
            assert_eq!(neutralize_remote(input), expected, "{input:?}");
        }
    }

    #[test]
    fn a_backslash_before_whitespace_written_as_an_entity_stays_visible() {
        for (input, expected) in [
            // A single backslash would escape the entity's `&`, showing the
            // entity's text and dropping the backslash, so it is doubled.
            ("a\\\tb", "a\\\\&#9;b"),
            ("x\\ ", "x\\\\&#32;"),
            ("x\\\t", "x\\\\&#9;"),
            // An even run already shows every backslash.
            ("x\\\\ ", "x\\\\&#32;"),
            // A backslash before a raw space stays as it is.
            ("x\\ y", "x\\ y"),
            ("a\\    b", "a\\   &#32;b"),
            // Form feed and vertical tab, which some renderers treat as
            // blank, are written as spaces.
            ("a\u{c}b", "a&#32;b"),
            ("a\u{b}b", "a&#32;b"),
            ("\\\u{c}", "\\\\&#32;"),
        ] {
            assert_eq!(neutralize_remote(input), expected, "{input:?}");
        }
    }

    #[test]
    fn no_code_span_fence_indented_code_or_math_can_form() {
        for (input, expected) in [
            // Dataview inline queries live in code spans.
            ("`= this.file.name`", "&#96;= this.file.name&#96;"),
            ("`$= dv.current()`", "&#96;&#36;= dv.current()&#96;"),
            ("``a `b` c``", "&#96;&#96;a &#96;b&#96; c&#96;&#96;"),
            // Mermaid, dataviewjs, math and plugin blocks live in fences of
            // either kind, at any indent and in any container.
            (
                "```mermaid\nA@{ img: \"u\" }\n```",
                "&#96;&#96;&#96;mermaid\nA@{ img: \"u\" }\n&#96;&#96;&#96;",
            ),
            (
                "~~~dataviewjs\nx\n~~~",
                "&#126;&#126;&#126;dataviewjs\nx\n&#126;&#126;&#126;",
            ),
            (
                "- a\n  ~~~~ {.x}\n  y",
                "- a\n  &#126;&#126;&#126;&#126; {.x}\n  y",
            ),
            ("a ~~~ b", "a &#126;&#126;&#126; b"),
            // Strikethrough runs of one or two are kept.
            ("~~s~~ and ~t~ x", "~~s~~ and ~t~ x"),
            // Math.
            ("$x$", "&#36;x&#36;"),
            ("$$\\href{u}{x}$$\n", "&#36;&#36;\\href{u}{x}&#36;&#36;\n"),
            // A backslash escaping one of these is dropped, since the entity
            // already renders as the plain character; an escaped backslash
            // before one stays.
            ("\\`x\\`", "&#96;x&#96;"),
            ("\\$5", "&#36;5"),
            ("\\~~~", "&#126;&#126;&#126;"),
            ("\\\\$x", "\\\\&#36;x"),
            ("\\\\\\`x", "\\\\&#96;x"),
            ("\\~~ x", "\\~~ x"),
            // Indented code: no tab and no run of four spaces is left.
            ("    = this.file.name", "   &#32;= this.file.name"),
            ("\t$= x", "&#9;&#36;= x"),
            ("-     x", "-   &#32;&#32;x"),
            ("- a\n\n      = x", "- a\n\n   &#32;&#32;&#32;= x"),
            (">     = x", ">   &#32;&#32;= x"),
            ("a\t b", "a&#9; b"),
            ("- a\n  - b\n   - c", "- a\n  - b\n   - c"),
        ] {
            assert_eq!(neutralize_remote(input), expected, "{input:?}");
        }
    }

    #[test]
    fn neutralized_fragments_stay_inert_when_joined() {
        let fragments = [
            "x\\",
            "x\\\\\\",
            "x!",
            "x\\!",
            "!",
            "\\",
            "",
            "![y](https://e.example/a.png)",
            "[y](https://e.example/a.png)",
            "[[https://e.example/x.png]]",
            "\\![y](u)",
            "plain",
            "x~~",
            "~~",
            "~y",
            "x\n  ",
            "  ",
            "  y",
            "x\t",
        ];
        for left in fragments {
            for right in fragments {
                let joined = neutralize_remote(left) + &neutralize_remote(right);
                for (at, _) in joined.match_indices("![") {
                    assert!(
                        escaped_at(&joined, at),
                        "{left:?} + {right:?} gives {joined:?}"
                    );
                }
                assert!(
                    !joined.contains("~~~") && !joined.contains("    "),
                    "{left:?} + {right:?} gives {joined:?}"
                );
            }
        }
    }

    #[test]
    fn every_tag_in_the_list_is_escaped_in_any_case() {
        for tag in [
            "img", "iframe", "video", "audio", "object", "embed", "link", "style", "svg",
            "picture", "source", "script", "image",
        ] {
            for tag in [tag.to_owned(), tag.to_ascii_uppercase()] {
                assert_eq!(
                    neutralize_remote(&format!("<{tag} src=u></{tag}>")),
                    format!("&lt;{tag} src=u>&lt;/{tag}>")
                );
            }
        }
    }

    #[test]
    fn ordinary_markdown_is_left_alone() {
        for text in [
            "[a](https://x.example)",
            "[a](https://x.example \"t\") and code",
            "# T\n\n*e* **s** ~~d~~ c\n\n- one\n1. two\n  - nested\n\n> q\n\n---",
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "- [ ] open\n- [x] done",
            "[^1]: a note",
            "a  \nhard break",
            "Hello! [a](u) ! [b] !\n",
            "&lt;already an entity",
            "",
        ] {
            assert_eq!(neutralize_remote(text), text);
        }
    }

    #[test]
    fn yaml_strings_hold_no_raw_lt_or_image_opener_and_decode_unchanged() {
        assert_eq!(
            yaml_string("<img src=u> ![x](u) \\![y] a\"b"),
            r#""\u003cimg src=u> \u0021[x](u) \\\u0021[y] a\"b""#
        );
        for value in [
            "<img src=https://evil.example/x.png>",
            "![t](https://evil.example/t.png)",
            "\\![x] \\\\![y] !! [z] ! [",
            "plain — text",
            "",
        ] {
            let quoted = yaml_string(value);
            assert!(!quoted.contains('<') && !quoted.contains("!["), "{quoted}");
            assert_eq!(serde_json::from_str::<String>(&quoted).unwrap(), value);
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
    fn pathological_inputs_run_in_linear_time() {
        let started = std::time::Instant::now();
        for input in [
            "![".repeat(65_536),
            format!("{}]", "![".repeat(65_536)),
            "![a](".repeat(32_768),
            (1..400).map(|length| "`".repeat(length) + "x").collect(),
            "<a>".repeat(262_144),
            format!("[a](u \"\n{}", "<a>".repeat(262_144)),
            "\\".repeat(262_144) + "![",
            "~".repeat(262_144),
            " ".repeat(262_144) + "x",
            "\\`".repeat(131_072),
        ] {
            let output = neutralize_remote(&input);
            assert!(!output.contains('<') && !output.contains('`'));
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn inline_text_cannot_form_code_fences_or_math() {
        assert_eq!(
            inline("`$= dv.current()` $x$ ~~~ ~~s~~", 200),
            "&#96;&#36;\\= dv.current()&#96; &#36;x&#36; &#126;&#126;&#126; &#126;&#126;s&#126;&#126;"
        );
        // An agent backslash stays a visible backslash.
        assert_eq!(inline("\\` \\$", 200), "\\\\&#96; \\\\&#36;");
        assert_eq!(inline("~~~mermaid", 200), "&#126;&#126;&#126;mermaid");
        // No run of four spaces is left, so a line placed at a line start
        // cannot make an indented code block in a list item.
        assert_eq!(inline("-     = x", 200), "-   &#32;&#32;\\= x");
        assert_eq!(inline("1.\t\t= x", 200), "1.  \\= x");
        assert_eq!(inline("a    b", 200), "a   &#32;b");
        for value in ["`= this.file.name`", "$$x$$", "~~~", "\\`a`", "a ` b"] {
            let line = inline(value, 200);
            assert!(
                !line.contains('`') && !line.contains('$') && !line.contains('~'),
                "{line}"
            );
        }
    }

    #[test]
    fn wikilinks_cannot_break_out() {
        assert_eq!(wikilink("a]]|b#^", None), "[[ab]]");
        assert_eq!(wikilink("a", Some("b]]|c")), "[[a|bc]]");
        assert_eq!(wikilink("a", Some("[]")), "[[a]]");
    }

    #[test]
    fn wikilinks_cannot_form_code_spans_or_math() {
        // A viewer without wikilinks, such as VS Code preview, would make a
        // code span (a Dataview inline query in Obsidian) or math of these.
        assert_eq!(
            wikilink(
                "`$= dv.el(\"img\", \"\", {attr: {src: \"https://e/x.png\"}})`",
                Some("$x$ `= this.file.name`")
            ),
            "[[= dv.el(\"img\", \"\", {attr: {src: \"https://e/x.png\"}})|x = this.file.name]]"
        );
        assert_eq!(wikilink("`$`", Some("x")), "x");
    }

    #[test]
    fn wikilinks_hold_no_raw_html() {
        // A viewer without wikilinks, such as VS Code preview, parses the
        // inside as Markdown and HTML.
        assert_eq!(
            wikilink("<img src=https://evil.example/x.png>", Some("<b>")),
            "[[img src=https://evil.example/x.png|b]]"
        );
        assert_eq!(wikilink("<>", Some("<img>")), "\\<img\\>");
    }

    #[test]
    fn a_wikilink_with_no_target_left_is_plain_text() {
        assert_eq!(wikilink("", Some("x")), "x");
        assert_eq!(wikilink("[]|#^", Some("<img> *x*")), "\\<img\\> \\*x\\*");
        assert_eq!(wikilink("]]", None), "Untitled");
    }
}

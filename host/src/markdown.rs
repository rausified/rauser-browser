//! Markdown and YAML safety helpers shared by every host-written file
//! (DESIGN.md §5.2 step 6, §6.3). Single-line fields are escaped with
//! `inline`; multi-line agent text keeps its Markdown, with `<` and image
//! `!`s escaped by `neutralize_remote` so no CommonMark, GFM or Obsidian
//! embed construct fetches a remote resource (fenced blocks a viewer hands
//! to another engine, such as Mermaid, are out of scope). Links follow the
//! profile's link style.

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

/// The longest label written as plain text in place of a target-less
/// wikilink.
const MAX_WIKILINK_LABEL_CHARS: usize = 200;

/// An Obsidian wikilink that cannot close early, alias, or point at a
/// heading or block: `[ ] | # ^` and control characters are removed. `<` and
/// `>` are removed too, since a viewer without wikilinks (VS Code preview)
/// would parse raw HTML inside the brackets, and neither can appear in an
/// Obsidian file name. When
/// nothing of the target is left, the label is written as escaped plain
/// text instead (`Untitled` without one).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn wikilink(target: &str, label: Option<&str>) -> String {
    fn clean(value: &str) -> String {
        value
            .chars()
            .filter(|ch| !ch.is_control() && !matches!(ch, '[' | ']' | '|' | '#' | '^' | '<' | '>'))
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
/// resource, by a context-free escape:
///
/// - every `<` becomes `&lt;`. An entity is only ever text in CommonMark and
///   GFM, so no tag, autolink, comment, CDATA section, processing
///   instruction or declaration can form, in any context;
/// - every `!` directly before `[` gets a backslash (`\!`), unless an odd run
///   of backslashes already escapes it. An escaped `!` cannot start an image,
///   so inline, full, collapsed and shortcut reference images and Obsidian
///   `![[embeds]]` cannot form;
/// - a `!` at the very end is escaped as `\!`, and a trailing odd run of
///   backslashes gets one more, so the output can be joined with another
///   neutralized fragment or with host text without re-forming `![` or
///   cancelling a following `\!`. Text written directly before the output
///   must likewise not end in an unescaped `!` or an odd run of backslashes;
///   both `neutralize_remote` and `inline` output satisfy that;
/// - nothing else changes. Links, emphasis, lists, code, tables and fences
///   still render.
///
/// The escape does not depend on where a character falls, so it needs no
/// model of a viewer's parser. An earlier scanner turned each construct into
/// a code span, which meant mirroring how each viewer finds fences, list
/// items, table cells, code spans, link titles and autolink literals; review
/// kept finding inputs where a real renderer paired things differently and
/// rendered a tag live. Inside code spans and blocks, where Markdown is not
/// parsed, the escapes show literally (`&lt;`, `\!`), and an `<https://..>`
/// autolink shows as text. Those cosmetic costs are accepted. Fenced blocks
/// that a viewer renders with a separate engine (Mermaid, math, plugin
/// blocks) are not covered (DESIGN.md §5.2 step 6).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn neutralize_remote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut backslashes = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '<' => out.push_str("&lt;"),
            // A `!` at the end is escaped too, so a `[` written after the
            // output cannot make an image of it.
            '!' if matches!(chars.peek(), Some('[') | None) && backslashes.is_multiple_of(2) => {
                out.push_str("\\!")
            }
            _ => out.push(ch),
        }
        backslashes = if ch == '\\' { backslashes + 1 } else { 0 };
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
            ("![`a`](u)", "\\![`a`](u)"),
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
            ("`a\n![x](u)\nb`", "`a\n\\![x](u)\nb`"),
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
            // Next to backticks, inside code, fences of any indent, table
            // cells and reference definitions, the rule is the same.
            (
                "`<img src=https://e.example/c.png>",
                "`&lt;img src=https://e.example/c.png>",
            ),
            ("`<img>`", "`&lt;img>`"),
            ("``<img src=u>`", "``&lt;img src=u>`"),
            ("`x`![a](u)`", "`x`\\![a](u)`"),
            (
                "<a title='`'>`<img src=u>`",
                "&lt;a title='`'>`&lt;img src=u>`",
            ),
            ("```\n<img src=u>\n```", "```\n&lt;img src=u>\n```"),
            (
                "  ```\n  <img src=u>\n  ```",
                "  ```\n  &lt;img src=u>\n  ```",
            ),
            (
                "- a\n  ```\n  x\n```\n<img src=y>\n```\n<!--\n",
                "- a\n  ```\n  x\n```\n&lt;img src=y>\n```\n&lt;!--\n",
            ),
            (
                "- a\n  ~~~\n  x\n ~~~\n![x](u)",
                "- a\n  ~~~\n  x\n ~~~\n\\![x](u)",
            ),
            (
                "| a | b |\n|---|---|\n| <b|<img src=x> | c |",
                "| a | b |\n|---|---|\n| &lt;b|&lt;img src=x> | c |",
            ),
            (
                "| `x|y` ![a](https://e/x.png) |",
                "| `x|y` \\![a](https://e/x.png) |",
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
            ("[a](u '`')<img src=x>`", "[a](u '`')&lt;img src=x>`"),
            ("[a](<img src=x>)", "[a](&lt;img src=x>)"),
            ("a\r<img src=u>\r`b", "a\r&lt;img src=u>\r`b"),
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
            "[a](https://x.example \"t\") and `code`",
            "# T\n\n*e* **s** ~~d~~ `c`\n\n- one\n1. two\n\n> q\n\n---",
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "- [ ] open\n- [x] done",
            "[^1]: a note",
            "```rust\nlet x = 1 > 0;\n```",
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
        ] {
            let output = neutralize_remote(&input);
            assert!(!output.contains('<'));
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn wikilinks_cannot_break_out() {
        assert_eq!(wikilink("a]]|b#^", None), "[[ab]]");
        assert_eq!(wikilink("a", Some("b]]|c")), "[[a|bc]]");
        assert_eq!(wikilink("a", Some("[]")), "[[a]]");
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

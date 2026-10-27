//! Bounded, inert presentation of untrusted captured text.
//!
//! Apply only when rendering; canonical event payloads and record identities
//! must keep their original bytes so replay and reindex remain lossless.

/// Unicode 17 General_Category=Cf, checked against the local Node Unicode
/// property data. Keep this complete table together when updating Unicode.
fn is_format(ch: char) -> bool {
    matches!(
        ch,
        '\u{00ad}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// Replace controls and invisible formatting with spaces rather than joining
/// words. Ordinary RTL letters, accents, and other visible Unicode survive.
/// Count input scalars before escaping, so output is bounded by twice `limit`.
pub(crate) fn plain_line(value: &str, limit: usize) -> String {
    value
        .trim()
        .chars()
        .take(limit)
        .map(|ch| {
            if ch.is_control() || is_format(ch) || matches!(ch, '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Escape Markdown plus Obsidian comments, highlights, and tags. YAML fields
/// and plain worklogs use `plain_line` instead, without Markdown backslashes.
pub(crate) fn markdown_line(value: &str, limit: usize) -> String {
    let text = plain_line(value, limit);
    if text.is_empty() {
        return "Untitled".into();
    }
    let mut out = String::new();
    for ch in text.chars() {
        if matches!(
            ch,
            '\\' | '!' | '[' | ']' | '<' | '>' | '*' | '_' | '`' | '|' | '%' | '=' | '#'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_preserves_visible_unicode_and_separates_invisible_text() {
        let visible = "العربية עברית 日本語 e\u{301} 😀";
        assert_eq!(plain_line(visible, 100), visible);
        assert_eq!(markdown_line(visible, 100), visible);
        for ch in [
            '\0',
            '\n',
            '\t',
            '\u{0085}',
            '\u{00ad}',
            '\u{061c}',
            '\u{200b}',
            '\u{200d}',
            '\u{200f}',
            '\u{2028}',
            '\u{2029}',
            '\u{202e}',
            '\u{2060}',
            '\u{2066}',
            '\u{2069}',
            '\u{feff}',
            '\u{1343f}',
            '\u{e0001}',
            '\u{e007f}',
        ] {
            assert_eq!(
                plain_line(&format!("left{ch}right"), 100),
                "left right",
                "{ch:?}"
            );
        }
    }

    #[test]
    fn markdown_is_literal_bounded_and_has_an_empty_fallback() {
        assert_eq!(
            markdown_line(
                r"![x](url) <img> **a** _b_ `c` | %%hide%% ==mark== #tag ",
                100
            ),
            r"\!\[x\](url) \<img\> \*\*a\*\* \_b\_ \`c\` \| \%\%hide\%\% \=\=mark\=\= \#tag"
        );
        assert_eq!(markdown_line("##more", 2), r"\#\#");
        assert_eq!(plain_line("日本語", 2), "日本");
        assert_eq!(markdown_line("\u{202e}\u{200b}\n", 300), "Untitled");
        assert_eq!(markdown_line("text", 0), "Untitled");
        assert_eq!(plain_line("\u{202e}\u{200b}", 500), "");
    }
}

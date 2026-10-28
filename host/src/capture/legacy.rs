//! Frozen renderers for v1 hash-only intent recovery. Never use for new
//! writes or display, and never replace these with live presentation helpers.
//! A future sanitizer change must add a generation rather than edit these.

/// The pre-presentation-sanitizer generation and the Unicode-17 sanitized
/// generation (before markdown::inline neutralized math and code). Neither
/// retains prepared bytes in its v1 journal: callers must match the complete
/// candidate hash and exact torn bytes before completing an append.
pub(super) fn lines(value: &str, limit: usize) -> [String; 2] {
    [
        pre_sanitizer_line(value, limit),
        sanitized_line(value, limit),
    ]
}

fn pre_sanitizer_line(value: &str, limit: usize) -> String {
    let mut text = String::new();
    for ch in value.trim().chars().take(limit) {
        if ch.is_control() {
            text.push(' ');
        } else {
            if matches!(ch, '\\' | '[' | ']' | '<' | '>' | '*' | '_' | '`' | '|') {
                text.push('\\');
            }
            text.push(ch);
        }
    }
    if text.trim().is_empty() {
        text = "Untitled".into();
    }
    text
}

/// Frozen Unicode 17 General_Category=Cf table from the sanitized generation.
/// Keep this snapshot unchanged even when live presentation updates Unicode.
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
fn plain_line(value: &str, limit: usize) -> String {
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

/// Historical Markdown escaping, before code and math became entities.
fn sanitized_line(value: &str, limit: usize) -> String {
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

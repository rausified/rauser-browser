//! The shared neutralizer corpus (`host/tests/fixtures/markdown-neutralize-corpus.json`):
//! every input the earlier scanner's tests and review rounds used, plus
//! ordinary Markdown. The properties here hold for each input, and
//! `scripts/check-markdown-neutralizer.mjs` renders the same outputs with
//! real CommonMark/GFM renderers. This module only names
//! `markdown::neutralize_remote`, `markdown::yaml_string`, `markdown::inline`
//! and `markdown::wikilink`, so it runs unchanged against an older
//! `markdown.rs`.

use crate::answer::{Entities, Importance, SummaryAnswer};
use crate::markdown::{LinkStyle, inline, neutralize_remote, wikilink, yaml_string};
use crate::presentation::plain_line;
use crate::summary::{SessionLink, SummaryDocument, owned_summary, render_summary};
use crate::vault::url_record_key;
use brauser_protocol::{HarnessId, ImportanceLevel};

const CORPUS: &str = include_str!("../tests/fixtures/markdown-neutralize-corpus.json");

/// Set to a file path to have `the_corpus_comes_out_inert` write, for each
/// input, `{input, output, frontmatter, inline, wikilink, joined0..}` there
/// as JSON, for the renderer check. `frontmatter` is the input as a YAML front
/// matter value, which a viewer without front matter support renders as body
/// text; `inline` and `wikilink` are the input as a single-line field and as a
/// wikilink target and label; and each `joinedN` is the output followed by
/// the neutralized `FOLLOWERS[N]`.
const OUTPUT_ENV: &str = "BRAUSER_NEUTRALIZE_CORPUS_OUT";

fn corpus() -> Vec<String> {
    serde_json::from_str(CORPUS).expect("corpus is a JSON array of strings")
}

/// Whether the character before byte `at` of `text` ends an odd run of
/// backslashes, so the character at `at` is backslash-escaped.
fn escaped(text: &str, at: usize) -> bool {
    text[..at]
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count()
        % 2
        == 1
}

/// A character's place in a run of one repeated character.
#[derive(Clone, Copy)]
struct Run {
    /// From zero.
    position: usize,
    length: usize,
    at_end: bool,
}

/// For each character of `chars`, its place in its run of `target`; `None`
/// off a run.
fn runs(chars: &[char], target: char) -> Vec<Option<Run>> {
    let mut out = vec![None; chars.len()];
    let mut start = 0;
    while start < chars.len() {
        if chars[start] != target {
            start += 1;
            continue;
        }
        let end = (start..chars.len())
            .find(|at| chars[*at] != target)
            .unwrap_or(chars.len());
        for (at, slot) in out.iter_mut().enumerate().take(end).skip(start) {
            *slot = Some(Run {
                position: at - start,
                length: end - start,
                at_end: end == chars.len(),
            });
        }
        start = end;
    }
    out
}

/// Whether `output` is `input` with exactly the neutralizer's changes and
/// nothing else, read off the input one character at a time: each `<` as
/// `&lt;`; each `!` before `[` or at the end, after an even run of
/// backslashes, as `\!`; each backtick, `$` and tab as `&#96;`, `&#36;` and
/// `&#9;`; each form feed and vertical tab as `&#32;`; each `~` in a run of
/// three or more, or in a run that ends the input, as `&#126;`; each space
/// from the fourth of a run on, and every space of a run that ends the input,
/// as `&#32;`; a backslash that ends an odd run directly before a backtick,
/// `$` or `~` written as an entity dropped, and one directly before a
/// whitespace character written as an entity doubled; and one more backslash
/// after a trailing odd run. Every other input character must
/// appear unchanged, so a neutralizer that dropped, added or moved any other
/// character, backslashes before `![` included, fails.
fn only_the_escapes_changed(input: &str, output: &str) -> bool {
    let chars: Vec<char> = input.chars().collect();
    let tildes = runs(&chars, '~');
    let spaces = runs(&chars, ' ');
    let entity = |at: usize| -> Option<&'static str> {
        match chars[at] {
            '`' => Some("&#96;"),
            '$' => Some("&#36;"),
            '\t' => Some("&#9;"),
            '\u{b}' | '\u{c}' => Some("&#32;"),
            '~' => tildes[at]
                .filter(|run| run.at_end || run.length >= 3)
                .map(|_| "&#126;"),
            ' ' => spaces[at]
                .filter(|run| run.at_end || run.position >= 3)
                .map(|_| "&#32;"),
            _ => None,
        }
    };
    let mut rest = output;
    let mut backslashes = 0usize;
    for (at, &ch) in chars.iter().enumerate() {
        let next = chars.get(at + 1).copied();
        let expected = match ch {
            '<' => "&lt;".to_owned(),
            '!' if matches!(next, Some('[') | None) && backslashes.is_multiple_of(2) => {
                "\\!".to_owned()
            }
            // An escaped backtick, `$` or `~` loses its backslash, since the
            // entity shows the plain character.
            '\\' if backslashes.is_multiple_of(2)
                && matches!(next, Some('`' | '$' | '~'))
                && entity(at + 1).is_some() =>
            {
                String::new()
            }
            // A backslash before whitespace is literal, but before an entity
            // it would escape the `&`, so it is doubled to stay visible.
            '\\' if backslashes.is_multiple_of(2)
                && matches!(next, Some(' ' | '\t' | '\u{b}' | '\u{c}'))
                && entity(at + 1).is_some() =>
            {
                "\\\\".to_owned()
            }
            _ => match entity(at) {
                Some(escape) => escape.to_owned(),
                None => ch.to_string(),
            },
        };
        match rest.strip_prefix(expected.as_str()) {
            Some(after) => rest = after,
            None => return false,
        }
        backslashes = if ch == '\\' { backslashes + 1 } else { 0 };
    }
    if !backslashes.is_multiple_of(2) {
        rest = match rest.strip_prefix('\\') {
            Some(after) => after,
            None => return false,
        };
    }
    rest.is_empty()
}

/// Fragments written after each neutralized corpus output, themselves
/// neutralized, to check that joined fragments stay inert.
const FOLLOWERS: [&str; 6] = [
    "[j](https://e.example/j.png)",
    "![j](https://e.example/j.png)",
    "[[https://e.example/j.png]]",
    "~~x",
    "   y",
    "\n    = x",
];

struct Case {
    input: String,
    output: String,
    frontmatter: String,
    /// The input as a single-line field.
    inline: String,
    /// The input as a wikilink's target and label.
    wikilink: String,
    /// The output followed by each neutralized `FOLLOWERS` fragment.
    joined: Vec<String>,
    /// The canonical page URL of the last two summaries.
    query_url: String,
    /// A summary file with every agent field set to the input, in the
    /// relative and wikilink styles, for `SUMMARY_URL` and then for
    /// `query_url`.
    summaries: [String; 4],
}

/// A summary file of `url` whose agent fields and title are all `input`.
fn summary_file(input: &str, url: &str, style: LinkStyle) -> String {
    let answer = SummaryAnswer {
        document_type: input.into(),
        category: input.into(),
        entities: Entities {
            technologies: vec![input.into()],
            products: vec![input.into(), "Widget".into()],
            people: vec![input.into()],
            companies: vec![input.into()],
        },
        description: input.into(),
        importance: Importance {
            level: ImportanceLevel::Medium,
            reason: input.into(),
        },
        assessment: input.into(),
        related_notes: vec![input.into(), "projects/widget.md".into()],
    };
    let key = url_record_key(url).unwrap();
    let note_link = match style {
        LinkStyle::Relative => "[Page note](../notes/e-example-a.md)".to_owned(),
        LinkStyle::Wikilink => wikilink("notes/e-example-a", Some("Page note")),
    };
    render_summary(
        &SummaryDocument {
            key: &key,
            canonical_url: url,
            title: input,
            summarized_at: "2026-10-26T14:05:00-07:00",
            harness: HarnessId::Codex,
            session: SessionLink::CodexThread(input.into()),
            note_link,
            answer: &answer,
        },
        style,
    )
}

const SUMMARY_URL: &str = "https://e.example/a";

/// A page URL whose query WHATWG serialization leaves a backtick and `$` in.
const HOSTILE_QUERY_URL: &str = "https://e.example/p?q=`$=dv.el('b','x')`&r=$x$";

/// The canonical URL of `SUMMARY_URL` with `input` as its query, as the host
/// normalizes it, or of `HOSTILE_QUERY_URL` when that does not parse. The
/// page chooses its URL, so its query is as hostile as any agent field.
fn query_url(input: &str) -> String {
    crate::capture::canonical_url(&format!("{SUMMARY_URL}?q={input}"), &[])
        .ok()
        .filter(|url| url_record_key(url).is_ok())
        .unwrap_or_else(|| crate::capture::canonical_url(HOSTILE_QUERY_URL, &[]).unwrap())
}

/// A rendered summary's frontmatter (with its fences) and body.
fn split_summary(file: &str) -> (&str, &str) {
    let end = "---\n".len() + file["---\n".len()..].find("\n---\n").unwrap() + "\n---\n".len();
    file.split_at(end)
}

#[test]
fn the_corpus_comes_out_inert() {
    let cases: Vec<Case> = corpus()
        .into_iter()
        .map(|input| {
            let output = neutralize_remote(&input);
            let query_url = query_url(&input);
            Case {
                joined: FOLLOWERS
                    .iter()
                    .map(|follower| output.clone() + &neutralize_remote(follower))
                    .collect(),
                frontmatter: format!("---\ntitle: {}\n---\n", yaml_string(&input)),
                inline: inline(&input, 2048),
                summaries: [
                    summary_file(&input, SUMMARY_URL, LinkStyle::Relative),
                    summary_file(&input, SUMMARY_URL, LinkStyle::Wikilink),
                    summary_file(&input, &query_url, LinkStyle::Relative),
                    summary_file(&input, &query_url, LinkStyle::Wikilink),
                ],
                query_url,
                wikilink: wikilink(&input, Some(&input)),
                output,
                input,
            }
        })
        .collect();
    // Written before any assertion, so the renderer check can also be run
    // against a neutralizer that fails these properties.
    if let Some(path) = std::env::var_os(OUTPUT_ENV) {
        let json: Vec<_> = cases
            .iter()
            .map(|case| {
                let mut pair = serde_json::json!({
                    "input": case.input,
                    "output": case.output,
                    "frontmatter": case.frontmatter,
                    "inline": case.inline,
                    "wikilink": case.wikilink,
                });
                for (index, joined) in case.joined.iter().enumerate() {
                    pair[format!("joined{index}")] = joined.clone().into();
                }
                // The summary body is checked as body text; its frontmatter
                // as front matter.
                for (name, file) in [
                    "summary",
                    "summaryWikilink",
                    "summaryQuery",
                    "summaryQueryWikilink",
                ]
                .iter()
                .zip(&case.summaries)
                {
                    let (frontmatter, body) = split_summary(file);
                    pair[*name] = body.into();
                    pair[format!("frontmatter_{name}")] = frontmatter.into();
                }
                pair
            })
            .collect();
        std::fs::write(path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }
    for Case {
        input,
        output,
        frontmatter,
        inline,
        wikilink,
        joined,
        query_url,
        summaries,
    } in &cases
    {
        // A summary file keeps its identity and hands nothing to another
        // engine, whatever the agent wrote; its frontmatter values read back
        // as the input's plain line.
        let keys = [SUMMARY_URL, query_url.as_str()].map(|url| url_record_key(url).unwrap());
        for (index, file) in summaries.iter().enumerate() {
            assert!(owned_summary(file.as_bytes(), &keys[index / 2]), "{file:?}");
            let (front, body) = split_summary(file);
            for raw in ["`", "$"] {
                assert!(
                    !body.contains(raw),
                    "raw {raw:?} in {body:?} from {input:?}"
                );
            }
            for (at, _) in body.match_indices("![") {
                assert!(
                    escaped(body, at),
                    "unescaped '![' in {body:?} from {input:?}"
                );
            }
            assert!(!front.contains('<') && !front.contains("!["), "{front:?}");
            let title = front
                .lines()
                .find_map(|line| line.strip_prefix("title: "))
                .unwrap();
            assert_eq!(
                serde_json::from_str::<String>(title).unwrap(),
                plain_line(input, 2048)
            );
        }
        for text in std::iter::once(output).chain(joined) {
            assert!(!text.contains('<'), "raw '<' in {text:?} from {input:?}");
            // Nothing can open a code span, fence, indented code block or
            // math (DESIGN.md §5.2 step 6).
            for raw in ["`", "$", "~~~", "\t", "    "] {
                assert!(
                    !text.contains(raw),
                    "raw {raw:?} in {text:?} from {input:?}"
                );
            }
            for (at, _) in text.match_indices("![") {
                assert!(
                    escaped(text, at),
                    "unescaped '![' in {text:?} from {input:?}"
                );
            }
        }
        // A single-line field or wikilink cannot open a code span or math
        // either. A single-line field writes every `~` as an entity, so it
        // cannot open a fence even at a line start; a wikilink starts with
        // `[[`. (`inline` backslash-escapes `<`; the renderer check covers
        // its HTML.)
        for text in [inline, wikilink] {
            for raw in ["`", "$"] {
                assert!(
                    !text.contains(raw),
                    "raw {raw:?} in {text:?} from {input:?}"
                );
            }
        }
        // Nor, placed at a line start, a fence or an indented code block.
        assert!(
            !inline.contains('~') && !inline.contains("    "),
            "{inline:?} from {input:?}"
        );
        // Nothing but the escapes changes.
        assert!(
            only_the_escapes_changed(input, output),
            "{input:?} gave {output:?}"
        );
        // A front matter value holds neither, and still reads back as the
        // input.
        assert!(
            !frontmatter.contains('<') && !frontmatter.contains("!["),
            "{frontmatter:?}"
        );
        let value = frontmatter
            .strip_prefix("---\ntitle: ")
            .and_then(|rest| rest.strip_suffix("\n---\n"))
            .unwrap();
        assert_eq!(&serde_json::from_str::<String>(value).unwrap(), input);
    }
}

#[test]
fn the_corpus_has_no_duplicates() {
    let inputs = corpus();
    let unique: std::collections::BTreeSet<_> = inputs.iter().collect();
    assert_eq!(unique.len(), inputs.len());
}

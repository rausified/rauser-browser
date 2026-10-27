//! The shared neutralizer corpus (`host/tests/fixtures/markdown-neutralize-corpus.json`):
//! every input the earlier scanner's tests and review rounds used, plus
//! ordinary Markdown. The properties here hold for each input, and
//! `scripts/check-markdown-neutralizer.mjs` renders the same outputs with
//! real CommonMark/GFM renderers. This module only names
//! `markdown::neutralize_remote` and `markdown::yaml_string`, so it runs
//! unchanged against an older `markdown.rs`.

use crate::markdown::{neutralize_remote, yaml_string};

const CORPUS: &str = include_str!("../tests/fixtures/markdown-neutralize-corpus.json");

/// Set to a file path to have `the_corpus_comes_out_inert` write, for each
/// input, `{input, output, frontmatter, joined0..}` there as JSON, for the
/// renderer check. `frontmatter` is the input as a YAML front matter value,
/// which a viewer without front matter support renders as body text, and
/// each `joinedN` is the output followed by the neutralized `FOLLOWERS[N]`.
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

/// Whether `output` is `input` with exactly the neutralizer's changes and
/// nothing else, read off the input one character at a time: each `<` as
/// `&lt;`; each `!` before `[` or at the end, after an even run of
/// backslashes, as `\!`; and one more backslash after a trailing odd run.
/// Every other input character must appear unchanged, so a neutralizer that
/// dropped, added or moved any other character, backslashes before `![`
/// included, fails.
fn only_the_escapes_changed(input: &str, output: &str) -> bool {
    let mut rest = output;
    let mut backslashes = 0usize;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        let expected = match ch {
            '<' => "&lt;".to_owned(),
            '!' if matches!(chars.peek(), Some('[') | None) && backslashes.is_multiple_of(2) => {
                "\\!".to_owned()
            }
            _ => ch.to_string(),
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
const FOLLOWERS: [&str; 3] = [
    "[j](https://e.example/j.png)",
    "![j](https://e.example/j.png)",
    "[[https://e.example/j.png]]",
];

struct Case {
    input: String,
    output: String,
    frontmatter: String,
    /// The output followed by each neutralized `FOLLOWERS` fragment.
    joined: Vec<String>,
}

#[test]
fn the_corpus_comes_out_inert() {
    let cases: Vec<Case> = corpus()
        .into_iter()
        .map(|input| {
            let output = neutralize_remote(&input);
            Case {
                joined: FOLLOWERS
                    .iter()
                    .map(|follower| output.clone() + &neutralize_remote(follower))
                    .collect(),
                frontmatter: format!("---\ntitle: {}\n---\n", yaml_string(&input)),
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
                });
                for (index, joined) in case.joined.iter().enumerate() {
                    pair[format!("joined{index}")] = joined.clone().into();
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
        joined,
    } in &cases
    {
        for text in std::iter::once(output).chain(joined) {
            assert!(!text.contains('<'), "raw '<' in {text:?} from {input:?}");
            for (at, _) in text.match_indices("![") {
                assert!(
                    escaped(text, at),
                    "unescaped '![' in {text:?} from {input:?}"
                );
            }
        }
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

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
/// input, `{input, output, frontmatter}` there as JSON, for the renderer
/// check. `frontmatter` is the input as a YAML front matter value, which a
/// viewer without front matter support renders as body text.
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

/// `text` with the neutralizer's two changes undone, as far as they can be
/// told apart from the input: `&lt;` read as `<`, and any backslashes
/// directly before `![` dropped.
fn undo(text: &str) -> String {
    let text = text.replace("&lt;", "<");
    let mut out = String::with_capacity(text.len());
    for (index, piece) in text.split("![").enumerate() {
        if index > 0 {
            while out.ends_with('\\') {
                out.pop();
            }
            out.push_str("![");
        }
        out.push_str(piece);
    }
    out
}

struct Case {
    input: String,
    output: String,
    frontmatter: String,
}

#[test]
fn the_corpus_comes_out_inert() {
    let cases: Vec<Case> = corpus()
        .into_iter()
        .map(|input| Case {
            output: neutralize_remote(&input),
            frontmatter: format!("---\ntitle: {}\n---\n", yaml_string(&input)),
            input,
        })
        .collect();
    // Written before any assertion, so the renderer check can also be run
    // against a neutralizer that fails these properties.
    if let Some(path) = std::env::var_os(OUTPUT_ENV) {
        let json: Vec<_> = cases
            .iter()
            .map(|case| {
                serde_json::json!({
                    "input": case.input,
                    "output": case.output,
                    "frontmatter": case.frontmatter,
                })
            })
            .collect();
        std::fs::write(path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }
    for Case {
        input,
        output,
        frontmatter,
    } in &cases
    {
        assert!(
            !output.contains('<'),
            "raw '<' in {output:?} from {input:?}"
        );
        for (at, _) in output.match_indices("![") {
            assert!(
                escaped(output, at),
                "unescaped '![' in {output:?} from {input:?}"
            );
        }
        // Nothing but those two characters changes.
        assert_eq!(undo(output), undo(input), "{input:?}");
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

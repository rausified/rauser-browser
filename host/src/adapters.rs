//! `brauser adapters eval`: evaluates adapter templates from the command line
//! (adapter-templates.md §12). The differential tests drive this and the
//! Node harness (`scripts/adapter-templates-eval.mjs`) with the same requests
//! and compare the output line by line, so the two must keep one JSON shape.
//!
//! Input is JSON Lines on stdin, one request per line:
//!
//! ```json
//! {"doc": {"format": 1, "artifacts": [...]}, "url": "https://...", "title": "..."}
//! ```
//!
//! `adapter` (one adapter table, evaluated as a one-adapter document) may
//! replace `doc`, and `title` is optional. `url` and `title` are strings, or
//! arrays of UTF-16 code units so a request can carry lone surrogates, which
//! JSON strings cannot portably hold; the host decodes those lossily, which
//! is its well-formedness step (§9.1). Each request prints one line: the
//! shared vectors' `evaluate` result, or `{"error": {...}}` when the document
//! fails validation. Nothing is read from or written to the config or vault.

use std::io::{self, BufRead, BufWriter, Write};

use adapter_templates::{Document, Match, ValidationError, Value};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value as Json, json};

mod aliases;

pub const COMMAND: &str = "adapters";

/// Runs `brauser adapters <subcommand>`.
pub fn run(mut args: impl Iterator<Item = String>) -> Result<()> {
    match args.next().as_deref() {
        Some("eval") => {
            if let Some(extra) = args.next() {
                bail!(
                    "adapters eval takes no arguments (got {extra}); requests are JSON Lines on stdin"
                );
            }
            eval_lines(io::stdin().lock(), io::stdout().lock())
        }
        Some("aliases-preview") => {
            if args.next().is_some() {
                bail!(
                    "adapters aliases-preview takes no arguments; requests are JSON Lines on stdin"
                );
            }
            aliases::preview_lines(io::stdin().lock(), io::stdout().lock())
        }
        Some("relearn-plan") => {
            let config_path = match args.next().as_deref() {
                None => None,
                Some("--config") => {
                    let path = std::path::PathBuf::from(
                        args.next().context("--config needs an absolute path")?,
                    );
                    anyhow::ensure!(path.is_absolute(), "--config needs an absolute path");
                    Some(path)
                }
                Some(_) => bail!(
                    "adapters relearn-plan accepts only --config /absolute/path; one JSON request is read from stdin"
                ),
            };
            anyhow::ensure!(args.next().is_none(), "unexpected relearn-plan arguments");
            crate::relearn::run(config_path, io::stdin().lock(), io::stdout().lock())
        }
        Some(other) => bail!("unknown adapters command: {other}"),
        None => bail!("usage: adapters <eval|aliases-preview|relearn-plan> < requests.jsonl"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    doc: Option<Value>,
    adapter: Option<Value>,
    url: Text,
    title: Option<Text>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Text {
    String(String),
    Utf16(Vec<u16>),
}

impl Text {
    fn into_string(self) -> String {
        match self {
            Text::String(s) => s,
            Text::Utf16(units) => String::from_utf16_lossy(&units),
        }
    }
}

/// Evaluates every request line. Blank lines are skipped; a malformed line
/// stops the run, naming its line number.
pub fn eval_lines(input: impl BufRead, output: impl Write) -> Result<()> {
    let mut output = BufWriter::new(output);
    for (index, line) in input.lines().enumerate() {
        let line = line.context("reading requests")?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = serde_json::from_str(&line)
            .with_context(|| format!("request on line {} is not valid", index + 1))?;
        let result = evaluate(request).with_context(|| format!("request on line {}", index + 1))?;
        serde_json::to_writer(&mut output, &result)?;
        output.write_all(b"\n")?;
    }
    output.flush()?;
    Ok(())
}

fn evaluate(request: Request) -> Result<Json> {
    let doc = match (request.doc, request.adapter) {
        (Some(doc), None) => doc,
        (None, Some(adapter)) => Value::Table(vec![
            ("format".into(), Value::Int(1)),
            ("artifacts".into(), Value::Array(vec![adapter])),
        ]),
        _ => bail!("give exactly one of doc and adapter"),
    };
    let doc = match Document::from_value(&doc) {
        Ok(doc) => doc,
        Err(error) => return Ok(json!({"error": validation_json(&error)})),
    };
    let url = request.url.into_string();
    let title = request.title.map(Text::into_string);
    Ok(match doc.evaluate(&url, title.as_deref()) {
        Err(reason) => json!({"match": false, "reason": reason.as_str()}),
        Ok(found) => match_json(&found),
    })
}

fn validation_json(error: &ValidationError) -> Json {
    json!({"code": error.code.as_str(), "adapter": error.adapter, "rule": error.rule, "entry": error.entry})
}

fn match_json(found: &Match) -> Json {
    let captures: Map<String, Json> = found
        .captures
        .iter()
        .map(|(name, value)| (name.clone(), Json::String(value.clone())))
        .collect();
    let mut out = json!({
        "match": true,
        "adapter": found.adapter,
        "rule": found.rule,
        "type": found.artifact_type,
        "captures": captures,
        "artifact_id": found.artifact_id,
        "mode": found.mode.map(|mode| mode.as_str()),
    });
    if let (Some(within), Json::Object(fields)) = (&found.within, &mut out) {
        fields.insert(
            "within".into(),
            json!({"type": within.artifact_type, "artifact_id": within.artifact_id}),
        );
    }
    if let (Some(title), Json::Object(fields)) = (&found.title, &mut out) {
        let refs: Vec<Json> = title
            .refs
            .iter()
            .map(|r| json!({"type": r.artifact_type, "value": r.value}))
            .collect();
        fields.insert("title".into(), json!(title.title));
        fields.insert("title_entry".into(), json!(title.entry));
        fields.insert("alias_id".into(), json!(title.alias_id));
        fields.insert("refs".into(), Json::Array(refs));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(input: &str) -> Result<String> {
        let mut out = Vec::new();
        eval_lines(input.as_bytes(), &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn evaluates_each_line_in_the_vector_shape() {
        let adapter = r#"{"type": "acme.item", "origin": "https://acme.example.com", "match": [{"path": "/items/{id:int}"}], "id": "acme:{host}/{id}", "title": "{title} - Acme"}"#;
        let input = format!(
            "{{\"adapter\": {adapter}, \"url\": \"https://acme.example.com/items/7\", \"title\": \"Plan - Acme\"}}\n\n\
             {{\"adapter\": {adapter}, \"url\": \"https://acme.example.com/other\"}}\n"
        );
        let lines: Vec<Json> = eval(&input)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            lines,
            [
                json!({"match": true, "adapter": 0, "rule": 0, "type": "acme.item", "captures": {"id": "7"},
                       "artifact_id": "acme:acme.example.com/7", "mode": null, "title": "Plan",
                       "title_entry": 0, "alias_id": null, "refs": []}),
                json!({"match": false, "reason": "no_rule"}),
            ]
        );
    }

    #[test]
    fn reports_validation_errors_and_decodes_utf16_titles() {
        let out = eval(r#"{"doc": {"format": 2}, "url": "https://a.example"}"#).unwrap();
        assert_eq!(
            out,
            "{\"error\":{\"adapter\":null,\"code\":\"unsupported_format\",\"entry\":null,\"rule\":null}}\n"
        );
        let doc = r#"{"format": 1, "artifacts": [{"type": "a.b", "origin": "https://a.example", "match": [{"path": "/x/{id:int}"}], "id": "a:{host}/{id}"}]}"#;
        let out = eval(&format!(
            "{{\"doc\": {doc}, \"url\": \"https://a.example/x/1\", \"title\": [97, 55357]}}"
        ))
        .unwrap();
        let got: Json = serde_json::from_str(&out).unwrap();
        assert_eq!(got["title"], "a\u{fffd}");
    }

    #[test]
    fn rejects_malformed_requests_with_their_line() {
        let error = eval("\n{\"url\": \"https://a.example\"}").unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "request on line 2: give exactly one of doc and adapter"
        );
        let error = eval("{\"doc\": {}, \"url\": 1, \"extra\": 0}").unwrap_err();
        assert!(format!("{error}").starts_with("request on line 1 is not valid"));
    }
}

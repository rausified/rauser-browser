//! The Capture Site agent's answer (plan "Contract decision: where the
//! agent's answer is judged"): parsing and validating the form of a
//! site-draft run's final message, and the JSON Schema Codex is given to
//! constrain it.
//!
//! The service checks only the form, so its one repair turn can fix what an
//! agent gets wrong by accident. Safety is the host's to judge: a table with
//! a forbidden key (`title`, `scope`, `fields`, ...) or a rule that fails a
//! drafting check passes here unchanged, so the host sees exactly what the
//! agent tried (`structural::check_agent_draft`).

use adapter_templates::MAX_RULES;
use adapter_templates::structural::MAX_DRAFT_ADAPTERS;
use serde_json::{Map, Value, json};

use crate::answer::strip_fence;

/// The largest final answer [`parse_site_draft_answer`] takes.
pub const MAX_SITE_DRAFT_ANSWER_BYTES: usize = 64 * 1024;

/// Parse a site-draft run's final message and re-serialize it as
/// `{"adapters": [...]}`. The error names the field at fault and carries
/// nothing from the answer, so it can go back to the agent in the repair
/// prompt and be shown as the run's error.
///
/// The answer must be one JSON object (a single surrounding code fence is
/// stripped) whose only key is `adapters`: 1 to [`MAX_DRAFT_ADAPTERS`]
/// tables, each an object whose `origin` is `origin` exactly and whose
/// `match` has at most [`MAX_RULES`] rules, and the whole answer must parse
/// as adapter-templates values, which refuses a repeated key anywhere.
pub fn parse_site_draft_answer(final_text: &str, origin: &str) -> Result<Value, String> {
    if final_text.len() > MAX_SITE_DRAFT_ANSWER_BYTES {
        return Err("the answer is too large".to_string());
    }
    let text = strip_fence(final_text.trim());
    if text.is_empty() {
        return Err("the answer is empty".to_string());
    }
    let value: Value =
        serde_json::from_str(text).map_err(|_| "the answer is not valid JSON".to_string())?;
    // The crate's own parser, as the host will read the tables: it refuses
    // a key written twice, which serde_json would silently collapse.
    if serde_json::from_str::<adapter_templates::Value>(text).is_err() {
        return Err("the answer repeats a key in one object".to_string());
    }
    let Value::Object(object) = value else {
        return Err("the answer is not a JSON object".to_string());
    };
    let adapters = adapters(&object)?;
    for (index, table) in adapters.iter().enumerate() {
        check_table(table, index, origin)?;
    }
    Ok(json!({ "adapters": adapters }))
}

/// The JSON Schema of the answer, for a harness that accepts one. The
/// tables themselves are left open: their form is checked by
/// [`parse_site_draft_answer`] and their content by the host.
pub fn site_draft_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["adapters"],
        "properties": {
            "adapters": {
                "type": "array",
                "items": { "type": "object" }
            }
        }
    })
}

fn adapters(object: &Map<String, Value>) -> Result<&Vec<Value>, String> {
    if object.keys().any(|key| key != "adapters") {
        return Err("the answer must have only the key adapters".to_string());
    }
    let adapters = match object.get("adapters") {
        Some(Value::Array(adapters)) => adapters,
        Some(_) => return Err("adapters must be an array".to_string()),
        None => return Err("adapters is missing".to_string()),
    };
    if adapters.is_empty() {
        return Err("adapters is empty".to_string());
    }
    if adapters.len() > MAX_DRAFT_ADAPTERS {
        return Err(format!(
            "adapters has more than {MAX_DRAFT_ADAPTERS} entries"
        ));
    }
    Ok(adapters)
}

fn check_table(table: &Value, index: usize, origin: &str) -> Result<(), String> {
    let name = format!("adapters[{index}]");
    let Value::Object(table) = table else {
        return Err(format!("{name} must be an object"));
    };
    match table.get("origin") {
        Some(Value::String(value)) if value == origin => {}
        Some(Value::String(_)) => return Err(format!("{name}.origin must be {origin}")),
        Some(_) => return Err(format!("{name}.origin must be a string")),
        None => return Err(format!("{name}.origin is missing")),
    }
    if let Some(Value::Array(rules)) = table.get("match")
        && rules.len() > MAX_RULES
    {
        return Err(format!("{name}.match has more than {MAX_RULES} rules"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://tools.example.com";

    fn table(path: &str) -> Value {
        json!({
            "type": "tools.docs",
            "origin": ORIGIN,
            "match": [{ "path": path }],
            "id": "tools.docs:{host}/{n}",
        })
    }

    fn answer(tables: &[Value]) -> String {
        json!({ "adapters": tables }).to_string()
    }

    #[test]
    fn site_draft_answer_accepts_a_fenced_object_and_normalizes_it() {
        let tables = [table("/docs/{n:int}")];
        let expected = json!({ "adapters": tables });
        let pretty = serde_json::to_string_pretty(&expected).unwrap();
        for text in [
            answer(&tables),
            format!("  {pretty}\n"),
            format!("```json\n{pretty}\n```"),
            format!("```\n{pretty}\n```\n"),
        ] {
            assert_eq!(parse_site_draft_answer(&text, ORIGIN), Ok(expected.clone()));
        }
        let eight: Vec<Value> = (0..8).map(|_| table("/docs/{n:int}")).collect();
        assert!(parse_site_draft_answer(&answer(&eight), ORIGIN).is_ok());
        let rules: Vec<Value> = (0..16)
            .map(|i| json!({ "path": format!("/docs{i}/{{n:int}}") }))
            .collect();
        let full = json!({ "type": "t", "origin": ORIGIN, "match": rules, "id": "t:{host}/{n}" });
        assert!(parse_site_draft_answer(&answer(&[full]), ORIGIN).is_ok());
    }

    #[test]
    fn site_draft_answer_rejects_non_json_extra_keys_wrong_origin_too_many_adapters_and_oversize() {
        let good = table("/docs/{n:int}");
        let mut other_origin = good.clone();
        other_origin["origin"] = json!("https://evil.example.com");
        let mut no_origin = good.clone();
        no_origin.as_object_mut().unwrap().remove("origin");
        let mut number_origin = good.clone();
        number_origin["origin"] = json!(7);
        let rules: Vec<Value> = (0..17)
            .map(|i| json!({ "path": format!("/docs{i}/{{n:int}}") }))
            .collect();
        let mut many_rules = good.clone();
        many_rules["match"] = json!(rules);
        let nine: Vec<Value> = (0..9).map(|_| good.clone()).collect();
        let padding = "x".repeat(MAX_SITE_DRAFT_ANSWER_BYTES);
        let cases: Vec<(String, &str)> = vec![
            (String::new(), "the answer is empty"),
            ("  \n".into(), "the answer is empty"),
            (
                "Here is the draft: /docs/{n:int}".into(),
                "the answer is not valid JSON",
            ),
            (
                "```json\n{\"adapters\": [\n```".into(),
                "the answer is not valid JSON",
            ),
            (
                format!("{} trailing", answer(std::slice::from_ref(&good))),
                "the answer is not valid JSON",
            ),
            (
                r#"{"adapters": [], "adapters": []}"#.into(),
                "the answer repeats a key in one object",
            ),
            (
                format!(r#"{{"adapters": [{{"type": "a", "type": "b", "origin": "{ORIGIN}"}}]}}"#),
                "the answer repeats a key in one object",
            ),
            ("[]".into(), "the answer is not a JSON object"),
            ("\"adapters\"".into(), "the answer is not a JSON object"),
            ("{}".into(), "adapters is missing"),
            (
                json!({ "adapters": [good.clone()], "notes": "x" }).to_string(),
                "the answer must have only the key adapters",
            ),
            (
                json!({ "adapters": {} }).to_string(),
                "adapters must be an array",
            ),
            (answer(&[]), "adapters is empty"),
            (answer(&nine), "adapters has more than 8 entries"),
            (
                answer(&[good.clone(), json!("/docs")]),
                "adapters[1] must be an object",
            ),
            (
                answer(&[good.clone(), other_origin]),
                "adapters[1].origin must be https://tools.example.com",
            ),
            (answer(&[no_origin]), "adapters[0].origin is missing"),
            (
                answer(&[number_origin]),
                "adapters[0].origin must be a string",
            ),
            (
                answer(&[many_rules]),
                "adapters[0].match has more than 16 rules",
            ),
            (
                format!("{}{padding}", answer(std::slice::from_ref(&good))),
                "the answer is too large",
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(
                parse_site_draft_answer(&text, ORIGIN),
                Err(expected.to_string()),
                "{text:.80}"
            );
        }
    }

    /// The error never echoes the answer, so it is safe to show and to send
    /// back in the repair prompt.
    #[test]
    fn site_draft_answer_errors_carry_no_answer_text() {
        let mut planted = table("/SENTINEL/{n:int}");
        planted["origin"] = json!("https://SENTINEL.example.com");
        for text in [
            "SENTINEL prose".to_string(),
            answer(&[planted]),
            json!({ "adapters": [], "SENTINEL": 1 }).to_string(),
        ] {
            let error = parse_site_draft_answer(&text, ORIGIN).unwrap_err();
            assert!(!error.contains("SENTINEL"), "{error}");
        }
    }

    #[test]
    fn site_draft_answer_keeps_forbidden_keys_for_the_host_to_judge() {
        let mut forbidden = table("/notes/{n:int}");
        let object = forbidden.as_object_mut().unwrap();
        for (key, value) in [
            ("title", json!("{title}")),
            ("scope", json!({ "n": ["1"] })),
            ("unscoped", json!("acknowledged")),
            ("alias", json!({ "from": "tab_title", "capture": "n" })),
            ("fields", json!({ "status": ".status" })),
            ("path_case", json!("insensitive")),
        ] {
            object.insert(key.to_string(), value);
        }
        // A rule that fails every drafting check, and a table that is not
        // a valid adapter, pass the form check too.
        let failing = table("/search/{*}");
        let broken = json!({ "origin": ORIGIN, "match": "/tickets" });
        let tables = [forbidden, failing, broken];
        assert_eq!(
            parse_site_draft_answer(&answer(&tables), ORIGIN),
            Ok(json!({ "adapters": tables }))
        );
    }

    #[test]
    fn site_draft_schema_requires_only_adapters() {
        let schema = site_draft_schema();
        assert_eq!(schema["required"], json!(["adapters"]));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["properties"]["adapters"]["items"]["type"], "object");
    }
}

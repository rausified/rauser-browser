//! The structured summary answer (spec §6.4): parsing and validating an
//! agent's final message, and the JSON Schema a harness may be given to
//! constrain it. The run record keeps the normalized [`SummaryAnswer`], so
//! later readers can deserialize it strictly.

use std::path::{Component, Path};

use brauser_protocol::ImportanceLevel;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The largest final answer [`parse_answer`] takes.
pub const MAX_ANSWER_BYTES: usize = 256 * 1024;
/// Bounds on a document type or category, in characters.
pub const MAX_LABEL_CHARS: usize = 64;
/// The most items an entity list may have.
pub const MAX_ENTITIES: usize = 32;
/// Bounds on one entity, in characters.
pub const MAX_ENTITY_CHARS: usize = 128;
pub const MAX_DESCRIPTION_CHARS: usize = 2000;
pub const MAX_REASON_CHARS: usize = 300;
pub const MAX_ASSESSMENT_CHARS: usize = 4000;
/// The most related notes kept; later ones are dropped.
pub const MAX_RELATED_NOTES: usize = 32;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entities {
    pub technologies: Vec<String>,
    pub products: Vec<String>,
    pub people: Vec<String>,
    pub companies: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Importance {
    pub level: ImportanceLevel,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SummaryAnswer {
    pub document_type: String,
    pub category: String,
    pub entities: Entities,
    pub description: String,
    pub importance: Importance,
    pub assessment: String,
    pub related_notes: Vec<String>,
}

/// Parse and normalize an agent's final message. The error names the field
/// at fault and carries nothing from the answer, so it can go back to the
/// agent in the repair prompt and be shown as the run's error.
///
/// Unknown keys are ignored. Strings are trimmed; entity lists drop blanks
/// and case-insensitive duplicates; `related_notes` silently drops entries
/// that are not existing regular files inside `knowledge_base`.
pub fn parse_answer(final_text: &str, knowledge_base: &Path) -> Result<SummaryAnswer, String> {
    if final_text.len() > MAX_ANSWER_BYTES {
        return Err("the answer is too large".to_string());
    }
    let text = strip_fence(final_text.trim());
    if text.is_empty() {
        return Err("the answer is empty".to_string());
    }
    let value: Value =
        serde_json::from_str(text).map_err(|_| "the answer is not valid JSON".to_string())?;
    let Value::Object(object) = value else {
        return Err("the answer is not a JSON object".to_string());
    };
    // Fields are checked in §6.4 order, so the first error is predictable.
    let document_type = label(&object, "document_type")?;
    let category = label(&object, "category")?;
    let entities = object_field(&object, "entities", "entities")?;
    let entities = Entities {
        technologies: entity_list(entities, "technologies")?,
        products: entity_list(entities, "products")?,
        people: entity_list(entities, "people")?,
        companies: entity_list(entities, "companies")?,
    };
    let description = text_field(&object, "description", "description", MAX_DESCRIPTION_CHARS)?;
    let importance = object_field(&object, "importance", "importance")?;
    let importance = Importance {
        level: level(importance)?,
        reason: line(importance, "reason", "importance.reason", MAX_REASON_CHARS)?,
    };
    Ok(SummaryAnswer {
        document_type,
        category,
        entities,
        description,
        importance,
        assessment: text_field(&object, "assessment", "assessment", MAX_ASSESSMENT_CHARS)?,
        related_notes: related_notes(&object, knowledge_base)?,
    })
}

/// The JSON Schema of the answer, for a harness that accepts one. Lengths
/// are left to [`parse_answer`], so the schema stays within what strict
/// structured-output modes accept.
pub fn answer_schema() -> Value {
    let string = || json!({ "type": "string" });
    let strings = || json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "document_type", "category", "entities", "description",
            "importance", "assessment", "related_notes"
        ],
        "properties": {
            "document_type": string(),
            "category": string(),
            "entities": {
                "type": "object",
                "additionalProperties": false,
                "required": ["technologies", "products", "people", "companies"],
                "properties": {
                    "technologies": strings(),
                    "products": strings(),
                    "people": strings(),
                    "companies": strings()
                }
            },
            "description": string(),
            "importance": {
                "type": "object",
                "additionalProperties": false,
                "required": ["level", "reason"],
                "properties": {
                    "level": { "type": "string", "enum": ["high", "medium", "low"] },
                    "reason": string()
                }
            },
            "assessment": string(),
            "related_notes": strings()
        }
    })
}

/// `text` without one surrounding Markdown code fence (```` ``` ```` or
/// ```` ```json ````), when it has one.
fn strip_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let Some((info, body)) = rest.split_once('\n') else {
        return text;
    };
    if info.contains('`') {
        return text;
    }
    match body.trim_end().strip_suffix("```") {
        Some(inner) => inner.trim(),
        None => text,
    }
}

fn object_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    name: &str,
) -> Result<&'a Map<String, Value>, String> {
    match object.get(key) {
        Some(Value::Object(inner)) => Ok(inner),
        Some(_) => Err(format!("{name} must be an object")),
        None => Err(format!("{name} is missing")),
    }
}

fn string_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    name: &str,
) -> Result<&'a str, String> {
    match object.get(key) {
        Some(Value::String(value)) => Ok(value.trim()),
        Some(_) => Err(format!("{name} must be a string")),
        None => Err(format!("{name} is missing")),
    }
}

/// A trimmed string of 1 to `max` characters.
fn text_field(
    object: &Map<String, Value>,
    key: &str,
    name: &str,
    max: usize,
) -> Result<String, String> {
    let value = string_field(object, key, name)?;
    bounded(value, name, max)?;
    Ok(value.to_string())
}

/// As [`text_field`], with no control characters.
fn line(object: &Map<String, Value>, key: &str, name: &str, max: usize) -> Result<String, String> {
    let value = text_field(object, key, name, max)?;
    no_controls(&value, name)?;
    Ok(value)
}

fn label(object: &Map<String, Value>, key: &str) -> Result<String, String> {
    line(object, key, key, MAX_LABEL_CHARS)
}

fn bounded(value: &str, name: &str, max: usize) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{name} is empty"));
    }
    if value.chars().count() > max {
        return Err(format!("{name} is longer than {max} characters"));
    }
    Ok(())
}

fn no_controls(value: &str, name: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err(format!("{name} contains a control character"));
    }
    Ok(())
}

fn level(importance: &Map<String, Value>) -> Result<ImportanceLevel, String> {
    let name = "importance.level";
    match string_field(importance, "level", name)?
        .to_ascii_lowercase()
        .as_str()
    {
        "high" => Ok(ImportanceLevel::High),
        "medium" => Ok(ImportanceLevel::Medium),
        "low" => Ok(ImportanceLevel::Low),
        _ => Err(format!("{name} must be high, medium, or low")),
    }
}

fn string_list<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    name: &str,
) -> Result<Vec<&'a str>, String> {
    match object.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(value) => Ok(value.trim()),
                _ => Err(format!("{name} must contain only strings")),
            })
            .collect(),
        Some(_) => Err(format!("{name} must be a list of strings")),
        None => Err(format!("{name} is missing")),
    }
}

fn entity_list(entities: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    let name = format!("entities.{key}");
    let mut kept: Vec<String> = Vec::new();
    // Lowercased beside `kept`, and the walk stops past the limit, so an
    // untrusted answer's long list costs at most MAX_ENTITIES per item.
    let mut seen: Vec<String> = Vec::new();
    for item in string_list(entities, key, &name)? {
        if item.is_empty() {
            continue;
        }
        let lower = item.to_lowercase();
        if seen.contains(&lower) {
            continue;
        }
        if kept.len() == MAX_ENTITIES {
            return Err(format!("{name} has more than {MAX_ENTITIES} items"));
        }
        bounded(item, &name, MAX_ENTITY_CHARS)?;
        no_controls(item, &name)?;
        kept.push(item.to_string());
        seen.push(lower);
    }
    Ok(kept)
}

/// The listed notes that are existing regular files inside the knowledge
/// base, as given, at most [`MAX_RELATED_NOTES`].
fn related_notes(
    object: &Map<String, Value>,
    knowledge_base: &Path,
) -> Result<Vec<String>, String> {
    let listed = string_list(object, "related_notes", "related_notes")?;
    let Ok(root) = knowledge_base.canonicalize() else {
        return Ok(Vec::new());
    };
    let mut kept: Vec<String> = Vec::new();
    for note in listed {
        if kept.len() == MAX_RELATED_NOTES {
            break;
        }
        let path = Path::new(note);
        let relative = !note.is_empty()
            && !path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
        if !relative || kept.iter().any(|seen| seen == note) {
            continue;
        }
        let inside = root
            .join(path)
            .canonicalize()
            .is_ok_and(|real| real.starts_with(&root) && real.is_file());
        if inside {
            kept.push(note.to_string());
        }
    }
    Ok(kept)
}

/// A valid one-line answer for tests that run a fake harness.
#[cfg(test)]
pub(crate) const TEST_ANSWER: &str = r#"{"document_type":"report","category":"news","entities":{"technologies":[],"products":[],"people":[],"companies":[]},"description":"A page.","importance":{"level":"high","reason":"It matters."},"assessment":"It adds to the notes.","related_notes":[]}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn knowledge_base() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let kb = dir.path().join("kb");
        std::fs::create_dir_all(kb.join("notes")).unwrap();
        std::fs::write(kb.join("notes/a.md"), "# A\n").unwrap();
        std::fs::write(kb.join("b.md"), "# B\n").unwrap();
        std::fs::write(dir.path().join("outside.md"), "# Outside\n").unwrap();
        (dir, kb)
    }

    fn answer() -> Value {
        json!({
            "document_type": "specification",
            "category": "technical specification",
            "entities": {
                "technologies": ["Rust", "JSON"],
                "products": ["Brauser"],
                "people": [],
                "companies": ["Example Co"]
            },
            "description": "A spec for a summary feature.",
            "importance": { "level": "HIGH", "reason": "It extends the notes on agents." },
            "assessment": "It adds the answer format the notes lack.",
            "related_notes": ["notes/a.md"]
        })
    }

    fn parse(value: &Value, kb: &Path) -> Result<SummaryAnswer, String> {
        parse_answer(&value.to_string(), kb)
    }

    #[test]
    fn valid_answer_parses_and_lowercases_level() {
        let (_dir, kb) = knowledge_base();
        let mut value = answer();
        value["entities"]["products"] = json!(["Brauser", " ", "brauser", "  Codex  "]);
        value["unknown"] = json!("ignored");
        let parsed = parse(&value, &kb).unwrap();
        assert_eq!(parsed.importance.level, ImportanceLevel::High);
        assert_eq!(parsed.document_type, "specification");
        assert_eq!(parsed.entities.products, ["Brauser", "Codex"]);
        assert_eq!(parsed.related_notes, ["notes/a.md"]);
        // Re-serialized, it deserializes strictly with the level lowercased.
        let stored = serde_json::to_value(&parsed).unwrap();
        assert_eq!(stored["importance"]["level"], "high");
        assert_eq!(
            serde_json::from_value::<SummaryAnswer>(stored).unwrap(),
            parsed
        );
    }

    #[test]
    fn fenced_json_is_accepted() {
        let (_dir, kb) = knowledge_base();
        let text = format!("\n```json\n{}\n```\n", answer());
        assert_eq!(
            parse_answer(&text, &kb).unwrap(),
            parse(&answer(), &kb).unwrap()
        );
        let bare = format!("```\n{}\n```", answer());
        assert!(parse_answer(&bare, &kb).is_ok());
        // Prose around the object, or two objects, is still invalid.
        assert!(parse_answer(&format!("Here it is: {}", answer()), &kb).is_err());
        assert!(parse_answer(&format!("{} {}", answer(), answer()), &kb).is_err());
        assert!(parse_answer("[1]", &kb).is_err());
    }

    #[test]
    fn related_notes_outside_or_missing_are_dropped() {
        let (dir, kb) = knowledge_base();
        // A link out of the knowledge base is dropped (and is simply
        // missing where there are no Unix symlinks).
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("outside.md"), kb.join("escape.md")).unwrap();
        let mut value = answer();
        value["related_notes"] = json!([
            "notes/a.md",
            "b.md",
            "notes/a.md",
            "missing.md",
            "notes",
            "../outside.md",
            "notes/../b.md",
            dir.path().join("outside.md").display().to_string(),
            kb.join("b.md").display().to_string(),
            "escape.md",
            ""
        ]);
        let parsed = parse(&value, &kb).unwrap();
        assert_eq!(parsed.related_notes, ["notes/a.md", "b.md"]);

        let many: Vec<String> = (0..40)
            .map(|index| {
                let name = format!("n{index}.md");
                std::fs::write(kb.join(&name), "").unwrap();
                name
            })
            .collect();
        value["related_notes"] = json!(many);
        assert_eq!(
            parse(&value, &kb).unwrap().related_notes.len(),
            MAX_RELATED_NOTES
        );
    }

    #[test]
    fn overlong_description_names_the_field() {
        let (_dir, kb) = knowledge_base();
        let mut value = answer();
        value["description"] = json!("x".repeat(MAX_DESCRIPTION_CHARS + 1));
        let error = parse(&value, &kb).unwrap_err();
        assert!(error.contains("description"), "{error}");
        assert!(!error.contains("xxx"), "the error must not echo the answer");
        // Exactly at the limit, counted in characters, is fine.
        value["description"] = json!("é".repeat(MAX_DESCRIPTION_CHARS));
        assert!(parse(&value, &kb).is_ok());
    }

    #[test]
    fn missing_importance_level_is_invalid() {
        let (_dir, kb) = knowledge_base();
        let mut value = answer();
        value["importance"] = json!({ "reason": "r" });
        let error = parse(&value, &kb).unwrap_err();
        assert!(error.contains("importance.level"), "{error}");
        value["importance"] = json!({ "level": "critical", "reason": "r" });
        let error = parse(&value, &kb).unwrap_err();
        assert!(error.contains("importance.level"), "{error}");
    }

    #[test]
    fn every_field_is_checked_and_named() {
        let (_dir, kb) = knowledge_base();
        let cases: [(&str, Value, &str); 12] = [
            ("/document_type", json!(""), "document_type"),
            ("/document_type", json!("a\u{7}b"), "document_type"),
            (
                "/category",
                json!("c".repeat(MAX_LABEL_CHARS + 1)),
                "category",
            ),
            ("/category", json!(7), "category"),
            ("/entities", json!([]), "entities"),
            ("/entities/people", json!("Ann"), "entities.people"),
            ("/entities/people", json!([1]), "entities.people"),
            (
                "/entities/people",
                json!(["p".repeat(MAX_ENTITY_CHARS + 1)]),
                "entities.people",
            ),
            (
                "/entities/companies",
                json!(
                    (0..=MAX_ENTITIES)
                        .map(|i| format!("c{i}"))
                        .collect::<Vec<_>>()
                ),
                "entities.companies",
            ),
            ("/importance/reason", json!(""), "importance.reason"),
            (
                "/assessment",
                json!("a".repeat(MAX_ASSESSMENT_CHARS + 1)),
                "assessment",
            ),
            ("/related_notes", json!("b.md"), "related_notes"),
        ];
        for (pointer, replacement, field) in cases {
            let mut value = answer();
            *value.pointer_mut(pointer).unwrap() = replacement;
            let error = parse(&value, &kb).unwrap_err();
            assert!(error.contains(field), "{pointer}: {error}");
        }
        let mut value = answer();
        value.as_object_mut().unwrap().remove("assessment");
        assert!(parse(&value, &kb).unwrap_err().contains("assessment"));
    }

    #[test]
    fn entity_list_stops_at_the_first_item_past_the_limit() {
        let (_dir, kb) = knowledge_base();
        // Duplicates in any case do not count toward the limit.
        let mut people: Vec<String> = (0..MAX_ENTITIES).map(|i| format!("P{i}")).collect();
        people.extend((0..MAX_ENTITIES).map(|i| format!("p{i}")));
        let mut value = answer();
        value["entities"]["people"] = json!(people);
        assert_eq!(
            parse(&value, &kb).unwrap().entities.people.len(),
            MAX_ENTITIES
        );
        // One more distinct item fails before anything after it is read,
        // so an untrusted answer's long list costs no more than the limit.
        people.push("extra".to_string());
        people.push("p".repeat(MAX_ENTITY_CHARS + 1));
        value["entities"]["people"] = json!(people);
        let error = parse(&value, &kb).unwrap_err();
        assert!(error.contains("more than"), "{error}");
        let many: Vec<String> = (0..29_000).map(|i| format!("a{i}")).collect();
        value["entities"]["people"] = json!(many);
        let error = parse(&value, &kb).unwrap_err();
        assert!(error.contains("more than"), "{error}");
    }

    #[test]
    fn empty_too_large_or_non_json_answers_are_invalid() {
        let (_dir, kb) = knowledge_base();
        assert!(parse_answer("  \n", &kb).unwrap_err().contains("empty"));
        assert!(parse_answer("not json", &kb).unwrap_err().contains("JSON"));
        let mut value = answer();
        value["padding"] = json!("x".repeat(MAX_ANSWER_BYTES));
        assert!(parse(&value, &kb).unwrap_err().contains("too large"));
    }

    #[test]
    fn schema_requires_every_field_and_the_level_enum() {
        let schema = answer_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect();
        assert_eq!(
            required,
            [
                "document_type",
                "category",
                "entities",
                "description",
                "importance",
                "assessment",
                "related_notes"
            ]
        );
        assert_eq!(
            schema["properties"]["importance"]["properties"]["level"]["enum"],
            json!(["high", "medium", "low"])
        );
        assert_eq!(
            schema["properties"]["entities"]["required"],
            json!(["technologies", "products", "people", "companies"])
        );
    }
}

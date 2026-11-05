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
/// Bounds on the one-sentence overview, in characters.
pub const MAX_OVERVIEW_CHARS: usize = 300;
/// The most key points kept; later ones are dropped.
pub const MAX_KEY_POINTS: usize = 10;
/// Bounds on one key point, in characters.
pub const MAX_KEY_POINT_CHARS: usize = 500;
/// The most keywords kept; later ones are dropped.
pub const MAX_KEYWORDS: usize = 20;
/// Bounds on one metadata value (and one author), in characters.
pub const MAX_METADATA_CHARS: usize = 200;
/// The most references kept; later ones are dropped.
pub const MAX_REFERENCES: usize = 20;
/// Bounds on a reference title, in characters.
pub const MAX_REFERENCE_TITLE_CHARS: usize = 300;
/// The longest reference address kept, in characters.
pub const MAX_REFERENCE_URL_CHARS: usize = 2048;
/// The most related notes kept; later ones are dropped.
pub const MAX_RELATED_NOTES: usize = 32;
/// The most `related_notes` entries looked at, kept or not; later ones are
/// dropped unchecked, so an answer cannot make the service check the
/// filesystem for every entry it fits in [`MAX_ANSWER_BYTES`].
pub const MAX_RELATED_CANDIDATES: usize = 4 * MAX_RELATED_NOTES;

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

/// What the artifact itself states about itself; `""` (or no authors) for
/// a value it does not state.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub authors: Vec<String>,
    pub owner: String,
    pub status: String,
    pub created: String,
    pub updated: String,
    pub version: String,
}

impl Metadata {
    /// The non-empty single values, in answer order, with their names.
    pub fn values(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            ("owner", self.owner.as_str()),
            ("status", self.status.as_str()),
            ("created", self.created.as_str()),
            ("updated", self.updated.as_str()),
            ("version", self.version.as_str()),
        ]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
    }
}

/// Something the artifact cites or depends on (an outgoing source). `url`
/// is an `http`/`https` address or `""`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub title: String,
    pub url: String,
}

/// How much of the artifact the agent read.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadScope {
    Whole,
    Partial,
    PageOnly,
}

impl ReadScope {
    pub fn as_str(self) -> &'static str {
        match self {
            ReadScope::Whole => "whole",
            ReadScope::Partial => "partial",
            ReadScope::PageOnly => "page_only",
        }
    }
}

impl From<ReadScope> for brauser_protocol::AnswerReadScope {
    fn from(scope: ReadScope) -> Self {
        match scope {
            ReadScope::Whole => Self::Whole,
            ReadScope::Partial => Self::Partial,
            ReadScope::PageOnly => Self::PageOnly,
        }
    }
}

/// Whether the agent could see the artifact, or met a wall instead. Anything
/// but `Ok` means the run saves no summary (`finisher::SummaryFinisher`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    #[default]
    Ok,
    SignIn,
    Denied,
    NotFound,
}

/// The validated answer. The fields added with the second `SUMMARY.md`
/// shape default when missing, so a run record an older service wrote
/// still deserializes; `parse_answer` requires them of a new answer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SummaryAnswer {
    #[serde(default)]
    pub overview: String,
    pub description: String,
    #[serde(default)]
    pub key_points: Vec<String>,
    pub document_type: String,
    pub category: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub entities: Entities,
    #[serde(default)]
    pub metadata: Metadata,
    #[serde(default)]
    pub references: Vec<Reference>,
    #[serde(default)]
    pub read_scope: Option<ReadScope>,
    #[serde(default)]
    pub access: Access,
    pub importance: Importance,
    pub assessment: String,
    pub related_notes: Vec<String>,
}

/// Parse and normalize an agent's final message. The error names the field
/// at fault and carries nothing from the answer, so it can go back to the
/// agent in the repair prompt and be shown as the run's error.
///
/// Unknown keys are ignored. Strings are trimmed; entity, keyword and
/// author lists drop blanks and case-insensitive duplicates; key points,
/// keywords and references past their limits are dropped, as is a
/// reference address that is not `http`/`https`; `related_notes` silently
/// drops entries that are not existing regular files inside
/// `knowledge_base`.
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
    // Fields are checked in SUMMARY.md order, so the first error is
    // predictable.
    let overview = line(&object, "overview", "overview", MAX_OVERVIEW_CHARS)?;
    let description = text_field(&object, "description", "description", MAX_DESCRIPTION_CHARS)?;
    let key_points = key_points(&object)?;
    let document_type = label(&object, "document_type")?;
    let category = label(&object, "category")?;
    let keywords = distinct_lines(
        string_list(&object, "keywords", "keywords")?,
        "keywords",
        MAX_KEYWORDS,
        MAX_LABEL_CHARS,
        Overflow::Drop,
    )?;
    let entities = object_field(&object, "entities", "entities")?;
    let entities = Entities {
        technologies: entity_list(entities, "technologies")?,
        products: entity_list(entities, "products")?,
        people: entity_list(entities, "people")?,
        companies: entity_list(entities, "companies")?,
    };
    let metadata = metadata(object_field(&object, "metadata", "metadata")?)?;
    let references = references(&object)?;
    let read_scope = read_scope(&object)?;
    let access = access(&object)?;
    if access != Access::Ok && read_scope != ReadScope::PageOnly {
        return Err(
            "access and read_scope contradict each other: an access wall requires page_only".into(),
        );
    }
    let importance = object_field(&object, "importance", "importance")?;
    let importance = Importance {
        level: level(importance)?,
        reason: line(importance, "reason", "importance.reason", MAX_REASON_CHARS)?,
    };
    Ok(SummaryAnswer {
        overview,
        description,
        key_points,
        document_type,
        category,
        keywords,
        entities,
        metadata,
        references,
        read_scope: Some(read_scope),
        access,
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
            "overview", "description", "key_points", "document_type",
            "category", "keywords", "entities", "metadata", "references",
            "read_scope", "access", "importance", "assessment", "related_notes"
        ],
        "properties": {
            "overview": string(),
            "description": string(),
            "key_points": strings(),
            "document_type": string(),
            "category": string(),
            "keywords": strings(),
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
            "metadata": {
                "type": "object",
                "additionalProperties": false,
                "required": ["authors", "owner", "status", "created", "updated", "version"],
                "properties": {
                    "authors": strings(),
                    "owner": string(),
                    "status": string(),
                    "created": string(),
                    "updated": string(),
                    "version": string()
                }
            },
            "references": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "url"],
                    "properties": { "title": string(), "url": string() }
                }
            },
            "read_scope": { "type": "string", "enum": ["whole", "partial", "page_only"] },
            "access": { "type": "string", "enum": ["ok", "sign_in", "denied", "not_found"] },
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
pub(crate) fn strip_fence(text: &str) -> &str {
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

/// What a list does with an item past its limit.
#[derive(Clone, Copy)]
enum Overflow {
    /// The answer is invalid.
    Fail,
    /// The item and the rest are dropped.
    Drop,
}

/// `items` without blanks and case-insensitive duplicates, each a single
/// line of at most `max_chars`, at most `limit` of them.
fn distinct_lines(
    items: Vec<&str>,
    name: &str,
    limit: usize,
    max_chars: usize,
    overflow: Overflow,
) -> Result<Vec<String>, String> {
    let mut kept: Vec<String> = Vec::new();
    // Lowercased beside `kept`, and the walk stops past the limit, so an
    // untrusted answer's long list costs at most `limit` per item.
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        if item.is_empty() {
            continue;
        }
        let lower = item.to_lowercase();
        if seen.contains(&lower) {
            continue;
        }
        if kept.len() == limit {
            return match overflow {
                Overflow::Fail => Err(format!("{name} has more than {limit} items")),
                Overflow::Drop => Ok(kept),
            };
        }
        bounded(item, name, max_chars)?;
        no_controls(item, name)?;
        kept.push(item.to_string());
        seen.push(lower);
    }
    Ok(kept)
}

fn entity_list(entities: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    let name = format!("entities.{key}");
    distinct_lines(
        string_list(entities, key, &name)?,
        &name,
        MAX_ENTITIES,
        MAX_ENTITY_CHARS,
        Overflow::Fail,
    )
}

/// One to [`MAX_KEY_POINTS`] single-line points; later ones are dropped.
fn key_points(object: &Map<String, Value>) -> Result<Vec<String>, String> {
    let name = "key_points";
    let points = distinct_lines(
        string_list(object, name, name)?,
        name,
        MAX_KEY_POINTS,
        MAX_KEY_POINT_CHARS,
        Overflow::Drop,
    )?;
    if points.is_empty() {
        return Err(format!("{name} is empty"));
    }
    Ok(points)
}

/// A single line of at most `max` characters, or `""`.
fn optional_line(
    object: &Map<String, Value>,
    key: &str,
    name: &str,
    max: usize,
) -> Result<String, String> {
    let value = string_field(object, key, name)?;
    if value.is_empty() {
        return Ok(String::new());
    }
    bounded(value, name, max)?;
    no_controls(value, name)?;
    Ok(value.to_string())
}

fn metadata(object: &Map<String, Value>) -> Result<Metadata, String> {
    let value =
        |key: &str| optional_line(object, key, &format!("metadata.{key}"), MAX_METADATA_CHARS);
    Ok(Metadata {
        authors: distinct_lines(
            string_list(object, "authors", "metadata.authors")?,
            "metadata.authors",
            MAX_ENTITIES,
            MAX_METADATA_CHARS,
            Overflow::Drop,
        )?,
        owner: value("owner")?,
        status: value("status")?,
        created: value("created")?,
        updated: value("updated")?,
        version: value("version")?,
    })
}

/// An `http`/`https` address with no whitespace or control characters,
/// else `""`. The address is the agent's; it is only ever written as a
/// link destination (`summary::render_summary`).
fn reference_url(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let web = (lower.starts_with("https://") && url.len() > "https://".len())
        || (lower.starts_with("http://") && url.len() > "http://".len());
    if web
        && url.chars().count() <= MAX_REFERENCE_URL_CHARS
        && !url.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    {
        url.to_string()
    } else {
        String::new()
    }
}

/// At most [`MAX_REFERENCES`] references with a title; untitled entries
/// and later ones are dropped, and so is an address that is not a web one.
fn references(object: &Map<String, Value>) -> Result<Vec<Reference>, String> {
    let name = "references";
    let items = match object.get(name) {
        Some(Value::Array(items)) => items,
        Some(_) => return Err(format!("{name} must be a list")),
        None => return Err(format!("{name} is missing")),
    };
    let mut kept: Vec<Reference> = Vec::new();
    for item in items {
        let Value::Object(reference) = item else {
            return Err(format!("{name} must contain only objects"));
        };
        let title = optional_line(
            reference,
            "title",
            "references.title",
            MAX_REFERENCE_TITLE_CHARS,
        )?;
        let url = reference_url(string_field(reference, "url", "references.url")?);
        if title.is_empty() {
            continue;
        }
        kept.push(Reference { title, url });
        if kept.len() == MAX_REFERENCES {
            break;
        }
    }
    Ok(kept)
}

fn access(object: &Map<String, Value>) -> Result<Access, String> {
    let name = "access";
    match string_field(object, name, name)?
        .to_ascii_lowercase()
        .as_str()
    {
        "ok" => Ok(Access::Ok),
        "sign_in" => Ok(Access::SignIn),
        "denied" => Ok(Access::Denied),
        "not_found" => Ok(Access::NotFound),
        _ => Err(format!("{name} must be ok, sign_in, denied, or not_found")),
    }
}

fn read_scope(object: &Map<String, Value>) -> Result<ReadScope, String> {
    let name = "read_scope";
    match string_field(object, name, name)?
        .to_ascii_lowercase()
        .as_str()
    {
        "whole" => Ok(ReadScope::Whole),
        "partial" => Ok(ReadScope::Partial),
        "page_only" => Ok(ReadScope::PageOnly),
        _ => Err(format!("{name} must be whole, partial, or page_only")),
    }
}

/// The listed notes that are existing regular files inside the knowledge
/// base, as given, at most [`MAX_RELATED_NOTES`] from the first
/// [`MAX_RELATED_CANDIDATES`] entries.
fn related_notes(
    object: &Map<String, Value>,
    knowledge_base: &Path,
) -> Result<Vec<String>, String> {
    let listed = string_list(object, "related_notes", "related_notes")?;
    let Ok(root) = knowledge_base.canonicalize() else {
        return Ok(Vec::new());
    };
    let mut kept: Vec<String> = Vec::new();
    for note in listed.into_iter().take(MAX_RELATED_CANDIDATES) {
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
pub(crate) const TEST_ANSWER: &str = r#"{"overview":"A page about a thing.","description":"A page.","key_points":["It says one thing."],"document_type":"report","category":"news","keywords":["thing"],"entities":{"technologies":[],"products":[],"people":[],"companies":[]},"metadata":{"authors":[],"owner":"","status":"","created":"","updated":"","version":""},"references":[],"read_scope":"whole","access":"ok","importance":{"level":"high","reason":"It matters."},"assessment":"It adds to the notes.","related_notes":[]}"#;

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
            "overview": "The summary feature's spec, which fixes the answer format.",
            "description": "A spec for a summary feature.",
            "key_points": ["Answers are JSON.", "Every field is validated."],
            "document_type": "specification",
            "category": "technical specification",
            "keywords": ["summary", "answer format"],
            "metadata": {
                "authors": ["Ann Lee"], "owner": "", "status": "draft",
                "created": "", "updated": "2026-10-01", "version": ""
            },
            "references": [{ "title": "Answer schema", "url": "https://example.com/schema" }],
            "read_scope": "whole",
            "access": "ok",
            "entities": {
                "technologies": ["Rust", "JSON"],
                "products": ["Brauser"],
                "people": [],
                "companies": ["Example Co"]
            },
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

    /// Each candidate costs a filesystem check, so only the first
    /// [`MAX_RELATED_CANDIDATES`] are looked at, whatever they are.
    #[test]
    fn related_notes_examine_a_bounded_number_of_candidates() {
        let (_dir, kb) = knowledge_base();
        let mut listed: Vec<String> = (0..MAX_RELATED_CANDIDATES - 1)
            .map(|index| format!("missing-{index}.md"))
            .collect();
        listed.push("b.md".to_string());
        listed.push("notes/a.md".to_string());
        let mut value = answer();
        value["related_notes"] = json!(listed);
        assert_eq!(parse(&value, &kb).unwrap().related_notes, ["b.md"]);

        // Escaping entries count as candidates too.
        let mut listed = vec!["../outside.md".to_string(); 10_000];
        listed.push("b.md".to_string());
        value["related_notes"] = json!(listed);
        assert!(parse(&value, &kb).unwrap().related_notes.is_empty());
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
        let cases: [(&str, Value, &str); 23] = [
            ("/overview", json!(""), "overview"),
            ("/overview", json!("a\nb"), "overview"),
            (
                "/overview",
                json!("o".repeat(MAX_OVERVIEW_CHARS + 1)),
                "overview",
            ),
            ("/key_points", json!([]), "key_points"),
            ("/key_points", json!([" "]), "key_points"),
            ("/key_points", json!(["a\u{7}b"]), "key_points"),
            ("/keywords", json!("k"), "keywords"),
            ("/metadata", json!([]), "metadata"),
            ("/metadata/status", json!(null), "metadata.status"),
            ("/references", json!([{ "title": "t" }]), "references.url"),
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
            ("/read_scope", json!("most"), "read_scope"),
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
    fn new_fields_are_normalized() {
        let (_dir, kb) = knowledge_base();
        let mut value = answer();
        value["read_scope"] = json!("PAGE_ONLY");
        value["keywords"] = json!(["Rust", "rust", " ", "JSON"]);
        value["metadata"]["authors"] = json!(["Ann", "ann", ""]);
        value["metadata"]["owner"] = json!("  ");
        value["references"] = json!([
            { "title": "Kept", "url": "https://example.com/a" },
            { "title": "", "url": "https://example.com/untitled" },
            { "title": "Local path", "url": "file:///etc/passwd" },
            { "title": "Script", "url": "javascript:alert(1)" },
            { "title": "Spaced", "url": "https://example.com/a b" },
            { "title": "No address", "url": "" }
        ]);
        let parsed = parse(&value, &kb).unwrap();
        assert_eq!(parsed.read_scope, Some(ReadScope::PageOnly));
        assert_eq!(parsed.keywords, ["Rust", "JSON"]);
        assert_eq!(parsed.metadata.authors, ["Ann"]);
        assert_eq!(parsed.metadata.owner, "");
        assert_eq!(
            parsed.metadata.values().collect::<Vec<_>>(),
            [("status", "draft"), ("updated", "2026-10-01")]
        );
        let references: Vec<(&str, &str)> = parsed
            .references
            .iter()
            .map(|reference| (reference.title.as_str(), reference.url.as_str()))
            .collect();
        assert_eq!(
            references,
            [
                ("Kept", "https://example.com/a"),
                ("Local path", ""),
                ("Script", ""),
                ("Spaced", ""),
                ("No address", "")
            ]
        );
    }

    #[test]
    fn long_optional_lists_are_cut_not_failed() {
        let (_dir, kb) = knowledge_base();
        let mut value = answer();
        value["key_points"] = json!((0..50).map(|i| format!("Point {i}.")).collect::<Vec<_>>());
        value["keywords"] = json!((0..50).map(|i| format!("k{i}")).collect::<Vec<_>>());
        value["references"] = json!(
            (0..50)
                .map(|i| json!({ "title": format!("r{i}"), "url": "" }))
                .collect::<Vec<_>>()
        );
        let parsed = parse(&value, &kb).unwrap();
        assert_eq!(parsed.key_points.len(), MAX_KEY_POINTS);
        assert_eq!(parsed.keywords.len(), MAX_KEYWORDS);
        assert_eq!(parsed.references.len(), MAX_REFERENCES);
        assert_eq!(parsed.references[0].title, "r0");
    }

    #[test]
    fn access_is_required_and_matched_case_insensitively() {
        let (_dir, kb) = knowledge_base();
        let mut value = answer();
        value["read_scope"] = json!("page_only");
        for (wire, access) in [
            ("ok", Access::Ok),
            ("SIGN_IN", Access::SignIn),
            ("denied", Access::Denied),
            ("Not_Found", Access::NotFound),
        ] {
            value["access"] = json!(wire);
            assert_eq!(parse(&value, &kb).unwrap().access, access);
        }
        value["access"] = json!("blocked");
        assert_eq!(
            parse(&value, &kb).unwrap_err(),
            "access must be ok, sign_in, denied, or not_found"
        );
        value.as_object_mut().unwrap().remove("access");
        assert!(parse(&value, &kb).unwrap_err().contains("access"));
        let schema = answer_schema();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("access"))
        );
        assert_eq!(
            schema["properties"]["access"]["enum"],
            json!(["ok", "sign_in", "denied", "not_found"])
        );
    }

    #[test]
    fn access_walls_cannot_claim_artifact_reading() {
        let (_dir, kb) = knowledge_base();
        for access in ["sign_in", "DENIED", "Not_Found"] {
            for scope in ["whole", "PARTIAL"] {
                let mut value = answer();
                value["access"] = json!(access);
                value["read_scope"] = json!(scope);
                assert_eq!(
                    parse(&value, &kb).unwrap_err(),
                    "access and read_scope contradict each other: an access wall requires page_only",
                    "{access}/{scope}"
                );
            }
        }
    }

    #[test]
    fn readable_artifacts_allow_every_reading_scope() {
        let (_dir, kb) = knowledge_base();
        for scope in ["whole", "partial", "page_only"] {
            let mut value = answer();
            value["read_scope"] = json!(scope);
            assert_eq!(parse(&value, &kb).unwrap().access, Access::Ok);
        }
    }

    #[test]
    fn an_old_record_reads_access_as_ok() {
        let old = json!({
            "document_type": "report",
            "category": "news",
            "entities": {"technologies": [], "products": [], "people": [], "companies": []},
            "description": "d",
            "importance": {"level": "low", "reason": "r"},
            "assessment": "a",
            "related_notes": []
        });
        let answer: SummaryAnswer = serde_json::from_value(old).unwrap();
        assert_eq!(answer.access, Access::Ok);
    }

    #[test]
    fn a_record_from_before_the_new_fields_still_deserializes() {
        let old = json!({
            "document_type": "report",
            "category": "news",
            "entities": {"technologies": [], "products": [], "people": [], "companies": []},
            "description": "d",
            "importance": {"level": "low", "reason": "r"},
            "assessment": "a",
            "related_notes": []
        });
        let answer: SummaryAnswer = serde_json::from_value(old).unwrap();
        assert_eq!(answer.overview, "");
        assert!(answer.key_points.is_empty() && answer.references.is_empty());
        assert_eq!(answer.metadata, Metadata::default());
        assert_eq!(answer.read_scope, None);
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
                "overview",
                "description",
                "key_points",
                "document_type",
                "category",
                "keywords",
                "entities",
                "metadata",
                "references",
                "read_scope",
                "access",
                "importance",
                "assessment",
                "related_notes"
            ]
        );
        // Every property is required, as strict structured-output modes need.
        let properties: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();
        assert_eq!(properties.len(), required.len());
        assert_eq!(
            schema["properties"]["read_scope"]["enum"],
            json!(["whole", "partial", "page_only"])
        );
        assert_eq!(
            schema["properties"]["metadata"]["required"],
            json!([
                "authors", "owner", "status", "created", "updated", "version"
            ])
        );
        assert_eq!(
            schema["properties"]["references"]["items"]["required"],
            json!(["title", "url"])
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

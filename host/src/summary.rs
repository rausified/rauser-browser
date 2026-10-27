//! Summaries (spec §7): the Brauser-owned summary file and the `summary`
//! daily-log event that records it.
//!
//! The file side names, owns and writes one summary per record key, beside
//! the page note: `summaries/<note stem>.summary.md`, whose leading
//! frontmatter `brauser:` block is `kind: summary`, `canonical_url`, then
//! `url_id` or `artifact_id` (DESIGN.md §5.2, §6.6). `owned_summary` reads
//! only that block, and `Vault::write_summary` replaces a file only when it
//! passes, so a note, another record's summary, or the user's own file is a
//! conflict and is never adopted. `render_summary` builds the file's text
//! from PR 3's validated `SummaryAnswer` (spec §7.1), escaping agent text
//! for YAML and neutralizing it for Markdown (DESIGN.md §5.2 step 6). This
//! module never invokes a harness.
//!
//! The event side is storage-layer support for "Summarize & log" (§7.2): a
//! page with no matching adapter becomes a `web.page` artifact, keyed by its
//! own URL, and `summary_path` names the summary file written above.

use anyhow::Result;
use brauser_protocol::{HarnessId, ImportanceLevel, RecordKey, RecordKeyKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path};
use url::Url;

use crate::answer::{MAX_ENTITY_CHARS, MAX_LABEL_CHARS, MAX_REASON_CHARS, SummaryAnswer};
use crate::markdown::{LinkStyle, inline, neutralize_remote, wikilink, yaml_string};
use crate::presentation::plain_line;
use crate::vault::{note_filename, owned_frontmatter};

pub const MAX_TITLE_BYTES: usize = 2048;
pub const MAX_DOCUMENT_TYPE_BYTES: usize = 64;
pub const MAX_CATEGORY_BYTES: usize = 64;
pub const MAX_SUMMARY_PATH_BYTES: usize = 1024;

/// What a caller supplies to record a summary. The host derives the
/// `web.page` artifact identity from `url`; callers never supply one.
#[derive(Clone, Debug)]
pub struct PageSummary {
    pub event_id: String,
    pub url: String,
    pub title: String,
    pub occurred_at: String,
    pub document_type: String,
    pub category: String,
    pub importance: String,
    pub summary_path: String,
}

/// The durable, indexed form of a `PageSummary`: the marker payload stored in
/// the daily log and replayed by `ArtifactIndex`. `url` and `artifact_id`
/// hold the normalized URL and the identity derived from it, not whatever a
/// caller originally supplied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryEvent {
    pub event_id: String,
    pub artifact_id: String,
    pub artifact_type: String,
    pub url: String,
    pub title: String,
    pub occurred_at: String,
    pub document_type: String,
    pub category: String,
    pub importance: String,
    pub summary_path: String,
}

pub(crate) fn validate_page_summary(summary: &PageSummary) -> Result<(), &'static str> {
    if summary.title.trim().is_empty() || summary.title.len() > MAX_TITLE_BYTES {
        return Err("invalid_summary_title");
    }
    if summary.document_type.trim().is_empty()
        || summary.document_type.len() > MAX_DOCUMENT_TYPE_BYTES
    {
        return Err("invalid_summary_document_type");
    }
    if summary.category.trim().is_empty() || summary.category.len() > MAX_CATEGORY_BYTES {
        return Err("invalid_summary_category");
    }
    if !matches!(summary.importance.as_str(), "high" | "medium" | "low") {
        return Err("invalid_summary_importance");
    }
    check_summary_path(&summary.summary_path)
}

/// The `summary_path` rule shared by `validate_page_summary` and
/// `Vault::summary_relative_path`: a vault-relative `/`-separated path to a
/// `.summary.md` file that a summary event's link can name unchanged.
pub(crate) fn check_summary_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty()
        || path.len() > MAX_SUMMARY_PATH_BYTES
        || path.starts_with('/')
        || path.split('/').any(|part| part == "..")
        || !path.ends_with(".summary.md")
        // The rendered link wraps this in `<...>` (CommonMark's bracketed
        // destination form), which forbids line breaks and unescaped `<`/
        // `>`; `#`/`?` are not special to CommonMark there, but some
        // Markdown viewers (DESIGN.md's Obsidian/VS Code preview bar) treat
        // a link destination as a URI reference and would resolve a
        // trailing fragment or query instead of opening the file. Reject
        // rather than escape: a vault-relative path has no legitimate
        // reason to need any of these.
        || path
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | '#' | '?'))
    {
        return Err("invalid_summary_path");
    }
    Ok(())
}

/// Re-validate a `SummaryEvent` replayed from the log: everything
/// `validate_page_summary` checks, plus the identity invariant that only
/// exists once a URL has been normalized and hashed.
pub(crate) fn validate_summary_event(event: &SummaryEvent) -> bool {
    let as_summary = PageSummary {
        event_id: event.event_id.clone(),
        url: event.url.clone(),
        title: event.title.clone(),
        occurred_at: event.occurred_at.clone(),
        document_type: event.document_type.clone(),
        category: event.category.clone(),
        importance: event.importance.clone(),
        summary_path: event.summary_path.clone(),
    };
    if validate_page_summary(&as_summary).is_err() {
        return false;
    }
    event.artifact_type == "web.page"
        && web_page_artifact_id(&event.url).as_deref() == Ok(event.artifact_id.as_str())
}

/// The `web:<host>/<sha256(url)>` identity `host/src/index.rs`'s legacy
/// visits already use for an unrecognized page. Summaries reuse it exactly
/// so a page that later gains both a summary and a legacy visit share one
/// artifact.
pub(crate) fn web_page_artifact_id(normalized_url: &str) -> Result<String, &'static str> {
    let parsed = Url::parse(normalized_url).map_err(|_| "invalid_summary_url")?;
    let host = parsed.host_str().ok_or("invalid_summary_url")?;
    let hash = hex::encode(Sha256::digest(normalized_url.as_bytes()));
    Ok(format!("web:{host}/{hash}"))
}

/// The summary file's name: the page note's stem plus `.summary.md`, so a
/// summary and a note for one record share a stem and never a name. A URL key
/// must match `canonical_url`, exactly as for the note.
#[cfg_attr(not(test), allow(dead_code))]
pub fn summary_filename(key: &RecordKey, canonical_url: &str) -> Result<String> {
    let note = note_filename(key, canonical_url)?;
    let stem = note.strip_suffix(".md").unwrap_or(&note);
    Ok(format!("{stem}.summary.md"))
}

/// True only for a summary file of `key`: its leading frontmatter holds one
/// `brauser:` block with `kind: summary` and this record's `url_id` or
/// `artifact_id`, and not the other key. Body text is never read, so agent
/// text cannot change a file's identity. `canonical_url` is not part of the
/// check: an artifact's summary keeps one name across its URLs.
#[cfg_attr(not(test), allow(dead_code))]
pub fn owned_summary(bytes: &[u8], key: &RecordKey) -> bool {
    let (id, forbidden) = match key.kind {
        RecordKeyKind::Url => ("url_id", "artifact_id"),
        RecordKeyKind::Artifact => ("artifact_id", "url_id"),
    };
    owned_frontmatter(bytes, "summary", &[(id, key.value.as_str())], &[forbidden])
}

/// Where the run that wrote a summary can be resumed: a Codex thread
/// (`codex://threads/<id>`) or a Claude Code session.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionLink {
    CodexThread(String),
    ClaudeSession(String),
    None,
}

/// Everything a summary file holds (spec §7.1). `note_link` is the page
/// note link already built by `Vault::note_link` in the same link style.
#[cfg_attr(not(test), allow(dead_code))]
pub struct SummaryDocument<'a> {
    pub key: &'a RecordKey,
    pub canonical_url: &'a str,
    pub title: &'a str,
    pub summarized_at: &'a str,
    pub harness: HarnessId,
    pub session: SessionLink,
    pub note_link: String,
    pub answer: &'a SummaryAnswer,
}

/// The importance level's wire name, as `ImportanceLevel` serializes.
#[cfg_attr(not(test), allow(dead_code))]
pub fn importance_str(level: ImportanceLevel) -> &'static str {
    match level {
        ImportanceLevel::High => "high",
        ImportanceLevel::Medium => "medium",
        ImportanceLevel::Low => "low",
    }
}

/// The harness's wire name, as `HarnessId` serializes (not the host's
/// `Adapter::harness_id`, which says `claude-code`).
fn harness_str(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::Codex => "codex",
        HarnessId::Claude => "claude",
    }
}

/// The most characters of one frontmatter value; agent fields are bounded
/// well below this by `crate::answer`.
const MAX_FRONTMATTER_CHARS: usize = 2048;
/// The most characters of a related note path in the body.
const MAX_RELATED_NOTE_CHARS: usize = 1024;

/// An untrusted frontmatter value: every control character (C0, DEL and
/// C1) and invisible format character becomes a space, as
/// `presentation::plain_line` writes it, and the result is a JSON string
/// (`yaml_string`). No Markdown entities: Obsidian's properties and
/// Dataview read YAML values literally.
fn yaml_text(value: &str) -> String {
    yaml_string(&plain_line(value, MAX_FRONTMATTER_CHARS))
}

/// A YAML flow sequence of `yaml_text` values.
fn yaml_list(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|item| yaml_text(item)).collect();
    format!("[{}]", items.join(", "))
}

/// The page URL as a bracketed link destination. The canonical URL is
/// host-normalized, but the page chooses it, and WHATWG serialization leaves
/// a backtick, `$`, `!`, `[` and `]` raw in the query. Those become numeric
/// entities, as do `%`, `=` and `~` next to the same character (an Obsidian
/// comment, highlight or strikethrough), so no code span, math, image or
/// embed syntax reaches Dataview, a math engine or Obsidian's own passes.
/// CommonMark decodes entities in a destination, so the link target is
/// unchanged. Controls, whitespace, `<`, `>` and `\\` are percent-encoded to
/// guard the destination's own syntax.
fn page_destination(url: &str) -> String {
    let chars: Vec<char> = url.chars().collect();
    let mut out = String::with_capacity(url.len());
    for (at, &ch) in chars.iter().enumerate() {
        let doubled = |ch: char| (at > 0 && chars[at - 1] == ch) || chars.get(at + 1) == Some(&ch);
        let entity = match ch {
            '`' | '$' | '!' | '[' | ']' => true,
            '%' | '=' | '~' => doubled(ch),
            _ => false,
        };
        if entity {
            out.push_str(&format!("&#{};", u32::from(ch)));
        } else if ch.is_control() || ch.is_whitespace() || matches!(ch, '<' | '>' | '\\') {
            let mut buffer = [0u8; 4];
            for byte in ch.encode_utf8(&mut buffer).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// A wikilink to `target` when it can carry `target` unchanged, and the
/// escaped single-line text otherwise, so a name with `#`, `|` or `[` never
/// links to some other note. A backslash (which can escape the closing
/// bracket), `%%` (an Obsidian comment), `==` (a highlight) or `~~` inside
/// the link also falls back, since `markdown::wikilink` keeps those.
fn wikilink_or_text(target: &str, text: &str, limit: usize) -> String {
    let target = target.trim();
    let link = wikilink(target, None);
    let syntax =
        target.contains('\\') || ["%%", "==", "~~"].iter().any(|pair| target.contains(pair));
    if !syntax && link == format!("[[{target}]]") {
        link
    } else {
        inline(text, limit)
    }
}

/// A related note path with `/` separators and no `.` components, so the
/// Related line is portable and Obsidian resolves the wikilink
/// (`answer::related_notes` keeps the path as the agent gave it, which on
/// Windows may use `\\`).
fn portable_note(note: &str) -> String {
    let parts: Vec<String> = Path::new(note)
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        note.to_owned()
    } else {
        parts.join("/")
    }
}

/// The summary file (spec §7.1, DESIGN.md §5.2): the `brauser:` identity
/// block, the answer's fields as frontmatter, then a body of links, the
/// labels line, entities, the description and assessment, and related
/// notes.
///
/// Agent text is made inert where it lands (DESIGN.md §5.2 step 6):
/// frontmatter values through `yaml_text`; single-line fields (type,
/// category, importance reason, entities, related notes) through
/// `markdown::inline`, or as wikilinks in the `Wikilink` style;
/// `description` and `assessment` through `markdown::neutralize_remote`,
/// each starting a line right after a host-written newline. The identity
/// values (`canonical_url`, the record key) are written exactly, so
/// `owned_summary` reads them back.
#[cfg_attr(not(test), allow(dead_code))]
pub fn render_summary(doc: &SummaryDocument<'_>, style: LinkStyle) -> String {
    let answer = doc.answer;
    let id_key = match doc.key.kind {
        RecordKeyKind::Url => "url_id",
        RecordKeyKind::Artifact => "artifact_id",
    };
    let entities = &answer.entities;
    let groups = [
        ("technologies", "Technologies", &entities.technologies),
        ("products", "Products", &entities.products),
        ("people", "People", &entities.people),
        ("companies", "Companies", &entities.companies),
    ];

    let mut out = String::new();
    out.push_str("---\nbrauser:\n  kind: summary\n");
    out.push_str(&format!(
        "  canonical_url: {}\n",
        yaml_string(doc.canonical_url)
    ));
    out.push_str(&format!("  {id_key}: {}\n", yaml_string(&doc.key.value)));
    out.push_str(&format!(
        "canonical_url: {}\n",
        yaml_string(doc.canonical_url)
    ));
    out.push_str(&format!("title: {}\n", yaml_text(doc.title)));
    out.push_str(&format!(
        "summarized_at: {}\n",
        yaml_text(doc.summarized_at)
    ));
    out.push_str(&format!(
        "document_type: {}\n",
        yaml_text(&answer.document_type)
    ));
    out.push_str(&format!("category: {}\n", yaml_text(&answer.category)));
    out.push_str(&format!(
        "importance: {}\n",
        importance_str(answer.importance.level)
    ));
    out.push_str("entities:\n");
    for (key, _, items) in &groups {
        out.push_str(&format!("  {key}: {}\n", yaml_list(items)));
    }
    out.push_str(&format!("harness: {}\n", harness_str(doc.harness)));
    match &doc.session {
        SessionLink::CodexThread(id) => out.push_str(&format!(
            "thread: {}\n",
            yaml_text(&format!("codex://threads/{id}"))
        )),
        SessionLink::ClaudeSession(id) => {
            out.push_str(&format!("claude_session: {}\n", yaml_text(id)));
        }
        SessionLink::None => {}
    }
    out.push_str("---\n");

    out.push_str(&format!(
        "[Page](<{}>) · {}\n\n",
        page_destination(doc.canonical_url),
        doc.note_link
    ));
    out.push_str(&format!(
        "**Type:** {} · **Category:** {} · **Importance:** {} — {}\n\n",
        inline(&answer.document_type, MAX_LABEL_CHARS),
        inline(&answer.category, MAX_LABEL_CHARS),
        importance_str(answer.importance.level),
        inline(&answer.importance.reason, MAX_REASON_CHARS),
    ));
    let entity = |name: &String| match style {
        LinkStyle::Relative => inline(name, MAX_ENTITY_CHARS),
        LinkStyle::Wikilink => wikilink_or_text(name, name, MAX_ENTITY_CHARS),
    };
    let lines: Vec<String> = groups
        .iter()
        .filter(|(_, _, items)| !items.is_empty())
        .map(|(_, label, items)| {
            let names: Vec<String> = items.iter().map(entity).collect();
            format!("{label}: {}", names.join(", "))
        })
        .collect();
    if !lines.is_empty() {
        out.push_str(&format!("**Entities:** {}\n\n", lines.join(" · ")));
    }
    // Each neutralized text starts a line after a host newline, never after
    // host text ending in a space (DESIGN.md §5.2 step 6's joining rule).
    out.push_str("## Summary\n");
    out.push_str(&neutralize_remote(&answer.description));
    out.push_str("\n\n## Assessment\n");
    out.push_str(&neutralize_remote(&answer.assessment));
    out.push('\n');
    if !answer.related_notes.is_empty() {
        let related: Vec<String> = answer
            .related_notes
            .iter()
            .map(|note| {
                let note = portable_note(note);
                match style {
                    LinkStyle::Relative => inline(&note, MAX_RELATED_NOTE_CHARS),
                    LinkStyle::Wikilink => wikilink_or_text(
                        note.strip_suffix(".md").unwrap_or(&note),
                        &note,
                        MAX_RELATED_NOTE_CHARS,
                    ),
                }
            })
            .collect();
        out.push_str(&format!("\nRelated: {}\n", related.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> PageSummary {
        PageSummary {
            event_id: "11111111-1111-1111-1111-111111111111".into(),
            url: "https://news.ycombinator.com/item?id=50251344".into(),
            title: "A good discussion".into(),
            occurred_at: "2026-10-26T18:00:00Z".into(),
            document_type: "discussion".into(),
            category: "reading".into(),
            importance: "medium".into(),
            summary_path: "pages/news-ycombinator-com.summary.md".into(),
        }
    }

    #[test]
    fn a_well_formed_summary_is_valid() {
        assert!(validate_page_summary(&summary()).is_ok());
    }

    #[test]
    fn an_unknown_importance_is_rejected() {
        let mut bad = summary();
        bad.importance = "urgent".into();
        assert_eq!(
            validate_page_summary(&bad),
            Err("invalid_summary_importance")
        );
    }

    #[test]
    fn a_summary_path_outside_the_vault_is_rejected() {
        let mut bad = summary();
        bad.summary_path = "../../etc/passwd.summary.md".into();
        assert_eq!(validate_page_summary(&bad), Err("invalid_summary_path"));
    }

    #[test]
    fn a_summary_path_without_the_summary_suffix_is_rejected() {
        let mut bad = summary();
        bad.summary_path = "pages/news-ycombinator-com.md".into();
        assert_eq!(validate_page_summary(&bad), Err("invalid_summary_path"));
    }

    #[test]
    fn a_summary_path_with_a_link_breaking_character_is_rejected() {
        for path in [
            "pages/>evil.summary.md",
            "pages/<script>.summary.md",
            "pages/evil\n.summary.md",
            "pages/evil#frag.summary.md",
            "pages/evil?query.summary.md",
        ] {
            let mut bad = summary();
            bad.summary_path = path.into();
            assert_eq!(
                validate_page_summary(&bad),
                Err("invalid_summary_path"),
                "{path} should be rejected"
            );
        }
    }

    #[test]
    fn web_page_artifact_id_is_stable_for_the_same_url() {
        let first = web_page_artifact_id("https://news.ycombinator.com/item?id=50251344").unwrap();
        let second = web_page_artifact_id("https://news.ycombinator.com/item?id=50251344").unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("web:news.ycombinator.com/"));
    }

    #[test]
    fn web_page_artifact_id_differs_by_full_url_not_just_host() {
        let a = web_page_artifact_id("https://news.ycombinator.com/item?id=1").unwrap();
        let b = web_page_artifact_id("https://news.ycombinator.com/item?id=2").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_summary_event_with_a_forged_artifact_id_is_invalid() {
        let id = web_page_artifact_id("https://news.ycombinator.com/item?id=50251344").unwrap();
        let event = SummaryEvent {
            event_id: summary().event_id,
            artifact_id: "web:evil.example/0000".into(),
            artifact_type: "web.page".into(),
            url: "https://news.ycombinator.com/item?id=50251344".into(),
            title: summary().title,
            occurred_at: summary().occurred_at,
            document_type: summary().document_type,
            category: summary().category,
            importance: summary().importance,
            summary_path: summary().summary_path,
        };
        assert_ne!(event.artifact_id, id);
        assert!(!validate_summary_event(&event));
    }

    use crate::brand::NAMESPACE;
    use crate::layout;
    use crate::markdown::LinkStyle;
    use crate::vault::{OwnedWrite, Vault, note_filename, url_record_key};
    use brauser_protocol::{RecordKey, RecordKeyKind, StorageConfig};
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    const PAGE_URL: &str = "https://e.example/docs/a";

    fn fixed_storage(root: &Path) -> StorageConfig {
        StorageConfig {
            root: root.to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: layout::LOGS_DIR.into(),
            pages_dir: layout::NOTES_DIR.into(),
            later_dir: layout::LATER_DIR.into(),
            artifacts_dir: layout::ARTIFACTS_DIR.into(),
            summaries_dir: Some(layout::SUMMARIES_DIR.into()),
            knowledge_base_dir: None,
        }
    }

    fn fixed_vault() -> (tempfile::TempDir, Vault) {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open(&fixed_storage(root.path())).unwrap();
        (root, vault)
    }

    fn artifact_key(value: &str) -> RecordKey {
        RecordKey {
            kind: RecordKeyKind::Artifact,
            value: value.into(),
        }
    }

    /// A summary file's leading frontmatter, built by hand in the PR 4
    /// identity layout: `kind: summary`, `canonical_url`, then the record key.
    fn summary_fixture(canonical_url: &str, key_lines: &str) -> String {
        format!(
            "---\ntitle: \"T\"\nbrauser:\n  kind: summary\n  canonical_url: {}\n{key_lines}---\n\nAgent text.\n",
            serde_json::to_string(canonical_url).unwrap()
        )
    }

    fn url_id_line(value: &str) -> String {
        format!("  url_id: {}\n", serde_json::to_string(value).unwrap())
    }

    fn artifact_id_line(value: &str) -> String {
        format!("  artifact_id: {}\n", serde_json::to_string(value).unwrap())
    }

    /// The vault-relative path with `/` separators, as the summary event
    /// stores it on every platform.
    fn slash_path(path: &Path) -> String {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    #[test]
    fn summary_name_never_equals_the_note_name() {
        let (_root, vault) = fixed_vault();
        let url_key = url_record_key(PAGE_URL).unwrap();
        let jira = artifact_key("jira:acme.example/PROJ-1");
        for key in [&url_key, &jira] {
            let note = note_filename(key, PAGE_URL).unwrap();
            let summary = summary_filename(key, PAGE_URL).unwrap();
            assert_ne!(summary, note);
            assert_eq!(
                summary.strip_suffix(".summary.md"),
                note.strip_suffix(".md"),
                "the summary shares the note's stem"
            );
            let summary_path = vault.summary_relative_path(key, PAGE_URL).unwrap();
            let note_path = vault.note_relative_path(key, PAGE_URL).unwrap();
            assert_ne!(summary_path, note_path);
            assert_eq!(
                summary_path,
                Path::new(layout::SUMMARIES_DIR).join(&summary)
            );
        }
        // A URL key must still match its canonical URL, as for notes.
        assert!(summary_filename(&url_key, "https://e.example/other").is_err());
    }

    #[test]
    fn summary_paths_pass_the_summary_event_path_check() {
        let (_root, vault) = fixed_vault();
        let tricky_url = "https://e.example/a#b/c?d=<e>&f=#g";
        let tricky_canonical = {
            let mut parsed = url::Url::parse(tricky_url).unwrap();
            parsed.set_fragment(None);
            parsed.to_string()
        };
        let cases = [
            (url_record_key(PAGE_URL).unwrap(), PAGE_URL.to_owned()),
            (url_record_key(&tricky_canonical).unwrap(), tricky_canonical),
            (
                artifact_key("jira:acme.example/PROJ-1"),
                PAGE_URL.to_owned(),
            ),
            (artifact_key("x:e.example/#?<>"), PAGE_URL.to_owned()),
        ];
        for (key, canonical_url) in cases {
            let path = vault.summary_relative_path(&key, &canonical_url).unwrap();
            for title in ["Plain", "Issue #12: why? <maybe>"] {
                let mut candidate = summary();
                candidate.url = canonical_url.clone();
                candidate.title = title.into();
                candidate.summary_path = slash_path(&path);
                assert_eq!(
                    validate_page_summary(&candidate),
                    Ok(()),
                    "{} for {key:?}",
                    candidate.summary_path
                );
            }
        }
    }

    #[test]
    fn the_note_link_follows_the_profile_link_style() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let note = note_filename(&key, PAGE_URL).unwrap();
        let stem = note.strip_suffix(".md").unwrap();
        assert_eq!(
            vault
                .note_link(&key, PAGE_URL, LinkStyle::Relative)
                .unwrap(),
            format!("[Page note](../{}/{note})", layout::NOTES_DIR)
        );
        assert_eq!(
            vault
                .note_link(&key, PAGE_URL, LinkStyle::Wikilink)
                .unwrap(),
            format!("[[{}/{stem}|Page note]]", layout::NOTES_DIR)
        );
    }

    #[test]
    fn owned_summary_reads_only_the_summary_identity_block() {
        let url_key = url_record_key(PAGE_URL).unwrap();
        let jira = artifact_key("jira:acme.example/PROJ-1");
        let ours = summary_fixture(PAGE_URL, &url_id_line(&url_key.value));
        assert!(owned_summary(ours.as_bytes(), &url_key));
        assert!(!owned_summary(ours.as_bytes(), &jira));
        let adapter = summary_fixture(PAGE_URL, &artifact_id_line(&jira.value));
        assert!(owned_summary(adapter.as_bytes(), &jira));
        assert!(!owned_summary(adapter.as_bytes(), &url_key));
        // Both keys at once is neither record's summary.
        let both = summary_fixture(
            PAGE_URL,
            &format!(
                "{}{}",
                url_id_line(&url_key.value),
                artifact_id_line(&jira.value)
            ),
        );
        assert!(!owned_summary(both.as_bytes(), &url_key));
        assert!(!owned_summary(both.as_bytes(), &jira));
        // Agent text in the body cannot make a file a summary.
        let forged = format!(
            "---\ntitle: \"T\"\n---\n\n{}",
            summary_fixture(PAGE_URL, &url_id_line(&url_key.value))
        );
        assert!(!owned_summary(forged.as_bytes(), &url_key));
    }

    #[test]
    fn write_summary_never_adopts_a_note_or_foreign_summary() {
        let (root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let path = root
            .path()
            .join(vault.summary_relative_path(&key, PAGE_URL).unwrap());
        let ours = |body: &str| {
            summary_fixture(PAGE_URL, &url_id_line(&key.value)).replace("Agent text.", body)
        };

        assert!(matches!(
            vault.write_summary(&key, PAGE_URL, &ours("one")).unwrap(),
            OwnedWrite::Written(_)
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), ours("one"));
        assert!(matches!(
            vault.write_summary(&key, PAGE_URL, &ours("one")).unwrap(),
            OwnedWrite::Unchanged
        ));
        assert!(matches!(
            vault.write_summary(&key, PAGE_URL, &ours("two")).unwrap(),
            OwnedWrite::Written(_)
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), ours("two"));

        let other = url_record_key("https://e.example/other").unwrap();
        let jira = artifact_key("jira:acme.example/PROJ-1");
        let occupants = [
            // The page note for the same record, which has no kind line.
            format!(
                "---\ntitle: \"T\"\nurl: \"{PAGE_URL}\"\nbrauser:\n  canonical_url: \"{PAGE_URL}\"\n{}  created: 2026-01-01T00:00:00Z\n---\n\nThe user's note.\n",
                url_id_line(&key.value)
            ),
            // The same layout with an explicit page kind.
            summary_fixture(PAGE_URL, &url_id_line(&key.value))
                .replace("kind: summary", "kind: page"),
            // Another record's summary.
            summary_fixture("https://e.example/other", &url_id_line(&other.value)),
            // An adapter artifact's summary.
            summary_fixture(PAGE_URL, &artifact_id_line(&jira.value)),
            // This record's key and an artifact key at once.
            summary_fixture(
                PAGE_URL,
                &format!(
                    "{}{}",
                    url_id_line(&key.value),
                    artifact_id_line(&jira.value)
                ),
            ),
            "The user's own Markdown.\n".to_owned(),
        ];
        for occupant in occupants {
            fs::write(&path, &occupant).unwrap();
            assert!(
                matches!(
                    vault.write_summary(&key, PAGE_URL, &ours("three")).unwrap(),
                    OwnedWrite::Conflict
                ),
                "adopted {occupant:?}"
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), occupant);
        }

        // An artifact-keyed summary is owned by its artifact key alone.
        let jira_path = root
            .path()
            .join(vault.summary_relative_path(&jira, PAGE_URL).unwrap());
        let jira_summary = summary_fixture(PAGE_URL, &artifact_id_line(&jira.value));
        fs::write(
            &jira_path,
            summary_fixture(PAGE_URL, &url_id_line(&key.value)),
        )
        .unwrap();
        assert!(matches!(
            vault.write_summary(&jira, PAGE_URL, &jira_summary).unwrap(),
            OwnedWrite::Conflict
        ));
        fs::remove_file(&jira_path).unwrap();
        assert!(matches!(
            vault.write_summary(&jira, PAGE_URL, &jira_summary).unwrap(),
            OwnedWrite::Written(_)
        ));
        // Nothing was ever written to the notes folder.
        assert!(!root.path().join(layout::NOTES_DIR).exists());
    }

    #[test]
    fn a_vault_without_a_summaries_folder_has_no_summary_paths() {
        let root = tempfile::tempdir().unwrap();
        let mut storage = fixed_storage(root.path());
        storage.summaries_dir = None;
        let vault = Vault::open(&storage).unwrap();
        let key = url_record_key(PAGE_URL).unwrap();
        assert!(vault.summary_relative_path(&key, PAGE_URL).is_err());
        assert!(
            vault
                .note_link(&key, PAGE_URL, LinkStyle::Relative)
                .is_err()
        );
        assert!(vault.write_summary(&key, PAGE_URL, "x").is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn stale_summary_temporaries_are_swept() {
        let (root, vault) = fixed_vault();
        let summaries = root.path().join(layout::SUMMARIES_DIR);
        let notes = root.path().join(layout::NOTES_DIR);
        fs::create_dir_all(&summaries).unwrap();
        fs::create_dir_all(&notes).unwrap();
        let old = crate::vault::STALE_TEMPORARY_AGE + Duration::from_secs(60);
        let uuid = || uuid::Uuid::new_v4();
        let stale = summaries.join(format!(".{NAMESPACE}-summary-{}.tmp", uuid()));
        let stale_note = notes.join(format!(".{NAMESPACE}-{}.tmp", uuid()));
        let fresh = summaries.join(format!(".{NAMESPACE}-summary-{}.tmp", uuid()));
        // Names this host never writes in the summaries folder stay.
        let kept = [
            summaries.join(format!(".{NAMESPACE}-artifact-{}.tmp", uuid())),
            summaries.join(format!(".{NAMESPACE}-{}.tmp", uuid())),
            summaries.join(format!(".{NAMESPACE}-summary-notes.tmp")),
            summaries.join("page.summary.md"),
        ];
        for path in kept.iter().chain([&stale, &stale_note, &fresh]) {
            fs::write(path, "content").unwrap();
        }
        for path in kept.iter().chain([&stale, &stale_note]) {
            crate::vault::tests::age_file(path, old);
        }

        assert_eq!(vault.sweep_stale_temporaries().unwrap(), 2);
        assert!(!stale.exists());
        assert!(!stale_note.exists());
        for path in kept.iter().chain([&fresh]) {
            assert!(path.exists(), "{} was removed", path.display());
        }
    }

    #[test]
    fn a_summaries_folder_failure_does_not_stop_the_notes_sweep() {
        let (root, vault) = fixed_vault();
        let notes = root.path().join(layout::NOTES_DIR);
        fs::create_dir_all(&notes).unwrap();
        // A file in place of the summaries folder cannot be listed, and the
        // error is not NotFound.
        fs::write(root.path().join(layout::SUMMARIES_DIR), "not a folder").unwrap();
        let stale_note = notes.join(format!(".{NAMESPACE}-{}.tmp", uuid::Uuid::new_v4()));
        fs::write(&stale_note, "content").unwrap();
        crate::vault::tests::age_file(
            &stale_note,
            crate::vault::STALE_TEMPORARY_AGE + Duration::from_secs(60),
        );

        assert_eq!(vault.sweep_stale_temporaries().unwrap(), 1);
        assert!(!stale_note.exists());
    }

    #[test]
    fn a_notes_folder_failure_does_not_stop_the_summaries_sweep() {
        let (root, vault) = fixed_vault();
        let summaries = root.path().join(layout::SUMMARIES_DIR);
        fs::create_dir_all(&summaries).unwrap();
        fs::write(root.path().join(layout::NOTES_DIR), "not a folder").unwrap();
        let stale = summaries.join(format!(".{NAMESPACE}-summary-{}.tmp", uuid::Uuid::new_v4()));
        fs::write(&stale, "content").unwrap();
        crate::vault::tests::age_file(
            &stale,
            crate::vault::STALE_TEMPORARY_AGE + Duration::from_secs(60),
        );

        assert_eq!(vault.sweep_stale_temporaries().unwrap(), 1);
        assert!(!stale.exists());
    }

    #[test]
    fn a_summaries_folder_that_would_break_the_summary_link_is_refused() {
        // Since the layout move the folder is always `summaries`, but a vault
        // opened from an older config can still name another folder.
        for folder in ["sum#maries", "sum?maries", "sum<maries", "sum>maries"] {
            let root = tempfile::tempdir().unwrap();
            let mut storage = fixed_storage(root.path());
            storage.summaries_dir = Some(folder.into());
            let vault = Vault::open(&storage).unwrap();
            let key = url_record_key(PAGE_URL).unwrap();
            assert!(
                vault.summary_relative_path(&key, PAGE_URL).is_err(),
                "{folder} was accepted"
            );
            assert!(vault.write_summary(&key, PAGE_URL, "x").is_err());
            assert!(!root.path().join(folder).exists());
        }
    }

    #[test]
    fn the_sweep_tolerates_a_missing_summaries_folder() {
        let (_root, vault) = fixed_vault();
        assert_eq!(vault.sweep_stale_temporaries().unwrap(), 0);
    }

    use crate::answer::{Entities, Importance, SummaryAnswer};
    use crate::markdown::{neutralize_remote, yaml_string};
    use brauser_protocol::{HarnessId, ImportanceLevel};

    fn answer() -> SummaryAnswer {
        SummaryAnswer {
            document_type: "specification".into(),
            category: "technical specification".into(),
            entities: Entities {
                technologies: vec!["Rust".into(), "SQLite".into()],
                products: vec!["Widget".into()],
                people: vec![],
                companies: vec!["Acme".into()],
            },
            description: "A spec for widgets.".into(),
            importance: Importance {
                level: ImportanceLevel::High,
                reason: "It defines the widget API.".into(),
            },
            assessment: "Fills a gap in the widget notes.".into(),
            related_notes: vec!["projects/widget.md".into()],
        }
    }

    fn document<'a>(
        key: &'a RecordKey,
        answer: &'a SummaryAnswer,
        note_link: String,
    ) -> SummaryDocument<'a> {
        SummaryDocument {
            key,
            canonical_url: PAGE_URL,
            title: "Widget spec",
            summarized_at: "2026-10-26T14:05:00-07:00",
            harness: HarnessId::Codex,
            session: SessionLink::CodexThread("t-1".into()),
            note_link,
            answer,
        }
    }

    /// The leading frontmatter of a rendered summary, without its fences.
    fn frontmatter(rendered: &str) -> &str {
        rendered
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(front, _)| front)
            .expect("a leading frontmatter block")
    }

    /// The body of a rendered summary, after its frontmatter.
    fn body(rendered: &str) -> &str {
        rendered
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(_, body)| body)
            .expect("a leading frontmatter block")
    }

    /// A top-level frontmatter value, read back as the JSON string it is
    /// written as (YAML reads a JSON string as a double-quoted scalar with
    /// the same escapes).
    fn frontmatter_value(rendered: &str, key: &str) -> String {
        let prefix = format!("{key}: ");
        let lines: Vec<&str> = frontmatter(rendered)
            .lines()
            .filter_map(|line| line.strip_prefix(prefix.as_str()))
            .collect();
        assert_eq!(lines.len(), 1, "{key} in {rendered:?}");
        serde_json::from_str(lines[0]).unwrap_or_else(|_| panic!("{key} is {:?}", lines[0]))
    }

    #[test]
    fn importance_levels_have_their_wire_names() {
        for level in [
            ImportanceLevel::High,
            ImportanceLevel::Medium,
            ImportanceLevel::Low,
        ] {
            assert_eq!(
                serde_json::to_value(level).unwrap(),
                serde_json::Value::from(importance_str(level))
            );
        }
    }

    #[test]
    fn summary_file_has_the_exact_layout() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let answer = answer();
        let note_link = vault
            .note_link(&key, PAGE_URL, LinkStyle::Relative)
            .unwrap();
        let note = note_filename(&key, PAGE_URL).unwrap();
        let rendered = render_summary(&document(&key, &answer, note_link), LinkStyle::Relative);
        let expected = format!(
            "---\n\
             brauser:\n  kind: summary\n  canonical_url: \"{PAGE_URL}\"\n  url_id: \"{id}\"\n\
             canonical_url: \"{PAGE_URL}\"\n\
             title: \"Widget spec\"\n\
             summarized_at: \"2026-10-26T14:05:00-07:00\"\n\
             document_type: \"specification\"\n\
             category: \"technical specification\"\n\
             importance: high\n\
             entities:\n  technologies: [\"Rust\", \"SQLite\"]\n  products: [\"Widget\"]\n  people: []\n  companies: [\"Acme\"]\n\
             harness: codex\n\
             thread: \"codex://threads/t-1\"\n\
             ---\n\
             [Page](<{PAGE_URL}>) · [Page note](../{notes}/{note})\n\
             \n\
             **Type:** specification · **Category:** technical specification · **Importance:** high — It defines the widget API.\n\
             \n\
             **Entities:** Technologies: Rust, SQLite · Products: Widget · Companies: Acme\n\
             \n\
             ## Summary\n\
             A spec for widgets.\n\
             \n\
             ## Assessment\n\
             Fills a gap in the widget notes.\n\
             \n\
             Related: projects/widget.md\n",
            id = key.value,
            notes = layout::NOTES_DIR,
        );
        assert_eq!(rendered, expected);
        assert!(owned_summary(rendered.as_bytes(), &key));
    }

    #[test]
    fn empty_entity_groups_and_related_notes_are_left_out() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let mut answer = answer();
        answer.entities.technologies.clear();
        answer.entities.companies.clear();
        let note_link = vault
            .note_link(&key, PAGE_URL, LinkStyle::Relative)
            .unwrap();
        let rendered = render_summary(
            &document(&key, &answer, note_link.clone()),
            LinkStyle::Relative,
        );
        assert!(rendered.contains("\n**Entities:** Products: Widget\n"));
        assert!(rendered.contains("  technologies: []\n"));

        answer.entities.products.clear();
        answer.related_notes.clear();
        let rendered = render_summary(&document(&key, &answer, note_link), LinkStyle::Relative);
        assert!(!rendered.contains("**Entities:**"), "{rendered}");
        assert!(!rendered.contains("Related:"), "{rendered}");
        assert!(rendered.ends_with(
            "**Importance:** high — It defines the widget API.\n\n## Summary\nA spec for widgets.\n\n## Assessment\nFills a gap in the widget notes.\n"
        ), "{rendered}");
    }

    #[test]
    fn claude_runs_record_the_session_not_a_thread() {
        let (_root, vault) = fixed_vault();
        let key = artifact_key("jira:acme.example/PROJ-1");
        let answer = answer();
        let mut doc = document(
            &key,
            &answer,
            vault
                .note_link(&key, PAGE_URL, LinkStyle::Relative)
                .unwrap(),
        );
        doc.harness = HarnessId::Claude;
        doc.session = SessionLink::ClaudeSession("s-1".into());
        let rendered = render_summary(&doc, LinkStyle::Relative);
        let front = frontmatter(&rendered);
        assert!(
            front.contains("\nharness: claude\nclaude_session: \"s-1\"\n")
                || front.ends_with("\nharness: claude\nclaude_session: \"s-1\""),
            "{front}"
        );
        assert!(!front.contains("thread:"), "{front}");
        assert!(!rendered.contains("codex://"), "{rendered}");
        assert!(front.starts_with(&format!(
            "brauser:\n  kind: summary\n  canonical_url: \"{PAGE_URL}\"\n  artifact_id: \"jira:acme.example/PROJ-1\"\ncanonical_url:"
        )), "{front}");
        assert!(owned_summary(rendered.as_bytes(), &key));

        // A run with no session records neither line.
        doc.session = SessionLink::None;
        let front_none = render_summary(&doc, LinkStyle::Relative);
        let front_none = frontmatter(&front_none);
        assert!(front_none.ends_with("\nharness: claude"), "{front_none}");
        assert!(!front_none.contains("claude_session") && !front_none.contains("thread:"));
    }

    #[test]
    fn obsidian_style_uses_wikilinks() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let answer = answer();
        let note = note_filename(&key, PAGE_URL).unwrap();
        let stem = note.strip_suffix(".md").unwrap();
        let note_link = vault
            .note_link(&key, PAGE_URL, LinkStyle::Wikilink)
            .unwrap();
        let rendered = render_summary(&document(&key, &answer, note_link), LinkStyle::Wikilink);
        let body = body(&rendered);
        assert!(
            body.starts_with(&format!(
                "[Page](<{PAGE_URL}>) · [[{}/{stem}|Page note]]\n\n",
                layout::NOTES_DIR
            )),
            "{body}"
        );
        assert!(body.contains(
            "\n**Entities:** Technologies: [[Rust]], [[SQLite]] · Products: [[Widget]] · Companies: [[Acme]]\n"
        ), "{body}");
        assert!(
            body.ends_with("\n\nRelated: [[projects/widget]]\n"),
            "{body}"
        );
        // Labels and frontmatter are the same in both styles.
        assert!(body.contains("**Type:** specification · "));
        assert_eq!(frontmatter_value(&rendered, "title"), "Widget spec");

        // An entity a wikilink cannot carry is written as escaped text.
        let mut odd = answer.clone();
        odd.entities.technologies = vec!["[[]]".into(), "C#".into()];
        let rendered = render_summary(
            &document(
                &key,
                &odd,
                vault
                    .note_link(&key, PAGE_URL, LinkStyle::Wikilink)
                    .unwrap(),
            ),
            LinkStyle::Wikilink,
        );
        assert!(
            rendered.contains("Technologies: \\[\\[\\]\\], C\\# · "),
            "{rendered}"
        );
    }

    #[test]
    fn neutralized_text_starts_at_column_zero() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let mut answer = answer();
        answer.description =
            "    indented = code\n![x](https://e.example/a.png)\n- item ~~~".into();
        answer.assessment = "`= this.file.name`\n\n$x$ <img src=u>".into();
        let rendered = render_summary(
            &document(
                &key,
                &answer,
                vault
                    .note_link(&key, PAGE_URL, LinkStyle::Relative)
                    .unwrap(),
            ),
            LinkStyle::Relative,
        );
        let description = neutralize_remote(&answer.description);
        let assessment = neutralize_remote(&answer.assessment);
        assert!(
            rendered.contains(&format!(
                "\n## Summary\n{description}\n\n## Assessment\n{assessment}\n"
            )),
            "{rendered}"
        );
        // Every line of agent prose begins a line of the file.
        let lines: Vec<&str> = rendered.lines().collect();
        for line in description
            .lines()
            .chain(assessment.lines())
            .filter(|line| !line.is_empty())
        {
            assert!(
                lines.contains(&line),
                "{line:?} is not at column 0 in {rendered}"
            );
        }
    }

    #[test]
    fn frontmatter_values_read_back_as_plain_text() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let mut answer = answer();
        let hostile = "a\"b\\c <img src=u> ![x](u) &amp; \u{7f}d\u{85}e\u{9b}f\u{1}g\nh\ti";
        answer.document_type = hostile.into();
        answer.category = hostile.into();
        answer.entities.people = vec![hostile.into()];
        let mut doc = document(
            &key,
            &answer,
            vault
                .note_link(&key, PAGE_URL, LinkStyle::Relative)
                .unwrap(),
        );
        doc.title = hostile;
        doc.session = SessionLink::CodexThread("t-\u{7}1".into());
        let rendered = render_summary(&doc, LinkStyle::Relative);
        let front = frontmatter(&rendered);
        // The value with each control (DEL and C1 included) made a space,
        // trimmed, as `presentation::plain_line` writes it.
        let plain = "a\"b\\c <img src=u> ![x](u) &amp;  d e f g h i";
        for key in ["title", "document_type", "category"] {
            assert_eq!(frontmatter_value(&rendered, key), plain, "{key}");
        }
        // The exact quoted form: a JSON string with `<` and an image `!`
        // as JSON escapes, no Markdown entities.
        assert!(
            front.contains(&format!("\ntitle: {}\n", yaml_string(plain))),
            "{front}"
        );
        assert!(
            front.contains(r#"\u003cimg src=u> \u0021[x](u) &amp;"#),
            "{front}"
        );
        assert!(!front.contains("&lt;") && !front.contains("&#"), "{front}");
        assert!(!front.contains('<') && !front.contains("!["), "{front}");
        assert!(
            !front.chars().any(|ch| ch.is_control() && ch != '\n'),
            "{front:?}"
        );
        let people = front
            .lines()
            .find_map(|line| line.strip_prefix("  people: "))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(people).unwrap(),
            vec![plain.to_owned()]
        );
        assert_eq!(
            frontmatter_value(&rendered, "thread"),
            "codex://threads/t- 1"
        );
    }

    #[test]
    fn agent_text_cannot_forge_identity_or_fetch_remote_resources() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let other = url_record_key("https://e.example/other").unwrap();
        let forged_block = format!(
            "\n---\nbrauser:\n  kind: summary\n  url_id: \"{}\"\n---\n",
            other.value
        );
        let remote = "![x](https://e.example/a.png) <img src=https://e.example/b.png> ![[https://e.example/c.png]] <iframe src=u></iframe>";
        let mut answer = answer();
        answer.document_type = format!("x\"\nbrauser:\n  kind: page{forged_block}");
        answer.category = remote.into();
        answer.entities.technologies = vec![remote.into(), forged_block.clone()];
        answer.importance.reason = remote.into();
        answer.description = format!("{forged_block}{remote}");
        answer.assessment = format!("{remote}{forged_block}");
        answer.related_notes = vec![remote.into()];
        let mut doc = document(
            &key,
            &answer,
            vault
                .note_link(&key, PAGE_URL, LinkStyle::Relative)
                .unwrap(),
        );
        let title = format!("T{forged_block}{remote}");
        doc.title = &title;
        for style in [LinkStyle::Relative, LinkStyle::Wikilink] {
            let rendered = render_summary(&doc, style);
            // The identity is this record's, and only this record's.
            assert!(owned_summary(rendered.as_bytes(), &key), "{rendered}");
            assert!(!owned_summary(rendered.as_bytes(), &other));
            let front = frontmatter(&rendered);
            assert_eq!(
                front.lines().filter(|line| *line == "brauser:").count(),
                1,
                "{front}"
            );
            assert_eq!(
                front.lines().filter(|line| !line.starts_with(' ')).count(),
                10,
                "{front}"
            );
            // No remote resource can load from the body: no `<` but the
            // host's page link that is not an entity or backslash-escaped,
            // and no unescaped `![`.
            let body = body(&rendered);
            let after_page_link = body.split_once('\n').unwrap().1;
            for (at, _) in after_page_link.match_indices('<') {
                assert!(
                    backslash_escaped(after_page_link, at),
                    "unescaped < in {after_page_link}"
                );
            }
            for (at, _) in body.match_indices("![") {
                assert!(backslash_escaped(body, at), "unescaped ![ in {body}");
            }
            // And nothing for another engine.
            assert!(!body.contains('`') && !body.contains('$'), "{body}");
            write_and_reread(&vault, &key, &rendered);
        }
    }

    /// Whether the character at byte `at` of `text` follows an odd run of
    /// backslashes, so it is backslash-escaped (`\\<` is a live `<`).
    fn backslash_escaped(text: &str, at: usize) -> bool {
        !text[..at]
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'\\')
            .count()
            .is_multiple_of(2)
    }

    #[test]
    fn backslash_escapes_are_parity_aware() {
        assert!(backslash_escaped("\\<", 1));
        assert!(!backslash_escaped("\\\\<", 2));
        assert!(backslash_escaped("\\\\\\<", 3));
        assert!(!backslash_escaped("<", 0));
    }

    #[test]
    fn the_page_link_holds_no_code_span_or_math() {
        let (_root, vault) = fixed_vault();
        let url =
            crate::capture::canonical_url("https://evil.example/p?q=`$=dv.el('b','x')`&r=$x$", &[])
                .unwrap();
        // WHATWG serialization leaves the backticks and `$` in the query.
        assert!(url.contains('`') && url.contains('$'), "{url}");
        let key = url_record_key(&url).unwrap();
        let answer = answer();
        for style in [LinkStyle::Relative, LinkStyle::Wikilink] {
            let mut doc = document(&key, &answer, vault.note_link(&key, &url, style).unwrap());
            doc.canonical_url = &url;
            let rendered = render_summary(&doc, style);
            let body = body(&rendered);
            assert!(!body.contains('`') && !body.contains('$'), "{body}");
            // CommonMark decodes the entities, so the link target is the URL.
            let destination = url
                .replace('`', "&#96;")
                .replace('$', "&#36;")
                .replace('!', "&#33;")
                .replace('[', "&#91;")
                .replace(']', "&#93;");
            assert!(
                body.starts_with(&format!("[Page](<{destination}>) · ")),
                "{body}"
            );
            assert!(owned_summary(rendered.as_bytes(), &key), "{rendered}");
        }
    }

    #[test]
    fn the_page_destination_escapes_obsidian_syntax_as_entities() {
        let url = crate::capture::canonical_url(
            "https://e.example/p?s=![[x]]&t=%%c%%&u=a==b&v=~~d&w=%20=~",
            &[],
        )
        .unwrap();
        assert_eq!(
            url,
            "https://e.example/p?s=![[x]]&t=%%c%%&u=a==b&v=~~d&w=%20=~"
        );
        assert_eq!(
            page_destination(&url),
            "https://e.example/p?s=&#33;&#91;&#91;x&#93;&#93;&t=&#37;&#37;c&#37;&#37;\
             &u=a&#61;&#61;b&v=&#126;&#126;d&w=%20=~"
        );
        assert_eq!(
            page_destination("https://e.example/a b<>\\"),
            "https://e.example/a%20b%3C%3E%5C"
        );
    }

    #[test]
    fn wikilinks_fall_back_to_text_for_obsidian_syntax() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let mut answer = answer();
        // A trailing backslash escapes the closing bracket; `%%` opens an
        // Obsidian comment and `==` a highlight inside the link.
        let odd: Vec<String> = ["a\\", "%%x", "a%%b", "==hi==", "~~s~~", "100% sure"]
            .map(String::from)
            .to_vec();
        answer.entities.technologies = odd.clone();
        let rendered = render_summary(
            &document(
                &key,
                &answer,
                vault
                    .note_link(&key, PAGE_URL, LinkStyle::Wikilink)
                    .unwrap(),
            ),
            LinkStyle::Wikilink,
        );
        let mut expected: Vec<String> = odd[..5]
            .iter()
            .map(|name| inline(name, MAX_ENTITY_CHARS))
            .collect();
        expected.push("[[100% sure]]".into());
        assert!(
            rendered.contains(&format!("Technologies: {} · ", expected.join(", "))),
            "{rendered}"
        );
        assert!(!rendered.contains("[[a\\]]") && !rendered.contains("[[%%"));
    }

    #[test]
    fn related_notes_are_written_with_forward_slashes() {
        let (_root, vault) = fixed_vault();
        let key = url_record_key(PAGE_URL).unwrap();
        let mut answer = answer();
        answer.related_notes = vec!["./projects/./widget.md".into()];
        #[cfg(windows)]
        answer.related_notes.push(r"projects\gadget.md".into());
        let mut expected_relative = vec!["projects/widget.md"];
        let mut expected_wikilink = vec!["[[projects/widget]]"];
        if cfg!(windows) {
            expected_relative.push("projects/gadget.md");
            expected_wikilink.push("[[projects/gadget]]");
        }
        for (style, expected) in [
            (LinkStyle::Relative, expected_relative),
            (LinkStyle::Wikilink, expected_wikilink),
        ] {
            let rendered = render_summary(
                &document(
                    &key,
                    &answer,
                    vault.note_link(&key, PAGE_URL, style).unwrap(),
                ),
                style,
            );
            assert!(
                rendered.ends_with(&format!("\n\nRelated: {}\n", expected.join(", "))),
                "{rendered}"
            );
        }
    }

    fn write_and_reread(vault: &Vault, key: &RecordKey, rendered: &str) {
        assert!(matches!(
            vault.write_summary(key, PAGE_URL, rendered).unwrap(),
            OwnedWrite::Written(_) | OwnedWrite::Unchanged
        ));
    }
}

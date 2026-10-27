//! Versioned whole-file page notes (§4.4, §5.3). The side panel is the only
//! editor: a note is created on the first save and replaced whole on every
//! later save. Each save names the version it replaces; a stale version is
//! refused and the note's current content is returned so nothing the user
//! typed is silently lost.

use brauser_protocol::{NoteSaveOutcome, OlderNote, RecordKey, RecordKeyKind};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::brand::FRONTMATTER_KEY;
use crate::native::MAX_OUTBOUND_BYTES;
use crate::vault::Vault;

/// A new note's title is clamped to this many UTF-8 bytes, never refused.
const MAX_TITLE_BYTES: usize = 2048;
/// A body is refused above this many UTF-8 bytes. `note_loaded` and
/// `note_conflict` echo the title and body, and JSON escaping at most doubles
/// ordinary text (quotes, backslashes, newlines, tabs), so a note at both
/// limits still fits under `MAX_OUTBOUND_BYTES`. Only other control
/// characters (six bytes each as `\u00XX`) can overflow it;
/// `fits_in_response` catches those exactly. The schema and the panel's
/// textarea cap the body at 98304 characters, at most four bytes each.
const MAX_BODY_BYTES: usize = 384 * 1024;
/// A body is also refused, on save and on load, above this many UTF-16 code
/// units: the panel textarea's `maxlength`, which counts UTF-16 units, and the
/// schema's `maxLength`, which counts code points (never more than the UTF-16
/// count). A longer body loaded into the panel would break the protocol
/// contract, and Chrome would refuse every keystroke that lengthened it.
const MAX_BODY_UTF16_UNITS: usize = 98_304;
/// Room left in a note response for everything but the title and body: the
/// type tag, a request ID of at most 128 characters (512 bytes escaped), the
/// revision, and the field names.
const RESPONSE_ENVELOPE_BYTES: usize = 4 * 1024;
const MISSING_REVISION: &str = "missing";

#[derive(Debug)]
pub struct LoadedNote {
    pub exists: bool,
    pub revision: String,
    pub title: String,
    pub body: String,
    pub key: RecordKey,
    pub older_notes: Vec<OlderNote>,
}

pub enum SaveOutcome {
    Saved {
        outcome: NoteSaveOutcome,
        revision: String,
        relative_path: String,
    },
    /// `expected_revision` did not match the note's current version. Nothing
    /// was written; this carries the note's current content so the caller
    /// can show it and let the user copy back what it could not save.
    Stale {
        revision: String,
        exists: bool,
        title: String,
        body: String,
    },
}

/// An existing file occupies the note's generated name but is not a Brauser
/// page note for this exact URL (wrong identity, or `brauser.kind` is not
/// absent/`page`, e.g. a summary file). Never adopted or overwritten (§6.4).
#[derive(Debug)]
pub struct OwnershipConflict(pub String);

#[derive(Debug)]
pub enum NoteRequestError {
    /// The note, as it would be saved or as found on disk, cannot be sent back
    /// in one native message, so it is refused instead of failing the
    /// response.
    TooLarge(String),
    Conflict(OwnershipConflict),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for NoteRequestError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

struct ParsedNote {
    title: String,
    created: String,
    body: String,
    canonical_url: String,
}

pub fn load_note(vault: &Vault, canonical_url: &str) -> Result<LoadedNote, NoteRequestError> {
    let key = crate::vault::url_record_key(canonical_url)?;
    load_keyed_note(vault, canonical_url, &key, &[])
}

/// Loads an older URL-keyed note by its own key, without the current page's
/// adapter match upgrading it to the shared artifact note. Lets an older
/// note stay reachable (view-only; it is not re-keyed) after a sibling note
/// has already been migrated into the shared artifact note.
pub fn load_older_note(vault: &Vault, source_url: &str) -> Result<LoadedNote, NoteRequestError> {
    let key = crate::vault::url_record_key(source_url)?;
    load_keyed_note(vault, source_url, &key, &[])
}

pub fn load_keyed_note(
    vault: &Vault,
    canonical_url: &str,
    key: &RecordKey,
    older_urls: &[String],
) -> Result<LoadedNote, NoteRequestError> {
    let mut older_notes = older_notes(vault, key, older_urls)?;
    match vault.read_note(key, canonical_url)? {
        None => {
            trim_older_notes("", "", &mut older_notes);
            Ok(LoadedNote {
                exists: false,
                revision: MISSING_REVISION.to_owned(),
                title: String::new(),
                body: String::new(),
                key: key.clone(),
                older_notes,
            })
        }
        Some(contents) => {
            let parsed = parse_owned_note(key, canonical_url, &contents).ok_or_else(not_owned)?;
            if !body_fits_panel(&parsed.body) || !fits_in_response(&parsed.title, &parsed.body) {
                return Err(too_large_on_disk());
            }
            trim_older_notes(&parsed.title, &parsed.body, &mut older_notes);
            Ok(LoadedNote {
                exists: true,
                revision: revision_for(&contents),
                title: parsed.title,
                body: parsed.body,
                key: key.clone(),
                older_notes,
            })
        }
    }
}

pub fn save_note(
    vault: &Vault,
    canonical_url: &str,
    title: &str,
    body: &str,
    expected_revision: &str,
) -> Result<SaveOutcome, NoteRequestError> {
    let key = crate::vault::url_record_key(canonical_url)?;
    save_keyed_note(vault, canonical_url, &key, title, body, expected_revision)
}

pub fn save_keyed_note(
    vault: &Vault,
    canonical_url: &str,
    key: &RecordKey,
    title: &str,
    body: &str,
    expected_revision: &str,
) -> Result<SaveOutcome, NoteRequestError> {
    if body.len() > MAX_BODY_BYTES || !body_fits_panel(body) {
        return Err(NoteRequestError::TooLarge(
            "note is too long to save".into(),
        ));
    }
    let existing = vault.read_note(key, canonical_url)?;
    let (current_revision, parsed) = match &existing {
        None => (MISSING_REVISION.to_owned(), None),
        Some(contents) => {
            let parsed = parse_owned_note(key, canonical_url, contents).ok_or_else(not_owned)?;
            (revision_for(contents), Some(parsed))
        }
    };
    if expected_revision != current_revision {
        if let Some(note) = &parsed
            && (!body_fits_panel(&note.body) || !fits_in_response(&note.title, &note.body))
        {
            return Err(too_large_on_disk());
        }
        return Ok(SaveOutcome::Stale {
            revision: current_revision,
            exists: parsed.is_some(),
            title: parsed
                .as_ref()
                .map(|note| note.title.clone())
                .unwrap_or_default(),
            body: parsed
                .as_ref()
                .map(|note| note.body.clone())
                .unwrap_or_default(),
        });
    }

    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| NoteRequestError::Internal(error.into()))?;
    let created = parsed
        .as_ref()
        .map(|note| note.created.clone())
        .unwrap_or_else(|| now.clone());
    // The title only matters when the note is created, so an over-long page
    // title is clamped there and never blocks a save.
    let effective_title = parsed
        .as_ref()
        .map(|note| note.title.clone())
        .unwrap_or_else(|| clamped_title(normalized_title(canonical_url, title)).to_owned());
    // A save must never produce a note that a later load cannot return.
    if !fits_in_response(&effective_title, body) {
        return Err(NoteRequestError::TooLarge(
            "note has too many special characters to save".into(),
        ));
    }
    // An artifact-keyed note's canonical_url is informational, not part of
    // its ownership check (only artifact_id is, above), and is this
    // lookup's only durable, activity-independent trail back to a URL after
    // the adapter that produced the key is removed. Keep recording the
    // note's original URL rather than whichever alternate route last saved
    // it, so that original URL stays able to recover it.
    let effective_canonical_url = match key.kind {
        RecordKeyKind::Artifact => parsed
            .as_ref()
            .map(|note| note.canonical_url.clone())
            .unwrap_or_else(|| canonical_url.to_owned()),
        RecordKeyKind::Url => canonical_url.to_owned(),
    };
    let markdown = render_note(
        key,
        &effective_canonical_url,
        &effective_title,
        body,
        &created,
        &now,
    )?;
    let replacing = existing.is_some();
    let published = vault.replace_note(key, canonical_url, &markdown)?;
    let outcome = match (replacing, published.warnings.is_empty()) {
        (false, true) => NoteSaveOutcome::Created,
        (false, false) => NoteSaveOutcome::CreatedWithWarning,
        (true, true) => NoteSaveOutcome::Replaced,
        (true, false) => NoteSaveOutcome::ReplacedWithWarning,
    };
    Ok(SaveOutcome::Saved {
        outcome,
        revision: revision_for(&markdown),
        relative_path: published.relative_path.to_string_lossy().into_owned(),
    })
}

fn not_owned() -> NoteRequestError {
    NoteRequestError::Conflict(OwnershipConflict(
        "existing file at this page's note path is not a Brauser page note for this page".into(),
    ))
}

fn normalized_title<'a>(canonical_url: &'a str, title: &'a str) -> &'a str {
    if title.trim().is_empty() {
        canonical_url
    } else {
        title.trim()
    }
}

fn clamped_title(title: &str) -> &str {
    if title.len() <= MAX_TITLE_BYTES {
        return title;
    }
    let mut end = MAX_TITLE_BYTES;
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    title[..end].trim_end()
}

fn too_large_on_disk() -> NoteRequestError {
    NoteRequestError::TooLarge(
        "this page's note is too large to open here; edit it in your notes folder".into(),
    )
}

/// Whether the panel can hold this body within its textarea's `maxlength`,
/// which also keeps it within the schema's `maxLength`.
fn body_fits_panel(body: &str) -> bool {
    // A UTF-8 byte is at most one UTF-16 unit, so short bodies need no count.
    body.len() <= MAX_BODY_UTF16_UNITS || body.encode_utf16().count() <= MAX_BODY_UTF16_UNITS
}

/// Whether a response echoing this title and body fits in one native message,
/// measured with the same JSON escaping the response uses.
fn fits_in_response(title: &str, body: &str) -> bool {
    escaped_len(title) + escaped_len(body) + RESPONSE_ENVELOPE_BYTES <= MAX_OUTBOUND_BYTES
}

fn escaped_len(value: &str) -> usize {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    // Serializing a string into this writer cannot fail; if it somehow did,
    // treat the value as too large rather than guessing.
    match serde_json::to_writer(&mut counter, value) {
        Ok(()) => counter.0,
        Err(_) => usize::MAX,
    }
}

fn revision_for(contents: &str) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(contents.as_bytes()))
    )
}

fn render_note(
    key: &RecordKey,
    canonical_url: &str,
    title: &str,
    body: &str,
    created: &str,
    updated: &str,
) -> Result<String, NoteRequestError> {
    let title_json =
        serde_json::to_string(title).map_err(|error| NoteRequestError::Internal(error.into()))?;
    let url_json = serde_json::to_string(canonical_url)
        .map_err(|error| NoteRequestError::Internal(error.into()))?;
    let key_line = match key.kind {
        RecordKeyKind::Url => {
            let digest_json = serde_json::to_string(&key.value)
                .map_err(|error| NoteRequestError::Internal(error.into()))?;
            format!("  url_id: {digest_json}\n")
        }
        RecordKeyKind::Artifact => {
            let id_json = serde_json::to_string(&key.value)
                .map_err(|error| NoteRequestError::Internal(error.into()))?;
            format!("  artifact_id: {id_json}\n")
        }
    };
    // The body is stored verbatim; parse_owned_note removes only the one
    // newline written after it.
    Ok(format!(
        "---\ntitle: {title_json}\nurl: {url_json}\n{FRONTMATTER_KEY}\n  canonical_url: {url_json}\n{key_line}  created: {created}\n  updated: {updated}\n---\n\n{body}\n"
    ))
}

/// Parse a note this host previously wrote, or refuse anything it does not
/// recognize as its own. The side panel is the only editor, so this need not
/// tolerate arbitrary hand edits; it must never adopt an unrelated file.
fn parse_owned_note(key: &RecordKey, canonical_url: &str, contents: &str) -> Option<ParsedNote> {
    let (frontmatter, rest) = contents.strip_prefix("---\n")?.split_once("\n---\n")?;
    let body = rest.strip_prefix('\n').unwrap_or(rest);
    // render_note always appends exactly one trailing newline after the body.
    let body = body.strip_suffix('\n').unwrap_or(body);

    let mut title = None;
    let mut fm_canonical: Option<String> = None;
    let mut fm_url_id: Option<String> = None;
    let mut fm_artifact_id: Option<String> = None;
    let mut created: Option<String> = None;
    let mut kind: Option<String> = None;
    let mut in_block = false;
    for line in frontmatter.lines() {
        if line == FRONTMATTER_KEY {
            in_block = true;
            continue;
        }
        if in_block {
            if !line.starts_with("  ") {
                in_block = false;
            } else if let Some(value) = line.strip_prefix("  canonical_url: ") {
                fm_canonical = unquote(value);
            } else if let Some(value) = line.strip_prefix("  url_id: ") {
                fm_url_id = unquote(value);
            } else if let Some(value) = line.strip_prefix("  artifact_id: ") {
                fm_artifact_id = unquote(value);
            } else if let Some(value) = line.strip_prefix("  created: ") {
                // Timestamps are written as plain RFC 3339 scalars (§6.5), not
                // JSON-quoted like the string fields above.
                created = Some(value.to_owned());
            } else if let Some(value) = line.strip_prefix("  kind: ") {
                kind = Some(value.to_owned());
            }
        }
        if let Some(value) = line.strip_prefix("title: ") {
            title = unquote(value);
        }
    }

    let title = title?;
    let fm_canonical = fm_canonical?;
    let created = created?;
    // §6.4: a page note has no kind key, or an explicit "page" kind. Anything
    // else (such as "summary") must never be treated as an owned page note.
    if kind.is_some_and(|value| value != "page") {
        return None;
    }
    match key.kind {
        RecordKeyKind::Url => {
            if fm_canonical != canonical_url
                || fm_url_id.as_deref() != Some(key.value.as_str())
                || fm_artifact_id.is_some()
            {
                return None;
            }
        }
        RecordKeyKind::Artifact => {
            if fm_artifact_id.as_deref() != Some(key.value.as_str()) || fm_url_id.is_some() {
                return None;
            }
        }
    }
    Some(ParsedNote {
        title,
        created,
        body: body.to_owned(),
        canonical_url: fm_canonical,
    })
}

/// Returns the artifact_id of an artifact-keyed page note whose own recorded
/// `canonical_url` is exactly this URL. Used, independent of any activity
/// history, to recover a note's identity when no adapter currently
/// recognizes the URL (its adapter was removed) but the note itself was
/// saved while one did. This is used only on bounded files from
/// `Vault::note_candidates`; malformed, URL-keyed, summary, and unrelated
/// Markdown are ignored.
pub fn artifact_keyed_note_identity(contents: &str, url: &str) -> Option<String> {
    let (frontmatter, _) = contents.strip_prefix("---\n")?.split_once("\n---\n")?;
    let mut title = None;
    let mut canonical = None;
    let mut artifact_id = None;
    let mut created = None;
    let mut kind = None;
    let mut in_block = false;
    for line in frontmatter.lines() {
        if line == FRONTMATTER_KEY {
            in_block = true;
            continue;
        }
        if in_block && !line.starts_with("  ") {
            in_block = false;
        }
        if in_block {
            if let Some(value) = line.strip_prefix("  canonical_url: ") {
                canonical = unquote(value);
            } else if let Some(value) = line.strip_prefix("  artifact_id: ") {
                artifact_id = unquote(value);
            } else if let Some(value) = line.strip_prefix("  created: ") {
                created = Some(value.to_owned());
            } else if let Some(value) = line.strip_prefix("  kind: ") {
                kind = Some(value.to_owned());
            }
        }
        if let Some(value) = line.strip_prefix("title: ") {
            title = unquote(value);
        }
    }
    // Require the same shape a fully validated owned page note has
    // (parse_owned_note), not just the two fields this lookup reads, so an
    // unrelated or malformed file can never be adopted as an artifact's
    // identity.
    title?;
    created?;
    if kind.is_some_and(|value| value != "page") {
        return None;
    }
    let artifact_id = artifact_id?;
    (canonical.as_deref() == Some(url)).then_some(artifact_id)
}

/// Return a verified URL-keyed identity from a legacy page note. This is used
/// only on bounded files from `Vault::note_candidates`; malformed, artifact-
/// keyed, summary, and unrelated Markdown are ignored.
pub fn url_keyed_note_url(contents: &str) -> Option<String> {
    let (frontmatter, _) = contents.strip_prefix("---\n")?.split_once("\n---\n")?;
    let mut canonical = None;
    let mut url_id = None;
    let mut artifact_id = None;
    let mut kind = None;
    let mut in_block = false;
    for line in frontmatter.lines() {
        if line == FRONTMATTER_KEY {
            in_block = true;
            continue;
        }
        if in_block && !line.starts_with("  ") {
            in_block = false;
        }
        if !in_block {
            continue;
        }
        if let Some(value) = line.strip_prefix("  canonical_url: ") {
            canonical = unquote(value);
        } else if let Some(value) = line.strip_prefix("  url_id: ") {
            url_id = unquote(value);
        } else if let Some(value) = line.strip_prefix("  artifact_id: ") {
            artifact_id = unquote(value);
        } else if let Some(value) = line.strip_prefix("  kind: ") {
            kind = Some(value.to_owned());
        }
    }
    if artifact_id.is_some() || kind.is_some_and(|value| value != "page") {
        return None;
    }
    let canonical = canonical?;
    let expected = crate::vault::url_record_key(&canonical).ok()?;
    (url_id.as_deref() == Some(expected.value.as_str())).then_some(canonical)
}

fn older_notes(
    vault: &Vault,
    primary_key: &RecordKey,
    urls: &[String],
) -> Result<Vec<OlderNote>, NoteRequestError> {
    if primary_key.kind != RecordKeyKind::Artifact {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for canonical_url in urls {
        if result.len() >= 24 {
            break;
        }
        if !seen.insert(canonical_url) {
            continue;
        }
        // An unreadable, oversized, or otherwise broken optional older
        // candidate must never take down the primary note load below it;
        // skip just this one candidate.
        let Ok(key) = crate::vault::url_record_key(canonical_url) else {
            continue;
        };
        let Ok(Some(contents)) = vault.read_note(&key, canonical_url) else {
            continue;
        };
        let Some(parsed) = parse_owned_note(&key, canonical_url, &contents) else {
            continue;
        };
        result.push(OlderNote {
            key,
            canonical_url: canonical_url.clone(),
            title: parsed.title,
            revision: revision_for(&contents),
        });
    }
    Ok(result)
}

fn trim_older_notes(title: &str, body: &str, notes: &mut Vec<OlderNote>) {
    while !notes.is_empty()
        && escaped_len(title)
            .saturating_add(escaped_len(body))
            .saturating_add(
                serde_json::to_vec(notes)
                    .map(|bytes| bytes.len())
                    .unwrap_or(MAX_OUTBOUND_BYTES),
            )
            .saturating_add(RESPONSE_ENVELOPE_BYTES)
            > MAX_OUTBOUND_BYTES
    {
        notes.pop();
    }
}

pub fn use_as_artifact_note(
    vault: &Vault,
    destination_url: &str,
    artifact_key: &RecordKey,
    source_url: &str,
    expected_revision: &str,
) -> Result<LoadedNote, NoteRequestError> {
    if artifact_key.kind != RecordKeyKind::Artifact {
        return Err(NoteRequestError::Conflict(OwnershipConflict(
            "this page is not a recognized artifact".into(),
        )));
    }
    if vault.read_note(artifact_key, destination_url)?.is_some() {
        return Err(NoteRequestError::Conflict(OwnershipConflict(
            "an artifact note already exists; the older note was left unchanged".into(),
        )));
    }
    let source_key = crate::vault::url_record_key(source_url)?;
    let source = vault.read_note(&source_key, source_url)?.ok_or_else(|| {
        NoteRequestError::Conflict(OwnershipConflict("the older note no longer exists".into()))
    })?;
    if revision_for(&source) != expected_revision {
        return Err(NoteRequestError::Conflict(OwnershipConflict(
            "the older note changed; load it again before re-keying".into(),
        )));
    }
    let parsed = parse_owned_note(&source_key, source_url, &source).ok_or_else(not_owned)?;
    // Migration returns the new artifact note in the same response. Refuse
    // before publishing if the source has been enlarged outside the panel;
    // otherwise the move would succeed and only the response would fail.
    if !body_fits_panel(&parsed.body) || !fits_in_response(&parsed.title, &parsed.body) {
        return Err(too_large_on_disk());
    }
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| NoteRequestError::Internal(error.into()))?;
    let markdown = render_note(
        artifact_key,
        destination_url,
        &parsed.title,
        &parsed.body,
        &parsed.created,
        &now,
    )?;
    vault.move_note_no_clobber(
        &source_key,
        source_url,
        artifact_key,
        destination_url,
        &source,
        &markdown,
    )?;
    load_keyed_note(vault, destination_url, artifact_key, &[])
}

fn unquote(value: &str) -> Option<String> {
    serde_json::from_str(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use brauser_protocol::StorageConfig;

    fn vault() -> (tempfile::TempDir, Vault) {
        let root = tempfile::tempdir().unwrap();
        let storage = StorageConfig {
            root: root.path().to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: "pages".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: None,
            knowledge_base_dir: None,
        };
        let vault = Vault::open(&storage).unwrap();
        (root, vault)
    }

    #[test]
    fn loading_an_absent_note_reports_missing_revision() {
        let (_root, vault) = vault();
        let loaded = load_note(&vault, "https://example.com/a").unwrap();
        assert!(!loaded.exists);
        assert_eq!(loaded.revision, "missing");
        assert_eq!(loaded.title, "");
        assert_eq!(loaded.body, "");
    }

    #[test]
    fn saving_a_new_note_creates_it_from_the_missing_revision() {
        let (_root, vault) = vault();
        let SaveOutcome::Saved {
            outcome, revision, ..
        } = save_note(
            &vault,
            "https://example.com/a",
            "A Title",
            "hello",
            "missing",
        )
        .unwrap()
        else {
            panic!("expected a save, not a conflict");
        };
        assert!(matches!(outcome, NoteSaveOutcome::Created));
        let loaded = load_note(&vault, "https://example.com/a").unwrap();
        assert!(loaded.exists);
        assert_eq!(loaded.revision, revision);
        assert_eq!(loaded.title, "A Title");
        assert_eq!(loaded.body, "hello");
    }

    #[test]
    fn saving_with_a_stale_revision_is_refused_and_returns_current_content() {
        let (_root, vault) = vault();
        save_note(&vault, "https://example.com/a", "Title", "first", "missing").unwrap();

        let SaveOutcome::Stale {
            revision,
            exists,
            title,
            body,
        } = save_note(
            &vault,
            "https://example.com/a",
            "Title",
            "second",
            "missing",
        )
        .unwrap()
        else {
            panic!("expected a stale-revision conflict");
        };
        assert!(exists);
        assert_eq!(title, "Title");
        assert_eq!(body, "first");
        assert_ne!(revision, "missing");

        // The refused save must not have changed the file.
        let loaded = load_note(&vault, "https://example.com/a").unwrap();
        assert_eq!(loaded.body, "first");
    }

    #[test]
    fn resaving_with_the_current_revision_preserves_title_and_created_time() {
        let (_root, vault) = vault();
        let SaveOutcome::Saved { revision, .. } = save_note(
            &vault,
            "https://example.com/a",
            "Original Title",
            "one",
            "missing",
        )
        .unwrap() else {
            panic!("expected a save");
        };
        let SaveOutcome::Saved {
            outcome,
            revision: next_revision,
            ..
        } = save_note(
            &vault,
            "https://example.com/a",
            "Ignored New Title",
            "two",
            &revision,
        )
        .unwrap()
        else {
            panic!("expected a save");
        };
        assert!(matches!(outcome, NoteSaveOutcome::Replaced));
        assert_ne!(revision, next_revision);

        let loaded = load_note(&vault, "https://example.com/a").unwrap();
        assert_eq!(loaded.title, "Original Title");
        assert_eq!(loaded.body, "two");
    }

    #[test]
    fn a_file_owned_by_a_different_page_is_never_adopted() {
        let (root, vault) = vault();
        // Write a note for one URL, then hand-craft a filename collision by
        // writing unrelated content directly at the generated path.
        let path = vault.page_relative_path("https://example.com/a").unwrap();
        let full = root.path().join(&path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, "not a brauser note").unwrap();

        assert!(matches!(
            load_note(&vault, "https://example.com/a"),
            Err(NoteRequestError::Conflict(_))
        ));
        assert!(matches!(
            save_note(&vault, "https://example.com/a", "T", "b", "missing"),
            Err(NoteRequestError::Conflict(_))
        ));
        assert_eq!(
            std::fs::read_to_string(&full).unwrap(),
            "not a brauser note"
        );
    }

    #[test]
    fn a_summary_file_at_the_same_name_is_a_conflict_not_a_note() {
        let (root, vault) = vault();
        let url = "https://example.com/a";
        let path = vault.page_relative_path(url).unwrap();
        let full = root.path().join(&path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        // Everything but `kind` matches a real note for this URL, so only the
        // kind guard can refuse it.
        let owned = |kind_line: &str| {
            format!(
                "---\ntitle: \"T\"\nurl: \"{url}\"\nbrauser:\n{kind_line}  canonical_url: \"{url}\"\n  url_id: \"{}\"\n  created: 2026-01-01T00:00:00Z\n---\n\nbody\n",
                hex::encode(Sha256::digest(url.as_bytes()))
            )
        };
        std::fs::write(&full, owned("")).unwrap();
        assert!(load_note(&vault, url).unwrap().exists);

        std::fs::write(&full, owned("  kind: summary\n")).unwrap();
        assert!(matches!(
            load_note(&vault, url),
            Err(NoteRequestError::Conflict(_))
        ));
    }

    #[test]
    fn trailing_blank_lines_in_a_body_survive_a_round_trip() {
        let (_root, vault) = vault();
        for body in ["Todo:\n- a\n\n\n", "\n", "\n\nlead", ""] {
            let url = format!("https://example.com/{}", body.len());
            save_note(&vault, &url, "T", body, "missing").unwrap();
            assert_eq!(load_note(&vault, &url).unwrap().body, body);
        }
    }

    #[test]
    fn use_as_artifact_note_refuses_when_the_artifact_note_exists() {
        let (_root, vault) = vault();
        let source_url = "https://example.com/items/7?view=old";
        let current_url = "https://example.com/items/7";
        let source = match save_note(&vault, source_url, "Old", "older text", "missing").unwrap() {
            SaveOutcome::Saved { revision, .. } => revision,
            SaveOutcome::Stale { .. } => panic!("new URL note was unexpectedly stale"),
        };
        let artifact_key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:example.com/7".into(),
        };
        save_keyed_note(
            &vault,
            current_url,
            &artifact_key,
            "Current",
            "artifact text",
            "missing",
        )
        .unwrap();
        let error = use_as_artifact_note(&vault, current_url, &artifact_key, source_url, &source)
            .unwrap_err();
        assert!(matches!(error, NoteRequestError::Conflict(_)));
        assert_eq!(load_note(&vault, source_url).unwrap().body, "older text");
        assert_eq!(
            load_keyed_note(&vault, current_url, &artifact_key, &[])
                .unwrap()
                .body,
            "artifact text"
        );
    }

    #[test]
    fn a_broken_older_candidate_cannot_take_down_the_primary_note_load() {
        let (root, vault) = vault();
        let primary_url = "https://example.com/items/7";
        let artifact_key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:example.com/7".into(),
        };
        save_keyed_note(
            &vault,
            primary_url,
            &artifact_key,
            "Issue 7",
            "primary text",
            "missing",
        )
        .unwrap();

        // A broken older candidate: a directory sitting where its note
        // file would be, so reading it fails outright rather than
        // returning "not found".
        let broken_url = "https://example.com/board?selected=7";
        let pages = root.path().join("pages");
        let broken_filename = crate::vault::page_filename(broken_url).unwrap();
        std::fs::create_dir(pages.join(&broken_filename)).unwrap();

        let loaded =
            load_keyed_note(&vault, primary_url, &artifact_key, &[broken_url.into()]).unwrap();
        assert!(loaded.exists, "the primary note must still load");
        assert_eq!(loaded.body, "primary text");
        assert!(
            loaded.older_notes.is_empty(),
            "the broken candidate must be skipped, not surfaced"
        );
    }

    #[test]
    fn an_older_note_past_two_dozen_candidate_urls_is_still_found() {
        let (_root, vault) = vault();
        let current_url = "https://example.com/items/7";
        let artifact_key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:example.com/7".into(),
        };
        let source_url = "https://example.com/board?selected=7";
        save_note(
            &vault,
            source_url,
            "Before adapter",
            "keep this text",
            "missing",
        )
        .unwrap();
        // 24 candidates with no saved note, then the one that actually has
        // one. The candidate limit must not cut off examination before this
        // older note is even looked at.
        let mut older_urls: Vec<String> = (0..24)
            .map(|index| format!("https://example.com/noise/{index}"))
            .collect();
        older_urls.push(source_url.into());
        let loaded = load_keyed_note(&vault, current_url, &artifact_key, &older_urls).unwrap();
        assert_eq!(loaded.older_notes.len(), 1);
        assert_eq!(loaded.older_notes[0].canonical_url, source_url);
    }

    #[test]
    fn an_artifact_notes_creation_url_survives_a_save_from_an_alternate_route() {
        let (_root, vault) = vault();
        let artifact_key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:example.com/7".into(),
        };
        let created_url = "https://example.com/items/7?mode=edit";
        let alternate_url = "https://example.com/items/7?mode=view";

        let revision = match save_keyed_note(
            &vault,
            created_url,
            &artifact_key,
            "Issue 7",
            "first",
            "missing",
        )
        .unwrap()
        {
            SaveOutcome::Saved { revision, .. } => revision,
            SaveOutcome::Stale { .. } => panic!("new artifact note was unexpectedly stale"),
        };
        // Saved again from a different alternate route for the same artifact.
        save_keyed_note(
            &vault,
            alternate_url,
            &artifact_key,
            "Issue 7",
            "second",
            &revision,
        )
        .unwrap();

        let raw = vault
            .read_note(&artifact_key, alternate_url)
            .unwrap()
            .unwrap();
        assert!(
            artifact_keyed_note_identity(&raw, created_url).is_some(),
            "the note must still be recoverable by the URL it was created on"
        );
        assert!(
            artifact_keyed_note_identity(&raw, alternate_url).is_none(),
            "canonical_url must record the creation URL, not the latest alternate route"
        );
    }

    #[test]
    fn artifact_keyed_note_identity_rejects_a_candidate_missing_required_fields() {
        let url = "https://example.com/x";
        let missing_created = format!(
            "---\ntitle: \"Stolen\"\nurl: \"{url}\"\n{FRONTMATTER_KEY}\n  canonical_url: \"{url}\"\n  artifact_id: \"work:example.com/7\"\n---\n\nbody\n"
        );
        assert!(artifact_keyed_note_identity(&missing_created, url).is_none());

        let summary_kind = format!(
            "---\ntitle: \"Summary\"\nurl: \"{url}\"\n{FRONTMATTER_KEY}\n  canonical_url: \"{url}\"\n  artifact_id: \"work:example.com/7\"\n  created: 2026-01-01T00:00:00Z\n  kind: summary\n---\n\nbody\n"
        );
        assert!(artifact_keyed_note_identity(&summary_kind, url).is_none());
    }

    #[test]
    fn an_older_note_can_be_viewed_without_migrating_it() {
        let (_root, vault) = vault();
        let source_url = "https://example.com/board?selected=7";
        save_note(
            &vault,
            source_url,
            "Before adapter",
            "keep this text",
            "missing",
        )
        .unwrap();
        let viewed = load_older_note(&vault, source_url).unwrap();
        assert!(viewed.exists);
        assert_eq!(viewed.body, "keep this text");
        assert_eq!(viewed.key.kind, RecordKeyKind::Url);
        // Viewing never migrates or changes the note.
        assert!(load_note(&vault, source_url).unwrap().exists);
    }

    #[test]
    fn url_keyed_note_is_listed_and_can_become_the_artifact_note() {
        let (_root, vault) = vault();
        let source_url = "https://example.com/board?selected=7";
        let current_url = "https://example.com/items/7";
        let revision = match save_note(
            &vault,
            source_url,
            "Before adapter",
            "keep this text",
            "missing",
        )
        .unwrap()
        {
            SaveOutcome::Saved { revision, .. } => revision,
            SaveOutcome::Stale { .. } => panic!("new URL note was unexpectedly stale"),
        };
        let artifact_key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:example.com/7".into(),
        };
        let loaded =
            load_keyed_note(&vault, current_url, &artifact_key, &[source_url.into()]).unwrap();
        assert!(!loaded.exists);
        assert_eq!(loaded.older_notes.len(), 1);
        assert_eq!(loaded.older_notes[0].canonical_url, source_url);
        let migrated =
            use_as_artifact_note(&vault, current_url, &artifact_key, source_url, &revision)
                .unwrap();
        assert_eq!(migrated.body, "keep this text");
        let artifact_markdown = vault
            .read_note(&artifact_key, current_url)
            .unwrap()
            .unwrap();
        assert!(artifact_markdown.contains(&format!(
            "  canonical_url: {}",
            serde_json::to_string(current_url).unwrap()
        )));
        assert!(!load_note(&vault, source_url).unwrap().exists);
    }

    #[test]
    fn oversized_url_note_is_not_moved_before_the_response_is_refused() {
        let (root, vault) = vault();
        let source_url = "https://example.com/board?selected=7";
        let current_url = "https://example.com/items/7";
        save_note(&vault, source_url, "Old", "small", "missing").unwrap();
        let source_path = root
            .path()
            .join(vault.page_relative_path(source_url).unwrap());
        let grown = std::fs::read_to_string(&source_path)
            .unwrap()
            .replace("small", &"x".repeat(MAX_BODY_UTF16_UNITS + 1));
        std::fs::write(&source_path, &grown).unwrap();
        let artifact_key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:example.com/7".into(),
        };

        assert!(matches!(
            use_as_artifact_note(
                &vault,
                current_url,
                &artifact_key,
                source_url,
                &revision_for(&grown),
            ),
            Err(NoteRequestError::TooLarge(_))
        ));
        assert_eq!(std::fs::read_to_string(source_path).unwrap(), grown);
        assert!(
            vault
                .read_note(&artifact_key, current_url)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn an_m1_url_keyed_note_loads_without_being_rewritten() {
        let (root, vault) = vault();
        let url = "https://example.com/original";
        save_note(&vault, url, "Original", "M1 body", "missing").unwrap();
        let path = root.path().join(vault.page_relative_path(url).unwrap());
        let before = std::fs::read(&path).unwrap();
        let loaded = load_note(&vault, url).unwrap();
        assert_eq!(loaded.key.kind, RecordKeyKind::Url);
        assert_eq!(loaded.body, "M1 body");
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn a_long_title_is_clamped_on_creation_and_ignored_afterwards() {
        let (_root, vault) = vault();
        // 3-byte characters, so a byte cut could land inside one.
        let long = "界".repeat(MAX_TITLE_BYTES);
        let SaveOutcome::Saved { revision, .. } =
            save_note(&vault, "https://example.com/a", &long, "one", "missing").unwrap()
        else {
            panic!("expected a save");
        };
        let loaded = load_note(&vault, "https://example.com/a").unwrap();
        assert!(loaded.title.len() <= MAX_TITLE_BYTES);
        assert!(long.starts_with(&loaded.title));
        assert!(!loaded.title.is_empty());

        // A later save sends the same over-long title; it must not be refused.
        assert!(matches!(
            save_note(&vault, "https://example.com/a", &long, "two", &revision),
            Ok(SaveOutcome::Saved { .. })
        ));
        assert_eq!(
            load_note(&vault, "https://example.com/a").unwrap().body,
            "two"
        );
    }

    #[test]
    fn the_largest_accepted_note_fits_in_one_response() {
        let (_root, vault) = vault();
        // Quotes are the worst ordinary escape (two bytes each).
        let body = "\"".repeat(MAX_BODY_UTF16_UNITS);
        let title = "\"".repeat(MAX_TITLE_BYTES);
        assert!(matches!(
            save_note(&vault, "https://example.com/a", &title, &body, "missing"),
            Ok(SaveOutcome::Saved { .. })
        ));
        let loaded = load_note(&vault, "https://example.com/a").unwrap();
        assert_eq!(loaded.body, body);
        // Worst-case envelope: the longest request_id, every character escaped.
        let response = brauser_protocol::Response::NoteConflict(brauser_protocol::NoteConflict {
            protocol_version: brauser_protocol::PROTOCOL_VERSION,
            request_id: "\"".repeat(128),
            exists: true,
            revision: loaded.revision,
            title: loaded.title,
            body: loaded.body,
            key: loaded.key,
            reason: "stale".into(),
        });
        assert!(serde_json::to_vec(&response).unwrap().len() <= MAX_OUTBOUND_BYTES);
    }

    #[test]
    fn older_note_metadata_is_trimmed_before_a_large_note_exceeds_the_message_cap() {
        let mut notes = (0..24)
            .map(|index| OlderNote {
                key: RecordKey {
                    kind: RecordKeyKind::Url,
                    value: format!("url:{index}:{}", "\u{0001}".repeat(500)),
                },
                canonical_url: format!("https://example.com/{index}/{}", "\u{0001}".repeat(8_000)),
                title: "\u{0001}".repeat(MAX_TITLE_BYTES),
                revision: "f".repeat(64),
            })
            .collect::<Vec<_>>();
        let title = "\\\"".repeat(MAX_TITLE_BYTES);
        let body = "\\\"".repeat(MAX_BODY_UTF16_UNITS);

        trim_older_notes(&title, &body, &mut notes);

        let estimated = escaped_len(&title)
            + escaped_len(&body)
            + serde_json::to_vec(&notes).unwrap().len()
            + RESPONSE_ENVELOPE_BYTES;
        assert!(estimated <= MAX_OUTBOUND_BYTES);
        assert!(notes.len() < 24);
    }

    #[test]
    fn schema_and_panel_body_limits_stay_within_the_host_byte_limit() {
        // A code point is at most four UTF-8 bytes, and the panel's
        // `maxlength` counts UTF-16 units, of which a code point has at least
        // one; so any body both allow is within MAX_BODY_BYTES.
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../protocol/schema.json")).unwrap();
        for name in ["SaveNoteRequest", "NoteLoaded", "NoteConflict"] {
            let limit = schema["$defs"][name]["properties"]["body"]["maxLength"]
                .as_u64()
                .unwrap() as usize;
            assert!(limit * 4 <= MAX_BODY_BYTES, "{name}.body maxLength");
        }
        let panel = include_str!("../../extension/panel.html");
        let textarea = &panel[panel.find("id=\"note-body\"").unwrap()..];
        let textarea = &textarea[..textarea.find('>').unwrap()];
        let panel_limit: u64 = textarea
            .split("maxlength=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            Some(panel_limit),
            schema["$defs"]["SaveNoteRequest"]["properties"]["body"]["maxLength"].as_u64()
        );
    }

    #[test]
    fn a_body_over_its_limit_or_too_large_once_escaped_is_refused() {
        let (_root, vault) = vault();
        let over = "a".repeat(MAX_BODY_BYTES + 1);
        assert!(matches!(
            save_note(&vault, "https://example.com/a", "T", &over, "missing"),
            Err(NoteRequestError::TooLarge(_))
        ));
        // Within the byte limit, but longer than the panel's textarea allows.
        let over_panel = "a".repeat(MAX_BODY_UTF16_UNITS + 1);
        assert!(matches!(
            save_note(&vault, "https://example.com/a", "T", &over_panel, "missing"),
            Err(NoteRequestError::TooLarge(_))
        ));
        // Astral characters are two UTF-16 units each, as the textarea counts.
        let astral = "\u{1F600}".repeat(MAX_BODY_UTF16_UNITS / 2 + 1);
        assert!(astral.len() <= MAX_BODY_BYTES);
        assert!(matches!(
            save_note(&vault, "https://example.com/a", "T", &astral, "missing"),
            Err(NoteRequestError::TooLarge(_))
        ));
        let at_limit = "a".repeat(MAX_BODY_UTF16_UNITS);
        assert!(matches!(
            save_note(&vault, "https://example.com/b", "T", &at_limit, "missing"),
            Ok(SaveOutcome::Saved { .. })
        ));
        assert!(!load_note(&vault, "https://example.com/a").unwrap().exists);
    }

    #[test]
    fn a_note_grown_outside_the_panel_is_refused_not_returned() {
        let (root, vault) = vault();
        let url = "https://example.com/a";
        save_note(&vault, url, "T", "small", "missing").unwrap();
        let full = root.path().join(vault.page_relative_path(url).unwrap());
        let grown = std::fs::read_to_string(&full)
            .unwrap()
            .replace("small", &"x".repeat(MAX_OUTBOUND_BYTES));
        std::fs::write(&full, grown).unwrap();

        assert!(matches!(
            load_note(&vault, url),
            Err(NoteRequestError::TooLarge(_))
        ));
        // A stale save would echo the note back; that must be refused too.
        assert!(matches!(
            save_note(&vault, url, "T", "new", "missing"),
            Err(NoteRequestError::TooLarge(_))
        ));
    }

    #[test]
    fn a_note_grown_past_the_panel_limit_outside_the_panel_is_refused() {
        // Well within the byte and response limits, but longer than the
        // schema and the textarea allow.
        let (root, vault) = vault();
        let url = "https://example.com/a";
        save_note(&vault, url, "T", "small", "missing").unwrap();
        let full = root.path().join(vault.page_relative_path(url).unwrap());
        let grown = std::fs::read_to_string(&full)
            .unwrap()
            .replace("small", &"x".repeat(150_000));
        std::fs::write(&full, grown).unwrap();

        assert!(matches!(
            load_note(&vault, url),
            Err(NoteRequestError::TooLarge(_))
        ));
        assert!(matches!(
            save_note(&vault, url, "T", "new", "missing"),
            Err(NoteRequestError::TooLarge(_))
        ));
    }

    #[test]
    fn title_only_matters_on_creation() {
        let (_root, vault) = vault();
        save_note(&vault, "https://example.com/a", "", "body", "missing").unwrap();
        let loaded = load_note(&vault, "https://example.com/a").unwrap();
        // An empty title at creation falls back to the canonical URL.
        assert_eq!(loaded.title, "https://example.com/a");
    }
}

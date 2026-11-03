//! Immutable selected-text evidence. No extractor, harness, log or network.
use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use brauser_protocol::{EvidenceSaved, PROTOCOL_VERSION, RecordKey};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::{
    markdown,
    vault::{TempFileCleanup, Vault, read_inspected_occupant, sync_directory},
};

pub(crate) const MAX_SELECTION_BYTES: usize = 32_768;
const MAX_CARD_BYTES: u64 = 1024 * 1024;
const PREFIX: &str = "---\nbrauser:\n  kind: evidence\n  format: 1\n  captured_at: ";

#[derive(Debug)]
pub(crate) struct Conflict;
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an unrelated or edited file occupies this evidence card's name; nothing was overwritten")
    }
}
impl std::error::Error for Conflict {}

pub(crate) fn valid_selection(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_SELECTION_BYTES
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

// Preserve metadata values as JSON-compatible YAML while keeping inline
// plugin/code syntax inert even in viewers that display the frontmatter.
fn metadata(value: &str) -> String {
    markdown::yaml_string(value)
        .replace('`', "\\u0060")
        .replace('$', "\\u0024")
        .replace('~', "\\u007e")
}

/// Exact text is recoverable from selection_utf8_hex, including CRLF and
/// whitespace. Display text uses the same neutralizer as saved summaries.
fn render(key: &RecordKey, canonical: &str, text: &str, captured_at: &str) -> String {
    let kind = match key.kind {
        brauser_protocol::RecordKeyKind::Artifact => "artifact",
        brauser_protocol::RecordKeyKind::Url => "url",
    };
    format!(
        "{PREFIX}{}\n  canonical_url: {}\n  record_key_kind: {kind}\n  record_key_value: {}\n  selection_sha256: {}\n  selection_utf8_hex: {}\n---\n\n# Selected passage\n\nSelected text supplied by the browser; source identity recorded at capture.\n\nSource: {}\n\nCaptured: {captured_at}\n\n## Passage\n\n{}\n",
        markdown::yaml_string(captured_at),
        metadata(canonical),
        metadata(&key.value),
        markdown::yaml_string(&digest(text.as_bytes())),
        markdown::yaml_string(&hex::encode(text.as_bytes())),
        markdown::inline(canonical, 8192),
        markdown::neutralize_remote(text),
    )
}

fn existing(
    dir: &Dir,
    name: &str,
    key: &RecordKey,
    canonical: &str,
    text: &str,
) -> Result<Option<String>> {
    let inspected = match dir.symlink_metadata(name) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if inspected.file_type().is_symlink()
        || !inspected.is_file()
        || inspected.len() > MAX_CARD_BYTES
    {
        return Err(Conflict.into());
    }
    let bytes = read_inspected_occupant(dir, name.as_ref(), &inspected)?.ok_or(Conflict)?;
    let raw = std::str::from_utf8(&bytes).map_err(|_| Conflict)?;
    let encoded = raw
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.lines().next())
        .ok_or(Conflict)?;
    let captured_at: String = serde_json::from_str(encoded).map_err(|_| Conflict)?;
    OffsetDateTime::parse(&captured_at, &Rfc3339).map_err(|_| Conflict)?;
    if captured_at.len() > 64 || raw != render(key, canonical, text, &captured_at) {
        return Err(Conflict.into());
    }
    // A published card can survive a failed directory sync; an exact retry
    // completes durability before acknowledging it as already saved.
    sync_directory(dir)?;
    Ok(Some(captured_at))
}

pub(crate) fn save(
    vault: &Vault,
    key: &RecordKey,
    canonical: &str,
    text: &str,
    request_id: String,
) -> Result<EvidenceSaved> {
    if !valid_selection(text) {
        bail!("select nonblank text of at most 32768 UTF-8 bytes");
    }
    let identity = serde_json::to_vec(&("brauser.evidence.v1", key, canonical, text))?;
    let name = format!("{}.md", digest(&identity));
    let dir = vault.evidence_dir()?;
    let mut warnings = Vec::new();
    let (captured_at, already_saved) = if let Some(at) =
        existing(&dir, &name, key, canonical, text)?
    {
        (at, true)
    } else {
        let at = OffsetDateTime::now_utc().format(&Rfc3339)?;
        let contents = render(key, canonical, text, &at);
        let temporary = format!(
            ".{}-evidence-{}.tmp",
            crate::brand::NAMESPACE,
            Uuid::new_v4()
        );
        let mut file =
            dir.open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
        let cleanup = TempFileCleanup::new(&dir, &temporary);
        file.write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())?;
        drop(file);
        // hard_link is the only publication operation: no rename/replace.
        let result = match dir.hard_link(&temporary, &dir, &name) {
            Ok(()) => {
                if sync_directory(&dir).is_err() {
                    warnings.push("The card was saved, but its directory could not be synced; repeat the capture to retry durability.".into());
                }
                (at, false)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (
                existing(&dir, &name, key, canonical, text)?
                    .context("evidence race winner disappeared")?,
                true,
            ),
            Err(error) => return Err(error).context("publishing evidence card"),
        };
        drop(cleanup);
        result
    };
    Ok(EvidenceSaved {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        relative_path: format!("notes/evidence/{name}"),
        key: key.clone(),
        canonical_url: canonical.into(),
        captured_at,
        selection_sha256: digest(text.as_bytes()),
        already_saved,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use brauser_protocol::{RecordKeyKind, StorageConfig};
    use std::{fs, path::Path};

    fn vault(root: &Path) -> Vault {
        Vault::open(&StorageConfig {
            root: root.to_string_lossy().into_owned(),
            profile: "neutral".into(),
            pages_dir: "notes".into(),
            log_dir: "logs".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: Some("summaries".into()),
            knowledge_base_dir: None,
        })
        .unwrap()
    }
    fn key() -> RecordKey {
        RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:7".into(),
        }
    }
    const URL: &str = "https://work.example/items/7";

    #[test]
    fn exact_passage_retry_and_changed_selection_preserve_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let vault = vault(root.path());
        let text = "  A decision 🚀\r\n\twith context.  \n";
        let first = save(&vault, &key(), URL, text, "first".into()).unwrap();
        let path = root.path().join(&first.relative_path);
        let original = fs::read(&path).unwrap();
        let raw = String::from_utf8(original.clone()).unwrap();
        assert!(raw.contains(&format!(
            "selection_utf8_hex: {}",
            markdown::yaml_string(&hex::encode(text.as_bytes()))
        )));
        assert!(raw.contains(&format!(
            "selection_sha256: {}",
            markdown::yaml_string(&digest(text.as_bytes()))
        )));
        assert!(raw.contains("record_key_kind: artifact\n  record_key_value: \"work:7\""));
        let again = save(&vault, &key(), URL, text, "retry".into()).unwrap();
        assert!(again.already_saved);
        assert_eq!(again.captured_at, first.captured_at);
        assert_eq!(again.relative_path, first.relative_path);
        assert_eq!(fs::read(path).unwrap(), original);
        let changed = save(&vault, &key(), URL, "1234567890", "changed".into()).unwrap();
        let numeric = fs::read_to_string(root.path().join(&changed.relative_path)).unwrap();
        for (field, expected) in [
            ("selection_utf8_hex", hex::encode("1234567890")),
            ("selection_sha256", digest(b"1234567890")),
        ] {
            let scalar = numeric
                .lines()
                .find_map(|line| line.strip_prefix(&format!("  {field}: ")))
                .unwrap();
            assert_eq!(serde_json::from_str::<String>(scalar).unwrap(), expected);
        }
        assert!(!changed.already_saved);
        assert_ne!(changed.relative_path, first.relative_path);
        assert_eq!(
            fs::read_dir(root.path().join("notes/evidence"))
                .unwrap()
                .count(),
            2
        );
        assert!(!root.path().join("logs").exists());
        assert!(!root.path().join("summaries").exists());
    }

    #[test]
    fn hostile_markup_has_exact_encoded_copy_and_inert_display() {
        let text = "---\nbrauser:\n  kind: page\n---\n<img src=https://evil.test/a>\n![x](https://evil.test/a)\n![[secret]]\n```dataviewjs\nfetch('https://evil.test')\n```\n`=1` $x$\n~~~mermaid\nflowchart TD\n";
        let hostile_url = "https://work.example/page?q=`=1`$x$~~~";
        let hostile_key = RecordKey {
            value: "work:`=1`$x$~~~".into(),
            ..key()
        };
        let rendered = render(&hostile_key, hostile_url, text, "2026-11-03T00:00:00Z");
        let encoded = rendered
            .lines()
            .find_map(|line| line.strip_prefix("  canonical_url: "))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<String>(encoded).unwrap(),
            hostile_url
        );
        assert!(rendered.contains(&format!(
            "selection_utf8_hex: {}",
            markdown::yaml_string(&hex::encode(text))
        )));
        assert!(!rendered.contains("<img"));
        assert!(!rendered.contains("\n!["));
        assert!(!rendered.contains("```"));
        assert!(!rendered.contains("~~~"));
        assert!(!rendered.contains("`=1`"));
        assert!(!rendered.contains("$x$"));
    }

    #[test]
    fn edited_unowned_large_and_directory_occupants_never_change() {
        for replacement in [
            "unrelated file".to_owned(),
            "x".repeat(MAX_CARD_BYTES as usize + 1),
        ] {
            let root = tempfile::tempdir().unwrap();
            let vault = vault(root.path());
            let saved = save(&vault, &key(), URL, "decision", "first".into()).unwrap();
            let path = root.path().join(saved.relative_path);
            fs::write(&path, &replacement).unwrap();
            assert!(
                save(&vault, &key(), URL, "decision", "retry".into())
                    .unwrap_err()
                    .is::<Conflict>()
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), replacement);
        }
        let root = tempfile::tempdir().unwrap();
        let vault = vault(root.path());
        let saved = save(&vault, &key(), URL, "decision", "first".into()).unwrap();
        let path = root.path().join(saved.relative_path);
        let original = fs::read_to_string(&path).unwrap();
        fs::write(&path, original.replace("## Passage", "## Edited")).unwrap();
        assert!(
            save(&vault, &key(), URL, "decision", "retry".into())
                .unwrap_err()
                .is::<Conflict>()
        );
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(
            save(&vault, &key(), URL, "decision", "retry".into())
                .unwrap_err()
                .is::<Conflict>()
        );
        assert!(path.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cards_and_parents_refuse_without_touching_the_target() {
        use std::os::unix::fs::symlink;
        for parent in ["notes", "notes/evidence"] {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            if parent.contains('/') {
                fs::create_dir(root.path().join("notes")).unwrap();
            }
            symlink(outside.path(), root.path().join(parent)).unwrap();
            assert!(save(&vault(root.path()), &key(), URL, "decision", "r".into()).is_err());
            assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        }
        let root = tempfile::tempdir().unwrap();
        let vault = vault(root.path());
        let saved = save(&vault, &key(), URL, "decision", "first".into()).unwrap();
        let path = root.path().join(saved.relative_path);
        fs::remove_file(&path).unwrap();
        let target = root.path().join("private");
        fs::write(&target, "keep me").unwrap();
        symlink(&target, &path).unwrap();
        assert!(
            save(&vault, &key(), URL, "decision", "retry".into())
                .unwrap_err()
                .is::<Conflict>()
        );
        assert_eq!(fs::read_to_string(target).unwrap(), "keep me");
    }

    #[test]
    fn concurrent_identical_publication_keeps_one_card_and_one_timestamp() {
        let root = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(8);
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|i| {
                    let root = root.path();
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let vault = vault(root);
                        barrier.wait();
                        save(&vault, &key(), URL, "shared selection", i.to_string()).unwrap()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            results
                .iter()
                .filter(|result| !result.already_saved)
                .count(),
            1
        );
        assert!(
            results
                .iter()
                .all(|result| result.captured_at == results[0].captured_at)
        );
        assert_eq!(
            fs::read_dir(root.path().join("notes/evidence"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn blank_and_oversized_text_refuse_before_creating_folders() {
        let root = tempfile::tempdir().unwrap();
        for text in [
            String::new(),
            " \r\n\t".into(),
            "é".repeat(MAX_SELECTION_BYTES / 2 + 1),
        ] {
            assert!(save(&vault(root.path()), &key(), URL, &text, "r".into()).is_err());
        }
        assert!(!root.path().join("notes").exists());
        assert!(valid_selection(&"é".repeat(MAX_SELECTION_BYTES / 2)));
    }
}

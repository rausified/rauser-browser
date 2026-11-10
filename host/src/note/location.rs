//! Host-derived note selection. A logical artifact and a file's ownership key
//! are different identities after re-keying. Never render or write the latter
//! using the former. Recompute this location for every request under the config
//! lock; neither a browser request nor a journal approval supplies a location.
use super::*;
use crate::relearn::aliases::Resolution;

pub(crate) struct NoteLocation {
    logical: RecordKey,
    physical: RecordKey,
    history_revision: Option<String>,
}

impl NoteLocation {
    pub(crate) fn resolve(
        vault: &Vault,
        canonical_url: &str,
        key: &RecordKey,
        resolution: &Resolution,
    ) -> Result<Self, NoteRequestError> {
        if key.kind == RecordKeyKind::Url {
            return Ok(Self {
                logical: key.clone(),
                physical: key.clone(),
                history_revision: None,
            });
        }
        let logical = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: resolution.canonical(&key.value).into(),
        };
        let history_revision = resolution.history_revision(&key.value).map(str::to_owned);
        let mut physical = logical.clone();
        if history_revision.is_some() {
            if let Some(contents) = vault.read_note(&logical, canonical_url)? {
                // A foreign file at the canonical path is a conflict, never
                // permission to route around it and edit a different note.
                parse_owned_note(&logical, canonical_url, &contents).ok_or_else(not_owned)?;
            } else {
                let mut selected = None;
                for member in resolution.members(&logical.value) {
                    if member == logical.value {
                        continue;
                    }
                    let candidate = RecordKey {
                        kind: RecordKeyKind::Artifact,
                        value: member.into(),
                    };
                    if let Some(contents) = vault.read_note(&candidate, canonical_url)? {
                        parse_owned_note(&candidate, canonical_url, &contents)
                            .ok_or_else(not_owned)?;
                        if selected.replace(candidate).is_some() {
                            return Err(NoteRequestError::Conflict(OwnershipConflict(
                                "several notes belong to this artifact and no canonical note exists; review the original notes before editing".into(),
                            )));
                        }
                    }
                }
                if let Some(selected) = selected {
                    physical = selected;
                }
            }
        }
        Ok(Self {
            logical,
            physical,
            history_revision,
        })
    }

    pub(crate) fn key(&self) -> &RecordKey {
        &self.logical
    }

    fn revision(&self, raw_revision: &str) -> String {
        let Some(history) = &self.history_revision else {
            // Preserve existing direct revisions and browser-local recovery.
            return raw_revision.into();
        };
        // JSON framing keeps arbitrary artifact IDs unambiguous. The hash is
        // an opaque optimistic version, not a caller-selected physical path.
        let bytes = serde_json::to_vec(&(
            "brauser-note-location-v1",
            &self.logical,
            &self.physical,
            history,
            raw_revision,
        ))
        .expect("serializing note location strings");
        format!("location-v1:{}", hex::encode(Sha256::digest(bytes)))
    }

    pub(crate) fn load(
        &self,
        vault: &Vault,
        canonical_url: &str,
        older_urls: &[String],
    ) -> Result<LoadedNote, NoteRequestError> {
        let mut loaded = load_keyed_note(vault, canonical_url, &self.physical, older_urls)?;
        loaded.key = self.logical.clone();
        loaded.revision = self.revision(&loaded.revision);
        Ok(loaded)
    }

    /// An older URL copy may become the artifact note only when no selected
    /// owner exists. In particular, absence at the canonical path must never
    /// hide an existing alias note. The source keeps its raw URL revision.
    pub(crate) fn use_older_note(
        &self,
        vault: &Vault,
        canonical_url: &str,
        source_url: &str,
        expected_revision: &str,
    ) -> Result<LoadedNote, NoteRequestError> {
        if self.load(vault, canonical_url, &[])?.exists || self.physical != self.logical {
            return Err(NoteRequestError::Conflict(OwnershipConflict(
                "an artifact note already exists; the older note was left unchanged".into(),
            )));
        }
        let mut loaded = use_as_artifact_note(
            vault,
            canonical_url,
            &self.logical,
            source_url,
            expected_revision,
        )?;
        loaded.key = self.logical.clone();
        loaded.revision = self.revision(&loaded.revision);
        Ok(loaded)
    }

    pub(crate) fn save(
        &self,
        vault: &Vault,
        stored: &crate::url_provenance::StoredUrl,
        title: &str,
        body: &str,
        expected_revision: &str,
    ) -> Result<SaveOutcome, NoteRequestError> {
        if self.history_revision.is_none() {
            return save_keyed_note_with_provenance(
                vault,
                stored,
                &self.physical,
                title,
                body,
                expected_revision,
            );
        }
        // Read and verify the physical owner before comparing opaque versions.
        // The low-level save repeats its ordinary revision/ownership checks.
        let current = load_keyed_note(vault, &stored.url, &self.physical, &[])?;
        if expected_revision != self.revision(&current.revision) {
            return Ok(SaveOutcome::Stale {
                revision: self.revision(&current.revision),
                exists: current.exists,
                title: current.title,
                body: current.body,
            });
        }
        let outcome = save_keyed_note_with_provenance(
            vault,
            stored,
            &self.physical,
            title,
            body,
            &current.revision,
        )?;
        Ok(match outcome {
            SaveOutcome::Saved {
                outcome,
                revision,
                relative_path,
            } => SaveOutcome::Saved {
                outcome,
                revision: self.revision(&revision),
                relative_path,
            },
            SaveOutcome::Stale {
                revision,
                exists,
                title,
                body,
            } => SaveOutcome::Stale {
                revision: self.revision(&revision),
                exists,
                title,
                body,
            },
        })
    }
}

#[cfg(test)]
mod tests;

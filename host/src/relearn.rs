//! Read-only, host-derived impact review for capture re-learning. A report is
//! evidence for a future review, never an authorization to change identity.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};

use adapter_templates::{Document, Value};
use anyhow::{Context, Result, ensure};
use brauser_protocol::{AdapterDocument, RecordKey, RecordKeyKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{config::ConfigStore, index::ArtifactIndex, url_provenance::StoredUrl, vault::Vault};

const MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_EVIDENCE: usize = 32_768;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRequest {
    pub origin: String,
    pub expected_revision: String,
    /// A replacement document for exactly this origin, not the whole config.
    pub proposed: AdapterDocument,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Identity {
    pub artifact_type: String,
    pub artifact_id: String,
}

#[derive(Debug, Serialize)]
pub struct SavedFile {
    pub path: String,
    pub kind: &'static str,
    pub owned: bool,
    pub revision: String,
}

#[derive(Debug, Serialize)]
pub struct Impact {
    pub source: RecordKey,
    pub targets: BTreeSet<Identity>,
    /// A split can also merge into a key reached by another source.
    pub classifications: BTreeSet<&'static str>,
    pub issues: BTreeSet<&'static str>,
    pub evidence_count: usize,
    pub files: Vec<SavedFile>,
    pub destination_files: Vec<SavedFile>,
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub review_only: bool,
    pub origin: String,
    pub config_revision: String,
    pub contracts_revision: String,
    pub evidence_revision: String,
    pub proposed_revision: String,
    pub plan_revision: String,
    pub identities: Vec<Impact>,
    pub unowned_files: Vec<String>,
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn compile(artifacts: &[serde_json::Value]) -> Result<Document> {
    let raw = serde_json::json!({"format": 1, "artifacts": artifacts});
    Document::from_value(&serde_json::from_value::<Value>(raw)?)
        .map_err(|error| anyhow::anyhow!("invalid recognition contract: {error}"))
}

fn key_order(key: &RecordKey) -> (u8, String) {
    (u8::from(key.kind == RecordKeyKind::Url), key.value.clone())
}

fn artifact_key(value: &str) -> RecordKey {
    RecordKey {
        kind: RecordKeyKind::Artifact,
        value: value.into(),
    }
}

fn stored_origin(stored: &StoredUrl) -> Option<String> {
    let url = Url::parse(&stored.url).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.origin().ascii_serialization())
}

/// Parse only leading frontmatter, retaining legacy unknown separately from
/// explicit empty provenance. Duplicates and malformed evidence refuse review.
fn summary_provenance(bytes: &[u8]) -> Result<Option<Vec<String>>> {
    let text = std::str::from_utf8(bytes)?;
    let front = text
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n"))
        .context("invalid summary frontmatter")?
        .0;
    let mut in_block = false;
    let mut names = None;
    for line in front.lines() {
        if !line.starts_with(' ') {
            in_block = line == "brauser:";
        } else if in_block && let Some(value) = line.strip_prefix("  stripped_params: ") {
            ensure!(names.is_none(), "duplicate stripped-parameter evidence");
            let parsed: Vec<String> = serde_json::from_str(value)?;
            ensure!(
                crate::url_provenance::valid_names(&parsed),
                "invalid stripped-parameter evidence"
            );
            names = Some(parsed);
        }
    }
    Ok(names)
}

/// Locks serialize Brauser writers only; a later apply must recompute all
/// revisions and recheck owned files. This function never writes config,
/// adopted contracts, cache, log, notes, summaries or browser recovery.
pub fn plan(config: &ConfigStore, request: &PlanRequest) -> Result<Plan> {
    let origin_url = Url::parse(&request.origin).context("invalid origin")?;
    ensure!(
        stored_origin(&StoredUrl {
            url: request.origin.clone(),
            stripped_params: None
        })
        .as_deref()
            == Some(request.origin.as_str())
            && origin_url.path() == "/"
            && origin_url.query().is_none()
            && origin_url.fragment().is_none(),
        "give an exact HTTP(S) origin"
    );
    let _config_lock = config.lock_current()?;
    // A separate read avoids refresh's deliberate adopted-contract writes.
    let current = config.reopen();
    ensure!(
        current.configured(),
        "choose or repair the home folder before review"
    );
    ensure!(
        request.expected_revision == current.revision(),
        "configuration changed; read it again before review"
    );
    current.adapter_document()?;
    ensure!(
        !current.adopted_contracts_unknown_for(&request.origin),
        "recorded adapter state needs repair"
    );
    let _capture_lock = crate::capture::lock_capture(current.dir()?)?;
    let storage = current
        .snapshot()
        .storage
        .as_ref()
        .context("choose a home folder")?;
    let vault = Vault::open_checked(storage, current.root_identity())?;
    let (log_revision, mut evidence) =
        ArtifactIndex::open_in(storage, current.root_identity(), &current.cache_dir()?)?
            .relearn_evidence()?;
    let files = vault.relearn_files()?;
    let raw_contracts: Vec<_> = current
        .relearn_contracts()
        .into_iter()
        .filter(|c| c["origin"].as_str() == Some(request.origin.as_str()))
        .collect();
    ensure!(
        !raw_contracts.is_empty(),
        "no recorded recognition contracts for this origin"
    );
    ensure!(
        request.proposed.format == 1,
        "unsupported proposed adapter format"
    );
    ensure!(
        request
            .proposed
            .artifacts
            .iter()
            .all(|a| a["origin"].as_str() == Some(request.origin.as_str())),
        "proposed rules must belong to the reviewed origin only"
    );
    let proposed = compile(&request.proposed.artifacts)?;
    let before = raw_contracts
        .iter()
        .map(|raw| compile(std::slice::from_ref(raw)))
        .collect::<Result<Vec<_>>>()?;
    let readers: BTreeSet<_> = raw_contracts
        .iter()
        .chain(&request.proposed.artifacts)
        .flat_map(crate::config::query_keys_read)
        .map(|(_, key)| key.to_owned())
        .collect();

    // Hash the entire inspected inventory, including unowned files. An edit or
    // an occupied destination must not disappear behind the same plan token.
    let mut inventory_hash = Sha256::new();
    let mut file_keys = BTreeMap::new();
    for (index, file) in files.iter().enumerate() {
        let path = file.relative.to_str().context("non-UTF-8 review path")?;
        inventory_hash.update(serde_json::to_vec(&(path, hash(&file.bytes)))?);
        let parsed = if file.summary {
            crate::summary::summary_identity(&file.bytes)
                .filter(|(key, url)| {
                    crate::summary::owned_summary(&file.bytes, key)
                        && vault.summary_relative_path(key, url).ok().as_ref()
                            == Some(&file.relative)
                })
                .map(|(key, url)| {
                    summary_provenance(&file.bytes).map(|names| {
                        (
                            key,
                            StoredUrl {
                                url,
                                stripped_params: names,
                            },
                        )
                    })
                })
                .transpose()?
        } else {
            let text = std::str::from_utf8(&file.bytes).ok();
            text.and_then(|text| {
                if let Some((key, _)) = crate::note::preserved_identity(text) {
                    crate::note::artifact_keyed_note_evidence(text, &key).map(|url| (key, url))
                } else {
                    crate::note::url_keyed_note_evidence(text).and_then(|stored| {
                        crate::vault::url_record_key(&stored.url)
                            .ok()
                            .map(|key| (key, stored))
                    })
                }
            })
            .filter(|(key, stored)| {
                vault.note_relative_path(key, &stored.url).ok().as_ref() == Some(&file.relative)
            })
        };
        if let Some((key, stored)) = parsed {
            file_keys.insert(index, key.clone());
            if stored_origin(&stored).as_deref() == Some(request.origin.as_str()) {
                evidence.push((key, stored));
            }
        }
    }
    // Roll-ups have durable identity even without a direct visit or note.
    // Derive them only from a contract that verifies the logged source ID.
    let mut containers = Vec::new();
    for (key, stored) in &evidence {
        if key.kind != RecordKeyKind::Artifact
            || stored_origin(stored).as_deref() != Some(request.origin.as_str())
        {
            continue;
        }
        for doc in &before {
            if let Ok(found) = doc.evaluate(&stored.url, None)
                && found.artifact_id == key.value
                && let Some(within) = found.within
            {
                containers.push((artifact_key(&within.artifact_id), stored.clone()));
                ensure!(
                    containers.len() <= MAX_EVIDENCE,
                    "too much container evidence for review"
                );
            }
        }
    }
    evidence.extend(containers);
    ensure!(
        evidence.len() <= MAX_EVIDENCE,
        "too much recorded evidence for one re-learning review"
    );
    let evidence_revision = hash(&serde_json::to_vec(&(
        log_revision,
        hex::encode(inventory_hash.finalize()),
    ))?);
    let mut rows = BTreeMap::new();
    for (key, stored) in evidence {
        if stored_origin(&stored).as_deref() != Some(request.origin.as_str()) {
            continue;
        }
        let row = rows.entry(key_order(&key)).or_insert_with(|| Impact {
            source: key.clone(),
            targets: BTreeSet::new(),
            classifications: BTreeSet::new(),
            issues: BTreeSet::new(),
            evidence_count: 0,
            files: Vec::new(),
            destination_files: Vec::new(),
        });
        row.evidence_count += 1;
        if !readers.is_empty()
            && !stored.stripped_params.as_ref().is_some_and(|names| {
                crate::url_provenance::valid_names(names)
                    && !readers
                        .iter()
                        .any(|reader| names.iter().any(|name| name.eq_ignore_ascii_case(reader)))
            })
        {
            row.issues.insert("insufficient_stripped_query_provenance");
            continue;
        }
        let old: BTreeSet<_> = before
            .iter()
            .filter_map(|doc| doc.evaluate(&stored.url, None).ok())
            .flat_map(|found| {
                std::iter::once(Identity {
                    artifact_type: found.artifact_type,
                    artifact_id: found.artifact_id,
                })
                .chain(found.within.map(|within| Identity {
                    artifact_type: within.artifact_type,
                    artifact_id: within.artifact_id,
                }))
            })
            .filter(|id| id.artifact_id == key.value)
            .collect();
        let source_type = if key.kind == RecordKeyKind::Artifact {
            if old.len() != 1 {
                row.issues.insert("recorded_identity_unresolvable");
                continue;
            }
            old.first().map(|id| id.artifact_type.as_str())
        } else {
            if crate::vault::url_record_key(&stored.url).ok().as_ref() != Some(&key) {
                row.issues.insert("url_key_mismatch");
                continue;
            }
            // A URL note stays a separate preserved copy, even if its address
            // also occurs in an artifact's evidence. Never treat it as primary.
            row.issues.insert("url_note_requires_explicit_choice");
            None
        };
        match proposed.evaluate(&stored.url, None) {
            Ok(found) => {
                let mut target = Identity {
                    artifact_type: found.artifact_type,
                    artifact_id: found.artifact_id,
                };
                // Contained visits also supply evidence for their declared
                // container. Preserve that identity when reviewing its roll-up.
                if source_type.is_some_and(|kind| kind != target.artifact_type)
                    && let Some(within) = found.within
                    && source_type == Some(within.artifact_type.as_str())
                {
                    target = Identity {
                        artifact_type: within.artifact_type,
                        artifact_id: within.artifact_id,
                    };
                }
                if source_type.is_some_and(|kind| kind != target.artifact_type) {
                    row.issues.insert("artifact_type_changed");
                }
                row.targets.insert(target);
            }
            Err(_) => {
                row.issues
                    .insert("proposed_rule_does_not_recognize_evidence");
            }
        }
    }
    let mut incoming: BTreeMap<String, BTreeSet<_>> = BTreeMap::new();
    for (source, row) in &rows {
        for target in &row.targets {
            incoming
                .entry(target.artifact_id.clone())
                .or_default()
                .insert(source.clone());
        }
    }
    let reused: BTreeSet<_> = rows
        .values()
        .flat_map(|row| &row.targets)
        .filter_map(|target| {
            let dest = rows.get(&key_order(&artifact_key(&target.artifact_id)))?;
            (dest
                .targets
                .iter()
                .any(|next| next.artifact_id != target.artifact_id)
                || !dest.issues.is_empty()
                || dest.targets.is_empty())
            .then_some(target.artifact_id.clone())
        })
        .collect();
    let by_path: BTreeMap<_, _> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.relative.clone(), i))
        .collect();
    let mut by_key: BTreeMap<_, Vec<usize>> = BTreeMap::new();
    for (&index, key) in &file_keys {
        by_key.entry(key_order(key)).or_default().push(index);
    }
    for row in rows.values_mut() {
        if !row.targets.is_empty() {
            row.classifications.insert(if row.targets.len() > 1 {
                "split"
            } else if row.targets.first().is_some_and(|target| {
                row.source.kind == RecordKeyKind::Artifact && target.artifact_id == row.source.value
            }) {
                "unchanged"
            } else {
                "changed"
            });
        }
        if row
            .targets
            .iter()
            .any(|target| incoming[target.artifact_id.as_str()].len() > 1)
        {
            row.classifications.insert("merged");
        }
        if row
            .targets
            .iter()
            .any(|target| reused.contains(&target.artifact_id))
        {
            row.issues.insert("simultaneous_key_reuse");
        }
        if !row.issues.is_empty() || row.targets.is_empty() {
            row.classifications.insert("unresolvable");
        }
        let sources: BTreeSet<_> = by_key
            .get(&key_order(&row.source))
            .into_iter()
            .flatten()
            .copied()
            .collect();
        let mut destinations = BTreeSet::new();
        for target in &row.targets {
            let key = artifact_key(&target.artifact_id);
            // Artifact filenames use only the known ID; no address selects a key.
            for path in [
                vault.note_relative_path(&key, "https://review.invalid/")?,
                vault.summary_relative_path(&key, "https://review.invalid/")?,
            ] {
                if let Some(index) = by_path.get(&path) {
                    destinations.insert(*index);
                }
            }
        }
        for index in sources.union(&destinations) {
            let file = &files[*index];
            let view = SavedFile {
                path: file.relative.to_string_lossy().into_owned(),
                kind: if file.summary { "summary" } else { "note" },
                owned: file_keys.contains_key(index),
                revision: hash(&file.bytes),
            };
            if sources.contains(index) {
                row.files.push(view);
            } else {
                if !view.owned {
                    row.issues.insert("unowned_destination");
                    row.classifications.insert("unresolvable");
                }
                row.destination_files.push(view);
            }
        }
    }
    let unowned_files = files
        .iter()
        .enumerate()
        .filter(|(index, _)| !file_keys.contains_key(index))
        .map(|(_, file)| file.relative.to_string_lossy().into_owned())
        .collect();

    let mut plan = Plan {
        review_only: true,
        origin: request.origin.clone(),
        config_revision: current.revision().into(),
        contracts_revision: hash(&serde_json::to_vec(&(
            raw_contracts,
            current.adopted_state().revision,
        ))?),
        evidence_revision,
        proposed_revision: hash(&serde_json::to_vec(&request.proposed)?),
        plan_revision: String::new(),
        identities: rows.into_values().collect(),
        unowned_files,
    };
    plan.plan_revision = hash(&serde_json::to_vec(&plan)?);
    Ok(plan)
}

pub(crate) fn run(
    config_path: Option<std::path::PathBuf>,
    input: impl Read,
    mut output: impl Write,
) -> Result<()> {
    let mut bytes = Vec::new();
    input.take(MAX_REQUEST_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_REQUEST_BYTES,
        "re-learning request is too large"
    );
    let request: PlanRequest =
        serde_json::from_slice(&bytes).context("invalid re-learning request")?;
    let config = ConfigStore::load_from(match config_path {
        Some(path) => path,
        None => ConfigStore::default_path()?,
    });
    let plan = plan(&config, &request)?;
    serde_json::to_writer(&mut output, &plan)?;
    output.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureOutcome, CapturePolicy, CaptureStore};
    use serde_json::{Value as Json, json};
    use std::fs;
    use time::format_description::well_known::Rfc3339;

    const ORIGIN: &str = "https://work.example";
    struct Fixture {
        _temp: tempfile::TempDir,
        root: std::path::PathBuf,
        config: ConfigStore,
        path: std::path::PathBuf,
    }
    fn adapter(path: &str, id: &str) -> Json {
        json!({"type":"work.item", "origin": ORIGIN, "match":[{"path":path}], "id":id})
    }
    fn standard() -> Json {
        adapter("/items/{id:int}", "work:{host}/{id}")
    }
    fn fixture(adapters: Vec<Json>) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("home");
        fs::create_dir(&root).unwrap();
        let path = temp.path().join("config.toml");
        write_config(&path, &root, adapters, Vec::new());
        let config = ConfigStore::load_from(path.clone());
        assert!(config.configured(), "{:?}", config.config_issue());
        Fixture {
            _temp: temp,
            root,
            config,
            path,
        }
    }
    fn write_config(
        path: &std::path::Path,
        root: &std::path::Path,
        adapters: Vec<Json>,
        history: Vec<Json>,
    ) {
        let raw = json!({
            "root_picker_confirmed":true,
            "root_identity":crate::vault::selected_root_identity(root).unwrap(),
            "capture_enabled":true,
            "storage":{"root":root.to_str().unwrap(), "profile":"neutral", "log_dir":"logs",
                "pages_dir":"notes", "later_dir":"later", "artifacts_dir":"artifacts", "summaries_dir":"summaries"},
            "adapters":{"format":1}, "artifacts":adapters, "adapter_id_history":history
        });
        fs::write(path, toml::to_string(&raw).unwrap()).unwrap();
    }
    impl Fixture {
        fn request(&self, artifacts: Vec<Json>) -> PlanRequest {
            PlanRequest {
                origin: ORIGIN.into(),
                expected_revision: self.config.revision().into(),
                proposed: AdapterDocument {
                    format: 1,
                    artifacts,
                },
            }
        }
        fn vault(&self) -> Vault {
            Vault::open_checked(
                self.config.snapshot().storage.as_ref().unwrap(),
                self.config.root_identity(),
            )
            .unwrap()
        }
        fn note(&self, id: &str, route: &str, provenance: Option<Vec<String>>) {
            let key = artifact_key(id);
            crate::note::save_keyed_note_with_provenance(
                &self.vault(),
                &StoredUrl {
                    url: format!("{ORIGIN}{route}"),
                    stripped_params: provenance,
                },
                &key,
                "Title",
                "Keep this body",
                "missing",
            )
            .unwrap();
        }
        fn summary(&self, id: &str, route: &str) {
            let url = format!("{ORIGIN}{route}");
            let key = artifact_key(id);
            let bytes = format!(
                "---\ntitle: \"Summary\"\nbrauser:\n  kind: summary\n  canonical_url: {}\n  artifact_id: {}\n  stripped_params: []\n---\n\nKeep this summary.\n",
                serde_json::to_string(&url).unwrap(),
                serde_json::to_string(id).unwrap()
            );
            self.vault().write_summary(&key, &url, &bytes).unwrap();
        }
        fn log(&self, id: &str, route: &str) -> std::path::PathBuf {
            let store = CaptureStore::open_in(
                self.config.snapshot().storage.as_ref().unwrap(),
                self.config.root_identity(),
                self.config.dir().unwrap(),
            )
            .unwrap();
            let event = brauser_protocol::WhyEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                artifact_id: id.into(),
                url: format!("{ORIGIN}{route}"),
                text: "Keep context".into(),
                occurred_at: crate::capture::fixtures::event_time()
                    .format(&Rfc3339)
                    .unwrap(),
                incognito: false,
            };
            let outcome = store.record_why(
                CapturePolicy {
                    enabled: true,
                    origins: &[ORIGIN.into()],
                    strip_params: &[],
                },
                &event,
            );
            assert!(
                matches!(outcome, CaptureOutcome::Persisted { .. }),
                "{outcome:?}"
            );
            match outcome {
                CaptureOutcome::Persisted { relative_path } => self.root.join(relative_path),
                _ => unreachable!(),
            }
        }
        fn review(&self, adapters: Vec<Json>) -> Plan {
            plan(&self.config, &self.request(adapters)).unwrap()
        }
    }
    fn row<'a>(plan: &'a Plan, key: &str) -> &'a Impact {
        plan.identities
            .iter()
            .find(|r| r.source.value == key)
            .unwrap()
    }

    #[test]
    fn cli_request_uses_injected_configuration_and_never_accepts_supplied_evidence() {
        let f = fixture(vec![standard()]);
        f.note("work:work.example/1", "/items/1", None);
        let mut input = json!({"origin":ORIGIN, "expected_revision":f.config.revision(),
            "proposed":{"format":1,"artifacts":[standard()]}});
        let mut output = Vec::new();
        run(
            Some(f.path.clone()),
            input.to_string().as_bytes(),
            &mut output,
        )
        .unwrap();
        let got: Json = serde_json::from_slice(&output).unwrap();
        assert_eq!(got["review_only"], true);
        assert_eq!(
            got["identities"][0]["source"]["value"],
            "work:work.example/1"
        );
        input["evidence"] = json!([]);
        output.clear();
        assert!(
            run(
                Some(f.path.clone()),
                input.to_string().as_bytes(),
                &mut output
            )
            .is_err()
        );
        assert!(output.is_empty());
    }

    #[test]
    fn incomplete_logs_refuse_without_creating_cache_checkpoints() {
        let f = fixture(vec![standard()]);
        let logs = f.root.join("logs");
        fs::create_dir(&logs).unwrap();
        fs::File::create(logs.join("large.md"))
            .unwrap()
            .set_len(65 * 1024 * 1024)
            .unwrap();
        assert!(plan(&f.config, &f.request(vec![standard()])).is_err());
        assert!(!f.config.cache_dir().unwrap().exists());
    }

    #[test]
    fn plans_changed_and_unchanged_from_real_logs_without_cache_or_content_writes() {
        let f = fixture(vec![standard()]);
        f.log("work:work.example/1", "/items/1");
        f.note("work:work.example/2", "/items/2", Some(vec![]));
        let before_config = fs::read(&f.path).unwrap();
        let before_note = fs::read_dir(f.root.join("notes"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = fs::read(&before_note).unwrap();
        let same = f.review(vec![standard()]);
        assert!(
            same.identities
                .iter()
                .all(|r| r.classifications == BTreeSet::from(["unchanged"]))
        );
        let next = f.review(vec![adapter("/items/{id:int}", "new:{host}/{id}")]);
        assert_eq!(next.identities.len(), 2);
        assert!(
            next.identities
                .iter()
                .all(|r| r.classifications == BTreeSet::from(["changed"]))
        );
        assert_ne!(same.plan_revision, next.plan_revision);
        assert_eq!(fs::read(&f.path).unwrap(), before_config);
        assert_eq!(fs::read(before_note).unwrap(), bytes);
        assert!(!f.config.cache_dir().unwrap().exists());
        assert!(
            !f.config
                .dir()
                .unwrap()
                .join("adopted-contracts.json")
                .exists()
        );
        assert!(next.review_only);
        let mut update = f.config.snapshot().clone();
        update.adapters = f
            .request(vec![adapter("/items/{id:int}", "new:{host}/{id}")])
            .proposed;
        assert!(crate::config::validate_adapter_transition(f.config.snapshot(), &update).is_err());
    }

    #[test]
    fn reports_splits_and_merges_instead_of_sequential_aliases() {
        let old = adapter("/items/{id:int}/{_}", "work:{host}/{id}");
        let f = fixture(vec![old]);
        f.log("work:work.example/1", "/items/1/10");
        f.log("work:work.example/1", "/items/1/20");
        f.log("work:work.example/2", "/items/2/10");
        let p = f.review(vec![adapter("/items/{_}/{part:int}", "new:{host}/{part}")]);
        let one = row(&p, "work:work.example/1");
        assert_eq!(one.targets.len(), 2);
        assert!(one.classifications.contains("split"));
        assert!(one.classifications.contains("merged"));
        assert!(
            row(&p, "work:work.example/2")
                .classifications
                .contains("merged")
        );
    }

    #[test]
    fn detects_swaps_and_reused_keys_even_when_sequential_aliases_would_collapse_them() {
        let a = adapter("/a/{id:int}", "work:{host}/a/{id}");
        let b = adapter("/b/{id:int}", "work:{host}/b/{id}");
        let f = fixture(vec![a.clone(), b.clone()]);
        f.note("work:work.example/a/1", "/a/1", None);
        f.note("work:work.example/b/1", "/b/1", None);
        let swapped = f.review(vec![
            adapter("/a/{id:int}", "work:{host}/b/{id}"),
            adapter("/b/{id:int}", "work:{host}/a/{id}"),
        ]);
        assert!(
            swapped
                .identities
                .iter()
                .all(|r| r.issues.contains("simultaneous_key_reuse"))
        );
        let reused = f.review(vec![
            adapter("/a/{id:int}", "work:{host}/b/{id}"),
            adapter("/b/{id:int}", "work:{host}/c/{id}"),
        ]);
        assert!(
            row(&reused, "work:work.example/a/1")
                .issues
                .contains("simultaneous_key_reuse")
        );
        assert!(
            !row(&reused, "work:work.example/b/1")
                .issues
                .contains("simultaneous_key_reuse")
        );
    }

    #[test]
    fn keeps_notes_and_summaries_on_both_sides_visible_and_unchanged() {
        let f = fixture(vec![standard()]);
        for id in ["1", "2"] {
            f.note(
                &format!("work:work.example/{id}"),
                &format!("/items/{id}"),
                Some(vec![]),
            );
            f.summary(&format!("work:work.example/{id}"), &format!("/items/{id}"));
        }
        let files = f.vault().relearn_files().unwrap();
        let next = adapter("/items/(1|2)", "work:{host}/2");
        let p = f.review(vec![next]);
        let one = row(&p, "work:work.example/1");
        assert_eq!(one.files.len(), 2);
        assert_eq!(one.destination_files.len(), 2);
        assert!(one.destination_files.iter().all(|file| file.owned));
        assert!(one.classifications.contains("merged"));
        for file in files {
            assert_eq!(fs::read(f.root.join(file.relative)).unwrap(), file.bytes);
        }
    }

    #[test]
    fn summaries_without_logs_are_evidence_and_unowned_destinations_are_reported() {
        let f = fixture(vec![standard()]);
        f.summary("work:work.example/1", "/items/1");
        let target = artifact_key("new:work.example/1");
        let path = f
            .vault()
            .note_relative_path(&target, "https://work.example/items/1")
            .unwrap();
        fs::create_dir_all(f.root.join("notes")).unwrap();
        fs::write(f.root.join(&path), "Personal file; leave alone").unwrap();
        let p = f.review(vec![adapter("/items/{id:int}", "new:{host}/{id}")]);
        let r = row(&p, "work:work.example/1");
        assert_eq!(r.files[0].kind, "summary");
        assert_eq!(r.destination_files.len(), 1);
        assert!(!r.destination_files[0].owned);
        assert_eq!(
            fs::read_to_string(f.root.join(path)).unwrap(),
            "Personal file; leave alone"
        );
    }

    #[test]
    fn query_and_absence_readers_refuse_unknown_or_removed_evidence() {
        for condition in [
            json!({"query":{"view":"detail"}}),
            json!({"absent":["view"]}),
        ] {
            let f = fixture(vec![standard()]);
            f.note("work:work.example/1", "/items/1", None);
            f.note("work:work.example/2", "/items/2", Some(vec!["VIEW".into()]));
            f.note("work:work.example/3", "/items/3?view=detail", Some(vec![]));
            let mut proposed = standard();
            for (key, value) in condition.as_object().unwrap() {
                proposed["match"][0][key] = value.clone();
            }
            let p = f.review(vec![proposed]);
            for id in ["1", "2"] {
                assert!(
                    row(&p, &format!("work:work.example/{id}"))
                        .issues
                        .contains("insufficient_stripped_query_provenance")
                );
            }
            assert!(
                !row(&p, "work:work.example/3")
                    .issues
                    .contains("insufficient_stripped_query_provenance")
            );
        }
    }

    #[test]
    fn summary_provenance_is_read_from_frontmatter_and_never_from_agent_text() {
        for (replacement, refused) in [
            ("", true),
            ("  stripped_params: [\"VIEW\"]\n", true),
            ("  stripped_params: []\n", false),
        ] {
            let f = fixture(vec![standard()]);
            f.summary("work:work.example/1", "/items/1");
            let file = f.vault().relearn_files().unwrap().remove(0);
            let contents = String::from_utf8(file.bytes)
                .unwrap()
                .replace("  stripped_params: []\n", replacement)
                + "\nbrauser:\n  stripped_params: []\n";
            fs::write(f.root.join(file.relative), contents).unwrap();
            let mut proposed = standard();
            proposed["match"][0]["absent"] = json!(["view"]);
            let p = f.review(vec![proposed]);
            assert_eq!(
                p.identities[0]
                    .issues
                    .contains("insufficient_stripped_query_provenance"),
                refused
            );
        }
    }

    #[test]
    fn conflicting_owned_markers_refuse_instead_of_trusting_a_cached_winner() {
        let f = fixture(vec![standard()]);
        let path = f.log("work:work.example/1", "/items/1");
        let contents = fs::read_to_string(&path).unwrap();
        let original = contents.lines().last().unwrap();
        let edited = original.replace(&hex::encode("Keep context"), &hex::encode("Other context"));
        assert_ne!(original, edited);
        fs::write(path, format!("{contents}{edited}\n")).unwrap();
        assert!(
            format!(
                "{:#}",
                plan(&f.config, &f.request(vec![standard()])).unwrap_err()
            )
            .contains("conflicting recorded event IDs")
        );
        assert!(!f.config.cache_dir().unwrap().exists());
    }

    #[test]
    fn retired_contract_readers_protect_evidence_and_removed_routes_still_review() {
        let f = fixture(vec![standard()]);
        f.note("work:work.example/1", "/items/1", None);
        let mut retired = adapter("/past/{id:int}", "past:{host}/{id}");
        retired["match"][0]["absent"] = json!(["mode"]);
        write_config(&f.path, &f.root, vec![standard()], vec![retired]);
        let current = f.config.reopen();
        let mut request = f.request(vec![standard()]);
        request.expected_revision = current.revision().into();
        let p = plan(&f.config, &request).unwrap();
        assert!(
            p.identities[0]
                .issues
                .contains("insufficient_stripped_query_provenance")
        );
        write_config(&f.path, &f.root, Vec::new(), vec![standard()]);
        request.expected_revision = f.config.reopen().revision().into();
        let p = plan(&f.config, &request).unwrap();
        assert!(p.identities[0].classifications.contains("unchanged"));
    }

    #[test]
    fn hand_removed_contracts_are_included_and_damaged_state_refuses_review() {
        let mut f = fixture(vec![standard()]);
        {
            let _lock = f.config.lock_current().unwrap();
            f.config.refresh().unwrap();
        }
        f.note("work:work.example/1", "/items/1", None);
        write_config(&f.path, &f.root, Vec::new(), Vec::new());
        let mut request = f.request(vec![standard()]);
        request.expected_revision = f.config.reopen().revision().into();
        assert!(
            plan(&f.config, &request).unwrap().identities[0]
                .classifications
                .contains("unchanged")
        );
        fs::write(
            f.config.dir().unwrap().join("adopted-contracts.json"),
            "broken",
        )
        .unwrap();
        assert!(
            format!("{:#}", plan(&f.config, &request).unwrap_err())
                .contains("recorded adapter state needs repair")
        );
    }

    #[test]
    fn revisions_change_for_same_identity_log_delivery_file_edit_and_config_change() {
        let f = fixture(vec![standard()]);
        f.note("work:work.example/1", "/items/1", None);
        let first = f.review(vec![standard()]);
        assert_eq!(
            first.plan_revision,
            f.review(vec![standard()]).plan_revision
        );
        f.log("work:work.example/1", "/items/1");
        let second = f.review(vec![standard()]);
        assert_ne!(first.evidence_revision, second.evidence_revision);
        let path = f.root.join(&second.identities[0].files[0].path);
        let contents = fs::read_to_string(&path).unwrap();
        fs::write(path, contents.replace("Keep this body", "A new body")).unwrap();
        let third = f.review(vec![standard()]);
        assert_ne!(second.plan_revision, third.plan_revision);
        let config = fs::read_to_string(&f.path).unwrap();
        fs::write(
            &f.path,
            config.replace("capture_enabled = true", "capture_enabled = false"),
        )
        .unwrap();
        assert!(
            format!(
                "{:#}",
                plan(&f.config, &f.request(vec![standard()])).unwrap_err()
            )
            .contains("configuration changed")
        );
    }

    #[test]
    fn a_url_note_stays_separate_from_an_artifact_at_the_same_address() {
        let f = fixture(vec![standard()]);
        f.note("work:work.example/1", "/items/1", Some(vec![]));
        crate::note::save_note(
            &f.vault(),
            "https://work.example/items/1",
            "URL copy",
            "Separate",
            "missing",
        )
        .unwrap();
        let p = f.review(vec![standard()]);
        assert_eq!(p.identities.len(), 2);
        let url = p
            .identities
            .iter()
            .find(|r| r.source.kind == RecordKeyKind::Url)
            .unwrap();
        assert!(url.issues.contains("url_note_requires_explicit_choice"));
        assert_eq!(url.files.len(), 1);
        assert!(
            row(&p, "work:work.example/1")
                .classifications
                .contains("merged")
        );
    }

    #[test]
    fn unrecognized_and_cross_type_proposals_are_unresolvable() {
        let f = fixture(vec![standard()]);
        f.note("work:work.example/1", "/items/1", None);
        let p = f.review(vec![]);
        assert!(
            p.identities[0]
                .issues
                .contains("proposed_rule_does_not_recognize_evidence")
        );
        let mut next = standard();
        next["type"] = json!("work.other");
        let p = f.review(vec![next]);
        assert!(p.identities[0].issues.contains("artifact_type_changed"));
    }

    #[test]
    fn contained_history_includes_container_impact_without_a_direct_open() {
        let mut child = adapter(
            "/repos/{repo:int}/items/{id:int}",
            "work:{host}/{repo}/{id}",
        );
        child["within"] = json!({"type":"work.repo", "id":"repo:{host}/{repo}"});
        let mut parent = adapter("/repos/{repo:int}/{*}", "repo:{host}/{repo}");
        parent["type"] = json!("work.repo");
        let f = fixture(vec![child.clone(), parent.clone()]);
        f.log("work:work.example/10/1", "/repos/10/items/1");
        let p = f.review(vec![child.clone(), parent.clone()]);
        assert_eq!(p.identities.len(), 2);
        assert!(
            p.identities
                .iter()
                .all(|r| r.classifications == BTreeSet::from(["unchanged"]))
        );
        child["within"]["id"] = json!("nextrepo:{host}/{repo}");
        parent["id"] = json!("nextrepo:{host}/{repo}");
        let p = f.review(vec![child, parent]);
        assert!(
            row(&p, "repo:work.example/10")
                .classifications
                .contains("changed")
        );
        assert_eq!(
            row(&p, "repo:work.example/10")
                .targets
                .first()
                .unwrap()
                .artifact_id,
            "nextrepo:work.example/10"
        );
    }

    #[test]
    fn unrelated_origins_and_wrong_filenames_never_supply_identity() {
        let f = fixture(vec![standard()]);
        f.note("work:work.example/1", "/items/1", None);
        let file = f.vault().relearn_files().unwrap().remove(0);
        fs::rename(f.root.join(file.relative), f.root.join("notes/copy.md")).unwrap();
        assert!(f.review(vec![standard()]).identities.is_empty());
        let mut request = f.request(vec![standard()]);
        request.proposed.artifacts[0]["origin"] = json!("https://another.example");
        assert!(
            format!("{:#}", plan(&f.config, &request).unwrap_err())
                .contains("reviewed origin only")
        );
        request.origin = "https://work.example/".into();
        assert!(
            format!("{:#}", plan(&f.config, &request).unwrap_err())
                .contains("exact HTTP(S) origin")
        );
    }

    #[test]
    fn oversized_inventory_refuses_instead_of_reporting_partial_impact() {
        let f = fixture(vec![standard()]);
        fs::create_dir(f.root.join("notes")).unwrap();
        fs::File::create(f.root.join("notes/large.md"))
            .unwrap()
            .set_len(4 * 1024 * 1024 + 1)
            .unwrap();
        assert!(
            format!(
                "{:#}",
                plan(&f.config, &f.request(vec![standard()])).unwrap_err()
            )
            .contains("cannot inspect a Markdown file")
        );
    }

    // POSIX symlink fixtures are unavailable without Windows symlink privileges.
    #[cfg(unix)]
    #[test]
    fn symlinked_markdown_refuses_incomplete_review() {
        let f = fixture(vec![standard()]);
        fs::create_dir(f.root.join("notes")).unwrap();
        std::os::unix::fs::symlink("missing", f.root.join("notes/link.md")).unwrap();
        assert!(plan(&f.config, &f.request(vec![standard()])).is_err());
    }
}

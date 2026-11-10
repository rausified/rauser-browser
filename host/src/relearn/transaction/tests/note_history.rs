//! Exercise ordinary native dispatch with real published transaction history.
use super::*;
use brauser_protocol::{ErrorCode, NoteLoaded, PROTOCOL_VERSION, Request, Response};

const URL: &str = "https://work.example/items/1";
const OLD: &str = "old:work.example/1";
const NEW: &str = "new:work.example/1";

fn key(id: &str) -> RecordKey {
    RecordKey {
        kind: RecordKeyKind::Artifact,
        value: id.into(),
    }
}

fn call(f: &Fixture, request: Request) -> Response {
    let mut input = Vec::new();
    crate::frame::write_request(&mut input, &request).unwrap();
    let mut output = Vec::new();
    crate::native::serve_with_context(
        std::io::Cursor::new(input),
        &mut output,
        ConfigStore::load_from(f.path.clone()),
        &crate::native::SetupContext::new(),
    )
    .unwrap();
    let mut reader = std::io::Cursor::new(output);
    let response = crate::frame::read_frame(&mut reader).unwrap().unwrap();
    assert!(crate::frame::read_frame(&mut reader).unwrap().is_none());
    serde_json::from_slice(&response).unwrap()
}

fn load_request(url: &str) -> Request {
    Request::LoadNote(brauser_protocol::LoadNoteRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: "load-history".into(),
        url: url.into(),
    })
}

fn load(f: &Fixture, url: &str) -> NoteLoaded {
    match call(f, load_request(url)) {
        Response::NoteLoaded(note) => note,
        response => panic!("expected loaded note: {response:?}"),
    }
}

fn save_request(loaded: &NoteLoaded, body: &str) -> Request {
    Request::SaveNote(brauser_protocol::SaveNoteRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: "save-history".into(),
        url: URL.into(),
        title: "Edited".into(),
        body: body.into(),
        expected_key: loaded.key.clone(),
        expected_revision: loaded.revision.clone(),
    })
}

// Ordinary loads record adopted contracts, and extra URL notes change review
// evidence. Each fixture must obtain a fresh approval before private apply.
fn reapprove(f: &mut Fixture) {
    f.request.operation_id = uuid::Uuid::new_v4().to_string();
    f.request.expected_plan_revision = super::super::super::plan(&f.config, &f.request.review)
        .unwrap()
        .plan_revision;
    journal::record(&f.config, &f.request).unwrap();
}

fn vault(f: &Fixture) -> crate::vault::Vault {
    crate::vault::Vault::open_checked(
        f.config.snapshot().storage.as_ref().unwrap(),
        f.config.root_identity(),
    )
    .unwrap()
}

fn assert_refused(response: Response) {
    match response {
        Response::Error(err) => {
            assert_eq!(err.code, ErrorCode::Internal);
            assert!(err.message.contains("re-key history"), "{err:?}");
        }
        response => panic!("expected unavailable identity: {response:?}"),
    }
}

#[test]
fn ordinary_notes_without_history_keep_direct_revisions_and_create_no_history() {
    let f = Fixture::new();
    let original = load(&f, URL);
    assert_eq!(original.key, key(OLD));
    assert!(!original.revision.starts_with("location-v1:"));
    assert!(matches!(
        call(&f, save_request(&original, "ordinary edit")),
        Response::NoteSaved(_)
    ));
    assert_eq!(load(&f, URL).body, "ordinary edit");
    assert!(!f.history_path().exists());
    assert!(!f.pending_path().exists());
}

#[test]
fn removed_site_keeps_the_rekeyed_note_when_retired_contracts_agree_after_replay() {
    let f = Fixture::new();
    commit(&f.config, &f.request).unwrap();
    let current = load(&f, URL);
    let v = vault(&f);
    crate::note::save_note(&v, URL, "Older URL note", "separate", "missing").unwrap();
    let mut config = ConfigStore::load_from(f.path.clone());
    let mut next = config.snapshot().clone();
    next.adapters
        .artifacts
        .retain(|rule| rule["origin"] != ORIGIN);
    let revision = config.revision().to_owned();
    config.update(next, &revision).unwrap().unwrap();
    let removed = load(&f, URL);
    assert_eq!(removed.key, key(NEW));
    assert_eq!(removed.body, current.body);
    assert_eq!(removed.revision, current.revision);
    assert_eq!(removed.older_notes.len(), 1);
    match call(
        &f,
        Request::ViewOlderNote(brauser_protocol::ViewOlderNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "view-removed-site".into(),
            url: URL.into(),
            source_url: URL.into(),
        }),
    ) {
        Response::NoteLoaded(note) => assert_eq!(note.body, "separate"),
        response => panic!("expected older URL note: {response:?}"),
    }
    assert!(matches!(
        call(&f, save_request(&removed, "edit with capture removed")),
        Response::NoteSaved(_)
    ));
    assert_eq!(load(&f, URL).body, "edit with capture removed");
    assert_eq!(crate::note::load_note(&v, URL).unwrap().body, "separate");
}

#[test]
fn removed_site_stays_ambiguous_when_a_retired_contract_names_an_unaliased_artifact() {
    let f = Fixture::new();
    commit(&f.config, &f.request).unwrap();
    let current = load(&f, URL);
    let mut config = ConfigStore::load_from(f.path.clone());
    let mut next = config.snapshot().clone();
    next.adapters
        .artifacts
        .retain(|rule| rule["origin"] != ORIGIN);
    let revision = config.revision().to_owned();
    config.update(next, &revision).unwrap().unwrap();
    let mut raw: toml::Table = toml::from_str(&fs::read_to_string(&f.path).unwrap()).unwrap();
    raw.get_mut("adapter_id_history")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(toml::Value::try_from(adapter("unrelated:{host}/{id}")).unwrap());
    fs::write(&f.path, toml::to_string(&raw).unwrap()).unwrap();
    let originals = f.notes();
    let ambiguous = load(&f, URL);
    assert_eq!(ambiguous.key, crate::vault::url_record_key(URL).unwrap());
    assert!(!ambiguous.exists);
    match call(&f, save_request(&current, "must not guess")) {
        Response::NoteConflict(conflict) => assert_eq!(conflict.reason, "key_changed"),
        response => panic!("expected ambiguous-key conflict: {response:?}"),
    }
    assert_eq!(f.notes(), originals);
}

#[test]
fn ordinary_notes_follow_published_history_and_edit_original_physical_owner() {
    let mut f = Fixture::new();
    let original = load(&f, URL);
    reapprove(&mut f);
    commit(&f.config, &f.request).unwrap();
    let history = fs::read(f.history_path()).unwrap();
    let decisions = f.decisions();
    let current = load(&f, URL);
    assert_eq!(current.key, key(NEW));
    assert_eq!(current.body, original.body);
    assert!(current.revision.starts_with("location-v1:"));
    match call(&f, save_request(&original, "old editor must conflict")) {
        Response::NoteConflict(conflict) => {
            assert_eq!(conflict.reason, "key_changed");
            assert_eq!(conflict.key, current.key);
            assert_eq!(conflict.body, original.body);
        }
        response => panic!("expected old-key conflict: {response:?}"),
    }
    let physical = vault(&f).note_relative_path(&key(OLD), URL).unwrap();
    match call(&f, save_request(&current, "edit through new identity")) {
        Response::NoteSaved(saved) => {
            assert_eq!(saved.key, key(NEW));
            assert_eq!(saved.relative_path, physical.to_string_lossy());
        }
        response => panic!("expected original-file save: {response:?}"),
    }
    assert!(vault(&f).read_note(&key(NEW), URL).unwrap().is_none());
    assert_eq!(load(&f, URL).body, "edit through new identity");
    assert_eq!(fs::read(f.history_path()).unwrap(), history);
    assert_eq!(f.decisions(), decisions);
}

#[test]
fn canonical_note_wins_without_overwriting_alias_or_url_notes() {
    let f = Fixture::new();
    let v = vault(&f);
    let url_key = crate::vault::url_record_key(URL).unwrap();
    commit(&f.config, &f.request).unwrap();
    crate::note::save_keyed_note(
        &v,
        URL,
        &url_key,
        "URL note",
        "separate URL note",
        "missing",
    )
    .unwrap();
    let before = load(&f, URL);
    crate::note::save_keyed_note(&v, URL, &key(NEW), "Canonical", "canonical note", "missing")
        .unwrap();
    let originals = f.notes();
    let current = load(&f, URL);
    assert_eq!(current.body, "canonical note");
    assert!(
        current
            .older_notes
            .iter()
            .any(|note| note.canonical_url == URL)
    );
    match call(
        &f,
        save_request(&before, "do not change physical selection"),
    ) {
        Response::NoteConflict(conflict) => {
            assert_eq!(conflict.reason, "stale");
            assert_eq!(conflict.key, key(NEW));
            assert_eq!(conflict.body, "canonical note");
        }
        response => panic!("expected selection conflict: {response:?}"),
    }
    match call(
        &f,
        Request::UseArtifactNote(brauser_protocol::UseArtifactNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "import-history".into(),
            url: URL.into(),
            source_url: URL.into(),
            expected_revision: current.revision,
        }),
    ) {
        Response::Error(err) => assert_eq!(err.code, ErrorCode::Conflict),
        response => panic!("expected guarded import: {response:?}"),
    }
    match call(
        &f,
        Request::ViewOlderNote(brauser_protocol::ViewOlderNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "view-history".into(),
            url: URL.into(),
            source_url: URL.into(),
        }),
    ) {
        Response::NoteLoaded(note) => {
            assert_eq!(note.key, url_key);
            assert_eq!(note.body, "separate URL note");
        }
        response => panic!("expected read-only URL note: {response:?}"),
    }
    assert_eq!(f.notes(), originals);
}

#[test]
fn importing_an_older_url_note_refuses_alias_owner_and_returns_a_usable_bound_revision() {
    let f = Fixture::new();
    commit(&f.config, &f.request).unwrap();
    let v = vault(&f);
    let url_key = crate::vault::url_record_key(URL).unwrap();
    crate::note::save_keyed_note(&v, URL, &url_key, "Older URL note", "import me", "missing")
        .unwrap();
    let source = crate::note::load_older_note(&v, URL).unwrap();
    let request = || {
        Request::UseArtifactNote(brauser_protocol::UseArtifactNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "import-history".into(),
            url: URL.into(),
            source_url: URL.into(),
            expected_revision: source.revision.clone(),
        })
    };
    let originals = f.notes();
    // No canonical file exists, but the alias owns the selected note.
    match call(&f, request()) {
        Response::Error(err) => assert_eq!(err.code, ErrorCode::Conflict),
        response => panic!("expected alias-owner refusal: {response:?}"),
    }
    assert_eq!(f.notes(), originals);
    // Explicit deletion of that file leaves the group without an owner.
    fs::remove_file(f.home.join(v.note_relative_path(&key(OLD), URL).unwrap())).unwrap();
    let imported = match call(&f, request()) {
        Response::NoteLoaded(note) => note,
        response => panic!("expected import into empty group: {response:?}"),
    };
    assert_eq!(imported.key, key(NEW));
    assert_eq!(imported.body, "import me");
    assert!(imported.revision.starts_with("location-v1:"));
    assert!(matches!(
        call(&f, save_request(&imported, "edit after import")),
        Response::NoteSaved(_)
    ));
    assert_eq!(load(&f, URL).body, "edit after import");
    assert!(v.read_note(&key(OLD), URL).unwrap().is_none());
}

#[test]
fn foreign_canonical_file_refuses_instead_of_editing_alias_note() {
    let f = Fixture::new();
    commit(&f.config, &f.request).unwrap();
    let v = vault(&f);
    let path = f.home.join(v.note_relative_path(&key(NEW), URL).unwrap());
    fs::write(&path, "User-owned content, not a Brauser note").unwrap();
    let originals = f.notes();
    match call(&f, load_request(URL)) {
        Response::Error(err) => assert_eq!(err.code, ErrorCode::Conflict),
        response => panic!("expected ownership conflict: {response:?}"),
    }
    assert_eq!(f.notes(), originals);
}

#[test]
fn pending_publication_refuses_ordinary_notes_without_running_recovery() {
    for phase in ["prepared", "history_appended", "config_published"] {
        let mut f = Fixture::new();
        let original = load(&f, URL);
        reapprove(&mut f);
        assert!(commit_with(&f.config, &f.request, &fail_at(phase)).is_err());
        let pending = fs::read(f.pending_path()).unwrap();
        let history = fs::read(f.history_path()).ok();
        let config = fs::read(&f.path).unwrap();
        let notes = f.notes();
        assert_refused(call(&f, load_request(URL)));
        assert_refused(call(&f, save_request(&original, "must not save")));
        assert_eq!(fs::read(f.pending_path()).unwrap(), pending);
        assert_eq!(fs::read(f.history_path()).ok(), history);
        assert_eq!(fs::read(&f.path).unwrap(), config);
        assert_eq!(f.notes(), notes);
    }
}

#[test]
fn torn_corrupt_and_foreign_history_refuse_notes_but_preserved_view_stays_independent() {
    for mode in [
        "torn",
        "corrupt",
        "foreign-home",
        "foreign-identity",
        "unknown-event-field",
        "unknown-endpoint-field",
    ] {
        let mut f = Fixture::new();
        let original = load(&f, URL);
        reapprove(&mut f);
        let mut entry = commit(&f.config, &f.request).unwrap();
        let bytes = match mode {
            "torn" => serde_json::to_vec(&entry).unwrap(),
            "corrupt" => b"{broken}\n".to_vec(),
            "unknown-event-field" | "unknown-endpoint-field" => {
                let raw = serde_json::to_string(&entry).unwrap();
                let field = if mode == "unknown-event-field" {
                    "action"
                } else {
                    "artifact_type"
                };
                format!(
                    "{}\n",
                    raw.replace(
                        &format!("\"{field}\":"),
                        &format!("\"worker_path\":\"untrusted\",\"{field}\":")
                    )
                )
                .into_bytes()
            }
            _ => {
                match mode {
                    "foreign-home" => entry.payload.home_root.push_str("-foreign"),
                    "foreign-identity" => entry.payload.home_identity.push_str("-foreign"),
                    _ => unreachable!(),
                }
                entry.checksum = hash(&serde_json::to_vec(&entry.payload).unwrap());
                format!("{}\n", serde_json::to_string(&entry).unwrap()).into_bytes()
            }
        };
        fs::write(f.history_path(), &bytes).unwrap();
        let notes = f.notes();
        assert_refused(call(&f, load_request(URL)));
        assert_refused(call(&f, save_request(&original, "must not save")));
        match call(
            &f,
            Request::ListPreservedNotes(brauser_protocol::ListPreservedNotesRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "preserved-history".into(),
                url: URL.into(),
            }),
        ) {
            Response::PreservedNotesListed(list) => assert!(!list.notes.is_empty()),
            response => panic!("expected independent preserved notes: {response:?}"),
        }
        assert_eq!(fs::read(f.history_path()).unwrap(), bytes);
        assert_eq!(f.notes(), notes);
    }
}

#[test]
fn note_history_requires_a_matching_live_config_lock() {
    let f = Fixture::new();
    commit(&f.config, &f.request).unwrap();
    let other = ConfigStore::load_from(f.path.with_file_name("other.toml"));
    let guard = other.lock_for_read().unwrap();
    assert!(
        note_resolution(&f.config, &guard)
            .err()
            .unwrap()
            .to_string()
            .contains("another path")
    );
}

#[test]
fn move_back_history_does_not_restore_an_old_editors_save_revision() {
    let mut f = Fixture::new();
    let original_config = fs::read(&f.path).unwrap();
    let original = load(&f, URL);
    reapprove(&mut f);
    let first = commit(&f.config, &f.request).unwrap();
    let moved = load(&f, URL);
    // A valid two-entry replay fixture, not a claim that multi-apply review
    // is integrated yet. The ordinary reader must respect its entire chain.
    let mut back: Entry = serde_json::from_value(serde_json::to_value(&first).unwrap()).unwrap();
    back.payload.sequence = 2;
    back.payload.previous_checksum = first.checksum.clone();
    back.payload.operation_id = uuid::Uuid::new_v4().to_string();
    back.payload.before_hash = first.payload.after_hash.clone();
    back.payload.after_hash = hash(&original_config);
    let event = &mut back.payload.events[0];
    std::mem::swap(&mut event.from, &mut event.to);
    event.event_id = format!("{}:0", back.payload.operation_id);
    back.checksum = hash(&serde_json::to_vec(&back.payload).unwrap());
    let history = format!(
        "{}\n{}\n",
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&back).unwrap()
    );
    fs::write(f.history_path(), &history).unwrap();
    fs::write(&f.path, original_config).unwrap();
    let notes = f.notes();
    let current = load(&f, URL);
    assert_eq!(current.key, original.key);
    assert_eq!(current.body, original.body);
    assert_ne!(current.revision, original.revision);
    match call(&f, save_request(&original, "pre-history edit")) {
        Response::NoteConflict(conflict) => assert_eq!(conflict.reason, "stale"),
        response => panic!("expected history-bound stale conflict: {response:?}"),
    }
    match call(&f, save_request(&moved, "first-move edit")) {
        Response::NoteConflict(conflict) => assert_eq!(conflict.reason, "key_changed"),
        response => panic!("expected moved-key conflict: {response:?}"),
    }
    assert_eq!(f.notes(), notes);
    assert_eq!(fs::read_to_string(f.history_path()).unwrap(), history);
}

#[test]
fn older_url_discovery_checks_the_original_alias_notes_provenance() {
    let mut f = Fixture::new();
    let v = vault(&f);
    let candidate = "https://work.example/items/1?view=detail";
    let old = crate::note::load_keyed_note(&v, URL, &key(OLD), &[]).unwrap();
    crate::note::save_keyed_note_with_provenance(
        &v,
        &crate::url_provenance::StoredUrl {
            url: candidate.into(),
            stripped_params: Some(vec![]),
        },
        &key(OLD),
        "Artifact note",
        "original route",
        &old.revision,
    )
    .unwrap();
    reapprove(&mut f);
    commit(&f.config, &f.request).unwrap();
    crate::note::save_keyed_note(
        &v,
        candidate,
        &crate::vault::url_record_key(candidate).unwrap(),
        "Older URL note",
        "view me",
        "missing",
    )
    .unwrap();
    let before = f.notes();
    let current = load(&f, URL);
    assert_eq!(current.key, key(NEW));
    assert!(
        current
            .older_notes
            .iter()
            .any(|note| note.canonical_url == candidate)
    );
    match call(
        &f,
        Request::ViewOlderNote(brauser_protocol::ViewOlderNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "view-alias-route".into(),
            url: URL.into(),
            source_url: candidate.into(),
        }),
    ) {
        Response::NoteLoaded(note) => assert_eq!(note.body, "view me"),
        response => panic!("expected alias route's older note: {response:?}"),
    }
    assert_eq!(f.notes(), before);
}

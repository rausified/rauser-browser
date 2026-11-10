use super::*;
use crate::{
    relearn::{
        Identity,
        aliases::{Action, Event},
    },
    url_provenance::StoredUrl,
};
use std::fs;

const URL: &str = "https://work.example/items/1";

fn key(id: &str) -> RecordKey {
    RecordKey {
        kind: RecordKeyKind::Artifact,
        value: id.into(),
    }
}

fn event(id: &str, from: &str, to: &str, action: Action) -> Event {
    let identity = |value: &str| Identity {
        artifact_type: "work.item".into(),
        artifact_id: value.into(),
    };
    Event {
        event_id: id.into(),
        action,
        from: identity(from),
        to: identity(to),
    }
}

fn confirmed() -> Resolution {
    Resolution::from_events(&[event("one", "A", "B", Action::Confirm)]).unwrap()
}

fn stored(url: &str) -> StoredUrl {
    StoredUrl {
        url: url.into(),
        stripped_params: Some(vec!["utm_source".into()]),
    }
}

fn create(vault: &Vault, id: &str, body: &str) {
    save_keyed_note_with_provenance(vault, &stored(URL), &key(id), "Original", body, "missing")
        .unwrap();
}

fn location(vault: &Vault, id: &str, resolution: &Resolution) -> NoteLocation {
    NoteLocation::resolve(vault, URL, &key(id), resolution).unwrap()
}

#[test]
fn older_url_import_refuses_an_owned_alias_even_without_a_canonical_file() {
    let (_folder, vault) = super::super::tests::vault();
    create(&vault, "A", "original owner");
    let url_key = crate::vault::url_record_key(URL).unwrap();
    save_keyed_note(&vault, URL, &url_key, "URL note", "separate", "missing").unwrap();
    let source = load_older_note(&vault, URL).unwrap();
    let selected = location(&vault, "B", &confirmed());
    let original = vault.read_note(&key("A"), URL).unwrap().unwrap();
    assert!(matches!(
        selected.use_older_note(&vault, URL, URL, &source.revision),
        Err(NoteRequestError::Conflict(_))
    ));
    assert_eq!(vault.read_note(&key("A"), URL).unwrap().unwrap(), original);
    assert!(vault.read_note(&key("B"), URL).unwrap().is_none());
    assert_eq!(load_older_note(&vault, URL).unwrap().body, "separate");
}

#[test]
fn older_url_import_into_empty_group_binds_the_returned_edit_revision() {
    let (_folder, vault) = super::super::tests::vault();
    let url_key = crate::vault::url_record_key(URL).unwrap();
    save_keyed_note(&vault, URL, &url_key, "URL note", "import me", "missing").unwrap();
    let source = load_older_note(&vault, URL).unwrap();
    let selected = location(&vault, "A", &confirmed());
    let imported = selected
        .use_older_note(&vault, URL, URL, &source.revision)
        .unwrap();
    assert_eq!(imported.key, key("B"));
    assert!(imported.revision.starts_with("location-v1:"));
    assert_eq!(
        selected.load(&vault, URL, &[]).unwrap().revision,
        imported.revision
    );
    assert!(matches!(
        selected.save(
            &vault,
            &stored(URL),
            "Edited",
            "after import",
            &imported.revision
        ),
        Ok(SaveOutcome::Saved { .. })
    ));
    assert_eq!(
        selected.load(&vault, URL, &[]).unwrap().body,
        "after import"
    );
}

fn bytes(vault: &Vault, id: &str) -> String {
    vault.read_note(&key(id), URL).unwrap().unwrap()
}

#[test]
fn a_resolved_editor_updates_the_original_file_and_ownership_from_an_alternate_route() {
    let (_root, vault) = super::super::tests::vault();
    create(&vault, "A", "before");
    let resolution = confirmed();
    let selected = location(&vault, "A", &resolution);
    let loaded = selected.load(&vault, URL, &[]).unwrap();
    assert_eq!(loaded.key, key("B"));
    assert_eq!(loaded.body, "before");
    assert_ne!(
        loaded.revision,
        load_keyed_note(&vault, URL, &key("A"), &[])
            .unwrap()
            .revision
    );
    let mut alternate = stored("https://work.example/items/1/edit");
    alternate.stripped_params = Some(vec![]);
    let saved = selected
        .save(&vault, &alternate, "Different", "after", &loaded.revision)
        .unwrap();
    let SaveOutcome::Saved {
        revision,
        relative_path,
        ..
    } = saved
    else {
        panic!("not saved")
    };
    assert_eq!(
        relative_path,
        vault
            .note_relative_path(&key("A"), URL)
            .unwrap()
            .to_string_lossy()
    );
    assert_eq!(revision, selected.load(&vault, URL, &[]).unwrap().revision);
    assert!(revision.len() <= 128);
    let note = bytes(&vault, "A");
    assert!(note.contains("  artifact_id: \"A\""));
    assert!(!note.contains("  artifact_id: \"B\""));
    assert!(note.contains(&format!("  canonical_url: {URL:?}")));
    assert!(note.contains("  stripped_params: [\"utm_source\"]"));
    assert!(note.contains("title: \"Original\""));
    assert_eq!(
        load_keyed_note(&vault, URL, &key("A"), &[]).unwrap().body,
        "after"
    );
    assert!(vault.read_note(&key("B"), URL).unwrap().is_none());
}

#[test]
fn canonical_note_wins_and_never_overwrites_the_old_note() {
    let (_root, vault) = super::super::tests::vault();
    create(&vault, "A", "keep old");
    create(&vault, "B", "canonical");
    let old = bytes(&vault, "A");
    let selected = location(&vault, "A", &confirmed());
    let loaded = selected.load(&vault, URL, &[]).unwrap();
    assert_eq!(loaded.body, "canonical");
    selected
        .save(
            &vault,
            &stored(URL),
            "Unused",
            "new canonical",
            &loaded.revision,
        )
        .unwrap();
    assert_eq!(bytes(&vault, "A"), old);
    assert_eq!(
        load_keyed_note(&vault, URL, &key("B"), &[]).unwrap().body,
        "new canonical"
    );
}

#[test]
fn selecting_a_new_canonical_file_refuses_the_old_edit_even_when_its_body_is_identical() {
    let (root, vault) = super::super::tests::vault();
    create(&vault, "A", "same body");
    let resolution = confirmed();
    let loaded = location(&vault, "B", &resolution)
        .load(&vault, URL, &[])
        .unwrap();
    // A real second file takes over selection; timestamps, title and body can
    // all be equal. Its recorded owner and filename are different.
    let a = bytes(&vault, "A");
    let b = a.replace("  artifact_id: \"A\"", "  artifact_id: \"B\"");
    fs::write(
        root.path()
            .join(vault.note_relative_path(&key("B"), URL).unwrap()),
        &b,
    )
    .unwrap();
    let fresh = location(&vault, "B", &resolution);
    assert_eq!(fresh.key(), &loaded.key);
    let stale = fresh
        .save(
            &vault,
            &stored(URL),
            "Unused",
            "must not save",
            &loaded.revision,
        )
        .unwrap();
    let SaveOutcome::Stale {
        exists,
        body,
        revision,
        ..
    } = stale
    else {
        panic!("not stale")
    };
    assert!(exists);
    assert_eq!(body, "same body");
    assert_ne!(revision, loaded.revision);
    assert_eq!(bytes(&vault, "A"), a);
    assert_eq!(bytes(&vault, "B"), b);
}

#[test]
fn replay_history_prevents_an_old_revision_reviving_after_move_back_and_forward() {
    let (_root, vault) = super::super::tests::vault();
    create(&vault, "A", "keep");
    let first = event("one", "A", "B", Action::Confirm);
    let initial = Resolution::from_events(std::slice::from_ref(&first)).unwrap();
    let loaded = location(&vault, "A", &initial)
        .load(&vault, URL, &[])
        .unwrap();
    let back = event("two", "B", "A", Action::Confirm);
    let moved_back = Resolution::from_events(&[first.clone(), back.clone()]).unwrap();
    assert_eq!(location(&vault, "B", &moved_back).key(), &key("A"));
    let forward =
        Resolution::from_events(&[first, back, event("three", "A", "B", Action::Confirm)]).unwrap();
    let selected = location(&vault, "B", &forward);
    assert_eq!(selected.key(), &loaded.key);
    assert!(matches!(
        selected
            .save(&vault, &stored(URL), "T", "lost edit", &loaded.revision)
            .unwrap(),
        SaveOutcome::Stale { .. }
    ));
    assert_eq!(
        load_keyed_note(&vault, URL, &key("A"), &[]).unwrap().body,
        "keep"
    );
}

#[test]
fn unalias_separates_original_files_and_retains_a_history_bound_revision() {
    let (_root, vault) = super::super::tests::vault();
    create(&vault, "A", "old");
    create(&vault, "B", "new");
    let confirm = event("one", "A", "B", Action::Confirm);
    let active = Resolution::from_events(std::slice::from_ref(&confirm)).unwrap();
    let loaded = location(&vault, "A", &active)
        .load(&vault, URL, &[])
        .unwrap();
    let split =
        Resolution::from_events(&[confirm, event("two", "A", "B", Action::Unalias)]).unwrap();
    let a = location(&vault, "A", &split)
        .load(&vault, URL, &[])
        .unwrap();
    let b = location(&vault, "B", &split)
        .load(&vault, URL, &[])
        .unwrap();
    assert_eq!((a.key, a.body.as_str()), (key("A"), "old"));
    assert_eq!((b.key, b.body.as_str()), (key("B"), "new"));
    assert_ne!(loaded.revision, b.revision);
    assert!(b.revision.starts_with("location-v1:"));
    assert!(matches!(
        location(&vault, "B", &split)
            .save(&vault, &stored(URL), "T", "stale", &loaded.revision)
            .unwrap(),
        SaveOutcome::Stale { .. }
    ));
}

#[test]
fn multiple_old_notes_without_a_canonical_note_refuse_selection() {
    let (_root, vault) = super::super::tests::vault();
    create(&vault, "A", "first");
    create(&vault, "B", "second");
    let a = bytes(&vault, "A");
    let b = bytes(&vault, "B");
    let resolution = Resolution::from_events(&[
        event("one", "A", "B", Action::Confirm),
        event("two", "B", "C", Action::Confirm),
    ])
    .unwrap();
    assert!(matches!(
        NoteLocation::resolve(&vault, URL, &key("C"), &resolution),
        Err(NoteRequestError::Conflict(_))
    ));
    assert_eq!(bytes(&vault, "A"), a);
    assert_eq!(bytes(&vault, "B"), b);
    assert!(vault.read_note(&key("C"), URL).unwrap().is_none());
}

#[test]
fn an_empty_component_creates_at_its_canonical_path_only_with_the_bound_missing_revision() {
    let (_root, vault) = super::super::tests::vault();
    let selected = location(&vault, "A", &confirmed());
    let loaded = selected.load(&vault, URL, &[]).unwrap();
    assert!(!loaded.exists);
    assert_ne!(loaded.revision, "missing");
    assert!(matches!(
        selected
            .save(&vault, &stored(URL), "T", "wrong", "missing")
            .unwrap(),
        SaveOutcome::Stale { exists: false, .. }
    ));
    assert!(matches!(
        selected
            .save(&vault, &stored(URL), "T", "created", &loaded.revision)
            .unwrap(),
        SaveOutcome::Saved { .. }
    ));
    assert!(vault.read_note(&key("A"), URL).unwrap().is_none());
    assert_eq!(
        load_keyed_note(&vault, URL, &key("B"), &[]).unwrap().body,
        "created"
    );
}

#[test]
fn wrong_ownership_at_either_selected_path_refuses_instead_of_creating_a_replacement() {
    for id in ["A", "B"] {
        let (root, vault) = super::super::tests::vault();
        create(&vault, id, "do not overwrite");
        let path = root
            .path()
            .join(vault.note_relative_path(&key(id), URL).unwrap());
        let invalid = bytes(&vault, id).replace(
            &format!("  artifact_id: {id:?}"),
            "  artifact_id: \"other\"",
        );
        fs::write(&path, &invalid).unwrap();
        assert!(matches!(
            NoteLocation::resolve(&vault, URL, &key("B"), &confirmed()),
            Err(NoteRequestError::Conflict(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    }
}

#[test]
fn a_bad_optional_alias_does_not_hide_the_owned_canonical_note() {
    let (root, vault) = super::super::tests::vault();
    create(&vault, "B", "primary");
    fs::write(
        root.path()
            .join(vault.note_relative_path(&key("A"), URL).unwrap()),
        "unowned",
    )
    .unwrap();
    assert_eq!(
        location(&vault, "B", &confirmed())
            .load(&vault, URL, &[])
            .unwrap()
            .body,
        "primary"
    );
}

#[cfg(unix)]
#[test]
fn alias_selection_never_follows_symlinks_at_either_note_path() {
    for id in ["A", "B"] {
        let (root, vault) = super::super::tests::vault();
        create(&vault, "A", "keep");
        let owned = bytes(&vault, "A");
        let path = root
            .path()
            .join(vault.note_relative_path(&key(id), URL).unwrap());
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("outside.md");
        fs::write(&target, &owned).unwrap();
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(NoteLocation::resolve(&vault, URL, &key("B"), &confirmed()).is_err());
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(target).unwrap(), owned);
    }
}

#[test]
fn url_copies_never_supply_an_artifact_editor_and_direct_notes_keep_their_revisions() {
    let (_root, vault) = super::super::tests::vault();
    let url_key = crate::vault::url_record_key(URL).unwrap();
    save_note(&vault, URL, "URL note", "separate", "missing").unwrap();
    let url_location = NoteLocation::resolve(&vault, URL, &url_key, &confirmed()).unwrap();
    let direct = load_note(&vault, URL).unwrap();
    let selected = url_location.load(&vault, URL, &[]).unwrap();
    assert_eq!(selected.key, url_key);
    assert_eq!(selected.revision, direct.revision);
    let artifact = location(&vault, "A", &confirmed())
        .load(&vault, URL, &[URL.into()])
        .unwrap();
    assert!(!artifact.exists);
    assert_eq!(artifact.older_notes.len(), 1);
    assert_eq!(artifact.older_notes[0].key, url_key);
    assert_eq!(load_note(&vault, URL).unwrap().body, "separate");
    let unchanged = location(&vault, "untouched", &confirmed())
        .load(&vault, URL, &[])
        .unwrap();
    assert_eq!(unchanged.revision, "missing");
}

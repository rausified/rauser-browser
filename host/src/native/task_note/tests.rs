use super::*;
use std::path::PathBuf;

const URL: &str = "https://work.example/items/7";
const ID: &str = "00000000-0000-4000-8000-000000000071";

fn run() -> RunView {
    serde_json::from_value(serde_json::json!({
        "run_id": ID, "kind": "task", "state": "succeeded", "harness": "codex",
        "record_key": { "kind": "url", "value": "https://wrong.example/ignored" },
        "written_record_key": null, "url": URL, "title": "Work page", "cwd": null,
        "created_at": "2026-11-03T02:00:00Z", "started_at": "2026-11-03T02:00:00Z",
        "finished_at": "2026-11-03T02:01:00Z", "error": null, "error_kind": null,
        "thread_id": null, "session_id": null, "summary_path": null,
        "stream_bytes": 0, "can_retry_save": false, "answer": null,
        "instruction": "Find open questions", "task_report": "Three open questions."
    }))
    .unwrap()
}

fn world() -> (tempfile::TempDir, ConfigStore, PathBuf) {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("home");
    std::fs::create_dir(&root).unwrap();
    let path = folder.path().join("config.toml");
    config::fixtures::write(&path, &root, "", "capture_enabled = false", "");
    let mut config = ConfigStore::for_test(path);
    config.refresh().unwrap();
    (folder, config, root)
}

fn request(config: &ConfigStore) -> SaveTaskNoteRequest {
    SaveTaskNoteRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: "save-report".into(),
        run_id: ID.into(),
        expected_revision: config.revision().into(),
    }
}

fn append(config: &mut ConfigStore, run: RunView) -> Response {
    let mut value = request(config);
    value.run_id = run.run_id.clone();
    save_with(value, config, |request| {
        let Request::GetRun(get) = request else {
            panic!("must fetch the retained run")
        };
        assert_eq!(get.run_id, run.run_id);
        assert_eq!(get.request_id, "save-report");
        Response::RunFetched(brauser_protocol::RunFetched {
            protocol_version: PROTOCOL_VERSION,
            request_id: get.request_id,
            run,
            stream_text: "ignored agent stream".into(),
            next_offset: 20,
            more: false,
        })
    })
}

fn load(config: &mut ConfigStore) -> NoteLoaded {
    let Response::NoteLoaded(loaded) = load_note(
        brauser_protocol::LoadNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "load".into(),
            url: URL.into(),
        },
        config,
    ) else {
        panic!("load note")
    };
    loaded
}

fn write(config: &mut ConfigStore, body: &str) -> NoteSaved {
    let loaded = load(config);
    let Response::NoteSaved(saved) = save_note(
        brauser_protocol::SaveNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "edit".into(),
            url: URL.into(),
            title: "My existing title".into(),
            body: body.into(),
            expected_key: loaded.key,
            expected_revision: loaded.revision,
        },
        config,
    ) else {
        panic!("write note")
    };
    saved
}

fn assert_saved(response: Response, already_saved: bool) {
    let Response::TaskNoteSaved(saved) = response else {
        panic!("not saved: {response:?}")
    };
    assert_eq!(saved.already_saved, already_saved);
    assert_eq!(saved.request_id, "save-report");
}

fn assert_error(response: Response, expected: ErrorCode) {
    let Response::Error(err) = response else {
        panic!("not refused: {response:?}")
    };
    assert_eq!(err.code, expected, "{}", err.message);
}

#[test]
fn an_explicit_append_creates_the_host_resolved_note_and_no_other_content() {
    let (folder, mut config, root) = world();
    let before_config = std::fs::read(folder.path().join("config.toml")).unwrap();
    assert!(!root.join("notes").exists());
    assert_saved(append(&mut config, run()), false);
    let note = load(&mut config);
    assert_eq!(note.key, crate::vault::url_record_key(URL).unwrap());
    assert_eq!(note.body, report_block(&run()).unwrap());
    assert_eq!(note.title, "Work page");
    assert!(!note.body.contains("ignored agent stream"));
    for dir in ["logs", "summaries", "later", "artifacts"] {
        assert!(!root.join(dir).exists());
    }
    assert_eq!(
        std::fs::read(folder.path().join("config.toml")).unwrap(),
        before_config
    );
}

#[test]
fn saving_keeps_the_entire_longest_accepted_instruction() {
    let (_folder, mut config, _root) = world();
    let mut task = run();
    let limit = crate::service::record::MAX_TASK_INSTRUCTION_CHARS;
    let ending = " Keep this final requirement.";
    let instruction = format!("{}{ending}", "界".repeat(limit - ending.chars().count()));
    assert_eq!(instruction.chars().count(), limit);
    crate::service::record::check_task_instruction(&instruction).unwrap();
    task.instruction = Some(instruction.clone());
    assert_saved(append(&mut config, task), false);
    assert!(load(&mut config).body.contains(&format!(
        "**Task:** {}\n\n**Finished:**",
        crate::markdown::inline(&instruction, limit),
    )));
}

#[test]
fn current_artifact_identity_and_stripped_url_provenance_come_from_the_host() {
    let (folder, mut config, root) = world();
    config::fixtures::write(
        &folder.path().join("config.toml"),
        &root,
        "",
        "capture_enabled = false\nstrip_params = [\"token\"]",
        r#"
[adapters]
format = 1
[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
    );
    config.refresh().unwrap();
    let original = write(&mut config, "Artifact note");
    let mut task = run();
    task.url = format!("{URL}?token=removed-value");
    assert_saved(append(&mut config, task), false);
    let note = load(&mut config);
    assert_eq!(note.key.kind, RecordKeyKind::Artifact);
    assert_eq!(note.key.value, "work:work.example/7");
    assert!(note.body.starts_with("Artifact note\n\n## Task report"));
    let bytes = std::fs::read_to_string(root.join(original.relative_path)).unwrap();
    assert!(!bytes.contains("removed-value"));
    assert!(!bytes.contains("wrong.example"));
    assert_eq!(std::fs::read_dir(root.join("notes")).unwrap().count(), 1);
}

#[test]
fn append_keeps_existing_body_title_and_stale_editor_edits_get_a_conflict() {
    let (_folder, mut config, root) = world();
    let original = "My work\n\nKeep the trailing spaces.  ";
    let saved = write(&mut config, original);
    let stale = load(&mut config);
    let before = std::fs::read_to_string(root.join(&saved.relative_path)).unwrap();
    assert_saved(append(&mut config, run()), false);
    let note = load(&mut config);
    assert_eq!(
        note.body,
        format!("{original}\n\n{}", report_block(&run()).unwrap())
    );
    assert_eq!(note.title, "My existing title");
    let after = std::fs::read_to_string(root.join(saved.relative_path)).unwrap();
    let created = before
        .lines()
        .find(|line| line.starts_with("  created:"))
        .unwrap();
    assert!(after.lines().any(|line| line == created));
    let conflict = save_note(
        brauser_protocol::SaveNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "stale-editor".into(),
            url: URL.into(),
            title: stale.title,
            body: "Unsaved panel draft".into(),
            expected_key: stale.key,
            expected_revision: stale.revision,
        },
        &mut config,
    );
    let Response::NoteConflict(conflict) = conflict else {
        panic!("must preserve newer note")
    };
    assert_eq!(conflict.reason, "stale");
    assert_eq!(conflict.body, note.body);
    assert_eq!(load(&mut config).body, note.body);
}

#[test]
fn uncertain_reply_retry_keeps_later_edits_and_does_not_duplicate_the_report() {
    let (_folder, mut config, root) = world();
    // A quoted run marker alone is not a saved report.
    write(
        &mut config,
        &format!("<!-- {}:task-report {ID} -->", crate::brand::NAMESPACE),
    );
    assert_saved(append(&mut config, run()), false);
    let note = load(&mut config);
    let saved = write(&mut config, &format!("{}\nLater user text.", note.body));
    let before = std::fs::read(root.join(&saved.relative_path)).unwrap();
    assert_saved(append(&mut config, run()), true);
    assert_eq!(
        std::fs::read(root.join(saved.relative_path)).unwrap(),
        before
    );
    assert_eq!(
        load(&mut config)
            .body
            .matches("Three open questions.")
            .count(),
        1
    );
}

#[test]
fn non_page_unfinished_empty_wrong_and_missing_runs_write_nothing() {
    let (_folder, mut config, root) = world();
    let mut invalid = Vec::new();
    for state in [
        RunState::Queued,
        RunState::Running,
        RunState::Failed,
        RunState::Canceled,
    ] {
        let mut candidate = run();
        candidate.state = state;
        invalid.push(candidate);
    }
    for kind in [None, Some(RunKind::Summary), Some(RunKind::SiteDraft)] {
        let mut candidate = run();
        candidate.kind = kind;
        invalid.push(candidate);
    }
    let mut no_page = run();
    no_page.no_page = Some(true);
    invalid.push(no_page);
    for url in ["brauser:task-without-page", "file:///tmp/data", "broken"] {
        let mut candidate = run();
        candidate.url = url.into();
        invalid.push(candidate);
    }
    for report in [None, Some("".into()), Some(" \n".into())] {
        let mut candidate = run();
        candidate.task_report = report;
        invalid.push(candidate);
    }
    for candidate in invalid {
        assert_error(append(&mut config, candidate), ErrorCode::InvalidRequest);
    }
    let value = request(&config);
    assert_error(
        save_with(value.clone(), &mut config, |_| {
            error("save-report", ErrorCode::InvalidRequest, "unknown run")
        }),
        ErrorCode::InvalidRequest,
    );
    let mut wrong = run();
    wrong.run_id = "00000000-0000-4000-8000-000000000099".into();
    assert_error(
        save_with(value, &mut config, |_| {
            Response::RunFetched(brauser_protocol::RunFetched {
                protocol_version: PROTOCOL_VERSION,
                request_id: "save-report".into(),
                run: wrong,
                stream_text: String::new(),
                next_offset: 0,
                more: false,
            })
        }),
        ErrorCode::InvalidRequest,
    );
    let mut invalid_id = request(&config);
    invalid_id.run_id = "not-a-run".into();
    assert_error(
        save_with(invalid_id, &mut config, |_| {
            panic!("invalid id must not contact service")
        }),
        ErrorCode::InvalidRequest,
    );
    assert!(!root.join("notes").exists());
}

#[test]
fn settings_and_home_changes_during_the_fetch_refuse_before_any_note_write() {
    let (folder, mut config, root) = world();
    let value = request(&config);
    let other = folder.path().join("other-home");
    std::fs::create_dir(&other).unwrap();
    let response = save_with(value, &mut config, |_| {
        config::fixtures::write(
            &folder.path().join("config.toml"),
            &other,
            "",
            "capture_enabled = false",
            "",
        );
        Response::RunFetched(brauser_protocol::RunFetched {
            protocol_version: PROTOCOL_VERSION,
            request_id: "save-report".into(),
            run: run(),
            stream_text: String::new(),
            next_offset: 0,
            more: false,
        })
    });
    assert_error(response, ErrorCode::Conflict);
    assert!(!root.join("notes").exists());
    assert!(!other.join("notes").exists());
}

#[test]
fn unowned_and_symlinked_notes_are_never_replaced() {
    let (folder, mut config, root) = world();
    let saved = write(&mut config, "original");
    let path = root.join(saved.relative_path);
    std::fs::write(&path, "An unrelated user's file").unwrap();
    assert_error(append(&mut config, run()), ErrorCode::Conflict);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "An unrelated user's file"
    );
    let outside = folder.path().join("outside.md");
    std::fs::write(&outside, "Keep outside").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert!(matches!(append(&mut config, run()), Response::Error(_)));
    assert!(path.is_symlink());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "Keep outside");
}

#[test]
fn an_append_over_the_note_limit_keeps_the_original_bytes() {
    let (_folder, mut config, root) = world();
    let saved = write(&mut config, &"x".repeat(98_300));
    let path = root.join(saved.relative_path);
    let before = std::fs::read(&path).unwrap();
    assert_error(append(&mut config, run()), ErrorCode::MessageTooLarge);
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn simultaneous_reports_both_append_to_the_latest_note() {
    let (folder, mut config, _root) = world();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut second = run();
    second.run_id = "00000000-0000-4000-8000-000000000072".into();
    let handles: Vec<_> = [run(), second]
        .into_iter()
        .map(|run| {
            let path = folder.path().join("config.toml");
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut config = ConfigStore::for_test(path);
                config.refresh().unwrap();
                barrier.wait();
                assert_saved(append(&mut config, run), false);
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let note = load(&mut config);
    assert_eq!(note.body.matches("## Task report").count(), 2);
    assert_eq!(note.body.matches("Three open questions.").count(), 2);
}

#[test]
fn report_and_instruction_use_the_shared_markdown_safety_helpers() {
    let mut run = run();
    let hostile = "<img src='https://evil.example'>\n![image](https://evil.example)\n![[embed]]\n```dataviewjs\nfetch('bad')\n```\n`=query`\n$$math$$\n    indented\n<!-- marker -->";
    run.task_report = Some(hostile.into());
    run.instruction = Some(hostile.into());
    let block = report_block(&run).unwrap();
    assert!(block.contains(&crate::markdown::neutralize_note_append(hostile)));
    assert!(block.contains(&crate::markdown::inline(
        hostile,
        crate::service::record::MAX_TASK_INSTRUCTION_CHARS,
    )));
    let content = block.split("<!--").next().unwrap();
    for active in ['`', '$'] {
        assert!(!content.contains(active));
    }
    assert!(content.contains("\\!&#91;&#91;"));
    assert!(content.contains("\\!&#91;image&#93;"));
    assert!(content.contains("&lt;img"));

    // The existing real-renderer checker accepts these actual rendered
    // report sections too. Only the trusted UUID comment is omitted, since
    // that checker deliberately treats every HTML comment as foreign markup.
    let corpus: Vec<String> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/markdown-neutralize-corpus.json"
    ))
    .unwrap();
    let pairs: Vec<_> = corpus
        .iter()
        .map(|input| {
            run.instruction = Some(input.clone());
            run.task_report = Some(input.clone());
            let block = report_block(&run).unwrap();
            let section = block.rsplit_once("\n\n<!--").unwrap().0;
            assert!(!section.contains('`'));
            assert!(!section.contains('$'));
            serde_json::json!({ "input": input, "summaryTaskNote": format!("![Earlier image][r]\n\nEarlier [link][r]\n\n{section}") })
        })
        .collect();
    if let Some(path) = std::env::var_os("BRAUSER_TASK_NOTE_CORPUS_OUT") {
        std::fs::write(path, serde_json::to_vec(&pairs).unwrap()).unwrap();
    }
}

#[test]
fn an_appended_report_cannot_define_an_earlier_notes_image_or_link() {
    let (_folder, mut config, _root) = world();
    let original = "![Earlier image][r]\n\nEarlier [link][r]";
    write(&mut config, original);
    let mut task = run();
    task.task_report = Some("[r]: https://evil.example/image.png\n\n\\[r] and \\\\[r]".into());
    assert_saved(append(&mut config, task), false);
    let note = load(&mut config);
    assert!(note.body.starts_with(original));
    assert!(!note.body.contains("[r]:"));
    assert!(
        note.body
            .contains("&#91;r&#93;: https://evil.example/image.png")
    );
    // One escaping slash is consumed; a literal slash pair still displays
    // as one slash, just as the shared Markdown helpers promise.
    assert!(note.body.contains("\n\n&#91;r&#93; and \\\\&#91;r&#93;"));
}

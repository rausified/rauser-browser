//! The task bar's wire additions: `start_task`, the `task` run kind, the
//! host-to-service `instruction` on `start_run`, and the task fields on a run
//! view (all additive within protocol version 8).

use brauser_protocol::{Request, Response, RunKind};
use serde_json::{Value, json};

mod common;
use common::{assert_request_round_trips, assert_response_round_trips, schema_subset};

#[test]
fn saving_a_task_report_supplies_only_the_run_and_config_revision() {
    let request = json!({
        "type": "save_task_note", "protocol_version": 8, "request_id": "save",
        "run_id": "00000000-0000-4000-8000-000000000071", "expected_revision": "config-rev",
    });
    assert_request_round_trips("SaveTaskNoteRequest", &request);
    for field in ["url", "body", "report", "record_key", "instruction"] {
        let mut forged = request.clone();
        forged[field] = json!("untrusted");
        assert!(
            serde_json::from_value::<Request>(forged.clone()).is_err(),
            "{field}"
        );
        assert!(!schema_subset::valid("SaveTaskNoteRequest", &forged));
    }
    for already_saved in [false, true] {
        assert_response_round_trips(
            "TaskNoteSaved",
            &json!({
                "type": "task_note_saved", "protocol_version": 8, "request_id": "save",
                "run_id": request["run_id"], "already_saved": already_saved,
            }),
        );
    }
}

fn start_task() -> Value {
    json!({
        "type": "start_task", "protocol_version": 8, "request_id": "r",
        "url": "https://a.example/x", "title": "T", "content": "body",
        "selection": null, "instruction": "Summarize this for my team.",
        "harness": null, "requested_at": "2026-10-28T14:00:00-07:00",
    })
}

fn start_run() -> Value {
    json!({
        "type": "start_run", "protocol_version": 8, "request_id": "r",
        "url": "https://a.example/x", "title": "T", "content": "body",
        "selection": null, "harness": null, "record_key": null,
        "requested_at": "2026-10-28T14:00:00-07:00",
    })
}

fn run_view() -> Value {
    json!({
        "run_id": "00000000-0000-4000-8000-000000000001", "state": "running",
        "harness": "codex", "record_key": { "kind": "url", "value": "https://a.example/x" },
        "written_record_key": null, "url": "https://a.example/x", "title": "T",
        "cwd": null, "created_at": "2026-10-28T14:00:00-07:00", "started_at": null,
        "finished_at": null, "error": null, "error_kind": null, "thread_id": null,
        "session_id": null, "summary_path": null, "stream_bytes": 0,
        "can_retry_save": false, "answer": null,
    })
}

fn run_fetched(view: Value) -> Value {
    json!({
        "type": "run_fetched", "protocol_version": 8, "request_id": "r",
        "run": view, "stream_text": "x", "next_offset": 1, "more": false,
    })
}

fn task_view() -> Value {
    let mut view = run_view();
    view["state"] = json!("succeeded");
    view["kind"] = json!("task");
    view["instruction"] = json!("Summarize this\nfor my team.");
    view["task_report"] = json!("- Wrote notes/team.md");
    view
}

#[test]
fn task_start_request_correlation_round_trips_in_list_and_fetch_views() {
    let mut view = task_view();
    view["start_request_id"] = json!("00000000-0000-4000-8000-000000000071");
    assert_response_round_trips("RunFetched", &run_fetched(view.clone()));
    assert_response_round_trips(
        "RunsListed",
        &json!({
            "type": "runs_listed", "protocol_version": 8, "request_id": "list",
            "runs": [view], "more": false,
        }),
    );
    for invalid in [json!(null), json!(42), json!(""), json!("x".repeat(129))] {
        let mut bad = task_view();
        bad["start_request_id"] = invalid;
        assert!(!schema_subset::valid("RunView", &bad));
    }
}

#[test]
fn tasks_without_a_page_round_trip_without_changing_legacy_tasks() {
    let mut value = start_task();
    for key in ["url", "title", "content"] {
        value.as_object_mut().unwrap().remove(key);
    }
    value["no_page"] = json!(true);
    let Request::StartTask(task) = assert_request_round_trips("StartTaskRequest", &value) else {
        panic!("expected start_task");
    };
    assert_eq!(task.no_page, Some(true));
    assert_eq!(task.url, None);
    assert_eq!(task.title, None);
    assert_eq!(task.content, None);
    assert_eq!(task.selection, None);
    let mut forwarded = start_run();
    forwarded["no_page"] = json!(true);
    forwarded["kind"] = json!("task");
    forwarded["instruction"] = json!("Organize notes");
    assert_request_round_trips("StartRunRequest", &forwarded);
    let mut view = task_view();
    view["no_page"] = json!(true);
    assert_response_round_trips("RunFetched", &run_fetched(view));
    value["no_page"] = json!("true");
    assert!(serde_json::from_value::<Request>(value.clone()).is_err());
    assert!(!schema_subset::valid("StartTaskRequest", &value));
}

#[test]
fn capture_truncation_is_an_optional_boolean_on_start_task() {
    let mut value = start_task();
    let Request::StartTask(legacy) = assert_request_round_trips("StartTaskRequest", &value) else {
        panic!("expected start_task");
    };
    assert_eq!(legacy.capture_truncated, None);
    for flag in [true, false] {
        value["capture_truncated"] = json!(flag);
        let Request::StartTask(request) = assert_request_round_trips("StartTaskRequest", &value)
        else {
            panic!("expected start_task");
        };
        assert_eq!(request.capture_truncated, Some(flag));
    }
    value["capture_truncated"] = json!(1);
    assert!(serde_json::from_value::<Request>(value).is_err());
}

#[test]
fn start_task_round_trips_and_is_schema_valid() {
    let parsed = assert_request_round_trips("StartTaskRequest", &start_task());
    let Request::StartTask(value) = &parsed else {
        panic!("not start_task: {parsed:?}");
    };
    assert_eq!(value.instruction, "Summarize this for my team.");
    assert_eq!(value.harness, None);
    assert_eq!(value.selection, None);
    assert_eq!(parsed.request_id(), "r");
    assert_eq!(parsed.protocol_version(), 8);

    let mut chosen = start_task();
    chosen["selection"] = json!("quote");
    chosen["harness"] = json!("claude");
    assert_request_round_trips("StartTaskRequest", &chosen);
}

#[test]
fn start_task_instruction_bounds() {
    for (instruction, valid) in [
        (String::new(), false),
        ("x".repeat(4096), true),
        ("x".repeat(4097), false),
        // The bound is characters, not bytes.
        ("\u{1F600}".repeat(4096), true),
    ] {
        let mut request = start_task();
        request["instruction"] = json!(instruction);
        assert_eq!(
            schema_subset::valid("StartTaskRequest", &request),
            valid,
            "{} chars",
            instruction.chars().count()
        );
    }
}

#[test]
fn start_task_without_instruction_or_with_kind_or_record_key_does_not_parse() {
    let mut missing = start_task();
    missing.as_object_mut().unwrap().remove("instruction");
    assert!(serde_json::from_value::<Request>(missing.clone()).is_err());
    assert!(!schema_subset::valid("StartTaskRequest", &missing));

    let mut null = start_task();
    null["instruction"] = Value::Null;
    assert!(serde_json::from_value::<Request>(null.clone()).is_err());
    assert!(!schema_subset::valid("StartTaskRequest", &null));

    // The kind and the record key are host-chosen: the extension cannot send them.
    for (key, value) in [
        ("kind", json!("task")),
        ("kind", json!("summary")),
        ("record_key", Value::Null),
        (
            "record_key",
            json!({ "kind": "url", "value": "https://a.example/x" }),
        ),
        ("cwd", json!("/tmp")),
    ] {
        let mut extra = start_task();
        extra[key] = value;
        assert!(
            serde_json::from_value::<Request>(extra.clone()).is_err(),
            "{key}"
        );
        assert!(!schema_subset::valid("StartTaskRequest", &extra), "{key}");
    }

    // Page completeness is a native semantic check now that no_page permits
    // these three fields to be omitted together.
    for key in ["selection", "harness", "requested_at"] {
        let mut without = start_task();
        without.as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<Request>(without).is_err(), "{key}");
    }
}

#[test]
fn start_run_instruction_is_optional_and_bounded() {
    let parsed = assert_request_round_trips("StartRunRequest", &start_run());
    let Request::StartRun(value) = parsed else {
        panic!("not start_run");
    };
    assert_eq!(value.instruction, None);
    // None is never written, so an older service never sees the key.
    let encoded = serde_json::to_value(Request::StartRun(value)).unwrap();
    assert!(encoded.get("instruction").is_none());

    let mut task = start_run();
    task["kind"] = json!("task");
    task["instruction"] = json!("Do the thing.");
    task["record_key"] = json!({ "kind": "url", "value": "https://a.example/x" });
    let parsed = assert_request_round_trips("StartRunRequest", &task);
    let Request::StartRun(value) = parsed else {
        panic!("not start_run");
    };
    assert_eq!(value.kind, Some(RunKind::Task));
    assert_eq!(value.instruction.as_deref(), Some("Do the thing."));

    for (instruction, valid) in [
        (String::new(), false),
        ("x".repeat(4096), true),
        ("x".repeat(4097), false),
    ] {
        let mut request = task.clone();
        request["instruction"] = json!(instruction);
        assert_eq!(
            schema_subset::valid("StartRunRequest", &request),
            valid,
            "{} chars",
            instruction.chars().count()
        );
    }
}

#[test]
fn run_view_with_task_kind_instruction_and_report_round_trips() {
    let parsed = assert_response_round_trips("RunFetched", &run_fetched(task_view()));
    let Response::RunFetched(fetched) = parsed else {
        panic!("not run_fetched");
    };
    assert_eq!(fetched.run.kind, Some(RunKind::Task));
    assert_eq!(
        fetched.run.instruction.as_deref(),
        Some("Summarize this\nfor my team.")
    );
    assert_eq!(
        fetched.run.task_report.as_deref(),
        Some("- Wrote notes/team.md")
    );
    assert_eq!(fetched.run.site_draft, None);
    assert_eq!(fetched.run.answer, None);

    // Before success there is no report: missing or null, both read as None.
    let mut running = task_view();
    running["state"] = json!("running");
    running.as_object_mut().unwrap().remove("task_report");
    let parsed = assert_response_round_trips("RunFetched", &run_fetched(running.clone()));
    let Response::RunFetched(fetched) = parsed else {
        panic!("not run_fetched");
    };
    assert_eq!(fetched.run.task_report, None);
    running["task_report"] = Value::Null;
    assert!(schema_subset::valid("RunView", &running));
    let parsed: Response = serde_json::from_value(run_fetched(running)).unwrap();
    let Response::RunFetched(fetched) = parsed else {
        panic!("not run_fetched");
    };
    assert_eq!(fetched.run.task_report, None);

    // A list carries task views too.
    let listed = json!({
        "type": "runs_listed", "protocol_version": 8, "request_id": "r",
        "runs": [run_view(), task_view()],
    });
    assert_response_round_trips("RunsListed", &listed);
}

#[test]
fn run_view_task_report_is_bounded() {
    for (report, valid) in [
        (String::new(), true),
        ("x".repeat(16384), true),
        ("x".repeat(16385), false),
    ] {
        let mut view = task_view();
        view["task_report"] = json!(report);
        assert_eq!(
            schema_subset::valid("RunView", &view),
            valid,
            "{} chars",
            report.chars().count()
        );
    }
    for (instruction, valid) in [("x".repeat(4096), true), ("x".repeat(4097), false)] {
        let mut view = task_view();
        view["instruction"] = json!(instruction);
        assert_eq!(schema_subset::valid("RunView", &view), valid);
    }
    let mut wrong = task_view();
    wrong["task_report"] = json!({ "report": "x" });
    assert!(!schema_subset::valid("RunView", &wrong));
    assert!(serde_json::from_value::<Response>(run_fetched(wrong)).is_err());
}

#[test]
fn run_view_without_task_fields_still_parses() {
    let parsed = assert_response_round_trips("RunFetched", &run_fetched(run_view()));
    let Response::RunFetched(fetched) = parsed else {
        panic!("not run_fetched");
    };
    assert_eq!(fetched.run.instruction, None);
    assert_eq!(fetched.run.task_report, None);
    let encoded = serde_json::to_value(&fetched.run).unwrap();
    assert!(encoded.get("instruction").is_none());
    assert!(encoded.get("task_report").is_none());
}

#[test]
fn run_kind_accepts_task() {
    assert_eq!(
        serde_json::from_value::<RunKind>(json!("task")).unwrap(),
        RunKind::Task
    );
    assert_eq!(serde_json::to_value(RunKind::Task).unwrap(), json!("task"));
    assert!(schema_subset::valid("RunKind", &json!("task")));
    assert!(!schema_subset::valid("RunKind", &json!("chat")));
    assert!(serde_json::from_value::<RunKind>(json!("chat")).is_err());
}

#[test]
fn task_command_catalogues_expose_descriptors_and_revision_without_prompts() {
    assert_request_round_trips(
        "ListTaskCommandsRequest",
        &json!({
            "type":"list_task_commands", "protocol_version":8, "request_id":"commands",
        }),
    );
    let catalogue = json!({
        "type":"task_commands_listed", "protocol_version":8, "request_id":"commands",
        "revision":"current", "commands":[{"name":"actions", "description":"Extract actions"}], "issue":null,
    });
    assert_response_round_trips("TaskCommandsListed", &catalogue);
    let mut forged = catalogue.clone();
    forged["commands"][0]["prompt"] = json!("caller instruction");
    assert!(serde_json::from_value::<Response>(forged.clone()).is_err());
    assert!(!schema_subset::valid("TaskCommandsListed", &forged));
    let mut invocation = start_task();
    invocation["instruction"] = json!("/actions Focus on today");
    invocation["command_revision"] = json!("current");
    assert_request_round_trips("StartTaskRequest", &invocation);
    invocation["command_revision"] = json!(123);
    assert!(serde_json::from_value::<Request>(invocation.clone()).is_err());
    assert!(!schema_subset::valid("StartTaskRequest", &invocation));
}

#[test]
fn task_command_settings_are_revision_bound_and_carry_only_the_registry() {
    assert_request_round_trips(
        "GetTaskCommandSettingsRequest",
        &json!({
            "type":"get_task_command_settings", "protocol_version":8,
            "request_id":"settings", "expected_revision":"current",
        }),
    );
    let save = json!({
        "type":"save_task_commands", "protocol_version":8,
        "request_id":"settings", "expected_revision":"current", "commands_toml":"commands = []",
    });
    assert_request_round_trips("SaveTaskCommandsRequest", &save);
    for (key, value) in [("config", json!({})), ("cwd", json!("/tmp"))] {
        let mut forged = save.clone();
        forged[key] = value;
        assert!(serde_json::from_value::<Request>(forged.clone()).is_err());
        assert!(!schema_subset::valid("SaveTaskCommandsRequest", &forged));
    }
    let mut missing = save.clone();
    missing.as_object_mut().unwrap().remove("expected_revision");
    assert!(serde_json::from_value::<Request>(missing.clone()).is_err());
    assert!(!schema_subset::valid("SaveTaskCommandsRequest", &missing));
    let mut oversized = save;
    oversized["commands_toml"] = json!("x".repeat(65537));
    assert!(!schema_subset::valid("SaveTaskCommandsRequest", &oversized));
    assert_response_round_trips(
        "TaskCommandSettings",
        &json!({
            "type":"task_command_settings", "protocol_version":8, "request_id":"settings",
            "revision":"current", "original_toml":"commands = []", "issue":null,
            "commands":[{"name":"actions", "description":"Extract actions", "prompt":"Return a checklist."}],
        }),
    );
}

#[test]
fn task_folders_use_ids_and_picker_tokens_never_client_paths() {
    let id = "00000000-0000-4000-8000-000000000001";
    let mut task = start_task();
    task["task_folder_id"] = json!(id);
    assert_request_round_trips("StartTaskRequest", &task);
    task["cwd"] = json!("/arbitrary");
    assert!(serde_json::from_value::<Request>(task.clone()).is_err());
    assert!(!schema_subset::valid("StartTaskRequest", &task));
    for (name, value) in [
        (
            "ListTaskFoldersRequest",
            json!({"type":"list_task_folders","protocol_version":8,"request_id":"r"}),
        ),
        (
            "AddTaskFolderRequest",
            json!({"type":"add_task_folder","protocol_version":8,"request_id":"r","expected_revision":"rev","picker_token":"picked"}),
        ),
        (
            "RemoveTaskFolderRequest",
            json!({"type":"remove_task_folder","protocol_version":8,"request_id":"r","expected_revision":"rev","folder_id":id}),
        ),
    ] {
        assert_request_round_trips(name, &value);
        let mut forged = value;
        forged["path"] = json!("/arbitrary");
        assert!(serde_json::from_value::<Request>(forged.clone()).is_err());
        assert!(!schema_subset::valid(name, &forged));
    }
    assert_response_round_trips(
        "TaskFoldersListed",
        &json!({"type":"task_folders_listed","protocol_version":8,"request_id":"r","revision":"rev","folders":[{"id":id,"path":"/project"}]}),
    );
    let mut view = task_view();
    view["task_folder_id"] = json!(id);
    assert_response_round_trips("RunFetched", &run_fetched(view));
}

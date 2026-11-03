//! A user's explicit copy of a retained task report into its source-page note.
//! Finishing a task still writes no content. The service supplies the report;
//! ordinary note ownership, identity and revision checks guard the append.

use crate::url_provenance::StoredUrl;
use brauser_protocol::{
    GetRunRequest, RunKind, RunState, RunView, SaveTaskNoteRequest, TaskNoteSaved,
};

use super::*;

pub(super) fn save(
    value: SaveTaskNoteRequest,
    config: &mut ConfigStore,
    service: Option<&ServiceLink>,
) -> Response {
    save_with(value, config, |request| forward(&request, service))
}

fn save_with(
    value: SaveTaskNoteRequest,
    config: &mut ConfigStore,
    fetch: impl FnOnce(Request) -> Response,
) -> Response {
    if uuid::Uuid::parse_str(&value.run_id).is_err() {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "invalid task run id",
        );
    }
    // Do not hold a config lock across service I/O: the service uses it too.
    let response = fetch(Request::GetRun(GetRunRequest {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id.clone(),
        run_id: value.run_id.clone(),
        since_offset: 0,
    }));
    let run = match response {
        Response::RunFetched(fetched) => fetched.run,
        response @ Response::Error(_) => return response,
        _ => {
            return error(
                &value.request_id,
                ErrorCode::ServiceUnavailable,
                "The service did not return the task report.",
            );
        }
    };
    let Some(block) = report_block(&run).filter(|_| run.run_id == value.run_id) else {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "Only a successful task with a source page and a report can be saved to a note.",
        );
    };
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(lock_error) => {
            warn_cause(&lock_error, "config lock failed");
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "configuration is unavailable",
            );
        }
    };
    if config.refresh().is_err() || config.config_issue().is_some() {
        return error(
            &value.request_id,
            ErrorCode::InvalidConfig,
            "configuration is unavailable or needs repair",
        );
    }
    if config.revision() != value.expected_revision {
        return error(
            &value.request_id,
            ErrorCode::Conflict,
            "Settings changed. Reload Tasks before saving the report to a note.",
        );
    }
    let context = match note_context(config, &run.url, false) {
        Ok(context) => context,
        Err((code, message)) => return error(&value.request_id, code, &message),
    };
    let location =
        match current_note_location(&context, &crate::relearn::aliases::Resolution::default()) {
            Ok(location) => location,
            Err(err) => return note_error(&value.request_id, err),
        };
    let loaded = match location.load(&context.vault, &context.canonical, &context.older_urls) {
        Ok(loaded) => loaded,
        Err(err) => return note_error(&value.request_id, err),
    };
    // Compare the entire host-rendered block, not a marker that another
    // report or user text could quote. A lost reply/reopened view can retry
    // without duplicating the report or overwriting a newer note.
    if loaded.body.contains(&block) {
        return saved(value, true);
    }
    let mut body = loaded.body;
    if !body.is_empty() {
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push('\n');
    }
    body.push_str(&block);
    let title = if loaded.title.is_empty() {
        &run.title
    } else {
        &loaded.title
    };
    match location.save(
        &context.vault,
        &StoredUrl {
            url: context.canonical,
            stripped_params: context.stripped_params,
        },
        title,
        &body,
        &loaded.revision,
    ) {
        Ok(note::SaveOutcome::Saved { .. }) => saved(value, false),
        Ok(note::SaveOutcome::Stale { .. }) => error(
            &value.request_id,
            ErrorCode::Conflict,
            "This note changed while the report was being appended. Try Save to note again.",
        ),
        Err(err) => note_error(&value.request_id, err),
    }
}

fn saved(value: SaveTaskNoteRequest, already_saved: bool) -> Response {
    Response::TaskNoteSaved(TaskNoteSaved {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id,
        run_id: value.run_id,
        already_saved,
    })
}

fn report_block(run: &RunView) -> Option<String> {
    if run.kind != Some(RunKind::Task)
        || run.state != RunState::Succeeded
        || run.no_page == Some(true)
    {
        return None;
    }
    let url = url::Url::parse(&run.url).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let report = run
        .task_report
        .as_deref()
        .filter(|text| !text.trim().is_empty())?;
    let instruction = run.instruction.as_deref()?;
    let finished = run.finished_at.as_deref()?;
    // Reuse the summary writer's neutralizer: agent text cannot create
    // remote embeds, raw HTML, plugin code blocks or executable inline code.
    Some(format!(
        "## Task report\n\n**Task:** {}\n\n**Finished:** {}\n\n{}\n\n<!-- {}:task-report {} -->\n",
        crate::markdown::inline(
            instruction,
            crate::service::record::MAX_TASK_INSTRUCTION_CHARS
        ),
        crate::markdown::inline(finished, 64),
        crate::markdown::neutralize_note_append(report),
        crate::brand::NAMESPACE,
        run.run_id,
    ))
}

#[cfg(test)]
mod tests;

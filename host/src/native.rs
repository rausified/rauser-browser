//! Chrome native messaging uses a four-byte, native-endian length prefix
//! followed by one UTF-8 JSON value. Stdout contains frames only.

use std::io::{self, Read, Write};

use anyhow::{Context, Result, bail};
use brauser_protocol::{
    ActivityRecorded, AgentChecked, ArtifactListResult, ArtifactResult, ArtifactView,
    ArtifactsRefreshed, BatchEventResult, BatchRecorded, CaptureEventKind, ConfigResult,
    ConfigUpdated, ErrorCode, ErrorResponse, FolderChosen, HarnessesDiscovered, HelloResult,
    NoteConflict, NoteLoaded, NoteSaved, PROTOCOL_VERSION, RecordKey, RecordKeyKind, Request,
    Response, VisitOutcome, VisitRecorded, WhyRecorded, WorklogResult,
};

use crate::brand::NAMESPACE;
use crate::capture::{CaptureOutcome, CapturePolicy, CaptureStore, activity_shape_is_valid};
use crate::config::{self, ConfigStore, NoteKeyRule};
use crate::consent::ConsentAuthority;
use crate::harness::HarnessEnv;
use crate::harness_setup;
use crate::index::ArtifactIndex;
use crate::note;
use crate::vault::Vault;

const MAX_INBOUND_BYTES: usize = 4 * 1024 * 1024;
// Chrome limits host-to-extension messages to 1 MiB. Leave room for the
// framing prefix and future envelope fields. Note limits (`note.rs`) are
// derived from this so a saved note can always be loaded again.
pub(crate) const MAX_OUTBOUND_BYTES: usize = 900 * 1024;
const MAX_REQUEST_ID_CHARS: usize = 128;

pub fn serve(config: ConfigStore) -> Result<()> {
    let input = io::stdin();
    let output = io::stdout();
    serve_with_io(input.lock(), output.lock(), config)
}

/// What harness setup may use from the host process. Built once per
/// connection; tests build it by hand with a fake search path.
pub struct SetupContext<'a> {
    pub env: &'a HarnessEnv,
}

/// Process requests until the browser closes the native messaging pipe.
pub fn serve_with_io<R: Read, W: Write>(input: R, output: W, config: ConfigStore) -> Result<()> {
    let env = HarnessEnv::from_process(config.dir()?);
    let context = SetupContext { env: &env };
    serve_with_context(input, output, config, &context)
}

pub fn serve_with_context<R: Read, W: Write>(
    mut input: R,
    mut output: W,
    mut config: ConfigStore,
    context: &SetupContext<'_>,
) -> Result<()> {
    let mut consent = ConsentAuthority::new();
    // Whether this is the panel's connection (`panel_request`).
    let mut interactive = false;
    loop {
        let body = match read_frame(&mut input) {
            Ok(Some(body)) => body,
            Ok(None) => {
                regenerate_artifacts(&mut config, interactive);
                return Ok(());
            }
            Err(FrameError::TooLarge) => {
                write_frame(
                    &mut output,
                    &error(
                        "",
                        ErrorCode::MessageTooLarge,
                        "native message exceeds the 4 MiB inbound limit",
                    ),
                )?;
                bail!("oversized native message; connection closed");
            }
            Err(FrameError::InvalidLength) => {
                write_frame(
                    &mut output,
                    &error(
                        "",
                        ErrorCode::InvalidRequest,
                        "native message has an empty body",
                    ),
                )?;
                continue;
            }
            Err(FrameError::Io(error)) => return Err(error).context("reading native message"),
        };
        interactive = interactive || panel_request(&body);
        let response = dispatch_json(&body, &mut config, &mut consent, context);
        write_frame(&mut output, &response)?;
    }
}

/// The requests only the side panel sends: its page note, artifact card,
/// context, Recent, and worklog. Closing a connection that sent one runs the
/// artifact pass (§6.7). The worker's background sync (lease and delivery)
/// and the settings page (which has its own "Update artifact files now"
/// action) only get the first pass of each local day.
fn panel_request(body: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Kind<'a> {
        #[serde(rename = "type", borrow)]
        kind: Option<std::borrow::Cow<'a, str>>,
    }
    serde_json::from_slice::<Kind<'_>>(body).is_ok_and(|value| {
        matches!(
            value.kind.as_deref(),
            Some(
                "load_note"
                    | "save_note"
                    | "use_artifact_note"
                    | "view_older_note"
                    | "get_artifact"
                    | "record_why"
                    | "list_artifacts"
                    | "worklog"
            )
        )
    })
}

/// Read the common envelope before the version-specific request. A newer
/// extension may add fields or message types this host cannot deserialize.
fn dispatch_json(
    body: &[u8],
    config: &mut ConfigStore,
    consent: &mut ConsentAuthority,
    context: &SetupContext<'_>,
) -> Response {
    let envelope: serde_json::Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return error("", ErrorCode::InvalidRequest, "invalid request JSON"),
    };
    let Some(object) = envelope.as_object() else {
        return error(
            "",
            ErrorCode::InvalidRequest,
            "request must be a JSON object",
        );
    };
    let Some(id) = object.get("request_id").and_then(serde_json::Value::as_str) else {
        return error("", ErrorCode::InvalidRequest, "request_id is required");
    };
    if !valid_request_id(id) {
        return error(
            "",
            ErrorCode::InvalidRequest,
            "request_id must be 1–128 non-control characters",
        );
    }
    let Some(version) = object
        .get("protocol_version")
        .and_then(serde_json::Value::as_u64)
    else {
        return error(
            id,
            ErrorCode::InvalidRequest,
            "protocol_version is required",
        );
    };
    if version != u64::from(PROTOCOL_VERSION) {
        return error(
            id,
            ErrorCode::UnsupportedProtocolVersion,
            "extension and host protocol versions differ",
        );
    }
    match serde_json::from_slice::<Request>(body) {
        Ok(request) => dispatch(request, config, consent, context),
        Err(_) => error(
            id,
            ErrorCode::InvalidRequest,
            "invalid or unsupported request JSON",
        ),
    }
}

fn valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_REQUEST_ID_CHARS
        && !id.chars().any(char::is_control)
}

fn validate_config_update(
    current: &brauser_protocol::ConfigSnapshot,
    next: &brauser_protocol::ConfigSnapshot,
) -> Result<()> {
    config::validate_update(current, next)?;
    if current.adapters != next.adapters {
        config::validate_adapter_transition(current, next)?;
    }
    Ok(())
}

fn dispatch(
    request: Request,
    config: &mut ConfigStore,
    consent: &mut ConsentAuthority,
    context: &SetupContext<'_>,
) -> Response {
    let id = request.request_id();
    if !valid_request_id(id) {
        return error(
            "",
            ErrorCode::InvalidRequest,
            "request_id must be 1–128 non-control characters",
        );
    }
    if request.protocol_version() != PROTOCOL_VERSION {
        return error(
            id,
            ErrorCode::UnsupportedProtocolVersion,
            "extension and host protocol versions differ",
        );
    }
    match request {
        // An unreadable config file leaves the store inert with an issue.
        // Report that state instead of an error: the extension treats a
        // failed hello as a missing host, and the issue says how to repair.
        Request::Hello(value) => {
            // The panel and settings ask on every open; that retries a
            // refused layout migration once the user has moved the folder.
            let _ = config.refresh();
            config.migrate_layout_or_warn();
            Response::HelloResult(HelloResult {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                host_version: env!("CARGO_PKG_VERSION").to_owned(),
                configured: config.configured(),
                config_issue: config.config_issue().map(str::to_owned),
            })
        }
        Request::GetConfig(value) => {
            // The panel and settings ask on every open; that retries a
            // refused layout migration once the user has moved the folder.
            let _ = config.refresh();
            config.migrate_layout_or_warn();
            let adapter_warnings = config.adapter_warnings().to_vec();
            Response::ConfigResult(ConfigResult {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                config: config.snapshot().clone(),
                revision: config.revision().to_owned(),
                config_issue: config.config_issue().map(str::to_owned),
                agent_status: config.agent_status(),
                adapter_issue: config.adapter_issue().map(str::to_owned),
                adapter_warnings,
            })
        }
        Request::UpdateConfig(value) => {
            if config.refresh().is_err() {
                return error(
                    &value.request_id,
                    ErrorCode::InvalidConfig,
                    "configuration file is unavailable",
                );
            }
            if value.expected_revision != config.revision() {
                return error(
                    &value.request_id,
                    ErrorCode::Conflict,
                    "configuration changed; read it again before updating",
                );
            }
            if let Err(validation_error) = validate_config_update(config.snapshot(), &value.config)
            {
                return error(
                    &value.request_id,
                    ErrorCode::InvalidConfig,
                    &validation_error.to_string(),
                );
            }
            let previous = config.snapshot().clone();
            let previous_revision = config.revision().to_owned();
            let authorized = match consent.authorize_update(
                &previous,
                &previous_revision,
                &value.config,
                value.picker_token.as_deref(),
                value.knowledge_base_token.as_deref(),
            ) {
                Ok(authorized) => authorized,
                Err(authority_error) => {
                    return error(
                        &value.request_id,
                        ErrorCode::Unauthorized,
                        &authority_error.to_string(),
                    );
                }
            };
            match config.update_with_grants(
                value.config.clone(),
                &value.expected_revision,
                authorized.selected_identity.as_deref(),
                authorized.knowledge_base_identity.as_deref(),
                None,
            ) {
                Ok(Some(revision)) => {
                    consent.consume_update(
                        &previous,
                        &previous_revision,
                        &value.config,
                        value.picker_token.as_deref(),
                        value.knowledge_base_token.as_deref(),
                    );
                    Response::ConfigUpdated(ConfigUpdated {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id,
                        config: value.config,
                        revision,
                    })
                }
                Ok(None) => error(
                    &value.request_id,
                    ErrorCode::Conflict,
                    "configuration changed; read it again before updating",
                ),
                // Saving the same home folder moves its older folders first;
                // a refusal says which folder is in the way.
                Err(save_error) => match save_error.downcast_ref::<crate::layout::Refusal>() {
                    Some(refusal) => error(&value.request_id, ErrorCode::InvalidConfig, &refusal.0),
                    None => error(
                        &value.request_id,
                        ErrorCode::Internal,
                        "could not save configuration",
                    ),
                },
            }
        }
        Request::ChooseFolder(value) => {
            if config.refresh().is_err() {
                return error(
                    &value.request_id,
                    ErrorCode::InvalidConfig,
                    "configuration file is unavailable",
                );
            }
            match consent.choose_folder(config.revision(), value.purpose) {
                Ok(Some(chosen)) => Response::FolderChosen(FolderChosen {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: value.request_id,
                    path: chosen.path,
                    picker_token: chosen.picker_token,
                }),
                Ok(None) => error(
                    &value.request_id,
                    ErrorCode::Cancelled,
                    "folder selection was canceled",
                ),
                Err(choose_error) => {
                    // The failure may come from the picker itself or from
                    // checking the folder the user picked (for example one
                    // macOS privacy settings protect); keep the cause.
                    eprintln!("{NAMESPACE}: folder selection failed: {choose_error:#}");
                    error(
                        &value.request_id,
                        ErrorCode::Internal,
                        "could not open the folder picker or use the selected folder",
                    )
                }
            }
        }
        Request::RecordVisit(value) => record_visit(value, config),
        Request::RecordActivity(value) => record_activity(value, config),
        Request::RecordBatch(value) => record_batch(value, config),
        Request::RecordWhy(value) => record_why(value, config),
        Request::LoadNote(value) => load_note(value, config),
        Request::SaveNote(value) => save_note(value, config),
        Request::UseArtifactNote(value) => use_artifact_note(value, config),
        Request::ViewOlderNote(value) => view_older_note(value, config),
        Request::GetArtifact(value) => get_artifact(value, config),
        Request::ListArtifacts(value) => list_artifacts(value, config),
        Request::Worklog(value) => worklog(value, config),
        Request::RefreshArtifacts(value) => refresh_artifacts(value, config),
        Request::CheckAgent(value) => check_agent(value, config),
        Request::DiscoverHarnesses(value) => Response::HarnessesDiscovered(HarnessesDiscovered {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            offers: harness_setup::offers(context.env),
        }),
        Request::SetUpHarness(value) => set_up_harness(value, config, context),
    }
}

/// Harness setup: rediscover, probe, and commit in one request. There is no
/// dialog and no token; success is the same `config_updated` a save returns.
fn set_up_harness(
    value: brauser_protocol::SetUpHarnessRequest,
    config: &mut ConfigStore,
    context: &SetupContext<'_>,
) -> Response {
    match harness_setup::set_up(config, &value, context.env) {
        Ok(revision) => Response::ConfigUpdated(ConfigUpdated {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            config: config.snapshot().clone(),
            revision,
        }),
        Err(setup_error) => error(&value.request_id, setup_error.code, &setup_error.message),
    }
}

/// The minimal agent-gated request (§7.3): every AI command goes through the
/// same readiness gate first.
fn check_agent(value: brauser_protocol::CheckAgentRequest, config: &mut ConfigStore) -> Response {
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "configuration is unavailable",
            );
        }
    };
    if config.refresh().is_err() {
        return error(
            &value.request_id,
            ErrorCode::InvalidConfig,
            "configuration is unavailable or needs repair",
        );
    }
    let ready = match config.agent_readiness() {
        Ok(ready) => ready,
        Err((code, message)) => return error(&value.request_id, code, &message),
    };
    Response::AgentChecked(AgentChecked {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id,
        harness_id: ready.harness_id,
        harness_version: ready.harness_version,
    })
}

fn record_visit(value: brauser_protocol::RecordVisitRequest, config: &mut ConfigStore) -> Response {
    let event_id = value.event.event_id.clone();
    if event_id.len() > 64 {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "event_id is too long",
        );
    }
    let retryable = |reason: &str| {
        Response::VisitRecorded(VisitRecorded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id.clone(),
            event_id: event_id.clone(),
            outcome: VisitOutcome::Retryable,
            reason: Some(reason.to_owned()),
            relative_path: None,
        })
    };
    // Serialize with all host config updates, then re-read policy while the
    // lock is held. A revoked site cannot be appended after revocation commits.
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => return retryable("configuration is unavailable"),
    };
    if config.refresh().is_err() || config.config_issue().is_some() {
        return retryable("configuration is unavailable or needs repair");
    }
    if !config.snapshot().capture_enabled {
        return Response::VisitRecorded(VisitRecorded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            event_id,
            outcome: VisitOutcome::Suppressed,
            reason: Some("capture_disabled".into()),
            relative_path: None,
        });
    }
    if config.adapter_issue().is_some() {
        return retryable("adapter policy is unavailable");
    }
    match config.adapter_matches(&value.event.url, value.event.title.as_deref()) {
        Ok(true) => {}
        Ok(false) => {
            return Response::VisitRecorded(VisitRecorded {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                event_id,
                outcome: VisitOutcome::Suppressed,
                reason: Some("not_an_artifact".into()),
                relative_path: None,
            });
        }
        Err(_) => return retryable("adapter policy is unavailable"),
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return retryable("choose a home folder before capture");
    };
    let store = match config
        .dir()
        .and_then(|dir| CaptureStore::open_in(storage, config.root_identity(), dir))
    {
        Ok(store) => store,
        Err(_) => return retryable("home folder is unavailable"),
    };
    let policy = CapturePolicy {
        enabled: config.snapshot().capture_enabled,
        origins: &config.capture_origins(),
        strip_params: &config.snapshot().strip_params,
    };
    let capture_outcome = store.record(policy, &value.event);
    if capture_outcome_updates_index(&capture_outcome) {
        reconcile_index_from_logs(config);
    }
    let (outcome, reason, relative_path) = match capture_outcome {
        CaptureOutcome::Persisted { relative_path } => (
            VisitOutcome::Persisted,
            None,
            Some(relative_path.to_string_lossy().into_owned()),
        ),
        CaptureOutcome::Suppressed { reason } => (VisitOutcome::Suppressed, Some(reason), None),
        CaptureOutcome::Rejected { reason } => (VisitOutcome::Rejected, Some(reason), None),
        CaptureOutcome::Retryable { message } => (VisitOutcome::Retryable, Some(message), None),
    };
    Response::VisitRecorded(VisitRecorded {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id,
        event_id,
        outcome,
        reason,
        relative_path,
    })
}

fn record_activity(
    value: brauser_protocol::RecordActivityRequest,
    config: &mut ConfigStore,
) -> Response {
    let event_id = value.event.event_id.clone();
    let answer = |outcome, reason: Option<String>, relative_path: Option<String>| {
        Response::ActivityRecorded(ActivityRecorded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id.clone(),
            event_id: event_id.clone(),
            outcome,
            reason,
            relative_path,
        })
    };
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return answer(
                VisitOutcome::Retryable,
                Some("configuration is unavailable".into()),
                None,
            );
        }
    };
    if config.refresh().is_err() || config.config_issue().is_some() {
        return answer(
            VisitOutcome::Retryable,
            Some("configuration is unavailable or needs repair".into()),
            None,
        );
    }
    if !config.snapshot().capture_enabled {
        return answer(
            VisitOutcome::Suppressed,
            Some("capture_disabled".into()),
            None,
        );
    }
    if config.adapter_issue().is_some() {
        return answer(
            VisitOutcome::Retryable,
            Some("adapter policy is unavailable".into()),
            None,
        );
    }
    if let Err((outcome, reason)) = check_activity_policy(config, &value.event) {
        return answer(outcome, Some(reason.into()), None);
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return answer(
            VisitOutcome::Retryable,
            Some("choose a home folder before capture".into()),
            None,
        );
    };
    let store = match config
        .dir()
        .and_then(|dir| CaptureStore::open_in(storage, config.root_identity(), dir))
    {
        Ok(store) => store,
        Err(_) => {
            return answer(
                VisitOutcome::Retryable,
                Some("home folder is unavailable".into()),
                None,
            );
        }
    };
    let policy = CapturePolicy {
        enabled: true,
        origins: &config.capture_origins(),
        strip_params: &config.snapshot().strip_params,
    };
    let capture_outcome = store.record_activity(policy, &value.event);
    if matches!(&capture_outcome, CaptureOutcome::Persisted { .. }) {
        update_activity_index(config, &value.event);
    } else if capture_outcome_is_duplicate(&capture_outcome) {
        reconcile_index_from_logs(config);
    }
    let (outcome, reason, relative_path) = match capture_outcome {
        CaptureOutcome::Persisted { relative_path } => (
            VisitOutcome::Persisted,
            None,
            Some(relative_path.to_string_lossy().into_owned()),
        ),
        CaptureOutcome::Suppressed { reason } => (VisitOutcome::Suppressed, Some(reason), None),
        CaptureOutcome::Rejected { reason } => (VisitOutcome::Rejected, Some(reason), None),
        CaptureOutcome::Retryable { message } => (VisitOutcome::Retryable, Some(message), None),
    };
    answer(outcome, reason, relative_path)
}

/// Deliver a bounded set of frozen capture records in one native exchange.
/// Legacy single-record messages remain accepted for extension upgrades, but
/// the current worker uses this response to apply all terminal acknowledgments
/// to its queue in one storage update.
fn record_batch(value: brauser_protocol::RecordBatchRequest, config: &mut ConfigStore) -> Response {
    if value.events.is_empty() || value.events.len() > 64 {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "a capture batch must contain 1 to 64 events",
        );
    }
    if uuid::Uuid::parse_str(&value.batch_id).is_err() {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "batch_id must be a UUID",
        );
    }

    enum Accepted {
        Visit(brauser_protocol::VisitEvent),
        Activity(brauser_protocol::ActivityEvent),
    }
    let mut accepted = Vec::with_capacity(value.events.len());
    let mut event_ids = std::collections::HashSet::with_capacity(value.events.len());
    for item in value.events {
        let accepted_event = match (item.kind, item.visit, item.activity) {
            (CaptureEventKind::Visit, Some(event), None) => {
                if !event_ids.insert(event.event_id.clone()) {
                    return error(
                        &value.request_id,
                        ErrorCode::InvalidRequest,
                        "capture batch contains a duplicate event_id",
                    );
                }
                Accepted::Visit(event)
            }
            (CaptureEventKind::Activity, None, Some(event)) => {
                if !event_ids.insert(event.event_id.clone()) {
                    return error(
                        &value.request_id,
                        ErrorCode::InvalidRequest,
                        "capture batch contains a duplicate event_id",
                    );
                }
                Accepted::Activity(event)
            }
            _ => {
                return error(
                    &value.request_id,
                    ErrorCode::InvalidRequest,
                    "capture batch item does not match its kind",
                );
            }
        };
        accepted.push(accepted_event);
    }

    let answer_all = |outcome: VisitOutcome, reason: &str| {
        accepted
            .iter()
            .map(|event| {
                let event_id = match event {
                    Accepted::Visit(event) => event.event_id.clone(),
                    Accepted::Activity(event) => event.event_id.clone(),
                };
                BatchEventResult {
                    event_id,
                    outcome,
                    reason: Some(reason.to_owned()),
                    relative_path: None,
                }
            })
            .collect()
    };
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return Response::BatchRecorded(BatchRecorded {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                batch_id: value.batch_id,
                results: answer_all(VisitOutcome::Retryable, "configuration is unavailable"),
                overflow_recorded: false,
            });
        }
    };
    if config.refresh().is_err() || config.config_issue().is_some() {
        return Response::BatchRecorded(BatchRecorded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            batch_id: value.batch_id,
            results: answer_all(
                VisitOutcome::Retryable,
                "configuration is unavailable or needs repair",
            ),
            overflow_recorded: false,
        });
    }
    if !config.snapshot().capture_enabled {
        return Response::BatchRecorded(BatchRecorded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            batch_id: value.batch_id,
            results: answer_all(VisitOutcome::Suppressed, "capture_disabled"),
            overflow_recorded: false,
        });
    }
    if config.adapter_issue().is_some() {
        return Response::BatchRecorded(BatchRecorded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            batch_id: value.batch_id,
            results: answer_all(VisitOutcome::Retryable, "adapter policy is unavailable"),
            overflow_recorded: false,
        });
    }

    let mut results = Vec::with_capacity(accepted.len());
    let mut durable = Vec::with_capacity(accepted.len());
    for event in &accepted {
        let policy_result = match event {
            Accepted::Visit(event) => {
                match config.adapter_matches(&event.url, event.title.as_deref()) {
                    Ok(true) => Ok(()),
                    Ok(false) => Err((VisitOutcome::Suppressed, "not_an_artifact")),
                    Err(_) => Err((VisitOutcome::Retryable, "adapter policy is unavailable")),
                }
            }
            Accepted::Activity(event) => check_activity_policy(config, event),
        };
        match policy_result {
            Ok(()) => durable.push(event),
            Err((outcome, reason)) => results.push(BatchEventResult {
                event_id: match event {
                    Accepted::Visit(event) => event.event_id.clone(),
                    Accepted::Activity(event) => event.event_id.clone(),
                },
                outcome,
                reason: Some(reason.into()),
                relative_path: None,
            }),
        }
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        results.extend(durable.into_iter().map(|event| BatchEventResult {
            event_id: match event {
                Accepted::Visit(event) => event.event_id.clone(),
                Accepted::Activity(event) => event.event_id.clone(),
            },
            outcome: VisitOutcome::Retryable,
            reason: Some("choose a home folder before capture".into()),
            relative_path: None,
        }));
        return Response::BatchRecorded(BatchRecorded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            batch_id: value.batch_id,
            results,
            overflow_recorded: false,
        });
    };
    let store = match config
        .dir()
        .and_then(|dir| CaptureStore::open_in(storage, config.root_identity(), dir))
    {
        Ok(store) => store,
        Err(_) => {
            results.extend(durable.into_iter().map(|event| BatchEventResult {
                event_id: match event {
                    Accepted::Visit(event) => event.event_id.clone(),
                    Accepted::Activity(event) => event.event_id.clone(),
                },
                outcome: VisitOutcome::Retryable,
                reason: Some("home folder is unavailable".into()),
                relative_path: None,
            }));
            return Response::BatchRecorded(BatchRecorded {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                batch_id: value.batch_id,
                results,
                overflow_recorded: false,
            });
        }
    };
    let capture_events = durable
        .iter()
        .map(|event| match event {
            Accepted::Visit(event) => crate::capture::CaptureBatchEvent::Visit(event),
            Accepted::Activity(event) => crate::capture::CaptureBatchEvent::Activity(event),
        })
        .collect::<Vec<_>>();
    let outcome = store.record_batch(
        CapturePolicy {
            enabled: config.snapshot().capture_enabled,
            origins: &config.capture_origins(),
            strip_params: &config.snapshot().strip_params,
        },
        &value.batch_id,
        &capture_events,
        value.overflow.as_ref(),
    );
    let indexable = outcome
        .events
        .iter()
        .filter_map(|(event_id, result)| {
            capture_outcome_updates_index(result).then_some(event_id.as_str())
        })
        .collect::<std::collections::HashSet<_>>();
    let duplicate_ids = outcome
        .events
        .iter()
        .filter_map(|(event_id, result)| {
            capture_outcome_is_duplicate(result).then_some(event_id.as_str())
        })
        .collect::<std::collections::HashSet<_>>();
    let reconcile = durable.iter().any(|event| match event {
        Accepted::Visit(event) => indexable.contains(event.event_id.as_str()),
        Accepted::Activity(event) => duplicate_ids.contains(event.event_id.as_str()),
    });
    let activities = durable
        .iter()
        .filter_map(|event| match event {
            Accepted::Activity(event)
                if !reconcile && indexable.contains(event.event_id.as_str()) =>
            {
                Some(event)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if reconcile {
        reconcile_index_from_logs(config);
    } else if !activities.is_empty()
        && let Ok(cache_dir) = config.cache_dir()
        && let Ok(index) = open_index(config, storage, &cache_dir)
        && let Err(error) = index.record_activities(activities, &config.snapshot().strip_params)
    {
        eprintln!("{NAMESPACE}: warning: could not update artifact index: {error:#}");
    }
    results.extend(outcome.events.into_iter().map(|(event_id, outcome)| {
        let (outcome, reason, relative_path) = match outcome {
            CaptureOutcome::Persisted { relative_path } => (
                VisitOutcome::Persisted,
                None,
                Some(relative_path.to_string_lossy().into_owned()),
            ),
            CaptureOutcome::Suppressed { reason } => (VisitOutcome::Suppressed, Some(reason), None),
            CaptureOutcome::Rejected { reason } => (VisitOutcome::Rejected, Some(reason), None),
            CaptureOutcome::Retryable { message } => (VisitOutcome::Retryable, Some(message), None),
        };
        BatchEventResult {
            event_id,
            outcome,
            reason,
            relative_path,
        }
    }));
    Response::BatchRecorded(BatchRecorded {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id,
        batch_id: value.batch_id,
        results,
        overflow_recorded: outcome.overflow_recorded,
    })
}

fn check_activity_policy(
    config: &ConfigStore,
    event: &brauser_protocol::ActivityEvent,
) -> std::result::Result<(), (VisitOutcome, &'static str)> {
    if !config.snapshot().capture_enabled {
        return Err((VisitOutcome::Suppressed, "capture_disabled"));
    }
    // Serde enforces types, not the JSON Schema's maxItems limits. Reject
    // oversized collections before replaying adapter evaluation for each
    // claimed mode or trail from this untrusted native message.
    if !activity_shape_is_valid(event) {
        return Err((VisitOutcome::Rejected, "invalid_activity"));
    }
    let matched = config
        .adapter_match(&event.url, event.title.as_deref())
        .map_err(|_| (VisitOutcome::Suppressed, "not_an_artifact"))?;
    if matched.artifact_id != event.artifact_id || matched.artifact_type != event.artifact_type {
        return Err((VisitOutcome::Rejected, "artifact_identity_mismatch"));
    }
    for mode in &event.modes {
        match config.adapter_match(&mode.url, None) {
            Ok(evidence)
                if evidence.artifact_id == event.artifact_id
                    && evidence
                        .mode
                        .is_some_and(|found| found.as_str() == mode.mode) => {}
            _ => return Err((VisitOutcome::Rejected, "activity_mode_mismatch")),
        }
    }
    for trail in &event.trails {
        match (&trail.from_artifact_id, &trail.from_url) {
            (None, None) => {}
            (Some(id), Some(url)) => match config.adapter_match(url, None) {
                Ok(source) if &source.artifact_id == id => {}
                _ => return Err((VisitOutcome::Rejected, "trail_identity_mismatch")),
            },
            _ => return Err((VisitOutcome::Rejected, "trail_identity_mismatch")),
        }
    }
    let expected_refs = matched.title.as_ref().map(|title| &title.refs);
    if event.refs.iter().any(|supplied| {
        !expected_refs.is_some_and(|refs| {
            refs.iter().any(|expected| {
                expected.artifact_type == supplied.artifact_type && expected.value == supplied.value
            })
        })
    }) {
        return Err((VisitOutcome::Rejected, "activity_refs_mismatch"));
    }
    Ok(())
}

fn record_why(value: brauser_protocol::RecordWhyRequest, config: &mut ConfigStore) -> Response {
    let event_id = value.event.event_id.clone();
    let answer = |outcome, reason: Option<String>, relative_path: Option<String>| {
        Response::WhyRecorded(WhyRecorded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id.clone(),
            event_id: event_id.clone(),
            outcome,
            reason,
            relative_path,
        })
    };
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return answer(
                VisitOutcome::Retryable,
                Some("configuration is unavailable".into()),
                None,
            );
        }
    };
    if config.refresh().is_err() || config.config_issue().is_some() {
        return answer(
            VisitOutcome::Retryable,
            Some("configuration is unavailable or needs repair".into()),
            None,
        );
    }
    let matched = match config.adapter_match(&value.event.url, None) {
        Ok(matched) if matched.artifact_id == value.event.artifact_id => matched,
        _ => {
            return answer(
                VisitOutcome::Suppressed,
                Some("not_an_artifact".into()),
                None,
            );
        }
    };
    let _ = matched;
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return answer(
            VisitOutcome::Retryable,
            Some("choose a home folder before capture".into()),
            None,
        );
    };
    let store = match config
        .dir()
        .and_then(|dir| CaptureStore::open_in(storage, config.root_identity(), dir))
    {
        Ok(store) => store,
        Err(_) => {
            return answer(
                VisitOutcome::Retryable,
                Some("home folder is unavailable".into()),
                None,
            );
        }
    };
    let policy = CapturePolicy {
        enabled: config.snapshot().capture_enabled,
        origins: &config.capture_origins(),
        strip_params: &config.snapshot().strip_params,
    };
    let capture_outcome = store.record_why(policy, &value.event);
    if matches!(&capture_outcome, CaptureOutcome::Persisted { .. })
        && let Ok(cache_dir) = config.cache_dir()
        && let Ok(index) = open_index(config, storage, &cache_dir)
        && let Err(error) = index.record_why(&value.event, &config.snapshot().strip_params)
    {
        eprintln!("{NAMESPACE}: warning: could not update artifact index: {error:#}");
    } else if capture_outcome_is_duplicate(&capture_outcome) {
        reconcile_index_from_logs(config);
    }
    let (outcome, reason, relative_path) = match capture_outcome {
        CaptureOutcome::Persisted { relative_path } => (
            VisitOutcome::Persisted,
            None,
            Some(relative_path.to_string_lossy().into_owned()),
        ),
        CaptureOutcome::Suppressed { reason } => (VisitOutcome::Suppressed, Some(reason), None),
        CaptureOutcome::Rejected { reason } => (VisitOutcome::Rejected, Some(reason), None),
        CaptureOutcome::Retryable { message } => (VisitOutcome::Retryable, Some(message), None),
    };
    answer(outcome, reason, relative_path)
}

fn update_activity_index(config: &ConfigStore, event: &brauser_protocol::ActivityEvent) {
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return;
    };
    if let Ok(cache_dir) = config.cache_dir()
        && let Ok(index) = open_index(config, storage, &cache_dir)
        && let Err(error) = index.record_activity(event, &config.snapshot().strip_params)
    {
        eprintln!("{NAMESPACE}: warning: could not update artifact index: {error:#}");
    }
}

fn capture_outcome_updates_index(outcome: &CaptureOutcome) -> bool {
    matches!(outcome, CaptureOutcome::Persisted { .. }) || capture_outcome_is_duplicate(outcome)
}

fn capture_outcome_is_duplicate(outcome: &CaptureOutcome) -> bool {
    matches!(outcome, CaptureOutcome::Suppressed { reason } if reason == "duplicate_event")
}

/// Open the artifact index with the confirmed adapter document, which
/// resolves title refs into mentions edges (adapter-templates.md §10.1).
fn open_index(
    config: &ConfigStore,
    storage: &brauser_protocol::StorageConfig,
    cache_dir: &std::path::Path,
) -> Result<ArtifactIndex> {
    Ok(
        ArtifactIndex::open_in(storage, config.root_identity(), cache_dir)?
            .with_adapters(config.adapter_document().ok()),
    )
}

fn reconcile_index_from_logs(config: &ConfigStore) {
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return;
    };
    if let Ok(cache_dir) = config.cache_dir()
        && let Ok(index) = open_index(config, storage, &cache_dir)
        && let Err(error) = index.reconcile_from_logs()
    {
        eprintln!("{NAMESPACE}: warning: could not reconcile artifact index: {error:#}");
    }
}

/// The connection-boundary pass (§6.7): when the panel's connection closes,
/// and on the first connection of each local day. Delivery alone only marks
/// artifacts dirty, so a background sync does not rewrite their files.
fn regenerate_artifacts(config: &mut ConfigStore, interactive: bool) {
    let Ok(_lock) = config.lock_current() else {
        return;
    };
    if config.refresh().is_err() || config.config_issue().is_some() {
        return;
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return;
    };
    let Ok(cache_dir) = config.cache_dir() else {
        return;
    };
    let index = match open_index(config, storage, &cache_dir) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("{NAMESPACE}: warning: could not regenerate artifact files: {error:#}");
            return;
        }
    };
    if !interactive && !index.daily_pass_due(time::OffsetDateTime::now_utc()) {
        return;
    }
    match index.regenerate_dirty() {
        Ok(report) => {
            if let Some(failure) = report.first_failure {
                eprintln!(
                    "{NAMESPACE}: warning: {} artifact file(s) could not be regenerated: {failure}",
                    report.failed
                );
            }
        }
        Err(error) => {
            eprintln!("{NAMESPACE}: warning: could not regenerate artifact files: {error:#}");
        }
    }
}

/// Resolve the canonical URL and vault shared by `load_note` and `save_note`.
/// Notes work on any HTTP(S) page (§5.3): unlike visit logging, this never
/// checks the capture origins. The error is a small `(code, message)` pair,
/// not a `Response`, so callers pay for the large `Response` enum only once.
struct NoteContext {
    canonical: String,
    vault: Vault,
    key: RecordKey,
    older_urls: Vec<String>,
}

fn note_context(
    config: &mut ConfigStore,
    url: &str,
    include_older_notes: bool,
) -> Result<NoteContext, (ErrorCode, &'static str)> {
    if config.refresh().is_err() || config.config_issue().is_some() {
        return Err((
            ErrorCode::InvalidConfig,
            "configuration is unavailable or needs repair",
        ));
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return Err((ErrorCode::NotConfigured, "choose a home folder first"));
    };
    let canonical = crate::capture::canonical_url(url, &config.snapshot().strip_params)
        .map_err(|_| (ErrorCode::InvalidRequest, "page URL is invalid"))?;
    let vault = Vault::open_checked(storage, config.root_identity())
        .map_err(|_| (ErrorCode::Internal, "home folder is unavailable"))?;
    // Match on the original URL, not the stripped canonical form: capture
    // (record_activity, record_why) evaluates adapter identity before any
    // strip-parameter normalization, so a parameter an adapter's match rule
    // requires must still be visible here, or this path and capture would
    // disagree on whether the page is recognized.
    let matched = config.adapter_match(url, None).ok();
    let key = match matched.as_ref() {
        Some(found) => RecordKey {
            kind: RecordKeyKind::Artifact,
            value: found.artifact_id.clone(),
        },
        None => match config.note_key_rule(url) {
            NoteKeyRule::Artifact(artifact_id) => RecordKey {
                kind: RecordKeyKind::Artifact,
                value: artifact_id,
            },
            // A covered origin is decided by its contracts alone; an
            // ambiguous one keeps the URL note rather than guessing. On an
            // origin no recorded contract covers (a config repair discarded
            // its history, or it was removed before that history existed),
            // recorded URLs are stripped canonical forms: a Jira board and
            // every issue modal on it share one once `selectedIssue` is
            // stripped, so no recorded URL can name this page's artifact.
            // The URL note is used, and any artifact note stays on disk.
            NoteKeyRule::Ambiguous | NoteKeyRule::Unrecognized | NoteKeyRule::Uncovered => {
                crate::vault::url_record_key(&canonical).expect("canonical HTTP URL has a key")
            }
            // A recorded contract this host cannot compile may name an
            // artifact here; refuse rather than pick an autosave target.
            NoteKeyRule::Unavailable => {
                return Err((
                    ErrorCode::Internal,
                    "note identity is unavailable; an adapter contract cannot be read",
                ));
            }
        },
    };
    // `older_urls` is both the older-note list and the only authorization
    // for viewing or migrating one, so every candidate must name this
    // artifact itself. Recorded URLs are stripped canonical forms: the
    // page's own canonical URL, an indexed modal route, or a note's creation
    // URL can equal an unrelated page's URL once `strip_params` removed the
    // query value that identified the artifact. A URL on an origin config.toml no longer covers names no artifact, for
    // the same reason it gets no artifact key above; that holds even for the
    // artifact's own indexed routes and its note's creation URL, which are
    // stripped forms too. A current match is final, as it is for the page's
    // own key: a retired shape cannot also claim a URL the current policy
    // names differently.
    let names_artifact = |candidate: &str| {
        if let Ok(found) = config.adapter_match(candidate, None) {
            return found.artifact_id == key.value;
        }
        match config.note_key_rule(candidate) {
            NoteKeyRule::Artifact(artifact_id) => artifact_id == key.value,
            NoteKeyRule::Ambiguous
            | NoteKeyRule::Unrecognized
            | NoteKeyRule::Unavailable
            | NoteKeyRule::Uncovered => false,
        }
    };
    let mut older_urls = Vec::new();
    if names_artifact(&canonical) {
        older_urls.push(canonical.clone());
    }
    if key.kind == RecordKeyKind::Artifact && include_older_notes {
        if let Ok(cache_dir) = config.cache_dir()
            && let Ok(index) = open_index(config, storage, &cache_dir)
            && let Ok(urls) = index.artifact_urls(&key.value)
        {
            older_urls.extend(urls.into_iter().filter(|url| names_artifact(url)));
        }
        if let Ok(candidates) = vault.note_candidates() {
            for contents in candidates {
                let candidates = [
                    note::artifact_keyed_note_url(&contents, &key),
                    note::url_keyed_note_url(&contents),
                ];
                older_urls.extend(
                    candidates
                        .into_iter()
                        .flatten()
                        .filter(|candidate| names_artifact(candidate)),
                );
            }
        }
        older_urls.sort();
        older_urls.dedup();
    }
    Ok(NoteContext {
        canonical,
        vault,
        key,
        older_urls,
    })
}

fn note_error(request_id: &str, error_value: note::NoteRequestError) -> Response {
    match error_value {
        note::NoteRequestError::TooLarge(message) => {
            error(request_id, ErrorCode::MessageTooLarge, &message)
        }
        note::NoteRequestError::Conflict(note::OwnershipConflict(message)) => {
            error(request_id, ErrorCode::Conflict, &message)
        }
        note::NoteRequestError::Internal(_) => error(
            request_id,
            ErrorCode::Internal,
            "could not access this page's note",
        ),
    }
}

fn load_note(value: brauser_protocol::LoadNoteRequest, config: &mut ConfigStore) -> Response {
    // Note identity lookup can catch up and publish the history cache.
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "configuration is unavailable",
            );
        }
    };
    let context = match note_context(config, &value.url, true) {
        Ok(resolved) => resolved,
        Err((code, message)) => return error(&value.request_id, code, message),
    };
    match note::load_keyed_note(
        &context.vault,
        &context.canonical,
        &context.key,
        &context.older_urls,
    ) {
        Ok(loaded) => Response::NoteLoaded(NoteLoaded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            exists: loaded.exists,
            revision: loaded.revision,
            title: loaded.title,
            body: loaded.body,
            key: loaded.key,
            older_notes: loaded.older_notes,
        }),
        Err(note_error_value) => note_error(&value.request_id, note_error_value),
    }
}

fn save_note(value: brauser_protocol::SaveNoteRequest, config: &mut ConfigStore) -> Response {
    // Serialize with config updates and other note saves so the version check
    // below and the write it guards happen without a concurrent writer.
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "configuration is unavailable",
            );
        }
    };
    // An ordinary autosave needs only the current key. Discovering every
    // legacy URL note scans the bounded pages directory and is reserved for
    // loads and explicit migrations, not repeated after each keystroke.
    let context = match note_context(config, &value.url, false) {
        Ok(resolved) => resolved,
        Err((code, message)) => return error(&value.request_id, code, message),
    };
    if value.expected_key != context.key {
        return match note::load_keyed_note(
            &context.vault,
            &context.canonical,
            &context.key,
            &context.older_urls,
        ) {
            Ok(current) => Response::NoteConflict(NoteConflict {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                exists: current.exists,
                revision: current.revision,
                title: current.title,
                body: current.body,
                key: current.key,
                reason: "key_changed".into(),
            }),
            Err(note_error_value) => note_error(&value.request_id, note_error_value),
        };
    }
    match note::save_keyed_note(
        &context.vault,
        &context.canonical,
        &context.key,
        &value.title,
        &value.body,
        &value.expected_revision,
    ) {
        Ok(note::SaveOutcome::Saved {
            outcome,
            revision,
            relative_path,
        }) => Response::NoteSaved(NoteSaved {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            outcome,
            revision,
            relative_path,
            key: context.key,
        }),
        Ok(note::SaveOutcome::Stale {
            revision,
            exists,
            title,
            body,
        }) => Response::NoteConflict(NoteConflict {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            exists,
            revision,
            title,
            body,
            key: context.key,
            reason: "stale".into(),
        }),
        Err(note_error_value) => note_error(&value.request_id, note_error_value),
    }
}

fn use_artifact_note(
    value: brauser_protocol::UseArtifactNoteRequest,
    config: &mut ConfigStore,
) -> Response {
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "configuration is unavailable",
            );
        }
    };
    let context = match note_context(config, &value.url, true) {
        Ok(resolved) => resolved,
        Err((code, message)) => return error(&value.request_id, code, message),
    };
    if context.key.kind != RecordKeyKind::Artifact {
        return error(
            &value.request_id,
            ErrorCode::Conflict,
            "this page is no longer a recognized artifact",
        );
    }
    // `source_url` is the exact identity returned by load_note. Do not apply
    // today's strip-parameter policy: a note created before that policy
    // changed is stored under the older canonical URL and must stay findable.
    if crate::vault::url_record_key(&value.source_url).is_err() {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "older note URL is invalid",
        );
    }
    if !context
        .older_urls
        .iter()
        .any(|url| url == &value.source_url)
    {
        return error(
            &value.request_id,
            ErrorCode::Unauthorized,
            "that older note is not part of this artifact's recorded history",
        );
    }
    match note::use_as_artifact_note(
        &context.vault,
        &context.canonical,
        &context.key,
        &value.source_url,
        &value.expected_revision,
    ) {
        Ok(loaded) => Response::NoteLoaded(NoteLoaded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            exists: loaded.exists,
            revision: loaded.revision,
            title: loaded.title,
            body: loaded.body,
            key: loaded.key,
            older_notes: loaded.older_notes,
        }),
        Err(note_error_value) => note_error(&value.request_id, note_error_value),
    }
}

/// Opens an older URL-keyed note read-only, without migrating it, so it
/// stays reachable even after a sibling older note has already become the
/// shared artifact note (at which point visiting its own URL would
/// otherwise resolve to that shared note instead).
fn view_older_note(
    value: brauser_protocol::ViewOlderNoteRequest,
    config: &mut ConfigStore,
) -> Response {
    // Note identity lookup can catch up and publish the history cache.
    let _config_lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "configuration is unavailable",
            );
        }
    };
    let context = match note_context(config, &value.url, true) {
        Ok(resolved) => resolved,
        Err((code, message)) => return error(&value.request_id, code, message),
    };
    if context.key.kind != RecordKeyKind::Artifact {
        return error(
            &value.request_id,
            ErrorCode::Conflict,
            "this page is no longer a recognized artifact",
        );
    }
    if !context
        .older_urls
        .iter()
        .any(|url| url == &value.source_url)
    {
        return error(
            &value.request_id,
            ErrorCode::Unauthorized,
            "that older note is not part of this artifact's recorded history",
        );
    }
    match note::load_older_note(&context.vault, &value.source_url) {
        Ok(loaded) => Response::NoteLoaded(NoteLoaded {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            exists: loaded.exists,
            revision: loaded.revision,
            title: loaded.title,
            body: loaded.body,
            key: loaded.key,
            older_notes: Vec::new(),
        }),
        Err(note_error_value) => note_error(&value.request_id, note_error_value),
    }
}

fn get_artifact(value: brauser_protocol::GetArtifactRequest, config: &mut ConfigStore) -> Response {
    let _lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
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
    let canonical = match crate::capture::canonical_url(&value.url, &config.snapshot().strip_params)
    {
        Ok(url) => url,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::InvalidRequest,
                "page URL is invalid",
            );
        }
    };
    // Match on the original URL, not the stripped canonical form (§ see
    // note_context): a query identity strip_params removes, such as Jira's
    // `selectedIssue`, must still be visible here or the artifact card and
    // context controls would disagree with capture and note lookup about
    // whether this page is recognized.
    let matched = config
        .adapter_match(&value.url, value.title.as_deref())
        .ok();
    let Some(matched) = matched else {
        return Response::ArtifactResult(ArtifactResult {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            recognized: false,
            artifact: None,
        });
    };
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return error(
            &value.request_id,
            ErrorCode::NotConfigured,
            "choose a home folder first",
        );
    };
    let cache_dir = match config.cache_dir() {
        Ok(path) => path,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "artifact index is unavailable",
            );
        }
    };
    let index = match open_index(config, storage, &cache_dir) {
        Ok(index) => index,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "artifact index is unavailable",
            );
        }
    };
    let artifact = match index.artifact(&matched.artifact_id) {
        Ok(Some(artifact)) => artifact,
        Ok(None) => ArtifactView {
            // Context can be saved before the artifact's first activity is
            // delivered, since activity derives rows but context does not
            // wait on it; look it up directly instead of hardcoding `None`
            // and blanking context the user just saved.
            why: index.latest_why(&matched.artifact_id).ok().flatten(),
            artifact_id: matched.artifact_id,
            artifact_type: matched.artifact_type,
            title: matched
                .title
                .map(|title| title.title)
                .or(value.title)
                .unwrap_or_else(|| "Untitled".into()),
            url: canonical,
            first_seen: String::new(),
            last_seen: String::new(),
            focused_seconds: 0,
            background_seconds: 0,
            opens: 0,
            days_opened: 0,
            last_mode: matched.mode.map(|mode| mode.as_str().to_owned()),
            linked_count: 0,
        },
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "artifact history is unavailable; log catch-up could not complete",
            );
        }
    };
    Response::ArtifactResult(ArtifactResult {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id,
        recognized: true,
        artifact: Some(artifact),
    })
}

fn list_artifacts(
    value: brauser_protocol::ListArtifactsRequest,
    config: &mut ConfigStore,
) -> Response {
    let _lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "configuration is unavailable",
            );
        }
    };
    let index = match read_index(config) {
        Ok(index) => index,
        Err((code, message)) => return error(&value.request_id, code, message),
    };
    // Thirty-two maximal rows still fit comfortably below Chrome's native
    // messaging cap after JSON escaping.
    let limit = usize::try_from(value.limit.clamp(1, 32)).unwrap_or(32);
    match index.recent(
        value.artifact_type.as_deref(),
        value.title_query.as_deref(),
        limit,
    ) {
        Ok((items, artifact_types, truncated)) => {
            Response::ArtifactListResult(ArtifactListResult {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                items,
                artifact_types,
                truncated,
            })
        }
        Err(_) => error(
            &value.request_id,
            ErrorCode::Internal,
            "artifact history is unavailable; log catch-up could not complete",
        ),
    }
}

fn worklog(value: brauser_protocol::WorklogRequest, config: &mut ConfigStore) -> Response {
    let _lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "configuration is unavailable",
            );
        }
    };
    let start = match local_date(&value.start_date) {
        Some(date) => date,
        None => {
            return error(
                &value.request_id,
                ErrorCode::InvalidRequest,
                "worklog start_date is invalid",
            );
        }
    };
    let end = match local_date(&value.end_date) {
        Some(date) => date,
        None => {
            return error(
                &value.request_id,
                ErrorCode::InvalidRequest,
                "worklog end_date is invalid",
            );
        }
    };
    if end < start || (end - start).whole_days() > 6 {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "worklog range must be one to seven inclusive local dates",
        );
    }
    let index = match read_index(config) {
        Ok(index) => index,
        Err((code, message)) => return error(&value.request_id, code, message),
    };
    match index.worklog(start, end) {
        Ok((text, count)) => Response::WorklogResult(WorklogResult {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            start_date: value.start_date,
            end_date: value.end_date,
            text,
            artifact_count: u32::try_from(count).unwrap_or(u32::MAX),
        }),
        Err(_) => error(
            &value.request_id,
            ErrorCode::Internal,
            "worklog history is unavailable; log catch-up could not complete",
        ),
    }
}

fn refresh_artifacts(
    value: brauser_protocol::RefreshArtifactsRequest,
    config: &mut ConfigStore,
) -> Response {
    let _lock = match config.lock_current() {
        Ok(lock) => lock,
        Err(_) => {
            return error(
                &value.request_id,
                ErrorCode::Internal,
                "configuration is unavailable",
            );
        }
    };
    let index = match read_index(config) {
        Ok(index) => index,
        Err((code, message)) => return error(&value.request_id, code, message),
    };
    match index.regenerate_dirty() {
        // The others were updated; name the path that still needs attention,
        // and any unowned conflicts, since an error carries no paths field.
        Ok(report) if report.failed > 0 => {
            let mut message = format!(
                "{} artifact file(s) could not be updated ({} written); first: {}",
                report.failed,
                report.written,
                report.failed_paths.first().map_or("", String::as_str)
            );
            if report.conflicts > 0 {
                message.push_str(&format!(
                    ". {} unowned path conflict(s) were not overwritten: {}",
                    report.conflicts,
                    report.conflict_paths.join(", ")
                ));
            }
            error(&value.request_id, ErrorCode::Internal, &message)
        }
        Ok(report) => Response::ArtifactsRefreshed(ArtifactsRefreshed {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            written: u32::try_from(report.written).unwrap_or(u32::MAX),
            unchanged: u32::try_from(report.unchanged).unwrap_or(u32::MAX),
            conflicts: u32::try_from(report.conflicts).unwrap_or(u32::MAX),
            conflict_paths: report.conflict_paths,
        }),
        Err(_) => error(
            &value.request_id,
            ErrorCode::Internal,
            "artifact files could not be updated",
        ),
    }
}

fn read_index(config: &mut ConfigStore) -> Result<ArtifactIndex, (ErrorCode, &'static str)> {
    if config.refresh().is_err() || config.config_issue().is_some() {
        return Err((
            ErrorCode::InvalidConfig,
            "configuration is unavailable or needs repair",
        ));
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return Err((ErrorCode::NotConfigured, "choose a home folder first"));
    };
    let unavailable = || (ErrorCode::Internal, "artifact index is unavailable");
    let cache_dir = config.cache_dir().map_err(|_| unavailable())?;
    open_index(config, storage, &cache_dir).map_err(|_| unavailable())
}

fn local_date(value: &str) -> Option<time::Date> {
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]").ok()?;
    time::Date::parse(value, &format).ok()
}

fn error(request_id: &str, code: ErrorCode, message: &str) -> Response {
    Response::Error(ErrorResponse {
        protocol_version: PROTOCOL_VERSION,
        request_id: request_id.to_owned(),
        code,
        message: message.to_owned(),
    })
}

#[derive(Debug)]
enum FrameError {
    TooLarge,
    InvalidLength,
    Io(io::Error),
}

fn read_frame<R: Read>(input: &mut R) -> std::result::Result<Option<Vec<u8>>, FrameError> {
    let mut prefix = [0u8; 4];
    let received = input.read(&mut prefix[..1]).map_err(FrameError::Io)?;
    if received == 0 {
        return Ok(None);
    }
    input.read_exact(&mut prefix[1..]).map_err(FrameError::Io)?;
    let size = u32::from_ne_bytes(prefix) as usize;
    if size == 0 {
        return Err(FrameError::InvalidLength);
    }
    if size > MAX_INBOUND_BYTES {
        return Err(FrameError::TooLarge);
    }
    let mut body = vec![0u8; size];
    input.read_exact(&mut body).map_err(FrameError::Io)?;
    Ok(Some(body))
}

fn write_frame<W: Write>(output: &mut W, response: &Response) -> Result<()> {
    let mut body = serde_json::to_vec(response).context("serializing native response")?;
    if body.len() > MAX_OUTBOUND_BYTES {
        // One oversized answer must not end the connection: that would drop
        // every other in-flight request and show the extension only a
        // disconnect. Answer the same request with a small error instead.
        let fallback = error(
            response.request_id(),
            ErrorCode::MessageTooLarge,
            "response exceeds the host-to-extension size limit",
        );
        body = serde_json::to_vec(&fallback).context("serializing native response")?;
    }
    let size = u32::try_from(body.len()).context("native response length overflow")?;
    output.write_all(&size.to_ne_bytes())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use brauser_protocol::{
        ActivityEvent, ActivityMode, ActivityTrail, CaptureBatchItem, ConfigSnapshot,
        GetConfigRequest, HelloRequest, RecordActivityRequest, RecordBatchRequest,
        RecordVisitRequest, UpdateConfigRequest, VisitEvent,
    };
    use std::io::Cursor;

    fn encoded_request(request: Request) -> Vec<u8> {
        let json = serde_json::to_vec(&request).unwrap();
        encoded_json(&json)
    }

    fn encoded_json(json: &[u8]) -> Vec<u8> {
        let mut wire = (json.len() as u32).to_ne_bytes().to_vec();
        wire.extend_from_slice(json);
        wire
    }

    fn decoded_response(wire: Vec<u8>) -> Response {
        let mut input = Cursor::new(wire);
        let json = read_frame(&mut input).unwrap().unwrap();
        serde_json::from_slice(&json).unwrap()
    }

    #[test]
    fn an_unreadable_config_answers_hello_and_get_config_with_its_issue() {
        // The extension treats a failed hello as a missing host, so an
        // unreadable config must still answer hello and report the repair.
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        std::fs::create_dir(&path).unwrap();
        let config = ConfigStore::for_test(path);
        let requests = [
            Request::Hello(HelloRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "hello".into(),
            }),
            Request::GetConfig(GetConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "read".into(),
            }),
            Request::UpdateConfig(UpdateConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "write".into(),
                expected_revision: "unreadable".into(),
                config: ConfigSnapshot {
                    storage: None,
                    capture_enabled: false,
                    strip_params: Vec::new(),
                    agent: None,
                    adapters: crate::config::empty_config().adapters,
                },
                picker_token: None,
                knowledge_base_token: None,
            }),
        ];
        let input: Vec<u8> = requests.into_iter().flat_map(encoded_request).collect();
        let mut output = Vec::new();
        serve_with_io(Cursor::new(input), &mut output, config).unwrap();
        let mut reader = Cursor::new(output);
        let mut responses = Vec::new();
        while let Some(body) = read_frame(&mut reader).unwrap() {
            responses.push(serde_json::from_slice::<Response>(&body).unwrap());
        }
        match &responses[0] {
            Response::HelloResult(value) => {
                assert!(!value.configured);
                assert!(value.config_issue.is_some());
            }
            other => panic!("expected hello_result, got {other:?}"),
        }
        match &responses[1] {
            Response::ConfigResult(value) => {
                assert!(value.config.storage.is_none());
                assert!(value.config.adapters.artifacts.is_empty());
                assert!(value.config_issue.is_some());
            }
            other => panic!("expected config_result, got {other:?}"),
        }
        match &responses[2] {
            Response::Error(value) => assert_eq!(value.code, ErrorCode::InvalidConfig),
            other => panic!("expected invalid_config, got {other:?}"),
        }
    }

    #[test]
    fn config_revision_round_trips_and_rejects_a_stale_update() {
        let folder = tempfile::tempdir().unwrap();
        let config = ConfigStore::for_test(folder.path().join("config.toml"));
        let snapshot = ConfigSnapshot {
            storage: None,
            capture_enabled: false,
            strip_params: vec!["utm_*".into(), "fbclid".into(), "gclid".into()],
            ..crate::config::empty_config()
        };
        let requests = [
            Request::GetConfig(GetConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "read-1".into(),
            }),
            Request::UpdateConfig(UpdateConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "write-1".into(),
                expected_revision: "missing".into(),
                config: snapshot.clone(),
                picker_token: None,
                knowledge_base_token: None,
            }),
            Request::GetConfig(GetConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "read-2".into(),
            }),
            Request::UpdateConfig(UpdateConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "write-stale".into(),
                expected_revision: "missing".into(),
                config: snapshot,
                picker_token: None,
                knowledge_base_token: None,
            }),
        ];
        let input: Vec<u8> = requests.into_iter().flat_map(encoded_request).collect();
        let mut output = Vec::new();
        serve_with_io(Cursor::new(input), &mut output, config).unwrap();

        let mut reader = Cursor::new(output);
        let mut responses = Vec::new();
        while let Some(body) = read_frame(&mut reader).unwrap() {
            responses.push(serde_json::from_slice::<Response>(&body).unwrap());
        }
        assert_eq!(responses.len(), 4);
        match &responses[0] {
            Response::ConfigResult(value) => assert_eq!(value.revision, "missing"),
            other => panic!("expected initial config, got {other:?}"),
        }
        let revision = match &responses[1] {
            Response::ConfigUpdated(value) => value.revision.clone(),
            other => panic!("expected config update, got {other:?}"),
        };
        assert!(revision.starts_with("sha256:"));
        match &responses[2] {
            Response::ConfigResult(value) => assert_eq!(value.revision, revision),
            other => panic!("expected saved config, got {other:?}"),
        }
        match &responses[3] {
            Response::Error(value) => {
                assert_eq!(value.code, ErrorCode::Conflict);
                assert_eq!(value.request_id, "write-stale");
            }
            other => panic!("expected stale-update conflict, got {other:?}"),
        }
    }

    #[test]
    fn native_config_flow_preserves_an_invalid_adapter_document() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        crate::config::fixtures::write(
            &path,
            folder.path(),
            "",
            "capture_enabled = false\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            "[adapters]\nformat = 2\n\n[[artifacts]]\ntype = \"work.issue\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/issues/{id:int}\" }]\nid = \"work.issue:{host}/{id}\"\n",
        );
        let mut config = ConfigStore::for_test(path);
        config.refresh().unwrap();
        let revision = config.revision().to_owned();
        let mut next = config.snapshot().clone();
        next.strip_params.push("campaign".into());
        let mut consent = ConsentAuthority::new();
        let env = no_harnesses(folder.path());
        let context = SetupContext { env: &env };

        let updated = dispatch(
            Request::UpdateConfig(UpdateConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "update".into(),
                expected_revision: revision,
                config: next,
                picker_token: None,
                knowledge_base_token: None,
            }),
            &mut config,
            &mut consent,
            &context,
        );

        match updated {
            Response::ConfigUpdated(value) => {
                assert_eq!(value.config.adapters.format, 2);
                assert!(value.config.strip_params.iter().any(|v| v == "campaign"));
            }
            other => panic!("expected config update, got {other:?}"),
        }
        assert!(config.adapter_issue().is_some());
    }

    #[test]
    fn mismatched_protocol_returns_a_correlated_error() {
        let folder = tempfile::tempdir().unwrap();
        let config = ConfigStore::for_test(folder.path().join("config.toml"));
        let request = Request::Hello(HelloRequest {
            protocol_version: PROTOCOL_VERSION + 1,
            request_id: "request-1".into(),
        });
        let mut output = Vec::new();
        serve_with_io(Cursor::new(encoded_request(request)), &mut output, config).unwrap();
        match decoded_response(output) {
            Response::Error(value) => {
                assert_eq!(value.code, ErrorCode::UnsupportedProtocolVersion);
                assert_eq!(value.request_id, "request-1");
            }
            other => panic!("expected protocol error, got {other:?}"),
        }
    }

    #[test]
    fn future_message_type_reports_version_before_shape_error() {
        let folder = tempfile::tempdir().unwrap();
        let config = ConfigStore::for_test(folder.path().join("config.toml"));
        let request = format!(
            r#"{{"type":"future_operation","protocol_version":{},"request_id":"future-1","new_field":true}}"#,
            PROTOCOL_VERSION + 1
        );
        let mut output = Vec::new();
        serve_with_io(
            Cursor::new(encoded_json(request.as_bytes())),
            &mut output,
            config,
        )
        .unwrap();
        match decoded_response(output) {
            Response::Error(value) => {
                assert_eq!(value.code, ErrorCode::UnsupportedProtocolVersion);
                assert_eq!(value.request_id, "future-1");
            }
            other => panic!("expected version error, got {other:?}"),
        }
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocating_its_body() {
        let folder = tempfile::tempdir().unwrap();
        let config = ConfigStore::for_test(folder.path().join("config.toml"));
        let prefix = ((MAX_INBOUND_BYTES + 1) as u32).to_ne_bytes();
        let mut output = Vec::new();
        assert!(serve_with_io(Cursor::new(prefix), &mut output, config).is_err());
        match decoded_response(output) {
            Response::Error(value) => assert_eq!(value.code, ErrorCode::MessageTooLarge),
            other => panic!("expected size error, got {other:?}"),
        }
    }

    #[test]
    fn unauthorized_roots_are_refused_before_any_filesystem_probe() {
        // An extension-supplied root is untrusted. Whether it exists must not
        // change the answer, or the host becomes a directory-existence oracle.
        let folder = tempfile::tempdir().unwrap();
        let existing = folder.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        let mut config = ConfigStore::for_test(folder.path().join("config.toml"));
        let mut consent = ConsentAuthority::new();
        let env = no_harnesses(folder.path());
        let context = SetupContext { env: &env };
        for root in [existing.clone(), folder.path().join("absent")] {
            let snapshot = ConfigSnapshot {
                storage: Some(brauser_protocol::StorageConfig {
                    root: root.to_string_lossy().into_owned(),
                    profile: "neutral".into(),
                    log_dir: "logs".into(),
                    pages_dir: "notes".into(),
                    later_dir: "later".into(),
                    artifacts_dir: "artifacts".into(),
                    summaries_dir: Some("summaries".into()),
                    knowledge_base_dir: None,
                }),
                capture_enabled: false,
                strip_params: Vec::new(),
                agent: None,
                adapters: crate::config::empty_config().adapters,
            };
            let update = dispatch(
                Request::UpdateConfig(UpdateConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "probe-update".into(),
                    expected_revision: "missing".into(),
                    config: snapshot,
                    picker_token: None,
                    knowledge_base_token: None,
                }),
                &mut config,
                &mut consent,
                &context,
            );
            match update {
                Response::Error(value) => assert_eq!(
                    value.code,
                    ErrorCode::Unauthorized,
                    "{} for {}",
                    value.message,
                    root.display()
                ),
                other => panic!("expected an authorization error, got {other:?}"),
            }
        }
    }

    fn update_request(revision: &str, config: ConfigSnapshot) -> Request {
        Request::UpdateConfig(UpdateConfigRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "update".into(),
            expected_revision: revision.into(),
            config,
            picker_token: None,
            knowledge_base_token: None,
        })
    }

    #[test]
    fn update_config_turning_capture_on_with_a_new_adapter_needs_no_confirmation() {
        let folder = tempfile::tempdir().unwrap();
        let mut config = configured_store(folder.path());
        let mut consent = ConsentAuthority::new();
        let env = no_harnesses(folder.path());
        let context = SetupContext { env: &env };
        let read = dispatch(
            Request::GetConfig(GetConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "read".into(),
            }),
            &mut config,
            &mut consent,
            &context,
        );
        let (current, revision) = match &read {
            Response::ConfigResult(value) => (value.config.clone(), value.revision.clone()),
            other => panic!("expected config_result, got {other:?}"),
        };
        let mut next = current;
        next.capture_enabled = true;
        next.adapters.artifacts.push(serde_json::json!({
            "type": "work.issue",
            "origin": "https://work.example",
            "match": [{"path": "/issues/{id:int}"}],
            "id": "work.issue:{host}/{id}",
        }));
        let updated = dispatch(
            update_request(&revision, next),
            &mut config,
            &mut consent,
            &context,
        );
        let responses = [read, updated];
        assert!(
            matches!(responses[1], Response::ConfigUpdated(_)),
            "{:?}",
            responses[1]
        );
        assert!(config.snapshot().capture_enabled);
        assert_eq!(config.snapshot().adapters.artifacts.len(), 1);
    }

    #[test]
    fn update_config_still_refuses_a_stale_revision() {
        let folder = tempfile::tempdir().unwrap();
        let mut config = configured_store(folder.path());
        let revision = config.revision().to_owned();
        let mut next = config.snapshot().clone();
        next.strip_params.push("campaign".into());
        let mut consent = ConsentAuthority::new();
        let env = no_harnesses(folder.path());
        let context = SetupContext { env: &env };
        match dispatch(
            update_request("stale", next),
            &mut config,
            &mut consent,
            &context,
        ) {
            Response::Error(value) => assert_eq!(value.code, ErrorCode::Conflict),
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert_eq!(config.revision(), revision);
    }

    fn configured_store(root: &std::path::Path) -> ConfigStore {
        let mut store = ConfigStore::for_test(root.join("config.toml"));
        let snapshot = brauser_protocol::ConfigSnapshot {
            storage: Some(brauser_protocol::StorageConfig {
                root: root.to_string_lossy().into_owned(),
                profile: "neutral".into(),
                log_dir: "logs".into(),
                pages_dir: "notes".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: Some("summaries".into()),
                knowledge_base_dir: None,
            }),
            capture_enabled: false,
            strip_params: Vec::new(),
            ..crate::config::empty_config()
        };
        let identity = crate::vault::selected_root_identity(root).unwrap();
        store
            .update_with_root_identity(snapshot, "missing", Some(&identity))
            .unwrap()
            .unwrap();
        store
    }

    fn write_adapter_config(path: &std::path::Path, root: &std::path::Path, adapter: &str) {
        config::fixtures::write(
            path,
            root,
            "artifacts_dir = \"artifacts\"",
            "capture_enabled = true",
            &format!("[adapters]\nformat = 1\n\n{adapter}"),
        );
    }

    fn note_load(config: &mut ConfigStore, url: &str) -> brauser_protocol::NoteLoaded {
        match load_note(
            brauser_protocol::LoadNoteRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "load".into(),
                url: url.into(),
            },
            config,
        ) {
            Response::NoteLoaded(note) => note,
            other => panic!("expected note_loaded, got {other:?}"),
        }
    }

    fn note_save(
        config: &mut ConfigStore,
        url: &str,
        body: &str,
        revision: &str,
        key: RecordKey,
    ) -> Response {
        save_note(
            brauser_protocol::SaveNoteRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "save".into(),
                url: url.into(),
                title: "Shared note".into(),
                body: body.into(),
                expected_revision: revision.into(),
                expected_key: key,
            },
            config,
        )
    }

    #[test]
    fn google_doc_edit_and_view_routes_share_one_note() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "gdoc"
origin = "https://docs.google.com"
unscoped = "acknowledged"
match = [{ path = "/document/d/{id:token(20,64)}/{mode?:(edit|view|preview)}" }]
modes = { edit = "edit", view = "view", preview = "view" }
id = "gdoc:{host}/{id}"
"#,
        );
        let mut config = loaded(&path);
        let edit = "https://docs.google.com/document/d/abcdefghijklmnopqrst/edit";
        let view = "https://docs.google.com/document/d/abcdefghijklmnopqrst/view";
        let legacy_vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
        note::save_note(
            &legacy_vault,
            edit,
            "Before adapter",
            "older URL note",
            "missing",
        )
        .unwrap();
        let first = note_load(&mut config, view);
        assert_eq!(first.key.kind, RecordKeyKind::Artifact);
        assert_eq!(first.older_notes.len(), 1);
        assert_eq!(first.older_notes[0].canonical_url, edit);
        assert!(matches!(
            note_save(
                &mut config,
                view,
                "same document",
                &first.revision,
                first.key.clone()
            ),
            Response::NoteSaved(_)
        ));
        let loaded = note_load(&mut config, edit);
        assert_eq!(loaded.key, first.key);
        assert_eq!(loaded.body, "same document");
    }

    #[test]
    fn older_note_migration_keeps_the_source_url_from_before_param_stripping() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let mut config = loaded(&path);
        let old_url = "https://work.example/items/7?utm_source=before-policy";
        let current_url = "https://work.example/items/7";
        let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
        let revision = match note::save_note(
            &vault,
            old_url,
            "Before stripping",
            "keep this text",
            "missing",
        )
        .unwrap()
        {
            note::SaveOutcome::Saved { revision, .. } => revision,
            note::SaveOutcome::Stale { .. } => panic!("new URL note was unexpectedly stale"),
        };
        let loaded = note_load(&mut config, current_url);
        assert_eq!(loaded.older_notes.len(), 1);
        assert_eq!(loaded.older_notes[0].canonical_url, old_url);

        let response = use_artifact_note(
            brauser_protocol::UseArtifactNoteRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "migrate".into(),
                url: current_url.into(),
                source_url: old_url.into(),
                expected_revision: revision,
            },
            &mut config,
        );
        match response {
            Response::NoteLoaded(note) => assert_eq!(note.body, "keep this text"),
            other => panic!("expected migrated note, got {other:?}"),
        }
        assert!(!note::load_note(&vault, old_url).unwrap().exists);
    }

    #[test]
    fn an_older_note_can_be_viewed_through_the_native_host_without_migrating() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let mut config = loaded(&path);
        let old_url = "https://work.example/items/7?utm_source=before-policy";
        let current_url = "https://work.example/items/7";
        let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
        note::save_note(
            &vault,
            old_url,
            "Before stripping",
            "keep this text",
            "missing",
        )
        .unwrap();
        // Populate `older_urls`, the same precondition `use_artifact_note` relies on.
        assert_eq!(note_load(&mut config, current_url).older_notes.len(), 1);

        let response = view_older_note(
            brauser_protocol::ViewOlderNoteRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "view-1".into(),
                url: current_url.into(),
                source_url: old_url.into(),
            },
            &mut config,
        );
        match response {
            Response::NoteLoaded(note) => {
                assert_eq!(note.body, "keep this text");
                assert_eq!(note.key.kind, RecordKeyKind::Url);
            }
            other => panic!("expected the older note's body, got {other:?}"),
        }
        // Viewing must not migrate anything: the older note stays a
        // separate URL-keyed note, not merged into the shared artifact note.
        assert!(note::load_note(&vault, old_url).unwrap().exists);
    }

    #[test]
    fn viewing_an_unrelated_url_as_an_older_note_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let mut config = loaded(&path);
        let response = view_older_note(
            brauser_protocol::ViewOlderNoteRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "view-2".into(),
                url: "https://work.example/items/7".into(),
                source_url: "https://unrelated.example/other".into(),
            },
            &mut config,
        );
        match response {
            Response::Error(error) => assert_eq!(error.code, ErrorCode::Unauthorized),
            other => panic!("expected an unauthorized error, got {other:?}"),
        }
    }

    #[test]
    fn jira_board_modal_and_browse_routes_share_one_note() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "jira.issue"
origin = "https://acme.atlassian.net"
match = [{ path = "/browse/{key:jira_key}" }, { path = "/jira/{*}", query = { selectedIssue = "{key:jira_key}" } }]
id = "jira:{host}/{key}"
"#,
        );
        let mut config = loaded(&path);
        let modal =
            "https://acme.atlassian.net/jira/software/projects/ABC/boards/1?selectedIssue=ABC-123";
        let browse = "https://acme.atlassian.net/browse/ABC-123";
        let first = note_load(&mut config, modal);
        assert!(matches!(
            note_save(
                &mut config,
                modal,
                "same issue",
                &first.revision,
                first.key.clone()
            ),
            Response::NoteSaved(_)
        ));
        let loaded = note_load(&mut config, browse);
        assert_eq!(loaded.key, first.key);
        assert_eq!(loaded.body, "same issue");
    }

    #[test]
    fn adapter_change_mid_edit_is_a_key_changed_conflict() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        let adapter = |prefix: &str| {
            format!(
                r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{{ path = "/items/{{id:int}}" }}]
id = "{prefix}:{{host}}/{{id}}"
"#
            )
        };
        write_adapter_config(&path, &root, &adapter("work"));
        let mut config = loaded(&path);
        let url = "https://work.example/items/7";
        let loaded = note_load(&mut config, url);
        write_adapter_config(&path, &root, &adapter("replacement"));
        match note_save(&mut config, url, "unsaved", &loaded.revision, loaded.key) {
            Response::NoteConflict(conflict) => {
                assert_eq!(conflict.reason, "key_changed");
                assert_eq!(conflict.key.value, "replacement:work.example/7");
            }
            other => panic!("expected key_changed conflict, got {other:?}"),
        }
    }

    #[test]
    fn an_ambiguous_historical_identity_is_rejected_rather_than_guessed() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(&path, &root, "");
        let mut config = loaded(&path);
        let url = "https://work.example/items/7";

        // Two different artifacts, each with their own note, that happen
        // to share one stripped canonical URL.
        let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
        let artifact_a = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:a/7".into(),
        };
        let artifact_b = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:b/7".into(),
        };
        note::save_keyed_note(&vault, url, &artifact_a, "A", "note a", "missing").unwrap();
        note::save_keyed_note(&vault, url, &artifact_b, "B", "note b", "missing").unwrap();

        let loaded_note = note_load(&mut config, url);
        assert_eq!(
            loaded_note.key.kind,
            RecordKeyKind::Url,
            "an ambiguous historical match must fall back to a fresh URL key, not guess"
        );
    }

    #[test]
    fn without_recorded_contracts_recorded_history_does_not_choose_a_note() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let occurred_at = time::OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let mut config = loaded(&path);
        let url = "https://work.example/items/7";

        // Record a real activity so the index remembers this URL belonged
        // to the artifact while the adapter was still enabled.
        let activity_response = record_activity(
            RecordActivityRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "activity-1".into(),
                event: ActivityEvent {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    artifact_id: "work:work.example/7".into(),
                    artifact_type: "work.item".into(),
                    url: url.into(),
                    title: Some("Issue 7".into()),
                    occurred_at,
                    incognito: false,
                    opens: 1,
                    focused_seconds: 3,
                    background_seconds: 0,
                    modes: Vec::new(),
                    trails: Vec::new(),
                    refs: Vec::new(),
                },
            },
            &mut config,
        );
        assert!(matches!(
            activity_response,
            Response::ActivityRecorded(ActivityRecorded {
                outcome: VisitOutcome::Persisted,
                ..
            })
        ));

        // Save the shared note while the adapter still recognizes the URL,
        // so it is keyed by the artifact, not the URL.
        let loaded_before = note_load(&mut config, url);
        assert_eq!(loaded_before.key.kind, RecordKeyKind::Artifact);
        let saved = match note_save(
            &mut config,
            url,
            "keep this text",
            &loaded_before.revision,
            loaded_before.key,
        ) {
            Response::NoteSaved(saved) => saved,
            other => panic!("expected note_saved, got {other:?}"),
        };

        // Rewrite config.toml with no adapter and no `[[adapter_id_history]]`,
        // as a hand edit or repair would. Recorded history alone is a stripped
        // canonical URL and cannot name the artifact, so the page gets its URL
        // note, and the shared note stays untouched on disk.
        write_adapter_config(&path, &root, "");
        assert_shared_note_is_kept_but_not_guessed(&mut config, &root, url, &saved);
    }

    fn assert_shared_note_is_kept_but_not_guessed(
        config: &mut ConfigStore,
        root: &std::path::Path,
        url: &str,
        shared: &NoteSaved,
    ) {
        let shared_path = root.join(&shared.relative_path);
        let shared_bytes = std::fs::read(&shared_path).unwrap();
        let loaded_after = note_load(config, url);
        assert_eq!(loaded_after.key.kind, RecordKeyKind::Url);
        assert!(!loaded_after.exists);
        let Response::NoteSaved(saved) = note_save(
            config,
            url,
            "a separate URL note",
            &loaded_after.revision,
            loaded_after.key,
        ) else {
            panic!("expected a separate URL note")
        };
        assert_ne!(saved.relative_path, shared.relative_path);
        assert_eq!(std::fs::read(shared_path).unwrap(), shared_bytes);
    }

    #[test]
    fn without_recorded_contracts_a_notes_stored_identity_does_not_choose_it() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let mut config = loaded(&path);
        let url = "https://work.example/items/7";

        // Save the shared note while the adapter still recognizes the URL,
        // but with no activity ever recorded for it (capture paused, or
        // simply not delivered yet) -- there is no index row, only the
        // note's own stored identity.
        let loaded_before = note_load(&mut config, url);
        assert_eq!(loaded_before.key.kind, RecordKeyKind::Artifact);
        let saved = match note_save(
            &mut config,
            url,
            "keep this text",
            &loaded_before.revision,
            loaded_before.key,
        ) {
            Response::NoteSaved(saved) => saved,
            other => panic!("expected note_saved, got {other:?}"),
        };

        // The note's stored creation URL is a stripped form too.
        write_adapter_config(&path, &root, "");
        assert_shared_note_is_kept_but_not_guessed(&mut config, &root, url, &saved);
    }

    #[test]
    fn jira_board_modal_matches_before_strip_params_removes_its_query_identity() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        config::fixtures::write(
            &path,
            &root,
            "artifacts_dir = \"artifacts\"",
            "capture_enabled = true\nsites = [{ origin = \"https://acme.atlassian.net\", path_prefix = \"/\" }]\nstrip_params = [\"selectedIssue\"]",
            r#"[adapters]
format = 1

[[artifacts]]
type = "jira.issue"
origin = "https://acme.atlassian.net"
match = [{ path = "/browse/{key:jira_key}" }, { path = "/jira/{*}", query = { selectedIssue = "{key:jira_key}" } }]
id = "jira:{host}/{key}"
"#,
        );
        let mut config = loaded(&path);
        let modal =
            "https://acme.atlassian.net/jira/software/projects/ABC/boards/1?selectedIssue=ABC-123";
        let browse = "https://acme.atlassian.net/browse/ABC-123";

        // strip_params removes `selectedIssue` from the stored canonical
        // URL, but adapter matching must still see it on the original URL,
        // the same way capture does, or this route would stop being
        // recognized as an artifact at all.
        let modal_loaded = note_load(&mut config, modal);
        assert_eq!(modal_loaded.key.kind, RecordKeyKind::Artifact);

        assert!(matches!(
            note_save(
                &mut config,
                modal,
                "same issue",
                &modal_loaded.revision,
                modal_loaded.key.clone()
            ),
            Response::NoteSaved(_)
        ));
        let browse_loaded = note_load(&mut config, browse);
        assert_eq!(browse_loaded.key, modal_loaded.key);
        assert_eq!(browse_loaded.body, "same issue");
    }

    fn older_note_urls(config: &mut ConfigStore, url: &str) -> Vec<String> {
        note_load(config, url)
            .older_notes
            .into_iter()
            .map(|note| note.canonical_url)
            .collect()
    }

    #[test]
    fn a_url_the_current_policy_names_another_artifact_is_not_an_older_note() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        // A recognition edit that kept the type and ID template but rebound
        // the captures leaves the old shape in `[[adapter_id_history]]`.
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}/edit" }, { path = "/items/{id:int}/view" }]
id = "work:{host}/{id}"

[[adapter_id_history]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{*}", query = { item = "{id:int}" } }]
id = "work:{host}/{id}"
"#,
        );
        let mut config = loaded(&path);
        assert_eq!(config.adapter_issue(), None);
        let shared = "https://work.example/items/7/edit?item=8";
        assert_eq!(
            config.adapter_match(shared, None).unwrap().artifact_id,
            "work:work.example/7"
        );
        let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
        note::save_note(&vault, shared, "Item 7 edit", "Item 7 text", "missing").unwrap();
        let revision = note::load_note(&vault, shared).unwrap().revision;

        assert_eq!(
            older_note_urls(&mut config, "https://work.example/items/7/view"),
            vec![shared.to_owned()]
        );
        let other = "https://work.example/items/8/view";
        assert!(older_note_urls(&mut config, other).is_empty());
        assert!(matches!(
            use_artifact_note(
                brauser_protocol::UseArtifactNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "migrate-other".into(),
                    url: other.into(),
                    source_url: shared.into(),
                    expected_revision: revision,
                },
                &mut config,
            ),
            Response::Error(reply) if reply.code == ErrorCode::Unauthorized
        ));
        assert_eq!(note::load_note(&vault, shared).unwrap().body, "Item 7 text");
    }

    #[test]
    fn recorded_routes_on_an_origin_without_contracts_are_not_older_notes() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let mut config = loaded(&path);
        let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
        // The artifact note was created on an origin whose contract a config
        // repair has since discarded, beside an older URL note there.
        let old_origin = "http://work.example/items/7";
        let key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:work.example/7".into(),
        };
        note::save_keyed_note(&vault, old_origin, &key, "Item 7", "Shared", "missing").unwrap();
        note::save_note(&vault, old_origin, "Before adapters", "Separate", "missing").unwrap();
        assert_eq!(
            config.note_key_rule(old_origin),
            crate::config::NoteKeyRule::Uncovered
        );

        // The covered route keeps the shared note, but its creation URL on the
        // uncovered origin is a stripped form that could be another page, so
        // that origin's URL note is neither listed nor migratable here. It
        // stays reachable on its own page.
        let current_url = "https://work.example/items/7";
        let current = note_load(&mut config, current_url);
        assert_eq!(current.key, key);
        assert_eq!(current.body, "Shared");
        assert!(current.older_notes.is_empty(), "{:?}", current.older_notes);
        let revision = note::load_note(&vault, old_origin).unwrap().revision;
        assert!(matches!(
            use_artifact_note(
                brauser_protocol::UseArtifactNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "migrate-uncovered".into(),
                    url: current_url.into(),
                    source_url: old_origin.into(),
                    expected_revision: revision,
                },
                &mut config,
            ),
            Response::Error(reply) if reply.code == ErrorCode::Unauthorized
        ));
        let own = note_load(&mut config, old_origin);
        assert_eq!(own.key.kind, RecordKeyKind::Url);
        assert_eq!(own.body, "Separate");
    }

    /// `strip_params` can remove the query value that is a Jira board modal's
    /// whole artifact identity, so the stored canonical URL of the modal's
    /// note is the bare board URL. That shared canonical form is never
    /// evidence that the board, or another issue's modal, is the same page.
    mod stripped_identity_collisions {
        use super::*;

        const BOARD: &str = "https://acme.atlassian.net/jira/software/projects/ABC/boards/1";
        const ABC_123: &str =
            "https://acme.atlassian.net/jira/software/projects/ABC/boards/1?selectedIssue=ABC-123";
        const ABC_456: &str =
            "https://acme.atlassian.net/jira/software/projects/ABC/boards/1?selectedIssue=ABC-456";
        const BROWSE_123: &str = "https://acme.atlassian.net/browse/ABC-123";
        const JIRA: &str = r#"[[artifacts]]
type = "jira.issue"
origin = "https://acme.atlassian.net"
match = [{ path = "/browse/{key:jira_key}" }, { path = "/jira/{*}", query = { selectedIssue = "{key:jira_key}" } }]
id = "jira:{host}/{key}"
"#;

        fn jira_store(folder: &tempfile::TempDir) -> (std::path::PathBuf, ConfigStore) {
            let root = folder.path().join("notes");
            std::fs::create_dir(&root).unwrap();
            let path = folder.path().join("config.toml");
            config::fixtures::write(
                &path,
                &root,
                "artifacts_dir = \"artifacts\"",
                "capture_enabled = true\nsites = [{ origin = \"https://acme.atlassian.net\", path_prefix = \"/\" }]\nstrip_params = [\"selectedIssue\"]",
                &format!("[adapters]\nformat = 1\n\n{JIRA}"),
            );
            let config = loaded(&path);
            (root, config)
        }

        fn save(config: &mut ConfigStore, url: &str, body: &str) -> NoteSaved {
            let current = note_load(config, url);
            match note_save(config, url, body, &current.revision, current.key) {
                Response::NoteSaved(saved) => saved,
                other => panic!("expected note_saved on {url}, got {other:?}"),
            }
        }

        fn record_modal_activity(config: &mut ConfigStore, url: &str, issue: &str) {
            let occurred_at = time::OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            let response = record_activity(
                RecordActivityRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "modal-activity".into(),
                    event: ActivityEvent {
                        event_id: uuid::Uuid::new_v4().to_string(),
                        artifact_id: format!("jira:acme.atlassian.net/{issue}"),
                        artifact_type: "jira.issue".into(),
                        url: url.into(),
                        title: Some(issue.into()),
                        occurred_at,
                        incognito: false,
                        opens: 1,
                        focused_seconds: 3,
                        background_seconds: 0,
                        modes: Vec::new(),
                        trails: Vec::new(),
                        refs: Vec::new(),
                    },
                },
                config,
            );
            assert!(
                matches!(
                    response,
                    Response::ActivityRecorded(ActivityRecorded {
                        outcome: VisitOutcome::Persisted,
                        ..
                    })
                ),
                "{response:?}"
            );
        }

        fn older(config: &mut ConfigStore, url: &str) -> Vec<String> {
            note_load(config, url)
                .older_notes
                .into_iter()
                .map(|note| note.canonical_url)
                .collect()
        }

        #[test]
        fn the_bare_board_never_reads_or_autosaves_into_a_modal_issues_note() {
            let folder = tempfile::tempdir().unwrap();
            let (root, mut config) = jira_store(&folder);
            let issue = save(&mut config, ABC_123, "ABC-123 text");
            let issue_path = root.join(&issue.relative_path);
            let issue_bytes = std::fs::read(&issue_path).unwrap();

            for activity in [false, true] {
                if activity {
                    // Delivered modal history for one issue, then another,
                    // must not flip the board's key either way.
                    record_modal_activity(&mut config, ABC_123, "ABC-123");
                }
                let board = note_load(&mut config, BOARD);
                assert_eq!(board.key.kind, RecordKeyKind::Url, "activity={activity}");
                assert!(!board.exists, "activity={activity}");
                assert!(board.body.is_empty(), "activity={activity}");
            }
            let board = save(&mut config, BOARD, "Notes about the whole board");
            assert_eq!(board.key.kind, RecordKeyKind::Url);
            assert_ne!(board.relative_path, issue.relative_path);
            assert_eq!(std::fs::read(&issue_path).unwrap(), issue_bytes);

            record_modal_activity(&mut config, ABC_456, "ABC-456");
            let board = note_load(&mut config, BOARD);
            assert_eq!(board.key.kind, RecordKeyKind::Url);
            assert_eq!(board.body, "Notes about the whole board");
            assert_eq!(note_load(&mut config, ABC_123).body, "ABC-123 text");
            assert_eq!(note_load(&mut config, BROWSE_123).body, "ABC-123 text");
        }

        #[test]
        fn after_adapter_removal_another_issues_modal_does_not_load_the_first_issues_note() {
            let folder = tempfile::tempdir().unwrap();
            let (root, mut config) = jira_store(&folder);
            let issue = save(&mut config, ABC_123, "ABC-123 text");
            let issue_bytes = std::fs::read(root.join(&issue.relative_path)).unwrap();
            let mut next = config.snapshot().clone();
            next.adapters.artifacts.clear();
            let revision = config.revision().to_owned();
            config.update(next, &revision).unwrap().unwrap();

            let other = note_load(&mut config, ABC_456);
            assert_ne!(other.body, "ABC-123 text");
            assert!(
                other.key.value != issue.key.value,
                "ABC-456 must not resolve to ABC-123's key: {:?}",
                other.key
            );
            let board = note_load(&mut config, BOARD);
            assert_eq!(board.key.kind, RecordKeyKind::Url);
            assert!(!board.exists);
            // The issue's own routes keep its note.
            assert_eq!(note_load(&mut config, ABC_123).body, "ABC-123 text");
            assert_eq!(note_load(&mut config, BROWSE_123).body, "ABC-123 text");
            assert_eq!(
                std::fs::read(root.join(&issue.relative_path)).unwrap(),
                issue_bytes
            );
        }

        #[test]
        fn a_board_url_note_is_not_an_older_note_of_any_issue_in_its_modal() {
            let folder = tempfile::tempdir().unwrap();
            let (root, mut config) = jira_store(&folder);
            let board = save(&mut config, BOARD, "Notes about the whole board");
            assert_eq!(board.key.kind, RecordKeyKind::Url);
            let board_path = root.join(&board.relative_path);
            let board_bytes = std::fs::read(&board_path).unwrap();
            let board_revision = note_load(&mut config, BOARD).revision;

            for modal in [ABC_456, ABC_123] {
                assert!(older(&mut config, modal).is_empty(), "{modal}");
            }
            record_modal_activity(&mut config, ABC_123, "ABC-123");
            assert!(older(&mut config, BROWSE_123).is_empty());
            assert!(older(&mut config, ABC_123).is_empty());

            for url in [ABC_456, ABC_123, BROWSE_123] {
                let viewed = view_older_note(
                    brauser_protocol::ViewOlderNoteRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "view-board".into(),
                        url: url.into(),
                        source_url: BOARD.into(),
                    },
                    &mut config,
                );
                assert!(
                    matches!(&viewed, Response::Error(reply) if reply.code == ErrorCode::Unauthorized),
                    "{url}: {viewed:?}"
                );
                let migrated = use_artifact_note(
                    brauser_protocol::UseArtifactNoteRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "migrate-board".into(),
                        url: url.into(),
                        source_url: BOARD.into(),
                        expected_revision: board_revision.clone(),
                    },
                    &mut config,
                );
                assert!(
                    matches!(&migrated, Response::Error(reply) if reply.code == ErrorCode::Unauthorized),
                    "{url}: {migrated:?}"
                );
            }
            assert_eq!(std::fs::read(&board_path).unwrap(), board_bytes);
            assert_eq!(
                note_load(&mut config, BOARD).body,
                "Notes about the whole board"
            );
        }

        /// Removes the Jira adapter and then `[[adapter_id_history]]`, as a
        /// config repair or an install older than that history would, so no
        /// recorded contract covers the origin any more.
        fn forget_jira_contracts(folder: &tempfile::TempDir, config: &mut ConfigStore) {
            let mut next = config.snapshot().clone();
            next.adapters.artifacts.clear();
            let revision = config.revision().to_owned();
            config.update(next, &revision).unwrap().unwrap();
            let path = folder.path().join("config.toml");
            let mut table: toml::Table =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(table.remove("adapter_id_history").is_some());
            std::fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
            *config = loaded(&path);
            assert_eq!(
                config.note_key_rule(ABC_123),
                crate::config::NoteKeyRule::Uncovered
            );
        }

        #[test]
        fn without_recorded_contracts_the_board_and_another_modal_never_take_an_issues_note() {
            for activity in [false, true] {
                let folder = tempfile::tempdir().unwrap();
                let (root, mut config) = jira_store(&folder);
                let issue = save(&mut config, ABC_123, "ABC-123 text");
                if activity {
                    record_modal_activity(&mut config, ABC_123, "ABC-123");
                }
                let issue_path = root.join(&issue.relative_path);
                let issue_bytes = std::fs::read(&issue_path).unwrap();
                forget_jira_contracts(&folder, &mut config);

                // The stripped canonical form is shared by the board and
                // every modal on it, so it never names ABC-123 by itself.
                for url in [BOARD, ABC_456] {
                    let page = note_load(&mut config, url);
                    assert_eq!(page.key.kind, RecordKeyKind::Url, "{activity}/{url}");
                    assert!(!page.exists, "{activity}/{url}");
                    assert!(page.body.is_empty(), "{activity}/{url}");
                }
                let other = save(&mut config, ABC_456, "ABC-456 text");
                assert_eq!(other.outcome, brauser_protocol::NoteSaveOutcome::Created);
                assert_ne!(other.relative_path, issue.relative_path);
                assert_eq!(std::fs::read(&issue_path).unwrap(), issue_bytes);
            }
        }

        #[test]
        fn without_recorded_contracts_a_board_note_cannot_migrate_into_a_modal_issue() {
            let folder = tempfile::tempdir().unwrap();
            let (root, mut config) = jira_store(&folder);
            let board = save(&mut config, BOARD, "Notes about the whole board");
            let board_path = root.join(&board.relative_path);
            let board_bytes = std::fs::read(&board_path).unwrap();
            record_modal_activity(&mut config, ABC_123, "ABC-123");
            forget_jira_contracts(&folder, &mut config);

            let loaded_board = note_load(&mut config, BOARD);
            assert_eq!(loaded_board.key, board.key);
            assert_eq!(loaded_board.body, "Notes about the whole board");
            for url in [ABC_456, ABC_123] {
                let migrated = use_artifact_note(
                    brauser_protocol::UseArtifactNoteRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "migrate-board".into(),
                        url: url.into(),
                        source_url: BOARD.into(),
                        expected_revision: loaded_board.revision.clone(),
                    },
                    &mut config,
                );
                assert!(
                    matches!(&migrated, Response::Error(_)),
                    "{url}: {migrated:?}"
                );
            }
            assert_eq!(std::fs::read(&board_path).unwrap(), board_bytes);
        }

        /// endurance-22 case 1: notes and activity were recorded while
        /// `selectedIssue` was stripped, so ABC-123's stored canonical URL is
        /// the bare board. Dropping it from `strip_params` through settings
        /// must not let the board read or autosave over ABC-123's note.
        #[test]
        fn unstripping_an_identity_parameter_keeps_the_board_off_an_issues_note() {
            for activity in [false, true] {
                let folder = tempfile::tempdir().unwrap();
                let (root, mut config) = jira_store(&folder);
                let issue = save(&mut config, ABC_123, "ABC-123 text");
                if activity {
                    record_modal_activity(&mut config, ABC_123, "ABC-123");
                }
                let issue_path = root.join(&issue.relative_path);
                let issue_bytes = std::fs::read(&issue_path).unwrap();

                let mut next = config.snapshot().clone();
                next.strip_params.retain(|name| name != "selectedIssue");
                let revision = config.revision().to_owned();
                config.update(next, &revision).unwrap().unwrap();

                let board = note_load(&mut config, BOARD);
                assert_ne!(board.key, issue.key, "activity={activity}");
                assert_ne!(board.body, "ABC-123 text", "activity={activity}");
                let saved = save(&mut config, BOARD, "Notes about the whole board");
                assert_ne!(
                    saved.relative_path, issue.relative_path,
                    "activity={activity}"
                );
                assert_eq!(
                    std::fs::read(&issue_path).unwrap(),
                    issue_bytes,
                    "activity={activity}"
                );
                assert_eq!(note_load(&mut config, ABC_123).body, "ABC-123 text");
                assert_eq!(
                    note_load(&mut config, BOARD).body,
                    "Notes about the whole board"
                );
            }
        }

        const JIRA_BOARD: &str = r#"[[artifacts]]
type = "jira.board"
origin = "https://acme.atlassian.net"
match = [{ path = "/jira/software/projects/ABC/boards/{id:int}", absent = ["selectedIssue"] }]
id = "jira.board:{host}/{id}"
"#;

        /// endurance-22 case 2: a live jira.issue adapter whose modal rule
        /// reads the stripped `selectedIssue`, beside a jira.board adapter.
        /// Removing only jira.board by hand must keep the board's note
        /// reachable, and the next autosave must update it, not split it.
        #[test]
        #[ignore = "brwr-hrc: hand-removed contract recovery, follow-up (B)"]
        fn hand_removing_only_the_board_adapter_keeps_its_note_beside_the_modal_rule() {
            let folder = tempfile::tempdir().unwrap();
            let root = folder.path().join("notes");
            std::fs::create_dir(&root).unwrap();
            let path = folder.path().join("config.toml");
            config::fixtures::write(
                &path,
                &root,
                "artifacts_dir = \"artifacts\"",
                "capture_enabled = true\nstrip_params = [\"selectedIssue\"]",
                &format!("[adapters]\nformat = 1\n\n{JIRA}\n{JIRA_BOARD}"),
            );
            let mut config = loaded(&path);
            let issue = save(&mut config, ABC_123, "ABC-123 text");
            let board = save(&mut config, BOARD, "board artifact note");
            assert_eq!(board.key.kind, RecordKeyKind::Artifact);
            assert_eq!(board.key.value, "jira.board:acme.atlassian.net/1");
            let issue_path = root.join(&issue.relative_path);
            let issue_bytes = std::fs::read(&issue_path).unwrap();

            let mut table: toml::Table =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let artifacts = table
                .get_mut("artifacts")
                .and_then(toml::Value::as_array_mut)
                .expect("top-level [[artifacts]]");
            let before = artifacts.len();
            artifacts.retain(|rule| {
                rule.get("type").and_then(toml::Value::as_str) != Some("jira.board")
            });
            assert_eq!(artifacts.len(), before - 1);
            std::fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
            config = loaded(&path);

            let loaded_board = note_load(&mut config, BOARD);
            assert!(loaded_board.exists, "{loaded_board:?}");
            assert_eq!(loaded_board.body, "board artifact note");
            let Response::NoteSaved(saved) = note_save(
                &mut config,
                BOARD,
                "board artifact note, edited",
                &loaded_board.revision,
                loaded_board.key,
            ) else {
                panic!("expected the board's autosave to succeed")
            };
            assert_eq!(saved.relative_path, board.relative_path);
            assert_eq!(saved.outcome, brauser_protocol::NoteSaveOutcome::Replaced);
            assert_eq!(
                note_load(&mut config, BOARD).body,
                "board artifact note, edited"
            );
            assert_eq!(std::fs::read(&issue_path).unwrap(), issue_bytes);
            assert_eq!(note_load(&mut config, ABC_123).body, "ABC-123 text");
        }
    }

    /// A page on an origin no recorded contract covers needs no scan of the
    /// home folder, so its note works however many notes the folder holds.
    #[test]
    fn a_large_notes_folder_does_not_block_notes_on_uncovered_pages() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(&path, &root, "");
        let mut config = loaded(&path);
        let url = "https://news.example/a";
        let first = note_load(&mut config, url);
        let Response::NoteSaved(saved) =
            note_save(&mut config, url, "News text", &first.revision, first.key)
        else {
            panic!("expected note_saved")
        };
        let pages = root.join(saved.relative_path).parent().unwrap().to_owned();
        for number in 0..4097 {
            std::fs::write(pages.join(format!("000-{number:04}.md")), b"").unwrap();
        }
        let current = note_load(&mut config, url);
        assert_eq!(current.body, "News text");
        assert!(matches!(
            note_save(
                &mut config,
                url,
                "Still saving",
                &current.revision,
                current.key
            ),
            Response::NoteSaved(_)
        ));
    }

    mod note_policy_removal {
        use super::*;

        const EDIT: &str = "https://work.example/items/7/edit";
        const VIEW: &str = "https://work.example/items/7/view";
        const CONTEXT: &str = "https://work.example/items/7/context";

        struct Session {
            folder: tempfile::TempDir,
            root: std::path::PathBuf,
            path: std::path::PathBuf,
            // Capture rejects events more than 30 days old, so fixtures are
            // dated from the clock rather than a literal.
            at: time::OffsetDateTime,
        }

        impl Session {
            fn new() -> Self {
                let folder = tempfile::tempdir().unwrap();
                let root = folder.path().join("notes");
                std::fs::create_dir(&root).unwrap();
                let path = folder.path().join("config.toml");
                write_adapter_config(
                    &path,
                    &root,
                    r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}/edit", mode = "edit" }, { path = "/items/{id:int}/view", mode = "view" }, { path = "/items/{id:int}/context", mode = "view" }, { path = "/items/{id:int}/retained", mode = "view" }]
id = "work:{host}/{id}"
"#,
                );
                let at = time::OffsetDateTime::now_utc()
                    .replace_nanosecond(0)
                    .unwrap();
                Self {
                    folder,
                    root,
                    path,
                    at,
                }
            }

            fn occurred_at(&self) -> String {
                self.at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap()
            }

            fn log_path(&self) -> std::path::PathBuf {
                let date = self.at.date();
                self.root.join(format!(
                    "logs/{:04}/{:02}/{:04}-{:02}-{:02}.md",
                    date.year(),
                    u8::from(date.month()),
                    date.year(),
                    u8::from(date.month()),
                    date.day()
                ))
            }

            // Exercise real request/response framing and reopen the host for
            // every request, so config history and recovery must survive EOF.
            fn call(&self, request: Request) -> Response {
                let mut output = Vec::new();
                serve_with_context(
                    Cursor::new(encoded_request(request)),
                    &mut output,
                    loaded(&self.path),
                    &SetupContext {
                        env: &no_harnesses(self.folder.path()),
                    },
                )
                .unwrap();
                decoded_response(output)
            }

            fn load(&self, url: &str) -> NoteLoaded {
                match self.call(Request::LoadNote(brauser_protocol::LoadNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "load-removal".into(),
                    url: url.into(),
                })) {
                    Response::NoteLoaded(note) => note,
                    other => panic!("expected note_loaded, got {other:?}"),
                }
            }

            fn save(&self, url: &str, note: &NoteLoaded, body: &str) -> Response {
                self.call(Request::SaveNote(brauser_protocol::SaveNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "save-removal".into(),
                    url: url.into(),
                    title: "Shared note".into(),
                    body: body.into(),
                    expected_revision: note.revision.clone(),
                    expected_key: note.key.clone(),
                }))
            }

            fn policy(&self, removal: &str) {
                let config = loaded(&self.path);
                let mut next = config.snapshot().clone();
                match removal {
                    "adapter" => next.adapters.artifacts.clear(),
                    "site" => {
                        // Sites are derived from adapters; removing the
                        // site means removing its adapters.
                        next.adapters.artifacts.clear();
                        next.capture_enabled = false;
                    }
                    "narrow" => {
                        next.adapters.artifacts[0]["match"] = serde_json::json!([{ "path": "/items/{id:int}/retained", "mode": "view" }]);
                    }
                    _ => panic!("unknown removal"),
                }
                // Open access: adapter edits and removals need no native
                // confirmation, so every removal goes through the real
                // request/response framing.
                let response = self.call(Request::UpdateConfig(UpdateConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "remove-policy".into(),
                    expected_revision: config.revision().into(),
                    config: next,
                    picker_token: None,
                    knowledge_base_token: None,
                }));
                assert!(
                    matches!(response, Response::ConfigUpdated(_)),
                    "{response:?}"
                );
            }

            /// A config repair or hand edit can discard `[[adapter_id_history]]`;
            /// the origin is then uncovered and its pages use URL keys.
            fn forget_policy_history(&self) {
                let mut table: toml::Table =
                    toml::from_str(&std::fs::read_to_string(&self.path).unwrap()).unwrap();
                assert!(table.remove("adapter_id_history").is_some());
                std::fs::write(&self.path, toml::to_string(&table).unwrap()).unwrap();
                let config = loaded(&self.path);
                assert!(config.config_issue().is_none());
                assert_eq!(
                    config.note_key_rule(EDIT),
                    crate::config::NoteKeyRule::Uncovered
                );
            }

            fn activity(&self) -> ActivityEvent {
                ActivityEvent {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    artifact_id: "work:work.example/7".into(),
                    artifact_type: "work.item".into(),
                    url: VIEW.into(),
                    title: Some("Item 7".into()),
                    occurred_at: self.occurred_at(),
                    incognito: false,
                    opens: 1,
                    focused_seconds: 3,
                    background_seconds: 0,
                    modes: vec![ActivityMode {
                        mode: "edit".into(),
                        url: EDIT.into(),
                    }],
                    trails: Vec::new(),
                    refs: Vec::new(),
                }
            }

            fn index(&self) -> ArtifactIndex {
                let config = loaded(&self.path);
                ArtifactIndex::open_in(
                    config.snapshot().storage.as_ref().unwrap(),
                    config.root_identity(),
                    &config.cache_dir().unwrap(),
                )
                .unwrap()
            }
        }

        #[test]
        fn removal_keeps_shared_and_older_route_notes_separate_across_rebuilds() {
            for removal in ["adapter", "narrow", "site"] {
                let session = Session::new();
                let config = loaded(&session.path);
                let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
                // Separate URL notes already occupy both known routes.
                for url in [EDIT, VIEW] {
                    note::save_note(&vault, url, "Before adapters", url, "missing").unwrap();
                }
                let first = session.load(VIEW);
                assert_eq!(first.key.kind, RecordKeyKind::Artifact);
                let Response::NoteSaved(saved) = session.save(VIEW, &first, "Shared artifact text")
                else {
                    panic!("expected new shared artifact note")
                };
                let shared = session.load(VIEW);
                assert_eq!(shared.older_notes.len(), 2);
                let event = session.activity();
                let response = session.call(Request::RecordActivity(RecordActivityRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "remember-routes".into(),
                    event: event.clone(),
                }));
                assert!(
                    matches!(
                        response,
                        Response::ActivityRecorded(ActivityRecorded {
                            outcome: VisitOutcome::Persisted,
                            ..
                        })
                    ),
                    "{response:?}"
                );
                session.policy(removal);
                let shared_path = session.root.join(saved.relative_path);
                let original = std::fs::read(&shared_path).unwrap();
                let log_path = session.log_path();
                let original_log = std::fs::read(&log_path).unwrap();
                for rebuild in ["existing", "deleted", "reindex"] {
                    if rebuild == "deleted" {
                        std::fs::remove_dir_all(config.cache_dir().unwrap()).unwrap();
                    } else if rebuild == "reindex" {
                        session.index().reindex().unwrap();
                    }
                    for url in [EDIT, VIEW] {
                        let recovered = session.load(url);
                        assert_eq!(recovered.key, shared.key, "{removal}/{rebuild}/{url}");
                        assert_eq!(recovered.body, "Shared artifact text");
                        assert_eq!(recovered.revision, shared.revision);
                        assert_eq!(
                            recovered.older_notes.len(),
                            2,
                            "{removal}/{rebuild}/{url}: both separate URL notes must stay listed"
                        );
                        for older in &recovered.older_notes {
                            let viewed = session.call(Request::ViewOlderNote(
                                brauser_protocol::ViewOlderNoteRequest {
                                    protocol_version: PROTOCOL_VERSION,
                                    request_id: "view-separate".into(),
                                    url: url.into(),
                                    source_url: older.canonical_url.clone(),
                                },
                            ));
                            assert!(
                                matches!(viewed, Response::NoteLoaded(note) if note.body == older.canonical_url)
                            );
                            // An explicit migration also cannot clobber the
                            // existing shared note or delete its source.
                            assert!(
                                matches!(session.call(Request::UseArtifactNote(brauser_protocol::UseArtifactNoteRequest {
                                protocol_version: PROTOCOL_VERSION,
                                request_id: "no-merge".into(),
                                url: url.into(),
                                source_url: older.canonical_url.clone(),
                                expected_revision: older.revision.clone(),
                            })), Response::Error(reply) if reply.code == ErrorCode::Conflict)
                            );
                            assert_eq!(
                                note::load_note(&vault, &older.canonical_url)
                                    .unwrap()
                                    .revision,
                                older.revision
                            );
                        }
                        // A stale URL key must not redirect an autosave.
                        let wrong_key = NoteLoaded {
                            key: crate::vault::url_record_key(url).unwrap(),
                            ..recovered
                        };
                        assert!(
                            matches!(session.save(url, &wrong_key, "Keep this copy-out"),
                            Response::NoteConflict(reply) if reply.reason == "key_changed" && reply.body == "Shared artifact text")
                        );
                    }
                    assert_eq!(std::fs::read(&shared_path).unwrap(), original);
                    let response = session.call(Request::RecordActivity(RecordActivityRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "must-stay-off".into(),
                        event: ActivityEvent {
                            event_id: uuid::Uuid::new_v4().to_string(),
                            ..event.clone()
                        },
                    }));
                    assert!(
                        matches!(response, Response::ActivityRecorded(reply) if reply.outcome == VisitOutcome::Suppressed)
                    );
                    assert_eq!(std::fs::read(&log_path).unwrap(), original_log);
                }
                assert!(matches!(
                    session.save(EDIT, &shared, "Edit after removal"),
                    Response::NoteSaved(_)
                ));
                assert_eq!(session.load(VIEW).body, "Edit after removal");
            }
        }

        #[test]
        fn context_only_routes_recover_the_note_without_creating_activity() {
            let session = Session::new();
            let first = session.load(VIEW);
            assert!(matches!(
                session.save(VIEW, &first, "Note with context only"),
                Response::NoteSaved(_)
            ));
            let shared = session.load(VIEW);
            assert!(matches!(
                session.call(Request::RecordWhy(brauser_protocol::RecordWhyRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "context-route".into(),
                    event: brauser_protocol::WhyEvent {
                        event_id: uuid::Uuid::new_v4().to_string(),
                        artifact_id: shared.key.value.clone(),
                        url: CONTEXT.into(),
                        occurred_at: session.occurred_at(),
                        incognito: false,
                        text: "Preparing the review".into(),
                    },
                })),
                Response::WhyRecorded(WhyRecorded {
                    outcome: VisitOutcome::Persisted,
                    ..
                })
            ));
            session.policy("adapter");
            for rebuild in [false, true] {
                if rebuild {
                    std::fs::remove_dir_all(loaded(&session.path).cache_dir().unwrap()).unwrap();
                    session.index().reindex().unwrap();
                }
                let recovered = session.load(CONTEXT);
                assert_eq!(recovered.key, shared.key);
                assert_eq!(recovered.body, shared.body);
                assert!(
                    session
                        .index()
                        .artifact(&shared.key.value)
                        .unwrap()
                        .is_none(),
                    "note recovery cannot invent measured activity"
                );
            }
        }

        #[test]
        fn recovered_routes_keep_the_older_note_on_the_shared_notes_creation_url() {
            for removal in ["adapter", "narrow", "site"] {
                let session = Session::new();
                let config = loaded(&session.path);
                let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
                note::save_note(
                    &vault,
                    VIEW,
                    "Before adapters",
                    "Separate URL text",
                    "missing",
                )
                .unwrap();
                let first = session.load(VIEW);
                assert!(matches!(
                    session.save(VIEW, &first, "Shared artifact text"),
                    Response::NoteSaved(_)
                ));
                let shared = session.load(VIEW);
                assert_eq!(shared.older_notes.len(), 1);
                assert!(matches!(
                    session.call(Request::RecordWhy(brauser_protocol::RecordWhyRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "remember-context-route".into(),
                        event: brauser_protocol::WhyEvent {
                            event_id: uuid::Uuid::new_v4().to_string(),
                            artifact_id: shared.key.value.clone(),
                            url: CONTEXT.into(),
                            occurred_at: session.occurred_at(),
                            incognito: false,
                            text: "Preparing the review".into(),
                        },
                    })),
                    Response::WhyRecorded(WhyRecorded {
                        outcome: VisitOutcome::Persisted,
                        ..
                    })
                ));
                session.policy(removal);
                for rebuild in [false, true] {
                    if rebuild {
                        std::fs::remove_dir_all(config.cache_dir().unwrap()).unwrap();
                        session.index().reindex().unwrap();
                    }
                    let recovered = session.load(CONTEXT);
                    assert_eq!(recovered.key, shared.key);
                    assert_eq!(recovered.revision, shared.revision);
                    assert_eq!(recovered.older_notes, shared.older_notes, "{removal}");
                    assert!(matches!(
                        session.call(Request::ViewOlderNote(brauser_protocol::ViewOlderNoteRequest {
                            protocol_version: PROTOCOL_VERSION,
                            request_id: "view-creation-url".into(),
                            url: CONTEXT.into(),
                            source_url: VIEW.into(),
                        })),
                        Response::NoteLoaded(note) if note.body == "Separate URL text"
                    ));
                    assert_eq!(
                        note::load_note(&vault, VIEW).unwrap().body,
                        "Separate URL text"
                    );
                }
            }
        }

        #[test]
        fn lost_contracts_never_authorize_unrelated_url_notes() {
            let session = Session::new();
            let config = loaded(&session.path);
            let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
            let first = session.load(EDIT);
            assert!(matches!(
                session.save(EDIT, &first, "Shared artifact text"),
                Response::NoteSaved(_)
            ));
            let unrelated = "https://unrelated.example/page";
            note::save_note(&vault, unrelated, "Unrelated", "Unrelated text", "missing").unwrap();
            let unrelated_revision = note::load_note(&vault, unrelated).unwrap().revision;
            session.policy("adapter");
            session.forget_policy_history();

            // Recorded history no longer chooses the key, so EDIT is a URL
            // page and cannot list, view, or absorb any older note.
            let recovered = session.load(EDIT);
            assert_eq!(recovered.key.kind, RecordKeyKind::Url);
            assert!(recovered.body.is_empty());
            assert!(recovered.older_notes.is_empty());
            assert!(matches!(
                session.call(Request::ViewOlderNote(brauser_protocol::ViewOlderNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "view-unrelated".into(),
                    url: EDIT.into(),
                    source_url: unrelated.into(),
                })),
                Response::Error(reply) if reply.code == ErrorCode::Conflict
            ));
            assert!(matches!(
                session.call(Request::UseArtifactNote(brauser_protocol::UseArtifactNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "migrate-unrelated".into(),
                    url: EDIT.into(),
                    source_url: unrelated.into(),
                    expected_revision: unrelated_revision,
                })),
                Response::Error(reply) if reply.code == ErrorCode::Conflict
            ));
            assert_eq!(
                note::load_note(&vault, unrelated).unwrap().body,
                "Unrelated text"
            );
        }

        #[test]
        fn a_note_without_delivered_history_survives_each_policy_removal() {
            for removal in ["adapter", "narrow", "site"] {
                let session = Session::new();
                let first = session.load(VIEW);
                assert!(matches!(
                    session.save(VIEW, &first, "No activity required"),
                    Response::NoteSaved(_)
                ));
                let saved = session.load(VIEW);
                session.policy(removal);
                std::fs::remove_dir_all(loaded(&session.path).cache_dir().unwrap()).unwrap();
                let recovered = session.load(VIEW);
                assert_eq!(recovered.key, saved.key);
                assert_eq!(recovered.body, saved.body);
                assert_eq!(recovered.revision, saved.revision);
                assert!(matches!(
                    session.save(VIEW, &recovered, "Still editable"),
                    Response::NoteSaved(_)
                ));
                assert!(
                    matches!(session.save(VIEW, &saved, "Stale text for copy-out"),
                    Response::NoteConflict(reply) if reply.reason == "stale" && reply.body == "Still editable")
                );
                assert!(
                    !session.root.join("logs").exists(),
                    "notes cannot restart capture"
                );
            }
        }

        #[test]
        fn turning_a_site_off_does_not_split_a_note_across_routes_without_history() {
            for removal in ["adapter", "narrow", "site"] {
                let session = Session::new();
                // The note exists only on its creation route; nothing was
                // ever recorded for the other route.
                let first = session.load(EDIT);
                assert!(matches!(
                    session.save(EDIT, &first, "One note for item 7"),
                    Response::NoteSaved(_)
                ));
                let shared = session.load(EDIT);
                session.policy(removal);
                let view = session.load(VIEW);
                assert_eq!(view.key, shared.key, "{removal}");
                assert_eq!(view.body, "One note for item 7", "{removal}");
                // A document first noted after the removal still gets one note.
                let new_edit = "https://work.example/items/8/edit";
                let new_view = "https://work.example/items/8/view";
                let created = session.load(new_edit);
                assert_eq!(created.key.kind, RecordKeyKind::Artifact, "{removal}");
                assert!(matches!(
                    session.save(new_edit, &created, "Item 8"),
                    Response::NoteSaved(_)
                ));
                let other_route = session.load(new_view);
                assert_eq!(other_route.key, created.key, "{removal}");
                assert_eq!(other_route.body, "Item 8", "{removal}");
                assert!(
                    !session.root.join("logs").exists(),
                    "{removal}: note keys never restart capture"
                );
            }
        }

        #[test]
        fn an_unsupported_adapter_table_key_does_not_split_a_note() {
            let session = Session::new();
            let first = session.load(EDIT);
            assert!(matches!(
                session.save(EDIT, &first, "One note for item 7"),
                Response::NoteSaved(_)
            ));
            let shared = session.load(EDIT);
            let text = std::fs::read_to_string(&session.path).unwrap();
            let edited = text.replacen("[adapters]\n", "[adapters]\nfuture_key = true\n", 1);
            assert_ne!(edited, text, "fixture must contain an adapters table");
            std::fs::write(&session.path, edited).unwrap();
            assert!(loaded(&session.path).adapter_issue().is_some());

            let view = session.load(VIEW);
            assert_eq!(view.key, shared.key);
            assert_eq!(view.body, "One note for item 7");
        }

        #[test]
        fn an_unsupported_adapter_format_refuses_note_identity_on_its_origins() {
            let session = Session::new();
            let first = session.load(EDIT);
            let Response::NoteSaved(saved) = session.save(EDIT, &first, "Keep original text")
            else {
                panic!("expected saved note")
            };
            let current = session.load(EDIT);
            let note_path = session.root.join(saved.relative_path);
            let original = std::fs::read(&note_path).unwrap();
            let text = std::fs::read_to_string(&session.path).unwrap();
            let edited = text.replacen("format = 1", "format = 2", 1);
            assert_ne!(edited, text, "fixture must name its adapter format");
            std::fs::write(&session.path, edited).unwrap();
            let config = loaded(&session.path);
            assert!(config.config_issue().is_none());
            assert!(config.adapter_issue().is_some());

            for url in [EDIT, VIEW] {
                let response = session.call(Request::LoadNote(brauser_protocol::LoadNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "unsupported-format".into(),
                    url: url.into(),
                }));
                assert!(
                    matches!(&response, Response::Error(reply) if reply.message.contains("note identity is unavailable")),
                    "{url}: {response:?}"
                );
                assert!(
                    matches!(session.save(url, &current, "Must not autosave"), Response::Error(reply) if reply.message.contains("note identity is unavailable")),
                    "{url}"
                );
            }
            assert_eq!(std::fs::read(&note_path).unwrap(), original);
            // Other sites keep working.
            assert_eq!(
                session.load("https://other.example/page").key.kind,
                RecordKeyKind::Url
            );
        }

        #[test]
        fn ambiguous_removed_notes_never_replace_an_occupied_url_note() {
            let session = Session::new();
            let config = loaded(&session.path);
            let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
            // Two historical artifacts once used the same canonical form.
            let keys = ["work:work.example/7", "work:other.example/8"];
            let mut originals = Vec::new();
            for id in keys {
                let key = RecordKey {
                    kind: RecordKeyKind::Artifact,
                    value: id.into(),
                };
                note::save_keyed_note(&vault, EDIT, &key, "Separate artifact", id, "missing")
                    .unwrap();
                originals.push((key.clone(), vault.read_note(&key, EDIT).unwrap().unwrap()));
            }
            note::save_note(&vault, EDIT, "URL note", "Separate URL text", "missing").unwrap();
            let url_key = crate::vault::url_record_key(EDIT).unwrap();
            let url_bytes = vault.read_note(&url_key, EDIT).unwrap().unwrap();
            session.policy("adapter");
            // While config.toml still records the removed contract, it alone
            // names EDIT's artifact. The competing URL note stays separate:
            // listed and viewable, never merged over the artifact note.
            let current = session.load(EDIT);
            assert_eq!(current.key, originals[0].0);
            assert_eq!(current.body, "work:work.example/7");
            assert_eq!(current.older_notes.len(), 1);
            assert_eq!(current.older_notes[0].key, url_key);
            assert!(matches!(
                session.call(Request::UseArtifactNote(brauser_protocol::UseArtifactNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "no-merge".into(),
                    url: EDIT.into(),
                    source_url: EDIT.into(),
                    expected_revision: current.older_notes[0].revision.clone(),
                })),
                Response::Error(reply) if reply.code == ErrorCode::Conflict
            ));
            assert!(matches!(
                session.call(Request::ViewOlderNote(brauser_protocol::ViewOlderNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "view-url-note".into(),
                    url: EDIT.into(),
                    source_url: EDIT.into(),
                })),
                Response::NoteLoaded(note) if note.body == "Separate URL text"
            ));
            for (key, bytes) in &originals {
                assert_eq!(vault.read_note(key, EDIT).unwrap().as_ref(), Some(bytes));
            }
            assert_eq!(vault.read_note(&url_key, EDIT).unwrap().unwrap(), url_bytes);
            // Without that history, two recorded identities are ambiguous.
            session.forget_policy_history();
            for rebuild in [false, true] {
                if rebuild {
                    std::fs::remove_dir_all(config.cache_dir().unwrap()).unwrap();
                    session.index().reindex().unwrap();
                }
                let current = session.load(EDIT);
                assert_eq!(current.key, url_key);
                assert_eq!(current.body, "Separate URL text");
                for (key, bytes) in &originals {
                    let stale = NoteLoaded {
                        key: key.clone(),
                        ..current.clone()
                    };
                    assert!(matches!(session.save(EDIT, &stale, "Keep copy-out"),
                        Response::NoteConflict(reply) if reply.reason == "key_changed" && reply.body == current.body));
                    assert_eq!(vault.read_note(key, EDIT).unwrap().as_ref(), Some(bytes));
                }
                assert_eq!(vault.read_note(&url_key, EDIT).unwrap().unwrap(), url_bytes);
            }
        }

        #[test]
        fn a_full_or_damaged_notes_folder_cannot_choose_an_autosave_target() {
            for fault in ["count", "file bytes", "total bytes", "unreadable"] {
                let session = Session::new();
                let first = session.load(EDIT);
                let Response::NoteSaved(saved) = session.save(EDIT, &first, "Keep original text")
                else {
                    panic!("expected saved note")
                };
                let current = session.load(EDIT);
                session.policy("adapter");
                session.forget_policy_history();
                let note_path = session.root.join(saved.relative_path);
                let original = std::fs::read(&note_path).unwrap();
                let pages = session.root.join("notes");
                let mut faults = Vec::new();
                match fault {
                    "count" => {
                        // A conflicting identity sorts after the scan cap.
                        // The first artifact remains in the truncated set.
                        let config = loaded(&session.path);
                        let vault =
                            Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
                        let hidden_key = RecordKey {
                            kind: RecordKeyKind::Artifact,
                            value: "zzz:work.example/8".into(),
                        };
                        note::save_keyed_note(
                            &vault,
                            EDIT,
                            &hidden_key,
                            "Other artifact",
                            "Keep other text",
                            "missing",
                        )
                        .unwrap();
                        let path = session
                            .root
                            .join(vault.note_relative_path(&hidden_key, EDIT).unwrap());
                        assert!(
                            path > note_path,
                            "the conflicting note must sort after the original"
                        );
                        faults.push(path);
                        for number in 0..4095 {
                            let path = pages.join(format!("000-{number:04}.md"));
                            std::fs::write(&path, b"").unwrap();
                            faults.push(path);
                        }
                    }
                    "file bytes" => {
                        let path = pages.join("000-large.md");
                        std::fs::File::create(&path)
                            .unwrap()
                            .set_len(4 * 1024 * 1024 + 1)
                            .unwrap();
                        faults.push(path);
                    }
                    "total bytes" => {
                        for number in 0..4 {
                            let path = pages.join(format!("000-large-{number}.md"));
                            std::fs::write(&path, vec![b'x'; 4 * 1024 * 1024]).unwrap();
                            faults.push(path);
                        }
                    }
                    "unreadable" => {
                        let path = pages.join("000-non-utf8.md");
                        std::fs::write(&path, [0xff]).unwrap();
                        faults.push(path);
                    }
                    _ => unreachable!(),
                }
                // No scan decides the key: EDIT is a URL page whatever the
                // folder holds, and a client still holding the artifact key
                // gets a key_changed conflict instead of an autosave.
                let response = session.load(EDIT);
                assert_eq!(response.key.kind, RecordKeyKind::Url, "{fault}");
                assert!(!response.exists, "{fault}");
                assert!(
                    matches!(session.save(EDIT, &current, "Must not autosave"), Response::NoteConflict(reply) if reply.reason == "key_changed"),
                    "{fault}"
                );
                assert_eq!(std::fs::read(&note_path).unwrap(), original);
                // The optional discovery list retains its best-effort behavior.
                let config = loaded(&session.path);
                let vault = Vault::open(config.snapshot().storage.as_ref().unwrap()).unwrap();
                assert!(vault.note_candidates().is_ok());
                for path in faults {
                    std::fs::remove_file(path).unwrap();
                }
                assert_eq!(session.load(EDIT).key.kind, RecordKeyKind::Url);
                assert_eq!(std::fs::read(&note_path).unwrap(), original);
            }
        }
    }

    #[test]
    fn saved_context_appears_before_the_first_activity_is_delivered() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let occurred_at = time::OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let mut config = loaded(&path);
        let url = "https://work.example/items/7";
        let artifact_id = "work:work.example/7";

        let why_response = record_why(
            brauser_protocol::RecordWhyRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "why-1".into(),
                event: brauser_protocol::WhyEvent {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    artifact_id: artifact_id.into(),
                    url: url.into(),
                    occurred_at,
                    incognito: false,
                    text: "blocking the release".into(),
                },
            },
            &mut config,
        );
        assert!(matches!(
            why_response,
            Response::WhyRecorded(WhyRecorded {
                outcome: VisitOutcome::Persisted,
                ..
            })
        ));

        let artifact_response = get_artifact(
            brauser_protocol::GetArtifactRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "artifact-1".into(),
                url: url.into(),
                title: Some("Issue 7".into()),
            },
            &mut config,
        );
        match artifact_response {
            Response::ArtifactResult(result) => {
                let artifact = result.artifact.expect("recognized artifact");
                assert_eq!(artifact.why.as_deref(), Some("blocking the release"));
            }
            other => panic!("expected artifact_result, got {other:?}"),
        }
    }

    #[test]
    fn loading_a_note_works_on_any_http_page_without_site_authorization() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        // No site is enabled for logging; notes must still work (§5.3).
        let response = dispatch(
            Request::LoadNote(brauser_protocol::LoadNoteRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "load-1".into(),
                url: "https://example.com/a".into(),
            }),
            &mut config,
            &mut ConsentAuthority::new(),
            &SetupContext {
                env: &no_harnesses(folder.path()),
            },
        );
        match response {
            Response::NoteLoaded(value) => {
                assert!(!value.exists);
                assert_eq!(value.revision, "missing");
            }
            other => panic!("expected note_loaded, got {other:?}"),
        }
    }

    fn responses_from(output: Vec<u8>) -> Vec<Response> {
        let mut reader = Cursor::new(output);
        let mut responses = Vec::new();
        while let Some(body) = read_frame(&mut reader).unwrap() {
            responses.push(serde_json::from_slice::<Response>(&body).unwrap());
        }
        responses
    }

    fn save_request(id: &str, body: &str, revision: &str) -> Request {
        Request::SaveNote(brauser_protocol::SaveNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: id.into(),
            url: "https://example.com/a".into(),
            title: "A Title".into(),
            body: body.into(),
            expected_revision: revision.into(),
            expected_key: crate::vault::url_record_key("https://example.com/a").unwrap(),
        })
    }

    fn load_request(id: &str) -> Request {
        Request::LoadNote(brauser_protocol::LoadNoteRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: id.into(),
            url: "https://example.com/a".into(),
        })
    }

    fn hello_request(id: &str) -> Request {
        Request::Hello(HelloRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: id.into(),
        })
    }

    #[test]
    fn the_largest_valid_note_round_trips_without_ending_the_serve_loop() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let config = configured_store(&notes);
        // 98304 quotes, the panel's and the schema's limit, which double
        // once escaped.
        let body = "\"".repeat(98_304);
        let requests = [
            save_request("save-1", &body, "missing"),
            load_request("load-1"),
            // A stale save echoes the whole note back.
            save_request("save-stale", "other", "missing"),
            hello_request("hello-1"),
        ];
        let input: Vec<u8> = requests.into_iter().flat_map(encoded_request).collect();
        let mut output = Vec::new();
        serve_with_io(Cursor::new(input), &mut output, config).unwrap();

        let responses = responses_from(output);
        assert_eq!(responses.len(), 4);
        assert!(matches!(&responses[0], Response::NoteSaved(_)));
        match &responses[1] {
            Response::NoteLoaded(value) => assert_eq!(value.body, body),
            other => panic!("expected note_loaded, got {other:?}"),
        }
        match &responses[2] {
            Response::NoteConflict(value) => assert_eq!(value.body, body),
            other => panic!("expected note_conflict, got {other:?}"),
        }
        assert!(matches!(&responses[3], Response::HelloResult(_)));
    }

    #[test]
    fn a_note_too_large_to_return_gets_a_correlated_error_and_serving_continues() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        let saved = match dispatch(
            save_request("save-1", "small", "missing"),
            &mut config,
            &mut ConsentAuthority::new(),
            &SetupContext {
                env: &no_harnesses(folder.path()),
            },
        ) {
            Response::NoteSaved(value) => value,
            other => panic!("expected note_saved, got {other:?}"),
        };
        // The note grows outside the panel (for example in Obsidian) past
        // what one response can carry, while staying under the read limit.
        let path = notes.join(&saved.relative_path);
        let grown = std::fs::read_to_string(&path)
            .unwrap()
            .replace("small", &"x".repeat(MAX_OUTBOUND_BYTES));
        std::fs::write(&path, grown).unwrap();

        let requests = [
            load_request("load-1"),
            save_request("save-stale", "new", "missing"),
            hello_request("hello-1"),
        ];
        let input: Vec<u8> = requests.into_iter().flat_map(encoded_request).collect();
        let mut output = Vec::new();
        serve_with_io(Cursor::new(input), &mut output, config).unwrap();

        let responses = responses_from(output);
        assert_eq!(responses.len(), 3);
        for (response, id) in responses[..2].iter().zip(["load-1", "save-stale"]) {
            match response {
                Response::Error(value) => {
                    assert_eq!(value.code, ErrorCode::MessageTooLarge);
                    assert_eq!(value.request_id, id);
                }
                other => panic!("expected message_too_large, got {other:?}"),
            }
        }
        assert!(matches!(&responses[2], Response::HelloResult(_)));
    }

    #[test]
    fn an_oversized_response_becomes_a_correlated_error_frame() {
        let response = Response::NoteLoaded(NoteLoaded {
            protocol_version: PROTOCOL_VERSION,
            request_id: "load-big".into(),
            exists: true,
            revision: "missing".into(),
            title: String::new(),
            body: "x".repeat(MAX_OUTBOUND_BYTES + 1),
            key: crate::vault::url_record_key("https://example.com/a").unwrap(),
            older_notes: Vec::new(),
        });
        let mut output = Vec::new();
        write_frame(&mut output, &response).unwrap();
        match decoded_response(output) {
            Response::Error(value) => {
                assert_eq!(value.code, ErrorCode::MessageTooLarge);
                assert_eq!(value.request_id, "load-big");
            }
            other => panic!("expected message_too_large, got {other:?}"),
        }
    }

    #[test]
    fn saving_a_note_then_reusing_a_stale_revision_returns_the_current_note() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        let mut consent = ConsentAuthority::new();
        let env = no_harnesses(folder.path());
        let context = SetupContext { env: &env };
        let save = |config: &mut ConfigStore,
                    consent: &mut ConsentAuthority,
                    body: &str,
                    revision: &str| {
            dispatch(
                Request::SaveNote(brauser_protocol::SaveNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "save-1".into(),
                    url: "https://example.com/a".into(),
                    title: "A Title".into(),
                    body: body.into(),
                    expected_revision: revision.into(),
                    expected_key: crate::vault::url_record_key("https://example.com/a").unwrap(),
                }),
                config,
                consent,
                &context,
            )
        };
        let first = match save(&mut config, &mut consent, "hello", "missing") {
            Response::NoteSaved(value) => value,
            other => panic!("expected note_saved, got {other:?}"),
        };
        assert!(matches!(
            first.outcome,
            brauser_protocol::NoteSaveOutcome::Created
        ));

        // Reusing the old (now stale) revision must be refused, not clobber
        // the note that was just saved.
        match save(&mut config, &mut consent, "goodbye", "missing") {
            Response::NoteConflict(value) => {
                assert!(value.exists);
                assert_eq!(value.body, "hello");
                assert_eq!(value.revision, first.revision);
            }
            other => panic!("expected note_conflict, got {other:?}"),
        }

        // The correct revision replaces the note whole.
        match save(&mut config, &mut consent, "goodbye", &first.revision) {
            Response::NoteSaved(value) => {
                assert!(matches!(
                    value.outcome,
                    brauser_protocol::NoteSaveOutcome::Replaced
                ));
                assert_ne!(value.revision, first.revision);
            }
            other => panic!("expected note_saved, got {other:?}"),
        }
    }

    /// A harness environment that finds nothing and passes nothing.
    fn no_harnesses(folder: &std::path::Path) -> HarnessEnv {
        HarnessEnv {
            search_path: std::ffi::OsString::new(),
            home: None,
            vars: std::collections::BTreeMap::new(),
            work_root: folder.join("agent-work"),
        }
    }

    fn served(requests: &[String], config: ConfigStore) -> Vec<Response> {
        let folder = tempfile::tempdir().unwrap();
        let env = no_harnesses(folder.path());
        served_with(requests, config, &SetupContext { env: &env })
    }

    fn served_with(
        requests: &[String],
        config: ConfigStore,
        context: &SetupContext<'_>,
    ) -> Vec<Response> {
        let input: Vec<u8> = requests
            .iter()
            .flat_map(|json| encoded_json(json.as_bytes()))
            .collect();
        let mut output = Vec::new();
        serve_with_context(Cursor::new(input), &mut output, config, context).unwrap();
        let mut reader = Cursor::new(output);
        let mut responses = Vec::new();
        while let Some(body) = read_frame(&mut reader).unwrap() {
            responses.push(serde_json::from_slice::<Response>(&body).unwrap());
        }
        responses
    }

    fn check_agent_json() -> String {
        serde_json::to_string(&Request::CheckAgent(brauser_protocol::CheckAgentRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "agent-1".into(),
        }))
        .unwrap()
    }

    fn loaded(path: &std::path::Path) -> ConfigStore {
        let mut store = ConfigStore::for_test(path.to_path_buf());
        store.refresh().unwrap();
        store
    }

    #[test]
    fn invalid_adapter_policy_keeps_a_visit_retryable_before_vault_io() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        config::fixtures::write(
            &path,
            &root,
            "",
            "capture_enabled = true\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            "[adapters]\nformat = 2\n",
        );
        let response = record_visit(
            RecordVisitRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "visit-1".into(),
                event: VisitEvent {
                    event_id: "not-even-a-uuid".into(),
                    url: "https://work.example/issues/1".into(),
                    title: Some("Issue 1".into()),
                    occurred_at: "not-a-timestamp".into(),
                    incognito: false,
                },
            },
            &mut loaded(&path),
        );
        match response {
            Response::VisitRecorded(recorded) => {
                assert_eq!(recorded.outcome, VisitOutcome::Retryable);
                assert_eq!(
                    recorded.reason.as_deref(),
                    Some("adapter policy is unavailable")
                );
            }
            other => panic!("expected a retryable visit, got {other:?}"),
        }
    }

    fn activity_for_invalid_policy() -> ActivityEvent {
        ActivityEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            artifact_id: "work:work.example/1".into(),
            artifact_type: "work.item".into(),
            url: "https://work.example/issues/1".into(),
            title: Some("Issue 1".into()),
            // Relative to now: a fixed date ages out of the 30-day window.
            occurred_at: time::OffsetDateTime::now_utc()
                .replace_minute(0)
                .unwrap()
                .replace_second(0)
                .unwrap()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            incognito: false,
            opens: 1,
            focused_seconds: 3,
            background_seconds: 0,
            modes: Vec::new(),
            trails: Vec::new(),
            refs: Vec::new(),
        }
    }

    /// The local log day `activity_for_invalid_policy` events land on.
    fn invalid_policy_day() -> String {
        activity_for_invalid_policy().occurred_at[..10].to_owned()
    }

    fn invalid_adapter_config(folder: &tempfile::TempDir) -> ConfigStore {
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        config::fixtures::write(
            &path,
            &root,
            "",
            "capture_enabled = true\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            "[adapters]\nformat = 2\n",
        );
        loaded(&path)
    }

    #[test]
    fn invalid_adapter_policy_keeps_single_activity_retryable() {
        let folder = tempfile::tempdir().unwrap();
        let response = record_activity(
            RecordActivityRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "activity-1".into(),
                event: activity_for_invalid_policy(),
            },
            &mut invalid_adapter_config(&folder),
        );
        match response {
            Response::ActivityRecorded(recorded) => {
                assert_eq!(recorded.outcome, VisitOutcome::Retryable);
                assert_eq!(
                    recorded.reason.as_deref(),
                    Some("adapter policy is unavailable")
                );
            }
            other => panic!("expected a retryable activity, got {other:?}"),
        }
    }

    #[test]
    fn invalid_adapter_policy_keeps_every_batch_event_retryable() {
        let folder = tempfile::tempdir().unwrap();
        let activity = activity_for_invalid_policy();
        let visit_id = uuid::Uuid::new_v4().to_string();
        let response = record_batch(
            RecordBatchRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "batch-1".into(),
                batch_id: uuid::Uuid::new_v4().to_string(),
                events: vec![
                    CaptureBatchItem {
                        kind: CaptureEventKind::Visit,
                        visit: Some(VisitEvent {
                            event_id: visit_id.clone(),
                            url: "https://work.example/issues/1".into(),
                            title: Some("Issue 1".into()),
                            occurred_at: "2026-10-16T13:05:00Z".into(),
                            incognito: false,
                        }),
                        activity: None,
                    },
                    CaptureBatchItem {
                        kind: CaptureEventKind::Activity,
                        visit: None,
                        activity: Some(activity.clone()),
                    },
                ],
                overflow: None,
            },
            &mut invalid_adapter_config(&folder),
        );
        match response {
            Response::BatchRecorded(recorded) => {
                assert_eq!(recorded.results.len(), 2);
                for result in recorded.results {
                    assert_eq!(result.outcome, VisitOutcome::Retryable);
                    assert_eq!(
                        result.reason.as_deref(),
                        Some("adapter policy is unavailable")
                    );
                }
            }
            other => panic!("expected a retryable batch, got {other:?}"),
        }
    }

    #[test]
    fn host_suppresses_unrecognized_and_out_of_scope_worker_deliveries() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        config::fixtures::write(
            &path,
            &root,
            "",
            "capture_enabled = true\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            r#"[adapters]
format = 1

[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/teams/{owner:name|lower}/items/{id:int}" }]
id = "work:{host}/{owner}/{id}"
scope = { owner = ["acme"] }
"#,
        );
        let occurred_at = time::OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let mut config = loaded(&path);

        for (url, label) in [
            ("https://work.example/search", "unrecognized"),
            (
                "https://work.example/teams/personal/items/7",
                "outside scope",
            ),
        ] {
            let response = record_visit(
                RecordVisitRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: format!("visit-{label}"),
                    event: VisitEvent {
                        event_id: uuid::Uuid::new_v4().to_string(),
                        url: url.into(),
                        title: Some("Private item".into()),
                        occurred_at: occurred_at.clone(),
                        incognito: false,
                    },
                },
                &mut config,
            );
            match response {
                Response::VisitRecorded(recorded) => {
                    assert_eq!(recorded.outcome, VisitOutcome::Suppressed, "{label}");
                    assert_eq!(
                        recorded.reason.as_deref(),
                        Some("not_an_artifact"),
                        "{label}"
                    );
                    assert!(recorded.relative_path.is_none(), "{label}");
                }
                other => panic!("expected a suppressed visit for {label}, got {other:?}"),
            }
        }
        assert!(!root.join("logs").exists());

        let response = record_visit(
            RecordVisitRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "visit-recognized".into(),
                event: VisitEvent {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    url: "https://work.example/teams/acme/items/7".into(),
                    title: Some("Work item".into()),
                    occurred_at,
                    incognito: false,
                },
            },
            &mut config,
        );
        match response {
            Response::VisitRecorded(recorded) => {
                assert_ne!(recorded.outcome, VisitOutcome::Suppressed);
                assert_ne!(recorded.reason.as_deref(), Some("not_an_artifact"));
            }
            other => panic!("expected a recognized visit result, got {other:?}"),
        }
        let output = std::process::Command::new("python3")
            .args([
                "-c",
                "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('select count(*) from visits').fetchone()[0])",
            ])
            .arg(config.cache_dir().unwrap().join("index.db"))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "1");
    }

    #[test]
    fn unknown_trail_transitions_are_terminal_on_single_and_batch_delivery() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        config::fixtures::write(
            &path,
            &root,
            "",
            "capture_enabled = true\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            r#"[adapters]
format = 1

[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/issues/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let mut config = loaded(&path);
        for transition in [
            "mentions",
            "unknown",
            "Link",
            "link,forward_back",
            "link\n",
            "",
            "\u{202e}link",
        ] {
            let mut event = activity_for_invalid_policy();
            event.occurred_at = time::OffsetDateTime::now_utc()
                .replace_minute(0)
                .unwrap()
                .replace_second(0)
                .unwrap()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            event.trails.push(ActivityTrail {
                from_artifact_id: Some("work:work.example/2".into()),
                from_url: Some("https://work.example/issues/2".into()),
                transition: transition.into(),
            });
            let single = record_activity(
                RecordActivityRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "single-transition".into(),
                    event: event.clone(),
                },
                &mut config,
            );
            let batch = record_batch(
                RecordBatchRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "batch-transition".into(),
                    batch_id: uuid::Uuid::new_v4().to_string(),
                    events: vec![CaptureBatchItem {
                        kind: CaptureEventKind::Activity,
                        visit: None,
                        activity: Some(event),
                    }],
                    overflow: None,
                },
                &mut config,
            );
            let Response::ActivityRecorded(single) = single else {
                panic!("expected an activity result");
            };
            let Response::BatchRecorded(batch) = batch else {
                panic!("expected a batch result");
            };
            assert_eq!(batch.results.len(), 1);
            for (outcome, reason, relative_path) in [
                (single.outcome, single.reason, single.relative_path),
                (
                    batch.results[0].outcome,
                    batch.results[0].reason.clone(),
                    batch.results[0].relative_path.clone(),
                ),
            ] {
                assert_eq!(outcome, VisitOutcome::Rejected, "{transition:?}");
                assert_eq!(reason.as_deref(), Some("invalid_activity"));
                assert!(relative_path.is_none());
            }
        }
        assert!(!root.join("logs").exists());
        assert!(!config.cache_dir().unwrap().join("index.db").exists());
    }

    #[test]
    fn single_and_batch_activity_enforce_the_same_policy() {
        use serde_json::json;

        // Override one part of otherwise valid activity. Expected outcomes
        // are explicit so two equally permissive paths cannot pass by agreeing.
        let cases = [
            ("accepted", json!({}), VisitOutcome::Persisted, None),
            (
                "accepted mode, trail, and title reference",
                json!({"title": "Work PROJ-7", "modes": [{"mode": "view",
                    "url": "https://work.example/teams/acme/items/1"}],
                    "trails": [{"from_artifact_id": "work:work.example/acme/2",
                        "from_url": "https://work.example/teams/acme/items/2",
                        "transition": "link"}],
                    "refs": [{"artifact_type": "jira.issue", "value": "PROJ-7"}]}),
                VisitOutcome::Persisted,
                None,
            ),
            (
                "oversized modes before recognition",
                json!({"url": "https://work.example/search", "modes": vec![json!({"mode": "view", "url": "https://work.example/issues/1"}); 4]}),
                VisitOutcome::Rejected,
                Some("invalid_activity"),
            ),
            (
                "unknown transition",
                json!({"trails": [{"from_artifact_id": null, "from_url": null,
                    "transition": "mentions"}]}),
                VisitOutcome::Rejected,
                Some("invalid_activity"),
            ),
            (
                "unpaired trail source",
                json!({"trails": [{"from_artifact_id": "work:work.example/acme/2",
                    "from_url": null, "transition": "link"}]}),
                VisitOutcome::Rejected,
                Some("invalid_activity"),
            ),
            (
                "unrecognized route",
                json!({"url": "https://work.example/search"}),
                VisitOutcome::Suppressed,
                Some("not_an_artifact"),
            ),
            (
                "outside scope",
                json!({"url": "https://work.example/teams/personal/items/1"}),
                VisitOutcome::Suppressed,
                Some("not_an_artifact"),
            ),
            (
                "forged artifact ID",
                json!({"artifact_id": "work:work.example/acme/2"}),
                VisitOutcome::Rejected,
                Some("artifact_identity_mismatch"),
            ),
            (
                "forged artifact type",
                json!({"artifact_type": "other.item"}),
                VisitOutcome::Rejected,
                Some("artifact_identity_mismatch"),
            ),
            (
                "unsupported mode evidence",
                json!({"modes": [{"mode": "edit",
                    "url": "https://work.example/teams/acme/items/1"}]}),
                VisitOutcome::Rejected,
                Some("activity_mode_mismatch"),
            ),
            (
                "forged trail identity",
                json!({"trails": [{"from_artifact_id": "work:work.example/acme/99",
                    "from_url": "https://work.example/teams/acme/items/2",
                    "transition": "link"}]}),
                VisitOutcome::Rejected,
                Some("trail_identity_mismatch"),
            ),
            (
                "ungranted trail origin",
                json!({"trails": [{"from_artifact_id": "work:work.example/acme/2",
                    "from_url": "https://private.example/teams/acme/items/2",
                    "transition": "link"}]}),
                VisitOutcome::Rejected,
                Some("trail_identity_mismatch"),
            ),
            (
                "invented title reference",
                json!({"refs": [{"artifact_type": "jira.issue", "value": "SECRET-1"}]}),
                VisitOutcome::Rejected,
                Some("activity_refs_mismatch"),
            ),
        ];
        for (label, patch, expected_outcome, expected_reason) in cases {
            let mut event = activity_for_invalid_policy();
            event.url = "https://work.example/teams/acme/items/1".into();
            event.artifact_id = "work:work.example/acme/1".into();
            event.occurred_at = time::OffsetDateTime::now_utc()
                .replace_minute(0)
                .unwrap()
                .replace_second(0)
                .unwrap()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            let mut payload = serde_json::to_value(event).unwrap();
            payload
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            let event: ActivityEvent = serde_json::from_value(payload).unwrap();
            let mut outcomes = Vec::new();
            // Separate stores let the same accepted event persist on both
            // paths instead of hiding one behind duplicate-event suppression.
            for batched in [false, true] {
                let folder = tempfile::tempdir().unwrap();
                let root = folder.path().join("notes");
                std::fs::create_dir(&root).unwrap();
                let path = folder.path().join("config.toml");
                config::fixtures::write(
                    &path,
                    &root,
                    "",
                    "capture_enabled = true\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
                    r#"[adapters]
format = 1
[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/teams/{owner:name|lower}/items/{id:int}", mode = "view" }]
id = "work:{host}/{owner}/{id}"
scope = { owner = ["acme"] }
title = "{title}"
refs = [{ type = "jira.issue", find = "jira_key" }]
"#,
                );
                let mut config = loaded(&path);
                let (outcome, reason, relative_path) = if batched {
                    let Response::BatchRecorded(mut reply) = record_batch(
                        RecordBatchRequest {
                            protocol_version: PROTOCOL_VERSION,
                            request_id: "batch-policy".into(),
                            batch_id: uuid::Uuid::new_v4().to_string(),
                            events: vec![CaptureBatchItem {
                                kind: CaptureEventKind::Activity,
                                visit: None,
                                activity: Some(event.clone()),
                            }],
                            overflow: None,
                        },
                        &mut config,
                    ) else {
                        panic!("expected batch result for {label}");
                    };
                    assert_eq!(reply.request_id, "batch-policy");
                    assert_eq!(reply.results.len(), 1);
                    let result = reply.results.remove(0);
                    assert_eq!(result.event_id, event.event_id);
                    (result.outcome, result.reason, result.relative_path)
                } else {
                    let Response::ActivityRecorded(reply) = record_activity(
                        RecordActivityRequest {
                            protocol_version: PROTOCOL_VERSION,
                            request_id: "single-policy".into(),
                            event: event.clone(),
                        },
                        &mut config,
                    ) else {
                        panic!("expected activity result for {label}");
                    };
                    assert_eq!(reply.request_id, "single-policy");
                    assert_eq!(reply.event_id, event.event_id);
                    (reply.outcome, reply.reason, reply.relative_path)
                };
                assert_eq!(outcome, expected_outcome, "{label}, batch={batched}");
                assert_eq!(
                    reason.as_deref(),
                    expected_reason,
                    "{label}, batch={batched}"
                );
                if expected_outcome == VisitOutcome::Persisted {
                    let log = std::fs::read_to_string(root.join(relative_path.as_ref().unwrap()))
                        .unwrap();
                    let encoded = log
                        .split(" data=")
                        .nth(1)
                        .unwrap()
                        .split(' ')
                        .next()
                        .unwrap();
                    let stored: ActivityEvent =
                        serde_json::from_slice(&hex::decode(encoded).unwrap()).unwrap();
                    assert_eq!(stored, event, "{label}, batch={batched}");
                } else {
                    assert!(relative_path.is_none());
                    assert!(!root.join("logs").exists(), "{label}, batch={batched}");
                    assert!(!config.cache_dir().unwrap().join("index.db").exists());
                }
                outcomes.push((outcome, reason, relative_path));
            }
            assert_eq!(outcomes[0], outcomes[1], "{label}");
        }
    }

    #[test]
    fn unavailable_history_cannot_redirect_a_note_on_an_uncovered_origin() {
        for (publication_failure, context_only) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let folder = tempfile::tempdir().unwrap();
            let root = folder.path().join("notes");
            std::fs::create_dir(&root).unwrap();
            let path = folder.path().join("config.toml");
            let write_config = |adapters: &str| {
                config::fixtures::write(
                    &path,
                    &root,
                    "artifacts_dir = \"artifacts\"",
                    "capture_enabled = true\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]\nstrip_params = [\"item\"]",
                    &format!("[adapters]\nformat = 1\n{adapters}"),
                );
            };
            write_config(
                r#"
[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items", query = { item = "{id:int}" } }, { path = "/issues/{id:int}" }]
id = "work:{host}/{id}"
"#,
            );
            let mut config = loaded(&path);
            let canonical = "https://work.example/items";
            let first_url = "https://work.example/items?item=1";
            let second_url = "https://work.example/issues/2";
            let first = note_load(&mut config, first_url);
            let Response::NoteSaved(first_saved) = note_save(
                &mut config,
                first_url,
                "First note",
                &first.revision,
                first.key.clone(),
            ) else {
                panic!("expected first note")
            };
            let second = note_load(&mut config, second_url);
            let Response::NoteSaved(second_saved) = note_save(
                &mut config,
                second_url,
                "Second note",
                &second.revision,
                second.key.clone(),
            ) else {
                panic!("expected second note")
            };
            // Only history reveals that the stripped URL also belongs to
            // the second artifact; its note was created on another route.
            let mut event = activity_for_invalid_policy();
            event.url = "https://work.example/items?item=2".into();
            event.artifact_id = second.key.value;
            let persisted = if context_only {
                matches!(
                    record_why(
                        brauser_protocol::RecordWhyRequest {
                            protocol_version: PROTOCOL_VERSION,
                            request_id: "history-context".into(),
                            event: brauser_protocol::WhyEvent {
                                event_id: event.event_id,
                                artifact_id: event.artifact_id,
                                url: event.url,
                                occurred_at: event.occurred_at,
                                incognito: false,
                                text: "Context on the shared canonical URL".into(),
                            },
                        },
                        &mut config
                    ),
                    Response::WhyRecorded(WhyRecorded {
                        outcome: VisitOutcome::Persisted,
                        ..
                    })
                )
            } else {
                matches!(
                    record_activity(
                        RecordActivityRequest {
                            protocol_version: PROTOCOL_VERSION,
                            request_id: "history".into(),
                            event,
                        },
                        &mut config
                    ),
                    Response::ActivityRecorded(ActivityRecorded {
                        outcome: VisitOutcome::Persisted,
                        ..
                    })
                )
            };
            assert!(persisted);
            write_config("");
            assert_eq!(
                note_load(&mut config, canonical).key.kind,
                RecordKeyKind::Url
            );
            let first_path = root.join(first_saved.relative_path);
            let second_path = root.join(second_saved.relative_path);
            let first_bytes = std::fs::read(&first_path).unwrap();
            let second_bytes = std::fs::read(&second_path).unwrap();
            let cache = config.cache_dir().unwrap();
            let fault = if publication_failure {
                std::fs::remove_file(cache.join("index-state.json")).unwrap();
                let fault = cache.join("index.db");
                std::fs::remove_file(&fault).unwrap();
                std::fs::create_dir(&fault).unwrap();
                fault
            } else {
                let fault = root.join("logs/oversized.md");
                std::fs::File::create(&fault)
                    .unwrap()
                    .set_len(32 * 1024 * 1024 + 1)
                    .unwrap();
                fault
            };
            let responses = [
                load_note(
                    brauser_protocol::LoadNoteRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "load-unavailable".into(),
                        url: canonical.into(),
                    },
                    &mut config,
                ),
                // A client holding a formerly valid artifact key must not
                // be able to overwrite it using an incomplete candidate set.
                note_save(
                    &mut config,
                    canonical,
                    "Wrong note",
                    &first_saved.revision,
                    first.key.clone(),
                ),
            ];
            // Note identity on an uncovered origin never consults history,
            // so its failure cannot select, or refuse, a key.
            let [loaded_note, saved_note] = responses;
            assert!(
                matches!(&loaded_note, Response::NoteLoaded(note) if note.key.kind == RecordKeyKind::Url),
                "{loaded_note:?}"
            );
            assert!(
                matches!(&saved_note, Response::NoteConflict(NoteConflict { reason, .. }) if reason == "key_changed"),
                "{saved_note:?}"
            );
            assert_eq!(std::fs::read(first_path).unwrap(), first_bytes);
            assert_eq!(std::fs::read(second_path).unwrap(), second_bytes);
            if publication_failure {
                std::fs::remove_dir(fault).unwrap();
            } else {
                std::fs::remove_file(fault).unwrap();
            }
            assert_eq!(
                note_load(&mut config, canonical).key.kind,
                RecordKeyKind::Url
            );
            assert!(
                matches!(note_save(&mut config, canonical, "Keep copy-out", &first_saved.revision, first.key),
                Response::NoteConflict(NoteConflict { reason, .. }) if reason == "key_changed")
            );
        }
    }

    #[test]
    fn durable_activity_missing_from_a_valid_cache_catches_up_without_replay() {
        for query_first in [false, true] {
            for batched in [false, true] {
                let folder = tempfile::tempdir().unwrap();
                let root = folder.path().join("notes");
                std::fs::create_dir(&root).unwrap();
                let path = folder.path().join("config.toml");
                write_adapter_config(
                    &path,
                    &root,
                    r#"[[artifacts]]
 type = "work.item"
 origin = "https://work.example"
 match = [{ path = "/issues/{id:int}" }]
 id = "work:{host}/{id}"
 "#,
                );
                let mut config = loaded(&path);
                let deliver = |config: &mut ConfigStore, number: u8| {
                    let mut event = activity_for_invalid_policy();
                    event.event_id = format!("00000000-0000-4000-8000-{number:012}");
                    event.url = format!("https://work.example/issues/{number}");
                    event.artifact_id = format!("work:work.example/{number}");
                    event.title = Some(format!("Issue {number}"));
                    let outcomes = if batched {
                        let Response::BatchRecorded(reply) = record_batch(
                            RecordBatchRequest {
                                protocol_version: PROTOCOL_VERSION,
                                request_id: "batch".into(),
                                batch_id: uuid::Uuid::new_v4().to_string(),
                                events: vec![CaptureBatchItem {
                                    kind: CaptureEventKind::Activity,
                                    visit: None,
                                    activity: Some(event),
                                }],
                                overflow: None,
                            },
                            config,
                        ) else {
                            panic!("expected batch result")
                        };
                        reply
                            .results
                            .into_iter()
                            .map(|r| r.outcome)
                            .collect::<Vec<_>>()
                    } else {
                        let Response::ActivityRecorded(reply) = record_activity(
                            RecordActivityRequest {
                                protocol_version: PROTOCOL_VERSION,
                                request_id: "single".into(),
                                event,
                            },
                            config,
                        ) else {
                            panic!("expected activity result")
                        };
                        vec![reply.outcome]
                    };
                    assert_eq!(outcomes, vec![VisitOutcome::Persisted]);
                };
                deliver(&mut config, 1);
                let cache = config.cache_dir().unwrap();
                let state_path = cache.join("index-state.json");
                let old_state = std::fs::read(&state_path).unwrap();
                let old_db = std::fs::read(cache.join("index.db")).unwrap();
                deliver(&mut config, 2);
                // The append is durable, but both published caches are from
                // before it (interruption between append and index update).
                std::fs::write(&state_path, &old_state).unwrap();
                std::fs::write(cache.join("index.db"), &old_db).unwrap();
                drop(config);
                let mut config = loaded(&path);
                if !query_first {
                    deliver(&mut config, 3);
                }
                let Response::ArtifactListResult(reply) = list_artifacts(
                    brauser_protocol::ListArtifactsRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "recent".into(),
                        artifact_type: None,
                        title_query: None,
                        limit: 32,
                    },
                    &mut config,
                ) else {
                    panic!("expected complete history")
                };
                let expected = if query_first { 2 } else { 3 };
                assert_eq!(
                    reply.items.len(),
                    expected,
                    "query_first={query_first}, batched={batched}"
                );
                assert!(
                    reply
                        .items
                        .iter()
                        .any(|item| item.artifact_id == "work:work.example/2")
                );
                assert_eq!(
                    reply
                        .items
                        .iter()
                        .map(|item| u64::from(item.opens))
                        .sum::<u64>(),
                    expected as u64
                );
                let Response::WorklogResult(log) = worklog(
                    brauser_protocol::WorklogRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "worklog".into(),
                        start_date: invalid_policy_day(),
                        end_date: invalid_policy_day(),
                    },
                    &mut config,
                ) else {
                    panic!("expected worklog")
                };
                assert_eq!(log.artifact_count as usize, expected);
                assert!(log.text.contains("Issue 2"));
                // Catch-up must persist its checkpoint and repair the public
                // SQLite snapshot too, without waiting for another delivery.
                let output = std::process::Command::new("python3")
                    .args(["-c", "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('select count(*) from visits').fetchone()[0])"])
                    .arg(cache.join("index.db")).output().unwrap();
                assert!(output.status.success());
                assert_eq!(
                    String::from_utf8(output.stdout).unwrap().trim(),
                    expected.to_string()
                );
            }
        }
    }

    #[test]
    fn cache_failure_keeps_capture_durable_and_history_explicitly_unavailable() {
        for oversized_log in [false, true] {
            let folder = tempfile::tempdir().unwrap();
            let root = folder.path().join("notes");
            std::fs::create_dir(&root).unwrap();
            let path = folder.path().join("config.toml");
            write_adapter_config(
                &path,
                &root,
                r#"[[artifacts]]
 type = "work.item"
 origin = "https://work.example"
 match = [{ path = "/issues/{id:int}" }]
 id = "work:{host}/{id}"
 "#,
            );
            let mut config = loaded(&path);
            let mut event = activity_for_invalid_policy();
            let deliver = |config: &mut ConfigStore, event: ActivityEvent| {
                let Response::ActivityRecorded(reply) = record_activity(
                    RecordActivityRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "capture".into(),
                        event,
                    },
                    config,
                ) else {
                    panic!("expected capture result")
                };
                reply
            };
            assert_eq!(
                deliver(&mut config, event.clone()).outcome,
                VisitOutcome::Persisted
            );
            let cache = config.cache_dir().unwrap();
            let old_state = std::fs::read(cache.join("index-state.json")).unwrap();
            let fault = if oversized_log {
                let fault = root.join("logs/oversized.md");
                let file = std::fs::File::create(&fault).unwrap();
                file.set_len(32 * 1024 * 1024 + 1).unwrap();
                fault
            } else {
                let fault = cache.join("index.db");
                std::fs::remove_file(&fault).unwrap();
                std::fs::create_dir(&fault).unwrap();
                fault
            };
            event.event_id = uuid::Uuid::new_v4().to_string();
            event.artifact_id = "work:work.example/2".into();
            event.url = "https://work.example/issues/2".into();
            event.title = Some("Durable despite cache failure".into());
            let captured = deliver(&mut config, event.clone());
            assert_eq!(captured.outcome, VisitOutcome::Persisted);
            let log_path = root.join(captured.relative_path.unwrap());
            let durable_bytes = std::fs::read(&log_path).unwrap();
            assert_eq!(
                std::fs::read(cache.join("index-state.json")).unwrap(),
                old_state
            );
            let replies = [
                list_artifacts(
                    brauser_protocol::ListArtifactsRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "recent".into(),
                        artifact_type: None,
                        title_query: None,
                        limit: 32,
                    },
                    &mut config,
                ),
                get_artifact(
                    brauser_protocol::GetArtifactRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "artifact".into(),
                        url: event.url.clone(),
                        title: None,
                    },
                    &mut config,
                ),
                worklog(
                    brauser_protocol::WorklogRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "worklog".into(),
                        start_date: invalid_policy_day(),
                        end_date: invalid_policy_day(),
                    },
                    &mut config,
                ),
            ];
            for reply in replies {
                let Response::Error(reply) = reply else {
                    panic!("must not return partial history: {reply:?}")
                };
                assert_eq!(reply.code, ErrorCode::Internal);
                assert!(reply.message.contains("history is unavailable"));
            }
            if oversized_log {
                std::fs::remove_file(fault).unwrap();
            } else {
                std::fs::remove_dir(fault).unwrap();
            }
            drop(config);
            let mut config = loaded(&path);
            let Response::ArtifactListResult(reply) = list_artifacts(
                brauser_protocol::ListArtifactsRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "recovered".into(),
                    artifact_type: None,
                    title_query: None,
                    limit: 32,
                },
                &mut config,
            ) else {
                panic!("expected recovered history")
            };
            assert_eq!(reply.items.len(), 2);
            assert!(
                reply
                    .items
                    .iter()
                    .any(|item| item.title == "Durable despite cache failure")
            );
            let duplicate = deliver(&mut config, event);
            assert_eq!(duplicate.outcome, VisitOutcome::Suppressed);
            assert_eq!(duplicate.reason.as_deref(), Some("duplicate_event"));
            assert_eq!(std::fs::read(log_path).unwrap(), durable_bytes);
        }
    }

    fn deliver_activity(config: &mut ConfigStore, event: ActivityEvent) -> VisitOutcome {
        let Response::ActivityRecorded(reply) = record_activity(
            RecordActivityRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "capture".into(),
                event,
            },
            config,
        ) else {
            panic!("expected capture result")
        };
        reply.outcome
    }

    #[test]
    fn title_refs_resolve_through_the_confirmed_adapters_into_links() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        config::fixtures::write(
            &path,
            &root,
            "artifacts_dir = \"artifacts\"",
            "capture_enabled = true\nsites = [{ origin = \"https://acme.atlassian.net\", path_prefix = \"/\" }, { origin = \"https://work.example\", path_prefix = \"/\" }]",
            r#"[adapters]
format = 1

[[artifacts]]
type = "jira.issue"
origin = "https://acme.atlassian.net"
match = [{ path = "/browse/{key:jira_key}" }]
id = "jira:{host}/{key}"

[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/issues/{id:int}" }]
id = "work:{host}/{id}"
title = "{title}"
refs = [{ type = "jira.issue", find = "jira_key" }]
"#,
        );
        let mut config = loaded(&path);
        let mut issue = activity_for_invalid_policy();
        issue.artifact_id = "jira:acme.atlassian.net/PROJ-1".into();
        issue.artifact_type = "jira.issue".into();
        issue.url = "https://acme.atlassian.net/browse/PROJ-1".into();
        issue.title = Some("Export pagination".into());
        assert_eq!(
            deliver_activity(&mut config, issue),
            VisitOutcome::Persisted
        );
        let mut pull = activity_for_invalid_policy();
        pull.url = "https://work.example/issues/412".into();
        pull.artifact_id = "work:work.example/412".into();
        pull.title = Some("PROJ-1 Fix SBOM export pagination".into());
        pull.refs = vec![brauser_protocol::ActivityRef {
            artifact_type: "jira.issue".into(),
            value: "PROJ-1".into(),
        }];
        assert_eq!(deliver_activity(&mut config, pull), VisitOutcome::Persisted);
        for url in [
            "https://acme.atlassian.net/browse/PROJ-1",
            "https://work.example/issues/412",
        ] {
            let Response::ArtifactResult(reply) = get_artifact(
                brauser_protocol::GetArtifactRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "artifact".into(),
                    url: url.into(),
                    title: None,
                },
                &mut config,
            ) else {
                panic!("expected artifact result")
            };
            assert_eq!(reply.artifact.unwrap().linked_count, 1, "{url}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn refreshing_artifacts_updates_the_rest_and_names_an_unwritable_path() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
 type = "work.item"
 origin = "https://work.example"
 match = [{ path = "/issues/{id:int}" }]
 id = "work:{host}/{id}"
 "#,
        );
        let mut config = loaded(&path);
        let deliver = |config: &mut ConfigStore, number: u8, title: &str| {
            let mut event = activity_for_invalid_policy();
            event.url = format!("https://work.example/issues/{number}");
            event.artifact_id = format!("work:work.example/{number}");
            event.title = Some(title.into());
            assert_eq!(deliver_activity(config, event), VisitOutcome::Persisted);
        };
        let refresh = |config: &mut ConfigStore| {
            refresh_artifacts(
                brauser_protocol::RefreshArtifactsRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "refresh".into(),
                },
                config,
            )
        };
        deliver(&mut config, 1, "First");
        deliver(&mut config, 2, "Second");
        let Response::ArtifactsRefreshed(reply) = refresh(&mut config) else {
            panic!("expected refreshed artifacts")
        };
        assert_eq!(reply.written, 2);
        let files = std::fs::read_dir(root.join("artifacts/work-item/work-example"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        let locked = files
            .iter()
            .find(|path| std::fs::read_to_string(path).unwrap().contains("First"))
            .unwrap()
            .clone();
        let other = files.iter().find(|path| **path != locked).unwrap().clone();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        deliver(&mut config, 1, "First again");
        deliver(&mut config, 2, "Second again");
        // A third artifact's path is occupied by someone else's file.
        let occupied = root.join(format!(
            "artifacts/work-item/work-example/3-{}.md",
            &hex::encode(<sha2::Sha256 as sha2::Digest>::digest(
                b"work:work.example/3"
            ))[..12]
        ));
        std::fs::write(&occupied, "not ours\n").unwrap();
        deliver(&mut config, 3, "Third");
        let reply = refresh(&mut config);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).unwrap();
        let Response::Error(reply) = reply else {
            panic!("expected the failure to be reported: {reply:?}")
        };
        let name = locked.file_name().unwrap().to_string_lossy().into_owned();
        assert!(reply.message.contains(&name), "{}", reply.message);
        let occupied_name = occupied.file_name().unwrap().to_string_lossy().into_owned();
        assert!(reply.message.contains(&occupied_name), "{}", reply.message);
        assert_eq!(std::fs::read_to_string(&occupied).unwrap(), "not ours\n");
        assert!(
            std::fs::read_to_string(&other)
                .unwrap()
                .contains("Second again")
        );
        let Response::ArtifactsRefreshed(reply) = refresh(&mut config) else {
            panic!("expected refreshed artifacts")
        };
        assert_eq!(reply.written, 1);
        assert!(
            std::fs::read_to_string(&locked)
                .unwrap()
                .contains("First again")
        );
    }

    #[test]
    fn host_revalidates_activity_identity_and_trail_sources() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        config::fixtures::write(
            &path,
            &root,
            "",
            "capture_enabled = true\nsites = [{ origin = \"https://work.example\", path_prefix = \"/\" }]",
            r#"[adapters]
format = 1

[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let occurred_at = time::OffsetDateTime::now_utc()
            .replace_minute(0)
            .unwrap()
            .replace_second(0)
            .unwrap()
            .replace_nanosecond(0)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let event = |artifact_id: &str, trails: Vec<ActivityTrail>| ActivityEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            artifact_id: artifact_id.into(),
            artifact_type: "work.item".into(),
            url: "https://work.example/items/7".into(),
            title: Some("Work item".into()),
            occurred_at: occurred_at.clone(),
            incognito: false,
            opens: 1,
            focused_seconds: 3,
            background_seconds: 0,
            modes: Vec::new(),
            trails,
            refs: Vec::new(),
        };
        let invoke = |config: &mut ConfigStore, event| {
            record_activity(
                RecordActivityRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "activity-1".into(),
                    event,
                },
                config,
            )
        };
        let mut config = loaded(&path);
        let accepted = event("work:work.example/7", Vec::new());
        match invoke(&mut config, accepted.clone()) {
            Response::ActivityRecorded(recorded) => {
                assert_eq!(
                    recorded.outcome,
                    VisitOutcome::Persisted,
                    "{:?}",
                    recorded.reason
                );
            }
            other => panic!("expected accepted activity, got {other:?}"),
        }
        assert!(config.cache_dir().unwrap().join("index.db").is_file());
        let mut forged_retry = accepted;
        forged_retry.title = Some("Forged retry".into());
        match invoke(&mut config, forged_retry) {
            Response::ActivityRecorded(recorded) => {
                assert_eq!(recorded.outcome, VisitOutcome::Suppressed);
                assert_eq!(recorded.reason.as_deref(), Some("duplicate_event"));
            }
            other => panic!("expected duplicate activity suppression, got {other:?}"),
        }
        let output = std::process::Command::new("python3")
            .args([
                "-c",
                "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('select title from artifacts').fetchone()[0])",
            ])
            .arg(config.cache_dir().unwrap().join("index.db"))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "Work item"
        );
        assert!(!root.join("artifacts").exists());
        regenerate_artifacts(&mut config, true);
        assert!(root.join("artifacts").is_dir());
        match invoke(&mut config, event("work:work.example/8", Vec::new())) {
            Response::ActivityRecorded(recorded) => {
                assert_eq!(recorded.outcome, VisitOutcome::Rejected);
                assert_eq!(
                    recorded.reason.as_deref(),
                    Some("artifact_identity_mismatch")
                );
            }
            other => panic!("expected rejected activity, got {other:?}"),
        }
        let mut oversized_modes = event("work:work.example/7", Vec::new());
        oversized_modes.modes = (0..4)
            .map(|_| ActivityMode {
                mode: "view".into(),
                url: "https://work.example/items/7".into(),
            })
            .collect();
        match invoke(&mut config, oversized_modes) {
            Response::ActivityRecorded(recorded) => {
                assert_eq!(recorded.outcome, VisitOutcome::Rejected);
                assert_eq!(recorded.reason.as_deref(), Some("invalid_activity"));
            }
            other => panic!("expected oversized activity rejection, got {other:?}"),
        }
        let mut hostile_mode = event("work:work.example/7", Vec::new());
        hostile_mode.modes.push(ActivityMode {
            mode: "edit".into(),
            url: "https://work.example/items/7".into(),
        });
        match invoke(&mut config, hostile_mode) {
            Response::ActivityRecorded(recorded) => {
                assert_eq!(recorded.outcome, VisitOutcome::Rejected);
                assert_eq!(recorded.reason.as_deref(), Some("activity_mode_mismatch"));
            }
            other => panic!("expected rejected mode evidence, got {other:?}"),
        }
        let hostile_trail = ActivityTrail {
            from_artifact_id: Some("work:work.example/99".into()),
            from_url: Some("https://work.example/items/2".into()),
            transition: "link".into(),
        };
        match invoke(
            &mut config,
            event("work:work.example/7", vec![hostile_trail]),
        ) {
            Response::ActivityRecorded(recorded) => {
                assert_eq!(recorded.outcome, VisitOutcome::Rejected);
                assert_eq!(recorded.reason.as_deref(), Some("trail_identity_mismatch"));
            }
            other => panic!("expected rejected trail, got {other:?}"),
        }
    }

    /// A v6 config: capture on, one adapter, and no `sites` key at all.
    fn adapter_only_config(folder: &tempfile::TempDir) -> (ConfigStore, std::path::PathBuf) {
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        config::fixtures::write(
            &path,
            &root,
            "",
            "capture_enabled = true",
            r#"[adapters]
format = 1

[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        (loaded(&path), root)
    }

    fn hour_timestamp() -> String {
        time::OffsetDateTime::now_utc()
            .replace_minute(0)
            .unwrap()
            .replace_second(0)
            .unwrap()
            .replace_nanosecond(0)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    }

    fn work_item_activity(occurred_at: &str, trails: Vec<ActivityTrail>) -> ActivityEvent {
        ActivityEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            artifact_id: "work:work.example/7".into(),
            artifact_type: "work.item".into(),
            url: "https://work.example/items/7".into(),
            title: Some("Work item".into()),
            occurred_at: occurred_at.into(),
            incognito: false,
            opens: 1,
            focused_seconds: 3,
            background_seconds: 0,
            modes: Vec::new(),
            trails,
            refs: Vec::new(),
        }
    }

    #[test]
    fn adapter_origin_is_captured_without_a_sites_entry() {
        let folder = tempfile::tempdir().unwrap();
        let (mut config, _root) = adapter_only_config(&folder);
        assert_eq!(config.adapter_issue(), None);
        assert_eq!(
            config.capture_origins(),
            vec!["https://work.example".to_owned()]
        );
        let response = record_activity(
            RecordActivityRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "activity-1".into(),
                event: work_item_activity(&hour_timestamp(), Vec::new()),
            },
            &mut config,
        );
        match response {
            Response::ActivityRecorded(recorded) => {
                assert_eq!(
                    recorded.outcome,
                    VisitOutcome::Persisted,
                    "{:?}",
                    recorded.reason
                );
            }
            other => panic!("expected persisted activity, got {other:?}"),
        }
    }

    #[test]
    fn host_still_suppresses_deliveries_outside_every_adapter_origin() {
        let folder = tempfile::tempdir().unwrap();
        let (mut config, root) = adapter_only_config(&folder);
        let occurred_at = hour_timestamp();
        let response = record_visit(
            RecordVisitRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "visit-other".into(),
                event: VisitEvent {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    url: "https://other.example/items/7".into(),
                    title: Some("Elsewhere".into()),
                    occurred_at: occurred_at.clone(),
                    incognito: false,
                },
            },
            &mut config,
        );
        match response {
            Response::VisitRecorded(recorded) => {
                assert_eq!(recorded.outcome, VisitOutcome::Suppressed);
                assert_eq!(recorded.reason.as_deref(), Some("not_an_artifact"));
            }
            other => panic!("expected a suppressed visit, got {other:?}"),
        }
        let foreign_trail = ActivityTrail {
            from_artifact_id: Some("other:other.example/2".into()),
            from_url: Some("https://other.example/items/2".into()),
            transition: "link".into(),
        };
        let response = record_activity(
            RecordActivityRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "activity-trail".into(),
                event: work_item_activity(&occurred_at, vec![foreign_trail]),
            },
            &mut config,
        );
        match response {
            Response::ActivityRecorded(recorded) => {
                assert_eq!(recorded.outcome, VisitOutcome::Rejected);
                assert_eq!(recorded.reason.as_deref(), Some("trail_identity_mismatch"));
            }
            other => panic!("expected a rejected trail, got {other:?}"),
        }
        assert!(!root.join("logs").exists());
    }

    fn refused(response: &Response) -> (ErrorCode, &str) {
        match response {
            Response::Error(value) => (value.code, value.message.as_str()),
            other => panic!("expected an agent refusal, got {other:?}"),
        }
    }

    #[test]
    fn check_agent_refused_for_m1_config_missing_key() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("config.toml");
        config::fixtures::write(&path, folder.path(), "", "", "");
        let responses = served(&[check_agent_json()], loaded(&path));
        let (code, message) = refused(&responses[0]);
        assert_eq!(code, ErrorCode::NotConfigured);
        assert!(message.contains(&format!("{} settings", crate::brand::APP_NAME)));
    }

    #[cfg(unix)]
    #[test]
    fn check_agent_reports_harness() {
        let folder = tempfile::tempdir().unwrap();
        let (path, _, _) = config::fixtures::ready(folder.path());
        match &served(&[check_agent_json()], loaded(&path))[0] {
            Response::AgentChecked(value) => {
                assert_eq!(value.harness_id, "claude-code");
                assert_eq!(value.harness_version, "2.1.284");
            }
            other => panic!("expected agent_checked, got {other:?}"),
        }
    }

    /// Step 1 refuses AI commands off unix, even when setup's record still
    /// matches the harness.
    #[cfg(not(unix))]
    #[test]
    fn check_agent_refused_on_this_platform() {
        let folder = tempfile::tempdir().unwrap();
        let (path, _, _) = config::fixtures::ready(folder.path());
        let responses = served(&[check_agent_json()], loaded(&path));
        assert_eq!(responses.len(), 1);
        for response in &responses {
            assert_eq!(
                refused(response),
                (
                    ErrorCode::NotConfigured,
                    "AI commands are not available on this platform yet."
                )
            );
        }
    }

    #[test]
    fn get_config_reports_agent_status_not_set_up() {
        let folder = tempfile::tempdir().unwrap();
        let config = ConfigStore::for_test(folder.path().join("config.toml"));
        let request = serde_json::to_string(&Request::GetConfig(GetConfigRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "read-1".into(),
        }))
        .unwrap();
        match &served(&[request], config)[0] {
            Response::ConfigResult(value) => {
                assert_eq!(
                    value.agent_status.state,
                    brauser_protocol::AgentState::NotSetUp
                );
                assert_eq!(value.agent_status.harness_version, None);
                assert!(value.agent_status.message.is_some());
            }
            other => panic!("expected config_result, got {other:?}"),
        }
    }

    #[test]
    fn protocol_v3_request_gets_unsupported_version() {
        let folder = tempfile::tempdir().unwrap();
        let config = ConfigStore::for_test(folder.path().join("config.toml"));
        let request =
            r#"{"type":"get_config","protocol_version":3,"request_id":"old-1"}"#.to_owned();
        match &served(&[request], config)[0] {
            Response::Error(value) => {
                assert_eq!(value.code, ErrorCode::UnsupportedProtocolVersion);
                assert_eq!(value.request_id, "old-1");
            }
            other => panic!("expected version error, got {other:?}"),
        }
    }

    #[test]
    fn protocol_v5_request_gets_unsupported_version() {
        let folder = tempfile::tempdir().unwrap();
        let config = ConfigStore::for_test(folder.path().join("config.toml"));
        let request =
            r#"{"type":"get_config","protocol_version":5,"request_id":"old-5"}"#.to_owned();
        match &served(&[request], config)[0] {
            Response::Error(value) => {
                assert_eq!(value.code, ErrorCode::UnsupportedProtocolVersion);
                assert_eq!(value.request_id, "old-5");
            }
            other => panic!("expected version error, got {other:?}"),
        }
    }

    /// Native harness setup against fake harnesses on an injected search
    /// path. No real harness is ever found or run, and no dialog exists.
    #[cfg(unix)]
    mod harness_setup {
        use super::*;
        use crate::harness::fake::{Fake, PASSING_BODY};
        use brauser_protocol::{
            AgentState, DiscoverHarnessesRequest, HarnessOffer, SetUpHarnessRequest,
        };

        struct Session {
            fake: Fake,
            env: HarnessEnv,
            path: std::path::PathBuf,
            config: ConfigStore,
            consent: ConsentAuthority,
        }

        fn session(body: &str) -> Session {
            let fake = Fake::new();
            fake.claude(body);
            let root = fake.path("notes");
            std::fs::create_dir(&root).unwrap();
            let path = fake.path("config.toml");
            config::fixtures::write(&path, &root, "", "", "");
            let env = fake.env(&[]);
            Session {
                config: loaded(&path),
                fake,
                env,
                path,
                consent: ConsentAuthority::new(),
            }
        }

        impl Session {
            fn call(&mut self, request: Request) -> Response {
                let context = SetupContext { env: &self.env };
                dispatch(request, &mut self.config, &mut self.consent, &context)
            }

            fn revision(&mut self) -> String {
                self.config.refresh().unwrap();
                self.config.revision().to_owned()
            }

            fn offers(&mut self) -> Vec<HarnessOffer> {
                match self.call(Request::DiscoverHarnesses(DiscoverHarnessesRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "discover-1".into(),
                })) {
                    Response::HarnessesDiscovered(value) => value.offers,
                    other => panic!("expected harnesses_discovered, got {other:?}"),
                }
            }

            fn set_up_harness(
                &mut self,
                revision: &str,
                harness_id: &str,
                summaries_dir: Option<&str>,
            ) -> Response {
                self.call(Request::SetUpHarness(SetUpHarnessRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "setup-1".into(),
                    expected_revision: revision.into(),
                    harness_id: harness_id.into(),
                    env_names: Vec::new(),
                    summaries_dir: summaries_dir.map(str::to_owned),
                }))
            }

            fn update(&mut self, next: ConfigSnapshot) -> Response {
                let revision = self.revision();
                self.call(Request::UpdateConfig(UpdateConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "write-1".into(),
                    expected_revision: revision,
                    config: next,
                    picker_token: None,
                    knowledge_base_token: None,
                }))
            }

            /// Discover, then set up the fake claude.
            fn set_up(&mut self) -> Response {
                let offers = self.offers();
                assert!(offers.iter().any(|offer| offer.harness_id == "claude-code"));
                let revision = self.revision();
                self.set_up_harness(&revision, "claude-code", None)
            }

            fn status(&mut self) -> brauser_protocol::AgentStatus {
                match self.call(Request::GetConfig(GetConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "read-1".into(),
                })) {
                    Response::ConfigResult(value) => value.agent_status,
                    other => panic!("expected config_result, got {other:?}"),
                }
            }
        }

        #[test]
        fn set_up_harness_probes_and_commits_without_a_dialog() {
            let mut session = session(PASSING_BODY);
            let before = session.revision();
            let response = session.set_up();
            let Response::ConfigUpdated(updated) = &response else {
                panic!("expected config_updated, got {response:?}");
            };
            let agent = updated.config.agent.as_ref().unwrap();
            assert_eq!(agent.harness_id, "claude-code");
            assert_eq!(
                agent.binary,
                session.fake.path("bin").join("claude").to_string_lossy()
            );
            assert_eq!(agent.env_allow, ["HOME", "PATH"]);
            assert_eq!(
                updated
                    .config
                    .storage
                    .as_ref()
                    .unwrap()
                    .summaries_dir
                    .as_deref(),
                Some("summaries")
            );
            assert_ne!(updated.revision, before);
            assert_eq!(updated.revision, session.revision());
            assert!(session.fake.log("claude", "stdin.log").is_some());
            let status = session.status();
            assert_eq!(status.state, AgentState::Ready);
            assert_eq!(status.harness_version.as_deref(), Some("2.1.284"));
        }

        #[test]
        fn set_up_harness_refuses_a_failing_probe_and_leaves_config_unchanged() {
            let mut session = session("exit 3");
            let before = std::fs::read(&session.path).unwrap();
            let revision = session.revision();
            let response = session.set_up();
            let (code, message) = refused(&response);
            assert_eq!(code, ErrorCode::InvalidConfig);
            assert_eq!(
                message,
                "Claude Code failed its test run (test run failed (exit 3)); AI commands stay off"
            );
            assert_eq!(session.revision(), revision);
            assert_eq!(std::fs::read(&session.path).unwrap(), before);
            assert_eq!(session.status().state, AgentState::NotSetUp);
        }

        #[test]
        fn set_up_harness_on_a_stale_revision_is_a_conflict() {
            let mut session = session(PASSING_BODY);
            let response = session.set_up_harness("stale", "claude-code", None);
            assert_eq!(refused(&response).0, ErrorCode::Conflict);
            assert_eq!(session.fake.log("claude", "stdin.log"), None);
        }

        #[test]
        fn set_up_harness_refuses_an_unknown_or_refused_harness() {
            let mut session = session(PASSING_BODY);
            session
                .fake
                .install("codex", "codex-cli 0.130.0", "", "exit 0");
            let revision = session.revision();
            let response = session.set_up_harness(&revision, "other", None);
            let (code, message) = refused(&response);
            assert_eq!(code, ErrorCode::Unauthorized);
            assert!(message.contains("detect again"), "{message}");
            let response = session.set_up_harness(&revision, "codex", None);
            let (code, message) = refused(&response);
            assert_eq!(code, ErrorCode::Unauthorized);
            assert!(message.contains("has not been reviewed"), "{message}");
            assert_eq!(session.revision(), revision);
            assert_eq!(session.fake.log("claude", "stdin.log"), None);
        }

        #[test]
        fn update_config_cannot_set_an_agent() {
            let mut session = session(PASSING_BODY);
            let mut next = session.config.snapshot().clone();
            next.agent = Some(brauser_protocol::AgentConfig {
                harness_id: "claude-code".into(),
                adapter: brauser_protocol::HarnessAdapter::ClaudeCode,
                binary: session
                    .fake
                    .path("bin")
                    .join("claude")
                    .to_string_lossy()
                    .into_owned(),
                args: crate::harness::template_args(crate::harness::Adapter::ClaudeCode),
                env_allow: vec!["HOME".into(), "PATH".into()],
                timeout_secs: 120,
            });
            let response = session.update(next);
            assert_eq!(refused(&response).0, ErrorCode::Unauthorized);
            assert_eq!(session.status().state, AgentState::NotSetUp);
        }

        #[test]
        fn discover_lists_fake_claude_ready_and_codex_refused() {
            let mut session = session(PASSING_BODY);
            session
                .fake
                .install("codex", "codex-cli 0.130.0", "", "exit 0");
            let revision = session.revision();
            let offers = session.offers();
            assert_eq!(offers.len(), 2);
            let claude = offers
                .iter()
                .find(|offer| offer.harness_id == "claude-code")
                .unwrap();
            assert_eq!(claude.refusal, None);
            assert_eq!(claude.version.as_deref(), Some("2.1.284"));
            assert_eq!(
                claude.binary,
                session.fake.path("bin").join("claude").to_string_lossy()
            );
            let codex = offers
                .iter()
                .find(|offer| offer.harness_id == "codex")
                .unwrap();
            assert_eq!(
                codex.refusal.as_deref(),
                Some("Codex 0.130.0 has not been reviewed for Brauser")
            );
            // An unreviewed version stops after --version.
            assert_eq!(
                session.fake.log("codex", "argv.log").as_deref(),
                Some("[--version]\n")
            );
            // Discovery never writes config.
            assert_eq!(session.revision(), revision);
        }

        #[test]
        fn setup_cannot_choose_a_summaries_folder() {
            let mut session = session(PASSING_BODY);
            let before = std::fs::read(&session.path).unwrap();
            let revision = session.revision();
            let response = session.set_up_harness(&revision, "claude-code", Some("ai"));
            let (code, message) = refused(&response);
            assert_eq!(code, ErrorCode::InvalidConfig);
            assert!(message.contains("content folders are fixed"), "{message}");
            assert_eq!(std::fs::read(&session.path).unwrap(), before);
            assert_eq!(session.fake.log("claude", "stdin.log"), None);
            // The fixed name, or none, sets up the harness.
            let response = session.set_up_harness(&revision, "claude-code", Some("summaries"));
            match response {
                Response::ConfigUpdated(value) => assert_eq!(
                    value.config.storage.unwrap().summaries_dir.as_deref(),
                    Some("summaries")
                ),
                other => panic!("expected config_updated, got {other:?}"),
            }
        }

        #[test]
        fn check_agent_reports_harness_after_setup() {
            let mut session = session(PASSING_BODY);
            assert!(matches!(session.set_up(), Response::ConfigUpdated(_)));
            match session.call(serde_json::from_str(&check_agent_json()).unwrap()) {
                Response::AgentChecked(value) => {
                    assert_eq!(value.harness_id, "claude-code");
                    assert_eq!(value.harness_version, "2.1.284");
                }
                other => panic!("expected agent_checked, got {other:?}"),
            }
        }

        #[test]
        fn get_config_reports_agent_status_states() {
            let mut session = session(PASSING_BODY);
            assert_eq!(session.status().state, AgentState::NotSetUp);
            assert!(matches!(session.set_up(), Response::ConfigUpdated(_)));
            let status = session.status();
            assert_eq!(status.state, AgentState::Ready);
            assert_eq!(status.harness_version.as_deref(), Some("2.1.284"));
            assert_eq!(status.message, None);

            // The harness updated itself in place.
            let binary = session.fake.path("bin").join("claude");
            let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
            std::fs::File::options()
                .write(true)
                .open(&binary)
                .unwrap()
                .set_modified(later)
                .unwrap();
            let status = session.status();
            assert_eq!(status.state, AgentState::HarnessProblem);
            assert_eq!(status.harness_version.as_deref(), Some("2.1.284"));

            // A v5 file whose retired privacy confirmation is false is
            // still ready: the committed harness is enough.
            let root = session.fake.path("notes");
            config::fixtures::write(
                &session.path,
                &root,
                "summaries_dir = \"summaries\"",
                "agent_denylist_confirmed = false",
                &format!(
                    "{}{}",
                    config::fixtures::agent_table(&binary),
                    config::fixtures::record_table(&binary)
                ),
            );
            assert_eq!(session.status().state, AgentState::Ready);
        }
    }

    /// DESIGN.md §6.7: delivery marks artifacts dirty and does not touch their
    /// files. Only the panel's connection close, the first connection of each
    /// local day, and the settings action regenerate them; closing settings
    /// does not.
    #[test]
    fn delivery_only_connections_leave_artifact_files_to_later_passes() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
        let occurred_at = time::OffsetDateTime::now_utc()
            .replace_minute(0)
            .unwrap()
            .replace_second(0)
            .unwrap()
            .replace_nanosecond(0)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let connect = |requests: Vec<Request>| {
            let json = requests
                .iter()
                .map(|request| serde_json::to_string(request).unwrap())
                .collect::<Vec<_>>();
            served(&json, loaded(&path))
        };
        let delivery = |focused_seconds| {
            vec![
                Request::GetConfig(GetConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "lease".into(),
                }),
                Request::RecordActivity(RecordActivityRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "deliver".into(),
                    event: ActivityEvent {
                        event_id: uuid::Uuid::new_v4().to_string(),
                        artifact_id: "work:work.example/7".into(),
                        artifact_type: "work.item".into(),
                        url: "https://work.example/items/7".into(),
                        title: Some("Work item".into()),
                        occurred_at: occurred_at.clone(),
                        incognito: false,
                        opens: 1,
                        focused_seconds,
                        background_seconds: 0,
                        modes: Vec::new(),
                        trails: Vec::new(),
                        refs: Vec::new(),
                    },
                }),
            ]
        };
        let artifact_file = || {
            let mut files = Vec::new();
            for entry in walk(&root.join("artifacts")) {
                files.push(std::fs::read_to_string(entry).unwrap());
            }
            files
        };
        fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return Vec::new();
            };
            let mut found = Vec::new();
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    found.extend(walk(&path));
                } else {
                    found.push(path);
                }
            }
            found
        }

        // The first connection of the day runs the pass, even for delivery.
        connect(delivery(60));
        let first = artifact_file();
        assert_eq!(first.len(), 1, "the daily pass writes the artifact file");

        // Later deliveries the same day leave the file for a later pass.
        connect(delivery(120));
        connect(delivery(180));
        assert_eq!(
            artifact_file(),
            first,
            "delivery must not rewrite artifact files"
        );
        // Nor does closing the settings page, which has its own action.
        connect(vec![
            Request::Hello(HelloRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "settings".into(),
            }),
            Request::GetConfig(GetConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "settings-config".into(),
            }),
        ]);
        assert_eq!(
            artifact_file(),
            first,
            "settings must not rewrite artifact files"
        );

        // The panel's connection close regenerates what delivery dirtied.
        connect(vec![Request::GetArtifact(
            brauser_protocol::GetArtifactRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "card".into(),
                url: "https://work.example/items/7".into(),
                title: Some("Work item".into()),
            },
        )]);
        let refreshed = artifact_file();
        assert_eq!(refreshed.len(), 1);
        assert_ne!(
            refreshed, first,
            "the panel's close regenerates dirty files"
        );
    }

    /// One-time move of an older install's folders to the fixed layout
    /// (DESIGN.md §6.1), against a fixture shaped like a real pre-home install.
    mod home_layout_migration {
        use super::*;
        use std::fs;
        use std::path::{Path, PathBuf};

        const FIXTURE: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/pre-home-layout"
        );
        const FORUM_NOTE: &str = "forum-example-com-item-5888e994a837414002b5aa66675dd4546e95a792d52a480a1f1b95ac4fd02c42.md";
        const NEWS_URL: &str = "https://news.example.org/region/livestory/provincial-election-results-live-updates-2026-9.1234567";

        struct Install {
            folder: tempfile::TempDir,
            home: PathBuf,
            path: PathBuf,
        }

        impl Install {
            fn new() -> Self {
                let folder = tempfile::tempdir().unwrap();
                let home = folder.path().join("notes-brauser");
                fs::create_dir_all(home.join("pages")).unwrap();
                for entry in fs::read_dir(Path::new(FIXTURE).join("home/pages")).unwrap() {
                    let entry = entry.unwrap();
                    fs::copy(entry.path(), home.join("pages").join(entry.file_name())).unwrap();
                }
                let quoted = |value: &str| toml::Value::String(value.to_owned()).to_string();
                let identity = crate::vault::selected_root_identity(&home).unwrap();
                let config = fs::read_to_string(Path::new(FIXTURE).join("config.toml"))
                    .unwrap()
                    .replace("\"@ROOT_IDENTITY@\"", &quoted(&identity))
                    .replace("\"@ROOT@\"", &quoted(&home.to_string_lossy()));
                let state = folder.path().join("state");
                fs::create_dir(&state).unwrap();
                let path = state.join("config.toml");
                fs::write(&path, config).unwrap();
                Self { folder, home, path }
            }

            fn store(&self) -> ConfigStore {
                ConfigStore::for_test(self.path.clone())
            }

            fn call(&self, config: &mut ConfigStore, request: Request) -> Response {
                dispatch(
                    request,
                    config,
                    &mut ConsentAuthority::new(),
                    &SetupContext {
                        env: &no_harnesses(self.folder.path()),
                    },
                )
            }

            fn hello(&self, config: &mut ConfigStore) -> HelloResult {
                let request = Request::Hello(brauser_protocol::HelloRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "hello".into(),
                });
                match self.call(config, request) {
                    Response::HelloResult(value) => value,
                    other => panic!("expected hello_result, got {other:?}"),
                }
            }

            fn load(&self, config: &mut ConfigStore, url: &str) -> Response {
                self.call(
                    config,
                    Request::LoadNote(brauser_protocol::LoadNoteRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "load".into(),
                        url: url.into(),
                    }),
                )
            }

            fn fixture_note(name: &str) -> Vec<u8> {
                fs::read(Path::new(FIXTURE).join("home/pages").join(name)).unwrap()
            }

            fn table(&self) -> toml::Table {
                fs::read_to_string(&self.path).unwrap().parse().unwrap()
            }
        }

        fn without_layout(mut table: toml::Table) -> toml::Table {
            let storage = table["storage"].as_table_mut().unwrap();
            for key in [
                "log_dir",
                "pages_dir",
                "later_dir",
                "artifacts_dir",
                "summaries_dir",
            ] {
                storage.remove(key);
            }
            table
        }

        #[test]
        fn a_real_pre_home_install_moves_pages_to_notes_once() {
            let install = Install::new();
            let before = install.table();
            let mut config = install.store();
            let hello = install.hello(&mut config);
            assert_eq!(hello.config_issue, None);
            assert!(hello.configured);

            assert!(!install.home.join("pages").exists());
            for entry in fs::read_dir(Path::new(FIXTURE).join("home/pages")).unwrap() {
                let name = entry.unwrap().file_name();
                assert_eq!(
                    fs::read(install.home.join("notes").join(&name)).unwrap(),
                    Install::fixture_note(name.to_str().unwrap())
                );
            }
            // Only folders that existed are renamed; nothing else is created.
            let mut names: Vec<_> = fs::read_dir(&install.home)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            assert_eq!(names, ["notes"]);

            let after = install.table();
            let storage = after["storage"].as_table().unwrap();
            for (key, value) in [
                ("log_dir", "logs"),
                ("pages_dir", "notes"),
                ("later_dir", "later"),
                ("artifacts_dir", "artifacts"),
                ("summaries_dir", "summaries"),
            ] {
                assert_eq!(storage[key].as_str(), Some(value), "{key}");
            }
            // Every other setting, including the harness and adapters, is kept.
            assert_eq!(without_layout(after), without_layout(before));

            match install.load(&mut config, NEWS_URL) {
                Response::NoteLoaded(value) => {
                    assert!(value.exists);
                    assert_eq!(value.body.trim(), "Note");
                }
                other => panic!("expected note_loaded, got {other:?}"),
            }
            // A second connection finds nothing to do.
            let bytes = fs::read(&install.path).unwrap();
            let mut second = install.store();
            assert_eq!(install.hello(&mut second).config_issue, None);
            assert_eq!(fs::read(&install.path).unwrap(), bytes);
        }

        #[test]
        fn an_existing_notes_folder_refuses_without_changing_anything_until_moved() {
            let install = Install::new();
            fs::create_dir(install.home.join("notes")).unwrap();
            fs::write(install.home.join("notes/mine.md"), "the user's own").unwrap();
            let bytes = fs::read(&install.path).unwrap();
            let mut config = install.store();
            let hello = install.hello(&mut config);
            assert!(!hello.configured);
            let issue = hello.config_issue.unwrap();
            assert!(
                issue.contains("notes/ already exists beside pages/"),
                "{issue}"
            );
            assert_eq!(fs::read(&install.path).unwrap(), bytes);
            assert_eq!(
                fs::read(install.home.join("pages").join(FORUM_NOTE)).unwrap(),
                Install::fixture_note(FORUM_NOTE)
            );
            assert_eq!(
                fs::read_to_string(install.home.join("notes/mine.md")).unwrap(),
                "the user's own"
            );
            // Notes, capture, and settings saves stay off meanwhile.
            assert!(matches!(
                install.load(&mut config, NEWS_URL),
                Response::Error(_)
            ));
            let mut next = config.snapshot().clone();
            next.storage = Some(crate::layout::fixed(next.storage.as_ref().unwrap()));
            let revision = config.revision().to_owned();
            let response = install.call(
                &mut config,
                Request::UpdateConfig(brauser_protocol::UpdateConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "save".into(),
                    config: next,
                    expected_revision: revision,
                    picker_token: None,
                    knowledge_base_token: None,
                }),
            );
            match response {
                Response::Error(value) => {
                    assert_eq!(value.code, ErrorCode::InvalidConfig);
                    assert!(
                        value.message.contains("notes/ already exists"),
                        "{}",
                        value.message
                    );
                }
                other => panic!("expected a refusal, got {other:?}"),
            }
            assert_eq!(fs::read(&install.path).unwrap(), bytes);

            // The user moves their folder; the next open finishes the move.
            fs::rename(install.home.join("notes"), install.home.join("my-notes")).unwrap();
            let hello = install.hello(&mut config);
            assert_eq!(hello.config_issue, None);
            assert!(hello.configured);
            assert_eq!(
                fs::read(install.home.join("notes").join(FORUM_NOTE)).unwrap(),
                Install::fixture_note(FORUM_NOTE)
            );
            assert_eq!(
                fs::read_to_string(install.home.join("my-notes/mine.md")).unwrap(),
                "the user's own"
            );
        }

        #[test]
        fn a_crash_between_renames_and_the_config_write_resumes() {
            let install = Install::new();
            fs::create_dir_all(install.home.join("log/2026/10")).unwrap();
            fs::write(install.home.join("log/2026/10/2026-10-26.md"), "log").unwrap();
            // As if a host renamed pages/ and then died.
            fs::rename(install.home.join("pages"), install.home.join("notes")).unwrap();
            let mut config = install.store();
            config.refresh().unwrap();
            assert!(!config.configured());
            config.migrate_layout().unwrap();
            assert!(config.configured());
            assert!(crate::layout::is_fixed(
                config.snapshot().storage.as_ref().unwrap()
            ));
            assert_eq!(
                fs::read_to_string(install.home.join("logs/2026/10/2026-10-26.md")).unwrap(),
                "log"
            );
            assert!(!install.home.join("log").exists());
            assert_eq!(
                fs::read(install.home.join("notes").join(FORUM_NOTE)).unwrap(),
                Install::fixture_note(FORUM_NOTE)
            );
            // Intents that name log/ are pointed at logs/ before it moves.
            let relocations = fs::read_to_string(
                install
                    .path
                    .parent()
                    .unwrap()
                    .join("visit-ids/log-relocations.json"),
            )
            .unwrap();
            assert!(
                relocations.contains("\"from\":\"log\",\"to\":\"logs\""),
                "{relocations}"
            );

            // Every folder renamed, config still old: only the config changes.
            let install = Install::new();
            fs::rename(install.home.join("pages"), install.home.join("notes")).unwrap();
            let mut config = install.store();
            config.migrate_layout().unwrap();
            assert!(config.configured());
            assert!(crate::layout::is_fixed(
                config.snapshot().storage.as_ref().unwrap()
            ));
        }

        #[test]
        fn migration_waits_for_every_writer_and_runs_once_across_hosts() {
            for hold_capture in [false, true] {
                let install = Install::new();
                let state = install.path.parent().unwrap().to_path_buf();
                // A writer in another host process holds its lock.
                let writer = install.store();
                let held = if hold_capture {
                    crate::capture::lock_capture(&state).unwrap()
                } else {
                    writer.lock_current().unwrap()
                };
                let hosts: Vec<_> = (0..3)
                    .map(|_| {
                        let mut store = install.store();
                        std::thread::spawn(move || {
                            store.migrate_layout().unwrap();
                            store.configured()
                        })
                    })
                    .collect();
                std::thread::sleep(std::time::Duration::from_millis(300));
                assert!(
                    install.home.join("pages").is_dir(),
                    "hold_capture={hold_capture}"
                );
                assert!(
                    !install.home.join("notes").exists(),
                    "hold_capture={hold_capture}"
                );
                drop(held);
                for host in hosts {
                    assert!(host.join().unwrap(), "hold_capture={hold_capture}");
                }
                assert!(!install.home.join("pages").exists());
                assert_eq!(
                    fs::read(install.home.join("notes").join(FORUM_NOTE)).unwrap(),
                    Install::fixture_note(FORUM_NOTE)
                );
            }
        }

        #[test]
        fn an_update_cannot_choose_its_own_folder_names() {
            let install = Install::new();
            let mut config = install.store();
            assert!(install.hello(&mut config).configured);
            let mut next = config.snapshot().clone();
            next.storage.as_mut().unwrap().pages_dir = "pages".into();
            let revision = config.revision().to_owned();
            match install.call(
                &mut config,
                Request::UpdateConfig(brauser_protocol::UpdateConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "save".into(),
                    config: next,
                    expected_revision: revision,
                    picker_token: None,
                    knowledge_base_token: None,
                }),
            ) {
                Response::Error(value) => {
                    assert_eq!(value.code, ErrorCode::InvalidConfig);
                    assert!(
                        value.message.contains("content folders are fixed"),
                        "{}",
                        value.message
                    );
                }
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
    }
}

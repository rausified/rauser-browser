//! Chrome native messaging with the extension. Framing lives in
//! `crate::frame`; stdout contains frames only.

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
use crate::config::{self, ConfigStore};
use crate::consent::ConsentAuthority;
use crate::frame::{FrameError, read_frame, write_frame};
use crate::harness::HarnessEnv;
use crate::harness_setup;
use crate::index::ArtifactIndex;
use crate::note;
#[cfg(unix)]
use crate::service::client::{ServiceError, ServiceLink};
use crate::vault::Vault;

const MAX_REQUEST_ID_CHARS: usize = 128;

pub fn serve(config: ConfigStore) -> Result<()> {
    let input = io::stdin();
    let output = io::stdout();
    serve_with_io(input.lock(), output.lock(), config)
}

/// What harness setup and the run proxy may use from the host process.
/// Built once per connection; tests build it by hand with a fake search path
/// and, for run messages, a fake service.
pub struct SetupContext<'a> {
    pub env: &'a HarnessEnv,
    /// Where run messages go. `None` answers them `service_unavailable`.
    #[cfg(unix)]
    pub service: Option<&'a ServiceLink>,
}

impl<'a> SetupContext<'a> {
    /// A context with no service link.
    pub fn new(env: &'a HarnessEnv) -> Self {
        Self {
            env,
            #[cfg(unix)]
            service: None,
        }
    }
}

/// Process requests until the browser closes the native messaging pipe.
pub fn serve_with_io<R: Read, W: Write>(input: R, output: W, config: ConfigStore) -> Result<()> {
    let env = HarnessEnv::from_process(config.dir()?);
    // Building the link never connects; only a run message does.
    #[cfg(unix)]
    let link = ServiceLink::for_config(&config)
        .inspect_err(|link_error| {
            eprintln!("{NAMESPACE}: the service cannot be reached: {link_error:#}");
        })
        .ok();
    let context = SetupContext {
        #[cfg(unix)]
        service: link.as_ref(),
        ..SetupContext::new(&env)
    };
    serve_with_context(input, output, config, &context)
}

pub fn serve_with_context<R: Read, W: Write>(
    mut input: R,
    mut output: W,
    mut config: ConfigStore,
    context: &SetupContext<'_>,
) -> Result<()> {
    let mut consent = ConsentAuthority::new();
    loop {
        let body = match read_frame(&mut input) {
            Ok(Some(body)) => body,
            Ok(None) => {
                regenerate_artifacts(&mut config);
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
        let response = dispatch_json(&body, &mut config, &mut consent, context);
        write_frame(&mut output, &response)?;
    }
}

fn dispatch_json(
    body: &[u8],
    config: &mut ConfigStore,
    consent: &mut ConsentAuthority,
    context: &SetupContext<'_>,
) -> Response {
    match parse_envelope(body) {
        Ok((_, request)) => dispatch(request, config, consent, context),
        Err(response) => response,
    }
}

/// Read the common envelope before the version-specific request. A newer
/// peer may add fields or message types this build cannot deserialize, so
/// the request id and version are checked first and a mismatch answers
/// `unsupported_protocol_version`. Shared by the native host and the
/// service; the error is the response to send.
// The error is the response itself, sent as is; boxing it would only add an
// allocation to the rare refusal path.
#[allow(clippy::result_large_err)]
pub(crate) fn parse_envelope(body: &[u8]) -> std::result::Result<(String, Request), Response> {
    let envelope: serde_json::Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return Err(error("", ErrorCode::InvalidRequest, "invalid request JSON")),
    };
    let Some(object) = envelope.as_object() else {
        return Err(error(
            "",
            ErrorCode::InvalidRequest,
            "request must be a JSON object",
        ));
    };
    let Some(id) = object.get("request_id").and_then(serde_json::Value::as_str) else {
        return Err(error(
            "",
            ErrorCode::InvalidRequest,
            "request_id is required",
        ));
    };
    if !valid_request_id(id) {
        return Err(error(
            "",
            ErrorCode::InvalidRequest,
            "request_id must be 1–128 non-control characters",
        ));
    }
    let Some(version) = object
        .get("protocol_version")
        .and_then(serde_json::Value::as_u64)
    else {
        return Err(error(
            id,
            ErrorCode::InvalidRequest,
            "protocol_version is required",
        ));
    };
    if version != u64::from(PROTOCOL_VERSION) {
        return Err(error(
            id,
            ErrorCode::UnsupportedProtocolVersion,
            "extension and host protocol versions differ",
        ));
    }
    match serde_json::from_slice::<Request>(body) {
        Ok(request) => Ok((id.to_owned(), request)),
        Err(_) => Err(error(
            id,
            ErrorCode::InvalidRequest,
            "invalid or unsupported request JSON",
        )),
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
            let _ = config.refresh();
            Response::HelloResult(HelloResult {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                host_version: env!("CARGO_PKG_VERSION").to_owned(),
                configured: config.configured(),
                config_issue: config.config_issue().map(str::to_owned),
            })
        }
        Request::GetConfig(value) => {
            let _ = config.refresh();
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
                Err(_) => error(
                    &value.request_id,
                    ErrorCode::Internal,
                    "could not save configuration",
                ),
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
        #[cfg(unix)]
        Request::StartRun(value) => start_run(value, config, context.service),
        // Run ids and offsets are the service's to check.
        #[cfg(unix)]
        request @ (Request::GetRun(_)
        | Request::ListRuns(_)
        | Request::CancelRun(_)
        | Request::RetrySave(_)) => forward(&request, context.service),
        #[cfg(not(unix))]
        Request::StartRun(_)
        | Request::GetRun(_)
        | Request::ListRuns(_)
        | Request::CancelRun(_)
        | Request::RetrySave(_) => error(
            id,
            ErrorCode::ServiceUnavailable,
            "The Brauser service is not supported on Windows.",
        ),
        // service_hello is host-to-service only.
        Request::ServiceHello(_) => error(
            id,
            ErrorCode::InvalidRequest,
            "service_hello is not accepted from the extension",
        ),
    }
}

/// Check what only the host may decide, then forward: the record key is
/// always the one the page's note uses, never the extension's.
#[cfg(unix)]
fn start_run(
    mut value: brauser_protocol::StartRunRequest,
    config: &mut ConfigStore,
    service: Option<&ServiceLink>,
) -> Response {
    if value.record_key.is_some() {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "record_key is chosen by the host",
        );
    }
    if let Err(message) = crate::service::record::check_start_run(
        &value.url,
        &value.title,
        &value.content,
        value.selection.as_deref(),
    ) {
        return error(&value.request_id, ErrorCode::InvalidRequest, message);
    }
    if crate::capture::parse_event_time(&value.requested_at)
        .and_then(crate::capture::check_event_window)
        .is_err()
    {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "requested_at must be a current RFC 3339 timestamp",
        );
    }
    // Resolve under the same lock as note loading so a config update cannot
    // interleave; release it before forwarding, since the service takes it
    // too when it reads the run's working directory.
    let resolved = match config.lock_current() {
        Ok(_config_lock) => resolve_record_key(config, &value.url),
        Err(_) => Err((ErrorCode::Internal, "configuration is unavailable")),
    };
    match resolved {
        Ok(key) => value.record_key = Some(key),
        Err((code, message)) => return error(&value.request_id, code, message),
    }
    forward(&Request::StartRun(value), service)
}

/// Send a run message to the service and return its answer as is.
#[cfg(unix)]
fn forward(request: &Request, service: Option<&ServiceLink>) -> Response {
    let id = request.request_id();
    let Some(service) = service else {
        return error(
            id,
            ErrorCode::ServiceUnavailable,
            "The Brauser service cannot be reached from this configuration folder.",
        );
    };
    match service.request(request) {
        Ok(response) => response,
        Err(ServiceError::Unavailable(message)) => {
            error(id, ErrorCode::ServiceUnavailable, &message)
        }
        Err(ServiceError::Protocol(detail)) => {
            eprintln!("{NAMESPACE}: service protocol error: {detail}");
            error(
                id,
                ErrorCode::Internal,
                "The Brauser service sent an unexpected reply.",
            )
        }
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
        return retryable("choose a notes folder before capture");
    };
    let store = match config
        .dir()
        .and_then(|dir| CaptureStore::open_in(storage, config.root_identity(), dir))
    {
        Ok(store) => store,
        Err(_) => return retryable("notes folder is unavailable"),
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
            Some("choose a notes folder before capture".into()),
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
                Some("notes folder is unavailable".into()),
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
            reason: Some("choose a notes folder before capture".into()),
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
                reason: Some("notes folder is unavailable".into()),
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
        && let Ok(index) = ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir)
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
            Some("choose a notes folder before capture".into()),
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
                Some("notes folder is unavailable".into()),
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
        && let Ok(index) = ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir)
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
        && let Ok(index) = ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir)
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

fn reconcile_index_from_logs(config: &ConfigStore) {
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return;
    };
    if let Ok(cache_dir) = config.cache_dir()
        && let Ok(index) = ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir)
        && let Err(error) = index.reconcile_from_logs()
    {
        eprintln!("{NAMESPACE}: warning: could not reconcile artifact index: {error:#}");
    }
}

fn regenerate_artifacts(config: &mut ConfigStore) {
    let Ok(_lock) = config.lock_current() else {
        return;
    };
    if config.refresh().is_err() || config.config_issue().is_some() {
        return;
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return;
    };
    if let Ok(cache_dir) = config.cache_dir()
        && let Ok(index) = ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir)
        && let Err(error) = index.regenerate_dirty()
    {
        eprintln!("{NAMESPACE}: warning: could not regenerate artifact files: {error:#}");
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

/// The key-only part of `note_context`, shared by notes, the run proxy, and
/// the run finisher, so a run and the page's note always agree on the record
/// they belong to. It never lists older notes. Callers hold `lock_current`.
// Only the Unix run proxy calls it outside tests.
#[cfg_attr(not(any(test, unix)), allow(dead_code))]
pub(crate) fn resolve_record_key(
    config: &mut ConfigStore,
    url: &str,
) -> Result<RecordKey, (ErrorCode, &'static str)> {
    resolve_key_parts(config, url).map(|parts| parts.key)
}

/// What key resolution already computed, kept so `note_context` can build on
/// it without opening the vault or matching adapters a second time.
struct KeyParts {
    canonical: String,
    vault: Vault,
    key: RecordKey,
}

fn resolve_key_parts(
    config: &mut ConfigStore,
    url: &str,
) -> Result<KeyParts, (ErrorCode, &'static str)> {
    if config.refresh().is_err() || config.config_issue().is_some() {
        return Err((
            ErrorCode::InvalidConfig,
            "configuration is unavailable or needs repair",
        ));
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return Err((ErrorCode::NotConfigured, "choose a notes folder first"));
    };
    let canonical = crate::capture::canonical_url(url, &config.snapshot().strip_params)
        .map_err(|_| (ErrorCode::InvalidRequest, "page URL is invalid"))?;
    let vault = Vault::open_checked(storage, config.root_identity())
        .map_err(|_| (ErrorCode::Internal, "notes folder is unavailable"))?;
    // Match on the original URL, not the stripped canonical form: capture
    // (record_activity, record_why) evaluates adapter identity before any
    // strip-parameter normalization, so a parameter an adapter's match rule
    // requires must still be visible here, or this path and capture would
    // disagree on whether the page is recognized.
    let matched = config.adapter_match(url, None).ok();
    let key =
        match matched.as_ref() {
            Some(found) => RecordKey {
                kind: RecordKeyKind::Artifact,
                value: found.artifact_id.clone(),
            },
            // No adapter currently recognizes this URL (for example, its
            // adapter or last enabled site was removed). Before falling back to
            // a fresh URL key, check whether it was previously recorded under an
            // artifact, so that artifact's existing shared note stays reachable
            // instead of appearing empty. The note's stored identity
            // survives even without delivered activity; indexed history
            // also recovers notes created on a different alternate route.
            None => {
                // Collect every distinct match from BOTH sources into one set
                // before deciding uniqueness: stripped query identity can make
                // two different artifacts share the same canonical URL, and
                // trusting either source alone -- even one that is itself
                // unambiguous -- could still miss a conflicting match the
                // other source holds. Guessing between them would silently
                // redirect a save into an unrelated artifact's note, so the
                // recovery is trusted only when the union is unambiguous.
                let unavailable = || {
                    (
                        ErrorCode::Internal,
                        "note identity is unavailable; recovery lookup failed",
                    )
                };
                let mut candidates = std::collections::BTreeSet::new();
                let note_candidates = vault
                    .note_identity_candidates()
                    .map_err(|_| unavailable())?;
                candidates.extend(note_candidates.iter().filter_map(|contents| {
                    note::artifact_keyed_note_identity(contents, &canonical)
                }));
                // Failure is not an empty candidate set: a missing source can
                // conceal a conflicting identity and redirect an autosave.
                let cache_dir = config.cache_dir().map_err(|_| unavailable())?;
                let index = ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir)
                    .map_err(|_| unavailable())?;
                candidates.extend(
                    index
                        .artifact_candidates_for_url(&canonical)
                        .map_err(|_| unavailable())?,
                );
                match candidates.len() {
                    1 => RecordKey {
                        kind: RecordKeyKind::Artifact,
                        value: candidates
                            .into_iter()
                            .next()
                            .expect("checked len == 1 above"),
                    },
                    _ => crate::vault::url_record_key(&canonical)
                        .expect("canonical HTTP URL has a key"),
                }
            }
        };
    Ok(KeyParts {
        canonical,
        vault,
        key,
    })
}

fn note_context(
    config: &mut ConfigStore,
    url: &str,
    include_older_notes: bool,
) -> Result<NoteContext, (ErrorCode, &'static str)> {
    let KeyParts {
        canonical,
        vault,
        key,
    } = resolve_key_parts(config, url)?;
    let mut older_urls = vec![canonical.clone()];
    if key.kind == RecordKeyKind::Artifact && include_older_notes {
        let Some(storage) = config.snapshot().storage.as_ref() else {
            return Err((ErrorCode::NotConfigured, "choose a notes folder first"));
        };
        if let Ok(cache_dir) = config.cache_dir()
            && let Ok(index) = ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir)
            && let Ok(urls) = index.artifact_urls(&key.value)
        {
            older_urls.extend(urls);
        }
        if let Ok(candidates) = vault.note_candidates() {
            for contents in candidates {
                if let Some(candidate) = note::artifact_keyed_note_url(&contents, &key) {
                    older_urls.push(candidate);
                }
                let Some(candidate) = note::url_keyed_note_url(&contents) else {
                    continue;
                };
                if config
                    .adapter_match(&candidate, None)
                    .is_ok_and(|candidate_match| candidate_match.artifact_id == key.value)
                {
                    older_urls.push(candidate);
                }
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
            "choose a notes folder first",
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
    let index = match ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir) {
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
        return Err((ErrorCode::NotConfigured, "choose a notes folder first"));
    };
    let unavailable = || (ErrorCode::Internal, "artifact index is unavailable");
    let cache_dir = config.cache_dir().map_err(|_| unavailable())?;
    ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir).map_err(|_| unavailable())
}

fn local_date(value: &str) -> Option<time::Date> {
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]").ok()?;
    time::Date::parse(value, &format).ok()
}

pub(crate) fn error(request_id: &str, code: ErrorCode, message: &str) -> Response {
    Response::Error(ErrorResponse {
        protocol_version: PROTOCOL_VERSION,
        request_id: request_id.to_owned(),
        code,
        message: message.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{MAX_INBOUND_BYTES, MAX_OUTBOUND_BYTES};
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
        let context = SetupContext::new(&env);

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
        let context = SetupContext::new(&env);
        for root in [existing.clone(), folder.path().join("absent")] {
            let snapshot = ConfigSnapshot {
                storage: Some(brauser_protocol::StorageConfig {
                    root: root.to_string_lossy().into_owned(),
                    profile: "neutral".into(),
                    log_dir: "log".into(),
                    pages_dir: "pages".into(),
                    later_dir: "later".into(),
                    artifacts_dir: "artifacts".into(),
                    summaries_dir: None,
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
        let context = SetupContext::new(&env);
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
        let context = SetupContext::new(&env);
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
                log_dir: "log".into(),
                pages_dir: "pages".into(),
                later_dir: "later".into(),
                artifacts_dir: "artifacts".into(),
                summaries_dir: None,
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
    fn run_key_matches_page_note_key() {
        // Plain URL on the configured store: a URL key, identical to load_note's.
        let folder = tempfile::tempdir().unwrap();
        let mut plain = configured_store(folder.path());
        let url = "https://example.com/a?b=1";
        let resolved = resolve_record_key(&mut plain, url).unwrap();
        assert_eq!(resolved.kind, RecordKeyKind::Url);
        assert_eq!(resolved, note_load(&mut plain, url).key);

        // Adapter URL: an artifact key, identical to load_note's.
        let adapter_folder = tempfile::tempdir().unwrap();
        let root = adapter_folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = adapter_folder.path().join("config.toml");
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
        let mut adapter = loaded(&path);
        let doc = "https://docs.google.com/document/d/abcdefghijklmnopqrst/edit";
        let resolved = resolve_record_key(&mut adapter, doc).unwrap();
        assert_eq!(resolved.kind, RecordKeyKind::Artifact);
        assert_eq!(resolved, note_load(&mut adapter, doc).key);

        // Errors match note_context's: no storage is not_configured.
        let empty_folder = tempfile::tempdir().unwrap();
        let mut empty = ConfigStore::for_test(empty_folder.path().join("config.toml"));
        assert!(matches!(
            resolve_record_key(&mut empty, url),
            Err((ErrorCode::NotConfigured, _))
        ));
        assert!(matches!(
            resolve_record_key(&mut plain, "not a url"),
            Err((ErrorCode::InvalidRequest, _))
        ));
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
    fn a_removed_adapters_shared_note_stays_reachable_by_its_recorded_history() {
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
        match note_save(
            &mut config,
            url,
            "keep this text",
            &loaded_before.revision,
            loaded_before.key,
        ) {
            Response::NoteSaved(_) => {}
            other => panic!("expected note_saved, got {other:?}"),
        }

        // Remove the adapter. The URL no longer resolves through any
        // adapter, as if its site had been removed.
        write_adapter_config(&path, &root, "");

        let loaded_after = note_load(&mut config, url);
        assert!(
            loaded_after.exists,
            "the shared artifact note must stay discoverable after the adapter is removed"
        );
        assert_eq!(loaded_after.body, "keep this text");
    }

    #[test]
    fn a_note_saved_without_any_activity_stays_reachable_after_the_adapter_is_removed() {
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
        // simply not delivered yet) -- there is no index row to recover
        // from, only the note's own stored identity.
        let loaded_before = note_load(&mut config, url);
        assert_eq!(loaded_before.key.kind, RecordKeyKind::Artifact);
        match note_save(
            &mut config,
            url,
            "keep this text",
            &loaded_before.revision,
            loaded_before.key,
        ) {
            Response::NoteSaved(_) => {}
            other => panic!("expected note_saved, got {other:?}"),
        }

        write_adapter_config(&path, &root, "");

        let loaded_after = note_load(&mut config, url);
        assert!(
            loaded_after.exists,
            "a note saved with no activity history must still be found by its own stored identity"
        );
        assert_eq!(loaded_after.body, "keep this text");
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

    mod note_policy_removal {
        use super::*;

        const EDIT: &str = "https://work.example/items/7/edit";
        const VIEW: &str = "https://work.example/items/7/view";
        const CONTEXT: &str = "https://work.example/items/7/context";

        struct Session {
            folder: tempfile::TempDir,
            root: std::path::PathBuf,
            path: std::path::PathBuf,
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
                Self { folder, root, path }
            }

            // Exercise real request/response framing and reopen the host for
            // every request, so config history and recovery must survive EOF.
            fn call(&self, request: Request) -> Response {
                let mut output = Vec::new();
                serve_with_context(
                    Cursor::new(encoded_request(request)),
                    &mut output,
                    loaded(&self.path),
                    &SetupContext::new(&no_harnesses(self.folder.path())),
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

            fn activity(&self) -> ActivityEvent {
                ActivityEvent {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    artifact_id: "work:work.example/7".into(),
                    artifact_type: "work.item".into(),
                    url: VIEW.into(),
                    title: Some("Item 7".into()),
                    occurred_at: "2026-10-16T13:00:00Z".into(),
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
                let log_path = session.root.join("log/2026/10/2026-10-16.md");
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
                        occurred_at: "2026-10-16T13:00:00Z".into(),
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
                            occurred_at: "2026-10-16T13:00:00Z".into(),
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
                    !session.root.join("log").exists(),
                    "notes cannot restart capture"
                );
            }
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
        fn an_incomplete_note_scan_cannot_choose_an_autosave_target() {
            for fault in ["count", "file bytes", "total bytes", "unreadable"] {
                let session = Session::new();
                let first = session.load(EDIT);
                let Response::NoteSaved(saved) = session.save(EDIT, &first, "Keep original text")
                else {
                    panic!("expected saved note")
                };
                let current = session.load(EDIT);
                session.policy("adapter");
                let note_path = session.root.join(saved.relative_path);
                let original = std::fs::read(&note_path).unwrap();
                let pages = session.root.join("pages");
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
                let response = session.call(Request::LoadNote(brauser_protocol::LoadNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "incomplete-scan".into(),
                    url: EDIT.into(),
                }));
                assert!(
                    matches!(response, Response::Error(reply) if reply.request_id == "incomplete-scan" && reply.message.contains("note identity is unavailable")),
                    "{fault}"
                );
                assert!(
                    matches!(session.save(EDIT, &current, "Must not autosave"), Response::Error(reply) if reply.message.contains("note identity is unavailable")),
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
                assert_eq!(session.load(EDIT).body, "Keep original text");
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
            &SetupContext::new(&no_harnesses(folder.path())),
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
            &SetupContext::new(&no_harnesses(folder.path())),
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
        let context = SetupContext::new(&env);
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
        served_with(requests, config, &SetupContext::new(&env))
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
        assert!(!root.join("log").exists());

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
        assert!(!root.join("log").exists());
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
                    assert!(!root.join("log").exists(), "{label}, batch={batched}");
                    assert!(!config.cache_dir().unwrap().join("index.db").exists());
                }
                outcomes.push((outcome, reason, relative_path));
            }
            assert_eq!(outcomes[0], outcomes[1], "{label}");
        }
    }

    #[test]
    fn unavailable_history_cannot_select_a_partial_note_identity() {
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
                let fault = root.join("log/oversized.md");
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
            for response in responses {
                let Response::Error(reply) = response else {
                    panic!("history failure must not resolve a note identity: {response:?}")
                };
                assert_eq!(reply.code, ErrorCode::Internal);
                assert!(reply.message.contains("note identity is unavailable"));
            }
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
                let fault = root.join("log/oversized.md");
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
        regenerate_artifacts(&mut config);
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
        assert!(!root.join("log").exists());
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
                let context = SetupContext::new(&self.env);
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
        fn summaries_overlap_at_setup_is_recoverable_invalid_config() {
            let mut session = session(PASSING_BODY);
            let before = std::fs::read(&session.path).unwrap();
            let revision = session.revision();
            let response = session.set_up_harness(&revision, "claude-code", Some("log"));
            let (code, message) = refused(&response);
            assert_eq!(code, ErrorCode::InvalidConfig);
            assert!(message.contains("must not overlap"), "{message}");
            assert_eq!(std::fs::read(&session.path).unwrap(), before);
            assert_eq!(session.fake.log("claude", "stdin.log"), None);
            // Choose a different folder and set up again.
            let response = session.set_up_harness(&revision, "claude-code", Some("ai"));
            match response {
                Response::ConfigUpdated(value) => assert_eq!(
                    value.config.storage.unwrap().summaries_dir.as_deref(),
                    Some("ai")
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

    /// The run proxy against a fake service on a short `/tmp` socket. No
    /// launchd, no real service, and no harness is ever involved.
    #[cfg(unix)]
    mod run_proxy {
        use super::*;
        use crate::service::client::fake::{self, FakeService};
        use crate::service::client::{Launch, NOT_INSTALLED, ServiceLink};
        use brauser_protocol::{
            CancelRunRequest, GetRunRequest, LoadNoteRequest, RunStarted, ServiceHelloRequest,
            StartRunRequest,
        };
        use std::sync::Arc;
        use std::time::Duration;

        const BUILD: &str = "this-build";

        fn now() -> String {
            time::OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        }

        fn start_run(url: &str, record_key: Option<RecordKey>, requested_at: &str) -> Request {
            Request::StartRun(StartRunRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "run-1".into(),
                url: url.into(),
                title: "A page".into(),
                content: "Some text".into(),
                selection: None,
                harness: None,
                record_key,
                requested_at: requested_at.into(),
            })
        }

        fn link(socket: std::path::PathBuf, launch: fn(bool) -> Launch) -> ServiceLink {
            ServiceLink {
                restart_lock: socket.with_file_name("restart.lock"),
                socket,
                launch: Box::new(launch),
                wait: Duration::from_secs(2),
                build_id: BUILD.into(),
                disk_build: || BUILD.to_owned(),
            }
        }

        fn never(_: bool) -> Launch {
            panic!("the service must not be started")
        }

        /// Dispatch one request with `link` as the connection's service.
        fn call(config: &mut ConfigStore, link: &ServiceLink, request: Request) -> Response {
            let folder = tempfile::tempdir().unwrap();
            let env = no_harnesses(folder.path());
            let context = SetupContext {
                service: Some(link),
                ..SetupContext::new(&env)
            };
            dispatch(request, config, &mut ConsentAuthority::new(), &context)
        }

        /// A fake service whose start_run answer echoes the forwarded key.
        fn echoing_service(socket: &std::path::Path) -> FakeService {
            FakeService::listen(
                socket,
                Arc::new(|request, _| match request {
                    Request::ServiceHello(_) => fake::ready(request, BUILD),
                    Request::StartRun(value) => Response::RunStarted(RunStarted {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id.clone(),
                        run: fake::run_view(value.record_key.clone().unwrap()),
                        existing: false,
                    }),
                    _ => fake::runs_listed(request),
                }),
            )
        }

        #[test]
        fn start_run_forwards_host_resolved_key() {
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let url = "https://example.com/a?b=1";
            let expected = note_load(&mut config, url).key;
            let dir = fake::short_dir();
            let socket = fake::socket_in(&dir);
            let service = echoing_service(&socket);
            let response = call(
                &mut config,
                &link(socket, never),
                start_run(url, None, &now()),
            );
            match response {
                Response::RunStarted(started) => assert_eq!(started.run.record_key, expected),
                other => panic!("expected run_started, got {other:?}"),
            }
            let requests = service.requests();
            assert!(matches!(requests[0], Request::ServiceHello(_)));
            match &requests[1] {
                Request::StartRun(forwarded) => {
                    assert_eq!(forwarded.record_key.as_ref(), Some(&expected));
                    assert_eq!(forwarded.url, url);
                    assert_eq!(forwarded.content, "Some text");
                }
                other => panic!("expected start_run, got {other:?}"),
            }
        }

        /// The key is resolved under the same config lock as note loading,
        /// so a config update cannot interleave with it.
        #[test]
        fn start_run_resolves_the_key_under_the_config_lock() {
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let url = "https://example.com/a";
            let expected = note_load(&mut config, url).key;
            let dir = fake::short_dir();
            let socket = fake::socket_in(&dir);
            let service = echoing_service(&socket);
            let held = config.lock_current().unwrap();
            std::thread::scope(|scope| {
                let worker = scope.spawn(|| {
                    call(
                        &mut config,
                        &link(socket.clone(), never),
                        start_run(url, None, &now()),
                    )
                });
                std::thread::sleep(Duration::from_millis(300));
                assert!(!worker.is_finished(), "start_run ran without the lock");
                assert!(service.requests().is_empty());
                drop(held);
                match worker.join().unwrap() {
                    Response::RunStarted(started) => assert_eq!(started.run.record_key, expected),
                    other => panic!("expected run_started, got {other:?}"),
                }
            });
        }

        #[test]
        fn extension_supplied_record_key_is_rejected_without_connecting() {
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let dir = fake::short_dir();
            let key = RecordKey {
                kind: RecordKeyKind::Artifact,
                value: "gdoc:docs.google.com/abc".into(),
            };
            let response = call(
                &mut config,
                &link(fake::socket_in(&dir), never),
                start_run("https://example.com/", Some(key), &now()),
            );
            assert_eq!(
                refused(&response),
                (
                    ErrorCode::InvalidRequest,
                    "record_key is chosen by the host"
                )
            );
        }

        #[test]
        fn start_run_with_an_unparseable_requested_at_is_invalid_request_without_connecting() {
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let dir = fake::short_dir();
            let link = link(fake::socket_in(&dir), never);
            for requested_at in ["yesterday", "2026-10-26 00:00:00Z", "2001-01-01T00:00:00Z"] {
                let response = call(
                    &mut config,
                    &link,
                    start_run("https://example.com/", None, requested_at),
                );
                assert_eq!(
                    refused(&response).0,
                    ErrorCode::InvalidRequest,
                    "{requested_at}"
                );
            }
        }

        #[test]
        fn start_run_over_the_schema_bounds_is_invalid_request_without_connecting() {
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let dir = fake::short_dir();
            let link = link(fake::socket_in(&dir), never);
            let oversized = |change: &dyn Fn(&mut StartRunRequest)| {
                let Request::StartRun(mut value) = start_run("https://example.com/", None, &now())
                else {
                    unreachable!()
                };
                change(&mut value);
                Request::StartRun(value)
            };
            for request in [
                oversized(&|value| value.title = "t".repeat(2049)),
                oversized(&|value| {
                    value.url = format!("https://example.com/{}", "a".repeat(8192));
                }),
                oversized(&|value| value.content = "c".repeat(1024 * 1024 + 1)),
                oversized(&|value| value.selection = Some("s".repeat(64 * 1024 + 1))),
            ] {
                let response = call(&mut config, &link, request);
                assert_eq!(
                    refused(&response).0,
                    ErrorCode::InvalidRequest,
                    "{response:?}"
                );
            }
        }

        #[test]
        fn start_run_key_errors_are_reported_without_connecting() {
            let empty = tempfile::tempdir().unwrap();
            let mut config = ConfigStore::for_test(empty.path().join("config.toml"));
            let dir = fake::short_dir();
            let response = call(
                &mut config,
                &link(fake::socket_in(&dir), never),
                start_run("https://example.com/", None, &now()),
            );
            assert_eq!(refused(&response).0, ErrorCode::NotConfigured);
        }

        #[test]
        fn not_installed_service_gives_service_unavailable() {
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let dir = fake::short_dir();
            let link = link(fake::socket_in(&dir), |_| Launch::NotInstalled);
            for request in [
                start_run("https://example.com/", None, &now()),
                Request::GetRun(GetRunRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "get-1".into(),
                    run_id: "r".into(),
                    since_offset: 0,
                }),
            ] {
                let id = request.request_id().to_owned();
                let response = call(&mut config, &link, request);
                assert_eq!(response.request_id(), id);
                assert_eq!(
                    refused(&response),
                    (ErrorCode::ServiceUnavailable, NOT_INSTALLED)
                );
            }
        }

        #[test]
        fn run_messages_other_than_start_are_forwarded_verbatim() {
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let dir = fake::short_dir();
            let socket = fake::socket_in(&dir);
            let service = echoing_service(&socket);
            let link = link(socket, never);
            let cancel = Request::CancelRun(CancelRunRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "cancel-1".into(),
                run_id: "not-checked-by-the-host".into(),
            });
            let response = call(&mut config, &link, cancel.clone());
            assert_eq!(response.request_id(), "cancel-1");
            assert_eq!(service.requests()[1], cancel);
        }

        #[test]
        fn service_hello_from_the_extension_is_refused() {
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let dir = fake::short_dir();
            let response = call(
                &mut config,
                &link(fake::socket_in(&dir), never),
                Request::ServiceHello(ServiceHelloRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "hello-1".into(),
                }),
            );
            assert_eq!(refused(&response).0, ErrorCode::InvalidRequest);
        }

        #[test]
        fn notes_and_capture_do_not_touch_the_service() {
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let dir = fake::short_dir();
            let socket = fake::socket_in(&dir);
            let service = echoing_service(&socket);
            let link = link(socket, never);
            let url = "https://example.com/page";
            let loaded = match call(
                &mut config,
                &link,
                Request::LoadNote(LoadNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "load-1".into(),
                    url: url.into(),
                }),
            ) {
                Response::NoteLoaded(note) => note,
                other => panic!("expected note_loaded, got {other:?}"),
            };
            let saved = call(
                &mut config,
                &link,
                Request::SaveNote(brauser_protocol::SaveNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "save-1".into(),
                    url: url.into(),
                    title: "Page".into(),
                    body: "A note".into(),
                    expected_revision: loaded.revision,
                    expected_key: loaded.key,
                }),
            );
            assert!(matches!(saved, Response::NoteSaved(_)), "{saved:?}");
            for request in [
                hello_request("hello-1"),
                Request::GetConfig(GetConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "config-1".into(),
                }),
                Request::RecordVisit(RecordVisitRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "visit-1".into(),
                    event: VisitEvent {
                        event_id: "00000000-0000-4000-8000-0000000000aa".into(),
                        url: url.into(),
                        title: Some("Page".into()),
                        occurred_at: now(),
                        incognito: false,
                    },
                }),
            ] {
                assert!(
                    !matches!(call(&mut config, &link, request), Response::Error(ref e) if e.code == ErrorCode::ServiceUnavailable)
                );
            }
            assert!(service.requests().is_empty());
        }
    }
}

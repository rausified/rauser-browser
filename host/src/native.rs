//! Chrome native messaging with the extension. Framing lives in
//! `crate::frame`; stdout contains frames only.

use std::io::{self, Read, Write};

use anyhow::{Context, Result, bail};
use brauser_protocol::{
    ActivityRecorded, ArtifactListResult, ArtifactResult, ArtifactView, ArtifactsRefreshed,
    BatchEventResult, BatchRecorded, CaptureEventKind, ConfigResult, ConfigUpdated, ErrorCode,
    ErrorResponse, FolderChosen, HelloResult, NoteConflict, NoteLoaded, NoteSaved,
    PROTOCOL_VERSION, RecordKey, RecordKeyKind, Request, Response, VisitOutcome, VisitRecorded,
    WhyRecorded, WorklogResult,
};

use crate::brand::NAMESPACE;
use crate::capture::{CaptureOutcome, CapturePolicy, CaptureStore, activity_shape_is_valid};
use crate::config::{self, ConfigStore, NoteKeyRule};
use crate::consent::ConsentAuthority;
use crate::frame::{FrameError, read_frame, write_frame};
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

/// What the run proxy and `open_summary` may use from the host process.
/// Built once per connection; tests build it by hand with a fake service or
/// a recording opener.
pub struct SetupContext<'a> {
    /// Where run and harness setup messages go. `None` answers them
    /// `service_unavailable`.
    #[cfg(unix)]
    pub service: Option<&'a ServiceLink>,
    #[cfg(not(unix))]
    pub service: std::marker::PhantomData<&'a ()>,
    /// How `open_summary` opens a file: `/usr/bin/open` in a macOS release
    /// build, the `OPEN_LOG_ENV` recorder in test builds, and `Unsupported`
    /// elsewhere.
    pub open: crate::open::Opener,
}

impl SetupContext<'_> {
    /// A context with no service link.
    pub fn new() -> Self {
        Self {
            #[cfg(unix)]
            service: None,
            #[cfg(not(unix))]
            service: std::marker::PhantomData,
            open: crate::open::default_opener(),
        }
    }
}

impl Default for SetupContext<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Process requests until the browser closes the native messaging pipe.
pub fn serve_with_io<R: Read, W: Write>(input: R, output: W, config: ConfigStore) -> Result<()> {
    // Building the link never connects; only a run message does.
    #[cfg(unix)]
    let link = ServiceLink::for_config(&config)
        .inspect_err(|link_error| {
            eprintln!("{NAMESPACE}: the service cannot be reached: {link_error:#}");
        })
        .ok();
    #[cfg(unix)]
    let context = SetupContext {
        service: link.as_ref(),
        ..SetupContext::new()
    };
    #[cfg(not(unix))]
    let context = SetupContext::new();
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

fn config_result(config: &mut ConfigStore, request_id: String) -> Response {
    let adapter_warnings = config.adapter_warnings().to_vec();
    Response::ConfigResult(ConfigResult {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        config: config.snapshot().clone(),
        revision: config.revision().to_owned(),
        config_issue: config.config_issue().map(str::to_owned),
        agent_status: config.agent_status(),
        adapter_issue: config.adapter_issue().map(str::to_owned),
        adapter_warnings,
        adopted_state: config.adopted_state(),
    })
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
                host_version: crate::BUILD_LABEL.to_owned(),
                configured: config.configured(),
                config_issue: config.config_issue().map(str::to_owned),
            })
        }
        Request::GetConfig(value) => {
            // The panel and settings ask on every open; that retries a
            // refused layout migration once the user has moved the folder.
            let _ = config.refresh();
            config.migrate_layout_or_warn();
            // Under the lock, the refresh also records the current adapter
            // contracts, so `adopted_state` reports this host's own attempt.
            match config.lock_current() {
                Ok(_config_lock) => {
                    let _ = config.refresh();
                }
                Err(lock_error) => {
                    let _ = config.refresh();
                    config.warn_adopted_unrecorded(&lock_error);
                }
            }
            config_result(config, value.request_id)
        }
        Request::ForgetAdoptedContracts(value) => {
            match config.forget_adopted_contracts(&value.expected_revision) {
                Ok(true) => config_result(config, value.request_id),
                Ok(false) => error(
                    &value.request_id,
                    ErrorCode::Conflict,
                    "the record of adopted adapters changed; read the settings again before forgetting it",
                ),
                Err(forget_error) => {
                    eprintln!("{NAMESPACE}: could not forget adopted adapters: {forget_error:#}");
                    error(
                        &value.request_id,
                        ErrorCode::Internal,
                        "could not back up and replace the record of adopted adapters",
                    )
                }
            }
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
                .and_then(|()| config.validate_strip_params(&value.config))
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
                Err(save_error) => match (
                    save_error.downcast_ref::<crate::layout::Refusal>(),
                    save_error.downcast_ref::<config::StripParamsRefusal>(),
                ) {
                    (Some(crate::layout::Refusal(message)), _)
                    | (_, Some(config::StripParamsRefusal(message))) => {
                        error(&value.request_id, ErrorCode::InvalidConfig, message)
                    }
                    _ => error(
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
        Request::OpenSummary(value) => open_summary(value, config, context.open),
        #[cfg(unix)]
        Request::StartRun(value) => start_run(value, config, context.service),
        // Run ids and offsets are the service's to check, harness setup
        // detects and probes on the service's PATH and environment, and
        // agent.env and run threads live in the service's directory.
        #[cfg(unix)]
        request @ (Request::GetRun(_)
        | Request::ListRuns(_)
        | Request::CancelRun(_)
        | Request::RetrySave(_)
        | Request::DiscoverHarnesses(_)
        | Request::SetUpHarness(_)
        | Request::OpenPath(_)
        | Request::GetAgentEnv(_)) => forward(&request, context.service),
        #[cfg(not(unix))]
        Request::StartRun(_)
        | Request::GetRun(_)
        | Request::ListRuns(_)
        | Request::CancelRun(_)
        | Request::RetrySave(_)
        | Request::DiscoverHarnesses(_)
        | Request::SetUpHarness(_)
        | Request::OpenPath(_)
        | Request::GetAgentEnv(_) => error(
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
        Err(_) => Err((ErrorCode::Internal, "configuration is unavailable".into())),
    };
    match resolved {
        Ok(key) => value.record_key = Some(key),
        Err((code, message)) => return error(&value.request_id, code, &message),
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
    // The service reads frames under the same cap as the extension's, but a
    // forwarded request can be larger than the one received: start_run gains
    // its record_key here. Refuse it now, before connecting (which can start
    // the service), rather than failing the write as a protocol error.
    if !crate::frame::request_fits(request) {
        let message = match request {
            Request::StartRun(_) => "This page is too large to send to the Brauser service.",
            _ => "This request is too large to send to the Brauser service.",
        };
        return error(id, ErrorCode::MessageTooLarge, message);
    }
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
pub(crate) fn open_index(
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

/// A request refusal: its code and the message shown to the user.
pub(crate) type Refusal = (ErrorCode, std::borrow::Cow<'static, str>);

/// The key-only part of `note_context`, shared by notes, the run proxy, and
/// the run finisher, so a run and the page's note always agree on the record
/// they belong to. It never lists older notes. Callers hold `lock_current`.
// Only the Unix run proxy calls it outside tests.
#[cfg_attr(not(any(test, unix)), allow(dead_code))]
pub(crate) fn resolve_record_key(
    config: &mut ConfigStore,
    url: &str,
) -> Result<RecordKey, Refusal> {
    resolve_key_parts(config, url).map(|parts| parts.key)
}

/// What key resolution already computed, kept so `note_context` can build on
/// it without opening the vault or matching adapters a second time.
struct KeyParts {
    canonical: String,
    vault: Vault,
    key: RecordKey,
}

/// `resolve_record_key`'s reason when the home folder is missing or was
/// replaced; callers that map it to their own message match on this.
pub(crate) const HOME_UNAVAILABLE: &str = "home folder is unavailable";

/// The configured home folder's vault, refused as `resolve_key_parts`
/// refuses: a config needing repair, no home folder, or one replaced.
fn open_vault(config: &mut ConfigStore) -> Result<Vault, Refusal> {
    if config.refresh().is_err() || config.config_issue().is_some() {
        return Err((
            ErrorCode::InvalidConfig,
            "configuration is unavailable or needs repair".into(),
        ));
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return Err((
            ErrorCode::NotConfigured,
            "choose a home folder first".into(),
        ));
    };
    Vault::open_checked(storage, config.root_identity())
        .map_err(|_| (ErrorCode::Internal, HOME_UNAVAILABLE.into()))
}

fn resolve_key_parts(config: &mut ConfigStore, url: &str) -> Result<KeyParts, Refusal> {
    if config.refresh().is_err() || config.config_issue().is_some() {
        return Err((
            ErrorCode::InvalidConfig,
            "configuration is unavailable or needs repair".into(),
        ));
    }
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return Err((
            ErrorCode::NotConfigured,
            "choose a home folder first".into(),
        ));
    };
    let canonical = crate::capture::canonical_url(url, &config.snapshot().strip_params)
        .map_err(|_| (ErrorCode::InvalidRequest, "page URL is invalid".into()))?;
    let vault = Vault::open_checked(storage, config.root_identity())
        .map_err(|_| (ErrorCode::Internal, HOME_UNAVAILABLE.into()))?;
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
            // origin no recorded contract covers (both records were lost,
            // or it was removed before either existed),
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
                    "note identity is unavailable; an adapter contract cannot be read".into(),
                ));
            }
            NoteKeyRule::StateUnavailable => {
                return Err((
                    ErrorCode::Internal,
                    config.adopted_contracts_unavailable().into(),
                ));
            }
        },
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
) -> Result<NoteContext, Refusal> {
    let KeyParts {
        canonical,
        vault,
        key,
    } = resolve_key_parts(config, url)?;
    // `older_urls` is both the older-note list and the only authorization
    // for viewing or migrating one, so every candidate must name this
    // artifact itself. Recorded URLs are stripped canonical forms: the
    // page's own canonical URL, an indexed modal route, or a note's creation
    // URL can equal an unrelated page's URL once `strip_params` removed the
    // query value that identified the artifact. A URL on an origin no
    // recorded contract covers names no artifact, for the same reason it
    // gets no artifact key above; that holds even for the artifact's own
    // indexed routes and its note's creation URL, which are stripped forms
    // too. A current match is final, as it is for the page's
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
            | NoteKeyRule::StateUnavailable
            | NoteKeyRule::Uncovered => false,
        }
    };
    let mut older_urls = Vec::new();
    if names_artifact(&canonical) {
        older_urls.push(canonical.clone());
    }
    if key.kind == RecordKeyKind::Artifact && include_older_notes {
        let Some(storage) = config.snapshot().storage.as_ref() else {
            return Err((
                ErrorCode::NotConfigured,
                "choose a notes folder first".into(),
            ));
        };
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
        Err((code, message)) => return error(&value.request_id, code, &message),
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
        Err((code, message)) => return error(&value.request_id, code, &message),
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
        Err((code, message)) => return error(&value.request_id, code, &message),
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
        Err((code, message)) => return error(&value.request_id, code, &message),
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
    let Some(storage) = config.snapshot().storage.as_ref() else {
        if matched.is_none() {
            // Without a home folder there is no summary to report.
            return Response::ArtifactResult(ArtifactResult {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                recognized: false,
                artifact: None,
                summary: None,
            });
        }
        return error(
            &value.request_id,
            ErrorCode::NotConfigured,
            "choose a home folder first",
        );
    };
    let Some(matched) = matched else {
        // An unrecognized page is not an artifact, but a summary may still
        // describe it (spec §7.2). Looking one up is best-effort: when the
        // index or log catch-up fails the page is simply unrecognized, as
        // it was before summaries, rather than an error on every tab.
        let summary = unrecognized_page_summary(config, storage, &value.url, &canonical);
        return Response::ArtifactResult(ArtifactResult {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            recognized: false,
            artifact: None,
            summary,
        });
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
    let history_unavailable = |request_id: &str| {
        error(
            request_id,
            ErrorCode::Internal,
            "artifact history is unavailable; log catch-up could not complete",
        )
    };
    let (artifact, summary) = match index.artifact_with_summary(&matched.artifact_id) {
        Ok((Some(artifact), summary)) => (artifact, summary),
        Ok((None, _)) => (
            ArtifactView {
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
            None,
        ),
        Err(_) => return history_unavailable(&value.request_id),
    };
    Response::ArtifactResult(ArtifactResult {
        protocol_version: PROTOCOL_VERSION,
        request_id: value.request_id,
        recognized: true,
        artifact: Some(artifact),
        summary,
    })
}

/// Open the summary file of the record the page's note uses (§7.2). The key
/// is resolved, the file's ownership read, and the opener run, all under
/// `lock_current`, so a config update cannot change the record in between.
/// Only a file `summary::owned_summary` accepts for that key is opened,
/// never another file at the generated name: just before the opener runs,
/// the ambient path must still name the very inode whose bytes were checked
/// (`vault::same_file`), not a symlink, so a leaf or folder swapped since
/// is refused.
///
/// Accepted window: `/usr/bin/open` takes a pathname, not a handle, so a
/// local writer that swaps the name between that last check and open's own
/// lookup (microseconds) can still redirect it, as `replace_owned_file`'s
/// replace window is accepted. Closing it would need opening by handle,
/// which LaunchServices does not offer.
fn open_summary(
    value: brauser_protocol::OpenSummaryRequest,
    config: &mut ConfigStore,
    open: crate::open::Opener,
) -> Response {
    let Ok(_config_lock) = config.lock_current() else {
        return error(
            &value.request_id,
            ErrorCode::Internal,
            "configuration is unavailable",
        );
    };
    // A named path is the summary a row or run shows, opened whatever
    // record the URL resolves to now; the page's own otherwise.
    if let Some(path) = &value.summary_path
        && crate::summary::check_summary_path(path).is_err()
    {
        return error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            "summary_path is not a summary path",
        );
    }
    let found = match &value.summary_path {
        Some(path) => open_vault(config).and_then(|vault| {
            vault
                .find_summary_at(path)
                .map_err(|_| (ErrorCode::Internal, "summary file is unavailable".into()))
        }),
        None => resolve_key_parts(config, &value.url).and_then(|parts| {
            parts
                .vault
                .find_summary(&parts.key, &parts.canonical)
                .map_err(|_| (ErrorCode::Internal, "summary file is unavailable".into()))
        }),
    };
    let (relative, linked, file_id) = match found {
        Ok(crate::vault::FoundSummary::Owned {
            relative,
            linked,
            file_id,
            ..
        }) => (relative, linked, file_id),
        Ok(crate::vault::FoundSummary::Absent) => {
            let message = if value.summary_path.is_some() {
                "This summary file no longer exists."
            } else {
                "This page has no summary yet."
            };
            return error(&value.request_id, ErrorCode::InvalidRequest, message);
        }
        Ok(crate::vault::FoundSummary::Unowned) => return not_a_summary(&value.request_id),
        Err((code, message)) => return error(&value.request_id, code, &message),
    };
    // resolve_key_parts only succeeds with a home folder configured.
    let Some(storage) = config.snapshot().storage.as_ref() else {
        return error(
            &value.request_id,
            ErrorCode::NotConfigured,
            "choose a home folder first",
        );
    };
    let path = std::path::Path::new(&storage.root).join(relative);
    if !crate::vault::same_file(&path, file_id) {
        return not_a_summary(&value.request_id);
    }
    // `--` ends open's options, so no path can be read as one.
    match open(&["--".into(), path.into_os_string()]) {
        Ok(()) => Response::SummaryOpened(brauser_protocol::SummaryOpened {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            relative_path: linked,
        }),
        Err(open_error) if open_error.kind() == io::ErrorKind::Unsupported => error(
            &value.request_id,
            ErrorCode::InvalidRequest,
            crate::open::UNSUPPORTED_MESSAGE,
        ),
        Err(open_error) => error(
            &value.request_id,
            ErrorCode::Internal,
            &format!("Could not open the summary: {open_error}"),
        ),
    }
}

fn not_a_summary(request_id: &str) -> Response {
    error(
        request_id,
        ErrorCode::Conflict,
        "The file at the summary path is not a Brauser summary.",
    )
}

/// The record key and artifact type the summary's log event uses:
/// - a `Url` key is its own `web.page`;
/// - an `Artifact` key takes the type of the adapter that matches `url` to
///   it, or, for a key recovered from an older contract, of its indexed row;
/// - an artifact whose type neither knows falls back to the URL's own key
///   and `web.page`. The summary file keeps the resolved key.
///
/// `None` when the answer needs the index and it cannot give one (it is
/// `None`, or its catch-up failed): that is not an answer of "no row", and
/// falling back then would log the summary under an identity the page no
/// longer looks under once the index recovers.
pub(crate) fn summary_artifact(
    config: &ConfigStore,
    url: &str,
    key: &RecordKey,
    index: Option<&ArtifactIndex>,
) -> Option<(RecordKey, String)> {
    if key.kind == RecordKeyKind::Url {
        return Some((key.clone(), crate::summary::WEB_PAGE_TYPE.into()));
    }
    let matched = config
        .adapter_match(url, None)
        .ok()
        .filter(|found| found.artifact_id == key.value)
        .map(|found| found.artifact_type);
    let known = match matched {
        Some(artifact_type) => Some(artifact_type),
        None => index?
            .artifact(&key.value)
            .ok()?
            .map(|artifact| artifact.artifact_type)
            .filter(|artifact_type| artifact_type != crate::summary::WEB_PAGE_TYPE),
    };
    if let Some(artifact_type) = known {
        return Some((key.clone(), artifact_type));
    }
    let url_key = crate::capture::canonical_url(url, &config.snapshot().strip_params)
        .ok()
        .and_then(|canonical| crate::vault::url_record_key(&canonical).ok())
        .unwrap_or_else(|| key.clone());
    Some((url_key, crate::summary::WEB_PAGE_TYPE.into()))
}

/// The latest summary of a page no live adapter recognizes, under the
/// artifact the finisher logs it to: a recorded contract's artifact (a
/// removed adapter's page) when its type is known, else the page's
/// `web.page` identity ([`summary_artifact`] decides both). Any
/// failure, including a contract this host cannot read, means no summary.
fn unrecognized_page_summary(
    config: &ConfigStore,
    storage: &brauser_protocol::StorageConfig,
    url: &str,
    canonical: &str,
) -> Option<brauser_protocol::SummaryView> {
    let cache_dir = config.cache_dir().ok()?;
    let index = open_index(config, storage, &cache_dir).ok()?;
    let artifact_id = match config.note_key_rule(url) {
        NoteKeyRule::Artifact(artifact_id) => {
            let key = RecordKey {
                kind: RecordKeyKind::Artifact,
                value: artifact_id,
            };
            let (event_key, _) = summary_artifact(config, url, &key, Some(&index))?;
            match event_key.kind {
                RecordKeyKind::Artifact => event_key.value,
                RecordKeyKind::Url => crate::summary::web_page_artifact_id(canonical).ok()?,
            }
        }
        NoteKeyRule::Unavailable | NoteKeyRule::StateUnavailable => return None,
        NoteKeyRule::Ambiguous | NoteKeyRule::Unrecognized | NoteKeyRule::Uncovered => {
            crate::summary::web_page_artifact_id(canonical).ok()?
        }
    };
    index.summary_for(&artifact_id).ok().flatten()
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
                    summary_vocabulary: crate::config::default_summary_vocabulary(),
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
                // The build commit rides as semver build metadata.
                let (version, build) = value.host_version.split_once('+').unwrap();
                assert_eq!(version, env!("CARGO_PKG_VERSION"));
                assert!(build == "unknown" || build.len() >= crate::build_id::COMMIT_LENGTH);
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
        let context = SetupContext::new();

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
        let context = SetupContext::new();
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
                summary_vocabulary: crate::config::default_summary_vocabulary(),
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
        let context = SetupContext::new();
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
        let context = SetupContext::new();
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

    /// brwr-f4s: a strip_params entry an adapter rule reads is refused as
    /// invalid_config naming the parameter and the adapter, and the config
    /// is unchanged.
    #[test]
    fn update_config_refuses_stripping_a_parameter_an_adapter_reads() {
        let folder = tempfile::tempdir().unwrap();
        let mut config = configured_store(folder.path());
        let mut consent = ConsentAuthority::new();
        let context = SetupContext::new();
        let mut next = config.snapshot().clone();
        next.adapters.artifacts.push(serde_json::json!({
            "type": "jira.issue",
            "origin": "https://acme.atlassian.net",
            "match": [{"path": "/jira/{*}", "query": {"selectedIssue": "{key:jira_key}"}}],
            "id": "jira:{host}/{key}",
        }));
        let revision = config.revision().to_owned();
        assert!(matches!(
            dispatch(
                update_request(&revision, next),
                &mut config,
                &mut consent,
                &context
            ),
            Response::ConfigUpdated(_)
        ));
        let revision = config.revision().to_owned();
        let mut next = config.snapshot().clone();
        next.strip_params.push("selected*".into());
        match dispatch(
            update_request(&revision, next),
            &mut config,
            &mut consent,
            &context,
        ) {
            Response::Error(value) => {
                assert_eq!(value.code, ErrorCode::InvalidConfig);
                assert!(
                    value.message.contains("\"selectedIssue\""),
                    "{}",
                    value.message
                );
                assert!(value.message.contains("jira.issue"), "{}", value.message);
            }
            other => panic!("expected invalid_config, got {other:?}"),
        }
        assert_eq!(config.revision(), revision);
        assert!(config.snapshot().strip_params.is_empty());
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
        // and forget the host's adopted contracts, as a repair on a fresh
        // install would. Recorded history alone is a stripped canonical URL
        // and cannot name the artifact, so the page gets its URL note, and
        // the shared note stays untouched on disk.
        forget_every_contract(&mut config, &path, &root, url, &saved);
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
        forget_every_contract(&mut config, &path, &root, url, &saved);
        assert_shared_note_is_kept_but_not_guessed(&mut config, &root, url, &saved);
    }

    /// Delete every adapter from config.toml by hand. The host's adopted
    /// contracts still name the shared note; then remove that state too.
    fn forget_every_contract(
        config: &mut ConfigStore,
        path: &std::path::Path,
        root: &std::path::Path,
        url: &str,
        shared: &NoteSaved,
    ) {
        write_adapter_config(path, root, "");
        let kept = note_load(config, url);
        assert_eq!(kept.key, shared.key);
        assert_eq!(kept.body, "keep this text");
        std::fs::remove_file(config::adopted_contracts_path(path.parent().unwrap())).unwrap();
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

        /// Removes the Jira adapter, then `[[adapter_id_history]]` and the
        /// host's adopted-contract state, as a config repair on a fresh
        /// install or an install older than both records would, so no
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
            // The host's own record of adopted contracts goes too, as on a
            // fresh install or one older than that record.
            std::fs::remove_file(config::adopted_contracts_path(folder.path())).unwrap();
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

            hand_remove_board_adapter(&path);
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

        fn board_and_issue_store(folder: &tempfile::TempDir) -> (std::path::PathBuf, ConfigStore) {
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
            (path.clone(), loaded(&path))
        }

        /// Delete the jira.board adapter from config.toml in an editor.
        fn hand_remove_board_adapter(path: &std::path::Path) {
            let mut table: toml::Table =
                toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            let artifacts = table
                .get_mut("artifacts")
                .and_then(toml::Value::as_array_mut)
                .expect("top-level [[artifacts]]");
            let before = artifacts.len();
            artifacts.retain(|rule| {
                rule.get("type").and_then(toml::Value::as_str) != Some("jira.board")
            });
            assert_eq!(artifacts.len(), before - 1);
            std::fs::write(path, toml::to_string(&table).unwrap()).unwrap();
        }

        /// If the host's record of the removed board contract cannot be
        /// read, the board's key cannot be established: loads and saves
        /// refuse rather than start a URL note beside the artifact note.
        #[test]
        fn unreadable_contract_state_refuses_the_hand_removed_boards_note() {
            let folder = tempfile::tempdir().unwrap();
            let (path, mut config) = board_and_issue_store(&folder);
            let board = save(&mut config, BOARD, "board artifact note");
            let root = folder.path().join("notes");
            let board_bytes = std::fs::read(root.join(&board.relative_path)).unwrap();
            hand_remove_board_adapter(&path);
            let state = config::adopted_contracts_path(folder.path());
            std::fs::write(&state, b"{").unwrap();
            config = loaded(&path);

            // The refusal names the state file and its repair, not just a
            // contract that cannot be read.
            let named = |message: &str| {
                message.starts_with("Notes on this page are paused to protect note identity")
                    && message.contains(&state.display().to_string())
                    && message.contains("Forget recorded adapters")
            };
            let refused = |response: &Response| matches!(response, Response::Error(reply) if reply.code == ErrorCode::Internal && named(&reply.message));
            let loaded_board = load_note(
                brauser_protocol::LoadNoteRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "load".into(),
                    url: BOARD.into(),
                },
                &mut config,
            );
            assert!(refused(&loaded_board), "{loaded_board:?}");
            let url_key = crate::vault::url_record_key(BOARD).unwrap();
            for key in [url_key, board.key.clone()] {
                let saved = note_save(&mut config, BOARD, "split", "missing", key);
                assert!(refused(&saved), "{saved:?}");
            }
            assert_eq!(
                std::fs::read(root.join(&board.relative_path)).unwrap(),
                board_bytes
            );
            // The live issue adapter still decides its own pages.
            assert_eq!(
                note_load(&mut config, BROWSE_123).key.kind,
                RecordKeyKind::Artifact
            );

            // Settings is shown the same problem, and its Forget recorded
            // adapters repair forgets the board contract: the board gets its
            // URL note again, and the artifact note stays untouched on disk.
            let call = |config: &mut ConfigStore, request| {
                dispatch(
                    request,
                    config,
                    &mut ConsentAuthority::new(),
                    &SetupContext::new(),
                )
            };
            let get = |config: &mut ConfigStore| match call(
                config,
                Request::GetConfig(brauser_protocol::GetConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "read".into(),
                }),
            ) {
                Response::ConfigResult(result) => result,
                other => panic!("expected config_result, got {other:?}"),
            };
            let forget = |revision: &str| {
                Request::ForgetAdoptedContracts(brauser_protocol::ForgetAdoptedContractsRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "forget".into(),
                    expected_revision: revision.into(),
                })
            };
            let shown = get(&mut config).adopted_state;
            let issue = shown.issue.expect("settings names the damaged record");
            assert!(issue.contains(&state.display().to_string()), "{issue}");
            assert!(
                matches!(call(&mut config, forget("sha256:stale")), Response::Error(reply) if reply.code == ErrorCode::Conflict)
            );
            assert_eq!(std::fs::read(&state).unwrap(), b"{");
            match call(&mut config, forget(&shown.revision)) {
                Response::ConfigResult(result) => {
                    assert_eq!(result.request_id, "forget");
                    assert_eq!(result.adopted_state.issue, None);
                }
                other => panic!("expected config_result, got {other:?}"),
            }
            let reloaded = note_load(&mut config, BOARD);
            assert_eq!(reloaded.key, crate::vault::url_record_key(BOARD).unwrap());
            assert!(!reloaded.exists);
            assert_eq!(
                std::fs::read(root.join(&board.relative_path)).unwrap(),
                board_bytes
            );
        }
    }

    /// get_config records adopted contracts under the config lock; when it
    /// cannot take the lock, settings is told protection is degraded rather
    /// than shown a healthy record.
    #[test]
    fn get_config_warns_when_it_cannot_lock_to_record_adopted_contracts() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(&path, &root, "");
        std::fs::create_dir(folder.path().join(".config.lock")).unwrap();
        let mut config = loaded(&path);
        let response = dispatch(
            Request::GetConfig(GetConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "read".into(),
            }),
            &mut config,
            &mut ConsentAuthority::new(),
            &SetupContext::new(),
        );
        let Response::ConfigResult(result) = response else {
            panic!("expected config_result, got {response:?}");
        };
        assert_eq!(result.adopted_state.warnings.len(), 1);
        assert!(
            result.adopted_state.warnings[0].contains("configuration lock"),
            "{:?}",
            result.adopted_state.warnings
        );
        // Forgetting needs the same lock, so it is not offered.
        assert!(!result.adopted_state.full);
        assert!(!result.adopted_state.warnings[0].contains("Forget"));
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
                    &SetupContext::new(),
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

            /// A config repair on a fresh install, or one older than both
            /// records, has neither `[[adapter_id_history]]` nor the host's
            /// adopted contracts; the origin is then uncovered and its pages
            /// use URL keys.
            fn forget_policy_history(&self) {
                let mut table: toml::Table =
                    toml::from_str(&std::fs::read_to_string(&self.path).unwrap()).unwrap();
                assert!(table.remove("adapter_id_history").is_some());
                std::fs::write(&self.path, toml::to_string(&table).unwrap()).unwrap();
                match std::fs::remove_file(config::adopted_contracts_path(self.folder.path())) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => panic!("{error}"),
                    _ => {}
                }
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

    /// Log and index a summary the way the service's finisher does: the
    /// daily-log event first, then the index row from the event it wrote.
    fn record_page_summary(
        config: &ConfigStore,
        url: &str,
        record_key: RecordKey,
        artifact_type: &str,
    ) -> crate::summary::SummaryEvent {
        let storage = config.snapshot().storage.as_ref().unwrap();
        let capture =
            CaptureStore::open_in(storage, config.root_identity(), config.dir().unwrap()).unwrap();
        let policy = CapturePolicy {
            enabled: config.snapshot().capture_enabled,
            origins: &[],
            strip_params: &config.snapshot().strip_params,
        };
        let summary = crate::summary::PageSummary {
            event_id: uuid::Uuid::new_v4().to_string(),
            url: url.into(),
            title: "A summarized page".into(),
            occurred_at: time::OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            document_type: "specification".into(),
            category: "technical specification".into(),
            importance: brauser_protocol::ImportanceLevel::High,
            summary_path: "summaries/a.summary.md".into(),
            record_key,
            artifact_type: artifact_type.into(),
            harness: brauser_protocol::HarnessId::Claude,
        };
        let (CaptureOutcome::Persisted { .. }, Some(event)) =
            capture.record_summary(policy, &summary)
        else {
            panic!("expected a persisted summary");
        };
        open_index(config, storage, &config.cache_dir().unwrap())
            .unwrap()
            .record_summary(&event)
            .unwrap();
        event
    }

    fn expected_summary(event: &crate::summary::SummaryEvent) -> brauser_protocol::SummaryView {
        brauser_protocol::SummaryView {
            summarized_at: event.occurred_at.clone(),
            document_type: "specification".into(),
            category: "technical specification".into(),
            importance: brauser_protocol::ImportanceLevel::High,
            summary_path: "summaries/a.summary.md".into(),
            harness: brauser_protocol::HarnessId::Claude,
        }
    }

    fn artifact_result(config: &mut ConfigStore, url: &str) -> ArtifactResult {
        match get_artifact(
            brauser_protocol::GetArtifactRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "artifact".into(),
                url: url.into(),
                title: None,
            },
            config,
        ) {
            Response::ArtifactResult(result) => result,
            other => panic!("expected artifact_result, got {other:?}"),
        }
    }

    fn recent_items(config: &mut ConfigStore) -> Vec<brauser_protocol::ArtifactListItem> {
        match list_artifacts(
            brauser_protocol::ListArtifactsRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "recent".into(),
                artifact_type: None,
                title_query: None,
                limit: 32,
            },
            config,
        ) {
            Response::ArtifactListResult(result) => result.items,
            other => panic!("expected artifact_list_result, got {other:?}"),
        }
    }

    #[test]
    fn get_artifact_reports_the_summary_of_an_unrecognized_page() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        let url = "https://example.com/spec";
        let none = artifact_result(&mut config, url);
        assert!(!none.recognized);
        assert_eq!((none.artifact, none.summary), (None, None));

        let event = record_page_summary(
            &config,
            url,
            crate::vault::url_record_key(url).unwrap(),
            crate::summary::WEB_PAGE_TYPE,
        );
        let result = artifact_result(&mut config, url);
        assert!(!result.recognized);
        assert_eq!(result.artifact, None);
        assert_eq!(result.summary, Some(expected_summary(&event)));
        // Another page has no summary of its own.
        assert_eq!(
            artifact_result(&mut config, "https://example.com/other").summary,
            None
        );

        let items = recent_items(&mut config);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].artifact_type, "web.page");
        assert_eq!(items[0].summary, Some(expected_summary(&event)));
    }

    #[test]
    fn get_artifact_on_an_unrecognized_page_without_a_home_has_no_summary() {
        let folder = tempfile::tempdir().unwrap();
        let mut config = ConfigStore::for_test(folder.path().join("config.toml"));
        let result = artifact_result(&mut config, "https://example.com/spec");
        assert!(!result.recognized);
        assert_eq!((result.artifact, result.summary), (None, None));
    }

    /// A summary is extra information on an unrecognized page: when the
    /// artifact history cannot be read, the page is still plainly
    /// unrecognized rather than an error on every ordinary tab.
    #[test]
    fn get_artifact_on_an_unrecognized_page_stays_quiet_when_history_is_unavailable() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        // A file where the log folder belongs: catch-up cannot complete.
        std::fs::write(notes.join("logs"), b"not a folder").unwrap();
        let storage = config.snapshot().storage.clone().unwrap();
        let index = open_index(&config, &storage, &config.cache_dir().unwrap()).unwrap();
        assert!(index.summary_for("web:https://example.com/spec").is_err());
        let result = artifact_result(&mut config, "https://example.com/spec");
        assert!(!result.recognized);
        assert_eq!((result.artifact, result.summary), (None, None));
    }

    /// A page whose adapter was removed keeps the summary logged under its
    /// recorded artifact key, the key open_summary and Recent use too.
    #[test]
    fn get_artifact_reports_a_removed_adapters_summary_under_its_artifact_key() {
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
        let event = record_page_summary(
            &config,
            url,
            RecordKey {
                kind: RecordKeyKind::Artifact,
                value: "work:work.example/7".into(),
            },
            "work.item",
        );
        let mut next = config.snapshot().clone();
        next.adapters.artifacts.clear();
        let revision = config.revision().to_owned();
        config.update(next, &revision).unwrap().unwrap();
        assert!(matches!(
            config.note_key_rule(url),
            crate::config::NoteKeyRule::Artifact(_)
        ));

        let result = artifact_result(&mut config, url);
        assert!(!result.recognized);
        assert_eq!(result.artifact, None);
        assert_eq!(result.summary, Some(expected_summary(&event)));
    }

    /// A removed adapter's page whose artifact type nothing records: the
    /// finisher logs its summary under the URL's `web.page` key
    /// (`summary_artifact`), and get_artifact looks under that same key.
    #[test]
    fn get_artifact_finds_a_summary_logged_under_the_web_page_of_an_untyped_artifact() {
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
        let mut next = config.snapshot().clone();
        next.adapters.artifacts.clear();
        let revision = config.revision().to_owned();
        config.update(next, &revision).unwrap().unwrap();
        let url = "https://work.example/items/7";
        let artifact_key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:work.example/7".into(),
        };
        assert!(matches!(
            config.note_key_rule(url),
            crate::config::NoteKeyRule::Artifact(ref id) if *id == artifact_key.value
        ));
        let storage = config.snapshot().storage.clone().unwrap();
        let index = open_index(&config, &storage, &config.cache_dir().unwrap()).unwrap();
        let (event_key, artifact_type) =
            summary_artifact(&config, url, &artifact_key, Some(&index)).unwrap();
        assert_eq!(event_key, crate::vault::url_record_key(url).unwrap());
        assert_eq!(artifact_type, crate::summary::WEB_PAGE_TYPE);
        let event = record_page_summary(&config, url, event_key, &artifact_type);

        let result = artifact_result(&mut config, url);
        assert!(!result.recognized);
        assert_eq!(result.summary, Some(expected_summary(&event)));
        // Recent lists the same summary.
        let items = recent_items(&mut config);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].summary, Some(expected_summary(&event)));
    }

    #[test]
    fn get_artifact_reports_an_adapter_artifacts_summary() {
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
        assert_eq!(artifact_result(&mut config, url).summary, None);
        let event = record_page_summary(
            &config,
            url,
            RecordKey {
                kind: RecordKeyKind::Artifact,
                value: "work:work.example/7".into(),
            },
            "work.item",
        );

        let result = artifact_result(&mut config, url);
        assert!(result.recognized);
        let artifact = result.artifact.expect("the summary created the row");
        assert_eq!(artifact.artifact_id, "work:work.example/7");
        assert_eq!((artifact.opens, artifact.days_opened), (0, 0));
        assert_eq!(result.summary, Some(expected_summary(&event)));
        let items = recent_items(&mut config);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].artifact_id, "work:work.example/7");
        assert_eq!(items[0].summary, Some(expected_summary(&event)));
    }

    std::thread_local! {
        /// What the recording opener was asked to open on this test's thread.
        static OPENED: std::cell::RefCell<Vec<Vec<String>>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn recording_opener(args: &[std::ffi::OsString]) -> io::Result<()> {
        OPENED.with(|opened| {
            opened.borrow_mut().push(
                args.iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect(),
            );
        });
        Ok(())
    }

    fn take_opened() -> Vec<Vec<String>> {
        OPENED.with(|opened| std::mem::take(&mut *opened.borrow_mut()))
    }

    fn open_summary_with(
        config: &mut ConfigStore,
        url: &str,
        open: crate::open::Opener,
    ) -> Response {
        open_summary_at(config, url, None, open)
    }

    fn open_summary_at(
        config: &mut ConfigStore,
        url: &str,
        summary_path: Option<&str>,
        open: crate::open::Opener,
    ) -> Response {
        dispatch(
            Request::OpenSummary(brauser_protocol::OpenSummaryRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "open-summary".into(),
                url: url.into(),
                summary_path: summary_path.map(Into::into),
            }),
            config,
            &mut ConsentAuthority::new(),
            &SetupContext {
                open,
                ..SetupContext::new()
            },
        )
    }

    /// A minimal file `summary::owned_summary` accepts for `key`.
    fn owned_summary_text(key: &RecordKey, canonical_url: &str) -> String {
        let id = match key.kind {
            RecordKeyKind::Url => "url_id",
            RecordKeyKind::Artifact => "artifact_id",
        };
        format!(
            "---\ntitle: \"T\"\nbrauser:\n  kind: summary\n  canonical_url: {}\n  {id}: {}\n---\n\nAgent text.\n",
            serde_json::to_string(canonical_url).unwrap(),
            serde_json::to_string(&key.value).unwrap(),
        )
    }

    /// Write `contents` at the record's summary path; returns that path,
    /// home-folder relative with `/` separators.
    fn place_summary(
        config: &ConfigStore,
        key: &RecordKey,
        canonical_url: &str,
        contents: &str,
    ) -> String {
        let storage = config.snapshot().storage.as_ref().unwrap();
        let vault = Vault::open_checked(storage, config.root_identity()).unwrap();
        let relative = vault.summary_relative_path(key, canonical_url).unwrap();
        let path = std::path::Path::new(&storage.root).join(&relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    fn error_of(response: Response) -> (ErrorCode, String) {
        match response {
            Response::Error(ErrorResponse { code, message, .. }) => (code, message),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn open_summary_opens_only_an_owned_summary() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        let url = "https://example.com/spec";
        let key = crate::vault::url_record_key(url).unwrap();
        take_opened();

        // A file at the summary path that is not this record's summary is
        // never opened: a plain note, then another record's summary.
        let relative = place_summary(&config, &key, url, "# My own notes\n");
        let (code, message) = error_of(open_summary_with(&mut config, url, recording_opener));
        assert_eq!(code, ErrorCode::Conflict);
        assert_eq!(
            message,
            "The file at the summary path is not a Brauser summary."
        );
        let other = crate::vault::url_record_key("https://example.com/other").unwrap();
        place_summary(
            &config,
            &key,
            url,
            &owned_summary_text(&other, "https://example.com/other"),
        );
        let (code, _) = error_of(open_summary_with(&mut config, url, recording_opener));
        assert_eq!(code, ErrorCode::Conflict);
        assert!(take_opened().is_empty());

        place_summary(&config, &key, url, &owned_summary_text(&key, url));
        match open_summary_with(&mut config, url, recording_opener) {
            Response::SummaryOpened(opened) => {
                assert_eq!(opened.request_id, "open-summary");
                assert_eq!(opened.relative_path, relative);
            }
            other => panic!("expected summary_opened, got {other:?}"),
        }
        let absolute = notes.join(&relative).to_string_lossy().into_owned();
        assert_eq!(take_opened(), vec![vec!["--".to_owned(), absolute]]);
    }

    #[test]
    fn open_summary_uses_the_key_the_pages_note_uses() {
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
        let artifact = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:work.example/7".into(),
        };
        take_opened();
        // A summary under the page's URL key is not the adapter artifact's.
        let url_key = crate::vault::url_record_key(url).unwrap();
        place_summary(&config, &url_key, url, &owned_summary_text(&url_key, url));
        let (code, _) = error_of(open_summary_with(&mut config, url, recording_opener));
        assert_eq!(code, ErrorCode::InvalidRequest);

        let relative = place_summary(&config, &artifact, url, &owned_summary_text(&artifact, url));
        assert!(matches!(
            open_summary_with(&mut config, url, recording_opener),
            Response::SummaryOpened(ref opened) if opened.relative_path == relative
        ));
        assert_eq!(
            take_opened(),
            vec![vec![
                "--".to_owned(),
                root.join(&relative).to_string_lossy().into_owned()
            ]]
        );
    }

    /// Review finding: a Recent row or run card opens the summary it shows,
    /// by that summary's own path, even when the page's URL now resolves to
    /// another record (an adapter removed, or `strip_params` changed).
    #[test]
    fn open_summary_by_path_opens_the_summary_the_row_shows() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        let url = "https://work.example/items/7";
        let artifact = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "work:work.example/7".into(),
        };
        take_opened();
        // Saved under an adapter's artifact that no longer matches the URL.
        let relative = place_summary(&config, &artifact, url, &owned_summary_text(&artifact, url));
        let (code, _) = error_of(open_summary_with(&mut config, url, recording_opener));
        assert_eq!(code, ErrorCode::InvalidRequest);
        assert!(take_opened().is_empty());

        match open_summary_at(&mut config, url, Some(&relative), recording_opener) {
            Response::SummaryOpened(opened) => assert_eq!(opened.relative_path, relative),
            other => panic!("expected summary_opened, got {other:?}"),
        }
        assert_eq!(
            take_opened(),
            vec![vec![
                "--".to_owned(),
                notes.join(&relative).to_string_lossy().into_owned()
            ]]
        );
    }

    /// A path opens only a summary at the very name its own record's
    /// summary has, so a request cannot point the opener anywhere else.
    #[test]
    fn open_summary_by_path_refuses_any_other_file() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        let url = "https://example.com/spec";
        let key = crate::vault::url_record_key(url).unwrap();
        let other_url = "https://example.com/other";
        let other = crate::vault::url_record_key(other_url).unwrap();
        take_opened();

        // Another record's summary copied to this record's name.
        let relative = place_summary(&config, &key, url, &owned_summary_text(&other, other_url));
        let (code, _) = error_of(open_summary_at(
            &mut config,
            url,
            Some(&relative),
            recording_opener,
        ));
        assert_eq!(code, ErrorCode::Conflict);
        // A plain file at a summary name.
        place_summary(&config, &key, url, "# Mine\n");
        let (code, _) = error_of(open_summary_at(
            &mut config,
            url,
            Some(&relative),
            recording_opener,
        ));
        assert_eq!(code, ErrorCode::Conflict);
        // An owned summary outside the summaries folder.
        let elsewhere = notes.join("notes/x.summary.md");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, owned_summary_text(&key, url)).unwrap();
        let (code, _) = error_of(open_summary_at(
            &mut config,
            url,
            Some("notes/x.summary.md"),
            recording_opener,
        ));
        assert_eq!(code, ErrorCode::Conflict);
        // Not a summary path at all.
        for bad in ["../x.summary.md", "summaries/a.md", "/etc/x.summary.md"] {
            let (code, _) = error_of(open_summary_at(
                &mut config,
                url,
                Some(bad),
                recording_opener,
            ));
            assert_eq!(code, ErrorCode::InvalidRequest, "{bad}");
        }
        // Gone since the row was drawn.
        std::fs::remove_file(notes.join(&relative)).unwrap();
        let (code, message) = error_of(open_summary_at(
            &mut config,
            url,
            Some(&relative),
            recording_opener,
        ));
        assert_eq!(code, ErrorCode::InvalidRequest);
        assert_eq!(message, "This summary file no longer exists.");
        assert!(take_opened().is_empty());
    }

    #[test]
    fn open_summary_without_a_summary_is_invalid_request() {
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        take_opened();
        let (code, message) = error_of(open_summary_with(
            &mut config,
            "https://example.com/spec",
            recording_opener,
        ));
        assert_eq!(code, ErrorCode::InvalidRequest);
        assert_eq!(message, "This page has no summary yet.");
        // A name the host cannot read as a file is not a summary either.
        let url = "https://example.com/spec";
        let key = crate::vault::url_record_key(url).unwrap();
        let relative = place_summary(&config, &key, url, "");
        let path = notes.join(&relative);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let (code, _) = error_of(open_summary_with(&mut config, url, recording_opener));
        assert_eq!(code, ErrorCode::Conflict);
        assert!(take_opened().is_empty());

        // Without a home folder there is nothing to open.
        let mut unconfigured = ConfigStore::for_test(folder.path().join("none.toml"));
        let (code, _) = error_of(open_summary_with(
            &mut unconfigured,
            "https://example.com/spec",
            recording_opener,
        ));
        assert_eq!(code, ErrorCode::NotConfigured);
        assert!(take_opened().is_empty());
    }

    #[test]
    fn open_summary_reports_an_opener_that_cannot_open() {
        fn unsupported(_: &[std::ffi::OsString]) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "unsupported"))
        }
        fn failing(_: &[std::ffi::OsString]) -> io::Result<()> {
            Err(io::Error::other("no opener"))
        }
        let folder = tempfile::tempdir().unwrap();
        let notes = folder.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let mut config = configured_store(&notes);
        let url = "https://example.com/spec";
        let key = crate::vault::url_record_key(url).unwrap();
        place_summary(&config, &key, url, &owned_summary_text(&key, url));
        let (code, message) = error_of(open_summary_with(&mut config, url, unsupported));
        assert_eq!(code, ErrorCode::InvalidRequest);
        assert_eq!(message, "Opening is only supported on macOS.");
        let (code, message) = error_of(open_summary_with(&mut config, url, failing));
        assert_eq!(code, ErrorCode::Internal);
        assert!(message.contains("no opener"), "{message}");
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
            &SetupContext::new(),
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
            &SetupContext::new(),
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
        let context = SetupContext::new();
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

    fn served(requests: &[String], config: ConfigStore) -> Vec<Response> {
        served_with(requests, config, &SetupContext::new())
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

    #[test]
    fn a_trail_from_a_removed_site_rejects_only_its_own_batch_hour() {
        // The host never rewrites a delivered event: a trail whose source no
        // adapter resolves is rejected, never logged, and costs only that hour.
        // The worker strips such trails before dispatch (DESIGN.md §5.1 Trails).
        let folder = tempfile::tempdir().unwrap();
        let (mut config, root) = adapter_only_config(&folder);
        let occurred_at = hour_timestamp();
        let stale = work_item_activity(
            &occurred_at,
            vec![ActivityTrail {
                from_artifact_id: Some("removed:removed.example/2".into()),
                from_url: Some("https://removed.example/items/2".into()),
                transition: "link".into(),
            }],
        );
        let mut clean = work_item_activity(&occurred_at, Vec::new());
        clean.artifact_id = "work:work.example/8".into();
        clean.url = "https://work.example/items/8".into();
        let item = |event: &ActivityEvent| CaptureBatchItem {
            kind: CaptureEventKind::Activity,
            visit: None,
            activity: Some(event.clone()),
        };
        let response = record_batch(
            RecordBatchRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "batch-stale-trail".into(),
                batch_id: uuid::Uuid::new_v4().to_string(),
                events: vec![item(&stale), item(&clean)],
                overflow: None,
            },
            &mut config,
        );
        let Response::BatchRecorded(recorded) = response else {
            panic!("expected batch results, got {response:?}");
        };
        let outcome = |id: &str| {
            recorded
                .results
                .iter()
                .find(|result| result.event_id == id)
                .map(|result| (result.outcome, result.reason.clone()))
                .unwrap()
        };
        assert_eq!(
            outcome(&stale.event_id),
            (
                VisitOutcome::Rejected,
                Some("trail_identity_mismatch".into())
            )
        );
        assert_eq!(outcome(&clean.event_id).0, VisitOutcome::Persisted);
        let mut logged = String::new();
        let mut pending = vec![root.join(crate::layout::LOGS_DIR)];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    logged.push_str(&std::fs::read_to_string(path).unwrap());
                }
            }
        }
        assert!(logged.contains("https://work.example/items/8"));
        assert!(!logged.contains("removed.example"));
        assert!(!logged.contains(&stale.event_id));
    }

    fn refused(response: &Response) -> (ErrorCode, &str) {
        match response {
            Response::Error(value) => (value.code, value.message.as_str()),
            other => panic!("expected an agent refusal, got {other:?}"),
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

    /// Only the service's harness setup commits a harness; update_config
    /// may echo it or clear it, never write one.
    #[test]
    fn update_config_cannot_set_an_agent() {
        let folder = tempfile::tempdir().unwrap();
        let mut config = configured_store(folder.path());
        let mut next = config.snapshot().clone();
        next.agent = Some(brauser_protocol::AgentConfig {
            harness: brauser_protocol::HarnessId::Claude,
            binary: config::fixtures::absent_binary("claude"),
            version: Some("2.1.284".into()),
            timeout_secs: 600,
            probed_at: "2026-10-27T10:00:00Z".into(),
        });
        let revision = config.revision().to_owned();
        let response = dispatch(
            Request::UpdateConfig(UpdateConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "write-1".into(),
                expected_revision: revision.clone(),
                config: next,
                picker_token: None,
                knowledge_base_token: None,
            }),
            &mut config,
            &mut ConsentAuthority::new(),
            &SetupContext::new(),
        );
        assert_eq!(refused(&response).0, ErrorCode::Unauthorized);
        config.refresh().unwrap();
        assert_eq!(config.revision(), revision);
        assert_eq!(
            config.agent_status().state,
            brauser_protocol::AgentState::NotSetUp
        );
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
            let context = SetupContext {
                service: Some(link),
                ..SetupContext::new()
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

        /// Harness setup runs in the service: both requests go there as
        /// they are, and its answers come back unchanged.
        #[test]
        fn discover_harnesses_is_forwarded_to_the_service() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let dir = fake::short_dir();
            let socket = fake::socket_in(&dir);
            let snapshot = config.snapshot().clone();
            let service = FakeService::listen(
                &socket,
                Arc::new(move |request, _| match request {
                    Request::ServiceHello(_) => fake::ready(request, BUILD),
                    Request::DiscoverHarnesses(value) => {
                        Response::HarnessesDiscovered(brauser_protocol::HarnessesDiscovered {
                            protocol_version: PROTOCOL_VERSION,
                            request_id: value.request_id.clone(),
                            harnesses: vec![brauser_protocol::HarnessFound {
                                harness: brauser_protocol::HarnessId::Codex,
                                binary: Some("/opt/homebrew/bin/codex".into()),
                                version: Some("0.144.4".into()),
                                problem: None,
                            }],
                        })
                    }
                    Request::SetUpHarness(value) => Response::ConfigUpdated(ConfigUpdated {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id.clone(),
                        config: snapshot.clone(),
                        revision: "after-setup".into(),
                    }),
                    _ => fake::runs_listed(request),
                }),
            );
            let link = link(socket, never);
            match call(
                &mut config,
                &link,
                Request::DiscoverHarnesses(brauser_protocol::DiscoverHarnessesRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "discover-1".into(),
                }),
            ) {
                Response::HarnessesDiscovered(found) => {
                    assert_eq!(found.request_id, "discover-1");
                    assert_eq!(found.harnesses[0].version.as_deref(), Some("0.144.4"));
                }
                other => panic!("expected harnesses_discovered, got {other:?}"),
            }
            let set_up = brauser_protocol::SetUpHarnessRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "setup-1".into(),
                expected_revision: config.revision().to_owned(),
                harness: brauser_protocol::HarnessId::Codex,
                timeout_secs: Some(900),
                summaries_dir: Some("ai".into()),
            };
            match call(&mut config, &link, Request::SetUpHarness(set_up.clone())) {
                Response::ConfigUpdated(updated) => assert_eq!(updated.revision, "after-setup"),
                other => panic!("expected config_updated, got {other:?}"),
            }
            let requests = service.requests();
            assert!(matches!(
                &requests[1],
                Request::DiscoverHarnesses(value) if value.request_id == "discover-1"
            ));
            assert!(matches!(
                &requests[3],
                Request::SetUpHarness(value) if *value == set_up
            ));

            // Without a service link, setup is unavailable, not attempted here.
            let response = dispatch(
                Request::SetUpHarness(set_up),
                &mut config,
                &mut ConsentAuthority::new(),
                &SetupContext::new(),
            );
            assert_eq!(refused(&response).0, ErrorCode::ServiceUnavailable);
        }

        /// open_path and get_agent_env act on the service's own directory,
        /// so the host forwards them unchanged.
        #[test]
        fn open_path_and_get_agent_env_are_forwarded_to_the_service() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            use brauser_protocol::{
                AgentEnvListed, GetAgentEnvRequest, OpenPathRequest, OpenTarget, PathOpened,
            };
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let dir = fake::short_dir();
            let socket = fake::socket_in(&dir);
            let service = FakeService::listen(
                &socket,
                Arc::new(move |request, _| match request {
                    Request::ServiceHello(_) => fake::ready(request, BUILD),
                    Request::OpenPath(value) => Response::PathOpened(PathOpened {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id.clone(),
                    }),
                    Request::GetAgentEnv(value) => Response::AgentEnvListed(AgentEnvListed {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id.clone(),
                        names: vec!["FOO".into()],
                        problem: None,
                    }),
                    _ => fake::runs_listed(request),
                }),
            );
            let link = link(socket, never);
            let open = OpenPathRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "open-1".into(),
                target: OpenTarget::AgentEnv,
                run_id: None,
            };
            assert!(matches!(
                call(&mut config, &link, Request::OpenPath(open.clone())),
                Response::PathOpened(ref opened) if opened.request_id == "open-1"
            ));
            match call(
                &mut config,
                &link,
                Request::GetAgentEnv(GetAgentEnvRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "env-1".into(),
                }),
            ) {
                Response::AgentEnvListed(listed) => assert_eq!(listed.names, vec!["FOO"]),
                other => panic!("expected agent_env_listed, got {other:?}"),
            }
            let requests = service.requests();
            assert!(matches!(&requests[1], Request::OpenPath(value) if *value == open));
            assert!(matches!(&requests[3], Request::GetAgentEnv(_)));
        }

        #[test]
        fn start_run_forwards_host_resolved_key() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
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
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
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
            // A current time with fractional seconds, as new Date().toISOString()
            // gives, is refused too: the schema's pattern admits whole seconds only.
            let fractional = time::OffsetDateTime::now_utc()
                .replace_millisecond(452)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            assert!(fractional.ends_with(".452Z"), "{fractional}");
            for requested_at in [
                "yesterday",
                "2026-10-26 00:00:00Z",
                "2001-01-01T00:00:00Z",
                fractional.as_str(),
            ] {
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

        /// A start_run whose content brings its serialized size, as the
        /// forwarded request (with `record_key` set to `key`), to `target`.
        /// Content uses four-byte characters so a frame-sized request stays
        /// within MAX_CONTENT_CHARS.
        fn start_run_sized(url: &str, key: Option<RecordKey>, target: usize) -> StartRunRequest {
            let Request::StartRun(mut value) = start_run(url, None, &now()) else {
                unreachable!()
            };
            value.content = String::new();
            let mut measured = value.clone();
            measured.record_key = key;
            let base = serde_json::to_vec(&Request::StartRun(measured))
                .unwrap()
                .len();
            let bytes = target - base;
            value.content = format!("{}{}", "😀".repeat(bytes / 4), "x".repeat(bytes % 4));
            assert!(
                value.content.chars().count() <= crate::service::record::MAX_CONTENT_CHARS,
                "the content must pass check_start_run"
            );
            value
        }

        /// A start_run that fits the extension's frame but not the service's
        /// once the host adds `record_key` is refused as too large before the
        /// service is contacted (or started), not reported as a broken reply.
        #[test]
        fn start_run_too_large_once_keyed_is_refused_without_connecting() {
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
            use crate::frame::MAX_INBOUND_BYTES;
            let notes = tempfile::tempdir().unwrap();
            let mut config = configured_store(notes.path());
            let url = "https://example.com/a";
            let key = note_load(&mut config, url).key;
            let dir = fake::short_dir();
            let socket = fake::socket_in(&dir);
            let service = echoing_service(&socket);
            let link = link(socket, never);

            let inbound = start_run_sized(url, None, MAX_INBOUND_BYTES - 20);
            assert_eq!(
                serde_json::to_vec(&Request::StartRun(inbound.clone()))
                    .unwrap()
                    .len(),
                MAX_INBOUND_BYTES - 20
            );
            let one_over = start_run_sized(url, Some(key.clone()), MAX_INBOUND_BYTES + 1);
            for request in [inbound, one_over] {
                let response = call(&mut config, &link, Request::StartRun(request));
                let (code, message) = refused(&response);
                assert_eq!(code, ErrorCode::MessageTooLarge, "{message}");
                assert!(message.contains("too large"), "{message}");
                assert_eq!(response.request_id(), "run-1");
            }
            assert!(
                service.requests().is_empty(),
                "the service was contacted: {:?}",
                service.requests().len()
            );

            // Exactly at the cap once keyed, it is forwarded.
            let at_cap = start_run_sized(url, Some(key.clone()), MAX_INBOUND_BYTES);
            match call(&mut config, &link, Request::StartRun(at_cap)) {
                Response::RunStarted(started) => assert_eq!(started.run.record_key, key),
                other => panic!("expected run_started, got {other:?}"),
            }
            let requests = service.requests();
            assert!(matches!(requests[0], Request::ServiceHello(_)));
            assert!(matches!(requests[1], Request::StartRun(_)));
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
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
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
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
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
            if crate::ipc_test_guard::ipc_denied() {
                return;
            }
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
            _folder: tempfile::TempDir,
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
                Self {
                    _folder: folder,
                    home,
                    path,
                }
            }

            fn store(&self) -> ConfigStore {
                ConfigStore::for_test(self.path.clone())
            }

            fn call(&self, config: &mut ConfigStore, request: Request) -> Response {
                dispatch(
                    request,
                    config,
                    &mut ConsentAuthority::new(),
                    &SetupContext::new(),
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
            // Every other setting, including the adapters, is kept. The v5
            // `[agent]` and `[harness_record]` tables load as not set up and
            // are dropped by this first save, which also writes the default
            // summary vocabulary (open access PR 3).
            let mut before = without_layout(before);
            assert!(before.remove("agent").is_some());
            assert!(before.remove("harness_record").is_some());
            let mut after = without_layout(after);
            assert!(!after.contains_key("agent") && !after.contains_key("harness_record"));
            assert!(after.remove("summary_vocabulary").is_some());
            assert_eq!(after, before);

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
                let held: Box<dyn std::any::Any> = if hold_capture {
                    Box::new(crate::capture::lock_capture(&state).unwrap())
                } else {
                    Box::new(writer.lock_current().unwrap())
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

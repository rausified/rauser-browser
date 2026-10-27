//! Chrome native messaging uses a four-byte, native-endian length prefix
//! followed by one UTF-8 JSON value. Stdout contains frames only.

use std::io::{self, Read, Write};

use anyhow::{Context, Result, bail};
use brauser_protocol::{
    ActivityRecorded, AgentChecked, ArtifactListResult, ArtifactResult, ArtifactView,
    ArtifactsRefreshed, BatchEventResult, BatchRecorded, CaptureEventKind, ConfigConfirmed,
    ConfigResult, ConfigUpdated, ErrorCode, ErrorResponse, FolderChosen, HarnessSetupConfirmed,
    HarnessesDiscovered, HelloResult, NoteConflict, NoteLoaded, NoteSaved, PROTOCOL_VERSION,
    RecordKey, RecordKeyKind, Request, Response, VisitOutcome, VisitRecorded, WhyRecorded,
    WorklogResult,
};

use crate::brand::NAMESPACE;
use crate::capture::{CaptureOutcome, CapturePolicy, CaptureStore, activity_shape_is_valid};
use crate::config::{self, ConfigStore};
use crate::consent::ConsentAuthority;
use crate::dialog::{self, DialogText};
use crate::harness::HarnessEnv;
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
/// connection; tests build it by hand with a fake search path and dialog.
pub struct SetupContext<'a> {
    pub env: &'a HarnessEnv,
    pub confirm: fn(DialogText<'_>) -> Result<bool>,
}

/// Process requests until the browser closes the native messaging pipe.
pub fn serve_with_io<R: Read, W: Write>(input: R, output: W, config: ConfigStore) -> Result<()> {
    let env = HarnessEnv::from_process(config.dir()?);
    let context = SetupContext {
        env: &env,
        confirm: dialog::confirm,
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
                value.consent_token.as_deref(),
                value.harness_token.as_deref(),
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
                authorized.harness_commit,
            ) {
                Ok(Some(revision)) => {
                    consent.consume_update(
                        &previous,
                        &previous_revision,
                        &value.config,
                        value.picker_token.as_deref(),
                        value.consent_token.as_deref(),
                        value.harness_token.as_deref(),
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
            match consent.choose_folder(config.revision()) {
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
        Request::ConfirmConfig(value) => {
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
                    "configuration changed; read it again before confirming",
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
            match consent.confirm_config(
                config.snapshot(),
                config.revision(),
                &value.config,
                value.picker_token.as_deref(),
            ) {
                Ok(Some(confirmed)) => Response::ConfigConfirmed(ConfigConfirmed {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: value.request_id,
                    consent_token: confirmed.consent_token,
                    summary: confirmed.summary,
                }),
                Ok(None) => error(
                    &value.request_id,
                    ErrorCode::Cancelled,
                    "configuration confirmation was canceled",
                ),
                Err(authority_error) => error(
                    &value.request_id,
                    ErrorCode::Unauthorized,
                    &authority_error.to_string(),
                ),
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
        Request::DiscoverHarnesses(value) => {
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
                    "configuration changed; read it again before setting up AI",
                );
            }
            let offers = consent.discover_harnesses(config.revision(), context.env);
            Response::HarnessesDiscovered(HarnessesDiscovered {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                offers,
            })
        }
        Request::ConfirmHarnessSetup(value) => {
            confirm_harness_setup(value, config, consent, context)
        }
    }
}

/// Native harness setup (§7.3): the host shows its own dialog and runs one
/// test prompt. The returned token authorizes exactly the returned config.
fn confirm_harness_setup(
    value: brauser_protocol::ConfirmHarnessSetupRequest,
    config: &mut ConfigStore,
    consent: &mut ConsentAuthority,
    context: &SetupContext<'_>,
) -> Response {
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
            "configuration changed; read it again before setting up AI",
        );
    }
    // Setup rewrites the whole file from this snapshot, so it must be the
    // file's own content, not a stand-in for one that needs repair.
    if config.config_issue().is_some() {
        return error(
            &value.request_id,
            ErrorCode::InvalidConfig,
            "configuration needs repair before AI setup",
        );
    }
    let current = config.snapshot().clone();
    match consent.confirm_harness_setup_with(
        &current,
        config.revision(),
        &value,
        context.env,
        context.confirm,
    ) {
        Ok(Some(confirmed)) => Response::HarnessSetupConfirmed(HarnessSetupConfirmed {
            protocol_version: PROTOCOL_VERSION,
            request_id: value.request_id,
            harness_token: confirmed.harness_token,
            config: confirmed.config,
            summary: confirmed.summary,
        }),
        Ok(None) => error(
            &value.request_id,
            ErrorCode::Cancelled,
            "harness setup was canceled",
        ),
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
        sites: &config.snapshot().sites,
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
    // Serde enforces types, not the JSON Schema's maxItems limits. Reject
    // oversized collections before replaying adapter evaluation for each
    // claimed mode or trail from this untrusted native message.
    if !activity_shape_is_valid(&value.event) {
        return answer(
            VisitOutcome::Rejected,
            Some("invalid_activity".into()),
            None,
        );
    }
    let matched = match config.adapter_match(&value.event.url, value.event.title.as_deref()) {
        Ok(matched) => matched,
        Err(_) => {
            return answer(
                VisitOutcome::Suppressed,
                Some("not_an_artifact".into()),
                None,
            );
        }
    };
    if matched.artifact_id != value.event.artifact_id
        || matched.artifact_type != value.event.artifact_type
    {
        return answer(
            VisitOutcome::Rejected,
            Some("artifact_identity_mismatch".into()),
            None,
        );
    }
    for mode in &value.event.modes {
        match config.adapter_match(&mode.url, None) {
            Ok(evidence)
                if evidence.artifact_id == value.event.artifact_id
                    && evidence
                        .mode
                        .is_some_and(|found| found.as_str() == mode.mode) => {}
            _ => {
                return answer(
                    VisitOutcome::Rejected,
                    Some("activity_mode_mismatch".into()),
                    None,
                );
            }
        }
    }
    for trail in &value.event.trails {
        match (&trail.from_artifact_id, &trail.from_url) {
            (None, None) => {}
            (Some(id), Some(url)) => match config.adapter_match(url, None) {
                Ok(source) if &source.artifact_id == id => {}
                _ => {
                    return answer(
                        VisitOutcome::Rejected,
                        Some("trail_identity_mismatch".into()),
                        None,
                    );
                }
            },
            _ => {
                return answer(
                    VisitOutcome::Rejected,
                    Some("trail_identity_mismatch".into()),
                    None,
                );
            }
        }
    }
    let expected_refs = matched.title.as_ref().map(|title| &title.refs);
    if value.event.refs.iter().any(|supplied| {
        !expected_refs.is_some_and(|refs| {
            refs.iter().any(|expected| {
                expected.artifact_type == supplied.artifact_type && expected.value == supplied.value
            })
        })
    }) {
        return answer(
            VisitOutcome::Rejected,
            Some("activity_refs_mismatch".into()),
            None,
        );
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
        sites: &config.snapshot().sites,
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
            sites: &config.snapshot().sites,
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
        sites: &config.snapshot().sites,
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
/// checks the site allowlist. The error is a small `(code, message)` pair,
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
    let key = match matched.as_ref() {
        Some(found) => RecordKey {
            kind: RecordKeyKind::Artifact,
            value: found.artifact_id.clone(),
        },
        // No adapter currently recognizes this URL (for example, its
        // adapter or last enabled site was removed). Before falling back to
        // a fresh URL key, check whether it was previously recorded under an
        // artifact, so that artifact's existing shared note stays reachable
        // instead of appearing empty. The note's own stored identity is
        // checked first because it survives independently of activity (a
        // note saved while capture was paused, or before any activity was
        // ever delivered, has no index row to recover from); the index is
        // only a fallback for a note saved under a different alternate URL.
        None => {
            // Collect every distinct match from BOTH sources into one set
            // before deciding uniqueness: stripped query identity can make
            // two different artifacts share the same canonical URL, and
            // trusting either source alone -- even one that is itself
            // unambiguous -- could still miss a conflicting match the
            // other source holds. Guessing between them would silently
            // redirect a save into an unrelated artifact's note, so the
            // recovery is trusted only when the union is unambiguous.
            let mut candidates = std::collections::BTreeSet::new();
            if let Ok(note_candidates) = vault.note_candidates() {
                candidates.extend(note_candidates.iter().filter_map(|contents| {
                    note::artifact_keyed_note_identity(contents, &canonical)
                }));
            }
            if let Ok(cache_dir) = config.cache_dir()
                && let Ok(index) =
                    ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir)
                && let Ok(ids) = index.artifact_candidates_for_url(&canonical)
            {
                candidates.extend(ids);
            }
            match candidates.len() {
                1 => RecordKey {
                    kind: RecordKeyKind::Artifact,
                    value: candidates
                        .into_iter()
                        .next()
                        .expect("checked len == 1 above"),
                },
                _ => {
                    crate::vault::url_record_key(&canonical).expect("canonical HTTP URL has a key")
                }
            }
        }
    };
    let mut older_urls = vec![canonical.clone()];
    if let Some(found) = matched
        && include_older_notes
    {
        if let Ok(cache_dir) = config.cache_dir()
            && let Ok(index) = ArtifactIndex::open_in(storage, config.root_identity(), &cache_dir)
            && let Ok(urls) = index.artifact_urls(&found.artifact_id)
        {
            older_urls.extend(urls);
        }
        if let Ok(candidates) = vault.note_candidates() {
            for contents in candidates {
                let Some(candidate) = note::url_keyed_note_url(&contents) else {
                    continue;
                };
                if config
                    .adapter_match(&candidate, None)
                    .is_ok_and(|candidate_match| candidate_match.artifact_id == found.artifact_id)
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
                "artifact index could not be read",
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
            "artifact index could not be read",
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
            "worklog could not be generated",
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
        ConfirmConfigRequest, GetConfigRequest, HelloRequest, RecordActivityRequest,
        RecordBatchRequest, RecordVisitRequest, UpdateConfigRequest, VisitEvent,
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
                    sites: Vec::new(),
                    strip_params: Vec::new(),
                    agent: None,
                    adapters: crate::config::empty_config().adapters,
                },
                picker_token: None,
                consent_token: None,
                harness_token: None,
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
                assert!(value.config.sites.is_empty());
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
            sites: Vec::new(),
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
                consent_token: None,
                harness_token: None,
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
                consent_token: None,
                harness_token: None,
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
        let context = SetupContext {
            env: &env,
            confirm: no_dialog,
        };

        let confirmed = dispatch(
            Request::ConfirmConfig(ConfirmConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "confirm".into(),
                expected_revision: revision.clone(),
                config: next.clone(),
                picker_token: None,
            }),
            &mut config,
            &mut consent,
            &context,
        );
        let consent_token = match confirmed {
            Response::ConfigConfirmed(value) => value.consent_token,
            other => panic!("expected config confirmation, got {other:?}"),
        };
        let updated = dispatch(
            Request::UpdateConfig(UpdateConfigRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "update".into(),
                expected_revision: revision,
                config: next,
                picker_token: None,
                consent_token: Some(consent_token),
                harness_token: None,
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
        let context = SetupContext {
            env: &env,
            confirm: no_dialog,
        };
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
                }),
                capture_enabled: false,
                sites: Vec::new(),
                strip_params: Vec::new(),
                agent: None,
                adapters: crate::config::empty_config().adapters,
            };
            let update = dispatch(
                Request::UpdateConfig(UpdateConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "probe-update".into(),
                    expected_revision: "missing".into(),
                    config: snapshot.clone(),
                    picker_token: None,
                    consent_token: None,
                    harness_token: None,
                }),
                &mut config,
                &mut consent,
                &context,
            );
            let confirm = dispatch(
                Request::ConfirmConfig(brauser_protocol::ConfirmConfigRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "probe-confirm".into(),
                    expected_revision: "missing".into(),
                    config: snapshot,
                    picker_token: None,
                }),
                &mut config,
                &mut consent,
                &context,
            );
            for response in [update, confirm] {
                match response {
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
            }),
            capture_enabled: false,
            sites: Vec::new(),
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

    fn write_adapter_config(
        path: &std::path::Path,
        root: &std::path::Path,
        origin: &str,
        adapter: &str,
    ) {
        config::fixtures::write(
            path,
            root,
            "artifacts_dir = \"artifacts\"",
            &format!(
                "capture_enabled = true\nsites = [{{ origin = {origin:?}, path_prefix = \"/\" }}]"
            ),
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
            "https://docs.google.com",
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
            "https://work.example",
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
            "https://work.example",
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
            "https://work.example",
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
            "https://acme.atlassian.net",
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
        write_adapter_config(&path, &root, "https://work.example", &adapter("work"));
        let mut config = loaded(&path);
        let url = "https://work.example/items/7";
        let loaded = note_load(&mut config, url);
        write_adapter_config(
            &path,
            &root,
            "https://work.example",
            &adapter("replacement"),
        );
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
        write_adapter_config(&path, &root, "https://work.example", "");
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
            "https://work.example",
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
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
                    occurred_at: "2026-10-16T13:00:00Z".into(),
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
        write_adapter_config(&path, &root, "https://work.example", "");

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
            "https://work.example",
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

        write_adapter_config(&path, &root, "https://work.example", "");

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

    #[test]
    fn saved_context_appears_before_the_first_activity_is_delivered() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("notes");
        std::fs::create_dir(&root).unwrap();
        let path = folder.path().join("config.toml");
        write_adapter_config(
            &path,
            &root,
            "https://work.example",
            r#"[[artifacts]]
type = "work.item"
origin = "https://work.example"
match = [{ path = "/items/{id:int}" }]
id = "work:{host}/{id}"
"#,
        );
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
                    occurred_at: "2026-10-16T13:00:00Z".into(),
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
                confirm: no_dialog,
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
                confirm: no_dialog,
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
        let context = SetupContext {
            env: &env,
            confirm: no_dialog,
        };
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

    fn no_dialog(_: DialogText<'_>) -> Result<bool> {
        bail!("no dialog is expected in this test")
    }

    fn served(requests: &[String], config: ConfigStore) -> Vec<Response> {
        let folder = tempfile::tempdir().unwrap();
        let env = no_harnesses(folder.path());
        served_with(
            requests,
            config,
            &SetupContext {
                env: &env,
                confirm: no_dialog,
            },
        )
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
            occurred_at: "2026-10-16T13:00:00Z".into(),
            incognito: false,
            opens: 1,
            focused_seconds: 3,
            background_seconds: 0,
            modes: Vec::new(),
            trails: Vec::new(),
            refs: Vec::new(),
        }
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
    /// path. No real harness is ever found or run.
    #[cfg(unix)]
    mod harness_setup {
        use super::*;
        use crate::harness::fake::{Fake, PASSING_BODY};
        use brauser_protocol::{
            AgentState, ConfirmHarnessSetupRequest, DiscoverHarnessesRequest, HarnessOffer,
        };

        struct Session {
            fake: Fake,
            env: HarnessEnv,
            path: std::path::PathBuf,
            config: ConfigStore,
            consent: ConsentAuthority,
        }

        fn accept(_: DialogText<'_>) -> Result<bool> {
            Ok(true)
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
            fn call(
                &mut self,
                request: Request,
                confirm: fn(DialogText<'_>) -> Result<bool>,
            ) -> Response {
                let context = SetupContext {
                    env: &self.env,
                    confirm,
                };
                dispatch(request, &mut self.config, &mut self.consent, &context)
            }

            fn revision(&mut self) -> String {
                self.config.refresh().unwrap();
                self.config.revision().to_owned()
            }

            fn discover(&mut self, revision: &str) -> Response {
                self.call(
                    Request::DiscoverHarnesses(DiscoverHarnessesRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "discover-1".into(),
                        expected_revision: revision.into(),
                    }),
                    no_dialog,
                )
            }

            fn offers(&mut self) -> Vec<HarnessOffer> {
                let revision = self.revision();
                match self.discover(&revision) {
                    Response::HarnessesDiscovered(value) => value.offers,
                    other => panic!("expected harnesses_discovered, got {other:?}"),
                }
            }

            fn claude_offer(&mut self) -> String {
                self.offers()
                    .into_iter()
                    .find(|offer| offer.harness_id == "claude-code")
                    .and_then(|offer| offer.offer_id)
                    .expect("the fake claude is offered")
            }

            fn confirm(
                &mut self,
                revision: &str,
                offer_id: &str,
                summaries_dir: Option<&str>,
                confirm: fn(DialogText<'_>) -> Result<bool>,
            ) -> Response {
                self.call(
                    Request::ConfirmHarnessSetup(ConfirmHarnessSetupRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "confirm-1".into(),
                        expected_revision: revision.into(),
                        offer_id: offer_id.into(),
                        env_names: Vec::new(),
                        summaries_dir: summaries_dir.map(str::to_owned),
                    }),
                    confirm,
                )
            }

            fn update(&mut self, next: ConfigSnapshot, harness_token: Option<&str>) -> Response {
                let revision = self.revision();
                self.call(
                    Request::UpdateConfig(UpdateConfigRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "write-1".into(),
                        expected_revision: revision,
                        config: next,
                        picker_token: None,
                        consent_token: None,
                        harness_token: harness_token.map(str::to_owned),
                    }),
                    no_dialog,
                )
            }

            /// Discover, confirm, and commit the fake claude.
            fn set_up(&mut self) {
                let offer = self.claude_offer();
                let revision = self.revision();
                let confirmed = match self.confirm(&revision, &offer, None, accept) {
                    Response::HarnessSetupConfirmed(value) => value,
                    other => panic!("expected harness_setup_confirmed, got {other:?}"),
                };
                match self.update(confirmed.config.clone(), Some(&confirmed.harness_token)) {
                    Response::ConfigUpdated(_) => {}
                    other => panic!("expected config_updated, got {other:?}"),
                }
                // The token was consumed by the commit.
                let again = self.update(confirmed.config, Some(&confirmed.harness_token));
                assert_eq!(refused(&again).0, ErrorCode::Unauthorized);
            }

            fn status(&mut self) -> brauser_protocol::AgentStatus {
                match self.call(
                    Request::GetConfig(GetConfigRequest {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: "read-1".into(),
                    }),
                    no_dialog,
                ) {
                    Response::ConfigResult(value) => value.agent_status,
                    other => panic!("expected config_result, got {other:?}"),
                }
            }
        }

        #[test]
        fn discover_lists_fake_claude_ready_and_codex_refused() {
            let mut session = session(PASSING_BODY);
            session
                .fake
                .install("codex", "codex-cli 0.130.0", "", "exit 0");
            let offers = session.offers();
            assert_eq!(offers.len(), 2);
            let claude = offers
                .iter()
                .find(|offer| offer.harness_id == "claude-code")
                .unwrap();
            assert!(claude.offer_id.is_some());
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
            assert_eq!(codex.offer_id, None);
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
            assert_eq!(session.revision(), loaded(&session.path).revision());
            assert_eq!(refused(&session.discover("stale")).0, ErrorCode::Conflict);
        }

        #[test]
        fn confirm_harness_setup_rejects_unknown_or_stale_offer() {
            let mut session = session(PASSING_BODY);
            let revision = session.revision();
            let response = session.confirm(&revision, "forged", None, no_dialog);
            assert_eq!(refused(&response).0, ErrorCode::Unauthorized);

            let offer = session.claude_offer();
            let stale = session.revision();
            let next = ConfigSnapshot {
                ..session.config.snapshot().clone()
            };
            assert!(matches!(
                session.update(next, None),
                Response::ConfigUpdated(_)
            ));
            let response = session.confirm(&stale, &offer, None, no_dialog);
            assert_eq!(refused(&response).0, ErrorCode::Conflict);
            // An offer minted before the change is bound to the old revision.
            let current = session.revision();
            let response = session.confirm(&current, &offer, None, no_dialog);
            assert_eq!(refused(&response).0, ErrorCode::Unauthorized);
            assert_eq!(session.fake.log("claude", "stdin.log"), None);
        }

        #[test]
        fn summaries_overlap_at_setup_is_recoverable_invalid_config() {
            let mut session = session(PASSING_BODY);
            let before = std::fs::read(&session.path).unwrap();
            let offer = session.claude_offer();
            let revision = session.revision();
            let response = session.confirm(&revision, &offer, Some("log"), no_dialog);
            let (code, message) = refused(&response);
            assert_eq!(code, ErrorCode::InvalidConfig);
            assert!(message.contains("must not overlap"), "{message}");
            assert_eq!(std::fs::read(&session.path).unwrap(), before);
            // Detect again and choose a different folder.
            let offer = session.claude_offer();
            let response = session.confirm(&revision, &offer, Some("ai"), accept);
            match response {
                Response::HarnessSetupConfirmed(value) => assert_eq!(
                    value.config.storage.unwrap().summaries_dir.as_deref(),
                    Some("ai")
                ),
                other => panic!("expected harness_setup_confirmed, got {other:?}"),
            }
        }

        #[test]
        fn check_agent_reports_harness_after_setup() {
            let mut session = session(PASSING_BODY);
            session.set_up();
            match session.call(
                serde_json::from_str(&check_agent_json()).unwrap(),
                no_dialog,
            ) {
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
            session.set_up();
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
}

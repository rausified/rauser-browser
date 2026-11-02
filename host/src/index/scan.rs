//! Daily-log discovery and marker parsing, and `LogScan`: one bounded,
//! log-only pass over the daily logs that never reads or writes the cache.

use super::*;

/// The events, positions, gaps and conflicts one pass over the daily logs
/// finds, with no persistence and no cache. Relearn uses
/// it for authoritative evidence that no cache can influence.
pub(super) struct LogScan {
    /// One body per event ID: the copy at the earliest log position.
    pub(super) events: BTreeMap<String, CachedEvent>,
    /// Each event's earliest durable position (log path, zero-based line).
    pub(super) positions: BTreeMap<String, (PathBuf, u64)>,
    /// Overflow notices, each at its earliest log position.
    pub(super) gaps: BTreeMap<String, CachedGap>,
    /// Rejected bodies for an event ID, at most 8, each at its earliest
    /// position.
    pub(super) conflicts: BTreeMap<String, Vec<Conflict>>,
    /// Every log was read; otherwise the per-pass read bound stopped the scan
    /// before the next log in sorted order.
    pub(super) complete: bool,
}

impl LogScan {
    /// Read the selected home folder's logs in sorted order, within the
    /// per-file and per-pass bounds. `strict` refuses symlinked or
    /// non-regular log history instead of skipping it, so a review can never
    /// present an omission as a complete inventory.
    pub(super) fn read(storage: &StorageConfig, root_identity: &str, strict: bool) -> Result<Self> {
        let mut scan = Self {
            events: BTreeMap::new(),
            positions: BTreeMap::new(),
            gaps: BTreeMap::new(),
            conflicts: BTreeMap::new(),
            complete: true,
        };
        let (root, _) = open_selected_root(Path::new(&storage.root), Some(root_identity))?;
        let log_dir = checked_relative_dir(&storage.log_dir)?;
        if strict
            && root
                .symlink_metadata(&log_dir)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("re-learning review cannot inspect symlinked log history");
        }
        let logs = match root.open_dir(&log_dir) {
            Ok(logs) => logs,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(scan),
            Err(error) => {
                return Err(error).context("artifact history is unavailable: opening logs");
            }
        };
        let mut files = Vec::new();
        collect_markdown(&logs, Path::new(""), 0, &mut files, strict)?;
        files.sort();
        let shown = |relative: &Path| log_dir.join(relative).display().to_string();
        let mut checkpoints = Vec::with_capacity(files.len());
        for relative in files {
            let metadata = logs
                .symlink_metadata(&relative)
                .with_context(|| format!("checking artifact log file {}", shown(&relative)))?;
            if !metadata.is_file() {
                bail!(
                    "artifact history is unavailable: {} is not a regular file",
                    shown(&relative)
                );
            }
            checkpoints.push((relative, LogCheckpoint::read(&metadata)?));
        }
        let limit = scan_byte_limit();
        let mut scanned = 0;
        for (relative, checkpoint) in &checkpoints {
            if checkpoint.bytes > MAX_LOG_BYTES {
                bail!(
                    "artifact history is unavailable: log exceeds its 32 MiB limit: {}",
                    shown(relative)
                );
            }
            // One log always fits (the per-file bound is below the pass
            // bound), so a pass always reads at least one log.
            if scanned > 0 && scanned + checkpoint.bytes > limit {
                scan.complete = false;
                break;
            }
            scanned += checkpoint.bytes;
            let mut file = logs
                .open(relative)
                .with_context(|| format!("opening artifact log file {}", shown(relative)))?;
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(checkpoint.bytes + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("reading artifact log file {}", shown(relative)))?;
            if bytes.len() as u64 != checkpoint.bytes
                || LogCheckpoint::read(&file.metadata()?)? != *checkpoint
            {
                bail!(
                    "artifact history is unavailable: {} changed during catch-up",
                    shown(relative)
                );
            }
            #[cfg(test)]
            LOG_BYTES_READ.with(|count| count.set(count.get() + bytes.len() as u64));
            // Markers are ASCII. A torn multibyte prose line (capture
            // tolerates one and keeps appending) must not drop the whole day.
            let text = String::from_utf8_lossy(&bytes);
            if !text.starts_with(LOG_HEADER) {
                continue;
            }
            for (line, event) in parse_log_lines(&text, strict)
                .with_context(|| format!("inspecting artifact log {}", shown(relative)))?
            {
                scan.ingest(event, (relative.clone(), line));
            }
            for (number, line) in text.lines().enumerate() {
                if let Some((id, marker)) = parse_overflow_marker(line) {
                    let position = (relative.clone(), number as u64);
                    // Retries reuse the marker ID; the earliest copy wins.
                    let gap = scan.gaps.entry(id).or_insert_with(|| CachedGap {
                        marker: marker.clone(),
                        position: position.clone(),
                    });
                    if position < gap.position {
                        *gap = CachedGap { marker, position };
                    }
                }
            }
        }
        Ok(scan)
    }

    /// Add one logged event. A reused ID with a different body keeps the copy
    /// at the earliest log position and records the other as a conflict; the
    /// same body seen earlier takes that position.
    fn ingest(&mut self, event: CachedEvent, position: (PathBuf, u64)) {
        let (id, _) = event_identity(&event);
        let Some(previous) = self.events.get(&id) else {
            self.positions.insert(id.clone(), position);
            self.events.insert(id, event);
            return;
        };
        if previous == &event {
            if self
                .positions
                .get(&id)
                .is_none_or(|indexed| &position < indexed)
            {
                self.positions.insert(id, position);
            }
            return;
        }
        let (event, position) = if self
            .positions
            .get(&id)
            .is_some_and(|indexed| &position < indexed)
        {
            let rejected_at = self.positions.insert(id.clone(), position);
            let rejected = self.events.insert(id.clone(), event).unwrap();
            (rejected, rejected_at)
        } else {
            (event, Some(position))
        };
        let chosen = &self.events[&id];
        let seen = self.conflicts.entry(id).or_default();
        seen.retain(|(body, _)| body != chosen);
        if let Some((_, earliest)) = seen.iter_mut().find(|(body, _)| body == &event) {
            if let Some(position) = position
                && earliest.as_ref().is_none_or(|indexed| &position < indexed)
            {
                *earliest = Some(position);
            }
        } else if seen.len() < 8 {
            seen.push((event, position));
        }
    }

    /// The relearn revision: a hash of the scanned events, keyed by ID.
    pub(super) fn revision(&self) -> Result<String> {
        Ok(sha256_hex(&serde_json::to_vec(&self.events)?))
    }
}

pub(super) fn collect_markdown(
    dir: &Dir,
    prefix: &Path,
    depth: usize,
    output: &mut Vec<PathBuf>,
    strict: bool,
) -> Result<()> {
    if depth > 8 {
        bail!("artifact log directory nesting exceeds its limit");
    }
    for entry in dir.entries().context("listing artifact log directory")? {
        let entry = entry.context("listing artifact log directory")?;
        let name = entry.file_name();
        let relative = prefix.join(&name);
        let metadata = dir
            .symlink_metadata(&name)
            .context("checking artifact log entry")?;
        if metadata.file_type().is_symlink() {
            if strict {
                bail!("re-learning review cannot inspect symlinked log history");
            }
            continue;
        }
        if metadata.is_dir() {
            let child = dir
                .open_dir(&name)
                .context("opening artifact log subdirectory")?;
            collect_markdown(&child, &relative, depth + 1, output, strict)?;
        } else if relative.extension().and_then(|value| value.to_str()) == Some("md") {
            if !metadata.is_file() {
                if strict {
                    bail!("re-learning review cannot inspect a non-regular log file");
                }
                continue;
            }
            output.push(relative);
            if output.len() > MAX_SCAN_FILES {
                bail!("artifact log contains too many files to index");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn parse_log(text: &str) -> Vec<CachedEvent> {
    parse_log_lines(text, false)
        .unwrap()
        .into_iter()
        .map(|(_, event)| event)
        .collect()
}

/// Owned events with their zero-based line numbers. `strict` refuses a
/// damaged owned marker instead of skipping it.
pub(super) fn parse_log_lines(text: &str, strict: bool) -> Result<Vec<(u64, CachedEvent)>> {
    let mut output = Vec::new();
    let mut previous = "";
    for (number, line) in text.lines().enumerate() {
        let event = parse_marker_event(line, previous);
        let marker_start = concat!("<!-- ", crate::namespace!(), ":visit");
        if strict
            && line.contains(marker_start)
            && (line.matches(marker_start).count() != 1
                || (event.is_none() && parse_overflow_marker(line).is_none()))
        {
            // Indexing tolerates torn/invalid entries so later completed
            // appends can recover them. An identity review must never turn
            // the same omission into an apparently complete, approvable plan.
            bail!("damaged owned log marker at line {}", number + 1);
        }
        if let Some(event) = event {
            output.push((number as u64, event));
        }
        previous = line;
    }
    Ok(output)
}

/// Overflow is system evidence, never an artifact, visit, duration or open.
/// Only bounded complete owned markers count; presentation text is ignored.
pub(super) fn parse_overflow_marker(line: &str) -> Option<(String, OverflowMarker)> {
    let rest = line.strip_prefix(MARKER_PREFIX)?.strip_suffix(" -->")?;
    let fields = rest.split(' ').collect::<Vec<_>>();
    if !(5..=6).contains(&fields.len()) || !valid_hash(fields[0]) || fields[3] != "kind=overflow" {
        return None;
    }
    let url_hash = fields[1].strip_prefix("url=")?;
    if !valid_hash(url_hash) {
        return None;
    }
    let at = fields[2].strip_prefix("at=")?;
    OffsetDateTime::parse(at, &Rfc3339).ok()?;
    let data = fields[4].strip_prefix("data=")?;
    // An 8 KiB anchor URL can double in JSON (query backslashes), then
    // double again in hex. Leave room for that and the bounded gap fields.
    if data.len() > 64 * 1024 {
        return None;
    }
    let mut payload: serde_json::Value = serde_json::from_slice(&hex::decode(data).ok()?).ok()?;
    // Attached markers used a visit as their anchor: its URL was added to
    // the payload and its time selected the log. Neither identifies lost work.
    let anchor = payload.as_object_mut()?.remove("url");
    let standalone = anchor.is_none();
    match anchor {
        Some(serde_json::Value::String(url))
            if safe_page_url(&url) && sha256_hex(url.as_bytes()) == url_hash => {}
        None if sha256_hex(b"") == url_hash => {}
        _ => return None,
    }
    let marker: OverflowMarker = serde_json::from_value(payload).ok()?;
    if marker.first_occurred_at.len() > 64 || marker.last_occurred_at.len() > 64 {
        return None;
    }
    let first = OffsetDateTime::parse(&marker.first_occurred_at, &Rfc3339).ok()?;
    let last = OffsetDateTime::parse(&marker.last_occurred_at, &Rfc3339).ok()?;
    if marker.count == 0 || first > last || (standalone && at != marker.last_occurred_at) {
        return None;
    }
    if let Some(encoded) = fields.get(5) {
        let encoded = encoded.strip_prefix("stripped=")?;
        if encoded.len() > 512 * 1024 {
            return None;
        }
        let provenance: UrlProvenance = serde_json::from_slice(&hex::decode(encoded).ok()?).ok()?;
        if !provenance.valid(0, &[]) {
            return None;
        }
    }
    Some((fields[0].to_owned(), marker))
}

pub(super) fn parse_marker_event(line: &str, previous: &str) -> Option<CachedEvent> {
    let rest = line.strip_prefix(MARKER_PREFIX)?.strip_suffix(" -->")?;
    let (id_hash, rest) = rest.split_once(" url=")?;
    let (url_hash, rest) = rest.split_once(" at=")?;
    if !valid_hash(id_hash) || !valid_hash(url_hash) {
        return None;
    }
    let mut fields = rest.split(' ');
    let at = fields.next()?;
    OffsetDateTime::parse(at, &Rfc3339).ok()?;
    let metadata = fields.collect::<Vec<_>>();
    let kind = metadata
        .iter()
        .find_map(|field| field.strip_prefix("kind="));
    let data = metadata
        .iter()
        .find_map(|field| field.strip_prefix("data="));
    let data = match (kind, data) {
        (Some("activity"), Some(data)) => {
            let bytes = hex::decode(data).ok()?;
            if bytes.len() > 256 * 1024 {
                return None;
            }
            let mut event: ActivityEvent = serde_json::from_slice(&bytes).ok()?;
            // Older payloads kept the queued spelling; the marker always
            // hashed the canonical UUID. Normalize before validating or caching.
            event.event_id = Uuid::parse_str(&event.event_id).ok()?.to_string();
            if !logged_activity_is_bounded(&event)
                || event.artifact_id.is_empty()
                || event.artifact_id.len() > 512
                || event.artifact_type.is_empty()
                || event.artifact_type.len() > 64
                || !safe_page_url(&event.url)
                || event.modes.iter().any(|mode| !safe_page_url(&mode.url))
                || event
                    .trails
                    .iter()
                    .filter_map(|trail| trail.from_url.as_deref())
                    .any(|url| !safe_page_url(url))
                || sha256_hex(event.event_id.as_bytes()) != id_hash
                || sha256_hex(event.url.as_bytes()) != url_hash
                || event.occurred_at != at
            {
                return None;
            }
            Some(EventData::Activity(event))
        }
        (Some("why"), Some(data)) => {
            let bytes = hex::decode(data).ok()?;
            if bytes.len() > 256 * 1024 {
                return None;
            }
            let mut event: WhyEvent = serde_json::from_slice(&bytes).ok()?;
            event.event_id = Uuid::parse_str(&event.event_id).ok()?.to_string();
            if event.artifact_id.is_empty()
                || event.artifact_id.len() > 512
                || event.text.chars().count() > 500
                || !safe_page_url(&event.url)
                || sha256_hex(event.event_id.as_bytes()) != id_hash
                || sha256_hex(event.url.as_bytes()) != url_hash
                || event.occurred_at != at
            {
                return None;
            }
            Some(EventData::Why(event))
        }
        (Some("summary"), Some(data)) => {
            let bytes = hex::decode(data).ok()?;
            if bytes.len() > 256 * 1024 {
                return None;
            }
            let event: SummaryEvent = serde_json::from_slice(&bytes).ok()?;
            if event.event_id.is_empty()
                || event.event_id.len() > 512
                || !safe_page_url(&event.url)
                || sha256_hex(event.event_id.as_bytes()) != id_hash
                || sha256_hex(event.url.as_bytes()) != url_hash
                || event.occurred_at != at
                || !crate::summary::validate_summary_event(&event)
            {
                return None;
            }
            Some(EventData::Summary(event))
        }
        (None, None) => parse_legacy(previous, id_hash, url_hash, at).map(EventData::Legacy),
        _ => None,
    }?;
    let encoded = metadata
        .iter()
        .filter_map(|field| field.strip_prefix("stripped="))
        .collect::<Vec<_>>();
    let url_provenance = match encoded.as_slice() {
        [] => None,
        [encoded] => {
            if encoded.len() > 512 * 1024 {
                return None;
            }
            let provenance: UrlProvenance =
                serde_json::from_slice(&hex::decode(encoded).ok()?).ok()?;
            let (modes, trails) = match &data {
                EventData::Activity(event) => (
                    event.modes.len(),
                    event
                        .trails
                        .iter()
                        .map(|trail| trail.from_url.is_some())
                        .collect::<Vec<_>>(),
                ),
                _ => (0, Vec::new()),
            };
            if !provenance.valid(modes, &trails) {
                return None;
            }
            Some(provenance)
        }
        _ => return None,
    };
    Some(CachedEvent {
        data,
        url_provenance,
    })
}

pub(super) fn parse_legacy(
    line: &str,
    marker_id: &str,
    url_hash: &str,
    at: &str,
) -> Option<LegacyVisit> {
    let body = line.strip_prefix("- ")?;
    let (_, tail) = body.split_once(" — ")?;
    let (title, url) = tail.rsplit_once(" — <")?;
    let url = url.strip_suffix('>')?;
    let parsed = Url::parse(url).ok()?;
    if url.len() > 8192
        || title.len() > 2048
        || !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || sha256_hex(url.as_bytes()) != url_hash
    {
        return None;
    }
    let host = parsed.host_str()?;
    Some(LegacyVisit {
        marker_id: marker_id.into(),
        artifact_id: format!("web:{host}/{url_hash}"),
        occurred_at: at.into(),
        url: url.into(),
        title: unescape_markdown(title),
    })
}

pub(super) fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn safe_page_url(value: &str) -> bool {
    value.len() <= 8192
        && Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

pub(super) fn unescape_markdown(value: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
}

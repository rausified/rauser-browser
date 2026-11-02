//! Daily-log discovery and marker parsing.

use super::*;

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
    parse_log_lines(text)
        .into_iter()
        .map(|(_, event)| event)
        .collect()
}

/// Owned events with their zero-based line numbers.
pub(super) fn parse_log_lines(text: &str) -> Vec<(u64, CachedEvent)> {
    let mut output = Vec::new();
    let mut previous = "";
    for (number, line) in text.lines().enumerate() {
        if let Some(event) = parse_marker_event(line, previous) {
            output.push((number as u64, event));
        }
        previous = line;
    }
    output
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

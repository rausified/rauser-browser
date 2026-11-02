//! Incremental catch-up of the daily logs into `index.db` (plan T9).
//!
//! `brauser_logs` is the only progress kept for the logs: per log, its size
//! and modification time when last read, how far it was parsed (the end of
//! its last complete line, and how many lines that is) and a SHA-256 of the
//! up-to-4-KiB window before that offset. A pass, run inside the caller's
//! `write_txn` so the rows it ingests and the watermark commit together:
//!
//! - skips a log whose size and modification time are unchanged;
//! - parses only the appended tail of a grown log, when the byte before the
//!   parsed offset is a newline and the window still hashes the same;
//! - rereads a log in full (bounded by `MAX_LOG_BYTES`) on any doubt: the same
//!   size with a new modification time, a guard mismatch, a log never parsed
//!   past its header, or a legacy marker whose prose line is not in the
//!   window. Ingest is idempotent, so a reread never duplicates anything;
//! - requests a staged rebuild (`brauser_meta.rebuild = 'staging'`) for a
//!   shrunk or removed log, and ingests nothing while one is pending.
//!
//! Events go to the store through the caller's [`Ingest`] sink, in log
//! order with their positions. Overflow markers go into `brauser_gaps`,
//! each at its earliest position. A trailing partial line is parsed (a
//! complete marker without its newline still counts) but never passes the
//! watermark, so its completion is read again. An in-place edit outside the
//! hashed window is not noticed; the logs are append-only, and
//! `brauser reindex` is the documented recovery.

use std::io::{Seek, SeekFrom};
use std::path::Component;

use rusqlite::{OptionalExtension, Transaction, params};

use super::*;

/// The bytes before the parsed offset whose hash guards a tail parse.
pub(super) const TAIL_WINDOW: u64 = 4096;
/// `brauser_logs.purpose` of the watermark this module keeps.
const EVENTS: &str = "events";

#[cfg(test)]
thread_local! {
    /// Runs once, right after the next pass on this thread stats the logs:
    /// a test's stand-in for another process appending at that moment.
    pub(super) static AFTER_STAT: std::cell::Cell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::Cell::new(None) };
}

/// Which watermark and gap tables a pass reads and advances: the live ones,
/// or the staged rebuild's (`rebuild.rs`), which are read the same way into
/// `brauser_stage_*` while the live ones keep serving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Live,
    Stage,
}

impl Target {
    fn logs(self) -> &'static str {
        match self {
            Self::Live => "brauser_logs",
            Self::Stage => "brauser_stage_logs",
        }
    }

    fn gaps(self) -> &'static str {
        match self {
            Self::Live => "brauser_gaps",
            Self::Stage => "brauser_stage_gaps",
        }
    }
}

/// Where one logged event goes: the store's ingest, inside the same
/// transaction, with the event's durable position (log-dir-relative path,
/// zero-based line).
pub(super) type Ingest<'a> =
    dyn FnMut(&Transaction<'_>, CachedEvent, (PathBuf, u64)) -> Result<()> + 'a;

/// What one catch-up pass did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CatchUp {
    /// A staged rebuild is pending (requested now or earlier); nothing was
    /// read or ingested.
    Rebuilding,
    /// `logs_read` changed logs were read in sorted order. `complete` is
    /// false when the per-pass read bound stopped the pass before the next
    /// one; the logs already read are checkpointed, so the next pass resumes.
    Caught { logs_read: usize, complete: bool },
}

/// The stat-only comparison of the logs with the watermark, for the reads'
/// freshness check (plan T11).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Freshness {
    /// Every log is checkpointed at its current size and modification time.
    Current,
    /// Some log is new, changed, shrunk or removed: a catch-up is due.
    Behind,
    /// A staged rebuild is pending.
    Rebuilding,
}

/// One log file as stat'ed for this pass.
struct Stat {
    relative: PathBuf,
    /// `relative` with `/` separators, the `brauser_logs.path` key.
    key: String,
    checkpoint: LogCheckpoint,
    modified_ns: i64,
}

/// One `brauser_logs` row.
struct Watermark {
    bytes: u64,
    modified_ns: i64,
    parsed_bytes: u64,
    parsed_lines: u64,
    tail_sha256: Vec<u8>,
}

/// The bytes one pass read from a log, and where parsing starts in them.
struct LogRead {
    bytes: Vec<u8>,
    /// The file offset of `bytes[0]`.
    base: u64,
    /// The file offset parsing starts at, always at a line start.
    start: u64,
    /// The zero-based line number at `start`.
    first_line: u64,
    /// The line before `start` (empty at the start of the file), which a
    /// legacy marker takes its title and URL from.
    previous: String,
}

/// Catch the database up with the logs, inside the caller's write
/// transaction, which commits the ingested rows and the watermark together.
/// The watermark is read inside that transaction, so two hosts never ingest
/// the same tail twice.
pub(super) fn catch_up(
    tx: &Transaction<'_>,
    storage: &StorageConfig,
    root_identity: &str,
    ingest: &mut Ingest<'_>,
) -> Result<CatchUp> {
    catch_up_into(tx, Target::Live, storage, root_identity, ingest)
}

/// [`catch_up`] against `target`'s tables. For [`Target::Stage`] a pending
/// rebuild is the point, so it is not checked, and a shrunk or removed log
/// returns [`CatchUp::Rebuilding`] without a request: the stage must restart.
pub(super) fn catch_up_into(
    tx: &Transaction<'_>,
    target: Target,
    storage: &StorageConfig,
    root_identity: &str,
    ingest: &mut Ingest<'_>,
) -> Result<CatchUp> {
    if target == Target::Live && rebuilding(tx)? {
        return Ok(CatchUp::Rebuilding);
    }
    let stored = watermarks_in(tx, target)?;
    let Some((logs, log_dir, stats)) = stat_logs(storage, root_identity, !stored.is_empty())?
    else {
        return Ok(CatchUp::Caught {
            logs_read: 0,
            complete: true,
        });
    };
    #[cfg(test)]
    if let Some(hook) = AFTER_STAT.with(std::cell::Cell::take) {
        hook();
    }
    // A removed or truncated log invalidates what was ingested from it.
    // Rebuild from the remaining logs instead of keeping ghosts.
    let current = stats
        .iter()
        .map(|stat| (stat.key.as_str(), stat))
        .collect::<BTreeMap<_, _>>();
    if stored.iter().any(|(path, mark)| {
        current
            .get(path.as_str())
            .is_none_or(|stat| stat.checkpoint.bytes < mark.bytes)
    }) {
        if target == Target::Live {
            request_rebuild(tx)?;
        }
        return Ok(CatchUp::Rebuilding);
    }
    let shown = |relative: &Path| log_dir.join(relative).display().to_string();
    let limit = scan_byte_limit();
    let (mut scanned, mut logs_read, mut complete) = (0u64, 0usize, true);
    for stat in &stats {
        let mark = stored.get(&stat.key);
        if mark.is_some_and(|mark| unchanged(mark, stat)) {
            continue;
        }
        let bytes = stat.checkpoint.bytes;
        if bytes > MAX_LOG_BYTES {
            bail!(
                "artifact history is unavailable: log exceeds its 32 MiB limit: {}",
                shown(&stat.relative)
            );
        }
        // A tail read is charged as the whole log, so a pass never reads
        // more than its bound, even when a guard turns it into a reread.
        // One log always fits (the per-file bound is below the pass
        // bound), so every pass makes progress.
        if scanned > 0 && scanned + bytes > limit {
            complete = false;
            break;
        }
        scanned += bytes;
        let tail = mark.filter(|mark| bytes > mark.bytes && mark.parsed_bytes > 0);
        let read = read_log(&logs, stat, tail)
            .with_context(|| format!("reading artifact log file {}", shown(&stat.relative)))?;
        ingest_log(tx, target, stat, read, ingest)?;
        logs_read += 1;
    }
    Ok(CatchUp::Caught {
        logs_read,
        complete,
    })
}

/// Compare the logs with the watermark by their metadata alone; no log
/// byte is read. Runs in a `read_txn` before a read decides whether a
/// catch-up must run first.
pub(super) fn freshness(
    tx: &Transaction<'_>,
    storage: &StorageConfig,
    root_identity: &str,
) -> Result<Freshness> {
    if rebuilding(tx)? {
        return Ok(Freshness::Rebuilding);
    }
    let stored = watermarks(tx)?;
    let Some((_, _, stats)) = stat_logs(storage, root_identity, !stored.is_empty())? else {
        return Ok(Freshness::Current);
    };
    let current = stats.len() == stored.len()
        && stats.iter().all(|stat| {
            stored
                .get(&stat.key)
                .is_some_and(|mark| unchanged(mark, stat))
        });
    Ok(if current {
        Freshness::Current
    } else {
        Freshness::Behind
    })
}

/// Whether a staged rebuild is pending (`brauser_meta.rebuild`).
pub(super) fn rebuilding(tx: &Transaction<'_>) -> Result<bool> {
    let value: Option<String> = tx
        .query_row(
            "SELECT value FROM brauser_meta WHERE key = 'rebuild'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value.as_deref() == Some("staging"))
}

/// Ask for a staged rebuild; the rebuild itself (`rebuild.rs`) prepares the
/// stage tables and clears the flag when it finalizes.
pub(super) fn request_rebuild(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        "INSERT INTO brauser_meta(key, value) VALUES ('rebuild', 'staging')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [],
    )?;
    Ok(())
}

/// A log-dir-relative path with `/` separators, as `brauser_logs.path` and
/// `brauser_events.log_path` store it. `None` for a path with a component
/// that is not a UTF-8 name, which is never indexed.
pub(super) fn portable_path(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => parts.push(name.to_str()?),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn unchanged(mark: &Watermark, stat: &Stat) -> bool {
    mark.bytes == stat.checkpoint.bytes && mark.modified_ns == stat.modified_ns
}

fn modified_ns(checkpoint: &LogCheckpoint) -> i64 {
    let (secs, nanos) = checkpoint.modified;
    i64::try_from(i128::from(secs) * 1_000_000_000 + i128::from(nanos)).unwrap_or(i64::MAX)
}

fn to_u64(value: i64) -> Result<u64> {
    u64::try_from(value).context("artifact index watermark is negative")
}

fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).context("artifact index watermark is out of range")
}

fn watermarks(tx: &Transaction<'_>) -> Result<BTreeMap<String, Watermark>> {
    watermarks_in(tx, Target::Live)
}

fn watermarks_in(tx: &Transaction<'_>, target: Target) -> Result<BTreeMap<String, Watermark>> {
    let mut statement = tx.prepare(&format!(
        "SELECT path, bytes, modified_ns, parsed_bytes, parsed_lines, tail_sha256
         FROM {} WHERE purpose = ?1",
        target.logs()
    ))?;
    let rows = statement.query_map([EVENTS], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, Vec<u8>>(5)?,
        ))
    })?;
    let mut marks = BTreeMap::new();
    for row in rows {
        let (path, bytes, modified_ns, parsed_bytes, parsed_lines, tail_sha256) = row?;
        let mark = Watermark {
            bytes: to_u64(bytes)?,
            modified_ns,
            parsed_bytes: to_u64(parsed_bytes)?,
            parsed_lines: to_u64(parsed_lines)?,
            tail_sha256,
        };
        anyhow::ensure!(
            mark.parsed_bytes <= mark.bytes,
            "artifact index watermark is inconsistent"
        );
        marks.insert(path, mark);
    }
    Ok(marks)
}

/// Every log under the selected home folder's log directory, in sorted
/// order, with its metadata. `None` when the directory does not exist and
/// nothing was read from it yet; once logs were read, a missing directory
/// (an unmounted volume, a move in progress) makes history unavailable
/// rather than wiping it.
fn stat_logs(
    storage: &StorageConfig,
    root_identity: &str,
    read_before: bool,
) -> Result<Option<(Dir, PathBuf, Vec<Stat>)>> {
    let (root, _) = open_selected_root(Path::new(&storage.root), Some(root_identity))?;
    let log_dir = checked_relative_dir(&storage.log_dir)?;
    let logs = match root.open_dir(&log_dir) {
        Ok(logs) => logs,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !read_before => {
            return Ok(None);
        }
        Err(error) => return Err(error).context("artifact history is unavailable: opening logs"),
    };
    let mut files = Vec::new();
    collect_markdown(&logs, Path::new(""), 0, &mut files, false)?;
    files.sort();
    let shown = |relative: &Path| log_dir.join(relative).display().to_string();
    let mut stats = Vec::with_capacity(files.len());
    for relative in files {
        let Some(key) = portable_path(&relative) else {
            tracing::warn!(
                path = %shown(&relative),
                "skipping an artifact log whose name is not UTF-8"
            );
            continue;
        };
        let metadata = logs
            .symlink_metadata(&relative)
            .with_context(|| format!("checking artifact log file {}", shown(&relative)))?;
        if !metadata.is_file() {
            bail!(
                "artifact history is unavailable: {} is not a regular file",
                shown(&relative)
            );
        }
        let checkpoint = LogCheckpoint::read(&metadata)?;
        stats.push(Stat {
            relative,
            key,
            modified_ns: modified_ns(&checkpoint),
            checkpoint,
        });
    }
    Ok(Some((logs, log_dir, stats)))
}

/// Read `len` bytes at `offset`, failing if the file no longer holds them
/// (it shrank since it was stat'ed). Bytes appended since are not read:
/// the logs are append-only, so the stat'ed prefix is a consistent cut, and
/// the watermark records the stat'ed size, so the next pass reads the rest.
fn read_exact_range(file: &mut cap_std::fs::File, offset: u64, len: u64) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    Read::by_ref(file).take(len).read_to_end(&mut bytes)?;
    #[cfg(test)]
    LOG_BYTES_READ.with(|count| count.set(count.get() + bytes.len() as u64));
    if bytes.len() as u64 != len {
        bail!("artifact history is unavailable: the log changed during catch-up");
    }
    Ok(bytes)
}

/// Read what this pass must parse of one changed log: only the tail after
/// `tail`'s watermark when its guards hold, otherwise the whole log.
fn read_log(logs: &Dir, stat: &Stat, tail: Option<&Watermark>) -> Result<LogRead> {
    let mut file = logs.open(&stat.relative).context("opening the log")?;
    let size = stat.checkpoint.bytes;
    let mut read = None;
    if let Some(mark) = tail {
        let base = mark.parsed_bytes - mark.parsed_bytes.min(TAIL_WINDOW);
        let bytes = read_exact_range(&mut file, base, size - base)?;
        let window = &bytes[..(mark.parsed_bytes - base) as usize];
        let guarded = window.last() == Some(&b'\n')
            && Sha256::digest(window).as_slice() == mark.tail_sha256.as_slice();
        let previous = guarded
            .then(|| previous_line(window, base == 0))
            .flatten()
            .or_else(|| {
                // Only a legacy marker needs the line before it.
                (guarded && !needs_previous(&bytes[window.len()..])).then(String::new)
            });
        read = Some(match previous {
            Some(previous) => LogRead {
                bytes,
                base,
                start: mark.parsed_bytes,
                first_line: mark.parsed_lines,
                previous,
            },
            // Any doubt rereads the whole log: read the prefix the tail
            // read skipped, so the log is still read only once.
            None => {
                let mut whole = read_exact_range(&mut file, 0, base)?;
                whole.extend_from_slice(&bytes);
                LogRead {
                    bytes: whole,
                    base: 0,
                    start: 0,
                    first_line: 0,
                    previous: String::new(),
                }
            }
        });
    }
    let read = match read {
        Some(read) => read,
        None => LogRead {
            bytes: read_exact_range(&mut file, 0, size)?,
            base: 0,
            start: 0,
            first_line: 0,
            previous: String::new(),
        },
    };
    // Another process appending meanwhile (a concurrent capture) is normal
    // and only grows the log past what was read. A log that shrank is not
    // an append, so what was read may be gone.
    if LogCheckpoint::read(&file.metadata()?)?.bytes < size {
        bail!("artifact history is unavailable: the log changed during catch-up");
    }
    Ok(read)
}

/// The complete line ending at `window`'s final newline, as `str::lines`
/// yields it, if the window holds all of it.
fn previous_line(window: &[u8], at_file_start: bool) -> Option<String> {
    let body = &window[..window.len() - 1];
    let line = match body.iter().rposition(|byte| *byte == b'\n') {
        Some(newline) => &body[newline + 1..],
        None if at_file_start => body,
        None => return None,
    };
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    Some(String::from_utf8_lossy(line).into_owned())
}

/// Whether the tail's first line could be a legacy (M1) marker, which
/// takes its title and URL from the line before it.
fn needs_previous(tail: &[u8]) -> bool {
    let first = tail.split(|byte| *byte == b'\n').next().unwrap_or_default();
    let first = String::from_utf8_lossy(first);
    first.starts_with(MARKER_PREFIX) && !first.contains(" kind=")
}

/// Parse what `read` holds, hand its events to the store, upsert its
/// overflow markers and advance the log's watermark.
fn ingest_log(
    tx: &Transaction<'_>,
    target: Target,
    stat: &Stat,
    read: LogRead,
    ingest: &mut Ingest<'_>,
) -> Result<()> {
    let LogRead {
        bytes,
        base,
        start,
        first_line,
        previous,
    } = read;
    let offset = |at: u64| (at - base) as usize;
    let parsed = &bytes[offset(start)..];
    // Markers are ASCII. A torn multibyte prose line (capture tolerates one
    // and keeps appending) must not drop the whole day. Every line starts
    // after a newline, which never belongs to a multibyte sequence, so a
    // tail decodes line by line exactly as the whole log would.
    let text = String::from_utf8_lossy(parsed);
    // A tail is only ever parsed after a header was seen.
    let is_log = start > 0 || text.starts_with(LOG_HEADER);
    let (parsed_bytes, parsed_lines, tail_sha256) = if is_log {
        let end = parsed
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(start, |newline| start + newline as u64 + 1);
        let lines = parsed[..offset(end) - offset(start)]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count() as u64;
        // The new window is inside what was read: it starts no earlier
        // than the old one, which `read_log` read from.
        let window = &bytes[offset(end - end.min(TAIL_WINDOW))..offset(end)];
        let mut previous = previous.as_str();
        for (number, line) in text.lines().enumerate() {
            let number = first_line + number as u64;
            if let Some(event) = parse_marker_event(line, previous) {
                ingest(tx, event, (stat.relative.clone(), number))?;
            }
            if let Some((id, marker)) = parse_overflow_marker(line) {
                upsert_gap(tx, target, &id, &marker, stat, number)?;
            }
            previous = line;
        }
        (end, first_line + lines, Sha256::digest(window).to_vec())
    } else {
        // Not a log: checkpointed so it is not read again until it changes,
        // and then read whole, since nothing in it was parsed.
        (0, 0, Sha256::digest([]).to_vec())
    };
    tx.execute(
        &format!(
            "INSERT INTO {}(path, purpose, bytes, modified_ns, parsed_bytes, parsed_lines,
                            tail_sha256)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(path, purpose) DO UPDATE SET
           bytes = excluded.bytes, modified_ns = excluded.modified_ns,
           parsed_bytes = excluded.parsed_bytes, parsed_lines = excluded.parsed_lines,
           tail_sha256 = excluded.tail_sha256",
            target.logs()
        ),
        params![
            stat.key,
            EVENTS,
            to_i64(stat.checkpoint.bytes)?,
            stat.modified_ns,
            to_i64(parsed_bytes)?,
            to_i64(parsed_lines)?,
            tail_sha256,
        ],
    )?;
    Ok(())
}

/// Store an overflow marker. Retries reuse the marker ID; the copy at the
/// earliest log position wins, whichever pass found it first.
fn upsert_gap(
    tx: &Transaction<'_>,
    target: Target,
    id: &str,
    marker: &OverflowMarker,
    stat: &Stat,
    line: u64,
) -> Result<()> {
    let existing: Option<(Option<String>, Option<i64>)> = tx
        .query_row(
            &format!(
                "SELECT log_path, log_line FROM {} WHERE gap_id = ?1",
                target.gaps()
            ),
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((Some(path), Some(indexed))) = existing
        && (Path::new(&path), to_u64(indexed)?) <= (stat.relative.as_path(), line)
    {
        return Ok(());
    }
    // `parse_overflow_marker` has validated both instants. Endpoint dates
    // keep the offsets recorded when activity froze; travel can reverse
    // them, so the range is their enclosing one.
    let first = OffsetDateTime::parse(&marker.first_occurred_at, &Rfc3339)?.date();
    let last = OffsetDateTime::parse(&marker.last_occurred_at, &Rfc3339)?.date();
    tx.execute(
        &format!(
            "INSERT INTO {}(gap_id, marker, log_path, log_line, start_date, end_date)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(gap_id) DO UPDATE SET
           marker = excluded.marker, log_path = excluded.log_path,
           log_line = excluded.log_line, start_date = excluded.start_date,
           end_date = excluded.end_date",
            target.gaps()
        ),
        params![
            id,
            serde_json::to_string(marker)?,
            stat.key,
            to_i64(line)?,
            first.min(last).to_string(),
            first.max(last).to_string(),
        ],
    )?;
    Ok(())
}

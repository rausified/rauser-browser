//! The reads (plan T11): the card, Recent, the worklog, why, summaries and
//! URL evidence, each a bounded query over `index.db`'s committed rows
//! inside the caller's `read_txn`. Every answer is byte-identical to the
//! one a full derive of the same events gave; the T2 goldens pin it.

use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OptionalExtension, Transaction};

use super::*;

/// The wire view of a summary event. `summarized_at` is the event's own
/// timestamp; one longer than the wire's 32 bytes (fractional seconds in a
/// hand-edited log) is shown at whole seconds in the same offset. A
/// timestamp that still cannot fit is not shown as a summary at all, so
/// `summarized_at` is always the RFC 3339 instant the schema promises.
pub(super) fn summary_view(event: &SummaryEvent) -> Option<SummaryView> {
    let summarized_at = if event.occurred_at.len() <= 32 {
        event.occurred_at.clone()
    } else {
        parsed_timestamp(&event.occurred_at)
            .and_then(|at| at.replace_nanosecond(0).ok())
            .and_then(|at| at.format(&Rfc3339).ok())
            .filter(|value| value.len() <= 32)?
    };
    Some(SummaryView {
        summarized_at,
        document_type: event.document_type.clone(),
        category: event.category.clone(),
        importance: event.importance,
        summary_path: event.summary_path.clone(),
        harness: event.harness,
    })
}

/// The most artifacts an artifact view lists as contained in it.
pub(super) const MAX_CONTAINED: usize = 20;

pub(super) const MAX_INCOMING_TRAILS: usize = 5;

/// The most artifact types Recent offers as filters.
const MAX_TYPES: usize = 64;

/// The most overflow notices one worklog reports.
const MAX_WORKLOG_GAPS: usize = 32;

/// Register `brauser_fold(text)`, Rust's `to_lowercase`, on a reading
/// connection, so a title match folds non-ASCII text exactly as before
/// (SQLite's own `lower()` folds ASCII only).
pub(super) fn register(conn: &Connection) -> Result<()> {
    conn.create_scalar_function(
        "brauser_fold",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            Ok(ctx
                .get::<Option<String>>(0)?
                .map(|text| text.to_lowercase()))
        },
    )
    .context("registering brauser_fold")?;
    Ok(())
}

/// A stored count as the views' u64, never negative.
fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn sql_count(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn event_body(body: &str) -> Result<CachedEvent> {
    serde_json::from_str(body).context("artifact history is unavailable: unreadable indexed event")
}

/// An artifact's own summary events in `order_key` order, stably
/// sorted by instant, as a derived row keeps them: the newest is last, and
/// two in the same second keep their log order. `date` limits them to one
/// local day.
fn summaries(
    tx: &Transaction<'_>,
    artifact_id: &str,
    date: Option<&str>,
) -> Result<Vec<SummaryEvent>> {
    let mut statement = tx.prepare_cached(
        "SELECT body FROM brauser_events
         WHERE artifact_id = ?1 AND kind = 'summary' AND (?2 IS NULL OR local_date = ?2)
         ORDER BY order_key",
    )?;
    let bodies = statement
        .query_map(rusqlite::params![artifact_id, date], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut summaries = Vec::with_capacity(bodies.len());
    for body in bodies {
        if let EventData::Summary(summary) = event_body(&body)?.data {
            summaries.push(summary);
        }
    }
    summaries.sort_by(|a, b| timestamp_order(&a.occurred_at, &b.occurred_at));
    Ok(summaries)
}

/// The newest summary that renders as a view; a broken one hides no
/// earlier one.
fn latest_summary_view(summaries: &[SummaryEvent]) -> Option<SummaryView> {
    summaries.iter().rev().find_map(summary_view)
}

/// The artifact's latest summary (by instant, then log order, as its file's
/// Summary line picks it) that renders as a view.
pub(super) fn summary_for(tx: &Transaction<'_>, artifact_id: &str) -> Result<Option<SummaryView>> {
    Ok(latest_summary_view(&summaries(tx, artifact_id, None)?))
}

/// The card for one artifact and its latest summary. A summary always
/// derives a row, so a missing row means no summary either.
pub(super) fn card(
    tx: &Transaction<'_>,
    artifact_id: &str,
) -> Result<(Option<ArtifactView>, Option<SummaryView>)> {
    let row = tx
        .prepare_cached(
            "SELECT artifact_type, title, url, first_seen, last_seen, focused_secs,
               background_secs, opens, days_opened, last_mode, why
             FROM artifacts WHERE artifact_id = ?1",
        )?
        .query_row([artifact_id], |row| {
            Ok(ArtifactView {
                artifact_id: artifact_id.to_owned(),
                artifact_type: row.get(0)?,
                title: row.get(1)?,
                url: row.get(2)?,
                first_seen: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                last_seen: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                focused_seconds: saturating_u32(count(row.get(5)?)),
                background_seconds: saturating_u32(count(row.get(6)?)),
                opens: saturating_u32(count(row.get(7)?)),
                days_opened: saturating_u32(count(row.get(8)?)),
                last_mode: row.get(9)?,
                why: row.get(10)?,
                linked_count: 0,
                within: None,
                contained: Vec::new(),
                incoming_trails: Vec::new(),
            })
        })
        .optional()?;
    let Some(mut view) = row else {
        return Ok((None, None));
    };
    view.linked_count = saturating_u32(linked_count(tx, artifact_id)?);
    // A child normally has one container; if an adapter change left more
    // than one, the most-used edge wins, then the lowest ID.
    view.within = tx
        .prepare_cached(
            "SELECT a.artifact_id, a.artifact_type, a.title, a.url
             FROM edges e JOIN artifacts a ON a.artifact_id = e.target_artifact_id
             WHERE e.source_artifact_id = ?1 AND e.kind = 'within'
               AND e.target_artifact_id != ?1
             ORDER BY e.weight DESC, e.target_artifact_id
             LIMIT 1",
        )?
        .query_row([artifact_id], reference)
        .optional()?;
    view.contained = tx
        .prepare_cached(
            "SELECT a.artifact_id, a.artifact_type, a.title, a.url, a.focused_secs
             FROM edges e JOIN artifacts a ON a.artifact_id = e.source_artifact_id
             WHERE e.target_artifact_id = ?1 AND e.kind = 'within'
               AND e.source_artifact_id != ?1
             ORDER BY a.focused_secs DESC, a.artifact_id
             LIMIT ?2",
        )?
        .query_map(
            rusqlite::params![artifact_id, MAX_CONTAINED as i64],
            |row| {
                Ok(ContainedArtifactView {
                    artifact_id: row.get(0)?,
                    artifact_type: row.get(1)?,
                    title: row.get(2)?,
                    url: row.get(3)?,
                    focused_seconds: saturating_u32(count(row.get(4)?)),
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    view.incoming_trails = incoming_trails(tx, artifact_id)?;
    Ok((Some(view), summary_for(tx, artifact_id)?))
}

fn reference(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRefView> {
    Ok(ArtifactRefView {
        artifact_id: row.get(0)?,
        artifact_type: row.get(1)?,
        title: row.get(2)?,
        url: row.get(3)?,
    })
}

/// Distinct artifacts with a row linked to this one in either direction.
fn linked_count(tx: &Transaction<'_>, artifact_id: &str) -> Result<u64> {
    let linked: i64 = tx
        .prepare_cached(
            "SELECT COUNT(DISTINCT other) FROM (
               SELECT target_artifact_id AS other FROM edges WHERE source_artifact_id = ?1
               UNION ALL
               SELECT source_artifact_id FROM edges WHERE target_artifact_id = ?1)
             WHERE other != ?1 AND other IN (SELECT artifact_id FROM artifacts)",
        )?
        .query_row([artifact_id], |row| row.get(0))?;
    Ok(count(linked))
}

/// The newest valid trail transitions into this artifact from web pages.
fn incoming_trails(tx: &Transaction<'_>, artifact_id: &str) -> Result<Vec<IncomingTrailView>> {
    let mut statement = tx.prepare_cached(
        "SELECT e.kind, e.last_seen, a.artifact_id, a.artifact_type, a.title, a.url
         FROM edges e JOIN artifacts a ON a.artifact_id = e.source_artifact_id
         WHERE e.target_artifact_id = ?1 AND e.source_artifact_id != ?1
           AND e.kind NOT IN ('mentions', 'within', 'forward_back')
         ORDER BY e.last_seen DESC, e.source_artifact_id, e.kind",
    )?;
    let mut rows = statement.query([artifact_id])?;
    let mut incoming = Vec::new();
    while incoming.len() < MAX_INCOMING_TRAILS
        && let Some(row) = rows.next()?
    {
        let transition: String = row.get(0)?;
        let source = ArtifactRefView {
            artifact_id: row.get(2)?,
            artifact_type: row.get(3)?,
            title: row.get(4)?,
            url: row.get(5)?,
        };
        // Back/forward has no attributed same-tab source, so it was never
        // selected: a historical paired source is not apparent provenance.
        if !trail_transition_is_valid(&transition)
            || !Url::parse(&source.url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
        {
            continue;
        }
        incoming.push(IncomingTrailView {
            source,
            transition,
            last_seen: row.get(1)?,
        });
    }
    Ok(incoming)
}

/// The latest "why I'm here" text recorded for an artifact, with or without
/// a derived row.
pub(super) fn latest_why(tx: &Transaction<'_>, artifact_id: &str) -> Result<Option<String>> {
    let body: Option<String> = tx
        .prepare_cached(
            "SELECT body FROM brauser_events WHERE artifact_id = ?1 AND kind = 'why'
             ORDER BY order_key DESC LIMIT 1",
        )?
        .query_row([artifact_id], |row| row.get(0))
        .optional()?;
    Ok(match body.map(|body| event_body(&body)).transpose()? {
        Some(CachedEvent {
            data: EventData::Why(why),
            ..
        }) => Some(why.text),
        _ => None,
    })
}

/// Every URL recorded for an artifact's own events, trail sources aside.
pub(super) fn url_evidence(tx: &Transaction<'_>, artifact_id: &str) -> Result<Vec<StoredUrl>> {
    let mut statement = tx.prepare_cached(
        "SELECT url, stripped_params FROM url_provenance
         WHERE artifact_id = ?1 AND substr(role, 1, 6) != 'trail:'",
    )?;
    let rows = statement
        .query_map([artifact_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut urls = BTreeSet::new();
    for (url, stripped) in rows {
        let stripped_params = stripped
            .map(|names| serde_json::from_str(&names))
            .transpose()
            .context("artifact history is unavailable: unreadable URL provenance")?;
        urls.insert(StoredUrl {
            url,
            stripped_params,
        });
    }
    Ok(urls.into_iter().collect())
}

/// One row per artifact and local day, newest day first and then by that
/// day's measured focus, with the artifact types to filter by and whether
/// more rows matched than `limit`.
pub(super) fn recent(
    tx: &Transaction<'_>,
    artifact_type: Option<&str>,
    title_query: Option<&str>,
    limit: usize,
) -> Result<(Vec<ArtifactListItem>, Vec<String>, bool)> {
    let types = tx
        .prepare_cached("SELECT DISTINCT artifact_type FROM artifacts ORDER BY 1 LIMIT ?1")?
        .query_map([MAX_TYPES as i64], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let query = title_query
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    let mut statement = tx.prepare_cached(
        "SELECT d.local_date, d.artifact_id, a.artifact_type, a.title, a.url, d.focused_secs,
           d.background_secs, d.opens, a.why, d.summaries
         FROM brauser_artifact_days d JOIN artifacts a ON a.artifact_id = d.artifact_id
         WHERE (?1 IS NULL OR a.artifact_type = ?1)
           AND (?2 IS NULL OR instr(brauser_fold(a.title), ?2) > 0)
         ORDER BY d.local_date DESC, min(d.focused_secs, 4294967295) DESC, a.title, d.artifact_id
         LIMIT ?3",
    )?;
    let rows = statement
        .query_map(
            rusqlite::params![
                artifact_type,
                query,
                sql_count(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
            ],
            |row| {
                Ok((
                    ArtifactListItem {
                        local_date: row.get(0)?,
                        artifact_id: row.get(1)?,
                        artifact_type: row.get(2)?,
                        title: row.get(3)?,
                        url: row.get(4)?,
                        focused_seconds: saturating_u32(count(row.get(5)?)),
                        background_seconds: saturating_u32(count(row.get(6)?)),
                        opens: saturating_u32(count(row.get(7)?)),
                        why: row.get(8)?,
                        summary: None,
                    },
                    row.get::<_, i64>(9)? > 0,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let truncated = rows.len() > limit;
    let mut items = Vec::with_capacity(rows.len().min(limit));
    for (mut item, summarized) in rows.into_iter().take(limit) {
        // The day's latest summary that renders.
        if summarized {
            item.summary =
                latest_summary_view(&summaries(tx, &item.artifact_id, Some(&item.local_date))?);
        }
        items.push(item);
    }
    Ok((items, types, truncated))
}

#[derive(Default)]
struct WorklogRow {
    artifact: Option<(String, String, Option<String>)>,
    focused: u64,
    background: u64,
    opens: u64,
    /// Focus and opens from the artifact's own visits, without those of
    /// artifacts within it, for the range's totals.
    own_focused: u64,
    own_opens: u64,
    /// The latest summary (by `occurred_at`) among this artifact's visits
    /// within the worklog's date range, if any.
    summary: Option<SummaryEvent>,
}

/// A measured, agent-free summary of an inclusive range of local dates
/// (callers bound it to at most a week), the number of artifacts in it, and
/// the overflow notices that intersect it.
pub(super) fn worklog(
    tx: &Transaction<'_>,
    start: Date,
    end: Date,
) -> Result<(String, usize, Vec<OverflowMarker>, bool)> {
    let (first, last) = (start.to_string(), end.to_string());
    let (gaps, gaps_truncated) = worklog_gaps(tx, start, end)?;
    let mut rows = BTreeMap::<String, WorklogRow>::new();
    {
        let mut statement = tx.prepare_cached(
            "SELECT d.artifact_id, d.focused_secs, d.background_secs, d.opens,
               d.own_focused_secs, d.own_opens, a.artifact_type, a.title, a.why
             FROM brauser_artifact_days d LEFT JOIN artifacts a ON a.artifact_id = d.artifact_id
             WHERE d.local_date BETWEEN ?1 AND ?2",
        )?;
        let mut days = statement.query([&first, &last])?;
        while let Some(day) = days.next()? {
            let row = rows.entry(day.get(0)?).or_default();
            row.focused = row.focused.saturating_add(count(day.get(1)?));
            row.background = row.background.saturating_add(count(day.get(2)?));
            row.opens = row.opens.saturating_add(count(day.get(3)?));
            row.own_focused = row.own_focused.saturating_add(count(day.get(4)?));
            row.own_opens = row.own_opens.saturating_add(count(day.get(5)?));
            if let Some(artifact_type) = day.get::<_, Option<String>>(6)? {
                row.artifact = Some((artifact_type, day.get(7)?, day.get(8)?));
            }
        }
    }
    {
        let mut statement = tx.prepare_cached(
            "SELECT artifact_id, body FROM brauser_events
             WHERE kind = 'summary' AND local_date BETWEEN ?1 AND ?2
             ORDER BY order_key",
        )?;
        let found = statement
            .query_map([&first, &last], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut by_artifact = BTreeMap::<String, Vec<SummaryEvent>>::new();
        for (artifact_id, body) in found {
            if let EventData::Summary(summary) = event_body(&body)?.data {
                by_artifact.entry(artifact_id).or_default().push(summary);
            }
        }
        for (artifact_id, mut summaries) in by_artifact {
            summaries.sort_by(|a, b| timestamp_order(&a.occurred_at, &b.occurred_at));
            let Some(row) = rows.get_mut(&artifact_id) else {
                continue;
            };
            for summary in summaries {
                if row.summary.as_ref().is_none_or(|current| {
                    timestamp_order(&summary.occurred_at, &current.occurred_at).is_ge()
                }) {
                    row.summary = Some(summary);
                }
            }
        }
    }
    let (text, count) = worklog_text(rows);
    Ok((text, count, gaps, gaps_truncated))
}

fn worklog_text(rows: BTreeMap<String, WorklogRow>) -> (String, usize) {
    // A container is still listed with its rolled-up measures, but the
    // range's totals count each visit once, by its own artifact. A
    // summary is not measured activity: it is counted on its own and
    // never as an open.
    let opened = rows.values().filter(|row| row.own_opens > 0).count();
    let carried = rows
        .values()
        .filter(|row| row.own_opens == 0 && row.own_focused > 0)
        .count();
    let summarized = rows.values().filter(|row| row.summary.is_some()).count();
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let mut text = if summarized > 0 {
        format!(
            "Opened {opened} artifact{}, focused on {carried} left open from an earlier day, and summarized {summarized} page{}.",
            plural(opened),
            plural(summarized)
        )
    } else {
        format!(
            "Opened {opened} artifact{}, and focused on {carried} left open from an earlier day.",
            plural(opened)
        )
    };
    let mut ordered = rows.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        right
            .1
            .focused
            .cmp(&left.1.focused)
            .then(right.1.opens.cmp(&left.1.opens))
            .then(left.0.cmp(&right.0))
    });
    let mut shown = 0usize;
    for (id, row) in &ordered {
        let Some((artifact_type, title, why)) = &row.artifact else {
            continue;
        };
        let mut line = if let Some(summary) = &row.summary {
            let mut line = format!(
                "Summarized: {} — {}, {} importance",
                plain_text(&summary.title),
                plain_text(&summary.document_type),
                crate::summary::importance_str(summary.importance)
            );
            // A future adapter-attached summary (spec §7.2's "the
            // adapter does recognize the page" branch) can land on an
            // artifact that also has real measured activity; keep both
            // instead of silently dropping the measures.
            if row.focused > 0 || row.background > 0 || row.opens > 0 {
                line.push_str(&format!(
                    " — {}",
                    measures(row.focused, row.background, row.opens)
                ));
            }
            line
        } else {
            let mut line = format!("{} {}: {}", artifact_type, id, plain_text(title));
            line.push_str(&format!(
                " — {}",
                measures(row.focused, row.background, row.opens)
            ));
            line
        };
        if let Some(why) = why {
            line.push_str(&format!(". Why: {}", plain_text(why)));
        }
        if text.len().saturating_add(line.len()).saturating_add(1) > MAX_WORKLOG_BYTES {
            break;
        }
        text.push('\n');
        text.push_str(&line);
        shown += 1;
    }
    if shown < ordered.len() {
        text.push_str(&format!(
            "\n… {} more artifacts omitted.",
            ordered.len() - shown
        ));
    }
    (text, ordered.len())
}

/// The overflow notices whose span intersects the range, oldest first, at
/// most `MAX_WORKLOG_GAPS`, and whether more did.
fn worklog_gaps(
    tx: &Transaction<'_>,
    start: Date,
    end: Date,
) -> Result<(Vec<OverflowMarker>, bool)> {
    let mut statement = tx.prepare_cached(
        "SELECT gap_id, marker FROM brauser_gaps WHERE start_date <= ?2 AND end_date >= ?1",
    )?;
    let found = statement
        .query_map([start.to_string(), end.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut gaps = Vec::with_capacity(found.len());
    for (id, marker) in found {
        let marker: OverflowMarker = serde_json::from_str(&marker)
            .context("artifact history is unavailable: unreadable overflow notice")?;
        let Ok(first) = OffsetDateTime::parse(&marker.first_occurred_at, &Rfc3339) else {
            continue;
        };
        let Ok(last) = OffsetDateTime::parse(&marker.last_occurred_at, &Rfc3339) else {
            continue;
        };
        // Endpoint dates retain the offsets recorded when activity froze.
        // Travel can reverse the local dates, so use their enclosing range.
        if first.date().min(last.date()) <= end && first.date().max(last.date()) >= start {
            gaps.push((first, last, id, marker));
        }
    }
    gaps.sort_by(|left, right| (&left.0, &left.1, &left.2).cmp(&(&right.0, &right.1, &right.2)));
    let truncated = gaps.len() > MAX_WORKLOG_GAPS;
    gaps.truncate(MAX_WORKLOG_GAPS);
    Ok((
        gaps.into_iter().map(|(_, _, _, marker)| marker).collect(),
        truncated,
    ))
}

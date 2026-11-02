//! The SQL store (plan T8): `brauser_events` is the cache of every indexed
//! event body, and every derived row is produced by the shared [`Fold`].
//!
//! - [`ingest`] adds one event under the conflict rules: the first-indexed
//!   body wins, the same body at an earlier log position moves there, and a
//!   different body for an indexed ID is a conflict that changes nothing.
//! - [`rederive_groups`] re-derives each affected group (one artifact's own
//!   events plus, for a container, its children's) and replaces only the
//!   rows that group owns, so no delivery runs a full derive.
//! - [`mark_dirty`] gives each mark a fresh generation, so a later
//!   generation-conditional clear never loses a newer mark.
//!
//! Row ownership partitions every derived table by group: an artifact's
//! row, own visits, URL provenance, days and state belong to its group; a
//! `within` edge belongs to its target (the container, derived from the
//! children); a `mentions` edge to its source; every trail edge to its
//! target. Each is complete in its owner's fold.
//!
//! Until the JSON cache is removed (plan T15), [`sync_to_state`] keeps this
//! store a mirror of `index-state.json` after paths that still change that
//! cache directly (catch-up, rebuilds, regeneration). It touches only the
//! groups whose events differ.
//!
//! Under `cfg(test)` every [`write`] transaction ends with the oracle:
//! the derived tables must equal the same writer's rows for a full derive of
//! `brauser_events`.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::*;

/// The append fast path (plan T14). Until it lands, every affected group
/// takes the exact per-group re-derive.
pub(super) const FAST_PATH: bool = false;

/// Opens the store for a write, creating it (or replacing a legacy file)
/// as needed.
pub(super) fn open(cache_dir: &Path) -> Result<Connection> {
    db::open(cache_dir, db::Mode::ReadWrite)?
        .context("artifact history is unavailable: index.db could not be opened")
}

/// Runs `f` in one `write_txn`. In tests the derived tables are then
/// checked against a full derive of `brauser_events` before the commit.
pub(super) fn write<T>(
    conn: &mut Connection,
    adapters: Option<&Document>,
    f: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    db::write_txn(conn, |tx| {
        let value = f(tx)?;
        #[cfg(test)]
        oracle::check(tx, adapters)?;
        #[cfg(not(test))]
        let _ = adapters;
        Ok(value)
    })
}

/// What [`ingest`] did with one event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ingest {
    /// A new event ID; its body is stored.
    Inserted,
    /// The indexed body (same data) seen again where it already sorts.
    Same,
    /// The indexed body seen at an earlier log position, which it now takes.
    Moved,
    /// A different body for an indexed ID; the first-indexed one is kept.
    Conflict,
}

/// One stored event, as the fold and the row writers need it.
pub(super) struct StoredEvent {
    pub(super) artifact_id: String,
    pub(super) container_id: Option<String>,
    pub(super) order_key: Vec<u8>,
    pub(super) event: CachedEvent,
}

fn event_kind(event: &CachedEvent) -> &'static str {
    match event.data {
        EventData::Activity(_) => "activity",
        EventData::Why(_) => "why",
        EventData::Legacy(_) => "legacy",
        EventData::Summary(_) => "summary",
    }
}

/// The container a visit rolls up into under the current adapters. Only
/// activity and M1 visits roll up.
fn container_of(containers: &mut Containers<'_>, event: &CachedEvent) -> Option<String> {
    let (url, artifact_id) = match &event.data {
        EventData::Activity(event) => (&event.url, &event.artifact_id),
        EventData::Legacy(event) => (&event.url, &event.artifact_id),
        EventData::Why(_) | EventData::Summary(_) => return None,
    };
    containers
        .of(url, artifact_id)
        .map(|(_, container, _)| container)
}

fn has_refs(event: &CachedEvent) -> bool {
    matches!(&event.data, EventData::Activity(event) if !event.refs.is_empty())
}

/// A log-dir-relative position as stored: `/`-separated components.
fn log_path_text(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn key_of(
    event: &CachedEvent,
    id: &str,
    position: Option<&(PathBuf, u64)>,
    sequence: Option<u64>,
) -> Vec<u8> {
    ord::order_key(
        event_time(event),
        position.map(|(path, line)| (path.as_path(), *line)),
        sequence,
        id,
    )
}

fn sql_int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn body_of(event: &CachedEvent) -> Result<String> {
    serde_json::to_string(event).context("serializing an indexed event")
}

fn parse_body(body: &str) -> Result<CachedEvent> {
    serde_json::from_str(body).context("artifact history is unavailable: unreadable indexed event")
}

pub(super) fn meta(tx: &Transaction<'_>, key: &str) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT value FROM brauser_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?)
}

/// Sets a meta key, writing nothing when it already has `value`.
pub(super) fn set_meta(tx: &Transaction<'_>, key: &str, value: &str) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO brauser_meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value WHERE value != excluded.value",
    )?
    .execute([key, value])?;
    Ok(())
}

fn meta_u64(tx: &Transaction<'_>, key: &str) -> Result<u64> {
    match meta(tx, key)? {
        Some(value) => value
            .parse()
            .with_context(|| format!("artifact history is unavailable: bad {key} in index.db")),
        None => Ok(0),
    }
}

/// Takes the next value of a meta counter.
fn next_counter(tx: &Transaction<'_>, key: &str) -> Result<u64> {
    let value = meta_u64(tx, key)?;
    set_meta(tx, key, &(value + 1).to_string())?;
    Ok(value)
}

fn insert_event(
    tx: &Transaction<'_>,
    containers: &mut Containers<'_>,
    event: &CachedEvent,
    position: Option<&(PathBuf, u64)>,
    sequence: Option<u64>,
) -> Result<StoredEvent> {
    let (id, artifact_id) = event_identity(event);
    let body = body_of(event)?;
    let container_id = container_of(containers, event);
    let order_key = key_of(event, &id, position, sequence);
    tx.prepare_cached(
        "INSERT INTO brauser_events(event_id, artifact_id, kind, order_key, local_date,
           log_path, log_line, sequence, container_id, has_refs, body, body_sha256)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(event_id) DO UPDATE SET artifact_id = excluded.artifact_id,
           kind = excluded.kind, order_key = excluded.order_key,
           local_date = excluded.local_date, log_path = excluded.log_path,
           log_line = excluded.log_line, sequence = excluded.sequence,
           container_id = excluded.container_id, has_refs = excluded.has_refs,
           body = excluded.body, body_sha256 = excluded.body_sha256",
    )?
    .execute(params![
        id,
        artifact_id,
        event_kind(event),
        order_key,
        event_time(event).date().to_string(),
        position.map(|(path, _)| log_path_text(path)),
        position.map(|(_, line)| sql_int(*line)),
        sequence.map(sql_int),
        container_id,
        has_refs(event),
        body,
        sha256(body.as_bytes()),
    ])?;
    Ok(StoredEvent {
        artifact_id,
        container_id,
        order_key,
        event: event.clone(),
    })
}

/// The groups an event belongs to: its artifact and its container.
fn groups_of(artifact_id: &str, container_id: Option<&str>) -> BTreeSet<String> {
    std::iter::once(artifact_id)
        .chain(container_id)
        .map(str::to_owned)
        .collect()
}

/// Add one event to `brauser_events`, assigning the next arrival sequence
/// to a new ID. Bodies with the same data are the same event; a different
/// body for an indexed ID is a conflict, and the first-indexed body (never
/// an untrusted later delivery) stays. Returns what happened and the groups
/// to re-derive (none for a conflict).
pub(super) fn ingest(
    tx: &Transaction<'_>,
    containers: &mut Containers<'_>,
    event: &CachedEvent,
    position: Option<&(PathBuf, u64)>,
) -> Result<(Ingest, BTreeSet<String>)> {
    let (id, _) = event_identity(event);
    let stored = tx
        .prepare_cached(
            "SELECT body, artifact_id, container_id, order_key, sequence
             FROM brauser_events WHERE event_id = ?1",
        )?
        .query_row([&id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .optional()?;
    let Some((body, artifact_id, container_id, order_key, sequence)) = stored else {
        let sequence = next_counter(tx, "next_sequence")?;
        let stored = insert_event(tx, containers, event, position, Some(sequence))?;
        let groups = groups_of(&stored.artifact_id, stored.container_id.as_deref());
        return Ok((Ingest::Inserted, groups));
    };
    if parse_body(&body)?.data != event.data {
        return Ok((Ingest::Conflict, BTreeSet::new()));
    }
    let groups = groups_of(&artifact_id, container_id.as_deref());
    // The same body at an earlier position (a log discovered later) takes
    // it, as a fresh rebuild would. Same instant, sequence and ID, so the
    // keys order exactly as the positions do.
    if let Some((path, line)) = position {
        let sequence = sequence.and_then(|value| u64::try_from(value).ok());
        let moved = key_of(event, &id, position, sequence);
        if moved < order_key {
            tx.prepare_cached(
                "UPDATE brauser_events SET order_key = ?2, log_path = ?3, log_line = ?4
                 WHERE event_id = ?1",
            )?
            .execute(params![id, moved, log_path_text(path), sql_int(*line)])?;
            return Ok((Ingest::Moved, groups));
        }
    }
    Ok((Ingest::Same, groups))
}

fn stored_event(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, Option<String>, Vec<u8>, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

/// A group's events in `ordered_events` order: the artifact's own, and the
/// children that roll up into it.
fn group_events(tx: &Transaction<'_>, group: &str) -> Result<Vec<StoredEvent>> {
    let mut statement = tx.prepare_cached(
        "SELECT artifact_id, container_id, order_key, body FROM brauser_events
           WHERE artifact_id = ?1
         UNION ALL
         SELECT artifact_id, container_id, order_key, body FROM brauser_events
           WHERE container_id = ?1 AND artifact_id != ?1
         ORDER BY 3",
    )?;
    let rows = statement
        .query_map([group], stored_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    #[cfg(test)]
    EVENTS_LOADED.with(|count| count.set(count.get() + rows.len() as u64));
    rows.into_iter()
        .map(|(artifact_id, container_id, order_key, body)| {
            Ok(StoredEvent {
                artifact_id,
                container_id,
                order_key,
                event: parse_body(&body)?,
            })
        })
        .collect()
}

/// Every stored event in `ordered_events` order.
#[cfg(test)]
fn all_events(tx: &Transaction<'_>) -> Result<Vec<StoredEvent>> {
    let mut statement = tx.prepare_cached(
        "SELECT artifact_id, container_id, order_key, body FROM brauser_events ORDER BY order_key",
    )?;
    let rows = statement
        .query_map([], stored_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(artifact_id, container_id, order_key, body)| {
            Ok(StoredEvent {
                artifact_id,
                container_id,
                order_key,
                event: parse_body(&body)?,
            })
        })
        .collect()
}

fn fold_events(adapters: Option<&Document>, events: &[StoredEvent]) -> Derived {
    let mut fold = Fold::new(adapters);
    for stored in events {
        fold.apply(&stored.event);
    }
    fold.finish()
}

#[cfg(test)]
thread_local! {
    /// Events read back from `brauser_events` by group re-derives.
    pub(super) static EVENTS_LOADED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Groups re-derived.
    pub(super) static GROUP_DERIVES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The rows of the derived tables, typed so a dump compares exactly.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Rows {
    artifacts: Vec<ArtifactSql>,
    visits: Vec<VisitSql>,
    edges: Vec<EdgeSql>,
    provenance: Vec<ProvenanceSql>,
    days: Vec<DaySql>,
    groups: Vec<GroupSql>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ArtifactSql {
    artifact_id: String,
    artifact_type: String,
    site: String,
    title: String,
    url: String,
    first_seen: String,
    last_seen: String,
    focused_secs: i64,
    background_secs: i64,
    opens: i64,
    days_opened: i64,
    last_mode: Option<String>,
    why: Option<String>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct VisitSql {
    event_id: String,
    artifact_id: String,
    occurred_at: String,
    local_date: String,
    focused_secs: i64,
    background_secs: i64,
    opens: i64,
    mode: Option<String>,
    title: String,
    url: String,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EdgeSql {
    source: String,
    target: String,
    kind: String,
    weight: i64,
    last_seen: String,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ProvenanceSql {
    event_id: String,
    artifact_id: String,
    role: String,
    url: String,
    stripped_params: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct DaySql {
    artifact_id: String,
    local_date: String,
    focused_secs: i64,
    background_secs: i64,
    opens: i64,
    own_focused_secs: i64,
    own_background_secs: i64,
    own_opens: i64,
    summaries: i64,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GroupSql {
    artifact_id: String,
    latest_key: Vec<u8>,
    latest_at: String,
    why_at: Option<String>,
    rolled_up: bool,
}

/// The group that owns an edge (see the module documentation).
fn edge_owner(edge: &EdgeRow) -> &str {
    match edge.kind.as_str() {
        "mentions" => &edge.source,
        _ => &edge.target,
    }
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

/// The rows `derived` (a fold over `events`) gives for the groups `owns`
/// accepts. A group re-derive accepts only its own group; the oracle
/// accepts every group of a full derive. Either way each row comes from
/// the fold of its owner's complete history, which is why the two agree.
fn rows_of(derived: &Derived, events: &[StoredEvent], owns: impl Fn(&str) -> bool) -> Rows {
    let mut rows = Rows::default();
    for row in derived.artifacts.values() {
        if !owns(&row.artifact_id) {
            continue;
        }
        rows.artifacts.push(ArtifactSql {
            artifact_id: row.artifact_id.clone(),
            artifact_type: row.artifact_type.clone(),
            site: row.site.clone(),
            title: row.title.clone(),
            url: row.url.clone(),
            first_seen: row.first_seen.clone(),
            last_seen: row.last_seen.clone(),
            focused_secs: sql_int(row.focused_secs),
            background_secs: sql_int(row.background_secs),
            opens: sql_int(row.opens),
            days_opened: sql_int(row.days.len() as u64),
            last_mode: row.last_mode.clone(),
            why: row.why.clone(),
        });
        let latest_key = events
            .iter()
            .filter(|event| {
                event.artifact_id == row.artifact_id
                    || event.container_id.as_deref() == Some(row.artifact_id.as_str())
            })
            .map(|event| &event.order_key)
            .max()
            .cloned()
            .unwrap_or_default();
        rows.groups.push(GroupSql {
            artifact_id: row.artifact_id.clone(),
            latest_key,
            latest_at: rfc3339(row.latest_at),
            why_at: derived.why_at.get(&row.artifact_id).copied().map(rfc3339),
            rolled_up: row.rolled_up,
        });
    }
    let mut days = BTreeMap::<(String, String), DaySql>::new();
    for visit in &derived.visits {
        if !owns(&visit.artifact_id) {
            continue;
        }
        let (focused, background, opens) = (
            sql_int(visit.focused_secs),
            sql_int(visit.background_secs),
            sql_int(visit.opens),
        );
        let day = days
            .entry((visit.artifact_id.clone(), visit.local_date.clone()))
            .or_insert_with(|| DaySql {
                artifact_id: visit.artifact_id.clone(),
                local_date: visit.local_date.clone(),
                ..DaySql::default()
            });
        day.focused_secs = day.focused_secs.saturating_add(focused);
        day.background_secs = day.background_secs.saturating_add(background);
        day.opens = day.opens.saturating_add(opens);
        if visit.summary.is_some() {
            day.summaries += 1;
        }
        if visit.contained {
            continue;
        }
        day.own_focused_secs = day.own_focused_secs.saturating_add(focused);
        day.own_background_secs = day.own_background_secs.saturating_add(background);
        day.own_opens = day.own_opens.saturating_add(opens);
        // One row per logged visit: a container's copy would count it
        // twice in any per-day total a reader takes.
        rows.visits.push(VisitSql {
            event_id: visit.event_id.clone(),
            artifact_id: visit.artifact_id.clone(),
            occurred_at: visit.occurred_at.clone(),
            local_date: visit.local_date.clone(),
            focused_secs: focused,
            background_secs: background,
            opens,
            mode: visit.mode.clone(),
            title: visit.title.clone(),
            url: visit.url.clone(),
        });
    }
    rows.days = days.into_values().collect();
    for edge in &derived.edges {
        if !owns(edge_owner(edge)) {
            continue;
        }
        rows.edges.push(EdgeSql {
            source: edge.source.clone(),
            target: edge.target.clone(),
            kind: edge.kind.clone(),
            weight: sql_int(edge.weight),
            last_seen: edge.last_seen.clone(),
        });
    }
    for stored in events {
        if !owns(&stored.artifact_id) {
            continue;
        }
        let (event_id, _) = event_identity(&stored.event);
        for (role, url) in stored.event.url_records() {
            rows.provenance.push(ProvenanceSql {
                event_id: event_id.clone(),
                artifact_id: stored.artifact_id.clone(),
                role,
                url: url.url,
                stripped_params: url
                    .stripped_params
                    .map(|names| serde_json::to_string(&names).expect("query names")),
            });
        }
    }
    rows
}

/// Replace every row `group` owns, keeping the regeneration columns of its
/// state row (`file_path`, `render_sha256`).
fn write_group(tx: &Transaction<'_>, group: &str, rows: &Rows) -> Result<()> {
    for sql in [
        "DELETE FROM artifacts WHERE artifact_id = ?1",
        "DELETE FROM visits WHERE artifact_id = ?1",
        "DELETE FROM url_provenance WHERE artifact_id = ?1",
        "DELETE FROM brauser_artifact_days WHERE artifact_id = ?1",
        "DELETE FROM edges WHERE kind = 'mentions' AND source_artifact_id = ?1",
        "DELETE FROM edges WHERE kind != 'mentions' AND target_artifact_id = ?1",
    ] {
        tx.prepare_cached(sql)?.execute([group])?;
    }
    for row in &rows.artifacts {
        tx.prepare_cached(
            "INSERT INTO artifacts(artifact_id, artifact_type, site, title, url, first_seen,
               last_seen, focused_secs, background_secs, opens, days_opened, last_mode, why)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?
        .execute(params![
            row.artifact_id,
            row.artifact_type,
            row.site,
            row.title,
            row.url,
            row.first_seen,
            row.last_seen,
            row.focused_secs,
            row.background_secs,
            row.opens,
            row.days_opened,
            row.last_mode,
            row.why,
        ])?;
    }
    for row in &rows.visits {
        tx.prepare_cached(
            "INSERT INTO visits(event_id, artifact_id, occurred_at, local_date, focused_secs,
               background_secs, opens, mode, title, url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?
        .execute(params![
            row.event_id,
            row.artifact_id,
            row.occurred_at,
            row.local_date,
            row.focused_secs,
            row.background_secs,
            row.opens,
            row.mode,
            row.title,
            row.url,
        ])?;
    }
    for row in &rows.edges {
        tx.prepare_cached(
            "INSERT INTO edges(source_artifact_id, target_artifact_id, kind, weight, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![
            row.source,
            row.target,
            row.kind,
            row.weight,
            row.last_seen
        ])?;
    }
    for row in &rows.provenance {
        tx.prepare_cached(
            "INSERT INTO url_provenance(event_id, artifact_id, role, url, stripped_params)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![
            row.event_id,
            row.artifact_id,
            row.role,
            row.url,
            row.stripped_params
        ])?;
    }
    for row in &rows.days {
        tx.prepare_cached(
            "INSERT INTO brauser_artifact_days(artifact_id, local_date, focused_secs,
               background_secs, opens, own_focused_secs, own_background_secs, own_opens,
               summaries)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?
        .execute(params![
            row.artifact_id,
            row.local_date,
            row.focused_secs,
            row.background_secs,
            row.opens,
            row.own_focused_secs,
            row.own_background_secs,
            row.own_opens,
            row.summaries,
        ])?;
    }
    match rows.groups.as_slice() {
        [] => {
            // A group with no row keeps a state row only while regeneration
            // still has a file of it to remove.
            tx.prepare_cached(
                "DELETE FROM brauser_group_state WHERE artifact_id = ?1 AND file_path IS NULL",
            )?
            .execute([group])?;
        }
        [row] => {
            tx.prepare_cached(
                "INSERT INTO brauser_group_state(artifact_id, latest_key, latest_at, why_at,
                   rolled_up)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(artifact_id) DO UPDATE SET latest_key = excluded.latest_key,
                   latest_at = excluded.latest_at, why_at = excluded.why_at,
                   rolled_up = excluded.rolled_up",
            )?
            .execute(params![
                row.artifact_id,
                row.latest_key,
                row.latest_at,
                row.why_at,
                row.rolled_up
            ])?;
        }
        _ => bail!("a group derived more than one row for itself"),
    }
    Ok(())
}

/// Re-derive each group from its stored events and replace the rows it
/// owns. This is the exact path for every case; the fast path (plan T14)
/// will only ever be a shortcut to the same rows.
pub(super) fn rederive_groups(
    tx: &Transaction<'_>,
    adapters: Option<&Document>,
    groups: &BTreeSet<String>,
) -> Result<()> {
    for group in groups {
        #[cfg(test)]
        GROUP_DERIVES.with(|count| count.set(count.get() + 1));
        let events = group_events(tx, group)?;
        let derived = fold_events(adapters, &events);
        write_group(
            tx,
            group,
            &rows_of(&derived, &events, |owner| owner == group),
        )?;
    }
    Ok(())
}

/// The full derived record of `group` (its own events and, for a
/// container, its children's), as regeneration renders it, or `None` when
/// the group derives no row.
pub(super) fn derived_row(
    tx: &Transaction<'_>,
    adapters: Option<&Document>,
    group: &str,
) -> Result<Option<ArtifactRow>> {
    let events = group_events(tx, group)?;
    Ok(fold_events(adapters, &events).artifacts.remove(group))
}

/// Mark each artifact dirty with a fresh generation, replacing an older
/// mark, so a clear conditional on the older generation keeps this one.
pub(super) fn mark_dirty<'a>(
    tx: &Transaction<'_>,
    ids: impl IntoIterator<Item = &'a String>,
) -> Result<()> {
    for id in ids {
        let generation = next_counter(tx, "next_generation")?;
        tx.prepare_cached(
            "INSERT INTO dirty_artifacts(artifact_id, generation) VALUES (?1, ?2)
             ON CONFLICT(artifact_id) DO UPDATE SET generation = excluded.generation",
        )?
        .execute(params![id, sql_int(generation)])?;
    }
    Ok(())
}

/// Make `dirty_artifacts` hold exactly `dirty`: marks not in it are cleared,
/// and a missing mark, or one in `remarked`, gets a fresh generation.
pub(super) fn sync_dirty(
    tx: &Transaction<'_>,
    dirty: &BTreeSet<String>,
    remarked: &BTreeSet<String>,
) -> Result<()> {
    let marked = tx
        .prepare_cached("SELECT artifact_id FROM dirty_artifacts")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    for id in marked.difference(dirty) {
        tx.prepare_cached("DELETE FROM dirty_artifacts WHERE artifact_id = ?1")?
            .execute([id])?;
    }
    mark_dirty(
        tx,
        dirty
            .iter()
            .filter(|id| !marked.contains(*id) || remarked.contains(*id)),
    )
}

/// Whether the store already holds `state`'s events: the cheap check a
/// delivery makes when the JSON cache needed no republishing.
pub(super) fn mirrors(tx: &Transaction<'_>, state: &IndexState, fingerprint: &str) -> Result<bool> {
    let count: i64 = tx.query_row("SELECT count(*) FROM brauser_events", [], |row| row.get(0))?;
    Ok(meta(tx, "adapters")?.as_deref() == Some(fingerprint)
        && meta_u64(tx, "next_sequence")? == state.next_sequence
        && usize::try_from(count).ok() == Some(state.events.len()))
}

/// Make `brauser_events` hold exactly `state`'s events, positions and
/// sequences, and re-derive the groups whose events changed (every group
/// after an adapter change, since containers and mentions follow the
/// document). Returns the re-derived groups.
pub(super) fn sync_to_state(
    tx: &Transaction<'_>,
    adapters: Option<&Document>,
    fingerprint: &str,
    state: &IndexState,
) -> Result<BTreeSet<String>> {
    let mut containers = Containers::new(adapters);
    let mut groups = BTreeSet::new();
    let readapt = meta(tx, "adapters")?.as_deref() != Some(fingerprint);
    let mut stored = tx
        .prepare_cached(
            "SELECT event_id, artifact_id, container_id, order_key, body_sha256
             FROM brauser_events",
        )?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ),
            ))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    for (id, event) in &state.events {
        let position = state.positions.get(id);
        let sequence = state.sequence.get(id).copied();
        let current = stored.remove(id);
        if let Some((artifact_id, container_id, order_key, sha)) = &current {
            if !readapt
                && *order_key == key_of(event, id, position, sequence)
                && *sha == sha256(body_of(event)?.as_bytes())
            {
                continue;
            }
            groups.extend(groups_of(artifact_id, container_id.as_deref()));
        }
        let inserted = insert_event(tx, &mut containers, event, position, sequence)?;
        groups.extend(groups_of(
            &inserted.artifact_id,
            inserted.container_id.as_deref(),
        ));
    }
    for (id, (artifact_id, container_id, _, _)) in stored {
        tx.prepare_cached("DELETE FROM brauser_events WHERE event_id = ?1")?
            .execute([&id])?;
        groups.extend(groups_of(&artifact_id, container_id.as_deref()));
    }
    if readapt {
        // Mentions resolve through the document too, and a container that
        // no longer derives must lose its rows: re-derive every group with
        // rows, as well as every group with events.
        let owners = tx
            .prepare_cached(
                "SELECT artifact_id FROM artifacts
                 UNION SELECT artifact_id FROM brauser_group_state
                 UNION SELECT source_artifact_id FROM edges WHERE kind = 'mentions'
                 UNION SELECT target_artifact_id FROM edges WHERE kind != 'mentions'
                 UNION SELECT artifact_id FROM visits
                 UNION SELECT artifact_id FROM url_provenance
                 UNION SELECT artifact_id FROM brauser_artifact_days",
            )?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        groups.extend(owners);
        set_meta(tx, "adapters", fingerprint)?;
    }
    set_meta(tx, "next_sequence", &state.next_sequence.to_string())?;
    rederive_groups(tx, adapters, &groups)?;
    Ok(groups)
}

#[cfg(test)]
pub(super) mod oracle {
    //! The permanent full-derive oracle (plan graft 2).

    use super::*;

    /// Every derived table, sorted, as stored.
    pub(in crate::index) fn dump(tx: &Transaction<'_>) -> Result<Rows> {
        let mut rows = Rows {
            artifacts: tx
                .prepare(
                    "SELECT artifact_id, artifact_type, site, title, url, first_seen, last_seen,
                       focused_secs, background_secs, opens, days_opened, last_mode, why
                     FROM artifacts",
                )?
                .query_map([], |row| {
                    Ok(ArtifactSql {
                        artifact_id: row.get(0)?,
                        artifact_type: row.get(1)?,
                        site: row.get(2)?,
                        title: row.get(3)?,
                        url: row.get(4)?,
                        first_seen: row.get(5)?,
                        last_seen: row.get(6)?,
                        focused_secs: row.get(7)?,
                        background_secs: row.get(8)?,
                        opens: row.get(9)?,
                        days_opened: row.get(10)?,
                        last_mode: row.get(11)?,
                        why: row.get(12)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?,
            visits: tx
                .prepare(
                    "SELECT event_id, artifact_id, occurred_at, local_date, focused_secs,
                       background_secs, opens, mode, title, url FROM visits",
                )?
                .query_map([], |row| {
                    Ok(VisitSql {
                        event_id: row.get(0)?,
                        artifact_id: row.get(1)?,
                        occurred_at: row.get(2)?,
                        local_date: row.get(3)?,
                        focused_secs: row.get(4)?,
                        background_secs: row.get(5)?,
                        opens: row.get(6)?,
                        mode: row.get(7)?,
                        title: row.get(8)?,
                        url: row.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?,
            edges: tx
                .prepare(
                    "SELECT source_artifact_id, target_artifact_id, kind, weight, last_seen
                     FROM edges",
                )?
                .query_map([], |row| {
                    Ok(EdgeSql {
                        source: row.get(0)?,
                        target: row.get(1)?,
                        kind: row.get(2)?,
                        weight: row.get(3)?,
                        last_seen: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?,
            provenance: tx
                .prepare(
                    "SELECT event_id, artifact_id, role, url, stripped_params FROM url_provenance",
                )?
                .query_map([], |row| {
                    Ok(ProvenanceSql {
                        event_id: row.get(0)?,
                        artifact_id: row.get(1)?,
                        role: row.get(2)?,
                        url: row.get(3)?,
                        stripped_params: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?,
            days: tx
                .prepare(
                    "SELECT artifact_id, local_date, focused_secs, background_secs, opens,
                       own_focused_secs, own_background_secs, own_opens, summaries
                     FROM brauser_artifact_days",
                )?
                .query_map([], |row| {
                    Ok(DaySql {
                        artifact_id: row.get(0)?,
                        local_date: row.get(1)?,
                        focused_secs: row.get(2)?,
                        background_secs: row.get(3)?,
                        opens: row.get(4)?,
                        own_focused_secs: row.get(5)?,
                        own_background_secs: row.get(6)?,
                        own_opens: row.get(7)?,
                        summaries: row.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?,
            // A state row a group keeps only for a file regeneration has yet
            // to remove derives nothing, so it is not compared.
            groups: tx
                .prepare(
                    "SELECT artifact_id, latest_key, latest_at, why_at, rolled_up
                     FROM brauser_group_state
                     WHERE file_path IS NULL
                        OR artifact_id IN (SELECT artifact_id FROM artifacts)",
                )?
                .query_map([], |row| {
                    Ok(GroupSql {
                        artifact_id: row.get(0)?,
                        latest_key: row.get(1)?,
                        latest_at: row.get(2)?,
                        why_at: row.get(3)?,
                        rolled_up: row.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?,
        };
        rows.sort();
        Ok(rows)
    }

    /// What the same writer gives for a full derive of `brauser_events`.
    pub(in crate::index) fn expected(
        tx: &Transaction<'_>,
        adapters: Option<&Document>,
    ) -> Result<Rows> {
        let events = all_events(tx)?;
        let mut rows = rows_of(&fold_events(adapters, &events), &events, |_| true);
        rows.sort();
        Ok(rows)
    }

    /// What the same writer gives for `derive` of an in-memory cache, which
    /// orders its events by `ordered_events` rather than by `order_key`.
    pub(in crate::index) fn state_rows(state: &IndexState, adapters: Option<&Document>) -> Rows {
        let mut containers = Containers::new(adapters);
        let events = state
            .events
            .iter()
            .map(|(id, event)| StoredEvent {
                artifact_id: event.artifact_id().to_owned(),
                container_id: container_of(&mut containers, event),
                order_key: key_of(
                    event,
                    id,
                    state.positions.get(id),
                    state.sequence.get(id).copied(),
                ),
                event: event.clone(),
            })
            .collect::<Vec<_>>();
        let mut rows = rows_of(&derive(state, adapters), &events, |_| true);
        rows.sort();
        rows
    }

    pub(in crate::index) fn check(tx: &Transaction<'_>, adapters: Option<&Document>) -> Result<()> {
        let (actual, expected) = (dump(tx)?, expected(tx, adapters)?);
        assert_eq!(
            actual, expected,
            "the derived tables differ from a full derive of brauser_events"
        );
        Ok(())
    }

    impl Rows {
        fn sort(&mut self) {
            self.artifacts.sort();
            self.visits.sort();
            self.edges.sort();
            self.provenance.sort();
            self.days.sort();
            self.groups.sort();
        }
    }
}

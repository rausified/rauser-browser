//! Direct activity only: a session is a breakdown of the existing worklog,
//! never an additional contribution to its range totals or container rollups.
use super::*;

const MAX_SESSIONS: usize = 64;
const MAX_ROWS: usize = 200;

pub(crate) struct FocusChoices {
    pub sessions: Vec<brauser_protocol::FocusSession>,
    pub sessions_truncated: bool,
    pub sessions_page: Option<brauser_protocol::WorklogChoicesPageInfo>,
    pub selected_session: Option<brauser_protocol::FocusSession>,
    pub name_changed: bool,
    pub choices: Vec<WorklogChoice>,
    pub truncated: bool,
    pub gaps: Vec<OverflowMarker>,
    pub gaps_truncated: bool,
}

/// Select before grouping: artifact-day choices have already combined every
/// session's measures and context and cannot safely be filtered afterwards.
pub(super) fn choices(
    tx: &rusqlite::Transaction<'_>,
    start: Date,
    end: Date,
    session_id: Option<&str>,
    paging: Option<(&[u8], &brauser_protocol::WorklogChoicesPageRequest)>,
) -> Result<FocusChoices> {
    if let Some((_, page)) = paging {
        anyhow::ensure!(valid_page_request(page), StaleWorklogPage);
    }
    let (gaps, gaps_truncated) = query::worklog_gaps(tx, start, end)?;
    let mut result = FocusChoices {
        sessions: Vec::new(),
        sessions_truncated: false,
        sessions_page: None,
        selected_session: None,
        name_changed: false,
        choices: Vec::new(),
        truncated: false,
        gaps,
        gaps_truncated,
    };
    let mut days = BTreeMap::<(String, String), query::ChoiceDay>::new();
    let mut seen = BTreeSet::new();
    let mut positions = BTreeMap::new();
    let offset = paging.map_or(0, |(_, page)| page.offset as usize);
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&(
        "focus-session-catalog-v1",
        paging.map(|(scope, _)| scope),
        start.to_string(),
        end.to_string(),
        &result.gaps,
        result.gaps_truncated,
    ))?);
    // `visits` holds direct activity only. Use its resolved ID, but the
    // selected event's own title/URL, never global artifact metadata.
    // Newest first also determines the bounded catalog's stable order.
    let mut statement = tx.prepare_cached(
        "SELECT v.local_date, v.artifact_id, e.body
         FROM visits v JOIN brauser_events e ON e.event_id = v.event_id
         WHERE v.local_date BETWEEN ?1 AND ?2 AND e.kind = 'activity'
         ORDER BY e.order_key DESC",
    )?;
    let mut rows = statement.query([start.to_string(), end.to_string()])?;
    while let Some(row) = rows.next()? {
        let local_date: String = row.get(0)?;
        let artifact_id: String = row.get(1)?;
        let body: String = row.get(2)?;
        // Frame every row, including activity outside the chosen session:
        // changing earlier pages must never shift a continuation silently.
        digest.update(serde_json::to_vec(&(&local_date, &artifact_id, &body))?);
        let EventData::Activity(event) = serde_json::from_str::<CachedEvent>(&body)?.data else {
            continue;
        };
        let Some(session) = event
            .focus_session
            .as_ref()
            .filter(|session| crate::capture::focus_session_is_valid(session))
        else {
            continue;
        };
        if let Some(&position) = positions.get(&session.id) {
            newest_name(&mut result.sessions[position], session);
        } else if seen.insert(session.id.clone())
            && seen.len() > offset
            && result.sessions.len() < MAX_SESSIONS
        {
            positions.insert(session.id.clone(), result.sessions.len());
            result.sessions.push(session.clone());
        }
        if session_id != Some(session.id.as_str()) {
            continue;
        }
        if let Some(known) = &mut result.selected_session {
            result.name_changed |= known.name != session.name;
            newest_name(known, session);
        } else {
            result.selected_session = Some(session.clone());
        }
        let day = days
            .entry((local_date, artifact_id))
            .or_insert_with(|| query::ChoiceDay {
                focused: 0,
                background: 0,
                opens: 0,
                summary: None,
                title: event.title.clone().unwrap_or_else(|| "Untitled".into()),
                url: event.url.clone(),
            });
        day.focused = day.focused.saturating_add(event.focused_seconds.into());
        day.background = day
            .background
            .saturating_add(event.background_seconds.into());
        day.opens = day.opens.saturating_add(event.opens.into());
    }
    result.sessions_truncated = seen.len() > offset.saturating_add(result.sessions.len());
    if let Some((_, page)) = paging {
        let snapshot = hex::encode(digest.finalize());
        anyhow::ensure!(
            offset <= seen.len()
                && page
                    .snapshot
                    .as_ref()
                    .is_none_or(|expected| expected == &snapshot),
            StaleWorklogPage
        );
        result.sessions_page = Some(brauser_protocol::WorklogChoicesPageInfo {
            snapshot,
            offset: page.offset,
            next_offset: result
                .sessions_truncated
                .then(|| u32::try_from(offset + result.sessions.len()))
                .transpose()?,
            omitted: false,
        });
    }
    (result.choices, result.truncated) = query::choice_rows(days)?;
    Ok(result)
}

fn newest_name(known: &mut brauser_protocol::FocusSession, fresh: &brauser_protocol::FocusSession) {
    if (fresh.revision, &fresh.name) > (known.revision, &known.name) {
        known.clone_from(fresh);
    }
}

#[derive(Default)]
pub(super) struct FocusGroups {
    groups: BTreeMap<String, Group>,
    rows: usize,
    omitted: bool,
}

struct Group {
    first_at: String,
    name: String,
    revision: u32,
    renamed: bool,
    artifacts: BTreeMap<String, Row>,
}

struct Row {
    artifact_type: String,
    title: String,
    focused: u64,
    background: u64,
    opens: u64,
}

impl FocusGroups {
    pub(super) fn add(&mut self, id: &str, event: &ActivityEvent) {
        let Some(session) = &event.focus_session else {
            return;
        };
        if !crate::capture::focus_session_is_valid(session) {
            return;
        }
        if !self.groups.contains_key(&session.id) && self.groups.len() == MAX_SESSIONS {
            self.omitted = true;
            return;
        }
        let group = self
            .groups
            .entry(session.id.clone())
            .or_insert_with(|| Group {
                first_at: event.occurred_at.clone(),
                name: session.name.clone(),
                revision: session.revision,
                renamed: false,
                artifacts: BTreeMap::new(),
            });
        if timestamp_order(&event.occurred_at, &group.first_at).is_lt() {
            group.first_at.clone_from(&event.occurred_at);
        }
        group.renamed |= group.name != session.name;
        if (session.revision, &session.name) > (group.revision, &group.name) {
            group.name.clone_from(&session.name);
            group.revision = session.revision;
        }
        if !group.artifacts.contains_key(id) {
            if self.rows == MAX_ROWS {
                self.omitted = true;
                return;
            }
            self.rows += 1;
        }
        let row = group.artifacts.entry(id.into()).or_insert_with(|| Row {
            artifact_type: event.artifact_type.clone(),
            title: String::new(),
            focused: 0,
            background: 0,
            opens: 0,
        });
        row.title = plain_text(event.title.as_deref().unwrap_or("Untitled"));
        row.focused = row.focused.saturating_add(event.focused_seconds.into());
        row.background = row
            .background
            .saturating_add(event.background_seconds.into());
        row.opens = row.opens.saturating_add(event.opens.into());
    }

    pub(super) fn append(self, text: &mut String) {
        if self.groups.is_empty() {
            return;
        }
        // Reserve space for an explicit omission notice, including when the
        // ordinary artifact list already filled the worklog's byte budget.
        const NOTICE: &str = "\n… Focus-session breakdown shortened.";
        let budget = MAX_WORKLOG_BYTES.saturating_sub(NOTICE.len());
        if text.len() > budget {
            const FULL_NOTICE: &str = "\n… Worklog shortened. Focus-session breakdown shortened.";
            let mut end = MAX_WORKLOG_BYTES.saturating_sub(FULL_NOTICE.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            // Remove a complete trailing line, preserving UTF-8 and making
            // both kinds of shortening explicit even when the main list
            // consumed the whole budget before session rendering began.
            text.truncate(text[..end].rfind('\n').unwrap_or(end));
            text.push_str(FULL_NOTICE);
            return;
        }
        let mut omitted = self.omitted;
        let mut append = |line: &str| {
            if text.len().saturating_add(line.len()) > budget {
                false
            } else {
                text.push_str(line);
                true
            }
        };
        if !append("\n\nFocus sessions (breakdown of activity above):") {
            omitted = true;
        } else {
            let mut groups: Vec<_> = self.groups.into_iter().collect();
            groups.sort_by(|(id_a, a), (id_b, b)| {
                timestamp_order(&a.first_at, &b.first_at).then(id_a.cmp(id_b))
            });
            'groups: for (_, group) in groups {
                let changed = if group.renamed {
                    " (name changed in this range)"
                } else {
                    ""
                };
                if !append(&format!("\nFocus: {}{changed}", plain_text(&group.name))) {
                    omitted = true;
                    break;
                }
                for (id, row) in group.artifacts {
                    if !append(&format!(
                        "\n  {} {}: {} — {}",
                        row.artifact_type,
                        id,
                        row.title,
                        measures(row.focused, row.background, row.opens)
                    )) {
                        omitted = true;
                        break 'groups;
                    }
                }
            }
        }
        if omitted && text.len().saturating_add(NOTICE.len()) <= MAX_WORKLOG_BYTES {
            text.push_str(NOTICE);
        }
    }
}

pub(super) fn from_index(
    tx: &rusqlite::Transaction<'_>,
    first: &str,
    last: &str,
) -> Result<FocusGroups> {
    let mut groups = FocusGroups::default();
    let mut statement = tx.prepare_cached(
        "SELECT artifact_id, body FROM brauser_events
         WHERE kind = 'activity' AND local_date BETWEEN ?1 AND ?2 ORDER BY order_key",
    )?;
    let mut rows = statement.query([first, last])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let body: String = row.get(1)?;
        if let EventData::Activity(event) = serde_json::from_str::<CachedEvent>(&body)?.data {
            groups.add(&id, &event);
        }
    }
    Ok(groups)
}

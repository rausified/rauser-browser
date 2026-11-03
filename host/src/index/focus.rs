//! Direct activity only: a session is a breakdown of the existing worklog,
//! never an additional contribution to its range totals or container rollups.
use super::*;

const MAX_SESSIONS: usize = 64;
const MAX_ROWS: usize = 200;

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

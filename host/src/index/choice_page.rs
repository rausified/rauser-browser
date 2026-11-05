//! Stateless pagination of direct artifact-day choices. The digest describes
//! the complete logical result, before either row or byte limits are applied.
use super::*;
use brauser_protocol::{WorklogChoicesPageInfo, WorklogChoicesPageRequest};

pub(crate) struct ChoicesPage {
    pub choices: Vec<WorklogChoice>,
    pub page: WorklogChoicesPageInfo,
    pub gaps: Vec<OverflowMarker>,
    pub gaps_truncated: bool,
}

#[derive(Debug)]
pub(crate) struct StaleWorklogPage;

impl std::fmt::Display for StaleWorklogPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("work to share changed or this page is unavailable; reload choices")
    }
}

impl std::error::Error for StaleWorklogPage {}

pub(crate) fn valid_page_request(page: &WorklogChoicesPageRequest) -> bool {
    (page.offset == 0 || page.snapshot.is_some())
        && page.snapshot.as_ref().is_none_or(|snapshot| {
            snapshot.len() == 64
                && snapshot
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
}

pub(super) fn query(
    tx: &rusqlite::Transaction<'_>,
    start: Date,
    end: Date,
    scope: &[u8],
    request: &WorklogChoicesPageRequest,
) -> Result<ChoicesPage> {
    anyhow::ensure!(valid_page_request(request), StaleWorklogPage);
    let rows =
        query::render_choices(query::worklog_choice_days(tx, start, end)?).collect::<Vec<_>>();
    let (gaps, gaps_truncated) = query::worklog_gaps(tx, start, end)?;
    // Include even notices beyond the displayed cap: an omitted notice may
    // change without changing the first 32 or the truncation flag.
    let mut statement = tx.prepare_cached(
        "SELECT gap_id, marker FROM brauser_gaps WHERE start_date <= ?2 AND end_date >= ?1 ORDER BY gap_id",
    )?;
    let all_gaps = statement
        .query_map([start.to_string(), end.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // JSON frames every value; escaped fields cannot collide at boundaries.
    let snapshot = sha256_hex(&serde_json::to_vec(&(
        "worklog-choices-page-v1",
        scope,
        start.to_string(),
        end.to_string(),
        &rows,
        all_gaps,
        &gaps,
        gaps_truncated,
    ))?);
    anyhow::ensure!(
        request
            .snapshot
            .as_ref()
            .is_none_or(|expected| expected == &snapshot)
            && request.offset as usize <= rows.len(),
        StaleWorklogPage
    );
    let (choices, page) = bound_rows(rows, snapshot, request)?;
    Ok(ChoicesPage {
        choices,
        page,
        gaps,
        gaps_truncated,
    })
}

pub(super) fn bound_rows(
    rows: Vec<WorklogChoice>,
    snapshot: String,
    request: &WorklogChoicesPageRequest,
) -> Result<(Vec<WorklogChoice>, WorklogChoicesPageInfo)> {
    let mut choices = Vec::new();
    let mut bytes = 2usize;
    let mut omitted = false;
    let mut next_offset = None;
    for (offset, choice) in rows.into_iter().enumerate().skip(request.offset as usize) {
        let size = serde_json::to_vec(&choice)?.len().saturating_add(1);
        if !query::choice_fits(&choice) || size.saturating_add(2) > MAX_WORKLOG_BYTES {
            omitted = true;
            continue;
        }
        if choices.len() == query::MAX_WORKLOG_CHOICES
            || bytes.saturating_add(size) > MAX_WORKLOG_BYTES
        {
            next_offset = Some(u32::try_from(offset)?);
            break;
        }
        bytes = bytes.saturating_add(size);
        choices.push(choice);
    }
    Ok((
        choices,
        WorklogChoicesPageInfo {
            snapshot,
            offset: request.offset,
            next_offset,
            omitted,
        },
    ))
}

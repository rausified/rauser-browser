//! Stream a full-result snapshot, retaining only one bounded Recent page.
use super::*;
use brauser_protocol::{RecentPageInfo, RecentPageRequest, SummaryReadingFilter};

#[derive(Debug)]
pub(crate) struct RecentPage {
    pub items: Vec<ArtifactListItem>,
    pub artifact_types: Vec<String>,
    pub page: RecentPageInfo,
}

#[derive(Debug)]
pub(crate) struct StaleRecentPage;

impl std::fmt::Display for StaleRecentPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Recent changed or this page is unavailable; reload results")
    }
}
impl std::error::Error for StaleRecentPage {}

pub(crate) fn valid_recent_page_request(page: &RecentPageRequest) -> bool {
    (page.offset == 0 || page.snapshot.is_some())
        && page.snapshot.as_ref().is_none_or(|snapshot| {
            snapshot.len() == 64
                && snapshot
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
}

pub(crate) struct Filters<'a> {
    pub artifact_type: Option<&'a str>,
    pub title_query: Option<&'a str>,
    pub reading: Option<SummaryReadingFilter>,
    pub limit: usize,
}

pub(super) fn query(
    tx: &rusqlite::Transaction<'_>,
    scope: &[u8],
    filters: Filters<'_>,
    request: &RecentPageRequest,
) -> Result<RecentPage> {
    anyhow::ensure!(valid_recent_page_request(request), StaleRecentPage);
    // Use the same effective cap as the native entry point even for internal callers.
    let limit = filters.limit.clamp(1, 32);
    let title = filters
        .title_query
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_lowercase);
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&(
        "recent-page-v1",
        scope,
        filters.artifact_type,
        &title,
        filters.reading,
        limit,
    ))?);
    let mut items = Vec::new();
    let mut total = 0u32;
    let artifact_types = query::scan_recent(
        tx,
        filters.artifact_type,
        title.as_deref(),
        filters.reading,
        None,
        |item| {
            let bytes = serde_json::to_vec(&item)?;
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
            if total >= request.offset && items.len() < limit {
                items.push(item);
            }
            total = total
                .checked_add(1)
                .context("Recent row count exceeds paging range")?;
            Ok(true)
        },
    )?;
    digest.update(serde_json::to_vec(&artifact_types)?);
    let snapshot = hex::encode(digest.finalize());
    anyhow::ensure!(
        request.offset <= total
            && request
                .snapshot
                .as_ref()
                .is_none_or(|expected| expected == &snapshot),
        StaleRecentPage
    );
    let end = request
        .offset
        .checked_add(u32::try_from(items.len())?)
        .context("Recent offset overflow")?;
    Ok(RecentPage {
        items,
        artifact_types,
        page: RecentPageInfo {
            snapshot,
            offset: request.offset,
            next_offset: (end < total).then_some(end),
        },
    })
}

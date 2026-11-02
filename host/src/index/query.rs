//! Read-side views built from derived rows: cards, trails and worklog gaps.

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

pub(super) fn incoming_trails(id: &str, derived: &Derived) -> Vec<IncomingTrailView> {
    let mut incoming = derived
        .edges
        .iter()
        .filter(|edge| {
            edge.target == id
                && edge.source != id
                && trail_transition_is_valid(&edge.kind)
                // Back/forward has no attributed same-tab source. Do not
                // turn a historical paired source into apparent provenance.
                && edge.kind != "forward_back"
        })
        .filter_map(|edge| Some((edge, derived.artifacts.get(&edge.source)?)))
        .filter(|(_, source)| {
            Url::parse(&source.url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
        })
        .collect::<Vec<_>>();
    incoming.sort_by(|(a, _), (b, _)| {
        b.last_seen
            .cmp(&a.last_seen)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.kind.cmp(&b.kind))
    });
    incoming
        .into_iter()
        .take(MAX_INCOMING_TRAILS)
        .map(|(edge, source)| IncomingTrailView {
            source: ArtifactRefView {
                artifact_id: source.artifact_id.clone(),
                artifact_type: source.artifact_type.clone(),
                title: source.title.clone(),
                url: source.url.clone(),
            },
            transition: edge.kind.clone(),
            last_seen: edge.last_seen.clone(),
        })
        .collect()
}

pub(super) fn artifact_view(row: &ArtifactRow, derived: &Derived) -> ArtifactView {
    let id = row.artifact_id.as_str();
    let linked = linked_count(&derived.edges, &derived.artifacts, id);
    // A child normally has one container; if an adapter change left more
    // than one, the most-used edge wins, then the lowest ID.
    let within = derived
        .edges
        .iter()
        .filter(|edge| edge.kind == "within" && edge.source == id && edge.target != id)
        .filter_map(|edge| Some((edge, derived.artifacts.get(&edge.target)?)))
        .max_by(|(a, _), (b, _)| a.weight.cmp(&b.weight).then(b.target.cmp(&a.target)))
        .map(|(_, container)| ArtifactRefView {
            artifact_id: container.artifact_id.clone(),
            artifact_type: container.artifact_type.clone(),
            title: container.title.clone(),
            url: container.url.clone(),
        });
    let mut contained = derived
        .edges
        .iter()
        .filter(|edge| edge.kind == "within" && edge.target == id && edge.source != id)
        .map(|edge| edge.source.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|source| derived.artifacts.get(source))
        .collect::<Vec<_>>();
    contained.sort_by(|a, b| {
        b.focused_secs
            .cmp(&a.focused_secs)
            .then_with(|| a.artifact_id.cmp(&b.artifact_id))
    });
    let contained = contained
        .into_iter()
        .take(MAX_CONTAINED)
        .map(|child| ContainedArtifactView {
            artifact_id: child.artifact_id.clone(),
            artifact_type: child.artifact_type.clone(),
            title: child.title.clone(),
            url: child.url.clone(),
            focused_seconds: saturating_u32(child.focused_secs),
        })
        .collect();
    ArtifactView {
        artifact_id: row.artifact_id.clone(),
        artifact_type: row.artifact_type.clone(),
        title: row.title.clone(),
        url: row.url.clone(),
        first_seen: row.first_seen.clone(),
        last_seen: row.last_seen.clone(),
        focused_seconds: saturating_u32(row.focused_secs),
        background_seconds: saturating_u32(row.background_secs),
        opens: saturating_u32(row.opens),
        days_opened: saturating_u32(row.days.len() as u64),
        last_mode: row.last_mode.clone(),
        why: row.why.clone(),
        linked_count: saturating_u32(linked as u64),
        within,
        contained,
        incoming_trails: incoming_trails(id, derived),
    }
}

pub(super) fn linked_count(
    edges: &[EdgeRow],
    artifacts: &BTreeMap<String, ArtifactRow>,
    artifact_id: &str,
) -> usize {
    let mut linked = BTreeSet::new();
    for edge in edges {
        let other = if edge.source == artifact_id {
            Some(edge.target.as_str())
        } else if edge.target == artifact_id {
            Some(edge.source.as_str())
        } else {
            None
        };
        if let Some(other) = other
            && other != artifact_id
            && artifacts.contains_key(other)
        {
            linked.insert(other);
        }
    }
    linked.len()
}

pub(super) fn worklog_gaps(
    state: &IndexState,
    start: Date,
    end: Date,
) -> (Vec<OverflowMarker>, bool) {
    let mut gaps = state
        .gaps
        .iter()
        .filter_map(|(id, gap)| {
            let first = OffsetDateTime::parse(&gap.marker.first_occurred_at, &Rfc3339).ok()?;
            let last = OffsetDateTime::parse(&gap.marker.last_occurred_at, &Rfc3339).ok()?;
            // Endpoint dates retain the offsets recorded when activity froze.
            // Travel can reverse the local dates, so use their enclosing range.
            (first.date().min(last.date()) <= end && first.date().max(last.date()) >= start)
                .then_some((first, last, id, gap.marker.clone()))
        })
        .collect::<Vec<_>>();
    gaps.sort_by(|left, right| (&left.0, &left.1, left.2).cmp(&(&right.0, &right.1, right.2)));
    let truncated = gaps.len() > 32;
    gaps.truncate(32);
    (
        gaps.into_iter().map(|(_, _, _, marker)| marker).collect(),
        truncated,
    )
}

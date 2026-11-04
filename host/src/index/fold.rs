//! Event ordering and the derivation of artifact, visit and edge rows,
//! including container roll-ups.

use super::*;

pub(super) fn event_identity(event: &CachedEvent) -> (String, String) {
    match &event.data {
        EventData::Activity(event) => (event.event_id.clone(), event.artifact_id.clone()),
        EventData::Why(event) => (event.event_id.clone(), event.artifact_id.clone()),
        EventData::Legacy(event) => (event.marker_id.clone(), event.artifact_id.clone()),
        EventData::Summary(event) => (event.event_id.clone(), event.artifact_id.clone()),
    }
}

pub(super) fn event_time(event: &CachedEvent) -> OffsetDateTime {
    let at = match &event.data {
        EventData::Activity(event) => &event.occurred_at,
        EventData::Why(event) => &event.occurred_at,
        EventData::Legacy(event) => &event.occurred_at,
        EventData::Summary(event) => &event.occurred_at,
    };
    parsed_timestamp(at).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// The aggregation of events, in `order_key` order (`ord.rs`: by instant,
/// then durable log position, then arrival, never by random event ID),
/// into artifact, visit and edge rows: the one place their semantics live. A full
/// rebuild folds every event from `new`; the append fast path folds only
/// events that sort after everything a group already holds, from
/// `seeded` with that group's stored rows, which is the same computation
/// as replaying the whole history.
pub(super) struct Fold<'a> {
    adapters: Option<&'a Document>,
    artifacts: BTreeMap<String, ArtifactRow>,
    visits: Vec<VisitRow>,
    edges: BTreeMap<(String, String, String), EdgeRow>,
    /// Each artifact's latest why (instant, text), including an artifact
    /// with no row yet: a why can precede the first visit.
    whys: BTreeMap<String, (OffsetDateTime, String)>,
    containers: Containers<'a>,
    /// Read-time identity projection (re-learn preview). Cached events,
    /// including summary ownership keys and paths, are never rewritten.
    resolution: Option<&'a Resolution>,
}

impl<'a> Fold<'a> {
    pub(super) fn new(adapters: Option<&'a Document>) -> Self {
        Self {
            adapters,
            artifacts: BTreeMap::new(),
            visits: Vec::new(),
            edges: BTreeMap::new(),
            whys: BTreeMap::new(),
            containers: Containers::new(adapters),
            resolution: None,
        }
    }

    /// A fold that projects every artifact ID through `resolution`, as
    /// the re-learn preview does. A URL-keyed summary keeps its own ID.
    pub(super) fn resolved(adapters: Option<&'a Document>, resolution: &'a Resolution) -> Self {
        let mut fold = Self::new(adapters);
        fold.resolution = Some(resolution);
        fold.containers.resolution = Some(resolution);
        fold
    }

    /// A fold resumed from stored rows, for events that all sort strictly
    /// after every event those rows were derived from.
    ///
    /// - `rows`: the artifacts' rows as a previous fold finished them. The
    ///   timelines (`activities`, `summaries`, `legacy_visits`) may be
    ///   empty; the new events' entries are then the only ones.
    /// - `days`: dates with at least one open, per artifact, added to the
    ///   rows' own `days`.
    /// - `whys`: each artifact's latest why (instant, text). A row's `why`
    ///   is replaced from this map at `finish`, as in a full replay.
    /// - `edges`: at least every stored edge the new events touch, or none,
    ///   for a caller that adds the stored edges back itself (weights add,
    ///   and `last_seen` is the latest day).
    ///
    /// No visit rows are seeded: `finish` returns only the new ones.
    pub(super) fn seeded(
        adapters: Option<&'a Document>,
        rows: impl IntoIterator<Item = ArtifactRow>,
        days: BTreeMap<String, BTreeSet<String>>,
        whys: BTreeMap<String, (OffsetDateTime, String)>,
        edges: impl IntoIterator<Item = EdgeRow>,
    ) -> Self {
        let mut fold = Self::new(adapters);
        fold.artifacts = rows
            .into_iter()
            .map(|row| (row.artifact_id.clone(), row))
            .collect();
        for (artifact_id, dates) in days {
            if let Some(row) = fold.artifacts.get_mut(&artifact_id) {
                row.days.extend(dates);
            }
        }
        fold.whys = whys;
        fold.edges = edges
            .into_iter()
            .map(|edge| {
                (
                    (edge.source.clone(), edge.target.clone(), edge.kind.clone()),
                    edge,
                )
            })
            .collect();
        fold
    }

    /// Fold in one event. Events must arrive in `order_key` order.
    pub(super) fn apply(&mut self, event: &CachedEvent) {
        let adapters = self.adapters;
        let resolution = self.resolution;
        let canonical = |id: &'_ str| -> String {
            resolution
                .map_or(id, |resolution| resolution.canonical(id))
                .to_owned()
        };
        let artifact_id = if matches!(&event.data, EventData::Summary(summary)
            if summary.record_key.kind == RecordKeyKind::Url)
        {
            event.artifact_id().to_owned()
        } else {
            canonical(event.artifact_id())
        };
        let Self {
            artifacts,
            visits,
            edges,
            whys,
            containers,
            ..
        } = self;
        match &event.data {
            EventData::Activity(event) => {
                let at = parsed_timestamp(&event.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
                let day = at.date().to_string();
                let site = Url::parse(&event.url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .unwrap_or_default();
                let title = event.title.clone().unwrap_or_else(|| "Untitled".into());
                let row = artifacts
                    .entry(artifact_id.clone())
                    .or_insert_with(|| ArtifactRow {
                        artifact_id: artifact_id.clone(),
                        artifact_type: event.artifact_type.clone(),
                        site,
                        title: title.clone(),
                        url: event.url.clone(),
                        first_seen: day.clone(),
                        last_seen: day.clone(),
                        focused_secs: 0,
                        background_secs: 0,
                        opens: 0,
                        days: BTreeSet::new(),
                        last_mode: None,
                        why: None,
                        activities: Vec::new(),
                        summaries: Vec::new(),
                        legacy_visits: Vec::new(),
                        latest_at: at,
                        rolled_up: false,
                    });
                row.focused_secs = row
                    .focused_secs
                    .saturating_add(u64::from(event.focused_seconds));
                row.background_secs = row
                    .background_secs
                    .saturating_add(u64::from(event.background_seconds));
                row.opens = row.opens.saturating_add(u64::from(event.opens));
                if event.opens > 0 {
                    row.days.insert(day.clone());
                }
                if day < row.first_seen {
                    row.first_seen = day.clone();
                }
                if day > row.last_seen {
                    row.last_seen = day.clone();
                }
                if at >= row.latest_at {
                    row.latest_at = at;
                    if let Some(title) = &event.title {
                        row.title = title.clone();
                    }
                    row.url = event.url.clone();
                    if let Some(mode) = event.modes.last() {
                        row.last_mode = Some(mode.mode.clone());
                    }
                }
                row.activities.push(event.clone());
                visits.push(VisitRow {
                    event_id: event.event_id.clone(),
                    artifact_id: artifact_id.clone(),
                    occurred_at: event.occurred_at.clone(),
                    local_date: day.clone(),
                    focused_secs: u64::from(event.focused_seconds),
                    background_secs: u64::from(event.background_seconds),
                    opens: u64::from(event.opens),
                    mode: event.modes.last().map(|mode| mode.mode.clone()),
                    title,
                    url: event.url.clone(),
                    summary: None,
                    contained: false,
                });
                for trail in &event.trails {
                    if trail_transition_is_valid(&trail.transition)
                        && let Some(source) = &trail.from_artifact_id
                        && canonical(source) != artifact_id
                    {
                        add_edge(
                            edges,
                            &canonical(source),
                            &artifact_id,
                            &trail.transition,
                            &day,
                        );
                    }
                }
                let refs = event
                    .refs
                    .iter()
                    .map(|reference| Ref {
                        artifact_type: reference.artifact_type.clone(),
                        value: reference.value.clone(),
                    })
                    .collect::<Vec<_>>();
                // Without a self ID the resolver keeps one output per ref,
                // so a resolved self-reference is dropped here, not kept as
                // an unresolved key.
                let resolved = adapters.map(|document| document.resolve_refs(&refs, None));
                let mut targets = BTreeSet::new();
                for (index, reference) in refs.iter().enumerate() {
                    // An unresolved key never equals a recorded artifact ID
                    // (adapter-templates.md §10.1), so it is never shown.
                    let target = resolved
                        .as_ref()
                        .and_then(|resolved| resolved.get(index))
                        .and_then(|found| found.artifact_id.as_deref())
                        .map(canonical)
                        .unwrap_or_else(|| {
                            format!("{}:{}", reference.artifact_type, reference.value)
                        });
                    // The host checks each ref but not uniqueness; one event
                    // is one mention of each target.
                    if target != artifact_id {
                        targets.insert(target);
                    }
                }
                for target in targets {
                    add_edge(edges, &artifact_id, &target, "mentions", &day);
                }
                if let Some(container) = containers.of(&event.url, &artifact_id) {
                    roll_up(
                        artifacts,
                        visits,
                        edges,
                        container,
                        ContainedVisit {
                            child: &artifact_id,
                            event_id: &event.event_id,
                            occurred_at: &event.occurred_at,
                            at,
                            day: &day,
                            focused: u64::from(event.focused_seconds),
                            background: u64::from(event.background_seconds),
                            opens: u64::from(event.opens),
                            activity: Some(event),
                        },
                    );
                }
            }
            EventData::Why(event) => {
                let candidate = (
                    parsed_timestamp(&event.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH),
                    event.text.clone(),
                );
                if whys
                    .get(&artifact_id)
                    .is_none_or(|current| candidate.0 >= current.0)
                {
                    whys.insert(artifact_id.clone(), candidate);
                }
            }
            EventData::Legacy(event) => {
                let at = parsed_timestamp(&event.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
                let day = at.date().to_string();
                let site = Url::parse(&event.url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .unwrap_or_default();
                let row = artifacts
                    .entry(artifact_id.clone())
                    .or_insert_with(|| ArtifactRow {
                        artifact_id: artifact_id.clone(),
                        artifact_type: "web.page".into(),
                        site,
                        title: event.title.clone(),
                        url: event.url.clone(),
                        first_seen: day.clone(),
                        last_seen: day.clone(),
                        focused_secs: 0,
                        background_secs: 0,
                        opens: 0,
                        days: BTreeSet::new(),
                        last_mode: None,
                        why: None,
                        activities: Vec::new(),
                        summaries: Vec::new(),
                        legacy_visits: Vec::new(),
                        latest_at: at,
                        rolled_up: false,
                    });
                // An M1 visit entry recorded one navigation to the page:
                // count it as one open, with no measured time.
                row.opens = row.opens.saturating_add(1);
                row.legacy_visits.push(at);
                row.days.insert(day.clone());
                if day < row.first_seen {
                    row.first_seen = day.clone();
                }
                if day > row.last_seen {
                    row.last_seen = day.clone();
                }
                if at >= row.latest_at {
                    row.latest_at = at;
                    row.title = event.title.clone();
                    row.url = event.url.clone();
                }
                visits.push(VisitRow {
                    event_id: event.marker_id.clone(),
                    artifact_id: artifact_id.clone(),
                    occurred_at: event.occurred_at.clone(),
                    local_date: day.clone(),
                    focused_secs: 0,
                    background_secs: 0,
                    opens: 1,
                    mode: None,
                    title: event.title.clone(),
                    url: event.url.clone(),
                    summary: None,
                    contained: false,
                });
                if let Some(container) = containers.of(&event.url, &artifact_id) {
                    roll_up(
                        artifacts,
                        visits,
                        edges,
                        container,
                        ContainedVisit {
                            child: &artifact_id,
                            event_id: &event.marker_id,
                            occurred_at: &event.occurred_at,
                            at,
                            day: &day,
                            focused: 0,
                            background: 0,
                            opens: 1,
                            activity: None,
                        },
                    );
                }
            }
            EventData::Summary(event) => {
                let at = parsed_timestamp(&event.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
                let day = at.date().to_string();
                let site = Url::parse(&event.url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .unwrap_or_default();
                let row = artifacts
                    .entry(artifact_id.clone())
                    .or_insert_with(|| ArtifactRow {
                        artifact_id: artifact_id.clone(),
                        artifact_type: event.artifact_type.clone(),
                        site,
                        title: event.title.clone(),
                        url: event.url.clone(),
                        first_seen: day.clone(),
                        last_seen: day.clone(),
                        focused_secs: 0,
                        background_secs: 0,
                        opens: 0,
                        days: BTreeSet::new(),
                        last_mode: None,
                        why: None,
                        activities: Vec::new(),
                        summaries: Vec::new(),
                        legacy_visits: Vec::new(),
                        latest_at: at,
                        rolled_up: false,
                    });
                // A summary moves first and last seen but is not an open,
                // so it never adds to the days opened.
                if day < row.first_seen {
                    row.first_seen = day.clone();
                }
                if day > row.last_seen {
                    row.last_seen = day.clone();
                }
                if at >= row.latest_at {
                    row.latest_at = at;
                    row.title = event.title.clone();
                    row.url = event.url.clone();
                }
                row.summaries.push(event.clone());
                visits.push(VisitRow {
                    event_id: event.event_id.clone(),
                    artifact_id: artifact_id.clone(),
                    occurred_at: event.occurred_at.clone(),
                    local_date: day,
                    focused_secs: 0,
                    background_secs: 0,
                    opens: 0,
                    mode: None,
                    title: event.title.clone(),
                    url: event.url.clone(),
                    summary: Some(event.clone()),
                    contained: false,
                });
            }
        }
    }

    /// The sorted rows, with each row's why and containers named for
    /// themselves.
    pub(super) fn finish(self) -> Derived {
        let Self {
            mut artifacts,
            mut visits,
            edges,
            mut whys,
            containers,
            ..
        } = self;
        let mut why_at = BTreeMap::new();
        // Visits and summaries were pushed in `order_key` order (instant,
        // then durable log position), and a stable sort by time keeps that
        // order for equal times: two summaries of a record in the same second
        // (stamps have whole-second resolution) keep their append order, which
        // is the order their summary file was written in, not their random
        // run IDs'.
        visits.sort_by(|a, b| timestamp_order(&a.occurred_at, &b.occurred_at));
        for row in artifacts.values_mut() {
            row.activities.sort_by(|a, b| {
                timestamp_order(&a.occurred_at, &b.occurred_at).then(a.event_id.cmp(&b.event_id))
            });
            row.summaries
                .sort_by(|a, b| timestamp_order(&a.occurred_at, &b.occurred_at));
            row.why = whys.remove(&row.artifact_id).map(|(at, text)| {
                why_at.insert(row.artifact_id.clone(), at);
                text
            });
            // A container is named for itself, never for whichever page inside
            // it was visited last (goal 4).
            if containers.types.contains(row.artifact_type.as_str())
                && let Ok(url) = Url::parse(&row.url)
            {
                row.title = container_title(&row.artifact_id);
                row.url = container_url(&url.origin().ascii_serialization(), &row.artifact_id);
            }
        }
        Derived {
            artifacts,
            visits,
            edges: edges.into_values().collect(),
            why_at,
        }
    }
}

/// The container (type, ID, adapter origin) a logged visit rolls up into:
/// only when the current document still recognizes the visit's URL as the
/// same artifact it was logged as, and that adapter declares `within`.
/// Under a `resolution`, both the recognized child and the container
/// endpoint are compared and named by their canonical IDs.
pub(super) fn container_for(
    adapters: Option<&Document>,
    url: &str,
    artifact_id: &str,
    resolution: Option<&Resolution>,
) -> Option<(String, String, String)> {
    let canonical = |id: &str| resolution.map_or(id, |r| r.canonical(id)).to_owned();
    let document = adapters?;
    let found = document.evaluate(url, None).ok()?;
    if canonical(&found.artifact_id) != artifact_id {
        return None;
    }
    let within = found.within?;
    let container_id = canonical(&within.artifact_id);
    if container_id == artifact_id {
        return None;
    }
    let origin = document.adapters().get(found.adapter)?.origin().to_owned();
    Some((within.artifact_type, container_id, origin))
}

/// A container ID's native part, after `<scheme>:<host>/`: for GitHub,
/// `owner/repo`.
pub(super) fn container_title(id: &str) -> String {
    let native = id.split_once(':').map_or(id, |(_, rest)| rest);
    native
        .split_once('/')
        .map_or(native, |(_, rest)| rest)
        .to_owned()
}

pub(super) fn container_url(origin: &str, id: &str) -> String {
    format!("{}/{}", origin.trim_end_matches('/'), container_title(id))
}

/// The title and URL for an artifact with no record yet, when some adapter
/// declares its type as a container: container rows are always named from
/// their ID, never from whichever sub-page happened to be open.
pub(crate) fn unvisited_container(
    document: &Document,
    found: &adapter_templates::Match,
) -> Option<(String, String)> {
    let is_container = document
        .adapters()
        .iter()
        .filter_map(|adapter| adapter.within())
        .any(|within| within.artifact_type() == found.artifact_type);
    if !is_container {
        return None;
    }
    let origin = document.adapters().get(found.adapter)?.origin();
    Some((
        container_title(&found.artifact_id),
        container_url(origin, &found.artifact_id),
    ))
}

/// The container an artifact with no record yet sits within, as its match
/// declares it, named the way `roll_up` names a container row: from its ID,
/// under the matching adapter's origin.
pub(crate) fn unvisited_within(
    document: &Document,
    found: &adapter_templates::Match,
) -> Option<ArtifactRefView> {
    let within = found.within.as_ref()?;
    if within.artifact_id == found.artifact_id {
        return None;
    }
    let origin = document.adapters().get(found.adapter)?.origin();
    Some(ArtifactRefView {
        artifact_id: within.artifact_id.clone(),
        artifact_type: within.artifact_type.clone(),
        title: container_title(&within.artifact_id),
        url: container_url(origin, &within.artifact_id),
    })
}

/// A container's (type, ID, adapter origin), as `container_for` finds it.
pub(super) type ContainerRef = (String, String, String);

/// Container lookups for one derivation, memoized by (URL, artifact ID)
/// because many logged visits share a URL.
pub(super) struct Containers<'a> {
    adapters: Option<&'a Document>,
    /// Types some adapter declares as its `within` container.
    pub(super) types: BTreeSet<&'a str>,
    found: BTreeMap<(String, String), Option<ContainerRef>>,
    resolution: Option<&'a Resolution>,
}

impl<'a> Containers<'a> {
    pub(super) fn new(adapters: Option<&'a Document>) -> Self {
        let types = adapters
            .into_iter()
            .flat_map(Document::adapters)
            .filter_map(|adapter| adapter.within().map(|within| within.artifact_type()))
            .collect();
        Self {
            adapters,
            types,
            found: BTreeMap::new(),
            resolution: None,
        }
    }

    pub(super) fn of(&mut self, url: &str, artifact_id: &str) -> Option<ContainerRef> {
        if self.types.is_empty() {
            return None;
        }
        let (adapters, resolution) = (self.adapters, self.resolution);
        self.found
            .entry((url.to_owned(), artifact_id.to_owned()))
            .or_insert_with(|| container_for(adapters, url, artifact_id, resolution))
            .clone()
    }
}

/// One visit to an artifact within a container.
pub(super) struct ContainedVisit<'a> {
    child: &'a str,
    event_id: &'a str,
    occurred_at: &'a str,
    at: OffsetDateTime,
    day: &'a str,
    focused: u64,
    background: u64,
    opens: u64,
    /// The activity, for the container's timeline; `None` for an M1 visit.
    activity: Option<&'a ActivityEvent>,
}

/// Add one contained visit to its container's record, creating the record
/// if the container was never visited directly. The child's own row is
/// unchanged; a visit matches exactly one adapter, so it reaches its
/// container exactly once. Days are a set, so they union.
pub(super) fn roll_up(
    artifacts: &mut BTreeMap<String, ArtifactRow>,
    visits: &mut Vec<VisitRow>,
    edges: &mut BTreeMap<(String, String, String), EdgeRow>,
    (container_type, container_id, origin): ContainerRef,
    visit: ContainedVisit<'_>,
) {
    let title = container_title(&container_id);
    let url = container_url(&origin, &container_id);
    let row = artifacts
        .entry(container_id.clone())
        .or_insert_with(|| ArtifactRow {
            artifact_id: container_id.clone(),
            artifact_type: container_type,
            site: Url::parse(&origin)
                .ok()
                .and_then(|origin| origin.host_str().map(str::to_owned))
                .unwrap_or_default(),
            title: title.clone(),
            url: url.clone(),
            first_seen: visit.day.to_owned(),
            last_seen: visit.day.to_owned(),
            focused_secs: 0,
            background_secs: 0,
            opens: 0,
            days: BTreeSet::new(),
            last_mode: None,
            why: None,
            activities: Vec::new(),
            summaries: Vec::new(),
            legacy_visits: Vec::new(),
            latest_at: visit.at,
            rolled_up: false,
        });
    row.rolled_up = true;
    row.focused_secs = row.focused_secs.saturating_add(visit.focused);
    row.background_secs = row.background_secs.saturating_add(visit.background);
    row.opens = row.opens.saturating_add(visit.opens);
    if visit.opens > 0 {
        row.days.insert(visit.day.to_owned());
    }
    if visit.day < row.first_seen.as_str() {
        row.first_seen = visit.day.to_owned();
    }
    if visit.day > row.last_seen.as_str() {
        row.last_seen = visit.day.to_owned();
    }
    match visit.activity {
        Some(activity) => row.activities.push(activity.clone()),
        None => row.legacy_visits.push(visit.at),
    }
    visits.push(VisitRow {
        event_id: visit.event_id.to_owned(),
        artifact_id: container_id.clone(),
        occurred_at: visit.occurred_at.to_owned(),
        local_date: visit.day.to_owned(),
        focused_secs: visit.focused,
        background_secs: visit.background,
        opens: visit.opens,
        mode: None,
        title,
        url,
        summary: None,
        contained: true,
    });
    add_edge(edges, visit.child, &container_id, "within", visit.day);
}

pub(super) fn add_edge(
    edges: &mut BTreeMap<(String, String, String), EdgeRow>,
    source: &str,
    target: &str,
    kind: &str,
    day: &str,
) {
    let key = (source.to_owned(), target.to_owned(), kind.to_owned());
    let row = edges.entry(key).or_insert_with(|| EdgeRow {
        source: source.into(),
        target: target.into(),
        kind: kind.into(),
        weight: 0,
        last_seen: day.into(),
    });
    row.weight = row.weight.saturating_add(1);
    if day > row.last_seen.as_str() {
        row.last_seen = day.into();
    }
}

pub(super) fn parsed_timestamp(timestamp: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(timestamp, &Rfc3339).ok()
}

pub(super) fn timestamp_order(left: &str, right: &str) -> std::cmp::Ordering {
    match (parsed_timestamp(left), parsed_timestamp(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

#[cfg(test)]
mod tests {
    //! The fold's seeded replay of a strictly newest suffix must equal a
    //! full replay; the append fast path depends on it.

    use super::*;
    use brauser_protocol::{ActivityMode, ActivityRef, ActivityTrail};

    /// SplitMix64: deterministic, so a failure names its seed.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        fn chance(&mut self, percent: u64) -> bool {
            self.below(100) < percent
        }
    }

    /// GitHub PRs and issues within their repo (a container), the repo
    /// catch-all, and a Jira adapter whose single-capture ID resolves refs.
    fn document() -> Document {
        let container = serde_json::json!({
            "type": "github.repo",
            "id": "github.repo:{host}/{owner}/{repo}",
        });
        let value: adapter_templates::Value = serde_json::from_value(serde_json::json!({
            "format": 1,
            "artifacts": [
                {
                    "type": "github.pr",
                    "origin": "https://github.com",
                    "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}"}],
                    "scope": {"owner": ["acme"]},
                    "id": "github.pr:{host}/{owner}/{repo}/{number}",
                    "within": container,
                },
                {
                    "type": "github.issue",
                    "origin": "https://github.com",
                    "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}"}],
                    "scope": {"owner": ["acme"]},
                    "id": "github.issue:{host}/{owner}/{repo}/{number}",
                    "within": container,
                },
                {
                    "type": "github.repo",
                    "origin": "https://github.com",
                    "match": [{"path": "/{owner:name|lower}/{repo:name|lower}/{*}"}],
                    "scope": {"owner": ["acme"]},
                    "id": "github.repo:{host}/{owner}/{repo}",
                },
                {
                    "type": "jira.issue",
                    "origin": "https://jira.example.com",
                    "match": [{"path": "/browse/{key:token}"}],
                    "id": "jira.issue:{host}/{key}",
                },
            ],
        }))
        .unwrap();
        Document::from_value(&value).unwrap()
    }

    /// (artifact type, artifact ID, URL) of one of a few targets: PRs and
    /// issues in two repos, a repo page itself, Jira issues and plain pages.
    fn target(rng: &mut Rng) -> (String, String, String) {
        let repo = ["acme/widgets", "acme/gadgets"][rng.below(2) as usize];
        let n = rng.below(3) + 1;
        match rng.below(6) {
            0 => (
                "github.pr".into(),
                format!("github.pr:github.com/{repo}/{n}"),
                format!("https://github.com/{repo}/pull/{n}"),
            ),
            1 => (
                "github.issue".into(),
                format!("github.issue:github.com/{repo}/{n}"),
                format!("https://github.com/{repo}/issues/{n}"),
            ),
            2 => (
                "github.repo".into(),
                format!("github.repo:github.com/{repo}"),
                format!("https://github.com/{repo}/tree/main"),
            ),
            3 => (
                "jira.issue".into(),
                format!("jira.issue:jira.example.com/ABC-{n}"),
                format!("https://jira.example.com/browse/ABC-{n}"),
            ),
            _ => (
                "web.page".into(),
                format!("web.page:example.com/{n}"),
                format!("https://example.com/{n}"),
            ),
        }
    }

    /// Instants within a few days, at minute resolution so ties are common.
    fn instant(rng: &mut Rng) -> String {
        let minutes = rng.below(3 * 24 * 60 / 97) * 97;
        let at = OffsetDateTime::UNIX_EPOCH
            + time::Duration::days(20_000)
            + time::Duration::minutes(minutes as i64);
        let offset = [0, 2, -5][rng.below(3) as usize];
        at.to_offset(UtcOffset::from_hms(offset, 0, 0).unwrap())
            .format(&Rfc3339)
            .unwrap()
    }

    fn event(rng: &mut Rng, n: u64) -> CachedEvent {
        let id = format!("00000000-0000-4000-8000-{n:012}");
        let (artifact_type, artifact_id, url) = target(rng);
        let occurred_at = instant(rng);
        let data = match rng.below(10) {
            0 => EventData::Why(WhyEvent {
                event_id: id,
                artifact_id,
                url,
                occurred_at,
                incognito: false,
                text: format!("why {n}"),
            }),
            1 => EventData::Legacy(LegacyVisit {
                marker_id: id,
                artifact_id,
                occurred_at,
                url,
                title: format!("Legacy {n}"),
            }),
            2 => EventData::Summary(SummaryEvent {
                overview: None,
                read_scope: None,
                event_id: id,
                artifact_id: artifact_id.clone(),
                artifact_type,
                url,
                title: format!("Summary {n}"),
                occurred_at,
                document_type: "discussion".into(),
                category: "reading".into(),
                importance: brauser_protocol::ImportanceLevel::High,
                summary_path: format!("summaries/{n}.summary.md"),
                record_key: brauser_protocol::RecordKey {
                    kind: brauser_protocol::RecordKeyKind::Artifact,
                    value: artifact_id,
                },
                harness: brauser_protocol::HarnessId::Claude,
            }),
            _ => {
                let modes = (0..rng.below(3))
                    .map(|index| ActivityMode {
                        mode: ["view", "edit", "review"][(index + rng.below(3)) as usize % 3]
                            .into(),
                        url: url.clone(),
                    })
                    .collect();
                let trails = (0..rng.below(3))
                    .map(|_| {
                        let (_, from, from_url) = target(rng);
                        ActivityTrail {
                            from_artifact_id: rng.chance(80).then_some(from),
                            from_url: Some(from_url),
                            transition: ["link", "typed", "forward_back", "bogus"]
                                [rng.below(4) as usize]
                                .into(),
                        }
                    })
                    .collect();
                let refs = (0..rng.below(3))
                    .map(|_| ActivityRef {
                        artifact_type: ["jira.issue", "unknown.kind"][rng.below(2) as usize].into(),
                        value: format!("ABC-{}", rng.below(3) + 1),
                    })
                    .collect();
                EventData::Activity(ActivityEvent {
                    focus_session: None,
                    event_id: id,
                    artifact_id,
                    artifact_type,
                    url,
                    title: rng.chance(75).then(|| format!("Title {n}")),
                    occurred_at,
                    incognito: false,
                    opens: rng.below(3) as u32,
                    focused_seconds: rng.below(600) as u32,
                    background_seconds: rng.below(600) as u32,
                    modes,
                    trails,
                    refs,
                })
            }
        };
        data.into()
    }

    /// A history whose events have random positions (or none, with an
    /// arrival sequence), in replay (`order_key`) order.
    fn history(rng: &mut Rng) -> Vec<CachedEvent> {
        let mut events = BTreeMap::new();
        for n in 0..rng.below(40) {
            let event = event(rng, n);
            let (id, _) = event_identity(&event);
            let (position, sequence) = if rng.chance(80) {
                let log = ["2024-10-01.md", "2024-10-02.md"][rng.below(2) as usize];
                (Some((PathBuf::from(log), rng.below(50))), None)
            } else {
                (None, Some(n))
            };
            let key = ord::order_key(
                event_time(&event),
                position
                    .as_ref()
                    .map(|(path, line)| (path.as_path(), *line)),
                sequence,
                &id,
            );
            events.insert(id, (key, event));
        }
        let mut ordered = events.into_values().collect::<Vec<_>>();
        ordered.sort_by(|left, right| left.0.cmp(&right.0));
        ordered.into_iter().map(|(_, event)| event).collect()
    }

    fn replay<'a>(mut fold: Fold<'a>, events: &[CachedEvent]) -> Derived {
        for event in events {
            fold.apply(event);
        }
        fold.finish()
    }

    /// Every aggregate field of a row; with `history`, also the per-row
    /// timelines a fast path does not load.
    fn row(row: &ArtifactRow, history: bool) -> String {
        let aggregate = format!(
            "{:?}",
            (
                (&row.artifact_id, &row.artifact_type, &row.site),
                (&row.title, &row.url, &row.first_seen, &row.last_seen),
                (row.focused_secs, row.background_secs, row.opens),
                (&row.days, &row.last_mode, &row.why),
                (row.latest_at, row.rolled_up),
            )
        );
        if history {
            format!(
                "{aggregate} {:?}",
                (&row.activities, &row.summaries, &row.legacy_visits)
            )
        } else {
            aggregate
        }
    }

    fn visit(visit: &VisitRow) -> String {
        format!(
            "{:?}",
            (
                (&visit.event_id, &visit.artifact_id, &visit.occurred_at),
                (&visit.local_date, visit.focused_secs, visit.background_secs),
                (visit.opens, &visit.mode, &visit.title, &visit.url),
                (&visit.summary, visit.contained),
            )
        )
    }

    fn edges(edges: &[EdgeRow]) -> Vec<String> {
        let mut edges = edges
            .iter()
            .map(|edge| {
                format!(
                    "{:?}",
                    (
                        &edge.source,
                        &edge.target,
                        &edge.kind,
                        edge.weight,
                        &edge.last_seen
                    )
                )
            })
            .collect::<Vec<_>>();
        edges.sort();
        edges
    }

    fn rows(derived: &Derived, history: bool) -> Vec<String> {
        derived
            .artifacts
            .values()
            .map(|artifact| row(artifact, history))
            .collect()
    }

    fn visits(visits: &[VisitRow]) -> Vec<String> {
        visits.iter().map(visit).collect()
    }

    /// The days each row was opened on: what `brauser_artifact_days` holds
    /// for dates with opens.
    fn opened_days(derived: &Derived) -> BTreeMap<String, BTreeSet<String>> {
        let mut days = BTreeMap::<String, BTreeSet<String>>::new();
        for visit in &derived.visits {
            if visit.opens > 0 {
                days.entry(visit.artifact_id.clone())
                    .or_default()
                    .insert(visit.local_date.clone());
            }
        }
        days
    }

    /// Each artifact's latest why among `events`, including an artifact
    /// with no row yet (a why can precede the first visit).
    fn whys(events: &[CachedEvent]) -> BTreeMap<String, (OffsetDateTime, String)> {
        let mut whys = BTreeMap::new();
        for event in events {
            if let EventData::Why(why) = &event.data {
                whys.insert(
                    why.artifact_id.clone(),
                    (event_time(event), why.text.clone()),
                );
            }
        }
        whys
    }

    #[test]
    fn seeded_suffix_equals_full_replay() {
        let document = document();
        let (mut checked, mut rolled_up, mut resolved) = (0, 0, 0);
        for seed in 0..120 {
            let mut rng = Rng(seed);
            let events = history(&mut rng);
            for adapters in [None, Some(&document)] {
                let full = replay(Fold::new(adapters), &events);
                rolled_up += full.artifacts.values().filter(|row| row.rolled_up).count();
                resolved += full
                    .edges
                    .iter()
                    .filter(|edge| {
                        edge.kind == "mentions" && edge.target.starts_with("jira.issue:jira")
                    })
                    .count();
                for split in 0..=events.len() {
                    let (before, after) = events.split_at(split);
                    let prefix = replay(Fold::new(adapters), before);
                    let context = format!(
                        "seed {seed}, split {split}, containers {}",
                        adapters.is_some()
                    );

                    // Seeded with the prefix's whole rows, the suffix gives
                    // exactly the full replay.
                    let seeded = replay(
                        Fold::seeded(
                            adapters,
                            prefix.artifacts.values().cloned(),
                            BTreeMap::new(),
                            whys(before),
                            prefix.edges.iter().cloned(),
                        ),
                        after,
                    );
                    assert_eq!(rows(&full, true), rows(&seeded, true), "{context}");
                    let mut combined = prefix.visits.clone();
                    combined.extend(seeded.visits.iter().cloned());
                    assert_eq!(visits(&full.visits), visits(&combined), "{context}");
                    assert_eq!(edges(&full.edges), edges(&seeded.edges), "{context}");
                    assert_eq!(full.why_at, seeded.why_at, "{context}");

                    // Seeded as the fast path loads it (aggregate rows with
                    // their days apart and no timelines), the aggregates
                    // and the new visits match.
                    let stored = prefix.artifacts.values().cloned().map(|mut row| {
                        row.days.clear();
                        row.activities.clear();
                        row.summaries.clear();
                        row.legacy_visits.clear();
                        row
                    });
                    let fast = replay(
                        Fold::seeded(
                            adapters,
                            stored,
                            opened_days(&prefix),
                            whys(before),
                            prefix.edges.iter().cloned(),
                        ),
                        after,
                    );
                    assert_eq!(rows(&full, false), rows(&fast, false), "{context}");
                    assert_eq!(
                        visits(&full.visits[prefix.visits.len()..]),
                        visits(&fast.visits),
                        "{context}"
                    );
                    assert_eq!(edges(&full.edges), edges(&fast.edges), "{context}");
                    assert_eq!(full.why_at, fast.why_at, "{context}");
                    checked += 1;
                }
            }
        }
        // The histories exercise containers and resolved mentions.
        assert!(checked > 1000, "{checked}");
        assert!(rolled_up > 0 && resolved > 0, "{rolled_up} {resolved}");
    }
}

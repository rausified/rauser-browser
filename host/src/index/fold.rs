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

/// Events by instant, then durable log position, never by (random) event
/// ID, so the latest title, mode, and context do not depend on UUID order.
pub(super) fn ordered_events(state: &IndexState) -> Vec<&CachedEvent> {
    let mut ordered = state
        .events
        .iter()
        .map(|(id, event)| {
            let position = state.positions.get(id);
            let sequence = state.sequence.get(id).copied().unwrap_or(u64::MAX);
            (
                (
                    event_time(event),
                    position.is_none(),
                    position,
                    sequence,
                    id,
                ),
                event,
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    ordered.into_iter().map(|(_, event)| event).collect()
}

pub(super) fn derive(state: &IndexState, adapters: Option<&Document>) -> Derived {
    let mut artifacts = BTreeMap::<String, ArtifactRow>::new();
    let mut visits = Vec::new();
    let mut edges = BTreeMap::<(String, String, String), EdgeRow>::new();
    let mut whys = BTreeMap::<String, (OffsetDateTime, String)>::new();
    let mut containers = Containers::new(adapters);
    for event in ordered_events(state) {
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
                    .entry(event.artifact_id.clone())
                    .or_insert_with(|| ArtifactRow {
                        artifact_id: event.artifact_id.clone(),
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
                    artifact_id: event.artifact_id.clone(),
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
                    {
                        add_edge(
                            &mut edges,
                            source,
                            &event.artifact_id,
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
                        .and_then(|found| found.artifact_id.clone())
                        .unwrap_or_else(|| {
                            format!("{}:{}", reference.artifact_type, reference.value)
                        });
                    // The host checks each ref but not uniqueness; one event
                    // is one mention of each target.
                    if target != event.artifact_id {
                        targets.insert(target);
                    }
                }
                for target in targets {
                    add_edge(&mut edges, &event.artifact_id, &target, "mentions", &day);
                }
                if let Some(container) = containers.of(&event.url, &event.artifact_id) {
                    roll_up(
                        &mut artifacts,
                        &mut visits,
                        &mut edges,
                        container,
                        ContainedVisit {
                            child: &event.artifact_id,
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
                    .get(&event.artifact_id)
                    .is_none_or(|current| candidate.0 >= current.0)
                {
                    whys.insert(event.artifact_id.clone(), candidate);
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
                    .entry(event.artifact_id.clone())
                    .or_insert_with(|| ArtifactRow {
                        artifact_id: event.artifact_id.clone(),
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
                    artifact_id: event.artifact_id.clone(),
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
                if let Some(container) = containers.of(&event.url, &event.artifact_id) {
                    roll_up(
                        &mut artifacts,
                        &mut visits,
                        &mut edges,
                        container,
                        ContainedVisit {
                            child: &event.artifact_id,
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
                    .entry(event.artifact_id.clone())
                    .or_insert_with(|| ArtifactRow {
                        artifact_id: event.artifact_id.clone(),
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
                    artifact_id: event.artifact_id.clone(),
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
    // Visits and summaries were pushed in `ordered_events` order (instant,
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
        row.why = whys.remove(&row.artifact_id).map(|(_, text)| text);
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
    }
}

/// The container (type, ID, adapter origin) a logged visit rolls up into:
/// only when the current document still recognizes the visit's URL as the
/// same artifact it was logged as, and that adapter declares `within`.
pub(super) fn container_for(
    adapters: Option<&Document>,
    url: &str,
    artifact_id: &str,
) -> Option<(String, String, String)> {
    let document = adapters?;
    let found = document.evaluate(url, None).ok()?;
    if found.artifact_id != artifact_id {
        return None;
    }
    let within = found.within?;
    if within.artifact_id == artifact_id {
        return None;
    }
    let origin = document.adapters().get(found.adapter)?.origin().to_owned();
    Some((within.artifact_type, within.artifact_id, origin))
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
        }
    }

    pub(super) fn of(&mut self, url: &str, artifact_id: &str) -> Option<ContainerRef> {
        if self.types.is_empty() {
            return None;
        }
        let adapters = self.adapters;
        self.found
            .entry((url.to_owned(), artifact_id.to_owned()))
            .or_insert_with(|| container_for(adapters, url, artifact_id))
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

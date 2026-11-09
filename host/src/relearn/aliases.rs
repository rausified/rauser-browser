//! Shared host-only alias projection. Inputs have already passed endpoint
//! recognition or a fresh impact review; this module grants no write authority.
use super::{
    Identity, Plan,
    journal::{Decision, validate_decision},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Confirm,
    Suggest,
    Unalias,
}

/// Endpoints here have already been derived by the host, including type and
/// scope. Native messages must never deserialize directly into this type.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) event_id: String,
    pub(crate) action: Action,
    pub(crate) from: Identity,
    pub(crate) to: Identity,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Group {
    pub(crate) canonical: Identity,
    pub(crate) members: Vec<Identity>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Suggestion {
    pub(crate) event_id: String,
    pub(crate) from: Identity,
    pub(crate) to: Identity,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Preview {
    /// This output describes only supplied evidence, never the entire vault.
    pub(crate) review_only: bool,
    pub(crate) groups: Vec<Group>,
    pub(crate) suggestions: Vec<Suggestion>,
}

/// Rebuild from the append-only operations, without mutating an earlier
/// projection. The latest confirmed operation for an unordered pair wins;
/// a suggestion cannot revoke a confirmation. Unalias removes that pair's
/// edge, not every path between its endpoints. Canonical identity is the
/// target of the most recent still-active confirmation in the component.
pub(crate) fn replay(events: &[Event]) -> Result<Preview> {
    let mut seen = BTreeMap::new();
    let mut identities = BTreeSet::new();
    let mut decisions = BTreeMap::new();
    let mut suggestions = BTreeMap::new();
    let mut types = BTreeMap::new();
    for (sequence, event) in events.iter().enumerate() {
        if let Some(previous) = seen.insert(&event.event_id, event) {
            ensure!(previous == event, "conflicting reuse of an event ID");
            continue;
        }
        ensure!(
            event.from != event.to,
            "an alias must name two different identities"
        );
        ensure!(
            event.from.artifact_type == event.to.artifact_type,
            "an alias cannot join different artifact types"
        );
        for endpoint in [&event.from, &event.to] {
            if let Some(previous) = types.insert(&endpoint.artifact_id, &endpoint.artifact_type) {
                ensure!(
                    previous == &endpoint.artifact_type,
                    "one artifact key names different types"
                );
            }
        }
        identities.extend([event.from.clone(), event.to.clone()]);
        let pair = if event.from < event.to {
            (&event.from, &event.to)
        } else {
            (&event.to, &event.from)
        };
        if event.action == Action::Suggest {
            suggestions.insert(pair, event);
        } else {
            decisions.insert(pair, (sequence, event));
            suggestions.remove(&pair);
        }
    }

    let mut neighbors: BTreeMap<&Identity, Vec<(&Identity, usize, &Identity)>> = BTreeMap::new();
    for &(sequence, event) in decisions.values() {
        if event.action == Action::Confirm {
            neighbors
                .entry(&event.from)
                .or_default()
                .push((&event.to, sequence, &event.to));
            neighbors
                .entry(&event.to)
                .or_default()
                .push((&event.from, sequence, &event.to));
        }
    }
    let mut remaining = identities;
    let mut canonical_by_id = BTreeMap::new();
    let mut groups = Vec::new();
    while let Some(first) = remaining.pop_first() {
        let mut members = BTreeSet::from([first.clone()]);
        let mut pending = vec![first.clone()];
        let mut newest = None;
        while let Some(current) = pending.pop() {
            for &(neighbor, sequence, target) in neighbors.get(&current).into_iter().flatten() {
                if newest.is_none_or(|(last, _)| sequence > last) {
                    newest = Some((sequence, target));
                }
                if remaining.remove(neighbor) {
                    members.insert(neighbor.clone());
                    pending.push(neighbor.clone());
                }
            }
        }
        let canonical = newest.map_or(first, |(_, target)| target.clone());
        for member in &members {
            canonical_by_id.insert(member.clone(), canonical.clone());
        }
        groups.push(Group {
            canonical,
            members: members.into_iter().collect(),
        });
    }
    groups.sort_by(|a, b| a.canonical.cmp(&b.canonical));
    let suggestions = suggestions
        .into_values()
        .filter(|event| canonical_by_id[&event.from] != canonical_by_id[&event.to])
        .map(|event| Suggestion {
            event_id: event.event_id.clone(),
            from: event.from.clone(),
            to: event.to.clone(),
        })
        .collect();
    Ok(Preview {
        review_only: true,
        groups,
        suggestions,
    })
}

/// One lookup table for host readers. No native request deserializes into it.
/// Raw event IDs, record ownership and physical file locations stay untouched.
#[derive(Default)]
pub(crate) struct Resolution {
    canonical: BTreeMap<String, String>,
    // Bind note edits to replay history, not merely its final mapping. A
    // move away and back must not revive an old editor's save authority.
    history_revision: Option<String>,
}

impl Resolution {
    pub(crate) fn from_events(events: &[Event]) -> Result<Self> {
        let preview = replay(events)?;
        Ok(Self {
            history_revision: (!events.is_empty())
                .then(|| serde_json::to_vec(events).map(|bytes| super::hash(&bytes)))
                .transpose()?,
            canonical: preview
                .groups
                .into_iter()
                .flat_map(|group| {
                    group.members.into_iter().map(move |member| {
                        (member.artifact_id, group.canonical.artifact_id.clone())
                    })
                })
                .collect(),
        })
    }

    /// A fresh, unambiguous impact report is a simultaneous mapping, never
    /// an ordered list of alias edges. Refuse key reuse before replaying it.
    pub(crate) fn from_plan(plan: &Plan) -> Result<Self> {
        validate_decision(Decision::Approve, plan)?;
        let moving: BTreeSet<_> = plan
            .identities
            .iter()
            .filter_map(|row| {
                let target = row.targets.first()?;
                (target.artifact_id != row.source.value).then_some(row.source.value.as_str())
            })
            .collect();
        let mut targets = BTreeSet::new();
        let mut events = Vec::new();
        for row in &plan.identities {
            let target = row.targets.first().expect("approval requires one target");
            ensure!(
                targets.insert(&target.artifact_id),
                "simultaneous re-key merge"
            );
            if target.artifact_id == row.source.value {
                continue;
            }
            ensure!(
                !moving.contains(target.artifact_id.as_str()),
                "simultaneous re-key reuse"
            );
            events.push(Event {
                event_id: row.source.value.clone(),
                action: Action::Confirm,
                from: Identity {
                    artifact_type: target.artifact_type.clone(),
                    artifact_id: row.source.value.clone(),
                },
                to: target.clone(),
            });
        }
        Self::from_events(&events)
    }

    pub(crate) fn canonical<'a>(&'a self, id: &'a str) -> &'a str {
        self.canonical.get(id).map(String::as_str).unwrap_or(id)
    }

    pub(crate) fn members<'a>(&'a self, id: &'a str) -> Vec<&'a str> {
        let canonical = self.canonical(id);
        if !self.canonical.contains_key(id) {
            return vec![id];
        }
        self.canonical
            .iter()
            .filter_map(|(member, target)| (target == canonical).then_some(member.as_str()))
            .collect()
    }

    pub(crate) fn history_revision(&self, id: &str) -> Option<&str> {
        self.canonical
            .contains_key(id)
            .then_some(self.history_revision.as_deref())
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relearn::Impact;
    use brauser_protocol::{RecordKey, RecordKeyKind};
    fn identity(id: &str, kind: &str) -> Identity {
        Identity {
            artifact_type: kind.into(),
            artifact_id: id.into(),
        }
    }
    fn event(id: &str, from: Identity, to: Identity) -> Event {
        Event {
            event_id: id.into(),
            action: Action::Confirm,
            from,
            to,
        }
    }
    #[test]
    fn a_record_key_cannot_name_two_types_even_in_separate_components() {
        let one = event(
            "one",
            identity("A", "work.item"),
            identity("B", "work.item"),
        );
        let two = event(
            "two",
            identity("A", "other.item"),
            identity("C", "other.item"),
        );
        assert!(Resolution::from_events(&[one, two]).is_err());
    }
    #[test]
    fn simultaneous_swaps_chains_and_merges_are_refused_even_without_report_labels() {
        for pairs in [
            [("A", "B"), ("B", "A")],
            [("A", "B"), ("B", "C")],
            [("A", "C"), ("B", "C")],
        ] {
            let plan = Plan {
                review_only: true,
                origin: String::new(),
                config_revision: String::new(),
                contracts_revision: String::new(),
                evidence_revision: String::new(),
                proposed_revision: String::new(),
                plan_revision: String::new(),
                unowned_files: vec![],
                identities: pairs
                    .into_iter()
                    .map(|(from, to)| Impact {
                        source: RecordKey {
                            kind: RecordKeyKind::Artifact,
                            value: from.into(),
                        },
                        targets: BTreeSet::from([identity(to, "work.item")]),
                        classifications: BTreeSet::new(),
                        issues: BTreeSet::new(),
                        evidence_count: 1,
                        files: vec![],
                        destination_files: vec![],
                    })
                    .collect(),
            };
            assert!(Resolution::from_plan(&plan).is_err(), "{pairs:?}");
        }
    }
}

//! Read-only alias rehearsal for Re-learn (brwr-6x2). Nothing in this module
//! opens the configuration or vault. Input order is append order, not a page
//! timestamp: a move back must beat the earlier move, even with a skewed clock.
//! The future durable importer must supply that same stable order and verify
//! its own evidence; a successful CLI rehearsal is never migration authority.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufWriter, Read, Write};

use adapter_templates::{Document, Value};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use url::Url;

const MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_EVENTS: usize = 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    before: serde_json::Value,
    /// Omitted for moves within the same adapter policy. For re-learning,
    /// endpoints are evaluated under the old and proposed policies separately.
    after: Option<serde_json::Value>,
    events: Vec<InputEvent>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputEvent {
    event_id: String,
    action: Action,
    from: Endpoint,
    to: Endpoint,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    url: String,
    artifact_id: String,
    /// Missing means legacy unknown. It cannot prove identity under a query
    /// reader. An explicit empty list describes an unstripped input URL.
    stripped_params: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Confirm,
    Suggest,
    Unalias,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct Identity {
    artifact_type: String,
    artifact_id: String,
}

/// Endpoints here have already been derived by the host, including type and
/// scope. Native messages must never deserialize directly into this type.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Event {
    event_id: String,
    action: Action,
    from: Identity,
    to: Identity,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Group {
    canonical: Identity,
    members: Vec<Identity>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Suggestion {
    event_id: String,
    from: Identity,
    to: Identity,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Preview {
    /// This output describes only supplied evidence, never the entire vault.
    review_only: bool,
    groups: Vec<Group>,
    suggestions: Vec<Suggestion>,
}

/// Rebuild from the append-only operations, without mutating an earlier
/// projection. The latest confirmed operation for an unordered pair wins;
/// a suggestion cannot revoke a confirmation. Unalias removes that pair's
/// edge, not every path between its endpoints. Canonical identity is the
/// target of the most recent still-active confirmation in the component.
fn replay(events: &[Event]) -> Result<Preview> {
    let mut seen = BTreeMap::new();
    let mut identities = BTreeSet::new();
    let mut decisions = BTreeMap::new();
    let mut suggestions = BTreeMap::new();
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

fn compile(raw: &serde_json::Value) -> Result<Document> {
    let value: Value = serde_json::from_value(raw.clone()).context("invalid adapter document")?;
    Document::from_value(&value)
        .map_err(|error| anyhow::anyhow!("invalid adapter document: {error}"))
}

fn endpoint(document: &Document, raw: &serde_json::Value, input: &Endpoint) -> Result<Identity> {
    let url = Url::parse(&input.url).context("invalid endpoint URL")?;
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "credentialed endpoint URL"
    );
    let origin = url.origin().ascii_serialization();
    if let Some(names) = &input.stripped_params {
        ensure!(
            crate::url_provenance::valid_names(names),
            "invalid stripped-parameter evidence"
        );
    }
    let readers = raw["artifacts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|adapter| adapter["origin"].as_str() == Some(origin.as_str()))
        .flat_map(crate::config::query_keys_read);
    for (_, key) in readers {
        ensure!(
            input
                .stripped_params
                .as_ref()
                .is_some_and(|names| { !names.iter().any(|name| name.eq_ignore_ascii_case(key)) }),
            "insufficient stripped-parameter evidence for a query reader"
        );
    }
    let found = document
        .evaluate(&input.url, None)
        .map_err(|reason| anyhow::anyhow!("endpoint is not recognized: {reason}"))?;
    ensure!(
        input.artifact_id == found.artifact_id,
        "endpoint ID does not match host recognition"
    );
    Ok(Identity {
        artifact_type: found.artifact_type,
        artifact_id: found.artifact_id,
    })
}

fn preview(request: Request) -> Result<Preview> {
    ensure!(request.events.len() <= MAX_EVENTS, "too many alias events");
    let before = compile(&request.before)?;
    let after_raw = request.after.as_ref().unwrap_or(&request.before);
    let after = compile(after_raw)?;
    let events = request
        .events
        .into_iter()
        .enumerate()
        .map(|(index, input)| {
            let checked = || -> Result<Event> {
                Ok(Event {
                    event_id: crate::capture::parse_event_id(&input.event_id)
                        .map_err(anyhow::Error::msg)?,
                    action: input.action,
                    from: endpoint(&before, &request.before, &input.from)?,
                    to: endpoint(&after, after_raw, &input.to)?,
                })
            };
            checked().with_context(|| format!("event {}", index + 1))
        })
        .collect::<Result<Vec<_>>>()?;
    replay(&events)
}

pub(super) fn preview_lines(mut input: impl BufRead, output: impl Write) -> Result<()> {
    let mut output = BufWriter::new(output);
    let mut line = String::new();
    let mut number = 0;
    loop {
        line.clear();
        let bytes = (&mut input)
            .take(MAX_REQUEST_BYTES + 1)
            .read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        number += 1;
        if bytes as u64 > MAX_REQUEST_BYTES {
            bail!("request on line {number} is too large");
        }
        if line.trim().is_empty() {
            continue;
        }
        let request = serde_json::from_str(&line)
            .with_context(|| format!("request on line {number} is not valid"))?;
        let result = preview(request).with_context(|| format!("request on line {number}"))?;
        serde_json::to_writer(&mut output, &result)?;
        output.write_all(b"\n")?;
    }
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value as Json, json};

    fn identity(key: &str) -> Identity {
        Identity {
            artifact_type: "work.item".into(),
            artifact_id: format!("work:work.example/{key}"),
        }
    }

    fn event(number: u128, action: Action, from: &str, to: &str) -> Event {
        Event {
            event_id: uuid::Uuid::from_u128(number).to_string(),
            action,
            from: identity(from),
            to: identity(to),
        }
    }

    fn canonical(preview: &Preview, key: &str) -> String {
        preview
            .groups
            .iter()
            .find(|group| group.members.contains(&identity(key)))
            .unwrap()
            .canonical
            .artifact_id
            .clone()
    }

    #[test]
    fn a_chain_uses_the_latest_target_and_replaying_duplicate_ids_changes_nothing() {
        let a = event(1, Action::Confirm, "A", "B");
        let b = event(2, Action::Confirm, "B", "C");
        let original = replay(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(original.groups.len(), 1);
        assert_eq!(canonical(&original, "A"), identity("C").artifact_id);
        assert_eq!(original, replay(&[a.clone(), b.clone(), a, b]).unwrap());
    }

    #[test]
    fn a_move_back_uses_append_order_and_unalias_is_undirected() {
        let history = [
            event(3, Action::Confirm, "A", "B"),
            event(2, Action::Confirm, "B", "C"),
            event(1, Action::Confirm, "C", "A"),
        ];
        let result = replay(&history).unwrap();
        assert_eq!(canonical(&result, "C"), identity("A").artifact_id);
        let result = replay(&[
            event(1, Action::Confirm, "A", "B"),
            event(2, Action::Confirm, "B", "A"),
        ])
        .unwrap();
        assert_eq!(canonical(&result, "B"), identity("A").artifact_id);
        let result = replay(&[
            event(1, Action::Confirm, "A", "B"),
            event(2, Action::Confirm, "B", "C"),
            event(3, Action::Unalias, "B", "A"),
        ])
        .unwrap();
        assert_eq!(result.groups.len(), 2);
        assert_eq!(canonical(&result, "A"), identity("A").artifact_id);
        assert_eq!(canonical(&result, "B"), identity("C").artifact_id);
    }

    #[test]
    fn unalias_preserves_other_paths_and_reconfirmation_can_join_again() {
        let mut history = vec![
            event(1, Action::Confirm, "A", "B"),
            event(2, Action::Confirm, "B", "C"),
            event(3, Action::Confirm, "A", "C"),
            event(4, Action::Unalias, "A", "C"),
        ];
        assert_eq!(
            canonical(&replay(&history).unwrap(), "A"),
            identity("C").artifact_id
        );
        history.push(event(5, Action::Unalias, "B", "C"));
        assert_eq!(
            canonical(&replay(&history).unwrap(), "A"),
            identity("B").artifact_id
        );
        history.push(event(6, Action::Confirm, "C", "A"));
        assert_eq!(
            canonical(&replay(&history).unwrap(), "C"),
            identity("A").artifact_id
        );
    }

    #[test]
    fn suggestions_do_not_merge_or_change_the_canonical_target() {
        let mut history = vec![event(1, Action::Suggest, "A", "B")];
        let result = replay(&history).unwrap();
        assert_eq!(result.groups.len(), 2);
        assert_eq!(result.suggestions.len(), 1);
        history.push(event(2, Action::Confirm, "A", "B"));
        history.push(event(3, Action::Suggest, "B", "A"));
        let result = replay(&history).unwrap();
        assert!(result.suggestions.is_empty());
        assert_eq!(canonical(&result, "A"), identity("B").artifact_id);
        history.push(event(4, Action::Unalias, "A", "B"));
        assert!(replay(&history).unwrap().suggestions.is_empty());
        history.push(event(5, Action::Suggest, "B", "A"));
        assert_eq!(replay(&history).unwrap().suggestions.len(), 1);
    }

    #[test]
    fn rejects_conflicting_reuse_self_links_and_cross_type_links() {
        let a = event(1, Action::Confirm, "A", "B");
        let changed = event(1, Action::Unalias, "A", "B");
        assert!(
            replay(&[a.clone(), changed])
                .unwrap_err()
                .to_string()
                .contains("conflicting reuse")
        );
        assert!(replay(&[event(2, Action::Confirm, "A", "A")]).is_err());
        let mut changed = a;
        changed.to.artifact_type = "work.project".into();
        assert!(
            replay(&[changed])
                .unwrap_err()
                .to_string()
                .contains("different artifact types")
        );
    }

    #[test]
    fn long_chains_are_iterative_and_disconnected_groups_are_sorted() {
        let mut history = (1..=4096)
            .map(|n| event(n, Action::Confirm, &(n - 1).to_string(), &n.to_string()))
            .collect::<Vec<_>>();
        history.push(event(5000, Action::Confirm, "x", "y"));
        let result = replay(&history).unwrap();
        assert_eq!(result.groups.len(), 2);
        assert_eq!(canonical(&result, "0"), identity("4096").artifact_id);
        assert!(
            result
                .groups
                .windows(2)
                .all(|pair| pair[0].canonical < pair[1].canonical)
        );
    }

    fn doc(prefix: &str) -> Json {
        json!({"format": 1, "artifacts": [{
            "type": "work.item", "origin": "https://work.example",
            "match": [{"path": "/items/{id:int}"}],
            "id": format!("work:{{host}}/{prefix}{{id}}")
        }]})
    }

    fn request() -> Json {
        json!({"before": doc(""), "events": [{
            "event_id": uuid::Uuid::from_u128(1).to_string(), "action": "confirm",
            "from": {"url": "https://work.example/items/1", "artifact_id": "work:work.example/1", "stripped_params": []},
            "to": {"url": "https://work.example/items/2", "artifact_id": "work:work.example/2", "stripped_params": []}
        }]})
    }

    fn run(request: &Json) -> Result<Json> {
        let mut output = Vec::new();
        preview_lines(serde_json::to_string(request)?.as_bytes(), &mut output)?;
        Ok(serde_json::from_slice(&output)?)
    }

    #[test]
    fn re_learning_can_preview_one_url_under_old_and_new_documents() {
        let mut request = request();
        request["after"] = doc("new/");
        request["events"][0]["to"]["url"] = json!("https://work.example/items/1");
        request["events"][0]["to"]["artifact_id"] = json!("work:work.example/new/1");
        let result = run(&request).unwrap();
        assert_eq!(result["review_only"], true);
        assert_eq!(
            result["groups"][0]["canonical"]["artifact_id"],
            "work:work.example/new/1"
        );
        assert_eq!(result["groups"][0]["members"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn endpoint_claims_never_override_host_recognition() {
        let mut input = request();
        input["events"][0]["to"]["artifact_id"] = json!("work:work.example/999");
        assert!(
            format!("{:#}", run(&input).unwrap_err())
                .contains("ID does not match host recognition")
        );
        for url in [
            "https://other.example/items/2",
            "https://work.example/search",
            "blob:https://work.example/items/2",
            "https://user:password@work.example/items/2",
        ] {
            let mut input = request();
            input["events"][0]["to"]["url"] = json!(url);
            assert!(run(&input).is_err(), "{url}");
        }
        let mut input = request();
        input["before"]["artifacts"][0]["scope"] = json!({"id": ["1"]});
        assert!(format!("{:#}", run(&input).unwrap_err()).contains("out_of_scope"));
    }

    #[test]
    fn query_and_absence_readers_require_non_lossy_evidence() {
        for condition in [
            json!({"query": {"view": "detail"}}),
            json!({"absent": ["view"]}),
        ] {
            let mut input = request();
            for (key, value) in condition.as_object().unwrap() {
                input["before"]["artifacts"][0]["match"][0][key] = value.clone();
            }
            if condition.get("query").is_some() {
                for side in ["from", "to"] {
                    let current = input["events"][0][side]["url"].as_str().unwrap();
                    input["events"][0][side]["url"] = json!(format!("{current}?view=detail"));
                }
            }
            assert!(run(&input).is_ok());
            input["events"][0]["from"]["stripped_params"] = json!(["VIEW"]);
            assert!(
                format!("{:#}", run(&input).unwrap_err())
                    .contains("insufficient stripped-parameter evidence")
            );
            input["events"][0]["from"]["stripped_params"] = Json::Null;
            assert!(run(&input).is_err());
            input["events"][0]["from"]["stripped_params"] = json!(["utm_source"]);
            assert!(run(&input).is_ok());
        }
        let mut input = request();
        input["events"][0]["from"]["stripped_params"] = Json::Null;
        assert!(
            run(&input).is_ok(),
            "path-only identity does not need query evidence"
        );
        input["events"][0]["from"]["stripped_params"] = json!(["b", "a"]);
        assert!(run(&input).is_err());
    }

    #[test]
    fn malformed_requests_fail_with_a_line_and_never_echo_url_values() {
        let mut input = request();
        input["events"][0]["from"]["artifact_id"] = json!("forged");
        input["events"][0]["from"]["url"] =
            json!("https://work.example/items/1?token=private-value");
        let error = format!("{:#}", run(&input).unwrap_err());
        assert!(error.starts_with("request on line 1: event 1:"));
        assert!(!error.contains("private-value"));
        let mut output = Vec::new();
        assert!(preview_lines("\n{\"before\":{}}\n".as_bytes(), &mut output).is_err());
        assert!(output.is_empty());
        input = request();
        input["events"][0]["unexpected"] = json!(true);
        assert!(run(&input).is_err());
    }

    #[test]
    fn multi_request_replay_is_independent_and_empty_histories_are_valid() {
        let mut empty = request();
        empty["events"] = json!([]);
        let input = format!("{}\n\n{}\n", request(), empty);
        let mut output = Vec::new();
        preview_lines(input.as_bytes(), &mut output).unwrap();
        let lines = String::from_utf8(output).unwrap();
        let results = lines
            .lines()
            .map(|line| serde_json::from_str::<Json>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[1],
            json!({"review_only": true, "groups": [], "suggestions": []})
        );
    }

    #[test]
    fn oversized_requests_and_histories_are_refused_before_projection() {
        let mut input = request();
        input["events"] = Json::Array(vec![input["events"][0].clone(); MAX_EVENTS + 1]);
        assert!(format!("{:#}", run(&input).unwrap_err()).contains("too many alias events"));
        let bytes = vec![b' '; MAX_REQUEST_BYTES as usize + 1];
        let mut output = Vec::new();
        let error = preview_lines(bytes.as_slice(), &mut output).unwrap_err();
        assert!(error.to_string().contains("line 1 is too large"));
        assert!(output.is_empty());
    }
}

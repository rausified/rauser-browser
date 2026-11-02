//! The SQL store (plan T8): ingest and its conflict rules, per-group
//! re-derive, dirty generations, and the randomized replay against a full
//! derive.

use super::*;
use crate::index::store;
use rusqlite::Connection;

fn connection(cache: &TempDir) -> Connection {
    store::open(cache.path()).unwrap()
}

fn generations(index: &ArtifactIndex) -> BTreeMap<String, i64> {
    sql::sql_rows(
        &index.database_path(),
        "select artifact_id, generation from dirty_artifacts",
    )
    .into_iter()
    .map(|row| match row.as_slice() {
        [sql::Value::Text(id), sql::Value::Integer(generation)] => (id.clone(), *generation),
        other => panic!("{other:?}"),
    })
    .collect()
}

#[test]
fn a_delivery_stores_each_body_with_its_key_container_and_refs() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let document = github_document(true);
    let index = index(&root, &cache).with_adapters(Some(&document));
    let mut pull = github_pr(1, 7, "2026-10-15T12:00:00-07:00");
    pull.refs.push(ActivityRef {
        artifact_type: "jira.issue".into(),
        value: "PROJ-1".into(),
    });
    index.record_activity(&pull, &[]).unwrap();
    let rows = sql::sql_rows(
        &index.database_path(),
        "select event_id, artifact_id, kind, local_date, log_path, log_line, sequence,
           container_id, has_refs, body, hex(order_key), hex(body_sha256)
         from brauser_events",
    );
    let event = CachedEvent::activity(&pull, &[]).unwrap();
    let body = serde_json::to_string(&event).unwrap();
    let key = ord::order_key(event_time(&event), None, Some(0), &pull.event_id);
    use sql::Value::{Integer, Null, Text};
    assert_eq!(
        rows,
        [vec![
            Text(pull.event_id.clone()),
            Text(pull.artifact_id.clone()),
            Text("activity".into()),
            // The visit's own local day, as the fold dates it.
            Text("2026-10-15".into()),
            Null,
            Null,
            Integer(0),
            Text(GITHUB_REPO_ID.into()),
            Integer(1),
            Text(body.clone()),
            Text(hex::encode_upper(key)),
            Text(hex::encode_upper(Sha256::digest(body.as_bytes()))),
        ]]
    );
    // The PR's group and its container's both re-derived.
    assert_eq!(
        sql::sql_rows(
            &index.database_path(),
            "select artifact_id, focused_secs, own_focused_secs, opens, own_opens
             from brauser_artifact_days order by artifact_id"
        ),
        [
            vec![
                Text(pull.artifact_id.clone()),
                Integer(125),
                Integer(125),
                Integer(1),
                Integer(1)
            ],
            vec![
                Text(GITHUB_REPO_ID.into()),
                Integer(125),
                Integer(0),
                Integer(1),
                Integer(0)
            ],
        ]
    );
    assert_eq!(
        sql::sql_rows(
            &index.database_path(),
            "select artifact_id, rolled_up from brauser_group_state order by artifact_id"
        ),
        [
            vec![Text(pull.artifact_id.clone()), Integer(0)],
            vec![Text(GITHUB_REPO_ID.into()), Integer(1)],
        ]
    );
}

#[test]
fn a_delivery_re_derives_only_its_groups() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let document = github_document(true);
    let index = index(&root, &cache).with_adapters(Some(&document));
    let history = (1..=20)
        .map(|n| github_pr(n, n, "2026-10-14T12:00:00Z"))
        .collect::<Vec<_>>();
    index.record_activities(history.iter(), &[]).unwrap();
    let other = activity(
        "00000000-0000-4000-8000-000000000500",
        "OTHER",
        "Other",
        "2026-10-14T13:00:00Z",
    );
    index.record_activity(&other, &[]).unwrap();
    store::GROUP_DERIVES.with(|count| count.set(0));
    store::EVENTS_LOADED.with(|count| count.set(0));
    FULL_DERIVES.with(|count| count.set(0));
    index
        .record_activity(&github_pr(21, 3, "2026-10-15T12:00:00Z"), &[])
        .unwrap();
    assert_eq!(FULL_DERIVES.with(|count| count.get()), 0);
    // PR 3 and its repo; never the unrelated artifact.
    assert_eq!(store::GROUP_DERIVES.with(|count| count.get()), 2);
    // PR 3's two events, then the repo's 21 children.
    assert_eq!(store::EVENTS_LOADED.with(|count| count.get()), 2 + 21);
}

#[test]
fn a_conflicting_redelivery_keeps_the_first_body_and_its_rows() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let first = activity(
        "00000000-0000-4000-8000-000000000001",
        "ABC",
        "First",
        "2026-10-15T12:00:00Z",
    );
    index.record_activity(&first, &[]).unwrap();
    let mut reused = first.clone();
    reused.title = Some("Replaced".into());
    let fresh = activity(
        "00000000-0000-4000-8000-000000000002",
        "XYZ",
        "Fresh",
        "2026-10-15T13:00:00Z",
    );
    let error = index.record_activities([&reused, &fresh], &[]).unwrap_err();
    assert!(error.to_string().contains("cannot be reused"), "{error:#}");
    assert_eq!(
        sql::sql_rows(
            &index.database_path(),
            "select title from artifacts order by title"
        ),
        [["First"], ["Fresh"]].map(|row| row.map(|value| sql::Value::Text(value.into())).to_vec())
    );
    let body = sql::sql_rows(
        &index.database_path(),
        &format!(
            "select body from brauser_events where event_id = '{}'",
            first.event_id
        ),
    );
    assert!(sql::display(&body[0][0]).contains("\"First\""));
}

#[test]
fn every_mark_takes_a_newer_generation() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    let a = activity(
        "00000000-0000-4000-8000-000000000001",
        "A",
        "A",
        "2026-10-15T12:00:00Z",
    );
    let b = activity(
        "00000000-0000-4000-8000-000000000002",
        "B",
        "B",
        "2026-10-15T12:00:00Z",
    );
    index.record_activities([&a, &b], &[]).unwrap();
    let before = generations(&index);
    assert_eq!(before.len(), 2);
    // A same-body redelivery marks its artifact again, so a pass that
    // snapshotted the older generation cannot clear the new mark.
    index.record_activity(&a, &[]).unwrap();
    let after = generations(&index);
    assert!(after[&a.artifact_id] > before[&a.artifact_id]);
    assert!(after[&a.artifact_id] > before[&b.artifact_id]);
    assert_eq!(after[&b.artifact_id], before[&b.artifact_id]);
    // A completed pass clears both marks.
    index.regenerate_dirty().unwrap();
    assert!(generations(&index).is_empty());
}

#[test]
fn the_same_body_moves_to_an_earlier_position_only() {
    let cache = TempDir::new().unwrap();
    let mut conn = connection(&cache);
    let event: CachedEvent = EventData::Activity(activity(
        "00000000-0000-4000-8000-000000000001",
        "A",
        "A",
        "2026-10-15T12:00:00Z",
    ))
    .into();
    let at = |log: &str, line| Some((PathBuf::from("2026").join(log), line));
    let outcomes = store::write(&mut conn, None, |tx| {
        let mut containers = Containers::new(None);
        let mut outcomes = Vec::new();
        for position in [at("b.md", 5), at("a.md", 9), at("c.md", 1), None] {
            let (outcome, groups) = store::ingest(tx, &mut containers, &event, position.as_ref())?;
            store::rederive_groups(tx, None, &groups)?;
            outcomes.push(outcome);
        }
        let mut conflicting = event.clone();
        if let EventData::Activity(activity) = &mut conflicting.data {
            activity.title = Some("Other".into());
        }
        outcomes.push(store::ingest(tx, &mut containers, &conflicting, at("0.md", 0).as_ref())?.0);
        Ok(outcomes)
    })
    .unwrap();
    use store::Ingest::*;
    assert_eq!(outcomes, [Inserted, Moved, Same, Same, Conflict]);
    drop(conn);
    let db = cache.path().join("index.db");
    assert_eq!(
        sql::sql_rows(
            &db,
            "select log_path, log_line, sequence from brauser_events"
        ),
        [vec![
            sql::Value::Text("2026/a.md".into()),
            sql::Value::Integer(9),
            sql::Value::Integer(0)
        ]]
    );
    assert_eq!(
        sql::sql_rows(&db, "select title from artifacts"),
        [[sql::Value::Text("A".into())]]
    );
}

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

/// GitHub PRs and issues within their repo, the repo itself, Jira issues
/// (which resolve refs into mentions) and plain pages.
fn replay_document() -> adapter_templates::Document {
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
    adapter_templates::Document::from_value(&value).unwrap()
}

fn target(rng: &mut Rng) -> (String, String, String) {
    let repo = ["acme/widgets", "acme/gadgets"][rng.below(2) as usize];
    let n = rng.below(3) + 1;
    match rng.below(5) {
        0 | 1 => (
            "github.pr".into(),
            format!("github.pr:github.com/{repo}/{n}"),
            format!("https://github.com/{repo}/pull/{n}"),
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

/// Instants over a few days at a coarse step, so equal instants are common.
fn instant(rng: &mut Rng) -> String {
    let at = OffsetDateTime::UNIX_EPOCH
        + time::Duration::days(20_000)
        + time::Duration::minutes(rng.below(40) as i64 * 97);
    let offset = [0, 2, -5][rng.below(3) as usize];
    at.to_offset(UtcOffset::from_hms(offset, 0, 0).unwrap())
        .format(&Rfc3339)
        .unwrap()
}

/// One event for ID `n`. `variant` changes its title, so the same ID with
/// another variant is a conflicting body.
fn replay_event(n: u64, variant: u64) -> CachedEvent {
    let id = format!("00000000-0000-4000-8000-{n:012}");
    let mut seeded = Rng(n.wrapping_mul(0x5851_f42d_4c95_7f2d));
    let (artifact_type, artifact_id, url) = target(&mut seeded);
    let occurred_at = instant(&mut seeded);
    let data = match seeded.below(10) {
        0 => EventData::Why(WhyEvent {
            event_id: id,
            artifact_id,
            url,
            occurred_at,
            incognito: false,
            text: format!("why {n}.{variant}"),
        }),
        1 => EventData::Legacy(LegacyVisit {
            marker_id: id,
            artifact_id,
            occurred_at,
            url,
            title: format!("Legacy {n}.{variant}"),
        }),
        2 => EventData::Summary(SummaryEvent {
            event_id: id,
            artifact_id: artifact_id.clone(),
            artifact_type,
            url,
            title: format!("Summary {n}.{variant}"),
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
            let trails = (0..seeded.below(3))
                .map(|_| {
                    let (_, from, from_url) = target(&mut seeded);
                    ActivityTrail {
                        from_artifact_id: seeded.chance(80).then_some(from),
                        from_url: Some(from_url),
                        transition: ["link", "typed", "forward_back"][seeded.below(3) as usize]
                            .into(),
                    }
                })
                .collect();
            let refs = (0..seeded.below(3))
                .map(|_| ActivityRef {
                    artifact_type: ["jira.issue", "unknown.kind"][seeded.below(2) as usize].into(),
                    value: format!("ABC-{}", seeded.below(3) + 1),
                })
                .collect();
            EventData::Activity(ActivityEvent {
                event_id: id,
                artifact_id,
                artifact_type,
                url: url.clone(),
                title: seeded.chance(75).then(|| format!("Title {n}.{variant}")),
                occurred_at,
                incognito: false,
                opens: seeded.below(3) as u32,
                focused_seconds: seeded.below(600) as u32,
                background_seconds: seeded.below(600) as u32,
                modes: (0..seeded.below(2))
                    .map(|_| ActivityMode {
                        mode: ["view", "edit"][seeded.below(2) as usize].into(),
                        url: url.clone(),
                    })
                    .collect(),
                trails,
                refs,
            })
        }
    };
    data.into()
}

#[test]
fn random_replays_match_a_full_derive() {
    let document = replay_document();
    for seed in 0..48u64 {
        let mut rng = Rng(seed);
        let cache = TempDir::new().unwrap();
        let mut conn = connection(&cache);
        let mut model = IndexState {
            version: STATE_VERSION,
            root: String::new(),
            root_identity: String::new(),
            log_dir: String::new(),
            artifacts_dir: String::new(),
            events: BTreeMap::new(),
            gaps: BTreeMap::new(),
            gap_logs: BTreeMap::new(),
            positions: BTreeMap::new(),
            sequence: BTreeMap::new(),
            next_sequence: 0,
            adapters: String::new(),
            conflicts: BTreeMap::new(),
            dirty: BTreeSet::new(),
            logs: BTreeMap::new(),
            containers: BTreeSet::new(),
            container_files: BTreeMap::new(),
        };
        // Arrivals in shuffled order: new IDs, redeliveries of the same body
        // (some with an earlier log position), and conflicting bodies, in
        // batches of one transaction each.
        let ids = 4 + rng.below(36);
        for _ in 0..1 + rng.below(12) {
            let batch = (0..1 + rng.below(8))
                .map(|_| {
                    let n = rng.below(ids);
                    let variant = u64::from(rng.chance(15));
                    let position = rng.chance(60).then(|| {
                        let log = ["2024-10-01.md", "2024-10-02.md", "x/2024-10-01.md"]
                            [rng.below(3) as usize];
                        (PathBuf::from(log), rng.below(20))
                    });
                    (replay_event(n, variant), position)
                })
                .collect::<Vec<_>>();
            store::write(&mut conn, Some(&document), |tx| {
                let mut containers = Containers::new(Some(&document));
                let mut groups = BTreeSet::new();
                for (event, position) in &batch {
                    let (outcome, touched) =
                        store::ingest(tx, &mut containers, event, position.as_ref())?;
                    if matches!(outcome, store::Ingest::Inserted | store::Ingest::Moved) {
                        groups.extend(touched);
                    }
                }
                store::rederive_groups(tx, Some(&document), &groups)
            })
            .unwrap_or_else(|error| panic!("seed {seed}: {error:#}"));
            for (event, position) in batch {
                model.ingest(event, position, false);
            }
        }
        let stored = db::read_txn(&mut conn, store::oracle::dump).unwrap();
        assert_eq!(
            stored,
            store::oracle::state_rows(&model, Some(&document)),
            "seed {seed}: the store differs from a full derive of the same arrivals"
        );
        drop(conn);
    }
}

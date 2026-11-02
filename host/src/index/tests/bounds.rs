//! Year-scale per-delivery bounds (plan T17, the brwr-b2e acceptance).
//!
//! A year of real daily logs, larger than the 64 MiB ceiling the format 8
//! JSON cache had, is built with `reindex`; then single deliveries (a fresh
//! artifact, a long-lived one, an in-order child of a container with more
//! than 20k child events, and a backdated event for that child) each run
//! one bounded transaction: no full derive, no `index.db` replacement, no
//! artifact file write, and log reads bounded by the append.

use super::*;
use crate::index::store;

/// The year: 365 daily logs of `per_day` events each.
const DAYS: i64 = 365;
/// Slots 1 to `CONTAINER_PER_DAY - 1` of each day go to children of the
/// one big container (slot 0 is the long-lived page): 21,535 a year.
const CONTAINER_PER_DAY: u32 = 60;
/// Distinct PRs (children of the container) and distinct pages.
const PRS: u32 = 400;
const PAGES: u32 = 1_600;
/// Padding on each title, so the year's bodies outgrow the old JSON cache.
const TITLE_PADDING: usize = 760;
const LONG_LIVED_ID: &str = "work:example.com/LONG";
const CHILD_ID: &str = "github.pr:github.com/acme-corp/widgets/1";
const FRESH_ID: &str = "work:example.com/FRESH";
/// A delivery writes a handful of rows per group: its event, visit,
/// provenance, artifact, day, group state and dirty marker, plus the log
/// watermark and the meta counters. Nothing proportional to history.
const ROWS_WRITTEN_BOUND: u64 = 40;
/// The loose debug wall-clock ceilings. An in-order delivery is
/// O(batch); a backdated child event re-derives its container,
/// O(container) (open decision 4), so it is reported against its own.
/// The counter assertions are what prove the bound; wall time only
/// catches a gross regression.
#[cfg(not(windows))]
const IN_ORDER_CEILING: Duration = Duration::from_millis(500);
/// Windows CI runners pay far more per commit (fsync on a slow virtual
/// disk, Defender scanning each new `index.db-wal` page): a correctly
/// bounded fresh-artifact delivery (11 rows, one transaction) took
/// 521 ms there. Looser, but still far below an O(history) derive.
#[cfg(windows)]
const IN_ORDER_CEILING: Duration = Duration::from_secs(3);
const BACKDATED_CEILING: Duration = Duration::from_secs(30);

struct Year {
    root: TempDir,
    cache: TempDir,
    index: ArtifactIndex,
    /// The newest daily log, which today's deliveries append to.
    today_log: PathBuf,
    last_day: Date,
    next_event: u64,
}

fn event_id(n: u64) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn title(name: &str, day: i64) -> String {
    format!(
        "Ledger {name} day {day} {}",
        "lorem ".repeat(TITLE_PADDING / 6)
    )
}

fn page(n: u64, native_id: &str, day: i64, at: &str) -> ActivityEvent {
    activity(&event_id(n), native_id, &title(native_id, day), at)
}

fn pull(n: u64, number: u32, day: i64, at: &str) -> ActivityEvent {
    let mut event = github_activity(
        0,
        (
            "github.pr",
            &format!("github.pr:github.com/acme-corp/widgets/{number}"),
        ),
        &format!("https://github.com/acme-corp/widgets/pull/{number}"),
        at,
        (125, 61),
    );
    event.event_id = event_id(n);
    event.title = Some(title(&format!("PR {number}"), day));
    event
}

fn stamp(date: Date, minutes: u32) -> String {
    format!("{date}T{:02}:{:02}:00Z", 8 + minutes / 60, minutes % 60)
}

fn log_path(root: &Path, date: Date) -> PathBuf {
    root.join(format!(
        "log/{:04}/{:02}/{date}.md",
        date.year(),
        u8::from(date.month())
    ))
}

/// The year as real daily logs: one long-lived page and the long-lived PR
/// every day, `CONTAINER_PER_DAY` PR events, and the rest spread over the
/// pages. Then `reindex` builds `index.db` from them.
fn year(per_day: u32) -> Year {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let document = github_document(true);
    let index = index(&root, &cache).with_adapters(Some(&document));
    let first_day = parse_date("2025-11-02").unwrap();
    let mut n = 0_u64;
    let mut last = None;
    for day in 0..DAYS {
        let date = first_day + time::Duration::days(day);
        let mut markers = String::from(LOG_HEADER);
        markers.push('\n');
        for slot in 0..per_day {
            n += 1;
            let at = stamp(date, slot * 3);
            let event = match slot {
                0 => page(n, "LONG", day, &at),
                1 => pull(n, 1, day, &at),
                slot if slot < CONTAINER_PER_DAY => {
                    let number = 2 + (day as u32 * CONTAINER_PER_DAY + slot) % (PRS - 1);
                    pull(n, number, day, &at)
                }
                slot => {
                    let pages = per_day - CONTAINER_PER_DAY;
                    let number = (day as u32 * pages + slot) % PAGES;
                    page(n, &format!("P{number:04}"), day, &at)
                }
            };
            markers.push_str(&activity_marker(&event));
        }
        let path = log_path(root.path(), date);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, markers).unwrap();
        last = Some((path, date));
    }
    let (today_log, last_day) = last.unwrap();
    let started = Instant::now();
    let report = index.reindex().unwrap();
    eprintln!(
        "year-scale reindex of {n} events: {:?}, {report:?}",
        started.elapsed()
    );
    assert_eq!(report.conflicts + report.failed, 0, "{report:?}");
    Year {
        root,
        cache,
        index,
        today_log,
        last_day,
        next_event: n,
    }
}

/// The one-row, one-integer answer of `query`.
fn scalar(index: &ArtifactIndex, query: &str) -> i64 {
    match sql::sql_rows(&index.database_path(), query).as_slice() {
        [row] => match row.as_slice() {
            [sql::Value::Integer(value)] => *value,
            other => panic!("{query}: {other:?}"),
        },
        other => panic!("{query}: {other:?}"),
    }
}

/// What identifies `index.db` as the same file: a replacement (a rename
/// over it, or a delete and re-create) changes it.
#[cfg(unix)]
type FileIdentity = (u64, u64);
#[cfg(windows)]
type FileIdentity = (u64, u64);

#[cfg(unix)]
fn file_identity(path: &Path) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}

/// The volume serial number and file index: std's by-handle accessors are
/// unstable, so winapi-util wraps GetFileInformationByHandle. Creation
/// time would not do, as NTFS name tunneling gives a file renamed or
/// created into a just-vacated name the old file's creation time.
#[cfg(windows)]
fn file_identity(path: &Path) -> FileIdentity {
    let file = fs::File::open(path).unwrap();
    let info = winapi_util::file::information(&file).unwrap();
    (info.volume_serial_number(), info.file_index())
}

type Tree = BTreeMap<PathBuf, (u64, SystemTime, FileIdentity)>;

/// Every file under `artifacts/`, with what a write would change: its
/// size and modification time, and its identity (an owned write renames a
/// new file into place).
fn artifact_tree(root: &Path) -> Tree {
    fn walk(path: &Path, output: &mut Tree) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                walk(&path, output);
            } else {
                let stamp = (
                    metadata.len(),
                    metadata.modified().unwrap(),
                    file_identity(&path),
                );
                output.insert(path, stamp);
            }
        }
    }
    let mut output = BTreeMap::new();
    walk(&root.join("artifacts"), &mut output);
    output
}

#[derive(Debug)]
struct Delivery {
    fast_paths: u64,
    group_derives: u64,
    events_loaded: u64,
    full_derives: u64,
    rows_written: u64,
    transactions: u64,
    log_bytes_read: u64,
    appended: u64,
    elapsed: Duration,
}

fn reset_counters() {
    store::FAST_PATHS.with(|count| count.set(0));
    store::GROUP_DERIVES.with(|count| count.set(0));
    store::EVENTS_LOADED.with(|count| count.set(0));
    FULL_DERIVES.with(|count| count.set(0));
    ROWS_WRITTEN.with(|count| count.set(0));
    TRANSACTIONS.with(|count| count.set(0));
    LOG_BYTES_READ.with(|count| count.set(0));
}

/// Deliver `event` as the native host does: append its marker to today's
/// log, then index it. Checks what every delivery must leave alone.
fn deliver(year: &mut Year, event: &ActivityEvent) -> Delivery {
    let database = year.index.database_path();
    let identity = file_identity(&database);
    let files = artifact_tree(year.root.path());
    let marker = activity_marker(event);
    StdOpenOptions::new()
        .append(true)
        .open(&year.today_log)
        .unwrap()
        .write_all(marker.as_bytes())
        .unwrap();
    reset_counters();
    let started = Instant::now();
    year.index.record_activity(event, &[]).unwrap();
    let delivery = Delivery {
        elapsed: started.elapsed(),
        fast_paths: store::FAST_PATHS.with(|count| count.get()),
        group_derives: store::GROUP_DERIVES.with(|count| count.get()),
        events_loaded: store::EVENTS_LOADED.with(|count| count.get()),
        full_derives: FULL_DERIVES.with(|count| count.get()),
        rows_written: ROWS_WRITTEN.with(|count| count.get()),
        transactions: TRANSACTIONS.with(|count| count.get()),
        log_bytes_read: LOG_BYTES_READ.with(|count| count.get()),
        appended: marker.len() as u64,
    };
    eprintln!("{}: {delivery:?}", event.artifact_id);
    assert_eq!(file_identity(&database), identity, "index.db was replaced");
    assert!(
        artifact_tree(year.root.path()) == files,
        "a delivery touched a file under artifacts/"
    );
    assert_eq!(delivery.full_derives, 0, "{delivery:?}");
    // The delivery's own, and the empty one `db::open` takes to check the
    // schema of an existing file.
    assert!(delivery.transactions <= 2, "{delivery:?}");
    // Only the append, plus the tail window that guards the watermark.
    assert!(
        delivery.log_bytes_read <= delivery.appended + catchup::TAIL_WINDOW,
        "{delivery:?}"
    );
    delivery
}

/// One in-order delivery of `event`: the fast path for each of its
/// `groups` that has a row, a re-derive of only the new event otherwise.
fn assert_in_order(delivery: &Delivery, fast_paths: u64, ceiling: Duration) {
    assert_eq!(delivery.fast_paths, fast_paths, "{delivery:?}");
    // At most the batch (one event) is read back from the store.
    assert!(delivery.events_loaded <= 1, "{delivery:?}");
    assert!(delivery.rows_written <= ROWS_WRITTEN_BOUND, "{delivery:?}");
    assert!(
        delivery.elapsed <= ceiling,
        "an in-order delivery took {:?}: {delivery:?}",
        delivery.elapsed
    );
}

fn run_year(per_day: u32, in_order_ceiling: Duration, backdated_ceiling: Duration) {
    // The full-derive oracle after every write would make each delivery
    // O(history); `incremental_equals_reindex` and the replay tests keep
    // the fold exact at small scale.
    let oracle = store::oracle::off();
    let mut year = year(per_day);
    let index = year.index.clone();
    let events = scalar(&index, "SELECT count(*) FROM brauser_events");
    assert_eq!(events, DAYS * i64::from(per_day));
    // The bodies alone exceed the old serialized state's ceiling: the
    // format 8 `index-state.json` held each of them, and more.
    let bodies = scalar(&index, "SELECT sum(length(body)) FROM brauser_events");
    assert!(bodies > MAX_STATE_BYTES as i64, "bodies are {bodies} bytes");
    let artifacts = scalar(&index, "SELECT count(*) FROM artifacts");
    assert!(
        (1_900..=2_100).contains(&artifacts),
        "{artifacts} artifacts"
    );
    let children = scalar(
        &index,
        &format!("SELECT count(*) FROM brauser_events WHERE container_id = '{GITHUB_REPO_ID}'"),
    );
    assert!(children > 20_000, "{children} child events");
    let child_events = scalar(
        &index,
        &format!("SELECT count(*) FROM brauser_events WHERE artifact_id = '{CHILD_ID}'"),
    );
    // Reindex regenerated every file; deliveries leave them all as written.
    assert!(artifact_tree(year.root.path()).len() as i64 >= artifacts);
    assert_no_json_cache(year.cache.path());

    let today = year.last_day;
    let mut next = || {
        year.next_event += 1;
        year.next_event
    };
    let (fresh, long_lived, child, backdated) = (next(), next(), next(), next());
    let late = stamp(today, 14 * 60);
    // The panel reads before it delivers; the first open after the
    // rebuild's WAL checkpoint is not a delivery's cost.
    assert!(index.artifact(LONG_LIVED_ID).unwrap().is_some());

    // A fresh artifact: no row to extend, so its one event is re-derived.
    let delivery = deliver(&mut year, &page(fresh, "FRESH", DAYS, &late));
    assert_in_order(&delivery, 0, in_order_ceiling);
    assert_eq!(delivery.group_derives, 1, "{delivery:?}");

    // A long-lived artifact, seen every day of the year: the fast path.
    let delivery = deliver(&mut year, &page(long_lived, "LONG", DAYS, &late));
    assert_in_order(&delivery, 1, in_order_ceiling);
    assert_eq!(delivery.group_derives, 0, "{delivery:?}");

    // In order to a child of the big container: both groups take the
    // fast path, and neither reads the container's history.
    let delivery = deliver(&mut year, &pull(child, 1, DAYS, &late));
    assert_in_order(&delivery, 2, in_order_ceiling);
    assert_eq!(delivery.group_derives, 0, "{delivery:?}");

    // Backdated to that child: the child and its container re-derive,
    // reading the child's events and the container's children only.
    let early = stamp(today - time::Duration::days(200), 1);
    let delivery = deliver(&mut year, &pull(backdated, 1, DAYS - 200, &early));
    assert_eq!(delivery.fast_paths, 0, "{delivery:?}");
    assert_eq!(delivery.group_derives, 2, "{delivery:?}");
    assert!(
        delivery.events_loaded <= (child_events + 2 + children + 2) as u64,
        "{delivery:?}"
    );
    // It rewrites only the rows the two groups own: each deleted and
    // inserted once, never the rest of the year.
    let owned = scalar(
        &index,
        &format!(
            "SELECT (SELECT count(*) FROM brauser_artifact_days WHERE artifact_id IN {groups})
                  + (SELECT count(*) FROM visits WHERE artifact_id IN {groups})
                  + (SELECT count(*) FROM url_provenance WHERE artifact_id IN {groups})
                  + (SELECT count(*) FROM edges
                       WHERE source_artifact_id IN {groups} OR target_artifact_id IN {groups})
                  + 2 * (SELECT count(*) FROM artifacts WHERE artifact_id IN {groups})",
            groups = format_args!("('{CHILD_ID}', '{GITHUB_REPO_ID}')"),
        ),
    );
    assert!(
        delivery.rows_written <= 2 * owned as u64 + ROWS_WRITTEN_BOUND,
        "{owned} owned rows: {delivery:?}"
    );
    eprintln!(
        "backdated child of a {children}-event container: {:?} (ceiling {backdated_ceiling:?}), \
         {} rows written for {owned} owned",
        delivery.elapsed, delivery.rows_written
    );
    assert!(
        delivery.elapsed <= backdated_ceiling,
        "a backdated container delivery took {:?}",
        delivery.elapsed
    );

    // Recent, the worklog and the card all answer over the year.
    let (items, _, truncated) = index.recent(None, Some("ledger"), 20).unwrap();
    assert!(truncated);
    assert_eq!(items.len(), 20);
    let today_text = today.to_string();
    assert!(items.iter().all(|item| item.local_date == today_text));
    assert!(items.iter().any(|item| item.artifact_id == LONG_LIVED_ID));
    let (items, _, _) = index.recent(None, Some("ledger fresh"), 20).unwrap();
    let found = items
        .iter()
        .map(|item| item.artifact_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(found, [FRESH_ID]);
    let (text, count, _, _) = index
        .worklog_report(today - time::Duration::days(6), today)
        .unwrap();
    assert!(count > 0 && text.contains("Ledger"), "{count}: {text}");
    let card = index.artifact(LONG_LIVED_ID).unwrap().unwrap();
    // Today already had its daily open, so the delivery adds no day.
    assert_eq!(card.days_opened, DAYS as u32);
    assert_eq!(card.opens, DAYS as u32 + 1);
    let card = index.artifact(CHILD_ID).unwrap().unwrap();
    assert_eq!(card.within.unwrap().artifact_id, GITHUB_REPO_ID);
    assert_eq!(card.opens, child_events as u32 + 2);
    let card = index.artifact(GITHUB_REPO_ID).unwrap().unwrap();
    assert_eq!(card.opens, children as u32 + 2);
    assert!(!card.contained.is_empty());
    assert!(index.artifact(FRESH_ID).unwrap().is_some());

    // Once at the end, the bounded writes equal a full derive at scale.
    drop(oracle);
    let mut conn = db::open(year.cache.path(), db::Mode::IfExists)
        .unwrap()
        .unwrap();
    db::read_txn(&mut conn, |tx| {
        store::oracle::check(tx, index.adapters.as_deref())
    })
    .unwrap();
}

#[test]
fn a_year_of_history_keeps_every_delivery_bounded() {
    run_year(200, IN_ORDER_CEILING, BACKDATED_CEILING);
}

/// The 100k-event variant, for a release build on one CI OS:
/// `cargo test --locked --release -p brauser --lib index::tests::bounds -- --ignored`.
#[test]
#[ignore = "release-only 100k-event variant; run with --release -- --ignored"]
fn a_100k_event_year_keeps_every_delivery_bounded() {
    run_year(274, IN_ORDER_CEILING, BACKDATED_CEILING);
}

/// The year-scale test measures deliveries, which the oracle's full derive
/// after every write would dominate, so it turns the oracle off on its
/// thread. Off means off, and dropping the guard turns it back on.
#[test]
fn the_oracle_is_off_only_while_its_guard_lives() {
    let root = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let index = index(&root, &cache);
    index
        .record_activity(&page(1, "A", 0, "2026-10-15T12:00:00Z"), &[])
        .unwrap();
    let corrupt = |opens: i64| {
        rusqlite::Connection::open(index.database_path())
            .unwrap()
            .execute("UPDATE artifacts SET opens = ?1", [opens])
            .unwrap();
    };
    corrupt(99);
    let guard = store::oracle::off();
    index
        .record_activity(&page(2, "B", 0, "2026-10-15T13:00:00Z"), &[])
        .unwrap();
    drop(guard);
    corrupt(98);
    let checked = std::panic::catch_unwind(|| {
        index.record_activity(&page(3, "C", 0, "2026-10-15T14:00:00Z"), &[])
    });
    assert!(checked.is_err(), "the oracle stayed off: {checked:?}");
}

//! Recovery on start and retention of run records (spec §5.2).
//!
//! - [`recover`] runs once when the service starts. A record still marked
//!   `running` belongs to a service process that is gone, so it becomes
//!   `failed`/`service_restarted`. Its process group is killed first, but only
//!   when the heartbeat is recent enough that the group number cannot have
//!   been reused (after a reboot or a long gap it may name someone else).
//! - [`prune`] keeps run directories for [`KEEP_DAYS`] days or the newest
//!   [`KEEP_NEWEST`], whichever is more. Queued and running runs are never
//!   removed.

use std::time::Duration;

use brauser_protocol::{RunErrorKind, RunState};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use super::record::{RunStatusFile, RunStore};

/// Finished runs younger than this are always kept.
pub const KEEP_DAYS: i64 = 30;
/// The newest runs are always kept, however old.
pub const KEEP_NEWEST: usize = 200;
/// A running record's process group is killed on recovery only when its
/// heartbeat is at most this old.
pub const ORPHAN_KILL_WINDOW: Duration = Duration::from_secs(60);
/// The error a recovered run records.
pub const SERVICE_RESTARTED: &str = "service restarted";

/// Recover from a service process that ended with runs in flight. Every
/// `running` record becomes `failed`/`service_restarted`; its process group is
/// killed first only when all of these hold:
///
/// - the recorded `service_pid` is not alive or is not `own_pid`;
/// - `heartbeat_at` is within [`ORPHAN_KILL_WINDOW`] of `now` (an old
///   heartbeat means a reboot or a long gap, when the number may have been
///   reused);
/// - `alive(group)` is true.
///
/// Returns the `queued` runs, oldest first by `created_at`, for the caller to
/// re-submit.
pub fn recover(
    store: &RunStore,
    now: OffsetDateTime,
    own_pid: u32,
    alive: &dyn Fn(i32) -> bool,
    kill: &dyn Fn(i32),
) -> Vec<Uuid> {
    let mut queued = Vec::new();
    for mut status in store.list() {
        let Ok(id) = Uuid::parse_str(&status.run_id) else {
            continue;
        };
        match status.state {
            RunState::Queued => queued.push(id),
            RunState::Running => {
                if let Some(group) = orphan_group(&status, now, own_pid, alive) {
                    kill(group);
                }
                status.state = RunState::Failed;
                status.error_kind = Some(RunErrorKind::ServiceRestarted);
                status.error = Some(SERVICE_RESTARTED.to_string());
                status.finished_at = Some(super::supervisor::timestamp(now));
                if let Err(error) = store.write_status(&status) {
                    eprintln!("brauser service: recovering run {id}: {error:#}");
                }
            }
            RunState::Succeeded | RunState::Failed | RunState::Canceled => {}
        }
    }
    queued
}

/// The process group of an interrupted run that is safe to kill, if any.
fn orphan_group(
    status: &RunStatusFile,
    now: OffsetDateTime,
    own_pid: u32,
    alive: &dyn Fn(i32) -> bool,
) -> Option<i32> {
    // Never signal init or "every process" (0 or negative).
    let group = status.process_group.filter(|group| *group > 1)?;
    let owner_gone = match status.service_pid {
        Some(pid) => pid != own_pid || i32::try_from(pid).map_or(true, |pid| !alive(pid)),
        None => true,
    };
    if !owner_gone {
        return None;
    }
    let heartbeat = parse(status.heartbeat_at.as_deref()?)?;
    let age = (now - heartbeat).abs();
    if age > time::Duration::try_from(ORPHAN_KILL_WINDOW).ok()? {
        return None;
    }
    alive(group).then_some(group)
}

/// Delete finished runs that are both older than [`KEEP_DAYS`] (by
/// `finished_at`, else `created_at`) and outside the newest [`KEEP_NEWEST`]
/// runs. Queued and running runs, and runs whose age cannot be read, are kept.
/// Returns how many were removed.
pub fn prune(store: &RunStore, now: OffsetDateTime) -> usize {
    prune_with(store, now, |_, remove| remove())
}

/// [`prune`], with each removal passed through `guarded`, which runs the
/// removal it is given (returning its result) or skips it (returning false).
pub fn prune_with(
    store: &RunStore,
    now: OffsetDateTime,
    mut guarded: impl FnMut(Uuid, &mut dyn FnMut() -> bool) -> bool,
) -> usize {
    let runs = store.list();
    let outside = runs.len().saturating_sub(KEEP_NEWEST);
    let cutoff = now - time::Duration::days(KEEP_DAYS);
    let mut removed = 0;
    for status in &runs[..outside] {
        if matches!(status.state, RunState::Queued | RunState::Running) {
            continue;
        }
        let ended = status.finished_at.as_deref().unwrap_or(&status.created_at);
        if !parse(ended).is_some_and(|ended| ended < cutoff) {
            continue;
        }
        let Ok(id) = Uuid::parse_str(&status.run_id) else {
            continue;
        };
        let mut remove = || match store.remove(id) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("brauser service: pruning run {id}: {error:#}");
                false
            }
        };
        if guarded(id, &mut remove) {
            removed += 1;
        }
    }
    removed
}

fn parse(stamp: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(stamp, &Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::process::Command;
    use std::thread;
    use std::time::Instant;

    use brauser_protocol::{HarnessId, RecordKey, RecordKeyKind};

    use crate::service::harness_api::ProcessSlot;
    use crate::service::supervisor::timestamp;

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-10-26T12:00:00Z", &Rfc3339).unwrap()
    }

    fn ago(seconds: i64) -> OffsetDateTime {
        now() - time::Duration::seconds(seconds)
    }

    fn store() -> (tempfile::TempDir, RunStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = RunStore::open(dir.path().join("runs")).unwrap();
        (dir, store)
    }

    fn record(state: RunState, created: OffsetDateTime) -> RunStatusFile {
        let id = Uuid::new_v4();
        let finished = !matches!(state, RunState::Queued | RunState::Running);
        RunStatusFile {
            version: 1,
            run_id: id.to_string(),
            state,
            harness: HarnessId::Codex,
            service_pid: Some(1_000_000),
            process_group: None,
            heartbeat_at: None,
            thread_id: None,
            session_id: None,
            cwd: Some("/tmp".to_string()),
            start_key: RecordKey {
                kind: RecordKeyKind::Url,
                value: format!("https://example.com/{id}"),
            },
            written_key: None,
            url: format!("https://example.com/{id}"),
            title: "Page".to_string(),
            requested_at: timestamp(created),
            created_at: timestamp(created),
            started_at: None,
            finished_at: finished.then(|| timestamp(created)),
            error: None,
            error_kind: None,
            summary_path: None,
            stream_truncated: false,
            run_timeout_secs: None,
            vocabulary: None,
            answered_at: None,
            save_final: false,
        }
    }

    fn add(store: &RunStore, status: &RunStatusFile) -> Uuid {
        store.create(status, "# Page\n").unwrap();
        Uuid::parse_str(&status.run_id).unwrap()
    }

    fn running(group: i32, heartbeat: OffsetDateTime) -> RunStatusFile {
        let mut status = record(RunState::Running, heartbeat - time::Duration::seconds(30));
        status.process_group = Some(group);
        status.heartbeat_at = Some(timestamp(heartbeat));
        status.started_at = Some(status.created_at.clone());
        status
    }

    fn assert_restarted(store: &RunStore, id: Uuid) {
        let status = store.status(id).unwrap();
        assert_eq!(status.state, RunState::Failed);
        assert_eq!(status.error_kind, Some(RunErrorKind::ServiceRestarted));
        assert_eq!(status.error.as_deref(), Some(SERVICE_RESTARTED));
        assert!(status.finished_at.is_some());
    }

    #[test]
    fn running_record_with_dead_group_becomes_service_restarted() {
        let (_dir, store) = store();
        let id = add(&store, &running(4242, ago(5)));
        let kills = RefCell::new(Vec::new());
        let queued = recover(&store, now(), std::process::id(), &|_| false, &|group| {
            kills.borrow_mut().push(group)
        });
        assert!(queued.is_empty());
        assert!(kills.borrow().is_empty());
        assert_restarted(&store, id);
    }

    #[test]
    fn running_record_with_live_group_and_fresh_heartbeat_is_killed_then_failed() {
        let (_dir, store) = store();
        let slot = ProcessSlot::default();
        let mut child = slot.spawn(Command::new("/bin/sleep").arg("30")).unwrap();
        let group = slot.group().unwrap();
        let heartbeat = OffsetDateTime::now_utc() - time::Duration::seconds(5);
        let id = add(&store, &running(group, heartbeat));

        let alive = |group: i32| {
            rustix::process::Pid::from_raw(group)
                .is_some_and(|pid| rustix::process::test_kill_process_group(pid).is_ok())
        };
        let kill = |group: i32| {
            if let Some(pid) = rustix::process::Pid::from_raw(group) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
        };
        recover(
            &store,
            OffsetDateTime::now_utc(),
            std::process::id(),
            &alive,
            &kill,
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "orphaned group {group} survived");
            thread::sleep(Duration::from_millis(20));
        }
        assert_restarted(&store, id);
    }

    #[test]
    fn stale_heartbeat_group_is_never_killed() {
        let (_dir, store) = store();
        let id = add(&store, &running(4242, ago(600)));
        let kills = RefCell::new(Vec::new());
        recover(&store, now(), std::process::id(), &|_| true, &|group| {
            kills.borrow_mut().push(group)
        });
        assert!(kills.borrow().is_empty());
        assert_restarted(&store, id);
    }

    #[test]
    fn group_of_this_service_process_is_not_killed() {
        // A record naming this very process (alive) is not an orphan.
        let (_dir, store) = store();
        let mut status = running(4242, ago(5));
        status.service_pid = Some(std::process::id());
        let id = add(&store, &status);
        let kills = RefCell::new(Vec::new());
        recover(&store, now(), std::process::id(), &|_| true, &|group| {
            kills.borrow_mut().push(group)
        });
        assert!(kills.borrow().is_empty());
        assert_restarted(&store, id);
    }

    #[test]
    fn queued_records_are_returned_oldest_first() {
        let (_dir, store) = store();
        let newer = add(&store, &record(RunState::Queued, ago(10)));
        let older = add(&store, &record(RunState::Queued, ago(100)));
        let done = add(&store, &record(RunState::Succeeded, ago(50)));
        let queued = recover(&store, now(), std::process::id(), &|_| false, &|_| {});
        assert_eq!(queued, vec![older, newer]);
        assert_eq!(store.status(older).unwrap().state, RunState::Queued);
        assert_eq!(store.status(done).unwrap().state, RunState::Succeeded);
    }

    const DAY: i64 = 24 * 60 * 60;

    #[test]
    fn prune_keeps_newest_200_of_old_runs() {
        let (_dir, store) = store();
        let ids: Vec<Uuid> = (0..KEEP_NEWEST as i64 + 5)
            .map(|n| {
                add(
                    &store,
                    &record(RunState::Succeeded, ago(40 * DAY + 1000 - n)),
                )
            })
            .collect();
        assert_eq!(prune(&store, now()), 5);
        for id in &ids[..5] {
            assert!(!store.run_dir(*id).exists(), "oldest run {id} kept");
        }
        for id in &ids[5..] {
            assert!(store.run_dir(*id).exists(), "newer run {id} removed");
        }
        assert_eq!(prune(&store, now()), 0);
    }

    #[test]
    fn prune_keeps_all_recent_runs_beyond_200() {
        let (_dir, store) = store();
        for n in 0..KEEP_NEWEST as i64 + 10 {
            add(&store, &record(RunState::Failed, ago(29 * DAY - n)));
        }
        assert_eq!(prune(&store, now()), 0);
        assert_eq!(store.list().len(), KEEP_NEWEST + 10);
    }

    #[test]
    fn prune_never_removes_running_or_queued() {
        let (_dir, store) = store();
        let running = add(&store, &record(RunState::Running, ago(90 * DAY)));
        let queued = add(&store, &record(RunState::Queued, ago(90 * DAY - 1)));
        let canceled = add(&store, &record(RunState::Canceled, ago(90 * DAY - 2)));
        for n in 0..KEEP_NEWEST as i64 {
            add(&store, &record(RunState::Succeeded, ago(40 * DAY - n)));
        }
        assert_eq!(prune(&store, now()), 1);
        assert!(store.run_dir(running).exists());
        assert!(store.run_dir(queued).exists());
        assert!(!store.run_dir(canceled).exists());
    }
}

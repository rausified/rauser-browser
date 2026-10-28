//! The run queue: at most [`MAX_RUNNING`] runs execute at once, the rest wait
//! FIFO, and each run kind and record key has at most one queued-or-running
//! run. A site-draft run (keyed by its origin) and a summary run never share
//! a key, whatever their values.
//!
//! This is a pure state machine. The supervisor owns the threads and calls
//! [`Scheduler::next_ready`] whenever a slot may have opened.

use std::collections::{HashMap, VecDeque};

use brauser_protocol::{RecordKey, RecordKeyKind, RunKind};
use uuid::Uuid;

/// How many runs may execute at the same time.
pub const MAX_RUNNING: usize = 2;

/// The result of [`Scheduler::submit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submit {
    /// The run was queued under its key.
    New,
    /// A run for the same record key is already queued or running.
    Existing(Uuid),
}

/// A hashable identity for a run kind and [`RecordKey`]. The generated
/// protocol types do not derive `Hash`, so each kind is mapped to a small
/// tag.
type KeyId = (u8, u8, String);

fn key_id(run: RunKind, key: &RecordKey) -> KeyId {
    let run = match run {
        RunKind::Summary => 0,
        RunKind::SiteDraft => 1,
    };
    let kind = match key.kind {
        RecordKeyKind::Url => 0,
        RecordKeyKind::Artifact => 1,
    };
    (run, kind, key.value.clone())
}

#[derive(Debug, Default)]
pub struct Scheduler {
    running: Vec<Uuid>,
    waiting: VecDeque<Uuid>,
    by_key: HashMap<KeyId, Uuid>,
    /// Keys held by runs recovery could not resume (no working directory,
    /// or a second queued run for one key): they were never scheduled, but
    /// until their end state is recorded their key must stay taken.
    held: HashMap<Uuid, KeyId>,
}

impl Scheduler {
    /// Queues `id` under `run` and `key` unless they already have a queued or
    /// running run, in which case the existing run's id is returned and
    /// nothing changes.
    pub fn submit(&mut self, run: RunKind, key: &RecordKey, id: Uuid) -> Submit {
        let key = key_id(run, key);
        if let Some(existing) = self.by_key.get(&key) {
            return Submit::Existing(*existing);
        }
        if let Some((holder, _)) = self.held.iter().find(|(_, held)| **held == key) {
            return Submit::Existing(*holder);
        }
        self.by_key.insert(key, id);
        self.waiting.push_back(id);
        Submit::New
    }

    /// Takes `key` for `id`, a run recovery could not resume, until
    /// [`Scheduler::finished`] frees it. It takes no slot.
    pub fn hold(&mut self, run: RunKind, key: &RecordKey, id: Uuid) {
        self.held.insert(id, key_id(run, key));
    }

    /// Moves the oldest waiting run to running, if a slot is free.
    pub fn next_ready(&mut self) -> Option<Uuid> {
        if self.running.len() >= MAX_RUNNING {
            return None;
        }
        let id = self.waiting.pop_front()?;
        self.running.push(id);
        Some(id)
    }

    /// Records that a run has ended (in any state), freeing its slot and key.
    pub fn finished(&mut self, id: Uuid) {
        self.release(id);
        self.free_key(id);
    }

    /// Frees a run's slot but keeps its key taken, for a run that has ended
    /// but whose end state is not yet recorded. [`Scheduler::finished`]
    /// frees the key later. A released run does not count as busy.
    pub fn release(&mut self, id: Uuid) {
        self.running.retain(|running| *running != id);
        self.waiting.retain(|waiting| *waiting != id);
    }

    /// Removes a run that is still waiting. Returns false when the run is not
    /// waiting (it is running, finished, or unknown), and changes nothing then.
    pub fn dequeue(&mut self, id: Uuid) -> bool {
        let Some(position) = self.waiting.iter().position(|waiting| *waiting == id) else {
            return false;
        };
        self.waiting.remove(position);
        self.free_key(id);
        true
    }

    /// True when `id` is waiting for a slot.
    pub fn is_waiting(&self, id: Uuid) -> bool {
        self.waiting.contains(&id)
    }

    /// True when nothing is queued or running.
    pub fn is_idle(&self) -> bool {
        self.running.is_empty() && self.waiting.is_empty()
    }

    /// How many runs wait for a slot.
    pub fn waiting_len(&self) -> usize {
        self.waiting.len()
    }

    /// How many runs hold a slot.
    pub fn running_len(&self) -> usize {
        self.running.len()
    }

    fn free_key(&mut self, id: Uuid) {
        self.by_key.retain(|_, owner| *owner != id);
        self.held.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMMARY: RunKind = RunKind::Summary;

    fn url(value: &str) -> RecordKey {
        RecordKey {
            kind: RecordKeyKind::Url,
            value: value.to_owned(),
        }
    }

    fn artifact(value: &str) -> RecordKey {
        RecordKey {
            kind: RecordKeyKind::Artifact,
            value: value.to_owned(),
        }
    }

    #[test]
    fn two_run_at_once_rest_fifo_one_per_key() {
        let (k1, k2, k3) = (url("https://a/"), url("https://b/"), url("https://c/"));
        let [a, b, c, d, e] = [(); 5].map(|()| Uuid::new_v4());
        let mut s = Scheduler::default();
        assert!(matches!(s.submit(SUMMARY, &k1, a), Submit::New));
        assert_eq!(s.submit(SUMMARY, &k2, b), Submit::New);
        assert_eq!(s.submit(SUMMARY, &k3, c), Submit::New);
        assert_eq!(
            [s.next_ready(), s.next_ready(), s.next_ready()],
            [Some(a), Some(b), None]
        );
        assert!(matches!(s.submit(SUMMARY, &k1, d), Submit::Existing(x) if x == a));
        // A waiting run's key is also taken.
        assert_eq!(s.submit(SUMMARY, &k3, d), Submit::Existing(c));
        s.finished(a);
        assert_eq!(s.next_ready(), Some(c));
        assert_eq!(s.next_ready(), None);
        assert!(matches!(s.submit(SUMMARY, &k1, e), Submit::New));
    }

    #[test]
    fn url_and_artifact_keys_with_the_same_value_are_distinct() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut s = Scheduler::default();
        assert_eq!(s.submit(SUMMARY, &url("same"), a), Submit::New);
        assert_eq!(s.submit(SUMMARY, &artifact("same"), b), Submit::New);
        assert_eq!(
            s.submit(SUMMARY, &artifact("same"), Uuid::new_v4()),
            Submit::Existing(b)
        );
        assert_eq!(
            s.submit(SUMMARY, &url("same"), Uuid::new_v4()),
            Submit::Existing(a)
        );
    }

    #[test]
    fn a_site_draft_run_and_a_summary_run_never_share_a_queue_slot() {
        let [a, b, c, d] = [(); 4].map(|()| Uuid::new_v4());
        let origin = url("https://tools.example.com");
        let mut s = Scheduler::default();
        assert_eq!(s.submit(RunKind::SiteDraft, &origin, a), Submit::New);
        assert_eq!(s.submit(SUMMARY, &origin, b), Submit::New);
        assert_eq!(
            s.submit(RunKind::SiteDraft, &origin, c),
            Submit::Existing(a)
        );
        assert_eq!(s.submit(SUMMARY, &origin, c), Submit::Existing(b));
        // A held site-draft key holds only that kind.
        s.finished(a);
        s.hold(RunKind::SiteDraft, &origin, c);
        assert_eq!(
            s.submit(RunKind::SiteDraft, &origin, d),
            Submit::Existing(c)
        );
        s.finished(b);
        assert_eq!(s.submit(SUMMARY, &origin, d), Submit::New);
    }

    #[test]
    fn dequeued_run_frees_its_key() {
        let [a, b, c, d] = [(); 4].map(|()| Uuid::new_v4());
        let mut s = Scheduler::default();
        s.submit(SUMMARY, &url("1"), a);
        s.submit(SUMMARY, &url("2"), b);
        s.submit(SUMMARY, &url("3"), c);
        assert_eq!(s.next_ready(), Some(a));
        assert_eq!(s.next_ready(), Some(b));
        assert!(s.is_waiting(c));
        assert!(!s.is_waiting(a), "running");
        // Running runs cannot be dequeued; unknown ones neither.
        assert!(!s.dequeue(a));
        assert!(!s.dequeue(Uuid::new_v4()));
        assert!(s.dequeue(c));
        assert!(!s.is_waiting(c));
        assert!(!s.dequeue(c));
        assert_eq!(s.submit(SUMMARY, &url("3"), d), Submit::New);
        s.finished(a);
        // The dequeued run never starts; the resubmitted one does.
        assert_eq!(s.next_ready(), Some(d));
        // A running run keeps its key after a failed dequeue.
        assert_eq!(
            s.submit(SUMMARY, &url("2"), Uuid::new_v4()),
            Submit::Existing(b)
        );
    }

    #[test]
    fn a_released_run_frees_its_slot_but_keeps_its_key_until_finished() {
        let [a, b, c, d] = [(); 4].map(|()| Uuid::new_v4());
        let mut s = Scheduler::default();
        s.submit(SUMMARY, &url("1"), a);
        s.submit(SUMMARY, &url("2"), b);
        s.submit(SUMMARY, &url("3"), c);
        assert_eq!(s.next_ready(), Some(a));
        assert_eq!(s.next_ready(), Some(b));
        s.release(a);
        // The slot is free for the next run, and nothing counts as busy.
        assert_eq!(s.next_ready(), Some(c));
        s.finished(b);
        s.finished(c);
        assert!(s.is_idle());
        // The key is still taken until the run is finished.
        assert_eq!(s.submit(SUMMARY, &url("1"), d), Submit::Existing(a));
        s.finished(a);
        assert_eq!(s.submit(SUMMARY, &url("1"), d), Submit::New);
    }

    #[test]
    fn idle_only_when_nothing_queued_or_running() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut s = Scheduler::default();
        assert!(s.is_idle());
        s.submit(SUMMARY, &url("1"), a);
        assert!(!s.is_idle(), "waiting run");
        assert_eq!(s.next_ready(), Some(a));
        assert!(!s.is_idle(), "running run");
        s.submit(SUMMARY, &url("2"), b);
        s.finished(a);
        assert!(!s.is_idle(), "still waiting");
        assert!(s.dequeue(b));
        assert!(s.is_idle());
        // Finishing a run that was still waiting also removes it.
        s.submit(SUMMARY, &url("3"), a);
        s.finished(a);
        assert!(s.is_idle());
        assert_eq!(s.next_ready(), None);
    }
}

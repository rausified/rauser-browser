//! The run queue: at most [`MAX_RUNNING`] runs execute at once, the rest wait
//! FIFO, and each record key has at most one queued-or-running run.
//!
//! This is a pure state machine. The supervisor owns the threads and calls
//! [`Scheduler::next_ready`] whenever a slot may have opened.

use std::collections::{HashMap, VecDeque};

use brauser_protocol::{RecordKey, RecordKeyKind};
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

/// A hashable identity for a [`RecordKey`]. The generated protocol types do
/// not derive `Hash`, so the kind is mapped to a small tag.
type KeyId = (u8, String);

fn key_id(key: &RecordKey) -> KeyId {
    let kind = match key.kind {
        RecordKeyKind::Url => 0,
        RecordKeyKind::Artifact => 1,
    };
    (kind, key.value.clone())
}

#[derive(Debug, Default)]
pub struct Scheduler {
    running: Vec<Uuid>,
    waiting: VecDeque<Uuid>,
    by_key: HashMap<KeyId, Uuid>,
}

impl Scheduler {
    /// Queues `id` under `key` unless that key already has a queued or running
    /// run, in which case the existing run's id is returned and nothing changes.
    pub fn submit(&mut self, key: &RecordKey, id: Uuid) -> Submit {
        let key = key_id(key);
        if let Some(existing) = self.by_key.get(&key) {
            return Submit::Existing(*existing);
        }
        self.by_key.insert(key, id);
        self.waiting.push_back(id);
        Submit::New
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
        self.running.retain(|running| *running != id);
        self.waiting.retain(|waiting| *waiting != id);
        self.free_key(id);
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

    fn free_key(&mut self, id: Uuid) {
        self.by_key.retain(|_, owner| *owner != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(matches!(s.submit(&k1, a), Submit::New));
        assert_eq!(s.submit(&k2, b), Submit::New);
        assert_eq!(s.submit(&k3, c), Submit::New);
        assert_eq!(
            [s.next_ready(), s.next_ready(), s.next_ready()],
            [Some(a), Some(b), None]
        );
        assert!(matches!(s.submit(&k1, d), Submit::Existing(x) if x == a));
        // A waiting run's key is also taken.
        assert_eq!(s.submit(&k3, d), Submit::Existing(c));
        s.finished(a);
        assert_eq!(s.next_ready(), Some(c));
        assert_eq!(s.next_ready(), None);
        assert!(matches!(s.submit(&k1, e), Submit::New));
    }

    #[test]
    fn url_and_artifact_keys_with_the_same_value_are_distinct() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut s = Scheduler::default();
        assert_eq!(s.submit(&url("same"), a), Submit::New);
        assert_eq!(s.submit(&artifact("same"), b), Submit::New);
        assert_eq!(
            s.submit(&artifact("same"), Uuid::new_v4()),
            Submit::Existing(b)
        );
        assert_eq!(s.submit(&url("same"), Uuid::new_v4()), Submit::Existing(a));
    }

    #[test]
    fn dequeued_run_frees_its_key() {
        let [a, b, c, d] = [(); 4].map(|()| Uuid::new_v4());
        let mut s = Scheduler::default();
        s.submit(&url("1"), a);
        s.submit(&url("2"), b);
        s.submit(&url("3"), c);
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
        assert_eq!(s.submit(&url("3"), d), Submit::New);
        s.finished(a);
        // The dequeued run never starts; the resubmitted one does.
        assert_eq!(s.next_ready(), Some(d));
        // A running run keeps its key after a failed dequeue.
        assert_eq!(s.submit(&url("2"), Uuid::new_v4()), Submit::Existing(b));
    }

    #[test]
    fn idle_only_when_nothing_queued_or_running() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut s = Scheduler::default();
        assert!(s.is_idle());
        s.submit(&url("1"), a);
        assert!(!s.is_idle(), "waiting run");
        assert_eq!(s.next_ready(), Some(a));
        assert!(!s.is_idle(), "running run");
        s.submit(&url("2"), b);
        s.finished(a);
        assert!(!s.is_idle(), "still waiting");
        assert!(s.dequeue(b));
        assert!(s.is_idle());
        // Finishing a run that was still waiting also removes it.
        s.submit(&url("3"), a);
        s.finished(a);
        assert!(s.is_idle());
        assert_eq!(s.next_ready(), None);
    }
}

//! The next-due table behind the scheduler: pure data, explicit instants.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::hash::Hash;
use std::time::Duration;

use tokio::time::Instant;

#[derive(Debug, Clone, Copy)]
struct Entry {
    due: Instant,
    slack: Duration,
    seq: u64,
}

/// One pending run per job, with a deadline and a slack: a job may piggyback on an earlier
/// wakeup when its deadline is within `slack`, so jobs of similar period share one wakeup.
#[derive(Debug)]
pub(crate) struct Timetable<J> {
    entries: HashMap<J, Entry>,
    /// Deadlines, earliest first. Entries superseded by a later `schedule` are skipped lazily.
    heap: BinaryHeap<Reverse<(Instant, u64)>>,
    jobs: HashMap<u64, J>,
    next_seq: u64,
}

impl<J: Hash + Eq + Clone> Timetable<J> {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            heap: BinaryHeap::new(),
            jobs: HashMap::new(),
            next_seq: 0,
        }
    }

    /// Plans `job` at `due`. A job already planned earlier keeps its earlier deadline.
    pub(crate) fn schedule(&mut self, job: J, due: Instant, slack: Duration) {
        if self.entries.get(&job).is_some_and(|e| e.due <= due) {
            return;
        }
        self.cancel(&job);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.heap.push(Reverse((due, seq)));
        self.jobs.insert(seq, job.clone());
        self.entries.insert(job, Entry { due, slack, seq });
    }

    /// Forgets the pending run of `job`, if any.
    pub(crate) fn cancel(&mut self, job: &J) {
        if let Some(entry) = self.entries.remove(job) {
            self.jobs.remove(&entry.seq);
        }
    }

    /// The earliest deadline: when the scheduler must wake up.
    pub(crate) fn next_due(&mut self) -> Option<Instant> {
        while let Some(&Reverse((due, seq))) = self.heap.peek() {
            if self.jobs.contains_key(&seq) {
                return Some(due);
            }
            self.heap.pop();
        }
        None
    }

    /// Removes and returns every job that is due at `now`, or within its slack of it.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<J> {
        let due: Vec<J> = self
            .entries
            .iter()
            .filter(|(_, e)| e.due <= now + e.slack)
            .map(|(job, _)| job.clone())
            .collect();
        for job in &due {
            self.cancel(job);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn wakes_at_the_earliest_deadline_and_runs_only_due_jobs() {
        let t0 = Instant::now();
        let mut table = Timetable::new();
        table.schedule("late", t0 + 10 * SECOND, Duration::ZERO);
        table.schedule("early", t0 + 2 * SECOND, Duration::ZERO);
        assert_eq!(table.next_due(), Some(t0 + 2 * SECOND));
        assert_eq!(table.take_due(t0 + SECOND), Vec::<&str>::new());
        assert_eq!(table.take_due(t0 + 2 * SECOND), vec!["early"]);
        assert_eq!(table.next_due(), Some(t0 + 10 * SECOND));
    }

    #[test]
    fn a_job_within_its_slack_shares_the_wakeup() {
        let t0 = Instant::now();
        let mut table = Timetable::new();
        table.schedule("a", t0 + 2 * SECOND, Duration::ZERO);
        table.schedule("b", t0 + 3 * SECOND, SECOND);
        table.schedule("c", t0 + 4 * SECOND, SECOND);
        let mut due = table.take_due(t0 + 2 * SECOND);
        due.sort_unstable();
        assert_eq!(due, vec!["a", "b"]);
        assert_eq!(table.next_due(), Some(t0 + 4 * SECOND));
    }

    #[test]
    fn the_earlier_deadline_wins_and_cancel_forgets() {
        let t0 = Instant::now();
        let mut table = Timetable::new();
        table.schedule("a", t0 + 5 * SECOND, Duration::ZERO);
        table.schedule("a", t0 + 9 * SECOND, Duration::ZERO);
        assert_eq!(table.next_due(), Some(t0 + 5 * SECOND));
        table.schedule("a", t0 + SECOND, Duration::ZERO);
        assert_eq!(table.next_due(), Some(t0 + SECOND));
        table.cancel(&"a");
        assert_eq!(table.next_due(), None);
        assert_eq!(table.take_due(t0 + 60 * SECOND), Vec::<&str>::new());
    }
}

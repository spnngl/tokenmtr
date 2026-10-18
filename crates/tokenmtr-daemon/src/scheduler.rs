//! The single scheduler: one task sleeping until the next deadline of any job.
//!
//! There are no per-task interval tickers: an idle daemon wakes only for a deadline in the
//! [`Timetable`], a control request or a file event. Jobs re-arm themselves when they finish.

use std::hash::Hash;
use std::time::Duration;

use tokenmtr_control::{Shutdown, wait_shutdown};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::timetable::Timetable;

enum Command<J> {
    Schedule {
        job: J,
        due: Instant,
        slack: Duration,
    },
    Cancel(J),
}

/// Plans and cancels jobs; cheap to clone, callable from any thread.
pub(crate) struct Scheduler<J> {
    commands: mpsc::UnboundedSender<Command<J>>,
}

impl<J> Clone for Scheduler<J> {
    fn clone(&self) -> Self {
        Self {
            commands: self.commands.clone(),
        }
    }
}

impl<J: Hash + Eq + Clone + Send + 'static> Scheduler<J> {
    /// A scheduler handle and the loop that serves it.
    pub(crate) fn new() -> (Self, Commands<J>) {
        let (commands, rx) = mpsc::unbounded_channel();
        (Self { commands }, Commands { rx })
    }

    /// Plans `job` at `due`; an earlier pending deadline of the same job wins.
    pub(crate) fn schedule(&self, job: J, due: Instant, slack: Duration) {
        // The loop is gone only during shutdown, when nothing is planned any more.
        let _ = self.commands.send(Command::Schedule { job, due, slack });
    }

    /// Plans `job` `delay` from now.
    pub(crate) fn schedule_in(&self, job: J, delay: Duration, slack: Duration) {
        self.schedule(job, Instant::now() + delay, slack);
    }

    /// Forgets the pending run of `job`.
    pub(crate) fn cancel(&self, job: J) {
        let _ = self.commands.send(Command::Cancel(job));
    }
}

/// The receiving end of a [`Scheduler`]; [`run`](Self::run) it on one task.
pub(crate) struct Commands<J> {
    rx: mpsc::UnboundedReceiver<Command<J>>,
}

impl<J: Hash + Eq + Clone> Commands<J> {
    /// Calls `dispatch` for every job that falls due, until `shutdown`.
    pub(crate) async fn run(mut self, mut shutdown: Shutdown, mut dispatch: impl FnMut(J)) {
        let mut table = Timetable::new();
        loop {
            let next = table.next_due();
            let wake = async {
                match next {
                    Some(due) => tokio::time::sleep_until(due).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = wait_shutdown(&mut shutdown) => return,
                command = self.rx.recv() => match command {
                    Some(Command::Schedule { job, due, slack }) => table.schedule(job, due, slack),
                    Some(Command::Cancel(job)) => table.cancel(&job),
                    None => return,
                },
                () = wake => {
                    for job in table.take_due(Instant::now()) {
                        dispatch(job);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::sync::watch;

    use super::*;

    #[tokio::test(start_paused = true)]
    async fn dispatches_in_deadline_order_and_sleeps_between() {
        let (scheduler, commands) = Scheduler::new();
        let (stop, shutdown) = watch::channel(false);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let task = tokio::spawn(commands.run(shutdown, move |job: &'static str| {
            sink.lock().map(|mut v| v.push((job, Instant::now()))).ok();
        }));
        let t0 = Instant::now();
        scheduler.schedule("b", t0 + Duration::from_secs(20), Duration::ZERO);
        scheduler.schedule("a", t0 + Duration::from_secs(5), Duration::ZERO);
        tokio::time::sleep(Duration::from_secs(30)).await;
        stop.send(true).ok();
        task.await.ok();
        let seen = seen.lock().map(|v| v.clone()).unwrap_or_default();
        let names: Vec<_> = seen.iter().map(|(job, _)| *job).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(seen[0].1, t0 + Duration::from_secs(5));
        assert_eq!(seen[1].1, t0 + Duration::from_secs(20));
    }
}

//! Timing of the hot path: one 5000-event transaction. Run in release:
//! `cargo test --release -p tokenmtr-store -- --ignored --nocapture throughput`.

use std::time::{Duration, Instant};

use super::*;

const BATCH: u64 = 5000;
/// The floor for a batch of new, changed or unchanged events, in events per second.
const FLOOR: u128 = 50_000;

fn sample(machine: &MachineId, n: u64, file: SourceFileId) -> UsageEvent {
    let mut e = event(machine, &format!("msg_{n}:req_{n}"), 20 + n % 30);
    e.at = at(1_772_323_200 + i64::try_from(n).unwrap());
    e.session = SessionId::try_from(format!("s{}", n / 500)).unwrap();
    e.cwd = Some(Utf8PathBuf::from("/bench/app"));
    e.git_branch = Some("main".to_owned());
    e.source_file = Some(file);
    e.tokens.input_uncached = 100;
    e.tokens.cache_read = 9000;
    e
}

fn rate(elapsed: Duration) -> u128 {
    u128::from(BATCH) * 1_000_000 / elapsed.as_micros().max(1)
}

#[test]
#[ignore = "timing: run in release"]
fn throughput() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let file = f
        .store
        .save_checkpoint(&checkpoint("/bench.jsonl", 0))
        .unwrap();
    let mut events: Vec<_> = (0..BATCH).map(|n| sample(&machine, n, file)).collect();
    let mut rates = Vec::new();
    for label in ["new", "unchanged", "changed"] {
        if label == "changed" {
            for e in &mut events {
                e.tokens.output += 1;
            }
        }
        let start = Instant::now();
        f.store.upsert_events(&events).unwrap();
        let events_per_second = rate(start.elapsed());
        println!(
            "{label}: {} ms, {events_per_second} events/s",
            start.elapsed().as_millis()
        );
        rates.push(events_per_second);
    }
    assert!(
        rates.iter().all(|r| *r >= FLOOR),
        "below {FLOOR} events/s: {rates:?}"
    );
}

//! Sink health states (D41) as a pure function.

use jiff::{SignedDuration, Timestamp};
use tokenmtr_core::SinkId;
use tokenmtr_export::{HealthInput, SinkHealth, SinkLag, sink_status, total_pending};
use tokenmtr_store::{Dataset, Pending, SinkRun};

use crate::fixtures::TestResult;

const INTERVAL: SignedDuration = SignedDuration::from_mins(5);

fn at(secs: i64) -> TestResult<Timestamp> {
    Ok(Timestamp::from_second(secs)?)
}

fn run(start: i64, error: Option<&str>) -> TestResult<SinkRun> {
    Ok(SinkRun {
        sink: SinkId::try_from("s")?,
        dataset: Dataset::UsageEvents,
        started: at(start)?,
        finished: at(start + 1)?,
        rows: 1,
        error: error.map(str::to_owned),
    })
}

fn status(
    enabled: bool,
    runs: &[SinkRun],
    pending: Pending,
) -> TestResult<tokenmtr_export::SinkStatus> {
    Ok(sink_status(&HealthInput {
        enabled,
        interval: INTERVAL,
        now: at(10_000)?,
        runs,
        pending,
    }))
}

fn pending(rows: u64, age_secs: i64) -> TestResult<Pending> {
    Ok(Pending {
        rows,
        oldest: Some(at(10_000 - age_secs)?),
    })
}

const NONE: Pending = Pending {
    rows: 0,
    oldest: None,
};

#[test]
fn nothing_pending_and_no_failure_is_healthy() -> TestResult {
    let s = status(true, &[run(9_000, None)?], NONE)?;
    assert_eq!(s.health, SinkHealth::Healthy);
    assert_eq!(s.lag, SinkLag { rows: 0, age: None });
    Ok(())
}

#[test]
fn a_young_backlog_is_healthy_and_an_old_one_lags() -> TestResult {
    let young = status(true, &[], pending(3, 600)?)?;
    assert_eq!(
        young.health,
        SinkHealth::Healthy,
        "exactly 2 intervals is not lagging"
    );
    assert_eq!(young.lag.rows, 3);
    assert_eq!(young.lag.age, Some(SignedDuration::from_secs(600)));

    let old = status(true, &[], pending(3, 601)?)?;
    assert_eq!(old.health, SinkHealth::Lagging);
    Ok(())
}

#[test]
fn a_failed_latest_run_fails_with_the_start_of_the_streak() -> TestResult {
    let runs = [
        run(7_000, None)?,
        run(8_000, Some("first"))?,
        run(9_000, Some("disk full"))?,
    ];
    let s = status(true, &runs, pending(1, 5_000)?)?;
    assert_eq!(
        s.health,
        SinkHealth::Failing {
            since: at(8_000)?,
            last_error: "disk full".to_owned()
        },
        "failing outranks lagging"
    );
    Ok(())
}

#[test]
fn a_success_after_failures_recovers() -> TestResult {
    let runs = [run(8_000, Some("boom"))?, run(9_000, None)?];
    assert_eq!(status(true, &runs, NONE)?.health, SinkHealth::Healthy);
    Ok(())
}

#[test]
fn disabled_overrides_everything() -> TestResult {
    let runs = [run(9_000, Some("boom"))?];
    assert_eq!(
        status(false, &runs, pending(9, 9_000)?)?.health,
        SinkHealth::Disabled
    );
    Ok(())
}

#[test]
fn pending_totals_sum_rows_and_keep_the_earliest() -> TestResult {
    let total = total_pending([pending(2, 100)?, NONE, pending(5, 900)?]);
    assert_eq!(total.rows, 7);
    assert_eq!(total.oldest, Some(at(9_100)?));
    assert_eq!(total_pending([]), NONE);
    Ok(())
}

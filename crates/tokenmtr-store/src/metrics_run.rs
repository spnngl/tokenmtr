//! Metrics output bookkeeping: one row per push, bind attempt or failed start.

use jiff::Timestamp;
use rusqlite::{Connection, named_params, params};
use tokenmtr_core::SinkId;

use crate::codec::{RowExt, to_i64, to_limit, to_us};
use crate::error::StoreError;

/// One attempt of one metrics output: a remote-write push, an exporter bind, or a start that
/// could not happen (a secret that cannot be read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsRun {
    /// The output (an `[[metrics.exporters]]` or `[[metrics.remote_write]]` id).
    pub output: SinkId,
    /// When the attempt started.
    pub started: Timestamp,
    /// When it ended.
    pub finished: Timestamp,
    /// Series delivered; zero on failure and for a bind.
    pub series: u64,
    /// The failure, when there was one. Status codes and kinds only, never headers or bodies.
    pub error: Option<String>,
}

const INSERT_RUN: &str = "INSERT INTO metrics_run \
    (output, started_at_us, finished_at_us, series, error) \
    VALUES (:output, :started_at_us, :finished_at_us, :series, :error)";

/// Runs kept per output; older ones are deleted as new ones arrive.
pub(crate) const RUN_HISTORY: usize = 1000;

const PRUNE_RUNS: &str = "DELETE FROM metrics_run WHERE id IN (SELECT id FROM metrics_run \
    WHERE output = ?1 ORDER BY started_at_us DESC, id DESC LIMIT -1 OFFSET ?2)";

/// Appends one run and drops the output's runs beyond the newest `keep`.
pub(crate) fn record_run(
    conn: &Connection,
    run: &MetricsRun,
    keep: usize,
) -> Result<(), StoreError> {
    conn.prepare_cached(INSERT_RUN)?.execute(named_params! {
        ":output": run.output.as_str(),
        ":started_at_us": to_us(run.started),
        ":finished_at_us": to_us(run.finished),
        ":series": to_i64(run.series, "series")?,
        ":error": run.error,
    })?;
    conn.prepare_cached(PRUNE_RUNS)?
        .execute(params![run.output.as_str(), to_limit(keep)?])?;
    Ok(())
}

/// The newest runs of `output`, newest first.
pub(crate) fn runs(
    conn: &Connection,
    output: &SinkId,
    limit: usize,
) -> Result<Vec<MetricsRun>, StoreError> {
    conn.prepare_cached(
        "SELECT started_at_us, finished_at_us, series, error FROM metrics_run \
         WHERE output = ?1 ORDER BY started_at_us DESC, id DESC LIMIT ?2",
    )?
    .query_and_then(params![output.as_str(), to_limit(limit)?], |row| {
        Ok(MetricsRun {
            output: output.clone(),
            started: row.ts("started_at_us")?,
            finished: row.ts("finished_at_us")?,
            series: row.count("series")?,
            error: row.get("error")?,
        })
    })?
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_run_keeps_the_newest_runs_of_each_output() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::migrations::migrations()
            .to_latest(&mut conn)
            .unwrap();
        let run = |output: &str, started: i64| MetricsRun {
            output: SinkId::try_from(output.to_owned()).unwrap(),
            started: Timestamp::from_second(started).unwrap(),
            finished: Timestamp::from_second(started + 1).unwrap(),
            series: u64::try_from(started).unwrap(),
            error: None,
        };
        record_run(&conn, &run("other", 1), 2).unwrap();
        for started in [10, 30, 20, 40] {
            record_run(&conn, &run("a", started), 2).unwrap();
        }
        let a = SinkId::try_from("a".to_owned()).unwrap();
        let kept: Vec<_> = runs(&conn, &a, 10)
            .unwrap()
            .iter()
            .map(|r| r.series)
            .collect();
        assert_eq!(kept, [40, 30]);
        let other = SinkId::try_from("other".to_owned()).unwrap();
        assert_eq!(runs(&conn, &other, 10).unwrap().len(), 1);
    }
}

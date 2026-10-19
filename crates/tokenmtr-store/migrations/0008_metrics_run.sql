-- One row per remote-write push, per exporter bind attempt and per output that failed to start.
-- Metrics health is computed from these rows; a scrape never writes one.
CREATE TABLE metrics_run (
    id             INTEGER PRIMARY KEY,
    output         TEXT NOT NULL,
    started_at_us  INTEGER NOT NULL,
    finished_at_us INTEGER NOT NULL,
    series         INTEGER NOT NULL CHECK (series >= 0),
    error          TEXT
) STRICT;

CREATE INDEX metrics_run_output_started ON metrics_run (output, started_at_us);

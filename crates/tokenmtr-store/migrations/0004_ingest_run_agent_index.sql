-- The latest run of each agent (ingest status). It replaces the index on `started_at_us`
-- alone, which no query used: finding each agent's latest run through it scanned every run.
DROP INDEX ingest_run_started;
CREATE INDEX ingest_run_agent_started ON ingest_run (agent, started_at_us);

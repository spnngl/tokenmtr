-- Stats-cache aggregates on days transcripts now cover are kept and flagged: queries
-- skip them, sinks receive the flag with a new revision.
ALTER TABLE usage_event
    ADD COLUMN superseded INTEGER NOT NULL DEFAULT 0 CHECK (superseded IN (0, 1));

-- A subscription removed from config keeps its row with the time it disappeared; billing
-- ignores it, exports keep it.
ALTER TABLE subscription ADD COLUMN removed_at_us INTEGER;

-- Latest and recent snapshots per profile (quota meters, history, the advisor).
CREATE INDEX quota_snapshot_profile_fetched ON quota_snapshot (agent, profile, fetched_at_us);

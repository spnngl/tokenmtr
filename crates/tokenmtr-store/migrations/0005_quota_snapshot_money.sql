-- Claude extra usage (pay-as-you-go spend beyond the plan) is stored as the `extra_usage`
-- window with its money; every other window keeps both columns NULL.
-- Spent this month, in USD micros.
ALTER TABLE quota_snapshot
    ADD COLUMN spent_usd_micros INTEGER CHECK (spent_usd_micros >= 0);

-- The monthly cap, in USD micros.
ALTER TABLE quota_snapshot
    ADD COLUMN limit_usd_micros INTEGER CHECK (limit_usd_micros >= 0);

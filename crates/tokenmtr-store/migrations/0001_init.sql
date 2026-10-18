-- TokenMtr store, schema version 1. See the crate docs (src/lib.rs) for the conventions.
--
--   * Timestamps are integer UTC microseconds since the Unix epoch (`*_us`).
--   * Money is integer micro-USD (`*_usd_micros`); the ECB rate is EUR per USD scaled by 1e9.
--   * Civil dates (`fx_rate.date`, `subscription.start_date`) are ISO-8601 text.
--   * Ids of usage events are 16-byte BLOBs.
--   * Tables are STRICT. Rows of event-like tables are never deleted.
--   * `revision` columns are unique per table and come from `dataset_revision`.

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value ANY NOT NULL
) STRICT;

-- One monotonically increasing counter per exported dataset. Dataset names are the export names
-- (`Dataset::as_str`).
CREATE TABLE dataset_revision (
    dataset  TEXT PRIMARY KEY,
    revision INTEGER NOT NULL CHECK (revision >= 0)
) STRICT;

INSERT INTO dataset_revision (dataset, revision) VALUES
    ('usage_events', 0),
    ('tool_calls', 0),
    ('subscriptions', 0),
    ('quota_snapshots', 0),
    ('fx_rates', 0);

-- Checkpoints. `dev` and `inode` are the u64 values reinterpreted as i64.
CREATE TABLE source_file (
    id             INTEGER PRIMARY KEY,
    path           TEXT NOT NULL UNIQUE,
    agent          TEXT NOT NULL,
    profile        TEXT NOT NULL,
    dev            INTEGER NOT NULL,
    inode          INTEGER NOT NULL,
    size           INTEGER NOT NULL CHECK (size >= 0),
    mtime_us       INTEGER NOT NULL,
    byte_offset    INTEGER NOT NULL CHECK (byte_offset >= 0),
    last_line_hash BLOB CHECK (last_line_hash IS NULL OR length(last_line_hash) = 32),
    updated_at_us  INTEGER NOT NULL
) STRICT;

CREATE TABLE usage_event (
    id                       INTEGER PRIMARY KEY,
    event_id                 BLOB NOT NULL UNIQUE CHECK (length(event_id) = 16),
    agent                    TEXT NOT NULL,
    dedup_key                TEXT NOT NULL,
    revision                 INTEGER NOT NULL CHECK (revision > 0),
    -- When the revision was last bumped. Sink lag in time is measured from here, not from `at_us`,
    -- because a backfill ingests old events now.
    updated_at_us            INTEGER NOT NULL,
    profile                  TEXT NOT NULL,
    at_us                    INTEGER NOT NULL,
    -- UTC calendar day (days since 1970-01-01): the key of FX rate lookups and UTC partitions.
    -- Zone-aware buckets are computed at query time from `at_us`.
    utc_day                  INTEGER GENERATED ALWAYS AS (
        (at_us - ((at_us % 86400000000) + 86400000000) % 86400000000) / 86400000000
    ) STORED,
    session                  TEXT NOT NULL,
    parent_session           TEXT,
    subagent                 TEXT,
    is_sidechain             INTEGER NOT NULL CHECK (is_sidechain IN (0, 1)),
    cwd                      TEXT,
    git_branch               TEXT,
    project                  TEXT,
    client                   TEXT,
    source_file_id           INTEGER REFERENCES source_file (id),
    approximate              INTEGER NOT NULL CHECK (approximate IN (0, 1)),
    provider                 TEXT,
    model                    TEXT NOT NULL,
    kind                     TEXT NOT NULL,
    input_uncached           INTEGER NOT NULL CHECK (input_uncached >= 0),
    cache_read               INTEGER NOT NULL CHECK (cache_read >= 0),
    cache_write_5m           INTEGER NOT NULL CHECK (cache_write_5m >= 0),
    cache_write_1h           INTEGER NOT NULL CHECK (cache_write_1h >= 0),
    output                   INTEGER NOT NULL CHECK (output >= 0),
    reasoning                INTEGER NOT NULL CHECK (reasoning >= 0),
    -- `reasoning` is a subset of `output`, never summed.
    total_tokens             INTEGER GENERATED ALWAYS AS (
        input_uncached + cache_read + cache_write_5m + cache_write_1h + output
    ) VIRTUAL,
    cost_reported_usd_micros INTEGER,
    cost_estimated_usd_micros INTEGER,
    pricing_version          TEXT,
    -- The reported cost wins over the estimate (`reported ?? estimated`), and where it came from.
    cost_api_usd_micros      INTEGER GENERATED ALWAYS AS (
        COALESCE(cost_reported_usd_micros, cost_estimated_usd_micros)
    ) VIRTUAL,
    cost_source              TEXT GENERATED ALWAYS AS (
        CASE
            WHEN cost_reported_usd_micros IS NOT NULL THEN 'reported'
            WHEN cost_estimated_usd_micros IS NOT NULL THEN 'estimated'
            ELSE 'unpriced'
        END
    ) VIRTUAL,
    service_tier             TEXT,
    UNIQUE (agent, dedup_key),
    CHECK ((cost_estimated_usd_micros IS NULL) = (pricing_version IS NULL))
) STRICT;

CREATE UNIQUE INDEX usage_event_revision ON usage_event (revision);
CREATE INDEX usage_event_at ON usage_event (at_us);
CREATE INDEX usage_event_agent_profile_at ON usage_event (agent, profile, at_us);
CREATE INDEX usage_event_session ON usage_event (session, at_us);
CREATE INDEX usage_event_project_at ON usage_event (project, at_us);
CREATE INDEX usage_event_model_at ON usage_event (model, at_us);

-- Tool names and error flags only, never content. `event_id` is not a foreign key: a tool call can be stored before its event.
CREATE TABLE tool_call (
    id            INTEGER PRIMARY KEY,
    agent         TEXT NOT NULL,
    tool_use_id   TEXT NOT NULL,
    revision      INTEGER NOT NULL CHECK (revision > 0),
    updated_at_us INTEGER NOT NULL,
    profile       TEXT NOT NULL,
    event_id      BLOB CHECK (event_id IS NULL OR length(event_id) = 16),
    session       TEXT NOT NULL,
    at_us         INTEGER NOT NULL,
    tool_name     TEXT NOT NULL,
    mcp_server    TEXT,
    is_error      INTEGER CHECK (is_error IS NULL OR is_error IN (0, 1)),
    UNIQUE (agent, tool_use_id)
) STRICT;

CREATE UNIQUE INDEX tool_call_revision ON tool_call (revision);
CREATE INDEX tool_call_at ON tool_call (at_us);
CREATE INDEX tool_call_session ON tool_call (session);
CREATE INDEX tool_call_tool ON tool_call (tool_name, mcp_server);

CREATE TABLE session_signal (
    id      INTEGER PRIMARY KEY,
    agent   TEXT NOT NULL,
    session TEXT NOT NULL,
    kind    TEXT NOT NULL,
    at_us   INTEGER NOT NULL,
    UNIQUE (agent, session, kind, at_us)
) STRICT;

CREATE INDEX session_signal_kind_at ON session_signal (kind, at_us);

-- A project is a git repository. `source` says how the project was resolved; `cwd` means "unresolved" (no rule, no git).
CREATE TABLE project_map (
    cwd            TEXT PRIMARY KEY,
    project        TEXT NOT NULL,
    name           TEXT NOT NULL,
    client         TEXT,
    repo_root      TEXT,
    remote         TEXT,
    resolved_at_us INTEGER NOT NULL,
    source         TEXT NOT NULL CHECK (source IN ('rule', 'remote', 'repo_root', 'cwd')),
    rule_id        TEXT,
    CHECK ((source = 'rule') = (rule_id IS NOT NULL))
) STRICT;

CREATE INDEX project_map_project ON project_map (project);

-- Alerts the daemon decided. `payload` is the `AlertKind` as JSON.
CREATE TABLE alert (
    id          INTEGER PRIMARY KEY,
    kind        TEXT NOT NULL,
    dedup_key   TEXT NOT NULL UNIQUE,
    fired_at_us INTEGER NOT NULL,
    payload     TEXT NOT NULL CHECK (json_valid(payload))
) STRICT;

CREATE INDEX alert_fired_at ON alert (fired_at_us);

-- One row per exported batch; sink health is computed from these rows.
CREATE TABLE sink_run (
    id             INTEGER PRIMARY KEY,
    sink           TEXT NOT NULL,
    dataset        TEXT NOT NULL,
    started_at_us  INTEGER NOT NULL,
    finished_at_us INTEGER NOT NULL,
    row_count      INTEGER NOT NULL CHECK (row_count >= 0),
    error          TEXT
) STRICT;

CREATE INDEX sink_run_sink_started ON sink_run (sink, started_at_us);

-- One row per backup run.
CREATE TABLE backup_run (
    id             INTEGER PRIMARY KEY,
    started_at_us  INTEGER NOT NULL,
    finished_at_us INTEGER NOT NULL,
    path           TEXT,
    size_bytes     INTEGER CHECK (size_bytes IS NULL OR size_bytes >= 0),
    error          TEXT
) STRICT;

-- LiteLLM pricing snapshots. `version` is `PricingVersion`; the body is the raw LiteLLM JSON, zstd-compressed.
-- The active snapshot is `meta.pricing_version`.
CREATE TABLE pricing_snapshot (
    version       TEXT PRIMARY KEY,
    fetched_at_us INTEGER NOT NULL,
    source        TEXT,
    body_zstd     BLOB NOT NULL,
    body_len      INTEGER NOT NULL CHECK (body_len >= 0)
) STRICT;

-- Subscriptions, mirrored from config; rows are never deleted so exports keep ended subscriptions.
CREATE TABLE subscription (
    id            INTEGER PRIMARY KEY,
    subscription_id TEXT NOT NULL UNIQUE,
    revision      INTEGER NOT NULL CHECK (revision > 0),
    updated_at_us INTEGER NOT NULL,
    provider      TEXT NOT NULL,
    plan          TEXT NOT NULL,
    agents        TEXT NOT NULL CHECK (json_valid(agents)),
    profiles      TEXT CHECK (profiles IS NULL OR json_valid(profiles)),
    fee_micros    INTEGER NOT NULL CHECK (fee_micros >= 0),
    fee_currency  TEXT NOT NULL CHECK (fee_currency IN ('USD', 'EUR')),
    period        TEXT NOT NULL,
    start_date    TEXT NOT NULL,
    end_date      TEXT
) STRICT;

CREATE UNIQUE INDEX subscription_revision ON subscription (revision);

-- Detected plan and identity, one row per agent profile.
CREATE TABLE account (
    agent           TEXT NOT NULL,
    profile         TEXT NOT NULL,
    email           TEXT,
    organization    TEXT,
    detected_plan   TEXT,
    rate_limit_tier TEXT,
    fetched_at_us   INTEGER NOT NULL,
    PRIMARY KEY (agent, profile)
) STRICT;

-- Polled quota: one row per window per poll. A poll without windows (signed out, expired token,
-- failure) is one row with `window_id = ''`, no percent and a non-`ok` status.
CREATE TABLE quota_snapshot (
    id            INTEGER PRIMARY KEY,
    agent         TEXT NOT NULL,
    profile       TEXT NOT NULL,
    window_id     TEXT NOT NULL,
    fetched_at_us INTEGER NOT NULL,
    revision      INTEGER NOT NULL CHECK (revision > 0),
    updated_at_us INTEGER NOT NULL,
    account       TEXT,
    used_percent  REAL CHECK (used_percent IS NULL OR used_percent >= 0),
    resets_at_us  INTEGER,
    status        TEXT NOT NULL CHECK (status IN ('ok', 'signed_out', 'token_expired', 'failed')),
    UNIQUE (agent, profile, window_id, fetched_at_us),
    CHECK ((status = 'ok') = (used_percent IS NOT NULL)),
    CHECK ((window_id = '') = (status <> 'ok'))
) STRICT;

CREATE UNIQUE INDEX quota_snapshot_revision ON quota_snapshot (revision);
CREATE INDEX quota_snapshot_fetched ON quota_snapshot (fetched_at_us);

-- ECB rates. `eur_per_usd_scaled` is EUR per 1 USD times 1e9.
CREATE TABLE fx_rate (
    id                 INTEGER PRIMARY KEY,
    date               TEXT NOT NULL UNIQUE,
    revision           INTEGER NOT NULL CHECK (revision > 0),
    updated_at_us      INTEGER NOT NULL,
    eur_per_usd_scaled INTEGER NOT NULL CHECK (eur_per_usd_scaled > 0),
    source             TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX fx_rate_revision ON fx_rate (revision);

-- The last revision a sink acknowledged, per dataset.
CREATE TABLE sink_cursor (
    sink     TEXT NOT NULL,
    dataset  TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    PRIMARY KEY (sink, dataset)
) STRICT, WITHOUT ROWID;

-- Parse issues and format drift. One open issue per (agent, kind, subject); repeats add to `occurrences`.
CREATE TABLE parse_issue (
    id             INTEGER PRIMARY KEY,
    agent          TEXT NOT NULL,
    kind           TEXT NOT NULL,
    subject        TEXT NOT NULL,
    occurrences    INTEGER NOT NULL CHECK (occurrences > 0),
    sample         TEXT,
    first_seen_us  INTEGER NOT NULL,
    last_seen_us   INTEGER NOT NULL,
    resolved_at_us INTEGER,
    UNIQUE (agent, kind, subject)
) STRICT;

CREATE TABLE ingest_run (
    id             INTEGER PRIMARY KEY,
    agent          TEXT NOT NULL,
    started_at_us  INTEGER NOT NULL,
    finished_at_us INTEGER NOT NULL,
    files_parsed   INTEGER NOT NULL CHECK (files_parsed >= 0),
    bytes_parsed   INTEGER NOT NULL CHECK (bytes_parsed >= 0),
    lines_read     INTEGER NOT NULL CHECK (lines_read >= 0),
    events_found   INTEGER NOT NULL CHECK (events_found >= 0),
    events_changed INTEGER NOT NULL CHECK (events_changed >= 0),
    skipped        TEXT NOT NULL CHECK (json_valid(skipped))
) STRICT;

CREATE INDEX ingest_run_started ON ingest_run (started_at_us);

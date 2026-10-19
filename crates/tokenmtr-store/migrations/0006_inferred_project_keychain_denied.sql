-- SQLite cannot alter a CHECK constraint: both tables are rebuilt with the new value allowed,
-- keeping every column, row and index.

-- `inferred`: a cwd that git could not inspect (deleted worktree, non-git folder) named through
-- another app's record of it (Superset's workspaces).
CREATE TABLE project_map_new (
    cwd            TEXT PRIMARY KEY,
    project        TEXT NOT NULL,
    name           TEXT NOT NULL,
    client         TEXT,
    repo_root      TEXT,
    remote         TEXT,
    resolved_at_us INTEGER NOT NULL,
    source         TEXT NOT NULL CHECK (source IN ('rule', 'remote', 'repo_root', 'inferred', 'cwd')),
    rule_id        TEXT,
    CHECK ((source = 'rule') = (rule_id IS NOT NULL))
) STRICT;

INSERT INTO project_map_new SELECT * FROM project_map;
DROP TABLE project_map;
ALTER TABLE project_map_new RENAME TO project_map;
CREATE INDEX project_map_project ON project_map (project);

-- `keychain_denied`: the Keychain refused the Claude credentials and no file holds them.
CREATE TABLE quota_snapshot_new (
    id               INTEGER PRIMARY KEY,
    agent            TEXT NOT NULL,
    profile          TEXT NOT NULL,
    window_id        TEXT NOT NULL,
    fetched_at_us    INTEGER NOT NULL,
    revision         INTEGER NOT NULL CHECK (revision > 0),
    updated_at_us    INTEGER NOT NULL,
    account          TEXT,
    used_percent     REAL CHECK (used_percent IS NULL OR used_percent >= 0),
    resets_at_us     INTEGER,
    status           TEXT NOT NULL
        CHECK (status IN ('ok', 'signed_out', 'token_expired', 'keychain_denied', 'failed')),
    spent_usd_micros INTEGER CHECK (spent_usd_micros >= 0),
    limit_usd_micros INTEGER CHECK (limit_usd_micros >= 0),
    UNIQUE (agent, profile, window_id, fetched_at_us),
    CHECK ((status = 'ok') = (used_percent IS NOT NULL)),
    CHECK ((window_id = '') = (status <> 'ok'))
) STRICT;

INSERT INTO quota_snapshot_new SELECT * FROM quota_snapshot;
DROP TABLE quota_snapshot;
ALTER TABLE quota_snapshot_new RENAME TO quota_snapshot;
CREATE UNIQUE INDEX quota_snapshot_revision ON quota_snapshot (revision);
CREATE INDEX quota_snapshot_fetched ON quota_snapshot (fetched_at_us);
CREATE INDEX quota_snapshot_profile_fetched ON quota_snapshot (agent, profile, fetched_at_us);

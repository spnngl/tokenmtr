-- Rows read from the TokenMtr SQLite exports of other machines (`[[sources]] kind = "sqlite"`).
--
-- Imported rows live beside local ones, so queries count both. Three columns tell them apart:
-- all NULL on a local row, all set on an imported one. Imported rows are never exported (each
-- machine exports itself) and the logic that only makes sense for this machine's own files
-- (project resolution, repricing, branch stamping, metrics, the live recap) selects
-- `origin_machine IS NULL`.
--
-- An imported row keeps the unique keys of its table with synthetic, permanent values:
-- `dedup_key = 'import:<event_id hex>'` and `tool_use_id = 'import:<tool_call_id hex>'`. Its
-- `event_id` is the one the origin exported. `revision` stays this store's own counter, so an
-- imported row that changes is a `StoreChanged` like any other.

-- The machine the row came from (`meta.machine_id` of the origin, 16 bytes).
ALTER TABLE usage_event ADD COLUMN origin_machine BLOB
    CHECK (origin_machine IS NULL OR length(origin_machine) = 16);
-- The `[[sources]]` id that imported it; `tokenmtr sources forget` deletes by this.
ALTER TABLE usage_event ADD COLUMN origin_source TEXT
    CHECK ((origin_source IS NULL) = (origin_machine IS NULL));
-- The `revision` the row had in the origin's export: a row is replaced only by a higher one.
ALTER TABLE usage_event ADD COLUMN origin_revision INTEGER
    CHECK ((origin_revision IS NULL) = (origin_machine IS NULL));

ALTER TABLE tool_call ADD COLUMN origin_machine BLOB
    CHECK (origin_machine IS NULL OR length(origin_machine) = 16);
ALTER TABLE tool_call ADD COLUMN origin_source TEXT
    CHECK ((origin_source IS NULL) = (origin_machine IS NULL));
ALTER TABLE tool_call ADD COLUMN origin_revision INTEGER
    CHECK ((origin_revision IS NULL) = (origin_machine IS NULL));

-- Project resolution and branch stamping only visit local rows, by directory: `origin_machine IS
-- NULL` leads, so the walk from one directory to the next still seeks and covers.
DROP INDEX usage_event_cwd;
CREATE INDEX usage_event_cwd ON usage_event (origin_machine, cwd);

-- Where each export file stands. A file is read again from `usage_events_revision` /
-- `tool_calls_revision` (origin revisions) when its fingerprint changed. The fingerprint
-- (`size`, `mtime_us`, `schema_version`) is set once the whole file was read and cleared by the
-- next page written, so a run that stopped half-way reads the file again. All three are NULL or
-- none is.
CREATE TABLE import_cursor (
    source                TEXT NOT NULL,
    -- Absolute path of the export file.
    path                  TEXT NOT NULL,
    -- The origin the file belongs to; another one in the file resets the cursor.
    machine_id            BLOB NOT NULL CHECK (length(machine_id) = 16),
    usage_events_revision INTEGER NOT NULL DEFAULT 0 CHECK (usage_events_revision >= 0),
    tool_calls_revision   INTEGER NOT NULL DEFAULT 0 CHECK (tool_calls_revision >= 0),
    size                  INTEGER CHECK (size IS NULL OR size >= 0),
    mtime_us              INTEGER CHECK ((mtime_us IS NULL) = (size IS NULL)),
    schema_version        INTEGER CHECK ((schema_version IS NULL) = (size IS NULL)),
    PRIMARY KEY (source, path)
) STRICT, WITHOUT ROWID;

-- One row per failed file and one per pass (`path` NULL); source health is computed from these
-- rows. `dataset` is unused.
CREATE TABLE import_run (
    id             INTEGER PRIMARY KEY,
    source         TEXT NOT NULL,
    path           TEXT,
    dataset        TEXT,
    started_at_us  INTEGER NOT NULL,
    finished_at_us INTEGER NOT NULL,
    row_count      INTEGER NOT NULL CHECK (row_count >= 0),
    error          TEXT
) STRICT;

CREATE INDEX import_run_source_started ON import_run (source, started_at_us);

-- `forget` and the per-source row counts of health seek by origin.
CREATE INDEX usage_event_origin ON usage_event (origin_source) WHERE origin_source IS NOT NULL;
CREATE INDEX tool_call_origin ON tool_call (origin_source) WHERE origin_source IS NOT NULL;

//! `SQLite` sink: one table per dataset, queryable with `sqlite3`, `DuckDB` or Datasette.
//!
//! Each batch is one transaction of `INSERT … ON CONFLICT(id) DO UPDATE … WHERE
//! excluded.revision > revision`, so a row exists once, at its highest delivered revision, and
//! redelivery is a no-op. The `_schema` table holds [`EXPORT_SCHEMA_VERSION`]; a file written by
//! another version is refused rather than mixed. The connection uses `synchronous = FULL` and
//! `fullfsync` so a committed batch survives power loss, and the default rollback journal so
//! the export stays a single file.

use std::fs::{self, OpenOptions};

use camino::Utf8Path;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, params_from_iter};
use tokenmtr_core::SinkId;

use crate::error::SinkError;
use crate::schema::{EXPORT_SCHEMA_VERSION, Value, schema};
use crate::sink::{ExportBatch, Sink, SqliteTarget};
use crate::sql;

/// How long a write waits for a reader holding the file, in milliseconds.
const BUSY_TIMEOUT_MS: u64 = 5_000;

/// Writes datasets into one `SQLite` database file.
#[derive(Debug)]
pub struct SqliteSink {
    id: SinkId,
    target: SqliteTarget,
    connection: Option<Connection>,
}

impl SqliteSink {
    /// A sink for `target`. Creates nothing until the first write.
    #[must_use]
    pub const fn new(id: SinkId, target: SqliteTarget) -> Self {
        Self {
            id,
            target,
            connection: None,
        }
    }

    fn sqlite(&self, source: rusqlite::Error) -> SinkError {
        SinkError::Sqlite {
            path: self.target.path.clone(),
            source,
        }
    }

    fn open(&self) -> Result<Connection, SinkError> {
        let path: &Utf8Path = &self.target.path;
        if let Some(dir) = path.parent().filter(|d| !d.as_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|e| SinkError::io("create", dir, e))?;
        }
        // Touch the file first so a refused path reports `PermissionDenied`, which
        // `SQLite` would flatten into "unable to open database file".
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options
            .open(path)
            .map_err(|e| SinkError::io("open", path, e))?;

        let connection = Connection::open(path).map_err(|e| self.sqlite(e))?;
        connection
            .busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS))
            .and_then(|()| {
                connection.execute_batch("PRAGMA synchronous = FULL; PRAGMA fullfsync = ON;")
            })
            .and_then(|()| connection.execute_batch(sql::CREATE_SCHEMA_TABLE))
            .and_then(|()| connection.execute_batch(&sql::insert_schema_version()))
            .map_err(|e| self.sqlite(e))?;
        let found: i64 = connection
            .query_row("SELECT version FROM _schema WHERE id = 1", [], |row| {
                row.get(0)
            })
            .map_err(|e| self.sqlite(e))?;
        if found != i64::from(EXPORT_SCHEMA_VERSION) {
            return Err(SinkError::SchemaVersion {
                path: self.target.path.clone(),
                found,
                expected: EXPORT_SCHEMA_VERSION,
            });
        }
        Ok(connection)
    }

    fn write_batch(connection: &mut Connection, batch: &ExportBatch) -> rusqlite::Result<()> {
        let schema = schema(batch.dataset);
        let tx = connection.transaction()?;
        tx.execute_batch(&sql::create_table(schema))?;
        {
            let mut upsert = tx.prepare(&sql::upsert(schema))?;
            for row in &batch.rows {
                upsert.execute(params_from_iter(row.values.iter().map(sql_value)))?;
            }
        }
        tx.commit()
    }
}

fn sql_value(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Text(text) => SqlValue::Text(text.clone()),
        Value::Integer(n) => SqlValue::Integer(*n),
        Value::Boolean(b) => SqlValue::Integer(i64::from(*b)),
        Value::Real(x) => SqlValue::Real(*x),
        Value::Decimal { units, .. } => SqlValue::Integer(*units),
    }
}

impl Sink for SqliteSink {
    fn id(&self) -> &SinkId {
        &self.id
    }

    fn write(&mut self, batch: &ExportBatch) -> Result<(), SinkError> {
        if batch.rows.is_empty() {
            return Ok(());
        }
        let mut connection = match self.connection.take() {
            Some(connection) => connection,
            None => self.open()?,
        };
        let result = Self::write_batch(&mut connection, batch);
        match result {
            Ok(()) => {
                self.connection = Some(connection);
                Ok(())
            }
            // The connection is dropped: the next write reopens and re-validates the file.
            Err(source) => Err(self.sqlite(source)),
        }
    }
}

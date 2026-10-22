//! What can go wrong reading one export file.
//!
//! Messages name columns and tables, never row values: they reach `import_run` and the logs.

use std::fmt;

use tokenmtr_store::StoreError;

/// Why a cell could not become a store value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// Empty where the schema forbids it.
    Missing,
    /// Another SQL type than the schema's.
    WrongType,
    /// A number outside the column's range.
    OutOfRange,
    /// Text that is not a valid value of the column.
    Invalid,
    /// A row of another machine than the file's first row.
    ForeignMachine,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "is empty",
            Self::WrongType => "has the wrong type",
            Self::OutOfRange => "is out of range",
            Self::Invalid => "is not valid",
            Self::ForeignMachine => "names another machine than the rest of the file",
        })
    }
}

/// A row the reader cannot decode: the column and what is wrong with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("column {column} {problem}")]
pub struct DecodeError {
    /// The export column name.
    pub column: &'static str,
    /// What is wrong.
    pub problem: Problem,
}

/// Why one file was not imported (completely). The next run retries it.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The OS refuses the file or its folder (macOS protected folders, cloud storage).
    #[error("permission denied")]
    PermissionDenied,
    /// Any other file system failure.
    #[error("cannot read the file: {0}")]
    Io(std::io::Error),
    /// The file changed while it was copied, or is mid-write; a sync client or the other
    /// machine's sink is writing it.
    #[error("the file is being written")]
    Busy,
    /// `SQLite` refused the copy: not a database, corrupt or locked.
    #[error("cannot read the database: {0}")]
    Sqlite(rusqlite::Error),
    /// A `SQLite` file without the `_schema` table: not a `TokenMtr` export.
    #[error("not a TokenMtr SQLite export (no _schema table)")]
    NotAnExport,
    /// An export of a schema version this build has no mapping for.
    #[error("export schema version {found} is not supported (this build reads {supported})")]
    Version {
        /// The version in the file.
        found: u32,
        /// The versions this build reads, for the message.
        supported: String,
    },
    /// A column of the schema the file lacks.
    #[error("table {table} has no column {column}")]
    MissingColumn {
        /// The export table.
        table: &'static str,
        /// The export column, as named in SQL.
        column: String,
    },
    /// Rows of more than one machine in one file.
    #[error("the file holds rows of several machines")]
    SeveralMachines,
    /// A row that does not decode.
    #[error("table {table}: {error}")]
    Decode {
        /// The export table.
        table: &'static str,
        /// The cell.
        error: DecodeError,
    },
    /// The local store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl ImportError {
    /// Sorts an I/O error: a refused folder is [`Self::PermissionDenied`].
    #[must_use]
    pub fn io(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            Self::PermissionDenied
        } else {
            Self::Io(error)
        }
    }
}

impl From<rusqlite::Error> for ImportError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

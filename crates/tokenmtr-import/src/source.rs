//! The reader of one opened export file, and the closed set of readers.

use camino::Utf8Path;
use tokenmtr_core::{Dataset, MachineId};
use tokenmtr_store::{ImportFingerprint, ImportedEvent, ImportedToolCall, Revision};

use crate::error::ImportError;
use crate::sqlite::SqliteExport;

/// One page of decoded rows: at most one of the two lists is filled, by the dataset read.
#[derive(Debug, Default)]
pub struct Rows {
    /// Decoded `usage_events` rows, `extra_usage` left out.
    pub events: Vec<ImportedEvent>,
    /// Decoded `tool_calls` rows.
    pub tool_calls: Vec<ImportedToolCall>,
}

/// An export file, opened and checked. Every method reads the same private copy.
pub trait ImportSource {
    /// Size, modification time and schema version of the file when it was copied.
    fn fingerprint(&self) -> ImportFingerprint;

    /// The machine whose rows the file holds, from its first row; `None` for an empty file.
    ///
    /// # Errors
    /// [`ImportError`] when the file cannot be read.
    fn machine(&self) -> Result<Option<MachineId>, ImportError>;

    /// The highest revision of `dataset` in the file; zero when it is empty or absent.
    ///
    /// # Errors
    /// [`ImportError`] when the file cannot be read.
    fn max_revision(&self, dataset: Dataset) -> Result<Revision, ImportError>;

    /// Reads the rows of `dataset` past `after`, in revision order, in pages of at most
    /// `page_size` source rows. `on_page` returns whether to go on; `false` stops the read.
    /// Returns whether the read reached the end.
    ///
    /// # Errors
    /// [`ImportError`] when a row is not a valid row of a file of `machine`, or `on_page` fails.
    fn read(
        &self,
        dataset: Dataset,
        machine: &MachineId,
        after: Revision,
        page_size: usize,
        on_page: &mut dyn FnMut(Rows) -> Result<bool, ImportError>,
    ) -> Result<bool, ImportError>;
}

/// Every kind of file a source reads. A new kind adds a variant and its arms.
#[derive(Debug)]
pub enum ImportKind {
    /// A `SQLite` export of a `[[sinks]] kind = "sqlite"`.
    Sqlite(SqliteExport),
}

impl ImportKind {
    /// Opens the export file at `path` for the `sqlite` source kind.
    ///
    /// # Errors
    /// See [`SqliteExport::open`].
    pub fn open_sqlite(path: &Utf8Path) -> Result<Self, ImportError> {
        SqliteExport::open(path).map(Self::Sqlite)
    }
}

impl ImportSource for ImportKind {
    fn fingerprint(&self) -> ImportFingerprint {
        match self {
            Self::Sqlite(file) => file.fingerprint(),
        }
    }

    fn machine(&self) -> Result<Option<MachineId>, ImportError> {
        match self {
            Self::Sqlite(file) => file.machine(),
        }
    }

    fn max_revision(&self, dataset: Dataset) -> Result<Revision, ImportError> {
        match self {
            Self::Sqlite(file) => file.max_revision(dataset),
        }
    }

    fn read(
        &self,
        dataset: Dataset,
        machine: &MachineId,
        after: Revision,
        page_size: usize,
        on_page: &mut dyn FnMut(Rows) -> Result<bool, ImportError>,
    ) -> Result<bool, ImportError> {
        match self {
            Self::Sqlite(file) => file.read(dataset, machine, after, page_size, on_page),
        }
    }
}

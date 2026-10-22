//! Reads the `SQLite` exports of other `TokenMtr` machines into this machine's store.
//!
//! # Model
//!
//! A `[[sources]] kind = "sqlite"` names files (globs) that other machines write with a
//! `[[sinks]] kind = "sqlite"`, usually through a folder a sync client shares. [`import_pending`]
//! does one pass over one source:
//!
//! 1. [`matching_files`] lists the files; a missing folder is not a failure.
//! 2. A file whose size and modification time are those of its last finished read is skipped.
//! 3. Otherwise [`ImportKind::open_sqlite`] copies it to a private temporary file (a sync client
//!    may rewrite the original at any moment), refuses a copy taken while the file moved or a
//!    rollback journal sat beside it, opens the copy read-only, and checks the `_schema` version
//!    against [`SUPPORTED_VERSIONS`] and the columns against `tokenmtr_export::schema`.
//! 4. The rows past the file's cursor are decoded ([`decode`]) and written page by page with
//!    [`tokenmtr_store::Store::import_page`], rows and cursor in one transaction.
//!
//! A file holds one machine's rows. A file of this machine is only remembered; a file whose
//! machine differs from the cursor's, or whose highest revision fell below the cursor's, restarts
//! from revision zero. A row replaces a stored one only with a higher origin revision, so a
//! repeated pass changes nothing.
//!
//! Only `usage_events` and `tool_calls` are read; `extra_usage` rows and everything the export
//! does not carry (quota snapshots, session names, transcripts) stay out. Values keep what the
//! origin exported. With `privacy.redact_paths` the working directory and git branch are hashed
//! with this store's key, and a session id the origin dropped is stored as
//! `redacted:<machine id>`.
//!
//! A failure (busy file, unsupported version, a column missing, several machines in one file, a
//! malformed row) fails that file only: it becomes an `import_run` row with the reason, never a
//! parse issue, and the next pass tries again. Errors never carry row values.

mod decode;
mod error;
mod files;
mod run;
mod source;
mod sqlite;

pub use decode::{tool_call, usage_event};
pub use error::{DecodeError, ImportError, Problem};
pub use files::{include_roots, matching_files};
pub use run::{
    DEFAULT_PAGE_SIZE, FileFailure, ImportOptions, ImportReport, ImportSpec, import_pending,
};
pub use source::{ImportKind, ImportSource, Rows};
pub use sqlite::{SUPPORTED_VERSIONS, SqliteExport, file_stamp};

//! `import_pending`: one pass over a source's files.
//!
//! Per file: skip it when its size and modification time are those of the last finished read;
//! otherwise copy it, check it, and read each dataset past the cursor in pages. Rows and cursor
//! commit together page by page, so a stop or a crash between pages resumes at the cursor and a
//! repeat changes nothing. One file's failure never stops the others; the next pass retries it.

use std::num::NonZeroUsize;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use tokenmtr_config::{GlobList, SourceConfig, SourceTarget};
use tokenmtr_core::{Dataset, SourceId};
use tokenmtr_store::{
    ImportCursor, ImportPage, ImportRun, RedactionKey, Revision, Store, StoreRead,
};

use crate::error::ImportError;
use crate::files::matching_files;
use crate::source::{ImportKind, ImportSource, Rows};
use crate::sqlite::file_stamp;

/// Source rows per page unless configured otherwise.
pub const DEFAULT_PAGE_SIZE: NonZeroUsize = match NonZeroUsize::new(5_000) {
    Some(size) => size,
    None => NonZeroUsize::MIN,
};

/// A `sqlite` source, resolved.
#[derive(Debug, Clone)]
pub struct ImportSpec {
    /// The source's id: the key of its cursors and rows.
    pub source: SourceId,
    /// Files to read.
    pub include: GlobList,
    /// Files to leave out.
    pub exclude: GlobList,
    /// `privacy.redact_paths`: replace imported cwds and git branches by this store's keyed
    /// hashes, as local rows are.
    pub redact_paths: bool,
}

impl ImportSpec {
    /// The spec of a `sqlite` source; `None` for a kind that holds no rows (Superset).
    #[must_use]
    pub fn from_config(source: &SourceConfig, redact_paths: bool) -> Option<Self> {
        match &source.target {
            SourceTarget::Sqlite(sqlite) => Some(Self {
                source: source.id.clone(),
                include: sqlite.include.clone(),
                exclude: sqlite.exclude.clone(),
                redact_paths,
            }),
            SourceTarget::Superset(_) => None,
        }
    }
}

/// Tunables of [`import_pending`].
#[derive(Debug, Clone, Copy)]
pub struct ImportOptions {
    /// Most source rows per page.
    pub page_size: NonZeroUsize,
    /// Time source for `import_run` timestamps.
    pub clock: fn() -> Timestamp,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_PAGE_SIZE,
            clock: Timestamp::now,
        }
    }
}

/// A file (or folder) this pass could not read; the next pass retries it.
#[derive(Debug)]
pub struct FileFailure {
    /// The file, or the folder that could not be listed.
    pub path: Utf8PathBuf,
    /// Why.
    pub error: ImportError,
}

/// What one pass did for one source.
#[derive(Debug)]
pub struct ImportReport {
    /// The source.
    pub source: SourceId,
    /// Files copied and read this pass.
    pub read: u64,
    /// Files skipped because they are as they were at the last finished read.
    pub unchanged: u64,
    /// Files holding this machine's own rows, skipped.
    pub own: u64,
    /// Rows written (inserted or replaced) from `usage_events`.
    pub events: u64,
    /// Rows written from `tool_calls`.
    pub tool_calls: u64,
    /// Files and folders that failed.
    pub failures: Vec<FileFailure>,
    /// The pass stopped early because `stopping` said so.
    pub stopped: bool,
}

impl ImportReport {
    /// Rows written, both datasets.
    #[must_use]
    pub const fn rows(&self) -> u64 {
        self.events + self.tool_calls
    }
}

/// Reads everything new in the files of `spec` into `store`.
///
/// `stopping` is asked between pages and files; a pass that stops resumes at the cursors.
pub fn import_pending(
    store: &mut Store,
    spec: &ImportSpec,
    options: &ImportOptions,
    stopping: &dyn Fn() -> bool,
) -> ImportReport {
    let started = (options.clock)();
    let (files, folder_failures) = matching_files(&spec.include, &spec.exclude);
    let mut report = ImportReport {
        source: spec.source.clone(),
        read: 0,
        unchanged: 0,
        own: 0,
        events: 0,
        tool_calls: 0,
        failures: Vec::new(),
        stopped: false,
    };
    for (path, error) in folder_failures {
        record_failure(store, spec, options, &path, started, &error);
        report.failures.push(FileFailure { path, error });
    }
    for path in files {
        if stopping() {
            report.stopped = true;
            break;
        }
        let file_started = (options.clock)();
        if let Err(error) = import_file(store, spec, options, &path, stopping, &mut report) {
            record_failure(store, spec, options, &path, file_started, &error);
            report.failures.push(FileFailure { path, error });
        }
    }
    let error = report.failures.first().map(|f| {
        format!(
            "{} of the files failed; first: {}",
            report.failures.len(),
            f.error
        )
    });
    let summary = ImportRun {
        source: spec.source.clone(),
        path: None,
        dataset: None,
        started,
        finished: (options.clock)(),
        rows: report.rows(),
        error,
    };
    if let Err(record_error) = store.record_import_run(&summary) {
        tracing::warn!(source = %spec.source, %record_error, "could not record the import run");
    }
    report
}

/// Records a failed file; a store failure here only costs the history row.
fn record_failure(
    store: &mut Store,
    spec: &ImportSpec,
    options: &ImportOptions,
    path: &Utf8Path,
    started: Timestamp,
    error: &ImportError,
) {
    let run = ImportRun {
        source: spec.source.clone(),
        path: Some(path.to_string()),
        dataset: None,
        started,
        finished: (options.clock)(),
        rows: 0,
        error: Some(error.to_string()),
    };
    if let Err(record_error) = store.record_import_run(&run) {
        tracing::warn!(source = %spec.source, %record_error, "could not record a failed import");
    }
}

fn import_file(
    store: &mut Store,
    spec: &ImportSpec,
    options: &ImportOptions,
    path: &Utf8Path,
    stopping: &dyn Fn() -> bool,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let (size, mtime) = file_stamp(path)?;
    let cursor = store.import_cursor(&spec.source, path)?;
    let unchanged = cursor
        .as_ref()
        .and_then(|c| c.fingerprint)
        .is_some_and(|f| f.size == size && f.mtime == mtime);
    if unchanged {
        report.unchanged += 1;
        return Ok(());
    }
    let file = ImportKind::open_sqlite(path)?;
    report.read += 1;
    let Some(machine) = file.machine()? else {
        return Ok(());
    };
    if machine == store.machine_id() {
        // Our own sink's file: remembered so an unchanged one is not copied again.
        report.own += 1;
        store.set_import_fingerprint(&spec.source, path, machine, &file.fingerprint())?;
        return Ok(());
    }
    let mut cursor = cursor.filter(|c| c.machine == machine);
    if let Some(known) = &cursor
        && erased(&file, known)?
    {
        cursor = None;
    }
    if cursor.is_none() {
        store.reset_import_cursor(&spec.source, path)?;
    }
    let key = spec.redact_paths.then(|| store.redaction_key().clone());
    for dataset in [Dataset::UsageEvents, Dataset::ToolCalls] {
        let after = cursor.as_ref().map_or(Revision::ZERO, |c| match dataset {
            Dataset::ToolCalls => c.tool_calls,
            _ => c.usage_events,
        });
        let finished = file.read(
            dataset,
            &machine,
            after,
            options.page_size.get(),
            &mut |mut rows: Rows| {
                if let Some(key) = &key {
                    redact(key, &mut rows);
                }
                let stats = store.import_page(&ImportPage {
                    source: &spec.source,
                    path,
                    machine,
                    events: &rows.events,
                    tool_calls: &rows.tool_calls,
                })?;
                report.events += stats.events.changed();
                report.tool_calls += stats.tool_calls.changed();
                Ok(!stopping())
            },
        )?;
        if !finished {
            report.stopped = true;
            return Ok(());
        }
    }
    store.set_import_fingerprint(&spec.source, path, machine, &file.fingerprint())?;
    Ok(())
}

/// Whether the file's rows end before the cursor: its origin was erased or reinstalled and
/// counts again from one.
fn erased(file: &impl ImportSource, cursor: &ImportCursor) -> Result<bool, ImportError> {
    Ok(
        file.max_revision(Dataset::UsageEvents)? < cursor.usage_events
            || file.max_revision(Dataset::ToolCalls)? < cursor.tool_calls,
    )
}

/// `privacy.redact_paths`: working directories and branches leave as this store's keyed hashes.
fn redact(key: &RedactionKey, rows: &mut Rows) {
    for row in &mut rows.events {
        let event = &mut row.event;
        event.cwd = event
            .cwd
            .take()
            .map(|cwd| Utf8PathBuf::from(key.hash(cwd.as_str())));
        event.git_branch = event.git_branch.take().map(|branch| key.hash(&branch));
    }
}

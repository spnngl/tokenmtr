//! `import_pending` end to end: a store's rows leave through a `SqliteSink` export and come back
//! into another store through a `[[sources]]`-style spec.

use std::cell::Cell;
use std::fs;
use std::str::FromStr;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use rusqlite::Connection;
use tempfile::TempDir;
use tokenmtr_config::GlobList;
use tokenmtr_core::{
    AgentId, DedupKey, Estimate, EventCost, EventId, EventKind, MachineId, PricingVersion,
    ProfileId, ProjectKey, ProviderId, SessionId, SinkId, SkillName, SourceId, TokenCounts,
    ToolCall, UsageEvent, UsdMicros,
};
use tokenmtr_export::{
    EXPORT_SCHEMA_VERSION, ExportBatch, Sink, SqliteSink, SqliteTarget, project,
};
use tokenmtr_import::{
    ImportError, ImportOptions, ImportReport, ImportSpec, SUPPORTED_VERSIONS, import_pending,
};
use tokenmtr_store::{
    Dataset, ImportedRows, ProfiledToolCall, Revision, Store, StoreRead, StoredEvent, Versioned,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const AT: &str = "2026-05-01T10:00:00Z";

fn clock() -> Timestamp {
    Timestamp::constant(1_777_680_000, 0)
}

fn utf8(dir: &TempDir, name: &str) -> TestResult<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(dir.path().join(name))
        .map_err(|p| format!("non-UTF-8 {}", p.display()).into())
}

fn open_store(dir: &TempDir) -> TestResult<Store> {
    Ok(Store::open(&utf8(dir, "usage.db")?)?.with_clock(clock))
}

fn machine(n: u8) -> TestResult<MachineId> {
    Ok(MachineId::from_str(&format!(
        "00000000-0000-0000-0000-0000000000{n:02}"
    ))?)
}

fn source() -> TestResult<SourceId> {
    Ok(SourceId::try_from("laptop")?)
}

fn options() -> ImportOptions {
    ImportOptions {
        clock,
        ..ImportOptions::default()
    }
}

/// A source reading every `.db` file of `dir`.
fn spec(dir: &Utf8Path, redact_paths: bool) -> TestResult<ImportSpec> {
    Ok(ImportSpec {
        source: source()?,
        include: GlobList::new(vec![format!("{dir}/*.db")])?,
        exclude: GlobList::new(vec![])?,
        redact_paths,
    })
}

fn run(store: &mut Store, spec: &ImportSpec) -> ImportReport {
    import_pending(store, spec, &options(), &|| false)
}

fn event(machine: &MachineId, session: &str, output: u64) -> TestResult<UsageEvent> {
    let dedup_key = DedupKey::try_from(format!("{session}|{AT}"))?;
    Ok(UsageEvent {
        id: EventId::new(machine, AgentId::CLAUDE, &dedup_key),
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        dedup_key,
        at: AT.parse()?,
        session: SessionId::try_from(session)?,
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: Some(Utf8PathBuf::from("/work/acme")),
        git_branch: Some("feature/import".to_owned()),
        project: Some(ProjectKey::try_from("acme-api")?),
        client: Some("Acme".to_owned()),
        source_file: None,
        approximate: false,
        provider: Some(ProviderId::try_from("anthropic")?),
        model: tokenmtr_core::ModelId::try_from("claude-sonnet-4")?,
        kind: EventKind::Message,
        tokens: TokenCounts {
            input_uncached: 100,
            cache_read: 2_000,
            cache_write_5m: 30,
            cache_write_1h: 4,
            output,
            reasoning: 0,
        },
        cost: EventCost::new(
            Some(UsdMicros::new(1_500_000)),
            Some(Estimate {
                usd: UsdMicros::new(1_234_567),
                pricing_version: PricingVersion::of_body(b"snapshot"),
            }),
        ),
        service_tier: Some("standard".to_owned()),
    })
}

fn call(session: &str) -> TestResult<ProfiledToolCall> {
    Ok(ProfiledToolCall {
        profile: ProfileId::default_profile(),
        call: ToolCall {
            agent: AgentId::CLAUDE,
            tool_use_id: format!("toolu_{session}"),
            event: None,
            session: SessionId::try_from(session)?,
            at: AT.parse()?,
            tool_name: "search".to_owned(),
            mcp_server: Some("docs".to_owned()),
            is_error: Some(false),
            skill: Some(SkillName::try_from("git-commit")?),
        },
    })
}

/// Writes `events` and `calls` (with their revisions) into the export at `path`, as the sink of
/// `machine` does.
fn export(
    path: &Utf8Path,
    machine: &MachineId,
    events: &[(u64, UsageEvent)],
    calls: &[(u64, ProfiledToolCall)],
) -> TestResult {
    let mut sink = SqliteSink::new(
        SinkId::try_from("db")?,
        SqliteTarget {
            path: path.to_owned(),
        },
    );
    let rows = events
        .iter()
        .map(|(revision, event)| {
            project::usage_event(
                machine,
                &Versioned {
                    revision: Revision::new(*revision),
                    row: StoredEvent {
                        event: event.clone(),
                        superseded: false,
                    },
                },
            )
        })
        .collect::<Vec<_>>();
    if !rows.is_empty() {
        sink.write(&ExportBatch {
            dataset: Dataset::UsageEvents,
            rows,
        })?;
    }
    let rows = calls
        .iter()
        .map(|(revision, call)| {
            project::tool_call(
                machine,
                &Versioned {
                    revision: Revision::new(*revision),
                    row: call.clone(),
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !rows.is_empty() {
        sink.write(&ExportBatch {
            dataset: Dataset::ToolCalls,
            rows,
        })?;
    }
    Ok(())
}

struct Fixture {
    dir: TempDir,
    exports: Utf8PathBuf,
    store: Store,
}

fn fixture() -> TestResult<Fixture> {
    let dir = TempDir::new()?;
    let exports = utf8(&dir, "exports")?;
    fs::create_dir(&exports)?;
    let store = open_store(&dir)?;
    Ok(Fixture {
        dir,
        exports,
        store,
    })
}

impl Fixture {
    /// One pass of the source over the export folder.
    fn pass(&mut self) -> TestResult<ImportReport> {
        let spec = self.spec()?;
        Ok(run(&mut self.store, &spec))
    }

    fn pass_redacted(&mut self) -> TestResult<ImportReport> {
        let spec = spec(&self.exports, true)?;
        Ok(run(&mut self.store, &spec))
    }

    fn pass_with(
        &mut self,
        options: &ImportOptions,
        stopping: &dyn Fn() -> bool,
    ) -> TestResult<ImportReport> {
        let spec = self.spec()?;
        Ok(import_pending(&mut self.store, &spec, options, stopping))
    }

    fn file(&self, name: &str) -> Utf8PathBuf {
        self.exports.join(name)
    }

    fn spec(&self) -> TestResult<ImportSpec> {
        spec(&self.exports, false)
    }

    fn rows(&self) -> TestResult<ImportedRows> {
        Ok(self.store.imported_rows(&source()?)?)
    }

    /// The error of the one failed file of `report`.
    fn failure(report: &ImportReport) -> Option<&ImportError> {
        report.failures.first().map(|f| &f.error)
    }
}

#[test]
fn supported_versions_cover_the_current_export_schema() {
    assert!(SUPPORTED_VERSIONS.contains(&EXPORT_SCHEMA_VERSION));
}

#[test]
fn an_export_comes_back_with_the_values_it_left_with() -> TestResult {
    let mut f = fixture()?;
    let origin = machine(1)?;
    let original = event(&origin, "s1", 500)?;
    let path = f.file("laptop.db");
    export(
        &path,
        &origin,
        &[(1, original.clone()), (2, event(&origin, "s2", 7)?)],
        &[(1, call("s1")?)],
    )?;

    let report = f.pass()?;

    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!((report.events, report.tool_calls, report.read), (2, 1, 1));
    assert_eq!(
        f.rows()?,
        ImportedRows {
            events: 2,
            tool_calls: 1
        }
    );
    let stored = f
        .store
        .usage_event(&original.id)?
        .ok_or("event not imported")?
        .row;
    let expected = UsageEvent {
        dedup_key: stored.dedup_key.clone(),
        ..original
    };
    assert_eq!(stored, expected);
    assert!(stored.dedup_key.as_str().starts_with("import:"));
    Ok(())
}

#[test]
fn a_repeated_pass_changes_nothing() -> TestResult {
    let mut f = fixture()?;
    let origin = machine(1)?;
    export(
        &f.file("laptop.db"),
        &origin,
        &[(1, event(&origin, "s1", 5)?)],
        &[(1, call("s1")?)],
    )?;
    let first = f.pass()?;
    let head = f.store.head_revision(Dataset::UsageEvents)?;

    let second = f.pass()?;

    assert_eq!((first.rows(), second.rows()), (2, 0));
    assert_eq!((second.read, second.unchanged), (0, 1));
    assert_eq!(f.store.head_revision(Dataset::UsageEvents)?, head);
    Ok(())
}

#[test]
fn a_higher_revision_replaces_the_row_and_a_lower_one_does_not() -> TestResult {
    let mut f = fixture()?;
    let origin = machine(1)?;
    let path = f.file("laptop.db");
    export(&path, &origin, &[(5, event(&origin, "s1", 5)?)], &[])?;
    f.pass()?;

    export(
        &path,
        &origin,
        &[(6, event(&origin, "s1", 9)?), (7, event(&origin, "s2", 1)?)],
        &[],
    )?;
    let report = f.pass()?;

    assert_eq!(report.events, 2);
    let id = event(&origin, "s1", 0)?.id;
    let stored = f.store.usage_event(&id)?.ok_or("missing")?.row;
    assert_eq!(stored.tokens.output, 9);
    Ok(())
}

#[test]
fn a_file_renamed_over_the_old_one_resumes_at_the_cursor() -> TestResult {
    let mut f = fixture()?;
    let origin = machine(1)?;
    let path = f.file("laptop.db");
    let one = event(&origin, "s1", 1)?;
    let two = event(&origin, "s2", 2)?;
    export(&path, &origin, &[(1, one.clone()), (2, two.clone())], &[])?;
    f.pass()?;

    let staged = utf8(&f.dir, "staged.sqlite")?;
    export(
        &staged,
        &origin,
        &[(1, one), (2, two), (3, event(&origin, "s3", 3)?)],
        &[],
    )?;
    fs::rename(&staged, &path)?;
    let report = f.pass()?;

    assert_eq!(report.events, 1);
    assert_eq!(f.rows()?.events, 3);
    Ok(())
}

#[test]
fn another_machine_in_the_same_file_restarts_the_cursor() -> TestResult {
    let mut f = fixture()?;
    let path = f.file("laptop.db");
    let old = machine(1)?;
    export(
        &path,
        &old,
        &[(1, event(&old, "s1", 1)?), (2, event(&old, "s2", 2)?)],
        &[],
    )?;
    f.pass()?;

    let reinstalled = machine(2)?;
    let staged = utf8(&f.dir, "staged.sqlite")?;
    export(
        &staged,
        &reinstalled,
        &[(1, event(&reinstalled, "s9", 9)?)],
        &[],
    )?;
    fs::rename(&staged, &path)?;
    let report = f.pass()?;

    assert_eq!(report.events, 1);
    assert_eq!(f.rows()?.events, 3);
    Ok(())
}

#[test]
fn a_file_that_ends_before_the_cursor_restarts_it() -> TestResult {
    let mut f = fixture()?;
    let path = f.file("laptop.db");
    let origin = machine(1)?;
    export(&path, &origin, &[(40, event(&origin, "s1", 1)?)], &[])?;
    f.pass()?;

    let staged = utf8(&f.dir, "staged.sqlite")?;
    export(&staged, &origin, &[(1, event(&origin, "s2", 2)?)], &[])?;
    fs::rename(&staged, &path)?;
    let report = f.pass()?;

    assert_eq!(report.events, 1);
    assert_eq!(f.rows()?.events, 2);
    Ok(())
}

#[test]
fn a_conflicted_copy_adds_no_row() -> TestResult {
    let mut f = fixture()?;
    let origin = machine(1)?;
    let rows = [(1, event(&origin, "s1", 1)?)];
    export(&f.file("laptop.db"), &origin, &rows, &[])?;
    f.pass()?;

    export(&f.file("laptop (conflicted copy).db"), &origin, &rows, &[])?;
    let report = f.pass()?;

    assert!(report.failures.is_empty());
    assert_eq!((report.rows(), report.read), (0, 1));
    assert_eq!(f.rows()?.events, 1);
    Ok(())
}

#[test]
fn a_file_with_a_journal_beside_it_is_retried_not_read() -> TestResult {
    let mut f = fixture()?;
    let origin = machine(1)?;
    let other = machine(2)?;
    let busy = f.file("busy.db");
    export(&busy, &origin, &[(1, event(&origin, "s1", 1)?)], &[])?;
    fs::write(format!("{busy}-journal"), b"half a transaction")?;
    export(
        &f.file("fine.db"),
        &other,
        &[(1, event(&other, "s2", 2)?)],
        &[],
    )?;

    let report = f.pass()?;

    assert!(matches!(Fixture::failure(&report), Some(ImportError::Busy)));
    assert_eq!(f.rows()?.events, 1);
    fs::remove_file(format!("{busy}-journal"))?;
    assert_eq!(f.pass()?.events, 1);
    Ok(())
}

#[test]
fn the_failed_file_is_in_the_run_history() -> TestResult {
    let mut f = fixture()?;
    fs::write(f.file("junk.db"), b"not a database at all")?;

    let report = f.pass()?;

    assert_eq!(report.failures.len(), 1);
    let runs = f.store.import_runs(&source()?, 10)?;
    assert!(runs.iter().any(|r| r.path.is_some() && r.error.is_some()));
    assert!(runs.iter().any(|r| r.path.is_none() && r.error.is_some()));
    Ok(())
}

#[test]
fn this_machines_own_export_is_skipped() -> TestResult {
    let mut f = fixture()?;
    let own = f.store.machine_id();
    export(&f.file("own.db"), &own, &[(1, event(&own, "s1", 1)?)], &[])?;

    let first = f.pass()?;
    let second = f.pass()?;

    assert_eq!((first.own, first.rows()), (1, 0));
    assert_eq!((second.own, second.unchanged), (0, 1));
    assert_eq!(f.rows()?, ImportedRows::default());
    Ok(())
}

#[test]
fn a_bad_file_fails_alone() -> TestResult {
    let mut f = fixture()?;
    let good = machine(1)?;
    export(
        &f.file("a-good.db"),
        &good,
        &[(1, event(&good, "s1", 1)?)],
        &[],
    )?;

    let wrong_version = machine(2)?;
    let path = f.file("b-version.db");
    export(
        &path,
        &wrong_version,
        &[(1, event(&wrong_version, "s2", 1)?)],
        &[],
    )?;
    Connection::open(&path)?.execute("UPDATE _schema SET version = 3", [])?;

    let missing_column = machine(3)?;
    let path = f.file("c-column.db");
    export(
        &path,
        &missing_column,
        &[(1, event(&missing_column, "s3", 1)?)],
        &[],
    )?;
    Connection::open(&path)?.execute("ALTER TABLE usage_events DROP COLUMN client", [])?;

    let not_export = f.file("d-other.db");
    Connection::open(&not_export)?.execute("CREATE TABLE notes (id INTEGER)", [])?;

    let first = machine(4)?;
    let second = machine(5)?;
    let path = f.file("e-several.db");
    export(&path, &first, &[(1, event(&first, "s4", 1)?)], &[])?;
    export(&path, &second, &[(2, event(&second, "s5", 1)?)], &[])?;

    let report = f.pass()?;

    let errors = report
        .failures
        .iter()
        .map(|failure| &failure.error)
        .collect::<Vec<_>>();
    assert!(
        matches!(
            errors.as_slice(),
            [
                ImportError::Version { .. },
                ImportError::MissingColumn { .. },
                ImportError::NotAnExport,
                ImportError::SeveralMachines,
            ]
        ),
        "{errors:?}"
    );
    assert_eq!(f.rows()?.events, 1, "{errors:?}");
    Ok(())
}

#[test]
fn extra_usage_rows_stay_out() -> TestResult {
    let mut f = fixture()?;
    let origin = machine(1)?;
    let mut extra = event(&origin, "s2", 0)?;
    extra.kind = EventKind::ExtraUsage;
    export(
        &f.file("laptop.db"),
        &origin,
        &[(1, event(&origin, "s1", 1)?), (2, extra)],
        &[],
    )?;

    let report = f.pass()?;

    assert_eq!(report.events, 1);
    assert_eq!(f.rows()?.events, 1);
    Ok(())
}

#[test]
fn redact_paths_hashes_the_working_directory_and_the_branch() -> TestResult {
    let mut f = fixture()?;
    let origin = machine(1)?;
    let original = event(&origin, "s1", 1)?;
    export(&f.file("laptop.db"), &origin, &[(1, original.clone())], &[])?;

    f.pass_redacted()?;

    let stored = f.store.usage_event(&original.id)?.ok_or("missing")?.row;
    let key = f.store.redaction_key().clone();
    assert_eq!(stored.cwd, Some(Utf8PathBuf::from(key.hash("/work/acme"))));
    assert_eq!(stored.git_branch, Some(key.hash("feature/import")));
    Ok(())
}

#[test]
fn a_pass_that_stops_resumes_at_the_cursor() -> TestResult {
    let mut f = fixture()?;
    let origin = machine(1)?;
    export(
        &f.file("laptop.db"),
        &origin,
        &[
            (1, event(&origin, "s1", 1)?),
            (2, event(&origin, "s2", 2)?),
            (3, event(&origin, "s3", 3)?),
        ],
        &[],
    )?;
    let options = ImportOptions {
        page_size: std::num::NonZeroUsize::MIN,
        clock,
    };
    let asked = Cell::new(0);
    let stopping = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };

    let stopped = f.pass_with(&options, &stopping)?;
    let resumed = f.pass_with(&options, &|| false)?;

    assert!(stopped.stopped);
    assert_eq!((stopped.events, resumed.events), (1, 2));
    assert!(!resumed.stopped);
    Ok(())
}

#[test]
fn a_folder_that_does_not_exist_is_not_a_failure() -> TestResult {
    let mut f = fixture()?;
    let missing = utf8(&f.dir, "not-synced-yet")?;

    let report = run(&mut f.store, &spec(&missing, false)?);

    assert!(report.failures.is_empty());
    assert_eq!(report.read, 0);
    Ok(())
}

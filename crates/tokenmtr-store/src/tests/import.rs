//! Imported rows: stored apart from local ones, replaced by origin revision, forgotten per source.

#![expect(clippy::assert_is_empty, reason = "emptiness asserts state the intent")]

use super::*;
use crate::import::{ImportFingerprint, ImportPage, ImportRun, ImportedEvent, ImportedToolCall};
use crate::{ProjectSource, RepriceCursor, RepriceScope};

fn origin() -> MachineId {
    "0190a6f2-7c1e-7b4a-9d3e-1a2b3c4d5e6f".parse().unwrap()
}

fn source(name: &str) -> SourceId {
    SourceId::try_from(name.to_owned()).unwrap()
}

fn imported(machine: &MachineId, key: &str, output: u64, revision: u64) -> ImportedEvent {
    ImportedEvent {
        event: event(machine, key, output),
        superseded: false,
        origin_revision: Revision::new(revision),
    }
}

fn imported_call(machine: &MachineId, key: &str, revision: u64) -> ImportedToolCall {
    let dedup_key = DedupKey::try_from(format!("tool_call:{key}")).unwrap();
    ImportedToolCall {
        id: EventId::new(machine, AgentId::CLAUDE, &dedup_key),
        call: ProfiledToolCall {
            profile: ProfileId::default_profile(),
            call: ToolCall {
                agent: AgentId::CLAUDE,
                tool_use_id: key.to_owned(),
                event: None,
                session: SessionId::try_from("s1".to_owned()).unwrap(),
                at: at(1_700_000_000),
                tool_name: "Bash".to_owned(),
                mcp_server: None,
                is_error: Some(false),
                skill: None,
            },
        },
        origin_revision: Revision::new(revision),
    }
}

fn page<'a>(
    source: &'a SourceId,
    path: &'a Utf8Path,
    events: &'a [ImportedEvent],
    tool_calls: &'a [ImportedToolCall],
) -> ImportPage<'a> {
    ImportPage {
        source,
        path,
        machine: origin(),
        events,
        tool_calls,
    }
}

#[test]
fn imported_rows_advance_the_head_but_never_reach_an_exporter() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    f.store
        .upsert_events(&[event(&machine, "local", 1)])
        .unwrap();
    let laptop = source("laptop");
    let path = Utf8Path::new("/x/laptop.db");
    let events = [
        imported(&origin(), "a", 5, 1),
        imported(&origin(), "b", 6, 2),
    ];
    let calls = [imported_call(&origin(), "t1", 1)];
    let before = head(&f.store, Dataset::UsageEvents);

    let stats = f
        .store
        .import_page(&page(&laptop, path, &events, &calls))
        .unwrap();
    assert_eq!((stats.events.inserted, stats.tool_calls.inserted), (2, 1));

    assert!(head(&f.store, Dataset::UsageEvents) > before);
    assert_eq!(head(&f.store, Dataset::ToolCalls), Revision::new(1));
    let exported: Vec<_> = f
        .store
        .usage_events_after(Revision::ZERO, 10)
        .unwrap()
        .into_iter()
        .map(|v| v.row.dedup_key.as_str().to_owned())
        .collect();
    assert_eq!(exported, ["local"]);
    assert_eq!(
        f.store
            .stored_events_after(Revision::ZERO, 10)
            .unwrap()
            .len(),
        1
    );
    assert!(
        f.store
            .tool_calls_after(Revision::ZERO, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.store
            .pending(Dataset::UsageEvents, Revision::ZERO)
            .unwrap()
            .rows,
        1
    );
    assert_eq!(
        f.store
            .pending(Dataset::ToolCalls, Revision::ZERO)
            .unwrap()
            .rows,
        0
    );
}

#[test]
fn imported_rows_are_found_by_their_exported_id_with_neutral_values() {
    let mut f = fixture();
    let laptop = source("laptop");
    let path = Utf8Path::new("/x/laptop.db");
    let mut aggregate = imported(&origin(), "a", 5, 1);
    aggregate.superseded = true;
    aggregate.event.subagent = Some("sub".to_owned());
    aggregate.event.is_sidechain = true;
    let id = aggregate.event.id;
    f.store
        .import_page(&page(&laptop, path, std::slice::from_ref(&aggregate), &[]))
        .unwrap();

    let found = f.store.usage_event(&id).unwrap().unwrap().row;
    assert!(found.dedup_key.as_str().starts_with("import:"));
    assert_eq!(
        (found.subagent, found.is_sidechain, found.source_file),
        (None, false, None)
    );
    assert_eq!(found.tokens.output, 5);
    let (superseded, source): (bool, String) = f
        .store
        .raw()
        .query_row(
            "SELECT superseded, origin_source FROM usage_event WHERE event_id = ?1",
            [id.as_bytes()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((superseded, source.as_str()), (true, "laptop"));
}

#[test]
fn a_row_is_replaced_only_by_a_higher_origin_revision() {
    let mut f = fixture();
    let laptop = source("laptop");
    let path = Utf8Path::new("/x/laptop.db");
    let first = [imported(&origin(), "a", 5, 3)];
    f.store
        .import_page(&page(&laptop, path, &first, &[]))
        .unwrap();
    let after_first = head(&f.store, Dataset::UsageEvents);

    for stale in [
        imported(&origin(), "a", 9, 3),
        imported(&origin(), "a", 9, 2),
    ] {
        let stats = f
            .store
            .import_page(&page(&laptop, path, std::slice::from_ref(&stale), &[]))
            .unwrap();
        assert_eq!(
            (
                stats.events.inserted,
                stats.events.updated,
                stats.events.unchanged
            ),
            (0, 0, 1)
        );
    }
    assert_eq!(head(&f.store, Dataset::UsageEvents), after_first);

    let newer = [imported(&origin(), "a", 9, 4)];
    let stats = f
        .store
        .import_page(&page(&laptop, path, &newer, &[]))
        .unwrap();
    assert_eq!(stats.events.updated, 1);
    assert_eq!(head(&f.store, Dataset::UsageEvents), after_first.next());
    let stored = f
        .store
        .usage_event(&newer[0].event.id)
        .unwrap()
        .unwrap()
        .row;
    assert_eq!(stored.tokens.output, 9);
}

#[test]
fn the_cursor_follows_pages_and_the_fingerprint_marks_a_finished_read() {
    let mut f = fixture();
    let laptop = source("laptop");
    let path = Utf8Path::new("/x/laptop.db");
    assert_eq!(f.store.import_cursor(&laptop, path).unwrap(), None);

    let events = [imported(&origin(), "a", 1, 7)];
    let calls = [imported_call(&origin(), "t", 4)];
    f.store
        .import_page(&page(&laptop, path, &events, &calls))
        .unwrap();
    let cursor = f.store.import_cursor(&laptop, path).unwrap().unwrap();
    assert_eq!(
        (
            cursor.machine,
            cursor.usage_events,
            cursor.tool_calls,
            cursor.fingerprint
        ),
        (origin(), Revision::new(7), Revision::new(4), None)
    );

    // A page of one dataset leaves the other's position alone and never moves backwards.
    let more = [imported(&origin(), "b", 1, 5)];
    f.store
        .import_page(&page(&laptop, path, &more, &[]))
        .unwrap();
    let cursor = f.store.import_cursor(&laptop, path).unwrap().unwrap();
    assert_eq!(
        (cursor.usage_events, cursor.tool_calls),
        (Revision::new(7), Revision::new(4))
    );

    let fingerprint = ImportFingerprint {
        size: 4096,
        mtime: at(1_700_000_000),
        schema_version: 4,
    };
    f.store
        .set_import_fingerprint(&laptop, path, origin(), &fingerprint)
        .unwrap();
    let cursor = f.store.import_cursor(&laptop, path).unwrap().unwrap();
    assert_eq!(cursor.fingerprint, Some(fingerprint));
    assert_eq!(cursor.usage_events, Revision::new(7));

    // The next page clears it; a reset forgets the file.
    f.store
        .import_page(&page(&laptop, path, &more, &[]))
        .unwrap();
    assert_eq!(
        f.store
            .import_cursor(&laptop, path)
            .unwrap()
            .unwrap()
            .fingerprint,
        None
    );
    assert_eq!(f.store.import_cursors(&laptop).unwrap().len(), 1);
    f.store.reset_import_cursor(&laptop, path).unwrap();
    assert_eq!(f.store.import_cursor(&laptop, path).unwrap(), None);
}

#[test]
fn an_empty_export_still_gets_a_cursor_from_its_fingerprint() {
    let mut f = fixture();
    let laptop = source("laptop");
    let path = Utf8Path::new("/x/empty.db");
    let fingerprint = ImportFingerprint {
        size: 8192,
        mtime: at(5),
        schema_version: 4,
    };
    f.store
        .set_import_fingerprint(&laptop, path, origin(), &fingerprint)
        .unwrap();
    let cursor = f.store.import_cursor(&laptop, path).unwrap().unwrap();
    assert_eq!(
        (cursor.usage_events, cursor.tool_calls, cursor.fingerprint),
        (Revision::ZERO, Revision::ZERO, Some(fingerprint))
    );
}

#[test]
fn runs_come_back_newest_first_and_the_history_is_bounded() {
    let mut f = fixture();
    let laptop = source("laptop");
    let run = |second: i64, error: Option<&str>| ImportRun {
        source: laptop.clone(),
        path: Some("/x/laptop.db".to_owned()),
        dataset: Some(Dataset::UsageEvents),
        started: at(second),
        finished: at(second + 1),
        rows: 3,
        error: error.map(str::to_owned),
    };
    f.store.record_import_run(&run(10, None)).unwrap();
    f.store.record_import_run(&run(20, Some("busy"))).unwrap();
    let runs = f.store.import_runs(&laptop, 10).unwrap();
    assert_eq!(
        runs.iter()
            .map(|r| (r.started, r.error.as_deref()))
            .collect::<Vec<_>>(),
        [(at(20), Some("busy")), (at(10), None)]
    );
    assert_eq!(runs[0].dataset, Some(Dataset::UsageEvents));
    assert_eq!(f.store.import_runs(&laptop, 1).unwrap().len(), 1);

    let conn = f.store.raw();
    for second in 0..5 {
        crate::import::record_run(conn, &run(100 + second, None), 3).unwrap();
    }
    let kept: i64 = conn
        .query_row("SELECT COUNT(*) FROM import_run", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept, 3);
}

#[test]
fn forgetting_a_source_deletes_only_what_it_imported() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    f.store
        .upsert_events(&[event(&machine, "local", 1)])
        .unwrap();
    let (laptop, desktop) = (source("laptop"), source("desktop"));
    let path = Utf8Path::new("/x/laptop.db");
    let other = Utf8Path::new("/x/desktop.db");
    let a = [
        imported(&origin(), "a", 1, 1),
        imported(&origin(), "b", 1, 2),
    ];
    let calls = [imported_call(&origin(), "t", 1)];
    f.store
        .import_page(&page(&laptop, path, &a, &calls))
        .unwrap();
    let keep = [imported(&origin(), "c", 1, 1)];
    f.store
        .import_page(&page(&desktop, other, &keep, &[]))
        .unwrap();
    f.store
        .record_import_run(&ImportRun {
            source: laptop.clone(),
            path: None,
            dataset: None,
            started: at(1),
            finished: at(2),
            rows: 0,
            error: Some("gone".to_owned()),
        })
        .unwrap();
    assert_eq!(f.store.imported_rows(&laptop).unwrap().events, 2);

    let deleted = f.store.forget_source(&laptop).unwrap();
    assert_eq!((deleted.events, deleted.tool_calls), (2, 1));
    assert_eq!(f.store.imported_rows(&laptop).unwrap().events, 0);
    assert_eq!(f.store.import_cursor(&laptop, path).unwrap(), None);
    assert!(f.store.import_runs(&laptop, 10).unwrap().is_empty());
    assert_eq!(f.store.imported_rows(&desktop).unwrap().events, 1);
    assert!(f.store.import_cursor(&desktop, other).unwrap().is_some());
    assert!(
        f.store
            .usage_event_by_key(
                AgentId::CLAUDE,
                &DedupKey::try_from("local".to_owned()).unwrap()
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn local_maintenance_never_touches_imported_rows() {
    let mut f = fixture();
    let laptop = source("laptop");
    let path = Utf8Path::new("/x/laptop.db");
    let mut foreign = imported(&origin(), "a", 5, 1);
    foreign.event.cwd = Some(Utf8PathBuf::from("/remote/dir"));
    let id = foreign.event.id;
    f.store
        .import_page(&page(&laptop, path, std::slice::from_ref(&foreign), &[]))
        .unwrap();
    let before = head(&f.store, Dataset::UsageEvents);

    // Directories of imported rows are not this machine's to resolve or stamp.
    assert!(f.store.unresolved_cwds().unwrap().is_empty());
    let resolution = crate::ProjectResolution {
        source: ProjectSource::Rule("r".to_owned()),
        ..super::api::resolution("/remote/dir", "remote", None)
    };
    assert_eq!(f.store.reassign_projects(&[resolution]).unwrap(), 0);
    assert_eq!(
        f.store
            .fill_branches(&[(Utf8PathBuf::from("/remote/dir"), "main".to_owned())])
            .unwrap(),
        0
    );
    assert!(
        f.store
            .usage_events_by_cwd(Utf8Path::new("/remote/dir"), None, 10)
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        f.store
            .earliest_transcript_event(AgentId::CLAUDE, &ProfileId::default_profile())
            .unwrap(),
        None
    );
    let zone = jiff::tz::TimeZone::UTC;
    assert_eq!(
        f.store
            .supersede_daily_aggregates_covered_by_transcripts(
                AgentId::CLAUDE,
                &ProfileId::default_profile(),
                &zone
            )
            .unwrap(),
        0
    );
    let page = f
        .store
        .reprice(RepriceScope::All, RepriceCursor::default(), 10, |_| None)
        .unwrap();
    assert_eq!(page.repriced, 0);
    assert_eq!(head(&f.store, Dataset::UsageEvents), before);
    let stored = f.store.usage_event(&id).unwrap().unwrap().row;
    assert_eq!((stored.project, stored.git_branch), (None, None));
}

#[test]
fn the_export_reads_still_walk_the_revision_index() {
    let mut f = fixture();
    let mut plan = |sql: &str| -> String {
        let mut statement = f
            .store
            .raw()
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let params = vec![rusqlite::types::Value::Integer(0); statement.parameter_count()];
        statement
            .query_map(rusqlite::params_from_iter(params), |r| {
                r.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n")
    };
    for sql in [crate::events::AFTER, crate::tool_calls::AFTER] {
        let found = plan(sql);
        assert!(found.contains("revision"), "{found}");
        assert!(!found.contains("TEMP B-TREE"), "{found}");
    }
}

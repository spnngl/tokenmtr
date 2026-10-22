//! The ingest write, point lookups, bulk project reassignment and superseding aggregates.

use jiff::tz::TimeZone;
use tokenmtr_core::ProjectKey;

use super::*;
use crate::codec::enum_text;
use crate::{IngestWrite, IssueUpdate, TempStore, ToolResultFlag};

fn call(id: &str, is_error: Option<bool>) -> ProfiledToolCall {
    ProfiledToolCall {
        profile: ProfileId::default_profile(),
        call: ToolCall {
            agent: AgentId::CLAUDE,
            tool_use_id: id.to_owned(),
            event: None,
            session: SessionId::try_from("s1".to_owned()).unwrap(),
            at: at(1_700_000_000),
            tool_name: "Bash".to_owned(),
            mcp_server: None,
            is_error,
            skill: None,
        },
    }
}

fn in_dir(machine: &MachineId, key: &str, cwd: &str) -> UsageEvent {
    let mut e = event(machine, key, 1);
    e.cwd = Some(Utf8PathBuf::from(cwd));
    e
}

pub(super) fn resolution(
    cwd: &str,
    project: &str,
    client: Option<&str>,
) -> crate::ProjectResolution {
    crate::ProjectResolution {
        cwd: Utf8PathBuf::from(cwd),
        project: project.to_owned().try_into().unwrap(),
        name: project.to_owned(),
        client: client.map(str::to_owned),
        repo_root: None,
        remote: None,
        resolved_at: at(3),
        source: crate::ProjectSource::Rule("r".to_owned()),
        remotes: Vec::new(),
    }
}

fn issue(count: u64) -> ParseIssue {
    ParseIssue {
        agent: AgentId::CLAUDE,
        kind: IssueKind::FormatDrift,
        subject: "usage".to_owned(),
        count: NonZeroU64::new(count).unwrap(),
        sample: Some("{}".to_owned()),
        at: at(8),
    }
}

#[test]
fn commit_ingest_stores_every_part_and_reports_it() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let signal = SessionSignal {
        agent: AgentId::CLAUDE,
        session: SessionId::try_from("s1".to_owned()).unwrap(),
        kind: SignalKind::Compaction,
        at: at(5),
    };
    let events = [event(&machine, "a", 1), event(&machine, "b", 2)];
    let calls = [call("t1", None), call("t2", None)];
    let results = [
        ToolResultFlag {
            agent: AgentId::CLAUDE,
            tool_use_id: "t1".to_owned(),
            is_error: true,
        },
        ToolResultFlag {
            agent: AgentId::CLAUDE,
            tool_use_id: "missing".to_owned(),
            is_error: true,
        },
    ];
    let issues = [IssueUpdate::Record(issue(2))];
    let checkpoints = [checkpoint("/a.jsonl", 42)];
    let write = IngestWrite {
        events: &events,
        tool_calls: &calls,
        signals: std::slice::from_ref(&signal),
        tool_results: &results,
        issues: &issues,
        checkpoints: &checkpoints,
    };

    let report = f.store.commit_ingest(&write).unwrap();
    assert_eq!(report.events.inserted, 2);
    assert_eq!(report.tool_calls.inserted, 2);
    assert_eq!(report.signals_new, 1);
    assert_eq!(report.tool_results_applied, 1);
    assert_eq!(report.tool_results_unmatched, 1);
    assert_eq!(
        f.store
            .usage_events_after(Revision::ZERO, 10)
            .unwrap()
            .len(),
        2
    );
    let stored = f.store.tool_call(AgentId::CLAUDE, "t1").unwrap().unwrap();
    assert_eq!(stored.row.call.is_error, Some(true));
    assert_eq!(f.store.open_parse_issues().unwrap().len(), 1);
    assert_eq!(
        f.store.load_checkpoints(AgentId::CLAUDE).unwrap()[0]
            .checkpoint
            .offset,
        42
    );

    // Repeating the flush changes nothing, and closes the issue when asked to.
    let heads = (
        head(&f.store, Dataset::UsageEvents),
        head(&f.store, Dataset::ToolCalls),
    );
    let close = [IssueUpdate::Close {
        agent: AgentId::CLAUDE,
        kind: IssueKind::FormatDrift,
        subject: "usage".to_owned(),
        at: at(9),
    }];
    let again = f
        .store
        .commit_ingest(&IngestWrite {
            issues: &close,
            ..write
        })
        .unwrap();
    assert_eq!(again.events.changed(), 0);
    assert_eq!(again.tool_calls.changed(), 0);
    assert_eq!(
        (again.tool_results_applied, again.tool_results_unchanged),
        (0, 1)
    );
    assert_eq!(
        heads,
        (
            head(&f.store, Dataset::UsageEvents),
            head(&f.store, Dataset::ToolCalls)
        )
    );
    assert_eq!(f.store.open_parse_issues().unwrap(), Vec::new());
}

#[test]
fn a_failing_commit_ingest_stores_nothing() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let events = [event(&machine, "a", 1)];
    let calls = [call("t1", None)];
    // The checkpoint is applied last and fails: its size does not fit SQLite's integers.
    let mut bad = checkpoint("/a.jsonl", 0);
    bad.size = u64::MAX;
    let checkpoints = [bad];
    let result = f.store.commit_ingest(&IngestWrite {
        events: &events,
        tool_calls: &calls,
        issues: &[IssueUpdate::Record(issue(1))],
        checkpoints: &checkpoints,
        ..IngestWrite::default()
    });
    assert!(
        matches!(result, Err(StoreError::OutOfRange { column: "size" })),
        "{result:?}"
    );
    assert_eq!(
        f.store.usage_events_after(Revision::ZERO, 10).unwrap(),
        Vec::new()
    );
    assert_eq!(
        f.store.tool_calls_after(Revision::ZERO, 10).unwrap(),
        Vec::new()
    );
    assert_eq!(f.store.open_parse_issues().unwrap(), Vec::new());
    assert_eq!(head(&f.store, Dataset::UsageEvents), Revision::ZERO);
    assert_eq!(
        f.store.load_checkpoints(AgentId::CLAUDE).unwrap(),
        Vec::new()
    );
}

#[test]
fn apply_tool_result_bumps_the_revision_only_on_change() {
    let mut f = fixture();
    f.store.upsert_tool_calls(&[call("t1", None)]).unwrap();
    let before = head(&f.store, Dataset::ToolCalls);

    assert!(
        f.store
            .apply_tool_result(AgentId::CLAUDE, "t1", true)
            .unwrap()
    );
    let after_first = head(&f.store, Dataset::ToolCalls);
    assert_eq!(after_first, before.next());
    assert!(
        !f.store
            .apply_tool_result(AgentId::CLAUDE, "t1", true)
            .unwrap()
    );
    assert_eq!(head(&f.store, Dataset::ToolCalls), after_first);
    assert!(
        f.store
            .apply_tool_result(AgentId::CLAUDE, "t1", false)
            .unwrap()
    );
    assert!(
        !f.store
            .apply_tool_result(AgentId::CLAUDE, "nope", true)
            .unwrap()
    );

    let rows = f.store.tool_calls_after(Revision::ZERO, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].row.call.is_error, Some(false));
    assert_eq!(rows[0].revision, after_first.next());
}

#[test]
fn point_lookups_find_rows_by_key_and_id() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let a = event(&machine, "a", 1);
    f.store
        .upsert_events(&[a.clone(), event(&machine, "b", 2)])
        .unwrap();

    let by_key = f
        .store
        .usage_event_by_key(AgentId::CLAUDE, &a.dedup_key)
        .unwrap()
        .unwrap();
    assert_eq!(by_key.row, a);
    let by_id = f.store.usage_event(&a.id).unwrap().unwrap();
    assert_eq!(by_id, by_key);
    let missing = DedupKey::try_from("zzz".to_owned()).unwrap();
    assert!(
        f.store
            .usage_event_by_key(AgentId::CLAUDE, &missing)
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .usage_event(&event(&machine, "zzz", 1).id)
            .unwrap()
            .is_none()
    );

    f.store.upsert_tool_calls(&[call("t1", None)]).unwrap();
    assert!(f.store.tool_call(AgentId::CLAUDE, "t1").unwrap().is_some());
    assert!(f.store.tool_call(AgentId::CLAUDE, "t2").unwrap().is_none());
}

#[test]
fn earliest_transcript_event_ignores_daily_aggregates() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let profile = ProfileId::default_profile();
    assert_eq!(
        f.store
            .earliest_transcript_event(AgentId::CLAUDE, &profile)
            .unwrap(),
        None
    );

    let mut aggregate = event(&machine, "agg", 1);
    aggregate.kind = EventKind::DailyAggregate;
    aggregate.at = at(1_000);
    let mut early = event(&machine, "early", 1);
    early.at = at(2_000);
    let mut late = event(&machine, "late", 1);
    late.at = at(3_000);
    let mut other = event(&machine, "other", 1);
    other.profile = "work".to_owned().try_into().unwrap();
    other.at = at(10);
    f.store
        .upsert_events(&[aggregate, late, early, other])
        .unwrap();

    assert_eq!(
        f.store
            .earliest_transcript_event(AgentId::CLAUDE, &profile)
            .unwrap(),
        Some(at(2_000))
    );
}

#[test]
fn readers_return_histories_newest_first() {
    let mut f = fixture();
    let run = |agent, finished| crate::IngestRun {
        agent,
        started: at(1),
        finished: at(finished),
        files_parsed: 1,
        bytes_parsed: 2,
        lines_read: 3,
        events_found: 4,
        events_changed: 5,
        skipped: [("bad".to_owned(), 1)].into(),
    };
    for (agent, finished) in [(AgentId::CLAUDE, 2), (AgentId::PI, 3), (AgentId::CLAUDE, 4)] {
        f.store.record_ingest_run(&run(agent, finished)).unwrap();
    }
    let all = f.store.ingest_runs(None, 10).unwrap();
    assert_eq!(
        all.iter().map(|r| r.finished).collect::<Vec<_>>(),
        [at(4), at(3), at(2)]
    );
    let claude = f.store.ingest_runs(Some(AgentId::CLAUDE), 1).unwrap();
    assert_eq!(claude, vec![run(AgentId::CLAUDE, 4)]);

    let sink = SinkId::try_from("s3".to_owned()).unwrap();
    for (started, rows) in [(10, 1), (30, 3), (20, 2)] {
        f.store
            .record_sink_run(&crate::SinkRun {
                sink: sink.clone(),
                dataset: Dataset::FxRates,
                started: at(started),
                finished: at(started + 1),
                rows,
                error: None,
            })
            .unwrap();
    }
    f.store
        .record_sink_run(&crate::SinkRun {
            sink: SinkId::try_from("other".to_owned()).unwrap(),
            dataset: Dataset::FxRates,
            started: at(99),
            finished: at(100),
            rows: 0,
            error: Some("x".to_owned()),
        })
        .unwrap();
    let runs = f.store.sink_runs(&sink, 2).unwrap();
    assert_eq!(runs.iter().map(|r| r.rows).collect::<Vec<_>>(), [3, 2]);
    assert_eq!(
        f.store
            .sink_runs(&SinkId::try_from("none".to_owned()).unwrap(), 5)
            .unwrap(),
        Vec::new()
    );
}

#[test]
fn open_parse_issues_list_unresolved_ones_oldest_first() {
    let mut f = fixture();
    f.store.record_parse_issue(&issue(2)).unwrap();
    let mut second = issue(1);
    second.kind = IssueKind::MissingCwd;
    second.at = at(20);
    f.store.record_parse_issue(&second).unwrap();
    f.store.record_parse_issue(&issue(3)).unwrap();

    let open = f.store.open_parse_issues().unwrap();
    assert_eq!(open.len(), 2);
    assert_eq!(
        (open[0].kind, open[0].occurrences),
        (IssueKind::FormatDrift, 5)
    );
    assert_eq!(open[1].kind, IssueKind::MissingCwd);
    assert_eq!(open[0].sample.as_deref(), Some("{}"));
    f.store
        .close_parse_issue(AgentId::CLAUDE, IssueKind::FormatDrift, "usage", at(30))
        .unwrap();
    assert_eq!(f.store.open_parse_issues().unwrap().len(), 1);
}

#[test]
fn events_page_by_cwd_through_the_index() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let events: Vec<_> = (0..5)
        .map(|n| in_dir(&machine, &format!("a{n}"), "/a"))
        .collect();
    f.store.upsert_events(&events).unwrap();
    f.store
        .upsert_events(&[in_dir(&machine, "b", "/b"), event(&machine, "none", 1)])
        .unwrap();

    let cwd = Utf8Path::new("/a");
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = f.store.usage_events_by_cwd(cwd, cursor, 2).unwrap();
        seen.extend(
            page.items
                .iter()
                .map(|v| v.row.dedup_key.as_str().to_owned()),
        );
        pages += 1;
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(
        (pages, seen),
        (
            3,
            ["a0", "a1", "a2", "a3", "a4"].map(str::to_owned).to_vec()
        )
    );

    let plan: String = f
        .store
        .raw()
        .query_row(
            &format!("EXPLAIN QUERY PLAN {}", crate::events::BY_CWD),
            rusqlite::params!["/a", 0, 10],
            |r| r.get(3),
        )
        .unwrap();
    assert!(plan.contains("usage_event_cwd"), "{plan}");
}

#[test]
fn reassigning_projects_rewrites_only_changed_events() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    f.store
        .upsert_events(&[
            in_dir(&machine, "a1", "/a"),
            in_dir(&machine, "a2", "/a"),
            in_dir(&machine, "b1", "/b"),
            event(&machine, "no-cwd", 1),
        ])
        .unwrap();
    assert_eq!(
        f.store.unresolved_cwds().unwrap(),
        vec![Utf8PathBuf::from("/a"), Utf8PathBuf::from("/b")]
    );
    let before = head(&f.store, Dataset::UsageEvents);

    let rules = [resolution("/a", "pa", Some("acme"))];
    assert_eq!(f.store.reassign_projects(&rules).unwrap(), 2);
    assert_eq!(head(&f.store, Dataset::UsageEvents).get(), before.get() + 2);
    let events = f.store.usage_events_after(before, 10).unwrap();
    assert_eq!(events.len(), 2);
    for v in &events {
        assert_eq!(v.row.project.as_ref().map(ProjectKey::as_str), Some("pa"));
        assert_eq!(v.row.client.as_deref(), Some("acme"));
    }
    assert_eq!(
        f.store.project(Utf8Path::new("/a")).unwrap(),
        Some(rules[0].clone())
    );
    assert_eq!(
        f.store.unresolved_cwds().unwrap(),
        vec![Utf8PathBuf::from("/b")]
    );

    // Same resolutions again: nothing differs, so no revision moves.
    let settled = head(&f.store, Dataset::UsageEvents);
    assert_eq!(f.store.reassign_projects(&rules).unwrap(), 0);
    assert_eq!(head(&f.store, Dataset::UsageEvents), settled);

    // A new client rewrites that directory's events only.
    let moved = [resolution("/a", "pa", None), resolution("/b", "pb", None)];
    assert_eq!(f.store.reassign_projects(&moved).unwrap(), 3);
    assert_eq!(
        f.store.unresolved_cwds().unwrap(),
        Vec::<Utf8PathBuf>::new()
    );
}

#[test]
fn covered_daily_aggregates_are_superseded_not_deleted() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let day = 86_400;
    let aggregate = |key: &str, profile: &str, at_s: i64| {
        let mut e = event(&machine, key, 1);
        e.kind = EventKind::DailyAggregate;
        e.at = at(at_s);
        e.profile = profile.to_owned().try_into().unwrap();
        e
    };
    let mut transcript = event(&machine, "t", 1);
    transcript.at = at(3 * day + 100);
    let before = [
        aggregate("d1", "default", day),
        aggregate("d2", "default", 2 * day),
        aggregate("d3", "default", 3 * day + 5),
        aggregate("d5", "default", 5 * day),
        aggregate("w5", "work", 5 * day),
        transcript,
    ];
    f.store.upsert_events(&before).unwrap();
    let profile = ProfileId::default_profile();

    let head_before = head(&f.store, Dataset::UsageEvents);
    let flagged = f
        .store
        .supersede_daily_aggregates_covered_by_transcripts(
            AgentId::CLAUDE,
            &profile,
            &TimeZone::UTC,
        )
        .unwrap();
    assert_eq!(flagged, 2);
    // Every row is kept; the two flagged rows got new revisions so sinks re-export them.
    let rows = f.store.stored_events_after(head_before, 10).unwrap();
    let superseded: Vec<_> = rows
        .iter()
        .map(|v| (v.row.event.dedup_key.as_str().to_owned(), v.row.superseded))
        .collect();
    assert_eq!(
        superseded,
        [("d3".to_owned(), true), ("d5".to_owned(), true)]
    );
    assert_eq!(
        f.store
            .usage_events_after(Revision::ZERO, 10)
            .unwrap()
            .len(),
        6
    );
    assert_eq!(
        head(&f.store, Dataset::UsageEvents).get(),
        head_before.get() + 2
    );

    // Nothing left to flag; a profile without transcripts is untouched.
    let work = ProfileId::try_from("work".to_owned()).unwrap();
    assert_eq!(
        f.store
            .supersede_daily_aggregates_covered_by_transcripts(
                AgentId::CLAUDE,
                &profile,
                &TimeZone::UTC
            )
            .unwrap(),
        0
    );
    assert_eq!(
        f.store
            .supersede_daily_aggregates_covered_by_transcripts(
                AgentId::CLAUDE,
                &work,
                &TimeZone::UTC
            )
            .unwrap(),
        0
    );
}

#[test]
fn temp_store_pairs_a_writer_with_readers_on_one_file() {
    let mut temp = TempStore::new().unwrap();
    let machine = temp.store().machine_id();
    let reader = temp.reader().unwrap();
    temp.store_mut()
        .upsert_events(&[event(&machine, "a", 1)])
        .unwrap();
    assert_eq!(
        reader.usage_events_after(Revision::ZERO, 10).unwrap().len(),
        1
    );
    assert!(temp.path().exists());
}

#[test]
fn issue_kind_names_match_the_stored_text() {
    for kind in [
        IssueKind::FormatDrift,
        IssueKind::UnknownVersion,
        IssueKind::ProtectedFolder,
        IssueKind::MissingCwd,
        IssueKind::ParseError,
    ] {
        assert_eq!(kind.as_str(), enum_text(&kind).unwrap());
    }
}

#[test]
fn optimize_keeps_index_averages_and_drops_samples() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let events: Vec<_> = (0..50)
        .map(|n| event(&machine, &format!("k{n}"), 1))
        .collect();
    f.store.upsert_events(&events).unwrap();
    f.store.optimize().unwrap();
    let conn = f.store.raw();
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert!(count("SELECT COUNT(*) FROM sqlite_stat1 WHERE tbl = 'usage_event'") > 0);
    assert_eq!(count("SELECT COUNT(*) FROM sqlite_stat4"), 0);
}

#[test]
fn unresolved_cwds_skip_from_directory_to_directory() {
    let mut f = fixture();
    let mut statement = f
        .store
        .raw()
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            crate::project::UNRESOLVED
        ))
        .unwrap();
    let plan = statement
        .query_map([], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("COVERING INDEX usage_event_cwd (origin_machine=? AND cwd>?)"),
        "{plan}"
    );
    assert!(!plan.contains("SCAN usage_event"), "{plan}");
}

#[test]
fn filling_branches_sets_only_missing_ones_and_a_reread_keeps_them() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let mut named = in_dir(&machine, "a2", "/a");
    named.git_branch = Some("logged".to_owned());
    let bare = in_dir(&machine, "a1", "/a");
    f.store
        .upsert_events(&[bare.clone(), named, in_dir(&machine, "b1", "/b")])
        .unwrap();
    let before = head(&f.store, Dataset::UsageEvents);
    let fill = [(Utf8PathBuf::from("/a"), "feat/x".to_owned())];
    assert_eq!(f.store.fill_branches(&fill).unwrap(), 1);
    assert_eq!(head(&f.store, Dataset::UsageEvents).get(), before.get() + 1);
    assert_eq!(f.store.fill_branches(&fill).unwrap(), 0, "idempotent");

    // Re-reading the line, which names no branch, keeps the one learned later.
    f.store.upsert_events(&[bare]).unwrap();
    let branches: Vec<Option<String>> = f
        .store
        .usage_events_after(Revision::ZERO, 10)
        .unwrap()
        .into_iter()
        .map(|v| v.row.git_branch)
        .collect();
    assert!(branches.contains(&Some("feat/x".to_owned())));
    assert!(branches.contains(&Some("logged".to_owned())));
    assert!(branches.contains(&None), "/b had no hint");
}

#[test]
fn an_inferred_resolution_round_trips() {
    let mut f = fixture();
    let mut inferred = resolution("/gone/wt", "github.com/acme/tool", None);
    inferred.source = crate::ProjectSource::Inferred;
    inferred.remote = Some("github.com/acme/tool".to_owned());
    f.store.save_project(&inferred).unwrap();
    assert_eq!(
        f.store.project(Utf8Path::new("/gone/wt")).unwrap(),
        Some(inferred)
    );
}

#[test]
fn the_last_scan_time_is_the_newest_of_any_agent() {
    let mut f = fixture();
    assert_eq!(f.store.last_scan_finished_at().unwrap(), None);
    f.store
        .record_scan_finished(AgentId::CLAUDE, at(20))
        .unwrap();
    f.store.record_scan_finished(AgentId::PI, at(10)).unwrap();
    assert_eq!(f.store.last_scan_finished_at().unwrap(), Some(at(20)));
    f.store.record_scan_finished(AgentId::PI, at(30)).unwrap();
    assert_eq!(f.store.last_scan_finished_at().unwrap(), Some(at(30)));
}

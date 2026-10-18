//! Behavioural tests of the store: revisions, last-wins, WAL concurrency, backup, schema guards.

use std::num::NonZeroU64;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;
use proptest::prelude::*;
use rusqlite::Connection;
use tempfile::TempDir;
use tokenmtr_core::event::{EventKind, SignalKind, ToolCall};
use tokenmtr_core::quota::{QuotaWindowState, Utilization};
use tokenmtr_core::{
    AgentId, Alert, AlertKind, DedupKey, Estimate, EurPerUsd, EventCost, EventId, FxRate,
    MachineId, ModelId, PricingVersion, ProfileId, QuotaWindow, SessionId, SinkId, SourceFileId,
    TokenCounts, UsageEvent, UsdMicros, WindowId,
};
use tokenmtr_core::{BillingPeriod, Currency, Money, Plan, Subscription, SubscriptionSpec};

use crate::{
    Checkpoint, Dataset, FxSource, IssueKind, ParseIssue, ProfiledToolCall, QuotaOutcome,
    QuotaPoll, QuotaReading, Revision, SCHEMA_VERSION, SessionSignal, Store, StoreError, StoreRead,
    StoreReader, restore,
};

fn clock() -> Timestamp {
    Timestamp::from_second(1_800_000_000).unwrap()
}

struct Fixture {
    _dir: TempDir,
    path: Utf8PathBuf,
    store: Store,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(dir.path().join("db").join("tokenmtr.db")).unwrap();
    let store = Store::open(&path).unwrap().with_clock(clock);
    Fixture {
        _dir: dir,
        path,
        store,
    }
}

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(second).unwrap()
}

fn event(machine: &MachineId, key: &str, output: u64) -> UsageEvent {
    let dedup_key = DedupKey::try_from(key.to_owned()).unwrap();
    UsageEvent {
        id: EventId::new(machine, AgentId::CLAUDE, &dedup_key),
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        dedup_key,
        at: at(1_700_000_000),
        session: SessionId::try_from("s1".to_owned()).unwrap(),
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: None,
        git_branch: None,
        project: None,
        client: None,
        source_file: None,
        approximate: false,
        provider: None,
        model: ModelId::try_from("claude-sonnet-4".to_owned()).unwrap(),
        kind: EventKind::Message,
        tokens: TokenCounts {
            output,
            ..TokenCounts::default()
        },
        cost: EventCost::new(None, None),
        service_tier: None,
    }
}

fn checkpoint(path: &str, offset: u64) -> Checkpoint {
    Checkpoint {
        path: Utf8PathBuf::from(path),
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        dev: 7,
        inode: u64::MAX,
        size: 100,
        mtime: at(1_700_000_000),
        offset,
        last_line_hash: Some([9; 32]),
    }
}

fn head(store: &impl StoreRead, dataset: Dataset) -> Revision {
    store.head_revision(dataset).unwrap()
}

fn revisions(store: &impl StoreRead) -> Vec<(u64, String)> {
    store
        .usage_events_after(Revision::ZERO, 1000)
        .unwrap()
        .into_iter()
        .map(|v| (v.revision.get(), v.row.dedup_key.as_str().to_owned()))
        .collect()
}

mod api;
mod perf;

#[test]
fn open_seeds_identity_and_reopens_with_the_same_one() {
    let Fixture {
        _dir, path, store, ..
    } = fixture();
    let machine = store.machine_id();
    let key = store.redaction_key().as_bytes().to_owned();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    drop(store);
    let again = Store::open(&path).unwrap();
    assert_eq!(again.machine_id(), machine);
    assert_eq!(again.redaction_key().as_bytes(), &key);
    let reader = StoreReader::open(&path).unwrap();
    assert_eq!(reader.machine_id(), machine);
}

#[test]
fn pragmas_follow_d6_and_d49() {
    let mut f = fixture();
    let conn = f.store.raw();
    let text = |name: &str| -> String {
        conn.pragma_query_value(None, name, |r| r.get::<_, rusqlite::types::Value>(0))
            .map(|v| format!("{v:?}"))
            .unwrap()
    };
    assert_eq!(text("journal_mode"), "Text(\"wal\")");
    assert_eq!(text("synchronous"), "Integer(1)");
    assert_eq!(text("foreign_keys"), "Integer(1)");
    assert_eq!(text("fullfsync"), "Integer(1)");
    assert_eq!(text("checkpoint_fullfsync"), "Integer(1)");
    assert_eq!(text("wal_autocheckpoint"), "Integer(4000)");
}

#[test]
fn events_round_trip_every_column() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let file = f.store.save_checkpoint(&checkpoint("/a.jsonl", 0)).unwrap();
    let mut e = event(&machine, "k", 5);
    e.parent_session = Some(SessionId::try_from("p".to_owned()).unwrap());
    e.subagent = Some("sub".to_owned());
    e.is_sidechain = true;
    e.cwd = Some(Utf8PathBuf::from("/work/x"));
    e.git_branch = Some("main".to_owned());
    e.project = Some("proj".to_owned().try_into().unwrap());
    e.client = Some("acme".to_owned());
    e.source_file = Some(file);
    e.approximate = true;
    e.provider = Some("anthropic".to_owned().try_into().unwrap());
    e.kind = EventKind::ToolResult;
    e.tokens = TokenCounts {
        input_uncached: 1,
        cache_read: 2,
        cache_write_5m: 3,
        cache_write_1h: 4,
        output: 5,
        reasoning: 2,
    };
    e.cost = EventCost::new(
        Some(UsdMicros::new(12)),
        Some(Estimate {
            usd: UsdMicros::new(34),
            pricing_version: PricingVersion::of_body(b"p"),
        }),
    );
    e.service_tier = Some("priority".to_owned());
    f.store.upsert_events(std::slice::from_ref(&e)).unwrap();
    let rows = f.store.usage_events_after(Revision::ZERO, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].row, e);
    let total: i64 = f
        .store
        .raw()
        .query_row("SELECT total_tokens FROM usage_event", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 1 + 2 + 3 + 4 + 5);
}

type Mutation = (&'static str, fn(&mut UsageEvent));

const MUTATIONS: &[Mutation] = &[
    ("profile", |e| {
        e.profile = "work".to_owned().try_into().unwrap();
    }),
    ("at", |e| e.at = at(1_700_000_001)),
    ("session", |e| {
        e.session = "s2".to_owned().try_into().unwrap();
    }),
    ("parent_session", |e| {
        e.parent_session = Some("p".to_owned().try_into().unwrap());
    }),
    ("subagent", |e| e.subagent = Some("s".to_owned())),
    ("is_sidechain", |e| e.is_sidechain = true),
    ("cwd", |e| e.cwd = Some(Utf8PathBuf::from("/x"))),
    ("git_branch", |e| e.git_branch = Some("b".to_owned())),
    ("project", |e| {
        e.project = Some("p".to_owned().try_into().unwrap());
    }),
    ("client", |e| e.client = Some("c".to_owned())),
    ("source_file", |e| {
        e.source_file = Some(SourceFileId::new(1));
    }),
    ("approximate", |e| e.approximate = true),
    ("provider", |e| {
        e.provider = Some("p".to_owned().try_into().unwrap());
    }),
    ("model", |e| {
        e.model = "other".to_owned().try_into().unwrap();
    }),
    ("kind", |e| e.kind = EventKind::Auxiliary),
    ("input_uncached", |e| e.tokens.input_uncached += 1),
    ("cache_read", |e| e.tokens.cache_read += 1),
    ("cache_write_5m", |e| e.tokens.cache_write_5m += 1),
    ("cache_write_1h", |e| e.tokens.cache_write_1h += 1),
    ("output", |e| e.tokens.output += 1),
    ("reasoning", |e| e.tokens.reasoning += 1),
    ("cost_reported", |e| {
        e.cost = EventCost::new(Some(UsdMicros::new(1)), e.cost.estimated().cloned());
    }),
    ("cost_estimated", |e| {
        e.cost = EventCost::new(
            e.cost.reported(),
            Some(Estimate {
                usd: UsdMicros::new(1),
                pricing_version: PricingVersion::of_body(b"a"),
            }),
        );
    }),
    ("service_tier", |e| e.service_tier = Some("t".to_owned())),
];

#[test]
fn every_value_column_change_bumps_the_revision() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    f.store.save_checkpoint(&checkpoint("/a.jsonl", 0)).unwrap();
    for (name, mutate) in MUTATIONS {
        let base = event(&machine, &format!("k-{name}"), 1);
        f.store.upsert_events(std::slice::from_ref(&base)).unwrap();
        let before = head(&f.store, Dataset::UsageEvents);
        let mut changed = base.clone();
        mutate(&mut changed);
        assert_ne!(changed, base, "{name}: mutation is a no-op");
        let stats = f.store.upsert_events(&[changed]).unwrap();
        assert_eq!((stats.updated, stats.unchanged), (1, 0), "{name}");
        assert_eq!(
            head(&f.store, Dataset::UsageEvents),
            before.next(),
            "{name}"
        );
    }
}

#[test]
fn identical_rows_keep_their_revision() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let a = event(&machine, "a", 1);
    let b = event(&machine, "b", 2);
    let first = f.store.upsert_events(&[a.clone(), b.clone()]).unwrap();
    assert_eq!((first.inserted, first.updated, first.unchanged), (2, 0, 0));
    let before = revisions(&f.store);
    let second = f.store.upsert_events(&[a, b]).unwrap();
    assert_eq!(
        (second.inserted, second.updated, second.unchanged),
        (0, 0, 2)
    );
    assert_eq!(second.changed(), 0);
    assert_eq!(revisions(&f.store), before);
}

#[test]
fn last_write_wins_within_and_across_batches() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let stats = f
        .store
        .upsert_events(&[event(&machine, "k", 1), event(&machine, "k", 2)])
        .unwrap();
    assert_eq!((stats.inserted, stats.updated, stats.unchanged), (1, 0, 0));
    let rows = f.store.usage_events_after(Revision::ZERO, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].row.tokens.output, 2);
    let stats = f.store.upsert_events(&[event(&machine, "k", 3)]).unwrap();
    assert_eq!(stats.updated, 1);
    let rows = f.store.usage_events_after(Revision::ZERO, 10).unwrap();
    assert_eq!(rows[0].row.tokens.output, 3);
    assert_eq!(rows[0].revision, Revision::new(2));
}

fn arbitrary_batch() -> impl Strategy<Value = Vec<(u8, u32, u8, bool)>> {
    prop::collection::vec((0..4u8, any::<u32>(), 0..3u8, any::<bool>()), 1..8)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    #[test]
    fn upserting_a_batch_twice_changes_nothing(batch in arbitrary_batch()) {
        let mut f = fixture();
        let machine = f.store.machine_id();
        let events: Vec<UsageEvent> = batch
            .iter()
            .map(|&(key, output, session, sidechain)| {
                let mut e = event(&machine, &format!("k{key}"), u64::from(output));
                e.session = SessionId::try_from(format!("s{session}")).unwrap();
                e.is_sidechain = sidechain;
                e
            })
            .collect();
        f.store.upsert_events(&events).unwrap();
        let rows = revisions(&f.store);
        let head_before = head(&f.store, Dataset::UsageEvents);
        let again = f.store.upsert_events(&events).unwrap();
        prop_assert_eq!(again.changed(), 0);
        prop_assert_eq!(head(&f.store, Dataset::UsageEvents), head_before);
        prop_assert_eq!(revisions(&f.store), rows);
    }
}

#[test]
fn export_readers_return_revision_order_and_resume() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let batch: Vec<_> = ["a", "b", "c", "d"]
        .iter()
        .map(|k| event(&machine, k, 1))
        .collect();
    f.store.upsert_events(&batch).unwrap();
    f.store.upsert_events(&[event(&machine, "a", 9)]).unwrap();

    let all = revisions(&f.store);
    let keys: Vec<_> = all.iter().map(|(_, k)| k.as_str()).collect();
    assert_eq!(keys, ["b", "c", "d", "a"]);
    assert!(all.windows(2).all(|w| w[0].0 < w[1].0));

    let first = f.store.usage_events_after(Revision::ZERO, 2).unwrap();
    assert_eq!(first.len(), 2);
    let cursor = first[1].revision;
    let sink = SinkId::try_from("s3".to_owned()).unwrap();
    assert_eq!(
        f.store.sink_cursor(&sink, Dataset::UsageEvents).unwrap(),
        Revision::ZERO
    );
    f.store
        .set_sink_cursor(&sink, Dataset::UsageEvents, cursor)
        .unwrap();
    let cursor = f.store.sink_cursor(&sink, Dataset::UsageEvents).unwrap();
    let rest = f.store.usage_events_after(cursor, 10).unwrap();
    let rest_keys: Vec<_> = rest.iter().map(|v| v.row.dedup_key.as_str()).collect();
    assert_eq!(rest_keys, ["d", "a"]);
    let pending = f.store.pending(Dataset::UsageEvents, cursor).unwrap();
    assert_eq!(pending.rows, 2);
    assert_eq!(pending.oldest, Some(clock()));
    assert_eq!(
        f.store
            .pending(Dataset::UsageEvents, head(&f.store, Dataset::UsageEvents))
            .unwrap()
            .rows,
        0
    );
}

#[test]
fn tool_calls_keep_event_link_and_error_flag() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    let linked = event(&machine, "e", 1).id;
    let call = |event: Option<EventId>, is_error: Option<bool>, name: &str| ProfiledToolCall {
        profile: ProfileId::default_profile(),
        call: ToolCall {
            agent: AgentId::CLAUDE,
            tool_use_id: "toolu_1".to_owned(),
            event: None.or(event),
            session: SessionId::try_from("s".to_owned()).unwrap(),
            at: at(1_700_000_000),
            tool_name: name.to_owned(),
            mcp_server: Some("srv".to_owned()),
            is_error,
        },
    };
    f.store.upsert_events(&[event(&machine, "e", 1)]).unwrap();
    f.store
        .upsert_tool_calls(&[call(Some(linked), None, "Bash")])
        .unwrap();
    // The later tool result carries only the flag.
    let stats = f
        .store
        .upsert_tool_calls(&[call(None, Some(true), "Bash")])
        .unwrap();
    assert_eq!(stats.updated, 1);
    let rows = f.store.tool_calls_after(Revision::ZERO, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].row.call.event, Some(linked));
    assert_eq!(rows[0].row.call.is_error, Some(true));
    // A row that lacks both changes nothing.
    let stats = f
        .store
        .upsert_tool_calls(&[call(None, None, "Bash")])
        .unwrap();
    assert_eq!(stats.changed(), 0);
    assert!(rows[0].row.export_id(&machine).is_ok());
}

fn window(id: &str, percent: f64) -> QuotaWindow {
    QuotaWindow {
        id: WindowId::new(id).unwrap(),
        state: QuotaWindowState {
            utilization: Utilization::new(percent).unwrap(),
            resets_at: Some(at(1_800_003_600)),
        },
    }
}

#[test]
fn quota_snapshots_store_windows_and_failures() {
    let mut f = fixture();
    let poll = |fetched: i64, outcome| QuotaPoll {
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        account: Some("me@example.com".to_owned()),
        fetched_at: at(fetched),
        outcome,
    };
    let polls = [
        poll(
            100,
            QuotaOutcome::Windows(vec![
                window(WindowId::FIVE_HOUR, 42.5),
                window(WindowId::SEVEN_DAY, 10.0),
            ]),
        ),
        poll(200, QuotaOutcome::TokenExpired),
    ];
    let stats = f.store.upsert_quota_snapshots(&polls).unwrap();
    assert_eq!(stats.inserted, 3);
    assert_eq!(f.store.upsert_quota_snapshots(&polls).unwrap().changed(), 0);
    let rows = f.store.quota_snapshots_after(Revision::ZERO, 10).unwrap();
    assert_eq!(rows.len(), 3);
    let labels: Vec<_> = rows.iter().map(|r| r.row.window_label()).collect();
    assert_eq!(
        labels,
        [Some("5-hour".to_owned()), Some("7-day".to_owned()), None]
    );
    assert!(matches!(rows[2].row.reading, QuotaReading::TokenExpired));
    assert_eq!(
        rows[0].row.window().unwrap(),
        &window(WindowId::FIVE_HOUR, 42.5)
    );
}

#[test]
fn fx_rates_and_subscriptions_mirror_by_key() {
    let mut f = fixture();
    let date = jiff::civil::date(2026, 5, 4);
    let rate = FxRate {
        date,
        eur_per_usd: EurPerUsd::from_scaled(920_000_000).unwrap(),
    };
    assert_eq!(
        f.store
            .upsert_fx_rates(FxSource::Ecb, &[rate])
            .unwrap()
            .inserted,
        1
    );
    assert_eq!(
        f.store
            .upsert_fx_rates(FxSource::Ecb, &[rate])
            .unwrap()
            .changed(),
        0
    );
    let revised = FxRate {
        date,
        eur_per_usd: EurPerUsd::from_scaled(930_000_000).unwrap(),
    };
    assert_eq!(
        f.store
            .upsert_fx_rates(FxSource::Ecb, &[revised])
            .unwrap()
            .updated,
        1
    );
    let rows = f.store.fx_rates_after(Revision::ZERO, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].row.rate, revised);
    assert_eq!(rows[0].revision, Revision::new(2));

    let sub = Subscription::new(SubscriptionSpec {
        id: "max".to_owned().try_into().unwrap(),
        provider: "anthropic".to_owned().try_into().unwrap(),
        plan: Plan::Max5x,
        agents: vec![AgentId::CLAUDE],
        profiles: Some(vec![ProfileId::default_profile()]),
        fee: Money::from_micros(Currency::Eur, 90_000_000),
        period: BillingPeriod::Monthly,
        start: date,
        end: None,
    })
    .unwrap();
    assert_eq!(
        f.store
            .upsert_subscriptions(std::slice::from_ref(&sub))
            .unwrap()
            .inserted,
        1
    );
    assert_eq!(
        f.store
            .upsert_subscriptions(std::slice::from_ref(&sub))
            .unwrap()
            .changed(),
        0
    );
    let rows = f.store.subscriptions_after(Revision::ZERO, 10).unwrap();
    assert_eq!(rows[0].row, sub);
}

#[test]
fn checkpoints_signals_alerts_pricing_and_issues_round_trip() {
    let mut f = fixture();

    // Checkpoints: upsert by path keeps the id; u64 values survive.
    let id = f.store.save_checkpoint(&checkpoint("/a.jsonl", 0)).unwrap();
    let again = f
        .store
        .save_checkpoint(&checkpoint("/a.jsonl", 55))
        .unwrap();
    assert_eq!(id, again);
    let loaded = f.store.load_checkpoints(AgentId::CLAUDE).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].checkpoint, checkpoint("/a.jsonl", 55));

    // Signals dedup.
    let signal = SessionSignal {
        agent: AgentId::CLAUDE,
        session: SessionId::try_from("s".to_owned()).unwrap(),
        kind: SignalKind::Compaction,
        at: at(5),
    };
    assert_eq!(
        f.store
            .record_signals(&[signal.clone(), signal.clone()])
            .unwrap(),
        1
    );
    assert_eq!(f.store.record_signals(&[signal]).unwrap(), 0);

    // Alerts dedup.
    let alert = Alert {
        kind: AlertKind::BackupFailed,
        dedup_key: "backup:1".to_owned(),
        at: at(6),
    };
    assert!(f.store.save_alert(&alert).unwrap());
    assert!(!f.store.save_alert(&alert).unwrap());
    assert!(f.store.fired_alert_keys().unwrap().contains("backup:1"));

    // Pricing snapshot: zstd round trip, active one wins.
    assert!(f.store.active_pricing_snapshot().unwrap().is_none());
    let version = f
        .store
        .put_pricing_snapshot(b"{\"a\":1}", at(7), Some("litellm"))
        .unwrap();
    assert_eq!(version, PricingVersion::of_body(b"{\"a\":1}"));
    let active = f.store.active_pricing_snapshot().unwrap().unwrap();
    assert_eq!(
        (active.version, active.body),
        (version, b"{\"a\":1}".to_vec())
    );

    // Parse issues accumulate.
    let issue = |count: u64| ParseIssue {
        agent: AgentId::CLAUDE,
        kind: IssueKind::FormatDrift,
        subject: "usage".to_owned(),
        count: NonZeroU64::new(count).unwrap(),
        sample: Some("{}".to_owned()),
        at: at(8),
    };
    f.store.record_parse_issue(&issue(2)).unwrap();
    f.store.record_parse_issue(&issue(3)).unwrap();
    let occurrences: i64 = f
        .store
        .raw()
        .query_row("SELECT occurrences FROM parse_issue", [], |r| r.get(0))
        .unwrap();
    assert_eq!(occurrences, 5);
    assert!(
        f.store
            .close_parse_issue(AgentId::CLAUDE, IssueKind::FormatDrift, "usage", at(9))
            .unwrap()
    );
}

#[test]
fn heartbeat_runs_projects_and_accounts_round_trip() {
    let mut f = fixture();

    // Heartbeat.
    assert_eq!(f.store.heartbeat().unwrap(), None);
    f.store.set_heartbeat().unwrap();
    assert_eq!(f.store.heartbeat().unwrap(), Some(clock()));

    // Runs.
    f.store
        .record_sink_run(&crate::SinkRun {
            sink: SinkId::try_from("s3".to_owned()).unwrap(),
            dataset: Dataset::FxRates,
            started: at(1),
            finished: at(2),
            rows: 3,
            error: None,
        })
        .unwrap();
    f.store
        .record_backup_run(&crate::BackupRun {
            started: at(1),
            finished: at(2),
            path: None,
            size_bytes: None,
            error: Some("disk full".to_owned()),
        })
        .unwrap();
    f.store
        .record_ingest_run(&crate::IngestRun {
            agent: AgentId::CLAUDE,
            started: at(1),
            finished: at(2),
            files_parsed: 1,
            bytes_parsed: 2,
            lines_read: 3,
            events_found: 4,
            events_changed: 5,
            skipped: [("bad".to_owned(), 1)].into(),
        })
        .unwrap();

    // Project map and accounts.
    let resolution = crate::ProjectResolution {
        cwd: Utf8PathBuf::from("/work/x"),
        project: "x".to_owned().try_into().unwrap(),
        name: "X".to_owned(),
        client: Some("acme".to_owned()),
        repo_root: Some(Utf8PathBuf::from("/work")),
        remote: None,
        resolved_at: at(3),
        source: crate::ProjectSource::Rule("r1".to_owned()),
    };
    f.store.save_project(&resolution).unwrap();
    assert_eq!(
        f.store.project(Utf8Path::new("/work/x")).unwrap(),
        Some(resolution.clone())
    );
    assert_eq!(f.store.projects().unwrap(), vec![resolution]);
    let account = crate::AccountRecord {
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        email: Some("me@example.com".to_owned()),
        organization: None,
        detected_plan: Some(Plan::Max5x),
        rate_limit_tier: None,
        fetched_at: at(4),
    };
    f.store.save_account(&account).unwrap();
    assert_eq!(f.store.accounts().unwrap(), vec![account]);
}

#[test]
fn reader_works_while_a_write_transaction_is_open() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    f.store.upsert_events(&[event(&machine, "a", 1)]).unwrap();
    let reader = StoreReader::open(&f.path).unwrap();

    f.store
        .raw()
        .execute_batch(
            "BEGIN IMMEDIATE; \
             INSERT INTO fx_rate(date, revision, updated_at_us, eur_per_usd_scaled, source) \
             VALUES ('2026-01-01', 1, 0, 1, 'ecb')",
        )
        .unwrap();
    assert_eq!(
        reader.usage_events_after(Revision::ZERO, 10).unwrap().len(),
        1
    );
    assert_eq!(reader.fx_rates_after(Revision::ZERO, 10).unwrap().len(), 0);
    f.store.raw().execute_batch("COMMIT").unwrap();
    assert_eq!(reader.fx_rates_after(Revision::ZERO, 10).unwrap().len(), 1);
}

#[test]
fn reader_cannot_write() {
    let f = fixture();
    let reader = StoreReader::open(&f.path).unwrap();
    assert!(
        reader
            .connection()
            .execute("INSERT INTO meta(key, value) VALUES ('x', 1)", [])
            .is_err()
    );
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn backup_during_a_write_transaction_restores_to_the_committed_rows() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    f.store
        .upsert_events(&[event(&machine, "a", 1), event(&machine, "b", 2)])
        .unwrap();
    f.store
        .raw()
        .execute_batch(
            "BEGIN IMMEDIATE; \
             INSERT INTO fx_rate(date, revision, updated_at_us, eur_per_usd_scaled, source) \
             VALUES ('2026-01-01', 1, 0, 1, 'ecb')",
        )
        .unwrap();

    let reader = StoreReader::open(&f.path).unwrap();
    let dest = f.path.with_file_name("backups").join("snap.db.zst");
    let info = reader.backup(&dest).unwrap();
    assert!(info.compressed_bytes > 0 && info.raw_bytes > 0);
    f.store.raw().execute_batch("ROLLBACK").unwrap();

    let restored = f.path.with_file_name("restored").join("tokenmtr.db");
    std::fs::create_dir_all(restored.parent().unwrap()).unwrap();
    restore(&dest, &restored).unwrap();
    let copy = StoreReader::open(&restored).unwrap();
    assert_eq!(count(copy.connection(), "usage_event"), 2);
    assert_eq!(count(copy.connection(), "fx_rate"), 0);
    assert_eq!(copy.machine_id(), machine);
    assert_eq!(revisions(&copy), revisions(&f.store));
}

#[test]
fn restore_replaces_an_existing_database_and_drops_stale_wal() {
    let mut f = fixture();
    let machine = f.store.machine_id();
    f.store.upsert_events(&[event(&machine, "a", 1)]).unwrap();
    let dest = f.path.with_file_name("snap.zst");
    f.store.backup(&dest).unwrap();
    f.store.upsert_events(&[event(&machine, "b", 1)]).unwrap();
    drop(f.store);
    restore(&dest, &f.path).unwrap();
    let store = Store::open(&f.path).unwrap();
    assert_eq!(
        store.usage_events_after(Revision::ZERO, 10).unwrap().len(),
        1
    );
}

#[test]
fn restore_rejects_corrupt_input() {
    let f = fixture();
    let bad = f.path.with_file_name("bad.zst");
    std::fs::write(&bad, b"not zstd").unwrap();
    let dest = f.path.with_file_name("out.db");
    assert!(restore(&bad, &dest).is_err());
    assert!(!dest.exists());
}

fn set_user_version(path: &Utf8Path, version: u32) {
    Connection::open(path)
        .unwrap()
        .pragma_update(None, "user_version", version)
        .unwrap();
}

#[test]
fn a_newer_schema_is_refused_by_writer_reader_and_restore() {
    let f = fixture();
    let snapshot = f.path.with_file_name("snap.zst");
    f.store.backup(&snapshot).unwrap();
    let path = f.path.clone();
    drop(f.store);
    set_user_version(&path, SCHEMA_VERSION + 1);

    let newer = |e: StoreError| {
        matches!(e, StoreError::NewerSchema { found, supported }
            if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION)
    };
    assert!(newer(Store::open(&path).unwrap_err()));
    assert!(newer(StoreReader::open(&path).unwrap_err()));
    // The refused file is untouched.
    let conn = Connection::open(&path).unwrap();
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION + 1);

    // A snapshot of a newer schema cannot be restored either.
    let newer_db = path.with_file_name("newer.db");
    std::fs::copy(&path, &newer_db).unwrap();
    let raw = std::fs::read(&newer_db).unwrap();
    let compressed = zstd::encode_all(raw.as_slice(), 3).unwrap();
    let newer_snapshot = path.with_file_name("newer.zst");
    std::fs::write(&newer_snapshot, compressed).unwrap();
    let target = path.with_file_name("target.db");
    assert!(newer(restore(&newer_snapshot, &target).unwrap_err()));
    assert!(!target.exists());
}

#[test]
fn reader_refuses_a_database_not_yet_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(dir.path().join("old.db")).unwrap();
    Connection::open(&path).unwrap();
    assert!(matches!(
        StoreReader::open(&path).unwrap_err(),
        StoreError::OlderSchema { found: 0, .. }
    ));
}

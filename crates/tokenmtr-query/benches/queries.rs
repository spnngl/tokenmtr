//! Query and store benchmarks over a realistic store: two years of two agents' traffic.
//!
//! The store is built once per run in a temporary directory, through the store's own writers:
//! `TOKENMTR_BENCH_EVENTS` usage events (default 1,000,000) in sessions of 40, a tool call every
//! third event, compaction and API-error signals, quota polls every 5 minutes of 12 hours a day,
//! ingest runs every half hour of those, daily FX rates, project resolutions and one subscription. `read` times the dashboard, recap and export reads;
//! `write` times one ingest commit of 5,000 new events into that store, so it runs last.
//! Run with `cargo bench -p tokenmtr-query --bench queries`. `TOKENMTR_BENCH_STORE=<file>` keeps the
//! store at that path and reuses it on later runs (the `write` bench grows it; delete the file to
//! rebuild).

#![expect(
    clippy::expect_used,
    reason = "a benchmark has no caller to report setup failures to"
)]

use std::sync::OnceLock;
use std::time::Instant;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::civil::{Date, date};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use tempfile::TempDir;
use tokenmtr_core::event::ToolCall;
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{
    AgentId, BillingPeriod, Currency, DedupKey, Estimate, EurPerUsd, EventCost, EventId, EventKind,
    FxRate, MachineId, Money, Plan, PricingVersion, ProfileId, ProjectKey, QuotaWindow, SessionId,
    SignalKind, Subscription, SubscriptionId, SubscriptionSpec, TokenCounts, UsageEvent, UsdMicros,
    Utilization, WindowId,
};
use tokenmtr_query::{
    DateRange, Filters, FxPolicy, QueryContext, RecapConfig, WeekStart, load_fx_rates,
    load_subscriptions,
};
use tokenmtr_store::{
    FxSource, IngestWrite, ProfiledToolCall, ProjectResolution, ProjectSource, QuotaOutcome,
    QuotaPoll, SessionSignal, Store, StoreRead as _, StoreReader,
};

/// Usage events per session.
const SESSION_EVENTS: u64 = 40;
/// Events per `commit_ingest`, like the engine's transaction size.
const COMMIT_EVENTS: u64 = 5000;
/// Working directories; the last `UNRESOLVED` have no git and resolve to themselves.
const PROJECTS: u64 = 15;
const UNRESOLVED: u64 = 3;
/// Days of history before [`now`].
const HISTORY_DAYS: i64 = 730;

fn main() {
    divan::main();
}

/// The bench's wall clock: the end of the generated history.
fn now() -> Timestamp {
    "2026-05-01T12:00:00Z".parse().expect("timestamp")
}

fn start() -> Timestamp {
    now() - SignedDuration::from_hours(24 * HISTORY_DAYS)
}

fn today() -> Date {
    date(2026, 5, 1)
}

struct Fixture {
    /// The temporary directory holding the store, unless the store is kept.
    _dir: Option<TempDir>,
    path: Utf8PathBuf,
    machine: MachineId,
    events: u64,
}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

fn fixture() -> &'static Fixture {
    FIXTURE.get_or_init(build)
}

/// A cheap deterministic mix of `n`, so the corpus is the same on every run.
const fn mix(n: u64) -> u64 {
    let mut z = n.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn signed(n: u64) -> i64 {
    i64::try_from(n).expect("fits i64")
}

fn id<T: TryFrom<String>>(text: impl Into<String>) -> T
where
    T::Error: std::fmt::Debug,
{
    T::try_from(text.into()).expect("valid id")
}

fn cwd(project: u64) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("/Users/dev/code/proj{project:02}"))
}

fn project_key(project: u64) -> ProjectKey {
    if project < PROJECTS - UNRESOLVED {
        id(format!("github.com/acme/proj{project:02}"))
    } else {
        id(cwd(project).into_string())
    }
}

fn agent_of(session: u64) -> AgentId {
    if session % 10 < 7 {
        AgentId::CLAUDE
    } else {
        AgentId::PI
    }
}

fn profile_of(session: u64) -> ProfileId {
    if agent_of(session) == AgentId::CLAUDE && session.is_multiple_of(3) {
        id("work")
    } else {
        ProfileId::default_profile()
    }
}

fn session_id(tag: &str, session: u64) -> SessionId {
    id(format!("{tag}sess-{session:07}"))
}

/// When `session` starts: sessions are spread evenly over the history.
fn session_start(session: u64, sessions: u64) -> Timestamp {
    let span = SignedDuration::from_hours(24 * HISTORY_DAYS).as_secs();
    let offset = span * signed(session) / signed(sessions);
    start() + SignedDuration::from_secs(offset + signed(mix(session) % 600))
}

/// Event `n` of `session`, `at` its time.
fn event(machine: &MachineId, tag: &str, session: u64, n: u64, at: Timestamp) -> UsageEvent {
    let agent = agent_of(session);
    let h = mix(session * SESSION_EVENTS + n);
    let project = mix(session) % PROJECTS;
    let (model, provider) = if agent == AgentId::CLAUDE {
        let models = ["claude-opus-4-5", "claude-sonnet-4-5", "claude-haiku-4-5"];
        (models[usize::try_from(h % 3).expect("index")], "anthropic")
    } else if h.is_multiple_of(4) {
        ("gemini-2.5-pro", "google")
    } else if h.is_multiple_of(2) {
        ("gpt-5", "openai")
    } else {
        ("gpt-5-mini", "openai")
    };
    // Random like real message ids, so index inserts land on random pages.
    let dedup_key: DedupKey = id(format!("{tag}msg_{:016x}", mix(!h)));
    let estimate = Estimate {
        usd: UsdMicros::new(signed(h % 90_000)),
        pricing_version: PricingVersion::of_body(b"bench"),
    };
    let reported =
        (agent == AgentId::PI && h.is_multiple_of(2)).then(|| UsdMicros::new(signed(h % 80_000)));
    UsageEvent {
        id: EventId::new(machine, agent, &dedup_key),
        agent,
        profile: profile_of(session),
        dedup_key,
        at,
        session: session_id(tag, session),
        parent_session: None,
        subagent: None,
        is_sidechain: h.is_multiple_of(13),
        cwd: Some(cwd(project)),
        git_branch: Some(if session.is_multiple_of(4) {
            "main".to_owned()
        } else {
            format!("feature/{}", session % 40)
        }),
        project: Some(project_key(project)),
        client: (project < 4).then(|| "acme".to_owned()),
        source_file: None,
        approximate: false,
        provider: Some(id(provider)),
        model: id(model),
        kind: EventKind::Message,
        tokens: TokenCounts {
            input_uncached: 100 + h % 5000,
            cache_read: 10_000 + (h >> 8) % 190_000,
            cache_write_5m: (h >> 16) % 5000,
            cache_write_1h: 0,
            output: 50 + (h >> 24) % 3000,
            reasoning: if agent == AgentId::PI {
                (h >> 32) % 2000
            } else {
                0
            },
        },
        cost: EventCost::new(reported, Some(estimate)),
        service_tier: None,
    }
}

/// Rows of one ingest commit.
#[derive(Default)]
struct Batch {
    events: Vec<UsageEvent>,
    calls: Vec<ProfiledToolCall>,
    signals: Vec<SessionSignal>,
}

impl Batch {
    /// Appends one session's events, tool calls and signals. `tag` prefixes every key, so
    /// different tags never collide.
    fn session(&mut self, machine: &MachineId, tag: &str, session: u64, mut at: Timestamp) {
        let tool_names = [
            "Read", "Edit", "Bash", "Grep", "Write", "Glob", "Task", "WebFetch",
        ];
        for n in 0..SESSION_EVENTS {
            let h = mix(session * SESSION_EVENTS + n);
            at += SignedDuration::from_secs(30 + signed(h % 120));
            let e = event(machine, tag, session, n, at);
            if n.is_multiple_of(3) {
                let mcp = h.is_multiple_of(5);
                self.calls.push(ProfiledToolCall {
                    profile: e.profile.clone(),
                    call: ToolCall {
                        agent: e.agent,
                        tool_use_id: format!("{tag}toolu_{:016x}", mix(h)),
                        event: Some(e.id),
                        session: e.session.clone(),
                        at,
                        tool_name: if mcp {
                            format!("mcp__github__tool{}", h % 6)
                        } else {
                            tool_names[usize::try_from(h % 8).expect("index")].to_owned()
                        },
                        mcp_server: mcp.then(|| "github".to_owned()),
                        is_error: Some(h.is_multiple_of(20)),
                    },
                });
            }
            self.events.push(e);
        }
        let kinds = [
            (session.is_multiple_of(3), SignalKind::Compaction),
            (session.is_multiple_of(17), SignalKind::ApiError),
        ];
        for (_, kind) in kinds.into_iter().filter(|(on, _)| *on) {
            self.signals.push(SessionSignal {
                agent: agent_of(session),
                session: session_id(tag, session),
                kind,
                at,
            });
        }
    }

    fn commit(&self, store: &mut Store) {
        store
            .commit_ingest(&IngestWrite {
                events: &self.events,
                tool_calls: &self.calls,
                signals: &self.signals,
                tool_results: &[],
                issues: &[],
                checkpoints: &[],
            })
            .expect("commit ingest");
    }
}

fn seed_traffic(store: &mut Store, machine: &MachineId, total: u64) {
    let sessions = total.div_ceil(SESSION_EVENTS);
    let per_commit = COMMIT_EVENTS / SESSION_EVENTS;
    let mut batch = Batch::default();
    for session in 0..sessions {
        batch.session(machine, "", session, session_start(session, sessions));
        if (session + 1).is_multiple_of(per_commit) || session + 1 == sessions {
            batch.commit(store);
            batch = Batch::default();
        }
    }
}

/// The daemon's default poll: every 5 minutes, here 12 awake hours a day, for both Claude
/// profiles.
fn seed_polls(store: &mut Store) {
    let profiles = [
        (AgentId::CLAUDE, ProfileId::default_profile()),
        (AgentId::CLAUDE, id("work")),
    ];
    let mut polls = Vec::new();
    for tick in 0..HISTORY_DAYS * 24 * 12 {
        if tick % (24 * 12) < 12 * 12 {
            continue;
        }
        let fetched_at = start() + SignedDuration::from_mins(5 * tick);
        for (agent, profile) in &profiles {
            let used = f64::from(u32::try_from(mix(tick.unsigned_abs()) % 100).expect("percent"));
            let window = |id: WindowId, hours: i64| QuotaWindow {
                id,
                state: QuotaWindowState {
                    utilization: Utilization::new(used).expect("utilization"),
                    resets_at: Some(fetched_at + SignedDuration::from_hours(hours)),
                },
            };
            polls.push(QuotaPoll {
                agent: *agent,
                profile: profile.clone(),
                account: Some("dev@example.com".to_owned()),
                fetched_at,
                outcome: QuotaOutcome::Windows {
                    windows: vec![
                        window(WindowId::five_hour(), 5),
                        window(WindowId::seven_day(), 168),
                    ],
                    extra_usage: None,
                },
            });
        }
        if polls.len() >= 5000 {
            store
                .upsert_quota_snapshots(&polls, &jiff::tz::TimeZone::UTC)
                .expect("quota");
            polls.clear();
        }
    }
    store
        .upsert_quota_snapshots(&polls, &jiff::tz::TimeZone::UTC)
        .expect("quota");
}

/// One ingest run per agent every awake half hour. `Store::record_ingest_run` commits (and
/// fsyncs) each run, so a raw connection writes them in one transaction.
fn seed_ingest_runs(path: &Utf8Path) {
    let mut conn = rusqlite::Connection::open(path).expect("raw connection");
    let tx = conn.transaction().expect("transaction");
    for half_hour in 0..HISTORY_DAYS * 48 {
        if half_hour % 48 < 24 {
            continue;
        }
        let at = start() + SignedDuration::from_mins(30 * half_hour);
        for agent in [AgentId::CLAUDE, AgentId::PI] {
            tx.execute(
                "INSERT INTO ingest_run (agent, started_at_us, finished_at_us, files_parsed, \
                 bytes_parsed, lines_read, events_found, events_changed, skipped) \
                 VALUES (?1, ?2, ?2 + 300000, 2, 40000, 60, 20, 20, '{}')",
                rusqlite::params![agent.as_str(), at.as_microsecond()],
            )
            .expect("ingest run");
        }
    }
    tx.commit().expect("commit");
}

fn seed_reference(store: &mut Store) {
    let first = today() - jiff::Span::new().days(HISTORY_DAYS);
    let rates: Vec<FxRate> = (0..HISTORY_DAYS)
        .map(|d| FxRate {
            date: first + jiff::Span::new().days(d),
            eur_per_usd: EurPerUsd::from_scaled(900_000_000 + d).expect("rate"),
        })
        .collect();
    store.upsert_fx_rates(FxSource::Ecb, &rates).expect("fx");
    for project in 0..PROJECTS {
        let resolved = project < PROJECTS - UNRESOLVED;
        store
            .save_project(&ProjectResolution {
                cwd: cwd(project),
                project: project_key(project),
                name: format!("proj{project:02}"),
                client: (project < 4).then(|| "acme".to_owned()),
                repo_root: resolved.then(|| cwd(project)),
                remote: resolved.then(|| format!("github.com/acme/proj{project:02}")),
                resolved_at: start(),
                source: if resolved {
                    ProjectSource::Remote
                } else {
                    ProjectSource::Cwd
                },
            })
            .expect("project");
    }
    let subscription = Subscription::new(SubscriptionSpec {
        id: id::<SubscriptionId>("claude-max"),
        provider: id("anthropic"),
        plan: Plan::Pro,
        agents: vec![AgentId::CLAUDE],
        profiles: None,
        fee: Money::from_micros(Currency::Usd, 100_000_000),
        period: BillingPeriod::Monthly,
        start: first,
        end: None,
    })
    .expect("subscription");
    store
        .mirror_subscriptions(&[subscription])
        .expect("subscriptions");
    store.set_heartbeat().expect("heartbeat");
}

fn build() -> Fixture {
    let events = std::env::var("TOKENMTR_BENCH_EVENTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    let (dir, path) = if let Ok(kept) = std::env::var("TOKENMTR_BENCH_STORE") {
        (None, Utf8PathBuf::from(kept))
    } else {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("tokenmtr.db");
        let path = Utf8PathBuf::from_path_buf(path).expect("utf-8 path");
        (Some(dir), path)
    };
    let started = Instant::now();
    let seeded = path.exists();
    let mut store = Store::open(&path).expect("open store");
    let machine = store.machine_id();
    if !seeded {
        seed_traffic(&mut store, &machine, events);
        seed_polls(&mut store);
        seed_ingest_runs(&path);
        seed_reference(&mut store);
    }
    drop(store);
    let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
    eprintln!(
        "bench store: {events} events, {} MB, built in {:.1?}",
        bytes / 1_000_000,
        started.elapsed()
    );
    Fixture {
        _dir: dir,
        path,
        machine,
        events,
    }
}

fn reader(path: &Utf8Path) -> StoreReader {
    StoreReader::open(path).expect("open reader")
}

fn context(reader: &StoreReader) -> QueryContext {
    let rates = load_fx_rates(reader).expect("fx rates");
    QueryContext::new(
        TimeZone::get("Europe/Paris").expect("zone"),
        WeekStart::Monday,
        Currency::Eur,
        FxPolicy::from_rates(&rates),
        now(),
    )
    .with_subscriptions(load_subscriptions(reader).expect("subscriptions"))
}

fn last_days(days: i64) -> Filters {
    let end = today() + jiff::Span::new().days(1);
    Filters::new(DateRange::new(end - jiff::Span::new().days(days), end).expect("range"))
}

fn recap_config() -> RecapConfig {
    RecapConfig {
        budgets: Vec::new(),
        sinks: Vec::new(),
        live_window: SignedDuration::from_mins(5),
        daemon_stale_after: SignedDuration::from_secs(i64::MAX / 4),
    }
}

mod read {
    use divan::Bencher;
    use tokenmtr_core::AgentId;
    use tokenmtr_export::fetch_batch;
    use tokenmtr_query::{
        Bucket, Granularity, GroupBy, Period, QueryContext, Selection, billing, billing_series,
        branches, heatmap, ingest_status, models, overview, projects, quota_history, quota_latest,
        recap, series, session_analytics, session_detail, session_timeline, sessions, status,
        tools, unresolved_cwds,
    };
    use tokenmtr_store::{Dataset, Revision, StoreRead, StoreReader};

    use super::{
        SESSION_EVENTS, agent_of, context, fixture, last_days, reader, recap_config, session_id,
    };

    /// Runs `query` against a fresh reader and context, timing only the query.
    fn run<T>(bencher: Bencher<'_, '_>, query: impl Fn(&StoreReader, &QueryContext) -> T) {
        let r = reader(&fixture().path);
        let ctx = context(&r);
        bencher.bench_local(|| query(&r, &ctx));
    }

    #[divan::bench]
    fn overview_30d(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| overview(r, c, &last_days(30)).expect("q"));
    }

    #[divan::bench]
    fn series_30d_day(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| {
            series(r, c, &last_days(30), Bucket::Day, GroupBy::None).expect("q")
        });
    }

    #[divan::bench]
    fn series_1y_week_by_model(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| {
            series(r, c, &last_days(365), Bucket::Week, GroupBy::Model).expect("q")
        });
    }

    #[divan::bench]
    fn models_30d(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| models(r, c, &last_days(30)).expect("q"));
    }

    #[divan::bench]
    fn projects_30d(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| projects(r, c, &last_days(30)).expect("q"));
    }

    #[divan::bench]
    fn branches_30d(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| branches(r, c, &last_days(30)).expect("q"));
    }

    #[divan::bench]
    fn sessions_30d_page(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| {
            sessions(r, c, &last_days(30), 0, 50).expect("q")
        });
    }

    #[divan::bench]
    fn session_detail_one(bencher: Bencher<'_, '_>) {
        let n = fixture().events / SESSION_EVENTS / 2;
        let session = session_id("", n);
        run(bencher, |r, c| {
            session_detail(r, c, agent_of(n), &session).expect("q")
        });
    }

    #[divan::bench]
    fn heatmap_90d(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| heatmap(r, c, &last_days(90)).expect("q"));
    }

    #[divan::bench]
    fn tools_30d(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| tools(r, c, &last_days(30)).expect("q"));
    }

    #[divan::bench]
    fn unresolved_cwds_30d(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| {
            unresolved_cwds(r, c, &last_days(30), 50).expect("q")
        });
    }

    #[divan::bench]
    fn ingest_status_all(bencher: Bencher<'_, '_>) {
        run(bencher, |r, _| ingest_status(r).expect("q"));
    }

    #[divan::bench]
    fn billing_1y(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| billing(r, c, &last_days(365)).expect("q"));
    }

    #[divan::bench]
    fn billing_series_30d_hourly_by_agent(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| {
            billing_series(r, c, &last_days(30), Granularity::Hourly, true).expect("q")
        });
    }

    #[divan::bench]
    fn billing_series_30d_daily(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| {
            billing_series(r, c, &last_days(30), Granularity::Daily, false).expect("q")
        });
    }

    #[divan::bench]
    fn session_analytics_30d(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| {
            session_analytics(r, c, &last_days(30)).expect("q")
        });
    }

    #[divan::bench]
    fn session_timeline_one(bencher: Bencher<'_, '_>) {
        let n = fixture().events / SESSION_EVENTS / 2;
        let session = session_id("", n);
        run(bencher, |r, c| {
            session_timeline(r, c, agent_of(n), &session).expect("q")
        });
    }

    #[divan::bench]
    fn recap_all_today(bencher: Bencher<'_, '_>) {
        let cfg = recap_config();
        run(bencher, |r, c| {
            recap(r, c, &cfg, Selection::All, Period::Today).expect("q")
        });
    }

    #[divan::bench]
    fn status_two_agents(bencher: Bencher<'_, '_>) {
        let cfg = recap_config();
        let agents = [AgentId::CLAUDE, AgentId::PI];
        run(bencher, |r, c| {
            status(r, c, &cfg, &agents, Period::Today).expect("q")
        });
    }

    #[divan::bench]
    fn quota_history_7d(bencher: Bencher<'_, '_>) {
        let range = last_days(7).range();
        run(bencher, |r, c| {
            quota_history(r, c, AgentId::CLAUDE, "default", range).expect("q")
        });
    }

    #[divan::bench]
    fn quota_latest_all(bencher: Bencher<'_, '_>) {
        run(bencher, |r, c| {
            quota_latest(r, c, &last_days(1)).expect("q")
        });
    }

    #[divan::bench]
    fn export_fetch_batch_1000(bencher: Bencher<'_, '_>) {
        let r = reader(&fixture().path);
        let middle = Revision::new(r.head_revision(Dataset::UsageEvents).expect("head").get() / 2);
        bencher.bench_local(|| fetch_batch(&r, Dataset::UsageEvents, middle, 1000).expect("q"));
    }
}

mod write {
    use std::sync::atomic::{AtomicU64, Ordering};

    use divan::Bencher;
    use jiff::SignedDuration;
    use tokenmtr_store::Store;

    use super::{Batch, COMMIT_EVENTS, SESSION_EVENTS, fixture, now, signed};

    static ROUND: AtomicU64 = AtomicU64::new(0);

    /// One ingest commit of 5,000 new events (125 sessions on the last day), with their tool
    /// calls and signals, into the full store.
    #[divan::bench(sample_count = 10, sample_size = 1)]
    fn ingest_commit_5000(bencher: Bencher<'_, '_>) {
        let fx = fixture();
        let mut store = Store::open(&fx.path).expect("open store");
        bencher
            .with_inputs(|| {
                // A fresh tag per round and per run, so every row is an insert even in a kept
                // store.
                let tag = format!(
                    "r{}-{}-",
                    jiff::Timestamp::now().as_microsecond(),
                    ROUND.fetch_add(1, Ordering::Relaxed)
                );
                let mut batch = Batch::default();
                for s in 0..COMMIT_EVENTS / SESSION_EVENTS {
                    let at = now() - SignedDuration::from_hours(12)
                        + SignedDuration::from_mins(signed(s));
                    batch.session(&fx.machine, &tag, s, at);
                }
                batch
            })
            .bench_local_values(|batch| batch.commit(&mut store));
    }
}

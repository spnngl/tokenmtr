//! Snapshot and fold benchmarks over a realistic store: two years of two agents' traffic.
//!
//! The store is built once per run in a temporary directory through the store's own writers:
//! `TOKENMTR_BENCH_EVENTS` usage events (default 1,000,000) in sessions of 40, a tool call every
//! third event, 15 projects and one subscription. `snapshot` is the store read every refresh
//! pays; `fold` is the per-output work. Measured on the default store: snapshot ~0.8 s (a refresh is background work, at most one per
//! `refresh_interval`), fold ~1 ms with 1,200 series.
//! Run with `cargo bench -p tokenmtr-metrics --bench metrics`. `TOKENMTR_BENCH_STORE=<file>`
//! keeps the store at that path and reuses it.

#![expect(
    clippy::expect_used,
    reason = "a benchmark has no caller to report setup failures to"
)]

use std::sync::OnceLock;
use std::time::Instant;

use camino::{Utf8Path, Utf8PathBuf};
use jiff::civil::date;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use tempfile::TempDir;
use tokenmtr_config::{AccountLabels, GlobList, OutputFilter};
use tokenmtr_core::event::ToolCall;
use tokenmtr_core::{
    AgentId, BillingPeriod, Currency, DedupKey, EventCost, EventId, EventKind, MachineId, Money,
    Plan, ProfileId, ProjectKey, Subscription, SubscriptionId, SubscriptionSpec, TokenCounts,
    UsageEvent,
};
use tokenmtr_metrics::{Machine, OutputScope, Snapshot, SnapshotContext, snapshot};
use tokenmtr_store::{
    IngestWrite, ProfiledToolCall, ProjectResolution, ProjectSource, Store, StoreRead as _,
    StoreReader,
};

const SESSION_EVENTS: u64 = 40;
const COMMIT_EVENTS: u64 = 5000;
const PROJECTS: u64 = 15;
const HISTORY_DAYS: i64 = 730;

fn main() {
    divan::main();
}

fn now() -> Timestamp {
    "2026-05-01T12:00:00Z".parse().expect("timestamp")
}

fn start() -> Timestamp {
    now() - SignedDuration::from_hours(24 * HISTORY_DAYS)
}

struct Fixture {
    _dir: Option<TempDir>,
    path: Utf8PathBuf,
}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

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
    id(format!("github.com/acme/proj{project:02}"))
}

fn event(machine: &MachineId, session: u64, n: u64, at: Timestamp) -> UsageEvent {
    let agent = if session % 10 < 7 {
        AgentId::CLAUDE
    } else {
        AgentId::PI
    };
    let h = mix(session * SESSION_EVENTS + n);
    let project = mix(session) % PROJECTS;
    let models = ["claude-opus-4-5", "claude-sonnet-4-5", "claude-haiku-4-5"];
    let dedup_key: DedupKey = id(format!("msg_{:016x}", mix(!h)));
    UsageEvent {
        id: EventId::new(machine, agent, &dedup_key),
        agent,
        profile: if session.is_multiple_of(3) {
            id("work")
        } else {
            ProfileId::default_profile()
        },
        dedup_key,
        at,
        session: id(format!("sess-{session:07}")),
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: Some(cwd(project)),
        git_branch: Some(format!("feature/{}", session % 40)),
        project: Some(project_key(project)),
        client: (project < 4).then(|| "acme".to_owned()),
        source_file: None,
        approximate: false,
        provider: Some(id("anthropic")),
        model: id(models[usize::try_from(h % 3).expect("index")]),
        kind: EventKind::Message,
        tokens: TokenCounts {
            input_uncached: 100 + h % 5000,
            cache_read: 10_000 + (h >> 8) % 190_000,
            cache_write_5m: (h >> 16) % 5000,
            cache_write_1h: 0,
            output: 50 + (h >> 24) % 3000,
            reasoning: 0,
        },
        cost: EventCost::new(None, None),
        service_tier: None,
    }
}

fn build() -> Fixture {
    let events: u64 = std::env::var("TOKENMTR_BENCH_EVENTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    let (dir, path) = if let Ok(kept) = std::env::var("TOKENMTR_BENCH_STORE") {
        (None, Utf8PathBuf::from(kept))
    } else {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = Utf8PathBuf::from_path_buf(dir.path().join("tokenmtr.db")).expect("utf-8 path");
        (Some(dir), path)
    };
    let started = Instant::now();
    let seeded = path.exists();
    let mut store = Store::open(&path).expect("open store");
    let machine = store.machine_id();
    if !seeded {
        let sessions = events.div_ceil(SESSION_EVENTS);
        let per_commit = COMMIT_EVENTS / SESSION_EVENTS;
        let span = SignedDuration::from_hours(24 * HISTORY_DAYS).as_secs();
        let (mut batch_events, mut batch_calls) = (Vec::new(), Vec::new());
        for session in 0..sessions {
            let mut at =
                start() + SignedDuration::from_secs(span * signed(session) / signed(sessions));
            for n in 0..SESSION_EVENTS {
                let h = mix(session * SESSION_EVENTS + n);
                at += SignedDuration::from_secs(30 + signed(h % 120));
                let e = event(&machine, session, n, at);
                if n.is_multiple_of(3) {
                    batch_calls.push(ProfiledToolCall {
                        profile: e.profile.clone(),
                        call: ToolCall {
                            agent: e.agent,
                            tool_use_id: format!("toolu_{:016x}", mix(h)),
                            event: (!(n % 6 == 0 && session % 50 == 0)).then_some(e.id),
                            session: e.session.clone(),
                            at,
                            tool_name: ["Read", "Edit", "Bash", "Grep"]
                                [usize::try_from(h % 4).expect("index")]
                            .to_owned(),
                            mcp_server: None,
                            is_error: Some(false),
                            skill: None,
                        },
                    });
                }
                batch_events.push(e);
            }
            if (session + 1).is_multiple_of(per_commit) || session + 1 == sessions {
                store
                    .commit_ingest(&IngestWrite {
                        events: &batch_events,
                        tool_calls: &batch_calls,
                        signals: &[],
                        tool_results: &[],
                        issues: &[],
                        checkpoints: &[],
                    })
                    .expect("commit ingest");
                batch_events.clear();
                batch_calls.clear();
            }
        }
        for project in 0..PROJECTS {
            store
                .save_project(&ProjectResolution {
                    cwd: cwd(project),
                    project: project_key(project),
                    name: format!("proj{project:02}"),
                    client: None,
                    repo_root: Some(cwd(project)),
                    remote: Some(format!("github.com/acme/proj{project:02}")),
                    resolved_at: start(),
                    source: ProjectSource::Remote,
                    remotes: Vec::new(),
                })
                .expect("project");
        }
    }
    drop(store);
    eprintln!(
        "bench store: {events} events, built in {:.1?}",
        started.elapsed()
    );
    Fixture { _dir: dir, path }
}

fn fixture() -> &'static Fixture {
    FIXTURE.get_or_init(build)
}

fn reader(path: &Utf8Path) -> StoreReader {
    StoreReader::open(path).expect("open reader")
}

fn context() -> SnapshotContext {
    let subscription = Subscription::new(SubscriptionSpec {
        id: id::<SubscriptionId>("claude-max"),
        provider: id("anthropic"),
        plan: Plan::Pro,
        agents: vec![AgentId::CLAUDE],
        profiles: None,
        fee: Money::from_micros(Currency::Usd, 100_000_000),
        period: BillingPeriod::Monthly,
        start: date(2025, 1, 1),
        end: None,
    })
    .expect("subscription");
    SnapshotContext::new(
        TimeZone::get("Europe/Paris").expect("zone"),
        vec![subscription],
        SignedDuration::from_hours(48),
        AccountLabels::default(),
        Machine::default(),
    )
}

fn take(reader: &StoreReader) -> Snapshot {
    snapshot(reader, &context(), now()).expect("snapshot")
}

#[divan::bench(sample_count = 5, sample_size = 1)]
fn snapshot_read(bencher: divan::Bencher) {
    let reader = reader(&fixture().path);
    bencher.bench_local(|| take(&reader));
}

#[divan::bench(sample_count = 20, sample_size = 1)]
fn fold_with_exclusions(bencher: divan::Bencher) {
    let reader = reader(&fixture().path);
    let taken = take(&reader);
    let globs = |list: &[&str]| {
        GlobList::new(list.iter().map(|g| (*g).to_owned()).collect()).expect("globs")
    };
    let filter = OutputFilter {
        exclude_projects: globs(&["github.com/acme/proj0[0-4]"]),
        exclude_paths: globs(&["/Users/dev/code/proj1[0-2]"]),
        redact: std::collections::BTreeMap::new(),
    };
    let scope = OutputScope::new(&filter, reader.redaction_key());
    eprintln!("series after fold: {}", taken.fold(&scope).series.len());
    bencher.bench_local(|| taken.fold(&scope));
}

//! Behavioural tests over a real store seeded through the store's own writers.
#![expect(
    clippy::cast_precision_loss,
    reason = "timestamps and counts in tests are small"
)]

use camino::Utf8PathBuf;
use jiff::civil::{Date, date};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use rusqlite::params_from_iter;
use rusqlite::types::Value as Sql;
use tempfile::TempDir;
use tokenmtr_config::{GlobList, OutputFilter, RedactColumn, RedactMode};
use tokenmtr_core::event::ToolCall;
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{
    AgentId, BillingPeriod, Currency, DedupKey, EventCost, EventId, EventKind, MachineId, ModelId,
    Money, Plan, ProfileId, ProjectKey, ProviderId, QuotaWindow, SessionId, Subscription,
    SubscriptionId, SubscriptionSpec, TokenCounts, UsageEvent, Utilization, WindowId,
};
use tokenmtr_query::{
    BillingFilter, DateRange, Filters, FxPolicy, QueryContext, WeekStart, overview,
};
use tokenmtr_store::{
    AccountRecord, ProfiledToolCall, ProjectResolution, ProjectSource, QuotaOutcome, QuotaPoll,
    Store, StoreRead, StoreReader,
};

use crate::snapshot::{FALLBACK_SQL, QUOTA_SQL, Segments, tools_sql, usage_sql};
use crate::{Family, Folded, OutputScope, Snapshot, SnapshotContext, Value, snapshot};

fn ts(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn now() -> Timestamp {
    ts("2026-03-01T00:00:00Z")
}

struct Seed {
    _dir: TempDir,
    path: Utf8PathBuf,
    store: Store,
    machine: MachineId,
}

impl Seed {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("db").join("tokenmtr.db")).unwrap();
        let store = Store::open(&path).unwrap().with_clock(now);
        let machine = store.machine_id();
        Self {
            _dir: dir,
            path,
            store,
            machine,
        }
    }

    /// An event of session `s1`, 1 input and 10 output tokens, with no cwd or project.
    fn ev(&self, key: &str, at: &str) -> UsageEvent {
        let dedup_key = DedupKey::try_from(key.to_owned()).unwrap();
        UsageEvent {
            id: EventId::new(&self.machine, AgentId::CLAUDE, &dedup_key),
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            dedup_key,
            at: ts(at),
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
            provider: Some(ProviderId::try_from("anthropic".to_owned()).unwrap()),
            model: ModelId::try_from("m1".to_owned()).unwrap(),
            kind: EventKind::Message,
            tokens: TokenCounts {
                input_uncached: 1,
                output: 10,
                ..TokenCounts::default()
            },
            cost: EventCost::new(None, None),
            service_tier: None,
        }
    }

    /// An event in `cwd`, resolved to `project` (a remote key).
    fn ev_in(&self, key: &str, at: &str, cwd: &str, project: &str) -> UsageEvent {
        let mut e = self.ev(key, at);
        e.cwd = Some(Utf8PathBuf::from(cwd));
        e.project = Some(ProjectKey::try_from(project.to_owned()).unwrap());
        e
    }

    fn put(&mut self, events: &[UsageEvent]) {
        self.store.upsert_events(events).unwrap();
    }

    fn project(&mut self, cwd: &str, project: &str, name: &str, remote: Option<&str>) {
        self.store
            .save_project(&ProjectResolution {
                cwd: Utf8PathBuf::from(cwd),
                project: ProjectKey::try_from(project.to_owned()).unwrap(),
                name: name.to_owned(),
                client: None,
                repo_root: None,
                remote: remote.map(str::to_owned),
                resolved_at: now(),
                source: ProjectSource::Cwd,
                remotes: Vec::new(),
            })
            .unwrap();
    }

    fn call(&mut self, id: &str, event: Option<&UsageEvent>, session: &str, tool: &str) {
        self.store
            .upsert_tool_calls(&[ProfiledToolCall {
                profile: ProfileId::default_profile(),
                call: ToolCall {
                    agent: AgentId::CLAUDE,
                    tool_use_id: id.to_owned(),
                    event: event.map(|e| e.id),
                    session: SessionId::try_from(session.to_owned()).unwrap(),
                    at: now(),
                    tool_name: tool.to_owned(),
                    mcp_server: None,
                    is_error: Some(false),
                },
            }])
            .unwrap();
    }

    fn reader(&self) -> StoreReader {
        StoreReader::open(&self.path).unwrap()
    }
}

fn context(subscriptions: Vec<Subscription>, zone: &str) -> SnapshotContext {
    SnapshotContext::new(
        TimeZone::get(zone).unwrap(),
        subscriptions,
        SignedDuration::from_hours(48),
    )
}

fn take(seed: &Seed) -> Snapshot {
    snapshot(&seed.reader(), &context(Vec::new(), "UTC"), now()).unwrap()
}

fn filter(projects: &[&str], paths: &[&str]) -> OutputFilter {
    let globs =
        |list: &[&str]| GlobList::new(list.iter().map(|g| (*g).to_owned()).collect()).unwrap();
    OutputFilter {
        exclude_projects: globs(projects),
        exclude_paths: globs(paths),
        redact: std::collections::BTreeMap::new(),
    }
}

fn fold(seed: &Seed, snapshot: &Snapshot, filter: &OutputFilter) -> Folded {
    snapshot.fold(&OutputScope::new(filter, seed.reader().redaction_key()))
}

fn everything(seed: &Seed, snapshot: &Snapshot) -> Folded {
    fold(seed, snapshot, &filter(&[], &[]))
}

/// Sum of the counters of `family` whose labels include every `(name, value)` of `want`.
fn sum(folded: &Folded, family: Family, want: &[(&str, &str)]) -> u64 {
    folded
        .series
        .iter()
        .filter(|s| s.family == family)
        .filter(|s| {
            want.iter()
                .all(|(n, v)| s.labels.iter().any(|(ln, lv)| ln == n && lv == v))
        })
        .map(|s| match s.value {
            Value::Count(n) => n,
            Value::Gauge(_) => panic!("a gauge"),
        })
        .sum()
}

fn label<'a>(series: &'a crate::Series, name: &str) -> &'a str {
    series
        .labels
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.as_str())
        .unwrap()
}

fn of(folded: &Folded, family: Family) -> Vec<&crate::Series> {
    folded
        .series
        .iter()
        .filter(|s| s.family == family)
        .collect()
}

fn gauge(series: &crate::Series) -> f64 {
    match series.value {
        Value::Gauge(g) => g,
        Value::Count(_) => panic!("a counter"),
    }
}

#[test]
fn an_empty_store_has_only_the_meta_series() {
    let seed = Seed::new();
    let folded = everything(&seed, &take(&seed));
    let families: Vec<_> = folded.series.iter().map(|s| s.family).collect();
    assert_eq!(
        families,
        [Family::BuildInfo, Family::Series, Family::SnapshotTimestamp]
    );
    assert_eq!(gauge(of(&folded, Family::Series)[0]), 3.0);
    assert_eq!(
        gauge(of(&folded, Family::SnapshotTimestamp)[0]),
        now().as_second() as f64
    );
    assert_eq!(
        of(&folded, Family::BuildInfo)[0].labels,
        [("version", env!("CARGO_PKG_VERSION").to_owned())]
    );
}

#[test]
fn counts_tokens_by_type_and_events_by_kind() {
    let mut seed = Seed::new();
    let mut e = seed.ev("e1", "2026-02-01T10:00:00Z");
    e.tokens = TokenCounts {
        input_uncached: 100,
        cache_read: 300,
        cache_write_5m: 50,
        cache_write_1h: 0,
        output: 20,
        reasoning: 5,
    };
    let mut compaction = seed.ev("e2", "2026-02-01T11:00:00Z");
    compaction.kind = EventKind::Compaction;
    seed.put(&[e, compaction]);
    let folded = everything(&seed, &take(&seed));

    let tokens = |ty| sum(&folded, Family::Tokens, &[("type", ty)]);
    assert_eq!(tokens("input_uncached"), 101);
    assert_eq!(tokens("cache_read"), 300);
    assert_eq!(tokens("cache_write_5m"), 50);
    assert_eq!(tokens("cache_write_1h"), 0, "zero series are not emitted");
    assert_eq!(tokens("output"), 30);
    assert!(
        of(&folded, Family::Tokens)
            .iter()
            .all(|s| label(s, "type") != "cache_write_1h")
    );
    assert_eq!(sum(&folded, Family::ReasoningTokens, &[]), 5);
    assert_eq!(sum(&folded, Family::Events, &[("kind", "message")]), 1);
    assert_eq!(sum(&folded, Family::Events, &[("kind", "compaction")]), 1);
}

#[test]
fn daily_aggregates_and_extra_usage_are_never_counted() {
    let mut seed = Seed::new();
    let mut aggregate = seed.ev("agg", "2026-02-01T00:00:00Z");
    aggregate.kind = EventKind::DailyAggregate;
    aggregate.tokens.output = 1_000_000;
    let mut extra = seed.ev("extra", "2026-02-01T00:00:00Z");
    extra.kind = EventKind::ExtraUsage;
    extra.tokens = TokenCounts::default();
    seed.put(&[aggregate, extra, seed.ev("real", "2026-02-01T01:00:00Z")]);
    let folded = everything(&seed, &take(&seed));
    assert_eq!(sum(&folded, Family::Tokens, &[("type", "output")]), 10);
    assert_eq!(sum(&folded, Family::Events, &[]), 1);
}

#[test]
fn token_totals_equal_the_query_overview() {
    let mut seed = Seed::new();
    let events: Vec<_> = (0..30)
        .map(|n| {
            let mut e = seed.ev(
                &format!("e{n}"),
                &format!("2026-02-{:02}T10:00:00Z", 1 + n % 20),
            );
            e.tokens = TokenCounts {
                input_uncached: 100 + n,
                cache_read: 1000 * n,
                cache_write_5m: n,
                cache_write_1h: n % 3,
                output: 7 + n,
                reasoning: n % 4,
            };
            e.session = SessionId::try_from(format!("s{}", n % 5)).unwrap();
            e
        })
        .collect();
    seed.put(&events);
    let folded = everything(&seed, &take(&seed));

    let reader = seed.reader();
    let ctx = QueryContext::new(
        TimeZone::UTC,
        WeekStart::Monday,
        Currency::Usd,
        FxPolicy::none(),
        now(),
    );
    let range = DateRange::new(date(2026, 1, 1), date(2026, 4, 1)).unwrap();
    let usage = overview(&reader, &ctx, &Filters::new(range)).unwrap().usage;
    assert_eq!(sum(&folded, Family::Tokens, &[]), usage.total_tokens);
    assert_eq!(sum(&folded, Family::Events, &[]), usage.events);
    assert_eq!(sum(&folded, Family::ReasoningTokens, &[]), usage.reasoning);
    assert_eq!(
        sum(&folded, Family::Tokens, &[("type", "output")]),
        usage.output
    );
}

#[test]
fn a_session_that_ended_long_ago_counts_in_totals_only() {
    let mut seed = Seed::new();
    let mut old = seed.ev("old", "2026-01-01T10:00:00Z");
    old.session = SessionId::try_from("old".to_owned()).unwrap();
    seed.put(&[old, seed.ev("new", "2026-02-28T10:00:00Z")]);
    let folded = everything(&seed, &take(&seed));
    assert_eq!(sum(&folded, Family::Events, &[]), 2);
    assert_eq!(sum(&folded, Family::SessionEvents, &[]), 1);
    let sessions = of(&folded, Family::SessionEvents);
    assert_eq!(label(sessions[0], "session_id"), "s1");
}

#[test]
fn session_expiry_follows_the_last_event_not_the_first() {
    let mut seed = Seed::new();
    let mut inside = Vec::new();
    for (n, at) in ["2026-01-20T00:00:00Z", "2026-02-27T01:00:00Z"]
        .iter()
        .enumerate()
    {
        let mut e = seed.ev(&format!("a{n}"), at);
        e.session = SessionId::try_from("a".to_owned()).unwrap();
        e.git_branch = Some("main".to_owned());
        inside.push(e);
    }
    let mut outside = seed.ev("b", "2026-02-26T23:00:00Z");
    outside.session = SessionId::try_from("b".to_owned()).unwrap();
    inside.push(outside);
    seed.put(&inside);

    let taken = take(&seed);
    let folded = everything(&seed, &taken);
    // Session `a` is 47 hours old, `b` 49: only `a` is active, with its whole history.
    let tokens = of(&folded, Family::SessionTokens);
    assert!(tokens.iter().all(|s| label(s, "session_id") == "a"));
    assert_eq!(
        sum(&folded, Family::SessionEvents, &[("session_id", "a")]),
        2
    );
    assert_eq!(
        sum(&folded, Family::SessionEvents, &[("git_branch", "main")]),
        2
    );
    assert_eq!(
        sum(&folded, Family::SessionEvents, &[("session_id", "b")]),
        0
    );
    assert_eq!(
        taken.next_expiry(),
        Some(ts("2026-02-27T01:00:00Z") + SignedDuration::from_hours(48))
    );
}

#[test]
fn no_active_session_means_no_expiry_deadline() {
    let mut seed = Seed::new();
    seed.put(&[seed.ev("old", "2026-01-01T00:00:00Z")]);
    assert_eq!(take(&seed).next_expiry(), None);
}

#[test]
fn the_next_expiry_is_the_earliest_of_the_active_sessions() {
    let mut seed = Seed::new();
    let mut events = Vec::new();
    for (session, at) in [("a", "2026-02-28T20:00:00Z"), ("b", "2026-02-27T06:00:00Z")] {
        let mut e = seed.ev(session, at);
        e.session = SessionId::try_from(session.to_owned()).unwrap();
        events.push(e);
    }
    seed.put(&events);
    assert_eq!(
        take(&seed).next_expiry(),
        Some(ts("2026-02-27T06:00:00Z") + SignedDuration::from_hours(48))
    );
}

fn seed_projects() -> Seed {
    let mut seed = Seed::new();
    seed.project(
        "/w/secret/a",
        "github.com/acme/secret",
        "secret",
        Some("github.com/acme/secret"),
    );
    seed.project(
        "/w/open/b",
        "github.com/acme/open",
        "open",
        Some("github.com/acme/open"),
    );
    let a = seed.ev_in(
        "a",
        "2026-02-28T10:00:00Z",
        "/w/secret/a",
        "github.com/acme/secret",
    );
    let b = seed.ev_in(
        "b",
        "2026-02-28T11:00:00Z",
        "/w/open/b",
        "github.com/acme/open",
    );
    seed.put(&[a.clone(), b.clone()]);
    seed.call("ca", Some(&a), "s1", "Read");
    seed.call("cb", Some(&b), "s1", "Read");
    seed
}

#[test]
fn an_excluded_path_leaves_that_output_only() {
    let seed = seed_projects();
    let taken = take(&seed);

    let all = everything(&seed, &taken);
    let no_secret = fold(&seed, &taken, &filter(&[], &["/w/secret/*"]));
    for (folded, tokens, events, calls, sessions) in
        [(&all, 22, 2, 2, 2), (&no_secret, 11, 1, 1, 1)]
    {
        assert_eq!(sum(folded, Family::Tokens, &[]), tokens);
        assert_eq!(sum(folded, Family::Events, &[]), events);
        assert_eq!(sum(folded, Family::ToolCalls, &[]), calls);
        assert_eq!(sum(folded, Family::SessionEvents, &[]), sessions);
        assert_eq!(sum(folded, Family::SessionToolCalls, &[]), calls);
    }
    assert_eq!(
        sum(
            &no_secret,
            Family::Tokens,
            &[("project", "github.com/acme/secret")]
        ),
        0
    );
    // The first fold is untouched by the second.
    assert_eq!(sum(&all, Family::Tokens, &[]), 22);
}

#[test]
fn an_excluded_project_matches_the_key_the_name_and_the_remote() {
    let seed = seed_projects();
    let taken = take(&seed);
    for pattern in ["github.com/acme/secret", "secret", "github.com/acme/s*"] {
        let folded = fold(&seed, &taken, &filter(&[pattern], &[]));
        assert_eq!(sum(&folded, Family::Events, &[]), 1, "{pattern}");
        assert_eq!(sum(&folded, Family::ToolCalls, &[]), 1, "{pattern}");
    }
    let none = fold(&seed, &taken, &filter(&["github.com/*"], &[]));
    assert_eq!(
        sum(&none, Family::Events, &[]),
        2,
        "`*` does not cross `/`, so a host wildcard matches nothing"
    );
    let all = fold(&seed, &taken, &filter(&["github.com/*/*"], &[]));
    assert_eq!(sum(&all, Family::Events, &[]), 0);
}

#[test]
fn a_tool_call_without_an_event_takes_its_sessions_directory() {
    let mut seed = Seed::new();
    let known = seed.ev_in(
        "e",
        "2026-02-28T10:00:00Z",
        "/w/secret/a",
        "github.com/acme/secret",
    );
    let mut elsewhere = seed.ev_in(
        "f",
        "2026-02-28T10:00:00Z",
        "/w/open/b",
        "github.com/acme/open",
    );
    elsewhere.session = SessionId::try_from("s2".to_owned()).unwrap();
    // The earliest event of `s3` has no cwd, a later one does: the later one decides.
    let mut blank = seed.ev("g1", "2026-02-27T10:00:00Z");
    blank.session = SessionId::try_from("s3".to_owned()).unwrap();
    let mut later = seed.ev_in(
        "g2",
        "2026-02-28T10:00:00Z",
        "/w/secret/c",
        "github.com/acme/secret",
    );
    later.session = SessionId::try_from("s3".to_owned()).unwrap();
    seed.put(&[known.clone(), elsewhere, blank, later]);
    seed.call("by-event", Some(&known), "unrelated", "Read");
    seed.call("by-session", None, "s1", "Read");
    seed.call("other-session", None, "s2", "Read");
    seed.call("blank-then-cwd", None, "s3", "Read");
    seed.call(
        "unknown-event",
        Some(&seed.ev("ghost", "2026-02-28T10:00:00Z")),
        "nowhere",
        "Read",
    );
    seed.call("no-session-events", None, "nowhere", "Read");

    let taken = take(&seed);
    let kept = fold(&seed, &taken, &filter(&[], &["/w/secret/*"]));
    // Excluded: by-event, by-session, blank-then-cwd. Kept: s2, and the two with no origin.
    assert_eq!(sum(&everything(&seed, &taken), Family::ToolCalls, &[]), 6);
    assert_eq!(sum(&kept, Family::ToolCalls, &[]), 3);
}

#[test]
fn a_null_cwd_is_never_excluded() {
    let mut seed = Seed::new();
    seed.put(&[seed.ev("e", "2026-02-28T10:00:00Z")]);
    let taken = take(&seed);
    let folded = fold(&seed, &taken, &filter(&["*"], &["/**", "**"]));
    assert_eq!(sum(&folded, Family::Events, &[]), 1);
    assert_eq!(
        of(&folded, Family::Events)[0]
            .labels
            .iter()
            .find(|(n, _)| *n == "project")
            .unwrap()
            .1,
        ""
    );
}

#[test]
fn the_project_label_is_the_remote_else_the_directory_name() {
    let mut seed = Seed::new();
    seed.project(
        "/w/with-remote",
        "github.com/acme/with-remote",
        "with-remote",
        Some("github.com/acme/with-remote"),
    );
    seed.project("/home/me/code/plain", "/home/me/code/plain", "plain", None);
    let a = seed.ev_in(
        "a",
        "2026-02-28T10:00:00Z",
        "/w/with-remote",
        "github.com/acme/with-remote",
    );
    let b = seed.ev_in(
        "b",
        "2026-02-28T10:00:00Z",
        "/home/me/code/plain",
        "/home/me/code/plain",
    );
    let c = seed.ev_in(
        "c",
        "2026-02-28T10:00:00Z",
        "/unmapped/deep/dir",
        "/unmapped/deep/dir",
    );
    seed.put(&[a, b, c]);
    let folded = everything(&seed, &take(&seed));
    let projects: Vec<_> = of(&folded, Family::Events)
        .iter()
        .map(|s| label(s, "project").to_owned())
        .collect();
    assert_eq!(projects, ["dir", "github.com/acme/with-remote", "plain"]);
}

#[test]
fn billing_follows_the_engines_rule_across_a_subscription_in_a_far_zone() {
    let mut seed = Seed::new();
    // Subscription days 2026-01-10 .. 2026-02-10 in Los Angeles (UTC-8): the boundaries are
    // 08:00Z, not 00:00Z.
    let times = [
        "2026-01-10T07:30:00Z",
        "2026-01-10T08:30:00Z",
        "2026-02-10T07:30:00Z",
        "2026-02-10T08:30:00Z",
        "2026-01-20T12:00:00Z",
        "2025-12-01T12:00:00Z",
    ];
    let mut events = Vec::new();
    for (n, at) in times.iter().enumerate() {
        events.push(seed.ev(&format!("e{n}"), at));
    }
    let mut other_provider = seed.ev("other", "2026-01-20T12:00:00Z");
    other_provider.provider = Some(ProviderId::try_from("openai".to_owned()).unwrap());
    events.push(other_provider);
    seed.put(&events);

    let subscription = Subscription::new(SubscriptionSpec {
        id: SubscriptionId::try_from("max".to_owned()).unwrap(),
        provider: ProviderId::try_from("anthropic".to_owned()).unwrap(),
        plan: Plan::Pro,
        agents: vec![AgentId::CLAUDE],
        profiles: None,
        fee: Money::from_micros(Currency::Usd, 100_000_000),
        period: BillingPeriod::Monthly,
        start: date(2026, 1, 10),
        end: Some(date(2026, 2, 10)),
    })
    .unwrap();
    let zone = "America/Los_Angeles";
    let taken = snapshot(
        &seed.reader(),
        &context(vec![subscription.clone()], zone),
        now(),
    )
    .unwrap();
    let folded = everything(&seed, &taken);

    let reader = seed.reader();
    let ctx = QueryContext::new(
        TimeZone::get(zone).unwrap(),
        WeekStart::Monday,
        Currency::Usd,
        FxPolicy::none(),
        now(),
    )
    .with_subscriptions(vec![subscription]);
    let range = DateRange::new(date(2025, 1, 1), date(2026, 4, 1)).unwrap();
    let engine = |billing| {
        overview(&reader, &ctx, &Filters::new(range).billing(billing))
            .unwrap()
            .usage
            .total_tokens
    };
    let subscribed = sum(&folded, Family::Tokens, &[("billing", "subscription")]);
    let api = sum(&folded, Family::Tokens, &[("billing", "api")]);
    assert_eq!(subscribed, engine(BillingFilter::Subscription(None)));
    assert_eq!(api, engine(BillingFilter::Api));
    // Events 2 (the 10th, 00:30 local) .. 3 (the 9th of February, 23:30 local) and the middle one.
    assert_eq!(subscribed, 3 * 11);
    assert_eq!(api, 4 * 11);
}

#[test]
fn billing_is_api_without_subscriptions() {
    let mut seed = Seed::new();
    seed.put(&[seed.ev("e", "2026-02-01T00:00:00Z")]);
    let folded = everything(&seed, &take(&seed));
    assert_eq!(sum(&folded, Family::Events, &[("billing", "api")]), 1);
}

#[test]
fn labels_come_from_the_event_and_the_account() {
    let mut seed = Seed::new();
    let mut e = seed.ev("e", "2026-02-28T00:00:00Z");
    e.client = Some("acme".to_owned());
    e.git_branch = Some("feature/x".to_owned());
    seed.put(&[e]);
    seed.store
        .save_account(&AccountRecord {
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            email: Some("me@example.com".to_owned()),
            organization: Some("Acme".to_owned()),
            detected_plan: None,
            rate_limit_tier: None,
            fetched_at: now(),
        })
        .unwrap();
    let folded = everything(&seed, &take(&seed));
    let totals = of(&folded, Family::Events)[0];
    assert_eq!(
        totals.labels,
        [
            ("account", "me@example.com".to_owned()),
            ("agent", "claude".to_owned()),
            ("billing", "api".to_owned()),
            ("client", "acme".to_owned()),
            ("kind", "message".to_owned()),
            ("model", "m1".to_owned()),
            ("profile", "default".to_owned()),
            ("project", String::new()),
            ("provider", "anthropic".to_owned()),
        ]
    );
    let session = of(&folded, Family::SessionEvents)[0];
    assert_eq!(label(session, "git_branch"), "feature/x");
    assert_eq!(label(session, "session_id"), "s1");
    assert!(
        totals
            .labels
            .iter()
            .all(|(n, _)| *n != "git_branch" && *n != "session_id")
    );
}

#[test]
fn the_account_falls_back_to_the_organization() {
    let mut seed = Seed::new();
    seed.put(&[seed.ev("e", "2026-02-28T00:00:00Z")]);
    seed.store
        .save_account(&AccountRecord {
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            email: None,
            organization: Some("Acme".to_owned()),
            detected_plan: None,
            rate_limit_tier: None,
            fetched_at: now(),
        })
        .unwrap();
    let folded = everything(&seed, &take(&seed));
    assert_eq!(label(of(&folded, Family::Events)[0], "account"), "Acme");
}

#[test]
fn created_is_the_earliest_row_of_the_series() {
    let mut seed = Seed::new();
    seed.put(&[
        seed.ev("late", "2026-02-20T00:00:00Z"),
        seed.ev("early", "2026-01-05T00:00:00Z"),
        seed.ev("mid", "2026-02-01T00:00:00Z"),
    ]);
    let folded = everything(&seed, &take(&seed));
    for family in [Family::Tokens, Family::Events] {
        for series in of(&folded, family) {
            assert_eq!(series.created, Some(ts("2026-01-05T00:00:00Z")));
        }
    }
    assert!(of(&folded, Family::Series)[0].created.is_none());
}

#[test]
fn hash_matches_what_sinks_write_and_drop_merges_series() {
    let mut seed = Seed::new();
    seed.project("/w/a", "github.com/acme/a", "a", Some("github.com/acme/a"));
    seed.project("/w/b", "github.com/acme/b", "b", Some("github.com/acme/b"));
    seed.put(&[
        seed.ev_in("a1", "2026-02-10T00:00:00Z", "/w/a", "github.com/acme/a"),
        seed.ev_in("a2", "2026-02-28T00:00:00Z", "/w/a", "github.com/acme/a"),
        seed.ev_in("b1", "2026-02-05T00:00:00Z", "/w/b", "github.com/acme/b"),
    ]);
    let taken = take(&seed);
    let key = seed.reader().redaction_key().clone();

    let mut hashed = filter(&[], &[]);
    hashed
        .redact
        .insert(RedactColumn::Project, RedactMode::Hash);
    hashed
        .redact
        .insert(RedactColumn::SessionId, RedactMode::Hash);
    let folded = fold(&seed, &taken, &hashed);
    let projects: Vec<_> = of(&folded, Family::Events)
        .iter()
        .map(|s| label(s, "project").to_owned())
        .collect();
    let mut expected = vec![key.hash("github.com/acme/a"), key.hash("github.com/acme/b")];
    expected.sort();
    assert_eq!(projects, expected);
    assert_eq!(
        label(of(&folded, Family::SessionEvents)[0], "session_id"),
        key.hash("s1")
    );

    let mut dropped = filter(&[], &[]);
    dropped
        .redact
        .insert(RedactColumn::Project, RedactMode::Drop);
    let folded = fold(&seed, &taken, &dropped);
    let events = of(&folded, Family::Events);
    assert_eq!(events.len(), 1, "two projects became one series");
    assert_eq!(label(events[0], "project"), "");
    assert_eq!(events[0].value, Value::Count(3));
    assert_eq!(events[0].created, Some(ts("2026-02-05T00:00:00Z")));
}

#[test]
fn redaction_leaves_an_empty_label_empty() {
    let mut seed = Seed::new();
    seed.put(&[seed.ev("e", "2026-02-28T00:00:00Z")]);
    let mut hashed = filter(&[], &[]);
    hashed.redact.insert(RedactColumn::Client, RedactMode::Hash);
    hashed
        .redact
        .insert(RedactColumn::Account, RedactMode::Hash);
    let folded = fold(&seed, &take(&seed), &hashed);
    let events = of(&folded, Family::Events)[0];
    assert_eq!(label(events, "client"), "");
    assert_eq!(label(events, "account"), "");
}

#[test]
fn quota_gauges_come_from_the_newest_successful_poll() {
    let mut seed = Seed::new();
    let window = |id: WindowId, percent: f64, resets: &str| QuotaWindow {
        id,
        state: QuotaWindowState {
            utilization: Utilization::new(percent).unwrap(),
            resets_at: Some(ts(resets)),
        },
    };
    let poll = |at: &str, outcome| QuotaPoll {
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        account: Some("polled@example.com".to_owned()),
        fetched_at: ts(at),
        outcome,
    };
    let windows = |five: f64, seven: f64| QuotaOutcome::Windows {
        windows: vec![
            window(WindowId::five_hour(), five, "2026-02-28T15:00:00Z"),
            window(WindowId::seven_day(), seven, "2026-03-05T00:00:00Z"),
        ],
        extra_usage: None,
    };
    seed.store
        .upsert_quota_snapshots(
            &[
                poll("2026-02-28T09:00:00Z", windows(10.0, 5.0)),
                poll("2026-02-28T10:00:00Z", windows(40.0, 12.5)),
                poll("2026-02-28T11:00:00Z", QuotaOutcome::SignedOut),
            ],
            &TimeZone::UTC,
        )
        .unwrap();
    let folded = everything(&seed, &take(&seed));

    let used = of(&folded, Family::QuotaUsedRatio);
    assert_eq!(used.len(), 2);
    let five = used
        .iter()
        .find(|s| label(s, "window") == "five_hour")
        .unwrap();
    assert!((gauge(five) - 0.4).abs() < 1e-9);
    let seven = used
        .iter()
        .find(|s| label(s, "window") == "seven_day")
        .unwrap();
    assert!((gauge(seven) - 0.125).abs() < 1e-9);
    assert_eq!(label(five, "account"), "polled@example.com");
    for series in of(&folded, Family::QuotaFetchedAt) {
        assert!((gauge(series) - ts("2026-02-28T10:00:00Z").as_second() as f64).abs() < 1e-6);
    }
    let resets = of(&folded, Family::QuotaResetsAt);
    assert_eq!(resets.len(), 2);
    assert!(
        resets
            .iter()
            .any(|s| (gauge(s) - ts("2026-02-28T15:00:00Z").as_second() as f64).abs() < 1e-6)
    );
}

#[test]
fn quota_ignores_exclusions_and_money_windows() {
    let mut seed = seed_projects();
    seed.store
        .upsert_quota_snapshots(
            &[QuotaPoll {
                agent: AgentId::CLAUDE,
                profile: ProfileId::default_profile(),
                account: None,
                fetched_at: ts("2026-02-28T10:00:00Z"),
                outcome: QuotaOutcome::Windows {
                    windows: vec![QuotaWindow {
                        id: WindowId::five_hour(),
                        state: QuotaWindowState {
                            utilization: Utilization::new(20.0).unwrap(),
                            resets_at: None,
                        },
                    }],
                    extra_usage: Some(tokenmtr_core::ExtraUsage {
                        utilization: Utilization::new(10.0).unwrap(),
                        spent: tokenmtr_core::UsdMicros::new(5_000_000),
                        limit: tokenmtr_core::UsdMicros::new(50_000_000),
                    }),
                },
            }],
            &TimeZone::UTC,
        )
        .unwrap();
    let folded = fold(&seed, &take(&seed), &filter(&["*/*/*"], &["/**"]));
    assert_eq!(of(&folded, Family::QuotaUsedRatio).len(), 1);
    assert!(
        of(&folded, Family::QuotaResetsAt).is_empty(),
        "no reset time, no series"
    );
    assert_eq!(of(&folded, Family::QuotaFetchedAt).len(), 1);
}

#[test]
fn the_series_gauge_counts_every_series() {
    let mut seed = Seed::new();
    seed.put(&[seed.ev("e", "2026-02-28T00:00:00Z")]);
    let folded = everything(&seed, &take(&seed));
    assert_eq!(
        gauge(of(&folded, Family::Series)[0]),
        folded.series.len() as f64
    );
}

#[test]
fn series_are_sorted_by_family_then_labels() {
    let seed = seed_projects();
    let folded = everything(&seed, &take(&seed));
    let keys: Vec<_> = folded
        .series
        .iter()
        .map(|s| (s.family, s.labels.clone()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    for series in &folded.series {
        let names: Vec<_> = series.labels.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, series.family.labels());
    }
}

/// `EXPLAIN QUERY PLAN` text of `sql` with every parameter bound to `NULL`.
fn plan(reader: &StoreReader, sql: &str) -> String {
    let mut statement = reader
        .connection()
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap();
    let params = vec![Sql::Null; statement.parameter_count()];
    let rows = statement
        .query_map(params_from_iter(params), |row| row.get::<_, String>(3))
        .unwrap();
    rows.collect::<Result<Vec<_>, _>>().unwrap().join("\n")
}

#[test]
fn the_session_queries_ride_the_session_and_time_indexes() {
    let seed = Seed::new();
    let reader = seed.reader();
    let segments = Segments::new(&TimeZone::UTC, &[]).unwrap();

    let sessions = plan(&reader, &usage_sql(&segments, true));
    assert!(
        sessions.contains("USING INDEX usage_event_session")
            || sessions.contains("USING COVERING INDEX usage_event_session"),
        "{sessions}"
    );
    assert!(sessions.contains("usage_event_at"), "{sessions}");

    let calls = plan(&reader, &tools_sql(true));
    assert!(calls.contains("tool_call_session"), "{calls}");

    let fallback = plan(&reader, FALLBACK_SQL);
    assert!(fallback.contains("usage_event_session"), "{fallback}");

    let quota = plan(&reader, QUOTA_SQL);
    assert!(quota.contains("quota_snapshot_profile_fetched"), "{quota}");
}

#[test]
fn segments_bind_their_bounds_and_never_inline_them() {
    let subscription = |start: Date, end: Option<Date>| {
        Subscription::new(SubscriptionSpec {
            id: SubscriptionId::try_from("s".to_owned()).unwrap(),
            provider: ProviderId::try_from("anthropic".to_owned()).unwrap(),
            plan: Plan::Pro,
            agents: vec![AgentId::CLAUDE],
            profiles: None,
            fee: Money::from_micros(Currency::Usd, 1),
            period: BillingPeriod::Monthly,
            start,
            end,
        })
        .unwrap()
    };
    let segments = Segments::new(
        &TimeZone::UTC,
        &[
            subscription(date(2026, 1, 1), Some(date(2026, 2, 1))),
            subscription(date(2026, 2, 1), None),
        ],
    )
    .unwrap();
    assert_eq!(
        segments.expression("u.at_us"),
        "(u.at_us >= ?1) + (u.at_us >= ?2)",
        "the shared 2026-02-01 boundary is bound once"
    );
    assert_eq!(segments.params().len(), 2);
    assert_eq!(
        Segments::new(&TimeZone::UTC, &[])
            .unwrap()
            .expression("u.at_us"),
        "CAST(0 AS INTEGER)"
    );
}

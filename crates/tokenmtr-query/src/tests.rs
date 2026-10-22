//! Behavioural tests over a real store seeded through the store's own writers.
#![expect(
    clippy::assert_is_empty,
    clippy::too_many_lines,
    reason = "scenario tests read top to bottom; emptiness asserts state the intent"
)]

use std::cell::{RefCell, RefMut};

use camino::Utf8PathBuf;
use jiff::civil::{Date, date};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use tempfile::TempDir;
use tokenmtr_core::event::ToolCall;
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{
    AgentId, Alert, AlertKind, BillingPeriod, Budget, BudgetId, BudgetMeasure, BudgetPeriod,
    Currency, DedupKey, Estimate, EurPerUsd, EventCost, EventId, EventKind, ExtraUsage, FxRate,
    MachineId, ModelId, Money, Plan, PlanCatalog, PricingCatalog, PricingVersion, ProfileId,
    ProjectKey, ProviderId, QuotaEta, QuotaWindow, SessionId, SignalKind, SinkId, SourceId,
    Subscription, SubscriptionId, SubscriptionSpec, TokenCounts, UsageEvent, UsdMicros,
    Utilization, WindowId,
};
use tokenmtr_store::{
    Dataset, FxSource, ImportRun, IngestRun, MetricsRun, ProfiledToolCall, QuotaOutcome, QuotaPoll,
    SessionSignal, SinkRun, Store, StoreRead, StoreReader,
};

use crate::*;

fn clock() -> Timestamp {
    ts("2026-01-01T00:00:00Z")
}

fn ts(text: &str) -> Timestamp {
    text.parse().unwrap()
}

/// A local wall-clock time in `zone`.
fn local(zone: &str, wall: &str) -> Timestamp {
    wall.parse::<jiff::civil::DateTime>()
        .unwrap()
        .to_zoned(TimeZone::get(zone).unwrap())
        .unwrap()
        .timestamp()
}

struct Seed {
    _dir: TempDir,
    path: Utf8PathBuf,
    store: RefCell<Store>,
    machine: MachineId,
}

impl Seed {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("db").join("tokenmtr.db")).unwrap();
        let store = Store::open(&path).unwrap().with_clock(clock);
        let machine = store.machine_id();
        Self {
            _dir: dir,
            path,
            store: RefCell::new(store),
            machine,
        }
    }

    fn store(&self) -> RefMut<'_, Store> {
        self.store.borrow_mut()
    }

    fn ev(&self, key: &str, at: Timestamp) -> UsageEvent {
        let dedup_key = DedupKey::try_from(key.to_owned()).unwrap();
        UsageEvent {
            id: EventId::new(&self.machine, AgentId::CLAUDE, &dedup_key),
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            dedup_key,
            at,
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
            model: ModelId::try_from("m1".to_owned()).unwrap(),
            kind: EventKind::Message,
            tokens: TokenCounts {
                output: 1,
                ..TokenCounts::default()
            },
            cost: EventCost::new(None, None),
            service_tier: None,
        }
    }

    fn put(&self, events: &[UsageEvent]) {
        self.store().upsert_events(events).unwrap();
    }

    fn reader(&self) -> StoreReader {
        StoreReader::open(&self.path).unwrap()
    }
}

fn reported(mut e: UsageEvent, micros: i64) -> UsageEvent {
    e.cost = EventCost::new(Some(UsdMicros::new(micros)), e.cost.estimated().cloned());
    e
}

fn estimated(mut e: UsageEvent, micros: i64) -> UsageEvent {
    e.cost = EventCost::new(
        e.cost.reported(),
        Some(Estimate {
            usd: UsdMicros::new(micros),
            pricing_version: PricingVersion::of_body(b"fixture"),
        }),
    );
    e
}

fn model(mut e: UsageEvent, name: &str) -> UsageEvent {
    e.model = ModelId::try_from(name.to_owned()).unwrap();
    e
}

fn provider(mut e: UsageEvent, name: &str) -> UsageEvent {
    e.provider = Some(ProviderId::try_from(name.to_owned()).unwrap());
    e
}

fn ctx(zone: &str, currency: Currency, fx: FxPolicy, now: &str) -> QueryContext {
    QueryContext::new(
        TimeZone::get(zone).unwrap(),
        WeekStart::Monday,
        currency,
        fx,
        ts(now),
    )
}

fn usd_ctx(zone: &str, now: &str) -> QueryContext {
    ctx(zone, Currency::Usd, FxPolicy::none(), now)
}

fn days(from: Date, to: Date) -> Filters {
    Filters::new(DateRange::new(from, to).unwrap())
}

fn json(value: &impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

#[test]
fn empty_store_returns_zeros_not_errors() {
    let seed = Seed::new();
    let reader = seed.reader();
    let ctx = usd_ctx("UTC", "2026-01-15T12:00:00Z");
    let f = days(date(2026, 1, 1), date(2026, 1, 16));

    let totals = overview(&reader, &ctx, &f).unwrap();
    assert_eq!(totals.usage, Usage::default());
    assert_eq!(totals.sessions, 0);
    assert_eq!(totals.cache_savings_micros, None);
    assert_eq!(totals.roi, None);
    assert!(models(&reader, &ctx, &f).unwrap().rows.is_empty());
    assert!(
        projects(&reader, &ctx, &f, ProjectGrouping::Project)
            .unwrap()
            .rows
            .is_empty()
    );
    assert_eq!(sessions(&reader, &ctx, &f, 0, 50).unwrap().total, 0);
    assert!(quota_latest(&reader, &ctx, &f).unwrap().is_empty());
    assert!(tools(&reader, &ctx, &f).unwrap().tools.is_empty());
    assert_eq!(heatmap(&reader, &ctx, &f).unwrap().cells.len(), 168);
    let s = series(&reader, &ctx, &f, Bucket::Day, GroupBy::Model).unwrap();
    assert_eq!(s.points.len(), 15);
    assert!(s.points.iter().all(|p| p.usage == Usage::default()));
    let signals = signals(&reader, &ctx, &f).unwrap();
    assert_eq!((signals.api_errors, signals.compactions), (0, 0));
    assert!(billing(&reader, &ctx, &f).unwrap().months.is_empty());
}

#[test]
fn totals_follow_cost_semantics_and_token_rules() {
    let seed = Seed::new();
    let at = ts("2026-01-15T12:00:00Z");
    let mut e1 = reported(seed.ev("e1", at), 1_000_000);
    e1.tokens = TokenCounts {
        input_uncached: 100,
        cache_read: 300,
        cache_write_5m: 50,
        cache_write_1h: 50,
        output: 200,
        reasoning: 80,
    };
    let mut e2 = estimated(seed.ev("e2", at), 500_000);
    e2.tokens = TokenCounts {
        input_uncached: 100,
        cache_read: 100,
        output: 100,
        ..TokenCounts::default()
    };
    let e3 = seed.ev("e3", at);
    // Both costs known: the reported one wins.
    let mut e4 = estimated(reported(seed.ev("e4", at), 700_000), 900_000);
    e4.tokens.output = 5;
    seed.put(&[e1, e2, e3, e4]);

    let totals = overview(
        &seed.reader(),
        &usd_ctx("UTC", "2026-01-15T13:00:00Z"),
        &Filters::new(DateRange::day(date(2026, 1, 15)).unwrap()),
    )
    .unwrap();
    let u = &totals.usage;
    assert_eq!(u.events, 4);
    // Reasoning is a subset of output and never added.
    assert_eq!(u.total_tokens, 700 + 300 + 1 + 5);
    assert_eq!(u.reasoning, 80);
    assert_eq!(u.api_value_micros, 1_000_000 + 500_000 + 700_000);
    assert_eq!(u.api_value_usd_micros, u.api_value_micros);
    assert_eq!(u.paid_micros, u.api_value_micros);
    assert_eq!(
        (u.reported_events, u.estimated_events, u.unpriced_events),
        (2, 1, 1)
    );
    assert!(u.approximate);
    // cache_read 400 of input 200 + 400 + 100.
    assert!((totals.cached_input_percent - 100.0 * 400.0 / 700.0).abs() < 1e-9);
    assert_eq!(totals.sessions, 1);
}

#[test]
fn cache_savings_use_the_catalog_uncached_input_rate() {
    let seed = Seed::new();
    let mut e = provider(
        model(
            reported(seed.ev("e", ts("2026-01-15T12:00:00Z")), 1),
            "claude-sonnet-4-5",
        ),
        "anthropic",
    );
    e.tokens.cache_read = 1_000_000;
    seed.put(&[e]);
    let catalog = PricingCatalog::parse(
        br#"{"claude-sonnet-4-5": {"litellm_provider": "anthropic", "mode": "chat",
        "input_cost_per_token": 3e-06, "output_cost_per_token": 1.5e-05,
        "cache_read_input_token_cost": 3e-07}}"#,
    )
    .unwrap();
    let f = Filters::new(DateRange::day(date(2026, 1, 15)).unwrap());
    let with = usd_ctx("UTC", "2026-01-15T13:00:00Z").with_pricing(catalog);
    // 1M cached tokens × ($3 − $0.30) per million.
    assert_eq!(
        overview(&seed.reader(), &with, &f)
            .unwrap()
            .cache_savings_micros,
        Some(2_700_000)
    );
}

#[test]
fn series_buckets_by_local_day_across_the_paris_spring_dst_change() {
    let seed = Seed::new();
    let z = "Europe/Paris";
    seed.put(&[
        seed.ev("d", local(z, "2026-03-28T23:30:00")),
        seed.ev("a", local(z, "2026-03-29T00:30:00")),
        seed.ev("b", local(z, "2026-03-29T23:30:00")),
        seed.ev("c", local(z, "2026-03-30T00:30:00")),
    ]);
    let reader = seed.reader();
    let c = usd_ctx(z, "2026-04-01T00:00:00Z");
    let f = days(date(2026, 3, 28), date(2026, 3, 31));

    let day = series(&reader, &c, &f, Bucket::Day, GroupBy::None).unwrap();
    let got: Vec<_> = day
        .points
        .iter()
        .map(|p| (p.start, p.usage.events, p.minutes))
        .collect();
    assert_eq!(
        got,
        vec![
            (date(2026, 3, 28), 1, 1440),
            (date(2026, 3, 29), 2, 1380), // 23-hour day
            (date(2026, 3, 30), 1, 1440),
        ]
    );
    // The same events in UTC land elsewhere: zone-correct bucketing matters.
    let utc = series(
        &reader,
        &usd_ctx("UTC", "2026-04-01T00:00:00Z"),
        &f,
        Bucket::Day,
        GroupBy::None,
    )
    .unwrap();
    assert_eq!(
        utc.points
            .iter()
            .map(|p| p.usage.events)
            .collect::<Vec<_>>(),
        vec![2, 2, 0]
    );

    let week = series(&reader, &c, &f, Bucket::Week, GroupBy::None).unwrap();
    let got: Vec<_> = week
        .points
        .iter()
        .map(|p| (p.start, p.usage.events, p.minutes))
        .collect();
    assert_eq!(
        got,
        vec![
            (date(2026, 3, 23), 3, 7 * 1440 - 60),
            (date(2026, 3, 30), 1, 7 * 1440),
        ]
    );
    let month = series(&reader, &c, &f, Bucket::Month, GroupBy::None).unwrap();
    assert_eq!(month.points.len(), 1);
    assert_eq!(month.points[0].usage.events, 4);

    let hm = heatmap(&reader, &c, &f).unwrap();
    let cell = |weekday: u8, hour: u8| {
        hm.cells
            .iter()
            .find(|x| x.weekday == weekday && x.hour == hour)
            .unwrap()
            .events
    };
    assert_eq!(cell(7, 23), 1); // Sunday 23:30
    assert_eq!(cell(7, 0), 1); // Sunday 00:30
    assert_eq!(cell(1, 0), 1); // Monday 00:30
    assert_eq!(cell(6, 23), 1); // Saturday 23:30
}

#[test]
fn series_buckets_by_local_day_across_the_paris_autumn_dst_change() {
    let seed = Seed::new();
    let z = "Europe/Paris";
    seed.put(&[
        seed.ev("a", local(z, "2026-10-25T00:30:00")),
        seed.ev("b", local(z, "2026-10-25T23:30:00")),
        seed.ev("c", local(z, "2026-10-26T00:30:00")),
    ]);
    let s = series(
        &seed.reader(),
        &usd_ctx(z, "2026-11-01T00:00:00Z"),
        &days(date(2026, 10, 24), date(2026, 10, 27)),
        Bucket::Day,
        GroupBy::None,
    )
    .unwrap();
    let got: Vec<_> = s
        .points
        .iter()
        .map(|p| (p.usage.events, p.minutes))
        .collect();
    assert_eq!(got, vec![(0, 1440), (2, 1500), (1, 1440)]); // 25-hour day
}

#[test]
fn series_buckets_by_local_day_across_the_new_york_dst_change() {
    let seed = Seed::new();
    let z = "America/New_York";
    seed.put(&[
        seed.ev("a", local(z, "2026-03-08T00:30:00")),
        seed.ev("b", local(z, "2026-03-08T23:30:00")),
        seed.ev("c", local(z, "2026-03-09T00:30:00")),
    ]);
    let s = series(
        &seed.reader(),
        &usd_ctx(z, "2026-03-20T00:00:00Z"),
        &days(date(2026, 3, 7), date(2026, 3, 10)),
        Bucket::Day,
        GroupBy::None,
    )
    .unwrap();
    let got: Vec<_> = s
        .points
        .iter()
        .map(|p| (p.usage.events, p.minutes))
        .collect();
    assert_eq!(got, vec![(0, 1440), (2, 1380), (1, 1440)]);
}

#[test]
fn week_buckets_follow_the_configured_week_start() {
    let seed = Seed::new();
    // 2026-03-29 is a Sunday.
    seed.put(&[seed.ev("a", ts("2026-03-29T12:00:00Z"))]);
    let sunday = QueryContext::new(
        TimeZone::UTC,
        WeekStart::Sunday,
        Currency::Usd,
        FxPolicy::none(),
        ts("2026-04-01T00:00:00Z"),
    );
    let s = series(
        &seed.reader(),
        &sunday,
        &days(date(2026, 3, 29), date(2026, 3, 31)),
        Bucket::Week,
        GroupBy::None,
    )
    .unwrap();
    assert_eq!(s.points.len(), 1);
    assert_eq!(s.points[0].start, date(2026, 3, 29));
    let monday = usd_ctx("UTC", "2026-04-01T00:00:00Z");
    let s = series(
        &seed.reader(),
        &monday,
        &days(date(2026, 3, 29), date(2026, 3, 31)),
        Bucket::Week,
        GroupBy::None,
    )
    .unwrap();
    assert_eq!(s.points[0].start, date(2026, 3, 23));
}

fn seed_fx(seed: &Seed) {
    let rate = |d: Date, scaled: i64| FxRate {
        date: d,
        eur_per_usd: EurPerUsd::from_scaled(scaled).unwrap(),
    };
    seed.store()
        .upsert_fx_rates(
            FxSource::Ecb,
            &[
                rate(date(2026, 3, 9), 900_000_000),
                rate(date(2026, 3, 10), 800_000_000),
            ],
        )
        .unwrap();
}

#[test]
fn eur_uses_the_rate_strictly_before_the_event_date() {
    let seed = Seed::new();
    seed_fx(&seed);
    seed.put(&[
        reported(
            seed.ev("before-history", ts("2026-03-09T12:00:00Z")),
            10_000_000,
        ),
        reported(
            seed.ev("same-day-as-rate", ts("2026-03-10T12:00:00Z")),
            10_000_000,
        ),
        reported(seed.ev("day-after", ts("2026-03-11T12:00:00Z")), 10_000_000),
        reported(seed.ev("stale", ts("2026-03-20T12:00:00Z")), 10_000_000),
    ]);
    let reader = seed.reader();
    let fx = FxPolicy::from_rates(&load_fx_rates(&reader).unwrap());
    let c = ctx("UTC", Currency::Eur, fx, "2026-03-25T00:00:00Z");
    let s = series(
        &reader,
        &c,
        &days(date(2026, 3, 9), date(2026, 3, 21)),
        Bucket::Day,
        GroupBy::None,
    )
    .unwrap();
    let point = |d: Date| s.points.iter().find(|p| p.start == d).unwrap();
    assert_eq!(s.currency, Currency::Eur);
    // No rate dated before 03-09: oldest rate, flagged.
    assert_eq!(point(date(2026, 3, 9)).usage.api_value_micros, 9_000_000);
    assert!(point(date(2026, 3, 9)).usage.approximate);
    // A rate dated 03-10 does not apply to events of 03-10.
    assert_eq!(point(date(2026, 3, 10)).usage.api_value_micros, 9_000_000);
    assert!(!point(date(2026, 3, 10)).usage.approximate);
    assert_eq!(point(date(2026, 3, 11)).usage.api_value_micros, 8_000_000);
    // Newest rate is ten days old: usable but approximate.
    assert_eq!(point(date(2026, 3, 20)).usage.api_value_micros, 8_000_000);
    assert!(point(date(2026, 3, 20)).usage.approximate);
    // USD stays available next to the converted amount.
    assert_eq!(
        point(date(2026, 3, 11)).usage.api_value_usd_micros,
        10_000_000
    );
}

#[test]
fn eur_without_rates_falls_back_to_usd_and_says_so() {
    let seed = Seed::new();
    seed.put(&[reported(
        seed.ev("a", ts("2026-03-09T12:00:00Z")),
        10_000_000,
    )]);
    let c = ctx(
        "UTC",
        Currency::Eur,
        FxPolicy::none(),
        "2026-03-25T00:00:00Z",
    );
    assert!(c.currency_fallback());
    let t = overview(
        &seed.reader(),
        &c,
        &days(date(2026, 3, 9), date(2026, 3, 10)),
    )
    .unwrap();
    assert_eq!(t.currency, Currency::Usd);
    assert!(t.currency_fallback);
    assert_eq!(t.usage.api_value_micros, 10_000_000);
}

fn anthropic_subscription() -> Subscription {
    Subscription::new(SubscriptionSpec {
        id: SubscriptionId::try_from("claude-max".to_owned()).unwrap(),
        provider: ProviderId::try_from("anthropic".to_owned()).unwrap(),
        plan: Plan::Pro,
        agents: vec![AgentId::CLAUDE],
        profiles: None,
        fee: Money::from_micros(Currency::Usd, 30_000_000),
        period: BillingPeriod::Monthly,
        start: date(2026, 4, 1),
        end: None,
    })
    .unwrap()
}

fn seed_mixed_billing() -> Seed {
    let seed = Seed::new();
    let at = ts("2026-04-10T10:00:00Z");
    seed.put(&[
        provider(
            model(reported(seed.ev("a", at), 3_000_000), "m1"),
            "anthropic",
        ),
        provider(
            model(reported(seed.ev("b", at), 1_000_000), "m2"),
            "anthropic",
        ),
        provider(model(reported(seed.ev("c", at), 2_000_000), "m3"), "openai"),
    ]);
    seed
}

#[test]
fn paid_is_prorated_fee_plus_api_events_and_api_value_stays_separate() {
    let seed = seed_mixed_billing();
    let reader = seed.reader();
    let c =
        usd_ctx("UTC", "2026-04-10T18:00:00Z").with_subscriptions(vec![anthropic_subscription()]);
    let f = days(date(2026, 4, 1), date(2026, 4, 11));

    // April has 30 days: $1 per day over the 10 elapsed days, used or not.
    let t = overview(&reader, &c, &f).unwrap();
    assert_eq!(t.usage.api_value_micros, 6_000_000);
    assert_eq!(t.usage.paid_micros, 10_000_000 + 2_000_000);
    assert_eq!(t.usage.subscription_fee_micros, 10_000_000);
    assert_eq!(t.usage.subscription_value_micros, 4_000_000);
    assert!((t.roi.unwrap() - 0.4).abs() < 1e-9);

    // The day's $1 fee is shared by API-value share inside the subscription.
    let by_model = models(&reader, &c, &f).unwrap().rows;
    let paid = |name: &str| {
        by_model
            .iter()
            .find(|m| m.model == name)
            .unwrap()
            .usage
            .paid_micros
    };
    assert_eq!(
        (paid("m1"), paid("m2"), paid("m3")),
        (750_000, 250_000, 2_000_000)
    );

    // A scoped view charges only its share and none of the idle fees.
    let one = overview(
        &reader,
        &c,
        &f.clone()
            .models([ModelId::try_from("m1".to_owned()).unwrap()]),
    )
    .unwrap();
    assert_eq!(one.usage.paid_micros, 750_000);
    assert_eq!(one.usage.api_value_micros, 3_000_000);

    let api_only = overview(&reader, &c, &f.clone().billing(BillingFilter::Api)).unwrap();
    assert_eq!(api_only.usage.paid_micros, 2_000_000);
    let sub_only = overview(
        &reader,
        &c,
        &f.clone().billing(BillingFilter::Subscription(None)),
    )
    .unwrap();
    assert_eq!(sub_only.usage.api_value_micros, 4_000_000);
    assert_eq!(sub_only.usage.paid_micros, 1_000_000);

    let report = billing(&reader, &c, &f).unwrap();
    assert_eq!(report.total.paid_micros, 12_000_000);
    assert_eq!(report.api.paid_micros, 2_000_000);
    assert_eq!(report.subscriptions.len(), 1);
    assert_eq!(
        report.subscriptions[0].usage.subscription_fee_micros,
        10_000_000
    );
    assert!((report.subscriptions[0].roi.unwrap() - 0.4).abs() < 1e-9);
    assert_eq!(report.months.len(), 1);
    assert_eq!(report.months[0].month, date(2026, 4, 1));

    // The series carries the idle fee days too: paid sums to the total.
    let s = series(&reader, &c, &f, Bucket::Day, GroupBy::None).unwrap();
    assert_eq!(s.points[0].usage.paid_micros, 1_000_000);
    assert_eq!(s.points[9].usage.paid_micros, 3_000_000);
    assert_eq!(s.points.last().unwrap().cumulative_paid_micros, 12_000_000);
}

#[test]
fn fees_do_not_accrue_for_future_days() {
    let seed = Seed::new();
    let c =
        usd_ctx("UTC", "2026-04-10T18:00:00Z").with_subscriptions(vec![anthropic_subscription()]);
    let t = overview(
        &seed.reader(),
        &c,
        &days(date(2026, 4, 1), date(2026, 5, 1)),
    )
    .unwrap();
    assert_eq!(t.usage.paid_micros, 10_000_000);
}

/// A Claude poll reporting `spent` of a $50 extra-usage cap at 24 %.
fn extra(spent: i64) -> QuotaOutcome {
    QuotaOutcome::Windows {
        windows: Vec::new(),
        extra_usage: Some(ExtraUsage {
            utilization: Utilization::new(24.0).unwrap(),
            spent: UsdMicros::new(spent),
            limit: UsdMicros::new(50_000_000),
        }),
    }
}

#[test]
fn extra_usage_is_paid_without_api_value_tokens_or_breakdown_rows() {
    let seed = seed_mixed_billing();
    // First observation on April 10: $5 lands on that day's extra-usage row (local noon).
    poll(
        &seed,
        "default",
        ts("2026-04-10T09:00:00Z"),
        extra(5_000_000),
    );
    let reader = seed.reader();
    let c =
        usd_ctx("UTC", "2026-04-10T18:00:00Z").with_subscriptions(vec![anthropic_subscription()]);
    let f = days(date(2026, 4, 1), date(2026, 4, 11));

    // Paid on a subscription-covered profile, never API value, tokens or events.
    let t = overview(&reader, &c, &f).unwrap();
    assert_eq!(t.usage.api_value_micros, 6_000_000);
    assert_eq!(t.usage.api_value_usd_micros, 6_000_000);
    assert_eq!(t.usage.paid_micros, 12_000_000 + 5_000_000);
    assert_eq!(t.usage.extra_usage_micros, 5_000_000);
    assert_eq!((t.usage.events, t.usage.total_tokens), (3, 3));
    assert_eq!(t.usage.subscription_fee_micros, 10_000_000);
    assert_eq!(t.sessions, 1);
    assert!(t.usage.approximate, "first observation is approximate");

    let report = billing(&reader, &c, &f).unwrap();
    assert_eq!(report.total.paid_micros, 17_000_000);
    assert_eq!(report.total.extra_usage_micros, 5_000_000);
    assert_eq!(report.api.paid_micros, 2_000_000);
    assert_eq!(report.api.extra_usage_micros, 0);
    assert_eq!(report.months[0].usage.extra_usage_micros, 5_000_000);

    // It is pay-as-you-go: the `api` billing filter keeps it, a subscription filter does not.
    let api_only = overview(&reader, &c, &f.clone().billing(BillingFilter::Api)).unwrap();
    assert_eq!(api_only.usage.paid_micros, 7_000_000);
    let sub_only = overview(
        &reader,
        &c,
        &f.clone().billing(BillingFilter::Subscription(None)),
    )
    .unwrap();
    assert_eq!(sub_only.usage.extra_usage_micros, 0);

    // Usage breakdowns leave it out.
    let by_model = models(&reader, &c, &f).unwrap().rows;
    assert_eq!(by_model.len(), 3);
    assert!(by_model.iter().all(|m| m.usage.extra_usage_micros == 0));
    assert!(
        projects(&reader, &c, &f, ProjectGrouping::Project)
            .unwrap()
            .rows
            .iter()
            .all(|p| p.usage.extra_usage_micros == 0)
    );
    let page = sessions(&reader, &c, &f, 0, 50).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(
        heatmap(&reader, &c, &f)
            .unwrap()
            .cells
            .iter()
            .map(|h| h.events)
            .sum::<u64>(),
        3
    );

    // Series: in the bucket totals and the agent split, in no model group.
    let s = series(&reader, &c, &f, Bucket::Day, GroupBy::Model).unwrap();
    assert_eq!(s.points[9].usage.paid_micros, 3_000_000 + 5_000_000);
    assert!(!s.points[9].groups.contains_key("extra_usage"));
    assert_eq!(s.points.last().unwrap().cumulative_paid_micros, 17_000_000);
    let s = series(&reader, &c, &f, Bucket::Day, GroupBy::Agent).unwrap();
    assert_eq!(
        s.points[9].groups["claude"].paid_micros,
        3_000_000 + 5_000_000
    );
    assert_eq!(s.points[9].groups["claude"].api_value_micros, 6_000_000);

    // The meter carries the money of the spend window; no reset, so no ETA.
    let meters = quota_latest(&reader, &c, &f).unwrap();
    let window = &meters[0].windows[0];
    assert_eq!(window.window.as_str(), "extra_usage");
    assert_eq!(window.resets_at, None);
    assert_eq!(window.eta, QuotaEta::NotEnoughData);
    assert_eq!(
        window.spend,
        Some(QuotaSpend {
            currency: Currency::Usd,
            spent_micros: 5_000_000,
            limit_micros: 50_000_000,
            approximate: false,
        })
    );
    assert_eq!(json(window)["spend"]["limit_micros"], 50_000_000);
}

#[test]
fn filters_compose_and_values_are_bound_not_spliced() {
    let seed = Seed::new();
    let at = ts("2026-02-02T10:00:00Z");
    let mut a = model(seed.ev("a", at), "m1");
    a.project = Some(ProjectKey::try_from("p1".to_owned()).unwrap());
    let mut b = model(seed.ev("b", at), "m2");
    b.project = Some(ProjectKey::try_from("p2".to_owned()).unwrap());
    let mut c = model(seed.ev("c", at), "m1");
    c.agent = AgentId::PI;
    c.id = EventId::new(&seed.machine, AgentId::PI, &c.dedup_key);
    c.project = Some(ProjectKey::try_from("p1".to_owned()).unwrap());
    c.client = Some("acme".to_owned());
    seed.put(&[a, b, c]);
    let reader = seed.reader();
    let ctx = usd_ctx("UTC", "2026-02-03T00:00:00Z");
    let base = Filters::new(DateRange::day(date(2026, 2, 2)).unwrap());
    let count = |f: &Filters| overview(&reader, &ctx, f).unwrap().usage.events;
    let m1 = || ModelId::try_from("m1".to_owned()).unwrap();

    assert_eq!(count(&base), 3);
    assert_eq!(count(&base.clone().agents([AgentId::PI])), 1);
    assert_eq!(count(&base.clone().models([m1()])), 2);
    assert_eq!(
        count(&base.clone().agents([AgentId::CLAUDE]).models([m1()])),
        1
    );
    assert_eq!(
        count(
            &base
                .clone()
                .agents([AgentId::CLAUDE, AgentId::PI])
                .models([m1()])
        ),
        2
    );
    assert_eq!(
        count(
            &base
                .clone()
                .projects([ProjectKey::try_from("p1".to_owned()).unwrap()])
                .clients([ClientName::try_from("acme".to_owned()).unwrap()])
        ),
        1
    );

    // Parsing untrusted input.
    let spec = FilterSpec {
        range: Some(RangeSpec::Custom {
            start: date(2026, 2, 2),
            end: date(2026, 2, 3),
        }),
        agents: vec!["claude".to_owned()],
        models: vec!["m1".to_owned()],
        ..FilterSpec::default()
    };
    assert_eq!(count(&Filters::parse(&spec, &ctx).unwrap()), 1);
    let bad = FilterSpec {
        agents: vec!["nope".to_owned()],
        ..FilterSpec::default()
    };
    assert!(matches!(
        Filters::parse(&bad, &ctx),
        Err(FilterError::BadValue { kind: "agent", .. })
    ));
    let injection = FilterSpec {
        projects: vec!["x'; DROP TABLE usage_event; --".to_owned()],
        range: Some(RangeSpec::LastDays { days: 30 }),
        ..FilterSpec::default()
    };
    assert_eq!(count(&Filters::parse(&injection, &ctx).unwrap()), 0);
    assert_eq!(count(&base), 3, "table must be intact");
    assert!(matches!(
        DateRange::new(date(2026, 2, 3), date(2026, 2, 3)),
        Err(FilterError::EmptyRange)
    ));
    assert!(matches!(
        RangeSpec::LastDays { days: 0 }.resolve(&ctx),
        Err(FilterError::ZeroDays)
    ));
}

#[test]
fn sessions_tools_signals_and_daily_aggregates() {
    let seed = Seed::new();
    let mut s1a = reported(seed.ev("s1a", ts("2026-02-02T10:00:00Z")), 100);
    s1a.git_branch = Some("main".to_owned());
    let s1b = reported(seed.ev("s1b", ts("2026-02-02T10:30:00Z")), 200);
    let mut s2 = reported(model(seed.ev("s2", ts("2026-02-02T11:00:00Z")), "m2"), 400);
    s2.session = SessionId::try_from("s2".to_owned()).unwrap();
    let mut agg = reported(seed.ev("agg", ts("2026-02-02T00:00:00Z")), 1_000);
    agg.session = SessionId::try_from("agg".to_owned()).unwrap();
    agg.kind = EventKind::DailyAggregate;
    agg.approximate = true;
    seed.put(&[s1a, s1b, s2, agg]);
    let call = |id: &str, name: &str, server: Option<&str>, err: Option<bool>, at: &str| {
        ProfiledToolCall {
            profile: ProfileId::default_profile(),
            call: ToolCall {
                agent: AgentId::CLAUDE,
                tool_use_id: id.to_owned(),
                event: None,
                session: SessionId::try_from("s1".to_owned()).unwrap(),
                at: ts(at),
                tool_name: name.to_owned(),
                mcp_server: server.map(str::to_owned),
                is_error: err,
                skill: None,
            },
        }
    };
    seed.store()
        .upsert_tool_calls(&[
            call("t1", "Read", None, Some(false), "2026-02-02T10:01:00Z"),
            call("t2", "Read", None, Some(true), "2026-02-02T10:02:00Z"),
            call("t3", "search", Some("docs"), None, "2026-02-02T10:03:00Z"),
        ])
        .unwrap();
    let signal = |session: &str, kind: SignalKind, at: &str| SessionSignal {
        agent: AgentId::CLAUDE,
        session: SessionId::try_from(session.to_owned()).unwrap(),
        kind,
        at: ts(at),
    };
    seed.store()
        .record_signals(&[
            signal("s1", SignalKind::Compaction, "2026-02-02T10:10:00Z"),
            signal("s1", SignalKind::Compaction, "2026-02-02T10:20:00Z"),
            signal("s2", SignalKind::ApiError, "2026-02-02T11:05:00Z"),
        ])
        .unwrap();
    let reader = seed.reader();
    let c = usd_ctx("UTC", "2026-02-03T00:00:00Z");
    let f = Filters::new(DateRange::day(date(2026, 2, 2)).unwrap());

    // Daily aggregates count in totals but never as sessions.
    let t = overview(&reader, &c, &f).unwrap();
    assert_eq!(t.usage.events, 4);
    assert_eq!(t.usage.api_value_micros, 1_700);
    assert_eq!(t.sessions, 2);
    assert!(t.usage.approximate);
    let page = sessions(&reader, &c, &f, 0, 50).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.rows[0].session, "s2"); // newest activity first
    let s1 = &page.rows[1];
    assert_eq!(s1.session, "s1");
    assert_eq!(s1.duration_secs, 1_800);
    assert_eq!((s1.tool_calls, s1.tool_errors), (3, 1));
    assert_eq!(s1.usage.api_value_micros, 300);
    assert_eq!(s1.models, vec!["m1"]);
    assert_eq!(page.rows[0].api_errors, 1);
    assert_eq!(
        sessions(&reader, &c, &f, 1, 1).unwrap().rows[0].session,
        "s1"
    );

    let detail = session_detail(
        &reader,
        &c,
        AgentId::CLAUDE,
        &SessionId::try_from("s1".to_owned()).unwrap(),
    )
    .unwrap();
    assert_eq!(detail.events.len(), 2);
    assert_eq!(detail.tool_calls.len(), 3);
    assert_eq!(detail.signals.len(), 2);
    assert!(!detail.events_truncated);

    let report = tools(&reader, &c, &f).unwrap();
    let read = report
        .tools
        .iter()
        .find(|t| t.tool_name.as_deref() == Some("Read"))
        .unwrap();
    assert_eq!((read.calls, read.errors, read.sessions), (2, 1, 1));
    assert!((read.error_rate_percent - 50.0).abs() < 1e-9);
    assert_eq!(report.mcp_servers.len(), 1);
    assert_eq!(report.mcp_servers[0].mcp_server.as_deref(), Some("docs"));

    let sig = signals(&reader, &c, &f).unwrap();
    assert_eq!((sig.api_errors, sig.compactions), (1, 2));
    assert_eq!(sig.top_sessions[0].session, "s1");
    assert_eq!(sig.days[0].compactions, 2);

    let by_branch = branches(&reader, &c, &f).unwrap().rows;
    assert_eq!(
        by_branch
            .iter()
            .find(|b| b.branch.as_deref() == Some("main"))
            .unwrap()
            .usage
            .events,
        1
    );
    let m = models(&reader, &c, &f).unwrap().rows;
    assert_eq!(m[0].model, "m1"); // 1_300 incl. the aggregate beats m2's 400
    assert!((m.iter().map(|r| r.share_percent).sum::<f64>() - 100.0).abs() < 1e-9);
}

#[test]
fn superseded_aggregates_never_count() {
    let seed = Seed::new();
    let mut agg = reported(seed.ev("agg", ts("2026-02-02T00:00:00Z")), 1_000);
    agg.kind = EventKind::DailyAggregate;
    agg.approximate = true;
    seed.put(&[agg, reported(seed.ev("x", ts("2026-02-02T10:07:00Z")), 100)]);
    let flagged = seed
        .store()
        .supersede_daily_aggregates_covered_by_transcripts(
            AgentId::CLAUDE,
            &ProfileId::default_profile(),
            &TimeZone::UTC,
        )
        .unwrap();
    assert_eq!(flagged, 1);
    let reader = seed.reader();
    let c = usd_ctx("UTC", "2026-02-03T00:00:00Z");
    let f = Filters::new(DateRange::day(date(2026, 2, 2)).unwrap());
    let t = overview(&reader, &c, &f).unwrap();
    assert_eq!((t.usage.events, t.usage.api_value_micros), (1, 100));
}

#[test]
fn profile_selectors_pin_the_agent_and_sessions_start_at_their_first_event() {
    let seed = Seed::new();
    let work = || ProfileId::try_from("work".to_owned()).unwrap();
    let mut claude = seed.ev("c", ts("2026-02-02T10:07:00Z"));
    claude.profile = work();
    let mut pi = seed.ev("p", ts("2026-02-02T10:08:00Z"));
    pi.agent = AgentId::PI;
    pi.id = EventId::new(&seed.machine, AgentId::PI, &pi.dedup_key);
    pi.profile = work();
    seed.put(&[claude, pi, seed.ev("d", ts("2026-02-02T10:09:00Z"))]);
    let reader = seed.reader();
    let c = usd_ctx("UTC", "2026-02-03T00:00:00Z");
    let base = Filters::new(DateRange::day(date(2026, 2, 2)).unwrap());
    let count = |f: &Filters| overview(&reader, &c, f).unwrap().usage.events;
    assert_eq!(count(&base.clone().profiles([work()])), 2);
    let spec = |profiles: &[&str]| FilterSpec {
        range: Some(RangeSpec::Custom {
            start: date(2026, 2, 2),
            end: date(2026, 2, 3),
        }),
        profiles: profiles.iter().map(|p| (*p).to_owned()).collect(),
        ..FilterSpec::default()
    };
    let pinned = Filters::parse(&spec(&["claude/work"]), &c).unwrap();
    assert_eq!(count(&pinned), 1);
    assert_eq!(pinned.profiles_of(AgentId::PI), Vec::<ProfileId>::new());
    let both = Filters::parse(&spec(&["claude/work", "pi/work"]), &c).unwrap();
    assert_eq!(count(&both), 2);
    assert!(Filters::parse(&spec(&["nope/work"]), &c).is_err());

    let page = sessions(&reader, &c, &base, 0, 50).unwrap();
    let started = |agent: AgentId| {
        page.rows
            .iter()
            .find(|r| r.agent == agent)
            .unwrap()
            .started_at
    };
    assert_eq!(started(AgentId::PI), ts("2026-02-02T10:08:00Z"));
    assert_eq!(started(AgentId::CLAUDE), ts("2026-02-02T10:07:00Z"));
}

#[test]
fn subscriptions_cover_events_by_their_local_day() {
    let seed = Seed::new();
    // 2026-03-31T23:30Z is 2026-04-01 01:30 in Paris: the first day of the subscription.
    seed.put(&[provider(
        reported(seed.ev("a", ts("2026-03-31T23:30:00Z")), 1_000_000),
        "anthropic",
    )]);
    let c = usd_ctx("Europe/Paris", "2026-04-02T00:00:00Z")
        .with_subscriptions(vec![anthropic_subscription()]);
    let f = days(date(2026, 4, 1), date(2026, 4, 2)).billing(BillingFilter::Subscription(None));
    assert_eq!(overview(&seed.reader(), &c, &f).unwrap().usage.events, 1);
}

#[test]
fn a_budget_without_any_fx_rate_is_listed_as_unmeasured() {
    let seed = Seed::new();
    let spec = BudgetSpec {
        budget: Budget::new(
            BudgetId::try_from("eur".to_owned()).unwrap(),
            BudgetPeriod::Month,
            Money::from_micros(Currency::Eur, 40_000_000),
            BudgetMeasure::ApiValue,
        )
        .unwrap(),
        scope: BudgetScope::default(),
    };
    seed.put(&[reported(
        seed.ev("a", ts("2026-02-02T09:00:00Z")),
        1_000_000,
    )]);
    let ctx = usd_ctx("UTC", "2026-02-08T12:00:00Z");
    let views = budgets(&seed.reader(), &ctx, &[spec]).unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].state, BudgetState::NoFxRate);
    assert_eq!(
        (views[0].limit_micros, views[0].spent_micros),
        (40_000_000, 0)
    );
    assert_eq!(json(&views[0])["state"], "no_fx_rate");
}

fn poll(seed: &Seed, profile: &str, at: Timestamp, outcome: QuotaOutcome) {
    seed.store()
        .upsert_quota_snapshots(
            &[QuotaPoll {
                agent: AgentId::CLAUDE,
                profile: ProfileId::try_from(profile.to_owned()).unwrap(),
                account: Some("me@example.com".to_owned()),
                fetched_at: at,
                outcome,
            }],
            &jiff::tz::TimeZone::UTC,
        )
        .unwrap();
}

fn window(percent: f64, resets_at: Timestamp) -> QuotaOutcome {
    QuotaOutcome::Windows {
        windows: vec![QuotaWindow {
            id: WindowId::five_hour(),
            state: QuotaWindowState {
                utilization: Utilization::new(percent).unwrap(),
                resets_at: Some(resets_at),
            },
        }],
        extra_usage: None,
    }
}

#[test]
fn quota_meters_history_and_eta() {
    let seed = Seed::new();
    let now = ts("2026-02-02T12:00:00Z");
    let resets = ts("2026-02-02T14:00:00Z");
    for i in 0..5_i64 {
        poll(
            &seed,
            "default",
            now.checked_sub(SignedDuration::from_mins(10 * (4 - i)))
                .unwrap(),
            window(f64::from(i32::try_from(10 * (i + 1)).unwrap()), resets),
        );
    }
    poll(&seed, "work", now, QuotaOutcome::Failed);
    let reader = seed.reader();
    let c = usd_ctx("UTC", "2026-02-02T12:00:00Z");
    let f = days(date(2026, 2, 2), date(2026, 2, 3));

    let meters = quota_latest(&reader, &c, &f).unwrap();
    assert_eq!(meters.len(), 2);
    let default = meters
        .iter()
        .find(|m| m.profile.as_str() == "default")
        .unwrap();
    assert_eq!(default.status, tokenmtr_store::QuotaStatus::Ok);
    assert_eq!(default.windows.len(), 1);
    assert!((default.windows[0].used_percent - 50.0).abs() < 1e-9);
    assert_eq!(default.windows[0].resets_in_secs, Some(7_200));
    // 1 % per minute: full in 50 minutes, long before the reset.
    assert!(matches!(default.windows[0].eta, QuotaEta::WillHit { .. }));
    let work = meters
        .iter()
        .find(|m| m.profile.as_str() == "work")
        .unwrap();
    assert_eq!(work.status, tokenmtr_store::QuotaStatus::Failed);
    assert!(work.windows.is_empty());

    let history = quota_history(
        &reader,
        &c,
        AgentId::CLAUDE,
        "default",
        DateRange::day(date(2026, 2, 2)).unwrap(),
    )
    .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].points.len(), 5);
    assert!(history[0].resets.is_empty()); // the reset is still ahead

    let later = usd_ctx("UTC", "2026-02-02T15:00:00Z");
    let history = quota_history(
        &reader,
        &later,
        AgentId::CLAUDE,
        "default",
        DateRange::day(date(2026, 2, 2)).unwrap(),
    )
    .unwrap();
    assert_eq!(history[0].resets, vec![resets]);

    // Advisor: under 14 days of polls there is not enough data.
    let report = advisor(
        &reader,
        &c,
        AgentId::CLAUDE,
        &ProfileId::default_profile(),
        Some(Plan::Pro),
        80,
    )
    .unwrap();
    assert!(matches!(
        report.advice,
        tokenmtr_core::Advice::NotEnoughData { .. }
    ));
    assert_eq!(report.snapshots, 5);
    assert!(PlanCatalog::embedded().is_ok());
}

/// 20 daily polls of the default profile peaking at `percent` of the five-hour window.
fn seed_daily_polls(percent: f64) -> Seed {
    let seed = Seed::new();
    let start = ts("2026-03-01T12:00:00Z");
    for day in 0..20 {
        let at = start
            .checked_add(SignedDuration::from_hours(24 * day))
            .unwrap();
        poll(&seed, "default", at, window(percent, at));
    }
    seed
}

fn seed_account(seed: &Seed, plan: Option<Plan>, tier: Option<&str>) {
    seed.store()
        .save_account(&tokenmtr_store::AccountRecord {
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            email: None,
            organization: Some("Wiremind".to_owned()),
            detected_plan: plan,
            rate_limit_tier: tier.map(str::to_owned),
            fetched_at: ts("2026-03-20T12:00:00Z"),
        })
        .unwrap();
}

fn team_subscription(fee: Money) -> Subscription {
    Subscription::new(SubscriptionSpec {
        id: SubscriptionId::try_from("team".to_owned()).unwrap(),
        provider: ProviderId::try_from("anthropic".to_owned()).unwrap(),
        plan: Plan::Team,
        agents: vec![AgentId::CLAUDE],
        profiles: Some(vec![ProfileId::default_profile()]),
        fee,
        period: BillingPeriod::Monthly,
        start: date(2026, 1, 1),
        end: None,
    })
    .unwrap()
}

fn advise_default(seed: &Seed, ctx: &QueryContext) -> AdvisorReport {
    advisor(
        &seed.reader(),
        ctx,
        AgentId::CLAUDE,
        &ProfileId::default_profile(),
        None,
        80,
    )
    .unwrap()
}

#[test]
fn advisor_sizes_a_team_seat_from_its_rate_tier_and_declared_fee() {
    let seed = seed_daily_polls(70.0);
    seed_account(&seed, Some(Plan::Team), Some("default_claude_max_5x"));
    let ctx = usd_ctx("UTC", "2026-03-21T00:00:00Z").with_subscriptions(vec![team_subscription(
        Money::from_micros(Currency::Usd, 100_000_000),
    )]);
    let report = advise_default(&seed, &ctx);
    assert_eq!(report.current_plan, Some(Plan::Team));
    assert_eq!(report.organization.as_deref(), Some("Wiremind"));
    let tokenmtr_core::Advice::Recommend(r) = &report.advice else {
        panic!("expected a recommendation, got {:?}", report.advice);
    };
    assert_eq!(r.sizing, tokenmtr_core::QuotaSizing::RateTier(Plan::Max5x));
    assert_eq!(r.current_monthly_fee, Some(UsdMicros::new(100_000_000)));
    let json = json(&report);
    assert_eq!(json["organization"], "Wiremind");
    assert_eq!(json["advice"]["sizing"]["source"], "rate_tier");
    assert_eq!(json["advice"]["sizing"]["plan"], "max_5x");
}

#[test]
fn advisor_converts_a_eur_seat_fee_to_usd() {
    let seed = seed_daily_polls(70.0);
    seed_account(&seed, Some(Plan::Team), Some("default_claude_max_5x"));
    let rate = FxPolicy::fixed(EurPerUsd::from_scaled(920_000_000).unwrap());
    let ctx = ctx("UTC", Currency::Usd, rate, "2026-03-21T00:00:00Z").with_subscriptions(vec![
        team_subscription(Money::from_micros(Currency::Eur, 92_000_000)),
    ]);
    let tokenmtr_core::Advice::Recommend(r) = advise_default(&seed, &ctx).advice else {
        panic!("expected a recommendation");
    };
    assert_eq!(r.current_monthly_fee, Some(UsdMicros::new(100_000_000)));
}

#[test]
fn advisor_names_why_it_cannot_advise() {
    let seed = seed_daily_polls(70.0);
    let ctx = usd_ctx("UTC", "2026-03-21T00:00:00Z");
    assert_eq!(
        advise_default(&seed, &ctx).advice,
        tokenmtr_core::Advice::NoPlan
    );
    // A Team seat whose tier names no plan: its quota size is unknown.
    seed_account(&seed, Some(Plan::Team), Some("default_claude_ai"));
    let report = advise_default(&seed, &ctx);
    assert_eq!(report.current_plan, Some(Plan::Team));
    assert_eq!(report.advice, tokenmtr_core::Advice::UnknownQuota);
    assert_eq!(json(&report)["advice"]["advice"], "unknown_quota");
}

fn recap_config(sinks: Vec<SinkSpec>, budgets: Vec<BudgetSpec>) -> RecapConfig {
    RecapConfig {
        budgets,
        sinks,
        metrics: Vec::new(),
        sources: Vec::new(),
        live_window: SignedDuration::from_mins(5),
        daemon_stale_after: SignedDuration::from_secs(60),
    }
}

#[test]
fn recap_and_status_documents() {
    let seed = Seed::new();
    let now = ts("2026-02-02T12:00:00Z");
    let mut live = reported(
        seed.ev(
            "live",
            now.checked_sub(SignedDuration::from_secs(60)).unwrap(),
        ),
        1_200_000,
    );
    live.tokens.output = 600;
    let yesterday = reported(seed.ev("y", ts("2026-02-01T09:00:00Z")), 500_000);
    seed.put(&[live, yesterday]);
    seed.store().set_heartbeat().unwrap();
    let reader = seed.reader();
    let c = usd_ctx("UTC", "2026-02-02T12:00:00Z");
    let budget = BudgetSpec {
        budget: Budget::new(
            BudgetId::try_from("daily".to_owned()).unwrap(),
            BudgetPeriod::Day,
            Money::from_micros(Currency::Usd, 4_800_000),
            BudgetMeasure::ApiValue,
        )
        .unwrap(),
        scope: BudgetScope::default(),
    };
    let cfg = recap_config(Vec::new(), vec![budget]);
    let cfg = RecapConfig {
        daemon_stale_after: SignedDuration::from_secs(i64::MAX / 4),
        ..cfg
    };

    let r = recap(
        &reader,
        &c,
        &cfg,
        Selection::Agent(AgentId::CLAUDE),
        Period::Today,
    )
    .unwrap();
    let v = json(&r);
    assert_eq!(v["status_schema"], 1);
    assert_eq!(v["agent"], "claude");
    assert_eq!(v["period"], "today");
    assert_eq!(v["currency"], "USD");
    assert_eq!(v["totals"]["api_value_micros"], 1_200_000);
    assert_eq!(v["system"]["level"], "ok");
    assert_eq!(v["top_model"], "m1");
    // 600 tokens over a five-minute window; $1.2 per five minutes.
    assert_eq!(r.recap.live.unwrap().tokens_per_minute, 120);
    assert_eq!(
        r.recap.live.unwrap().api_value_per_hour_usd_micros,
        14_400_000
    );
    assert_eq!(r.recap.sparkline.len(), 7);
    assert_eq!(r.recap.sparkline[5].api_value_micros, 500_000);
    assert_eq!(r.recap.sparkline[6].api_value_micros, 1_200_000);
    assert_eq!(r.recap.budgets.len(), 1);
    assert_eq!(r.recap.budgets[0].percent, 25);
    assert_eq!(r.recap.budgets[0].projected_micros, 1_200_000);
    assert!(r.recap.last_activity.is_some());

    let week = recap(&reader, &c, &cfg, Selection::All, Period::Month).unwrap();
    assert_eq!(week.recap.agent, None);
    assert_eq!(week.recap.totals.usage.api_value_micros, 1_700_000);

    let st = status(
        &reader,
        &c,
        &cfg,
        &[AgentId::CLAUDE, AgentId::PI],
        Period::Month,
    )
    .unwrap();
    let v = json(&st);
    assert_eq!(v["status_schema"], 1);
    assert_eq!(v["agents"].as_array().unwrap().len(), 2);
    assert_eq!(v["agents"][1]["agent"], "pi");
    assert_eq!(v["agents"][1]["totals"]["events"], 0);
    assert!(v["agents"][1]["live"].is_null());
}

#[test]
fn month_budget_projects_from_the_recent_daily_burn() {
    let seed = Seed::new();
    // $1 a day on 02-01..02-07, today is 02-08 with $0.5 so far, month has 28 days.
    let mut events = Vec::new();
    for day in 1..=7 {
        events.push(reported(
            seed.ev(
                &format!("d{day}"),
                ts(&format!("2026-02-{day:02}T09:00:00Z")),
            ),
            1_000_000,
        ));
    }
    events.push(reported(
        seed.ev("today", ts("2026-02-08T09:00:00Z")),
        500_000,
    ));
    seed.put(&events);
    let spec = BudgetSpec {
        budget: Budget::new(
            BudgetId::try_from("month".to_owned()).unwrap(),
            BudgetPeriod::Month,
            Money::from_micros(Currency::Usd, 40_000_000),
            BudgetMeasure::ApiValue,
        )
        .unwrap(),
        scope: BudgetScope::default(),
    };
    let views = budgets(
        &seed.reader(),
        &usd_ctx("UTC", "2026-02-08T12:00:00Z"),
        &[spec],
    )
    .unwrap();
    let b = &views[0];
    assert_eq!(b.spent_micros, 7_500_000);
    assert_eq!(b.percent, 18);
    // 7.5 + 20 remaining days × $1.
    assert_eq!(b.projected_micros, 27_500_000);
    assert_eq!(
        (b.period_start, b.period_end),
        (date(2026, 2, 1), date(2026, 3, 1))
    );
}

#[test]
fn an_import_fails_from_the_start_of_the_trailing_streak_of_failed_passes() {
    let seed = Seed::new();
    let spec = |name: &str, kind, enabled| SourceSpec {
        id: SourceId::try_from(name.to_owned()).unwrap(),
        kind,
        enabled,
    };
    let run = |source: &str, path: Option<&str>, at: &str, error: Option<&str>| ImportRun {
        source: SourceId::try_from(source.to_owned()).unwrap(),
        path: path.map(str::to_owned),
        dataset: None,
        started: ts(at),
        finished: ts(at),
        rows: 3,
        error: error.map(str::to_owned),
    };
    for r in [
        run("laptop", None, "2026-02-02T08:00:00Z", None),
        run(
            "laptop",
            None,
            "2026-02-02T09:00:00Z",
            Some("1 of the files failed"),
        ),
        // A file that failed inside a pass that went on does not make the source fail.
        run(
            "laptop",
            Some("/a.db"),
            "2026-02-02T09:30:00Z",
            Some("busy"),
        ),
        run(
            "laptop",
            None,
            "2026-02-02T10:00:00Z",
            Some("1 of the files failed"),
        ),
        run("desk", Some("/b.db"), "2026-02-02T09:00:00Z", Some("busy")),
        run("desk", None, "2026-02-02T09:00:00Z", None),
    ] {
        seed.store().record_import_run(&r).unwrap();
    }
    let specs = [
        spec("laptop", SourceKind::Sqlite, true),
        spec("desk", SourceKind::Sqlite, true),
        spec("never", SourceKind::Sqlite, true),
        spec("off", SourceKind::Sqlite, false),
        spec("superset", SourceKind::Superset, true),
    ];
    let health = source_health(&seed.reader(), &specs).unwrap();
    assert_eq!(
        health[0].state,
        SourceState::Failing {
            since: ts("2026-02-02T09:00:00Z"),
            last_error: "1 of the files failed".to_owned()
        }
    );
    assert_eq!(health[0].last_success_at, Some(ts("2026-02-02T08:00:00Z")));
    assert_eq!(health[0].recent_runs.len(), 4);
    assert_eq!(health[1].state, SourceState::Healthy);
    assert_eq!(health[2].state, SourceState::Healthy);
    assert!(health[2].recent_runs.is_empty());
    assert_eq!(health[3].state, SourceState::Disabled);
    assert_eq!(health[4].state, SourceState::Healthy);
    assert_eq!(
        json(&health[0])["state"],
        "failing",
        "the state is tagged and flattened"
    );
    assert_eq!(json(&health[0])["source"], "laptop");
}

#[test]
fn a_superset_source_fails_while_its_drift_issue_is_open() {
    let seed = Seed::new();
    seed.store()
        .record_parse_issue(&tokenmtr_store::ParseIssue {
            agent: AgentId::CLAUDE,
            kind: tokenmtr_store::IssueKind::IntegrationDrift,
            subject: "work".to_owned(),
            count: std::num::NonZeroU64::MIN,
            sample: Some("worktree: no such column".to_owned()),
            at: ts("2026-02-02T08:00:00Z"),
        })
        .unwrap();
    let spec = |name: &str| SourceSpec {
        id: SourceId::try_from(name.to_owned()).unwrap(),
        kind: SourceKind::Superset,
        enabled: true,
    };
    let health = source_health(&seed.reader(), &[spec("work"), spec("superset")]).unwrap();
    assert_eq!(
        health[0].state,
        SourceState::Failing {
            since: ts("2026-02-02T08:00:00Z"),
            last_error: "worktree: no such column".to_owned()
        }
    );
    assert_eq!(health[1].state, SourceState::Healthy);
}

#[test]
fn metrics_health_is_failing_from_the_start_of_the_trailing_streak() {
    let seed = Seed::new();
    let spec = |name: &str, kind: MetricsKind| MetricsSpec {
        id: SinkId::try_from(name.to_owned()).unwrap(),
        kind,
    };
    let run = |output: &str, at: &str, error: Option<&str>| MetricsRun {
        output: SinkId::try_from(output.to_owned()).unwrap(),
        started: ts(at),
        finished: ts(at),
        series: 7,
        error: error.map(str::to_owned),
    };
    for r in [
        run("push", "2026-02-02T08:00:00Z", None),
        run("push", "2026-02-02T09:00:00Z", Some("http status 503")),
        run("push", "2026-02-02T10:00:00Z", Some("http status 429")),
        run("scrape", "2026-02-02T08:00:00Z", Some("address in use")),
        run("scrape", "2026-02-02T09:00:00Z", None),
    ] {
        seed.store().record_metrics_run(&r).unwrap();
    }
    let specs = [
        spec("push", MetricsKind::RemoteWrite),
        spec("scrape", MetricsKind::Exporter),
        spec("never", MetricsKind::Exporter),
    ];
    let health = metrics_health(&seed.reader(), &specs).unwrap();
    assert_eq!(
        health[0].state,
        MetricsState::Failing {
            since: ts("2026-02-02T09:00:00Z"),
            last_error: "http status 429".to_owned()
        }
    );
    assert_eq!(health[0].last_success_at, Some(ts("2026-02-02T08:00:00Z")));
    assert_eq!(health[0].recent_runs.len(), 3);
    assert_eq!(health[1].state, MetricsState::Healthy);
    assert_eq!(health[2].state, MetricsState::Healthy);
    assert!(health[2].recent_runs.is_empty());
    assert_eq!(
        json(&health[0]),
        serde_json::json!({
            "output": "push",
            "kind": "remote_write",
            "state": "failing",
            "since": "2026-02-02T09:00:00Z",
            "last_error": "http status 429",
            "last_success_at": "2026-02-02T08:00:00Z",
            "recent_runs": json(&health[0])["recent_runs"],
        })
    );
}

#[test]
fn sink_health_daemon_alerts_and_ingest_state() {
    let seed = Seed::new();
    seed.put(&[seed.ev("a", ts("2026-02-02T10:00:00Z"))]);
    let reader = seed.reader();
    let now = "2026-02-02T12:00:00Z";
    let c = usd_ctx("UTC", now);
    let sink = |name: &str, enabled: bool| SinkSpec {
        id: SinkId::try_from(name.to_owned()).unwrap(),
        interval: SignedDuration::from_hours(1),
        datasets: vec![Dataset::UsageEvents],
        enabled,
    };

    // Rows waiting since the store clock (2026-01-01), far beyond 2 × 1h.
    let health = sink_health(&reader, &c, &[sink("lag", true), sink("off", false)]).unwrap();
    assert_eq!(health[0].state, SinkState::Lagging);
    assert_eq!(health[0].lag_rows, 1);
    assert_eq!(health[1].state, SinkState::Disabled);

    // Caught up: healthy.
    let head = seed.store().head_revision(Dataset::UsageEvents).unwrap();
    seed.store()
        .set_sink_cursor(
            &SinkId::try_from("ok".to_owned()).unwrap(),
            Dataset::UsageEvents,
            head,
        )
        .unwrap();
    let health = sink_health(&seed.reader(), &c, &[sink("ok", true)]).unwrap();
    assert_eq!(health[0].state, SinkState::Healthy);
    assert_eq!(health[0].lag_rows, 0);

    // Two failed runs after a success: failing since the first failure.
    let run = |at: &str, error: Option<&str>| SinkRun {
        sink: SinkId::try_from("bad".to_owned()).unwrap(),
        dataset: Dataset::UsageEvents,
        started: ts(at),
        finished: ts(at),
        rows: 1,
        error: error.map(str::to_owned),
    };
    for r in [
        run("2026-02-02T08:00:00Z", None),
        run("2026-02-02T09:00:00Z", Some("boom")),
        run("2026-02-02T10:00:00Z", Some("still boom")),
    ] {
        seed.store().record_sink_run(&r).unwrap();
    }
    let health = sink_health(&seed.reader(), &c, &[sink("bad", true)]).unwrap();
    assert_eq!(
        health[0].state,
        SinkState::Failing {
            since: ts("2026-02-02T09:00:00Z"),
            last_error: "still boom".to_owned()
        }
    );
    assert_eq!(health[0].last_success_at, Some(ts("2026-02-02T08:00:00Z")));
    assert_eq!(json(&health[0])["state"], "failing");

    // Daemon: never started, then stale.
    assert_eq!(
        daemon_health(&reader, &c, SignedDuration::from_mins(5))
            .unwrap()
            .state,
        DaemonState::NeverStarted
    );
    seed.store().set_heartbeat().unwrap();
    assert_eq!(
        daemon_health(&seed.reader(), &c, SignedDuration::from_mins(5))
            .unwrap()
            .state,
        DaemonState::Stale
    );

    // Alerts and ingest runs.
    seed.store()
        .save_alert(&Alert {
            kind: AlertKind::BackupFailed,
            dedup_key: "backup:1".to_owned(),
            at: ts("2026-02-02T11:00:00Z"),
        })
        .unwrap();
    let fired = alerts(&seed.reader(), ts("2026-02-01T00:00:00Z"), 10).unwrap();
    assert_eq!(fired.len(), 1);
    assert_eq!(json(&fired[0])["kind"], "backup_failed");
    assert!(
        alerts(&seed.reader(), ts("2026-02-03T00:00:00Z"), 10)
            .unwrap()
            .is_empty()
    );
    seed.store()
        .record_ingest_run(&IngestRun {
            agent: AgentId::CLAUDE,
            started: ts("2026-02-02T11:00:00Z"),
            finished: ts("2026-02-02T11:00:05Z"),
            files_parsed: 2,
            bytes_parsed: 10,
            lines_read: 20,
            events_found: 7,
            events_changed: 3,
            skipped: std::collections::BTreeMap::new(),
        })
        .unwrap();
    let ingest = ingest_status(&seed.reader()).unwrap();
    assert_eq!((ingest[0].events_found, ingest[0].events_changed), (7, 3));
    assert!(parse_issues(&seed.reader(), false, 10).unwrap().is_empty());
}

#[test]
fn ranges_and_buckets() {
    let c = usd_ctx("UTC", "2026-04-15T12:00:00Z"); // a Wednesday
    assert_eq!(
        RangeSpec::ThisWeek.resolve(&c).unwrap(),
        DateRange::new(date(2026, 4, 13), date(2026, 4, 20)).unwrap()
    );
    assert_eq!(
        RangeSpec::ThisMonth.resolve(&c).unwrap(),
        DateRange::new(date(2026, 4, 1), date(2026, 5, 1)).unwrap()
    );
    assert_eq!(
        RangeSpec::LastDays { days: 7 }.resolve(&c).unwrap(),
        DateRange::new(date(2026, 4, 9), date(2026, 4, 16)).unwrap()
    );
    let year = DateRange::new(date(2026, 1, 1), date(2027, 1, 1)).unwrap();
    assert_eq!(Bucket::pick(&year, 400), Bucket::Day);
    assert_eq!(Bucket::pick(&year, 100), Bucket::Week);
    assert_eq!(Bucket::pick(&year, 20), Bucket::Month);
    assert_eq!(year.days(), 365);
}

#[test]
fn stored_subscriptions_and_rates_load_into_the_context() {
    let seed = Seed::new();
    seed_fx(&seed);
    seed.store()
        .mirror_subscriptions(&[anthropic_subscription()])
        .unwrap();
    let reader = seed.reader();
    assert_eq!(load_subscriptions(&reader).unwrap().len(), 1);
    assert_eq!(load_fx_rates(&reader).unwrap().len(), 2);
    let rates = fx_rates(
        &reader,
        DateRange::new(date(2026, 3, 1), date(2026, 3, 11)).unwrap(),
    )
    .unwrap();
    assert_eq!(rates.len(), 2);
    assert_eq!(rates[0].eur_per_usd_scaled, 900_000_000);
}

/// The query plan of `sql`, one line per step; every parameter is `claude`.
fn plan(reader: &StoreReader, sql: &str) -> String {
    let mut statement = reader
        .connection()
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap();
    let params = std::iter::repeat_n("claude", statement.parameter_count());
    let steps: Vec<String> = statement
        .query_map(rusqlite::params_from_iter(params), |row| row.get(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    steps.join("\n")
}

#[test]
fn per_agent_lookups_seek_their_index_instead_of_scanning() {
    let seed = Seed::new();
    seed.put(&[seed.ev("a", ts("2026-03-01T10:00:00Z"))]);
    let reader = seed.reader();
    for (sql, index) in [
        (recap::LAST_ACTIVITY, "usage_event_at"),
        (
            recap::LAST_ACTIVITY_OF_AGENT,
            "usage_event_agent_profile_at",
        ),
        (quota::LATEST_POLLS, "quota_snapshot_profile_fetched"),
        (health::LATEST_INGEST_RUN, "ingest_run_agent_started"),
    ] {
        let plan = plan(&reader, sql);
        assert!(plan.contains(index), "{sql}\n{plan}");
        assert!(!plan.contains("SCAN usage_event"), "{sql}\n{plan}");
        assert!(!plan.contains("SCAN quota_snapshot"), "{sql}\n{plan}");
        assert!(!plan.contains("SCAN ingest_run"), "{sql}\n{plan}");
    }
}

#[test]
fn session_chart_lookups_seek_the_session_indexes() {
    let seed = Seed::new();
    seed.put(&[seed.ev("a", ts("2026-03-01T10:00:00Z"))]);
    let reader = seed.reader();
    for (sql, index) in [
        (
            session_analytics::TIMELINE_EVENTS.to_owned(),
            "usage_event_session",
        ),
        (
            session_analytics::TIMELINE_TOOLS.to_owned(),
            "tool_call_session",
        ),
        (
            session_analytics::TIMELINE_SIGNALS.to_owned(),
            "sqlite_autoindex_session_signal",
        ),
        (session_analytics::tool_counts_sql(2), "tool_call_session"),
    ] {
        let plan = plan(&reader, &sql);
        assert!(plan.contains(index), "{sql}\n{plan}");
        assert!(!plan.contains("SCAN usage_event"), "{sql}\n{plan}");
        assert!(!plan.contains("SCAN t"), "{sql}\n{plan}");
        assert!(!plan.contains("SCAN tool_call"), "{sql}\n{plan}");
    }
}

/// $31 a month from March 1, 2026: $1 per March day.
fn march_subscription() -> Subscription {
    Subscription::new(SubscriptionSpec {
        id: SubscriptionId::try_from("claude-max".to_owned()).unwrap(),
        provider: ProviderId::try_from("anthropic".to_owned()).unwrap(),
        plan: Plan::Pro,
        agents: vec![AgentId::CLAUDE],
        profiles: None,
        fee: Money::from_micros(Currency::Usd, 31_000_000),
        period: BillingPeriod::Monthly,
        start: date(2026, 3, 1),
        end: None,
    })
    .unwrap()
}

#[test]
fn billing_series_buckets_fees_and_extra_usage_across_the_paris_dst_day() {
    let seed = Seed::new();
    // March 29, 2026 is 23 hours long in Paris.
    seed.put(&[
        provider(
            reported(
                seed.ev("sub", local("Europe/Paris", "2026-03-29T10:00:00")),
                2_000_000,
            ),
            "anthropic",
        ),
        provider(
            reported(
                seed.ev("api", local("Europe/Paris", "2026-03-29T15:00:00")),
                500_000,
            ),
            "openai",
        ),
    ]);
    poll(
        &seed,
        "default",
        ts("2026-03-30T09:00:00Z"),
        extra(5_000_000),
    );
    let reader = seed.reader();
    let c = usd_ctx("Europe/Paris", "2026-04-02T12:00:00Z")
        .with_subscriptions(vec![march_subscription()]);
    let march = days(date(2026, 3, 1), date(2026, 4, 1));

    let daily = billing_series(&reader, &c, &march, Granularity::Daily, true).unwrap();
    assert_eq!(daily.points.len(), 31);
    assert!(
        daily
            .points
            .iter()
            .all(|p| p.amounts.subscription_fee_micros == 1_000_000)
    );
    let fees: i64 = daily
        .points
        .iter()
        .map(|p| p.amounts.subscription_fee_micros)
        .sum();
    assert_eq!(fees, 31_000_000, "the days sum to the monthly fee");
    let dst = &daily.points[28];
    assert_eq!(dst.local, date(2026, 3, 29).at(0, 0, 0, 0));
    assert_eq!(dst.minutes, 23 * 60);
    assert_eq!(
        dst.amounts,
        BillingAmounts {
            api_value_micros: 2_500_000,
            paid_micros: 1_500_000,
            subscription_fee_micros: 1_000_000,
            extra_usage_micros: 0,
            pay_as_you_go_micros: 500_000,
        }
    );
    assert_eq!(dst.savings_micros, 1_000_000);
    assert!((dst.roi.unwrap() - 2.5 / 1.5).abs() < 1e-9);
    assert_eq!(dst.agents["claude"], dst.amounts);
    // Extra usage is paid, never API value.
    let extra_day = &daily.points[29];
    assert_eq!(extra_day.amounts.extra_usage_micros, 5_000_000);
    assert_eq!(extra_day.amounts.api_value_micros, 0);
    assert_eq!(extra_day.amounts.paid_micros, 6_000_000);
    assert_eq!(extra_day.savings_micros, -6_000_000);
    assert!((extra_day.roi.unwrap()).abs() < 1e-9);
    // Cumulative sums never decrease and end at the totals.
    assert!(daily.points.windows(2).all(|w| {
        w[0].cumulative_paid_micros <= w[1].cumulative_paid_micros
            && w[0].cumulative_api_value_micros <= w[1].cumulative_api_value_micros
    }));
    let last = daily.points.last().unwrap();
    assert_eq!(
        last.cumulative_paid_micros,
        31_000_000 + 500_000 + 5_000_000
    );
    assert_eq!(last.cumulative_api_value_micros, 2_500_000);

    let monthly = billing_series(&reader, &c, &march, Granularity::Monthly, false).unwrap();
    assert_eq!(monthly.points.len(), 1);
    assert_eq!(monthly.points[0].minutes, 31 * 24 * 60 - 60);
    assert_eq!(monthly.points[0].amounts.paid_micros, 36_500_000);
    assert!(monthly.points[0].agents.is_empty());

    let day = days(date(2026, 3, 29), date(2026, 3, 30));
    let hourly = billing_series(&reader, &c, &day, Granularity::Hourly, true).unwrap();
    assert_eq!(hourly.points.len(), 23);
    assert!(hourly.points.iter().all(|p| p.minutes == 60));
    assert!(
        hourly.points.iter().all(|p| p.local.hour() != 2),
        "02:00 does not exist"
    );
    let fees: i64 = hourly
        .points
        .iter()
        .map(|p| p.amounts.subscription_fee_micros)
        .sum();
    assert_eq!(fees, 1_000_000, "the hours sum to the daily fee");
    assert!(
        hourly
            .points
            .iter()
            .all(|p| (43_478..=43_479).contains(&p.amounts.subscription_fee_micros))
    );
    let ten = hourly.points.iter().find(|p| p.local.hour() == 10).unwrap();
    assert_eq!(ten.amounts.api_value_micros, 2_000_000);
    assert_eq!(ten.amounts.pay_as_you_go_micros, 0);
    let fifteen = hourly.points.iter().find(|p| p.local.hour() == 15).unwrap();
    assert_eq!(fifteen.amounts.pay_as_you_go_micros, 500_000);
    assert_eq!(
        fifteen.agents["claude"].paid_micros,
        fifteen.amounts.paid_micros
    );
    assert_eq!(
        hourly.points.last().unwrap().cumulative_paid_micros,
        1_500_000
    );
    assert_eq!(json(&hourly)["granularity"], "hourly");
    assert_eq!(json(&hourly.points[0])["start"], "2026-03-28T23:00:00Z");
    assert_eq!(json(&hourly.points[0])["local"], "2026-03-29T00:00:00");

    // Nothing paid: no ROI.
    let before = days(date(2026, 2, 1), date(2026, 2, 2));
    let empty = billing_series(&reader, &c, &before, Granularity::Daily, false).unwrap();
    assert_eq!(empty.points[0].roi, None);
    assert_eq!(empty.points[0].savings_micros, 0);
}

#[test]
fn session_analytics_rows_histogram_and_activity() {
    let seed = Seed::new();
    let session = |e: UsageEvent, id: &str| UsageEvent {
        session: SessionId::try_from(id.to_owned()).unwrap(),
        ..e
    };
    let mut cached = provider(
        reported(
            seed.ev("a1", local("Europe/Paris", "2026-03-02T09:15:00")),
            5_000,
        ),
        "anthropic",
    );
    cached.tokens = TokenCounts {
        input_uncached: 100,
        cache_read: 600,
        cache_write_5m: 200,
        cache_write_1h: 100,
        output: 50,
        reasoning: 0,
    };
    let a2 = provider(
        reported(
            seed.ev("a2", local("Europe/Paris", "2026-03-02T09:45:00")),
            5_000,
        ),
        "anthropic",
    );
    seed.put(&[
        session(cached, "a"),
        session(a2, "a"),
        session(
            reported(
                seed.ev("b1", local("Europe/Paris", "2026-03-03T23:30:00")),
                10_000,
            ),
            "b",
        ),
        session(
            reported(
                seed.ev("c1", local("Europe/Paris", "2026-03-03T08:00:00")),
                150_000_000,
            ),
            "c",
        ),
    ]);
    let call = |id: &str, name: &str, err: Option<bool>| ProfiledToolCall {
        profile: ProfileId::default_profile(),
        call: ToolCall {
            agent: AgentId::CLAUDE,
            tool_use_id: id.to_owned(),
            event: None,
            session: SessionId::try_from("a".to_owned()).unwrap(),
            at: local("Europe/Paris", "2026-03-02T09:20:00"),
            tool_name: name.to_owned(),
            mcp_server: None,
            is_error: err,
            skill: None,
        },
    };
    seed.store()
        .upsert_tool_calls(&[
            call("t1", "Read", Some(false)),
            call("t2", "Read", Some(true)),
            call("t3", "Bash", None),
        ])
        .unwrap();
    seed.store()
        .record_signals(&[SessionSignal {
            agent: AgentId::CLAUDE,
            session: SessionId::try_from("a".to_owned()).unwrap(),
            kind: SignalKind::Compaction,
            at: local("Europe/Paris", "2026-03-02T09:30:00"),
        }])
        .unwrap();
    let reader = seed.reader();
    let ctx = usd_ctx("Europe/Paris", "2026-03-10T12:00:00Z")
        .with_subscriptions(vec![march_subscription()]);
    let range = days(date(2026, 3, 1), date(2026, 3, 8));

    let charts = session_analytics(&reader, &ctx, &range).unwrap();
    assert_eq!(charts.total, 3);
    let ids: Vec<&str> = charts.rows.iter().map(|r| r.session.as_str()).collect();
    assert_eq!(ids, ["c", "a", "b"], "most expensive first, ties by id");
    let row_a = &charts.rows[1];
    assert_eq!(row_a.duration_secs, 30 * 60);
    assert_eq!(row_a.api_value_micros, 10_000);
    assert_eq!(
        row_a.paid_micros, 0,
        "covered by the subscription, no fee split"
    );
    assert!((row_a.cache_hit_percent.unwrap() - 60.0).abs() < 1e-9);
    assert_eq!(row_a.tokens.cache_write_1h, 100);
    assert_eq!((row_a.tool_calls, row_a.tool_errors), (3, 1));
    let row_b = &charts.rows[2];
    assert_eq!(row_b.cache_hit_percent, None, "no input at all");
    assert_eq!(row_b.paid_micros, 10_000, "no provider: pay-as-you-go");
    assert_eq!((row_b.tool_calls, row_b.tool_errors), (0, 0));

    // Edges: [0, 0.01) holds nothing here, 0.01 exactly opens the next bucket, $150 the last.
    assert_eq!(charts.histogram.len(), COST_EDGES_MICROS.len());
    let count = |min: i64| {
        charts
            .histogram
            .iter()
            .find(|b| b.min_micros == min)
            .unwrap()
    };
    assert_eq!(count(10_000).sessions, 2);
    assert_eq!(count(10_000).max_micros, Some(20_000));
    assert_eq!(count(100_000_000).sessions, 1);
    assert_eq!(count(100_000_000).max_micros, None);
    assert_eq!(charts.histogram.iter().map(|b| b.sessions).sum::<u64>(), 3);

    assert_eq!(charts.days.len(), 7);
    assert_eq!(
        (charts.days[1].day, charts.days[1].sessions),
        (date(2026, 3, 2), 1)
    );
    assert_eq!(charts.days[2].sessions, 2);
    assert_eq!(charts.starts.len(), 168);
    // Tuesday March 3: 08:00 and 23:30 local.
    let at =
        |weekday: u8, hour: u8| charts.starts[usize::from(weekday - 1) * 24 + usize::from(hour)];
    assert_eq!(
        (at(2, 8).sessions, at(2, 23).sessions, at(1, 9).sessions),
        (1, 1, 1)
    );

    let timeline = session_timeline(
        &reader,
        &ctx,
        AgentId::CLAUDE,
        &SessionId::try_from("a".to_owned()).unwrap(),
    )
    .unwrap();
    assert_eq!(timeline.events.len(), 2);
    assert!(!timeline.events_truncated);
    assert_eq!(timeline.events[0].tokens.cache_read, 600);
    assert_eq!(timeline.events[1].cumulative_api_value_micros, 10_000);
    assert_eq!(timeline.events[1].cumulative_paid_micros, 0);
    assert_eq!(timeline.signals.len(), 1);
    assert_eq!(timeline.signals[0].kind, SignalKind::Compaction);
    assert_eq!(
        (
            timeline.tools[0].tool_name.as_str(),
            timeline.tools[0].calls,
            timeline.tools[0].failed
        ),
        ("Read", 2, 1)
    );
    assert_eq!((timeline.tools[1].calls, timeline.tools[1].failed), (1, 0));

    let other = session_timeline(
        &reader,
        &ctx,
        AgentId::CLAUDE,
        &SessionId::try_from("b".to_owned()).unwrap(),
    )
    .unwrap();
    assert_eq!(other.events[0].cumulative_paid_micros, 10_000);
    assert!(other.tools.is_empty());
}

#[test]
fn session_models_split_the_timeline_histogram_tools_and_signals() {
    let seed = Seed::new();
    let sid = |id: &str| SessionId::try_from(id.to_owned()).unwrap();
    let ev = |key: &str, at: &str, micros: i64, name: &str, id: &str| UsageEvent {
        session: sid(id),
        ..model(reported(seed.ev(key, ts(at)), micros), name)
    };
    seed.put(&[
        ev("a1", "2026-03-02T09:00:00Z", 30_000, "m1", "a"),
        ev("a2", "2026-03-02T10:00:00Z", 50_000, "m2", "a"),
        ev("a3", "2026-03-02T11:00:00Z", 10_000, "m1", "a"),
        ev("b1", "2026-03-03T09:00:00Z", 60_000, "m1", "b"),
    ]);
    let call = |id: &str, session: &str, at: &str, err: bool| ProfiledToolCall {
        profile: ProfileId::default_profile(),
        call: ToolCall {
            agent: AgentId::CLAUDE,
            tool_use_id: id.to_owned(),
            event: None,
            session: sid(session),
            at: ts(at),
            tool_name: "Read".to_owned(),
            mcp_server: None,
            is_error: Some(err),
            skill: None,
        },
    };
    seed.store()
        .upsert_tool_calls(&[
            call("t1", "a", "2026-03-02T08:00:00Z", false),
            call("t2", "a", "2026-03-02T09:30:00Z", false),
            call("t3", "a", "2026-03-02T10:00:00Z", true),
            call("t4", "z", "2026-03-02T10:00:00Z", false),
        ])
        .unwrap();
    let signal = |session: &str, kind: SignalKind, at: &str| SessionSignal {
        agent: AgentId::CLAUDE,
        session: sid(session),
        kind,
        at: ts(at),
    };
    seed.store()
        .record_signals(&[
            signal("a", SignalKind::Compaction, "2026-03-02T10:15:00Z"),
            signal("a", SignalKind::ApiError, "2026-03-02T10:20:00Z"),
            signal("z", SignalKind::ApiError, "2026-03-02T10:00:00Z"),
        ])
        .unwrap();
    let reader = seed.reader();
    let ctx = usd_ctx("UTC", "2026-03-10T12:00:00Z");
    let range = days(date(2026, 3, 1), date(2026, 3, 8));

    let timeline = session_timeline(&reader, &ctx, AgentId::CLAUDE, &sid("a")).unwrap();
    let models: Vec<(&str, u64, i64)> = timeline
        .models
        .iter()
        .map(|m| (m.model.as_str(), m.events, m.api_value_micros))
        .collect();
    assert_eq!(models, [("m2", 1, 50_000), ("m1", 2, 40_000)]);

    // Session a: m2 outweighs m1 (0.05 > 0.04), total 0.09; session b: m1, 0.06.
    let charts = session_analytics(&reader, &ctx, &range).unwrap();
    let bucket = charts
        .histogram
        .iter()
        .find(|b| b.min_micros == 50_000)
        .unwrap();
    let split: Vec<(&str, u64)> = bucket
        .models
        .iter()
        .map(|m| (m.model.as_str(), m.sessions))
        .collect();
    assert_eq!(split, [("m1", 1), ("m2", 1)], "ties by name");
    assert!(
        charts
            .histogram
            .iter()
            .all(|b| b.models.iter().map(|m| m.sessions).sum::<u64>() == b.sessions)
    );

    // Before the first event: the first one after; at an event: that event; no events: none.
    let report = tools(&reader, &ctx, &range).unwrap();
    let calls: Vec<(Option<&str>, u64, u64)> = report
        .by_model
        .iter()
        .map(|r| (r.model.as_deref(), r.calls, r.errors))
        .collect();
    assert_eq!(
        calls,
        [(Some("m1"), 2, 0), (None, 1, 0), (Some("m2"), 1, 1)]
    );

    let report = signals(&reader, &ctx, &range).unwrap();
    let split: Vec<(Option<&str>, u64, u64)> = report
        .models
        .iter()
        .map(|m| (m.model.as_deref(), m.api_errors, m.compactions))
        .collect();
    assert_eq!(split, [(Some("m2"), 1, 1), (None, 1, 0)]);
}

#[test]
fn project_models_split_each_project_and_sum_to_the_projects_breakdown() {
    let seed = Seed::new();
    let at = ts("2026-04-10T10:00:00Z");
    let in_project = |mut e: UsageEvent, key: &str| {
        e.project = Some(ProjectKey::try_from(key.to_owned()).unwrap());
        e
    };
    seed.put(&[
        in_project(
            provider(
                model(reported(seed.ev("a", at), 3_000_000), "m1"),
                "anthropic",
            ),
            "p1",
        ),
        in_project(
            provider(
                model(reported(seed.ev("b", at), 1_000_000), "m2"),
                "anthropic",
            ),
            "p1",
        ),
        in_project(
            provider(model(reported(seed.ev("c", at), 2_000_000), "m3"), "openai"),
            "p2",
        ),
        provider(
            model(reported(seed.ev("d", at), 500_000), "m1"),
            "anthropic",
        ),
    ]);
    let reader = seed.reader();
    let c =
        usd_ctx("UTC", "2026-04-10T18:00:00Z").with_subscriptions(vec![anthropic_subscription()]);
    let f = days(date(2026, 4, 10), date(2026, 4, 11));

    let split = project_models(&reader, &c, &f, ProjectGrouping::Project).unwrap();
    assert_eq!(split.currency, Currency::Usd);
    let keys: Vec<Option<&str>> = split.rows.iter().map(|r| r.project.as_deref()).collect();
    assert_eq!(keys, [Some("p1"), Some("p2"), None]);
    let p1: Vec<(&str, i64)> = split.rows[0]
        .models
        .iter()
        .map(|m| (m.model.as_str(), m.usage.api_value_micros))
        .collect();
    assert_eq!(p1, [("m1", 3_000_000), ("m2", 1_000_000)]);
    assert_eq!(
        split.rows[0].name.as_deref(),
        Some("p1"),
        "unmapped: the key"
    );

    let projects = projects(&reader, &c, &f, ProjectGrouping::Project).unwrap();
    for row in &projects.rows {
        let models = &split
            .rows
            .iter()
            .find(|r| r.project == row.project)
            .unwrap()
            .models;
        let api: i64 = models.iter().map(|m| m.usage.api_value_micros).sum();
        let paid: i64 = models.iter().map(|m| m.usage.paid_micros).sum();
        assert_eq!(
            (api, paid),
            (row.usage.api_value_micros, row.usage.paid_micros)
        );
    }
    assert!(split.rows[0].models[0].usage.subscription_fee_micros > 0);
    assert_eq!(json(&split)["rows"][0]["models"][0]["model"], "m1");
}

#[test]
fn projects_group_by_folder_merges_worktrees_and_splits_clones() {
    let seed = Seed::new();
    let at = ts("2026-04-10T10:00:00Z");
    let key = ProjectKey::try_from("github.com/acme/tool").unwrap();
    let remote = |name: &str, url: &str| tokenmtr_store::GitRemote {
        name: name.to_owned(),
        url: url.to_owned(),
        default_branch: (name == "upstream").then(|| "main".to_owned()),
    };
    for (cwd, root) in [("/w/a", "/w/a"), ("/w/a/sub", "/w/a"), ("/w/b", "/w/b")] {
        seed.store()
            .save_project(&tokenmtr_store::ProjectResolution {
                cwd: cwd.into(),
                project: key.clone(),
                name: "acme/tool".to_owned(),
                client: None,
                repo_root: Some(root.into()),
                remote: Some(key.as_str().to_owned()),
                resolved_at: at,
                source: tokenmtr_store::ProjectSource::Remote,
                remotes: vec![
                    remote("origin", "github.com/acme/tool"),
                    remote("upstream", "gitlab.com/up/stream/tool"),
                ],
            })
            .unwrap();
    }
    let event = |id: &str, cwd: Option<&str>, micros: i64| {
        let mut e = model(reported(seed.ev(id, at), micros), "m1");
        e.cwd = cwd.map(Into::into);
        e.project = cwd.is_some().then(|| key.clone());
        e
    };
    seed.put(&[
        event("a", Some("/w/a"), 3_000_000),
        event("b", Some("/w/a/sub"), 1_000_000),
        event("c", Some("/w/b"), 2_000_000),
        event("d", Some("/w/c/"), 500_000),
        event("e", None, 100_000),
    ]);
    let reader = seed.reader();
    let c = usd_ctx("UTC", "2026-04-10T18:00:00Z");
    let f = days(date(2026, 4, 10), date(2026, 4, 11));

    let by_project = projects(&reader, &c, &f, ProjectGrouping::Project).unwrap();
    let rows: Vec<(Option<&str>, Option<&str>)> = by_project
        .rows
        .iter()
        .map(|r| (r.project.as_deref(), r.name.as_deref()))
        .collect();
    assert_eq!(
        rows,
        [
            (Some("github.com/acme/tool"), Some("acme/tool")),
            (None, None)
        ]
    );

    let by_folder = projects(&reader, &c, &f, ProjectGrouping::Folder).unwrap();
    let rows: Vec<(Option<&str>, Option<&str>, i64)> = by_folder
        .rows
        .iter()
        .map(|r| {
            (
                r.project.as_deref(),
                r.name.as_deref(),
                r.usage.api_value_micros,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (Some("/w/a"), Some("a"), 4_000_000),
            (Some("/w/b"), Some("b"), 2_000_000),
            (Some("/w/c/"), Some("c"), 500_000),
            (None, None, 100_000),
        ],
        "worktrees of one root merge, a second clone is its own folder, unmapped cwds stand alone"
    );

    let split = project_models(&reader, &c, &f, ProjectGrouping::Folder).unwrap();
    let keys: Vec<Option<&str>> = split.rows.iter().map(|r| r.project.as_deref()).collect();
    assert_eq!(keys, [Some("/w/a"), Some("/w/b"), Some("/w/c/"), None]);
    assert_eq!(split.rows[0].name.as_deref(), Some("a"));

    let s = series(&reader, &c, &f, Bucket::Day, GroupBy::Folder).unwrap();
    let groups: Vec<&str> = s.points[0].groups.keys().map(String::as_str).collect();
    assert_eq!(groups, ["(none)", "/w/a", "/w/b", "/w/c/"]);
    assert_eq!(json(&s)["group_by"], "folder");

    let remotes = project_remotes(&reader, &key).unwrap();
    let shown: Vec<(&str, &str, &str, Option<&str>, bool)> = remotes
        .iter()
        .map(|r| {
            (
                r.name.as_str(),
                r.host.as_str(),
                r.repository.as_str(),
                r.default_branch.as_deref(),
                r.keys_project,
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("origin", "github.com", "acme/tool", None, true),
            (
                "upstream",
                "gitlab.com",
                "up/stream/tool",
                Some("main"),
                false
            ),
        ],
        "two clones with the same remotes list them once"
    );
    let unknown = ProjectKey::try_from("nowhere").unwrap();
    assert_eq!(project_remotes(&reader, &unknown).unwrap(), []);
}

#[test]
fn activity_calendar_counts_tokens_and_started_sessions_per_local_day() {
    let seed = Seed::new();
    let session = |e: UsageEvent, id: &str| UsageEvent {
        session: SessionId::try_from(id.to_owned()).unwrap(),
        ..e
    };
    let paris = |wall: &str| local("Europe/Paris", wall);
    let mut agg = session(seed.ev("agg", paris("2026-03-04T00:00:00")), "agg");
    agg.kind = EventKind::DailyAggregate;
    agg.tokens.output = 1_000;
    seed.put(&[
        // Session a starts the evening before its second event.
        session(seed.ev("a1", paris("2026-03-02T23:30:00")), "a"),
        session(seed.ev("a2", paris("2026-03-03T00:30:00")), "a"),
        session(seed.ev("b1", paris("2026-03-03T09:00:00")), "b"),
        agg,
        // The Sunday before the calendar starts.
        session(seed.ev("old", paris("2024-03-10T12:00:00")), "old"),
    ]);
    let reader = seed.reader();
    let ctx = usd_ctx("Europe/Paris", "2026-03-10T12:00:00Z");
    // The filtered range does not narrow the calendar.
    let calendar =
        activity_calendar(&reader, &ctx, &days(date(2026, 3, 9), date(2026, 3, 10))).unwrap();

    assert_eq!(
        calendar.days.first().unwrap().day,
        date(2024, 3, 11),
        "a Monday, 104 weeks back"
    );
    assert_eq!(
        calendar.days.last().unwrap().day,
        date(2026, 3, 10),
        "today"
    );
    assert_eq!(calendar.days.len(), 104 * 7 + 2);
    let shown: Vec<(Date, u64, u64)> = calendar
        .days
        .iter()
        .filter(|d| d.total_tokens > 0 || d.sessions > 0)
        .map(|d| (d.day, d.total_tokens, d.sessions))
        .collect();
    assert_eq!(
        shown,
        [
            (date(2026, 3, 2), 1, 1),
            (date(2026, 3, 3), 2, 1),
            (date(2026, 3, 4), 1_000, 0),
        ],
        "a session counts on its first local day; daily aggregates add tokens, not sessions"
    );
}

#[test]
fn activity_calendar_counts_a_session_on_its_first_day_passing_the_billing_filter() {
    let seed = Seed::new();
    let anthropic = ProviderId::try_from("anthropic".to_owned()).unwrap();
    let event = |key: &str, id: &str, at: &str, provider: Option<&ProviderId>| UsageEvent {
        session: SessionId::try_from(id.to_owned()).unwrap(),
        provider: provider.cloned(),
        ..seed.ev(key, ts(at))
    };
    seed.put(&[
        // Session x starts the day before the subscription and continues into it.
        event("x1", "x", "2026-02-28T23:00:00Z", Some(&anthropic)),
        event("x2", "x", "2026-03-01T10:00:00Z", Some(&anthropic)),
        // Session y moves from pay-as-you-go to the subscription provider.
        event("y1", "y", "2026-03-02T10:00:00Z", None),
        event("y2", "y", "2026-03-03T10:00:00Z", Some(&anthropic)),
        // A NUL inside an id must not shift the other columns.
        event("z1", "z\0Not A Profile", "2026-03-04T10:00:00Z", None),
    ]);
    let reader = seed.reader();
    let ctx = usd_ctx("UTC", "2026-03-10T12:00:00Z").with_subscriptions(vec![march_subscription()]);
    let starts = |billing: BillingFilter| -> Vec<Date> {
        let f = days(date(2026, 3, 9), date(2026, 3, 10)).billing(billing);
        activity_calendar(&reader, &ctx, &f)
            .unwrap()
            .days
            .iter()
            .filter(|d| d.sessions > 0)
            .map(|d| d.day)
            .collect()
    };

    assert_eq!(
        starts(BillingFilter::Any),
        [date(2026, 2, 28), date(2026, 3, 2), date(2026, 3, 4)]
    );
    assert_eq!(
        starts(BillingFilter::Api),
        [date(2026, 2, 28), date(2026, 3, 2), date(2026, 3, 4)]
    );
    assert_eq!(
        starts(BillingFilter::Subscription(None)),
        [date(2026, 3, 1), date(2026, 3, 3)],
        "a session counts on its first covered day"
    );
}

/// Imports `events` as another machine's export, under source `laptop`.
fn import(seed: &Seed, events: Vec<UsageEvent>) {
    let origin: MachineId = "0190a6f2-7c1e-7b4a-9d3e-1a2b3c4d5e6f".parse().unwrap();
    let imported: Vec<_> = events
        .into_iter()
        .zip(1..)
        .map(|(mut event, revision)| {
            event.id = EventId::new(&origin, event.agent, &event.dedup_key);
            tokenmtr_store::ImportedEvent {
                event,
                superseded: false,
                origin_revision: tokenmtr_store::Revision::new(revision),
            }
        })
        .collect();
    let source = tokenmtr_core::SourceId::try_from("laptop".to_owned()).unwrap();
    seed.store()
        .import_page(&tokenmtr_store::ImportPage {
            source: &source,
            path: camino::Utf8Path::new("/x/laptop.db"),
            machine: origin,
            events: &imported,
            tool_calls: &[],
        })
        .unwrap();
}

#[test]
fn imported_rows_count_in_totals_and_budgets_but_not_in_the_live_block() {
    let seed = Seed::new();
    let now = ts("2026-02-02T12:00:00Z");
    let recent = now.checked_sub(SignedDuration::from_secs(60)).unwrap();
    let mut local = reported(seed.ev("local", recent), 1_200_000);
    local.tokens.output = 600;
    seed.put(&[local]);
    let mut remote = reported(seed.ev("remote", recent), 2_400_000);
    remote.tokens.output = 1_200;
    remote.session = SessionId::try_from("remote".to_owned()).unwrap();
    remote.project = Some(ProjectKey::try_from("github.com/acme/tool".to_owned()).unwrap());
    let mut folder = reported(seed.ev("folder", recent), 600_000);
    folder.project = Some(ProjectKey::try_from("/home/me/src/widgets".to_owned()).unwrap());
    import(&seed, vec![remote, folder]);
    seed.store().set_heartbeat().unwrap();
    let reader = seed.reader();
    let c = usd_ctx("UTC", "2026-02-02T12:00:00Z");
    let budget = BudgetSpec {
        budget: Budget::new(
            BudgetId::try_from("daily".to_owned()).unwrap(),
            BudgetPeriod::Day,
            Money::from_micros(Currency::Usd, 4_800_000),
            BudgetMeasure::ApiValue,
        )
        .unwrap(),
        scope: BudgetScope::default(),
    };
    let cfg = RecapConfig {
        daemon_stale_after: SignedDuration::from_secs(i64::MAX / 4),
        ..recap_config(Vec::new(), vec![budget])
    };

    let r = recap(&reader, &c, &cfg, Selection::All, Period::Today).unwrap();
    // Totals and the budget count every machine ...
    assert_eq!(r.recap.totals.usage.api_value_micros, 4_200_000);
    assert_eq!(r.recap.budgets[0].percent, 87);
    // ... the live rate only this one: 600 tokens over five minutes.
    let live = r.recap.live.unwrap();
    assert_eq!((live.tokens_per_minute, live.active_sessions), (120, 1));

    // A project only imports know is named the way a local resolution would.
    let range = days(date(2026, 2, 2), date(2026, 2, 3));
    let rows = projects(&reader, &c, &range, ProjectGrouping::Project)
        .unwrap()
        .rows;
    let named: Vec<_> = rows
        .iter()
        .filter_map(|row| row.project.clone().zip(row.name.clone()))
        .collect();
    assert!(named.contains(&("github.com/acme/tool".to_owned(), "acme/tool".to_owned())));
    assert!(named.contains(&("/home/me/src/widgets".to_owned(), "widgets".to_owned())));
}

#[test]
fn an_imported_session_names_its_machine_and_a_local_one_does_not() {
    let seed = Seed::new();
    let at = ts("2026-02-02T11:00:00Z");
    seed.put(&[reported(seed.ev("local", at), 1_000_000)]);
    let mut remote = reported(seed.ev("remote", at), 2_000_000);
    remote.session = SessionId::try_from("remote".to_owned()).unwrap();
    import(&seed, vec![remote]);
    let reader = seed.reader();
    let c = usd_ctx("UTC", "2026-02-03T00:00:00Z");
    let f = Filters::new(DateRange::day(date(2026, 2, 2)).unwrap());

    let page = sessions(&reader, &c, &f, 0, 50).unwrap();
    let machine_of = |session: &str| {
        page.rows
            .iter()
            .find(|row| row.session == session)
            .map(|row| row.machine.clone())
    };
    assert_eq!(
        machine_of("remote"),
        Some(Some("laptop · laptop".to_owned()))
    );
    assert_eq!(machine_of("s1"), Some(None));

    let detail = |session: &str| {
        session_detail(
            &reader,
            &c,
            AgentId::CLAUDE,
            &SessionId::try_from(session.to_owned()).unwrap(),
        )
        .unwrap()
        .machine
    };
    assert_eq!(detail("remote").as_deref(), Some("laptop · laptop"));
    assert_eq!(detail("s1"), None);
}

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
    ProjectKey, ProviderId, QuotaEta, QuotaWindow, SessionId, SignalKind, SinkId, Subscription,
    SubscriptionId, SubscriptionSpec, TokenCounts, UsageEvent, UsdMicros, Utilization, WindowId,
};
use tokenmtr_store::{
    Dataset, FxSource, IngestRun, ProfiledToolCall, QuotaOutcome, QuotaPoll, SessionSignal,
    SinkRun, Store, StoreRead, StoreReader,
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
    assert!(projects(&reader, &ctx, &f).unwrap().rows.is_empty());
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
        projects(&reader, &c, &f)
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

fn recap_config(sinks: Vec<SinkSpec>, budgets: Vec<BudgetSpec>) -> RecapConfig {
    RecapConfig {
        budgets,
        sinks,
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

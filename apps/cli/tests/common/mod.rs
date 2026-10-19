//! A throwaway `HOME` with a seeded store, and the real `tokenmtr` binary pointed at it.
#![allow(
    dead_code,
    clippy::expect_used,
    reason = "each test binary uses a subset; clippy's allow-expect-in-tests covers #[test] functions, not the shared helpers"
)]

use std::error::Error;

use assert_cmd::Command;
use camino::Utf8PathBuf;
use jiff::Timestamp;
use jiff::civil::date;
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{
    AgentId, Alert, AlertKind, BillingPeriod, Currency, DedupKey, EurPerUsd, EventCost, EventId,
    EventKind, ExtraUsage, FxRate, ModelId, Money, Plan, ProfileId, ProjectKey, ProviderId,
    QuotaWindow, SessionId, Subscription, SubscriptionId, SubscriptionSpec, TokenCounts,
    UsageEvent, UsdMicros, Utilization, WindowId,
};
use tokenmtr_platform::{ControlTransport, Paths};
use tokenmtr_store::{FxSource, QuotaOutcome, QuotaPoll, Store, StoreRead as _};

pub type TestResult = Result<(), Box<dyn Error>>;

/// A day in the past that the report tests ask for.
pub const REPORT_MONTH: &str = "2026-03";

pub struct Home {
    _dir: tempfile::TempDir,
    pub root: Utf8PathBuf,
    pub paths: Paths,
}

impl Home {
    /// `/tmp` keeps the control socket path under the 104-byte `sun_path` limit.
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::Builder::new().prefix("tm").tempdir_in("/tmp")?;
        let root = Utf8PathBuf::from_path_buf(dir.path().canonicalize()?)
            .map_err(|p| format!("not utf-8: {}", p.display()))?;
        let home = root.to_string();
        let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.clone()))?;
        Ok(Self {
            _dir: dir,
            root,
            paths,
        })
    }

    pub fn store_path(&self) -> Utf8PathBuf {
        self.paths.data_dir().join("usage.db")
    }

    pub fn config_path(&self) -> Utf8PathBuf {
        self.paths.config_file().to_owned()
    }

    pub fn transport(&self) -> ControlTransport {
        ControlTransport::unix(self.paths.data_dir().join("control.sock"))
    }

    pub fn write_config(&self, text: &str) -> Result<(), Box<dyn Error>> {
        let path = self.config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
        Ok(())
    }

    /// The binary, with an environment that only knows this home.
    pub fn tokenmtr(&self) -> Command {
        let mut command = Command::cargo_bin("tokenmtr").expect("the tokenmtr binary is built");
        command
            .env_clear()
            .env("HOME", self.root.as_str())
            .env("TZ", "UTC");
        command
    }

    /// Opens the writer, which also creates the store.
    pub fn store(&self) -> Result<Store, Box<dyn Error>> {
        Ok(Store::open(&self.store_path())?)
    }
}

fn event(store: &Store, key: &str, at: Timestamp, micros: i64) -> UsageEvent {
    let dedup_key = DedupKey::try_from(key.to_owned()).expect("valid key");
    UsageEvent {
        id: EventId::new(&store.machine_id(), AgentId::CLAUDE, &dedup_key),
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        dedup_key,
        at,
        session: SessionId::try_from("s1".to_owned()).expect("valid session"),
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: None,
        git_branch: None,
        project: Some(ProjectKey::try_from("acme".to_owned()).expect("valid project")),
        client: Some("Acme".to_owned()),
        source_file: None,
        approximate: false,
        provider: Some(ProviderId::try_from("anthropic".to_owned()).expect("valid provider")),
        model: ModelId::try_from("claude-sonnet".to_owned()).expect("valid model"),
        kind: EventKind::Message,
        tokens: TokenCounts {
            output: 1_000,
            ..TokenCounts::default()
        },
        cost: EventCost::new(Some(UsdMicros::new(micros)), None),
        service_tier: None,
    }
}

/// Two priced events (one now, one on 2026-03-10), an FX rate, a quota window with $12.345 of
/// extra usage (stored as today's extra-usage event), an alert and a Claude subscription.
pub fn seed(home: &Home) -> TestResult {
    let mut store = home.store()?;
    let now = Timestamp::now();
    let march = "2026-03-10T12:00:00Z".parse::<Timestamp>()?;
    store.upsert_events(&[
        event(&store, "now", now, 2_500_000),
        event(&store, "march", march, 4_000_000),
    ])?;
    store.upsert_fx_rates(
        FxSource::Ecb,
        &[FxRate {
            date: date(2026, 3, 9),
            eur_per_usd: EurPerUsd::from_scaled(900_000_000)?,
        }],
    )?;
    store.upsert_quota_snapshots(
        &[QuotaPoll {
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            account: Some("me@example.com".to_owned()),
            fetched_at: now,
            outcome: QuotaOutcome::Windows {
                windows: vec![QuotaWindow {
                    id: WindowId::five_hour(),
                    state: QuotaWindowState {
                        utilization: Utilization::new(42.0)?,
                        resets_at: now.checked_add(jiff::SignedDuration::from_hours(2)).ok(),
                    },
                }],
                extra_usage: Some(ExtraUsage {
                    utilization: Utilization::new(24.69)?,
                    spent: UsdMicros::new(12_345_000),
                    limit: UsdMicros::new(50_000_000),
                }),
            },
        }],
        &jiff::tz::TimeZone::UTC,
    )?;
    store.save_alert(&Alert {
        kind: AlertKind::QuotaThreshold {
            window: WindowId::five_hour(),
            percent: 80,
        },
        dedup_key: "quota-80".to_owned(),
        at: now,
    })?;
    store.mirror_subscriptions(&[Subscription::new(SubscriptionSpec {
        id: SubscriptionId::try_from("claude-max".to_owned())?,
        provider: ProviderId::try_from("anthropic".to_owned())?,
        plan: Plan::Pro,
        agents: vec![AgentId::CLAUDE],
        profiles: None,
        fee: Money::from_micros(Currency::Usd, 20_000_000),
        period: BillingPeriod::Monthly,
        start: date(2026, 1, 1),
        end: None,
    })?])?;
    Ok(())
}

/// The config the seeded tests run with: agents follow nothing on disk, everything networked off.
pub const CONFIG: &str = "[menubar]\nagents = [\"claude\"]\n\
    [quota]\nenabled = false\n[fx]\nenabled = false\n[pricing]\nauto_update = false\n\
    [backup]\nenabled = false\n";

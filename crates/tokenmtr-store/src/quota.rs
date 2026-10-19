//! The `quota_snapshot` table: one row per quota window per poll.
//!
//! A poll that yields no windows (signed out, expired token, failure) is stored as one row with
//! an empty `window_id`, no percentage and a non-`ok` status, so exports show the gap.
//!
//! Claude extra usage is the `extra_usage` window with its money. Each poll that reports it
//! also adds the spend increase since the previous poll to that day's
//! [`tokenmtr_core::event::EventKind::ExtraUsage`] usage event, in the same transaction.

use std::sync::LazyLock;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use rusqlite::{Connection, OptionalExtension, Row, named_params, params};
use serde::{Deserialize, Serialize};
use tokenmtr_core::extra_usage::{DailyEventError, daily_dedup_key, daily_event, spend_increase};
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{
    AgentId, DedupKey, EXTRA_USAGE, EventId, ExtraUsage, IdError, MachineId, ProfileId,
    QuotaWindow, UsdMicros, Utilization, WindowId,
};

use crate::codec::{RowExt, enum_text, to_limit, to_us};
use crate::error::{CodecError, StoreError};
use crate::events;
use crate::types::{Dataset, Revision, UpsertStats, Versioned};
use crate::versioned::{self, Merge, UpsertSql};

/// How a poll ended; the `status` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaStatus {
    /// The provider answered with windows.
    Ok,
    /// The agent is not signed in.
    SignedOut,
    /// The credentials expired.
    TokenExpired,
    /// The request failed.
    Failed,
}

impl QuotaStatus {
    /// The export value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::SignedOut => "signed_out",
            Self::TokenExpired => "token_expired",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for QuotaStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one poll found.
#[derive(Debug, Clone, PartialEq)]
pub enum QuotaOutcome {
    /// The windows the provider reported, and the month's extra usage when enabled. Nothing
    /// reported stores nothing.
    Windows {
        /// The quota windows.
        windows: Vec<QuotaWindow>,
        /// Claude's extra usage, stored as the `extra_usage` window with money.
        extra_usage: Option<ExtraUsage>,
    },
    /// The agent is not signed in.
    SignedOut,
    /// The credentials expired.
    TokenExpired,
    /// The request failed.
    Failed,
}

/// One quota poll of one agent profile, as the daemon records it.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaPoll {
    /// The polled agent.
    pub agent: AgentId,
    /// Its profile.
    pub profile: ProfileId,
    /// The account the poll ran as (email), when known.
    pub account: Option<String>,
    /// When the poll happened.
    pub fetched_at: Timestamp,
    /// What it found.
    pub outcome: QuotaOutcome,
}

/// What a stored quota row holds.
#[derive(Debug, Clone, PartialEq)]
pub enum QuotaReading {
    /// A window's state.
    Window(QuotaWindow),
    /// The month's extra usage (the `extra_usage` window with money).
    ExtraUsage(ExtraUsage),
    /// The poll found the agent signed out.
    SignedOut,
    /// The poll found the credentials expired.
    TokenExpired,
    /// The poll failed.
    Failed,
}

/// One stored row of the `quota_snapshots` dataset.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaSample {
    /// The polled agent.
    pub agent: AgentId,
    /// Its profile.
    pub profile: ProfileId,
    /// The account the poll ran as, when known.
    pub account: Option<String>,
    /// When the poll happened.
    pub fetched_at: Timestamp,
    /// The window state or the failure.
    pub reading: QuotaReading,
}

impl QuotaSample {
    /// The `status` column.
    #[must_use]
    pub const fn status(&self) -> QuotaStatus {
        match self.reading {
            QuotaReading::Window(_) | QuotaReading::ExtraUsage(_) => QuotaStatus::Ok,
            QuotaReading::SignedOut => QuotaStatus::SignedOut,
            QuotaReading::TokenExpired => QuotaStatus::TokenExpired,
            QuotaReading::Failed => QuotaStatus::Failed,
        }
    }

    /// The window, when the poll reported one.
    #[must_use]
    pub fn window(&self) -> Option<QuotaWindow> {
        match &self.reading {
            QuotaReading::Window(window) => Some(window.clone()),
            QuotaReading::ExtraUsage(extra) => Some(extra.window()),
            QuotaReading::SignedOut | QuotaReading::TokenExpired | QuotaReading::Failed => None,
        }
    }

    /// The extra usage, when this row is the `extra_usage` window.
    #[must_use]
    pub const fn extra_usage(&self) -> Option<&ExtraUsage> {
        match &self.reading {
            QuotaReading::ExtraUsage(extra) => Some(extra),
            _ => None,
        }
    }

    /// The `window_label` column: a human name for the window id.
    #[must_use]
    pub fn window_label(&self) -> Option<String> {
        self.window().map(|w| match w.id.as_str() {
            WindowId::FIVE_HOUR => "5-hour".to_owned(),
            WindowId::SEVEN_DAY => "7-day".to_owned(),
            "seven_day_sonnet" => "7-day Sonnet".to_owned(),
            "seven_day_opus" => "7-day Opus".to_owned(),
            EXTRA_USAGE => "Extra usage".to_owned(),
            other => other.replace('_', " "),
        })
    }

    /// The `snapshot_id` of the export: stable per machine, agent, profile, window and poll time.
    ///
    /// # Errors
    /// Never in practice: the key is prefixed, so it is never blank.
    pub fn export_id(&self, machine: &MachineId) -> Result<EventId, IdError> {
        let window = self.window();
        let window = window.as_ref().map_or("", |w| w.id.as_str());
        let key = DedupKey::try_from(format!(
            "quota:{}:{window}:{}",
            self.profile,
            to_us(self.fetched_at)
        ))?;
        Ok(EventId::new(machine, self.agent, &key))
    }
}

const VALUES: [(&str, Merge); 6] = [
    ("account", Merge::Replace),
    ("used_percent", Merge::Replace),
    ("resets_at_us", Merge::Replace),
    ("status", Merge::Replace),
    ("spent_usd_micros", Merge::Replace),
    ("limit_usd_micros", Merge::Replace),
];

static UPSERT: LazyLock<UpsertSql> = LazyLock::new(|| {
    let key = ["agent", "profile", "window_id", "fetched_at_us"];
    UpsertSql::new("quota_snapshot", &key, &key, &VALUES)
});

const AFTER: &str = "SELECT revision, agent, profile, window_id, fetched_at_us, account, \
    used_percent, resets_at_us, status, spent_usd_micros, limit_usd_micros \
    FROM quota_snapshot WHERE revision > ?1 ORDER BY revision LIMIT ?2";

/// The spend of the latest `extra_usage` row of a profile polled before `?3`.
const PREVIOUS_SPENT: &str = "SELECT spent_usd_micros FROM quota_snapshot \
    WHERE agent = ?1 AND profile = ?2 AND window_id = 'extra_usage' AND fetched_at_us < ?3 \
    ORDER BY fetched_at_us DESC LIMIT 1";

/// Whether the `extra_usage` row of a poll is already stored.
const EXTRA_STORED: &str = "SELECT 1 FROM quota_snapshot \
    WHERE agent = ?1 AND profile = ?2 AND window_id = 'extra_usage' AND fetched_at_us = ?3";

/// One row to store: a poll and the window it reported, if any.
type Flat<'a> = (&'a QuotaPoll, Option<Window<'a>>);

/// A reported window: a plain quota window, or the extra usage with its money.
#[derive(Clone, Copy)]
enum Window<'a> {
    Quota(&'a QuotaWindow),
    Extra(&'a ExtraUsage),
}

impl Window<'_> {
    fn state(self) -> QuotaWindowState {
        match self {
            Self::Quota(window) => window.state,
            Self::Extra(extra) => extra.window().state,
        }
    }

    const fn money(self) -> Option<(UsdMicros, UsdMicros)> {
        match self {
            Self::Quota(_) => None,
            Self::Extra(extra) => Some((extra.spent, extra.limit)),
        }
    }
}

fn window_id<'a>(row: &Flat<'a>) -> &'a str {
    match row.1 {
        None => "",
        Some(Window::Quota(window)) => window.id.as_str(),
        Some(Window::Extra(_)) => EXTRA_USAGE,
    }
}

fn flatten(polls: &[QuotaPoll]) -> Vec<Flat<'_>> {
    polls
        .iter()
        .flat_map(|poll| -> Vec<Flat<'_>> {
            match &poll.outcome {
                QuotaOutcome::Windows {
                    windows,
                    extra_usage,
                } => windows
                    .iter()
                    .map(|w| (poll, Some(Window::Quota(w))))
                    .chain(extra_usage.iter().map(|e| (poll, Some(Window::Extra(e)))))
                    .collect(),
                QuotaOutcome::SignedOut | QuotaOutcome::TokenExpired | QuotaOutcome::Failed => {
                    vec![(poll, None)]
                }
            }
        })
        .collect()
}

fn status_of(row: &Flat<'_>) -> QuotaStatus {
    match row.0.outcome {
        QuotaOutcome::Windows { .. } => QuotaStatus::Ok,
        QuotaOutcome::SignedOut => QuotaStatus::SignedOut,
        QuotaOutcome::TokenExpired => QuotaStatus::TokenExpired,
        QuotaOutcome::Failed => QuotaStatus::Failed,
    }
}

/// Inserts or updates polls by `(agent, profile, window, fetched_at)`, and adds each new extra
/// usage observation to its day's usage event (the day is local to `zone`).
pub(crate) fn upsert(
    conn: &Connection,
    now: Timestamp,
    machine: &MachineId,
    zone: &TimeZone,
    polls: &[QuotaPoll],
) -> Result<UpsertStats, StoreError> {
    let mut new_extra = Vec::new();
    for poll in polls {
        if let QuotaOutcome::Windows {
            extra_usage: Some(extra),
            ..
        } = &poll.outcome
            && !extra_stored(conn, poll)?
        {
            new_extra.push((poll, extra.spent));
        }
    }
    let stats = upsert_snapshots(conn, now, polls)?;
    new_extra.sort_by_key(|(poll, _)| poll.fetched_at);
    for (poll, spent) in new_extra {
        record_extra_usage(conn, now, machine, zone, poll, spent)?;
    }
    Ok(stats)
}

fn extra_stored(conn: &Connection, poll: &QuotaPoll) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(EXTRA_STORED)?
        .query_row(
            params![
                poll.agent.as_str(),
                poll.profile.as_str(),
                to_us(poll.fetched_at)
            ],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Adds the spend increase since the previous stored observation of the poll's profile to the
/// usage event of the poll's local day. The previous observation may come from the same batch:
/// snapshots are stored first.
fn record_extra_usage(
    conn: &Connection,
    now: Timestamp,
    machine: &MachineId,
    zone: &TimeZone,
    poll: &QuotaPoll,
    spent: UsdMicros,
) -> Result<(), StoreError> {
    let previous: Option<i64> = conn
        .prepare_cached(PREVIOUS_SPENT)?
        .query_row(
            params![
                poll.agent.as_str(),
                poll.profile.as_str(),
                to_us(poll.fetched_at)
            ],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let increase = spend_increase(previous.map(UsdMicros::new), spent);
    if increase.amount.get() == 0 {
        return Ok(());
    }
    let day = poll.fetched_at.to_zoned(zone.clone()).date();
    let key = daily_dedup_key(&poll.profile, day).map_err(CodecError::from)?;
    let (so_far, approximate) = match events::by_key(conn, poll.agent, &key)? {
        Some(stored) => (
            stored.row.cost.reported().unwrap_or_default(),
            stored.row.approximate,
        ),
        None => (UsdMicros::default(), false),
    };
    let event = daily_event(
        machine,
        poll.agent,
        poll.profile.clone(),
        day,
        zone,
        so_far.saturating_add(increase.amount),
        approximate || increase.approximate,
    )
    .map_err(|error| match error {
        DailyEventError::Id(error) => CodecError::Id(error),
        DailyEventError::Time(error) => CodecError::Time(error),
    })?;
    events::upsert(conn, now, &[event])?;
    Ok(())
}

fn upsert_snapshots(
    conn: &Connection,
    now: Timestamp,
    polls: &[QuotaPoll],
) -> Result<UpsertStats, StoreError> {
    let flat = flatten(polls);
    let rows = versioned::last_wins(&flat, |r| {
        (
            r.0.agent,
            r.0.profile.clone(),
            window_id(r).to_owned(),
            to_us(r.0.fetched_at),
        )
    });
    versioned::run(
        conn,
        Dataset::QuotaSnapshots,
        &UPSERT,
        now,
        &rows,
        |upsert, row, revision, now_us| {
            let (poll, window) = **row;
            let state = window.map(Window::state);
            let money = window.and_then(Window::money);
            upsert.bind(named_params! {
                ":agent": poll.agent.as_str(),
                ":profile": poll.profile.as_str(),
                ":window_id": window_id(row),
                ":fetched_at_us": to_us(poll.fetched_at),
                ":revision": revision,
                ":updated_at_us": now_us,
                ":account": poll.account,
                ":used_percent": state.map(|s| s.utilization.percent()),
                ":resets_at_us": state.and_then(|s| s.resets_at).map(to_us),
                ":status": enum_text(&status_of(row))?,
                ":spent_usd_micros": money.map(|(spent, _)| spent.get()),
                ":limit_usd_micros": money.map(|(_, limit)| limit.get()),
            })
        },
    )
}

/// Quota rows with a revision above `after`, oldest revision first.
pub(crate) fn after(
    conn: &Connection,
    after: Revision,
    limit: usize,
) -> Result<Vec<Versioned<QuotaSample>>, StoreError> {
    let mut statement = conn.prepare_cached(AFTER)?;
    statement
        .query_and_then(params![after, to_limit(limit)?], decode)?
        .collect()
}

fn decode(row: &Row<'_>) -> Result<Versioned<QuotaSample>, StoreError> {
    let status: QuotaStatus = row.enumeration("status")?;
    let reading = match status {
        QuotaStatus::Ok => {
            let percent: Option<f64> = row.get("used_percent")?;
            let percent = percent.ok_or_else(|| CodecError::Value {
                column: "used_percent",
                value: "NULL".to_owned(),
            })?;
            let utilization = Utilization::new(percent).map_err(|_| CodecError::Value {
                column: "used_percent",
                value: percent.to_string(),
            })?;
            let spent: Option<i64> = row.get("spent_usd_micros")?;
            let limit: Option<i64> = row.get("limit_usd_micros")?;
            match (spent, limit) {
                (Some(spent), Some(limit)) => QuotaReading::ExtraUsage(ExtraUsage {
                    utilization,
                    spent: UsdMicros::new(spent),
                    limit: UsdMicros::new(limit),
                }),
                _ => QuotaReading::Window(QuotaWindow {
                    id: row.id::<WindowIdText>("window_id")?.0,
                    state: QuotaWindowState {
                        utilization,
                        resets_at: row.ts_opt("resets_at_us")?,
                    },
                }),
            }
        }
        QuotaStatus::SignedOut => QuotaReading::SignedOut,
        QuotaStatus::TokenExpired => QuotaReading::TokenExpired,
        QuotaStatus::Failed => QuotaReading::Failed,
    };
    Ok(Versioned {
        revision: row.get("revision")?,
        row: QuotaSample {
            agent: row.id::<AgentId>("agent")?,
            profile: row.id::<ProfileId>("profile")?,
            account: row.get("account")?,
            fetched_at: row.ts("fetched_at_us")?,
            reading,
        },
    })
}

/// `WindowId` has no `FromStr`; this adapts its constructor to [`RowExt::id`].
struct WindowIdText(WindowId);

impl std::str::FromStr for WindowIdText {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, IdError> {
        WindowId::new(text).map(Self)
    }
}

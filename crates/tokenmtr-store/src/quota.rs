//! The `quota_snapshot` table: one row per quota window per poll (D26).
//!
//! A poll that yields no windows (signed out, expired token, failure) is stored as one row with
//! an empty `window_id`, no percentage and a non-`ok` status, so exports show the gap.

use std::sync::LazyLock;

use jiff::Timestamp;
use rusqlite::{Connection, Row, named_params, params};
use serde::{Deserialize, Serialize};
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{
    AgentId, DedupKey, EventId, IdError, MachineId, ProfileId, QuotaWindow, Utilization, WindowId,
};

use crate::codec::{RowExt, enum_text, to_limit, to_us};
use crate::error::{CodecError, StoreError};
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
    /// The windows the provider reported. An empty list stores nothing.
    Windows(Vec<QuotaWindow>),
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
            QuotaReading::Window(_) => QuotaStatus::Ok,
            QuotaReading::SignedOut => QuotaStatus::SignedOut,
            QuotaReading::TokenExpired => QuotaStatus::TokenExpired,
            QuotaReading::Failed => QuotaStatus::Failed,
        }
    }

    /// The window, when the poll reported one.
    #[must_use]
    pub const fn window(&self) -> Option<&QuotaWindow> {
        match &self.reading {
            QuotaReading::Window(window) => Some(window),
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
            other => other.replace('_', " "),
        })
    }

    /// The `snapshot_id` of the export: stable per machine, agent, profile, window and poll time.
    ///
    /// # Errors
    /// Never in practice: the key is prefixed, so it is never blank.
    pub fn export_id(&self, machine: &MachineId) -> Result<EventId, IdError> {
        let window = self.window().map_or("", |w| w.id.as_str());
        let key = DedupKey::try_from(format!(
            "quota:{}:{window}:{}",
            self.profile,
            to_us(self.fetched_at)
        ))?;
        Ok(EventId::new(machine, self.agent, &key))
    }
}

const VALUES: [(&str, Merge); 4] = [
    ("account", Merge::Replace),
    ("used_percent", Merge::Replace),
    ("resets_at_us", Merge::Replace),
    ("status", Merge::Replace),
];

static UPSERT: LazyLock<UpsertSql> = LazyLock::new(|| {
    let key = ["agent", "profile", "window_id", "fetched_at_us"];
    UpsertSql::new("quota_snapshot", &key, &key, &VALUES)
});

const AFTER: &str = "SELECT revision, agent, profile, window_id, fetched_at_us, account, \
    used_percent, resets_at_us, status \
    FROM quota_snapshot WHERE revision > ?1 ORDER BY revision LIMIT ?2";

/// One row to store: a poll and the window it reported, if any.
type Flat<'a> = (&'a QuotaPoll, Option<&'a QuotaWindow>);

fn window_id<'a>(row: &Flat<'a>) -> &'a str {
    row.1.map_or("", |w| w.id.as_str())
}

fn flatten(polls: &[QuotaPoll]) -> Vec<Flat<'_>> {
    polls
        .iter()
        .flat_map(|poll| -> Vec<Flat<'_>> {
            match &poll.outcome {
                QuotaOutcome::Windows(windows) => windows.iter().map(|w| (poll, Some(w))).collect(),
                QuotaOutcome::SignedOut | QuotaOutcome::TokenExpired | QuotaOutcome::Failed => {
                    vec![(poll, None)]
                }
            }
        })
        .collect()
}

fn status_of(row: &Flat<'_>) -> QuotaStatus {
    match row.0.outcome {
        QuotaOutcome::Windows(_) => QuotaStatus::Ok,
        QuotaOutcome::SignedOut => QuotaStatus::SignedOut,
        QuotaOutcome::TokenExpired => QuotaStatus::TokenExpired,
        QuotaOutcome::Failed => QuotaStatus::Failed,
    }
}

/// Inserts or updates polls by `(agent, profile, window, fetched_at)`.
pub(crate) fn upsert(
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
            upsert.bind(named_params! {
                ":agent": poll.agent.as_str(),
                ":profile": poll.profile.as_str(),
                ":window_id": window_id(row),
                ":fetched_at_us": to_us(poll.fetched_at),
                ":revision": revision,
                ":updated_at_us": now_us,
                ":account": poll.account,
                ":used_percent": window.map(|w| w.state.utilization.percent()),
                ":resets_at_us": window.and_then(|w| w.state.resets_at).map(to_us),
                ":status": enum_text(&status_of(row))?,
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
            QuotaReading::Window(QuotaWindow {
                id: row.id::<WindowIdText>("window_id")?.0,
                state: QuotaWindowState {
                    utilization,
                    resets_at: row.ts_opt("resets_at_us")?,
                },
            })
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

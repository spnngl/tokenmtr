//! Claude quota polling (D26): `GET /api/oauth/usage` with the user's OAuth token.

use jiff::Timestamp;
use reqwest::{Client, StatusCode, Url};
use secrecy::ExposeSecret;
use serde::Deserialize;
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{QuotaSnapshot, QuotaWindow, Utilization, WindowId};

use crate::account::{AccountInfo, parse_profile_response};
use crate::credentials::ClaudeCredentials;
use crate::error::{NetworkKind, RemoteError};
use crate::http::{join, read_capped};

const USAGE_PATH: &str = "/api/oauth/usage";
const PROFILE_PATH: &str = "/api/oauth/profile";
/// Value of the `anthropic-beta` header the OAuth endpoints require.
const BETA_HEADER: &str = "oauth-2025-04-20";
const MAX_BODY_BYTES: usize = 1 << 20;

/// What one quota poll found: exactly the five cases a caller must tell apart.
#[derive(Debug)]
pub enum QuotaOutcome {
    /// HTTP 200 with a body that parsed.
    Ok(QuotaSnapshot),
    /// HTTP 401: Claude Code must refresh or re-login. This crate never refreshes (D26).
    TokenExpired,
    /// No credentials were found for the profile.
    SignedOut,
    /// HTTP 200 whose body is not the expected JSON, or whose body could not be read in full.
    Malformed,
    /// No usable answer: transport failure, timeout, or any status other than 200 and 401
    /// (429 and 5xx included). Retry on the next interval.
    NetworkDown,
}

/// The result of [`ClaudeQuota::poll`].
#[derive(Debug)]
pub struct QuotaPoll {
    /// The classified outcome.
    pub outcome: QuotaOutcome,
    /// `extra_usage` from the same response; `Some` only with [`QuotaOutcome::Ok`].
    pub extra_usage: Option<ExtraUsage>,
}

impl QuotaPoll {
    fn of(outcome: QuotaOutcome) -> Self {
        Self {
            outcome,
            extra_usage: None,
        }
    }
}

/// The `extra_usage` object (pay-as-you-go credits on top of a plan), as Claude reports it.
/// Amounts are raw numbers: their unit is not documented (believed to be minor currency units,
/// unverified), so D28's "paid cost" use must verify it first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ExtraUsage {
    /// Whether extra usage is switched on for the account.
    #[serde(default)]
    pub is_enabled: bool,
    /// The monthly spending cap, when set.
    pub monthly_limit: Option<f64>,
    /// Credits consumed this month.
    pub used_credits: Option<f64>,
    /// Percent of the monthly cap used.
    pub utilization: Option<Utilization>,
}

/// A successfully parsed usage response.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedQuota {
    /// The windows the response reported (missing and `null` windows are absent).
    pub snapshot: QuotaSnapshot,
    /// The `extra_usage` object, when present and non-null.
    pub extra_usage: Option<ExtraUsage>,
}

/// `/api/oauth/usage` response. Unknown fields are ignored; every window is optional.
#[derive(Debug, Deserialize)]
struct UsageResponse {
    five_hour: Option<RawWindow>,
    seven_day: Option<RawWindow>,
    seven_day_sonnet: Option<RawWindow>,
    seven_day_opus: Option<RawWindow>,
    extra_usage: Option<ExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct RawWindow {
    utilization: Option<Utilization>,
    resets_at: Option<Timestamp>,
}

/// Why a usage body was rejected. Carries the position and category only: serde messages quote
/// the offending value, which must not be echoed (D55).
#[derive(Debug, thiserror::Error)]
#[error("quota response is not the expected JSON ({category:?} at line {line}, column {column})")]
pub struct QuotaParseError {
    category: serde_json::error::Category,
    line: usize,
    column: usize,
}

impl From<serde_json::Error> for QuotaParseError {
    fn from(error: serde_json::Error) -> Self {
        Self {
            category: error.classify(),
            line: error.line(),
            column: error.column(),
        }
    }
}

/// Parses an `/api/oauth/usage` body into windows stamped `fetched_at`. A window that is
/// absent, `null` or has a `null` utilization is skipped.
///
/// # Errors
/// [`QuotaParseError`] when the body is not a JSON object, or a present field has the wrong
/// type (including a negative or non-finite utilization).
pub fn parse_quota_response(
    body: &[u8],
    fetched_at: Timestamp,
) -> Result<ParsedQuota, QuotaParseError> {
    let response: UsageResponse = serde_json::from_slice(body)?;
    let windows = [
        (WindowId::FIVE_HOUR, response.five_hour),
        (WindowId::SEVEN_DAY, response.seven_day),
        ("seven_day_sonnet", response.seven_day_sonnet),
        ("seven_day_opus", response.seven_day_opus),
    ]
    .into_iter()
    .filter_map(|(name, raw)| {
        let raw = raw?;
        Some(QuotaWindow {
            id: WindowId::new(name).ok()?,
            state: QuotaWindowState {
                utilization: raw.utilization?,
                resets_at: raw.resets_at,
            },
        })
    })
    .collect();
    Ok(ParsedQuota {
        snapshot: QuotaSnapshot {
            fetched_at,
            windows,
        },
        extra_usage: response.extra_usage,
    })
}

/// The Claude quota source: one instance serves every profile; the credentials select the
/// account.
#[derive(Debug, Clone)]
pub struct ClaudeQuota {
    client: Client,
    base: Url,
}

impl ClaudeQuota {
    /// Builds the source over a shared client ([`crate::build_client`]) and the Anthropic
    /// base URL ([`crate::Endpoints::anthropic`]).
    #[must_use]
    pub fn new(client: Client, base: Url) -> Self {
        Self { client, base }
    }

    /// Polls the usage endpoint once. `None` credentials mean the profile is signed out and no
    /// request is made.
    pub async fn poll(&self, credentials: Option<&ClaudeCredentials>) -> QuotaPoll {
        let Some(credentials) = credentials else {
            return QuotaPoll::of(QuotaOutcome::SignedOut);
        };
        let response = match self.get(USAGE_PATH, credentials).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, "quota request failed");
                return QuotaPoll::of(QuotaOutcome::NetworkDown);
            }
        };
        let status = response.status();
        tracing::debug!(status = status.as_u16(), "quota response");
        if status == StatusCode::UNAUTHORIZED {
            return QuotaPoll::of(QuotaOutcome::TokenExpired);
        }
        if status != StatusCode::OK {
            return QuotaPoll::of(QuotaOutcome::NetworkDown);
        }
        let body = match read_capped(response, MAX_BODY_BYTES).await {
            Ok(body) => body,
            Err(RemoteError::TooLarge { .. }) => return QuotaPoll::of(QuotaOutcome::Malformed),
            Err(error) => {
                tracing::warn!(%error, "quota body unreadable");
                return QuotaPoll::of(QuotaOutcome::NetworkDown);
            }
        };
        match parse_quota_response(&body, Timestamp::now()) {
            Ok(parsed) => QuotaPoll {
                outcome: QuotaOutcome::Ok(parsed.snapshot),
                extra_usage: parsed.extra_usage,
            },
            Err(error) => {
                tracing::warn!(%error, "quota response malformed");
                QuotaPoll::of(QuotaOutcome::Malformed)
            }
        }
    }

    /// Fetches the account email and organization from `/api/oauth/profile` (D26, once a day)
    /// and merges them with the plan and tier already known from `credentials`.
    ///
    /// # Errors
    /// [`RemoteError::Status`] for a non-200 answer (401 means the token expired),
    /// [`RemoteError::Network`] for transport failures, [`RemoteError::Malformed`] for a body
    /// that is not the expected JSON.
    pub async fn fetch_account(
        &self,
        credentials: &ClaudeCredentials,
    ) -> Result<AccountInfo, RemoteError> {
        let response = self.get(PROFILE_PATH, credentials).await?;
        let status = response.status();
        if status != StatusCode::OK {
            return Err(RemoteError::Status(status.as_u16()));
        }
        let body = read_capped(response, MAX_BODY_BYTES).await?;
        let profile = parse_profile_response(&body)
            .map_err(|_| RemoteError::Malformed("profile response is not the expected JSON"))?;
        Ok(credentials.account().clone().or(profile))
    }

    async fn get(
        &self,
        path: &str,
        credentials: &ClaudeCredentials,
    ) -> Result<reqwest::Response, RemoteError> {
        let url = join(&self.base, path)?;
        self.client
            .get(url)
            .bearer_auth(credentials.access_token().expose_secret())
            .header("anthropic-beta", BETA_HEADER)
            .send()
            .await
            .map_err(|e| RemoteError::Network(NetworkKind::of(&e)))
    }
}

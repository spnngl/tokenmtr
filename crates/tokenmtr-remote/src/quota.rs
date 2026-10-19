//! Claude quota polling: `GET /api/oauth/usage` with the user's OAuth token.

use std::time::Duration;

use jiff::Timestamp;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode, Url};
use secrecy::ExposeSecret;
use serde::Deserialize;
use tokenmtr_core::quota::QuotaWindowState;
use tokenmtr_core::{ExtraUsage, QuotaSnapshot, QuotaWindow, Utilization, WindowId};

use crate::account::{AccountInfo, parse_profile_response};
use crate::credentials::ClaudeCredentials;
use crate::error::{JsonPosition, NetworkKind, RemoteError};
use crate::http::{API_TIMEOUT, join, read_capped};

const USAGE_PATH: &str = "/api/oauth/usage";
const PROFILE_PATH: &str = "/api/oauth/profile";
/// Value of the `anthropic-beta` header the OAuth endpoints require.
const BETA_HEADER: &str = "oauth-2025-04-20";
const MAX_BODY_BYTES: usize = 1 << 20;

/// What one quota poll found: exactly the cases a caller must tell apart.
#[derive(Debug)]
pub enum QuotaOutcome {
    /// HTTP 200 with a body that parsed.
    Ok(QuotaSnapshot),
    /// The credentials' `expiresAt` has passed (no request sent), or HTTP 401: Claude Code must
    /// refresh or re-login. This crate never refreshes.
    TokenExpired,
    /// HTTP 403: the token is valid but may not read the usage (revoked scope, account type).
    Forbidden,
    /// HTTP 429. `retry_after` is the server's `Retry-After`, when it sent a usable one.
    RateLimited {
        /// How long the server asked to wait.
        retry_after: Option<Duration>,
    },
    /// No credentials were found for the profile.
    SignedOut,
    /// HTTP 200 whose body is not a JSON object, or whose body could not be read in full.
    Malformed,
    /// No usable answer: transport failure, timeout, or any other status (5xx included).
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

/// The `extra_usage` object (pay-as-you-go spend on top of a plan), as Claude reports it.
/// Amounts are US cents, possibly fractional (Superset reads them the same way).
#[derive(Debug, Deserialize)]
struct RawExtraUsage {
    /// Whether extra usage is switched on for the account.
    #[serde(default)]
    is_enabled: bool,
    /// The monthly spending cap; required when enabled.
    monthly_limit: Option<f64>,
    /// Spent this month; `null` means nothing yet.
    used_credits: Option<f64>,
    /// Percent of the monthly cap used; computed when absent.
    utilization: Option<Utilization>,
}

/// The month's extra usage of an enabled account; `None` when it is disabled, or malformed
/// (no cap, a negative or non-finite amount), which is logged.
fn extra_usage(raw: Option<serde_json::Value>) -> Option<ExtraUsage> {
    let raw: RawExtraUsage = part("extra_usage", raw)?;
    if !raw.is_enabled {
        return None;
    }
    let Some(limit) = raw.monthly_limit else {
        tracing::warn!(
            part = "extra_usage",
            "skipping extra usage without a monthly limit"
        );
        return None;
    };
    match ExtraUsage::from_cents(raw.used_credits.unwrap_or(0.0), limit, raw.utilization) {
        Ok(extra) => Some(extra),
        Err(error) => {
            tracing::warn!(part = "extra_usage", %error, "skipping a malformed part of the quota response");
            None
        }
    }
}

/// A successfully parsed usage response.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedQuota {
    /// The windows the response reported (missing and `null` windows are absent).
    pub snapshot: QuotaSnapshot,
    /// The month's extra usage, when the account has it enabled and the object is valid.
    pub extra_usage: Option<ExtraUsage>,
}

/// `/api/oauth/usage` response. Unknown fields are ignored; every window is optional. Each part
/// is kept raw and parsed on its own, so one bad window does not hide the others.
#[derive(Debug, Deserialize)]
struct UsageResponse {
    five_hour: Option<serde_json::Value>,
    seven_day: Option<serde_json::Value>,
    seven_day_sonnet: Option<serde_json::Value>,
    seven_day_opus: Option<serde_json::Value>,
    extra_usage: Option<serde_json::Value>,
    /// Newer limits, e.g. a weekly window scoped to one model; each entry is parsed on its own.
    limits: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RawWindow {
    utilization: Option<Utilization>,
    resets_at: Option<Timestamp>,
}

/// One `limits` entry: `{"kind": "weekly_scoped", "percent": 42.0, "resets_at": "...",
/// "scope": {"model": {"display_name": "Fable"}}}`.
#[derive(Debug, Deserialize)]
struct RawLimit {
    kind: String,
    percent: Option<Utilization>,
    resets_at: Option<Timestamp>,
    scope: Option<RawScope>,
}

#[derive(Debug, Deserialize)]
struct RawScope {
    model: Option<RawModel>,
}

#[derive(Debug, Deserialize)]
struct RawModel {
    display_name: Option<String>,
}

const WEEKLY_SCOPED: &str = "weekly_scoped";

/// The `weekly_scoped` windows of `limits`, one per model. Entries of another kind, without a
/// model name or a percent, or malformed are skipped one by one.
fn scoped_windows(limits: Option<serde_json::Value>) -> Vec<QuotaWindow> {
    let entries: Vec<serde_json::Value> = part("limits", limits).unwrap_or_default();
    let mut windows: Vec<QuotaWindow> = Vec::new();
    for raw in entries {
        let Some(limit) = part::<RawLimit>("limits", Some(raw)) else {
            continue;
        };
        if limit.kind != WEEKLY_SCOPED {
            continue;
        }
        let name = limit
            .scope
            .and_then(|s| s.model)
            .and_then(|m| m.display_name);
        let (Some(name), Some(utilization)) = (name, limit.percent) else {
            continue;
        };
        let Ok(id) = WindowId::weekly_scoped(&name) else {
            continue;
        };
        if windows.iter().all(|w| w.id != id) {
            windows.push(QuotaWindow {
                id,
                state: QuotaWindowState {
                    utilization,
                    resets_at: limit.resets_at,
                },
            });
        }
    }
    windows
}

/// Why a usage body was rejected. Carries the position and category only: serde messages quote
/// the offending value, which must not be echoed.
#[derive(Debug, thiserror::Error)]
#[error("quota response is not the expected JSON ({0})")]
pub struct QuotaParseError(JsonPosition);

/// Parses one part of the response; a part of the wrong shape is logged (category only, no body)
/// and dropped.
fn part<T: serde::de::DeserializeOwned>(name: &str, raw: Option<serde_json::Value>) -> Option<T> {
    match serde_json::from_value(raw?) {
        Ok(value) => value,
        Err(error) => {
            let error = JsonPosition::from(error);
            tracing::warn!(part = name, %error, "skipping a malformed part of the quota response");
            None
        }
    }
}

/// Parses an `/api/oauth/usage` body into windows stamped `fetched_at`. A window that is
/// absent, `null`, has a `null` utilization, or is malformed (wrong types, a negative or
/// non-finite utilization, an unparsable `resets_at`) is skipped; so is a disabled or malformed
/// `extra_usage`. Each `weekly_scoped` entry of `limits` adds a window per model
/// ([`WindowId::weekly_scoped`]) and replaces a top-level `seven_day_<model>` of the same model.
///
/// # Errors
/// [`QuotaParseError`] when the body is not a JSON object.
pub fn parse_quota_response(
    body: &[u8],
    fetched_at: Timestamp,
) -> Result<ParsedQuota, QuotaParseError> {
    let response: UsageResponse =
        serde_json::from_slice(body).map_err(|e| QuotaParseError(e.into()))?;
    let scoped = scoped_windows(response.limits);
    let mut windows: Vec<QuotaWindow> = [
        (WindowId::FIVE_HOUR, response.five_hour),
        (WindowId::SEVEN_DAY, response.seven_day),
        ("seven_day_sonnet", response.seven_day_sonnet),
        ("seven_day_opus", response.seven_day_opus),
    ]
    .into_iter()
    .filter(|(name, _)| {
        let model = name.strip_prefix("seven_day_");
        model.is_none() || !scoped.iter().any(|w| w.id.scoped_model() == model)
    })
    .filter_map(|(name, raw)| {
        let raw: RawWindow = part(name, raw)?;
        Some(QuotaWindow {
            id: WindowId::new(name).ok()?,
            state: QuotaWindowState {
                utilization: raw.utilization?,
                resets_at: raw.resets_at,
            },
        })
    })
    .collect();
    windows.extend(scoped);
    Ok(ParsedQuota {
        snapshot: QuotaSnapshot {
            fetched_at,
            windows,
        },
        extra_usage: extra_usage(response.extra_usage),
    })
}

/// The wait a `Retry-After` value asks for: delay-seconds, or an HTTP-date (a date in the past
/// is no wait).
fn parse_retry_after(value: &str, now: Timestamp) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = jiff::fmt::rfc2822::parse(value).ok()?.timestamp();
    Some(Duration::try_from(at.duration_since(now)).unwrap_or(Duration::ZERO))
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

    /// Polls the usage endpoint once. No request is made when the profile is signed out
    /// (`None` credentials) or the token is past its `expiresAt`.
    pub async fn poll(&self, credentials: Option<&ClaudeCredentials>) -> QuotaPoll {
        let Some(credentials) = credentials else {
            return QuotaPoll::of(QuotaOutcome::SignedOut);
        };
        if credentials.is_expired(Timestamp::now()) {
            return QuotaPoll::of(QuotaOutcome::TokenExpired);
        }
        let response = match self.get(USAGE_PATH, credentials).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, "quota request failed");
                return QuotaPoll::of(QuotaOutcome::NetworkDown);
            }
        };
        let status = response.status();
        tracing::debug!(status = status.as_u16(), "quota response");
        match status {
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED => return QuotaPoll::of(QuotaOutcome::TokenExpired),
            StatusCode::FORBIDDEN => return QuotaPoll::of(QuotaOutcome::Forbidden),
            StatusCode::TOO_MANY_REQUESTS => {
                let retry_after = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| parse_retry_after(value, Timestamp::now()));
                return QuotaPoll::of(QuotaOutcome::RateLimited { retry_after });
            }
            _ => return QuotaPoll::of(QuotaOutcome::NetworkDown),
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

    /// Fetches the account email and organization from `/api/oauth/profile` (once a day)
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
            .timeout(API_TIMEOUT)
            .bearer_auth(credentials.access_token().expose_secret())
            .header("anthropic-beta", BETA_HEADER)
            .send()
            .await
            .map_err(|e| RemoteError::Network(NetworkKind::of(&e)))
    }
}

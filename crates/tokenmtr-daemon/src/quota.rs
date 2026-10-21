//! Claude quota polling and plan detection.
//!
//! One poll per Claude profile with its own credentials. Tokens are read inside a blocking task,
//! live in [`ClaudeCredentials`] (a `SecretString`) and never reach a log line or an error.
//! They are kept in memory until they expire or the server rejects them, so the Keychain is not
//! read on every poll. Failed polls back off ([`ProfileRuntime::record`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use tokenmtr_config::{ProfileConfig, ProfileLocation};
use tokenmtr_control::{QuotaProfileStatus as Status, QuotaProfileSummary};
use tokenmtr_core::alert::{QuotaCycle, QuotaWindowStatus};
use tokenmtr_core::forecast::RESET_TOLERANCE;
use tokenmtr_core::{AgentId, ExtraUsage, ProfileId, QuotaWindow};
use tokenmtr_platform::ThreadPriority;
use tokenmtr_remote::{
    AccountInfo, ClaudeCredentials, ClaudeQuota, CredentialLocation, QuotaOutcome as Remote,
};
use tokenmtr_store::{AccountRecord, QuotaOutcome, QuotaPoll};

use crate::alerts::Evaluation;
use crate::changes::std_duration;
use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;

/// The plan is re-detected at most this often.
const ACCOUNT_REFRESH: Duration = Duration::from_hours(24);
/// The longest wait between two polls of a failing profile, unless the poll interval is longer.
const MAX_BACKOFF: Duration = Duration::from_hours(1);

/// What one profile's poll found, before it is stored.
struct Polled {
    profile: ProfileId,
    account: AccountInfo,
    /// `None` when the poll was skipped because the profile is backing off: nothing is stored.
    outcome: Option<QuotaOutcome>,
    status: Status,
    /// The account to save when plan detection ran.
    detected: Option<AccountInfo>,
}

impl Polled {
    fn without_windows(profile: ProfileId, outcome: Option<QuotaOutcome>, status: Status) -> Self {
        Self {
            profile,
            account: AccountInfo::default(),
            outcome,
            status,
            detected: None,
        }
    }
}

/// Quota state that lives as long as the process, per Claude profile.
#[derive(Debug, Default)]
pub(crate) struct QuotaRuntime {
    profiles: HashMap<ProfileId, ProfileRuntime>,
}

/// One profile's credential cache and retry schedule.
#[derive(Debug, Default)]
struct ProfileRuntime {
    /// Where the credentials were read; a reload that moves the profile starts afresh.
    location: Option<CredentialLocation>,
    /// The last credentials the server accepted: every poll reads the credentials again and
    /// falls back on these when the read fails, until they expire or the server answers 401/403.
    credentials: Option<Arc<ClaudeCredentials>>,
    /// The Keychain refused once: only files are read, and without one the profile is
    /// `keychain_denied`, until [`Ctx::connect_keychain`] or a restart.
    keychain_denied: bool,
    /// Consecutive failed polls; 0 after a success.
    failures: u32,
    /// No request before this instant.
    not_before: Option<Instant>,
}

impl ProfileRuntime {
    /// Updates the cache and the retry schedule after a poll that started at `started`.
    ///
    /// Failures (403, 429, malformed bodies, network trouble) wait `interval`, then twice as
    /// long after each further failure, up to [`MAX_BACKOFF`]; a 429's `Retry-After` replaces
    /// that wait. Credential states (signed out, expired) do not back off: re-reading them is
    /// local and cheap, and the user fixing them should show at the next interval.
    fn record(
        &mut self,
        outcome: &Remote,
        credentials: Option<Arc<ClaudeCredentials>>,
        interval: Duration,
        started: Instant,
    ) {
        let retry_after = match outcome {
            Remote::Ok(_) => {
                self.credentials = credentials;
                self.failures = 0;
                self.not_before = None;
                return;
            }
            Remote::SignedOut | Remote::TokenExpired => {
                self.credentials = None;
                self.failures = 0;
                self.not_before = None;
                return;
            }
            Remote::Forbidden => {
                self.credentials = None;
                None
            }
            Remote::RateLimited { retry_after } => *retry_after,
            Remote::Malformed | Remote::NetworkDown => None,
        };
        self.failures = self.failures.saturating_add(1);
        let wait = retry_after.unwrap_or_else(|| backoff(self.failures, interval));
        self.not_before = started.checked_add(wait.min(MAX_BACKOFF.max(interval)));
    }
}

/// `interval` doubled for each failure after the first.
fn backoff(failures: u32, interval: Duration) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    interval.saturating_mul(2_u32.saturating_pow(doublings))
}

impl Ctx {
    /// Polls every Claude profile once and stores the result.
    ///
    /// # Errors
    /// [`DaemonError::Disabled`] when `quota.enabled` is off; network trouble is not an error:
    /// it is a stored outcome.
    pub(crate) async fn poll_quota(
        self: &Arc<Self>,
    ) -> Result<Vec<QuotaProfileSummary>, DaemonError> {
        let active = self.active();
        if !active.config.quota.enabled {
            return Err(DaemonError::Disabled("quota polling"));
        }
        let profiles = claude_profiles(&active.config);
        let http = self.http()?;
        let source = ClaudeQuota::new(http.client.clone(), http.endpoints.anthropic.clone());
        let mut polled = Vec::with_capacity(profiles.len());
        for profile in profiles {
            polled.push(self.poll_profile(&source, profile).await?);
        }
        let summaries = polled
            .iter()
            .map(|poll| QuotaProfileSummary {
                agent: AgentId::CLAUDE,
                profile: poll.profile.as_str().to_owned(),
                status: poll.status,
            })
            .collect();
        self.blocking(ThreadPriority::Background, move |ctx| {
            ctx.store_quota(polled);
        })
        .await?;
        Ok(summaries)
    }

    /// Reads every Claude profile's credentials once with the Keychain's consent prompt
    /// allowed, waiting for the user, so they can grant this binary access for good; then
    /// polls as [`Ctx::poll_quota`]. Credentials read this way are cached like a poll's, so
    /// quota works until they expire even when the user allowed access only once.
    ///
    /// # Errors
    /// As [`Ctx::poll_quota`].
    pub(crate) async fn connect_keychain(
        self: &Arc<Self>,
    ) -> Result<Vec<QuotaProfileSummary>, DaemonError> {
        let active = self.active();
        if !active.config.quota.enabled {
            return Err(DaemonError::Disabled("quota polling"));
        }
        for profile in claude_profiles(&active.config) {
            let location = credential_location(&self.settings.paths, &profile);
            let lookup = location.clone();
            let loaded = self
                .blocking(ThreadPriority::UserInitiated, move |ctx| {
                    ClaudeCredentials::load_interactive(&lookup, &*ctx.settings.secrets)
                })
                .await?;
            let (credentials, keychain_denied) = match loaded {
                Ok(credentials) => (credentials.map(Arc::new), false),
                Err(error) => {
                    tracing::warn!(profile = %profile.id, %error, "cannot read the Claude credentials");
                    (None, error.is_keychain_denied())
                }
            };
            lock(&self.run).quota.profiles.insert(
                profile.id,
                ProfileRuntime {
                    location: Some(location),
                    credentials,
                    keychain_denied,
                    ..ProfileRuntime::default()
                },
            );
        }
        self.poll_quota().await
    }

    async fn poll_profile(
        self: &Arc<Self>,
        source: &ClaudeQuota,
        profile: ProfileConfig,
    ) -> Result<Polled, DaemonError> {
        let location = credential_location(&self.settings.paths, &profile);
        let interval = std_duration(self.active().config.quota.interval);
        let started = Instant::now();
        let (cached, keychain_denied) = {
            let mut run = lock(&self.run);
            let state = run.quota.profiles.entry(profile.id.clone()).or_default();
            if state.location.as_ref() != Some(&location) {
                *state = ProfileRuntime {
                    location: Some(location.clone()),
                    ..ProfileRuntime::default()
                };
            }
            if state.not_before.is_some_and(|at| started < at) {
                return Ok(Polled::without_windows(profile.id, None, Status::Backoff));
            }
            let fresh = state
                .credentials
                .clone()
                .filter(|credentials| !credentials.is_expired(Timestamp::now()));
            (fresh, state.keychain_denied)
        };
        let credentials = match self
            .read_credentials(&profile.id, location, cached, keychain_denied)
            .await?
        {
            Ok(credentials) => credentials,
            Err((outcome, status)) => {
                return Ok(Polled::without_windows(profile.id, Some(outcome), status));
            }
        };
        let poll = source.poll(credentials.as_deref()).await;
        if let Some(state) = lock(&self.run).quota.profiles.get_mut(&profile.id) {
            state.record(&poll.outcome, credentials.clone(), interval, started);
        }
        let (outcome, status) = store_outcome(poll.outcome, poll.extra_usage);
        let account = credentials
            .as_ref()
            .map(|credentials| credentials.account().clone())
            .unwrap_or_default();
        let detected = match (&credentials, status) {
            (Some(credentials), Status::Ok) if self.account_is_stale(&profile.id) => {
                match source.fetch_account(credentials).await {
                    Ok(info) => Some(info.or(account.clone())),
                    Err(error) => {
                        tracing::debug!(%error, "cannot fetch the Claude profile");
                        None
                    }
                }
            }
            _ => None,
        };
        Ok(Polled {
            profile: profile.id,
            account,
            outcome: Some(outcome),
            status,
            detected,
        })
    }

    /// Reads the profile's credentials at every poll, so a token the agent refreshed is used from
    /// the next poll on. A read that fails, or that finds nothing without the Keychain, keeps
    /// `cached` (such as the token one `connect_keychain` read) until it expires or is rejected.
    /// `Err` carries what to record when no credentials can be sent.
    async fn read_credentials(
        self: &Arc<Self>,
        profile: &ProfileId,
        location: CredentialLocation,
        cached: Option<Arc<ClaudeCredentials>>,
        keychain_denied: bool,
    ) -> Result<Result<Option<Arc<ClaudeCredentials>>, (QuotaOutcome, Status)>, DaemonError> {
        let lookup = if keychain_denied {
            location.without_keychain()
        } else {
            location
        };
        let loaded = self
            .blocking(ThreadPriority::Background, move |ctx| {
                ClaudeCredentials::load(&lookup, &*ctx.settings.secrets)
            })
            .await?;
        let denied = Err((QuotaOutcome::KeychainDenied, Status::KeychainDenied));
        Ok(match (loaded, cached) {
            (Ok(Some(credentials)), _) => Ok(Some(Arc::new(credentials))),
            (Ok(None), Some(cached)) if keychain_denied => Ok(Some(cached)),
            // Without the Keychain, finding no file says nothing about being signed in.
            (Ok(None), None) if keychain_denied => denied,
            (Ok(None), _) => Ok(None),
            (Err(error), cached) if error.is_keychain_denied() => {
                tracing::warn!(
                    profile = %profile,
                    "the Keychain denied access to the Claude credentials; \
                     not asking again until `tokenmtr quota connect` or a restart"
                );
                if let Some(state) = lock(&self.run).quota.profiles.get_mut(profile) {
                    state.keychain_denied = true;
                }
                cached.map_or(denied, |cached| Ok(Some(cached)))
            }
            (Err(_), Some(cached)) => Ok(Some(cached)),
            (Err(error), None) => {
                tracing::warn!(%profile, %error, "cannot read the Claude credentials");
                Err((QuotaOutcome::Failed, Status::Failed))
            }
        })
    }

    fn account_is_stale(&self, profile: &ProfileId) -> bool {
        let now = Timestamp::now();
        lock(&self.run)
            .account_checked
            .get(&(AgentId::CLAUDE, profile.clone()))
            .is_none_or(|at| {
                Duration::try_from(now.duration_since(*at)).is_ok_and(|age| age >= ACCOUNT_REFRESH)
            })
    }

    /// Stores the polls (extra usage spend becomes usage events on display-zone days), the
    /// detected accounts, then reacts (events, alerts; budgets too when extra usage was seen,
    /// since it counts as paid).
    fn store_quota(&self, polled: Vec<Polled>) {
        let now = Timestamp::now();
        let zone = tokenmtr_app::display_zone(&self.active().config);
        let mut statuses = Vec::new();
        let mut polls = Vec::with_capacity(polled.len());
        let mut extra_seen = false;
        for poll in polled {
            let Some(outcome) = poll.outcome else {
                continue;
            };
            if let QuotaOutcome::Windows {
                windows,
                extra_usage,
            } = &outcome
            {
                statuses.extend(self.window_statuses(&poll.profile, windows));
                if let Some(extra) = extra_usage {
                    extra_seen = true;
                    statuses.push(QuotaWindowStatus {
                        agent: AgentId::CLAUDE,
                        profile: poll.profile.clone(),
                        window: ExtraUsage::window_id(),
                        utilization: extra.utilization,
                        cycle: QuotaCycle::Monthly {
                            month: now.to_zoned(zone.clone()).date(),
                        },
                    });
                }
            }
            if let Some(info) = poll.detected {
                self.save_account(&poll.profile, info, now);
            }
            polls.push(QuotaPoll {
                agent: AgentId::CLAUDE,
                profile: poll.profile,
                account: poll.account.email,
                fetched_at: now,
                outcome,
            });
        }
        if let Err(error) = lock(&self.store).upsert_quota_snapshots(&polls, &zone) {
            tracing::warn!(%error, "cannot store quota snapshots");
        }
        self.after_write(false);
        self.evaluate_alerts(Evaluation {
            quota: statuses,
            budgets: extra_seen,
        });
    }

    fn save_account(&self, profile: &ProfileId, info: AccountInfo, now: Timestamp) {
        let record = AccountRecord {
            agent: AgentId::CLAUDE,
            profile: profile.clone(),
            email: info.email,
            organization: info.organization,
            detected_plan: info.plan,
            rate_limit_tier: info.rate_limit_tier,
            fetched_at: now,
        };
        match lock(&self.store).save_account(&record) {
            Ok(()) => {
                lock(&self.run)
                    .account_checked
                    .insert((AgentId::CLAUDE, profile.clone()), now);
            }
            Err(error) => tracing::warn!(%error, "cannot store the account"),
        }
    }

    /// The windows of one poll with the `resets_at` the previous poll saw; remembers this one.
    /// A `resets_at` within [`RESET_TOLERANCE`] of the previous one is the same window instance
    /// and keeps the previous value ([`same_instance`]).
    fn window_statuses(
        &self,
        profile: &ProfileId,
        windows: &[QuotaWindow],
    ) -> Vec<QuotaWindowStatus> {
        let mut run = lock(&self.run);
        windows
            .iter()
            .map(|window| {
                let key = (profile.clone(), window.id.clone());
                let previous_resets_at = run.quota_resets.get(&key).copied().flatten();
                let resets_at = same_instance(window.state.resets_at, previous_resets_at);
                run.quota_resets.insert(key, resets_at);
                QuotaWindowStatus {
                    agent: AgentId::CLAUDE,
                    profile: profile.clone(),
                    window: window.id.clone(),
                    utilization: window.state.utilization,
                    cycle: QuotaCycle::Rolling {
                        resets_at,
                        previous_resets_at,
                    },
                }
            })
            .collect()
    }
}

/// The store outcome and the `refresh_quota` status of a remote outcome. 403 and 429
/// are stored as `failed`: the store and the app know no finer kinds, and neither proves the
/// token is gone.
fn store_outcome(outcome: Remote, extra_usage: Option<ExtraUsage>) -> (QuotaOutcome, Status) {
    match outcome {
        Remote::Ok(snapshot) => (
            QuotaOutcome::Windows {
                windows: snapshot.windows,
                extra_usage,
            },
            Status::Ok,
        ),
        Remote::TokenExpired => (QuotaOutcome::TokenExpired, Status::TokenExpired),
        Remote::SignedOut => (QuotaOutcome::SignedOut, Status::SignedOut),
        Remote::Forbidden => (QuotaOutcome::Failed, Status::Forbidden),
        Remote::RateLimited { .. } => (QuotaOutcome::Failed, Status::RateLimited),
        Remote::Malformed => (QuotaOutcome::Failed, Status::Malformed),
        Remote::NetworkDown => (QuotaOutcome::Failed, Status::NetworkDown),
    }
}

/// `resets_at`, or `previous` when both are known and within [`RESET_TOLERANCE`]: the API
/// jitters the reset time between polls, and keeping the first value keeps the alert instance
/// stable even when the jitter crosses a rounding boundary.
fn same_instance(resets_at: Option<Timestamp>, previous: Option<Timestamp>) -> Option<Timestamp> {
    match (resets_at, previous) {
        (Some(at), Some(previous)) if at.duration_since(previous).abs() <= RESET_TOLERANCE => {
            Some(previous)
        }
        _ => resets_at,
    }
}

/// The configured Claude profiles; the default profile when the config names none.
fn claude_profiles(config: &tokenmtr_config::Config) -> Vec<ProfileConfig> {
    config
        .agents
        .get(&AgentId::CLAUDE)
        .filter(|agent| agent.enabled)
        .map(|agent| {
            agent
                .profiles
                .iter()
                .filter(|profile| !matches!(profile.location, ProfileLocation::Roots(_)))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Where a profile's OAuth credentials live. Profiles with explicit roots only have no
/// credentials of their own and are skipped before this.
fn credential_location(
    paths: &tokenmtr_platform::Paths,
    profile: &ProfileConfig,
) -> CredentialLocation {
    match &profile.location {
        ProfileLocation::ConfigDir(dir) => CredentialLocation::config_dir(dir),
        ProfileLocation::Default | ProfileLocation::Roots(_) => {
            CredentialLocation::default_profile(paths.home())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenmtr_core::QuotaSnapshot;

    const INTERVAL: Duration = Duration::from_mins(5);

    fn credentials() -> Option<Arc<ClaudeCredentials>> {
        ClaudeCredentials::parse(r#"{"claudeAiOauth":{"accessToken":"t"}}"#)
            .unwrap()
            .map(Arc::new)
    }

    fn ok() -> Remote {
        Remote::Ok(QuotaSnapshot {
            fetched_at: Timestamp::UNIX_EPOCH,
            windows: Vec::new(),
        })
    }

    fn wait(state: &ProfileRuntime, started: Instant) -> Option<Duration> {
        state.not_before.map(|at| at.duration_since(started))
    }

    #[test]
    fn failures_back_off_exponentially_up_to_the_cap_and_reset_on_success() {
        let mut state = ProfileRuntime::default();
        let started = Instant::now();
        let mut waits = Vec::new();
        for _ in 0..6 {
            state.record(&Remote::NetworkDown, credentials(), INTERVAL, started);
            waits.push(wait(&state, started).unwrap().as_secs() / 60);
        }
        assert_eq!(waits, [5, 10, 20, 40, 60, 60]);

        state.record(&ok(), credentials(), INTERVAL, started);
        assert_eq!((state.failures, state.not_before), (0, None));
        assert!(state.credentials.is_some());
    }

    #[test]
    fn retry_after_replaces_the_computed_wait() {
        let mut state = ProfileRuntime::default();
        let started = Instant::now();
        let limited = |seconds| Remote::RateLimited {
            retry_after: Some(Duration::from_secs(seconds)),
        };
        state.record(&limited(1_800), credentials(), INTERVAL, started);
        assert_eq!(wait(&state, started), Some(Duration::from_mins(30)));
        state.record(&limited(86_400), credentials(), INTERVAL, started);
        assert_eq!(wait(&state, started), Some(MAX_BACKOFF));
        state.record(
            &Remote::RateLimited { retry_after: None },
            None,
            INTERVAL,
            started,
        );
        assert_eq!(wait(&state, started), Some(Duration::from_mins(20)));
    }

    #[test]
    fn rejected_tokens_are_dropped_from_the_cache() {
        let started = Instant::now();
        for outcome in [Remote::TokenExpired, Remote::Forbidden, Remote::SignedOut] {
            let mut state = ProfileRuntime {
                credentials: credentials(),
                ..ProfileRuntime::default()
            };
            state.record(&outcome, credentials(), INTERVAL, started);
            assert!(state.credentials.is_none(), "{outcome:?}");
        }
        let mut state = ProfileRuntime::default();
        state.record(&Remote::TokenExpired, credentials(), INTERVAL, started);
        assert_eq!(
            state.not_before, None,
            "expiry is re-checked at the next interval"
        );
    }

    #[test]
    fn jittered_resets_keep_the_first_instance() {
        let at = |s: &str| Some(s.parse::<Timestamp>().unwrap());
        let previous = at("2026-05-12T14:59:59.943Z");
        assert_eq!(
            same_instance(at("2026-05-12T15:00:00.120Z"), previous),
            previous
        );
        assert_eq!(
            same_instance(at("2026-05-12T15:04:59Z"), previous),
            previous
        );
        let next = at("2026-05-12T19:59:59.900Z");
        assert_eq!(same_instance(next, previous), next);
        assert_eq!(same_instance(None, previous), None);
        assert_eq!(same_instance(next, None), next);
    }
}

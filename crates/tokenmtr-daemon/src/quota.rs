//! Claude quota polling and plan detection (D26).
//!
//! One poll per Claude profile with its own credentials. Tokens are read inside a blocking task,
//! live in [`ClaudeCredentials`] (a `SecretString`) and never reach a log line or an error (D55).

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use tokenmtr_config::{ProfileConfig, ProfileLocation};
use tokenmtr_control::QuotaProfileSummary;
use tokenmtr_core::alert::QuotaWindowStatus;
use tokenmtr_core::{AgentId, ProfileId, QuotaWindow};
use tokenmtr_platform::ThreadPriority;
use tokenmtr_remote::{
    AccountInfo, ClaudeCredentials, ClaudeQuota, CredentialLocation, QuotaOutcome as Remote,
};
use tokenmtr_store::{AccountRecord, QuotaOutcome, QuotaPoll};

use crate::alerts::Evaluation;
use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;

/// The plan is re-detected at most this often.
const ACCOUNT_REFRESH: Duration = Duration::from_hours(24);

/// What one profile's poll found, before it is stored.
struct Polled {
    profile: ProfileId,
    account: AccountInfo,
    outcome: QuotaOutcome,
    status: &'static str,
    /// The account to save when plan detection ran.
    detected: Option<AccountInfo>,
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
                status: poll.status.to_owned(),
            })
            .collect();
        self.blocking(ThreadPriority::Background, move |ctx| {
            ctx.store_quota(polled);
        })
        .await?;
        Ok(summaries)
    }

    async fn poll_profile(
        self: &Arc<Self>,
        source: &ClaudeQuota,
        profile: ProfileConfig,
    ) -> Result<Polled, DaemonError> {
        let location = credential_location(&self.settings.paths, &profile);
        let credentials = self
            .blocking(ThreadPriority::Background, move |ctx| {
                ClaudeCredentials::load(&location, &*ctx.settings.secrets)
            })
            .await?;
        let credentials = match credentials {
            Ok(credentials) => credentials,
            Err(error) => {
                tracing::warn!(profile = %profile.id, %error, "cannot read the Claude credentials");
                return Ok(Polled {
                    profile: profile.id,
                    account: AccountInfo::default(),
                    outcome: QuotaOutcome::Failed,
                    status: "failed",
                    detected: None,
                });
            }
        };
        let poll = source.poll(credentials.as_ref()).await;
        let (outcome, status) = match poll.outcome {
            Remote::Ok(snapshot) => (QuotaOutcome::Windows(snapshot.windows), "ok"),
            Remote::TokenExpired => (QuotaOutcome::TokenExpired, "token_expired"),
            Remote::SignedOut => (QuotaOutcome::SignedOut, "signed_out"),
            Remote::Malformed => (QuotaOutcome::Failed, "malformed"),
            Remote::NetworkDown => (QuotaOutcome::Failed, "network_down"),
        };
        let account = credentials
            .as_ref()
            .map(|credentials| credentials.account().clone())
            .unwrap_or_default();
        let detected = match (&credentials, status) {
            (Some(credentials), "ok") if self.account_is_stale(&profile.id) => {
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
            outcome,
            status,
            detected,
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

    /// Stores the polls, the detected accounts, then reacts (events, alerts).
    fn store_quota(&self, polled: Vec<Polled>) {
        let now = Timestamp::now();
        let mut statuses = Vec::new();
        let mut polls = Vec::with_capacity(polled.len());
        for poll in polled {
            if let QuotaOutcome::Windows(windows) = &poll.outcome {
                statuses.extend(self.window_statuses(&poll.profile, windows));
            }
            if let Some(info) = poll.detected {
                self.save_account(&poll.profile, info, now);
            }
            polls.push(QuotaPoll {
                agent: AgentId::CLAUDE,
                profile: poll.profile,
                account: poll.account.email,
                fetched_at: now,
                outcome: poll.outcome,
            });
        }
        if let Err(error) = lock(&self.store).upsert_quota_snapshots(&polls) {
            tracing::warn!(%error, "cannot store quota snapshots");
        }
        self.after_write(false);
        self.evaluate_alerts(Evaluation {
            quota: statuses,
            budgets: false,
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
                run.quota_resets.insert(key, window.state.resets_at);
                QuotaWindowStatus {
                    window: window.id.clone(),
                    utilization: window.state.utilization,
                    resets_at: window.state.resets_at,
                    previous_resets_at,
                }
            })
            .collect()
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

/// Where a profile's OAuth credentials live (D26). Profiles with explicit roots only have no
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

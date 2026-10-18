//! Alert evaluation (D36): gather the inputs, run the core rules, store and announce new alerts.

use jiff::Timestamp;
use tokenmtr_app::{budget_specs, query_context};
use tokenmtr_control::Event;
use tokenmtr_core::alert::{BudgetState, DriftState, QuotaWindowStatus, SinkState};
use tokenmtr_core::{AlertInputs, evaluate_alerts};
use tokenmtr_query::budgets;
use tokenmtr_store::{IssueKind, StoreRead};

use crate::ctx::{Active, Ctx, lock};
use crate::error::DaemonError;

/// What to look at besides the always-checked sinks, backups and format drift.
#[derive(Debug, Default)]
pub(crate) struct Evaluation {
    /// The quota windows a poll just reported, with their previous `resets_at`.
    pub quota: Vec<QuotaWindowStatus>,
    /// Whether to query the budgets (they read usage).
    pub budgets: bool,
}

impl Ctx {
    /// Evaluates the rules and stores the alerts that have not fired yet. A failure is logged:
    /// alerting must never stop the work it reports on.
    pub(crate) fn evaluate_alerts(&self, evaluation: Evaluation) {
        if let Err(error) = self.try_evaluate_alerts(evaluation) {
            tracing::warn!(%error, "cannot evaluate alerts");
        }
    }

    fn try_evaluate_alerts(&self, evaluation: Evaluation) -> Result<(), DaemonError> {
        let active = self.active();
        let now = Timestamp::now();
        let mut inputs = AlertInputs::new(now);
        inputs.quota = evaluation.quota;
        if evaluation.budgets {
            inputs.budgets = self.budget_states(&active, now)?;
        }
        {
            let run = lock(&self.run);
            inputs.sinks = active
                .config
                .sinks
                .iter()
                .map(|sink| SinkState {
                    id: sink.id.clone(),
                    failing_since: run.sinks.get(&sink.id).and_then(|s| s.failing_since),
                })
                .collect();
            inputs.backup_failed_at = run.backup_failed_at;
        }
        {
            let reader = lock(&self.reader);
            inputs.already_fired = reader.fired_alert_keys()?;
            inputs.drift = reader
                .open_parse_issues()?
                .into_iter()
                .filter(|issue| issue.kind == IssueKind::FormatDrift)
                .map(|issue| DriftState {
                    agent: issue.agent,
                    opened_at: issue.first_seen,
                })
                .collect();
        }
        for alert in evaluate_alerts(&inputs, &active.config.alerts.rules) {
            let saved = lock(&self.store).save_alert(&alert);
            match saved {
                Ok(true) => {
                    tracing::warn!(alert = %alert.dedup_key, "alert fired");
                    self.bus.publish(Event::AlertFired { alert });
                }
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "cannot store an alert"),
            }
        }
        Ok(())
    }

    /// Where every configured budget stands now.
    fn budget_states(
        &self,
        active: &Active,
        now: Timestamp,
    ) -> Result<Vec<BudgetState>, DaemonError> {
        let config = &active.config;
        if config.budgets.is_empty() {
            return Ok(Vec::new());
        }
        let reader = lock(&self.reader);
        let context = query_context(config, &reader, now, None)?;
        let specs = budget_specs(config);
        Ok(budgets(&reader, &context, &specs)?
            .into_iter()
            .filter_map(|view| {
                let entry = config
                    .budgets
                    .iter()
                    .find(|entry| entry.budget.id().as_str() == view.id)?;
                Some(BudgetState {
                    id: entry.budget.id().clone(),
                    percent: view.percent,
                    period_start: view.period_start,
                })
            })
            .collect())
    }
}

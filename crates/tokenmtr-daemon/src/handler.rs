//! Answers to control requests.

use std::sync::Arc;

use tokenmtr_agents::ingest::{ScanPriority, ScanReport, ScanRequest};
use tokenmtr_config::Config;
use tokenmtr_control::{
    AgentScanSummary, BackupSummary, DaemonStatus, ErrorKind, ExportSummary, Handler, IssueKind,
    IssueList, IssueSummary, QuotaSummary, RepriceSummary, Request, Response, ScanSummary,
    SinkExportSummary, SinkSummary, Task,
};
use tokenmtr_core::SinkId;
use tokenmtr_platform::ThreadPriority;
use tokenmtr_store::{IssueKind as StoreIssueKind, RepriceScope, SCHEMA_VERSION, StoreRead};

use crate::ctx::{Ctx, lock};
use crate::error::DaemonError;
use crate::reload::ReloadCause;

/// The optional tasks `config` switches on, ascending.
pub(crate) fn enabled_tasks(config: &Config) -> Vec<Task> {
    [
        (Task::Watch, config.daemon.watch),
        (Task::Quota, config.quota.enabled),
        (
            Task::Fx,
            config.fx.enabled && config.fx.fixed_eur_per_usd.is_none(),
        ),
        (Task::Pricing, config.pricing.auto_update),
        (Task::Backup, config.backup.enabled),
    ]
    .into_iter()
    .filter_map(|(task, on)| on.then_some(task))
    .collect()
}

/// The [`Handler`] of the daemon's control server.
pub(crate) struct ControlHandler(pub Arc<Ctx>);

impl Handler for ControlHandler {
    async fn handle(&self, request: Request) -> Response {
        match self.dispatch(request).await {
            Ok(response) => response,
            Err(error) => failure(&error),
        }
    }
}

impl ControlHandler {
    async fn dispatch(&self, request: Request) -> Result<Response, DaemonError> {
        let ctx = &self.0;
        match request {
            Request::Status => Ok(Response::Status(self.status())),
            Request::ScanNow { agent } => {
                let mut scan = ScanRequest::incremental(ScanPriority::UserInitiated);
                if let Some(agent) = agent {
                    scan = scan.for_agent(agent);
                }
                Ok(Response::Scanned(scan_summary(&ctx.scan(scan).await?)))
            }
            Request::ExportNow { sink } => {
                self.check_sink(sink.as_ref())?;
                let outcomes = ctx.export(sink, ThreadPriority::UserInitiated).await?;
                Ok(Response::Exported(ExportSummary {
                    sinks: outcomes
                        .into_iter()
                        .map(|outcome| SinkExportSummary {
                            sink: outcome.sink,
                            rows: outcome.rows,
                            error: outcome.error,
                        })
                        .collect(),
                }))
            }
            Request::BackupNow => {
                let backup = ctx.backup(ThreadPriority::UserInitiated).await?;
                Ok(Response::BackedUp(BackupSummary {
                    path: backup.path,
                    size_bytes: backup.size_bytes,
                }))
            }
            Request::Reprice { all } => {
                let scope = if all {
                    RepriceScope::All
                } else {
                    RepriceScope::Unpriced
                };
                Ok(Response::Repriced(RepriceSummary {
                    repriced: ctx.reprice_now(scope).await?,
                }))
            }
            Request::RefreshQuota => Ok(Response::QuotaRefreshed(QuotaSummary {
                profiles: ctx.poll_quota().await?,
            })),
            Request::ConnectKeychain => Ok(Response::QuotaRefreshed(QuotaSummary {
                profiles: ctx.connect_keychain().await?,
            })),
            Request::Reload => {
                ctx.reload(ReloadCause::Requested).await?;
                Ok(Response::Reloaded)
            }
            Request::Issues => self.issues().await,
            Request::Shutdown => {
                // The permit is kept if the owner is not waiting yet.
                ctx.stop_requested.notify_one();
                Ok(Response::ShuttingDown)
            }
            Request::Hello { .. } | Request::Subscribe => Ok(Response::Error {
                kind: ErrorKind::BadRequest,
                message: "handled by the connection, not the dispatcher".to_owned(),
            }),
        }
    }

    /// Rejects a request naming a sink that is not configured.
    fn check_sink(&self, sink: Option<&SinkId>) -> Result<(), DaemonError> {
        let Some(sink) = sink else {
            return Ok(());
        };
        if self.0.active().config.sinks.iter().any(|s| s.id == *sink) {
            Ok(())
        } else {
            Err(DaemonError::Unknown(format!("no sink named `{sink}`")))
        }
    }

    fn status(&self) -> DaemonStatus {
        let ctx = &self.0;
        let active = ctx.active();
        let run = lock(&ctx.run);
        DaemonStatus {
            pid: std::process::id(),
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            store_schema: SCHEMA_VERSION,
            started_at: ctx.started_at,
            last_scan_at: run.last_scan_at,
            config_error: run.config_error.clone(),
            tasks: enabled_tasks(&active.config),
            subscribers: u32::try_from(ctx.bus.subscribers()).unwrap_or(u32::MAX),
            sinks: active
                .config
                .sinks
                .iter()
                .map(|sink| {
                    let known = run.sinks.get(&sink.id);
                    SinkSummary {
                        id: sink.id.clone(),
                        failing_since: known.and_then(|k| k.failing_since),
                        last_error: known.and_then(|k| k.last_error.clone()),
                        last_ok_at: known.and_then(|k| k.last_ok_at),
                    }
                })
                .collect(),
        }
    }

    async fn issues(&self) -> Result<Response, DaemonError> {
        let open = self
            .0
            .blocking(ThreadPriority::UserInitiated, |ctx| {
                lock(&ctx.reader).open_parse_issues()
            })
            .await??;
        Ok(Response::Issues(IssueList {
            parse_issues: open
                .into_iter()
                .map(|issue| IssueSummary {
                    agent: issue.agent,
                    kind: issue_kind(issue.kind),
                    subject: issue.subject,
                    occurrences: issue.occurrences,
                    last_seen: issue.last_seen,
                })
                .collect(),
            config_error: lock(&self.0.run).config_error.clone(),
        }))
    }
}

/// The wire copy of a store issue kind.
const fn issue_kind(kind: StoreIssueKind) -> IssueKind {
    match kind {
        StoreIssueKind::FormatDrift => IssueKind::FormatDrift,
        StoreIssueKind::UnknownVersion => IssueKind::UnknownVersion,
        StoreIssueKind::ProtectedFolder => IssueKind::ProtectedFolder,
        StoreIssueKind::MissingCwd => IssueKind::MissingCwd,
        StoreIssueKind::ParseError => IssueKind::ParseError,
        StoreIssueKind::IntegrationDrift => IssueKind::IntegrationDrift,
    }
}

fn scan_summary(report: &ScanReport) -> ScanSummary {
    ScanSummary {
        agents: report
            .agents
            .iter()
            .map(|scan| AgentScanSummary {
                agent: scan.agent,
                files_parsed: scan.files_parsed,
                files_failed: scan.files_failed,
                events_inserted: scan.events.inserted,
                events_updated: scan.events.updated,
            })
            .collect(),
        elapsed_ms: u64::try_from(report.elapsed.as_millis()).unwrap_or(u64::MAX),
    }
}

/// The error answer for `error`: its class and a message free of credentials.
fn failure(error: &DaemonError) -> Response {
    let kind = match error {
        DaemonError::Disabled(_) => ErrorKind::Disabled,
        DaemonError::Config(_) => ErrorKind::InvalidConfig,
        DaemonError::Unknown(_) => ErrorKind::BadRequest,
        DaemonError::ShuttingDown => ErrorKind::ShuttingDown,
        _ => ErrorKind::Failed,
    };
    Response::Error {
        kind,
        message: error.to_string(),
    }
}

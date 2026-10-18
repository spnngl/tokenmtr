//! Commands that move data: `backup`, `restore`, `erase`, `scan`, `reprice` and `report`.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::{Context, bail};
use camino::Utf8PathBuf;
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use jiff::civil::Date;
use serde::Serialize;
use tokenmtr_app::{
    EraseScope, ReportFormat, ReportRange, ReportRequest, daemon_running, load_config,
    restore_backup, store_path,
};
use tokenmtr_config::ProcessEnv;
use tokenmtr_core::{AgentId, Currency};
use tokenmtr_platform::{ControlTransport, Platform};
use tokenmtr_report::Scope;
use tokenmtr_store::RepriceScope;

use crate::client::with_daemon;
use crate::format::{bytes, print_json, timestamp};
use crate::setup::{Setup, now};

/// Arguments of `tokenmtr backup`.
#[derive(Debug, Args)]
pub struct BackupArgs {
    /// `now` when absent.
    #[command(subcommand)]
    action: Option<BackupAction>,
    /// Print JSON.
    #[arg(long, global = true)]
    json: bool,
}

/// What `tokenmtr backup` does.
#[derive(Debug, Clone, Copy, Subcommand)]
pub enum BackupAction {
    /// Write a backup now: through the daemon when it runs, else directly.
    Now,
    /// List the backup files and the recorded attempts.
    List,
}

#[derive(Debug, Serialize)]
struct BackupListing {
    files: Vec<tokenmtr_app::BackupFile>,
    attempts: Vec<tokenmtr_query::BackupView>,
}

/// `tokenmtr backup [now|list]`.
pub fn backup(setup: &Setup, args: &BackupArgs) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    match args.action.unwrap_or(BackupAction::Now) {
        BackupAction::Now => {
            let file = if ctx.daemon_running() {
                with_daemon(ctx.transport(), |mut client| async move {
                    let summary = client
                        .backup_now()
                        .await
                        .context("the daemon's backup failed")?;
                    Ok(tokenmtr_app::BackupFile {
                        path: summary.path,
                        size_bytes: summary.size_bytes,
                    })
                })?
            } else {
                ctx.backup_direct(now())?
            };
            if args.json {
                return print_json(&file);
            }
            println!("backup written: {} ({})", file.path, bytes(file.size_bytes));
        }
        BackupAction::List => {
            let listing = BackupListing {
                files: ctx.backups(),
                attempts: ctx.backup_history(20)?,
            };
            if args.json {
                return print_json(&listing);
            }
            if listing.files.is_empty() {
                println!("no backup files");
            }
            for file in &listing.files {
                println!("{} ({})", file.path, bytes(file.size_bytes));
            }
            for attempt in &listing.attempts {
                let outcome = attempt
                    .error
                    .as_deref()
                    .map_or_else(|| "ok".to_owned(), |error| format!("failed: {error}"));
                println!("attempt {}: {outcome}", timestamp(attempt.started_at));
            }
        }
    }
    Ok(())
}

/// Arguments of `tokenmtr restore`.
#[derive(Debug, Args)]
pub struct RestoreArgs {
    /// The backup file (`usage-<time>.db.zst`, see `tokenmtr backup list`).
    file: Utf8PathBuf,
}

/// `tokenmtr restore <file>`: stops the daemon, replaces the store, then starts the service
/// again if it was loaded.
pub fn restore(setup: &Setup, args: &RestoreArgs) -> anyhow::Result<()> {
    // No store is opened: restore must work when the current one is damaged or too new.
    let config = load_config(setup.config_path(), &ProcessEnv)
        .with_context(|| format!("cannot load {}", setup.config_path()))?;
    let store = store_path(setup.paths(), &config);
    let transport = setup.transport();
    request_shutdown(&transport)?;
    restore_backup(&transport, setup.platform().service(), &store, &args.file)?;
    println!("restored {store} from {}", args.file);
    Ok(())
}

/// Arguments of `tokenmtr erase`.
#[derive(Debug, Args)]
pub struct EraseArgs {
    /// Do not ask for confirmation.
    #[arg(long)]
    yes: bool,
    /// Also delete the config file.
    #[arg(long)]
    delete_config: bool,
    /// Also remove the background service (this stops the daemon).
    #[arg(long)]
    service: bool,
}

/// `tokenmtr erase`: stops the daemon, then deletes the store, backups and logs. Sink
/// outputs and the agents' own files stay. A kept service is started again on an empty store.
pub fn erase(setup: &Setup, args: &EraseArgs) -> anyhow::Result<()> {
    // An invalid config must not block erasing: the default locations are erased then.
    let config = load_config(setup.config_path(), &ProcessEnv)
        .inspect_err(|error| {
            eprintln!(
                "warning: cannot load {} ({error}); erasing the default locations",
                setup.config_path()
            );
        })
        .ok();
    if !args.yes && !confirmed(args)? {
        bail!("not erased");
    }
    let transport = setup.transport();
    request_shutdown(&transport)?;
    let scope = EraseScope {
        config: args.delete_config,
        service: args.service,
    };
    let report = tokenmtr_app::erase(
        setup.paths(),
        &transport,
        setup.config_path(),
        config.as_ref(),
        scope,
        setup.platform().service(),
    )?;
    if report.service_removed {
        println!("background service removed");
    }
    report
        .removed
        .iter()
        .for_each(|path| println!("deleted {path}"));
    println!("exports and the agents' files were left alone");
    Ok(())
}

/// Asks a running daemon to stop. A daemon under launchd exits 0 and is not restarted
/// (`KeepAlive.SuccessfulExit = false`); one started in a terminal just exits. The caller then
/// waits for the daemon lock. A no-op when none runs.
fn request_shutdown(transport: &ControlTransport) -> anyhow::Result<()> {
    if !daemon_running(transport) {
        return Ok(());
    }
    with_daemon(transport, |mut client| async move {
        client
            .shutdown()
            .await
            .context("the daemon refused to stop")
    })
}

fn confirmed(args: &EraseArgs) -> anyhow::Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("not a terminal: pass --yes to erase");
    }
    let mut what = "the store, its backups and the logs".to_owned();
    if args.delete_config {
        what.push_str(", and the config");
    }
    print!("Delete {what}? Type 'erase' to confirm: ");
    std::io::stdout()
        .flush()
        .context("cannot write the prompt")?;
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("cannot read the answer")?;
    Ok(answer.trim() == "erase")
}

/// Arguments of `tokenmtr scan`.
#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Scan one agent only.
    #[arg(long)]
    agent: Option<AgentId>,
    /// Re-read every file instead of the changed ones (needs the daemon stopped).
    #[arg(long)]
    full: bool,
}

/// `tokenmtr scan`: ingests the agents' files now.
pub fn scan(setup: &Setup, args: &ScanArgs) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    if ctx.daemon_running() {
        if args.full {
            bail!("the daemon is running: stop it (`tokenmtr service uninstall`) for a full scan");
        }
        let agent = args.agent;
        let summary = with_daemon(ctx.transport(), |mut client| async move {
            client
                .scan_now(agent)
                .await
                .context("the daemon's scan failed")
        })?;
        for scan in &summary.agents {
            println!(
                "{}: {} files read ({} failed), {} events new, {} updated",
                scan.agent,
                scan.files_parsed,
                scan.files_failed,
                scan.events_inserted,
                scan.events_updated
            );
        }
        println!("took {} ms", summary.elapsed_ms);
        return Ok(());
    }
    let report = ctx.scan_direct(args.agent, args.full)?;
    for scan in &report.agents {
        println!(
            "{}: {} files read ({} failed, {} unchanged), {} events changed",
            scan.agent,
            scan.files_parsed,
            scan.files_failed,
            scan.files_unchanged,
            scan.events.changed()
        );
    }
    println!("took {:.1} s", report.elapsed.as_secs_f64());
    Ok(())
}

/// Arguments of `tokenmtr reprice`.
#[derive(Debug, Args)]
pub struct RepriceArgs {
    /// Re-estimate every event, not only the unpriced ones. Reported costs never change, and an
    /// estimate already from the active snapshot is kept.
    #[arg(long)]
    all: bool,
    /// Print JSON.
    #[arg(long)]
    json: bool,
}

/// What `tokenmtr reprice --json` prints.
#[derive(Debug, Serialize)]
struct RepriceOutput {
    repriced: u64,
}

/// `tokenmtr reprice [--all]`: re-estimates stored events with the active pricing snapshot
///, through the daemon when it runs, else directly.
pub fn reprice(setup: &Setup, args: &RepriceArgs) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    let repriced = if ctx.daemon_running() {
        let all = args.all;
        with_daemon(ctx.transport(), |mut client| async move {
            client
                .reprice(all)
                .await
                .context("the daemon's repricing failed")
        })?
        .repriced
    } else {
        let scope = if args.all {
            RepriceScope::All
        } else {
            RepriceScope::Unpriced
        };
        ctx.reprice_direct(scope)?
    };
    if args.json {
        return print_json(&RepriceOutput { repriced });
    }
    println!("{repriced} events repriced");
    Ok(())
}

/// The format of a report.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum FormatArg {
    /// Markdown.
    Md,
    /// CSV, one table per section.
    Csv,
    /// A single self-contained HTML page.
    Html,
}

/// The currency of a report.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CurrencyArg {
    /// US dollars.
    Usd,
    /// Euros, converted with the ECB rate of each day.
    Eur,
}

/// Arguments of `tokenmtr report`.
#[derive(Debug, Args)]
#[command(group = ArgGroup::new("range").required(true).args(["month", "from"]))]
pub struct ReportArgs {
    /// The month to report, `YYYY-MM`.
    #[arg(long)]
    month: Option<String>,
    /// First day of a custom range (inclusive), `YYYY-MM-DD`.
    #[arg(long, requires = "to")]
    from: Option<Date>,
    /// Last day of a custom range (inclusive), `YYYY-MM-DD`.
    #[arg(long, requires = "from")]
    to: Option<Date>,
    /// Only this project.
    #[arg(long, conflicts_with = "client")]
    project: Option<String>,
    /// Only the projects of this client.
    #[arg(long)]
    client: Option<String>,
    /// Output format.
    #[arg(long, value_enum, default_value = "md")]
    format: FormatArg,
    /// Currency; `display.currency` when absent.
    #[arg(long, value_enum)]
    currency: Option<CurrencyArg>,
    /// Write to this file instead of stdout.
    #[arg(long)]
    out: Option<Utf8PathBuf>,
}

impl ReportArgs {
    fn request(&self) -> anyhow::Result<ReportRequest> {
        let range = match (&self.month, self.from, self.to) {
            (Some(month), _, _) => {
                let first: Date = format!("{month}-01")
                    .parse()
                    .with_context(|| format!("`{month}` is not a month: use YYYY-MM"))?;
                ReportRange::Month(first)
            }
            (None, Some(first), Some(last)) => ReportRange::Custom { first, last },
            _ => bail!("pass --month or --from and --to"),
        };
        let scope = match (&self.project, &self.client) {
            (Some(project), _) => Scope::Project(project.clone()),
            (None, Some(client)) => Scope::Client(client.clone()),
            (None, None) => Scope::All,
        };
        Ok(ReportRequest {
            range,
            scope,
            format: match self.format {
                FormatArg::Md => ReportFormat::Markdown,
                FormatArg::Csv => ReportFormat::Csv,
                FormatArg::Html => ReportFormat::Html,
            },
            currency: self.currency.map(|currency| match currency {
                CurrencyArg::Usd => Currency::Usd,
                CurrencyArg::Eur => Currency::Eur,
            }),
        })
    }
}

/// `tokenmtr report`: a client report for a month or a date range.
pub fn report(setup: &Setup, args: &ReportArgs) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    let request = args.request()?;
    if let Some(path) = &args.out {
        ctx.write_report(now(), &request, path)?;
        println!("wrote {path}");
    } else {
        let rendered = ctx.generate_report(now(), &request)?;
        std::io::stdout()
            .write_all(&rendered.bytes)
            .context("cannot write the report")?;
    }
    Ok(())
}

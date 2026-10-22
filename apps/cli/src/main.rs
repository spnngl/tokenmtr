//! The `tokenmtr` command line.
//!
//! A thin wrapper over `tokenmtr-app` (queries, reports, data actions) and `tokenmtr-control`
//! (asking a running daemon). Exit codes: 0 success, 1 failure, 2 usage error.

mod client;
mod data;
mod format;
mod read;
mod run;
mod service;
mod setup;
mod sources;
mod system;

use anyhow::Context;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};

use crate::setup::Setup;

/// Track and export coding-agent usage.
#[derive(Debug, Parser)]
#[command(name = tokenmtr_core::brand::CLI_NAME, version)]
struct Cli {
    /// Config file; defaults to the platform config location (or `TOKENMTR_CONFIG`).
    #[arg(long, global = true)]
    config: Option<Utf8PathBuf>,
    #[command(subcommand)]
    command: Command,
}

/// One variant per subcommand.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run the daemon in the foreground.
    Run,
    /// Show daemon and sink health and a recap of every followed agent.
    Status(read::StatusArgs),
    /// Show the recap of one agent, or of all agents summed.
    Recap(read::RecapArgs),
    /// Write a client report for a month or a date range.
    Report(data::ReportArgs),
    /// List the alerts that fired.
    Alerts(read::AlertsArgs),
    /// Write a backup now, or list the backups.
    Backup(data::BackupArgs),
    /// Replace the store with a backup: stops the daemon, then restarts the service.
    Restore(data::RestoreArgs),
    /// Delete the store, its backups and the logs.
    Erase(data::EraseArgs),
    /// Ingest the agents' files now.
    Scan(data::ScanArgs),
    /// Re-estimate stored events with the active pricing snapshot.
    Reprice(data::RepriceArgs),
    /// Show the sources, import other machines' exports now, or forget a source's rows.
    Sources(sources::SourcesArgs),
    /// Check the config, the store, the daemon and the agents' files.
    Doctor(system::DoctorArgs),
    /// Install, remove or inspect the background service.
    #[command(subcommand)]
    Service(service::ServiceAction),
    /// Show the latest quota windows and the detected plans, or connect the Keychain (macOS).
    Quota(read::QuotaArgs),
    /// Show the latest FX rate.
    Fx(read::JsonArgs),
    /// Show the active pricing snapshot and the models it cannot price.
    Pricing(read::JsonArgs),
    /// Validate or edit the config file.
    #[command(subcommand)]
    Config(system::ConfigAction),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, Command::Run) {
        return run::run(cli.config.as_deref()).context("daemon failed");
    }
    let setup = Setup::detect(cli.config.as_deref())?;
    match &cli.command {
        Command::Run => Ok(()),
        Command::Status(args) => read::status(&setup, args),
        Command::Recap(args) => read::recap(&setup, args),
        Command::Report(args) => data::report(&setup, args),
        Command::Alerts(args) => read::alerts(&setup, args),
        Command::Backup(args) => data::backup(&setup, args),
        Command::Restore(args) => data::restore(&setup, args),
        Command::Erase(args) => data::erase(&setup, args),
        Command::Scan(args) => data::scan(&setup, args),
        Command::Reprice(args) => data::reprice(&setup, args),
        Command::Sources(args) => sources::sources(&setup, args),
        Command::Doctor(args) => system::doctor(&setup, args),
        Command::Service(action) => service::service(&setup, action),
        Command::Quota(args) => read::quota(&setup, args),
        Command::Fx(args) => read::fx(&setup, args),
        Command::Pricing(args) => read::pricing(&setup, args),
        Command::Config(action) => system::config(&setup, action),
    }
}

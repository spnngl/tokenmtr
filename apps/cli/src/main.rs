//! The `tokenmtr` command line.
//!
//! Today only `run` (the daemon) exists; `Command` is where the other subcommands go.

mod run;

use anyhow::Context;
use clap::{Parser, Subcommand};

/// Track and export coding-agent usage.
#[derive(Debug, Parser)]
#[command(name = "tokenmtr", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// One variant per subcommand.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run the daemon in the foreground.
    Run(run::RunArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run::run(&args).context("daemon failed"),
    }
}

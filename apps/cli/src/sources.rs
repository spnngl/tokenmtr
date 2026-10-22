//! `tokenmtr sources`: the configured `[[sources]]`, importing now, and forgetting a source's rows.

use anyhow::Context;
use clap::{Args, Subcommand};
use serde::Serialize;
use tokenmtr_app::SourceImport;
use tokenmtr_core::SourceId;
use tokenmtr_query::{Period, SourceHealth, SourceKind, SourceState};
use tokenmtr_store::ImportedRows;

use crate::client::with_daemon;
use crate::format::{print_json, timestamp};
use crate::setup::{Setup, now};

/// Arguments of `tokenmtr sources`.
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct SourcesArgs {
    /// `list` when absent.
    #[command(subcommand)]
    action: Option<SourcesAction>,
    /// Print JSON.
    #[arg(long)]
    json: bool,
}

/// What `tokenmtr sources` does.
#[derive(Debug, Subcommand)]
enum SourcesAction {
    /// Show every source with its state and the rows it holds.
    List(JsonArgs),
    /// Import the enabled `sqlite` sources now: through the daemon when it runs, else directly.
    Import(ImportArgs),
    /// Delete the rows imported for a source that the config no longer enables.
    Forget(ForgetArgs),
}

/// A command that only needs `--json`.
#[derive(Debug, Args)]
struct JsonArgs {
    /// Print JSON.
    #[arg(long)]
    json: bool,
}

/// Arguments of `tokenmtr sources import`.
#[derive(Debug, Args)]
struct ImportArgs {
    /// Import this source only.
    source: Option<SourceId>,
    /// Print JSON.
    #[arg(long)]
    json: bool,
}

/// Arguments of `tokenmtr sources forget`.
#[derive(Debug, Args)]
struct ForgetArgs {
    /// The source whose rows to delete.
    source: SourceId,
    /// Print JSON.
    #[arg(long)]
    json: bool,
}

/// What `tokenmtr sources forget --json` prints.
#[derive(Debug, Serialize)]
struct ForgetOutput {
    source: SourceId,
    events: u64,
    tool_calls: u64,
}

/// `tokenmtr sources [list|import|forget]`.
pub fn sources(setup: &Setup, args: &SourcesArgs) -> anyhow::Result<()> {
    match &args.action {
        None => list(setup, args.json),
        Some(SourcesAction::List(list_args)) => list(setup, list_args.json),
        Some(SourcesAction::Import(import_args)) => import(setup, import_args),
        Some(SourcesAction::Forget(forget_args)) => forget(setup, forget_args),
    }
}

fn list(setup: &Setup, json: bool) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    let sources = ctx.status(now(), Period::Today)?.system.sources;
    if json {
        return print_json(&sources);
    }
    if sources.is_empty() {
        println!("no sources (add a [[sources]] table to the config)");
    }
    sources.iter().for_each(render);
    Ok(())
}

/// One line of words for a source''s state.
pub fn state_text(state: &SourceState) -> String {
    match state {
        SourceState::Healthy => "healthy".to_owned(),
        SourceState::Failing { since, last_error } => {
            format!("failing since {}: {last_error}", timestamp(*since))
        }
        SourceState::Disabled => "disabled".to_owned(),
    }
}

fn render(source: &SourceHealth) {
    let kind = match source.kind {
        SourceKind::Superset => "superset",
        SourceKind::Sqlite => "sqlite",
    };
    println!("{} ({kind}): {}", source.source, state_text(&source.state));
    if source.kind == SourceKind::Sqlite {
        println!(
            "  {} files from {} machines, {} events and {} tool calls",
            source.files, source.machines, source.events, source.tool_calls
        );
        if let Some(at) = source.last_success_at {
            println!("  last import {}", timestamp(at));
        }
    }
}

fn import(setup: &Setup, args: &ImportArgs) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    let only = args.source.clone();
    let imports: Vec<SourceImport> = if ctx.daemon_running() {
        with_daemon(ctx.transport(), |mut client| async move {
            client
                .import_now(only)
                .await
                .context("the daemon's import failed")
        })?
        .sources
        .into_iter()
        .map(|summary| SourceImport {
            source: summary.source,
            files_read: summary.files_read,
            rows: summary.rows,
            error: summary.error,
        })
        .collect()
    } else {
        ctx.import_direct(only.as_ref())?
    };
    if args.json {
        return print_json(&imports);
    }
    if imports.is_empty() {
        println!("no enabled sqlite source");
    }
    for import in &imports {
        println!(
            "{}: {} files read, {} rows written",
            import.source, import.files_read, import.rows
        );
        if let Some(error) = &import.error {
            println!("  failed: {error}");
        }
    }
    Ok(())
}

fn forget(setup: &Setup, args: &ForgetArgs) -> anyhow::Result<()> {
    let ctx = setup.context()?;
    let ImportedRows { events, tool_calls } = ctx.forget_source(&args.source)?;
    if args.json {
        return print_json(&ForgetOutput {
            source: args.source.clone(),
            events,
            tool_calls,
        });
    }
    println!(
        "forgot {events} events and {tool_calls} tool calls of {}",
        args.source
    );
    Ok(())
}

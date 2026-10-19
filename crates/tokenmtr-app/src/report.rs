//! Client reports: the query results of one period mapped onto the report crate.

use std::collections::BTreeMap;
use std::str::FromStr;

use camino::Utf8Path;
use jiff::Timestamp;
use jiff::civil::Date;
use tokenmtr_config::Config;
use tokenmtr_core::{Currency, ProjectKey, SubscriptionId};
use tokenmtr_query::{
    BillingFilter, Bucket, ClientName, DateRange, Filters, GroupBy, ProjectGrouping, QueryContext,
    billing, models, overview, projects, series,
};
use tokenmtr_report::{
    Amount, DayInput, ModelInput, Period, ProjectInput, Report, ReportInput, Scope,
    SubscriptionInput, SubscriptionShare, render_csv, render_html, render_markdown,
};
use tokenmtr_store::StoreReader;

use crate::error::AppError;
use crate::settings::query_context;

/// The output format of a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// Markdown.
    Markdown,
    /// CSV, one row per project.
    Csv,
    /// A self-contained HTML page.
    Html,
}

impl ReportFormat {
    /// The file extension, without the dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Csv => "csv",
            Self::Html => "html",
        }
    }
}

/// The days a report covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportRange {
    /// The calendar month containing this date; the label is `YYYY-MM`.
    Month(Date),
    /// From `first` to `last`, both inclusive.
    Custom {
        /// First day.
        first: Date,
        /// Last day.
        last: Date,
    },
}

/// What to report on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRequest {
    /// The days.
    pub range: ReportRange,
    /// Everything, one project (by key) or one client.
    pub scope: Scope,
    /// The output format.
    pub format: ReportFormat,
    /// The report currency; the configured one when `None`. EUR without any FX rate falls back
    /// to USD and the report says which it used.
    pub currency: Option<Currency>,
}

/// A rendered report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedReport {
    /// The format of `bytes`.
    pub format: ReportFormat,
    /// The document, UTF-8.
    pub bytes: Vec<u8>,
    /// A file name for it, such as `tokenmtr-report-2026-05.md`.
    pub suggested_name: String,
}

impl RenderedReport {
    /// Writes the report to `path`, replacing the file atomically.
    ///
    /// # Errors
    ///
    /// [`AppError::Io`] when the file cannot be written.
    pub fn write_to(&self, path: &Utf8Path) -> Result<(), AppError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
        }
        tokenmtr_platform::fs::replace_atomically(path, &self.bytes)
            .map_err(|error| AppError::io(path, error))
    }
}

fn invalid(error: impl std::fmt::Display) -> AppError {
    AppError::InvalidRequest(error.to_string())
}

/// Builds and renders the report of `request` over the store behind `reader`.
///
/// # Errors
///
/// [`AppError::InvalidRequest`] for an empty range or a blank scope name; query and report
/// errors otherwise (a report whose totals disagree is an error, never a wrong document).
pub fn generate_report(
    config: &Config,
    reader: &StoreReader,
    now: Timestamp,
    request: &ReportRequest,
) -> Result<RenderedReport, AppError> {
    let input = report_input(config, reader, now, request)?;
    let scope_name = match &input.scope {
        Scope::All => None,
        Scope::Project(name) | Scope::Client(name) => Some(name.clone()),
    };
    let label = input.period.label.clone();
    let report = Report::from_input(input)?;
    let text = match request.format {
        ReportFormat::Markdown => render_markdown(&report),
        ReportFormat::Csv => render_csv(&report)?,
        ReportFormat::Html => render_html(&report),
    };
    let mut name = format!("tokenmtr-report-{label}");
    if let Some(scope) = scope_name {
        name.push('-');
        name.extend(scope.chars().map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        }));
    }
    name.push('.');
    name.push_str(request.format.extension());
    Ok(RenderedReport {
        format: request.format,
        bytes: text.into_bytes(),
        suggested_name: name,
    })
}

#[derive(Default)]
struct ProjectAcc {
    approximate: bool,
    pay_as_you_go: i64,
    subscriptions: BTreeMap<String, i64>,
}

fn amount(micros: i64) -> Amount {
    u64::try_from(micros).map_or(Amount::ZERO, Amount::from_micros)
}

fn period(range: ReportRange) -> Result<(Period, DateRange), AppError> {
    let (first, last, label) = match range {
        ReportRange::Month(day) => (
            day.first_of_month(),
            day.last_of_month(),
            format!("{:04}-{:02}", day.year(), day.month()),
        ),
        ReportRange::Custom { first, last } => (first, last, format!("{first}..{last}")),
    };
    let end = last.tomorrow().map_err(invalid)?;
    let days = DateRange::new(first, end).map_err(invalid)?;
    Ok((Period { label, first, last }, days))
}

fn report_input(
    config: &Config,
    reader: &StoreReader,
    now: Timestamp,
    request: &ReportRequest,
) -> Result<ReportInput, AppError> {
    let ctx = query_context(config, reader, now, request.currency)?;
    let (period, days) = period(request.range)?;
    let everything = Filters::new(days);
    let scoped = match &request.scope {
        Scope::All => everything.clone(),
        Scope::Project(key) => {
            let key = ProjectKey::from_str(key).map_err(invalid)?;
            everything.clone().projects([key])
        }
        Scope::Client(name) => {
            let name = ClientName::try_from(name.clone()).map_err(invalid)?;
            everything.clone().clients([name])
        }
    };

    let overall = billing(reader, &ctx, &everything)?;
    let in_scope = billing(reader, &ctx, &scoped)?;

    let mut accounts: BTreeMap<(String, Option<String>), ProjectAcc> = BTreeMap::new();
    let mut collect = |filters: &Filters, subscription: Option<&str>| -> Result<(), AppError> {
        for row in projects(reader, &ctx, filters, ProjectGrouping::Project)?.rows {
            let name = row
                .name
                .or(row.project)
                .unwrap_or_else(|| "(unassigned)".to_owned());
            let acc = accounts.entry((name, row.client)).or_default();
            acc.approximate |= row.usage.approximate;
            match subscription {
                None => acc.pay_as_you_go += row.usage.api_value_micros,
                Some(id) => {
                    *acc.subscriptions.entry(id.to_owned()).or_default() +=
                        row.usage.api_value_micros;
                }
            }
        }
        Ok(())
    };
    collect(&scoped.clone().billing(BillingFilter::Api), None)?;
    let mut subscriptions = Vec::new();
    for (line, scoped_line) in overall.subscriptions.iter().zip(&in_scope.subscriptions) {
        let id = SubscriptionId::from_str(&line.id).map_err(invalid)?;
        collect(
            &scoped
                .clone()
                .billing(BillingFilter::Subscription(Some(id))),
            Some(&line.id),
        )?;
        let paid = line.usage.subscription_fee_micros;
        if paid > 0 || line.usage.api_value_micros > 0 {
            subscriptions.push(SubscriptionInput {
                id: line.id.clone(),
                paid: amount(paid),
                outside_scope: amount(
                    line.usage
                        .api_value_micros
                        .saturating_sub(scoped_line.usage.api_value_micros),
                ),
            });
        }
    }

    Ok(ReportInput {
        period,
        scope: request.scope.clone(),
        currency: ctx.currency(),
        generated_at: now,
        sessions: overview(reader, &ctx, &scoped)?.sessions,
        projects: project_inputs(accounts),
        models: model_inputs(reader, &ctx, &scoped)?,
        days: day_inputs(reader, &ctx, &scoped)?,
        subscriptions,
        extra_usage: amount(overall.total.extra_usage_micros),
    })
}

fn project_inputs(accounts: BTreeMap<(String, Option<String>), ProjectAcc>) -> Vec<ProjectInput> {
    accounts
        .into_iter()
        .map(|((name, client), acc)| ProjectInput {
            name,
            client,
            approximate: acc.approximate,
            pay_as_you_go: amount(acc.pay_as_you_go),
            subscriptions: acc
                .subscriptions
                .into_iter()
                .filter(|(_, micros)| *micros > 0)
                .map(|(subscription, micros)| SubscriptionShare {
                    subscription,
                    api_value: amount(micros),
                })
                .collect(),
        })
        .collect()
}

fn model_inputs(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<Vec<ModelInput>, AppError> {
    Ok(models(reader, ctx, filters)?
        .rows
        .into_iter()
        .map(|row| ModelInput {
            model: row.model,
            tokens: row.usage.total_tokens,
            api_value: amount(row.usage.api_value_micros),
            approximate: row.usage.approximate,
        })
        .collect())
}

fn day_inputs(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<Vec<DayInput>, AppError> {
    Ok(series(reader, ctx, filters, Bucket::Day, GroupBy::None)?
        .points
        .into_iter()
        .filter(|point| point.usage.events > 0)
        .map(|point| DayInput {
            date: point.start,
            api_value: amount(point.usage.api_value_micros),
            approximate: point.usage.approximate,
        })
        .collect())
}

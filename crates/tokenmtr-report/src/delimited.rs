//! The CSV renderer: one table, a `section` column tells the row kinds apart.

use std::io;
use std::string::FromUtf8Error;

use crate::format::{code, exact_amount};
use crate::report::Report;

/// Why a report could not be written as CSV.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The CSV writer failed.
    #[error("writing CSV: {0}")]
    Csv(#[from] csv::Error),
    /// Flushing the buffer failed.
    #[error("flushing CSV: {0}")]
    Io(#[from] io::Error),
    /// The output was not UTF-8.
    #[error("CSV is not UTF-8: {0}")]
    Utf8(#[from] FromUtf8Error),
}

/// Header of the CSV table.
pub const CSV_HEADER: [&str; 9] = [
    "section",
    "name",
    "client",
    "sessions",
    "tokens",
    "api_value",
    "paid",
    "approximate",
    "currency",
];

/// Neutralizes text a spreadsheet would run as a formula.
fn safe(text: &str) -> String {
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_owned()
    }
}

/// Renders the report as CSV with amounts at six decimals and no currency symbol.
///
/// Sections: `total` (one row), `project`, `model`, `day`. Columns that do not apply stay empty.
///
/// # Errors
///
/// [`RenderError`] when the writer fails.
pub fn render_csv(report: &Report) -> Result<String, RenderError> {
    let currency = code(report.currency);
    let flag = |approximate: bool| approximate.to_string();
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(CSV_HEADER)?;
    let totals = &report.totals;
    writer.write_record([
        "total",
        &safe(&report.period.label),
        "",
        &report.sessions.to_string(),
        &totals.tokens.to_string(),
        &exact_amount(totals.api_value),
        &exact_amount(totals.paid),
        &flag(totals.approximate),
        currency,
    ])?;
    for project in &report.projects {
        writer.write_record([
            "project",
            &safe(&project.name),
            &project.client.as_deref().map(safe).unwrap_or_default(),
            "",
            "",
            &exact_amount(project.api_value),
            &exact_amount(project.paid),
            &flag(project.approximate),
            currency,
        ])?;
    }
    for model in &report.models {
        writer.write_record([
            "model",
            &safe(&model.model),
            "",
            "",
            &model.tokens.to_string(),
            &exact_amount(model.api_value),
            "",
            &flag(model.approximate),
            currency,
        ])?;
    }
    for day in &report.days {
        writer.write_record([
            "day",
            &day.date.to_string(),
            "",
            "",
            "",
            &exact_amount(day.api_value),
            "",
            &flag(day.approximate),
            currency,
        ])?;
    }
    let bytes = writer
        .into_inner()
        .map_err(csv::IntoInnerError::into_error)?;
    Ok(String::from_utf8(bytes)?)
}

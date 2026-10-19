//! The Markdown renderer.

use std::fmt::Write as _;

use crate::format::{APPROXIMATE_MARK, display_amount, instant, scope_label};
use crate::report::Report;

/// Escapes text for a Markdown table cell or heading.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '|' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// Renders the report as Markdown.
#[must_use]
pub fn render_markdown(report: &Report) -> String {
    let currency = report.currency;
    let money = |amount, approximate| display_amount(amount, currency, approximate);
    let mut out = String::new();
    let _ = writeln!(out, "# Usage report {}\n", escape(&report.period.label));
    let _ = writeln!(
        out,
        "{} · {} to {} · {}\n",
        escape(&scope_label(&report.scope)),
        report.period.first,
        report.period.last,
        crate::format::code(currency),
    );

    let totals = &report.totals;
    let _ = writeln!(out, "| Total | |\n| --- | ---: |");
    let _ = writeln!(
        out,
        "| API value | {} |",
        money(totals.api_value, totals.approximate)
    );
    let _ = writeln!(out, "| Paid | {} |", money(totals.paid, totals.approximate));
    let _ = writeln!(out, "| Sessions | {} |", report.sessions);
    let _ = writeln!(out, "| Tokens | {} |\n", totals.tokens);

    let _ = writeln!(out, "## Projects\n");
    let _ = writeln!(
        out,
        "| Project | Client | API value | Paid |\n| --- | --- | ---: | ---: |"
    );
    for project in &report.projects {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            escape(&project.name),
            project.client.as_deref().map(escape).unwrap_or_default(),
            money(project.api_value, project.approximate),
            money(project.paid, project.approximate),
        );
    }

    let _ = writeln!(out, "\n## Models\n");
    let _ = writeln!(out, "| Model | Tokens | API value |\n| --- | ---: | ---: |");
    for model in &report.models {
        let _ = writeln!(
            out,
            "| {} | {} | {} |",
            escape(&model.model),
            model.tokens,
            money(model.api_value, model.approximate),
        );
    }

    let _ = writeln!(out, "\n## Days\n");
    let _ = writeln!(out, "| Day | API value |\n| --- | ---: |");
    for day in &report.days {
        let _ = writeln!(
            out,
            "| {} | {} |",
            day.date,
            money(day.api_value, day.approximate)
        );
    }

    out.push('\n');
    if report.unused_paid.micros() > 0 {
        let _ = writeln!(
            out,
            "Subscription fees nobody used: {}.\n",
            money(report.unused_paid, false)
        );
    }
    if report.extra_usage.micros() > 0 {
        let _ = writeln!(
            out,
            "Claude extra usage, in Paid: {}.\n",
            money(report.extra_usage, false)
        );
    }
    if totals.approximate {
        let _ = writeln!(out, "{APPROXIMATE_MARK} includes approximate events.\n");
    }
    let _ = writeln!(out, "Generated {}.", instant(report.generated_at));
    out
}

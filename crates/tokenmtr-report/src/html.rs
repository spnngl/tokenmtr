//! The HTML renderer: one self-contained, print-styled page.

use std::fmt::Write as _;

use crate::format::{APPROXIMATE_MARK, code, display_amount, instant, scope_label};
use crate::report::Report;

const STYLE: &str = "\
body{font:14px/1.45 -apple-system,system-ui,sans-serif;color:#1b1b1f;max-width:48rem;margin:2rem auto;padding:0 1rem}\
h1{font-size:1.6rem;margin:0 0 .25rem}h2{font-size:1.1rem;margin:2rem 0 .5rem}\
.meta,.note{color:#5b5b66}\
table{border-collapse:collapse;width:100%}\
th,td{padding:.3rem .5rem;border-bottom:1px solid #d8d8de;text-align:left}\
th{font-weight:600}td.n,th.n{text-align:right;font-variant-numeric:tabular-nums}\
@media print{body{margin:0;max-width:none}h2{break-after:avoid}tr{break-inside:avoid}}";

/// Escapes text for HTML element content and attribute values.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Renders the report as a standalone HTML page: inline CSS, no scripts, no external assets.
#[must_use]
pub fn render_html(report: &Report) -> String {
    let currency = report.currency;
    let money = |amount, approximate| escape(&display_amount(amount, currency, approximate));
    let title = format!("Usage report {}", report.period.label);
    let mut out = String::new();
    let _ = write!(
        out,
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>{}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n",
        escape(&title)
    );
    let _ = writeln!(out, "<h1>{}</h1>", escape(&title));
    let _ = writeln!(
        out,
        "<p class=\"meta\">{} · {} to {} · {}</p>",
        escape(&scope_label(&report.scope)),
        report.period.first,
        report.period.last,
        code(currency),
    );

    let totals = &report.totals;
    let _ = writeln!(out, "<table>\n<tbody>");
    for (label, value) in [
        ("API value", money(totals.api_value, totals.approximate)),
        ("Paid", money(totals.paid, totals.approximate)),
        ("Sessions", report.sessions.to_string()),
        ("Tokens", totals.tokens.to_string()),
    ] {
        let _ = writeln!(out, "<tr><th>{label}</th><td class=\"n\">{value}</td></tr>");
    }
    let _ = writeln!(out, "</tbody>\n</table>");

    let _ = writeln!(out, "<h2>Projects</h2>\n<table>");
    let _ = writeln!(
        out,
        "<thead><tr><th>Project</th><th>Client</th><th class=\"n\">API value</th>\
         <th class=\"n\">Paid</th></tr></thead>\n<tbody>"
    );
    for project in &report.projects {
        let _ = writeln!(
            out,
            "<tr><td>{}</td><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
            escape(&project.name),
            project.client.as_deref().map(escape).unwrap_or_default(),
            money(project.api_value, project.approximate),
            money(project.paid, project.approximate),
        );
    }
    let _ = writeln!(out, "</tbody>\n</table>");

    let _ = writeln!(out, "<h2>Models</h2>\n<table>");
    let _ = writeln!(
        out,
        "<thead><tr><th>Model</th><th class=\"n\">Tokens</th><th class=\"n\">API value</th></tr></thead>\n<tbody>"
    );
    for model in &report.models {
        let _ = writeln!(
            out,
            "<tr><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
            escape(&model.model),
            model.tokens,
            money(model.api_value, model.approximate),
        );
    }
    let _ = writeln!(out, "</tbody>\n</table>");

    let _ = writeln!(out, "<h2>Days</h2>\n<table>");
    let _ = writeln!(
        out,
        "<thead><tr><th>Day</th><th class=\"n\">API value</th></tr></thead>\n<tbody>"
    );
    for day in &report.days {
        let _ = writeln!(
            out,
            "<tr><td>{}</td><td class=\"n\">{}</td></tr>",
            day.date,
            money(day.api_value, day.approximate),
        );
    }
    let _ = writeln!(out, "</tbody>\n</table>");

    if report.unused_paid.micros() > 0 {
        let _ = writeln!(
            out,
            "<p class=\"note\">Subscription fees nobody used: {}.</p>",
            money(report.unused_paid, false)
        );
    }
    if totals.approximate {
        let _ = writeln!(
            out,
            "<p class=\"note\">{APPROXIMATE_MARK} includes approximate events.</p>"
        );
    }
    let _ = writeln!(
        out,
        "<p class=\"note\">Generated {}.</p>\n</body>\n</html>",
        instant(report.generated_at)
    );
    out
}

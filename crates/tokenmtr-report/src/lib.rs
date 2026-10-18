//! Period reports (D46) as Markdown, CSV or HTML.
//!
//! Three steps, all pure:
//!
//! 1. Fill a [`ReportInput`] (a later phase does this from `tokenmtr-query`).
//! 2. [`Report::from_input`] validates it and allocates each subscription's fee over the projects
//!    that drew from it, in proportion to their API value. The allocated fees add up exactly to
//!    the fees paid.
//! 3. Render with [`render_markdown`], [`render_csv`] or [`render_html`].
//!
//! Amounts are non-negative [`Amount`]s in the report's currency; this crate never converts.
//! A `~` before a figure marks one that includes approximate events.

mod delimited;
mod format;
mod html;
mod input;
mod markdown;
mod report;

pub use delimited::{CSV_HEADER, RenderError, render_csv};
pub use html::render_html;
pub use input::{
    Amount, DayInput, ModelInput, Period, ProjectInput, ReportInput, Scope, SubscriptionInput,
    SubscriptionShare,
};
pub use markdown::render_markdown;
pub use report::{DayRow, ModelRow, ProjectRow, Report, ReportError, Totals, allocate};

//! The export schema: the one place that names every exported column.
//!
//! CSV headers, JSONL keys, `SQLite` DDL and `docs/export-schema.md` are all generated from
//! [`schema`]. A column added here appears everywhere; there is no second list to update.
//!
//! Layout rules every dataset follows, enforced by tests:
//!
//! - column 0 is the dataset's primary key (the row id sinks are idempotent on);
//! - column 1 is `revision`;
//! - [`DatasetSchema::partition_column`], when set, is a [`ColumnType::Timestamp`] or
//!   [`ColumnType::Date`] column that never changes for a given id, so a row never moves between
//!   partition files;
//! - every redactable column is nullable: `drop` empties it, which is an empty CSV field,
//!   JSON `null` and SQL `NULL` alike.

use std::fmt::Write as _;

use std::borrow::Cow;

use jiff::civil::Date;
use rust_decimal::Decimal;
use tokenmtr_config::RedactColumn;
use tokenmtr_store::{Dataset, Revision};

/// Version of the export schema; written to the `SQLite` sink's `_schema` table and bumped on any
/// column change.
pub const EXPORT_SCHEMA_VERSION: u32 = 3;

/// The fixed number of fractional digits of a [`ColumnType::Decimal`] column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// Six digits: money.
    Micros,
    /// Nine digits: FX rates.
    Nanos,
}

impl Scale {
    /// Fractional digits.
    #[must_use]
    pub const fn digits(self) -> u32 {
        match self {
            Self::Micros => 6,
            Self::Nanos => 9,
        }
    }

    /// Suffix of the integer column that holds the value in SQL sinks, e.g. `_micros`.
    #[must_use]
    pub const fn sql_suffix(self) -> &'static str {
        match self {
            Self::Micros => "_micros",
            Self::Nanos => "_nanos",
        }
    }
}

/// The type of an exported column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    /// UTF-8 text.
    Text,
    /// Signed 64-bit integer.
    Integer,
    /// `true` or `false` (`SQLite`: 0 or 1).
    Boolean,
    /// Floating point number.
    Real,
    /// Fixed-point decimal. Text sinks write the decimal; SQL sinks store the exact integer
    /// count of `10^-digits` units in a column named with [`Scale::sql_suffix`].
    Decimal(Scale),
    /// ISO-8601 UTC instant, e.g. `2026-05-01T12:00:00Z`.
    Timestamp,
    /// ISO-8601 civil date, e.g. `2026-05-01`.
    Date,
}

impl ColumnType {
    /// The `SQLite` column type in the generated DDL (tables are `STRICT`).
    #[must_use]
    pub const fn sql(self) -> &'static str {
        match self {
            Self::Text | Self::Timestamp | Self::Date => "TEXT",
            Self::Integer | Self::Boolean | Self::Decimal(_) => "INTEGER",
            Self::Real => "REAL",
        }
    }

    /// The type as shown in `docs/export-schema.md`.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Text => "text".to_owned(),
            Self::Integer => "integer".to_owned(),
            Self::Boolean => "boolean".to_owned(),
            Self::Real => "real".to_owned(),
            Self::Decimal(scale) => format!(
                "decimal({}); SQL: integer `*{}`",
                scale.digits(),
                scale.sql_suffix()
            ),
            Self::Timestamp => "timestamp (UTC)".to_owned(),
            Self::Date => "date".to_owned(),
        }
    }
}

/// One exported column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// Name in the CSV header, JSONL object and `SQLite` table.
    pub name: &'static str,
    /// Value type.
    pub ty: ColumnType,
    /// Whether the value can be empty (CSV empty field, JSON `null`, SQL `NULL`).
    pub nullable: bool,
    /// The per-sink redaction name that applies to this column, when it is redactable.
    pub redact: Option<RedactColumn>,
    /// One-line meaning, shown in `docs/export-schema.md`.
    pub doc: &'static str,
}

impl Column {
    /// The column name in SQL sinks: [`Column::name`], plus the unit suffix for decimals.
    #[must_use]
    pub fn sql_name(&self) -> Cow<'static, str> {
        match self.ty {
            ColumnType::Decimal(scale) => {
                Cow::Owned(format!("{}{}", self.name, scale.sql_suffix()))
            }
            _ => Cow::Borrowed(self.name),
        }
    }
}

const fn col(name: &'static str, ty: ColumnType, doc: &'static str) -> Column {
    Column {
        name,
        ty,
        nullable: false,
        redact: None,
        doc,
    }
}

const fn opt(name: &'static str, ty: ColumnType, doc: &'static str) -> Column {
    Column {
        name,
        ty,
        nullable: true,
        redact: None,
        doc,
    }
}

const fn redactable(mut column: Column, redact: RedactColumn) -> Column {
    column.redact = Some(redact);
    column
}

const REVISION: Column = col(
    "revision",
    ColumnType::Integer,
    "Change counter of the row within its dataset; the highest revision of an id is current.",
);

const USD: ColumnType = ColumnType::Decimal(Scale::Micros);
const COUNT: ColumnType = ColumnType::Integer;

static USAGE_EVENTS: [Column; 29] = [
    col(
        "event_id",
        ColumnType::Text,
        "Primary key: 32 hex chars, stable per machine, agent and dedup key.",
    ),
    REVISION,
    col(
        "machine_id",
        ColumnType::Text,
        "UUID of the machine that ingested the event.",
    ),
    col(
        "agent",
        ColumnType::Text,
        "Agent id, e.g. `claude` or `pi`.",
    ),
    col("profile", ColumnType::Text, "Agent profile name."),
    opt("provider", ColumnType::Text, "Model provider, when known."),
    col(
        "model",
        ColumnType::Text,
        "Model id as the agent reported it.",
    ),
    opt(
        "service_tier",
        ColumnType::Text,
        "Provider service tier, when reported.",
    ),
    redactable(
        opt(
            "session_id",
            ColumnType::Text,
            "Agent session id; empty only when dropped.",
        ),
        RedactColumn::SessionId,
    ),
    redactable(
        opt(
            "parent_session_id",
            ColumnType::Text,
            "Session that spawned this one (subagents).",
        ),
        RedactColumn::SessionId,
    ),
    redactable(
        opt("project", ColumnType::Text, "Resolved project key."),
        RedactColumn::Project,
    ),
    redactable(
        opt("client", ColumnType::Text, "Client label of the project."),
        RedactColumn::Client,
    ),
    redactable(
        opt("cwd", ColumnType::Text, "Working directory of the session."),
        RedactColumn::Cwd,
    ),
    redactable(
        opt(
            "git_branch",
            ColumnType::Text,
            "Git branch at the event. Claude logs it; pi does not, so a pi event takes its directory's current branch when it is at most two scan intervals old, else the last branch Superset recorded for that worktree.",
        ),
        RedactColumn::GitBranch,
    ),
    col(
        "at_utc",
        ColumnType::Timestamp,
        "When the event happened; partitions files.",
    ),
    col(
        "kind",
        ColumnType::Text,
        "`message`, `tool_result`, `compaction`, `branch_summary`, `auxiliary`, `daily_aggregate` or `extra_usage` (one row per profile and local day of Claude extra-usage spend, in `cost_reported_usd`, no tokens).",
    ),
    col(
        "approximate",
        ColumnType::Boolean,
        "True when the row is an estimate, e.g. an imported daily aggregate.",
    ),
    col(
        "input_uncached",
        COUNT,
        "Input tokens not served from cache.",
    ),
    col("cache_read", COUNT, "Input tokens read from cache."),
    col(
        "cache_write_5m",
        COUNT,
        "Input tokens written to the 5-minute cache.",
    ),
    col(
        "cache_write_1h",
        COUNT,
        "Input tokens written to the 1-hour cache.",
    ),
    col("output", COUNT, "Output tokens."),
    col(
        "reasoning",
        COUNT,
        "Reasoning tokens, when reported separately.",
    ),
    col(
        "total_tokens",
        COUNT,
        "input_uncached + cache_read + cache_write_5m + cache_write_1h + output; reasoning is part of output.",
    ),
    opt("cost_reported_usd", USD, "Cost the agent reported, in USD."),
    opt(
        "cost_estimated_usd",
        USD,
        "Pay-as-you-go cost estimated from `pricing_version`, in USD.",
    ),
    col(
        "cost_source",
        ColumnType::Text,
        "Which cost is the API value: `reported`, `estimated` or `unpriced`.",
    ),
    opt(
        "pricing_version",
        ColumnType::Text,
        "Pricing snapshot behind `cost_estimated_usd`.",
    ),
    col(
        "superseded",
        ColumnType::Boolean,
        "True for a `daily_aggregate` on a day transcript events now cover; exclude it from totals.",
    ),
];

static TOOL_CALLS: [Column; 11] = [
    col(
        "tool_call_id",
        ColumnType::Text,
        "Primary key: 32 hex chars, stable per machine, agent and tool-use id.",
    ),
    REVISION,
    col(
        "machine_id",
        ColumnType::Text,
        "UUID of the machine that ingested the call.",
    ),
    col("agent", ColumnType::Text, "Agent id."),
    col("profile", ColumnType::Text, "Agent profile name."),
    redactable(
        opt(
            "session_id",
            ColumnType::Text,
            "Agent session id; empty only when dropped.",
        ),
        RedactColumn::SessionId,
    ),
    opt(
        "event_id",
        ColumnType::Text,
        "`event_id` of the usage event of the same message, when known.",
    ),
    col(
        "at_utc",
        ColumnType::Timestamp,
        "When the tool was invoked; partitions files.",
    ),
    col(
        "tool_name",
        ColumnType::Text,
        "Tool name; for MCP tools the part after the server.",
    ),
    opt(
        "mcp_server",
        ColumnType::Text,
        "MCP server name for MCP tools.",
    ),
    opt(
        "is_error",
        ColumnType::Boolean,
        "Whether the result was an error; empty until a result is seen.",
    ),
];

static SUBSCRIPTIONS: [Column; 13] = [
    col(
        "subscription_id",
        ColumnType::Text,
        "Primary key: the id from config.",
    ),
    REVISION,
    col(
        "machine_id",
        ColumnType::Text,
        "UUID of the machine that wrote the row.",
    ),
    col(
        "provider",
        ColumnType::Text,
        "Provider whose traffic the plan covers.",
    ),
    col(
        "plan",
        ColumnType::Text,
        "`pro`, `max_5x`, `max_20x`, `team`, `enterprise` or `custom`.",
    ),
    col(
        "agents",
        ColumnType::Text,
        "Covered agent ids, comma-separated.",
    ),
    opt(
        "profiles",
        ColumnType::Text,
        "Covered profiles, comma-separated; empty means all.",
    ),
    col("fee", USD, "Fee per billing period, in `fee_currency`."),
    col("fee_currency", ColumnType::Text, "`USD` or `EUR`."),
    col("period", ColumnType::Text, "`monthly` or `yearly`."),
    col("start", ColumnType::Date, "First covered day (inclusive)."),
    opt(
        "end",
        ColumnType::Date,
        "First day no longer covered (exclusive); empty while open.",
    ),
    opt(
        "removed_at_utc",
        ColumnType::Timestamp,
        "When the subscription left the config; it bills nothing since. Empty while configured.",
    ),
];

static QUOTA_SNAPSHOTS: [Column; 14] = [
    col(
        "snapshot_id",
        ColumnType::Text,
        "Primary key: 32 hex chars, stable per machine, agent, profile, window and poll time.",
    ),
    REVISION,
    col(
        "machine_id",
        ColumnType::Text,
        "UUID of the machine that polled.",
    ),
    col("agent", ColumnType::Text, "Agent id."),
    col("profile", ColumnType::Text, "Agent profile name."),
    redactable(
        opt(
            "account",
            ColumnType::Text,
            "Account email or organization.",
        ),
        RedactColumn::Account,
    ),
    col(
        "window_id",
        ColumnType::Text,
        "Quota window id, e.g. `five_hour`; `''` (never NULL) when the poll failed.",
    ),
    opt(
        "window_label",
        ColumnType::Text,
        "Human name of the window, e.g. `5-hour`.",
    ),
    opt(
        "used_percent",
        ColumnType::Real,
        "Percent of the window used; may exceed 100.",
    ),
    opt(
        "resets_at_utc",
        ColumnType::Timestamp,
        "When the window resets, when the provider says.",
    ),
    col(
        "fetched_at_utc",
        ColumnType::Timestamp,
        "When the poll happened; partitions files.",
    ),
    col(
        "status",
        ColumnType::Text,
        "`ok`, `signed_out`, `token_expired`, `keychain_denied` or `failed`.",
    ),
    opt(
        "spent_usd",
        USD,
        "Spent this month, in USD; only on the `extra_usage` window (Claude pay-as-you-go spend).",
    ),
    opt(
        "limit_usd",
        USD,
        "Monthly cap, in USD; only on the `extra_usage` window.",
    ),
];

static FX_RATES: [Column; 4] = [
    col(
        "date",
        ColumnType::Date,
        "Primary key: publication date of the ECB reference rate; partitions files.",
    ),
    REVISION,
    col(
        "eur_per_usd",
        ColumnType::Decimal(Scale::Nanos),
        "EUR for 1 USD.",
    ),
    col("source", ColumnType::Text, "Rate source: `ecb`."),
];

/// The columns of one dataset and how its files are partitioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatasetSchema {
    /// The dataset.
    pub dataset: Dataset,
    /// Columns in export order.
    pub columns: &'static [Column],
    /// The date column that decides a row's partition file, or `None` for a dataset kept in one
    /// file because its date can change (subscriptions).
    pub partition_column: Option<&'static str>,
}

impl DatasetSchema {
    /// The primary key column.
    #[must_use]
    pub fn key(&self) -> &'static Column {
        &self.columns[0]
    }

    /// Index of the `revision` column.
    pub const REVISION_INDEX: usize = 1;

    /// Column names in export order.
    pub fn names(&self) -> impl Iterator<Item = &'static str> {
        self.columns.iter().map(|c| c.name)
    }
}

static SCHEMAS: [DatasetSchema; 5] = [
    DatasetSchema {
        dataset: Dataset::UsageEvents,
        columns: &USAGE_EVENTS,
        partition_column: Some("at_utc"),
    },
    DatasetSchema {
        dataset: Dataset::ToolCalls,
        columns: &TOOL_CALLS,
        partition_column: Some("at_utc"),
    },
    DatasetSchema {
        dataset: Dataset::Subscriptions,
        columns: &SUBSCRIPTIONS,
        partition_column: None,
    },
    DatasetSchema {
        dataset: Dataset::QuotaSnapshots,
        columns: &QUOTA_SNAPSHOTS,
        partition_column: Some("fetched_at_utc"),
    },
    DatasetSchema {
        dataset: Dataset::FxRates,
        columns: &FX_RATES,
        partition_column: Some("date"),
    },
];

/// The schema of `dataset`.
#[must_use]
pub fn schema(dataset: Dataset) -> &'static DatasetSchema {
    let index = match dataset {
        Dataset::UsageEvents => 0,
        Dataset::ToolCalls => 1,
        Dataset::Subscriptions => 2,
        Dataset::QuotaSnapshots => 3,
        Dataset::FxRates => 4,
    };
    &SCHEMAS[index]
}

/// One exported cell.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Empty: CSV empty field, JSON `null`, SQL `NULL`.
    Null,
    /// Text, timestamps and dates.
    Text(String),
    /// Integer.
    Integer(i64),
    /// Boolean.
    Boolean(bool),
    /// Floating point.
    Real(f64),
    /// Fixed-point decimal: `units` of `10^-scale.digits()`.
    Decimal {
        /// The exact integer count of units.
        units: i64,
        /// The unit.
        scale: Scale,
    },
}

impl Value {
    /// The cell as plain text, as CSV writes it; `None` for [`Value::Null`].
    #[must_use]
    pub fn to_plain(&self) -> Option<String> {
        match self {
            Self::Null => None,
            Self::Text(text) => Some(text.clone()),
            Self::Integer(n) => Some(n.to_string()),
            Self::Boolean(b) => Some(b.to_string()),
            Self::Real(x) => Some(x.to_string()),
            Self::Decimal { units, scale } => {
                Some(Decimal::new(*units, scale.digits()).to_string())
            }
        }
    }
}

/// One dataset row ready for a sink: values aligned with [`DatasetSchema::columns`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExportRow {
    /// The row's revision (also in `values[1]`).
    pub revision: Revision,
    /// UTC date that selects the partition file.
    pub partition_date: Date,
    /// One value per schema column.
    pub values: Vec<Value>,
}

impl ExportRow {
    /// The primary key as text.
    #[must_use]
    pub fn id(&self) -> String {
        self.values
            .first()
            .and_then(Value::to_plain)
            .unwrap_or_default()
    }
}

/// Renders `docs/export-schema.md`'s column tables from [`schema`]. A test keeps the committed
/// file equal to this output.
#[must_use]
pub fn schema_markdown() -> String {
    let mut out = String::new();
    for dataset in Dataset::ALL {
        let schema = schema(dataset);
        let _ = writeln!(out, "### `{dataset}`\n");
        let _ = writeln!(
            out,
            "Primary key: `{}`. Partitioned by: {}.\n",
            schema.key().name,
            schema
                .partition_column
                .map_or_else(|| "nothing (one file)".to_owned(), |c| format!("`{c}`")),
        );
        let _ = writeln!(out, "| Column | Type | Empty | Redact | Meaning |");
        let _ = writeln!(out, "| --- | --- | --- | --- | --- |");
        for column in schema.columns {
            let redact = column
                .redact
                .map_or_else(String::new, |r| format!("`{}`", redact_name(r)));
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} |",
                column.name,
                column.ty.label(),
                if column.nullable { "yes" } else { "no" },
                redact,
                column.doc,
            );
        }
        out.push('\n');
    }
    out
}

/// The config name of a redactable column.
#[must_use]
pub const fn redact_name(column: RedactColumn) -> &'static str {
    match column {
        RedactColumn::Cwd => "cwd",
        RedactColumn::Project => "project",
        RedactColumn::Client => "client",
        RedactColumn::GitBranch => "git_branch",
        RedactColumn::Account => "account",
        RedactColumn::SessionId => "session_id",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dataset_follows_the_layout_rules() {
        for dataset in Dataset::ALL {
            let schema = schema(dataset);
            assert_eq!(schema.dataset, dataset);
            assert!(!schema.key().nullable, "{dataset} key is nullable");
            assert_eq!(
                schema.columns[DatasetSchema::REVISION_INDEX].name,
                "revision"
            );
            if let Some(name) = schema.partition_column {
                let column = schema.columns.iter().find(|c| c.name == name);
                assert!(
                    column
                        .is_some_and(|c| matches!(c.ty, ColumnType::Timestamp | ColumnType::Date)
                            && !c.nullable),
                    "{dataset} partition column {name} must be a required date or timestamp"
                );
            }
            for column in schema.columns {
                assert!(
                    column.redact.is_none() || column.nullable,
                    "{dataset}.{} is redactable, so `drop` can empty it",
                    column.name
                );
            }
            let mut names: Vec<_> = schema.names().collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(
                names.len(),
                schema.columns.len(),
                "{dataset} repeats a column"
            );
            let mut sql_names: Vec<_> = schema.columns.iter().map(Column::sql_name).collect();
            sql_names.sort_unstable();
            sql_names.dedup();
            assert_eq!(
                sql_names.len(),
                schema.columns.len(),
                "{dataset} repeats a SQL column"
            );
        }
    }
}

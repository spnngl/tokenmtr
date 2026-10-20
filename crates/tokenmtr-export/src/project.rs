//! Maps store rows to export rows: the units and spellings of the export schema.
//!
//! Money leaves the store as integer `*_usd_micros` and is exported as decimals with six
//! fractional digits; instants leave as epoch microseconds and are exported as ISO-8601 UTC.

use jiff::Timestamp;
use jiff::civil::Date;
use tokenmtr_core::CostSource;
use tokenmtr_core::{
    BillingPeriod, Currency, EventId, EventKind, MachineId, Plan, SkillName, UsdMicros,
    fx::utc_date,
};
use tokenmtr_store::{
    Dataset, FxRateRow, FxSource, ProfiledToolCall, QuotaSample, QuotaStatus, Revision, StoreRead,
    StoredEvent, StoredSubscription, Versioned,
};

use crate::error::ExportError;
use crate::schema::{ExportRow, Scale, Value, schema};
use crate::sink::ExportBatch;

/// Reads the next batch of `dataset` after `after` (at most `limit` rows, revision order) and maps
/// it to export rows. Redaction is not applied here.
///
/// # Errors
/// [`ExportError::Store`] when the read fails; [`ExportError::Id`] when a row has no export id.
pub fn fetch_batch(
    store: &impl StoreRead,
    dataset: Dataset,
    after: Revision,
    limit: usize,
) -> Result<ExportBatch, ExportError> {
    let machine = store.machine_id();
    let rows = match dataset {
        Dataset::UsageEvents => store
            .stored_events_after(after, limit)?
            .iter()
            .map(|r| usage_event(&machine, r))
            .collect(),
        Dataset::ToolCalls => store
            .tool_calls_after(after, limit)?
            .iter()
            .map(|r| tool_call(&machine, r))
            .collect::<Result<_, _>>()?,
        Dataset::Subscriptions => store
            .subscriptions_after(after, limit)?
            .iter()
            .map(|r| subscription(&machine, r))
            .collect(),
        Dataset::QuotaSnapshots => store
            .quota_snapshots_after(after, limit)?
            .iter()
            .map(|r| quota_snapshot(&machine, r))
            .collect::<Result<_, _>>()?,
        Dataset::FxRates => store
            .fx_rates_after(after, limit)?
            .iter()
            .map(fx_rate)
            .collect(),
    };
    Ok(ExportBatch { dataset, rows })
}

fn text(value: impl Into<String>) -> Value {
    Value::Text(value.into())
}

fn opt_text(value: Option<impl Into<String>>) -> Value {
    value.map_or(Value::Null, |v| Value::Text(v.into()))
}

fn count(value: u64) -> Value {
    Value::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

fn revision(revision: Revision) -> Value {
    count(revision.get())
}

fn usd(micros: UsdMicros) -> Value {
    Value::Decimal {
        units: micros.get(),
        scale: Scale::Micros,
    }
}

fn instant(at: Timestamp) -> Value {
    Value::Text(at.to_string())
}

fn date_text(date: Date) -> Value {
    Value::Text(date.to_string())
}

const fn kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Message => "message",
        EventKind::ToolResult => "tool_result",
        EventKind::Compaction => "compaction",
        EventKind::BranchSummary => "branch_summary",
        EventKind::Auxiliary => "auxiliary",
        EventKind::DailyAggregate => "daily_aggregate",
        EventKind::ExtraUsage => "extra_usage",
    }
}

const fn cost_source_name(source: CostSource) -> &'static str {
    match source {
        CostSource::Reported => "reported",
        CostSource::Estimated => "estimated",
        CostSource::Unpriced => "unpriced",
    }
}

const fn plan_name(plan: Plan) -> &'static str {
    match plan {
        Plan::Pro => "pro",
        Plan::Max5x => "max_5x",
        Plan::Max20x => "max_20x",
        Plan::Team => "team",
        Plan::Enterprise => "enterprise",
        Plan::Custom => "custom",
    }
}

const fn period_name(period: BillingPeriod) -> &'static str {
    match period {
        BillingPeriod::Monthly => "monthly",
        BillingPeriod::Yearly => "yearly",
    }
}

const fn currency_name(currency: Currency) -> &'static str {
    match currency {
        Currency::Usd => "USD",
        Currency::Eur => "EUR",
    }
}

const fn status_name(status: QuotaStatus) -> &'static str {
    match status {
        QuotaStatus::Ok => "ok",
        QuotaStatus::SignedOut => "signed_out",
        QuotaStatus::TokenExpired => "token_expired",
        QuotaStatus::KeychainDenied => "keychain_denied",
        QuotaStatus::Failed => "failed",
    }
}

fn checked(dataset: Dataset, row: ExportRow) -> ExportRow {
    debug_assert_eq!(row.values.len(), schema(dataset).columns.len());
    row
}

/// Maps one `usage_events` row.
#[must_use]
pub fn usage_event(machine: &MachineId, versioned: &Versioned<StoredEvent>) -> ExportRow {
    let e = &versioned.row.event;
    let t = &e.tokens;
    let values = vec![
        text(e.id.to_string()),
        revision(versioned.revision),
        text(machine.to_string()),
        text(e.agent.as_str()),
        text(e.profile.as_str()),
        opt_text(e.provider.as_ref().map(tokenmtr_core::ProviderId::as_str)),
        text(e.model.as_str()),
        opt_text(e.service_tier.as_deref()),
        text(e.session.as_str()),
        opt_text(
            e.parent_session
                .as_ref()
                .map(tokenmtr_core::SessionId::as_str),
        ),
        opt_text(e.project.as_ref().map(tokenmtr_core::ProjectKey::as_str)),
        opt_text(e.client.as_deref()),
        opt_text(e.cwd.as_ref().map(|c| c.as_str())),
        opt_text(e.git_branch.as_deref()),
        instant(e.at),
        text(kind_name(e.kind)),
        Value::Boolean(e.approximate),
        count(t.input_uncached),
        count(t.cache_read),
        count(t.cache_write_5m),
        count(t.cache_write_1h),
        count(t.output),
        count(t.reasoning),
        count(t.total_tokens()),
        e.cost.reported().map_or(Value::Null, usd),
        e.cost.estimated().map_or(Value::Null, |c| usd(c.usd)),
        text(cost_source_name(e.cost.api_cost().source())),
        opt_text(e.cost.estimated().map(|c| c.pricing_version.as_str())),
        Value::Boolean(versioned.row.superseded),
    ];
    checked(
        Dataset::UsageEvents,
        ExportRow {
            revision: versioned.revision,
            partition_date: utc_date(e.at),
            values,
        },
    )
}

/// Maps one `tool_calls` row.
///
/// # Errors
/// [`ExportError::Id`] when the call has no valid export id.
pub fn tool_call(
    machine: &MachineId,
    versioned: &Versioned<ProfiledToolCall>,
) -> Result<ExportRow, ExportError> {
    let ProfiledToolCall { profile, call } = &versioned.row;
    let values = vec![
        text(versioned.row.export_id(machine)?.to_string()),
        revision(versioned.revision),
        text(machine.to_string()),
        text(call.agent.as_str()),
        text(profile.as_str()),
        text(call.session.as_str()),
        opt_text(call.event.as_ref().map(EventId::to_string)),
        instant(call.at),
        text(call.tool_name.as_str()),
        opt_text(call.mcp_server.as_deref()),
        call.is_error.map_or(Value::Null, Value::Boolean),
        opt_text(call.skill.as_ref().map(SkillName::as_str)),
    ];
    Ok(checked(
        Dataset::ToolCalls,
        ExportRow {
            revision: versioned.revision,
            partition_date: utc_date(call.at),
            values,
        },
    ))
}

/// Maps one `subscriptions` row. The partition date is the start date, but the dataset is never
/// partitioned (see [`crate::schema::DatasetSchema::partition_column`]).
#[must_use]
pub fn subscription(machine: &MachineId, versioned: &Versioned<StoredSubscription>) -> ExportRow {
    let s = &versioned.row.subscription;
    let join = |names: Vec<&str>| names.join(",");
    let profiles = s
        .profiles()
        .map(|p| join(p.iter().map(tokenmtr_core::ProfileId::as_str).collect()));
    let values = vec![
        text(s.id().as_str()),
        revision(versioned.revision),
        text(machine.to_string()),
        text(s.provider().as_str()),
        text(plan_name(s.plan())),
        text(join(s.agents().iter().map(|a| a.as_str()).collect())),
        opt_text(profiles),
        Value::Decimal {
            units: s.fee().micros(),
            scale: Scale::Micros,
        },
        text(currency_name(s.fee().currency())),
        text(period_name(s.period())),
        date_text(s.start()),
        s.end().map_or(Value::Null, date_text),
        versioned.row.removed_at.map_or(Value::Null, instant),
    ];
    checked(
        Dataset::Subscriptions,
        ExportRow {
            revision: versioned.revision,
            partition_date: s.start(),
            values,
        },
    )
}

/// Maps one `quota_snapshots` row.
///
/// # Errors
/// [`ExportError::Id`] when the sample has no valid export id.
pub fn quota_snapshot(
    machine: &MachineId,
    versioned: &Versioned<QuotaSample>,
) -> Result<ExportRow, ExportError> {
    let q = &versioned.row;
    let window = q.window();
    let window = window.as_ref();
    let extra = q.extra_usage();
    let values = vec![
        text(q.export_id(machine)?.to_string()),
        revision(versioned.revision),
        text(machine.to_string()),
        text(q.agent.as_str()),
        text(q.profile.as_str()),
        opt_text(q.account.as_deref()),
        text(window.map_or("", |w| w.id.as_str())),
        opt_text(q.window_label()),
        window.map_or(Value::Null, |w| Value::Real(w.state.utilization.percent())),
        window
            .and_then(|w| w.state.resets_at)
            .map_or(Value::Null, instant),
        instant(q.fetched_at),
        text(status_name(q.status())),
        extra.map_or(Value::Null, |e| usd(e.spent)),
        extra.map_or(Value::Null, |e| usd(e.limit)),
    ];
    Ok(checked(
        Dataset::QuotaSnapshots,
        ExportRow {
            revision: versioned.revision,
            partition_date: utc_date(q.fetched_at),
            values,
        },
    ))
}

/// Maps one `fx_rates` row.
#[must_use]
pub fn fx_rate(versioned: &Versioned<FxRateRow>) -> ExportRow {
    let r = &versioned.row;
    let source = match r.source {
        FxSource::Ecb => "ecb",
    };
    let values = vec![
        date_text(r.rate.date),
        revision(versioned.revision),
        Value::Decimal {
            units: r.rate.eur_per_usd.scaled(),
            scale: Scale::Nanos,
        },
        text(source),
    ];
    checked(
        Dataset::FxRates,
        ExportRow {
            revision: versioned.revision,
            partition_date: r.rate.date,
            values,
        },
    )
}

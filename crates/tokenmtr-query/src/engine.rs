//! The one SQL aggregation everything else folds.
//!
//! Events are grouped in SQLite by 15-minute UTC cell and a few dimension columns. Local
//! calendar fields are derived in Rust from the cell start: every real UTC offset is a multiple
//! of 15 minutes, so a cell never straddles a local hour or day, and DST days keep their length.

use std::collections::BTreeMap;
use std::str::FromStr;

use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::types::Value;
use tokenmtr_core::fx::utc_date;
use tokenmtr_core::{
    AgentId, Currency, EurPerUsd, ProfileId, ProviderId, Subscription, TokenCounts, UsdMicros,
    convert, prorated_fee, saturating_micros,
};
use tokenmtr_store::StoreReader;

use crate::context::{FxPolicy, QueryContext};
use crate::error::QueryError;
use crate::filters::{BillingFilter, Filters};

const CELL_US: i64 = 900_000_000;

/// Extra grouping of the aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dim {
    None,
    Model,
    Project,
    Branch,
    Session,
}

impl Dim {
    const fn columns(self) -> (&'static str, &'static str) {
        match self {
            Self::None => ("NULL", "NULL"),
            Self::Model => ("model", "NULL"),
            Self::Project => ("project", "client"),
            Self::Branch => ("git_branch", "NULL"),
            Self::Session => ("session", "NULL"),
        }
    }
}

/// Which events count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Everything, daily aggregates included: totals, series, models, projects.
    All,
    /// Real traffic only: sessions, heatmap. Fees are not allocated.
    Traffic,
}

/// One SQL group.
#[derive(Debug, Clone)]
pub(crate) struct Cell {
    pub start: Timestamp,
    pub agent: AgentId,
    pub profile: ProfileId,
    pub provider: Option<ProviderId>,
    pub k1: Option<String>,
    pub k2: Option<String>,
    pub events: u64,
    pub input_uncached: u64,
    pub cache_read: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cost_usd: i64,
    pub reported: u64,
    pub estimated: u64,
    pub unpriced: u64,
    pub approximate: u64,
    pub first_at: Timestamp,
    pub last_at: Timestamp,
}

impl Cell {
    /// The cell's tokens as [`TokenCounts::total_tokens`] counts them (saturating).
    pub(crate) const fn total_tokens(&self) -> u64 {
        TokenCounts {
            input_uncached: self.input_uncached,
            cache_read: self.cache_read,
            cache_write_5m: self.cache_write_5m,
            cache_write_1h: self.cache_write_1h,
            output: self.output,
            reasoning: self.reasoning,
        }
        .total_tokens()
    }
}

/// How a cell is billed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Api,
    Subscription(usize),
}

/// A cell with its local date, billing mode and converted money.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub cell: Cell,
    pub local: Date,
    pub mode: Mode,
    /// API value in the display currency.
    pub api: i64,
    /// Paid cost in the display currency.
    pub paid: i64,
    /// Subscription fee share inside `paid`.
    pub fee: i64,
    pub approximate: bool,
}

impl Row {
    pub(crate) const fn is_subscription(&self) -> bool {
        matches!(self.mode, Mode::Subscription(_))
    }
}

/// A fee share nobody used: counted in totals and series, in no breakdown row.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Idle {
    pub sub: usize,
    pub local: Date,
    pub paid: i64,
    pub approximate: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Fetched {
    pub rows: Vec<Row>,
    pub idle: Vec<Idle>,
}

fn micros_ts(us: i64) -> Result<Timestamp, QueryError> {
    Timestamp::from_microsecond(us).map_err(|_| QueryError::decode("timestamp", us))
}

fn count(row: &rusqlite::Row<'_>, i: usize) -> Result<u64, QueryError> {
    to_count(row.get(i)?)
}

/// A closed set stored by its snake-case name (a serde unit enum such as `CostSource`).
pub(crate) fn closed<T: serde::de::DeserializeOwned>(
    what: &'static str,
    text: &str,
) -> Result<T, QueryError> {
    T::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(text))
        .map_err(|_| QueryError::decode(what, text))
}

/// A SQL count or token sum as `u64`; a negative value means a corrupt store.
pub(crate) fn to_count(v: i64) -> Result<u64, QueryError> {
    u64::try_from(v).map_err(|_| QueryError::decode("count", v))
}

fn parse<T: FromStr>(what: &'static str, s: &str) -> Result<T, QueryError> {
    s.parse().map_err(|_| QueryError::decode(what, s))
}

/// The `WHERE` conditions (without the keyword) selecting `filters` on `usage_event`.
pub(crate) fn where_sql(
    ctx: &QueryContext,
    filters: &Filters,
    scope: Scope,
) -> Result<(String, Vec<Value>), QueryError> {
    let (from, to) = filters.range().instants(ctx.zone())?;
    // Superseded stats-cache rows stay stored for the export but never count.
    let mut sql = String::from("at_us >= ? AND at_us < ? AND superseded = 0");
    let mut params = vec![
        Value::Integer(from.as_microsecond()),
        Value::Integer(to.as_microsecond()),
    ];
    if scope == Scope::Traffic {
        sql.push_str(" AND kind <> 'daily_aggregate'");
    }
    filters.sql(&mut sql, &mut params);
    Ok((sql, params))
}

fn query_cells(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
    dim: Dim,
    scope: Scope,
) -> Result<Vec<Cell>, QueryError> {
    let (k1, k2) = dim.columns();
    let mut sql = format!(
        "SELECT at_us - ((at_us % {CELL_US}) + {CELL_US}) % {CELL_US} AS cell, agent, profile, \
         provider, {k1}, {k2}, COUNT(*), SUM(input_uncached), SUM(cache_read), \
         SUM(cache_write_5m), SUM(cache_write_1h), SUM(output), SUM(reasoning), \
         COALESCE(SUM(cost_api_usd_micros), 0), SUM(cost_source = 'reported'), \
         SUM(cost_source = 'estimated'), SUM(cost_source = 'unpriced'), SUM(approximate), \
         MIN(at_us), MAX(at_us) FROM usage_event WHERE "
    );
    let (conditions, params) = where_sql(ctx, filters, scope)?;
    sql.push_str(&conditions);
    sql.push_str(" GROUP BY 1, 2, 3, 4");
    if dim != Dim::None {
        sql.push_str(", 5");
    }
    if dim == Dim::Project {
        sql.push_str(", 6");
    }
    let mut statement = reader.connection().prepare(&sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(params))?;
    let mut cells = Vec::new();
    while let Some(row) = rows.next()? {
        let agent: String = row.get(1)?;
        let profile: String = row.get(2)?;
        let provider: Option<String> = row.get(3)?;
        cells.push(Cell {
            start: micros_ts(row.get(0)?)?,
            agent: parse("agent", &agent)?,
            profile: parse("profile", &profile)?,
            provider: provider
                .as_deref()
                .map(|p| parse("provider", p))
                .transpose()?,
            k1: row.get(4)?,
            k2: row.get(5)?,
            events: count(row, 6)?,
            input_uncached: count(row, 7)?,
            cache_read: count(row, 8)?,
            cache_write_5m: count(row, 9)?,
            cache_write_1h: count(row, 10)?,
            output: count(row, 11)?,
            reasoning: count(row, 12)?,
            cost_usd: row.get(13)?,
            reported: count(row, 14)?,
            estimated: count(row, 15)?,
            unpriced: count(row, 16)?,
            approximate: count(row, 17)?,
            first_at: micros_ts(row.get(18)?)?,
            last_at: micros_ts(row.get(19)?)?,
        });
    }
    Ok(cells)
}

/// First matching subscription wins, like `tokenmtr_core::billing_mode`: coverage uses the
/// cell's `local` date, the display-zone day its fee is charged on.
fn mode_of(subs: &[Subscription], cell: &Cell, local: Date) -> Mode {
    subs.iter()
        .position(|s| {
            s.agents().contains(&cell.agent)
                && s.profiles().is_none_or(|p| p.contains(&cell.profile))
                && cell.provider.as_ref() == Some(s.provider())
                && s.is_active_on(local)
        })
        .map_or(Mode::Api, Mode::Subscription)
}

/// USD micros in `currency`, converted with the rate strictly before `date`.
pub(crate) fn usd_to(
    currency: Currency,
    fx: &FxPolicy,
    date: Date,
    usd: i64,
) -> Result<(i64, bool), QueryError> {
    match currency {
        Currency::Usd => Ok((usd, false)),
        Currency::Eur => {
            let use_ = fx.rate(date).ok_or(QueryError::NoFxRate { date })?;
            Ok((
                convert(UsdMicros::new(usd), use_.rate).get(),
                use_.approximate,
            ))
        }
    }
}

fn eur_to_usd(eur: i64, rate: EurPerUsd) -> i64 {
    let rate = i128::from(rate.scaled());
    let usd = (i128::from(eur) * 1_000_000_000 + rate / 2) / rate;
    saturating_micros(usd)
}

/// The fee of `sub` on local `day` in `currency` (0 outside its active range).
///
/// A fee belongs to a local day, not to an instant, so it converts at the rate before that day;
/// API value converts at the rate before the event's UTC date, as the export documents.
/// The two dates differ only for events near midnight.
fn fee_in(
    ctx: &QueryContext,
    currency: Currency,
    sub: &Subscription,
    day: Date,
) -> Result<(i64, bool), QueryError> {
    let fee = prorated_fee(sub, day);
    let micros = fee.micros();
    if micros == 0 || fee.currency() == currency {
        return Ok((micros, false));
    }
    let use_ = ctx
        .fx()
        .rate(day)
        .ok_or(QueryError::NoFxRate { date: day })?;
    let converted = match currency {
        Currency::Eur => convert(UsdMicros::new(micros), use_.rate).get(),
        Currency::Usd => eur_to_usd(micros, use_.rate),
    };
    Ok((converted, use_.approximate))
}

/// Splits `fee` over `values` in proportion, exactly: the parts sum to `fee` when
/// `values` sums to `total`.
fn split(fee: i64, values: &[i64], total: i64) -> Vec<i64> {
    let (fee, total) = (i128::from(fee), i128::from(total));
    let mut cumulative = 0_i128;
    let mut given = 0_i128;
    values
        .iter()
        .map(|v| {
            cumulative += i128::from(*v);
            let upto = (fee * cumulative + total / 2) / total;
            let part = upto - given;
            given = upto;
            saturating_micros(part)
        })
        .collect()
}

/// Rows (with local dates, billing modes and money in the display currency) for `filters`.
///
/// Paid cost: pay-as-you-go rows pay their API value; subscription rows share that
/// day's prorated fee by API-value share of the day's subscription usage. When the filters
/// restrict the selection the shares still use the unrestricted usage as denominator and unused
/// fees stay out; when unrestricted, a fee nobody used is reported in [`Fetched::idle`].
pub(crate) fn fetch(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
    dim: Dim,
    scope: Scope,
) -> Result<Fetched, QueryError> {
    let currency = ctx.currency();
    let subs = ctx.subscriptions();
    let mut rows = Vec::new();
    for cell in query_cells(reader, ctx, filters, dim, scope)? {
        let utc = utc_date(cell.start);
        let local = ctx.zone().to_datetime(cell.start).date();
        let mode = mode_of(subs, &cell, local);
        let keep = match (filters.billing_filter(), mode) {
            (BillingFilter::Any, _)
            | (BillingFilter::Api, Mode::Api)
            | (BillingFilter::Subscription(None), Mode::Subscription(_)) => true,
            (BillingFilter::Subscription(Some(id)), Mode::Subscription(i)) => subs[i].id() == id,
            _ => false,
        };
        if !keep {
            continue;
        }
        let (api, rate_approx) = usd_to(currency, ctx.fx(), utc, cell.cost_usd)?;
        let approximate = rate_approx || cell.approximate + cell.estimated + cell.unpriced > 0;
        let paid = if mode == Mode::Api { api } else { 0 };
        rows.push(Row {
            cell,
            local,
            mode,
            api,
            paid,
            fee: 0,
            approximate,
        });
    }
    let mut fetched = Fetched {
        rows,
        idle: Vec::new(),
    };
    if scope == Scope::All && !subs.is_empty() {
        allocate_fees(reader, ctx, filters, &mut fetched)?;
    }
    Ok(fetched)
}

fn allocate_fees(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
    fetched: &mut Fetched,
) -> Result<(), QueryError> {
    let subs = ctx.subscriptions();
    let scoped = filters.is_scoped();
    // Subscription usage per (subscription, local day) in USD micros: the share denominators.
    let mut denominators: BTreeMap<(usize, Date), i64> = BTreeMap::new();
    if scoped {
        let all = fetch_plain(reader, ctx, &Filters::new(filters.range()))?;
        for (key, usd) in all {
            let total = denominators.entry(key).or_default();
            *total = total.saturating_add(usd);
        }
    } else {
        for row in &fetched.rows {
            if let Mode::Subscription(i) = row.mode {
                let total = denominators.entry((i, row.local)).or_default();
                *total = total.saturating_add(row.cell.cost_usd);
            }
        }
    }
    let mut groups: BTreeMap<(usize, Date), Vec<usize>> = BTreeMap::new();
    for (index, row) in fetched.rows.iter().enumerate() {
        if let Mode::Subscription(i) = row.mode {
            groups.entry((i, row.local)).or_default().push(index);
        }
    }
    for (i, sub) in subs.iter().enumerate() {
        for day in filters.range().dates() {
            if day > ctx.today() {
                break;
            }
            let (fee, fee_approx) = fee_in(ctx, ctx.currency(), sub, day)?;
            if fee == 0 {
                continue;
            }
            let total = denominators.get(&(i, day)).copied().unwrap_or(0);
            let indices = groups.get(&(i, day)).map_or(&[][..], Vec::as_slice);
            if total <= 0 {
                if !scoped {
                    fetched.idle.push(Idle {
                        sub: i,
                        local: day,
                        paid: fee,
                        approximate: fee_approx,
                    });
                }
                continue;
            }
            let values: Vec<i64> = indices
                .iter()
                .map(|&k| fetched.rows[k].cell.cost_usd)
                .collect();
            let parts = if scoped {
                let total = i128::from(total);
                values
                    .iter()
                    .map(|v| {
                        let part = (i128::from(fee) * i128::from(*v) + total / 2) / total;
                        saturating_micros(part)
                    })
                    .collect()
            } else {
                split(fee, &values, total)
            };
            for (&k, part) in indices.iter().zip(parts) {
                let row = &mut fetched.rows[k];
                row.paid = row.paid.saturating_add(part);
                row.fee = row.fee.saturating_add(part);
                row.approximate |= fee_approx;
            }
        }
    }
    Ok(())
}

/// Subscription usage per (subscription index, local day), USD micros.
type PlainUsage = Vec<((usize, Date), i64)>;

/// Subscription usage of the unrestricted range per (subscription, local day), USD micros.
fn fetch_plain(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
) -> Result<PlainUsage, QueryError> {
    Ok(query_cells(reader, ctx, filters, Dim::None, Scope::All)?
        .into_iter()
        .filter_map(|cell| {
            let local = ctx.zone().to_datetime(cell.start).date();
            match mode_of(ctx.subscriptions(), &cell, local) {
                Mode::Subscription(i) => Some(((i, local), cell.cost_usd)),
                Mode::Api => None,
            }
        })
        .collect())
}

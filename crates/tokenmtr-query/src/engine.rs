//! The one SQL aggregation everything else folds.
//!
//! Events are grouped in SQLite by 15-minute UTC cell and a few dimension columns. Local
//! calendar fields are derived in Rust from the cell start: every real UTC offset is a multiple
//! of 15 minutes, so a cell never straddles a local hour or day, and DST days keep their length.

use std::collections::{BTreeMap, HashMap};
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
    /// Project in `k1`, model in `k2`.
    ProjectModel,
    Branch,
    Session,
    /// Session in `k1`, model in `k2`.
    SessionModel,
}

impl Dim {
    const fn columns(self) -> (&'static str, &'static str) {
        match self {
            Self::None => ("NULL", "NULL"),
            Self::Model => ("model", "NULL"),
            Self::Project => ("project", "client"),
            Self::ProjectModel => ("project", "model"),
            Self::Branch => ("git_branch", "NULL"),
            Self::Session => ("session", "NULL"),
            Self::SessionModel => ("session", "model"),
        }
    }
}

/// Which events count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Everything, daily aggregates and extra usage included: totals, money series, billing,
    /// budgets.
    All,
    /// Token usage: daily aggregates included, extra usage (spend without tokens) left out:
    /// models, projects, branches, token series.
    Usage,
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
    /// Claude extra-usage rows: spend in `cost_usd`, no tokens.
    pub extra: bool,
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
    /// Claude extra usage: paid as spent, no API value, never shares a fee.
    Extra,
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

pub(crate) fn micros_ts(us: i64) -> Result<Timestamp, QueryError> {
    Timestamp::from_microsecond(us).map_err(|_| QueryError::decode("timestamp", us))
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
    match scope {
        Scope::All => {}
        Scope::Usage => sql.push_str(" AND kind <> 'extra_usage'"),
        Scope::Traffic => sql.push_str(" AND kind NOT IN ('daily_aggregate', 'extra_usage')"),
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
    let (conditions, params) = where_sql(ctx, filters, scope)?;
    let sql = format!(
        "SELECT at_us, agent, profile, provider, {k1}, {k2}, input_uncached, cache_read, \
         cache_write_5m, cache_write_1h, output, reasoning, cost_reported_usd_micros, \
         cost_estimated_usd_micros, approximate, kind = 'extra_usage' FROM usage_event \
         WHERE {conditions}"
    );
    let mut statement = reader.connection().prepare_cached(&sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(params))?;
    // Groups by cell start; a cell holds a handful of groups, searched linearly.
    let mut groups: HashMap<i64, Vec<Group>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let at: i64 = row.get(0)?;
        let key = GroupKey {
            agent: text(row, 1)?.ok_or_else(|| QueryError::decode("agent", "NULL"))?,
            profile: text(row, 2)?.ok_or_else(|| QueryError::decode("profile", "NULL"))?,
            provider: text(row, 3)?,
            k1: text(row, 4)?,
            k2: text(row, 5)?,
            extra: row.get(15)?,
        };
        let cell = groups.entry(at - at.rem_euclid(CELL_US)).or_default();
        let i = cell.iter().position(|g| g.key() == key).unwrap_or_else(|| {
            cell.push(Group::new(&key, at));
            cell.len() - 1
        });
        cell[i].add(row, at)?;
    }
    let mut cells = groups
        .into_iter()
        .flat_map(|(start, groups)| groups.into_iter().map(move |g| g.into_cell(start)))
        .collect::<Result<Vec<_>, _>>()?;
    cells.sort_unstable_by(|a, b| {
        (
            a.start,
            a.agent,
            a.profile.as_ref(),
            &a.provider,
            &a.k1,
            &a.k2,
        )
            .cmp(&(
                b.start,
                b.agent,
                b.profile.as_ref(),
                &b.provider,
                &b.k1,
                &b.k2,
            ))
    });
    Ok(cells)
}

/// A nullable text column, borrowed from the row.
fn text<'r>(row: &'r rusqlite::Row<'_>, i: usize) -> Result<Option<&'r str>, QueryError> {
    row.get_ref(i)?
        .as_str_or_null()
        .map_err(|e| QueryError::decode("text", e))
}

/// The grouping columns of one `usage_event` row, borrowed from the row.
#[derive(PartialEq, Eq)]
struct GroupKey<'a> {
    agent: &'a str,
    profile: &'a str,
    provider: Option<&'a str>,
    k1: Option<&'a str>,
    k2: Option<&'a str>,
    extra: bool,
}

/// The running sums of one group, before parsing into a [`Cell`].
struct Group {
    agent: String,
    profile: String,
    provider: Option<String>,
    k1: Option<String>,
    k2: Option<String>,
    extra: bool,
    events: u64,
    tokens: TokenCounts,
    cost_usd: i64,
    reported: u64,
    estimated: u64,
    unpriced: u64,
    approximate: u64,
    first_at: i64,
    last_at: i64,
}

impl Group {
    fn new(key: &GroupKey<'_>, at: i64) -> Self {
        Self {
            agent: key.agent.to_owned(),
            profile: key.profile.to_owned(),
            provider: key.provider.map(str::to_owned),
            k1: key.k1.map(str::to_owned),
            k2: key.k2.map(str::to_owned),
            extra: key.extra,
            events: 0,
            tokens: TokenCounts::default(),
            cost_usd: 0,
            reported: 0,
            estimated: 0,
            unpriced: 0,
            approximate: 0,
            first_at: at,
            last_at: at,
        }
    }

    fn key(&self) -> GroupKey<'_> {
        GroupKey {
            agent: &self.agent,
            profile: &self.profile,
            provider: self.provider.as_deref(),
            k1: self.k1.as_deref(),
            k2: self.k2.as_deref(),
            extra: self.extra,
        }
    }

    /// Adds a row of [`query_cells`]'s statement: the six token columns from column 6, then the
    /// reported and estimated costs (the reported cost wins, as in `cost_api_usd_micros`) and
    /// `approximate`.
    fn add(&mut self, row: &rusqlite::Row<'_>, at: i64) -> Result<(), QueryError> {
        self.events += 1;
        let t = &mut self.tokens;
        let sums = [
            &mut t.input_uncached,
            &mut t.cache_read,
            &mut t.cache_write_5m,
            &mut t.cache_write_1h,
            &mut t.output,
            &mut t.reasoning,
        ];
        for (i, sum) in sums.into_iter().enumerate() {
            *sum = sum.saturating_add(to_count(row.get(6 + i)?)?);
        }
        let reported: Option<i64> = row.get(12)?;
        let estimated: Option<i64> = row.get(13)?;
        match (reported, estimated) {
            (Some(usd), _) => {
                self.reported += 1;
                self.cost_usd = self.cost_usd.saturating_add(usd);
            }
            (None, Some(usd)) => {
                self.estimated += 1;
                self.cost_usd = self.cost_usd.saturating_add(usd);
            }
            (None, None) => self.unpriced += 1,
        }
        if row.get::<_, bool>(14)? {
            self.approximate += 1;
        }
        self.first_at = self.first_at.min(at);
        self.last_at = self.last_at.max(at);
        Ok(())
    }

    fn into_cell(self, start: i64) -> Result<Cell, QueryError> {
        Ok(Cell {
            start: micros_ts(start)?,
            agent: parse("agent", &self.agent)?,
            profile: parse("profile", &self.profile)?,
            provider: self
                .provider
                .as_deref()
                .map(|p| parse("provider", p))
                .transpose()?,
            k1: self.k1,
            k2: self.k2,
            events: self.events,
            input_uncached: self.tokens.input_uncached,
            cache_read: self.tokens.cache_read,
            cache_write_5m: self.tokens.cache_write_5m,
            cache_write_1h: self.tokens.cache_write_1h,
            output: self.tokens.output,
            reasoning: self.tokens.reasoning,
            cost_usd: self.cost_usd,
            reported: self.reported,
            estimated: self.estimated,
            unpriced: self.unpriced,
            approximate: self.approximate,
            first_at: micros_ts(self.first_at)?,
            last_at: micros_ts(self.last_at)?,
            extra: self.extra,
        })
    }
}

/// First matching subscription wins, like `tokenmtr_core::billing_mode`: coverage uses the
/// cell's `local` date, the display-zone day its fee is charged on. Extra usage is spend beyond
/// any plan, so no subscription covers it.
fn mode_of(subs: &[Subscription], cell: &Cell, local: Date) -> Mode {
    if cell.extra {
        return Mode::Extra;
    }
    covering(
        subs,
        cell.agent,
        &cell.profile,
        cell.provider.as_ref(),
        local,
    )
    .map_or(Mode::Api, Mode::Subscription)
}

/// Index of the first subscription covering a usage event of `agent`/`profile`/`provider` on
/// local day `local`.
pub(crate) fn covering(
    subs: &[Subscription],
    agent: AgentId,
    profile: &ProfileId,
    provider: Option<&ProviderId>,
    local: Date,
) -> Option<usize> {
    subs.iter().position(|s| {
        s.agents().contains(&agent)
            && s.profiles().is_none_or(|p| p.contains(profile))
            && provider == Some(s.provider())
            && s.is_active_on(local)
    })
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
pub(crate) fn split(fee: i64, values: &[i64], total: i64) -> Vec<i64> {
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
/// Paid cost: pay-as-you-go rows pay their API value; extra-usage rows pay their spend and have
/// no API value (they pass the `api` billing filter: they are pay-as-you-go); subscription rows share that
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
            | (BillingFilter::Api, Mode::Api | Mode::Extra)
            | (BillingFilter::Subscription(None), Mode::Subscription(_)) => true,
            (BillingFilter::Subscription(Some(id)), Mode::Subscription(i)) => subs[i].id() == id,
            _ => false,
        };
        if !keep {
            continue;
        }
        let (value, rate_approx) = usd_to(currency, ctx.fx(), utc, cell.cost_usd)?;
        let approximate = rate_approx || cell.approximate + cell.estimated + cell.unpriced > 0;
        let (api, paid) = match mode {
            Mode::Api => (value, value),
            Mode::Subscription(_) => (value, 0),
            Mode::Extra => (0, value),
        };
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
    if scope != Scope::Traffic && !subs.is_empty() {
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
                Mode::Api | Mode::Extra => None,
            }
        })
        .collect())
}

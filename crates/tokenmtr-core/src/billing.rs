//! Subscriptions, billing mode and fee proration (D28), plus the plan catalog.

use std::num::NonZeroU32;

use jiff::Span;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::event::UsageEvent;
use crate::ids::{AgentId, ProfileId, ProviderId, SubscriptionId};
use crate::money::{Currency, Money, UsdMicros, saturating_micros};

/// The embedded plan catalog (`pricing/plans.toml`).
pub const EMBEDDED_PLANS: &str = include_str!("../../../pricing/plans.toml");

/// A subscription plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Plan {
    /// Claude Pro.
    #[serde(rename = "pro")]
    Pro,
    /// Claude Max 5x.
    #[serde(rename = "max_5x")]
    Max5x,
    /// Claude Max 20x.
    #[serde(rename = "max_20x")]
    Max20x,
    /// Claude Team.
    #[serde(rename = "team")]
    Team,
    /// Claude Enterprise.
    #[serde(rename = "enterprise")]
    Enterprise,
    /// Any other flat-fee plan.
    #[serde(rename = "custom")]
    Custom,
}

/// How often the fee is billed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingPeriod {
    /// Billed every month, anchored on the start date.
    Monthly,
    /// Billed every year, anchored on the start date.
    Yearly,
}

/// How one event is billed, derived at query time.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    /// Pay-as-you-go API rates.
    Api,
    /// Covered by this subscription.
    Subscription(SubscriptionId),
}

/// Why a subscription was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BillingError {
    /// `agents` is empty.
    #[error("subscription {0} covers no agent")]
    NoAgents(SubscriptionId),
    /// `profiles` is present but empty.
    #[error("subscription {0} has an empty profiles list")]
    EmptyProfiles(SubscriptionId),
    /// `end` is not after `start`.
    #[error("subscription {0} must end after it starts")]
    EndNotAfterStart(SubscriptionId),
    /// The fee is negative.
    #[error("subscription {0} has a negative fee")]
    NegativeFee(SubscriptionId),
}

/// The fields of a [`Subscription`], before validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionSpec {
    /// Unique id from config.
    pub id: SubscriptionId,
    /// Provider whose traffic it covers.
    pub provider: ProviderId,
    /// The plan.
    pub plan: Plan,
    /// Agents whose traffic it covers.
    pub agents: Vec<AgentId>,
    /// Narrows the subscription to some profiles; `None` covers all.
    pub profiles: Option<Vec<ProfileId>>,
    /// The fee per billing period.
    pub fee: Money,
    /// Billing period.
    pub period: BillingPeriod,
    /// First day covered (inclusive).
    pub start: Date,
    /// First day no longer covered (exclusive).
    pub end: Option<Date>,
}

/// A flat-fee plan covering some agents' traffic to one provider over a date range (D28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SubscriptionSpec")]
pub struct Subscription {
    id: SubscriptionId,
    provider: ProviderId,
    plan: Plan,
    agents: Vec<AgentId>,
    profiles: Option<Vec<ProfileId>>,
    fee: Money,
    period: BillingPeriod,
    start: Date,
    end: Option<Date>,
}

impl Subscription {
    /// Validates a spec: at least one agent, non-empty `profiles` when set, `end > start`,
    /// non-negative fee.
    ///
    /// # Errors
    ///
    /// The matching [`BillingError`].
    pub fn new(spec: SubscriptionSpec) -> Result<Self, BillingError> {
        let SubscriptionSpec {
            id,
            provider,
            plan,
            agents,
            profiles,
            fee,
            period,
            start,
            end,
        } = spec;
        if agents.is_empty() {
            return Err(BillingError::NoAgents(id));
        }
        if profiles.as_ref().is_some_and(Vec::is_empty) {
            return Err(BillingError::EmptyProfiles(id));
        }
        if end.is_some_and(|e| e <= start) {
            return Err(BillingError::EndNotAfterStart(id));
        }
        if fee.micros() < 0 {
            return Err(BillingError::NegativeFee(id));
        }
        Ok(Self {
            id,
            provider,
            plan,
            agents,
            profiles,
            fee,
            period,
            start,
            end,
        })
    }

    /// Unique id.
    #[must_use]
    pub const fn id(&self) -> &SubscriptionId {
        &self.id
    }

    /// Provider whose traffic it covers.
    #[must_use]
    pub const fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// The plan.
    #[must_use]
    pub const fn plan(&self) -> Plan {
        self.plan
    }

    /// Agents it covers.
    #[must_use]
    pub fn agents(&self) -> &[AgentId] {
        &self.agents
    }

    /// Profiles it is narrowed to, if any.
    #[must_use]
    pub fn profiles(&self) -> Option<&[ProfileId]> {
        self.profiles.as_deref()
    }

    /// Fee per billing period.
    #[must_use]
    pub const fn fee(&self) -> Money {
        self.fee
    }

    /// Billing period.
    #[must_use]
    pub const fn period(&self) -> BillingPeriod {
        self.period
    }

    /// First day covered.
    #[must_use]
    pub const fn start(&self) -> Date {
        self.start
    }

    /// First day no longer covered.
    #[must_use]
    pub const fn end(&self) -> Option<Date> {
        self.end
    }

    /// `start ≤ day < end`.
    #[must_use]
    pub fn is_active_on(&self, day: Date) -> bool {
        self.start <= day && self.end.is_none_or(|end| day < end)
    }

    /// Whether this subscription covers the event: agent, profile (when narrowed), provider and
    /// date (`start ≤ day < end`, by the event's calendar date in `tz`, the display time zone
    /// that buckets days and prorates fees, D8).
    #[must_use]
    pub fn covers(&self, event: &UsageEvent, tz: &TimeZone) -> bool {
        self.agents.contains(&event.agent)
            && self
                .profiles
                .as_ref()
                .is_none_or(|p| p.contains(&event.profile))
            && event.provider.as_ref() == Some(&self.provider)
            && self.is_active_on(event.at.to_zoned(tz.clone()).date())
    }

    /// Whether two subscriptions would both cover some event (a config error, D28).
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        let shares_agent = self.agents.iter().any(|a| other.agents.contains(a));
        let shares_profile = match (&self.profiles, &other.profiles) {
            (Some(a), Some(b)) => a.iter().any(|p| b.contains(p)),
            _ => true,
        };
        let dates_overlap =
            self.end.is_none_or(|e| other.start < e) && other.end.is_none_or(|e| self.start < e);
        self.provider == other.provider && shares_agent && shares_profile && dates_overlap
    }
}

impl TryFrom<SubscriptionSpec> for Subscription {
    type Error = BillingError;
    fn try_from(spec: SubscriptionSpec) -> Result<Self, BillingError> {
        Self::new(spec)
    }
}

/// The first pair of subscriptions that overlap, if any.
#[must_use]
pub fn find_overlap(subscriptions: &[Subscription]) -> Option<(&SubscriptionId, &SubscriptionId)> {
    subscriptions.iter().enumerate().find_map(|(i, a)| {
        subscriptions[i + 1..]
            .iter()
            .find(|b| a.overlaps(b))
            .map(|b| (a.id(), b.id()))
    })
}

/// `subscription` when one covers the event (by its calendar date in `tz`), otherwise `api`
/// (D28). With overlapping subscriptions, which is a config error, the first match wins.
#[must_use]
pub fn billing_mode(
    event: &UsageEvent,
    tz: &TimeZone,
    subscriptions: &[Subscription],
) -> BillingMode {
    subscriptions
        .iter()
        .find(|s| s.covers(event, tz))
        .map_or(BillingMode::Api, |s| {
            BillingMode::Subscription(s.id.clone())
        })
}

/// The billing period containing `day`, anchored on `start`: `[begin, next)`.
///
/// Periods are `start + k` months (or years), so a start on the 31st clamps to each month's last
/// day without drifting.
fn period_bounds(start: Date, period: BillingPeriod, day: Date) -> Option<(Date, Date)> {
    let step = |k: i32| -> Option<Date> {
        let span = match period {
            BillingPeriod::Monthly => Span::new().try_months(k).ok()?,
            BillingPeriod::Yearly => Span::new().try_years(k).ok()?,
        };
        start.checked_add(span).ok()
    };
    let years = i32::from(day.year()) - i32::from(start.year());
    let estimate = match period {
        BillingPeriod::Yearly => years,
        BillingPeriod::Monthly => years * 12 + i32::from(day.month()) - i32::from(start.month()),
    };
    let k = if step(estimate)? > day {
        estimate - 1
    } else {
        estimate
    };
    Some((step(k)?, step(k + 1)?))
}

/// The fee charged for one day: `fee / days in the billing period containing the day`, rounded
/// half away from zero (D28). Zero outside `start ≤ day < end`, so rounding can leave a period's
/// days summing to within a few micros of the fee.
#[must_use]
pub fn prorated_fee(subscription: &Subscription, day: Date) -> Money {
    let fee = subscription.fee;
    let zero = Money::zero(fee.currency());
    if !subscription.is_active_on(day) {
        return zero;
    }
    let Some((begin, next)) = period_bounds(subscription.start, subscription.period, day) else {
        return zero;
    };
    let days = next
        .since(begin)
        .map_or(0, |span| i64::from(span.get_days()));
    if days <= 0 {
        return zero;
    }
    let micros = i128::from(fee.micros());
    let days = i128::from(days);
    let rounded = if micros >= 0 {
        (micros + days / 2) / days
    } else {
        (micros - days / 2) / days
    };
    Money::from_micros(fee.currency(), saturating_micros(rounded))
}

/// Why the plan catalog was rejected.
#[derive(Debug, thiserror::Error)]
pub enum PlanCatalogError {
    /// The TOML does not match the catalog format.
    #[error("invalid plan catalog: {0}")]
    Toml(#[from] toml::de::Error),
    /// A fee is not a valid non-negative decimal.
    #[error("invalid fee {value:?} for plan {plan:?}")]
    BadFee {
        /// The plan.
        plan: Plan,
        /// The offending text.
        value: String,
    },
    /// A plan appears twice.
    #[error("plan {0:?} is listed twice")]
    Duplicate(Plan),
    /// A quota multiplier is zero.
    #[error("plan {0:?} has a zero quota multiplier")]
    ZeroMultiplier(Plan),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    plan: Vec<RawPlan>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlan {
    id: Plan,
    display_name: String,
    monthly_fee_usd: String,
    yearly_fee_usd: Option<String>,
    quota_multiplier: u32,
}

/// List price and quota of one plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSpec {
    /// The plan.
    pub plan: Plan,
    /// Human-readable name.
    pub display_name: String,
    /// Monthly list price.
    pub monthly_fee: UsdMicros,
    /// Yearly list price, when Anthropic sells it.
    pub yearly_fee: Option<UsdMicros>,
    /// Quota relative to Pro (Pro 1, Max 5x 5, Max 20x 20).
    pub quota_multiplier: NonZeroU32,
}

/// The plan catalog (`pricing/plans.toml`): USD list prices and quota ratios (D28, D35).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanCatalog {
    plans: Vec<PlanSpec>,
}

impl PlanCatalog {
    /// Parses a catalog.
    ///
    /// # Errors
    ///
    /// [`PlanCatalogError`] for bad TOML, fees, duplicates or zero multipliers.
    pub fn parse(text: &str) -> Result<Self, PlanCatalogError> {
        let raw: RawCatalog = toml::from_str(text)?;
        let mut plans: Vec<PlanSpec> = Vec::with_capacity(raw.plan.len());
        for p in raw.plan {
            if plans.iter().any(|q| q.plan == p.id) {
                return Err(PlanCatalogError::Duplicate(p.id));
            }
            let Some(quota_multiplier) = NonZeroU32::new(p.quota_multiplier) else {
                return Err(PlanCatalogError::ZeroMultiplier(p.id));
            };
            let fee = |value: &str| {
                value
                    .parse::<Decimal>()
                    .ok()
                    .filter(|d| !d.is_sign_negative())
                    .and_then(|d| UsdMicros::from_decimal(d).ok())
                    .ok_or_else(|| PlanCatalogError::BadFee {
                        plan: p.id,
                        value: value.to_owned(),
                    })
            };
            plans.push(PlanSpec {
                plan: p.id,
                display_name: p.display_name.clone(),
                monthly_fee: fee(&p.monthly_fee_usd)?,
                yearly_fee: p.yearly_fee_usd.as_deref().map(fee).transpose()?,
                quota_multiplier,
            });
        }
        Ok(Self { plans })
    }

    /// Parses the embedded `pricing/plans.toml`.
    ///
    /// # Errors
    ///
    /// Same as [`PlanCatalog::parse`]; a test guarantees the shipped file parses.
    pub fn embedded() -> Result<Self, PlanCatalogError> {
        Self::parse(EMBEDDED_PLANS)
    }

    /// Every plan with a list price.
    #[must_use]
    pub fn plans(&self) -> &[PlanSpec] {
        &self.plans
    }

    /// One plan's entry; `None` for plans without a list price (team, enterprise, custom).
    #[must_use]
    pub fn get(&self, plan: Plan) -> Option<&PlanSpec> {
        self.plans.iter().find(|p| p.plan == plan)
    }

    /// The default subscription fee for a plan and period (D28), when the catalog has one.
    #[must_use]
    pub fn default_fee(&self, plan: Plan, period: BillingPeriod) -> Option<Money> {
        let spec = self.get(plan)?;
        let usd = match period {
            BillingPeriod::Monthly => Some(spec.monthly_fee),
            BillingPeriod::Yearly => spec.yearly_fee,
        }?;
        Some(Money::from_micros(Currency::Usd, usd.get()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestResult, usage_event};
    use jiff::civil::date;

    fn sub(
        start: Date,
        end: Option<Date>,
        period: BillingPeriod,
        fee: Money,
    ) -> Result<Subscription, Box<dyn std::error::Error>> {
        Ok(Subscription::new(SubscriptionSpec {
            id: SubscriptionId::try_from("s1")?,
            provider: ProviderId::try_from("anthropic")?,
            plan: Plan::Max20x,
            agents: vec![AgentId::CLAUDE],
            profiles: None,
            fee,
            period,
            start,
            end,
        })?)
    }

    fn usd(units: i64) -> Money {
        Money::from_micros(Currency::Usd, units * 1_000_000)
    }

    fn fee_on(s: &Subscription, d: Date) -> i64 {
        prorated_fee(s, d).micros()
    }

    #[test]
    fn mid_month_start_anchors_the_period() -> TestResult {
        let s = sub(date(2026, 1, 15), None, BillingPeriod::Monthly, usd(300))?;
        // [Jan 15, Feb 15) has 31 days; [Feb 15, Mar 15) has 28.
        assert_eq!(fee_on(&s, date(2026, 1, 15)), 9_677_419);
        assert_eq!(fee_on(&s, date(2026, 2, 14)), 9_677_419);
        assert_eq!(fee_on(&s, date(2026, 2, 15)), 10_714_286);
        assert_eq!(fee_on(&s, date(2026, 3, 14)), 10_714_286);
        assert_eq!(fee_on(&s, date(2026, 3, 15)), 9_677_419);
        assert_eq!(fee_on(&s, date(2026, 1, 14)), 0);
        Ok(())
    }

    #[test]
    fn february_has_twenty_eight_or_twenty_nine_days() -> TestResult {
        let s = sub(date(2026, 2, 1), None, BillingPeriod::Monthly, usd(280))?;
        assert_eq!(fee_on(&s, date(2026, 2, 10)), 10_000_000);
        let leap = sub(date(2028, 2, 1), None, BillingPeriod::Monthly, usd(290))?;
        assert_eq!(fee_on(&leap, date(2028, 2, 29)), 10_000_000);
        Ok(())
    }

    #[test]
    fn month_end_anchor_clamps_without_drifting() -> TestResult {
        let s = sub(date(2026, 1, 31), None, BillingPeriod::Monthly, usd(310))?;
        // [Jan 31, Feb 28) = 28 days, [Feb 28, Mar 31) = 31 days, [Mar 31, Apr 30) = 30 days.
        assert_eq!(fee_on(&s, date(2026, 2, 27)), 11_071_429);
        assert_eq!(fee_on(&s, date(2026, 2, 28)), 10_000_000);
        assert_eq!(fee_on(&s, date(2026, 3, 30)), 10_000_000);
        assert_eq!(fee_on(&s, date(2026, 3, 31)), 10_333_333);
        Ok(())
    }

    #[test]
    fn yearly_plan_divides_by_days_in_the_year() -> TestResult {
        let s = sub(date(2025, 6, 1), None, BillingPeriod::Yearly, usd(200))?;
        assert_eq!(fee_on(&s, date(2026, 3, 10)), 547_945); // 365 days
        let leap = sub(date(2023, 6, 1), None, BillingPeriod::Yearly, usd(200))?;
        assert_eq!(fee_on(&leap, date(2024, 3, 1)), 546_448); // 366 days
        assert_eq!(fee_on(&leap, date(2024, 5, 31)), 546_448);
        assert_eq!(fee_on(&leap, date(2024, 6, 1)), 547_945);
        Ok(())
    }

    #[test]
    fn fees_stop_at_the_end_date() -> TestResult {
        let s = sub(
            date(2026, 1, 1),
            Some(date(2026, 3, 1)),
            BillingPeriod::Monthly,
            usd(310),
        )?;
        assert_eq!(fee_on(&s, date(2026, 2, 28)), 11_071_429);
        assert_eq!(fee_on(&s, date(2026, 3, 1)), 0);
        assert_eq!(fee_on(&s, date(2026, 6, 1)), 0);
        Ok(())
    }

    #[test]
    fn eur_fees_stay_eur() -> TestResult {
        let s = sub(
            date(2026, 2, 1),
            None,
            BillingPeriod::Monthly,
            Money::from_micros(Currency::Eur, 280_000_000),
        )?;
        assert_eq!(
            prorated_fee(&s, date(2026, 2, 3)),
            Money::from_micros(Currency::Eur, 10_000_000)
        );
        assert_eq!(
            prorated_fee(&s, date(2026, 1, 3)),
            Money::zero(Currency::Eur)
        );
        Ok(())
    }

    #[test]
    fn billing_mode_follows_agent_provider_profile_and_dates() -> TestResult {
        let s = sub(
            date(2026, 1, 1),
            Some(date(2026, 2, 1)),
            BillingPeriod::Monthly,
            usd(200),
        )?;
        let subs = [s];
        let mode = |agent, at: &str| -> Result<BillingMode, Box<dyn std::error::Error>> {
            Ok(billing_mode(
                &usage_event(agent, "s", at)?,
                &TimeZone::UTC,
                &subs,
            ))
        };
        let covered = BillingMode::Subscription(SubscriptionId::try_from("s1")?);
        assert_eq!(mode(AgentId::CLAUDE, "2026-01-15T10:00:00Z")?, covered);
        assert_eq!(mode(AgentId::CLAUDE, "2026-01-01T00:00:00Z")?, covered);
        assert_eq!(
            mode(AgentId::CLAUDE, "2025-12-31T23:59:59Z")?,
            BillingMode::Api
        );
        // `end` is exclusive.
        assert_eq!(
            mode(AgentId::CLAUDE, "2026-02-01T00:00:00Z")?,
            BillingMode::Api
        );
        // Coverage follows the display-zone date: 23:30 UTC on Dec 31 is Jan 1 at UTC+1.
        let paris = TimeZone::fixed(jiff::tz::offset(1));
        let new_year = usage_event(AgentId::CLAUDE, "s", "2025-12-31T23:30:00Z")?;
        assert_eq!(billing_mode(&new_year, &paris, &subs), covered);
        // pi is not covered.
        assert_eq!(mode(AgentId::PI, "2026-01-15T10:00:00Z")?, BillingMode::Api);
        let mut other_provider = usage_event(AgentId::CLAUDE, "s", "2026-01-15T10:00:00Z")?;
        other_provider.provider = Some(ProviderId::try_from("openai")?);
        assert_eq!(
            billing_mode(&other_provider, &TimeZone::UTC, &subs),
            BillingMode::Api
        );
        other_provider.provider = None;
        assert_eq!(
            billing_mode(&other_provider, &TimeZone::UTC, &subs),
            BillingMode::Api
        );
        Ok(())
    }

    #[test]
    fn profiles_narrow_a_subscription() -> TestResult {
        let mut spec = SubscriptionSpec {
            id: SubscriptionId::try_from("team")?,
            provider: ProviderId::try_from("anthropic")?,
            plan: Plan::Team,
            agents: vec![AgentId::CLAUDE],
            profiles: Some(vec![ProfileId::try_from("work")?]),
            fee: usd(30),
            period: BillingPeriod::Monthly,
            start: date(2026, 1, 1),
            end: None,
        };
        let team = Subscription::new(spec.clone())?;
        let mut event = usage_event(AgentId::CLAUDE, "s", "2026-01-15T10:00:00Z")?;
        assert_eq!(
            billing_mode(&event, &TimeZone::UTC, std::slice::from_ref(&team)),
            BillingMode::Api
        );
        event.profile = ProfileId::try_from("work")?;
        assert_eq!(
            billing_mode(&event, &TimeZone::UTC, std::slice::from_ref(&team)),
            BillingMode::Subscription(team.id().clone())
        );
        spec.profiles = Some(Vec::new());
        assert!(matches!(
            Subscription::new(spec),
            Err(BillingError::EmptyProfiles(_))
        ));
        Ok(())
    }

    #[test]
    fn invalid_subscriptions_are_rejected() -> TestResult {
        let ok = sub(date(2026, 1, 1), None, BillingPeriod::Monthly, usd(1))?;
        assert!(matches!(
            sub(date(2026, 1, 1), Some(date(2026, 1, 1)), BillingPeriod::Monthly, usd(1)).map_err(|e| e.to_string()),
            Err(m) if m.contains("end after")
        ));
        assert!(sub(date(2026, 1, 1), None, BillingPeriod::Monthly, usd(-1)).is_err());
        let mut spec = SubscriptionSpec {
            id: ok.id().clone(),
            provider: ok.provider().clone(),
            plan: Plan::Pro,
            agents: Vec::new(),
            profiles: None,
            fee: usd(1),
            period: BillingPeriod::Monthly,
            start: date(2026, 1, 1),
            end: None,
        };
        assert!(matches!(
            Subscription::new(spec.clone()),
            Err(BillingError::NoAgents(_))
        ));
        spec.agents.push(AgentId::PI);
        assert!(Subscription::new(spec).is_ok());
        Ok(())
    }

    #[test]
    fn overlapping_subscriptions_are_found() -> TestResult {
        let a = sub(
            date(2026, 1, 1),
            Some(date(2026, 2, 1)),
            BillingPeriod::Monthly,
            usd(1),
        )?;
        let b = sub(date(2026, 2, 1), None, BillingPeriod::Monthly, usd(1))?;
        assert!(find_overlap(&[a.clone(), b]).is_none());
        let c = sub(date(2026, 1, 31), None, BillingPeriod::Monthly, usd(1))?;
        assert!(find_overlap(&[a, c]).is_some());
        Ok(())
    }

    #[test]
    fn embedded_plan_catalog_parses() -> TestResult {
        let catalog = PlanCatalog::embedded()?;
        let multiplier = |p| catalog.get(p).map(|s| s.quota_multiplier.get());
        assert_eq!(multiplier(Plan::Pro), Some(1));
        assert_eq!(multiplier(Plan::Max5x), Some(5));
        assert_eq!(multiplier(Plan::Max20x), Some(20));
        assert_eq!(catalog.get(Plan::Team), None);
        assert_eq!(
            catalog.default_fee(Plan::Max20x, BillingPeriod::Monthly),
            Some(usd(200))
        );
        assert_eq!(
            catalog.default_fee(Plan::Max20x, BillingPeriod::Yearly),
            None
        );
        Ok(())
    }

    #[test]
    fn plan_catalog_rejects_bad_input() {
        let plan = |body: &str| format!("[[plan]]\nid = \"pro\"\ndisplay_name = \"Pro\"\n{body}");
        assert!(
            PlanCatalog::parse(&plan("monthly_fee_usd = \"-1\"\nquota_multiplier = 1")).is_err()
        );
        assert!(
            PlanCatalog::parse(&plan("monthly_fee_usd = \"abc\"\nquota_multiplier = 1")).is_err()
        );
        assert!(
            PlanCatalog::parse(&plan("monthly_fee_usd = \"1\"\nquota_multiplier = 0")).is_err()
        );
        let twice = format!(
            "{}\n{}",
            plan("monthly_fee_usd = \"1\"\nquota_multiplier = 1"),
            plan("monthly_fee_usd = \"1\"\nquota_multiplier = 1")
        );
        assert!(matches!(
            PlanCatalog::parse(&twice),
            Err(PlanCatalogError::Duplicate(Plan::Pro))
        ));
        assert!(PlanCatalog::parse("nonsense = 1").is_err());
    }
}

//! Validates a [`ReportInput`] and computes the figures every renderer shows.

use std::collections::BTreeMap;

use jiff::Timestamp;
use jiff::civil::Date;
use tokenmtr_core::Currency;

use crate::input::{Amount, Period, ReportInput, Scope};

/// Why an input cannot become a report.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReportError {
    /// A project draws from a subscription that is not listed.
    #[error("project `{project}` uses unknown subscription `{subscription}`")]
    UnknownSubscription {
        /// Project name.
        project: String,
        /// Subscription id.
        subscription: String,
    },
    /// A subscription is listed twice.
    #[error("subscription `{0}` is listed twice")]
    DuplicateSubscription(String),
    /// A project names a subscription twice.
    #[error("project `{project}` lists subscription `{subscription}` twice")]
    DuplicateShare {
        /// Project name.
        project: String,
        /// Subscription id.
        subscription: String,
    },
    /// Projects, models and days do not add up to the same API value.
    #[error("API value differs: projects {projects} µ, models {models} µ, days {days} µ")]
    TotalsDisagree {
        /// Sum over projects, in millionths.
        projects: u64,
        /// Sum over models, in millionths.
        models: u64,
        /// Sum over days, in millionths.
        days: u64,
    },
    /// A sum does not fit 64 bits.
    #[error("amounts overflow")]
    Overflow,
}

/// Totals of the whole report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    /// What the events would cost at API rates.
    pub api_value: Amount,
    /// What was actually paid: pay-as-you-go cost, plus subscription fees (all of them for
    /// [`Scope::All`], else the in-scope share).
    pub paid: Amount,
    /// Tokens over all models.
    pub tokens: u64,
    /// True when any figure includes approximate events.
    pub approximate: bool,
}

/// One project line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    /// Project name.
    pub name: String,
    /// Client label.
    pub client: Option<String>,
    /// API value.
    pub api_value: Amount,
    /// Paid cost allocated to the project.
    pub paid: Amount,
    /// True when some events are approximate.
    pub approximate: bool,
}

/// One model line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRow {
    /// Model id.
    pub model: String,
    /// Total tokens.
    pub tokens: u64,
    /// API value.
    pub api_value: Amount,
    /// True when some events are approximate.
    pub approximate: bool,
}

/// One day line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayRow {
    /// The day.
    pub date: Date,
    /// API value.
    pub api_value: Amount,
    /// True when some events are approximate.
    pub approximate: bool,
}

/// A validated report: rows sorted, paid cost allocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Days covered.
    pub period: Period,
    /// What the report is about.
    pub scope: Scope,
    /// Currency of every amount.
    pub currency: Currency,
    /// When the report was made.
    pub generated_at: Timestamp,
    /// Number of sessions.
    pub sessions: u64,
    /// Totals.
    pub totals: Totals,
    /// Projects, highest API value first.
    pub projects: Vec<ProjectRow>,
    /// Models, highest API value first.
    pub models: Vec<ModelRow>,
    /// Days, oldest first.
    pub days: Vec<DayRow>,
    /// Fees of subscriptions nobody used; counted in [`Totals::paid`] for [`Scope::All`] only.
    pub unused_paid: Amount,
}

/// Splits `paid` over `weights` in proportion, in whole millionths, so the parts add up to `paid`.
///
/// The leftover millionths go to the largest fractional remainders, earlier entries first.
/// All-zero weights give all zeros: the caller decides what an unused amount means.
#[must_use]
pub fn allocate(paid: Amount, weights: &[Amount]) -> Vec<Amount> {
    let total: u128 = weights.iter().map(|w| u128::from(w.micros())).sum();
    if total == 0 {
        return vec![Amount::ZERO; weights.len()];
    }
    let paid = u128::from(paid.micros());
    let mut parts: Vec<(usize, u128, u128)> = weights
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let scaled = paid * u128::from(w.micros());
            (i, scaled / total, scaled % total)
        })
        .collect();
    let assigned: u128 = parts.iter().map(|(_, share, _)| share).sum();
    let mut left = paid - assigned;
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by(|&a, &b| parts[b].2.cmp(&parts[a].2).then(a.cmp(&b)));
    for index in order {
        if left == 0 {
            break;
        }
        parts[index].1 += 1;
        left -= 1;
    }
    parts
        .into_iter()
        // Each share is at most `paid`, which came from a u64.
        .map(|(_, share, _)| Amount::from_micros(u64::try_from(share).unwrap_or(u64::MAX)))
        .collect()
}

fn sum(amounts: impl IntoIterator<Item = Amount>) -> Result<Amount, ReportError> {
    amounts
        .into_iter()
        .try_fold(Amount::ZERO, Amount::checked_add)
        .ok_or(ReportError::Overflow)
}

/// Fees split over the projects.
struct Fees {
    /// API value per project, in input order.
    project_values: Vec<Amount>,
    /// Subscription fees allocated to each project, in input order.
    allocated: Vec<Amount>,
    /// Fees of subscriptions nobody used.
    unused: Amount,
}

fn split_fees(input: &ReportInput) -> Result<Fees, ReportError> {
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, subscription) in input.subscriptions.iter().enumerate() {
        if index.insert(subscription.id.as_str(), i).is_some() {
            return Err(ReportError::DuplicateSubscription(subscription.id.clone()));
        }
    }

    // Per subscription: the projects' API value drawn from it.
    let mut shares: Vec<Vec<(usize, Amount)>> = vec![Vec::new(); input.subscriptions.len()];
    let mut project_values = Vec::with_capacity(input.projects.len());
    for (p, project) in input.projects.iter().enumerate() {
        let mut seen: Vec<&str> = Vec::new();
        for share in &project.subscriptions {
            let Some(&s) = index.get(share.subscription.as_str()) else {
                return Err(ReportError::UnknownSubscription {
                    project: project.name.clone(),
                    subscription: share.subscription.clone(),
                });
            };
            if seen.contains(&share.subscription.as_str()) {
                return Err(ReportError::DuplicateShare {
                    project: project.name.clone(),
                    subscription: share.subscription.clone(),
                });
            }
            seen.push(share.subscription.as_str());
            shares[s].push((p, share.api_value));
        }
        project_values.push(sum(std::iter::once(project.pay_as_you_go)
            .chain(project.subscriptions.iter().map(|s| s.api_value)))?);
    }

    let mut allocated = vec![Amount::ZERO; input.projects.len()];
    let mut unused = Amount::ZERO;
    for (subscription, drawn) in input.subscriptions.iter().zip(&shares) {
        let weights: Vec<Amount> = drawn
            .iter()
            .map(|&(_, value)| value)
            .chain(std::iter::once(subscription.outside_scope))
            .collect();
        if sum(weights.iter().copied())? == Amount::ZERO {
            unused = sum([unused, subscription.paid])?;
            continue;
        }
        for (&(p, _), part) in drawn.iter().zip(allocate(subscription.paid, &weights)) {
            allocated[p] = sum([allocated[p], part])?;
        }
    }

    Ok(Fees {
        project_values,
        allocated,
        unused,
    })
}

impl Report {
    /// Checks the input and allocates each subscription's fee over the projects that drew from
    /// it, in proportion to their API value (D46).
    ///
    /// # Errors
    ///
    /// [`ReportError`] when the input contradicts itself.
    pub fn from_input(input: ReportInput) -> Result<Self, ReportError> {
        let Fees {
            project_values,
            allocated,
            unused,
        } = split_fees(&input)?;

        let api_projects = sum(project_values.iter().copied())?;
        let api_models = sum(input.models.iter().map(|m| m.api_value))?;
        let api_days = sum(input.days.iter().map(|d| d.api_value))?;
        if api_projects != api_models || api_projects != api_days {
            return Err(ReportError::TotalsDisagree {
                projects: api_projects.micros(),
                models: api_models.micros(),
                days: api_days.micros(),
            });
        }

        let mut projects = Vec::with_capacity(input.projects.len());
        for ((project, api_value), shared) in input
            .projects
            .into_iter()
            .zip(project_values)
            .zip(allocated)
        {
            projects.push(ProjectRow {
                paid: sum([project.pay_as_you_go, shared])?,
                name: project.name,
                client: project.client,
                api_value,
                approximate: project.approximate,
            });
        }
        projects.sort_by(|a, b| {
            b.api_value
                .cmp(&a.api_value)
                .then_with(|| a.name.cmp(&b.name))
        });

        let mut models: Vec<ModelRow> = input
            .models
            .into_iter()
            .map(|m| ModelRow {
                model: m.model,
                tokens: m.tokens,
                api_value: m.api_value,
                approximate: m.approximate,
            })
            .collect();
        models.sort_by(|a, b| {
            b.api_value
                .cmp(&a.api_value)
                .then_with(|| a.model.cmp(&b.model))
        });

        let mut days: Vec<DayRow> = input
            .days
            .into_iter()
            .map(|d| DayRow {
                date: d.date,
                api_value: d.api_value,
                approximate: d.approximate,
            })
            .collect();
        days.sort_by_key(|d| d.date);

        let in_scope_paid = sum(projects.iter().map(|p| p.paid))?;
        let paid = if input.scope == Scope::All {
            sum([in_scope_paid, unused])?
        } else {
            in_scope_paid
        };
        let tokens = models
            .iter()
            .try_fold(0_u64, |acc, m| acc.checked_add(m.tokens))
            .ok_or(ReportError::Overflow)?;
        let approximate = projects.iter().any(|p| p.approximate)
            || models.iter().any(|m| m.approximate)
            || days.iter().any(|d| d.approximate);

        Ok(Self {
            period: input.period,
            scope: input.scope,
            currency: input.currency,
            generated_at: input.generated_at,
            sessions: input.sessions,
            totals: Totals {
                api_value: api_projects,
                paid,
                tokens,
                approximate,
            },
            projects,
            models,
            days,
            unused_paid: unused,
        })
    }
}

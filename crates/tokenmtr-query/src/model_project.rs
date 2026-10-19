//! Cross breakdown of projects by model: which models each project's cost comes from.

use std::collections::BTreeMap;

use serde::Serialize;
use tokenmtr_store::StoreReader;

use crate::breakdowns::{Breakdown, ProjectGrouping, RowNames};
use crate::context::QueryContext;
use crate::engine::{Scope, fetch};
use crate::error::QueryError;
use crate::filters::Filters;
use crate::usage::Usage;

/// One model's part of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelSlice {
    /// Model id as reported.
    pub model: String,
    /// Tokens and money of the project's events on this model.
    #[serde(flatten)]
    pub usage: Usage,
}

/// One project split by model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectModelsRow {
    /// Project key, or folder path; absent for events no rule, repository or cwd could place.
    pub project: Option<String>,
    /// Display name: from the project map, or the folder's last component.
    pub name: Option<String>,
    /// The project's models, biggest API value first.
    pub models: Vec<ModelSlice>,
}

/// Projects of `filters` grouped by `grouping` and split by model, biggest API value first. Like [`crate::models`], the
/// split leaves Claude extra usage out; paid cost carries each event's share of the fees.
///
/// # Errors
///
/// Store, SQL or decode failures.
pub fn project_models(
    reader: &StoreReader,
    ctx: &QueryContext,
    filters: &Filters,
    grouping: ProjectGrouping,
) -> Result<Breakdown<ProjectModelsRow>, QueryError> {
    let fetched = fetch(reader, ctx, filters, grouping.model_dim(), Scope::Usage)?;
    let mut by_project: BTreeMap<Option<String>, BTreeMap<String, Usage>> = BTreeMap::new();
    for row in &fetched.rows {
        by_project
            .entry(row.cell.k1.clone())
            .or_default()
            .entry(row.cell.k2.clone().unwrap_or_default())
            .or_default()
            .add(row);
    }
    let names = RowNames::load(reader, grouping)?;
    let mut rows: Vec<(i64, ProjectModelsRow)> = by_project
        .into_iter()
        .map(|(project, models)| {
            let mut models: Vec<ModelSlice> = models
                .into_iter()
                .map(|(model, usage)| ModelSlice { model, usage })
                .collect();
            models.sort_by(|a, b| {
                b.usage
                    .api_value_micros
                    .cmp(&a.usage.api_value_micros)
                    .then_with(|| a.model.cmp(&b.model))
            });
            let api: i64 = models.iter().map(|m| m.usage.api_value_micros).sum();
            let name = names.of(project.as_ref());
            (
                api,
                ProjectModelsRow {
                    project,
                    name,
                    models,
                },
            )
        })
        .collect();
    rows.sort_by(|(a_api, a), (b_api, b)| b_api.cmp(a_api).then_with(|| a.project.cmp(&b.project)));
    Ok(Breakdown {
        currency: ctx.currency(),
        rows: rows.into_iter().map(|(_, row)| row).collect(),
    })
}

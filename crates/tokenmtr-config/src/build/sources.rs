//! Validation of `[[sources]]`: Superset directories and SQLite export files.

use camino::Utf8Path;
use jiff::SignedDuration;
use tokenmtr_core::SourceId;

use super::metrics::{at_least, globs};
use super::{Checked, non_empty_unique, optional_path, reject, unique};
use crate::env::Env;
use crate::error::{ConfigPath, Violation};
use crate::model::{MIN_SOURCE_INTERVAL, SourceConfig, SourceTarget, SqliteSource, SupersetSource};
use crate::raw::{RawConfig, RawSource, RawSourceKind};

const DEFAULT_SOURCE_INTERVAL: SignedDuration = SignedDuration::from_mins(5);

/// The declared sources, plus the implicit Superset one when none is declared.
pub(super) fn build_sources(raw: &RawConfig, env: &dyn Env) -> Checked<Vec<SourceConfig>> {
    let mut sources: Vec<SourceConfig> = Vec::with_capacity(raw.sources.len() + 1);
    for (i, source) in raw.sources.iter().enumerate() {
        let at = ConfigPath::root().key("sources").index(i);
        if sources.iter().any(|s| s.id == source.id) {
            return Err(reject(
                &at.key("id"),
                Violation::DuplicateSourceId(source.id.clone()),
            ));
        }
        let built = SourceConfig {
            id: source.id.clone(),
            enabled: source.enabled.unwrap_or(true),
            target: build_target(source, &at, env)?,
        };
        if let Some(other) = sources.iter().find(|s| same_files(s, &built)) {
            return Err(reject(
                &at.key("kind"),
                Violation::SourceTargetClash(other.id.clone()),
            ));
        }
        sources.push(built);
    }
    if !sources
        .iter()
        .any(|s| matches!(s.target, SourceTarget::Superset(_)))
    {
        let id = SourceId::implicit_superset();
        if let Some(i) = sources.iter().position(|s| s.id == id) {
            return Err(reject(
                &ConfigPath::root().key("sources").index(i).key("id"),
                Violation::ReservedSourceId,
            ));
        }
        sources.insert(
            0,
            SourceConfig {
                id,
                enabled: true,
                target: SourceTarget::Superset(SupersetSource { dir: None }),
            },
        );
    }
    Ok(sources)
}

/// Whether two sources would read the same files: the same Superset directory (an unset `dir`
/// is `~/.superset`, which the config does not know, so it is not compared with a spelled-out
/// one) or an `include` pattern in common.
fn same_files(a: &SourceConfig, b: &SourceConfig) -> bool {
    match (&a.target, &b.target) {
        (SourceTarget::Superset(x), SourceTarget::Superset(y)) => x.dir == y.dir,
        (SourceTarget::Sqlite(x), SourceTarget::Sqlite(y)) => x
            .include
            .patterns()
            .iter()
            .any(|pattern| y.include.patterns().contains(pattern)),
        _ => false,
    }
}

fn build_target(source: &RawSource, at: &ConfigPath, env: &dyn Env) -> Checked<SourceTarget> {
    let not_allowed = |kind: &'static str, field: &'static str| {
        reject(
            &at.key(field),
            Violation::SourceFieldNotAllowed { kind, field },
        )
    };
    match source.kind {
        RawSourceKind::Superset => {
            if source.include.is_some() {
                return Err(not_allowed("superset", "include"));
            }
            if source.exclude.is_some() {
                return Err(not_allowed("superset", "exclude"));
            }
            if source.interval.is_some() {
                return Err(not_allowed("superset", "interval"));
            }
            Ok(SourceTarget::Superset(SupersetSource {
                dir: optional_path(env, source.dir.as_deref(), &at.key("dir"))?,
            }))
        }
        RawSourceKind::Sqlite => {
            if source.dir.is_some() {
                return Err(not_allowed("sqlite", "dir"));
            }
            let include_at = at.key("include");
            let Some(raw_include) = &source.include else {
                return Err(reject(
                    at,
                    Violation::SourceFieldRequired {
                        kind: "sqlite",
                        field: "include",
                    },
                ));
            };
            non_empty_unique(raw_include, &include_at)?;
            let include = globs(Some(raw_include), env, &include_at)?;
            if let Some((i, pattern)) = include
                .patterns()
                .iter()
                .enumerate()
                .find(|(_, pattern)| !Utf8Path::new(pattern).is_absolute())
            {
                return Err(reject(
                    &include_at.index(i),
                    Violation::RelativePath {
                        path: pattern.clone(),
                    },
                ));
            }
            let exclude_at = at.key("exclude");
            if let Some(exclude) = &source.exclude {
                unique(exclude, &exclude_at)?;
            }
            Ok(SourceTarget::Sqlite(SqliteSource {
                include,
                exclude: globs(source.exclude.as_deref(), env, &exclude_at)?,
                interval: at_least(
                    source.interval,
                    DEFAULT_SOURCE_INTERVAL,
                    (MIN_SOURCE_INTERVAL, "30s"),
                    &at.key("interval"),
                )?,
            }))
        }
    }
}

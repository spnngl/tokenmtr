//! Claude `stats-cache.json` importer (D38): coarse per-day, per-model totals for days whose
//! transcripts Claude Code already deleted.
//!
//! The file format is not documented. The parser reads only the fields below and ignores the
//! rest; a file or row that does not match is counted, never guessed at (D42).
//!
//! ```json
//! { "dailyModelTokens": [ { "date": "2025-10-01", "tokensByModel": { "<model>": 12345 } } ],
//!   "modelUsage": { "<model>": { "inputTokens": 1, "outputTokens": 2,
//!                                "cacheReadInputTokens": 3, "cacheCreationInputTokens": 4 } } }
//! ```

use std::collections::BTreeMap;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde_json::Value;
use tokenmtr_core::{
    AgentId, DedupKey, EventKind, ModelId, ParsedEvent, ProfileId, ProviderId, SessionId,
    TokenCounts,
};

/// The session every imported aggregate carries. Core requires a session; D38 asks for none.
pub const AGGREGATE_SESSION: &str = "stats-cache";

/// Outcome of reading one `stats-cache.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatsCache {
    /// One event per (day, model), `EventKind::DailyAggregate`, flagged approximate.
    pub events: Vec<ParsedEvent>,
    /// Day rows that could not be turned into an event.
    pub bad_rows: u64,
    /// Whether the file has the `dailyModelTokens` list at all. `false` is format drift.
    pub recognised: bool,
}

/// Lifetime token mix of one model, used to split a day's total over the columns.
#[derive(Debug, Clone, Copy, Default)]
struct Mix {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

impl Mix {
    fn from_value(value: &Value) -> Self {
        let field = |name: &str| value.get(name).and_then(Value::as_u64).unwrap_or(0);
        Self {
            input: field("inputTokens"),
            output: field("outputTokens"),
            cache_read: field("cacheReadInputTokens"),
            cache_write: field("cacheCreationInputTokens"),
        }
    }

    /// Splits `total` by the mix with the largest-remainder method, so the parts sum to `total`.
    /// Without any lifetime ratio everything is counted as uncached input, the cheapest guess
    /// that never overstates cost.
    fn split(self, total: u64) -> TokenCounts {
        let weights = [self.input, self.output, self.cache_read, self.cache_write];
        let sum: u128 = weights.iter().map(|w| u128::from(*w)).sum();
        if sum == 0 {
            return TokenCounts {
                input_uncached: total,
                ..TokenCounts::default()
            };
        }
        let total_wide = u128::from(total);
        let mut parts = weights.map(|w| u128::from(w) * total_wide / sum);
        let remainders = weights.map(|w| u128::from(w) * total_wide % sum);
        let mut order = [0_usize, 1, 2, 3];
        order.sort_by_key(|i| std::cmp::Reverse(remainders[*i]));
        let assigned: u128 = parts.iter().sum();
        let missing = usize::try_from(total_wide - assigned).unwrap_or(0);
        for index in order.into_iter().take(missing) {
            parts[index] += 1;
        }
        let [input, output, cache_read, cache_write] = parts.map(|p| u64::try_from(p).unwrap_or(0));
        TokenCounts {
            input_uncached: input,
            output,
            cache_read,
            cache_write_5m: cache_write,
            ..TokenCounts::default()
        }
    }
}

/// Parses a `stats-cache.json` body. Each day becomes events at 12:00 in `zone` (D38).
#[must_use]
pub fn parse(body: &[u8], profile: &ProfileId, zone: &TimeZone) -> StatsCache {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return StatsCache::default();
    };
    let Some(days) = root.get("dailyModelTokens").and_then(Value::as_array) else {
        return StatsCache::default();
    };
    let mixes: BTreeMap<&str, Mix> = root
        .get("modelUsage")
        .and_then(Value::as_object)
        .map(|models| {
            models
                .iter()
                .map(|(model, usage)| (model.as_str(), Mix::from_value(usage)))
                .collect()
        })
        .unwrap_or_default();
    let mut out = StatsCache {
        recognised: true,
        ..StatsCache::default()
    };
    for day in days {
        let Some(models) = day.get("tokensByModel").and_then(Value::as_object) else {
            out.bad_rows += 1;
            continue;
        };
        let Some(date) = day
            .get("date")
            .and_then(Value::as_str)
            .and_then(|text| text.parse::<Date>().ok())
        else {
            out.bad_rows += 1;
            continue;
        };
        // Sorted: `serde_json/preserve_order` may be enabled by another crate in the build.
        let sorted: BTreeMap<&String, &Value> = models.iter().collect();
        for (model, total) in sorted {
            let mix = mixes.get(model.as_str()).copied().unwrap_or_default();
            match event(profile, zone, date, model, total.as_u64(), mix) {
                Some(event) => out.events.push(event),
                None => out.bad_rows += 1,
            }
        }
    }
    out
}

fn event(
    profile: &ProfileId,
    zone: &TimeZone,
    date: Date,
    model: &str,
    total: Option<u64>,
    mix: Mix,
) -> Option<ParsedEvent> {
    let total = total?;
    let at = date
        .at(12, 0, 0, 0)
        .to_zoned(zone.clone())
        .ok()?
        .timestamp();
    Some(ParsedEvent {
        agent: AgentId::CLAUDE,
        profile: profile.clone(),
        dedup_key: DedupKey::try_from(format!("stats-cache|{date}|{model}")).ok()?,
        at,
        session: SessionId::try_from(AGGREGATE_SESSION).ok()?,
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: None,
        git_branch: None,
        source_file: None,
        approximate: true,
        provider: ProviderId::try_from("anthropic").ok(),
        model: ModelId::try_from(model).ok()?,
        kind: EventKind::DailyAggregate,
        tokens: mix.split(total),
        reported_cost: None,
        service_tier: None,
        modifiers: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../../fixtures/claude/stats-cache.json");

    fn zone(name: &str) -> TimeZone {
        TimeZone::get(name).expect("zone")
    }

    #[test]
    fn fixture_yields_one_event_per_day_and_model() {
        let cache = parse(
            FIXTURE.as_bytes(),
            &ProfileId::default_profile(),
            &zone("Europe/Paris"),
        );
        assert!(cache.recognised);
        assert_eq!(cache.bad_rows, 1, "the row without a date");
        let keys: Vec<&str> = cache.events.iter().map(|e| e.dedup_key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "stats-cache|2025-09-01|claude-opus-4-1",
                "stats-cache|2025-09-01|claude-sonnet-4-5",
                "stats-cache|2025-09-02|claude-sonnet-4-5",
            ]
        );
        assert!(cache.events.iter().all(|e| e.approximate));
        assert!(
            cache
                .events
                .iter()
                .all(|e| e.kind == EventKind::DailyAggregate)
        );
    }

    #[test]
    fn events_sit_at_noon_in_the_display_zone() {
        let paris = parse(
            FIXTURE.as_bytes(),
            &ProfileId::default_profile(),
            &zone("Europe/Paris"),
        );
        let first = paris.events.first().expect("event").at;
        assert_eq!(first.to_string(), "2025-09-01T10:00:00Z");
    }

    #[test]
    fn totals_survive_the_split() {
        let cache = parse(
            FIXTURE.as_bytes(),
            &ProfileId::default_profile(),
            &TimeZone::UTC,
        );
        let sonnet = cache
            .events
            .iter()
            .find(|e| {
                e.dedup_key
                    .as_str()
                    .ends_with("2025-09-01|claude-sonnet-4-5")
            })
            .expect("event");
        let tokens = sonnet.tokens;
        assert_eq!(
            tokens.input_uncached + tokens.output + tokens.cache_read + tokens.cache_write_5m,
            1000
        );
        assert!(tokens.cache_read > tokens.output, "cache dominates the mix");
    }

    #[test]
    fn a_model_without_lifetime_usage_counts_as_uncached_input() {
        let mix = Mix::default();
        assert_eq!(mix.split(7).input_uncached, 7);
    }

    #[test]
    fn largest_remainder_keeps_the_sum_exact() {
        let mix = Mix {
            input: 1,
            output: 1,
            cache_read: 1,
            cache_write: 0,
        };
        let tokens = mix.split(10);
        assert_eq!(
            tokens.input_uncached + tokens.output + tokens.cache_read,
            10
        );
    }

    #[test]
    fn unknown_shapes_are_unrecognised() {
        let profile = ProfileId::default_profile();
        assert!(!parse(b"[]", &profile, &TimeZone::UTC).recognised);
        assert!(!parse(b"not json", &profile, &TimeZone::UTC).recognised);
        assert!(!parse(br#"{"days":[]}"#, &profile, &TimeZone::UTC).recognised);
    }
}

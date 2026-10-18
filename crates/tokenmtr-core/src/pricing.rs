//! API-cost estimation from `LiteLLM`'s price list (D10, D11).
//!
//! Rates are read through `rust_decimal`, never `f64`, and kept as integer pico-USD per token.
//! Costs are summed in pico-USD and rounded once to micro-USD.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use crate::event::{Estimate, ParsedEvent, TokenCounts};
use crate::ids::ModelId;
use crate::ids::ProviderId;
use crate::money::UsdMicros;

/// The `LiteLLM` snapshot vendored in the repository, embedded at compile time so the app prices
/// events offline from first run.
pub const EMBEDDED_LITELLM: &[u8] = include_bytes!("../../../pricing/litellm.json");

const PICO_PER_USD: i64 = 1_000_000_000_000;
const PICO_PER_MICRO: u128 = 1_000_000;
/// Prefixes that route one model through another vendor's platform (step 3 of the match).
const ROUTING_PREFIXES: &[&str] = &[
    "anthropic.",
    "us.",
    "eu.",
    "apac.",
    "au.",
    "jp.",
    "us-gov.",
    "global.",
];

/// Why a pricing snapshot or override was rejected.
#[derive(Debug, thiserror::Error)]
pub enum PricingError {
    /// The body is not a JSON object of entries.
    #[error("pricing snapshot is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The snapshot parsed but holds no usable chat model.
    #[error("pricing snapshot contains no priced chat model")]
    NoChatModels,
    /// A pricing version is not 12 lowercase hex characters.
    #[error("invalid pricing version {0:?}")]
    BadVersion(String),
    /// An override rate is negative or out of range.
    #[error("invalid override rate for model {model}")]
    BadOverrideRate {
        /// The override's model.
        model: String,
    },
}

/// First 12 hex characters of the sha256 of a snapshot body (D11).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "String")]
pub struct PricingVersion(String);

impl PricingVersion {
    /// Hashes a snapshot body.
    #[must_use]
    pub fn of_body(body: &[u8]) -> Self {
        let digest = Sha256::digest(body);
        let hex = digest
            .iter()
            .take(6)
            .fold(String::with_capacity(12), |mut acc, b| {
                use fmt::Write as _;
                // Writing to a `String` cannot fail.
                let _ = write!(acc, "{b:02x}");
                acc
            });
        Self(hex)
    }

    /// The version as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PricingVersion {
    type Error = PricingError;
    fn try_from(value: String) -> Result<Self, PricingError> {
        let ok = value.len() == 12
            && value
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if ok {
            Ok(Self(value))
        } else {
            Err(PricingError::BadVersion(value))
        }
    }
}

impl fmt::Display for PricingVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for PricingVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// `LiteLLM` price tier selected by the event's service tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Variant {
    Standard,
    Priority,
    Flex,
}

impl Variant {
    fn of(service_tier: Option<&str>) -> Self {
        match service_tier {
            Some(t) if t.eq_ignore_ascii_case("priority") => Self::Priority,
            Some(t) if t.eq_ignore_ascii_case("flex") => Self::Flex,
            _ => Self::Standard,
        }
    }
}

/// Per-token rates in pico-USD for one (variant, threshold) cell. Missing fields fall back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TierRates {
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    write_5m: Option<u64>,
    write_1h: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
enum Field {
    Input,
    Output,
    CacheRead,
    Write5m,
    Write1h,
}

impl TierRates {
    const fn get(&self, field: Field) -> Option<u64> {
        match field {
            Field::Input => self.input,
            Field::Output => self.output,
            Field::CacheRead => self.cache_read,
            Field::Write5m => self.write_5m,
            Field::Write1h => self.write_1h,
        }
    }

    fn set(&mut self, field: Field, pico: u64) {
        let slot = match field {
            Field::Input => &mut self.input,
            Field::Output => &mut self.output,
            Field::CacheRead => &mut self.cache_read,
            Field::Write5m => &mut self.write_5m,
            Field::Write1h => &mut self.write_1h,
        };
        *slot = Some(pico);
    }
}

/// Fully resolved per-token rates for one event, in pico-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Resolved {
    input: u64,
    output: u64,
    cache_read: u64,
    write_5m: u64,
    write_1h: u64,
}

impl Resolved {
    /// Anthropic's multipliers apply to whatever cache rate the source lacks (D11).
    fn with_fallbacks(input: u64, output: u64, pick: impl Fn(Field) -> Option<u64>) -> Self {
        Self {
            input,
            output,
            cache_read: pick(Field::CacheRead).unwrap_or_else(|| scale(input, 1, 10)),
            write_5m: pick(Field::Write5m).unwrap_or_else(|| scale(input, 5, 4)),
            write_1h: pick(Field::Write1h).unwrap_or_else(|| scale(input, 2, 1)),
        }
    }

    /// Cost in micro-USD, rounded half up, or `None` when it overflows `i64`.
    fn cost(&self, t: &TokenCounts) -> Option<UsdMicros> {
        let pico: u128 = [
            (t.input_uncached, self.input),
            (t.cache_read, self.cache_read),
            (t.cache_write_5m, self.write_5m),
            (t.cache_write_1h, self.write_1h),
            (t.output, self.output),
        ]
        .into_iter()
        .map(|(tokens, rate)| u128::from(tokens) * u128::from(rate))
        .sum();
        let micros = (pico + PICO_PER_MICRO / 2) / PICO_PER_MICRO;
        i64::try_from(micros).ok().map(UsdMicros::new)
    }
}

/// `value × num / den`, rounded half up.
fn scale(value: u64, num: u64, den: u64) -> u64 {
    let scaled = (u128::from(value) * u128::from(num) + u128::from(den) / 2) / u128::from(den);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// Converts a USD-per-token decimal to integer pico-USD per token (rounded), rejecting negatives.
fn to_pico(usd_per_token: Decimal) -> Option<u64> {
    usd_per_token
        .checked_mul(Decimal::from(PICO_PER_USD))?
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .to_u64()
}

/// Reads a JSON number's source text exactly, without going through `f64`.
fn pico_from_json(raw: &RawValue) -> Option<u64> {
    let text = raw.get().trim();
    let value = text
        .parse::<Decimal>()
        .or_else(|_| Decimal::from_scientific(text))
        .ok()?;
    to_pico(value)
}

/// Splits `<base>[_above_<N>k_tokens][_priority|_flex]` and maps `<base>` to a [`Field`].
fn parse_rate_key(key: &str) -> Option<(Field, u64, Variant)> {
    let (rest, variant) = if let Some(r) = key.strip_suffix("_priority") {
        (r, Variant::Priority)
    } else if let Some(r) = key.strip_suffix("_flex") {
        (r, Variant::Flex)
    } else {
        (key, Variant::Standard)
    };
    let (base, threshold) = match rest
        .strip_suffix("k_tokens")
        .and_then(|r| r.rsplit_once("_above_"))
    {
        Some((base, thousands)) => (base, thousands.parse::<u64>().ok()?.checked_mul(1000)?),
        None => (rest, 0),
    };
    let field = match base {
        "input_cost_per_token" => Field::Input,
        "output_cost_per_token" => Field::Output,
        "cache_read_input_token_cost" => Field::CacheRead,
        "cache_creation_input_token_cost" => Field::Write5m,
        "cache_creation_input_token_cost_above_1hr" => Field::Write1h,
        _ => return None,
    };
    Some((field, threshold, variant))
}

/// Rates of one catalog entry, keyed by (variant, prompt-size threshold in tokens).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelRates {
    tiers: BTreeMap<(Variant, u64), TierRates>,
}

impl ModelRates {
    fn from_raw(raw: &RawValue) -> Option<Self> {
        let entry: HashMap<String, Box<RawValue>> = serde_json::from_str(raw.get()).ok()?;
        let mode: String = serde_json::from_str(entry.get("mode")?.get()).ok()?;
        if mode != "chat" {
            return None;
        }
        let mut tiers: BTreeMap<(Variant, u64), TierRates> = BTreeMap::new();
        for (key, value) in &entry {
            let Some((field, threshold, variant)) = parse_rate_key(key) else {
                continue;
            };
            if let Some(pico) = pico_from_json(value) {
                tiers
                    .entry((variant, threshold))
                    .or_default()
                    .set(field, pico);
            }
        }
        let base = tiers.get(&(Variant::Standard, 0))?;
        (base.input.is_some() && base.output.is_some()).then_some(Self { tiers })
    }

    /// Highest applicable threshold wins, per field; the event's variant first, then standard.
    fn pick(&self, variant: Variant, prompt_tokens: u64, field: Field) -> Option<u64> {
        [variant, Variant::Standard].into_iter().find_map(|v| {
            self.tiers
                .iter()
                .rev()
                .filter(|((tv, threshold), _)| {
                    *tv == v && (*threshold == 0 || prompt_tokens > *threshold)
                })
                .find_map(|(_, rates)| rates.get(field))
        })
    }

    fn resolve(&self, variant: Variant, prompt_tokens: u64) -> Option<Resolved> {
        let pick = |field| self.pick(variant, prompt_tokens, field);
        Some(Resolved::with_fallbacks(
            pick(Field::Input)?,
            pick(Field::Output)?,
            pick,
        ))
    }
}

/// A user-defined price that beats `LiteLLM` (`[[pricing.overrides]]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    model: ModelId,
    rates: TierRates,
}

impl Override {
    /// Builds an override from USD-per-million-token rates. Missing cache rates fall back to the
    /// 0.1× / 1.25× / 2× multipliers.
    ///
    /// # Errors
    ///
    /// [`PricingError::BadOverrideRate`] when a rate is negative or overflows.
    pub fn new(
        model: ModelId,
        input_per_m: Decimal,
        output_per_m: Decimal,
        cache_read_per_m: Option<Decimal>,
        cache_write_five_min_per_m: Option<Decimal>,
        cache_write_one_hour_per_m: Option<Decimal>,
    ) -> Result<Self, PricingError> {
        let bad = || PricingError::BadOverrideRate {
            model: model.as_str().to_owned(),
        };
        // 1 USD per million tokens = 1e-6 USD per token.
        let pico = |per_m: Decimal| to_pico(per_m / Decimal::from(1_000_000)).ok_or_else(bad);
        let optional = |per_m: Option<Decimal>| per_m.map(pico).transpose();
        let rates = TierRates {
            input: Some(pico(input_per_m)?),
            output: Some(pico(output_per_m)?),
            cache_read: optional(cache_read_per_m)?,
            write_5m: optional(cache_write_five_min_per_m)?,
            write_1h: optional(cache_write_one_hour_per_m)?,
        };
        Ok(Self { model, rates })
    }

    fn resolve(&self) -> Option<Resolved> {
        Some(Resolved::with_fallbacks(
            self.rates.input?,
            self.rates.output?,
            |field| self.rates.get(field),
        ))
    }
}

/// A parsed `LiteLLM` snapshot: chat models only, rates as integer pico-USD per token.
#[derive(Debug, Clone)]
pub struct PricingCatalog {
    version: PricingVersion,
    models: HashMap<String, ModelRates>,
}

impl PricingCatalog {
    /// Parses a `model_prices_and_context_window.json` body (D11). Entries that are not
    /// `mode == "chat"` or lack a base input/output rate are dropped.
    ///
    /// # Errors
    ///
    /// [`PricingError::Json`] for malformed JSON, [`PricingError::NoChatModels`] when nothing
    /// usable remains.
    pub fn parse(body: &[u8]) -> Result<Self, PricingError> {
        let entries: HashMap<String, Box<RawValue>> = serde_json::from_slice(body)?;
        let models: HashMap<String, ModelRates> = entries
            .into_iter()
            .filter_map(|(name, raw)| ModelRates::from_raw(&raw).map(|rates| (name, rates)))
            .collect();
        if models.is_empty() {
            return Err(PricingError::NoChatModels);
        }
        Ok(Self {
            version: PricingVersion::of_body(body),
            models,
        })
    }

    /// Parses the snapshot embedded in the binary ([`EMBEDDED_LITELLM`]).
    ///
    /// # Errors
    ///
    /// Same as [`PricingCatalog::parse`]; a test guarantees the vendored file parses.
    pub fn embedded() -> Result<Self, PricingError> {
        Self::parse(EMBEDDED_LITELLM)
    }

    /// The snapshot's content hash.
    #[must_use]
    pub const fn version(&self) -> &PricingVersion {
        &self.version
    }

    /// Number of priced chat models.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the catalog holds no model (never true for a parsed catalog).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// First hit wins: exact id, `{provider}/{model}`, id without routing prefix, id without
    /// trailing `-YYYYMMDD` (D11).
    fn find(&self, model: &ModelId, provider: Option<&ProviderId>) -> Option<&ModelRates> {
        let id = model.as_str();
        let stripped = strip_routing_prefixes(id);
        let routed = provider.map(|p| format!("{p}/{id}"));
        let undated = strip_date_suffix(stripped);
        [Some(id), routed.as_deref(), Some(stripped), undated]
            .into_iter()
            .flatten()
            .find_map(|key| self.models.get(key))
    }
}

fn strip_routing_prefixes(mut id: &str) -> &str {
    while let Some(rest) = ROUTING_PREFIXES.iter().find_map(|p| id.strip_prefix(p)) {
        id = rest;
    }
    id
}

/// `claude-sonnet-4-20250514` → `claude-sonnet-4`.
fn strip_date_suffix(id: &str) -> Option<&str> {
    let (head, tail) = id.rsplit_once('-')?;
    (tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit())).then_some(head)
}

/// Estimates the API cost of one event. Overrides win over `LiteLLM`. `None` means unpriced: no
/// rate matched (never guess silently, D10).
#[must_use]
pub fn estimate(
    event: &ParsedEvent,
    catalog: &PricingCatalog,
    overrides: &[Override],
) -> Option<Estimate> {
    let resolved = match overrides.iter().find(|o| o.model == event.model) {
        Some(over) => over.resolve()?,
        None => catalog
            .find(&event.model, event.provider.as_ref())?
            .resolve(
                Variant::of(event.service_tier.as_deref()),
                event.tokens.prompt_tokens(),
            )?,
    };
    Some(Estimate {
        usd: resolved.cost(&event.tokens)?,
        pricing_version: catalog.version.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use crate::ids::{AgentId, DedupKey, ProfileId, SessionId};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Hand-written excerpt in `LiteLLM`'s format. Sonnet 4.5 carries Anthropic's published
    /// prices: $3 in, $15 out, $0.30 cache read, $3.75 5m write, $6 1h write per million tokens;
    /// above 200k prompt tokens: $6 / $22.50 / $0.60 / $7.50.
    const FIXTURE: &str = r#"{
      "sample_spec": {"mode": "chat", "note": "no rates"},
      "claude-sonnet-4-5": {
        "litellm_provider": "anthropic", "mode": "chat", "max_input_tokens": 1000000,
        "input_cost_per_token": 3e-06, "output_cost_per_token": 1.5e-05,
        "cache_read_input_token_cost": 3e-07,
        "cache_creation_input_token_cost": 3.75e-06,
        "cache_creation_input_token_cost_above_1hr": 6e-06,
        "input_cost_per_token_above_200k_tokens": 6e-06,
        "output_cost_per_token_above_200k_tokens": 2.25e-05,
        "cache_read_input_token_cost_above_200k_tokens": 6e-07,
        "cache_creation_input_token_cost_above_200k_tokens": 7.5e-06,
        "input_cost_per_token_batches": 1.5e-06
      },
      "claude-exact": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 1e-06},
      "openrouter/anthropic/claude-routed": {"mode": "chat", "input_cost_per_token": 2e-06, "output_cost_per_token": 2e-06},
      "claude-prefixed": {"mode": "chat", "input_cost_per_token": 3e-06, "output_cost_per_token": 3e-06},
      "claude-dated": {"mode": "chat", "input_cost_per_token": 4e-06, "output_cost_per_token": 4e-06},
      "multi-tier": {
        "mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 1e-06,
        "input_cost_per_token_above_200k_tokens": 2e-06, "output_cost_per_token_above_200k_tokens": 2e-06,
        "input_cost_per_token_above_272k_tokens": 3e-06, "output_cost_per_token_above_272k_tokens": 3e-06
      },
      "gpt-tiered": {
        "mode": "chat", "input_cost_per_token": 1.25e-06, "output_cost_per_token": 1e-05,
        "cache_read_input_token_cost": 1.25e-07,
        "input_cost_per_token_priority": 2.5e-06, "output_cost_per_token_priority": 2e-05,
        "input_cost_per_token_flex": 6.25e-07, "output_cost_per_token_flex": 5e-06
      },
      "plain": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06},
      "embedder": {"mode": "embedding", "input_cost_per_token": 1e-07, "output_cost_per_token": 0}
    }"#;

    fn catalog() -> Result<PricingCatalog, PricingError> {
        PricingCatalog::parse(FIXTURE.as_bytes())
    }

    fn event(
        model: &str,
        provider: Option<&str>,
        tokens: TokenCounts,
    ) -> Result<ParsedEvent, Box<dyn std::error::Error>> {
        Ok(ParsedEvent {
            agent: AgentId::CLAUDE,
            profile: ProfileId::default_profile(),
            dedup_key: DedupKey::try_from("k")?,
            at: jiff::Timestamp::UNIX_EPOCH,
            session: SessionId::try_from("s")?,
            parent_session: None,
            subagent: None,
            is_sidechain: false,
            cwd: None,
            git_branch: None,
            source_file: None,
            approximate: false,
            provider: provider.map(ProviderId::try_from).transpose()?,
            model: ModelId::try_from(model)?,
            kind: EventKind::Message,
            tokens,
            reported_cost: None,
            service_tier: None,
        })
    }

    fn usd(
        model: &str,
        provider: Option<&str>,
        tokens: TokenCounts,
    ) -> Result<Option<i64>, Box<dyn std::error::Error>> {
        let ev = event(model, provider, tokens)?;
        Ok(estimate(&ev, &catalog()?, &[]).map(|e| e.usd.get()))
    }

    fn input(n: u64, output: u64) -> TokenCounts {
        TokenCounts {
            input_uncached: n,
            output,
            ..TokenCounts::default()
        }
    }

    #[test]
    fn only_chat_models_with_base_rates_are_kept() -> TestResult {
        let c = catalog()?;
        assert_eq!(c.len(), 8);
        assert_eq!(usd("embedder", None, input(10, 0))?, None);
        assert_eq!(usd("sample_spec", None, input(10, 0))?, None);
        Ok(())
    }

    #[test]
    fn claude_estimate_equals_published_price() -> TestResult {
        let tokens = TokenCounts {
            input_uncached: 1000,
            cache_read: 20_000,
            cache_write_5m: 3000,
            cache_write_1h: 2000,
            output: 500,
            reasoning: 0,
        };
        // 1000×$3 + 20000×$0.30 + 3000×$3.75 + 2000×$6 + 500×$15, per million tokens
        // = 0.003 + 0.006 + 0.01125 + 0.012 + 0.0075 = $0.03975.
        assert_eq!(
            usd("claude-sonnet-4-5", Some("anthropic"), tokens)?,
            Some(39_750)
        );
        Ok(())
    }

    #[test]
    fn five_matching_steps() -> TestResult {
        let t = input(1_000_000, 0);
        // 1. exact
        assert_eq!(usd("claude-exact", None, t)?, Some(1_000_000));
        // 2. {provider}/{model}
        assert_eq!(
            usd("anthropic/claude-routed", Some("openrouter"), t)?,
            Some(2_000_000)
        );
        assert_eq!(usd("anthropic/claude-routed", None, t)?, None);
        // 3. routing prefixes stripped
        assert_eq!(
            usd("us.anthropic.claude-prefixed", None, t)?,
            Some(3_000_000)
        );
        // 4. trailing -YYYYMMDD stripped
        assert_eq!(usd("claude-dated-20250101", None, t)?, Some(4_000_000));
        assert_eq!(
            usd("us.anthropic.claude-dated-20250101", None, t)?,
            Some(4_000_000)
        );
        // 5. unpriced
        assert_eq!(usd("claude-unknown", Some("anthropic"), t)?, None);
        Ok(())
    }

    #[test]
    fn tier_above_200k_uses_tier_rates_for_every_column() -> TestResult {
        // Exactly 200k is not above the threshold.
        assert_eq!(
            usd("claude-sonnet-4-5", None, input(200_000, 0))?,
            Some(600_000)
        );
        // 250k × $6 + 1000 × $22.50 per million = 1.5 + 0.0225.
        assert_eq!(
            usd("claude-sonnet-4-5", None, input(250_000, 1000))?,
            Some(1_522_500)
        );
        Ok(())
    }

    #[test]
    fn highest_threshold_below_prompt_size_wins() -> TestResult {
        assert_eq!(usd("multi-tier", None, input(100_000, 0))?, Some(100_000));
        assert_eq!(usd("multi-tier", None, input(250_000, 0))?, Some(500_000));
        assert_eq!(usd("multi-tier", None, input(272_000, 0))?, Some(544_000));
        assert_eq!(usd("multi-tier", None, input(300_000, 0))?, Some(900_000));
        Ok(())
    }

    #[test]
    fn cache_write_1h_uses_its_own_field() -> TestResult {
        let t = TokenCounts {
            cache_write_1h: 1_000_000,
            ..TokenCounts::default()
        };
        assert_eq!(usd("claude-sonnet-4-5", None, t)?, Some(6_000_000));
        Ok(())
    }

    #[test]
    fn service_tier_selects_priority_and_flex_rates() -> TestResult {
        let c = catalog()?;
        let t = TokenCounts {
            input_uncached: 1_000_000,
            cache_read: 1_000_000,
            output: 1_000_000,
            ..TokenCounts::default()
        };
        let mut ev = event("gpt-tiered", None, t)?;
        let cost = |ev: &ParsedEvent| estimate(ev, &c, &[]).map(|e| e.usd.get());
        // standard: 1.25 + 0.125 + 10
        assert_eq!(cost(&ev), Some(11_375_000));
        ev.service_tier = Some("priority".to_owned());
        // priority input/output, cache read falls back to the standard field: 2.5 + 0.125 + 20
        assert_eq!(cost(&ev), Some(22_625_000));
        ev.service_tier = Some("Flex".to_owned());
        // flex: 0.625 + 0.125 + 5
        assert_eq!(cost(&ev), Some(5_750_000));
        ev.service_tier = Some("batch".to_owned());
        assert_eq!(cost(&ev), Some(11_375_000));
        Ok(())
    }

    #[test]
    fn missing_cache_fields_use_anthropic_multipliers() -> TestResult {
        // input $1/M: read 0.1×, 5m write 1.25×, 1h write 2×.
        let m = 1_000_000;
        let read = TokenCounts {
            cache_read: m,
            ..TokenCounts::default()
        };
        let w5 = TokenCounts {
            cache_write_5m: m,
            ..TokenCounts::default()
        };
        let w1 = TokenCounts {
            cache_write_1h: m,
            ..TokenCounts::default()
        };
        assert_eq!(usd("plain", None, read)?, Some(100_000));
        assert_eq!(usd("plain", None, w5)?, Some(1_250_000));
        assert_eq!(usd("plain", None, w1)?, Some(2_000_000));
        Ok(())
    }

    #[test]
    fn overrides_beat_litellm() -> TestResult {
        let c = catalog()?;
        let ev = event("plain", None, input(1_000_000, 1_000_000))?;
        let over = Override::new(
            ModelId::try_from("plain")?,
            "10".parse()?,
            "20".parse()?,
            None,
            None,
            None,
        )?;
        assert_eq!(
            estimate(&ev, &c, &[over]).map(|e| e.usd.get()),
            Some(30_000_000)
        );
        // Not in LiteLLM at all.
        let private = event("my-private-model", None, input(1_000_000, 0))?;
        assert_eq!(estimate(&private, &c, &[]), None);
        let over = Override::new(
            ModelId::try_from("my-private-model")?,
            "3".parse()?,
            "15".parse()?,
            Some("0.5".parse()?),
            None,
            None,
        )?;
        let est = estimate(&private, &c, &[over]).ok_or("priced")?;
        assert_eq!(est.usd.get(), 3_000_000);
        assert_eq!(&est.pricing_version, c.version());
        Ok(())
    }

    #[test]
    fn override_rates_must_be_non_negative() -> TestResult {
        let model = ModelId::try_from("m")?;
        assert!(Override::new(model, "-1".parse()?, "1".parse()?, None, None, None).is_err());
        Ok(())
    }

    #[test]
    fn rounding_is_half_up_on_the_total() -> TestResult {
        // 1 token at 1e-6 USD = 1 µUSD; 1 token at 4e-7 USD = 0.4 µUSD → 0; 5e-7 → 1.
        let c = PricingCatalog::parse(
            br#"{"a":{"mode":"chat","input_cost_per_token":4e-07,"output_cost_per_token":5e-07}}"#,
        )?;
        let ev = event("a", None, input(1, 0))?;
        assert_eq!(estimate(&ev, &c, &[]).map(|e| e.usd.get()), Some(0));
        let ev = event("a", None, input(0, 1))?;
        assert_eq!(estimate(&ev, &c, &[]).map(|e| e.usd.get()), Some(1));
        Ok(())
    }

    #[test]
    fn parse_rejects_bad_bodies() {
        assert!(matches!(
            PricingCatalog::parse(b"[]"),
            Err(PricingError::Json(_))
        ));
        assert!(matches!(
            PricingCatalog::parse(b"{}"),
            Err(PricingError::NoChatModels)
        ));
        assert!(matches!(
            PricingCatalog::parse(b"nope"),
            Err(PricingError::Json(_))
        ));
    }

    #[test]
    fn rate_keys_parse_generically() {
        let key = |k| parse_rate_key(k).map(|(f, t, v)| (format!("{f:?}"), t, v));
        assert_eq!(
            key("input_cost_per_token"),
            Some(("Input".into(), 0, Variant::Standard))
        );
        assert_eq!(
            key("output_cost_per_token_above_512k_tokens"),
            Some(("Output".into(), 512_000, Variant::Standard))
        );
        assert_eq!(
            key("cache_creation_input_token_cost_above_1hr_above_200k_tokens"),
            Some(("Write1h".into(), 200_000, Variant::Standard))
        );
        assert_eq!(
            key("input_cost_per_token_above_272k_tokens_priority"),
            Some(("Input".into(), 272_000, Variant::Priority))
        );
        assert_eq!(key("input_cost_per_token_batches"), None);
        assert_eq!(key("input_cost_per_audio_token"), None);
    }

    #[test]
    fn version_is_twelve_hex_chars_of_sha256() {
        // sha256("abc") = ba7816bf8f01cfea…
        assert_eq!(PricingVersion::of_body(b"abc").as_str(), "ba7816bf8f01");
        assert!(PricingVersion::try_from("ZZ".to_owned()).is_err());
        assert!(serde_json::from_str::<PricingVersion>("\"ba7816bf8f01\"").is_ok());
    }

    #[test]
    fn vendored_snapshot_parses_and_prices_opus() -> TestResult {
        let c = PricingCatalog::embedded()?;
        assert!(c.len() > 1000);
        let tokens = TokenCounts {
            input_uncached: 1000,
            cache_read: 10_000,
            cache_write_5m: 2000,
            cache_write_1h: 1000,
            output: 500,
            reasoning: 0,
        };
        let ev = event("claude-opus-4-5-20251101", Some("anthropic"), tokens)?;
        // Opus 4.5 published: $5 in, $25 out, $0.50 read, $6.25 5m write, $10 1h write.
        // 0.005 + 0.005 + 0.0125 + 0.01 + 0.0125 = $0.045.
        assert_eq!(estimate(&ev, &c, &[]).map(|e| e.usd.get()), Some(45_000));
        Ok(())
    }
}

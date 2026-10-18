//! API-cost estimation from `LiteLLM`'s price list.
//!
//! Rates are read through `rust_decimal`, never `f64`, and kept as integer pico-USD per token.
//! Costs are summed in pico-USD and rounded once to micro-USD. Resolution follows `LiteLLM`'s own
//! cost calculator (`litellm/litellm_core_utils/llm_cost_calc/utils.py`):
//!
//! - The prompt-size tier is chosen once, from the `input_cost_per_token_above_*` keys; every
//!   column then takes that tier's rate, else its base rate. xAI thresholds are inclusive.
//! - A service tier of `priority` (or `fast`) or `flex` selects the `_priority` / `_flex` keys,
//!   falling back to the standard ones.
//! - Reasoning tokens are billed at `output_cost_per_reasoning_token` when the entry has one.
//! - Missing cache rates: Claude entries use Anthropic's multipliers of the input rate (0.1× read,
//!   1.25× 5-minute write, 2× 1-hour write). Other entries use `LiteLLM`'s fallback (the input
//!   rate for reads and writes, the 5-minute rate for 1-hour writes) and the estimate is
//!   approximate.
//! - First-party Anthropic entries apply their `provider_specific_entry` multipliers (`fast` and
//!   one per inference region) and bill `search_context_cost_per_query` per web search.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::LazyLock;

use jiff::Timestamp;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use crate::event::{Estimate, ParsedEvent, PriceModifiers, TokenCounts};
use crate::ids::{ModelId, ProviderId};
use crate::money::UsdMicros;

/// The `LiteLLM` snapshot vendored in the repository, embedded at compile time so the app prices
/// events offline from first run.
pub const EMBEDDED_LITELLM: &[u8] = include_bytes!("../../../pricing/litellm.json");
/// Records the commit and fetch date of [`EMBEDDED_LITELLM`].
const EMBEDDED_LOCK: &str = include_str!("../../../pricing/litellm.lock");
/// `LiteLLM`-shaped entries for Claude models `LiteLLM` has removed. They price an event only when
/// the catalog has no match.
const LEGACY_LITELLM: &[u8] = include_bytes!("../../../pricing/legacy.json");

/// `source` of a snapshot stored from [`EMBEDDED_LITELLM`].
pub const SOURCE_EMBEDDED: &str = "embedded";
/// `source` of a snapshot downloaded from `LiteLLM`.
pub const SOURCE_LITELLM: &str = "litellm";
/// Marks an entry [`carry_forward`] copied from an earlier snapshot.
pub const CARRIED_KEY: &str = "tokenmtr_carried";
/// Models every real `LiteLLM` catalog prices ([`PricingCatalog::check_refresh`]).
const ANCHORS: [&str; 3] = ["claude-opus-4-7", "claude-sonnet-4-5", "gpt-5"];

const PICO_PER_USD: i64 = 1_000_000_000_000;
const PICO_PER_MICRO: u128 = 1_000_000;
/// Multipliers are kept in parts per million.
const PPM: u128 = 1_000_000;
/// Prefixes that route one model through another vendor's platform, stripped one at a time.
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

static LEGACY: LazyLock<HashMap<String, ModelRates>> = LazyLock::new(|| {
    // A test guarantees the vendored file parses.
    parse_models(LEGACY_LITELLM)
        .map(|(models, _)| models)
        .unwrap_or_default()
});

/// Why a pricing snapshot or override was rejected.
#[derive(Debug, thiserror::Error)]
pub enum PricingError {
    /// The body is not a JSON object of entries.
    #[error("pricing snapshot is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The snapshot parsed but holds no usable chat model.
    #[error("pricing snapshot contains no priced chat model")]
    NoChatModels,
    /// A downloaded snapshot does not price a model every real `LiteLLM` catalog prices.
    #[error("pricing snapshot does not price {model}")]
    MissingAnchor {
        /// The missing model.
        model: &'static str,
    },
    /// A downloaded snapshot prices under half the models of the active one.
    #[error(
        "pricing snapshot prices {models} models, under half of the active snapshot's {previous}"
    )]
    Shrunk {
        /// Models the download prices.
        models: usize,
        /// Models the active snapshot prices, carried ones excluded.
        previous: usize,
    },
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

/// First 12 hex characters of a sha256: of a snapshot body, or of an override.
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

/// When [`EMBEDDED_LITELLM`] was fetched: midnight UTC of the `fetched` date in
/// `pricing/litellm.lock`.
#[must_use]
pub fn embedded_fetched_at() -> Option<Timestamp> {
    let date = EMBEDDED_LOCK
        .lines()
        .find_map(|line| line.strip_prefix("fetched = "))?
        .trim()
        .trim_matches('"');
    let date: jiff::civil::Date = date.parse().ok()?;
    date.to_zoned(jiff::tz::TimeZone::UTC)
        .ok()
        .map(|zoned| zoned.timestamp())
}

/// `LiteLLM` price tier selected by the event's service tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Variant {
    Standard,
    Priority,
    Flex,
}

impl Variant {
    /// `fast` bills as `priority`, as in `LiteLLM`.
    fn of(service_tier: Option<&str>) -> Self {
        match service_tier {
            Some(t) if t.eq_ignore_ascii_case("priority") || t.eq_ignore_ascii_case("fast") => {
                Self::Priority
            }
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
    reasoning: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
enum Field {
    Input,
    Output,
    CacheRead,
    Write5m,
    Write1h,
    Reasoning,
}

impl TierRates {
    const fn get(&self, field: Field) -> Option<u64> {
        match field {
            Field::Input => self.input,
            Field::Output => self.output,
            Field::CacheRead => self.cache_read,
            Field::Write5m => self.write_5m,
            Field::Write1h => self.write_1h,
            Field::Reasoning => self.reasoning,
        }
    }

    fn set(&mut self, field: Field, pico: u64) {
        let slot = match field {
            Field::Input => &mut self.input,
            Field::Output => &mut self.output,
            Field::CacheRead => &mut self.cache_read,
            Field::Write5m => &mut self.write_5m,
            Field::Write1h => &mut self.write_1h,
            Field::Reasoning => &mut self.reasoning,
        };
        *slot = Some(pico);
    }
}

/// Which cache rates a resolution had to guess with `LiteLLM`'s fallback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Guessed {
    cache_read: bool,
    write_5m: bool,
    write_1h: bool,
}

/// Fully resolved per-token rates for one event, in pico-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Resolved {
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
    write_5m: u64,
    write_1h: u64,
    guessed: Guessed,
}

impl Resolved {
    /// Fills the cache rates `cache` lacks: Anthropic's multipliers for Claude, else
    /// `LiteLLM`'s fallback, recorded as guessed.
    fn new(input: u64, output: u64, reasoning: u64, cache: TierRates, claude: bool) -> Self {
        let (read, minutes, hour) = if claude {
            (
                cache.cache_read.unwrap_or_else(|| scale(input, 1, 10)),
                cache.write_5m.unwrap_or_else(|| scale(input, 5, 4)),
                cache.write_1h.unwrap_or_else(|| scale(input, 2, 1)),
            )
        } else {
            let minutes = cache.write_5m.unwrap_or(input);
            (
                cache.cache_read.unwrap_or(input),
                minutes,
                cache.write_1h.unwrap_or(minutes),
            )
        };
        let guessed = Guessed {
            cache_read: !claude && cache.cache_read.is_none(),
            write_5m: !claude && cache.write_5m.is_none(),
            write_1h: !claude && cache.write_1h.is_none(),
        };
        Self {
            input,
            output,
            reasoning,
            cache_read: read,
            write_5m: minutes,
            write_1h: hour,
            guessed,
        }
    }

    /// Whether a guessed rate billed any token.
    const fn guessed_for(&self, t: &TokenCounts) -> bool {
        (self.guessed.cache_read && t.cache_read > 0)
            || (self.guessed.write_5m && t.cache_write_5m > 0)
            || (self.guessed.write_1h && t.cache_write_1h > 0)
    }

    /// Token cost in pico-USD, `None` on overflow. Reasoning tokens are part of `output`.
    fn pico(&self, t: &TokenCounts) -> Option<u128> {
        let reasoning = t.reasoning.min(t.output);
        [
            (t.input_uncached, self.input),
            (t.cache_read, self.cache_read),
            (t.cache_write_5m, self.write_5m),
            (t.cache_write_1h, self.write_1h),
            (t.output - reasoning, self.output),
            (reasoning, self.reasoning),
        ]
        .into_iter()
        .try_fold(0_u128, |sum, (tokens, rate)| {
            sum.checked_add(u128::from(tokens) * u128::from(rate))
        })
    }
}

/// Pico-USD rounded half up to micro-USD, `None` when it overflows `i64`.
fn micros(pico: u128) -> Option<UsdMicros> {
    let micros = pico.checked_add(PICO_PER_MICRO / 2)? / PICO_PER_MICRO;
    i64::try_from(micros).ok().map(UsdMicros::new)
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

/// A rate value that is not a non-negative decimal: `LiteLLM` drift, counted, never guessed.
struct BadRate;

/// Reads a JSON number, or a string holding one as `LiteLLM` accepts, exactly: never through
/// `f64`. `null` is no rate.
fn decimal_from_json(raw: &RawValue) -> Result<Option<Decimal>, BadRate> {
    let text = raw.get().trim();
    if text == "null" {
        return Ok(None);
    }
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .map_or(text, str::trim);
    text.parse::<Decimal>()
        .or_else(|_| Decimal::from_scientific(text))
        .map(Some)
        .map_err(|_| BadRate)
}

fn pico_from_json(raw: &RawValue) -> Result<Option<u64>, BadRate> {
    decimal_from_json(raw)?
        .map(|usd| to_pico(usd).ok_or(BadRate))
        .transpose()
}

/// A multiplier (`1.1`) in parts per million.
fn ppm_from_json(raw: &RawValue) -> Option<u64> {
    decimal_from_json(raw)
        .ok()
        .flatten()?
        .checked_mul(Decimal::from(1_000_000))?
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .to_u64()
}

/// Splits `<base>[_above_<N>[k]_tokens][_priority|_flex]` and maps `<base>` to a [`Field`].
fn parse_rate_key(key: &str) -> Option<(Field, u64, Variant)> {
    let (rest, variant) = if let Some(r) = key.strip_suffix("_priority") {
        (r, Variant::Priority)
    } else if let Some(r) = key.strip_suffix("_flex") {
        (r, Variant::Flex)
    } else {
        (key, Variant::Standard)
    };
    let (base, threshold) = match rest
        .strip_suffix("_tokens")
        .and_then(|r| r.rsplit_once("_above_"))
    {
        Some((base, size)) => (base, parse_threshold(size)?),
        None => (rest, 0),
    };
    let field = match base {
        "input_cost_per_token" => Field::Input,
        "output_cost_per_token" => Field::Output,
        "cache_read_input_token_cost" => Field::CacheRead,
        "cache_creation_input_token_cost" => Field::Write5m,
        "cache_creation_input_token_cost_above_1hr" => Field::Write1h,
        "output_cost_per_reasoning_token" => Field::Reasoning,
        _ => return None,
    };
    Some((field, threshold, variant))
}

/// `200k` → 200 000, `128` → 128.
fn parse_threshold(size: &str) -> Option<u64> {
    match size.strip_suffix('k') {
        Some(thousands) => thousands.parse::<u64>().ok()?.checked_mul(1000),
        None => size.parse().ok(),
    }
}

/// Rates of one catalog entry, keyed by (variant, prompt-size threshold in tokens).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelRates {
    tiers: BTreeMap<(Variant, u64), TierRates>,
    /// A Claude model: missing cache rates follow Anthropic's multipliers.
    claude: bool,
    /// xAI bills a prompt of exactly the threshold at the tier rate.
    inclusive: bool,
    /// First-party Anthropic `provider_specific_entry` multipliers in ppm: `fast` and regions.
    multipliers: BTreeMap<String, u64>,
    /// First-party Anthropic price of one web search, pico-USD.
    web_search: Option<u64>,
    /// Copied from an earlier snapshot by [`carry_forward`].
    carried: bool,
}

impl ModelRates {
    /// `None` unless the entry is `mode` `chat` or `responses` with base input and output rates.
    /// Unreadable rate values are counted in `dropped`.
    fn from_raw(name: &str, raw: &RawValue, dropped: &mut usize) -> Option<Self> {
        let entry: HashMap<String, Box<RawValue>> = serde_json::from_str(raw.get()).ok()?;
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
        };
        let mode = text("mode")?;
        if mode != "chat" && mode != "responses" {
            return None;
        }
        let mut tiers: BTreeMap<(Variant, u64), TierRates> = BTreeMap::new();
        for (key, value) in &entry {
            let Some((field, threshold, variant)) = parse_rate_key(key) else {
                continue;
            };
            match pico_from_json(value) {
                Ok(Some(pico)) => tiers
                    .entry((variant, threshold))
                    .or_default()
                    .set(field, pico),
                Ok(None) => {}
                Err(BadRate) => *dropped += 1,
            }
        }
        let base = tiers.get(&(Variant::Standard, 0))?;
        if base.input.is_none() || base.output.is_none() {
            return None;
        }
        let provider = text("litellm_provider");
        let anthropic = provider.as_deref() == Some("anthropic");
        let object = |key: &str| {
            entry
                .get(key)
                .and_then(|v| serde_json::from_str::<HashMap<String, Box<RawValue>>>(v.get()).ok())
                .unwrap_or_default()
        };
        let (multipliers, web_search) = if anthropic {
            let multipliers = object("provider_specific_entry")
                .iter()
                .filter_map(|(key, value)| Some((key.to_ascii_lowercase(), ppm_from_json(value)?)))
                .collect();
            let web_search = object("search_context_cost_per_query")
                .get("search_context_size_medium")
                .and_then(|value| pico_from_json(value).ok().flatten());
            (multipliers, web_search)
        } else {
            (BTreeMap::new(), None)
        };
        Some(Self {
            tiers,
            claude: anthropic || name.contains("claude"),
            inclusive: provider.as_deref() == Some("xai"),
            multipliers,
            web_search,
            carried: entry
                .get(CARRIED_KEY)
                .is_some_and(|value| value.get().trim() == "true"),
        })
    }

    /// The highest threshold the prompt exceeds among the input keys of the standard variant
    /// and of `variant`; 0 for the base tier.
    fn threshold(&self, variant: Variant, prompt_tokens: u64) -> u64 {
        self.tiers
            .iter()
            .filter(|((v, threshold), rates)| {
                *threshold > 0
                    && (*v == variant || *v == Variant::Standard)
                    && rates.input.is_some()
                    && (prompt_tokens > *threshold
                        || (self.inclusive && prompt_tokens == *threshold))
            })
            .map(|((_, threshold), _)| *threshold)
            .max()
            .unwrap_or(0)
    }

    fn resolve(&self, variant: Variant, prompt_tokens: u64) -> Option<Resolved> {
        let threshold = self.threshold(variant, prompt_tokens);
        let cell = |v: Variant, t: u64, field: Field| {
            self.tiers.get(&(v, t)).and_then(|rates| rates.get(field))
        };
        let pick = |field: Field| {
            [
                (variant, threshold),
                (Variant::Standard, threshold),
                (variant, 0),
                (Variant::Standard, 0),
            ]
            .into_iter()
            .find_map(|(v, t)| cell(v, t, field))
        };
        let input = pick(Field::Input)?;
        let output = pick(Field::Output)?;
        // As `LiteLLM`: the tier's reasoning rate, else its output rate when it has one, else
        // the standard reasoning rate, else the output rate.
        let reasoning = cell(variant, 0, Field::Reasoning)
            .or_else(|| {
                (variant != Variant::Standard && cell(variant, 0, Field::Output).is_some())
                    .then_some(output)
            })
            .or_else(|| cell(Variant::Standard, 0, Field::Reasoning))
            .unwrap_or(output);
        let cache = TierRates {
            cache_read: pick(Field::CacheRead),
            write_5m: pick(Field::Write5m),
            write_1h: pick(Field::Write1h),
            ..TierRates::default()
        };
        Some(Resolved::new(input, output, reasoning, cache, self.claude))
    }

    /// Applies the speed and region multipliers and adds web searches. The flag is set when
    /// `modifiers` are unknown but the entry has rates for them.
    fn modify(&self, pico: u128, modifiers: Option<&PriceModifiers>) -> Option<(u128, bool)> {
        let Some(m) = modifiers else {
            let unknown = !self.multipliers.is_empty() || self.web_search.is_some();
            return Some((pico, unknown));
        };
        let factor = |key: Option<&str>| {
            key.and_then(|k| self.multipliers.get(k))
                .map_or(PPM, |ppm| u128::from(*ppm))
        };
        let fast = factor(m.fast.then_some("fast"));
        let geo = factor(m.inference_geo.as_deref().filter(|geo| *geo != "fast"));
        let scaled = pico
            .checked_mul(fast)?
            .checked_mul(geo)?
            .checked_add(PPM * PPM / 2)?
            / (PPM * PPM);
        let searches = u128::from(m.web_search_requests)
            .checked_mul(u128::from(self.web_search.unwrap_or(0)))?;
        Some((scaled.checked_add(searches)?, false))
    }
}

/// A user-defined price that beats `LiteLLM` (`[[pricing.overrides]]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    model: ModelId,
    provider: Option<ProviderId>,
    rates: TierRates,
    version: PricingVersion,
}

impl Override {
    /// Builds an override of `model` from USD-per-million-token rates, for every provider.
    /// Missing cache rates fall back to the 0.1× / 1.25× / 2× multipliers.
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
            reasoning: None,
        };
        Ok(Self::with(model, None, rates))
    }

    /// Restricts the override to events served by `provider`. For one model, an override with a
    /// provider beats one without.
    #[must_use]
    pub fn for_provider(self, provider: ProviderId) -> Self {
        Self::with(self.model, Some(provider), self.rates)
    }

    /// The version stamped on the estimates it makes: a hash of the override itself, so editing
    /// it changes the version and re-estimates the events it priced.
    #[must_use]
    pub const fn version(&self) -> &PricingVersion {
        &self.version
    }

    fn with(model: ModelId, provider: Option<ProviderId>, rates: TierRates) -> Self {
        let r = rates;
        let text = format!(
            "override\n{}\n{}\n{:?}",
            model.as_str(),
            provider.as_ref().map_or("", ProviderId::as_str),
            [r.input, r.output, r.cache_read, r.write_5m, r.write_1h],
        );
        Self {
            version: PricingVersion::of_body(text.as_bytes()),
            model,
            provider,
            rates,
        }
    }

    fn resolve(&self) -> Option<Resolved> {
        let output = self.rates.output?;
        Some(Resolved::new(
            self.rates.input?,
            output,
            output,
            self.rates,
            true,
        ))
    }
}

/// The override for `query`: one naming its provider first, then one for every provider.
fn matching_override<'o>(
    query: &PriceQuery<'_>,
    overrides: &'o [Override],
) -> Option<&'o Override> {
    let of_model = || overrides.iter().filter(|o| o.model == *query.model);
    of_model()
        .find(|o| o.provider.is_some() && o.provider.as_ref() == query.provider)
        .or_else(|| of_model().find(|o| o.provider.is_none()))
}

/// Parses the chat and responses entries of a body; returns them with the count of unreadable
/// rate values.
fn parse_models(body: &[u8]) -> Result<(HashMap<String, ModelRates>, usize), PricingError> {
    let entries: HashMap<String, Box<RawValue>> = serde_json::from_slice(body)?;
    let mut dropped = 0;
    let models = entries
        .into_iter()
        .filter_map(|(name, raw)| {
            ModelRates::from_raw(&name, &raw, &mut dropped).map(|rates| (name, rates))
        })
        .collect();
    Ok((models, dropped))
}

/// A parsed `LiteLLM` snapshot: chat and responses models, rates as integer pico-USD per token.
#[derive(Debug, Clone)]
pub struct PricingCatalog {
    version: PricingVersion,
    models: HashMap<String, ModelRates>,
    dropped_rates: usize,
}

impl PricingCatalog {
    /// Parses a `model_prices_and_context_window.json` body. Entries that are not
    /// `mode` `chat` or `responses`, or lack a base input/output rate, are dropped.
    ///
    /// # Errors
    ///
    /// [`PricingError::Json`] for malformed JSON, [`PricingError::NoChatModels`] when nothing
    /// usable remains.
    pub fn parse(body: &[u8]) -> Result<Self, PricingError> {
        let (models, dropped_rates) = parse_models(body)?;
        if models.is_empty() {
            return Err(PricingError::NoChatModels);
        }
        Ok(Self {
            version: PricingVersion::of_body(body),
            models,
            dropped_rates,
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

    /// Number of priced models.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the catalog holds no model (never true for a parsed catalog).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Models [`carry_forward`] copied from earlier snapshots.
    #[must_use]
    pub fn carried(&self) -> usize {
        self.models.values().filter(|m| m.carried).count()
    }

    /// Rate values of chat entries that are not non-negative decimals and were ignored: a sign
    /// that `LiteLLM`'s format drifted.
    #[must_use]
    pub const fn dropped_rates(&self) -> usize {
        self.dropped_rates
    }

    /// Validates a downloaded catalog before it replaces `previous`, the active one: it
    /// must price the anchor models (`claude-opus-4-7`, `claude-sonnet-4-5`, `gpt-5`) and at
    /// least half as many models as `previous`. Carried models count for neither.
    ///
    /// # Errors
    ///
    /// [`PricingError::MissingAnchor`] or [`PricingError::Shrunk`].
    pub fn check_refresh(&self, previous: Option<&Self>) -> Result<(), PricingError> {
        if let Some(model) = ANCHORS
            .into_iter()
            .find(|anchor| self.models.get(*anchor).is_none_or(|m| m.carried))
        {
            return Err(PricingError::MissingAnchor { model });
        }
        let models = self.len() - self.carried();
        if let Some(previous) = previous.map(|p| p.len() - p.carried())
            && models.saturating_mul(2) < previous
        {
            return Err(PricingError::Shrunk { models, previous });
        }
        Ok(())
    }
}

/// `body` plus every priced entry of `previous` that `body` lacks, marked with [`CARRIED_KEY`],
/// so models `LiteLLM` drops stay priced. `body` comes back unchanged when nothing is
/// missing, so its version stays the hash of the download.
///
/// # Errors
///
/// [`PricingError::Json`] when either body is not a JSON object of entries.
pub fn carry_forward(body: &[u8], previous: &[u8]) -> Result<Vec<u8>, PricingError> {
    let mut entries: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(body)?;
    let old: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(previous)?;
    let mut carried = 0_usize;
    for (name, raw) in old {
        if entries.contains_key(&name) || ModelRates::from_raw(&name, &raw, &mut 0).is_none() {
            continue;
        }
        let mut entry: BTreeMap<String, Box<RawValue>> = serde_json::from_str(raw.get())?;
        entry.insert(
            CARRIED_KEY.to_owned(),
            RawValue::from_string("true".to_owned())?,
        );
        entries.insert(name, serde_json::value::to_raw_value(&entry)?);
        carried += 1;
    }
    if carried == 0 {
        return Ok(body.to_vec());
    }
    Ok(serde_json::to_vec(&entries)?)
}

/// First hit wins: the exact id; `{provider}/{id}` (also under `LiteLLM`'s provider name);
/// the id with its routing prefixes stripped one at a time; then the fully stripped id without a
/// `-v1:0` version, without a `-YYYYMMDD` or `@YYYYMMDD` date, and as the Bedrock key
/// `anthropic.{id}-v1:0`.
fn find<'m>(
    models: &'m HashMap<String, ModelRates>,
    id: &str,
    provider: Option<&ProviderId>,
) -> Option<&'m ModelRates> {
    let get = |key: &str| models.get(key);
    if let Some(rates) = get(id) {
        return Some(rates);
    }
    if let Some(provider) = provider {
        let name = provider.as_str();
        let alias = litellm_provider(name);
        let routed = [Some(name), (alias != name).then_some(alias.as_str())]
            .into_iter()
            .flatten()
            .find_map(|p| get(&format!("{p}/{id}")));
        if routed.is_some() {
            return routed;
        }
    }
    let mut rest = id;
    while let Some(next) = ROUTING_PREFIXES.iter().find_map(|p| rest.strip_prefix(p)) {
        rest = next;
        if let Some(rates) = get(rest) {
            return Some(rates);
        }
    }
    let unversioned = strip_version_suffix(rest);
    [Some(unversioned), strip_date_suffix(unversioned)]
        .into_iter()
        .flatten()
        .find_map(get)
        .or_else(|| {
            unversioned
                .starts_with("claude")
                .then(|| get(&format!("anthropic.{unversioned}-v1:0")))
                .flatten()
        })
}

/// `LiteLLM`'s name for a provider id agents write (`fireworks` → `fireworks_ai`).
fn litellm_provider(provider: &str) -> String {
    match provider {
        "fireworks" => "fireworks_ai".to_owned(),
        "together" => "together_ai".to_owned(),
        "google" => "gemini".to_owned(),
        other => other.replace('-', "_"),
    }
}

/// `anthropic.claude-opus-4-1-20250805-v1:0` → `anthropic.claude-opus-4-1-20250805`.
fn strip_version_suffix(id: &str) -> &str {
    let Some((head, tail)) = id.rsplit_once("-v") else {
        return id;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let versioned = match tail.split_once(':') {
        Some((major, minor)) => digits(major) && digits(minor),
        None => digits(tail),
    };
    if versioned { head } else { id }
}

/// `claude-sonnet-4-20250514` and `claude-sonnet-4@20250514` → `claude-sonnet-4`.
fn strip_date_suffix(id: &str) -> Option<&str> {
    let (head, tail) = id.rsplit_once(['-', '@'])?;
    (tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit())).then_some(head)
}

/// What an estimate depends on.
#[derive(Debug, Clone, Copy)]
pub struct PriceQuery<'a> {
    /// Model as reported.
    pub model: &'a ModelId,
    /// Serving provider, if known.
    pub provider: Option<&'a ProviderId>,
    /// Token columns.
    pub tokens: &'a TokenCounts,
    /// Service tier.
    pub service_tier: Option<&'a str>,
    /// Billing modifiers; `None` when unknown.
    pub modifiers: Option<&'a PriceModifiers>,
}

impl<'a> PriceQuery<'a> {
    /// The query of a parsed event.
    #[must_use]
    pub fn of(event: &'a ParsedEvent) -> Self {
        Self {
            model: &event.model,
            provider: event.provider.as_ref(),
            tokens: &event.tokens,
            service_tier: event.service_tier.as_deref(),
            modifiers: event.modifiers.as_ref(),
        }
    }
}

/// Which price an estimate used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    /// The catalog, or the legacy table behind it; the version is the catalog's.
    Catalog,
    /// A `[[pricing.overrides]]` entry; the version is [`Override::version`].
    Override,
}

/// An estimate and how sure it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Priced {
    /// The estimate.
    pub estimate: Estimate,
    /// A rate was guessed: a cache rate from `LiteLLM`'s fallback, or unknown billing modifiers
    /// on an entry that has rates for them.
    pub approximate: bool,
    /// Which price was used.
    pub source: PriceSource,
}

/// Estimates the API cost of one event. Overrides win over `LiteLLM`, whose catalog wins over the
/// legacy table. `None` means unpriced: no rate matched (never guess silently).
#[must_use]
pub fn estimate(
    query: &PriceQuery<'_>,
    catalog: &PricingCatalog,
    overrides: &[Override],
) -> Option<Priced> {
    if let Some(over) = matching_override(query, overrides) {
        let pico = over.resolve()?.pico(query.tokens)?;
        return Some(Priced {
            estimate: Estimate {
                usd: micros(pico)?,
                pricing_version: over.version.clone(),
            },
            approximate: false,
            source: PriceSource::Override,
        });
    }
    let id = query.model.as_str();
    let rates =
        find(&catalog.models, id, query.provider).or_else(|| find(&LEGACY, id, query.provider))?;
    let resolved = rates.resolve(
        Variant::of(query.service_tier),
        query.tokens.prompt_tokens(),
    )?;
    let (pico, unknown) = rates.modify(resolved.pico(query.tokens)?, query.modifiers)?;
    Some(Priced {
        estimate: Estimate {
            usd: micros(pico)?,
            pricing_version: catalog.version.clone(),
        },
        approximate: unknown || resolved.guessed_for(query.tokens),
        source: PriceSource::Catalog,
    })
}

#[cfg(test)]
mod tests;

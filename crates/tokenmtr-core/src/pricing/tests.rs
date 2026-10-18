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
  "embedder": {"mode": "embedding", "input_cost_per_token": 1e-07, "output_cost_per_token": 0},
  "responder": {"litellm_provider": "openai", "mode": "responses", "input_cost_per_token": 1e-06, "output_cost_per_token": 2e-06},
  "claude-geo": {
    "litellm_provider": "anthropic", "mode": "chat",
    "input_cost_per_token": 5e-06, "output_cost_per_token": 2.5e-05,
    "cache_read_input_token_cost": 5e-07, "cache_creation_input_token_cost": 6.25e-06,
    "cache_creation_input_token_cost_above_1hr": 1e-05,
    "provider_specific_entry": {"us": 1.1, "fast": 2.0},
    "search_context_cost_per_query": {"search_context_size_low": 0.01, "search_context_size_medium": 0.01, "search_context_size_high": 0.01}
  },
  "anthropic.claude-bedrock-20250101-v1:0": {"litellm_provider": "bedrock_converse", "mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 1e-06},
  "fireworks_ai/fw-model": {"litellm_provider": "fireworks_ai", "mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 1e-06},
  "xai-grok": {
    "litellm_provider": "xai", "mode": "chat", "input_cost_per_token": 2e-06, "output_cost_per_token": 1e-05,
    "input_cost_per_token_above_128k_tokens": 4e-06, "output_cost_per_token_above_128k_tokens": 2e-05
  },
  "o-reasoner": {"litellm_provider": "openai", "mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 4e-06, "output_cost_per_reasoning_token": 1e-06},
  "output-tiered": {"mode": "chat", "input_cost_per_token": 1e-06, "output_cost_per_token": 1e-06, "output_cost_per_token_above_200k_tokens": 9e-06},
  "vertex_ai/claude-vertex@20250101": {"litellm_provider": "vertex_ai-anthropic_models", "mode": "chat", "input_cost_per_token": 7e-06, "output_cost_per_token": 7e-06},
  "stringy": {"mode": "chat", "input_cost_per_token": "1e-06", "output_cost_per_token": 1e-06, "cache_read_input_token_cost": -1}
}"#;

/// Prices the three anchors and nothing else.
const ANCHORED: &str = r#"{
  "claude-opus-4-7": {"litellm_provider": "anthropic", "mode": "chat", "input_cost_per_token": 5e-06, "output_cost_per_token": 2.5e-05},
  "claude-sonnet-4-5": {"litellm_provider": "anthropic", "mode": "chat", "input_cost_per_token": 3e-06, "output_cost_per_token": 1.5e-05},
  "gpt-5": {"litellm_provider": "openai", "mode": "responses", "input_cost_per_token": 1.25e-06, "output_cost_per_token": 1e-05}
}"#;

fn catalog() -> Result<PricingCatalog, PricingError> {
    PricingCatalog::parse(FIXTURE.as_bytes())
}

struct Ask<'a> {
    model: &'a str,
    provider: Option<&'a str>,
    tokens: TokenCounts,
    service_tier: Option<&'a str>,
    modifiers: Option<PriceModifiers>,
}

impl<'a> Ask<'a> {
    fn new(model: &'a str, provider: Option<&'a str>, tokens: TokenCounts) -> Self {
        Self {
            model,
            provider,
            tokens,
            service_tier: None,
            modifiers: None,
        }
    }

    fn priced(
        &self,
        catalog: &PricingCatalog,
        overrides: &[Override],
    ) -> Result<Option<Priced>, Box<dyn std::error::Error>> {
        let model = ModelId::try_from(self.model)?;
        let provider = self.provider.map(ProviderId::try_from).transpose()?;
        let query = PriceQuery {
            model: &model,
            provider: provider.as_ref(),
            tokens: &self.tokens,
            service_tier: self.service_tier,
            modifiers: self.modifiers.as_ref(),
        };
        Ok(estimate(&query, catalog, overrides))
    }

    fn usd(&self) -> Result<Option<i64>, Box<dyn std::error::Error>> {
        Ok(self.priced(&catalog()?, &[])?.map(|p| p.estimate.usd.get()))
    }
}

fn usd(
    model: &str,
    provider: Option<&str>,
    tokens: TokenCounts,
) -> Result<Option<i64>, Box<dyn std::error::Error>> {
    Ask::new(model, provider, tokens).usd()
}

fn input(n: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input_uncached: n,
        output,
        ..TokenCounts::default()
    }
}

const M: u64 = 1_000_000;

#[test]
fn only_chat_and_responses_models_with_base_rates_are_kept() -> TestResult {
    let c = catalog()?;
    assert_eq!(c.len(), 17);
    assert_eq!(usd("embedder", None, input(10, 0))?, None);
    assert_eq!(usd("sample_spec", None, input(10, 0))?, None);
    assert_eq!(usd("responder", None, input(M, 0))?, Some(1_000_000));
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
fn matching_steps() -> TestResult {
    let t = input(M, 0);
    // Exact.
    assert_eq!(usd("claude-exact", None, t)?, Some(1_000_000));
    // {provider}/{model}, also under LiteLLM's provider name.
    assert_eq!(
        usd("anthropic/claude-routed", Some("openrouter"), t)?,
        Some(2_000_000)
    );
    assert_eq!(usd("anthropic/claude-routed", None, t)?, None);
    assert_eq!(usd("fw-model", Some("fireworks"), t)?, Some(1_000_000));
    assert_eq!(
        usd("claude-vertex@20250101", Some("vertex_ai"), t)?,
        Some(7_000_000)
    );
    // Routing prefixes stripped one at a time, then a `-v1:0` version.
    assert_eq!(
        usd("us.anthropic.claude-prefixed", None, t)?,
        Some(3_000_000)
    );
    assert_eq!(
        usd("apac.anthropic.claude-prefixed-v1:0", None, t)?,
        Some(3_000_000)
    );
    assert_eq!(
        usd("us.anthropic.claude-bedrock-20250101-v1:0", None, t)?,
        Some(1_000_000)
    );
    // A trailing -YYYYMMDD or @YYYYMMDD stripped.
    assert_eq!(usd("claude-dated-20250101", None, t)?, Some(4_000_000));
    assert_eq!(usd("claude-dated@20250101", None, t)?, Some(4_000_000));
    assert_eq!(
        usd("us.anthropic.claude-dated-20250101", None, t)?,
        Some(4_000_000)
    );
    // The Bedrock key of a first-party id.
    assert_eq!(
        usd("claude-bedrock-20250101", Some("anthropic"), t)?,
        Some(1_000_000)
    );
    // Unpriced.
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
fn only_input_keys_select_a_tier() -> TestResult {
    // 300k × $1 + 1M × $1 per million: the output-only tier never applies.
    assert_eq!(
        usd("output-tiered", None, input(300_000, M))?,
        Some(1_300_000)
    );
    Ok(())
}

#[test]
fn xai_thresholds_are_inclusive() -> TestResult {
    assert_eq!(usd("xai-grok", None, input(128_000, 0))?, Some(512_000));
    assert_eq!(usd("xai-grok", None, input(127_999, 0))?, Some(255_998));
    Ok(())
}

#[test]
fn reasoning_tokens_use_their_own_rate() -> TestResult {
    let t = TokenCounts {
        output: 1000,
        reasoning: 400,
        ..TokenCounts::default()
    };
    // 600 × $4 + 400 × $1 per million.
    assert_eq!(usd("o-reasoner", None, t)?, Some(2_800));
    // Without a reasoning rate they bill as output, counted once.
    assert_eq!(usd("plain", None, t)?, Some(2_000));
    Ok(())
}

#[test]
fn cache_write_1h_uses_its_own_field() -> TestResult {
    let t = TokenCounts {
        cache_write_1h: M,
        ..TokenCounts::default()
    };
    assert_eq!(usd("claude-sonnet-4-5", None, t)?, Some(6_000_000));
    Ok(())
}

#[test]
fn service_tier_selects_priority_and_flex_rates() -> TestResult {
    let t = TokenCounts {
        input_uncached: M,
        cache_read: M,
        output: M,
        ..TokenCounts::default()
    };
    let mut ask = Ask::new("gpt-tiered", None, t);
    // standard: 1.25 + 0.125 + 10
    assert_eq!(ask.usd()?, Some(11_375_000));
    ask.service_tier = Some("priority");
    // priority input/output, cache read falls back to the standard field: 2.5 + 0.125 + 20
    assert_eq!(ask.usd()?, Some(22_625_000));
    ask.service_tier = Some("fast");
    assert_eq!(ask.usd()?, Some(22_625_000));
    ask.service_tier = Some("Flex");
    // flex: 0.625 + 0.125 + 5
    assert_eq!(ask.usd()?, Some(5_750_000));
    ask.service_tier = Some("batch");
    assert_eq!(ask.usd()?, Some(11_375_000));
    Ok(())
}

#[test]
fn missing_cache_rates_follow_the_provider() -> TestResult {
    let c = catalog()?;
    let read = TokenCounts {
        cache_read: M,
        ..TokenCounts::default()
    };
    let w5 = TokenCounts {
        cache_write_5m: M,
        ..TokenCounts::default()
    };
    let w1 = TokenCounts {
        cache_write_1h: M,
        ..TokenCounts::default()
    };
    let priced = |model, tokens| {
        Ask::new(model, None, tokens)
            .priced(&c, &[])
            .map(|p| p.map(|p| (p.estimate.usd.get(), p.approximate)))
    };
    // Claude, input $1/M: read 0.1×, 5m write 1.25×, 1h write 2×.
    assert_eq!(priced("claude-exact", read)?, Some((100_000, false)));
    assert_eq!(priced("claude-exact", w5)?, Some((1_250_000, false)));
    assert_eq!(priced("claude-exact", w1)?, Some((2_000_000, false)));
    // Others, as LiteLLM: the input rate, approximate.
    assert_eq!(priced("plain", read)?, Some((1_000_000, true)));
    assert_eq!(priced("plain", w5)?, Some((1_000_000, true)));
    assert_eq!(priced("plain", w1)?, Some((1_000_000, true)));
    // Known rates, or no token billed at a guessed one, are exact.
    assert_eq!(priced("gpt-tiered", read)?, Some((125_000, false)));
    assert_eq!(priced("plain", input(M, 0))?, Some((1_000_000, false)));
    Ok(())
}

#[test]
fn anthropic_modifiers_apply() -> TestResult {
    let c = catalog()?;
    let with = |modifiers| -> Result<Option<(i64, bool)>, Box<dyn std::error::Error>> {
        let ask = Ask {
            modifiers,
            ..Ask::new("claude-geo", Some("anthropic"), input(M, 0))
        };
        Ok(ask
            .priced(&c, &[])?
            .map(|p| (p.estimate.usd.get(), p.approximate)))
    };
    let geo = |geo: &str| PriceModifiers {
        inference_geo: Some(geo.to_owned()),
        ..PriceModifiers::default()
    };
    // Unknown modifiers on an entry that has them: base price, approximate.
    assert_eq!(with(None)?, Some((5_000_000, true)));
    assert_eq!(
        with(Some(PriceModifiers::default()))?,
        Some((5_000_000, false))
    );
    assert_eq!(with(Some(geo("us")))?, Some((5_500_000, false)));
    assert_eq!(with(Some(geo("eu")))?, Some((5_000_000, false)));
    let fast = PriceModifiers {
        fast: true,
        ..geo("us")
    };
    assert_eq!(with(Some(fast))?, Some((11_000_000, false)));
    let searches = PriceModifiers {
        web_search_requests: 3,
        ..PriceModifiers::default()
    };
    assert_eq!(with(Some(searches))?, Some((5_030_000, false)));
    // Entries without modifiers ignore them.
    let plain = Ask {
        modifiers: Some(PriceModifiers {
            fast: true,
            ..PriceModifiers::default()
        }),
        ..Ask::new("plain", None, input(M, 0))
    };
    assert_eq!(plain.usd()?, Some(1_000_000));
    Ok(())
}

#[test]
fn a_parsed_event_carries_its_modifiers() -> TestResult {
    let event = ParsedEvent {
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        dedup_key: DedupKey::try_from("k")?,
        at: Timestamp::UNIX_EPOCH,
        session: SessionId::try_from("s")?,
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: None,
        git_branch: None,
        source_file: None,
        approximate: false,
        provider: Some(ProviderId::try_from("anthropic")?),
        model: ModelId::try_from("claude-geo")?,
        kind: EventKind::Message,
        tokens: input(M, 0),
        reported_cost: None,
        service_tier: None,
        modifiers: Some(PriceModifiers {
            fast: true,
            ..PriceModifiers::default()
        }),
    };
    let priced = estimate(&PriceQuery::of(&event), &catalog()?, &[]).ok_or("priced")?;
    assert_eq!(priced.estimate.usd.get(), 10_000_000);
    Ok(())
}

#[test]
fn overrides_beat_litellm_and_carry_their_own_version() -> TestResult {
    let c = catalog()?;
    let over = |model: &str, input: &str| {
        Override::new(
            ModelId::try_from(model)?,
            input.parse()?,
            "20".parse()?,
            None,
            None,
            None,
        )
        .map_err(Box::<dyn std::error::Error>::from)
    };
    let ask = Ask::new("plain", None, input(M, M));
    let ten = over("plain", "10")?;
    let priced = ask
        .priced(&c, std::slice::from_ref(&ten))?
        .ok_or("priced")?;
    assert_eq!(priced.estimate.usd.get(), 30_000_000);
    assert_eq!(priced.source, PriceSource::Override);
    assert_eq!(&priced.estimate.pricing_version, ten.version());
    assert_ne!(ten.version(), c.version());
    // Editing an override changes its version.
    assert_ne!(over("plain", "11")?.version(), ten.version());
    // Not in LiteLLM at all.
    let private = Ask::new("my-private-model", None, input(M, 0));
    assert_eq!(private.priced(&c, &[])?, None);
    let cheap = Override::new(
        ModelId::try_from("my-private-model")?,
        "3".parse()?,
        "15".parse()?,
        Some("0.5".parse()?),
        None,
        None,
    )?;
    let priced = private.priced(&c, &[cheap])?.ok_or("priced")?;
    assert_eq!(priced.estimate.usd.get(), 3_000_000);
    Ok(())
}

#[test]
fn an_override_with_a_provider_wins_for_that_provider_only() -> TestResult {
    let c = catalog()?;
    let any = Override::new(
        ModelId::try_from("plain")?,
        "10".parse()?,
        "0".parse()?,
        None,
        None,
        None,
    )?;
    let bedrock = Override::new(
        ModelId::try_from("plain")?,
        "30".parse()?,
        "0".parse()?,
        None,
        None,
        None,
    )?
    .for_provider(ProviderId::try_from("bedrock")?);
    assert_ne!(bedrock.version(), any.version());
    let overrides = [any, bedrock];
    let cost = |provider| -> Result<Option<i64>, Box<dyn std::error::Error>> {
        Ok(Ask::new("plain", provider, input(M, 0))
            .priced(&c, &overrides)?
            .map(|p| p.estimate.usd.get()))
    };
    assert_eq!(cost(Some("bedrock"))?, Some(30_000_000));
    assert_eq!(cost(Some("anthropic"))?, Some(10_000_000));
    assert_eq!(cost(None)?, Some(10_000_000));
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
    let cost = |tokens| -> Result<Option<i64>, Box<dyn std::error::Error>> {
        Ok(Ask::new("a", None, tokens)
            .priced(&c, &[])?
            .map(|p| p.estimate.usd.get()))
    };
    assert_eq!(cost(input(1, 0))?, Some(0));
    assert_eq!(cost(input(0, 1))?, Some(1));
    Ok(())
}

#[test]
fn string_rates_are_read_and_bad_ones_counted() -> TestResult {
    let c = catalog()?;
    assert_eq!(c.dropped_rates(), 1);
    assert_eq!(usd("stringy", None, input(M, 0))?, Some(1_000_000));
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
        key("input_cost_per_token_above_128_tokens"),
        Some(("Input".into(), 128, Variant::Standard))
    );
    assert_eq!(
        key("cache_creation_input_token_cost_above_1hr_above_200k_tokens"),
        Some(("Write1h".into(), 200_000, Variant::Standard))
    );
    assert_eq!(
        key("input_cost_per_token_above_272k_tokens_priority"),
        Some(("Input".into(), 272_000, Variant::Priority))
    );
    assert_eq!(
        key("output_cost_per_reasoning_token"),
        Some(("Reasoning".into(), 0, Variant::Standard))
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
fn a_refresh_must_keep_the_anchors_and_half_the_models() -> TestResult {
    let anchored = PricingCatalog::parse(ANCHORED.as_bytes())?;
    anchored.check_refresh(None)?;
    assert!(matches!(
        catalog()?.check_refresh(None),
        Err(PricingError::MissingAnchor {
            model: "claude-opus-4-7"
        })
    ));
    let previous = PricingCatalog::parse(&carry_forward(ANCHORED.as_bytes(), FIXTURE.as_bytes())?)?;
    // Carried models do not count: 3 of 3 here.
    anchored.check_refresh(Some(&previous))?;
    assert!(matches!(
        anchored.check_refresh(Some(&catalog()?)),
        Err(PricingError::Shrunk {
            models: 3,
            previous: 17
        })
    ));
    Ok(())
}

#[test]
fn carry_forward_keeps_dropped_models_priced() -> TestResult {
    let merged = carry_forward(ANCHORED.as_bytes(), FIXTURE.as_bytes())?;
    let c = PricingCatalog::parse(&merged)?;
    // `claude-sonnet-4-5` is in both.
    assert_eq!(c.len(), 19);
    assert_eq!(c.carried(), 16);
    // The new snapshot's own entries win over the old ones.
    let sonnet = Ask::new("claude-sonnet-4-5", None, input(250_000, 0));
    assert_eq!(
        sonnet.priced(&c, &[])?.map(|p| p.estimate.usd.get()),
        Some(750_000)
    );
    assert_eq!(
        Ask::new("claude-exact", None, input(M, 0))
            .priced(&c, &[])?
            .map(|p| p.estimate.usd.get()),
        Some(1_000_000)
    );
    assert!(!String::from_utf8(merged)?.contains("\"embedder\""));
    // Nothing to carry: the body and its version are unchanged.
    assert_eq!(
        carry_forward(ANCHORED.as_bytes(), ANCHORED.as_bytes())?,
        ANCHORED.as_bytes()
    );
    Ok(())
}

#[test]
fn legacy_table_prices_models_litellm_removed() -> TestResult {
    assert_eq!(LEGACY.len(), 10);
    let c = catalog()?;
    let opus = Ask::new("claude-opus-4-1-20250805", Some("anthropic"), input(M, 0));
    let priced = opus.priced(&c, &[])?.ok_or("priced")?;
    assert_eq!(priced.estimate.usd.get(), 15_000_000);
    assert_eq!(&priced.estimate.pricing_version, c.version());
    // 1000 × $0.80 + 1000 × $1.60 (1h write) per million.
    let haiku = Ask::new(
        "claude-3-5-haiku-latest",
        None,
        TokenCounts {
            input_uncached: 1000,
            cache_write_1h: 1000,
            ..TokenCounts::default()
        },
    );
    assert_eq!(
        haiku.priced(&c, &[])?.map(|p| p.estimate.usd.get()),
        Some(2_400)
    );
    Ok(())
}

#[test]
fn vendored_snapshot_parses_and_prices_opus() -> TestResult {
    let c = PricingCatalog::embedded()?;
    assert!(c.len() > 1000);
    c.check_refresh(Some(&c))?;
    assert!(embedded_fetched_at().is_some());
    let tokens = TokenCounts {
        input_uncached: 1000,
        cache_read: 10_000,
        cache_write_5m: 2000,
        cache_write_1h: 1000,
        output: 500,
        reasoning: 0,
    };
    // Opus 4.5 published: $5 in, $25 out, $0.50 read, $6.25 5m write, $10 1h write.
    // 0.005 + 0.005 + 0.0125 + 0.01 + 0.0125 = $0.045.
    let ask = Ask {
        modifiers: Some(PriceModifiers::default()),
        ..Ask::new("claude-opus-4-5-20251101", Some("anthropic"), tokens)
    };
    assert_eq!(
        ask.priced(&c, &[])?.map(|p| p.estimate.usd.get()),
        Some(45_000)
    );
    // Removed from LiteLLM, priced through its Bedrock key or the legacy table.
    for model in [
        "claude-sonnet-4-20250514",
        "claude-opus-4-20250514",
        "claude-opus-4-1-20250805",
        "claude-3-7-sonnet-20250219",
        "claude-3-5-haiku-20241022",
    ] {
        let ask = Ask::new(model, Some("anthropic"), input(1000, 0));
        assert!(ask.priced(&c, &[])?.is_some(), "{model} is unpriced");
    }
    Ok(())
}

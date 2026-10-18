//! Fuzz entry points: one function per parser of outside data, each fed arbitrary bytes and
//! required never to panic.
//!
//! `cargo-fuzz` (nightly) reaches them through the shims in `fuzz_targets/`; `cargo test --lib`
//! (stable, `just fuzz-smoke`) replays `seeds/` plus mutated variants through [`TARGETS`].

use jiff::Timestamp;
use jiff::tz::{TimeZone, offset};
use tokenmtr_agents::{ParseBatch, SourceFile, claude, pi};
use tokenmtr_config::{ConfigFormat, Env, TomlCodec, load_str};
use tokenmtr_core::ProfileId;
use tokenmtr_remote::{ClaudeCredentials, parse_profile_response};

#[cfg(test)]
mod smoke;

/// A fuzz entry point.
pub type Entry = fn(&[u8]);

/// Every target by name; `fuzz_targets/<name>.rs`, `seeds/<name>/` and this list stay in step
/// (checked by the smoke test).
pub const TARGETS: &[(&str, Entry)] = &[
    ("claude_line", claude_line),
    ("pi_line", pi_line),
    ("stats_cache", stats_cache),
    ("litellm_catalog", litellm_catalog),
    ("quota_response", quota_response),
    ("ecb_xml", ecb_xml),
    ("config", config),
];

/// One Claude transcript line.
pub fn claude_line(data: &[u8]) {
    let file = SourceFile::new(
        "/fuzz/projects/p/sess.jsonl".into(),
        ProfileId::default_profile(),
    );
    let Ok(ctx) = claude::LineContext::new(&file) else {
        return;
    };
    let mut batch = ParseBatch::new();
    let _ = claude::parse_line(&ctx, data, &mut batch);
}

/// A pi session: the bytes up to the first newline are the header, the rest is an entry line.
pub fn pi_line(data: &[u8]) {
    let (first, rest) = data.split_at(data.iter().position(|b| *b == b'\n').unwrap_or(data.len()));
    let header = pi::SessionHeader::parse(first);
    let file = SourceFile::new(
        "/fuzz/sessions/--p--/2026-01-01T00-00-00-000Z_sess.jsonl".into(),
        ProfileId::default_profile(),
    );
    let Ok(ctx) = pi::LineContext::new(&file, header.as_ref()) else {
        return;
    };
    let mut batch = ParseBatch::new();
    let _ = pi::parse_line(&ctx, rest, &mut batch);
    let _ = pi::parse_line(&ctx, data, &mut batch);
}

/// A Claude `stats-cache.json` body, in UTC and in a fixed non-UTC zone.
pub fn stats_cache(data: &[u8]) {
    let profile = ProfileId::default_profile();
    let _ = claude::stats_cache::parse(data, &profile, &TimeZone::UTC);
    let _ = claude::stats_cache::parse(data, &profile, &TimeZone::fixed(offset(-8)));
}

/// A `LiteLLM` price catalog body: validated alone, then as the active snapshot an empty-ish
/// download carries forward.
pub fn litellm_catalog(data: &[u8]) {
    let _ = tokenmtr_remote::parse_catalog(data.to_vec(), None);
    let _ = tokenmtr_core::pricing::carry_forward(br#"{"gpt-5":{}}"#, data);
}

/// The Claude usage, profile and credentials documents (all fetched or read from outside).
pub fn quota_response(data: &[u8]) {
    let _ = tokenmtr_remote::parse_quota_response(data, Timestamp::UNIX_EPOCH);
    let _ = parse_profile_response(data);
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = ClaudeCredentials::parse(text);
    }
}

/// An ECB reference-rate XML body.
pub fn ecb_xml(data: &[u8]) {
    let _ = tokenmtr_remote::parse_ecb_xml(data);
}

/// A TOML config text.
pub fn config(data: &[u8]) {
    struct HomeOnly;
    impl Env for HomeOnly {
        fn var(&self, key: &str) -> Option<String> {
            (key == "HOME").then(|| "/home/fuzz".to_owned())
        }
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = load_str(text, ConfigFormat::Toml(TomlCodec), &HomeOnly);
}

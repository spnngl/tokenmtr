//! End-to-end tests of `TokenMtr`. The crate has no code: the scenarios live in
//! `tests/` and drive the real `tokenmtr` binary against a temporary `HOME`, fixture agent files
//! and a mock server that stands in for Anthropic, the ECB and `LiteLLM`.
//!
//! `cargo test -p tokenmtr-e2e` builds the binary itself (`cargo build -p tokenmtr`).

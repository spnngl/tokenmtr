//! D54: arbitrary bytes into the pi header and line parsers must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmtr_agents::{ParseBatch, SourceFile, pi};
use tokenmtr_core::ProfileId;

fuzz_target!(|data: &[u8]| {
    // The first line is the header, the rest is one entry line.
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
});

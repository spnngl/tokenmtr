//! D54: arbitrary bytes into the Claude line parser must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmtr_agents::{ParseBatch, SourceFile, claude};
use tokenmtr_core::ProfileId;

fuzz_target!(|data: &[u8]| {
    let file = SourceFile::new("/fuzz/projects/p/sess.jsonl".into(), ProfileId::default_profile());
    let Ok(ctx) = claude::LineContext::new(&file) else {
        return;
    };
    let mut batch = ParseBatch::new();
    let _ = claude::parse_line(&ctx, data, &mut batch);
});

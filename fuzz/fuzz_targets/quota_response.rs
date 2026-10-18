//! D54: arbitrary bytes into the `quota_response` parser must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| tokenmtr_fuzz::quota_response(data));

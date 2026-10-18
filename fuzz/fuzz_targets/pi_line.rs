//! D54: arbitrary bytes into the `pi_line` parser must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| tokenmtr_fuzz::pi_line(data));

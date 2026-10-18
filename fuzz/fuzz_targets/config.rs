//! D54: arbitrary bytes into the `config` parser must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| tokenmtr_fuzz::config(data));

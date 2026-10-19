//! D54: arbitrary bytes into the `accept_header` parsers must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| tokenmtr_fuzz::accept_header(data));

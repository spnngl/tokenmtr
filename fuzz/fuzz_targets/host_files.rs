//! D54: arbitrary bytes into the `host_files` parsers must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| tokenmtr_fuzz::host_files(data));

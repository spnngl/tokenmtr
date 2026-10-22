//! D54: arbitrary cells into the export row decoders must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| tokenmtr_fuzz::export_row(data));

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tokenmtr_remote::parse_ecb_xml(data);
});

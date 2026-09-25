#![no_main]
//! Decode-free walker: `probe_pict` over arbitrary bytes, plus the
//! typed QuickTime payload parsers on whatever the probe surfaced.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = oxideav_pict::probe_pict(data);
    let _ = oxideav_pict::parse_compressed_quicktime(data);
    let _ = oxideav_pict::parse_uncompressed_quicktime(data);
});

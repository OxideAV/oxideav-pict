#![no_main]
//! Whole-file decode: any byte string through `parse_pict` (the
//! default QuickTime decoder chain — `'raw '` built in, `'jpeg'` via
//! `oxideav-mjpeg`). Errors are fine; panics and runaway allocation are
//! the findings.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = oxideav_pict::parse_pict(data);
});

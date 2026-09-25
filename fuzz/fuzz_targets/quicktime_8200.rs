#![no_main]
//! `$8200` CompressedQuickTime compositor under hostile wrappers.
//!
//! The fuzzer's bytes become the *payload interior* of one `$8200`
//! opcode (Inside Macintosh: QuickTime Table 3-1: version, 3×3
//! matrix, matte, mode, srcRect, accuracy, mask, ImageDescription,
//! image data) inside a small valid PICT, so the matrix / mask /
//! matte / mode composition and the `'raw '` + `'jpeg'` decoders see
//! structurally-plausible wrappers far more often than a whole-file
//! target would reach them. A second variant wraps the same bytes as
//! `$8201` so the shared path is driven from both opcodes.

use libfuzzer_sys::fuzz_target;
use oxideav_pict::ops::PictBuilder;

fuzz_target!(|data: &[u8]| {
    if data.len() > 1 << 16 {
        return;
    }
    let frame = data.first().map_or(16i16, |&b| i16::from(b % 64) + 1);
    let mut b = PictBuilder::new(0, 0, frame, frame);
    if b.compressed_quicktime(data).is_ok() {
        // Emitter shape: image, then a text placeholder + NOP, so the
        // suppression lookahead is exercised too.
        b.push(&oxideav_pict::build_tx_size(12));
        if let Ok(t) = oxideav_pict::build_long_text(0, frame - 1, b"QuickTime and a") {
            b.push(&t);
        }
        b.push(&[0, 0]);
        let _ = oxideav_pict::parse_pict(&b.finish());
    }
    let mut b = PictBuilder::new(0, 0, frame, frame);
    if b.uncompressed_quicktime(data).is_ok() {
        let _ = oxideav_pict::parse_pict(&b.finish());
    }
});

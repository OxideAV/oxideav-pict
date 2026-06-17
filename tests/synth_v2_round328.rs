//! Round 328 — `DirectBits` `packType = 0` (default packing).
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3, "Packing types"
//! (book page A-16), documents `packType = 0` as **default packing**.
//! The accompanying PixData pseudocode forces *unpacked* data whenever
//! `rowBytes < 8`, regardless of `packType`; above that threshold the
//! spec states the default `packType` for a `pixelSize` of **16** is
//! type **3** (per-scanline 16-bit PackBits) and for a `pixelSize` of
//! **32** is type **4** (per-scanline component-separated PackBits).
//!
//! Earlier rounds mapped `(packType 0, 16)` and `(packType 0, 32)`
//! unconditionally onto the *raw* (unpacked) pixel-row decoders, so a
//! default-packed `DirectBitsRect` emitter (`packType = 0`,
//! `rowBytes ≥ 8`) was mis-read as raw rows and produced garbage. This
//! round resolves `packType = 0` to the spec's documented default
//! (type 3 for 16-bit, type 4 for 32-bit) before dispatch.
//!
//! The public `encode_pict_v2` only ever writes a concrete `packType`
//! word (1..=4), so a `packType = 0` stream is produced here by
//! emitting the concrete-`packType` encoding and then patching the
//! on-disk `packType` word to `0`. A correct decoder must produce the
//! *same* pixels for the patched (`packType = 0`) stream as for the
//! original concrete-`packType` stream.

use oxideav_pict::{encode_pict_v2, parse_pict, PackType};

/// Locate the `DirectBitsRect` (`0x009A`) opcode in an encoded v2 PICT
/// and overwrite its 2-byte `packType` word with `0`.
///
/// Within the §A-3 / Listing A-2 PixMap layout the `packType` word sits
/// at: opcode(2) + baseAddr(4) + rowBytes(2) + bounds(8) + pmVersion(2)
/// = **18 bytes** after the start of the `0x009A` opcode.
fn patch_pack_type_to_zero(bytes: &mut [u8]) {
    // Skip the 512-byte launch stub the encoder always prepends, then
    // scan word-aligned for the 0x009A opcode. (The stub is zero-filled
    // so it never contains a stray 0x009A on an even boundary.)
    let mut i = 512;
    let op = 0x009Au16.to_be_bytes();
    let opcode_off = loop {
        assert!(i + 2 <= bytes.len(), "0x009A opcode not found");
        if bytes[i] == op[0] && bytes[i + 1] == op[1] {
            break i;
        }
        i += 2;
    };
    let pack_type_off = opcode_off + 18;
    // Sanity: the word we are about to clobber must be the concrete
    // packType the encoder wrote (3 for Rle16, 4 for ComponentPackBits).
    let cur = u16::from_be_bytes([bytes[pack_type_off], bytes[pack_type_off + 1]]);
    assert!(
        cur == 3 || cur == 4,
        "expected concrete packType 3|4 at offset {pack_type_off}, found {cur}",
    );
    bytes[pack_type_off] = 0;
    bytes[pack_type_off + 1] = 0;
}

/// Build a deterministic RGBA test image with per-pixel-varying colour
/// so a wrong (raw-vs-packed) decode cannot coincidentally match.
fn checker_rgba(w: usize, h: usize) -> Vec<u8> {
    let mut data = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let off = (y * w + x) * 4;
            data[off] = ((x * 23 + y * 7) & 0xFF) as u8;
            data[off + 1] = ((x * 5 + y * 31) & 0xFF) as u8;
            data[off + 2] = (((x ^ y) * 17) & 0xFF) as u8;
            data[off + 3] = 0xFF;
        }
    }
    data
}

/// `packType = 0` with a 16-bit PixMap (`rowBytes ≥ 8`) must decode
/// identically to the concrete `packType = 3` (16-bit PackBits) stream.
#[test]
fn direct_bits_packtype0_16bpp_defaults_to_type3() {
    let (w, h) = (6usize, 4usize);
    let rgba = checker_rgba(w, h);

    let concrete = encode_pict_v2(w as u32, h as u32, &rgba, PackType::Rle16).unwrap();
    let mut patched = concrete.clone();
    patch_pack_type_to_zero(&mut patched);

    let want = parse_pict(&concrete).unwrap();
    let got = parse_pict(&patched).unwrap();

    assert_eq!(got.width, want.width);
    assert_eq!(got.height, want.height);
    assert_eq!(
        got.data, want.data,
        "packType 0 (16bpp) must resolve to the documented default (type 3)"
    );
}

/// `packType = 0` with a 32-bit PixMap (`rowBytes ≥ 8`) must decode
/// identically to the concrete `packType = 4` (component PackBits)
/// stream.
#[test]
fn direct_bits_packtype0_32bpp_defaults_to_type4() {
    let (w, h) = (5usize, 3usize);
    let rgba = checker_rgba(w, h);

    let concrete = encode_pict_v2(w as u32, h as u32, &rgba, PackType::ComponentPackBits).unwrap();
    let mut patched = concrete.clone();
    patch_pack_type_to_zero(&mut patched);

    let want = parse_pict(&concrete).unwrap();
    let got = parse_pict(&patched).unwrap();

    assert_eq!(got.width, want.width);
    assert_eq!(got.height, want.height);
    assert_eq!(
        got.data, want.data,
        "packType 0 (32bpp) must resolve to the documented default (type 4)"
    );
}

/// 32-bit `packType = 0` must agree with the original RGBA the encoder
/// was handed (the 16-bit path is lossy A1R5G5B5, so only the 32-bit
/// component path is bit-exact against the source pixels).
#[test]
fn direct_bits_packtype0_32bpp_is_bit_exact_source() {
    let (w, h) = (8usize, 2usize);
    let rgba = checker_rgba(w, h);

    let mut patched =
        encode_pict_v2(w as u32, h as u32, &rgba, PackType::ComponentPackBits).unwrap();
    patch_pack_type_to_zero(&mut patched);

    let got = parse_pict(&patched).unwrap();
    assert_eq!(got.width as usize, w);
    assert_eq!(got.height as usize, h);

    // Component PackBits carries R,G,B exactly; alpha defaults to 0xFF.
    for y in 0..h {
        for x in 0..w {
            let off = (y * w + x) * 4;
            assert_eq!(got.data[off], rgba[off], "R at ({x},{y})");
            assert_eq!(got.data[off + 1], rgba[off + 1], "G at ({x},{y})");
            assert_eq!(got.data[off + 2], rgba[off + 2], "B at ({x},{y})");
            assert_eq!(got.data[off + 3], 0xFF, "A at ({x},{y})");
        }
    }
}

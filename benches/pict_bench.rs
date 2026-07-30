//! Round 401 — decode / encode / probe throughput benches.
//!
//! Run with `cargo bench`. Three axes:
//!
//! * **raster** — the DirectBitsRect pixel path at each PackType
//!   (256 × 256 RGBA), decode and encode sides.
//! * **drawing** — a synthetic 400-opcode vector stream through the
//!   full rasteriser (shapes, patterns, text).
//! * **probe** — the same streams through the no-rasterisation
//!   `probe_pict` walker, to keep the "probe is cheap" contract
//!   measurable.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use oxideav_pict::ops::PictBuilder;
use oxideav_pict::{
    build_direct_bits_rect_op, encode_pict_v2, parse_pict, probe_pict, Fixed, ImageDescription,
    PackType, QuickTimeCompressed, QuickTimeUncompressed, Verb,
};

/// 256 × 256 synthetic RGBA gradient (compressible but not trivial).
fn gradient_rgba(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&[
                (x ^ y) as u8,
                (x.wrapping_mul(3) + y) as u8,
                (y.wrapping_mul(5)) as u8,
                0xFF,
            ]);
        }
    }
    v
}

/// A 400-opcode vector stream: shapes, patterns, pen state, text.
fn drawing_stream() -> Vec<u8> {
    let mut b = PictBuilder::new(0, 0, 320, 240);
    for i in 0..40i16 {
        let x = (i % 8) * 40;
        let y = (i / 8) * 48;
        b.fg_color((i as u8).wrapping_mul(37), 0x80, 0x40);
        b.pen_size(1 + (i % 3), 1 + (i % 3));
        b.rect(Verb::Paint, y + 2, x + 2, y + 20, x + 20);
        b.same_rect(Verb::Invert);
        b.oval(Verb::Frame, y + 4, x + 4, y + 18, x + 18);
        b.arc(Verb::Paint, y + 20, x + 2, y + 44, x + 30, 0, 270);
        b.line(x + 2, y + 2, x + 36, y + 44);
        b.poly(
            Verb::Fill,
            &[(x + 4, y + 30), (x + 30, y + 34), (x + 16, y + 44)],
        )
        .unwrap();
        b.long_text(x + 4, y + 40, b"Qd").unwrap();
        b.short_line_from(4, -2);
        b.region_rect(Verb::Fill, y + 24, x + 24, y + 32, x + 32);
        b.short_comment(i as u16);
    }
    b.finish()
}

fn bench_raster(c: &mut Criterion) {
    let rgba = gradient_rgba(256, 256);
    let mut g = c.benchmark_group("raster_256x256");
    for (name, pack) in [
        ("raw", PackType::Raw),
        ("packed24", PackType::Packed24),
        ("rle16", PackType::Rle16),
        ("component", PackType::ComponentPackBits),
    ] {
        let encoded = encode_pict_v2(256, 256, &rgba, pack).unwrap();
        g.bench_function(format!("encode_{name}"), |bch| {
            bch.iter(|| encode_pict_v2(256, 256, black_box(&rgba), pack).unwrap())
        });
        g.bench_function(format!("decode_{name}"), |bch| {
            bch.iter(|| parse_pict(black_box(&encoded)).unwrap())
        });
    }
    g.finish();
}

fn bench_drawing(c: &mut Criterion) {
    let stream = drawing_stream();
    c.bench_function("drawing_400_opcodes_decode", |b| {
        b.iter(|| parse_pict(black_box(&stream)).unwrap())
    });
}

fn bench_probe(c: &mut Criterion) {
    let stream = drawing_stream();
    let raster = encode_pict_v2(256, 256, &gradient_rgba(256, 256), PackType::Rle16).unwrap();
    c.bench_function("probe_drawing_stream", |b| {
        b.iter(|| probe_pict(black_box(&stream)).unwrap())
    });
    c.bench_function("probe_raster_stream", |b| {
        b.iter(|| probe_pict(black_box(&raster)).unwrap())
    });
}

/// A PICT carrying one `$8200` (64 KiB compressed-image blob behind a
/// jpeg-tagged `ImageDescription`) and one `$8201` (64×64
/// DirectBitsRect subopcode) — the round-435 typed QuickTime paths.
fn quicktime_stream() -> Vec<u8> {
    let mut name_raw = [0u8; 32];
    name_raw[0] = 5;
    name_raw[1..6].copy_from_slice(b"Bench");
    let desc = ImageDescription {
        id_size: 86,
        codec: *b"jpeg",
        version: 1,
        revision_level: 1,
        vendor: *b"appl",
        temporal_quality: 0,
        spatial_quality: 0x0200,
        width: 256,
        height: 256,
        h_res: Fixed::SEVENTY_TWO_DPI,
        v_res: Fixed::SEVENTY_TWO_DPI,
        data_size: 0,
        frame_count: 1,
        name_raw,
        depth: 24,
        clut_id: -1,
        extension: Vec::new(),
    };
    let blob: Vec<u8> = (0..65536u32).map(|i| (i * 31) as u8).collect();
    let compressed = QuickTimeCompressed::still(desc, blob);
    let sub =
        build_direct_bits_rect_op(0, 0, 64, 64, &gradient_rgba(64, 64), PackType::Raw).unwrap();
    let uncompressed = QuickTimeUncompressed::wrapping(&sub).unwrap();

    let mut b = PictBuilder::new(0, 0, 64, 64);
    b.compressed_quicktime_image(&compressed).unwrap();
    b.uncompressed_quicktime_image(&uncompressed).unwrap();
    b.finish()
}

fn bench_quicktime(c: &mut Criterion) {
    let stream = quicktime_stream();
    // Full decode: typed $8200 parse (64 KiB payload copy) + $8201
    // sub-blit onto the canvas.
    c.bench_function("quicktime_decode", |b| {
        b.iter(|| parse_pict(black_box(&stream)).unwrap())
    });
    // Probe: skims the wrappers into ProbeQuickTime rows without
    // keeping payload bytes — the "probe is cheap" contract must
    // stay measurable against the decode above.
    c.bench_function("quicktime_probe", |b| {
        b.iter(|| probe_pict(black_box(&stream)).unwrap())
    });
}

criterion_group!(
    benches,
    bench_raster,
    bench_drawing,
    bench_probe,
    bench_quicktime
);
criterion_main!(benches);

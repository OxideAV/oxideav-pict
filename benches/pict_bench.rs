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
use oxideav_pict::{encode_pict_v2, parse_pict, probe_pict, PackType, Verb};

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

criterion_group!(benches, bench_raster, bench_drawing, bench_probe);
criterion_main!(benches);

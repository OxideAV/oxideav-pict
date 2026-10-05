//! The `pict` container through the `oxideav-core` registries: the
//! structural probe (PICT has no magic), the demuxer, the muxer, and the
//! Layer-1-vs-registry byte-exact matrix (`IMAGE_CRATE_API`, Layer 2
//! acceptance for a container).
#![cfg(feature = "registry")]
// The depth encoders (`encode_pict_v1`, …) build the distinct opcode
// layouts the matrix pins.
#![allow(deprecated)]

use std::io::{Cursor, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use oxideav_core::PROBE_SCORE_EXTENSION;
use oxideav_core::{
    CodecId, CodecParameters, Error, Frame, Packet, PixelFormat, ProbeData, RuntimeContext,
    StreamInfo, TimeBase, VideoFrame, VideoPlane,
};
use oxideav_pict::container::{
    probe, EXTENSIONS, PROBE_SCORE_END_PIC, PROBE_SCORE_STANZA_WITH_EXTENSION,
    PROBE_SCORE_V2_OPCODES_CUT,
};
use oxideav_pict::{
    encode_pict_bits_rect, encode_pict_indexed_bits_rect, encode_pict_indexed_pack_bits_rect,
    encode_pict_pack_bits_rect, encode_pict_v1, encode_pict_v1_pack_bits_rect, EncodeOptions,
    IndexedPixelSize, CODEC_ID_STR,
};

const CONTAINER: &str = "pict";
const FIXTURE: &[u8] = include_bytes!("fixtures/imagemagick_gradient_64x48.pict");

/// A `Send + 'static` in-memory sink the muxer can own while the test
/// keeps a handle to read the bytes back.
#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Cursor<Vec<u8>>>>);

impl SharedSink {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

impl Seek for SharedSink {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.lock().unwrap().seek(pos)
    }
}

fn ctx() -> RuntimeContext {
    let mut ctx = RuntimeContext::new();
    oxideav_pict::register(&mut ctx);
    ctx
}

fn gradient_rgba(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&[
                (x * 255 / w.max(1)) as u8,
                (y * 255 / h.max(1)) as u8,
                ((x + y) * 7) as u8,
                255,
            ]);
        }
    }
    v
}

/// Every opcode layout the crate's writers produce, named.
fn layouts() -> Vec<(&'static str, Vec<u8>)> {
    let (w, h) = (13u32, 7u32);
    let rgba = gradient_rgba(w, h);
    let palette: Vec<[u8; 4]> = (0..16u8)
        .map(|i| [i * 16, 255 - i * 16, i * 7, 255])
        .collect();
    let idx16: Vec<u8> = (0..(w * h) as usize).map(|i| (i % 16) as u8).collect();
    let palette256: Vec<[u8; 4]> = (0..=255u8).map(|i| [i, i ^ 0x55, 255 - i, 255]).collect();
    let idx256: Vec<u8> = (0..(w * h) as usize).map(|i| (i * 3 % 256) as u8).collect();
    vec![
        (
            "v2 DirectBitsRect raw + stub (encoder default)",
            oxideav_pict::encode_rgba8(w, h, &rgba, &EncodeOptions::default()).unwrap(),
        ),
        (
            "v2 DirectBitsRect PackBits",
            encode_pict_pack_bits_rect(w, h, &rgba).unwrap(),
        ),
        ("v1 raw", encode_pict_v1(w, h, &rgba).unwrap()),
        (
            "v1 PackBits",
            encode_pict_v1_pack_bits_rect(w, h, &rgba).unwrap(),
        ),
        (
            "v2 BitsRect bitmap",
            encode_pict_bits_rect(w, h, &rgba).unwrap(),
        ),
        (
            "v2 indexed 4 bpp",
            encode_pict_indexed_bits_rect(w, h, &idx16, &palette, IndexedPixelSize::FourBpp)
                .unwrap(),
        ),
        (
            "v2 indexed 8 bpp PackBits",
            encode_pict_indexed_pack_bits_rect(
                w,
                h,
                &idx256,
                &palette256,
                IndexedPixelSize::EightBpp,
            )
            .unwrap(),
        ),
        ("ImageMagick 64×48 fixture", FIXTURE.to_vec()),
    ]
}

/// Open `bytes` through the registry and pump the packet through the
/// registry decoder.
fn demux_decode(
    ctx: &RuntimeContext,
    bytes: &[u8],
    ext: Option<&str>,
) -> (StreamInfo, Packet, VideoFrame) {
    let name = ctx
        .containers
        .probe_input(&mut Cursor::new(bytes), ext)
        .expect("probe");
    assert_eq!(name, CONTAINER);
    let mut demuxer = ctx
        .containers
        .open_demuxer(&name, Box::new(Cursor::new(bytes.to_vec())), &ctx.codecs)
        .expect("open_demuxer");
    assert_eq!(demuxer.format_name(), CONTAINER);
    assert_eq!(demuxer.streams().len(), 1);
    let stream = demuxer.streams()[0].clone();
    assert_eq!(stream.params.codec_id, CodecId::new(CODEC_ID_STR));
    assert_eq!(stream.time_base, TimeBase::new(1, 1));
    assert!(demuxer.metadata().is_empty());
    let mut dec = ctx.codecs.first_decoder(&stream.params).expect("decoder");
    let pkt = demuxer.next_packet().expect("one packet");
    assert!(pkt.flags.keyframe);
    assert_eq!(pkt.pts, Some(0));
    assert_eq!(pkt.stream_index, 0);
    assert!(matches!(demuxer.next_packet(), Err(Error::Eof)));
    dec.send_packet(&pkt).unwrap();
    let frame = match dec.receive_frame().unwrap() {
        Frame::Video(v) => v,
        _ => panic!("expected a video frame"),
    };
    (stream, pkt, frame)
}

// ---- probe ----------------------------------------------------------------

#[test]
fn probe_names_pict_from_structure_and_with_extension() {
    let ctx = ctx();
    for (label, bytes) in layouts() {
        for ext in [None, Some("pict"), Some("pct"), Some("pic")] {
            assert_eq!(
                ctx.containers
                    .probe_input(&mut Cursor::new(&bytes), ext)
                    .unwrap(),
                CONTAINER,
                "{label} / ext {ext:?}"
            );
        }
        assert_eq!(
            probe(&ProbeData {
                buf: &bytes,
                ext: None
            }),
            PROBE_SCORE_END_PIC,
            "{label}: a complete picture walks to OpEndPic"
        );
    }
    // A picture larger than the probe buffer still scores: the stanza and
    // header confirm the record, the raster payload is cut.
    let big = oxideav_pict::encode_rgba8(
        600,
        600,
        &gradient_rgba(600, 600),
        &EncodeOptions::default(),
    )
    .unwrap();
    assert!(big.len() > 256 * 1024);
    assert_eq!(
        probe(&ProbeData {
            buf: &big[..256 * 1024],
            ext: None
        }),
        PROBE_SCORE_V2_OPCODES_CUT
    );
    assert_eq!(
        probe(&ProbeData {
            buf: &big[..256 * 1024],
            ext: Some("pict")
        }),
        PROBE_SCORE_STANZA_WITH_EXTENSION
    );
    assert_eq!(
        ctx.containers
            .probe_input(&mut Cursor::new(&big), None)
            .unwrap(),
        CONTAINER
    );
}

/// Foreign headers synthesised in-test (always run) — no PICT stanza at
/// offset 10 or 522.
fn foreign_samples() -> Vec<(&'static str, Vec<u8>)> {
    let mut v: Vec<(&str, Vec<u8>)> = Vec::new();
    let mut farbfeld = b"farbfeld".to_vec();
    farbfeld.extend_from_slice(&2u32.to_be_bytes());
    farbfeld.extend_from_slice(&2u32.to_be_bytes());
    farbfeld.extend_from_slice(&[0x11; 32]);
    v.push(("ff", farbfeld));
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
    v.push(("png", png));
    v.push((
        "exr",
        vec![
            0x76, 0x2f, 0x31, 0x01, 0x02, 0, 0, 0, b'c', b'h', b'a', b'n', b'n', b'e', b'l', b's',
            0,
        ],
    ));
    v.push((
        "bmp",
        b"BM\x3a\0\0\0\0\0\0\0\x36\0\0\0\x28\0\0\0\x01\0\0\0\x01\0\0\0\x01\0\x18\0".to_vec(),
    ));
    v.push(("gif", b"GIF89a\x01\0\x01\0\x80\0\0\0\0\0\xff\xff\xff\x2c\0\0\0\0\x01\0\x01\0\0\x02\x02\x44\x01\0\x3b".to_vec()));
    v.push((
        "qoi",
        b"qoif\0\0\0\x01\0\0\0\x01\x04\0\xfe\0\0\0\0\0\0\0\0\0\0\0\x01".to_vec(),
    ));
    v.push(("ppm", b"P6\n1 1\n255\n\0\0\0".to_vec()));
    v.push(("tif", b"II*\0\x08\0\0\0\0\0\0\0\0\0\0\0".to_vec()));
    v.push((
        "webp",
        b"RIFF\x1a\0\0\0WEBPVP8L\x0d\0\0\0\x2f\0\0\0\0\x07\x10\x11\x11\x88\x88\xfe\x07\0".to_vec(),
    ));
    v.push((
        "jpg",
        vec![
            0xff, 0xd8, 0xff, 0xe0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0,
            0xff, 0xd9,
        ],
    ));
    v.push((
        "hdr",
        b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 1\n\0\0\0\0".to_vec(),
    ));
    v.push(("bin", vec![0u8; 1024]));
    v.push(("bin", (0..=255u8).cycle().take(2048).collect()));
    v
}

#[test]
fn probe_rejects_foreign_files() {
    let ctx = ctx();
    for (ext, bytes) in foreign_samples() {
        assert_eq!(
            probe(&ProbeData {
                buf: &bytes,
                ext: Some(ext)
            }),
            0,
            "{ext} sample must not score as PICT"
        );
        assert!(
            ctx.containers
                .probe_input(&mut Cursor::new(&bytes), Some(ext))
                .is_err(),
            "{ext} sample must not be named pict (the only registered container)"
        );
        assert!(ctx
            .containers
            .probe_input(&mut Cursor::new(&bytes), None)
            .is_err());
    }
}

/// Sweep the sibling crates' fixtures (the umbrella checkout) with the
/// first 256 KiB of every image file and its real extension: none may
/// score as PICT. Skips with a note outside the umbrella (CI checks out
/// this crate alone); the synthesised samples above always run.
#[test]
fn probe_rejects_sibling_crate_fixtures() {
    // `OXIDEAV_CRATES_DIR` points the sweep at the umbrella's `crates/`
    // when this crate is built elsewhere.
    let root = std::env::var_os("OXIDEAV_CRATES_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".."));
    let Ok(entries) = std::fs::read_dir(&root) else {
        eprintln!("no sibling crates next to this one — sweep skipped");
        return;
    };
    let mut files = Vec::new();
    for crate_dir in entries.flatten() {
        let name = crate_dir.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("oxideav-") || name == "oxideav-pict" {
            continue;
        }
        let tests = crate_dir.path().join("tests");
        if tests.is_dir() {
            collect_files(&tests, &mut files);
        }
    }
    if files.is_empty() {
        eprintln!("no sibling fixtures found — sweep skipped");
        return;
    }
    let image_exts = [
        "png", "bmp", "gif", "tif", "tiff", "exr", "ff", "qoi", "tga", "pcx", "dcx", "pbm", "pgm",
        "ppm", "pam", "hdr", "dds", "ico", "cur", "jp2", "j2k", "jpc", "jxl", "jxs", "webp",
        "avif", "heic", "heif", "svg", "iff", "lbm", "ilbm", "jpg", "jpeg", "wbmp", "icer",
    ];
    let mut checked = 0usize;
    for path in files {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let Some(ext) = ext else { continue };
        if !image_exts.contains(&ext.as_str()) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let head = &bytes[..bytes.len().min(256 * 1024)];
        // Never a stanza match: without a hint the score is 0 …
        let bare = probe(&ProbeData {
            buf: head,
            ext: None,
        });
        assert_eq!(bare, 0, "{} scored {bare} as PICT", path.display());
        // … and with the file's own extension only the ext-only floor
        // can appear, for extensions PICT shares (`.pic` is also Radiance
        // HDR's; its magic scores 100 there and wins the election).
        let hinted = probe(&ProbeData {
            buf: head,
            ext: Some(&ext),
        });
        let expected = if EXTENSIONS.contains(&ext.as_str()) {
            PROBE_SCORE_EXTENSION
        } else {
            0
        };
        assert_eq!(
            hinted,
            expected,
            "{} scored {hinted} with its extension",
            path.display()
        );
        checked += 1;
    }
    eprintln!("sibling sweep: {checked} image fixtures, none probed as PICT");
    assert!(checked > 0);
}

fn collect_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

// ---- demux: Layer 1 vs registry -------------------------------------------

#[test]
fn demux_matches_layer1_across_every_layout() {
    let ctx = ctx();
    let mut pinned = 0;
    for (label, bytes) in layouts() {
        let info = oxideav_pict::info(&bytes).unwrap();
        let l1 = oxideav_pict::decode(&bytes).unwrap();
        let (stream, pkt, frame) = demux_decode(&ctx, &bytes, None);
        assert_eq!(stream.params.width, Some(info.width), "{label}");
        assert_eq!(stream.params.height, Some(info.height), "{label}");
        assert_eq!(
            stream.params.pixel_format,
            Some(PixelFormat::Rgba),
            "{label}"
        );
        assert_eq!(
            stream.params.color_signal,
            CodecParameters::video(CodecId::new(CODEC_ID_STR)).color_signal,
            "{label}: PICT carries no colour information — nothing stamped"
        );
        assert_eq!(pkt.data, bytes, "{label}: the whole file is the packet");
        assert_eq!(frame.image_planes().len(), 1, "{label}");
        assert_eq!(frame.planes[0].stride, l1.planes[0].stride, "{label}");
        assert_eq!(
            frame.planes[0].data, l1.planes[0].data,
            "{label}: registry planes == Layer 1 planes"
        );
        assert_eq!(frame.color_signal(), None, "{label}");
        assert_eq!(frame.pts, Some(0));
        pinned += 1;
    }
    assert_eq!(pinned, 8);
}

// ---- mux ------------------------------------------------------------------

fn registry_mux(
    ctx: &RuntimeContext,
    w: u32,
    h: u32,
    format: PixelFormat,
    pixels: Vec<u8>,
) -> Vec<u8> {
    let mut params = CodecParameters::video(CodecId::new(CODEC_ID_STR));
    params.width = Some(w);
    params.height = Some(h);
    params.pixel_format = Some(format);
    let mut enc = ctx.codecs.first_encoder(&params).expect("encoder");
    let stream = StreamInfo {
        index: 0,
        time_base: TimeBase::new(1, 1),
        duration: None,
        start_time: Some(0),
        params: enc.output_params().clone(),
    };
    let bpp = match format {
        PixelFormat::Rgba => 4,
        PixelFormat::Rgb24 => 3,
        _ => unreachable!(),
    };
    let sink = SharedSink::default();
    {
        let mut muxer = ctx
            .containers
            .open_muxer(CONTAINER, Box::new(sink.clone()), &[stream])
            .expect("open_muxer");
        assert_eq!(muxer.format_name(), CONTAINER);
        muxer.write_header().unwrap();
        let vf = VideoFrame {
            pts: Some(0),
            planes: vec![VideoPlane {
                stride: w as usize * bpp,
                data: pixels,
            }],
        };
        enc.send_frame(&Frame::Video(vf)).unwrap();
        let pkt = enc.receive_packet().unwrap();
        muxer.write_packet(&pkt).unwrap();
        muxer.write_trailer().unwrap();
    }
    sink.bytes()
}

#[test]
fn mux_writes_the_encoder_packet_and_round_trips() {
    let ctx = ctx();
    let (w, h) = (17u32, 5u32);
    let rgba = gradient_rgba(w, h);
    let out = registry_mux(&ctx, w, h, PixelFormat::Rgba, rgba.clone());
    assert!(oxideav_pict::probe(&out));
    let back = oxideav_pict::decode(&out).unwrap();
    assert_eq!(back.planes[0].data, rgba, "lossless through mux");
    // demux(mux(frame)) == frame through the registry.
    let (stream, _, frame) = demux_decode(&ctx, &out, Some("pict"));
    assert_eq!(
        (stream.params.width, stream.params.height),
        (Some(w), Some(h))
    );
    assert_eq!(frame.planes[0].data, rgba);

    // Rgb24 input is accepted by the codec (wave-4 ruling); the file
    // decodes to Rgba with opaque alpha.
    let rgb: Vec<u8> = rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let out = registry_mux(&ctx, w, h, PixelFormat::Rgb24, rgb);
    let (stream, _, frame) = demux_decode(&ctx, &out, None);
    assert_eq!(stream.params.pixel_format, Some(PixelFormat::Rgba));
    assert_eq!(frame.planes[0].data, rgba);
}

#[test]
fn mux_rejects_bad_streams_and_packets() {
    let ctx = ctx();
    let video = StreamInfo {
        index: 0,
        time_base: TimeBase::new(1, 1),
        duration: None,
        start_time: Some(0),
        params: CodecParameters::video(CodecId::new(CODEC_ID_STR)),
    };
    let sink = || Box::new(Cursor::new(Vec::<u8>::new()));
    assert!(ctx.containers.open_muxer(CONTAINER, sink(), &[]).is_err());
    assert!(ctx
        .containers
        .open_muxer(CONTAINER, sink(), &[video.clone(), video.clone()])
        .is_err());
    let audio = StreamInfo {
        params: CodecParameters::audio(CodecId::new("pcm")),
        ..video.clone()
    };
    assert!(ctx
        .containers
        .open_muxer(CONTAINER, sink(), &[audio])
        .is_err());

    let mut muxer = ctx
        .containers
        .open_muxer(CONTAINER, sink(), std::slice::from_ref(&video))
        .unwrap();
    muxer.write_header().unwrap();
    assert!(muxer
        .write_packet(&Packet::new(0, TimeBase::new(1, 1), Vec::new()))
        .is_err());
    assert!(muxer
        .write_packet(&Packet::new(0, TimeBase::new(1, 1), b"not a pict".to_vec()))
        .is_err());
    assert!(muxer.write_trailer().is_err(), "no packet, no file");
    let pict = oxideav_pict::encode_rgba8(2, 2, &[1u8; 16], &EncodeOptions::default()).unwrap();
    muxer
        .write_packet(&Packet::new(0, TimeBase::new(1, 1), pict.clone()))
        .unwrap();
    assert!(
        muxer
            .write_packet(&Packet::new(0, TimeBase::new(1, 1), pict))
            .is_err(),
        "a PICT holds one picture"
    );
    muxer.write_trailer().unwrap();
}

// ---- registration ---------------------------------------------------------

#[test]
fn register_installs_codec_and_container() {
    let ctx = ctx();
    assert!(ctx
        .codecs
        .decoder_ids()
        .any(|c| *c == CodecId::new(CODEC_ID_STR)));
    assert!(ctx.containers.demuxer_names().any(|n| n == CONTAINER));
    assert!(ctx.containers.muxer_names().any(|n| n == CONTAINER));
    for ext in ["pict", "pct", "pic", "PICT"] {
        assert_eq!(ctx.containers.container_for_extension(ext), Some(CONTAINER));
    }
    let mut via_entry = RuntimeContext::new();
    oxideav_pict::__oxideav_entry(&mut via_entry);
    assert!(via_entry.containers.demuxer_names().any(|n| n == CONTAINER));
    assert!(via_entry.containers.muxer_names().any(|n| n == CONTAINER));
}

// ---- hostile input --------------------------------------------------------

#[test]
fn hostile_inputs_never_panic() {
    let ctx = ctx();
    let open = |bytes: &[u8]| {
        ctx.containers
            .open_demuxer(
                CONTAINER,
                Box::new(Cursor::new(bytes.to_vec())),
                &ctx.codecs,
            )
            .map(|mut d| while d.next_packet().is_ok() {})
    };
    assert!(open(&[]).is_err());
    assert!(open(&[0u8; 11]).is_err());
    // Degenerate picFrame is refused at the demux boundary.
    let mut degenerate = vec![0u8; 10];
    degenerate.extend_from_slice(&[0x00, 0x11, 0x02, 0xFF, 0x0C, 0x00]);
    degenerate.extend_from_slice(&[0u8; 24]);
    degenerate.extend_from_slice(&[0x00, 0xFF]);
    assert!(matches!(open(&degenerate), Err(Error::InvalidData(_))));
    for (_, src) in layouts() {
        for cut in (0..src.len()).step_by(3) {
            let _ = open(&src[..cut]);
            let _ = probe(&ProbeData {
                buf: &src[..cut],
                ext: Some("pict"),
            });
        }
        let mut mutated = src.clone();
        for i in 0..src.len().min(700) {
            for v in [0x00, 0xff, 0x7f] {
                let keep = mutated[i];
                mutated[i] = v;
                let _ = open(&mutated);
                let _ = probe(&ProbeData {
                    buf: &mutated,
                    ext: None,
                });
                mutated[i] = keep;
            }
        }
    }
}

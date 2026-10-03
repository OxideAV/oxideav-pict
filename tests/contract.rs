//! The image-crate contract (`IMAGE_CRATE_API`) surface of
//! `oxideav-pict`: `probe` / `info` / `decode*` / `encode*`, the
//! `PictImage` shape and constructors, `DecodeOptions` limits and
//! strictness, `EncodeOptions` fields, the lossless round-trip pin,
//! byte-identity of the deprecated wrappers with the contract
//! functions, and (with `registry`) the frame bridge and factories.

use std::io::Cursor;

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    decode, decode_from, decode_rgb8, decode_rgba8, decode_with, encode, encode_rgb8, encode_rgba8,
    encode_to, info, probe, ColorInfo, ColorRange, DecodeOptions, EncodeOptions, Fixed, PackType,
    PictComment, PictError, PictHeader, PictImage, PictPixelFormat, PictVersion, PixelFormat,
    Plane,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/imagemagick_gradient_64x48.pict");

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

fn minimal_v2(top: i16, left: i16, bottom: i16, right: i16, header: &[u8; 24]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&0u16.to_be_bytes());
    for v in [top, left, bottom, right] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b.extend_from_slice(&0x0011u16.to_be_bytes());
    b.extend_from_slice(&0x02FFu16.to_be_bytes());
    b.extend_from_slice(&0x0C00u16.to_be_bytes());
    b.extend_from_slice(header);
    b
}

// ---------------------------------------------------------------------------
// probe / info
// ---------------------------------------------------------------------------

#[test]
fn probe_sniffs_both_stanzas_and_the_launch_stub() {
    let v2 = encode_rgba8(2, 2, &[7u8; 16], &EncodeOptions::default()).unwrap();
    assert!(probe(&v2));
    assert!(v2.starts_with(&[0u8; 512]), "v2 default carries the stub");
    assert!(
        probe(&v2[512..]),
        "the record itself probes without the stub"
    );
    let v1 = encode_rgba8(
        2,
        2,
        &[7u8; 16],
        &EncodeOptions::default().with_version(PictVersion::V1),
    )
    .unwrap();
    assert!(probe(&v1));
    assert!(!probe(&[]));
    assert!(!probe(&[0u8; 11]));
    assert!(!probe(&[0u8; 600]));
    assert!(!probe(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"));
}

#[test]
fn info_reads_the_header_without_walking_opcodes() {
    let i = info(FIXTURE).unwrap();
    assert_eq!((i.width, i.height), (64, 48));
    assert_eq!(i.format, PixelFormat::Rgba);
    assert_eq!(i.frames, 1);
    assert!(!i.has_alpha);
    assert_eq!(i.color, ColorInfo::pict_default());
    assert!(!i.has_icc && !i.has_exif && !i.has_xmp);
    assert_eq!(i.version, PictVersion::V2);
    assert_eq!(i.frame, (0, 0, 48, 64));

    let img = decode(FIXTURE).unwrap();
    assert_eq!((img.width, img.height), (i.width, i.height));
    assert_eq!(img.header, i.header);

    // Resolution and header kind come from the HeaderOp payload.
    let bytes = encode_rgba8(
        3,
        5,
        &[1u8; 60],
        &EncodeOptions::default()
            .with_resolution_dpi(300, 150)
            .with_frame_origin(10, 20),
    )
    .unwrap();
    let i = info(&bytes).unwrap();
    assert_eq!((i.width, i.height), (3, 5));
    assert_eq!(i.frame, (10, 20, 15, 23));
    assert!(i.has_launch_stub);
    assert_eq!(i.resolution_dpi(), Some((300.0, 150.0)));
    assert!(matches!(i.header, Some(PictHeader::ExtendedV2 { .. })));

    // A v1 picture: no header, no stub.
    let v1 = encode_rgba8(
        3,
        5,
        &[1u8; 60],
        &EncodeOptions::default().with_version(PictVersion::V1),
    )
    .unwrap();
    let i = info(&v1).unwrap();
    assert_eq!(i.version, PictVersion::V1);
    assert!(!i.has_launch_stub);
    assert_eq!(i.header, None);
    assert_eq!(i.resolution_dpi(), None);
}

#[test]
fn info_succeeds_on_a_degenerate_frame_that_decode_rejects() {
    // An opcode-less record whose frame is 0 × 8: header-only `info`
    // reports it, `decode` refuses to rasterise nothing.
    let bytes = minimal_v2(0, 0, 8, 0, &[0u8; 24]);
    let i = info(&bytes).unwrap();
    assert_eq!((i.width, i.height), (0, 8));
    assert!(matches!(decode(&bytes), Err(PictError::InvalidData(_))));
    // Short / foreign input is an error, never a panic.
    assert!(matches!(info(&[]), Err(PictError::InvalidData(_))));
    assert!(matches!(info(&[0u8; 20]), Err(PictError::InvalidData(_))));
}

// ---------------------------------------------------------------------------
// decode shape
// ---------------------------------------------------------------------------

#[test]
fn decode_returns_the_contract_shape_in_rgba() {
    let img = decode(FIXTURE).unwrap();
    assert_eq!(img.format(), PixelFormat::Rgba);
    assert_eq!(img.planes.len(), 1);
    assert_eq!(img.stride(), 64 * 4);
    assert_eq!(img.planes[0].data.len(), 64 * 48 * 4);
    assert_eq!(img.as_bytes().map(|b| b.len()), Some(64 * 48 * 4));
    assert_eq!(img.color, ColorInfo::pict_default());
    assert_eq!(img.color.range, ColorRange::Full);
    assert!(img.metadata.is_empty());
    assert!(img.data().chunks_exact(4).all(|p| p[3] == 255));
    assert_eq!(img.to_rgba8(), img.data());
    let rgb = img.to_rgb8();
    assert_eq!(rgb.len(), 64 * 48 * 3);
    assert_eq!(&rgb[..3], &img.data()[..3]);
    let raw = img.clone().into_raw();
    assert_eq!(raw, img.data());
}

#[test]
fn raw_paths_match_the_image_kernels() {
    let img = decode(FIXTURE).unwrap();
    let rgb = decode_rgb8(FIXTURE).unwrap();
    assert_eq!((rgb.width, rgb.height), (64, 48));
    assert_eq!(rgb.data, img.to_rgb8());
    assert_eq!(rgb.stride(), 64 * 3);
    let rgba = decode_rgba8(FIXTURE).unwrap();
    assert_eq!(rgba.data, img.to_rgba8());
    assert_eq!(rgba.as_bytes().len(), 64 * 48 * 4);
    let from_reader = decode_from(Cursor::new(FIXTURE.to_vec())).unwrap();
    assert_eq!(from_reader, img);
}

#[test]
fn vector_only_pictures_rasterise_at_the_frame_size() {
    let mut b = PictBuilder::new(0, 0, 10, 20);
    b.fg_color(255, 0, 0).rect(Verb::Paint, 2, 2, 8, 18);
    let bytes = b.finish();
    let i = info(&bytes).unwrap();
    assert_eq!((i.width, i.height), (20, 10));
    let img = decode(&bytes).unwrap();
    assert_eq!((img.width, img.height), (20, 10));
    let px = |x: usize, y: usize| &img.data()[(y * 20 + x) * 4..(y * 20 + x) * 4 + 4];
    assert_eq!(px(0, 0), [255, 255, 255, 255]);
    assert_eq!(px(5, 5), [255, 0, 0, 255]);
}

// ---------------------------------------------------------------------------
// DecodeOptions
// ---------------------------------------------------------------------------

#[test]
fn limits_fire_before_the_canvas_is_allocated() {
    // 32767 × 32767 would be ~4.3 GB of canvas.
    let hostile = minimal_v2(-16384, -16384, 16383, 16383, &[0u8; 24]);
    assert!(matches!(decode(&hostile), Err(PictError::LimitExceeded(_))));
    let small = encode_rgba8(8, 4, &[9u8; 128], &EncodeOptions::default()).unwrap();
    assert!(decode_with(&small, &DecodeOptions::default()).is_ok());
    for opts in [
        DecodeOptions::default().with_max_width(7u32),
        DecodeOptions::default().with_max_height(3u32),
        DecodeOptions::default().with_max_pixels(31u64),
        DecodeOptions::default().with_max_bytes(127u64),
    ] {
        assert!(
            matches!(decode_with(&small, &opts), Err(PictError::LimitExceeded(_))),
            "{opts:?}"
        );
    }
    let exact = DecodeOptions::default()
        .with_max_width(8u32)
        .with_max_height(4u32)
        .with_max_pixels(32u64)
        .with_max_bytes(128u64);
    assert!(decode_with(&small, &exact).is_ok());
    assert!(decode_with(&small, &DecodeOptions::default().unlimited()).is_ok());
}

#[test]
fn strict_rejects_what_the_lenient_walker_tolerates() {
    let good = encode_rgba8(4, 4, &[3u8; 64], &EncodeOptions::default()).unwrap();
    let strict = DecodeOptions::default().with_strict(true);
    assert!(decode_with(&good, &strict).is_ok());

    // Trailing bytes after OpEndPic.
    let mut trailing = good.clone();
    trailing.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(decode(&trailing).unwrap(), decode(&good).unwrap());
    assert!(matches!(
        decode_with(&trailing, &strict),
        Err(PictError::InvalidData(_))
    ));

    // No OpEndPic: cut the last two bytes.
    let cut = &good[..good.len() - 2];
    assert_eq!(decode(cut).unwrap().data(), decode(&good).unwrap().data());
    assert!(matches!(
        decode_with(cut, &strict),
        Err(PictError::InvalidData(_))
    ));

    // A non-canonical HeaderOp payload (version word neither $FFFE nor
    // $FFFF) is skipped leniently and refused strictly.
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.fg_color(0, 0, 255).rect(Verb::Paint, 0, 0, 4, 4);
    let mut zero_header = b.finish();
    // stub(512) + picSize(2) + frame(8) + 0011 02FF 0C00 (6) → header
    // payload at 528.
    assert_eq!(&zero_header[528..530], &[0xFF, 0xFE]);
    zero_header[528..552].copy_from_slice(&[0u8; 24]);
    let lenient = decode(&zero_header).unwrap();
    assert_eq!(lenient.header, None);
    assert_eq!(&lenient.data()[..4], &[0, 0, 255, 255]);
    assert!(matches!(
        decode_with(&zero_header, &strict),
        Err(PictError::InvalidData(_))
    ));
}

// ---------------------------------------------------------------------------
// Constructors and the image kernels
// ---------------------------------------------------------------------------

#[test]
fn constructors_validate_geometry() {
    assert!(PictImage::from_rgba8(2, 2, vec![0; 16]).is_ok());
    assert!(
        PictImage::from_rgba8(2, 2, vec![0; 17]).is_ok(),
        "longer is fine"
    );
    assert!(matches!(
        PictImage::from_rgba8(2, 2, vec![0; 15]),
        Err(PictError::InvalidData(_))
    ));
    assert!(matches!(
        PictImage::from_rgb8(2, 2, vec![0; 11]),
        Err(PictError::InvalidData(_))
    ));
    assert!(matches!(
        PictImage::from_rgb8(0, 2, vec![]),
        Err(PictError::InvalidData(_))
    ));
    assert!(matches!(
        PictImage::new(2, 2, PixelFormat::Rgba, vec![]),
        Err(PictError::InvalidData(_))
    ));
    assert!(matches!(
        PictImage::new(
            2,
            2,
            PixelFormat::Rgba,
            vec![Plane::new(8, vec![0; 16]), Plane::new(8, vec![0; 16])]
        ),
        Err(PictError::InvalidData(_))
    ));
    assert!(matches!(
        PictImage::new(2, 2, PixelFormat::Rgba, vec![Plane::new(7, vec![0; 16])]),
        Err(PictError::InvalidData(_))
    ));
    // A padded stride is accepted and dropped by the kernels.
    let padded = PictImage::new(
        2,
        1,
        PixelFormat::Rgb24,
        vec![Plane::new(8, vec![1, 2, 3, 4, 5, 6, 0xAA, 0xBB])],
    )
    .unwrap();
    assert_eq!(padded.to_rgb8(), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(padded.to_rgba8(), vec![1, 2, 3, 255, 4, 5, 6, 255]);
    assert_eq!(padded.try_to_rgba8().unwrap(), padded.to_rgba8());
    assert_eq!(
        PictImage::packed(1, 1, PixelFormat::Rgb24, vec![0; 3])
            .unwrap()
            .stride(),
        3
    );
    assert_eq!(PictPixelFormat::Rgb24.bytes_per_pixel(), 3);
    assert!(PictPixelFormat::Rgba.has_alpha());
    assert!(!PictPixelFormat::Rgb24.has_alpha());
}

#[test]
fn rgba_kernels_are_exact() {
    let img = PictImage::from_rgba8(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
    assert_eq!(img.to_rgba8(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(img.to_rgb8(), vec![1, 2, 3, 5, 6, 7]);
    let img = PictImage::from_rgb8(2, 1, vec![1, 2, 3, 5, 6, 7]).unwrap();
    assert_eq!(img.to_rgba8(), vec![1, 2, 3, 255, 5, 6, 7, 255]);
    assert_eq!(img.to_rgb8(), vec![1, 2, 3, 5, 6, 7]);
}

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

#[test]
fn lossless_round_trip_for_every_lossless_pack_type() {
    let rgba = gradient_rgba(37, 11);
    let first = decode(&encode_rgba8(37, 11, &rgba, &EncodeOptions::default()).unwrap()).unwrap();
    assert_eq!(first.data(), rgba.as_slice());
    assert!(matches!(first.header, Some(PictHeader::ExtendedV2 { .. })));
    for pack in [
        PackType::Raw,
        PackType::Packed24,
        PackType::ComponentPackBits,
    ] {
        for version in [PictVersion::V2, PictVersion::V1] {
            let opts = EncodeOptions::default()
                .with_pack(pack)
                .with_version(version);
            let bytes = encode(&first, &opts).unwrap();
            let back = decode(&bytes).unwrap();
            if version == PictVersion::V2 {
                assert_eq!(back, first, "{pack:?} {version:?}");
            } else {
                // v1 has no HeaderOp: everything but the header survives.
                assert_eq!(back.header, None);
                let mut expect = first.clone();
                expect.header = None;
                assert_eq!(back, expect, "{pack:?} {version:?}");
            }
        }
    }
    // Rle16 is 5 bits per channel: close, never exact.
    let bytes = encode(&first, &EncodeOptions::default().with_pack(PackType::Rle16)).unwrap();
    let back = decode(&bytes).unwrap();
    for (a, b) in back.data().chunks_exact(4).zip(rgba.chunks_exact(4)) {
        for c in 0..3 {
            assert!((a[c] as i32 - b[c] as i32).abs() <= 8, "{a:?} vs {b:?}");
        }
        assert_eq!(a[3], 255);
    }
}

#[test]
fn rgb24_input_encodes_as_the_same_pixmap_and_decodes_as_rgba() {
    let rgb: Vec<u8> = (0..6 * 4 * 3).map(|i| (i * 5) as u8).collect();
    let bytes = encode_rgb8(6, 4, &rgb, &EncodeOptions::default()).unwrap();
    let img = decode(&bytes).unwrap();
    assert_eq!(img.format, PixelFormat::Rgba);
    assert_eq!(img.to_rgb8(), rgb);
    // Identical wire bytes to the RGBA path with opaque alpha.
    let rgba = PictImage::from_rgb8(6, 4, rgb.clone()).unwrap().to_rgba8();
    assert_eq!(
        bytes,
        encode_rgba8(6, 4, &rgba, &EncodeOptions::default()).unwrap()
    );
    // `encode` of an Rgb24 PictImage is the same file.
    let img24 = PictImage::from_rgb8(6, 4, rgb).unwrap();
    assert_eq!(encode(&img24, &EncodeOptions::default()).unwrap(), bytes);
}

#[test]
fn encode_rejects_short_buffers_and_invalid_images() {
    assert!(matches!(
        encode_rgba8(2, 2, &[0; 15], &EncodeOptions::default()),
        Err(PictError::InvalidData(_))
    ));
    assert!(matches!(
        encode_rgb8(2, 2, &[0; 11], &EncodeOptions::default()),
        Err(PictError::InvalidData(_))
    ));
    // rowBytes beyond the 14-bit PixMap field.
    let wide = vec![0u8; 4096 * 4];
    assert!(matches!(
        encode_rgba8(4096, 1, &wide, &EncodeOptions::default()),
        Err(PictError::InvalidData(_))
    ));
    // Frame origin pushing the frame past i16.
    assert!(matches!(
        encode_rgba8(
            4,
            4,
            &[0; 64],
            &EncodeOptions::default().with_frame_origin(32767, 0)
        ),
        Err(PictError::InvalidData(_))
    ));
}

#[test]
fn encode_options_are_fields_and_reach_the_wire() {
    let rgba = gradient_rgba(5, 3);
    // Stub override on v2 / v1.
    let no_stub = encode_rgba8(
        5,
        3,
        &rgba,
        &EncodeOptions::default().with_launch_stub(false),
    )
    .unwrap();
    assert!(!no_stub.starts_with(&[0u8; 12]));
    assert_eq!(
        &encode_rgba8(5, 3, &rgba, &EncodeOptions::default()).unwrap()[512..],
        &no_stub[..]
    );
    let v1_stub = encode_rgba8(
        5,
        3,
        &rgba,
        &EncodeOptions::default()
            .with_version(PictVersion::V1)
            .with_launch_stub(true),
    )
    .unwrap();
    assert!(v1_stub.starts_with(&[0u8; 512]));
    assert_eq!(decode(&v1_stub).unwrap().data(), rgba.as_slice());

    // Frame origin: the decoded picture is the same raster.
    let moved = encode_rgba8(
        5,
        3,
        &rgba,
        &EncodeOptions::default().with_frame_origin(-7, 100),
    )
    .unwrap();
    let img = decode(&moved).unwrap();
    assert_eq!(img.data(), rgba.as_slice());
    assert_eq!(info(&moved).unwrap().frame, (-7, 100, -4, 105));

    // Plain v2 header kind.
    let plain = encode_rgba8(
        5,
        3,
        &rgba,
        &EncodeOptions::default().with_extended_header(false),
    )
    .unwrap();
    let img = decode(&plain).unwrap();
    assert!(matches!(img.header, Some(PictHeader::V2 { .. })));
    // …and it is written back as read.
    let again = decode(&encode(&img, &EncodeOptions::default()).unwrap()).unwrap();
    assert_eq!(again, img);
    // An explicit resolution wins over the image header.
    let hires = decode(
        &encode(
            &img,
            &EncodeOptions::default().with_resolution_dpi(144, 144),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        hires.header, img.header,
        "plain header carries no resolution"
    );
    let ext = decode(
        &encode(
            &img,
            &EncodeOptions::default()
                .with_extended_header(true)
                .with_resolution((Fixed::from_integer(144), Fixed::from_integer(72))),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(matches!(
        ext.header,
        Some(PictHeader::ExtendedV2 { hres, vres, .. }) if hres == Fixed::from_integer(144) && vres == Fixed::SEVENTY_TWO_DPI
    ));

    // Clip: pixels outside the clip stay paper-white.
    let clipped = encode_rgba8(
        5,
        3,
        &rgba,
        &EncodeOptions::default().with_clip([0, 0, 3, 2]),
    )
    .unwrap();
    let img = decode(&clipped).unwrap();
    assert_eq!(&img.data()[..4], &rgba[..4]);
    assert_eq!(&img.data()[4 * 4..4 * 4 + 4], &[255, 255, 255, 255]);

    // Comments round-trip through the image.
    let src = PictImage::from_rgba8(5, 3, rgba.clone())
        .unwrap()
        .with_comments(vec![
            PictComment::short(150),
            PictComment::long(0x1234, b"hello pict".to_vec()),
            PictComment::long(1, vec![]),
        ]);
    for version in [PictVersion::V2, PictVersion::V1] {
        let bytes = encode(&src, &EncodeOptions::default().with_version(version)).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.comments, src.comments, "{version:?}");
        assert_eq!(back.data(), rgba.as_slice());
        let silent = decode(
            &encode(
                &src,
                &EncodeOptions::default()
                    .with_version(version)
                    .with_comments(false),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(silent.comments.is_empty());
    }
}

#[test]
fn encode_to_writes_the_same_bytes() {
    let img = PictImage::from_rgba8(3, 2, vec![42; 24]).unwrap();
    let mut buf = Vec::new();
    encode_to(&img, &EncodeOptions::default(), &mut buf).unwrap();
    assert_eq!(buf, encode(&img, &EncodeOptions::default()).unwrap());
}

#[test]
fn io_errors_are_carried_as_io() {
    struct Broken;
    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("nope"))
        }
    }
    struct Full;
    impl std::io::Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let e = decode_from(Broken).unwrap_err();
    assert!(matches!(e, PictError::Io(_)), "{e}");
    assert!(e.to_string().starts_with("io: "));
    assert!(std::error::Error::source(&e).is_some());
    let img = PictImage::from_rgba8(1, 1, vec![0; 4]).unwrap();
    assert!(matches!(
        encode_to(&img, &EncodeOptions::default(), Full),
        Err(PictError::Io(_))
    ));
    let e: oxideav_pict::Error = std::io::Error::other("x").into();
    assert!(matches!(e, PictError::Io(_)));
}

// ---------------------------------------------------------------------------
// Deprecated wrappers are byte-identical to the contract functions
// ---------------------------------------------------------------------------

#[test]
#[allow(deprecated)]
fn legacy_entry_points_match_the_contract_bytes() {
    let rgba = gradient_rgba(23, 9);
    assert_eq!(
        oxideav_pict::encode_pict(23, 9, &rgba).unwrap(),
        encode_rgba8(23, 9, &rgba, &EncodeOptions::default()).unwrap()
    );
    for pack in [
        PackType::Raw,
        PackType::Packed24,
        PackType::Rle16,
        PackType::ComponentPackBits,
    ] {
        assert_eq!(
            oxideav_pict::encode_pict_v2(23, 9, &rgba, pack).unwrap(),
            encode_rgba8(23, 9, &rgba, &EncodeOptions::default().with_pack(pack)).unwrap(),
            "{pack:?}"
        );
        assert_eq!(
            oxideav_pict::encode_pict_v1_with(23, 9, &rgba, pack).unwrap(),
            encode_rgba8(
                23,
                9,
                &rgba,
                &EncodeOptions::default()
                    .with_pack(pack)
                    .with_version(PictVersion::V1)
            )
            .unwrap(),
            "{pack:?}"
        );
    }
    assert_eq!(
        oxideav_pict::encode_pict_v1(23, 9, &rgba).unwrap(),
        encode_rgba8(
            23,
            9,
            &rgba,
            &EncodeOptions::default().with_version(PictVersion::V1)
        )
        .unwrap()
    );
    assert_eq!(
        oxideav_pict::encode_pict_v2_with_clip(23, 9, &rgba, PackType::Packed24, [1, 2, 7, 20])
            .unwrap(),
        encode_rgba8(
            23,
            9,
            &rgba,
            &EncodeOptions::default()
                .with_pack(PackType::Packed24)
                .with_clip([1, 2, 7, 20])
        )
        .unwrap()
    );
    let legacy = oxideav_pict::parse_pict(FIXTURE).unwrap();
    assert_eq!(legacy, decode(FIXTURE).unwrap());
    let probe = oxideav_pict::probe_pict(FIXTURE).unwrap();
    assert_eq!(probe, oxideav_pict::inspect(FIXTURE).unwrap());
}

// ---------------------------------------------------------------------------
// registry: factories + frame bridge
// ---------------------------------------------------------------------------

#[cfg(feature = "registry")]
#[test]
fn framework_adapters_call_the_standalone_functions() {
    use oxideav_core::{
        CodecId, CodecParameters, Frame, Packet, PixelFormat as Pf, TimeBase, VideoFrame,
    };

    let rgba = gradient_rgba(9, 7);
    let img = PictImage::from_rgba8(9, 7, rgba.clone()).unwrap();

    let mut params = CodecParameters::video(CodecId::new("pict"));
    params.width = Some(9);
    params.height = Some(7);
    params.pixel_format = Some(Pf::Rgba);

    let frame: VideoFrame = img.clone().into();
    assert!(frame.color_signal().is_none());
    let mut enc = oxideav_pict::make_encoder(&params).unwrap();
    enc.send_frame(&Frame::Video(frame.clone())).unwrap();
    let pkt = enc.receive_packet().unwrap();
    assert_eq!(pkt.data, encode(&img, &EncodeOptions::default()).unwrap());

    let mut dec = oxideav_pict::make_decoder(&params).unwrap();
    let mut in_pkt = Packet::new(0, TimeBase::new(1, 1), pkt.data.clone());
    in_pkt.pts = Some(5);
    dec.send_packet(&in_pkt).unwrap();
    let Frame::Video(v) = dec.receive_frame().unwrap() else {
        panic!("video frame expected");
    };
    assert_eq!(v.pts, Some(5));
    assert_eq!(v.planes[0].data, rgba);
    let back = PictImage::from_video_frame(&v, &params).unwrap();
    assert_eq!(back.data(), rgba.as_slice());
    assert_eq!(back, decode(&pkt.data).unwrap().with_header(None));
    assert_eq!(PictImage::try_from((&v, &params)).unwrap(), back);

    let mut ctx = oxideav_core::RuntimeContext::new();
    oxideav_pict::register(&mut ctx);
    assert!(ctx.codecs.first_decoder(&params).is_ok());
    assert!(ctx.codecs.first_encoder(&params).is_ok());
}

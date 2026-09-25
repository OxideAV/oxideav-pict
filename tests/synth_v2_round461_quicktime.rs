//! Round 461 — `$8200` CompressedQuickTime pixels on the canvas.
//!
//! Inside Macintosh: QuickTime (1993) `StdPix` (book pages 3-137 –
//! 3-139) is the routine the `$8200` opcode serialises: source pixel
//! map + `srcRect`, a 3×3 matrix that "specifies the mapping of the
//! source rectangle to the destination", a transfer `mode`, a `mask`
//! region "in the destination coordinate system", and a blend `matte`
//! "in the coordinate system of the source image" applied per Imaging
//! With QuickDraw `CopyDeepMask` (book page 3-120: "(1 – mask) × source
//! plus (mask) × destination", black = source, white = destination).
//! The matrix convention is the row-vector form of Figure 2-19 (book
//! page 2-26): `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`, third
//! column `Fract` 2.30.
//!
//! The image data is a codec boundary; these tests feed the `'raw '`
//! compressor (image data = pixel map) through the built-in path, a
//! caller-supplied [`QuickTimeImageDecoder`], and — with the
//! `registry` feature — `'jpeg'` through `oxideav-mjpeg`.

use oxideav_pict::ops::PictBuilder;
use oxideav_pict::state::RectI32;
use oxideav_pict::{
    build_direct_bits_rect_op, parse_pict, parse_pict_with, DecodedQuickTimeImage, Fixed,
    ImageDescription, PackType, PictError, QuickTimeCompressed, QuickTimeImageDecoder,
    QuickTimeMatrix, QuickTimeMatte, QuickTimeRender, QuickTimeUncompressed,
};

const ONE: Fixed = Fixed(0x0001_0000);

fn desc(codec: [u8; 4], width: u16, height: u16, depth: u16) -> ImageDescription {
    ImageDescription {
        id_size: 86,
        codec,
        version: 1,
        revision_level: 1,
        vendor: *b"appl",
        temporal_quality: 0,
        spatial_quality: 0x0300,
        width,
        height,
        h_res: Fixed::SEVENTY_TWO_DPI,
        v_res: Fixed::SEVENTY_TWO_DPI,
        data_size: 0,
        frame_count: 1,
        name_raw: [0u8; 32],
        depth,
        clut_id: -1,
        extension: Vec::new(),
    }
}

/// `'raw '` 32-bit image data (pad, R, G, B per pixel) from RGB
/// triples.
fn raw32(pixels: &[[u8; 3]]) -> Vec<u8> {
    pixels
        .iter()
        .flat_map(|p| [0xFF, p[0], p[1], p[2]])
        .collect()
}

fn raw_still(width: u16, height: u16, pixels: &[[u8; 3]]) -> QuickTimeCompressed {
    assert_eq!(pixels.len(), width as usize * height as usize);
    QuickTimeCompressed::still(desc(*b"raw ", width, height, 32), raw32(pixels))
}

fn px(img: &oxideav_pict::PictImage, x: u32, y: u32) -> [u8; 4] {
    let off = ((y * img.width + x) * 4) as usize;
    img.data[off..off + 4].try_into().unwrap()
}

fn pict_with(frame: (i16, i16, i16, i16), qt: &QuickTimeCompressed) -> Vec<u8> {
    let mut b = PictBuilder::new(frame.0, frame.1, frame.2, frame.3);
    b.compressed_quicktime_image(qt).unwrap();
    b.finish()
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

// ---------------------------------------------------------------------------
// Identity: the raw pixel map lands 1:1 at srcRect.
// ---------------------------------------------------------------------------

#[test]
fn raw_still_renders_pixel_exact_at_identity() {
    let pixels = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [10, 20, 30]];
    let qt = raw_still(2, 2, &pixels);
    let img = parse_pict(&pict_with((0, 0, 4, 4), &qt)).unwrap();
    assert_eq!(px(&img, 0, 0), [255, 0, 0, 255]);
    assert_eq!(px(&img, 1, 0), [0, 255, 0, 255]);
    assert_eq!(px(&img, 0, 1), [0, 0, 255, 255]);
    assert_eq!(px(&img, 1, 1), [10, 20, 30, 255]);
    assert_eq!(px(&img, 2, 2), WHITE);
    assert_eq!(
        img.quicktime[0].render,
        QuickTimeRender::Rendered {
            dst: RectI32::from_be(0, 0, 2, 2),
            matte_skipped: None,
        }
    );
    assert!(img.quicktime[0].render.is_rendered());
}

#[test]
fn src_rect_crops_in_source_space_and_places_the_crop() {
    // 4×1 image; srcRect selects columns 1..3. The crop's own source
    // coordinates (left = 1) are where it lands under the identity
    // matrix (page 3-138: the matrix maps *the source rectangle* to
    // the destination — identity keeps srcRect where it is).
    let pixels = [[1, 1, 1], [2, 2, 2], [3, 3, 3], [4, 4, 4]];
    let mut qt = raw_still(4, 1, &pixels);
    qt.src_rect = RectI32::from_be(0, 1, 1, 3);
    let img = parse_pict(&pict_with((0, 0, 2, 6), &qt)).unwrap();
    assert_eq!(px(&img, 0, 0), WHITE);
    assert_eq!(px(&img, 1, 0), [2, 2, 2, 255]);
    assert_eq!(px(&img, 2, 0), [3, 3, 3, 255]);
    assert_eq!(px(&img, 3, 0), WHITE);
    assert!(matches!(
        img.quicktime[0].render,
        QuickTimeRender::Rendered { dst, .. } if dst == RectI32::from_be(0, 1, 1, 3)
    ));

    // A srcRect outside the decoded image is a typed failure, canvas
    // untouched.
    let mut qt = raw_still(4, 1, &pixels);
    qt.src_rect = RectI32::from_be(5, 5, 6, 6);
    let img = parse_pict(&pict_with((0, 0, 2, 6), &qt)).unwrap();
    assert!(matches!(
        img.quicktime[0].render,
        QuickTimeRender::Failed(_)
    ));
    assert!(img.data.chunks_exact(4).all(|p| p == WHITE));
}

// ---------------------------------------------------------------------------
// Matrix: scale + translate, rotation, hostile translation.
// ---------------------------------------------------------------------------

#[test]
fn scale_translate_matrix_maps_src_rect_to_transform_rect() {
    // RectMatrix(src → dst): 2×2 image scaled ×2 and moved to (1,1).
    let pixels = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [9, 9, 9]];
    let mut qt = raw_still(2, 2, &pixels);
    let src = RectI32::from_be(0, 0, 2, 2);
    let dst = RectI32::from_be(1, 1, 5, 5);
    qt.matrix = QuickTimeMatrix::rect_matrix(src, dst).unwrap();
    assert_eq!(qt.matrix.transform_rect(src), dst);
    let img = parse_pict(&pict_with((0, 0, 6, 6), &qt)).unwrap();
    assert!(matches!(
        img.quicktime[0].render,
        QuickTimeRender::Rendered { dst: d, .. } if d == dst
    ));
    for y in 0..6 {
        for x in 0..6 {
            let expect = if (1..5).contains(&x) && (1..5).contains(&y) {
                let ix = (x - 1) / 2;
                let iy = (y - 1) / 2;
                let p = pixels[(iy * 2 + ix) as usize];
                [p[0], p[1], p[2], 255]
            } else {
                WHITE
            };
            assert_eq!(px(&img, x, y), expect, "pixel ({x},{y})");
        }
    }
}

#[test]
fn rotation_matrix_follows_figure_2_23() {
    // Figure 2-23 (book page 2-28), θ = 90°: [cos sin 0; −sin cos 0;
    // tx ty 1] = [0 1; −1 0] with tx = 2 so the result stays in frame.
    // Source pixel (ix, iy) → x' = −y + 2, y' = x: column ix becomes
    // row ix, row iy becomes column 1 − iy.
    let pixels = [
        [1, 0, 0],
        [2, 0, 0],
        [3, 0, 0],
        [4, 0, 0],
        [0, 1, 0],
        [0, 2, 0],
        [0, 3, 0],
        [0, 4, 0],
    ];
    let mut qt = raw_still(4, 2, &pixels);
    qt.matrix = QuickTimeMatrix([
        Fixed(0),
        ONE,
        Fixed(0),
        Fixed(-0x0001_0000),
        Fixed(0),
        Fixed(0),
        Fixed(0x0002_0000),
        Fixed(0),
        Fixed(0x4000_0000),
    ]);
    assert!(!qt.matrix.is_scale_translate());
    let img = parse_pict(&pict_with((0, 0, 4, 2), &qt)).unwrap();
    assert!(matches!(
        img.quicktime[0].render,
        QuickTimeRender::Rendered { dst, .. } if dst == RectI32::from_be(0, 0, 4, 2)
    ));
    for iy in 0..2u32 {
        for ix in 0..4u32 {
            let p = pixels[(iy * 4 + ix) as usize];
            assert_eq!(
                px(&img, 1 - iy, ix),
                [p[0], p[1], p[2], 255],
                "source ({ix},{iy})"
            );
        }
    }
}

#[test]
fn hostile_matrix_stays_bounded_and_off_canvas_is_not_an_error() {
    let pixels = [[255, 0, 0]; 4];
    // Translation far outside the frame.
    let mut qt = raw_still(2, 2, &pixels);
    qt.matrix = QuickTimeMatrix::scale_translate(ONE, ONE, Fixed(0x7FFF_0000), Fixed(0));
    let img = parse_pict(&pict_with((0, 0, 4, 4), &qt)).unwrap();
    assert!(img.quicktime[0].render.is_rendered());
    assert!(img.data.chunks_exact(4).all(|p| p == WHITE));

    // Enormous scale: only the canvas-visible part is materialised.
    let mut qt = raw_still(2, 2, &pixels);
    qt.matrix =
        QuickTimeMatrix::scale_translate(Fixed(i32::MAX), Fixed(i32::MAX), Fixed(0), Fixed(0));
    let img = parse_pict(&pict_with((0, 0, 4, 4), &qt)).unwrap();
    assert!(img.quicktime[0].render.is_rendered());
    assert_eq!(px(&img, 3, 3), RED);

    // Singular matrix: typed failure, nothing drawn.
    let mut qt = raw_still(2, 2, &pixels);
    qt.matrix = QuickTimeMatrix::scale_translate(Fixed(0), ONE, Fixed(0), Fixed(0));
    let img = parse_pict(&pict_with((0, 0, 4, 4), &qt)).unwrap();
    assert!(matches!(
        img.quicktime[0].render,
        QuickTimeRender::Failed(_)
    ));
    assert!(img.data.chunks_exact(4).all(|p| p == WHITE));

    // Perspective column (u ≠ 0): drawn, bounded, no panic.
    let mut qt = raw_still(2, 2, &pixels);
    qt.matrix = QuickTimeMatrix::IDENTITY_FRACT;
    qt.matrix.0[2] = Fixed(1 << 24);
    let img = parse_pict(&pict_with((0, 0, 4, 4), &qt)).unwrap();
    assert!(img.quicktime[0].render.is_rendered());
}

// ---------------------------------------------------------------------------
// Mask region (destination space), matte (source space), transfer mode.
// ---------------------------------------------------------------------------

#[test]
fn mask_region_clips_in_destination_space() {
    let pixels = [[255, 0, 0]; 16];
    let mut qt = raw_still(4, 4, &pixels);
    // Rectangular Region (rgnSize 10 + bbox) covering (1,1)–(3,3).
    let mut rgn = Vec::new();
    rgn.extend_from_slice(&10u16.to_be_bytes());
    for v in [1i16, 1, 3, 3] {
        rgn.extend_from_slice(&v.to_be_bytes());
    }
    qt.mask_region = Some(rgn);
    let img = parse_pict(&pict_with((0, 0, 4, 4), &qt)).unwrap();
    for y in 0..4 {
        for x in 0..4 {
            let inside = (1..3).contains(&x) && (1..3).contains(&y);
            assert_eq!(
                px(&img, x, y),
                if inside { RED } else { WHITE },
                "({x},{y})"
            );
        }
    }
    // The mask is not moved by the matrix: translate the image by 1
    // and the *same* destination window stays.
    qt.matrix = QuickTimeMatrix::scale_translate(ONE, ONE, ONE, ONE);
    let img = parse_pict(&pict_with((0, 0, 6, 6), &qt)).unwrap();
    assert_eq!(px(&img, 1, 1), RED);
    assert_eq!(px(&img, 2, 2), RED);
    assert_eq!(px(&img, 3, 3), WHITE);
    assert_eq!(px(&img, 0, 0), WHITE);
}

#[test]
fn matte_blends_per_copy_deep_mask() {
    // 3×1 red image; matte weights black / white / mid-gray.
    let pixels = [[255, 0, 0]; 3];
    let mut qt = raw_still(3, 1, &pixels);
    let matte_pixels = [[0, 0, 0], [255, 255, 255], [128, 128, 128]];
    qt.matte = Some(QuickTimeMatte {
        description: desc(*b"raw ", 3, 1, 32),
        data: raw32(&matte_pixels),
    });
    qt.matte_rect = RectI32::from_be(0, 0, 1, 3);
    let img = parse_pict(&pict_with((0, 0, 1, 3), &qt)).unwrap();
    // Black mask → source; white → destination (the white paper);
    // 128 → (127·src + 128·dst + 127) / 255 per component.
    assert_eq!(px(&img, 0, 0), RED);
    assert_eq!(px(&img, 1, 0), WHITE);
    assert_eq!(px(&img, 2, 0), [255, 128, 128, 255]);
    assert_eq!(
        img.quicktime[0].render,
        QuickTimeRender::Rendered {
            dst: RectI32::from_be(0, 0, 1, 3),
            matte_skipped: None,
        }
    );
}

#[test]
fn undecodable_matte_draws_unblended_and_says_so() {
    let pixels = [[255, 0, 0]; 4];
    let mut qt = raw_still(2, 2, &pixels);
    qt.matte = Some(QuickTimeMatte {
        description: desc(*b"rpza", 2, 2, 16),
        data: vec![0u8; 8],
    });
    let img = parse_pict(&pict_with((0, 0, 2, 2), &qt)).unwrap();
    assert_eq!(px(&img, 0, 0), RED);
    match &img.quicktime[0].render {
        QuickTimeRender::Rendered {
            matte_skipped: Some(reason),
            ..
        } => {
            assert!(reason.contains("rpza"), "{reason}");
        }
        other => panic!("expected Rendered with matte_skipped, got {other:?}"),
    }
}

#[test]
fn transfer_mode_word_is_honoured() {
    // notSrcCopy (4) reverses foreground and background: black source
    // pixels take the (white) background, white ones the (black)
    // foreground — Imaging With QuickDraw §4-34.
    let pixels = [[0, 0, 0], [255, 255, 255]];
    let mut qt = raw_still(2, 1, &pixels);
    qt.mode = 4;
    let img = parse_pict(&pict_with((0, 0, 1, 2), &qt)).unwrap();
    assert_eq!(px(&img, 0, 0), WHITE);
    assert_eq!(px(&img, 1, 0), [0, 0, 0, 255]);
    // ditherCopy (64) added to srcCopy is still a copy.
    qt.mode = 64;
    let img = parse_pict(&pict_with((0, 0, 1, 2), &qt)).unwrap();
    assert_eq!(px(&img, 0, 0), [0, 0, 0, 255]);
    assert_eq!(px(&img, 1, 0), WHITE);
}

// ---------------------------------------------------------------------------
// Unsupported compressors stay typed; caller-supplied decoders plug in.
// ---------------------------------------------------------------------------

#[test]
fn unsupported_compressor_is_typed_and_leaves_the_canvas_alone() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.rect(oxideav_pict::Verb::Paint, 0, 0, 1, 1);
    let qt = QuickTimeCompressed::still(desc(*b"rpza", 4, 4, 16), vec![0xAB; 40]);
    b.compressed_quicktime_image(&qt).unwrap();
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(px(&img, 0, 0), [0, 0, 0, 255]); // the painted rect survives
    assert_eq!(px(&img, 2, 2), WHITE);
    let q = &img.quicktime[0];
    assert!(q.image.is_some(), "wrapper stays typed");
    match &q.render {
        QuickTimeRender::Unsupported { codec, reason } => {
            assert_eq!(codec, b"rpza");
            assert!(reason.contains("rpza"), "{reason}");
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
    // Indexed 'raw ' depths need a colour table the staged docs do not
    // lay out: also typed as unsupported, not a failure.
    let qt = QuickTimeCompressed::still(desc(*b"raw ", 2, 2, 8), vec![0u8; 4]);
    let img = parse_pict(&pict_with((0, 0, 2, 2), &qt)).unwrap();
    assert!(matches!(
        img.quicktime[0].render,
        QuickTimeRender::Unsupported { codec, .. } if codec == *b"raw "
    ));
}

/// A caller-supplied decoder for a made-up compressor: paints every
/// pixel the colour spelled by the first three data bytes.
struct SolidDecoder {
    calls: usize,
}

impl QuickTimeImageDecoder for SolidDecoder {
    fn decode_image(
        &mut self,
        description: &ImageDescription,
        data: &[u8],
    ) -> oxideav_pict::Result<DecodedQuickTimeImage> {
        self.calls += 1;
        if &description.codec != b"solD" {
            return Err(PictError::unsupported("SolidDecoder only knows 'solD'"));
        }
        let n = description.width as usize * description.height as usize;
        let mut rgba = Vec::with_capacity(n * 4);
        for _ in 0..n {
            rgba.extend_from_slice(&[data[0], data[1], data[2], 255]);
        }
        DecodedQuickTimeImage::new(description.width as u32, description.height as u32, rgba)
    }
}

#[test]
fn parse_pict_with_routes_images_through_the_caller_decoder() {
    let qt = QuickTimeCompressed::still(desc(*b"solD", 3, 2, 24), vec![0, 200, 0]);
    let bytes = pict_with((0, 0, 2, 3), &qt);
    let mut dec = SolidDecoder { calls: 0 };
    let img = parse_pict_with(&bytes, &mut dec).unwrap();
    assert_eq!(dec.calls, 1);
    assert!(img.data.chunks_exact(4).all(|p| p == [0, 200, 0, 255]));
    // The default chain does not know 'solD'.
    let img = parse_pict(&bytes).unwrap();
    assert!(matches!(
        img.quicktime[0].render,
        QuickTimeRender::Unsupported { codec, .. } if codec == *b"solD"
    ));
}

// ---------------------------------------------------------------------------
// $8201 goes through the same compositor: wrapper matrix + matte apply.
// ---------------------------------------------------------------------------

#[test]
fn uncompressed_quicktime_honours_wrapper_matrix_and_matte() {
    let red: Vec<u8> = RED.repeat(4);
    let sub = build_direct_bits_rect_op(0, 0, 2, 2, &red, PackType::Raw).unwrap();
    let mut u = QuickTimeUncompressed::wrapping(&sub).unwrap();
    // Translate the subopcode's dstRect by (+4, +1).
    u.matrix = QuickTimeMatrix::scale_translate(ONE, ONE, Fixed(0x0004_0000), ONE);
    u.matte = Some(QuickTimeMatte {
        description: desc(*b"raw ", 2, 2, 32),
        data: raw32(&[[0, 0, 0], [255, 255, 255], [0, 0, 0], [255, 255, 255]]),
    });
    u.matte_rect = RectI32::from_be(0, 0, 2, 2);
    let mut b = PictBuilder::new(0, 0, 4, 8);
    b.uncompressed_quicktime_image(&u).unwrap();
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(px(&img, 0, 0), WHITE);
    assert_eq!(px(&img, 4, 1), RED); // matte black → source
    assert_eq!(px(&img, 5, 1), WHITE); // matte white → destination
    assert_eq!(px(&img, 4, 2), RED);
    assert_eq!(px(&img, 5, 2), WHITE);
    assert_eq!(
        img.quicktime[0].render,
        QuickTimeRender::Rendered {
            dst: RectI32::from_be(1, 4, 3, 6),
            matte_skipped: None,
        }
    );
}

// ---------------------------------------------------------------------------
// 'jpeg' through oxideav-mjpeg (registry feature).
// ---------------------------------------------------------------------------

#[cfg(feature = "registry")]
#[test]
fn photo_jpeg_payload_decodes_through_the_sibling_decoder() {
    use oxideav_core::{PixelFormat, VideoFrame, VideoPlane};

    // Packed-RGB JPEG (Adobe transform 0) of a flat colour.
    let (w, h) = (16u32, 8u32);
    let rgb: Vec<u8> = [200u8, 30, 60].repeat((w * h) as usize);
    let jpeg = oxideav_mjpeg::encoder::encode_jpeg_rgb24(w, h, &rgb, w as usize * 3, 95).unwrap();
    let qt = QuickTimeCompressed::still(desc(*b"jpeg", w as u16, h as u16, 24), jpeg);
    let img = parse_pict(&pict_with((0, 0, h as i16, w as i16), &qt)).unwrap();
    assert!(
        img.quicktime[0].render.is_rendered(),
        "{:?}",
        img.quicktime[0].render
    );
    for p in img.data.chunks_exact(4) {
        for (c, want) in p[..3].iter().zip([200u8, 30, 60]) {
            assert!(
                (*c as i32 - want as i32).abs() <= 4,
                "pixel {p:?} vs {:?}",
                [200, 30, 60]
            );
        }
    }

    // Planar 4:2:0 Y′CbCr JPEG of a flat mid-gray.
    let frame = VideoFrame {
        pts: None,
        planes: vec![
            VideoPlane {
                stride: w as usize,
                data: vec![128; (w * h) as usize],
            },
            VideoPlane {
                stride: (w / 2) as usize,
                data: vec![128; (w * h / 4) as usize],
            },
            VideoPlane {
                stride: (w / 2) as usize,
                data: vec![128; (w * h / 4) as usize],
            },
        ],
    };
    let jpeg = oxideav_mjpeg::encoder::encode_jpeg(&frame, w, h, PixelFormat::Yuv420P, 90).unwrap();
    let qt = QuickTimeCompressed::still(desc(*b"jpeg", w as u16, h as u16, 24), jpeg);
    let img = parse_pict(&pict_with((0, 0, h as i16, w as i16), &qt)).unwrap();
    assert!(img.quicktime[0].render.is_rendered());
    for p in img.data.chunks_exact(4) {
        assert!(p[..3].iter().all(|&c| (c as i32 - 128).abs() <= 2), "{p:?}");
        assert_eq!(p[3], 255);
    }

    // Garbage in a 'jpeg' payload is a typed failure, not a panic.
    let qt = QuickTimeCompressed::still(desc(*b"jpeg", 4, 4, 24), vec![0xFF, 0xD8, 0x00, 0x01]);
    let img = parse_pict(&pict_with((0, 0, 4, 4), &qt)).unwrap();
    assert!(matches!(
        img.quicktime[0].render,
        QuickTimeRender::Failed(_)
    ));
}

#[cfg(feature = "registry")]
#[test]
fn registry_decoder_falls_back_to_the_default_chain() {
    use oxideav_core::CodecRegistry;
    use oxideav_pict::RegistryQuickTimeDecoder;

    let reg = CodecRegistry::new();
    let mut qt = RegistryQuickTimeDecoder::new(&reg);
    let pixels = [[0, 0, 255]; 4];
    let bytes = pict_with((0, 0, 2, 2), &raw_still(2, 2, &pixels));
    let img = parse_pict_with(&bytes, &mut qt).unwrap();
    assert_eq!(px(&img, 1, 1), [0, 0, 255, 255]);

    // The framework decoder path (make_decoder → send_packet) renders
    // 'raw ' too, so `oxideav convert` gets the pixels.
    use oxideav_core::{CodecId, CodecParameters, Frame, Packet, TimeBase};
    let mut dec =
        oxideav_pict::decoder::make_decoder(&CodecParameters::video(CodecId::new("pict"))).unwrap();
    dec.send_packet(&Packet::new(0, TimeBase::new(1, 1), bytes))
        .unwrap();
    let Frame::Video(v) = dec.receive_frame().unwrap() else {
        panic!("video frame expected");
    };
    assert_eq!(&v.planes[0].data[12..16], &[0, 0, 255, 255]);
}

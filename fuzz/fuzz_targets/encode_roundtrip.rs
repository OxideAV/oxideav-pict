#![no_main]
//! Encoder under fuzzed geometry and options: the first bytes pick
//! width / height / pack type / version / stub / header kind / origin
//! and the rest are pixels. Every file the encoder writes must decode
//! back to the same `Rgba` pixels for the lossless pack types, and
//! `info` must agree with the picture frame.

use libfuzzer_sys::fuzz_target;
use oxideav_pict::{EncodeOptions, PackType, PictImage, PictVersion};

fuzz_target!(|data: &[u8]| {
    if data.len() < 6 {
        return;
    }
    let w = u32::from(data[0] % 48) + 1;
    let h = u32::from(data[1] % 48) + 1;
    let sel = data[2];
    let pack = match sel & 3 {
        0 => PackType::Raw,
        1 => PackType::Packed24,
        2 => PackType::Rle16,
        _ => PackType::ComponentPackBits,
    };
    let version = if sel & 4 != 0 {
        PictVersion::V1
    } else {
        PictVersion::V2
    };
    let origin = (i16::from(data[3] as i8), i16::from(data[4] as i8));
    let need = (w * h * 4) as usize;
    let mut rgba = data[5..].to_vec();
    rgba.resize(need, 0x5A);
    for px in rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
    let img = PictImage::from_rgba8(w, h, rgba.clone()).unwrap();
    let opts = EncodeOptions::default()
        .with_pack(pack)
        .with_version(version)
        .with_launch_stub(if sel & 8 != 0 { Some(sel & 16 != 0) } else { None })
        .with_extended_header(if sel & 32 != 0 { Some(sel & 64 != 0) } else { None })
        .with_frame_origin(origin.0, origin.1)
        .with_clip(if sel & 128 != 0 {
            Some([origin.0, origin.1, origin.0 + h as i16 / 2, origin.1 + w as i16 / 2])
        } else {
            None
        });
    let bytes = oxideav_pict::encode(&img, &opts).expect("encoder accepts every small image");
    assert!(oxideav_pict::probe(&bytes));
    let info = oxideav_pict::info(&bytes).expect("info on encoder output");
    assert_eq!((info.width, info.height), (w, h));
    let back = oxideav_pict::decode(&bytes).expect("decode encoder output");
    assert_eq!((back.width, back.height), (w, h));
    if pack != PackType::Rle16 && opts.clip.is_none() {
        assert_eq!(back.planes[0].data, rgba);
    }
});

#![no_main]
//! Whole-file decode through the contract entry points: `probe`,
//! `info`, `decode` (the default QuickTime decoder chain — `'raw '`
//! built in, `'jpeg'` via `oxideav-mjpeg`), `decode_rgb8` /
//! `decode_rgba8`, and `decode_with` under a byte-derived mix of
//! limits and strictness. Errors are fine; panics and runaway
//! allocation are the findings. Invariant: `probe == false` implies
//! `info` and `decode` fail; a decoded image has exactly one plane of
//! `width × 4 × height` bytes.

use libfuzzer_sys::fuzz_target;
use oxideav_pict::DecodeOptions;

fuzz_target!(|data: &[u8]| {
    let probed = oxideav_pict::probe(data);
    let info = oxideav_pict::info(data);
    if !probed {
        assert!(info.is_err());
    }
    match oxideav_pict::decode(data) {
        Ok(img) => {
            assert!(probed);
            assert_eq!(img.planes.len(), 1);
            assert_eq!(img.planes[0].stride, img.width as usize * 4);
            assert_eq!(
                img.planes[0].data.len(),
                img.width as usize * 4 * img.height as usize
            );
            let i = info.expect("decode implies info");
            assert_eq!((i.width, i.height), (img.width, img.height));
            assert_eq!(img.to_rgba8().len(), img.planes[0].data.len());
            assert_eq!(img.to_rgb8().len(), img.width as usize * 3 * img.height as usize);
        }
        Err(_) => {}
    }
    let _ = oxideav_pict::decode_rgb8(data);
    let _ = oxideav_pict::decode_rgba8(data);
    let sel = data.last().copied().unwrap_or(0);
    let opts = DecodeOptions::default()
        .with_strict(sel & 1 != 0)
        .with_max_width(if sel & 2 != 0 { Some(64) } else { None })
        .with_max_height(if sel & 4 != 0 { Some(64) } else { None })
        .with_max_pixels(if sel & 8 != 0 { Some(1 << 12) } else { None })
        .with_max_bytes(if sel & 16 != 0 { Some(1 << 16) } else { Some(1 << 24) });
    let _ = oxideav_pict::decode_with(data, &opts);
});

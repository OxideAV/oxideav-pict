#![no_main]
//! The `pict` container through the registry (feature `registry`): the
//! structural probe with and without an extension hint, the demuxer, the
//! registry decoder on the packet, and the muxer on the demuxed packet.
//! Every input must end in `Ok` or `Err` — never a panic, a debug
//! overflow or an attacker-sized allocation.

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use oxideav_core::{CodecId, CodecParameters, ProbeData, RuntimeContext, StreamInfo, TimeBase};

fuzz_target!(|data: &[u8]| {
    if data.len() > 1 << 20 {
        return;
    }
    let mut ctx = RuntimeContext::new();
    oxideav_pict::register(&mut ctx);
    let _ = oxideav_pict::container::probe(&ProbeData {
        buf: data,
        ext: None,
    });
    let _ = oxideav_pict::container::probe(&ProbeData {
        buf: data,
        ext: Some("pict"),
    });
    let _ = ctx
        .containers
        .probe_input(&mut Cursor::new(data), Some("pct"));
    let Ok(mut demuxer) = ctx.containers.open_demuxer(
        "pict",
        Box::new(Cursor::new(data.to_vec())),
        &ctx.codecs,
    ) else {
        return;
    };
    let stream = demuxer.streams()[0].clone();
    let Ok(pkt) = demuxer.next_packet() else {
        return;
    };
    if let Ok(mut dec) = ctx.codecs.first_decoder(&stream.params) {
        if dec.send_packet(&pkt).is_ok() {
            let _ = dec.receive_frame();
        }
    }
    let out_stream = StreamInfo {
        index: 0,
        time_base: TimeBase::new(1, 1),
        duration: None,
        start_time: Some(0),
        params: CodecParameters::video(CodecId::new(oxideav_pict::CODEC_ID_STR)),
    };
    if let Ok(mut muxer) = ctx.containers.open_muxer(
        "pict",
        Box::new(Cursor::new(Vec::<u8>::new())),
        &[out_stream],
    ) {
        let _ = muxer.write_header();
        let _ = muxer.write_packet(&pkt);
        let _ = muxer.write_packet(&pkt);
        let _ = muxer.write_trailer();
    }
});

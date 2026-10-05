//! PICT container: demuxer + muxer for the `oxideav-core` container
//! registry (the `pict` container, `.pict` / `.pct` / `.pic`).
//!
//! A PICT file *is* the picture body (optionally after Apple's 512-byte
//! launch stub), so the framework packet is the whole file and the
//! registry [`crate::make_decoder`] rasterises it with [`crate::decode`]
//! (one implementation):
//!
//! * **Probe.** PICT has no magic at offset 0. The score comes from the
//!   picture-record structure, the way Layer 1 [`crate::probe`] sniffs
//!   it: the §A-3 version stanza at offset 10 of a record starting at
//!   byte 0 or byte 512 (`$0011 $02FF` for version 2, `$11 $01` for
//!   version 1), then a non-rasterising walk of the opcode stream
//!   ([`crate::inspect`]). A walk that reaches `OpEndPic` scores 90; one
//!   that ends cleanly between opcodes 60; a record whose header parses
//!   but whose opcodes are cut (a picture larger than the probe buffer,
//!   or garbage after the header) 50 for v2 — the 6-byte `$0011 $02FF
//!   $0C00` signature — and 25 for v1; a stanza whose record header does
//!   not parse 30 (v2) / 15 (v1); a v1 record with no parsed opcode (a
//!   garbage first opcode, or a bare `OpEndPic` — an empty picture) is
//!   noise and scores 0 (a v1 picture is bounded by its 16-bit
//!   `picSize`, so it is never cut by the probe buffer). A `.pict` /
//!   `.pct` / `.pic` hint lifts
//!   any stanza match to at least 75 and a bare hint scores 25. Foreign
//!   files never carry the stanza at those offsets (pinned against the
//!   sibling crates' fixtures and synthesised headers).
//! * **Demuxer.** One video stream: `width` / `height` from `picFrame`
//!   (what [`crate::info`] reports), native `pixel_format` `Rgba` (the
//!   layout the decoder emits), `codec_id` `pict`, **no colour signal**
//!   (QuickDraw carries no colour information; the crate's documented
//!   default stays on the standalone `ColorInfo`). One packet holding
//!   the whole file, `pts` 0 in a `1/1` time base.
//! * **Muxer.** Exactly one packet — the encoder's complete PICT file —
//!   written verbatim; a second packet is `InvalidData` (a PICT holds
//!   one picture).
//!
//! Gated behind the `registry` feature: every type here comes from
//! `oxideav-core`.

use std::io::{Read, SeekFrom, Write};

use oxideav_core::{
    CodecId, CodecParameters, CodecResolver, ContainerRegistry, Demuxer, Error, MediaType, Muxer,
    Packet, ProbeData, ProbeScore, ReadSeek, Result, StreamInfo, TimeBase, WriteSeek,
    PROBE_SCORE_EXTENSION,
};

use crate::probe::ProbeTermination;
use crate::registry::to_core_pixel_format;
use crate::CODEC_ID_STR;

/// Registered container name (the same string as the codec id).
pub const CONTAINER_NAME: &str = CODEC_ID_STR;

/// File extensions routed to the container.
pub const EXTENSIONS: [&str; 3] = ["pict", "pct", "pic"];

/// Score of a picture whose opcode walk reaches `OpEndPic` inside the
/// probe buffer.
pub const PROBE_SCORE_END_PIC: ProbeScore = 90;
/// Score of a picture whose opcode walk ends cleanly between opcodes at
/// the end of the probe buffer (generators that omit `OpEndPic`).
pub const PROBE_SCORE_EOF: ProbeScore = 60;
/// Score of a v2 record whose 6-byte stanza (`$0011 $02FF $0C00`) and
/// header parse but whose opcode stream does not reach the end of the
/// buffer intact — the usual verdict on a picture larger than the probe
/// buffer, cut inside a raster payload.
pub const PROBE_SCORE_V2_OPCODES_CUT: ProbeScore = 50;
/// Score of a v1 record (2-byte stanza `$11 $01`) whose opcode stream
/// does not reach the end of the buffer intact but parsed at least one
/// opcode; a v1 record with no parsed opcode scores 0 whatever follows.
pub const PROBE_SCORE_V1_OPCODES_CUT: ProbeScore = 25;
/// Score of a v2 stanza whose picture-record header does not parse (no
/// `HeaderOp` after the sentinel, or cut inside the header).
pub const PROBE_SCORE_V2_STANZA: ProbeScore = 30;
/// Score of a v1 stanza whose picture-record header does not parse.
pub const PROBE_SCORE_V1_STANZA: ProbeScore = 15;
/// Floor for a stanza match corroborated by the file extension.
pub const PROBE_SCORE_STANZA_WITH_EXTENSION: ProbeScore = 75;

/// Register the PICT container: demuxer, muxer, extensions and the
/// structural probe.
pub fn register(reg: &mut ContainerRegistry) {
    reg.register_demuxer(CONTAINER_NAME, open_demuxer);
    reg.register_muxer(CONTAINER_NAME, open_muxer);
    for ext in EXTENSIONS {
        reg.register_extension(ext, CONTAINER_NAME);
    }
    reg.register_probe(CONTAINER_NAME, probe);
}

/// `true` when the record at `offset` carries the 4-byte v2 stanza.
fn v2_stanza_at(buf: &[u8], offset: usize) -> bool {
    buf.len() >= offset + 14 && buf[offset + 10..offset + 14] == [0x00, 0x11, 0x02, 0xFF]
}

/// Structural probe (see the module docs).
pub fn probe(data: &ProbeData) -> ProbeScore {
    let ext_hint = data.ext.map(|e| EXTENSIONS.contains(&e)).unwrap_or(false);
    if !crate::probe(data.buf) {
        return if ext_hint { PROBE_SCORE_EXTENSION } else { 0 };
    }
    let v2 = v2_stanza_at(data.buf, 0) || v2_stanza_at(data.buf, 512);
    let score = match crate::inspect(data.buf) {
        // The 2-byte v1 stanza matches 1 in 65 536 random files, and a
        // bare `$11 $01 $FF` is an empty picture `decode` rejects
        // (`NoRaster`): a v1 verdict needs at least one parsed opcode.
        Ok(p) if !v2 && opcodes_seen(&p) == 0 => 0,
        Ok(p) => match p.termination {
            ProbeTermination::EndPic => PROBE_SCORE_END_PIC,
            ProbeTermination::Eof => PROBE_SCORE_EOF,
            _ if v2 => PROBE_SCORE_V2_OPCODES_CUT,
            _ => PROBE_SCORE_V1_OPCODES_CUT,
        },
        Err(_) if v2 => PROBE_SCORE_V2_STANZA,
        Err(_) => PROBE_SCORE_V1_STANZA,
    };
    if score == 0 {
        return if ext_hint { PROBE_SCORE_EXTENSION } else { 0 };
    }
    if ext_hint {
        score.max(PROBE_SCORE_STANZA_WITH_EXTENSION)
    } else {
        score
    }
}

/// Opcodes the walk consumed before it stopped.
fn opcodes_seen(p: &crate::PictProbe) -> u32 {
    p.raster_count
        + p.indexed_raster_count
        + p.drawing_count
        + p.same_shape_count
        + p.text_count
        + p.comment_count
        + p.clip_rgn_count
        + p.pattern_set_count
        + p.pix_pattern_set_count
        + p.compressed_quicktime_count
        + p.uncompressed_quicktime_count
        + p.reserved_op_count
        + p.text_state_op_count
}

// ---------------------------------------------------------------------------
// Demuxer
// ---------------------------------------------------------------------------

/// Open a PICT file as a demuxer (see the module docs).
pub fn open_demuxer(
    mut input: Box<dyn ReadSeek>,
    _codecs: &dyn CodecResolver,
) -> Result<Box<dyn Demuxer>> {
    input.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::new();
    input.read_to_end(&mut buf)?;
    drop(input);
    // Header-only description: picFrame geometry and the decode layout,
    // the same accept / reject verdict as `info` without rasterising.
    let info = crate::info(&buf)?;
    if info.width == 0 || info.height == 0 {
        return Err(Error::invalid(format!(
            "PICT: degenerate picFrame {}×{} (top {}, left {}, bottom {}, right {})",
            info.width, info.height, info.frame.0, info.frame.1, info.frame.2, info.frame.3
        )));
    }
    let mut params = CodecParameters::video(CodecId::new(CODEC_ID_STR));
    params.width = Some(info.width);
    params.height = Some(info.height);
    params.pixel_format = Some(to_core_pixel_format(info.format));
    let stream = StreamInfo {
        index: 0,
        time_base: TimeBase::new(1, 1),
        duration: None,
        start_time: Some(0),
        params,
    };
    Ok(Box::new(PictDemuxer {
        streams: vec![stream],
        data: Some(buf),
    }))
}

struct PictDemuxer {
    streams: Vec<StreamInfo>,
    data: Option<Vec<u8>>,
}

impl Demuxer for PictDemuxer {
    fn format_name(&self) -> &str {
        CONTAINER_NAME
    }
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn next_packet(&mut self) -> Result<Packet> {
        match self.data.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.pts = Some(0);
                pkt.dts = Some(0);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => Err(Error::Eof),
        }
    }
}

// ---------------------------------------------------------------------------
// Muxer
// ---------------------------------------------------------------------------

/// Open a PICT muxer over `output` for exactly one video stream.
pub fn open_muxer(output: Box<dyn WriteSeek>, streams: &[StreamInfo]) -> Result<Box<dyn Muxer>> {
    if streams.len() != 1 {
        return Err(Error::invalid(
            "PICT muxer: expected exactly one video stream",
        ));
    }
    if streams[0].params.media_type != MediaType::Video {
        return Err(Error::invalid("PICT muxer: stream must be video"));
    }
    Ok(Box::new(PictMuxer {
        output,
        written: false,
    }))
}

struct PictMuxer {
    output: Box<dyn WriteSeek>,
    written: bool,
}

impl Muxer for PictMuxer {
    fn format_name(&self) -> &str {
        CONTAINER_NAME
    }
    fn write_header(&mut self) -> Result<()> {
        Ok(())
    }
    fn write_packet(&mut self, packet: &Packet) -> Result<()> {
        if packet.data.is_empty() {
            return Err(Error::invalid("PICT muxer: empty packet"));
        }
        if !crate::probe(&packet.data) {
            return Err(Error::invalid(
                "PICT muxer: packet is not a PICT picture record (no version stanza at offset 10 \
                 or 522)",
            ));
        }
        if self.written {
            return Err(Error::invalid(
                "PICT muxer: a PICT file holds exactly one picture; second packet refused",
            ));
        }
        // The encoder produces a complete PICT file in one packet.
        self.output.write_all(&packet.data)?;
        self.written = true;
        Ok(())
    }
    fn write_trailer(&mut self) -> Result<()> {
        if !self.written {
            return Err(Error::invalid(
                "PICT muxer: no packet written (a PICT file holds one picture)",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EncodeOptions;

    #[test]
    fn probe_scores_by_structure() {
        let v2 = crate::encode_rgba8(3, 2, &[9u8; 24], &EncodeOptions::default()).unwrap();
        let header_end = 512 + 10 + 4 + 2 + 24;
        assert_eq!(
            probe(&ProbeData {
                buf: &v2,
                ext: None
            }),
            PROBE_SCORE_END_PIC
        );
        assert_eq!(
            probe(&ProbeData {
                buf: &v2[512..],
                ext: Some("pct")
            }),
            PROBE_SCORE_END_PIC
        );
        // Cut between opcodes (right after the header): clean Eof.
        assert_eq!(
            probe(&ProbeData {
                buf: &v2[..header_end],
                ext: None
            }),
            PROBE_SCORE_EOF
        );
        // Cut inside the raster payload: the opcode stream is incomplete.
        assert_eq!(
            probe(&ProbeData {
                buf: &v2[..v2.len() - 6],
                ext: None
            }),
            PROBE_SCORE_V2_OPCODES_CUT
        );
        // Stanza + header with garbage opcodes.
        let mut garbage = v2[..header_end].to_vec();
        garbage.extend_from_slice(&[0xFFu8; 8]);
        assert_eq!(
            probe(&ProbeData {
                buf: &garbage,
                ext: None
            }),
            PROBE_SCORE_V2_OPCODES_CUT
        );
        assert_eq!(
            probe(&ProbeData {
                buf: &garbage,
                ext: Some("pict")
            }),
            PROBE_SCORE_STANZA_WITH_EXTENSION
        );
        // v2 stanza without the HeaderOp: the record header does not parse.
        let mut no_header = v2[..512 + 14].to_vec();
        no_header.extend_from_slice(&[0x00, 0xFF]);
        assert_eq!(
            probe(&ProbeData {
                buf: &no_header,
                ext: None
            }),
            PROBE_SCORE_V2_STANZA
        );
        // v1.
        let v1 = crate::encode_pict_v1(3, 2, &[9u8; 24]).unwrap();
        assert_eq!(
            probe(&ProbeData {
                buf: &v1,
                ext: None
            }),
            PROBE_SCORE_END_PIC
        );
        // Cut inside its only opcode (the raster): a v1 picture is bounded
        // by its 16-bit picSize, so a cut is hostile input, not a large
        // picture — zero opcodes parsed, no match.
        assert_eq!(
            probe(&ProbeData {
                buf: &v1[..v1.len() - 3],
                ext: None
            }),
            0
        );
        // Drop the trailing OpEndPic: a clean end between opcodes.
        assert_eq!(
            probe(&ProbeData {
                buf: &v1[..v1.len() - 1],
                ext: None
            }),
            PROBE_SCORE_EOF
        );
        // One opcode parsed, then a second raster opcode cut short.
        let mut one_then_cut = v1[..v1.len() - 1].to_vec();
        one_then_cut.extend_from_slice(&[0x90, 0x00, 0x04]);
        assert_eq!(
            probe(&ProbeData {
                buf: &one_then_cut,
                ext: None
            }),
            PROBE_SCORE_V1_OPCODES_CUT
        );
        // v1 stanza by chance in foreign bytes (JPEG-like header carrying
        // `11 01` at offset 10): with `$FF` next it is an empty picture
        // (`OpEndPic` and nothing drawn), with `$7F` next the first
        // opcode is garbage — neither is a PICT.
        let head = [
            0xFF, 0xD8, 0xFF, 0xC2, 0x00, 0x11, 0x08, 0x00, 0x40, 0x00, 0x11, 0x01,
        ];
        for first_opcode in [0xFFu8, 0x7F] {
            let mut chance = head.to_vec();
            chance.extend_from_slice(&[first_opcode, 0xFE, 0xFD, 0xFC, 0xFB, 0xFA]);
            assert_eq!(
                probe(&ProbeData {
                    buf: &chance,
                    ext: Some("jpg")
                }),
                0,
                "first opcode {first_opcode:#x}"
            );
            assert_eq!(
                probe(&ProbeData {
                    buf: &chance,
                    ext: Some("pict")
                }),
                PROBE_SCORE_EXTENSION
            );
        }
        // No stanza: extension only, or nothing.
        assert_eq!(
            probe(&ProbeData {
                buf: b"farbfeld\0\0\0\x01\0\0\0\x01",
                ext: Some("pic")
            }),
            PROBE_SCORE_EXTENSION
        );
        assert_eq!(
            probe(&ProbeData {
                buf: b"farbfeld\0\0\0\x01\0\0\0\x01",
                ext: Some("ff")
            }),
            0
        );
        assert_eq!(
            probe(&ProbeData {
                buf: &[],
                ext: None
            }),
            0
        );
    }
}

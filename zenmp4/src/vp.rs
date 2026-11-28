//! Canonical ISOBMFF `vpcC` (VP Codec Configuration Box, FullBox version 1)
//! parsing, validation, and synthesis from VP8/VP9 keyframes and WebM TLV records.

use alloc::vec::Vec;
use zencodec_media::track::CodecId;
use zenpixels::Cicp;

use crate::Mp4Error;

/// Parsed fields of a VP Codec Configuration Box (`vpcC`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VpCodecConfig {
    pub profile: u8,
    pub level: u8,
    pub bit_depth: u8,
    /// ISOBMFF VP codec binding chroma subsampling code:
    /// `0` = 4:2:0 vertical, `1` = 4:2:0 colocated, `2` = 4:2:2, `3` = 4:4:4.
    pub chroma_subsampling: u8,
    pub full_range: bool,
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
}

impl VpCodecConfig {
    /// Encode as a canonical 12-byte FullBox v1 `vpcC` payload
    /// (`version = 1, flags = 0, codecIntializationDataSize = 0`).
    #[must_use]
    pub(crate) const fn to_vpcc_v1(self) -> [u8; 12] {
        let b6 = ((self.bit_depth & 0x0F) << 4)
            | ((self.chroma_subsampling & 0x07) << 1)
            | (self.full_range as u8);
        [
            1, // FullBox version = 1
            0,
            0,
            0, // flags = 0
            self.profile,
            self.level,
            b6,
            self.primaries,
            self.transfer,
            self.matrix,
            0,
            0, // codecIntializationDataSize = 0
        ]
    }

    /// Convert the signaled color fields into a [`Cicp`] tag.
    ///
    /// A `vpcC` record always carries an explicit `videoFullRangeFlag`, so even
    /// unspecified primaries/transfer/matrix (`2/2/2`) preserve explicit narrow or
    /// full range when no `colr` box overrides them.
    #[must_use]
    pub(crate) const fn cicp(self) -> Option<Cicp> {
        Some(Cicp::new(
            self.primaries,
            self.transfer,
            self.matrix,
            self.full_range,
        ))
    }

    /// Validate codec-specific `vpcC` constraints per the VP Codec ISO Media File Format Binding.
    #[must_use]
    pub(crate) fn is_valid_for(self, codec: CodecId) -> bool {
        // Matrix 0 (RGB / Identity) requires 4:4:4 chroma and is invalid for subsampled or VP8.
        if self.matrix == 0 && (codec == CodecId::Vp8 || self.chroma_subsampling != 3) {
            return false;
        }
        match codec {
            CodecId::Vp8 => {
                self.profile == 0 && self.bit_depth == 8 && self.chroma_subsampling <= 1
            }
            CodecId::Vp9 => match self.profile {
                0 => self.bit_depth == 8 && self.chroma_subsampling <= 1,
                1 => self.bit_depth == 8 && matches!(self.chroma_subsampling, 2 | 3),
                2 => matches!(self.bit_depth, 10 | 12) && self.chroma_subsampling <= 1,
                3 => matches!(self.bit_depth, 10 | 12) && matches!(self.chroma_subsampling, 2 | 3),
                _ => false,
            },
            _ => false,
        }
    }
}

/// Parse an ISOBMFF `vpcC` FullBox body (version 1 canonical or legacy version 0).
#[must_use]
pub(crate) fn parse_vpcc(body: &[u8]) -> Option<VpCodecConfig> {
    let version = *body.first()?;
    if version == 1 {
        if body.len() != 12 || body[1..4] != [0, 0, 0] {
            return None;
        }
        // VP Codec ISO Media File Format Binding: codecIntializationDataSize MUST be 0
        // for VP8 and VP9.
        let init_len = usize::from(u16::from_be_bytes([body[10], body[11]]));
        if init_len != 0 {
            return None;
        }
        let bit_depth = body[6] >> 4;
        if !matches!(bit_depth, 8 | 10 | 12) {
            return None;
        }
        let chroma_subsampling = (body[6] >> 1) & 0x07;
        if chroma_subsampling > 3 {
            return None;
        }
        Some(VpCodecConfig {
            profile: body[4],
            level: body[5],
            bit_depth,
            chroma_subsampling,
            full_range: (body[6] & 1) != 0,
            primaries: body[7],
            transfer: body[8],
            matrix: body[9],
        })
    } else if version == 0 {
        if body.len() < 10 {
            return None;
        }
        let profile = body[4];
        let level = body[5];
        let bit_depth = body[6] >> 4;
        if !matches!(bit_depth, 8 | 10 | 12) {
            return None;
        }
        let cs = body[6] & 0x0F;
        let chroma_subsampling = body[7] >> 4;
        let transfer_idx = (body[7] >> 1) & 0x07;
        let full_range = (body[7] & 1) != 0;
        let (primaries, mut transfer, matrix) = vp9_colorspace_to_cicp(cs);
        if transfer_idx == 1 {
            transfer = 1;
        } else if transfer_idx == 2 {
            transfer = 16; // SMPTE ST 2084 PQ
        } else if transfer_idx == 3 {
            transfer = 18; // HLG
        }
        Some(VpCodecConfig {
            profile,
            level,
            bit_depth,
            chroma_subsampling: chroma_subsampling.min(3),
            full_range,
            primaries,
            transfer,
            matrix,
        })
    } else {
        None
    }
}

/// Map VP9 bitstream `color_space` (0..=7) to CICP `(primaries, transfer, matrix)`.
const fn vp9_colorspace_to_cicp(cs: u8) -> (u8, u8, u8) {
    match cs {
        1 => (6, 6, 6),  // BT.601
        2 => (1, 1, 1),  // BT.709
        3 => (6, 6, 6),  // SMPTE-170
        4 => (7, 7, 7),  // SMPTE-240
        5 => (9, 14, 9), // BT.2020
        7 => (1, 13, 0), // sRGB / RGB identity
        _ => (2, 2, 2),  // Unknown / reserved
    }
}

struct WebmVp9Tlv {
    profile: Option<u8>,
    level: Option<u8>,
    bit_depth: Option<u8>,
    chroma: Option<u8>,
}

/// Parse WebM `CodecPrivate` VP9 TLV feature list (`[id, len=1, val]...`).
fn parse_webm_vp9_tlv(bytes: &[u8]) -> Option<WebmVp9Tlv> {
    if bytes.is_empty() {
        return None;
    }
    let mut i = 0usize;
    let mut out = WebmVp9Tlv {
        profile: None,
        level: None,
        bit_depth: None,
        chroma: None,
    };
    while i < bytes.len() {
        let id = *bytes.get(i)?;
        let len = usize::from(*bytes.get(i + 1)?);
        if len != 1 || i + 2 + len > bytes.len() {
            return None;
        }
        let val = bytes[i + 2];
        match id {
            1 if val <= 3 => out.profile = Some(val),
            2 => out.level = Some(val),
            3 if matches!(val, 8 | 10 | 12) => out.bit_depth = Some(val),
            4 if val <= 3 => out.chroma = Some(val),
            _ => return None,
        }
        i += 3;
    }
    Some(out)
}

/// MSB-first bit reader for VP9 uncompressed frame headers.
struct Bits<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn f(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.b.get(self.pos / 8)?;
            v = (v << 1) | u32::from((byte >> (7 - (self.pos % 8))) & 1);
            self.pos += 1;
        }
        Some(v)
    }
}

/// Unwrap a VP9 superframe (Annex B) and return its first constituent frame,
/// or return the whole packet slice if it is not a superframe.
fn vp9_first_frame(packet: &[u8]) -> &[u8] {
    if packet.len() < 4 {
        return packet;
    }
    let marker = packet[packet.len() - 1];
    if (marker & 0xE0) != 0xC0 {
        return packet;
    }
    let bytes_per_size = usize::from((marker >> 3) & 0x03) + 1;
    let frames_in_superframe = usize::from(marker & 0x07) + 1;
    let index_sz = 2 + frames_in_superframe * bytes_per_size;
    if packet.len() <= index_sz || packet[packet.len() - index_sz] != marker {
        return packet;
    }
    let size_start = packet.len() - index_sz + 1;
    let mut first_len = 0usize;
    for b in 0..bytes_per_size {
        first_len |= usize::from(packet[size_start + b]) << (b * 8);
    }
    if first_len > 0 && first_len <= packet.len() - index_sz {
        &packet[..first_len]
    } else {
        packet
    }
}

/// Extract VP9 configuration fields from a VP9 keyframe packet (raw frame or superframe).
pub(crate) fn vp9_config_from_keyframe(
    packet: &[u8],
    track_color: Option<Cicp>,
) -> Option<VpCodecConfig> {
    let frame = vp9_first_frame(packet);
    let mut r = Bits { b: frame, pos: 0 };
    if r.f(2)? != 0b10 {
        return None;
    }
    let profile_low = r.f(1)?;
    let profile_high = r.f(1)?;
    let profile = (profile_low | (profile_high << 1)) as u8;
    if profile == 3 && r.f(1)? != 0 {
        return None;
    }
    let show_existing_frame = r.f(1)?;
    if show_existing_frame != 0 {
        return None;
    }
    let frame_type = r.f(1)?;
    if frame_type != 0 {
        // Not a keyframe.
        return None;
    }
    let _show_frame = r.f(1)?;
    let _error_resilient = r.f(1)?;
    // Keyframe sync code 0x49 0x83 0x42
    if r.f(8)? != 0x49 || r.f(8)? != 0x83 || r.f(8)? != 0x42 {
        return None;
    }
    // color_config()
    let bit_depth = if profile >= 2 {
        if r.f(1)? != 0 { 12 } else { 10 }
    } else {
        8
    };
    let cs = r.f(3)? as u8;
    let (mut primaries, mut transfer, mut matrix) = vp9_colorspace_to_cicp(cs);
    let (full_range, chroma_subsampling) = if cs != 7 {
        let full = r.f(1)? != 0;
        let chroma = if profile == 1 || profile == 3 {
            let sx = r.f(1)?;
            let sy = r.f(1)?;
            let _reserved_zero = r.f(1)?;
            match (sx, sy) {
                (0, 0) => 3, // 4:4:4
                (1, 0) => 2, // 4:2:2
                (1, 1) => 0, // 4:2:0 vertical
                _ => return None,
            }
        } else {
            0 // 4:2:0 vertical
        };
        (full, chroma)
    } else {
        // CS_RGB: full range, 4:4:4
        if profile == 1 || profile == 3 {
            let _reserved_zero = r.f(1)?;
        }
        (true, 3)
    };

    let mut effective_full = full_range;
    if let Some(c) = track_color {
        if c.color_primaries != 2 {
            primaries = c.color_primaries;
        }
        if c.transfer_characteristics != 2 {
            transfer = c.transfer_characteristics;
        }
        if c.matrix_coefficients != 2 {
            matrix = c.matrix_coefficients;
        }
        if cs == 0 {
            effective_full = c.full_range;
        }
    }

    Some(VpCodecConfig {
        profile,
        // VP Codec ISO Media File Format Binding: level = 0 denotes undefined/unknown level
        // (avoids falsely capping HD/4K streams at Level 1.0's 36,864-pixel picture limit).
        level: 0,
        bit_depth,
        chroma_subsampling,
        full_range: effective_full,
        primaries,
        transfer,
        matrix,
    })
}

/// Validate or synthesize a canonical ISOBMFF v1 `vpcC` payload for VP8/VP9.
///
/// Returns `Ok(None)` for VP9 when no complete `vpcC` can be constructed without
/// inspecting the first keyframe (either absent `codec_private` or a partial WebM
/// VP9 TLV missing profile, bit depth, or chroma subsampling).
pub(crate) fn canonical_vpcc(
    codec: CodecId,
    private: Option<&[u8]>,
    track_color: Option<Cicp>,
    keyframe: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, Mp4Error> {
    if let Some(bytes) = private {
        if let Some(parsed) = parse_vpcc(bytes)
            && bytes.first() == Some(&1)
        {
            if !parsed.is_valid_for(codec) {
                return Err(Mp4Error::BadCodecPrivate);
            }
            return Ok(Some(bytes.to_vec()));
        }
        if codec == CodecId::Vp9
            && let Some(tlv) = parse_webm_vp9_tlv(bytes)
        {
            let kf = keyframe.and_then(|k| vp9_config_from_keyframe(k, track_color));
            let profile = tlv.profile.or(kf.map(|k| k.profile));
            let bit_depth = tlv.bit_depth.or(kf.map(|k| k.bit_depth));
            let chroma = tlv.chroma.or(kf.map(|k| k.chroma_subsampling));
            let (Some(profile), Some(bit_depth), Some(chroma_subsampling)) =
                (profile, bit_depth, chroma)
            else {
                // Partial WebM TLV (e.g. profile-only `[1, 1, 2]`): defer to keyframe if
                // not yet seen, otherwise reject if keyframe couldn't complete it.
                return if keyframe.is_none() {
                    Ok(None)
                } else {
                    Err(Mp4Error::BadCodecPrivate)
                };
            };
            let c = track_color.unwrap_or_else(|| {
                kf.and_then(VpCodecConfig::cicp)
                    .unwrap_or(Cicp::new(2, 2, 2, false))
            });
            let cfg = VpCodecConfig {
                profile,
                level: tlv.level.unwrap_or(0),
                bit_depth,
                chroma_subsampling,
                full_range: c.full_range,
                primaries: c.color_primaries,
                transfer: c.transfer_characteristics,
                matrix: c.matrix_coefficients,
            };
            if !cfg.is_valid_for(CodecId::Vp9) {
                return Err(Mp4Error::BadCodecPrivate);
            }
            return Ok(Some(cfg.to_vpcc_v1().to_vec()));
        }
        return Err(Mp4Error::BadCodecPrivate);
    }

    match codec {
        CodecId::Vp8 => {
            // RFC 6386 VP8 is strictly profile 0, 8-bit 4:2:0, level undefined (0).
            let c = track_color.unwrap_or(Cicp::new(2, 2, 2, false));
            let cfg = VpCodecConfig {
                profile: 0,
                level: 0,
                bit_depth: 8,
                chroma_subsampling: 0,
                full_range: c.full_range,
                primaries: c.color_primaries,
                transfer: c.transfer_characteristics,
                matrix: c.matrix_coefficients,
            };
            if !cfg.is_valid_for(CodecId::Vp8) {
                return Err(Mp4Error::BadCodecPrivate);
            }
            Ok(Some(cfg.to_vpcc_v1().to_vec()))
        }
        CodecId::Vp9 => {
            if let Some(pkt) = keyframe {
                let cfg =
                    vp9_config_from_keyframe(pkt, track_color).ok_or(Mp4Error::BadCodecPrivate)?;
                if !cfg.is_valid_for(CodecId::Vp9) {
                    return Err(Mp4Error::BadCodecPrivate);
                }
                Ok(Some(cfg.to_vpcc_v1().to_vec()))
            } else {
                Ok(None)
            }
        }
        _ => Err(Mp4Error::UnsupportedCodec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffmpeg_vp9_vpcc_v1_exact() {
        // Exact vpcC body produced by FFmpeg 8.0.1 for 4:4:4 RGB VP9:
        // version=1, flags=0, profile=1, level=10, bitDepth=8, chroma=3, fullRange=1,
        // primaries=2, transfer=2, matrix=0, initSize=0
        let body = [
            0x01, 0x00, 0x00, 0x00, 0x01, 0x0A, 0x87, 0x02, 0x02, 0x00, 0x00, 0x00,
        ];
        let parsed = parse_vpcc(&body).unwrap();
        assert_eq!(parsed.profile, 1);
        assert_eq!(parsed.level, 10);
        assert_eq!(parsed.bit_depth, 8);
        assert_eq!(parsed.chroma_subsampling, 3);
        assert!(parsed.full_range);
        assert_eq!(parsed.matrix, 0);
        assert_eq!(parsed.to_vpcc_v1(), body);
        assert_eq!(parsed.cicp(), Some(Cicp::new(2, 2, 0, true)));
    }

    #[test]
    fn vp8_canonical_vpcc_is_12_byte_v1() {
        let bt601 = Cicp::new(6, 6, 6, false);
        let vpcc = canonical_vpcc(CodecId::Vp8, None, Some(bt601), None)
            .unwrap()
            .unwrap();
        assert_eq!(vpcc.len(), 12);
        let parsed = parse_vpcc(&vpcc).unwrap();
        assert_eq!(parsed.profile, 0);
        assert_eq!(parsed.bit_depth, 8);
        assert_eq!(parsed.chroma_subsampling, 0);
        assert_eq!(parsed.primaries, bt601.color_primaries);
    }

    #[test]
    fn webm_tlv_upgrades_to_canonical_v1() {
        // WebM VP9 CodecPrivate: profile=2, level=31, bit_depth=10, chroma=0
        let tlv = [1, 1, 2, 2, 1, 31, 3, 1, 10, 4, 1, 0];
        let bt2020_pq = Cicp::new(9, 16, 9, false);
        let vpcc = canonical_vpcc(CodecId::Vp9, Some(&tlv), Some(bt2020_pq), None)
            .unwrap()
            .unwrap();
        let parsed = parse_vpcc(&vpcc).unwrap();
        assert_eq!(parsed.profile, 2);
        assert_eq!(parsed.level, 31);
        assert_eq!(parsed.bit_depth, 10);
        assert_eq!(parsed.chroma_subsampling, 0);
        assert_eq!(parsed.primaries, 9);
        assert_eq!(parsed.transfer, 16);
        assert_eq!(parsed.matrix, 9);
    }
}

//! **What the AV1 payload already says about itself.**
//!
//! Every structural property an AVIF container declares about its primary item
//! — the `av1C` profile/level/depth/chroma bits, `pixi`'s channel count and
//! depth, and the `colr` nclx range — is ALSO coded in the sequence header of
//! the payload being muxed. Two independent statements of one fact drift, and
//! this crate has already been bitten once: a caller that set only the colour
//! knobs muxed a profile-0 4:2:0 payload under an `av1C` claiming profile 1 /
//! 4:4:4, which our own decoder ignores (it reads the sequence header) and a
//! strict consumer rejects. That incident is recorded at `build_color_ipma`,
//! and the single-field fix it prompted — reading `seq_profile` out of the
//! payload — is the pattern this module generalises.
//!
//! So: parse the sequence header once, and let the container RESTATE the
//! payload rather than take the caller's word for it.
//!
//! # What this deliberately does NOT decide
//!
//! **Colorimetry.** When a payload codes `color_description_present_flag = 0`
//! it is saying "unspecified" (CICP 2/2/2), and a container that restated that
//! would throw away the primaries and transfer function the `colr` box exists
//! to carry — AVIF's normal division of labour, and what
//! `zenavif`'s aom seam relies on (it codes `matrix_coefficients = 2` in the
//! bitstream and BT.601 in `colr`, because a decoder resolving "unspecified"
//! falls back to BT.601 anyway). The caller's colorimetry therefore stands
//! wherever the payload declines to speak. Where the payload DOES speak
//! (`color_description_present_flag = 1`), the two must agree, and that is a
//! check rather than an override.
//!
//! # Failure policy
//!
//! Every read is bounds-checked and returns `None` rather than panicking: this
//! parses attacker-influenced bytes in a muxer that must not abort. A payload
//! whose sequence header cannot be read (pre-split OBUs, a fixture, a
//! truncation) falls back to the caller's settings — the behaviour every
//! consumer had before any derivation existed.

/// A big-endian bit reader over an OBU payload. `f(n)` in AV1 spec terms
/// (5.3.1), bounds-checked, `n <= 32`.
struct BitReader<'a> {
    data: &'a [u8],
    /// Absolute bit position, so a read past the end is a comparison rather
    /// than an arithmetic edge case.
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// `f(n)`. Returns `None` past the end of the payload.
    fn f(&mut self, n: u32) -> Option<u32> {
        debug_assert!(n <= 32);
        let mut v: u32 = 0;
        for _ in 0..n {
            let byte = *self.data.get(self.pos >> 3)?;
            let bit = (byte >> (7 - (self.pos & 7))) & 1;
            v = (v << 1) | u32::from(bit);
            self.pos += 1;
        }
        Some(v)
    }

    fn flag(&mut self) -> Option<bool> {
        Some(self.f(1)? != 0)
    }

    /// `uvlc()` (AV1 5.9.29). Only reached inside `timing_info`, which nothing
    /// downstream consumes — it is parsed to stay aligned, not to be used.
    fn uvlc(&mut self) -> Option<u32> {
        let mut leading_zeros = 0u32;
        loop {
            if self.flag()? {
                break;
            }
            leading_zeros += 1;
            // The spec caps the value at 32 bits; a longer run is malformed.
            if leading_zeros >= 32 {
                return None;
            }
        }
        if leading_zeros == 0 {
            return Some(0);
        }
        let value = self.f(leading_zeros)?;
        Some(value + (1u32 << leading_zeros) - 1)
    }
}

/// The sequence-header facts an AVIF container restates.
///
/// Field names follow AV1 spec 5.5.1-5.5.2 so they can be read against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SeqHeader {
    pub seq_profile: u8,
    pub seq_level_idx_0: u8,
    pub seq_tier_0: bool,
    /// `max_frame_width_minus_1 + 1`.
    pub max_frame_width: u32,
    /// `max_frame_height_minus_1 + 1`.
    pub max_frame_height: u32,
    /// Derived by `color_config()`: 8, 10 or 12.
    pub bit_depth: u8,
    pub monochrome: bool,
    /// False means the payload codes CICP 2/2/2 ("unspecified") and the
    /// container's own colorimetry stands — see the module doc.
    pub color_description_present: bool,
    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    /// `color_range`: true is full range (0..255 at 8-bit), false is studio.
    pub color_range: bool,
    pub subsampling_x: bool,
    pub subsampling_y: bool,
    pub chroma_sample_position: u8,
}

const OBU_SEQUENCE_HEADER: u8 = 1;

/// Parse the first `OBU_SEQUENCE_HEADER` of an AV1 stream.
///
/// `None` when there is no readable sequence header — see the module doc's
/// failure policy.
pub(crate) fn parse(av1_data: &[u8]) -> Option<SeqHeader> {
    parse_seq_header_payload(find_sequence_header(av1_data)?)
}

/// Walk the OBUs and return the first sequence header's payload.
fn find_sequence_header(av1_data: &[u8]) -> Option<&[u8]> {
    let mut rest = av1_data;
    while let Some((&header, after_header)) = rest.split_first() {
        // obu_forbidden_bit(1) obu_type(4) obu_extension_flag(1)
        // obu_has_size_field(1) obu_reserved_1bit(1)
        if header & 0x80 != 0 {
            return None; // forbidden bit set: not an OBU stream
        }
        let obu_type = (header >> 3) & 0x0f;
        let has_size_field = header & 0x02 != 0;
        let mut body = after_header;
        if header & 0x04 != 0 {
            // obu_extension_flag: one more header byte
            body = body.get(1..)?;
        }
        let payload = if has_size_field {
            let (size, size_len) = read_leb128(body)?;
            let end = size_len.checked_add(usize::try_from(size).ok()?)?;
            let p = body.get(size_len..end)?;
            rest = &body[end..];
            p
        } else {
            // No size field: this OBU runs to the end of the temporal unit.
            rest = &[];
            body
        };
        if obu_type == OBU_SEQUENCE_HEADER {
            return Some(payload);
        }
    }
    None
}

/// Minimal leb128 reader for OBU sizes: returns `(value, bytes_consumed)`.
/// AV1 caps `leb128()` at 8 bytes (spec 4.10.5).
fn read_leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    for (i, &byte) in data.iter().take(8).enumerate() {
        value |= u64::from(byte & 0x7f) << (i * 7);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// `sequence_header_obu()`, AV1 spec 5.5.1.
fn parse_seq_header_payload(payload: &[u8]) -> Option<SeqHeader> {
    let r = &mut BitReader::new(payload);

    let seq_profile = u8::try_from(r.f(3)?).ok()?;
    let _still_picture = r.flag()?;
    let reduced_still_picture_header = r.flag()?;

    let seq_level_idx_0;
    let mut seq_tier_0 = false;
    if reduced_still_picture_header {
        seq_level_idx_0 = u8::try_from(r.f(5)?).ok()?;
    } else {
        let mut decoder_model_info_present = false;
        let mut buffer_delay_length = 0u32;
        if r.flag()? {
            // timing_info() — parsed only to stay aligned.
            let _num_units_in_display_tick = r.f(32)?;
            let _time_scale = r.f(32)?;
            if r.flag()? {
                let _num_ticks_per_picture_minus_1 = r.uvlc()?;
            }
            decoder_model_info_present = r.flag()?;
            if decoder_model_info_present {
                buffer_delay_length = r.f(5)? + 1;
                let _num_units_in_decoding_tick = r.f(32)?;
                let _buffer_removal_time_length_minus_1 = r.f(5)?;
                let _frame_presentation_time_length_minus_1 = r.f(5)?;
            }
        }
        let initial_display_delay_present = r.flag()?;
        let operating_points_cnt = r.f(5)? + 1;
        let mut level0 = 0u8;
        for i in 0..operating_points_cnt {
            let _operating_point_idc = r.f(12)?;
            let level = u8::try_from(r.f(5)?).ok()?;
            let tier = if level > 7 { r.flag()? } else { false };
            if i == 0 {
                level0 = level;
                seq_tier_0 = tier;
            }
            if decoder_model_info_present && r.flag()? {
                // operating_parameters_info()
                let _decoder_buffer_delay = r.f(buffer_delay_length)?;
                let _encoder_buffer_delay = r.f(buffer_delay_length)?;
                let _low_delay_mode_flag = r.flag()?;
            }
            if initial_display_delay_present && r.flag()? {
                let _initial_display_delay_minus_1 = r.f(4)?;
            }
        }
        seq_level_idx_0 = level0;
    }

    let frame_width_bits = r.f(4)? + 1;
    let frame_height_bits = r.f(4)? + 1;
    let max_frame_width = r.f(frame_width_bits)?.checked_add(1)?;
    let max_frame_height = r.f(frame_height_bits)?.checked_add(1)?;

    if !reduced_still_picture_header && r.flag()? {
        // frame_id_numbers_present_flag
        let _delta_frame_id_length_minus_2 = r.f(4)?;
        let _additional_frame_id_length_minus_1 = r.f(3)?;
    }

    let _use_128x128_superblock = r.flag()?;
    let _enable_filter_intra = r.flag()?;
    let _enable_intra_edge_filter = r.flag()?;

    if !reduced_still_picture_header {
        let _enable_interintra_compound = r.flag()?;
        let _enable_masked_compound = r.flag()?;
        let _enable_warped_motion = r.flag()?;
        let _enable_dual_filter = r.flag()?;
        let enable_order_hint = r.flag()?;
        if enable_order_hint {
            let _enable_jnt_comp = r.flag()?;
            let _enable_ref_frame_mvs = r.flag()?;
        }
        // seq_choose_screen_content_tools -> SELECT_SCREEN_CONTENT_TOOLS (2)
        let seq_force_screen_content_tools = if r.flag()? { 2 } else { u32::from(r.flag()?) };
        if seq_force_screen_content_tools > 0 {
            // seq_choose_integer_mv
            if !r.flag()? {
                let _seq_force_integer_mv = r.flag()?;
            }
        }
        if enable_order_hint {
            let _order_hint_bits_minus_1 = r.f(3)?;
        }
    }

    let _enable_superres = r.flag()?;
    let _enable_cdef = r.flag()?;
    let _enable_restoration = r.flag()?;

    // ── color_config(), AV1 spec 5.5.2 ────────────────────────────────────
    let high_bitdepth = r.flag()?;
    let bit_depth = if seq_profile == 2 && high_bitdepth {
        if r.flag()? { 12 } else { 10 }
    } else if seq_profile <= 2 {
        if high_bitdepth { 10 } else { 8 }
    } else {
        return None; // seq_profile 3..7 is reserved
    };
    // Profile 1 is 4:4:4 colour only, so `mono_chrome` is not coded.
    let monochrome = if seq_profile == 1 { false } else { r.flag()? };

    let color_description_present = r.flag()?;
    let (color_primaries, transfer_characteristics, matrix_coefficients) = if color_description_present
    {
        (
            u8::try_from(r.f(8)?).ok()?,
            u8::try_from(r.f(8)?).ok()?,
            u8::try_from(r.f(8)?).ok()?,
        )
    } else {
        (2, 2, 2) // CP_UNSPECIFIED / TC_UNSPECIFIED / MC_UNSPECIFIED
    };

    let (color_range, subsampling_x, subsampling_y, chroma_sample_position);
    if monochrome {
        color_range = r.flag()?;
        subsampling_x = true;
        subsampling_y = true;
        chroma_sample_position = 0; // CSP_UNKNOWN
    } else if color_primaries == 1 && transfer_characteristics == 13 && matrix_coefficients == 0 {
        // The sRGB triple. The spec fixes the range full and the format 4:4:4,
        // and codes NO range bit — a reader that consumed one here would be
        // misaligned for everything after it.
        color_range = true;
        subsampling_x = false;
        subsampling_y = false;
        chroma_sample_position = 0;
    } else {
        color_range = r.flag()?;
        match seq_profile {
            0 => {
                subsampling_x = true;
                subsampling_y = true;
            }
            1 => {
                subsampling_x = false;
                subsampling_y = false;
            }
            _ => {
                if bit_depth == 12 {
                    subsampling_x = r.flag()?;
                    subsampling_y = if subsampling_x { r.flag()? } else { false };
                } else {
                    // Profile 2 below 12-bit is 4:2:2 by definition.
                    subsampling_x = true;
                    subsampling_y = false;
                }
            }
        }
        chroma_sample_position = if subsampling_x && subsampling_y {
            u8::try_from(r.f(2)?).ok()?
        } else {
            0
        };
    }
    if !monochrome {
        let _separate_uv_delta_q = r.flag()?;
    }
    let _film_grain_params_present = r.flag()?;

    Some(SeqHeader {
        seq_profile,
        seq_level_idx_0,
        seq_tier_0,
        max_frame_width,
        max_frame_height,
        bit_depth,
        monochrome,
        color_description_present,
        color_primaries,
        transfer_characteristics,
        matrix_coefficients,
        color_range,
        subsampling_x,
        subsampling_y,
        chroma_sample_position,
    })
}

impl SeqHeader {
    /// The `av1C` this payload describes.
    ///
    /// Every field is a restatement of the sequence header, so a strict
    /// consumer that cross-checks the two (Chrome does) cannot find them in
    /// disagreement. Note `seq_level_idx_0`: the container used to hardcode 31
    /// ("unspecified"), discarding a level the encoder had already computed —
    /// `zenav1-aom` ports libaom's `set_bitstream_level_tier` for exactly this
    /// field.
    pub(crate) fn to_av1c(self) -> crate::boxes::Av1CBox {
        crate::boxes::Av1CBox {
            seq_profile: self.seq_profile,
            seq_level_idx_0: self.seq_level_idx_0,
            seq_tier_0: self.seq_tier_0,
            high_bitdepth: self.bit_depth >= 10,
            twelve_bit: self.bit_depth >= 12,
            monochrome: self.monochrome,
            chroma_subsampling_x: self.subsampling_x,
            chroma_subsampling_y: self.subsampling_y,
            chroma_sample_position: self.chroma_sample_position,
        }
    }

    /// The `colr` nclx box that agrees with this payload, starting from what
    /// the caller declared.
    ///
    /// **Range is always taken from the payload** — it is coded in every
    /// sequence header (the sRGB triple fixes it full rather than omitting the
    /// concept), it decides how samples are interpreted, and a container that
    /// contradicts it is the drift this module exists to remove.
    ///
    /// **The CICP triple is taken per FIELD, not per flag.** A payload that
    /// codes `color_description_present_flag = 1` has not necessarily named all
    /// three: `zenav1-aom`'s identity/GBR path signals `(2, 2, 0)` — an
    /// explicit `MC_IDENTITY`, because it changes how the planes are
    /// interpreted, over unspecified primaries and transfer, which it leaves
    /// for `colr` to carry. Deriving all three off the one flag would throw
    /// that colorimetry away.
    ///
    /// A code outside our enums (a CICP value this crate has no variant for)
    /// leaves the caller's value alone rather than guessing.
    pub(crate) fn agreeing_colr(self, declared: crate::boxes::ColrBox) -> crate::boxes::ColrBox {
        use crate::constants::{ColorPrimaries as Cp, MatrixCoefficients as Mc, TransferCharacteristics as Tc};
        let mut out = declared;
        out.full_range_flag = self.color_range;
        if let Some(cp) = (self.color_primaries != 2).then(|| match self.color_primaries {
            1 => Some(Cp::Bt709),
            6 => Some(Cp::Bt601),
            9 => Some(Cp::Bt2020),
            11 => Some(Cp::DciP3),
            12 => Some(Cp::DisplayP3),
            _ => None,
        }).flatten() {
            out.color_primaries = cp;
        }
        #[allow(deprecated)]
        if let Some(tc) = (self.transfer_characteristics != 2).then(|| match self.transfer_characteristics {
            1 => Some(Tc::Bt709),
            4 => Some(Tc::Bt470M),
            5 => Some(Tc::Bt470BG),
            6 => Some(Tc::Bt601),
            7 => Some(Tc::Smpte240),
            8 => Some(Tc::Linear),
            9 => Some(Tc::Log),
            10 => Some(Tc::LogSqrt),
            11 => Some(Tc::Iec61966),
            12 => Some(Tc::Bt1361),
            13 => Some(Tc::Srgb),
            14 => Some(Tc::Bt2020_10),
            15 => Some(Tc::Bt2020_12),
            16 => Some(Tc::Smpte2084),
            17 => Some(Tc::Smpte428),
            18 => Some(Tc::Hlg),
            _ => None,
        }).flatten() {
            out.transfer_characteristics = tc;
        }
        if let Some(mc) = (self.matrix_coefficients != 2).then(|| match self.matrix_coefficients {
            0 => Some(Mc::Rgb),
            1 => Some(Mc::Bt709),
            6 => Some(Mc::Bt601),
            8 => Some(Mc::Ycgco),
            9 => Some(Mc::Bt2020Ncl),
            10 => Some(Mc::Bt2020Cl),
            _ => None,
        }).flatten() {
            out.matrix_coefficients = mc;
        }
        out
    }
}

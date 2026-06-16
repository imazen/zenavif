#![allow(unused)]
#![allow(bad_style)]

use crate::{ChromaSubsampling, Error, Result};
use whereat::at;

use bitreader::BitReader;
use std::num::{NonZeroU8, NonZeroU32};

#[derive(Debug, Clone)]
struct Header {
    obu_size: usize,
    obu_type: u8,
}

impl Header {
    fn is_sequence_header(&self) -> bool {
        self.obu_type == 1
    }

    fn is_frame_header(&self) -> bool {
        // OBU type 3 = Frame Header, type 6 = Frame (contains frame header + tile data)
        self.obu_type == 3 || self.obu_type == 6
    }
}

/// Quantization parameters extracted from an AV1 frame header.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameQuantization {
    /// Base quantizer index (0-255). 0 = lossless candidate.
    pub base_q_idx: u8,
    /// Whether the frame is coded lossless (base_q_idx==0 and all delta-q==0).
    pub coded_lossless: bool,
}

fn get_byte(data: &mut &[u8]) -> Result<u8> {
    let (&b, rest) = (*data).split_first().ok_or_else(|| at!(Error::UnexpectedEOF))?;
    *data = rest;
    Ok(b)
}

const INTRA_FRAME: usize = 0;
const LAST_FRAME: usize = 1;
const LAST2_FRAME: usize = 2;
const LAST3_FRAME: usize = 3;
const GOLDEN_FRAME: usize = 4;
const BWDREF_FRAME: usize = 5;
const ALTREF2_FRAME: usize = 6;
const ALTREF_FRAME: usize = 7;

pub(crate) fn parse_obu(mut data: &[u8]) -> Result<SequenceHeaderObu> {
    let (seq, _) = parse_obu_with_frame_info(data)?;
    Ok(seq)
}

/// Parse OBUs to extract both the sequence header and (optionally) frame quantization info.
///
/// Scans OBUs looking for a sequence header first, then attempts to parse the
/// first frame header to extract quantization parameters for lossless detection.
pub(crate) fn parse_obu_with_frame_info(mut data: &[u8]) -> Result<(SequenceHeaderObu, Option<FrameQuantization>)> {
    let mut seq_header: Option<SequenceHeaderObu> = None;
    let mut frame_quant: Option<FrameQuantization> = None;

    while !data.is_empty() {
        let h = obu_header(&mut data)?;
        let remaining_data = data.get(..h.obu_size).ok_or_else(|| at!(Error::UnexpectedEOF))?;
        data = &data[h.obu_size..];

        if h.is_sequence_header() {
            seq_header = Some(SequenceHeaderObu::read(remaining_data)?);
        } else if h.is_frame_header() && seq_header.is_some() && frame_quant.is_none() {
            // Try to parse frame header for QP; ignore errors (best-effort)
            if let Some(ref seq) = seq_header {
                frame_quant = parse_frame_header_quantization(remaining_data, seq).ok();
            }
        }

        // Once we have both, stop scanning
        if seq_header.is_some() && frame_quant.is_some() {
            break;
        }
    }

    match seq_header {
        Some(seq) => Ok((seq, frame_quant)),
        None => Err(at!(Error::UnexpectedEOF)),
    }
}

impl SequenceHeaderObu {
    fn read(data: &[u8]) -> Result<Self> {
        let mut b = BitReader::new(data);

        let seq_profile = b.read_u8(3).map_err(|e| at!(Error::from(e)))?;
        if seq_profile > 2 {
            return Err(at!(Error::InvalidData("seq_profile")));
        }
        let still_picture = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        let reduced_still_picture_header = b.read_bool().map_err(|e| at!(Error::from(e)))?;

        let decoder_model_info_present_flag = false;
        read_operating_points(
            &mut b,
            reduced_still_picture_header,
            decoder_model_info_present_flag,
        )?;

        let frame_dims = read_max_frame_dims(&mut b)?;
        let frame_ids = read_frame_id_config(&mut b, reduced_still_picture_header)?;

        let use_128x128_superblock = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        let _enable_filter_intra = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        let _enable_intra_edge_filter = b.read_bool().map_err(|e| at!(Error::from(e)))?;

        let motion = read_motion_and_screen_content_flags(&mut b, reduced_still_picture_header)?;

        let enable_superres = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        let enable_cdef = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        let enable_restoration = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        let color = color_config(&mut b, seq_profile)?;
        let film_grain_params_present = b.read_bool().map_err(|e| at!(Error::from(e)))?;

        Ok(Self {
            color,
            seq_profile,
            still_picture,
            reduced_still_picture_header,
            max_frame_width: frame_dims.max_width,
            max_frame_height: frame_dims.max_height,
            frame_width_bits: frame_dims.width_bits,
            frame_height_bits: frame_dims.height_bits,
            enable_superres,
            enable_cdef,
            enable_restoration,
            frame_id_numbers_present_flag: frame_ids.present,
            delta_frame_id_length: frame_ids.delta_length,
            additional_frame_id_length: frame_ids.additional_length,
            film_grain_params_present,
            decoder_model_info_present_flag,
            seq_force_screen_content_tools: motion.seq_force_screen_content_tools,
            seq_force_integer_mv: motion.seq_force_integer_mv,
            order_hint_bits: motion.order_hint_bits,
            enable_order_hint: motion.enable_order_hint,
            use_128x128_superblock,
            enable_interintra_compound: motion.enable_interintra_compound,
            enable_masked_compound: motion.enable_masked_compound,
            enable_warped_motion: motion.enable_warped_motion,
            enable_dual_filter: motion.enable_dual_filter,
            enable_jnt_comp: motion.enable_jnt_comp,
            enable_ref_frame_mvs: motion.enable_ref_frame_mvs,
        })
    }
}

/// Per-operating-point fields read for their side effect on the bit position.
/// Errors propagate exactly as the inline code did (timing info / decoder
/// model trigger `Unsupported`).
fn read_operating_points(
    b: &mut BitReader,
    reduced_still_picture_header: bool,
    decoder_model_info_present_flag: bool,
) -> Result<()> {
    if reduced_still_picture_header {
        let _seq_level_idx = b.read_u8(5).map_err(|e| at!(Error::from(e)))?;
        return Ok(());
    }

    let timing_info_present_flag = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    if timing_info_present_flag {
        return Err(at!(Error::Unsupported("timing_info_present_flag")));
    }
    let initial_display_delay_present_flag = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    let operating_points_cnt = 1 + b.read_u8(5).map_err(|e| at!(Error::from(e)))?;

    for _ in 0..operating_points_cnt {
        let _operating_point_idc = b.read_u16(12).map_err(|e| at!(Error::from(e)))?;
        let seq_level_idx = b.read_u8(5).map_err(|e| at!(Error::from(e)))?;
        let _seq_tier = if seq_level_idx > 7 { b.read_bool().map_err(|e| at!(Error::from(e)))? } else { false };
        if decoder_model_info_present_flag {
            b.read_bool().map_err(|e| at!(Error::from(e)))?;
            return Err(at!(Error::Unsupported("decoder_model_info_present_flag")));
        }
        if initial_display_delay_present_flag {
            let initial_display_delay_present_for_this_op = b.read_bool().map_err(|e| at!(Error::from(e)))?;
            if initial_display_delay_present_for_this_op {
                let _initial_display_delay = 1 + b.read_u8(4).map_err(|e| at!(Error::from(e)))?;
            }
        }
    }
    Ok(())
}

struct MaxFrameDims {
    max_width: NonZeroU32,
    max_height: NonZeroU32,
    width_bits: u8,
    height_bits: u8,
}

/// Read frame_width_bits, frame_height_bits, and the (1 + raw) max
/// frame width/height fields. Returns an error on the 0-after-+1 overflow
/// case so the `NonZeroU32` check stays at the original position.
fn read_max_frame_dims(b: &mut BitReader) -> Result<MaxFrameDims> {
    let width_bits = 1 + b.read_u8(4).map_err(|e| at!(Error::from(e)))?;
    let height_bits = 1 + b.read_u8(4).map_err(|e| at!(Error::from(e)))?;
    let raw_width = 1 + b.read_u32(width_bits).map_err(|e| at!(Error::from(e)))?;
    let raw_height = 1 + b.read_u32(height_bits).map_err(|e| at!(Error::from(e)))?;
    let max_width = NonZeroU32::new(raw_width).ok_or_else(|| at!(Error::InvalidData("overflow")))?;
    let max_height = NonZeroU32::new(raw_height).ok_or_else(|| at!(Error::InvalidData("overflow")))?;
    Ok(MaxFrameDims { max_width, max_height, width_bits, height_bits })
}

struct FrameIdConfig {
    present: bool,
    delta_length: u8,
    additional_length: u8,
}

fn read_frame_id_config(b: &mut BitReader, reduced_still_picture_header: bool) -> Result<FrameIdConfig> {
    let present = if reduced_still_picture_header { false } else { b.read_bool().map_err(|e| at!(Error::from(e)))? };
    let delta_length = if present { 2 + b.read_u8(4).map_err(|e| at!(Error::from(e)))? } else { 0 };
    let additional_length = if present { 1 + b.read_u8(3).map_err(|e| at!(Error::from(e)))? } else { 0 };
    Ok(FrameIdConfig { present, delta_length, additional_length })
}

struct MotionAndScreenContent {
    enable_interintra_compound: bool,
    enable_masked_compound: bool,
    enable_warped_motion: bool,
    enable_dual_filter: bool,
    enable_jnt_comp: bool,
    enable_ref_frame_mvs: bool,
    enable_order_hint: bool,
    order_hint_bits: u8,
    seq_force_screen_content_tools: u8,
    seq_force_integer_mv: u8,
}

/// Read the block of motion / order-hint / screen-content-tools / integer-mv
/// flags that only exist when `!reduced_still_picture_header`.
fn read_motion_and_screen_content_flags(
    b: &mut BitReader,
    reduced_still_picture_header: bool,
) -> Result<MotionAndScreenContent> {
    let mut out = MotionAndScreenContent {
        enable_interintra_compound: false,
        enable_masked_compound: false,
        enable_warped_motion: false,
        enable_dual_filter: false,
        enable_jnt_comp: false,
        enable_ref_frame_mvs: false,
        enable_order_hint: false,
        order_hint_bits: 0,
        seq_force_screen_content_tools: SELECT_SCREEN_CONTENT_TOOLS,
        seq_force_integer_mv: SELECT_INTEGER_MV,
    };

    if reduced_still_picture_header {
        return Ok(out);
    }

    out.enable_interintra_compound = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    out.enable_masked_compound = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    out.enable_warped_motion = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    out.enable_dual_filter = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    out.enable_order_hint = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    if out.enable_order_hint {
        out.enable_jnt_comp = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        out.enable_ref_frame_mvs = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    }
    let seq_choose_screen_content_tools = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    if !seq_choose_screen_content_tools {
        out.seq_force_screen_content_tools = b.read_u8(1).map_err(|e| at!(Error::from(e)))?;
    }
    if out.seq_force_screen_content_tools > 0 {
        let seq_choose_integer_mv = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        if !seq_choose_integer_mv {
            out.seq_force_integer_mv = b.read_u8(1).map_err(|e| at!(Error::from(e)))?;
        }
    }
    if out.enable_order_hint {
        out.order_hint_bits = 1 + b.read_u8(3).map_err(|e| at!(Error::from(e)))?;
    }
    Ok(out)
}

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct SequenceHeaderObu {
    pub color: ColorConfig,

    pub seq_profile: u8,
    pub still_picture: bool,
    pub reduced_still_picture_header: bool,

    pub max_frame_width: NonZeroU32,
    pub max_frame_height: NonZeroU32,
    /// Bits needed to encode frame width (1-16).
    pub frame_width_bits: u8,
    /// Bits needed to encode frame height (1-16).
    pub frame_height_bits: u8,

    pub enable_superres: bool,
    pub enable_cdef: bool,
    pub enable_restoration: bool,

    pub frame_id_numbers_present_flag: bool,
    pub delta_frame_id_length: u8,
    pub additional_frame_id_length: u8,
    pub film_grain_params_present: bool,
    pub decoder_model_info_present_flag: bool,
    pub seq_force_screen_content_tools: u8,
    pub seq_force_integer_mv: u8,
    pub order_hint_bits: u8,
    pub enable_order_hint: bool,
    pub use_128x128_superblock: bool,

    pub enable_interintra_compound: bool,
    pub enable_masked_compound: bool,
    pub enable_warped_motion: bool,
    pub enable_dual_filter: bool,
    pub enable_jnt_comp: bool,
    pub enable_ref_frame_mvs: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ColorConfig {
    pub chroma_subsampling: ChromaSubsampling,
    pub chroma_sample_position: u8,
    pub separate_uv_delta_q: bool,
    pub color_range: u8,
    pub bit_depth: u8,
    pub monochrome: bool,

    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
}

fn color_config(b: &mut BitReader, seq_profile: u8) -> Result<ColorConfig> {
    let high_bitdepth = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    let bit_depth = if seq_profile == 2 && high_bitdepth {
        let twelve_bit = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        if twelve_bit {
            12
        } else {
            10
        }
    } else { // if seq_profile <= 2
        if high_bitdepth {
            10
        } else {
            8
        }
    };

    let monochrome = if seq_profile == 1 { false } else { b.read_bool().map_err(|e| at!(Error::from(e)))? };

    let num_planes = if monochrome { 1 } else { 3 };
    let color_description_present_flag = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    let mut color_primaries = 2;
    let mut transfer_characteristics = 2;
    let matrix_coefficients = if color_description_present_flag {
        color_primaries = b.read_u8(8).map_err(|e| at!(Error::from(e)))?;
        transfer_characteristics = b.read_u8(8).map_err(|e| at!(Error::from(e)))?;
        b.read_u8(8).map_err(|e| at!(Error::from(e)))?
    } else {
        2
    };

    let chroma_subsampling;
    let chroma_sample_position;
    let separate_uv_delta_q;
    let color_range;
    if monochrome {
        color_range = b.read_u8(1).map_err(|e| at!(Error::from(e)))?;
        chroma_subsampling = ChromaSubsampling::NONE;
        chroma_sample_position = 0;
        separate_uv_delta_q = false;
    } else if color_primaries == 1 //Bt709
        && transfer_characteristics == 13  // Srgb
        && matrix_coefficients == 0
    {
        color_range = 1;
        chroma_subsampling = ChromaSubsampling::NONE;
        chroma_sample_position = 0;
        separate_uv_delta_q = false;
    } else {
        color_range = b.read_u8(1).map_err(|e| at!(Error::from(e)))?;
        if seq_profile == 0 {
            chroma_subsampling = ChromaSubsampling::YUV420;
        } else if seq_profile == 1 {
            chroma_subsampling = ChromaSubsampling::NONE;
        } else if bit_depth == 12 {
            let x = b.read_bool().map_err(|e| at!(Error::from(e)))?;
            chroma_subsampling = if x {
                ChromaSubsampling { horizontal: x, vertical: b.read_bool().map_err(|e| at!(Error::from(e)))? }
            } else {
                ChromaSubsampling::NONE
            }
        } else {
            chroma_subsampling = ChromaSubsampling::YUV422;
        }
        chroma_sample_position = if chroma_subsampling.horizontal && chroma_subsampling.vertical { b.read_u8(2).map_err(|e| at!(Error::from(e)))? } else { 0 };
        separate_uv_delta_q = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    }

    Ok(ColorConfig {
        chroma_subsampling,
        chroma_sample_position,
        separate_uv_delta_q,
        color_range,
        bit_depth,
        monochrome,

        color_primaries,
        transfer_characteristics,
        matrix_coefficients,
    })
}

/// Read a delta-q value from the bitstream.
/// Returns 0 if the delta_coded flag is false, else reads su(7).
fn read_delta_q(b: &mut BitReader) -> Result<i8> {
    let delta_coded = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    if delta_coded {
        // su(7) — 7-bit signed value
        Ok(b.read_i8(7).map_err(|e| at!(Error::from(e)))?)
    } else {
        Ok(0)
    }
}

/// Parse the quantization_params section of an AV1 frame header.
///
/// Walks through the frame header fields that precede quantization_params,
/// then extracts base_q_idx and delta-q values. Returns `FrameQuantization`
/// with `coded_lossless` set when all quantization parameters are zero.
///
/// Reference: AV1 spec section 5.9 "Frame Header OBU Syntax"
fn parse_frame_header_quantization(data: &[u8], seq: &SequenceHeaderObu) -> Result<FrameQuantization> {
    let mut b = BitReader::new(data);
    let num_planes = if seq.color.monochrome { 1 } else { 3 };

    // uncompressed_header(): walk past everything preceding tile_info.
    read_uncompressed_header_until_tiles(&mut b, seq)?;

    // tile_info: skip past variable-length tile spacing.
    skip_tile_info(&mut b, seq)?;

    // quantization_params: extract base_q_idx + delta-q and detect lossless.
    read_quantization_params(&mut b, seq, num_planes)
}

/// State extracted from uncompressed_header() that downstream sections need.
#[derive(Debug, Clone, Copy)]
struct UncompressedHeaderState {
    frame_type: u8,
    allow_screen_content_tools: bool,
    error_resilient_mode: bool,
}

/// Walk through the uncompressed_header() fields that precede tile_info.
///
/// Bit-for-bit equivalent to the inline implementation: same conditional
/// reads in the same order, same errors on `show_existing_frame` / unsupported
/// frame types. For `reduced_still_picture_header` no bits are consumed
/// (matches the spec's implied defaults).
fn read_uncompressed_header_until_tiles(
    b: &mut BitReader,
    seq: &SequenceHeaderObu,
) -> Result<UncompressedHeaderState> {
    if seq.reduced_still_picture_header {
        return Ok(UncompressedHeaderState {
            frame_type: 0, // KEY_FRAME
            allow_screen_content_tools: false,
            error_resilient_mode: true,
        });
    }

    let show_existing_frame = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    if show_existing_frame {
        return Err(at!(Error::InvalidData("show_existing_frame")));
    }

    let frame_type = b.read_u8(2).map_err(|e| at!(Error::from(e)))?;
    let show_frame = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    if !show_frame {
        let _showable_frame = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    }

    let error_resilient_mode = if frame_type == 3 /* SWITCH_FRAME */ {
        true
    } else {
        b.read_bool().map_err(|e| at!(Error::from(e)))?
    };

    let _disable_cdf_update = b.read_bool().map_err(|e| at!(Error::from(e)))?;

    let allow_screen_content_tools = if seq.seq_force_screen_content_tools == SELECT_SCREEN_CONTENT_TOOLS {
        b.read_bool().map_err(|e| at!(Error::from(e)))?
    } else {
        seq.seq_force_screen_content_tools != 0
    };

    if allow_screen_content_tools && seq.seq_force_integer_mv == SELECT_INTEGER_MV {
        let _force_integer_mv = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    }

    if seq.frame_id_numbers_present_flag {
        let id_len = seq.delta_frame_id_length + seq.additional_frame_id_length;
        let _current_frame_id = b.read_u32(id_len).map_err(|e| at!(Error::from(e)))?;
    }

    let frame_size_override_flag = if frame_type == 3 /* SWITCH_FRAME */ {
        true
    } else {
        b.read_bool().map_err(|e| at!(Error::from(e)))?
    };

    if seq.enable_order_hint {
        let _order_hint = b.read_u32(seq.order_hint_bits).map_err(|e| at!(Error::from(e)))?;
    }

    // primary_ref_frame — only for non-intra, non-error-resilient
    if frame_type != 0 /* KEY_FRAME */
        && frame_type != 2 /* INTRA_ONLY */
        && !error_resilient_mode
    {
        let _primary_ref_frame = b.read_u8(3).map_err(|e| at!(Error::from(e)))?;
    }

    // decoder_model_info — already errored on in the sequence header read

    match frame_type {
        0 /* KEY_FRAME */ | 2 /* INTRA_ONLY */ => {
            read_intra_frame_geometry(
                b,
                seq,
                frame_size_override_flag,
                allow_screen_content_tools,
            )?;
        }
        _ => {
            // INTER or SWITCH — not expected for still AVIF, bail
            return Err(at!(Error::Unsupported("inter frame in probe")));
        }
    }

    Ok(UncompressedHeaderState {
        frame_type,
        allow_screen_content_tools,
        error_resilient_mode,
    })
}

/// Read the refresh_frame_flags / frame_size / superres / render_size /
/// allow_intrabc block shared by KEY_FRAME and INTRA_ONLY.
///
/// Each conditional read matches the spec ordering; the two branches were
/// already byte-identical in the inline version (the spec defines them
/// the same way for KEY_FRAME and INTRA_ONLY in this region).
fn read_intra_frame_geometry(
    b: &mut BitReader,
    seq: &SequenceHeaderObu,
    frame_size_override_flag: bool,
    allow_screen_content_tools: bool,
) -> Result<()> {
    let _refresh_frame_flags = b.read_u8(8).map_err(|e| at!(Error::from(e)))?;

    if frame_size_override_flag {
        let _frame_width = 1 + b.read_u32(seq.frame_width_bits).map_err(|e| at!(Error::from(e)))?;
        let _frame_height = 1 + b.read_u32(seq.frame_height_bits).map_err(|e| at!(Error::from(e)))?;
    }

    if seq.enable_superres {
        let use_superres = b.read_bool().map_err(|e| at!(Error::from(e)))?;
        if use_superres {
            let _coded_denom = b.read_u8(3).map_err(|e| at!(Error::from(e)))?;
        }
    }

    let render_and_frame_size_different = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    if render_and_frame_size_different {
        let _render_width = 1u32 + b.read_u16(16).map_err(|e| at!(Error::from(e)))? as u32;
        let _render_height = 1u32 + b.read_u16(16).map_err(|e| at!(Error::from(e)))? as u32;
    }

    if allow_screen_content_tools {
        let _allow_intrabc = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    }
    Ok(())
}

/// Skip past tile_info to reach quantization_params.
///
/// Handles both uniform and non-uniform tile spacing. tile_size_bytes
/// (only present when tile_cols * tile_rows > 1) is intentionally not
/// consumed here — for still AVIF probes the common case is a single
/// tile and adding multi-tile size handling would require reconstructing
/// the exact tile count, which the inline implementation already skipped.
fn skip_tile_info(b: &mut BitReader, seq: &SequenceHeaderObu) -> Result<()> {
    let sb_size = if seq.use_128x128_superblock { 128u32 } else { 64u32 };
    let sb_shift = if seq.use_128x128_superblock { 5 } else { 4 };
    let mi_cols = seq.max_frame_width.get().div_ceil(4);
    let mi_rows = seq.max_frame_height.get().div_ceil(4);
    let sb_cols = (mi_cols + (1 << sb_shift) - 1) >> sb_shift;
    let sb_rows = (mi_rows + (1 << sb_shift) - 1) >> sb_shift;

    let uniform_tile_spacing_flag = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    if uniform_tile_spacing_flag {
        skip_uniform_tile_spacing(b, sb_cols, sb_rows)?;
    } else {
        skip_non_uniform_tile_spacing(b, sb_cols, sb_rows, sb_size)?;
    }
    Ok(())
}

/// Uniform tile path: consume increment_tile_cols_log2 / rows_log2 bits.
fn skip_uniform_tile_spacing(b: &mut BitReader, sb_cols: u32, sb_rows: u32) -> Result<()> {
    let mut tile_cols_log2 = 0u32;
    let max_tile_cols_log2 = tile_log2(1, sb_cols);
    while tile_cols_log2 < max_tile_cols_log2 {
        if !b.read_bool().map_err(|e| at!(Error::from(e)))? {
            break;
        }
        tile_cols_log2 += 1;
    }
    let mut tile_rows_log2 = 0u32;
    let max_tile_rows_log2 = tile_log2(1, sb_rows);
    while tile_rows_log2 < max_tile_rows_log2 {
        if !b.read_bool().map_err(|e| at!(Error::from(e)))? {
            break;
        }
        tile_rows_log2 += 1;
    }
    Ok(())
}

/// Non-uniform tile path: read explicit width_in_sbs / height_in_sbs per tile.
fn skip_non_uniform_tile_spacing(
    b: &mut BitReader,
    sb_cols: u32,
    sb_rows: u32,
    sb_size: u32,
) -> Result<()> {
    let mut widest_tile_sb = 1u32;
    let mut start_sb = 0u32;
    while start_sb < sb_cols {
        let max_width = sb_cols - start_sb;
        let width_bits = tile_log2(1, max_width.min(MAX_TILE_WIDTH as u32 / sb_size));
        let width_in_sbs = 1 + b.read_u32(width_bits as u8).map_err(|e| at!(Error::from(e)))?;
        widest_tile_sb = widest_tile_sb.max(width_in_sbs);
        start_sb += width_in_sbs;
    }
    start_sb = 0;
    while start_sb < sb_rows {
        let max_height = sb_rows - start_sb;
        let max_tile_area_sb = MAX_TILE_AREA as u32 / (sb_size * sb_size);
        let max_tile_height = max_tile_area_sb.max(1) / widest_tile_sb.max(1);
        let height_bits = tile_log2(1, max_height.min(max_tile_height.max(1)));
        let height_in_sbs = 1 + b.read_u32(height_bits as u8).map_err(|e| at!(Error::from(e)))?;
        start_sb += height_in_sbs;
    }
    Ok(())
}

/// Read the quantization_params block: base_q_idx + delta-q values.
///
/// `coded_lossless` requires all components to be zero. Mirror exactly the
/// spec's UV-shared vs separate paths: when `separate_uv_delta_q` is false,
/// V values mirror U (not zero).
fn read_quantization_params(
    b: &mut BitReader,
    seq: &SequenceHeaderObu,
    num_planes: u8,
) -> Result<FrameQuantization> {
    let base_q_idx = b.read_u8(8).map_err(|e| at!(Error::from(e)))?;
    let delta_q_y_dc = read_delta_q(b)?;

    let mut delta_q_u_dc = 0i8;
    let mut delta_q_u_ac = 0i8;
    let mut delta_q_v_dc = 0i8;
    let mut delta_q_v_ac = 0i8;

    if num_planes > 1 {
        if seq.color.separate_uv_delta_q {
            delta_q_u_dc = read_delta_q(b)?;
            delta_q_u_ac = read_delta_q(b)?;
            delta_q_v_dc = read_delta_q(b)?;
            delta_q_v_ac = read_delta_q(b)?;
        } else {
            delta_q_u_dc = read_delta_q(b)?;
            delta_q_u_ac = read_delta_q(b)?;
            delta_q_v_dc = delta_q_u_dc;
            delta_q_v_ac = delta_q_u_ac;
        }
        let _using_qmatrix = b.read_bool().map_err(|e| at!(Error::from(e)))?;
    }

    let coded_lossless = base_q_idx == 0
        && delta_q_y_dc == 0
        && delta_q_u_dc == 0
        && delta_q_u_ac == 0
        && delta_q_v_dc == 0
        && delta_q_v_ac == 0;

    Ok(FrameQuantization {
        base_q_idx,
        coded_lossless,
    })
}

/// Compute floor(log2(n/d)) for tile size calculations.
fn tile_log2(d: u32, n: u32) -> u32 {
    if n == 0 || d == 0 {
        return 0;
    }
    let mut k = 0;
    while k < 31 {
        // Use checked_shl to prevent overflow: d << 32 wraps to 0 in
        // release builds, which would cause an infinite loop.
        let Some(shifted) = d.checked_shl(k + 1) else {
            break;
        };
        if shifted > n {
            break;
        }
        k += 1;
    }
    k
}

fn obu_header(data: &mut &[u8]) -> Result<Header> {
    let b = get_byte(data)?;
    if 0 != b & 0b1000_0000 {
        return Err(at!(Error::InvalidData("not obu")));
    }

    let obu_type = (b >> 3) & 0x0F;
    let obu_extension_flag = 0 != (b & 0b100);
    let obu_has_size_field = 0 != (b & 0b010);

    if obu_extension_flag {
        // obu_extension_header
        let _ext = get_byte(data)?;
    }

    let obu_size = if obu_has_size_field {
        leb128::read::unsigned(data)
            .map_err(|_| at!(Error::InvalidData("leb")))?
            .try_into()
            .map_err(|_| at!(Error::UnexpectedEOF))?
    } else {
        data.len()
    };

    Ok(Header { obu_size, obu_type })
}

const REFS_PER_FRAME: usize = 7; //   Number of reference frames that can be used for inter prediction
const TOTAL_REFS_PER_FRAME: usize = 8; //   Number of reference frame types (including intra type)
const BLOCK_SIZE_GROUPS: usize = 4; //   Number of contexts when decoding y_mode
const BLOCK_SIZES: usize = 22; //  Number of different block sizes used
const BLOCK_INVALID: usize = 22; //  Sentinel value to mark partition choices that are not allowed
const MAX_SB_SIZE: usize = 128; //     Maximum size of a superblock in luma samples
const MI_SIZE: usize = 4; //   Smallest size of a mode info block in luma samples
const MI_SIZE_LOG2: usize = 2; //   Base 2 logarithm of smallest size of a mode info block
const MAX_TILE_WIDTH: usize = 4096; //    Maximum width of a tile in units of luma samples
const MAX_TILE_AREA: usize = 4096; // * 2304     Maximum area of a tile in units of luma samples
const MAX_TILE_ROWS: usize = 64; //  Maximum number of tile rows
const MAX_TILE_COLS: usize = 64; //  Maximum number of tile columns
const INTRABC_DELAY_PIXELS: usize = 256; //     Number of horizontal luma samples before intra block copy can be used
const INTRABC_DELAY_SB64: usize = 4; //   Number of 64 by 64 blocks before intra block copy can be used
const NUM_REF_FRAMES: usize = 8; //   Number of frames that can be stored for future reference
const REF_CONTEXTS: usize = 3; //   Number of contexts for single_ref, comp_ref, comp_bwdref, uni_comp_ref, uni_comp_ref_p1 and uni_comp_ref_p2
const MAX_SEGMENTS: usize = 8; //   Number of segments allowed in segmentation map
const SEGMENT_ID_CONTEXTS: usize = 3; //   Number of contexts for segment_id
const SEG_LVL_ALT_Q: usize = 0; //   Index for quantizer segment feature
const SEG_LVL_ALT_LF_Y_V: usize = 1; //   Index for vertical luma loop filter segment feature
const SEG_LVL_REF_FRAME: usize = 5; //   Index for reference frame segment feature
const SEG_LVL_SKIP: usize = 6; //   Index for skip segment feature
const SEG_LVL_GLOBALMV: usize = 7; //   Index for global mv feature
const SEG_LVL_MAX: usize = 8; //   Number of segment features
const PLANE_TYPES: usize = 2; //   Number of different plane types (luma or chroma)
const TX_SIZE_CONTEXTS: usize = 3; //   Number of contexts for transform size
const INTERP_FILTERS: usize = 3; //   Number of values for interp_filter
const INTERP_FILTER_CONTEXTS: usize = 16; //  Number of contexts for interp_filter
const SKIP_MODE_CONTEXTS: usize = 3; //   Number of contexts for decoding skip_mode
const SKIP_CONTEXTS: usize = 3; //   Number of contexts for decoding skip
const PARTITION_CONTEXTS: usize = 4; //   Number of contexts when decoding partition
const TX_SIZES: usize = 5; //   Number of square transform sizes
const TX_SIZES_ALL: usize = 19; //  Number of transform sizes (including non-square sizes)
const TX_MODES: usize = 3; //   Number of values for tx_mode
const DCT_DCT: usize = 0; //   Inverse transform rows with DCT and columns with DCT
const ADST_DCT: usize = 1; //   Inverse transform rows with DCT and columns with ADST
const DCT_ADST: usize = 2; //   Inverse transform rows with ADST and columns with DCT
const ADST_ADST: usize = 3; //   Inverse transform rows with ADST and columns with ADST
const FLIPADST_DCT: usize = 4; //   Inverse transform rows with DCT and columns with FLIPADST
const DCT_FLIPADST: usize = 5; //   Inverse transform rows with FLIPADST and columns with DCT
const FLIPADST_FLIPADST: usize = 6; //   Inverse transform rows with FLIPADST and columns with FLIPADST
const ADST_FLIPADST: usize = 7; //   Inverse transform rows with FLIPADST and columns with ADST
const FLIPADST_ADST: usize = 8; //   Inverse transform rows with ADST and columns with FLIPADST
const IDTX: usize = 9; //   Inverse transform rows with identity and columns with identity
const V_DCT: usize = 10; //  Inverse transform rows with identity and columns with DCT
const H_DCT: usize = 11; //  Inverse transform rows with DCT and columns with identity
const V_ADST: usize = 12; //  Inverse transform rows with identity and columns with ADST
const H_ADST: usize = 13; //  Inverse transform rows with ADST and columns with identity
const V_FLIPADST: usize = 14; //  Inverse transform rows with identity and columns with FLIPADST
const H_FLIPADST: usize = 15; //  Inverse transform rows with FLIPADST and columns with identity
const TX_TYPES: usize = 16; //  Number of inverse transform types
const MB_MODE_COUNT: usize = 17; //  Number of values for YMode
const INTRA_MODES: usize = 13; //  Number of values for y_mode
const UV_INTRA_MODES_CFL_NOT_ALLOWED: usize = 13; //  Number of values for uv_mode when chroma from luma is not allowed
const UV_INTRA_MODES_CFL_ALLOWED: usize = 14; //  Number of values for uv_mode when chroma from luma is allowed
const COMPOUND_MODES: usize = 8; //   Number of values for compound_mode
const COMPOUND_MODE_CONTEXTS: usize = 8; //   Number of contexts for compound_mode
const COMP_NEWMV_CTXS: usize = 5; //   Number of new mv values used when constructing context for compound_mode
const NEW_MV_CONTEXTS: usize = 6; //   Number of contexts for new_mv
const ZERO_MV_CONTEXTS: usize = 2; //   Number of contexts for zero_mv
const REF_MV_CONTEXTS: usize = 6; //   Number of contexts for ref_mv
const DRL_MODE_CONTEXTS: usize = 3; //   Number of contexts for drl_mode
const MV_CONTEXTS: usize = 2; //   Number of contexts for decoding motion vectors including one for intra block copy
const MV_INTRABC_CONTEXT: usize = 1; //   Motion vector context used for intra block copy
const MV_JOINTS: usize = 4; //   Number of values for mv_joint
const MV_CLASSES: usize = 11; //  Number of values for mv_class
const CLASS0_SIZE: usize = 2; //   Number of values for mv_class0_bit
const MV_OFFSET_BITS: usize = 10; //  Maximum number of bits for decoding motion vectors
const MAX_LOOP_FILTER: usize = 63; //  Maximum value used for loop filtering
const REF_SCALE_SHIFT: usize = 14; //  Number of bits of precision when scaling reference frames
const SUBPEL_BITS: usize = 4; //   Number of bits of precision when choosing an inter prediction filter kernel
const SUBPEL_MASK: usize = 15; //  ( 1 << SUBPEL_BITS ) - 1
const SCALE_SUBPEL_BITS: usize = 10; //  Number of bits of precision when computing inter prediction locations
const MV_BORDER: usize = 128; //     Value used when clipping motion vectors
const PALETTE_COLOR_CONTEXTS: usize = 5; //   Number of values for color contexts
const PALETTE_MAX_COLOR_CONTEXT_HASH: usize = 8; //   Number of mappings between color context hash and color context
const PALETTE_BLOCK_SIZE_CONTEXTS: usize = 7; //   Number of values for palette block size
const PALETTE_Y_MODE_CONTEXTS: usize = 3; //   Number of values for palette Y plane mode contexts
const PALETTE_UV_MODE_CONTEXTS: usize = 2; //   Number of values for palette U and V plane mode contexts
const PALETTE_SIZES: usize = 7; //   Number of values for palette_size
const PALETTE_COLORS: usize = 8; //   Number of values for palette_color
const PALETTE_NUM_NEIGHBORS: usize = 3; //   Number of neighbors considered within palette computation
const DELTA_Q_SMALL: usize = 3; //   Value indicating alternative encoding of quantizer index delta values
const DELTA_LF_SMALL: usize = 3; //   Value indicating alternative encoding of loop filter delta values
const QM_TOTAL_SIZE: usize = 3344; //    Number of values in the quantizer matrix
const MAX_ANGLE_DELTA: usize = 3; //   Maximum magnitude of AngleDeltaY and AngleDeltaUV
const DIRECTIONAL_MODES: usize = 8; //   Number of directional intra modes
const ANGLE_STEP: usize = 3; //   Number of degrees of step per unit increase in AngleDeltaY or AngleDeltaUV.
const TX_SET_TYPES_INTRA: usize = 3; //   Number of intra transform set types
const TX_SET_TYPES_INTER: usize = 4; //   Number of inter transform set types
const WARPEDMODEL_PREC_BITS: usize = 16; //  Internal precision of warped motion models
const IDENTITY: usize = 0; //   Warp model is just an identity transform
const TRANSLATION: usize = 1; //   Warp model is a pure translation
const ROTZOOM: usize = 2; //   Warp model is a rotation + symmetric zoom + translation
const AFFINE: usize = 3; //   Warp model is a general affine transform
const GM_ABS_TRANS_BITS: usize = 12; //  Number of bits encoded for translational components of global motion models, if part of a ROTZOOM or AFFINE model
const GM_ABS_TRANS_ONLY_BITS: usize = 9; //   Number of bits encoded for translational components of global motion models, if part of a TRANSLATION model
const GM_ABS_ALPHA_BITS: usize = 12; //  Number of bits encoded for non-translational components of global motion models
const DIV_LUT_PREC_BITS: usize = 14; //  Number of fractional bits of entries in divisor lookup table
const DIV_LUT_BITS: usize = 8; //   Number of fractional bits for lookup in divisor lookup table
const DIV_LUT_NUM: usize = 257; //     Number of entries in divisor lookup table
const MOTION_MODES: usize = 3; //   Number of values for motion modes
const SIMPLE: usize = 0; //   Use translation or global motion compensation
const OBMC: usize = 1; //   Use overlapped block motion compensation
const LOCALWARP: usize = 2; //   Use local warp motion compensation
const LEAST_SQUARES_SAMPLES_MAX: usize = 8; //   Largest number of samples used when computing a local warp
const LS_MV_MAX: usize = 256; //     Largest motion vector difference to include in local warp computation
const WARPEDMODEL_TRANS_CLAMP: usize = 1; //<<23   Clamping value used for translation components of warp
const WARPEDMODEL_NONDIAGAFFINE_CLAMP: usize = 1; //<<13   Clamping value used for matrix components of warp
const WARPEDPIXEL_PREC_SHIFTS: usize = 1; //<<6    Number of phases used in warped filtering
const WARPEDDIFF_PREC_BITS: usize = 10; //  Number of extra bits of precision in warped filtering
const GM_ALPHA_PREC_BITS: usize = 15; //  Number of fractional bits for sending non-translational warp model coefficients
const GM_TRANS_PREC_BITS: usize = 6; //   Number of fractional bits for sending translational warp model coefficients
const GM_TRANS_ONLY_PREC_BITS: usize = 3; //   Number of fractional bits used for pure translational warps
const INTERINTRA_MODES: usize = 4; //   Number of inter intra modes
const MASK_MASTER_SIZE: usize = 64; //  Size of MasterMask array
const SEGMENT_ID_PREDICTED_CONTEXTS: usize = 3; //   Number of contexts for segment_id_predicted
const IS_INTER_CONTEXTS: usize = 4; //   Number of contexts for is_inter
const FWD_REFS: usize = 4; //   Number of syntax elements for forward reference frames
const BWD_REFS: usize = 3; //   Number of syntax elements for backward reference frames
const SINGLE_REFS: usize = 7; //   Number of syntax elements for single reference frames
const UNIDIR_COMP_REFS: usize = 4; //   Number of syntax elements for unidirectional compound reference frames
const COMPOUND_TYPES: usize = 2; //   Number of values for compound_type
const CFL_JOINT_SIGNS: usize = 8; //   Number of values for cfl_alpha_signs
const CFL_ALPHABET_SIZE: usize = 16; //  Number of values for cfl_alpha_u and cfl_alpha_v
const COMP_INTER_CONTEXTS: usize = 5; //   Number of contexts for comp_mode
const COMP_REF_TYPE_CONTEXTS: usize = 5; //   Number of contexts for comp_ref_type
const CFL_ALPHA_CONTEXTS: usize = 6; //   Number of contexts for cfl_alpha_u and cfl_alpha_v
const INTRA_MODE_CONTEXTS: usize = 5; //   Number of each of left and above contexts for intra_frame_y_mode
const COMP_GROUP_IDX_CONTEXTS: usize = 6; //   Number of contexts for comp_group_idx
const COMPOUND_IDX_CONTEXTS: usize = 6; //   Number of contexts for compound_idx
const INTRA_EDGE_KERNELS: usize = 3; //   Number of filter kernels for the intra edge filter
const INTRA_EDGE_TAPS: usize = 5; //   Number of kernel taps for the intra edge filter
const FRAME_LF_COUNT: usize = 4; //   Number of loop filter strength values
const MAX_VARTX_DEPTH: usize = 2; //   Maximum depth for variable transform trees
const TXFM_PARTITION_CONTEXTS: usize = 21; //  Number of contexts for txfm_split
const REF_CAT_LEVEL: usize = 640; //     Bonus weight for close motion vectors
const MAX_REF_MV_STACK_SIZE: usize = 8; //   Maximum number of motion vectors in the stack
const MFMV_STACK_SIZE: usize = 3; //   Stack size for motion field motion vectors
const MAX_TX_DEPTH: usize = 2; //   Maximum times the transform can be split
const WEDGE_TYPES: usize = 16; //  Number of directions for the wedge mask process
const FILTER_BITS: usize = 7; //   Number of bits used in Wiener filter coefficients
const WIENER_COEFFS: usize = 3; //   Number of Wiener filter coefficients to read
const SGRPROJ_PARAMS_BITS: usize = 4; //   Number of bits needed to specify self guided filter set
const SGRPROJ_PRJ_SUBEXP_K: usize = 4; //   Controls how self guided deltas are read
const SGRPROJ_PRJ_BITS: usize = 7; //   Precision bits during self guided restoration
const SGRPROJ_RST_BITS: usize = 4; //   Restoration precision bits generated higher than source before projection
const SGRPROJ_MTABLE_BITS: usize = 20; //  Precision of mtable division table
const SGRPROJ_RECIP_BITS: usize = 12; //  Precision of division by n table
const SGRPROJ_SGR_BITS: usize = 8; //   Internal precision bits for core selfguided_restoration
const EC_PROB_SHIFT: usize = 6; //   Number of bits to reduce CDF precision during arithmetic coding
const EC_MIN_PROB: usize = 4; //   Minimum probability assigned to each symbol during arithmetic coding
const SELECT_SCREEN_CONTENT_TOOLS: u8 = 2; //   Value that indicates the allow_screen_content_tools syntax element is coded
const SELECT_INTEGER_MV: u8 = 2; //   Value that indicates the force_integer_mv syntax element is coded
const RESTORATION_TILESIZE_MAX: usize = 256; //     Maximum size of a loop restoration tile
const MAX_FRAME_DISTANCE: usize = 31; //  Maximum distance when computing weighted prediction
const MAX_OFFSET_WIDTH: usize = 8; //   Maximum horizontal offset of a projected motion vector
const MAX_OFFSET_HEIGHT: usize = 0; //   Maximum vertical offset of a projected motion vector
const WARP_PARAM_REDUCE_BITS: usize = 6; //   Rounding bitwidth for the parameters to the shear process
const NUM_BASE_LEVELS: usize = 2; //   Number of quantizer base levels
const COEFF_BASE_RANGE: usize = 12; //  The quantizer range above NUM_BASE_LEVELS above which the Exp-Golomb coding process is activated
const BR_CDF_SIZE: usize = 4; //   Number of values for coeff_br
const SIG_COEF_CONTEXTS_EOB: usize = 4; //   Number of contexts for coeff_base_eob
const SIG_COEF_CONTEXTS_2D: usize = 26; //  Context offset for coeff_base for horizontal-only or vertical-only transforms.
const SIG_COEF_CONTEXTS: usize = 42; //  Number of contexts for coeff_base
const SIG_REF_DIFF_OFFSET_NUM: usize = 5; //   Maximum number of context samples to be used in determining the context index for coeff_base and coeff_base_eob.
const SUPERRES_NUM: usize = 8; //   Numerator for upscaling ratio
const SUPERRES_DENOM_MIN: usize = 9; //   Smallest denominator for upscaling ratio
const SUPERRES_DENOM_BITS: usize = 3; //   Number of bits sent to specify denominator of upscaling ratio
const SUPERRES_FILTER_BITS: usize = 6; //   Number of bits of fractional precision for upscaling filter selection
const SUPERRES_FILTER_SHIFTS: usize = 1; // << SUPERRES_FILTER_BITS   Number of phases of upscaling filters
const SUPERRES_FILTER_TAPS: usize = 8; //   Number of taps of upscaling filters
const SUPERRES_FILTER_OFFSET: usize = 3; //   Sample offset for upscaling filters
const SUPERRES_SCALE_BITS: usize = 14; //  Number of fractional bits for computing position in upscaling
const SUPERRES_SCALE_MASK: usize = (1 << 14) - 1; // Mask for computing position in upscaling
const SUPERRES_EXTRA_BITS: usize = 8; //   Difference in precision between SUPERRES_SCALE_BITS and SUPERRES_FILTER_BITS
const TXB_SKIP_CONTEXTS: usize = 13; //  Number of contexts for all_zero
const EOB_COEF_CONTEXTS: usize = 9; //   Number of contexts for eob_extra
const DC_SIGN_CONTEXTS: usize = 3; //   Number of contexts for dc_sign
const LEVEL_CONTEXTS: usize = 21; //  Number of contexts for coeff_br
const TX_CLASS_2D: usize = 0; //   Transform class for transform types performing non-identity transforms in both directions
const TX_CLASS_HORIZ: usize = 1; //   Transform class for transforms performing only a horizontal non-identity transform
const TX_CLASS_VERT: usize = 2; //   Transform class for transforms performing only a vertical non-identity transform
const REFMVS_LIMIT: usize = (1 << 12) - 1; //      Largest reference MV component that can be saved
const INTRA_FILTER_SCALE_BITS: usize = 4; //   Scaling shift for intra filtering process
const INTRA_FILTER_MODES: usize = 5; //   Number of types of intra filtering
const COEFF_CDF_Q_CTXS: usize = 4; //   Number of selectable context types for the coeff( ) syntax structure
const PRIMARY_REF_NONE: usize = 7; //   Value of primary_ref_frame indicating that there is no primary reference frame
const BUFFER_POOL_MAX_SIZE: usize = 10; //  Number of frames in buffer pool

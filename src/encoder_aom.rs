//! zenav1-aom AVIF encode backend (`zenav1-aom-encode` feature, EXPERIMENTAL).
//!
//! Routes [`crate::encoder::encode_rgb8`] and [`crate::encoder::encode_gray8`]
//! through the pure-Rust libaom port
//! ([imazen/zenav1-aom](https://github.com/imazen/zenav1-aom), the
//! `crates/aom-encode` crate) when [`crate::Av1Backend::Zenav1Aom`] is
//! selected. Like the [`crate::encoder_svt_rs`] backend — and unlike the
//! zenravif backend, where zenravif itself muxes — this seam gets raw AV1 OBUs
//! back and muxes the AVIF container in-crate via `zenavif-serialize`.
//!
//! # Scope: KEY FRAME / STILL ONLY
//!
//! `aom_encode::key_frame::encode_key_frame` encodes **one** AV1 KEY frame and
//! returns one temporal unit. There is no inter prediction, no reference
//! management and no multi-frame state in that entry point, so this backend
//! implements stills and **refuses animation by name** — see
//! [`reject_aom_backend`](crate::encoder::reject_aom_backend). Nothing here
//! falls back to zenravif: the `backend` field is a contract, not a hint.
//!
//! Within stills, this seam is deliberately narrower than the encoder it
//! drives. Wired:
//!
//! * 8-bit RGB -> YCbCr **4:2:0**, BT.601, **limited (studio) range** — see
//!   "Colour signalling" below for why the range is not full.
//! * 8-bit grayscale -> true monochrome (Cs400).
//!
//! Refused by name, each with the reason (`reject_unsupported_config`,
//! [`reject_out_of_envelope_depth`]):
//!
//! * 4:4:4 and the identity/RGB colour model — the *encoder* gates 4:2:0,
//!   4:2:2 and 4:4:4 (186/186 cells), but this seam has no forward RGB->YUV
//!   4:4:4 kernel: `src/yuv_convert.rs` ships `rgb8_to_yuv420` and no 4:4:4
//!   counterpart. Same refusal, and the same reason, as the zenav1-svt seam.
//! * 10- and 12-bit, and the 16-bit entry points — again an encoder capability
//!   (bd 8/10/12 are all byte-gated) this seam has not wired: it would need
//!   the u16 forward path plus profile-2 `av1C` signalling.
//! * Alpha (`encode_rgba8` / `encode_rgba16`) — the auxiliary Cs400 item is
//!   not wired yet; the mono encode it needs already exists here.
//! * Full pixel range, gain maps, animation.
//!
//! # Colour signalling (read this before comparing against the zenav1-svt seam)
//!
//! The port's `derive_sequence_header` pins the AV1 `color_config` to real
//! aomenc's defaults: CICP 2/2/2 (`color_description_present_flag = 0`) and
//! **`color_range = 0`, i.e. STUDIO/LIMITED range** (`AOM_CR_STUDIO_RANGE`;
//! `crates/aom-encode/src/key_frame.rs`, the `ColorConfigParams` literal).
//! That is the opposite of the zenav1-svt seam, whose sequence header pins
//! `color_range = 1`.
//!
//! So this backend converts with [`crate::yuv_convert::YuvRange::Limited`] and
//! muxes `colr` with `full_color_range = false`. Converting full-range samples
//! into a stream that declares limited range would decode to stretched
//! contrast, so the two have to agree.
//!
//! **Which of the two a decoder actually obeys, MEASURED (2026-09-02):**
//! `zenavif::decode` takes both the range and the matrix from the **AV1
//! sequence header**, not from the container `colr` — `src/decoder.rs`
//! reads `seq_hdr.color_range` / `seq_hdr.mtrx`, and flipping the `colr`
//! nclx `full_range` bit in an encoded file changes the decoded pixels not at
//! all (PSNR identical to six decimals). The `colr` value is therefore
//! written to agree, not to be the source of truth. The bitstream's
//! `matrix_coefficients` is 2 (unspecified), and `to_yuv_matrix`'s fallback
//! for that is BT.601 — which is what this seam converts with, so they agree
//! there too.
//!
//! The load-bearing gate is `tests/aom_encode_backend.rs`'s flat-content
//! round trip: a flat 235 source codes to luma 218 under the studio swing, so
//! a decoder that read the range wrongly would return 218 instead of 235. It
//! returns 235 exactly.
//!
//! The `colr` box still carries the colorimetry the bitstream declines to:
//! BT.709 primaries + sRGB transfer by default (override with
//! [`crate::EncoderConfig::color_primaries`] /
//! [`crate::EncoderConfig::transfer_characteristics`]) and BT.601 matrix.
//!
//! # Quality / speed mapping
//!
//! aomenc's own dials, not zenravif's fitted quality->quantizer curve:
//!
//! * quality 1..=100 -> `--cq-level` 63..=0, linear ([`quality_to_cq_level`]).
//!   Both ends are byte-gated upstream (`cq0` and `cq63` are sweep cells), so
//!   unlike the zenav1-svt seam there is no clamp away from the endpoint.
//! * speed 1..=10 -> `--cpu-used` 0..=9, linear ([`speed_to_cpu_used`]).
//!   The whole range is byte-gated.
//! * `--enable-cdef=0`, `--enable-restoration=1`: real aomenc's ALLINTRA
//!   defaults (`av1_cx_iface.c:3067`). All four combinations are gated
//!   upstream; this seam takes the defaults and does not expose the knobs.
//!
//! # Upstream parity, and where it stops
//!
//! At the pinned rev `encode_key_frame` is **186/186 cells byte-identical to
//! real aomenc** — mono / 4:2:0 / 4:2:2 / 4:4:4, bit depths 8/10/12,
//! 16x16..512x512, 20 crops including 1x1, all four CDEF x loop-restoration
//! combinations, `--cpu-used` 0..=9, and multi-tile up to four tiles — and its
//! streams decode to the same pixels under both real libaom and the in-repo
//! decoder (`crates/aom-encode/tests/self_contained_key_frame.rs`; re-run
//! 2026-09-02 here: 6/6 tests, 186/186 byte-exact, 20 decode cells).
//!
//! Three regions are pinned-divergent upstream rather than refused — the
//! streams are valid and decode, they just are not aomenc's bytes:
//! `--cpu-used >= 7` above roughly 3x3 superblocks, `--enable-cdef=1` at
//! `--cpu-used >= 4` (which this seam never selects), and a two-tile
//! 4160x64 cell at speeds >= 7. Byte-identity with aomenc is not something
//! this backend promises to zenavif callers in the first place; it is cited
//! because it is the evidence that the derived headers are right.
//!
//! # A third-party reader accepts the output (measured 2026-09-02)
//!
//! `tests/aom_encode_backend.rs` decodes with rav1d-safe, which is
//! independent of the aom port but still in-tree. Outside it entirely: a
//! 192x128 gradient encoded at quality 88 / speed 5 through this backend is
//! reported by `file(1)` as "ISO Media, AVIF Image", and macOS `sips` —
//! Apple's own AVIF decoder, sharing no code with this workspace — reads it
//! as 192x128 and transcodes it to PNG whose pixels match the source to
//! **mean 0.57 / max 3** per channel over 4608 sampled values.
//!
//! That number also confirms the range signalling from outside: the
//! top-left source pixel is (0, 0, 0), it codes to studio luma 16, and
//! Apple's decoder returns (1, 1, 1) — not (16, 16, 16), which is what a
//! decoder ignoring `color_range = 0` would give.
//!
//! # Encode speed (MEASURED 2026-09-02 — `benchmarks/aom_backend_2026-09-02.*`)
//!
//! Against the zenravif backend on the same buffer, 4:2:0 8-bit, medians over
//! six qualities (Apple M4 Pro; harness `examples/aom_backend_bench.rs`):
//!
//! | size | aom s1 | zenravif s1 | aom s5 | zenravif s5 | aom s9 | zenravif s9 |
//! |---|---|---|---|---|---|---|
//! | 64² | 84 ms | 284 ms | 11 ms | 3.5 ms | 0.6 ms | 2.2 ms |
//! | 256² | 681 ms | 2425 ms | 102 ms | 36 ms | 3.9 ms | 29 ms |
//! | 512² | 2301 ms | 5847 ms | 256 ms | 119 ms | 13 ms | 109 ms |
//! | 1024² | 7855 ms | 24047 ms | 897 ms | 432 ms | 49 ms | 393 ms |
//!
//! **The speed ladders are misaligned** — the same shape of finding the
//! zenav1-svt seam records. aom is 2.5–3.2× faster at speed 1 and 3.9–8.0×
//! faster at speed 9, but 2.0–3.2× *slower* at speed 5. zenavif speed N does
//! not mean comparable work across backends.
//!
//! Per-pixel cost is **not** constant (ms/MP falls 7–25× from 64² to 1024²
//! for both backends), so no single ms/MP number is quoted and the
//! `alpha + beta·MP` fit is deliberately omitted — it is badly conditioned on
//! that grid. See the `.meta` for the per-size table and for why the
//! byte/quality columns are NOT an RD comparison.

use crate::Result;
use crate::encoder::{EncodeChromaSubsampling, EncodeColorModel, EncodePixelRange};
use crate::encoder::{EncodedImage, EncoderConfig};
use crate::error::Error;
use imgref::ImgRef;
use rgb::Rgb;
use whereat::at;

/// CICP defaults when the config sets none — the same defaults the zenravif
/// and zenav1-svt backends use (BT.709 primaries + sRGB transfer).
const DEFAULT_COLOR_PRIMARIES: u8 = 1;
/// See [`DEFAULT_COLOR_PRIMARIES`].
const DEFAULT_TRANSFER_CHARACTERISTICS: u8 = 13;

/// `AOM_USAGE_ALL_INTRA` — the only usage `encode_key_frame` gates.
const AOM_USAGE_ALL_INTRA: u32 = 2;

/// Map zenavif quality 1..=100 to aomenc `--cq-level` 63..=0.
///
/// Linear and endpoint-exact: quality 100 -> cq 0, quality 1 -> cq 63. Both
/// endpoints are byte-gated upstream (`cq0` / `cq63` sweep cells), so there is
/// no clamp away from either end (contrast [`crate::encoder_svt_rs::
/// quality_to_qp_gated`], which must avoid QP 0).
pub(crate) fn quality_to_cq_level(quality: f32) -> i32 {
    let q = quality.clamp(1.0, 100.0);
    // round((100 - q) * 63 / 99), so q=100 -> 0 and q=1 -> 63.
    let cq = ((100.0 - q) * 63.0 / 99.0).round() as i32;
    cq.clamp(0, 63)
}

/// Map zenavif speed 1..=10 to aomenc `--cpu-used` 0..=9.
///
/// One-to-one: speed 1 (slowest/best) -> `--cpu-used 0`, speed 10 -> 9. The
/// whole range is byte-gated upstream, so unlike the zenav1-svt preset mapping
/// there is no remap or clamp hiding a distinction the encoder does not have.
pub(crate) fn speed_to_cpu_used(speed: u8) -> i32 {
    i32::from(speed.clamp(1, 10)) - 1
}

/// The RESOLVED encoder state an aom cell actually encodes with:
/// `(cpu_used, cq_level)`.
///
/// The aom half of the sweep planner's byte-identity contract
/// ([`crate::sweep::fingerprint`]). The planner's default mediators are
/// zenravif's (`quality_to_quantizer` + the `speed_derived` search table) and
/// this backend reads NEITHER — it resolves quality through
/// [`quality_to_cq_level`] and speed through [`speed_to_cpu_used`]. Hashing an
/// aom config with zenravif's mediators would both miss real aliases and risk
/// merging cells that differ, so the fingerprint routes here instead.
///
/// Both mappings are injective over their input ranges (speed 1..=10 is
/// one-to-one onto `--cpu-used` 0..=9; quality 1..=100 maps onto cq 63..=0 by
/// rounding, so adjacent qualities can alias — `(100 - q) * 63 / 99` steps by
/// 0.636 per quality point). Those aliases are exactly what a resolved-state
/// fingerprint is for.
#[cfg(feature = "__expert")]
pub(crate) fn aom_resolved_identity(config: &EncoderConfig) -> (u8, u8) {
    (
        speed_to_cpu_used(config.speed) as u8,
        quality_to_cq_level(config.quality) as u8,
    )
}

/// The configuration slice this seam implements. Everything else is refused by
/// name rather than silently served by zenravif or silently mis-encoded.
fn reject_unsupported_config(config: &EncoderConfig) -> Result<()> {
    if config.chroma_subsampling != EncodeChromaSubsampling::Yuv420 {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Aom encodes 4:2:0 only: set \
             .chroma_subsampling(EncodeChromaSubsampling::Yuv420). \
             The encoder itself gates 4:2:0, 4:2:2 and 4:4:4; this seam has no \
             forward RGB->YUV 4:4:4 kernel (src/yuv_convert.rs ships \
             rgb8_to_yuv420 and no 4:4:4 counterpart)"
        )));
    }
    if config.color_model != EncodeColorModel::YCbCr {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Aom supports the YCbCr color model only \
             (identity/RGB has no defined 4:2:0 subsampling)"
        )));
    }
    if config.pixel_range == Some(EncodePixelRange::Full) {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Aom signals LIMITED pixel range only \
             (the zenav1-aom sequence header pins color_range=0, \
             AOM_CR_STUDIO_RANGE); requesting full range would mis-signal the \
             stream"
        )));
    }
    if config.gain_map.is_some() {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Aom does not support gain maps \
             (use the zenravif backend)"
        )));
    }
    #[cfg(feature = "encode-imazen")]
    if config.lossless {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Aom has no lossless mode wired \
             (use the zenravif backend for lossless)"
        )));
    }
    Ok(())
}

/// The bit-depth envelope this seam accepts, shared by the encode path and
/// `EncoderConfig::validate` -- so a config that validates encodes and a
/// config that encodes validates. Returns the reason a `bit_depth`-bit encode
/// is refused, or `None` when it encodes.
///
/// 8, 10 and 12 all encode on the **colour** 4:2:0 path (all three are
/// byte-gated upstream). The grayscale (Cs400) path is 8-bit only -- see the
/// message for why that is a missing value-scaling rule, not a missing
/// encoder.
pub(crate) fn aom_depth_error(bit_depth: u8, monochrome: bool) -> Option<&'static str> {
    if !matches!(bit_depth, 8 | 10 | 12) {
        return Some(
            "Av1Backend::Zenav1Aom codes 8, 10 or 12 bits (aom_encode::key_frame gates \
             exactly those three); use EncodeBitDepth::Eight / ::Ten or \
             EncoderConfig::bit_depth_bits(12)",
        );
    }
    if monochrome && bit_depth != 8 {
        return Some(
            "Av1Backend::Zenav1Aom codes 8-bit grayscale (Cs400) only: encode_gray8 takes \
             u8 samples and this seam passes them through as the coded luma, so promoting \
             them to a 10- or 12-bit swing would need a value-scaling rule nothing here \
             measures. The 4:2:0 colour path codes 8, 10 and 12 -- use RGB input",
        );
    }
    None
}

/// Encode-time twin of `validate_aom_scope`'s depth check, both driven by
/// [`aom_depth_error`]. Returns the depth to code at.
///
/// The depth comes from the one shared resolver,
/// [`EncoderConfig::coded_bit_depth_bits`], so [`crate::EncodeBitDepth::Auto`],
/// [`crate::EncodeBitDepth::Ten`] and [`EncoderConfig::bit_depth_bits`] all land
/// here.
pub(crate) fn resolve_aom_depth(
    config: &EncoderConfig,
    input_is_16bit: bool,
    monochrome: bool,
) -> Result<u8> {
    let bit_depth = config.coded_bit_depth_bits(input_is_16bit);
    match aom_depth_error(bit_depth, monochrome) {
        None => Ok(bit_depth),
        Some(reason) => Err(at!(Error::Unsupported(reason))),
    }
}

/// Map a raw CICP colour-primaries code point to the muxer's enum. Unmapped
/// code points degrade to `Unspecified`, exactly as the zenav1-svt seam does.
fn cicp_to_serialize_primaries(cp: u8) -> zenavif_serialize::constants::ColorPrimaries {
    use zenavif_serialize::constants::ColorPrimaries as CP;
    match cp {
        1 => CP::Bt709,
        6 => CP::Bt601,
        9 => CP::Bt2020,
        11 => CP::DciP3,
        12 => CP::DisplayP3,
        _ => CP::Unspecified,
    }
}

/// See [`cicp_to_serialize_primaries`].
fn cicp_to_serialize_transfer(tc: u8) -> zenavif_serialize::constants::TransferCharacteristics {
    use zenavif_serialize::constants::TransferCharacteristics as TC;
    match tc {
        1 => TC::Bt709,
        6 => TC::Bt601,
        8 => TC::Linear,
        13 => TC::Srgb,
        14 => TC::Bt2020_10,
        16 => TC::Smpte2084,
        18 => TC::Hlg,
        _ => TC::Unspecified,
    }
}

/// Build the `aom_encode` config for one still.
fn key_frame_config(
    config: &EncoderConfig,
    width: usize,
    height: usize,
    bit_depth: u8,
    monochrome: bool,
) -> aom_encode::key_frame::KeyFrameConfig {
    aom_encode::key_frame::KeyFrameConfig {
        width,
        height,
        bit_depth,
        monochrome,
        // Monochrome carries (1, 1) — the AOM_IMG_FMT_I420 a mono image
        // allocates; `encode_key_frame` rejects any other ss for mono.
        ss_x: 1,
        ss_y: 1,
        cq_level: quality_to_cq_level(config.quality),
        cpu_used: speed_to_cpu_used(config.speed),
        usage: AOM_USAGE_ALL_INTRA,
        // Real aomenc's ALLINTRA defaults (`av1_cx_iface.c:3067`): CDEF off
        // ("CDEF has been found to blur images"), loop restoration on. Both
        // are byte-gated upstream at every speed in this combination.
        enable_cdef: false,
        enable_restoration: true,
    }
}

/// Run one `encode_key_frame` and translate its refusals into `Error`.
fn encode_key_frame_checked(
    planes: aom_encode::key_frame::KeyFramePlanes<'_>,
    cfg: &aom_encode::key_frame::KeyFrameConfig,
) -> Result<Vec<u8>> {
    aom_encode::key_frame::encode_key_frame(planes, cfg)
        .map_err(|e| at!(Error::Encode(format!("zenav1-aom key-frame encode: {e}"))))
}

/// Mux a colour (or monochrome) payload into an AVIF file with the config's
/// container-level metadata.
#[expect(clippy::too_many_arguments, reason = "internal mux helper")]
fn mux_aom(
    config: &EncoderConfig,
    payload: Vec<u8>,
    width: usize,
    height: usize,
    seq_profile: u8,
    bit_depth: u8,
    color_primaries: u8,
    transfer_characteristics: u8,
    monochrome: bool,
) -> Result<EncodedImage> {
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;
    let mut aviffy = zenavif_serialize::Aviffy::new();
    aviffy
        .set_seq_profile(seq_profile)
        .set_chroma_subsampling((true, true))
        .set_monochrome(monochrome)
        // MUST match the sequence header's `color_range = 0`. See the module
        // docs: the port pins AOM_CR_STUDIO_RANGE.
        .set_full_color_range(false)
        .set_color_primaries(cicp_to_serialize_primaries(color_primaries))
        .set_transfer_characteristics(cicp_to_serialize_transfer(transfer_characteristics))
        .set_matrix_coefficients(if monochrome {
            zenavif_serialize::constants::MatrixCoefficients::Unspecified
        } else {
            zenavif_serialize::constants::MatrixCoefficients::Bt601
        });
    if let Some(ref exif) = config.exif {
        aviffy.set_exif(exif.clone());
    }
    if let Some(ref xmp) = config.xmp {
        aviffy.set_xmp(xmp.clone());
    }
    if let Some(ref icc) = config.icc_profile {
        aviffy.set_icc_profile(icc.clone());
    }
    if let Some(angle) = config.rotation {
        aviffy.set_rotation(angle);
    }
    if let Some(axis) = config.mirror {
        aviffy.set_mirror(axis);
    }
    if let Some((max_cll, max_fall)) = config.content_light_level {
        aviffy.set_content_light_level(max_cll, max_fall);
    }
    if let Some(md) = config.mastering_display {
        aviffy.set_mastering_display(
            md.primaries,
            md.white_point,
            md.max_luminance,
            md.min_luminance,
        );
    }
    let color_byte_size = payload.len();
    // `bit_depth`, not a literal 8: `zenavif-serialize` derives the `av1C`
    // `high_bitdepth` / `twelve_bit` flags and the `pixi` depth from this, and
    // raises `seq_profile` to 2 at 12 bits. A 10-bit payload muxed as 8 would
    // be a container that contradicts its own bitstream.
    let avif_file = aviffy
        .try_to_vec(&payload, None, w, h, bit_depth)
        .map_err(|e| at!(Error::Encode(format!("AVIF serialization failed: {e}"))))?;
    Ok(EncodedImage {
        avif_file,
        color_byte_size,
        alpha_byte_size: 0,
    })
}

/// The colour input the 4:2:0 conversion reads, with its pixel stride.
enum ColorSource<'a> {
    /// 8-bit RGB, as `crate::encoder::encode_rgb8` hands it over.
    Rgb8(&'a [Rgb<u8>], usize),
    /// 16-bit RGB (full 0..=65535), as `crate::encoder::encode_rgb16` does.
    Rgb16(&'a [rgb::Rgb<u16>], usize),
}

/// The 4:2:0 colour planes this seam feeds `encode_key_frame`, as tight `u16`
/// samples in the `bit_depth`-bit range.
///
/// **Limited, not full range** — the port's sequence header pins
/// `color_range = 0`. See the module docs.
///
/// At `bit_depth == 8` from an 8-bit source this deliberately routes through
/// the dedicated `rgb8_to_yuv420` u8 kernel and widens, which is what the seam
/// has always done; the depth-generic `rgbx_to_yuv420_u16` recipe serves every
/// other cell. The two are not asserted to agree, so keeping the 8-bit path on
/// its original kernel keeps existing 8-bit output byte-for-byte unchanged.
/// `aom_bd8_output_is_unchanged_by_the_hbd_wiring` in
/// `tests/aom_encode_backend.rs` is the gate on that.
fn color_planes_420(
    src: ColorSource<'_>,
    width: usize,
    height: usize,
    bit_depth: u8,
) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
    use crate::yuv_convert::{YuvMatrix, YuvRange};
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let mut y = vec![0u16; width * height];
    let mut u = vec![0u16; cw * ch];
    let mut v = vec![0u16; cw * ch];
    match src {
        ColorSource::Rgb8(buf, stride) if bit_depth == 8 => {
            let mut y8 = vec![0u8; width * height];
            let mut u8p = vec![0u8; cw * ch];
            let mut v8p = vec![0u8; cw * ch];
            crate::yuv_convert::rgb8_to_yuv420(
                buf,
                stride,
                width,
                height,
                YuvRange::Limited,
                YuvMatrix::Bt601,
                &mut y8,
                &mut u8p,
                &mut v8p,
            );
            // `encode_key_frame` takes u16 samples in the bit_depth-bit range;
            // an 8-bit source carries 8-bit values, so this is a widen, not a
            // scale.
            for (d, s) in y.iter_mut().zip(&y8) {
                *d = u16::from(*s);
            }
            for (d, s) in u.iter_mut().zip(&u8p) {
                *d = u16::from(*s);
            }
            for (d, s) in v.iter_mut().zip(&v8p) {
                *d = u16::from(*s);
            }
        }
        ColorSource::Rgb8(buf, stride) => {
            crate::yuv_convert::rgbx_to_yuv420_u16(
                buf,
                stride,
                width,
                height,
                bit_depth,
                YuvRange::Limited,
                YuvMatrix::Bt601,
                &mut y,
                &mut u,
                &mut v,
            );
        }
        ColorSource::Rgb16(buf, stride) => {
            crate::yuv_convert::rgbx_to_yuv420_u16(
                buf,
                stride,
                width,
                height,
                bit_depth,
                YuvRange::Limited,
                YuvMatrix::Bt601,
                &mut y,
                &mut u,
                &mut v,
            );
        }
    }
    (y, u, v)
}

/// Shared tail of the colour entry points: encode the planes and mux.
fn finish_color_420(
    planes: (Vec<u16>, Vec<u16>, Vec<u16>),
    config: &EncoderConfig,
    width: usize,
    height: usize,
    bit_depth: u8,
    stop: &almost_enough::StopToken,
) -> Result<EncodedImage> {
    use almost_enough::Stop;
    let (y, u, v) = planes;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let cfg = key_frame_config(config, width, height, bit_depth, false);
    let payload = encode_key_frame_checked(
        aom_encode::key_frame::KeyFramePlanes {
            y: &y,
            u: &u,
            v: &v,
        },
        &cfg,
    )?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    mux_aom(
        config,
        payload,
        width,
        height,
        u8::try_from(cfg.profile()).unwrap_or(0),
        bit_depth,
        config.color_primaries.unwrap_or(DEFAULT_COLOR_PRIMARIES),
        config
            .transfer_characteristics
            .unwrap_or(DEFAULT_TRANSFER_CHARACTERISTICS),
        false,
    )
}

/// Reject a zero-sized input before anything allocates.
fn reject_empty(width: usize, height: usize) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(at!(Error::Encode(
            "Av1Backend::Zenav1Aom: width and height must be non-zero".into()
        )));
    }
    Ok(())
}

/// Encode an 8-bit RGB image to AVIF via the zenav1-aom backend.
///
/// **KEY-frame / still scope only** — see the module docs. Cancellation is
/// checked at the seam's phase boundaries (pre-conversion, pre-encode,
/// pre-mux); `encode_key_frame` takes no stop token, so a single frame's
/// encode is not interruptible once entered.
///
/// The coded depth follows [`EncoderConfig::bit_depth`] /
/// [`EncoderConfig::bit_depth_bits`]: 8 (the default for 8-bit input), 10 and
/// 12 all encode. At 10 or 12 the conversion quantizes at the OUTPUT depth, so
/// an 8-bit source gains chroma-average precision it would lose through an
/// 8-bit quantize-then-widen — it does not gain luma detail it never had.
pub(crate) fn encode_rgb8_aom(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    use almost_enough::Stop;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;
    let bit_depth = resolve_aom_depth(config, false, false)?;

    let width = img.width();
    let height = img.height();
    reject_empty(width, height)?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let planes = color_planes_420(
        ColorSource::Rgb8(img.buf(), img.stride()),
        width,
        height,
        bit_depth,
    );
    finish_color_420(planes, config, width, height, bit_depth, &stop)
}

/// Encode a 16-bit RGB image to AVIF via the zenav1-aom backend.
///
/// **KEY-frame / still scope only.** The coded depth follows
/// [`EncoderConfig::bit_depth`] / [`EncoderConfig::bit_depth_bits`], with
/// [`crate::EncodeBitDepth::Auto`]'s documented rule (16-bit input -> 10-bit AV1)
/// unchanged. `bit_depth_bits(12)` is what reaches profile 2.
///
/// Unlike the zenravif 16-bit path — which encodes identity-matrix GBR planes
/// at 4:4:4 — this converts to YCbCr 4:2:0 BT.601 limited range, the only
/// shape this seam has. That is the same shape as the zenav1-svt 16-bit path,
/// and it is why [`crate::EncoderConfig::validate_for_input`] allows
/// `16-bit + Yuv420` for these two backends and no other.
pub(crate) fn encode_rgb16_aom(
    img: ImgRef<'_, rgb::Rgb<u16>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    use almost_enough::Stop;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;
    let bit_depth = resolve_aom_depth(config, true, false)?;

    let width = img.width();
    let height = img.height();
    reject_empty(width, height)?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let planes = color_planes_420(
        ColorSource::Rgb16(img.buf(), img.stride()),
        width,
        height,
        bit_depth,
    );
    finish_color_420(planes, config, width, height, bit_depth, &stop)
}

/// Encode an 8-bit grayscale image to AVIF as true monochrome (Cs400) via the
/// zenav1-aom backend.
///
/// **KEY-frame / still scope only** — see the module docs. The luma plane is
/// passed through unchanged: a Cs400 stream carries the samples the caller
/// handed in, and the sequence header's `color_range = 0` is signalled in the
/// container as `full_color_range = false`.
#[cfg(feature = "encode-mono")]
pub(crate) fn encode_gray8_aom(
    img: ImgRef<'_, u8>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    use almost_enough::Stop;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;
    // `monochrome = true`: the Cs400 path is 8-bit only — see `aom_depth_error`.
    let bit_depth = resolve_aom_depth(config, false, true)?;

    let width = img.width();
    let height = img.height();
    reject_empty(width, height)?;

    let mut y = Vec::with_capacity(width * height);
    for row in img.rows() {
        y.extend(row.iter().map(|&s| u16::from(s)));
    }

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let cfg = key_frame_config(config, width, height, bit_depth, true);
    let payload = encode_key_frame_checked(
        aom_encode::key_frame::KeyFramePlanes {
            y: &y,
            u: &[],
            v: &[],
        },
        &cfg,
    )?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    mux_aom(
        config,
        payload,
        width,
        height,
        u8::try_from(cfg.profile()).unwrap_or(0),
        bit_depth,
        config.color_primaries.unwrap_or(DEFAULT_COLOR_PRIMARIES),
        config
            .transfer_characteristics
            .unwrap_or(DEFAULT_TRANSFER_CHARACTERISTICS),
        true,
    )
}

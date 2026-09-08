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
//! * RGB -> YCbCr **4:2:0**, BT.601, **limited (studio) range**, at **8, 10 or
//!   12 bits** ([`crate::EncodeBitDepth`]), from both the 8-bit
//!   ([`crate::encoder::encode_rgb8`]) and 16-bit
//!   ([`crate::encoder::encode_rgb16`]) entry points. See "Colour signalling"
//!   below for why the range is not full, and "Bit depth" for how the depth is
//!   chosen.
//! * grayscale -> true monochrome (Cs400), coded at 8, 10 or 12 bits. The
//!   input is 8-bit (`encode_gray8`); the CODED depth is the caller's, and
//!   the samples are scaled to it by the same rule the colour and alpha
//!   paths use.
//!
//! Refused by name, each with the reason (`reject_unsupported_config`,
//! [`aom_depth_error`]):
//!
//! * 4:4:4 and the identity/RGB colour model — the *encoder* gates 4:2:0,
//!   4:2:2 and 4:4:4 (186/186 cells), but this seam has no forward RGB->YUV
//!   4:4:4 kernel: `src/yuv_convert.rs` ships 4:2:0 kernels and no 4:4:4
//!   counterpart. Same refusal, and the same reason, as the zenav1-svt seam.
//!   Note this is a CHROMA-FORMAT gap and has nothing to do with bit depth —
//!   conflating the two is what kept 10/12-bit refused until 2026-09-03.
//! * Alpha (`encode_rgba8` / `encode_rgba16`) — the auxiliary Cs400 item is
//!   not wired yet; the mono encode it needs already exists here.
//! * High-bit-depth GRAYSCALE — `encode_gray8` takes `u8` samples and this
//!   seam passes them through as the coded luma, so promoting them to a 10- or
//!   12-bit swing would need a value-scaling rule nothing here measures.
//! * Full pixel range, gain maps, animation.
//!
//! # Bit depth (wired 2026-09-03)
//!
//! The colour path codes 8, 10 and 12 bits; all three are byte-gated upstream.
//! The depth comes from [`crate::EncoderConfig::coded_bit_depth_bits`] — the
//! one resolver `validate`, every encode seam and `resolve_plan` share — so
//! [`crate::EncodeBitDepth::Auto`] (8-bit input -> 8, 16-bit input -> 10),
//! `::Ten` and `::Twelve` all land in [`resolve_aom_depth`].
//!
//! Conversion is [`crate::yuv_convert::rgbx_to_yuv420_u16`], the depth-generic
//! recipe the zenav1-svt seam already used: it quantizes at the OUTPUT depth,
//! so the 2x2 chroma average keeps fraction bits an 8-bit
//! quantize-then-widen would drop, and `FwdConsts::for_depth` shifts the studio
//! swing by `<< (depth - 8)` per H.273 (offset `16 << (d-8)`, span
//! `219 << (d-8)`). The sequence header pins `color_range = 0` at **every**
//! depth, so full range stays refused at every depth too.
//!
//! One cell does not use that recipe: **8-bit input at depth 8** keeps the
//! dedicated `rgb8_to_yuv420` u8 kernel and widens, exactly as this seam
//! always did. That split is conservatism, **not** the reason 8-bit output is
//! stable — MEASURED (`benchmarks/aom_bd8_identity_2026-09-03.*`): 60 of 60
//! cells are byte-identical across the high-bit-depth wiring, and routing that
//! cell through the u16 recipe instead changes **0 of 60**, so the two kernels
//! agree at output depth 8. See [`color_planes_420`].
//!
//! 12-bit is AV1 **profile 2**: `KeyFrameConfig::profile()` returns 2 at that
//! depth, and `mux_aom` passes the depth to `zenavif-serialize`, which derives
//! the `av1C` `high_bitdepth` / `twelve_bit` flags and the `pixi` depth from it
//! and raises `seq_profile` to match (`aom_backend_12_bit_signals_profile_2`
//! reads all three back out of the box).
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
//!
//!   **`--cq-level 0` (quality 100) is coded-lossless** (`base_qindex == 0`,
//!   Walsh-Hadamard), and reconstructs the seam's 4:2:0 planes EXACTLY at 8,
//!   10 and 12 bits — gated with zero tolerance on both decoders by
//!   `aom_cq0_encodes_and_reconstructs_the_coded_planes_exactly`
//!   (`tests/aom_encode_backend.rs`). It is not a lossless *image* round trip:
//!   the RGB -> studio-range 4:2:0 conversion in front of it is lossy, and
//!   `EncoderConfig::lossless` is still refused by name here. History: at the
//!   pin before `45c53ddb` (zenavif#45) cq 0 hit a debug assertion in the
//!   port's leaf counter on flat content (`count_leaf` paraphrased libaom's
//!   `txb_split_count` predicate as a depth walk C never performs under
//!   `ONLY_4X4`); fixed at the root in zenav1-aom `21544fde`, and the canary
//!   that pinned the panic became the exactness gate above.
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
//! **Re-measured 2026-09-03 across all three coded depths** (192x128
//! gradient, quality 88, speed 5; emitted by `dev/downstream-probe`'s `emit`
//! binary). `sips` reports the DEPTH as well as the dimensions, so this is
//! independent confirmation of the `av1C` `high_bitdepth` / `twelve_bit`
//! signalling by a decoder that shares no code with this workspace:
//!
//! | file | `sips` `bitsPerSample` | mean per-channel delta vs source | max |
//! |---|---|---|---|
//! | 8-bit | 8 | 0.919 | 5 |
//! | 10-bit | 10 | 0.690 | 3 |
//! | 12-bit | 12 | 0.674 | 4 |
//!
//! (73,728 8-bit channel values per file, after `sips` transcodes to PNG.
//! The high-bit-depth files score BETTER, which is the expected direction —
//! more coded precision on the same source — not evidence of anything else.)
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
pub(crate) fn reject_unsupported_config(config: &EncoderConfig) -> Result<()> {
    if config.color_model != EncodeColorModel::YCbCr {
        // Identity (GBR) is now SUPPORTED — at 4:4:4, which is where AV1
        // allows it. `matrix_coefficients = MC_IDENTITY` means the three
        // planes ARE G/B/R, so there is nothing to subsample; AV1 5.5.2 makes
        // `subsampling_x == subsampling_y == 0` a conformance requirement and
        // upstream's `validate_configuration` refuses the pairing. The old
        // blanket refusal ("identity/RGB has no defined 4:2:0 subsampling")
        // was right about 4:2:0 and wrong to conclude the model was
        // unsupported: it is the only way to encode MATHEMATICALLY lossless.
        if config.chroma_subsampling != EncodeChromaSubsampling::Yuv444 {
            return Err(at!(Error::Unsupported(
                "Av1Backend::Zenav1Aom: the RGB (identity) colour model requires \
                 4:4:4 — subsampling G/B/R planes is not defined in AV1. \
                 Use .chroma_subsampling(EncodeChromaSubsampling::Yuv444)"
            )));
        }
    }
    if config.gain_map.is_some() {
        return Err(at!(Error::Unsupported(
            "Av1Backend::Zenav1Aom does not support gain maps \
             (use the zenravif backend)"
        )));
    }
    #[cfg(feature = "encode-imazen")]
    if config.lossless {
        // Lossless IS wired now — `--cq-level 0` is coded-lossless upstream
        // (base_qindex 0, Walsh-Hadamard), byte-identical to real aomenc
        // across 427 cells and asserted to reconstruct the CODED PLANES
        // exactly on 248/248 cells on both decoders.
        //
        // But "lossless" is a claim about the IMAGE, and everything in front
        // of the coded planes has to be lossless too. Refuse the combinations
        // that cannot be, rather than emit a smaller file and call it
        // lossless:
        if config.chroma_subsampling != EncodeChromaSubsampling::Yuv444 {
            return Err(at!(Error::Unsupported(
                "Av1Backend::Zenav1Aom: lossless requires 4:4:4 — 4:2:0 discards \
                 three quarters of the chroma before the encoder sees it. \
                 Use .chroma_subsampling(EncodeChromaSubsampling::Yuv444)"
            )));
        }
        if config.color_model != EncodeColorModel::Rgb {
            return Err(at!(Error::Unsupported(
                "Av1Backend::Zenav1Aom: mathematically lossless requires the RGB \
                 (identity/GBR) colour model — a YCbCr matrix round trip is not \
                 exactly invertible in integer samples. \
                 Use .color_model(EncodeColorModel::Rgb)"
            )));
        }
        if config.pixel_range == Some(EncodePixelRange::Limited) {
            return Err(at!(Error::Unsupported(
                "Av1Backend::Zenav1Aom: lossless requires full pixel range — the \
                 studio swing (16..235) cannot represent every input code point. \
                 Use .pixel_range(EncodePixelRange::Full)"
            )));
        }
    }
    Ok(())
}

/// The bit-depth envelope this seam accepts, shared by the encode path and
/// `EncoderConfig::validate` -- so a config that validates encodes and a
/// config that encodes validates. Returns the reason a `bit_depth`-bit encode
/// is refused, or `None` when it encodes.
///
/// 8, 10 and 12 all encode, on the colour path AND on the grayscale (Cs400)
/// path -- all byte-gated upstream. The grayscale arm refused above 8 bits
/// until the sample conversion learned to scale; `monochrome` survives as a
/// parameter so that history stays legible at the call sites.
pub(crate) fn aom_depth_error(bit_depth: u8, monochrome: bool) -> Option<&'static str> {
    if !matches!(bit_depth, 8 | 10 | 12) {
        return Some(
            "Av1Backend::Zenav1Aom codes 8, 10 or 12 bits (aom_encode::key_frame gates \
             exactly those three); use EncodeBitDepth::Eight, ::Ten or ::Twelve",
        );
    }
    // Grayscale used to be refused above 8 bits here, on the grounds that
    // promotion was "not wired or measured". It is now both: the sample
    // conversion in `encode_gray8_aom` scales to the coded depth with the same
    // rule the colour and alpha paths use, and the encoder's monochrome path
    // codes 8, 10 and 12 (the AVIF alpha auxiliary item is a Cs400 stream and
    // has been coded at 12 bits since alpha landed). `monochrome` is retained
    // as a parameter because it is what makes that history legible and because
    // a future depth limit would be depth-and-format shaped, not depth alone.
    let _ = monochrome;
    None
}

/// Encode-time twin of `reject_unsupported_config`'s depth check, both driven by
/// [`aom_depth_error`]. Returns the depth to code at.
///
/// The depth comes from the one shared resolver,
/// [`EncoderConfig::coded_bit_depth_bits`], so [`crate::EncodeBitDepth::Auto`],
/// [`crate::EncodeBitDepth::Ten`] and [`crate::EncodeBitDepth::Twelve`] all land
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
#[allow(deprecated)]
fn cicp_to_serialize_transfer(tc: u8) -> zenavif_serialize::constants::TransferCharacteristics {
    use zenavif_serialize::constants::TransferCharacteristics as TC;
    match tc {
        1 => TC::Bt709,
        4 => TC::Bt470M,
        5 => TC::Bt470BG,
        6 => TC::Bt601,
        7 => TC::Smpte240,
        9 => TC::Log,
        10 => TC::LogSqrt,
        11 => TC::Iec61966,
        12 => TC::Bt1361,
        15 => TC::Bt2020_12,
        17 => TC::Smpte428,
        8 => TC::Linear,
        13 => TC::Srgb,
        14 => TC::Bt2020_10,
        16 => TC::Smpte2084,
        18 => TC::Hlg,
        _ => TC::Unspecified,
    }
}

/// Build the `aom_encode` config for one still.
pub(crate) fn key_frame_config(
    config: &EncoderConfig,
    width: usize,
    height: usize,
    bit_depth: u8,
    monochrome: bool,
) -> aom_encode::key_frame::KeyFrameConfig {
    // Monochrome carries (1, 1) — the AOM_IMG_FMT_I420 a mono image allocates;
    // `encode_key_frame` rejects any other ss for mono. Colour honours the
    // caller's request, whose DEFAULT is 4:4:4.
    let (ss_x, ss_y) = if monochrome {
        (1, 1)
    } else {
        match config.chroma_subsampling {
            EncodeChromaSubsampling::Yuv444 => (0, 0),
            EncodeChromaSubsampling::Yuv420 => (1, 1),
        }
    };
    aom_encode::key_frame::KeyFrameConfig {
        width,
        height,
        bit_depth,
        monochrome,
        ss_x,
        ss_y,
        // `lossless` pins cq 0 — coded-lossless upstream (base_qindex 0), which
        // reconstructs the coded planes EXACTLY. `reject_unsupported_config` has
        // already refused every combination in front of it that would make the
        // IMAGE lossy anyway (subsampling, a YCbCr matrix, studio range).
        cq_level: if wants_lossless(config) {
            0
        } else {
            quality_to_cq_level(config.quality)
        },
        cpu_used: speed_to_cpu_used(config.speed),
        usage: AOM_USAGE_ALL_INTRA,
        // Real aomenc's ALLINTRA defaults (`av1_cx_iface.c:3067`): CDEF off
        // ("CDEF has been found to blur images"), loop restoration on. Both
        // are byte-gated upstream at every speed in this combination.
        enable_cdef: false,
        enable_restoration: true,
        // Explicit tile / superblock requests landed upstream after the
        // previous pin (`bda14f3d`, `abe20559`). `0` / `false` are the
        // `allintra_speed0` defaults and what the previous rev hard-coded, so
        // the seam's output is unchanged by the bump (the bd8 byte anchor
        // `aom_bd8_output_is_unchanged_by_the_hbd_wiring` holds). The tile
        // request is a FLOOR: `av1_get_tile_limits` still forces the minimum
        // a large frame needs, exactly as C clamps it.
        tile_columns_log2: 0,
        tile_rows_log2: 0,
        sb_size_128: false,
        // The CICP description + range. `full_range` is now CONFIGURATION
        // upstream (`ColorDescription`), so a full-range still is codable
        // instead of refused; the CICP triple stays "unspecified" because the
        // `colr` box carries the colorimetry for this seam (see the module
        // docs) and signalling it twice invites the two to disagree.
        color: aom_encode::key_frame::ColorDescription {
            full_range: wants_full_range(config),
            // MC_IDENTITY (0) for the GBR path, "unspecified" otherwise. The
            // primaries/transfer stay unspecified either way: the `colr` box
            // carries the colorimetry for this seam, and signalling it in two
            // places invites the two to disagree.
            matrix_coefficients: if wants_identity(config) { 0 } else { 2 },
            ..Default::default()
        },
    }
}

/// Whether this encode signals FULL pixel range.
///
/// `EncodePixelRange::Full` used to be refused here, citing the upstream
/// sequence header's pinned `color_range = 0`. That pin is gone
/// (`ColorDescription`), so the request is honoured: the samples are converted
/// with the full-range recipe AND the sequence header says so, which are the
/// two halves that have to agree.
fn wants_full_range(config: &EncoderConfig) -> bool {
    // Identity (GBR) planes ARE the source samples; coding them against the
    // studio swing would quantize them, so identity always carries full range.
    // Lossless implies identity (see `reject_unsupported_config`), hence full range
    // too.
    wants_identity(config) || matches!(config.pixel_range, Some(EncodePixelRange::Full))
}

/// Whether the caller asked for mathematically lossless output.
///
/// `EncoderConfig::lossless` is behind `encode-imazen`, so this is the one
/// place that knows it; everything else asks this.
fn wants_lossless(config: &EncoderConfig) -> bool {
    #[cfg(feature = "encode-imazen")]
    {
        config.lossless
    }
    #[cfg(not(feature = "encode-imazen"))]
    {
        let _ = config;
        false
    }
}

/// Whether this encode uses the identity (GBR) "matrix" — i.e. no colour
/// conversion at all, the AV1 `MC_IDENTITY` path. Only legal at 4:4:4.
fn wants_identity(config: &EncoderConfig) -> bool {
    config.color_model == EncodeColorModel::Rgb
}

/// **The CICP matrix this backend actually signals for a given request** — the
/// one value a caller may ask for and get.
///
/// Single source of truth for the two places that must agree, or the adapter
/// refuses what the container is about to write: the AVIF `colr` box (see
/// `mux_aom`'s `set_matrix_coefficients`) and the adapter's accept/refuse
/// predicate in `backend_router::validate_adapter_controls`.
///
/// It is a function rather than two copies of one expression because the two
/// copies HAD drifted, and the drift was measured: the muxer emits
/// `MatrixCoefficients::Rgb` (CICP 0) for an identity encode, while the
/// adapter predicate hardcoded `if mono { 2 } else { 6 }` and so refused CICP
/// 0 with *"requested matrix is not implemented by this pixel conversion
/// path"* — on the very configuration whose pixel conversion path is the
/// identity one. A caller asking for exactly what the muxer was about to write
/// got a refusal naming a capability that was not missing.
///
/// **Not the sequence header.** `key_frame_config` codes `MC_IDENTITY` (0) for
/// the GBR path and "unspecified" (2) otherwise, deliberately: this seam lets
/// the `colr` box carry the colorimetry (module docs above), and a decoder
/// resolving "unspecified" falls back to BT.601 — which is what
/// `rgbx_to_yuv*` converts with, so the two agree in effect without signalling
/// the same fact twice. The identity case is the one where the sequence header
/// MUST carry it, because it changes how the planes are interpreted rather
/// than merely describing them, and there both values are 0.
///
/// * **0 (identity / GBR)** when the RGB colour model is requested. The three
///   planes ARE G/B/R, so no matrix is applied; AV1 5.5.2 requires 4:4:4 for
///   it, which [`reject_unsupported_config`] enforces separately.
/// * **2 (unspecified)** for monochrome: there is no chroma to relate to luma,
///   and naming a matrix would describe planes that do not exist.
/// * **6 (BT.601)** otherwise, which is what `rgbx_to_yuv*` implements.
pub(crate) fn coded_matrix_coefficients(config: &EncoderConfig, monochrome: bool) -> u8 {
    if monochrome {
        2
    } else if wants_identity(config) {
        0
    } else {
        6
    }
}

/// The forward-conversion range that matches [`wants_full_range`].
fn fwd_range(config: &EncoderConfig) -> crate::yuv_convert::YuvRange {
    if wants_full_range(config) {
        crate::yuv_convert::YuvRange::Full
    } else {
        crate::yuv_convert::YuvRange::Limited
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
    chroma: (bool, bool),
    full_range: bool,
    alpha: Option<Vec<u8>>,
) -> Result<EncodedImage> {
    let w = u32::try_from(width).map_err(|_| at!(Error::Encode("width exceeds u32".into())))?;
    let h = u32::try_from(height).map_err(|_| at!(Error::Encode("height exceeds u32".into())))?;
    let mut aviffy = zenavif_serialize::Aviffy::new();
    aviffy
        .set_seq_profile(seq_profile)
        // Both of these MUST match what the sequence header actually codes —
        // a container that disagrees with the bitstream is the mis-signalling
        // this seam exists to avoid. They were hardcoded (4:2:0, studio) back
        // when the encoder could produce nothing else.
        .set_chroma_subsampling(chroma)
        .set_monochrome(monochrome)
        .set_full_color_range(full_range)
        .set_color_primaries(cicp_to_serialize_primaries(color_primaries))
        .set_transfer_characteristics(cicp_to_serialize_transfer(transfer_characteristics))
        // Derived, not passed: an `identity: bool` argument here would be a
        // second copy of a fact the config already carries, and the caller
        // could pass the wrong one. `MatrixCoefficients::Rgb` IS CICP 0
        // (GBR/identity); `Unspecified` is 2; `Bt601` is 6.
        .set_matrix_coefficients(match coded_matrix_coefficients(config, monochrome) {
            0 => zenavif_serialize::constants::MatrixCoefficients::Rgb,
            2 => zenavif_serialize::constants::MatrixCoefficients::Unspecified,
            _ => zenavif_serialize::constants::MatrixCoefficients::Bt601,
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
        .try_to_vec(&payload, alpha.as_deref(), w, h, bit_depth)
        .map_err(|e| at!(Error::Encode(format!("AVIF serialization failed: {e}"))))?;
    Ok(EncodedImage {
        avif_file,
        color_byte_size,
        alpha_byte_size: alpha.as_ref().map_or(0, Vec::len),
    })
}

/// The colour input the 4:2:0 conversion reads, with its pixel stride.
enum ColorSource<'a> {
    /// 8-bit RGB, as `crate::encoder::encode_rgb8` hands it over.
    Rgb8(&'a [Rgb<u8>], usize),
    /// 16-bit RGB (full 0..=65535), as `crate::encoder::encode_rgb16` does.
    Rgb16(&'a [rgb::Rgb<u16>], usize),
    /// 8-bit RGBA. Alpha rides its OWN Cs400 auxiliary item, so the colour
    /// conversion ignores it exactly as the RGB variants do.
    Rgba8(&'a [rgb::Rgba<u8>], usize),
    /// 16-bit RGBA.
    Rgba16(&'a [rgb::Rgba<u16>], usize),
}

/// The colour planes this seam feeds `encode_key_frame`, as tight `u16`
/// samples in the `bit_depth`-bit range, at the requested subsampling and
/// pixel range.
///
/// **4:4:4 used to be REFUSED here** — and 4:4:4 is
/// [`EncodeChromaSubsampling`]'s DEFAULT, so this backend could not serve an
/// unconfigured caller at all. The refusal cited a missing forward kernel,
/// which `yuv_convert::rgbx_to_yuv444*` now supplies; the ENCODER has been
/// byte-exact at 4:4:4 and 4:2:2 throughout.
///
/// **Range is the caller's** — the upstream sequence header no longer pins
/// `color_range = 0`, so a full-range request converts with the full-range
/// recipe AND signals it. See [`wants_full_range`].
///
/// At `bit_depth == 8` from an 8-bit source this routes through the dedicated
/// `rgb8_to_yuv420` u8 kernel and widens, which is what the seam has always
/// done; the depth-generic `rgbx_to_yuv420_u16` recipe serves every other cell.
///
/// **That split is a conservatism/perf choice, not a correctness guard, and
/// the first draft of this comment had it wrong.** MEASURED
/// (`benchmarks/aom_bd8_identity_2026-09-03.*`): routing the 8-bit cell through
/// the u16 recipe instead changes **0 of 60** cells — the two kernels agree
/// byte-for-byte at output depth 8 across 6 geometries x 5 (quality, speed)
/// pairs x 2 content classes. The same harness detects a real change (a
/// full-range mutation moves 60/60), so that is a result and not a broken
/// comparison. The u8 kernel is kept because it packs twice the lanes per
/// vector; nobody has measured by how much, so no speed number is claimed.
///
/// What actually establishes that 8-bit output did not move across the
/// high-bit-depth wiring is the same benchmark: 60/60 cells byte-identical
/// between `ec6728b` and this tree.
fn color_planes(
    src: ColorSource<'_>,
    width: usize,
    height: usize,
    bit_depth: u8,
    ss_x: usize,
    ss_y: usize,
    range: crate::yuv_convert::YuvRange,
    identity: bool,
) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
    use crate::yuv_convert::YuvMatrix;
    let sub = (ss_x, ss_y) == (1, 1);
    let (cw, ch) = if sub {
        (width.div_ceil(2), height.div_ceil(2))
    } else {
        (width, height)
    };
    let mut y = vec![0u16; width * height];
    let mut u = vec![0u16; cw * ch];
    let mut v = vec![0u16; cw * ch];
    // Identity: the planes ARE the source channels, in AV1's G/B/R order, with
    // no matrix and no range compression. This is the only path on which the
    // image (not merely the coded planes) can be lossless.
    if identity {
        debug_assert!(!sub, "MC_IDENTITY is 4:4:4 only (AV1 5.5.2)");
        match src {
            ColorSource::Rgb8(buf, stride) => {
                let shift = bit_depth - 8;
                for row in 0..height {
                    for x in 0..width {
                        let px = buf[row * stride + x];
                        let (r, g, b) = (u16::from(px.r), u16::from(px.g), u16::from(px.b));
                        // An 8-bit source at a deeper coded depth is SCALED,
                        // not widened: 255 must map to the new maximum or the
                        // round trip is not lossless.
                        let up = |c: u16| -> u16 {
                            if shift == 0 { c } else { (c << shift) | (c >> (8 - shift)) }
                        };
                        y[row * width + x] = up(g);
                        u[row * width + x] = up(b);
                        v[row * width + x] = up(r);
                    }
                }
            }
            ColorSource::Rgb16(buf, stride) => {
                let down = 16 - u32::from(bit_depth);
                for row in 0..height {
                    for x in 0..width {
                        let px = buf[row * stride + x];
                        y[row * width + x] = px.g >> down;
                        u[row * width + x] = px.b >> down;
                        v[row * width + x] = px.r >> down;
                    }
                }
            }
            ColorSource::Rgba8(buf, stride) => {
                let shift = bit_depth - 8;
                for row in 0..height {
                    for x in 0..width {
                        let px = buf[row * stride + x];
                        let up = |c: u8| -> u16 {
                            let c = u16::from(c);
                            if shift == 0 { c } else { (c << shift) | (c >> (8 - shift)) }
                        };
                        y[row * width + x] = up(px.g);
                        u[row * width + x] = up(px.b);
                        v[row * width + x] = up(px.r);
                    }
                }
            }
            ColorSource::Rgba16(buf, stride) => {
                let down = 16 - u32::from(bit_depth);
                for row in 0..height {
                    for x in 0..width {
                        let px = buf[row * stride + x];
                        y[row * width + x] = px.g >> down;
                        u[row * width + x] = px.b >> down;
                        v[row * width + x] = px.r >> down;
                    }
                }
            }
        }
        return (y, u, v);
    }
    match src {
        // The dedicated u8 kernel is kept for the 8-bit 4:2:0 cell only: it is
        // the historical path and the one the bd8 byte anchor pins. Measured
        // equal to the u16 recipe at output depth 8 (60/60 cells), so this is
        // a lane-packing choice, not a correctness one.
        ColorSource::Rgb8(buf, stride) if bit_depth == 8 && sub => {
            let mut y8 = vec![0u8; width * height];
            let mut u8p = vec![0u8; cw * ch];
            let mut v8p = vec![0u8; cw * ch];
            crate::yuv_convert::rgb8_to_yuv420(
                buf, stride, width, height, range, YuvMatrix::Bt601, &mut y8, &mut u8p, &mut v8p,
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
            if sub {
                crate::yuv_convert::rgbx_to_yuv420_u16(
                    buf, stride, width, height, bit_depth, range, YuvMatrix::Bt601, &mut y, &mut u,
                    &mut v,
                );
            } else {
                crate::yuv_convert::rgbx_to_yuv444_u16(
                    buf, stride, width, height, bit_depth, range, YuvMatrix::Bt601, &mut y, &mut u,
                    &mut v,
                );
            }
        }
        ColorSource::Rgb16(buf, stride) => {
            if sub {
                crate::yuv_convert::rgbx_to_yuv420_u16(
                    buf, stride, width, height, bit_depth, range, YuvMatrix::Bt601, &mut y, &mut u,
                    &mut v,
                );
            } else {
                crate::yuv_convert::rgbx_to_yuv444_u16(
                    buf, stride, width, height, bit_depth, range, YuvMatrix::Bt601, &mut y, &mut u,
                    &mut v,
                );
            }
        }
        // Alpha rides its own Cs400 item; the colour conversion drops it.
        ColorSource::Rgba8(buf, stride) => {
            if sub {
                crate::yuv_convert::rgbx_to_yuv420_u16(
                    buf, stride, width, height, bit_depth, range, YuvMatrix::Bt601, &mut y, &mut u,
                    &mut v,
                );
            } else {
                crate::yuv_convert::rgbx_to_yuv444_u16(
                    buf, stride, width, height, bit_depth, range, YuvMatrix::Bt601, &mut y, &mut u,
                    &mut v,
                );
            }
        }
        ColorSource::Rgba16(buf, stride) => {
            if sub {
                crate::yuv_convert::rgbx_to_yuv420_u16(
                    buf, stride, width, height, bit_depth, range, YuvMatrix::Bt601, &mut y, &mut u,
                    &mut v,
                );
            } else {
                crate::yuv_convert::rgbx_to_yuv444_u16(
                    buf, stride, width, height, bit_depth, range, YuvMatrix::Bt601, &mut y, &mut u,
                    &mut v,
                );
            }
        }
    }
    (y, u, v)
}

/// Shared tail of the colour entry points: encode the planes and mux.
fn finish_color(
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
        (cfg.ss_x == 1, cfg.ss_y == 1),
        cfg.color.full_range,
        None,
    )
}


/// Encode a straight (non-premultiplied) alpha plane as the Cs400 monochrome
/// auxiliary item an AVIF `auxl` alpha reference points at.
///
/// # Why this is a separate encode
///
/// AVIF codes alpha as its OWN AV1 image item — a monochrome stream with the
/// same dimensions as the colour item — not as a fourth plane. The mono encode
/// it needs has existed at this seam since the gray8 path landed; what was
/// missing was the item, which is why `encode_rgba8` / `encode_rgba16` used to
/// land in `reject_aom_backend`.
///
/// **Alpha is FULL RANGE.** An alpha sample is a coverage fraction: 0 means
/// none and the maximum means complete, so the studio swing would both clip
/// and quantize it. That is exactly what the upstream sequence header's pinned
/// `color_range = 0` made impossible, and why this could not be wired before
/// `ColorDescription` existed.
fn encode_alpha_plane(
    alpha: &[u16],
    width: usize,
    height: usize,
    bit_depth: u8,
    config: &EncoderConfig,
) -> Result<Vec<u8>> {
    let mut cfg = key_frame_config(config, width, height, bit_depth, true);
    cfg.cq_level = if wants_lossless(config) {
        0
    } else {
        quality_to_cq_level(crate::encoder::effective_alpha_quality(config))
    };
    cfg.color = aom_encode::key_frame::ColorDescription {
        full_range: true,
        ..Default::default()
    };
    // A mono encode reads only the luma plane; `encode_key_frame` still wants
    // the chroma slices to exist and be empty for monochrome.
    encode_key_frame_checked(
        aom_encode::key_frame::KeyFramePlanes {
            y: alpha,
            u: &[],
            v: &[],
        },
        &cfg,
    )
}

/// Shared tail of the RGBA entry points: colour + alpha, then mux both.
fn finish_color_with_alpha(
    src: ColorSource<'_>,
    alpha: Vec<u16>,
    config: &EncoderConfig,
    width: usize,
    height: usize,
    bit_depth: u8,
    stop: &almost_enough::StopToken,
) -> Result<EncodedImage> {
    use almost_enough::Stop;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let cfg = key_frame_config(config, width, height, bit_depth, false);
    let (y, u, v) = color_planes(
        src,
        width,
        height,
        bit_depth,
        cfg.ss_x,
        cfg.ss_y,
        fwd_range(config),
        wants_identity(config),
    );
    let payload = encode_key_frame_checked(
        aom_encode::key_frame::KeyFramePlanes {
            y: &y,
            u: &u,
            v: &v,
        },
        &cfg,
    )?;

    stop.check().map_err(|e| at!(Error::from(e)))?;
    let alpha_payload = encode_alpha_plane(&alpha, width, height, bit_depth, config)?;

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
        (cfg.ss_x == 1, cfg.ss_y == 1),
        cfg.color.full_range,
        Some(alpha_payload),
    )
}

/// Encode an 8-bit RGBA image to AVIF via the zenav1-aom backend: a colour
/// item plus a Cs400 alpha auxiliary item.
pub(crate) fn encode_rgba8_aom(
    img: ImgRef<'_, rgb::Rgba<u8>>,
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

    let shift = bit_depth - 8;
    let mut alpha = Vec::with_capacity(width * height);
    for row in img.rows() {
        alpha.extend(row.iter().map(|px| {
            let c = u16::from(px.a);
            // Scale, not widen: full-range alpha must map 255 -> the coded
            // maximum, or a fully opaque pixel stops being fully opaque.
            if shift == 0 { c } else { (c << shift) | (c >> (8 - shift)) }
        }));
    }
    finish_color_with_alpha(
        ColorSource::Rgba8(img.buf(), img.stride()),
        alpha,
        config,
        width,
        height,
        bit_depth,
        &stop,
    )
}

/// Encode a 16-bit RGBA image to AVIF via the zenav1-aom backend.
pub(crate) fn encode_rgba16_aom(
    img: ImgRef<'_, rgb::Rgba<u16>>,
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

    let down = 16 - u32::from(bit_depth);
    let mut alpha = Vec::with_capacity(width * height);
    for row in img.rows() {
        alpha.extend(row.iter().map(|px| px.a >> down));
    }
    finish_color_with_alpha(
        ColorSource::Rgba16(img.buf(), img.stride()),
        alpha,
        config,
        width,
        height,
        bit_depth,
        &stop,
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
/// [`crate::EncodeBitDepth`]: 8 (the default for 8-bit input), 10 and
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
    // The subsampling and range the config asks for -- derived from the SAME
    // `key_frame_config` the encode uses, so the conversion cannot disagree
    // with what the sequence header signals.
    let probe = key_frame_config(config, width, height, bit_depth, false);
    let planes = color_planes(
        ColorSource::Rgb8(img.buf(), img.stride()),
        width,
        height,
        bit_depth,
        probe.ss_x,
        probe.ss_y,
        fwd_range(config),
        wants_identity(config),
    );
    finish_color(planes, config, width, height, bit_depth, &stop)
}

/// Encode a 16-bit RGB image to AVIF via the zenav1-aom backend.
///
/// **KEY-frame / still scope only.** The coded depth follows
/// [`EncoderConfig::bit_depth`], with [`crate::EncodeBitDepth::Auto`]'s
/// documented rule (16-bit input -> 10-bit AV1) unchanged.
/// [`crate::EncodeBitDepth::Twelve`] is what reaches profile 2.
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
    // The subsampling and range the config asks for -- derived from the SAME
    // `key_frame_config` the encode uses, so the conversion cannot disagree
    // with what the sequence header signals.
    let probe = key_frame_config(config, width, height, bit_depth, false);
    let planes = color_planes(
        ColorSource::Rgb16(img.buf(), img.stride()),
        width,
        height,
        bit_depth,
        probe.ss_x,
        probe.ss_y,
        fwd_range(config),
        wants_identity(config),
    );
    finish_color(planes, config, width, height, bit_depth, &stop)
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
    let bit_depth = resolve_aom_depth(config, false, true)?;

    let width = img.width();
    let height = img.height();
    reject_empty(width, height)?;

    // Gray8 samples have the same FULL-range pixel semantics as RGB8, so the
    // conversion has to answer two questions the old one answered by hardcode:
    // which range the stream signals, and at what depth.
    //
    // This was a live mis-signalling bug for exactly as long as full range was
    // reachable at this seam. Before that landing `pixel_range(Full)` was
    // refused, so mapping unconditionally to studio 16..235 was always right;
    // afterwards a caller asking for full range got a header saying `full_range
    // = true` over studio-mapped samples — measured as coded luma 16..232 for a
    // 0..252 source, i.e. a washed-out image on any decoder that honours the
    // header. `the_gray_path_codes_the_range_it_signals` is the gate.
    let shift = bit_depth - 8;
    let full_range = wants_full_range(config);
    let mut y = Vec::with_capacity(width * height);
    for row in img.rows() {
        y.extend(row.iter().map(|&s| {
            let c = u16::from(s);
            if full_range {
                // Scale, not widen: 255 must reach the coded maximum, the same
                // rule `color_planes` and the alpha plane use.
                if shift == 0 { c } else { (c << shift) | (c >> (8 - shift)) }
            } else {
                // Studio swing at the coded depth: 16..235 scaled by 1 << shift.
                //
                // In u32 deliberately: at 12 bits the numerator reaches
                // 255 * (219 << 4) = 893,520, which WRAPS in u16 and produced a
                // non-monotonic ramp (measured: a 10-bit luma max of 319 where
                // the arithmetic calls for 930). The values fit u16 again only
                // after the divide.
                let scaled = (u32::from(c) * (219u32 << shift) + 127) / 255;
                (16 << shift) + u16::try_from(scaled).unwrap_or(u16::MAX)
            }
        }));
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
        // Mono codes ss (1, 1) — the AOM_IMG_FMT_I420 a mono image allocates —
        // and `set_monochrome(true)` is what actually suppresses chroma.
        (cfg.ss_x == 1, cfg.ss_y == 1),
        cfg.color.full_range,
        None,
    )
}

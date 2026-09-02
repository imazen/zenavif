//! AVIF encoding via ravif
//!
//! Provides [`EncoderConfig`] for configuring encoding and
//! [`encode_rgb8`] / [`encode_rgba8`] / [`encode_rgb16`] / [`encode_rgba16`]
//! for encoding images.

use crate::Result;
use crate::error::Error;
use almost_enough::Stop;
use imgref::{ImgRef, ImgVec};
use rgb::{RGB8, RGBA8, Rgb, Rgba};
use rgb::{RGB16, RGBA16};
use whereat::{ResultAtExt, at};

/// Classify a `ravif` (zenravif) encode failure into the right zenavif
/// [`Error`], instead of collapsing every case into the opaque `Encode`
/// bucket the pre-reshape code used uniformly.
///
/// `ravif::Error` is `#[non_exhaustive]`; an unrecognized future variant —
/// and `EncodingError` today, which flattens rav1e's own `InvalidConfig`
/// (a config fault) vs `EncoderStatus` (a runtime-state fault) into one
/// string, losing which — falls back to the opaque `Encode` bucket
/// (`Internal(Dependency)`; see `Error::category`).
fn error_from_ravif(e: ravif::Error) -> Error {
    match e {
        ravif::Error::TooFewPixels => {
            Error::InvalidBuffer("provided buffer is smaller than width * height".into())
        }
        ravif::Error::TooManyPixels { width, height, .. } => Error::ImageTooLarge {
            width: width as u32,
            height: height as u32,
        },
        ravif::Error::Unsupported(msg) => Error::InvalidParameters(msg.to_string()),
        ravif::Error::Cancelled => Error::Cancelled(enough::StopReason::Cancelled),
        other => Error::Encode(other.to_string()),
    }
}

/// Pre-encoded gain map data for embedding in an AVIF file.
///
/// Contains a pre-encoded AV1 bitstream of the gain map image plus the
/// ISO 21496-1 binary metadata. Used for UltraHDR / SDR+HDR tone mapping.
///
/// The gain map is typically a lower-resolution, monochrome or RGB image
/// encoding the per-pixel gain needed to reconstruct the HDR rendition from
/// the SDR base image.
#[derive(Debug, Clone)]
pub struct GainMapConfig {
    /// Pre-encoded AV1 bitstream of the gain map image.
    pub av1_data: Vec<u8>,
    /// Width of the gain map image in pixels.
    pub width: u32,
    /// Height of the gain map image in pixels.
    pub height: u32,
    /// Bit depth of the gain map AV1 data (typically 8 or 10).
    pub bit_depth: u8,
    /// ISO 21496-1 binary metadata blob.
    pub metadata: Vec<u8>,
}

/// Encoded AVIF image output
#[derive(Debug, Clone)]
pub struct EncodedImage {
    /// The complete AVIF file bytes
    pub avif_file: Vec<u8>,
    /// Bytes used for the color AV1 payload
    pub color_byte_size: usize,
    /// Bytes used for the alpha AV1 payload
    pub alpha_byte_size: usize,
}

/// Bit depth for encoding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodeBitDepth {
    /// 8 bits per channel
    Eight,
    /// 10 bits per channel
    Ten,
    /// Match input depth: 8-bit input → 8-bit AV1, 16-bit input → 10-bit AV1
    #[default]
    Auto,
}

/// Internal color model for encoding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodeColorModel {
    /// YCbCr color model (smaller files, standard)
    #[default]
    YCbCr,
    /// RGB color model (lossless-friendly)
    Rgb,
}

/// Alpha channel handling mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodeAlphaMode {
    /// Unassociated alpha, clean color values under transparent pixels
    #[default]
    UnassociatedClean,
    /// Unassociated alpha, preserve original color values (may compress worse)
    UnassociatedDirty,
    /// Premultiplied alpha
    Premultiplied,
}

/// Pixel value range for AV1 encoding.
///
/// Full range uses the entire value range (0–255 for 8-bit, 0–1023 for 10-bit).
/// Limited/narrow range uses the broadcast range (16–235 luma, 16–240 chroma
/// for 8-bit; 64–940 for 10-bit). Use limited range for broadcast/studio
/// content that is already in narrow range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodePixelRange {
    /// Full range (0–255 / 0–1023). Default.
    #[default]
    Full,
    /// Limited/narrow range (16–235 / 64–940). For broadcast/studio content.
    Limited,
}

/// Chroma subsampling for the encoded color image.
///
/// The biggest rate knob after quality itself: 4:2:0 stores chroma at
/// quarter resolution, cutting file size ~25–35 % on photographic
/// content. Keep the 4:4:4 default for text, screen content, line art,
/// or anything with sharp chroma edges.
///
/// 4:2:0 cannot be combined with [`EncodeColorModel::Rgb`] (the
/// identity matrix has no meaningful "chroma" to subsample);
/// [`EncoderConfig::validate`] rejects the pair and the encoder errors
/// at encode time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodeChromaSubsampling {
    /// Full-resolution chroma (4:4:4). Default, and recommended for AVIF.
    #[default]
    Yuv444,
    /// Quarter-resolution chroma (4:2:0). Smaller files on photographic
    /// content; not recommended for text or sharp synthetic edges.
    Yuv420,
}

/// Mastering display metadata for HDR encoding (SMPTE ST 2086).
///
/// Fields are in the **`mdcv` box units** — they are written verbatim into the
/// MP4 `mdcv` box and read back the same way, so they must already be ST-2086
/// scaled (do NOT use 0.16/24.8/18.14 fixed-point — that earlier doc was wrong
/// and produced ~1.31×/39× errors against compliant readers):
/// - chromaticity x/y: CIE 1931 in **0.00002 units** (multiply CIE xy by 50000)
/// - luminance: in **0.0001 cd/m² units** (multiply cd/m² by 10000)
#[derive(Debug, Clone, Copy)]
pub struct MasteringDisplayConfig {
    /// Display primary chromaticities `[(x, y); 3]`, in 0.00002 units
    /// (xy×50000), **in `mdcv` wire order: GREEN, BLUE, RED** (the
    /// SMPTE ST 2086 / HEVC SEI slot order; written verbatim into the box
    /// and read back the same by `MasteringDisplayColourVolume`).
    pub primaries: [(u16, u16); 3],
    /// White point chromaticity `(x, y)`, in 0.00002 units (xy×50000).
    pub white_point: (u16, u16),
    /// Maximum display luminance, in 0.0001 cd/m² units (cd/m²×10000).
    pub max_luminance: u32,
    /// Minimum display luminance, in 0.0001 cd/m² units (cd/m²×10000).
    pub min_luminance: u32,
}

/// Configuration for AVIF encoding
///
/// Uses a builder pattern matching [`crate::DecoderConfig`].
///
/// # Example
///
/// ```
/// use zenavif::EncoderConfig;
///
/// let config = EncoderConfig::new()
///     .quality(80.0)
///     .speed(6);
/// ```
/// AV1 encoder backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// `#[non_exhaustive]`: this enum gains a variant every time a backend is
/// added — `Zenav1Svt` is the most recent — so downstream exhaustive matches need
/// a `_` arm. Marking it now means the NEXT backend is additive instead of
/// another break; the CHANGELOG already queued this break for the `Zenav1Svt`
/// addition, so taking it in the same release costs nothing extra.
#[non_exhaustive]
pub enum Av1Backend {
    /// zenrav1e (rav1e fork) — default, production-proven.
    #[default]
    Zenravif,
    /// zenav1-svt — the retired first draft of the pure-Rust SVT-AV1 port
    /// integration.
    ///
    /// No build can use this: the `encode-svtav1` feature was never
    /// shipped (at draft time the port was early — its streams did not
    /// yet pass decode conformance — and the draft path returned raw AV1
    /// OBUs instead of an AVIF file). [`EncoderConfig::validate`] rejects
    /// it. The variant is retained only for enum compatibility; the
    /// working integration is [`Av1Backend::Zenav1Svt`].
    #[deprecated(
        note = "the encode-svtav1 feature was never shipped and no build can encode with this \
                backend; EncoderConfig::validate() rejects it — use Av1Backend::Zenav1Svt"
    )]
    Svtav1,
    /// zenav1-svt (pure Rust SVT-AV1 port) — EXPERIMENTAL, behind the
    /// `zenav1-svt` cargo feature (default off).
    ///
    /// This is the working successor the [`Av1Backend::Svtav1`] doc
    /// promised. At the pinned imazen/svtav1 rev the port emits
    /// bitstreams **byte-identical to the C SVT-AV1 encoder (v4.2.0)**
    /// across its verified battery — all-preset synthetic bd8, bd10,
    /// real-photo low-preset gates at both depths, partial SBs, SB128
    /// and multi-tile (upstream `rust/STATUS.md`); screen-content low
    /// presets still carry pinned RD near-ties and QP 0 / lossless is
    /// rejected upstream. Streams pass decode conformance under `aomdec`
    /// (2100 conformance cells at the pin) and the payload is muxed into
    /// a real AVIF container in-crate. The zenavif seam's scope stays
    /// deliberately narrow — 8- and 10-bit 4:2:0 stills (10-bit alpha or
    /// grayscale at speed >= 7 only).
    ///
    /// Dimensions, per `svt_rs_dims_error` in `src/encoder_svt_rs.rs`
    /// (which is the gate, and which this paragraph contradicted until
    /// 2026-09-02): multiples of 64 are always accepted; **any other size
    /// is accepted on the colour 4:2:0 path at every speed** — the
    /// partial-superblock floor was removed 2026-08-29; an alpha or
    /// grayscale (Cs400) item at a non-multiple-of-64 size additionally
    /// needs speed >= 5 (SVT preset >= 6) and multiples of 8.
    /// [`EncoderConfig::validate`] rejects the variant when the feature
    /// is off, and rejects configs outside the supported scope when on.
    Zenav1Svt,
    /// zenav1-aom (pure Rust libaom port) — EXPERIMENTAL, behind the
    /// `zenav1-aom-encode` cargo feature (default off).
    ///
    /// **KEY-frame / still scope only.** The backing entry point,
    /// `aom_encode::key_frame::encode_key_frame`, encodes exactly one AV1
    /// KEY frame and returns one temporal unit — there is no inter
    /// prediction and no multi-frame state in it — so
    /// [`encode_animation_rgb8`] and the other animation entry points
    /// refuse this backend by name, as do the 16-bit and alpha entry
    /// points. Within stills this seam wires **8-bit RGB → 4:2:0 BT.601
    /// limited range** and **8-bit grayscale → monochrome (Cs400)**; 4:4:4,
    /// 4:2:2, 10/12-bit, alpha, full pixel range and gain maps are each
    /// refused with a message naming what is unimplemented (see
    /// `src/encoder_aom.rs`).
    ///
    /// Unlike [`Av1Backend::Zenravif`] (where zenravif muxes), this backend
    /// gets raw AV1 OBUs back and muxes the AVIF container in-crate with
    /// `zenavif-serialize` — the same shape as [`Av1Backend::Zenav1Svt`],
    /// and the reason the retired [`Av1Backend::Svtav1`] draft (which
    /// returned raw OBUs) is rejected.
    ///
    /// Note the colour-range asymmetry with the zenav1-svt backend: the
    /// zenav1-aom sequence header pins `color_range = 0`
    /// (`AOM_CR_STUDIO_RANGE`, real aomenc's default), so this backend
    /// converts to **limited** range and signals `full_color_range = false`
    /// in `colr`; zenav1-svt pins full range. Requesting
    /// [`EncodePixelRange::Full`] is refused rather than mis-signalled.
    ///
    /// At the pinned rev the encoder is 186/186 cells byte-identical to real
    /// aomenc across mono/4:2:0/4:2:2/4:4:4, bit depths 8/10/12,
    /// 16×16–512×512, 20 crops including 1×1, all four CDEF ×
    /// loop-restoration combinations, `--cpu-used` 0..=9 and multi-tile, and
    /// its streams decode to the same pixels under both real libaom and the
    /// in-repo decoder. [`EncoderConfig::validate`] rejects the variant when
    /// the feature is off, and rejects configs outside the supported scope
    /// when on.
    Zenav1Aom,
}

impl Av1Backend {
    /// Deprecated spelling of [`Av1Backend::Zenav1Svt`].
    ///
    /// The backend crate was renamed `svtav1-rs` -> `zenav1-svt`, so this
    /// variant name no longer matches anything that exists. Kept as an
    /// associated constant so existing consumers keep compiling in both
    /// expression and pattern position (the enum derives `PartialEq` /
    /// `Eq`, so the constant is structural-match and is usable in a
    /// `match` arm).
    #[deprecated(
        since = "0.1.8",
        note = "renamed to `Av1Backend::Zenav1Svt` to match the zenav1-svt crate; \
                the alias is removed in 0.2"
    )]
    #[allow(non_upper_case_globals)]
    pub const SvtRs: Self = Self::Zenav1Svt;
}

#[derive(Debug, Clone)]
pub struct EncoderConfig {
    pub(crate) backend: Av1Backend,
    pub(crate) quality: f32,
    pub(crate) speed: u8,
    pub(crate) alpha_quality: Option<f32>,
    pub(crate) bit_depth: EncodeBitDepth,
    pub(crate) color_model: EncodeColorModel,
    pub(crate) chroma_subsampling: EncodeChromaSubsampling,
    pub(crate) alpha_color_mode: EncodeAlphaMode,
    pub(crate) threads: Option<usize>,
    pub(crate) exif: Option<Vec<u8>>,
    /// XMP metadata to embed
    pub(crate) xmp: Option<Vec<u8>>,
    /// ICC color profile to embed
    pub(crate) icc_profile: Option<Vec<u8>>,
    /// Image rotation (counter-clockwise degrees: 0, 90, 180, 270)
    pub(crate) rotation: Option<u8>,
    /// Image mirror axis (0 = vertical, 1 = horizontal)
    pub(crate) mirror: Option<u8>,
    /// Content light level (max_cll, max_fall)
    pub(crate) content_light_level: Option<(u16, u16)>,
    /// Mastering display metadata
    pub(crate) mastering_display: Option<MasteringDisplayConfig>,
    /// CICP color primaries code point (ITU-T H.273)
    pub(crate) color_primaries: Option<u8>,
    /// CICP transfer characteristics code point (ITU-T H.273)
    pub(crate) transfer_characteristics: Option<u8>,
    /// CICP matrix coefficients code point (ITU-T H.273)
    pub(crate) matrix_coefficients: Option<u8>,
    /// Pixel range: full (0–255/0–1023) or limited/narrow (16–235/64–940)
    pub(crate) pixel_range: Option<EncodePixelRange>,
    /// Pre-encoded gain map for UltraHDR / ISO 21496-1
    pub(crate) gain_map: Option<GainMapConfig>,
    /// CICP colr for the gain map's alternate rendition (`tmap` item colr):
    /// (primaries, transfer, matrix, full_range)
    pub(crate) gain_map_alt_colr: Option<(u8, u8, u8, bool)>,
    /// ICC profile for the gain map's alternate rendition (`tmap` item
    /// `colr` of type `prof`)
    pub(crate) gain_map_alt_icc: Option<std::vec::Vec<u8>>,
    /// Enable AV1 quantization matrices (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) enable_qm: bool,
    /// Enable variance adaptive quantization (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) enable_vaq: bool,
    /// VAQ strength 0.0–4.0 (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) vaq_strength: f64,
    /// Use Tune::StillImage instead of Tune::Psychovisual (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) tune_still_image: bool,
    /// Mathematically lossless encoding (quantizer=0) (imazen/rav1e fork)
    #[cfg(feature = "encode-imazen")]
    pub(crate) lossless: bool,
    /// Segmentation boost (1.0 = off, >1.0 widens per-segment QP deltas)
    #[cfg(feature = "encode-imazen")]
    pub(crate) seg_boost: Option<f64>,
    /// Override CDEF on/off (None = use speed preset default)
    #[cfg(feature = "encode-imazen")]
    pub(crate) override_cdef: Option<bool>,
    /// Override RDO transform-decision search
    #[cfg(feature = "encode-imazen")]
    pub(crate) override_rdo_tx_decision: Option<bool>,
    /// Override SGR complexity (true = Full / 16 sets, false = Reduced / 8 sets)
    #[cfg(feature = "encode-imazen")]
    pub(crate) override_sgr_full: Option<bool>,
    /// Override LRU on skip (loop restoration on no-coeff blocks)
    #[cfg(feature = "encode-imazen")]
    pub(crate) override_lru_on_skip: Option<bool>,
    /// Override segmentation Complex (k-means) vs Simple
    #[cfg(feature = "encode-imazen")]
    pub(crate) override_segmentation_complex: Option<bool>,
    /// Override bottom-up partition search vs top-down
    #[cfg(feature = "encode-imazen")]
    pub(crate) override_encode_bottomup: Option<bool>,
    /// Override partition block-size range (min, max) in pixels
    #[cfg(feature = "encode-imazen")]
    #[allow(dead_code)] // release-gated zenravif expert passthrough mirror (dep-bump wiring)
    pub(crate) override_partition_range: Option<(u8, u8)>,
    /// Override prediction modes (true = ComplexAll, false = Simple)
    #[cfg(feature = "encode-imazen")]
    #[allow(dead_code)] // release-gated zenravif expert passthrough mirror (dep-bump wiring)
    pub(crate) override_complex_prediction_modes: Option<bool>,
    /// Override loop restoration filter on/off
    #[cfg(feature = "encode-imazen")]
    #[allow(dead_code)] // release-gated zenravif expert passthrough mirror (dep-bump wiring)
    pub(crate) override_lrf: Option<bool>,
    /// Override fast vs full deblock filter search
    #[cfg(feature = "encode-imazen")]
    #[allow(dead_code)] // release-gated zenravif expert passthrough mirror (dep-bump wiring)
    pub(crate) override_fast_deblock: Option<bool>,
    /// Override trellis quantization (Viterbi DP)
    #[cfg(feature = "encode-imazen")]
    pub(crate) trellis: Option<bool>,
    /// Palette-mode preference for the AV1 screen-content palette tool
    /// (imazen/zenrav1e fork). `None` = encoder default. RELEASE-GATED:
    /// stored + introspectable today, applied at the zenrav1e-past-0.1.4
    /// dep bump — see `src/palette_gate.rs` module docs.
    #[cfg(feature = "encode-imazen")]
    pub(crate) palette_preference: Option<crate::palette_gate::PalettePreference>,
    /// Per-image fast-tier search budgets (tx + partition heads,
    /// FAST_TIER_PARITY P2). `None` = the speed table's globals.
    /// RELEASE-GATED like `palette_preference`: stored + introspectable
    /// today, applied at the zenrav1e-past-0.1.4 dep bump (needs the
    /// zenravif expert tx/prune passthroughs — see `src/fast_heads.rs`).
    #[cfg(feature = "encode-imazen")]
    pub(crate) fast_tier_budgets: Option<crate::fast_heads::FastTierBudgets>,
    /// Per-64×64-superblock AC quantizer scale map for the color encode
    /// (the diffmap-guided second passes set this internally — see
    /// [`crate::two_pass`] and [`crate::two_pass_zensim`]). Forwarded
    /// through zenravif's expert `sb_q_scale` passthrough; release-gated
    /// there behind `zenravif::FRAME_HINTS_LIVE`.
    #[cfg(any(feature = "two-pass-butteraugli", feature = "two-pass-zensim"))]
    pub(crate) sb_q_scale: Option<Box<[f32]>>,
    /// Still-image knobs for the [`Av1Backend::Zenav1Svt`] backend
    /// (`expert::SvtParams`). The [`Default`] is exactly what the
    /// svt-rs seam configures with no params set, so the field is inert
    /// unless a caller opts in via `EncoderConfig::with_svt_params`
    /// (`__expert` only). Ignored by every other backend (and by the sweep
    /// fingerprint on those backends, so an inert spelling cannot mint a
    /// duplicate cell).
    #[cfg(any(feature = "zenav1-svt", feature = "__expert"))]
    pub(crate) svt: crate::svt_params::SvtParams,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            backend: Av1Backend::default(),
            quality: 75.0,
            speed: 4,
            alpha_quality: None,
            bit_depth: EncodeBitDepth::default(),
            color_model: EncodeColorModel::default(),
            chroma_subsampling: EncodeChromaSubsampling::default(),
            alpha_color_mode: EncodeAlphaMode::default(),
            threads: None,
            exif: None,
            xmp: None,
            icc_profile: None,
            rotation: None,
            mirror: None,
            content_light_level: None,
            mastering_display: None,
            color_primaries: None,
            transfer_characteristics: None,
            matrix_coefficients: None,
            pixel_range: None,
            gain_map: None,
            gain_map_alt_colr: None,
            gain_map_alt_icc: None,
            #[cfg(feature = "encode-imazen")]
            enable_qm: true,
            #[cfg(feature = "encode-imazen")]
            enable_vaq: false,
            #[cfg(feature = "encode-imazen")]
            vaq_strength: 1.0,
            #[cfg(feature = "encode-imazen")]
            tune_still_image: false,
            #[cfg(feature = "encode-imazen")]
            lossless: false,
            #[cfg(feature = "encode-imazen")]
            seg_boost: None,
            #[cfg(feature = "encode-imazen")]
            override_cdef: None,
            #[cfg(feature = "encode-imazen")]
            override_rdo_tx_decision: None,
            #[cfg(feature = "encode-imazen")]
            override_sgr_full: None,
            #[cfg(feature = "encode-imazen")]
            override_lru_on_skip: None,
            #[cfg(feature = "encode-imazen")]
            override_segmentation_complex: None,
            #[cfg(feature = "encode-imazen")]
            override_encode_bottomup: None,
            #[cfg(feature = "encode-imazen")]
            override_partition_range: None,
            #[cfg(feature = "encode-imazen")]
            override_complex_prediction_modes: None,
            #[cfg(feature = "encode-imazen")]
            override_lrf: None,
            #[cfg(feature = "encode-imazen")]
            override_fast_deblock: None,
            #[cfg(feature = "encode-imazen")]
            trellis: None,
            #[cfg(feature = "encode-imazen")]
            palette_preference: None,
            #[cfg(feature = "encode-imazen")]
            fast_tier_budgets: None,
            #[cfg(any(feature = "two-pass-butteraugli", feature = "two-pass-zensim"))]
            sb_q_scale: None,
            #[cfg(any(feature = "zenav1-svt", feature = "__expert"))]
            svt: crate::svt_params::SvtParams::default(),
        }
    }
}

impl EncoderConfig {
    /// Create a new encoder configuration with default settings
    ///
    /// Defaults: quality 75, speed 4, auto bit depth, YCbCr color model
    pub fn new() -> Self {
        Self::default()
    }

    /// Select the AV1 encoder backend.
    ///
    /// `Av1Backend::Zenravif` (the default) is always available;
    /// `Av1Backend::Zenav1Svt` needs the `zenav1-svt` cargo feature (and
    /// supports 8-bit 4:2:0 stills only — see the variant docs); the
    /// deprecated `Svtav1` variant is rejected by
    /// [`validate`](Self::validate).
    pub fn backend(mut self, backend: Av1Backend) -> Self {
        self.backend = backend;
        self
    }

    /// Set encoding quality (1.0 = worst, 100.0 = best/lossless)
    pub fn quality(mut self, quality: f32) -> Self {
        self.quality = quality;
        self
    }

    /// Read the currently configured quality.
    pub fn quality_value(&self) -> f32 {
        self.quality
    }

    /// Set the per-64×64-superblock AC quantizer scale map for the color
    /// encode (`1.0` = neutral, frame superblock raster order, exactly
    /// `ceil(width/64) * ceil(height/64)` entries).
    ///
    /// Each superblock's quantizer is scaled by its entry and re-quantized
    /// to the next AV1 quantizer index at or above the result, so entries
    /// below 1.0 spend more bits there and entries above 1.0 spend fewer.
    /// The encoder clamps each entry to `[0.25, 4.0]`.
    ///
    /// Three behaviours worth knowing before relying on this, all of them
    /// the encoder's and none of them reported back through this API:
    ///
    /// * A map whose length does not match the superblock grid is
    ///   **silently ignored**, not partially applied — so a wrong grid
    ///   looks exactly like the map having no effect.
    /// * An all-`1.0` map is **inert**: it does not switch on delta-q
    ///   coding, so it is byte-identical to passing no map.
    /// * Any non-neutral entry switches delta-q coding on for the whole
    ///   frame — which also **disables segmentation**. That is not a small
    ///   perturbation: measured over 54 cells
    ///   (`benchmarks/zensim_hint_probe_2026-08-06`), merely activating the
    ///   channel with every superblock's delta quantizing to zero moves the
    ///   zensim score by a median **+1.10** (p90 |Δ| 4.49) and bytes by
    ///   +2.9% median / +21% max, at an unchanged quantizer. A very sparse
    ///   map is not a very small one.
    /// * **Per-superblock deltas are coded at a RESOLUTION of 1/2/4/8
    ///   quantizer indices**, chosen from the frame's base quantizer
    ///   (zenrav1e `variance_boost_delta_q_res_log2`: res 1 below 80, 2 to
    ///   119, 4 to 159, 8 above). A scale whose implied move is smaller
    ///   than that resolution **quantizes to zero** — the map still
    ///   activates delta-q, but nothing moves, so the encode is identical
    ///   for every such map and reads exactly like the content being
    ///   ignored. A 1.5% scale is below the resolution at ~76% of
    ///   quantizer indices. Size the scale against the resolution, and
    ///   confirm bytes actually changed before believing any null from
    ///   this channel.
    ///
    /// [`crate::two_pass_zensim::sb_q_scale_from_diffmap`] builds a
    /// correctly shaped map from a zensim diffmap.
    ///
    /// **RELEASE-GATED downstream**: forwarded through zenravif's expert
    /// `sb_q_scale` passthrough, which is inert while
    /// [`crate::FRAME_HINTS_LIVE`]` == false` (registry `zenrav1e` 0.1.4
    /// has no `FrameHints`; the input lands on zenrav1e master past
    /// 0.1.4). While the gate is off, supplied maps are accepted but not
    /// applied — encodes stay byte-identical — so closed-loop callers
    /// MUST check [`crate::FRAME_HINTS_LIVE`] or probe engagement
    /// (encode with two different maps, assert the bitstreams differ)
    /// and fail honestly instead of silently steering nothing.
    #[cfg(any(feature = "two-pass-butteraugli", feature = "two-pass-zensim"))]
    #[must_use]
    pub fn with_sb_q_scale(mut self, map: Option<Box<[f32]>>) -> Self {
        self.sb_q_scale = map;
        self
    }

    /// Read the currently configured speed (1..=10).
    pub fn speed_value(&self) -> u8 {
        self.speed
    }

    /// Set encoding speed (1 = slowest/best, 10 = fastest/worst)
    ///
    /// Lossless encodes currently clamp the running preset into the
    /// [6, 8] band: on the zenrav1e release the dep chain resolves
    /// today, speeds 1-4 measurably produce *larger* lossless files at
    /// 4.6-11x the encode time and speed 10 larger still (imazen/
    /// zenavif#8; the clamp is removed when the fixed zenrav1e — where
    /// slower is monotonically smaller — is released and adopted).
    /// Lossy encodes always run the configured speed.
    pub fn speed(mut self, speed: u8) -> Self {
        self.speed = speed;
        self
    }

    /// Set separate quality for the alpha channel
    ///
    /// If not set, the alpha channel is encoded at the same quality as
    /// color. (zenavif forwards this explicitly; zenravif's own default
    /// would pin alpha to the quality-80 equivalent instead.)
    pub fn alpha_quality(mut self, quality: f32) -> Self {
        self.alpha_quality = Some(quality);
        self
    }

    /// Set the output bit depth
    pub fn bit_depth(mut self, depth: EncodeBitDepth) -> Self {
        self.bit_depth = depth;
        self
    }

    /// Set the internal color model
    ///
    /// YCbCr (default) produces smaller files. RGB may be better for lossless.
    pub fn color_model(mut self, model: EncodeColorModel) -> Self {
        self.color_model = model;
        self
    }

    /// Set chroma subsampling for the encoded color image.
    ///
    /// Default is [`EncodeChromaSubsampling::Yuv444`] (full-resolution
    /// chroma). 4:2:0 trades chroma resolution for ~25–35 % smaller
    /// files on photographic content. Incompatible with
    /// [`EncodeColorModel::Rgb`].
    pub fn chroma_subsampling(mut self, subsampling: EncodeChromaSubsampling) -> Self {
        self.chroma_subsampling = subsampling;
        self
    }

    /// Set the alpha channel handling mode
    pub fn alpha_color_mode(mut self, mode: EncodeAlphaMode) -> Self {
        self.alpha_color_mode = mode;
        self
    }

    /// Set the number of threads
    ///
    /// `None` uses the rayon default. `Some(1)` for single-threaded.
    pub fn threads(mut self, threads: Option<usize>) -> Self {
        self.threads = threads;
        self
    }

    /// Embed EXIF metadata in the output
    pub fn exif(mut self, exif_data: Vec<u8>) -> Self {
        self.exif = Some(exif_data);
        self
    }

    /// Embed XMP metadata in the output
    pub fn xmp(mut self, xmp_data: Vec<u8>) -> Self {
        self.xmp = Some(xmp_data);
        self
    }

    /// Embed an ICC color profile in the output
    pub fn icc_profile(mut self, profile: Vec<u8>) -> Self {
        self.icc_profile = Some(profile);
        self
    }

    /// Set image rotation (counter-clockwise degrees: 0, 90, 180, 270)
    pub fn rotation(mut self, angle: u8) -> Self {
        self.rotation = Some(angle);
        self
    }

    /// Set image mirror axis (0 = vertical/left-right, 1 = horizontal/top-bottom)
    pub fn mirror(mut self, axis: u8) -> Self {
        self.mirror = Some(axis);
        self
    }

    /// Set content light level metadata (HDR)
    ///
    /// * `max_cll` - Maximum content light level (cd/m²)
    /// * `max_fall` - Maximum frame-average light level (cd/m²)
    pub fn content_light_level(mut self, max_cll: u16, max_fall: u16) -> Self {
        self.content_light_level = Some((max_cll, max_fall));
        self
    }

    /// Set mastering display metadata (HDR, SMPTE ST 2086)
    pub fn mastering_display(mut self, md: MasteringDisplayConfig) -> Self {
        self.mastering_display = Some(md);
        self
    }

    /// Set CICP color primaries code point (ITU-T H.273).
    ///
    /// Common values: 1 = BT.709/sRGB, 9 = BT.2020, 12 = Display P3.
    pub fn color_primaries(mut self, cp: u8) -> Self {
        self.color_primaries = Some(cp);
        self
    }

    /// Set CICP transfer characteristics code point (ITU-T H.273).
    ///
    /// Common values: 1 = BT.709, 13 = sRGB, 16 = PQ (HDR10), 18 = HLG.
    pub fn transfer_characteristics(mut self, tc: u8) -> Self {
        self.transfer_characteristics = Some(tc);
        self
    }

    /// Set CICP matrix coefficients code point (ITU-T H.273).
    ///
    /// Common values: 0 = Identity/RGB, 1 = BT.709, 6 = BT.601, 9 = BT.2020.
    ///
    /// **Backend note:** no available backend consults this field. The
    /// zenravif backend derives the matrix it actually signals from
    /// [`color_model`](Self::color_model) (YCbCr → BT.601, RGB →
    /// Identity); the experimental `Zenav1Svt` backend pins BT.601 (its
    /// only supported model is 4:2:0 YCbCr); the field's only reader
    /// was the deprecated svtav1 path. It is retained for config
    /// coherence — the zencodec layer mirrors the source CICP triple
    /// onto the config — and for future backends. Use
    /// [`resolve_plan`](Self::resolve_plan) to see the matrix an
    /// encode will really carry
    /// ([`EncodePlan::matrix_coefficients_cicp`](crate::EncodePlan::matrix_coefficients_cicp)).
    pub fn matrix_coefficients(mut self, mc: u8) -> Self {
        self.matrix_coefficients = Some(mc);
        self
    }

    /// Set pixel value range for AV1 encoding.
    ///
    /// Default is full range. Use limited/narrow range for broadcast content
    /// that already uses studio levels (16–235 for 8-bit, 64–940 for 10-bit).
    pub fn pixel_range(mut self, range: EncodePixelRange) -> Self {
        self.pixel_range = Some(range);
        self
    }

    /// Embed a pre-encoded gain map for UltraHDR / ISO 21496-1.
    ///
    /// The gain map enables SDR/HDR tone mapping: the primary image is the SDR
    /// base, and the gain map allows reconstruction of the HDR rendition.
    ///
    /// * `av1_data` - Pre-encoded AV1 bitstream of the gain map image.
    /// * `width` - Width of the gain map image in pixels.
    /// * `height` - Height of the gain map image in pixels.
    /// * `bit_depth` - Bit depth of the gain map AV1 data (typically 8 or 10).
    /// * `metadata` - ISO 21496-1 binary metadata blob.
    pub fn with_gain_map(
        mut self,
        av1_data: Vec<u8>,
        width: u32,
        height: u32,
        bit_depth: u8,
        metadata: Vec<u8>,
    ) -> Self {
        self.gain_map = Some(GainMapConfig {
            av1_data,
            width,
            height,
            bit_depth,
            metadata,
        });
        self
    }

    /// Set the CICP color description of the gain map's **alternate
    /// rendition** — written as a `colr` (nclx) property on the `tmap`
    /// item, telling readers what color space the fully-boosted rendition
    /// targets (e.g. BT.2020 + PQ for an SDR base carrying an HDR map).
    ///
    /// Code points are raw ITU-T H.273 values. Only meaningful together
    /// with [`EncoderConfig::with_gain_map`]. Values outside the muxer's
    /// supported set fail at encode time with an honest error rather than
    /// being silently dropped.
    pub fn with_gain_map_alt_color(
        mut self,
        color_primaries: u8,
        transfer_characteristics: u8,
        matrix_coefficients: u8,
        full_range: bool,
    ) -> Self {
        self.gain_map_alt_colr = Some((
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            full_range,
        ));
        self
    }

    /// Set an ICC profile as the color description of the gain map's
    /// **alternate rendition** — written as a `colr` box of type `prof` on
    /// the `tmap` item. May be combined with
    /// [`EncoderConfig::with_gain_map_alt_color`] (ISOBMFF permits one
    /// `colr` of each type per item); libavif interop vectors exist with an
    /// ICC-form tmap colr. Only meaningful together with
    /// [`EncoderConfig::with_gain_map`].
    pub fn with_gain_map_alt_icc(mut self, icc_profile: Vec<u8>) -> Self {
        self.gain_map_alt_icc = Some(icc_profile);
        self
    }

    /// Enable/disable AV1 quantization matrices (imazen/rav1e fork).
    ///
    /// QM applies frequency-dependent quantization weights for ~10% BD-rate improvement.
    /// Default: enabled.
    #[cfg(feature = "encode-imazen")]
    pub fn with_qm(mut self, enable: bool) -> Self {
        self.enable_qm = enable;
        self
    }

    /// Enable/disable variance adaptive quantization (imazen/rav1e fork).
    ///
    /// Allocates more bits to smooth regions, fewer to textured regions.
    /// Default: enabled, strength 0.5.
    #[cfg(feature = "encode-imazen")]
    pub fn with_vaq(mut self, enable: bool, strength: f64) -> Self {
        self.enable_vaq = enable;
        self.vaq_strength = strength;
        self
    }

    /// Enable/disable still-image tuning (imazen/rav1e fork).
    ///
    /// Uses perceptual distortion metric with reduced CDEF/deblock for detail preservation.
    /// Default: enabled.
    #[cfg(feature = "encode-imazen")]
    pub fn with_still_image_tuning(mut self, enable: bool) -> Self {
        self.tune_still_image = enable;
        self
    }

    /// Enable/disable mathematically lossless encoding (imazen/rav1e fork).
    ///
    /// Sets quantizer to 0. Default: disabled.
    ///
    /// Note: the zenrav1e release the dep chain currently resolves does
    /// not reach qindex 0 (zenrav1e#9 — output is near-lossless, |delta|
    /// ≤ 2 scatter), and its slow lossless speeds produce larger files;
    /// lossless encodes therefore clamp their speed preset into the
    /// [6, 8] band until the fixed zenrav1e releases (imazen/zenavif#8;
    /// see [`EncoderConfig::speed`]).
    #[cfg(feature = "encode-imazen")]
    pub fn with_lossless(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        self
    }

    /// Convenience preset: optimal still image settings (imazen/rav1e fork).
    ///
    /// Enables QM, VAQ (strength 0.5), and still-image tuning.
    #[cfg(feature = "encode-imazen")]
    pub fn still_image_preset(self) -> Self {
        self.with_qm(true)
            .with_vaq(true, 0.5)
            .with_still_image_tuning(true)
    }

    /// Set segmentation boost power. `1.0` = off, `>1.0` widens per-segment
    /// QP deltas. None = leave at zenravif default (1.0).
    #[cfg(feature = "encode-imazen")]
    pub fn with_seg_boost(mut self, boost: Option<f64>) -> Self {
        self.seg_boost = boost;
        self
    }

    /// Override CDEF on/off (None = use speed preset default).
    #[cfg(feature = "encode-imazen")]
    pub fn with_cdef(mut self, enable: Option<bool>) -> Self {
        self.override_cdef = enable;
        self
    }

    /// Override RDO transform-decision search at speed ≥ 6.
    ///
    /// `None` (default) follows the speed preset: speed 6+ disables RDO TX
    /// decision entirely (intra blocks use DCT-DCT only). `Some(true)` forces
    /// a full per-block TX-type search; `Some(false)` keeps it disabled.
    ///
    /// **When to flip on:** quality-priority still-image encodes where the
    /// 2-3× encode time cost is acceptable. Measured trade on a 63-image
    /// stills corpus (CID22, speed 6, with QM on):
    ///
    /// | Config | Mean BD-Rate vs upstream | Encode time vs QM-only |
    /// |---|---|---|
    /// | `with_qm(true)` (default) | −10.1 % | 1.0× |
    /// | `with_qm(true).with_rdo_tx_decision(Some(true))` | −10.3 % | ~3.0× |
    ///
    /// The marginal BD-rate gain over QM-only is small (~0.2 %), but on
    /// individual images it ranges up to −31 %. Recommended only for one-shot
    /// archival encodes, not bulk web pipelines.
    #[cfg(feature = "encode-imazen")]
    pub fn with_rdo_tx_decision(mut self, enable: Option<bool>) -> Self {
        self.override_rdo_tx_decision = enable;
        self
    }

    /// Override SGR self-guided restoration to Full (16 parameter sets) vs
    /// Reduced (8 sets at speed ≥5). None = use speed preset.
    #[cfg(feature = "encode-imazen")]
    pub fn with_sgr_full(mut self, enable: Option<bool>) -> Self {
        self.override_sgr_full = enable;
        self
    }

    /// Override searching loop restoration on skip blocks. None = preset.
    #[cfg(feature = "encode-imazen")]
    pub fn with_lru_on_skip(mut self, enable: Option<bool>) -> Self {
        self.override_lru_on_skip = enable;
        self
    }

    /// Override segmentation Complex (k-means) vs Simple. None = preset.
    #[cfg(feature = "encode-imazen")]
    pub fn with_segmentation_complex(mut self, enable: Option<bool>) -> Self {
        self.override_segmentation_complex = enable;
        self
    }

    /// Override bottom-up partition search (vs top-down at speed ≥4).
    /// None = preset.
    #[cfg(feature = "encode-imazen")]
    pub fn with_encode_bottomup(mut self, enable: Option<bool>) -> Self {
        self.override_encode_bottomup = enable;
        self
    }

    /// Apply expert-only [`crate::expert::InternalParams`].
    ///
    /// **Unstable surface** — may change in any patch release; see
    /// [`crate::expert`] module docs for the contract. Each `Some(_)`
    /// field overrides a speed-preset default; each `None` leaves the
    /// preset's value untouched. Calling this multiple times overwrites
    /// previously-set fields wholesale (the struct is the unit of
    /// configuration, not the individual fields).
    /// Apply the expert-only `expert::SvtParams` still-image knobs
    /// for the [`Av1Backend::Zenav1Svt`] backend.
    ///
    /// **Unstable surface** — see `crate::expert`. The struct is the unit
    /// of configuration: calling this replaces every field wholesale. The
    /// [`Default`] value is what the seam configures anyway, so passing it
    /// is a no-op.
    ///
    /// Other backends ignore these knobs entirely; the sweep planner's
    /// fingerprint ignores them too on those backends, so an inert spelling
    /// cannot mint a duplicate cell.
    #[cfg(feature = "__expert")]
    #[must_use]
    pub fn with_svt_params(mut self, params: crate::expert::SvtParams) -> Self {
        self.svt = params;
        self
    }

    /// The still-image knobs the [`Av1Backend::Zenav1Svt`] encode will run,
    /// after `SvtParams::clamped`.
    #[cfg(feature = "__expert")]
    #[must_use]
    pub fn svt_params(&self) -> crate::expert::SvtParams {
        self.svt_params_resolved()
    }

    /// The clamped still-image knobs the svt-rs seam actually applies.
    ///
    /// Always compiled: `apply_svt_params` in `src/encoder_svt_rs.rs` runs on
    /// the plain `zenav1-svt` path, where `__expert` (and therefore the
    /// public `svt_params()` above) is off.
    #[cfg(any(feature = "zenav1-svt", feature = "__expert"))]
    pub(crate) fn svt_params_resolved(&self) -> crate::svt_params::SvtParams {
        self.svt.clamped()
    }

    #[cfg(feature = "__expert")]
    pub fn with_internal_params(mut self, params: crate::expert::InternalParams) -> Self {
        self.override_partition_range = params.partition_range;
        self.override_complex_prediction_modes = params.complex_prediction_modes;
        self.override_lrf = params.lrf;
        self.override_fast_deblock = params.fast_deblock;
        self
    }

    /// Override trellis quantization (Viterbi DP coefficient optimization).
    /// None = leave at zenravif default (off).
    #[cfg(feature = "encode-imazen")]
    pub fn with_trellis(mut self, enable: Option<bool>) -> Self {
        self.trellis = enable;
        self
    }

    /// Palette-mode preference for the AV1 screen-content palette tool.
    /// `None` = encoder default. Set automatically by
    /// [`Self::auto_tune`] via the deterministic
    /// [`crate::palette_gate::palette_gate`] descriptor rule, or manually.
    ///
    /// **RELEASE-GATED**: registry `zenrav1e` 0.1.4 has no palette tool, so
    /// the stored preference is not yet forwarded to the encoder — the
    /// forwarding line in `build_ravif_encoder` is commented until the
    /// zenravif → zenrav1e dep chain bumps past 0.1.4 (see
    /// `src/palette_gate.rs` docs + the CLAUDE.md dep-bump checklist).
    #[cfg(feature = "encode-imazen")]
    pub fn with_palette_preference(
        mut self,
        pref: Option<crate::palette_gate::PalettePreference>,
    ) -> Self {
        self.palette_preference = pref;
        self
    }

    /// The stored palette-mode preference (see [`Self::with_palette_preference`]).
    #[cfg(feature = "encode-imazen")]
    pub fn palette_preference_value(&self) -> Option<crate::palette_gate::PalettePreference> {
        self.palette_preference
    }

    /// Per-image fast-tier search budgets (tx + partition heads). `None` =
    /// keep the speed table's global configuration. Set automatically by
    /// [`Self::auto_tune`] via the [`crate::fast_heads`] descriptor rules,
    /// or manually.
    ///
    /// **RELEASE-GATED**: registry `zenrav1e` 0.1.4 has none of the
    /// underlying knobs; the stored budgets are not yet forwarded to the
    /// encoder (see `src/fast_heads.rs` module docs + the CLAUDE.md
    /// dep-bump checklist).
    #[cfg(feature = "encode-imazen")]
    pub fn with_fast_tier_budgets(
        mut self,
        budgets: Option<crate::fast_heads::FastTierBudgets>,
    ) -> Self {
        self.fast_tier_budgets = budgets;
        self
    }

    /// The stored fast-tier budgets (see [`Self::with_fast_tier_budgets`]).
    #[cfg(feature = "encode-imazen")]
    pub fn fast_tier_budgets_value(&self) -> Option<crate::fast_heads::FastTierBudgets> {
        self.fast_tier_budgets
    }

    /// The stored per-superblock quantizer scale map (see
    /// [`Self::with_sb_q_scale`]).
    #[cfg(feature = "two-pass-butteraugli")]
    pub fn sb_q_scale_value(&self) -> Option<&[f32]> {
        self.sb_q_scale.as_deref()
    }
}

/// Convert a CICP color primaries code point to the ravif enum.
fn cicp_to_color_primaries(cp: u8) -> ravif::ColorPrimaries {
    match cp {
        1 => ravif::ColorPrimaries::BT709,
        4 => ravif::ColorPrimaries::BT470M,
        5 => ravif::ColorPrimaries::BT470BG,
        6 => ravif::ColorPrimaries::BT601,
        7 => ravif::ColorPrimaries::SMPTE240,
        8 => ravif::ColorPrimaries::GenericFilm,
        9 => ravif::ColorPrimaries::BT2020,
        10 => ravif::ColorPrimaries::XYZ,
        11 => ravif::ColorPrimaries::SMPTE431,
        12 => ravif::ColorPrimaries::SMPTE432,
        22 => ravif::ColorPrimaries::EBU3213,
        _ => ravif::ColorPrimaries::Unspecified,
    }
}

/// Convert a CICP transfer characteristics code point to the ravif enum.
fn cicp_to_transfer_characteristics(tc: u8) -> ravif::TransferCharacteristics {
    match tc {
        1 => ravif::TransferCharacteristics::BT709,
        4 => ravif::TransferCharacteristics::BT470M,
        5 => ravif::TransferCharacteristics::BT470BG,
        6 => ravif::TransferCharacteristics::BT601,
        7 => ravif::TransferCharacteristics::SMPTE240,
        8 => ravif::TransferCharacteristics::Linear,
        9 => ravif::TransferCharacteristics::Log100,
        10 => ravif::TransferCharacteristics::Log100Sqrt10,
        11 => ravif::TransferCharacteristics::IEC61966,
        12 => ravif::TransferCharacteristics::BT1361,
        13 => ravif::TransferCharacteristics::SRGB,
        14 => ravif::TransferCharacteristics::BT2020_10Bit,
        15 => ravif::TransferCharacteristics::BT2020_12Bit,
        16 => ravif::TransferCharacteristics::SMPTE2084,
        18 => ravif::TransferCharacteristics::HLG,
        _ => ravif::TransferCharacteristics::Unspecified,
    }
}

/// Resolve `EncodeBitDepth::Auto` based on whether the input is 8-bit or 16-bit.
///
/// Shared by `build_ravif_encoder` and `EncoderConfig::resolve_plan` so
/// the introspected plan cannot drift from what the encoder does.
pub(crate) fn resolve_bit_depth(
    configured: EncodeBitDepth,
    input_is_16bit: bool,
) -> ravif::BitDepth {
    match configured {
        EncodeBitDepth::Eight => ravif::BitDepth::Eight,
        EncodeBitDepth::Ten => ravif::BitDepth::Ten,
        EncodeBitDepth::Auto => {
            if input_is_16bit {
                ravif::BitDepth::Ten
            } else {
                ravif::BitDepth::Eight
            }
        }
    }
}

/// Effective alpha quality: unset follows the color quality, per the
/// [`EncoderConfig::alpha_quality`] contract.
///
/// Shared by `build_ravif_encoder` and `EncoderConfig::resolve_plan`.
/// zenravif's own default would otherwise leave the alpha quantizer at
/// its quality-80 equivalent regardless of the configured color quality
/// (zenravif 0.1.3 `av1encoder.rs`: `Default` sets `alpha_quantizer:
/// quality_to_quantizer(80.)` and `with_quality` never touches it), so
/// this must be forwarded explicitly.
pub(crate) fn effective_alpha_quality(config: &EncoderConfig) -> f32 {
    config.alpha_quality.unwrap_or(config.quality)
}

/// Effective QM after the lossless gate: quantization matrices are
/// meaningless at quantizer 0, so lossless forces them off.
///
/// Shared by `build_ravif_encoder` and `EncoderConfig::resolve_plan`.
#[cfg(feature = "encode-imazen")]
pub(crate) fn effective_qm(config: &EncoderConfig) -> bool {
    config.enable_qm && !config.lossless
}

/// Reject the experimental in-crate-muxing backends on entry points they do
/// not implement.
///
/// The zenav1-svt backend covers still encodes only (RGB/RGBA 8- and
/// 16-bit → 4:2:0 at 8 or 10 bits, plus grayscale); the zenav1-aom backend
/// covers a narrower still slice still (8-bit RGB → 4:2:0 and 8-bit
/// grayscale). Every other entry point — and every entry point when the
/// respective feature is off — fails honestly instead of silently serving
/// the request with zenravif. (The deprecated `Svtav1` variant keeps its
/// historical behavior: rejected by `validate()`, silently zenravif-served
/// otherwise.)
fn reject_svt_rs_backend(config: &EncoderConfig, entry: &'static str) -> Result<()> {
    if config.backend == Av1Backend::Zenav1Svt {
        return Err(at!(Error::Encode(format!(
            "Av1Backend::Zenav1Svt does not support {entry} \
             (RGB/RGBA/grayscale still encodes only); \
             use Av1Backend::Zenravif"
        ))));
    }
    reject_aom_backend(config, entry)
}

/// Reject `Av1Backend::Zenav1Aom` on entry points it does not implement.
///
/// `aom_encode::key_frame::encode_key_frame` is a ONE-KEY-FRAME entry point:
/// no inter prediction, no reference management, no multi-frame state. So
/// animation is not "not wired yet at this seam", it is absent from the
/// encoder — and alpha, 16-bit input and 10/12-bit output are the
/// seam-level gaps. Each is named in the message rather than silently
/// served by zenravif.
pub(crate) fn reject_aom_backend(config: &EncoderConfig, entry: &'static str) -> Result<()> {
    if config.backend == Av1Backend::Zenav1Aom {
        return Err(at!(Error::Encode(format!(
            "Av1Backend::Zenav1Aom does not support {entry}: it encodes ONE AV1 \
             KEY frame (8-bit RGB → 4:2:0 and 8-bit grayscale stills only — no \
             animation, no alpha auxiliary item, no 16-bit input, no 10/12-bit \
             output); use Av1Backend::Zenravif"
        ))));
    }
    Ok(())
}

fn build_ravif_encoder(
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
    input_is_16bit: bool,
) -> Result<ravif::Encoder<'_>> {
    let mut enc = ravif::Encoder::new()
        .with_quality(config.quality)
        // `speed_effective`, not `speed`: lossless clamps into the
        // registry-era band (imazen/zenavif#8; see encode_plan.rs).
        .with_speed(config.speed_effective())
        .with_bit_depth(resolve_bit_depth(config.bit_depth, input_is_16bit))
        .with_internal_color_model(match config.color_model {
            EncodeColorModel::YCbCr => ravif::ColorModel::YCbCr,
            EncodeColorModel::Rgb => ravif::ColorModel::RGB,
        })
        .with_chroma_subsampling(match config.chroma_subsampling {
            EncodeChromaSubsampling::Yuv444 => ravif::ChromaSubsampling::Yuv444,
            EncodeChromaSubsampling::Yuv420 => ravif::ChromaSubsampling::Yuv420,
        })
        .with_alpha_color_mode(match config.alpha_color_mode {
            EncodeAlphaMode::UnassociatedClean => ravif::AlphaColorMode::UnassociatedClean,
            EncodeAlphaMode::UnassociatedDirty => ravif::AlphaColorMode::UnassociatedDirty,
            EncodeAlphaMode::Premultiplied => ravif::AlphaColorMode::Premultiplied,
        })
        .with_num_threads(config.threads)
        // Always forwarded: zenravif's built-in default pins the alpha
        // quantizer to the quality-80 equivalent instead of following
        // the color quality (see `effective_alpha_quality`).
        .with_alpha_quality(effective_alpha_quality(config));
    if let Some(ref exif_data) = config.exif {
        enc = enc.with_exif(exif_data.as_slice());
    }
    if let Some(ref xmp_data) = config.xmp {
        enc = enc.with_xmp(xmp_data.clone());
    }
    if let Some(ref icc) = config.icc_profile {
        enc = enc.with_icc_profile(icc.clone());
    }
    if let Some(angle) = config.rotation {
        enc = enc.with_rotation(angle);
    }
    if let Some(axis) = config.mirror {
        enc = enc.with_mirror(axis);
    }
    if let Some((max_cll, max_fall)) = config.content_light_level {
        enc = enc.with_content_light(ravif::ContentLight {
            max_content_light_level: max_cll,
            max_frame_average_light_level: max_fall,
        });
    }
    if let Some(md) = config.mastering_display {
        enc = enc.with_mastering_display(ravif::MasteringDisplay {
            primaries: [
                ravif::ChromaticityPoint {
                    x: md.primaries[0].0,
                    y: md.primaries[0].1,
                },
                ravif::ChromaticityPoint {
                    x: md.primaries[1].0,
                    y: md.primaries[1].1,
                },
                ravif::ChromaticityPoint {
                    x: md.primaries[2].0,
                    y: md.primaries[2].1,
                },
            ],
            white_point: ravif::ChromaticityPoint {
                x: md.white_point.0,
                y: md.white_point.1,
            },
            max_luminance: md.max_luminance,
            min_luminance: md.min_luminance,
        });
    }
    if let Some(cp) = config.color_primaries {
        enc = enc.with_color_primaries(cicp_to_color_primaries(cp));
    }
    if let Some(tc) = config.transfer_characteristics {
        enc = enc.with_transfer_characteristics(cicp_to_transfer_characteristics(tc));
    }
    if let Some(pr) = config.pixel_range {
        enc = enc.with_pixel_range(match pr {
            EncodePixelRange::Full => ravif::PixelRange::Full,
            EncodePixelRange::Limited => ravif::PixelRange::Limited,
        });
    }
    if let Some(ref gm) = config.gain_map {
        // The muxed `av1C` (and `ispe`) of the gain-map item must describe
        // the actual byte-carried bitstream — derive subsampling/monochrome
        // from its sequence header and validate the caller-declared
        // dimensions/depth against it, instead of writing defaults that can
        // lie about the payload (e.g. a 4:4:4 map muxed as 4:2:0).
        let md = zenavif_parse::AV1Metadata::parse_av1_bitstream(&gm.av1_data).map_err(|e| {
            at!(Error::InvalidParameters(format!(
                "gain map AV1 payload failed to parse: {e}"
            )))
        })?;
        if (md.max_frame_width.get(), md.max_frame_height.get()) != (gm.width, gm.height) {
            return Err(at!(Error::InvalidParameters(format!(
                "gain map dimensions {}x{} do not match its AV1 payload ({}x{})",
                gm.width, gm.height, md.max_frame_width, md.max_frame_height
            ))));
        }
        if md.bit_depth != gm.bit_depth {
            return Err(at!(Error::InvalidParameters(format!(
                "gain map bit depth {} does not match its AV1 payload ({})",
                gm.bit_depth, md.bit_depth
            ))));
        }
        enc = enc.with_gain_map(ravif::GainMapData {
            av1_data: gm.av1_data.clone(),
            width: gm.width,
            height: gm.height,
            bit_depth: gm.bit_depth,
            metadata: gm.metadata.clone(),
            alt_colr_cicp: config.gain_map_alt_colr,
            chroma_subsampling: (
                md.chroma_subsampling.horizontal,
                md.chroma_subsampling.vertical,
            ),
            monochrome: md.monochrome,
            alt_icc: config.gain_map_alt_icc.clone(),
        });
    }
    #[cfg(feature = "encode-imazen")]
    {
        // QM must be disabled for lossless (quantizer=0); zenravif/zenrav1e
        // handles all other quality levels (the q>=96 cliff was fixed
        // upstream in zenrav1e 0.1.4 — see imazen/zenrav1e#7).
        let qm = effective_qm(config);
        enc = enc
            .with_qm(qm)
            .with_vaq(config.enable_vaq, config.vaq_strength)
            .with_still_image_tuning(config.tune_still_image)
            .with_lossless(config.lossless);
        if let Some(b) = config.seg_boost {
            enc = enc.with_seg_boost(b);
        }
        enc = enc
            .with_cdef(config.override_cdef)
            .with_rdo_tx_decision(config.override_rdo_tx_decision)
            .with_sgr_full(config.override_sgr_full)
            .with_lru_on_skip(config.override_lru_on_skip)
            .with_segmentation_complex(config.override_segmentation_complex)
            .with_encode_bottomup(config.override_encode_bottomup);
        #[cfg(any(
            feature = "__expert",
            feature = "two-pass-butteraugli",
            feature = "two-pass-zensim"
        ))]
        {
            // The deepest knobs live behind ravif's `__expert` feature
            // (which both two-pass features also enable, without exposing
            // zenavif's own `__expert` surface). Mirror EncoderConfig's
            // per-field overrides into ravif's InternalParams in one call.
            // Build via Default + field assignment because
            // `#[non_exhaustive]` prohibits struct literal construction
            // outside the defining crate.
            let mut params = ravif::expert::InternalParams::default();
            #[cfg(feature = "__expert")]
            {
                params.partition_range = config.override_partition_range;
                params.complex_prediction_modes = config.override_complex_prediction_modes;
                params.lrf = config.override_lrf;
                params.fast_deblock = config.override_fast_deblock;
            }
            // Closed-loop per-SB quantizer scale map (two-pass drivers).
            #[cfg(any(feature = "two-pass-butteraugli", feature = "two-pass-zensim"))]
            {
                params.sb_q_scale = config.sb_q_scale.clone();
            }
            enc = enc.with_internal_params(params);
        }
        if let Some(t) = config.trellis {
            enc = enc.with_trellis(t);
        }
        // UNCOMMENT at the zenrav1e dep bump (the palette tool lands
        // post-0.1.4, zenrav1e@68a8d81f..df27117c; ravif gains the
        // pass-through builder then — see src/palette_gate.rs + CLAUDE.md
        // "Known Bugs" dep-bump checklist). Until then the preference is
        // stored/introspectable but not forwarded:
        // if let Some(pref) = config.palette_preference {
        //     enc = enc.with_palette(match pref {
        //         crate::palette_gate::PalettePreference::Auto => ravif::PaletteMode::Auto,
        //         crate::palette_gate::PalettePreference::Always => ravif::PaletteMode::Always,
        //         crate::palette_gate::PalettePreference::Off => ravif::PaletteMode::Off,
        //     });
        // }
        let _ = &config.palette_preference; // release-gated; silence unused-field until the bump
    }
    // Forward stop token for per-superblock cooperative cancellation.
    enc = enc.with_stop(stop);
    Ok(enc)
}

/// Encode an 8-bit RGB image to AVIF.
///
/// This is the default encode path and carries an **automatic per-image
/// RD-vs-time monotonicity guarantee**: on structured content encoded at a
/// bundle speed (6/7/8), it transparently probes the reliable anchor tier and
/// keeps whichever result Pareto-dominates on (bytes, perceptual score), so a
/// *faster* speed can never silently beat the one you asked for. The guarantee
/// needs perceptual scoring (`target-quality`) + content analysis (`auto-tune`);
/// without those features, on photo-like content or non-bundle speeds (where an
/// inversion is impossible), or while the release gate is off, this is exactly a
/// single [`encode_rgb8_once`] — no decode, no score, no extra encode. See
/// `docs/MONOTONICITY_PROGRAM.md` "SELECTIVE probe".
///
/// # Arguments
///
/// * `img` - RGB8 image buffer
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked pre-encode, forwarded to ravif per-superblock)
pub fn encode_rgb8(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    // The monotonicity probe's anchor tiers are calibrated for the
    // zenravif speed ladder; the zenav1-svt backend takes the plain
    // single-encode path (its own speed↔RD behavior is unmeasured here).
    if config.backend == Av1Backend::Zenav1Svt || config.backend == Av1Backend::Zenav1Aom {
        return encode_rgb8_once(img, config, stop);
    }
    #[cfg(all(feature = "target-quality", feature = "auto-tune"))]
    {
        crate::target_quality::encode_rgb8_auto_monotone(img, config, stop)
    }
    #[cfg(not(all(feature = "target-quality", feature = "auto-tune")))]
    {
        encode_rgb8_once(img, config, stop)
    }
}

/// Single-encode primitive with no monotonicity probe — the building block used
/// by [`encode_rgb8`], the target-quality search, and the two-pass path (each of
/// which must control its own repeated encodes without nesting the probe).
pub(crate) fn encode_rgb8_once(
    img: ImgRef<'_, Rgb<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;

    // Backend dispatch: the zenav1-svt backend covers exactly this entry
    // point (8-bit RGB → 4:2:0 still). It must never be served silently
    // by zenravif — the backend field is a contract, not a hint.
    if config.backend == Av1Backend::Zenav1Svt {
        #[cfg(feature = "zenav1-svt")]
        {
            return crate::encoder_svt_rs::encode_rgb8_svt_rs(img, config, stop);
        }
        #[cfg(not(feature = "zenav1-svt"))]
        {
            return Err(at!(Error::Unsupported(
                "Av1Backend::Zenav1Svt requires the `zenav1-svt` cargo feature"
            )));
        }
    }
    // Same contract for the zenav1-aom backend: 8-bit RGB → 4:2:0 still is
    // exactly the slice it implements (`src/encoder_aom.rs`).
    if config.backend == Av1Backend::Zenav1Aom {
        #[cfg(feature = "zenav1-aom-encode")]
        {
            return crate::encoder_aom::encode_rgb8_aom(img, config, stop);
        }
        #[cfg(not(feature = "zenav1-aom-encode"))]
        {
            return Err(at!(Error::Unsupported(
                "Av1Backend::Zenav1Aom requires the `zenav1-aom-encode` cargo feature"
            )));
        }
    }

    let enc = build_ravif_encoder(config, stop, false)?;
    let result = enc
        .encode_rgb(img)
        .map_err_at(error_from_ravif)
        .at_crate(crate::at_crate_info())?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// Encode an 8-bit grayscale image to AVIF as true monochrome AV1 (Cs400).
///
/// The bitstream codes only a luma plane — no chroma planes exist and the
/// chroma RDO is skipped entirely, which measures 2–3× faster than the
/// gray→RGB expansion path at output-byte parity (imazen/zenavif#6,
/// `benchmarks/mono_encode_ab_2026-06-11.txt`). The container carries
/// spec-correct mono `av1C`/`pixi` properties.
///
/// `img` holds one `u8` luma sample per pixel (sRGB transfer). The
/// configured chroma subsampling is irrelevant (there is no chroma);
/// bit depth, quality, and speed apply as for color.
#[cfg(feature = "encode-mono")]
pub fn encode_gray8(
    img: ImgRef<'_, u8>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    #[cfg(feature = "zenav1-svt")]
    if config.backend == Av1Backend::Zenav1Svt {
        return crate::encoder_svt_rs::encode_gray8_svt_rs(img, config, stop);
    }
    #[cfg(feature = "zenav1-aom-encode")]
    if config.backend == Av1Backend::Zenav1Aom {
        return crate::encoder_aom::encode_gray8_aom(img, config, stop);
    }
    reject_svt_rs_backend(
        config,
        "encode_gray8 (requires the `zenav1-svt` cargo feature)",
    )?;

    let enc = build_ravif_encoder(config, stop, false)?;
    let result = enc
        .encode_gray8(img)
        .map_err_at(error_from_ravif)
        .at_crate(crate::at_crate_info())?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// Encode an 8-bit RGBA image to AVIF.
///
/// Like [`encode_rgb8`], this default path carries the automatic RD-vs-time
/// monotonicity guarantee (selective probe on structured content; `patch_fraction`
/// is taken over the color channels). Inert (a single [`encode_rgba8_once`])
/// without `target-quality` + `auto-tune`, on photo-like content, on non-bundle
/// speeds, or while the release gate is off.
///
/// # Arguments
///
/// * `img` - RGBA8 image buffer
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked pre-encode, forwarded to ravif per-superblock)
pub fn encode_rgba8(
    img: ImgRef<'_, Rgba<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    // Skip the probe machinery for the zenav1-svt backend: `encode_rgba8_once`
    // dispatches it directly (no monotonicity-probe support there).
    if config.backend == Av1Backend::Zenav1Svt {
        return encode_rgba8_once(img, config, stop);
    }
    #[cfg(all(feature = "target-quality", feature = "auto-tune"))]
    {
        crate::target_quality::encode_rgba8_auto_monotone(img, config, stop)
    }
    #[cfg(not(all(feature = "target-quality", feature = "auto-tune")))]
    {
        encode_rgba8_once(img, config, stop)
    }
}

/// Single-encode RGBA8 primitive (no monotonicity probe) — see [`encode_rgb8_once`].
pub(crate) fn encode_rgba8_once(
    img: ImgRef<'_, Rgba<u8>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    #[cfg(feature = "zenav1-svt")]
    if config.backend == Av1Backend::Zenav1Svt {
        return crate::encoder_svt_rs::encode_rgba8_svt_rs(img, config, stop);
    }
    reject_svt_rs_backend(
        config,
        "encode_rgba8 (requires the `zenav1-svt` cargo feature)",
    )?;
    let enc = build_ravif_encoder(config, stop, false)?;
    let result = enc
        .encode_rgba(img)
        .map_err_at(error_from_ravif)
        .at_crate(crate::at_crate_info())?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// Encode a 16-bit RGB image to AVIF (10-bit AV1 by default)
///
/// Input values should be in full u16 range (0–65535), in the image's native
/// transfer function (typically sRGB gamma).
///
/// The coded depth follows [`EncoderConfig::bit_depth`]: [`EncodeBitDepth::Auto`]
/// (the default) and [`EncodeBitDepth::Ten`] code 10 bits, while
/// [`EncodeBitDepth::Eight`] narrows to an 8-bit stream. Narrowing uses the
/// crate's `scale_from_u16` rule, the exact inverse of LSB replication, so
/// 8-bit content that was promoted to 16 bits round-trips to the same bytes.
///
/// # Arguments
///
/// * `img` - RGB16 image buffer (0–65535)
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked pre-encode, forwarded to ravif per-superblock)
pub fn encode_rgb16(
    img: ImgRef<'_, Rgb<u16>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    use crate::convert::{narrow_to_u8, scale_from_u16};
    stop.check().map_err(|e| at!(Error::from(e)))?;
    #[cfg(feature = "zenav1-svt")]
    if config.backend == Av1Backend::Zenav1Svt {
        return crate::encoder_svt_rs::encode_rgb16_svt_rs(img, config, stop);
    }
    reject_svt_rs_backend(
        config,
        "encode_rgb16 (requires the `zenav1-svt` cargo feature)",
    )?;
    let enc = build_ravif_encoder(config, stop, true)?;
    let width = img.width();
    let height = img.height();
    // Identity (MC=0) signaling means GBR plane order — plane 0 is G,
    // 1 is B, 2 is R (H.273; zenravif's own 8-bit identity path writes
    // the same order via `rgb_to_10_bit_gbr`). Feeding [r,g,b] here was
    // issue #14: every 16-bit encode channel-rotated for conforming
    // decoders. Pinned by tests/identity_roundtrip.rs.
    let pixel_range = match config.pixel_range {
        Some(EncodePixelRange::Limited) => ravif::PixelRange::Limited,
        _ => ravif::PixelRange::Full,
    };
    // `config.bit_depth` is HONOURED here, not assumed. An explicit
    // `EncodeBitDepth::Eight` narrows this 16-bit buffer to an 8-bit coded
    // stream; before this branch existed the request was silently coded at
    // 10 bits, because `encode_raw_planes_*` takes the coded depth as an
    // argument and overrides the encoder's own `with_bit_depth`. Registered
    // as a defect in zenmetrics `bitdepth_capability_matrix_2026-09-02` §2;
    // gated by tests/bit_depth_request.rs.
    //
    // `Auto` keeps its documented contract (16-bit input -> 10-bit AV1), so
    // the default path through here is byte-for-byte what it always was.
    let result = match resolve_bit_depth(config.bit_depth, true) {
        ravif::BitDepth::Eight => {
            let pixels: Vec<[u8; 3]> = img
                .pixels()
                .map(|p| [narrow_to_u8(p.g), narrow_to_u8(p.b), narrow_to_u8(p.r)])
                .collect();
            enc.encode_raw_planes_8_bit(
                width,
                height,
                pixels,
                None::<std::iter::Empty<u8>>,
                pixel_range,
                ravif::MatrixCoefficients::Identity,
            )
        }
        _ => {
            let pixels: Vec<[u16; 3]> = img
                .pixels()
                .map(|p| {
                    [
                        scale_from_u16(p.g, 10),
                        scale_from_u16(p.b, 10),
                        scale_from_u16(p.r, 10),
                    ]
                })
                .collect();
            enc.encode_raw_planes_10_bit(
                width,
                height,
                pixels,
                None::<std::iter::Empty<u16>>,
                pixel_range,
                ravif::MatrixCoefficients::Identity,
            )
        }
    }
    .map_err_at(error_from_ravif)
    .at_crate(crate::at_crate_info())?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// Encode a 16-bit RGBA image to AVIF (10-bit AV1 by default)
///
/// Input values should be in full u16 range (0–65535), in the image's native
/// transfer function (typically sRGB gamma).
///
/// The coded depth follows [`EncoderConfig::bit_depth`] exactly as in
/// [`encode_rgb16`]; colour and alpha are narrowed together.
///
/// # Arguments
///
/// * `img` - RGBA16 image buffer (0–65535)
/// * `config` - Encoder configuration
/// * `stop` - Cancellation token (checked pre-encode, forwarded to ravif per-superblock)
pub fn encode_rgba16(
    img: ImgRef<'_, Rgba<u16>>,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedImage> {
    use crate::convert::{narrow_to_u8, scale_from_u16};
    stop.check().map_err(|e| at!(Error::from(e)))?;
    #[cfg(feature = "zenav1-svt")]
    if config.backend == Av1Backend::Zenav1Svt {
        return crate::encoder_svt_rs::encode_rgba16_svt_rs(img, config, stop);
    }
    reject_svt_rs_backend(
        config,
        "encode_rgba16 (requires the `zenav1-svt` cargo feature)",
    )?;
    let enc = build_ravif_encoder(config, stop, true)?;
    let width = img.width();
    let height = img.height();
    // GBR plane order under identity signaling — see encode_rgb16.
    let pixel_range = match config.pixel_range {
        Some(EncodePixelRange::Limited) => ravif::PixelRange::Limited,
        _ => ravif::PixelRange::Full,
    };
    // Honours `config.bit_depth` for colour AND alpha — see encode_rgb16.
    let result = match resolve_bit_depth(config.bit_depth, true) {
        ravif::BitDepth::Eight => {
            let pixels: Vec<[u8; 3]> = img
                .pixels()
                .map(|p| [narrow_to_u8(p.g), narrow_to_u8(p.b), narrow_to_u8(p.r)])
                .collect();
            let alpha: Vec<u8> = img.pixels().map(|p| narrow_to_u8(p.a)).collect();
            enc.encode_raw_planes_8_bit(
                width,
                height,
                pixels,
                Some(alpha),
                pixel_range,
                ravif::MatrixCoefficients::Identity,
            )
        }
        _ => {
            let pixels: Vec<[u16; 3]> = img
                .pixels()
                .map(|p| {
                    [
                        scale_from_u16(p.g, 10),
                        scale_from_u16(p.b, 10),
                        scale_from_u16(p.r, 10),
                    ]
                })
                .collect();
            let alpha: Vec<u16> = img.pixels().map(|p| scale_from_u16(p.a, 10)).collect();
            enc.encode_raw_planes_10_bit(
                width,
                height,
                pixels,
                Some(alpha),
                pixel_range,
                ravif::MatrixCoefficients::Identity,
            )
        }
    }
    .map_err_at(error_from_ravif)
    .at_crate(crate::at_crate_info())?;
    Ok(EncodedImage {
        avif_file: result.avif_file,
        color_byte_size: result.color_byte_size,
        alpha_byte_size: result.alpha_byte_size,
    })
}

/// A single frame in an animated AVIF sequence
#[derive(Clone)]
pub struct AnimationFrame {
    /// Frame pixel data (RGB8)
    pub pixels: ImgVec<RGB8>,
    /// Duration of this frame in milliseconds
    pub duration_ms: u32,
}

/// A single frame with alpha in an animated AVIF sequence
#[derive(Clone)]
pub struct AnimationFrameRgba {
    /// Frame pixel data (RGBA8)
    pub pixels: ImgVec<RGBA8>,
    /// Duration of this frame in milliseconds
    pub duration_ms: u32,
}

/// Result of animated AVIF encoding
#[non_exhaustive]
#[derive(Clone)]
pub struct EncodedAnimation {
    /// Complete AVIF file bytes
    pub avif_file: Vec<u8>,
    /// Number of frames encoded
    pub frame_count: usize,
    /// Total duration in milliseconds
    pub total_duration_ms: u64,
}

/// Encode a sequence of RGB8 frames into an animated AVIF
///
/// All frames must have the same dimensions. Each frame has its own
/// duration in milliseconds.
///
/// # Arguments
///
/// * `frames` - Sequence of RGB8 frames with durations
/// * `config` - Encoder configuration (quality, speed, etc.)
pub fn encode_animation_rgb8(
    frames: &[AnimationFrame],
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedAnimation> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_svt_rs_backend(config, "animation encoding")?;
    let enc = build_ravif_encoder(config, stop, false)?;

    let ravif_frames: Vec<ravif::AnimFrame<'_>> = frames
        .iter()
        .map(|f| ravif::AnimFrame {
            rgb: f.pixels.as_ref(),
            duration_ms: f.duration_ms,
        })
        .collect();

    let result = enc
        .encode_animation_rgb(&ravif_frames)
        .map_err_at(error_from_ravif)
        .at_crate(crate::at_crate_info())?;

    Ok(EncodedAnimation {
        avif_file: result.avif_file,
        frame_count: result.frame_count,
        total_duration_ms: result.total_duration_ms,
    })
}

/// Encode a sequence of RGBA8 frames into an animated AVIF
///
/// All frames must have the same dimensions. If any frame has
/// non-opaque alpha, an alpha track is included automatically.
///
/// # Arguments
///
/// * `frames` - Sequence of RGBA8 frames with durations
/// * `config` - Encoder configuration (quality, speed, etc.)
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_animation_rgba8(
    frames: &[AnimationFrameRgba],
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedAnimation> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_svt_rs_backend(config, "animation encoding")?;
    let enc = build_ravif_encoder(config, stop, false)?;

    let ravif_frames: Vec<ravif::AnimFrameRgba<'_>> = frames
        .iter()
        .map(|f| ravif::AnimFrameRgba {
            rgba: f.pixels.as_ref(),
            duration_ms: f.duration_ms,
        })
        .collect();

    let result = enc
        .encode_animation_rgba(&ravif_frames)
        .map_err_at(error_from_ravif)
        .at_crate(crate::at_crate_info())?;

    Ok(EncodedAnimation {
        avif_file: result.avif_file,
        frame_count: result.frame_count,
        total_duration_ms: result.total_duration_ms,
    })
}

/// A single 16-bit RGB frame in an animated AVIF sequence
#[derive(Clone)]
pub struct AnimationFrame16 {
    /// Frame pixel data (RGB16, full 0–65535 range)
    pub pixels: ImgVec<RGB16>,
    /// Duration of this frame in milliseconds
    pub duration_ms: u32,
}

/// A single 16-bit RGBA frame in an animated AVIF sequence
#[derive(Clone)]
pub struct AnimationFrameRgba16 {
    /// Frame pixel data (RGBA16, full 0–65535 range)
    pub pixels: ImgVec<RGBA16>,
    /// Duration of this frame in milliseconds
    pub duration_ms: u32,
}

/// Encode a sequence of 16-bit RGB frames into an animated AVIF (10-bit AV1)
///
/// Input values should be in full u16 range (0–65535), in the image's native
/// transfer function (typically sRGB gamma). Values are scaled to 10-bit
/// internally. All frames must have the same dimensions.
///
/// # Arguments
///
/// * `frames` - Sequence of RGB16 frames with durations (0–65535)
/// * `config` - Encoder configuration (quality, speed, etc.)
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_animation_rgb16(
    frames: &[AnimationFrame16],
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedAnimation> {
    use crate::convert::scale_from_u16;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_svt_rs_backend(config, "animation encoding")?;
    let enc = build_ravif_encoder(config, stop, true)?;

    // Scale each frame from 0–65535 to 10-bit (0–1023)
    let scaled_frames: Vec<ImgVec<RGB16>> = frames
        .iter()
        .map(|f| {
            let scaled: Vec<RGB16> = f
                .pixels
                .buf()
                .iter()
                .map(|p| RGB16 {
                    r: scale_from_u16(p.r, 10),
                    g: scale_from_u16(p.g, 10),
                    b: scale_from_u16(p.b, 10),
                })
                .collect();
            ImgVec::new(scaled, f.pixels.width(), f.pixels.height())
        })
        .collect();

    let ravif_frames: Vec<ravif::AnimFrame16<'_>> = scaled_frames
        .iter()
        .zip(frames.iter())
        .map(|(scaled, orig)| ravif::AnimFrame16 {
            rgb: scaled.as_ref(),
            duration_ms: orig.duration_ms,
        })
        .collect();

    let result = enc
        .encode_animation_rgb16(&ravif_frames)
        .map_err_at(error_from_ravif)
        .at_crate(crate::at_crate_info())?;

    Ok(EncodedAnimation {
        avif_file: result.avif_file,
        frame_count: result.frame_count,
        total_duration_ms: result.total_duration_ms,
    })
}

/// Encode a sequence of 16-bit RGBA frames into an animated AVIF (10-bit AV1)
///
/// Input values should be in full u16 range (0–65535), in the image's native
/// transfer function (typically sRGB gamma). Values are scaled to 10-bit
/// internally. All frames must have the same dimensions.
///
/// # Arguments
///
/// * `frames` - Sequence of RGBA16 frames with durations (0–65535)
/// * `config` - Encoder configuration (quality, speed, etc.)
/// * `stop` - Cancellation token (checked before encoding starts)
pub fn encode_animation_rgba16(
    frames: &[AnimationFrameRgba16],
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<EncodedAnimation> {
    use crate::convert::scale_from_u16;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_svt_rs_backend(config, "animation encoding")?;
    let enc = build_ravif_encoder(config, stop, true)?;

    // Scale each frame from 0–65535 to 10-bit (0–1023)
    let scaled_frames: Vec<ImgVec<RGBA16>> = frames
        .iter()
        .map(|f| {
            let scaled: Vec<RGBA16> = f
                .pixels
                .buf()
                .iter()
                .map(|p| RGBA16 {
                    r: scale_from_u16(p.r, 10),
                    g: scale_from_u16(p.g, 10),
                    b: scale_from_u16(p.b, 10),
                    a: scale_from_u16(p.a, 10),
                })
                .collect();
            ImgVec::new(scaled, f.pixels.width(), f.pixels.height())
        })
        .collect();

    let ravif_frames: Vec<ravif::AnimFrameRgba16<'_>> = scaled_frames
        .iter()
        .zip(frames.iter())
        .map(|(scaled, orig)| ravif::AnimFrameRgba16 {
            rgba: scaled.as_ref(),
            duration_ms: orig.duration_ms,
        })
        .collect();

    let result = enc
        .encode_animation_rgba16(&ravif_frames)
        .map_err_at(error_from_ravif)
        .at_crate(crate::at_crate_info())?;

    Ok(EncodedAnimation {
        avif_file: result.avif_file,
        frame_count: result.frame_count,
        total_duration_ms: result.total_duration_ms,
    })
}

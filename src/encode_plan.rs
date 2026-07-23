//! Static encode-plan resolution: what will this config actually do?
//!
//! [`EncoderConfig::resolve_plan`] resolves every knob — quantizers,
//! the qm×lossless gate, the speed-preset-derived search settings after
//! overrides, chroma subsampling, tile count — into an [`EncodePlan`]
//! a caller can audit before spending an encode. The zenavif-side
//! decisions (bit-depth resolution, alpha-quality fallback, the QM
//! lossless gate) go through the *same functions* `build_ravif_encoder`
//! uses, so they cannot drift.
//!
//! # The zenravif mirror, and why it is trustworthy
//!
//! The deepest resolution lives inside zenravif as `pub(crate)` code:
//! the quality→quantizer curve and the per-speed search-setting tables.
//! Those are **mirrored** here (every mirror carries a provenance
//! comment citing zenravif 0.1.3 source line), which is a second
//! implementation and therefore a drift risk. The mitigation is
//! empirical, per the variant-generation discipline (see
//! `docs/VARIANT_GENERATION.md`): `examples/sweep_validate.rs` encodes
//! alias pairs the mirror predicts (equal quantizer ⇒ byte-identical
//! output; override == preset value ⇒ byte-identical output) and
//! hard-fails when the prediction is wrong. If zenravif's internals
//! change, the harness fails loudly instead of the plan lying quietly.
//! The structural fix — exposing resolution from zenravif itself — is
//! tracked as a follow-up for zenravif 0.1.4.
//!
//! Static plans report only what is statically knowable. Decisions the
//! encoder takes per-content (dropping an all-opaque alpha plane,
//! cleaning color under transparent pixels) are documented on the
//! fields they affect, never guessed.

use crate::encoder::{
    Av1Backend, EncodeChromaSubsampling, EncodeColorModel, EncodePixelRange, EncoderConfig,
};

/// Input facts `resolve_plan` needs about the image to be encoded.
///
/// Identity of an encode cell is `(config, input)` — tile count depends
/// on pixel dimensions, bit-depth resolution on the input's bitness,
/// and the alpha quantizer only applies when an alpha plane exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanInput {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// True for the `encode_rgb16` / `encode_rgba16` entry points.
    pub input_is_16bit: bool,
    /// True when the caller passes RGBA. Note the encoder drops the
    /// alpha plane entirely when every pixel is opaque — a
    /// content-dependent decision a static plan cannot make; alpha
    /// fields in the plan describe the "alpha plane is emitted" case.
    pub input_has_alpha: bool,
}

impl PlanInput {
    /// Plan input for an 8-bit RGB image.
    #[must_use]
    pub fn rgb8(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            input_is_16bit: false,
            input_has_alpha: false,
        }
    }

    /// Plan input for an 8-bit RGBA image.
    #[must_use]
    pub fn rgba8(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            input_is_16bit: false,
            input_has_alpha: true,
        }
    }
}

/// How many AV1 tiles the encode will request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TilesResolution {
    /// `threads` is pinned, so the tile count is a pure function of the
    /// config and image size: `min(threads, w·h / min_tile_size²)`.
    Fixed(usize),
    /// `threads` is `None`: zenravif substitutes the *machine's* rayon
    /// thread count into the tile formula, so the encoded bytes depend
    /// on the host's core count. `cap` is the image-size bound
    /// (`w·h / min_tile_size²`); the actual count is
    /// `min(host_threads, cap)`. Reproducible sweeps must pin
    /// [`EncoderConfig::threads`].
    ///
    /// Provenance: zenravif 0.1.3 `av1encoder.rs:1615`
    /// (`rav1e_config`: `threads.unwrap_or_else(rayon::current_num_threads)`).
    MachineDependent {
        /// Image-size bound on the tile count.
        cap: usize,
    },
}

/// Speed-preset-derived AV1 search settings, after zenavif's override
/// knobs are applied — the resolved values the encoder hands zenrav1e.
///
/// Mirror of zenravif 0.1.3 `SpeedTweaks::from_my_preset`
/// (`av1encoder.rs:1454`), validated by encode in
/// `examples/sweep_validate.rs` (override == preset value must be
/// byte-identical with the unset config). Fields zenravif leaves to the
/// underlying zenrav1e preset (`tx_domain_distortion`, the motion/
/// multiref settings) are functions of `speed_preset` alone and are not
/// duplicated here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SpeedDerived {
    /// Partition block-size search range `(min, max)` in pixels.
    pub partition_range: (u8, u8),
    /// Full intra/inter prediction-mode search (`ComplexAll`) vs the
    /// reduced `Simple` set. zenravif forces `Simple` for stills to
    /// dodge the zenrav1e#5 filter-intra cost bug.
    pub complex_prediction_modes: bool,
    /// Self-guided restoration searches all 16 parameter sets vs 8.
    pub sgr_complexity_full: bool,
    /// Bottom-up partition search. zenravif forces top-down (false):
    /// bottom-up's RDO cost model ignores QM weights.
    pub encode_bottomup: bool,
    /// Per-block RDO transform-type search.
    pub rdo_tx_decision: bool,
    /// Reduced transform-type set.
    pub reduced_tx_set: bool,
    /// Fine directional intra-prediction angles.
    pub fine_directional_intra: bool,
    /// Closed-form deblock level (fast) vs SSE-driven search.
    pub fast_deblock: bool,
    /// Loop restoration filter (Wiener + SGR).
    pub lrf: bool,
    /// Constrained directional enhancement filter.
    pub cdef: bool,
    /// Inter transform-split search.
    pub inter_tx_split: bool,
    /// Rate estimation in transform domain (faster, ~10 % larger).
    pub tx_domain_rate: bool,
    /// Segmentation `Complex` (k-means) vs `Simple`.
    pub segmentation_complex: bool,
    /// Loop-restoration search on skip blocks.
    pub lru_on_skip: bool,
    /// Non-square partition max threshold (block-size edge in pixels).
    pub non_square_partition_max: u8,
    /// Minimum tile edge in pixels — feeds the tile-count formula.
    pub min_tile_size: u16,
}

/// The resolved encode plan: every knob after defaults, gates, and
/// overrides, as the encoder will actually run it.
///
/// Produced by [`EncoderConfig::resolve_plan`]. Fields describe the
/// zenravif backend — the only available one (the deprecated svtav1
/// variant is rejected by [`EncoderConfig::validate`]).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct EncodePlan {
    /// Selected AV1 encoder backend.
    pub backend: Av1Backend,
    /// Resolved color quantizer (0–255; lossless forces 0).
    pub quantizer: u8,
    /// Resolved alpha quantizer. Applies only when an alpha plane is
    /// emitted (the encoder drops all-opaque alpha planes); `None` when
    /// the input declares no alpha at all.
    pub alpha_quantizer: Option<u8>,
    /// Resolved output bit depth (8 or 10).
    pub bit_depth: u8,
    /// Resolved internal color model. On the 16-bit entry points the
    /// current implementation always encodes identity-matrix RGB planes
    /// regardless of the configured model (`encoder.rs`
    /// `encode_rgb16`/`encode_rgba16` pass
    /// `MatrixCoefficients::Identity`), so the config's `YCbCr` is
    /// reported as overridden there.
    pub color_model: EncodeColorModel,
    /// CICP matrix coefficients the AV1 payload will signal, resolved
    /// from the color model: YCbCr → 6 (BT.601), RGB → 0 (Identity).
    /// The `EncoderConfig::matrix_coefficients` CICP field is **not**
    /// consulted by the zenravif backend (see
    /// [`EncoderConfig::validate`]).
    ///
    /// Provenance: zenravif 0.1.3 `av1encoder.rs:924`.
    pub matrix_coefficients_cicp: u8,
    /// Resolved chroma subsampling.
    pub chroma_subsampling: EncodeChromaSubsampling,
    /// Resolved pixel range (config default: full).
    pub pixel_range: EncodePixelRange,
    /// Alpha handling mode. Pixel-changing on alpha-bearing input:
    /// `UnassociatedClean` (default) rewrites color under transparent
    /// pixels for compressibility, `Premultiplied` rescales color by
    /// alpha. Irrelevant when no alpha plane is emitted.
    pub alpha_color_mode: crate::EncodeAlphaMode,
    /// Quantization matrices after the lossless gate.
    pub qm: bool,
    /// Variance adaptive quantization **with effect**: true only when
    /// VAQ is enabled AND the strength differs from 1.0. The
    /// psychovisual/still tunes always compute the activity mask, and
    /// zenrav1e skips the VAQ rescale at strength 1.0, so
    /// `with_vaq(true, 1.0)` is structurally byte-identical to off
    /// (zenrav1e `api/internal.rs:1379`).
    pub vaq: bool,
    /// VAQ strength; only meaningful when `vaq` is true.
    pub vaq_strength: f64,
    /// `Tune::StillImage` vs `Tune::Psychovisual`.
    pub tune_still_image: bool,
    /// Mathematically lossless mode (quantizer pinned to 0).
    pub lossless: bool,
    /// Segmentation boost (1.0 = off).
    pub seg_boost: f64,
    /// Trellis quantization (Viterbi coefficient optimization).
    pub trellis: bool,
    /// The speed preset number driving everything in `speed`.
    pub speed_preset: u8,
    /// Speed-derived search settings for the color plane, after
    /// overrides.
    pub speed: SpeedDerived,
    /// Speed-derived settings for the alpha plane (the quantizer
    /// thresholds re-derive from the alpha quantizer). `None` when the
    /// input declares no alpha.
    pub alpha_speed: Option<SpeedDerived>,
    /// Requested AV1 tile count for the color image.
    pub tiles: TilesResolution,
    /// An ICC profile blob will be embedded.
    pub has_icc: bool,
    /// An EXIF blob will be embedded.
    pub has_exif: bool,
    /// An XMP blob will be embedded.
    pub has_xmp: bool,
    /// A pre-encoded gain map will be embedded.
    pub has_gain_map: bool,
}

// ============================================================================
// zenravif mirrors (provenance-tagged; encode-validated by the harness)
// ============================================================================

/// Mirror of zenravif's quality→quantizer curve.
///
/// Provenance: zenravif 0.1.3 `av1encoder.rs:1416`
/// (`quality_to_quantizer`), copied verbatim. Encode-validated:
/// `sweep_validate` asserts q 80.0 and q 80.2 (equal quantizer 71)
/// produce byte-identical files while q 81.0 (quantizer 68) differs.
#[must_use]
pub(crate) fn quality_to_quantizer(quality: f32) -> u8 {
    let q = quality.clamp(1., 100.) / 100.;
    let x = if q >= 0.70 {
        (1. - q) * 1.4 // Q70-100 → qindex 0-107
    } else if q > 0.10 {
        0.42 + (0.70 - q) * 0.85 // Q10-70 → qindex 107-237
    } else {
        0.93 + (0.10 - q) * 0.78 // Q1-10 → qindex 237-255
    };
    (x.min(1.0) * 255.).round() as u8
}

/// Registry-era lossless speed band (imazen/zenavif#8): while the dep
/// chain resolves zenrav1e 0.1.4 (whose lossless is qi=1 lossy,
/// zenrav1e#9), lossless encodes clamp their speed preset into this
/// inclusive band. `None` disables the clamp. Full rationale +
/// measurements + the dep-bump removal instruction:
/// [`EncoderConfig::speed_effective`].
pub(crate) const LOSSLESS_REGISTRY_SPEED_BAND: Option<(u8, u8)> = Some((6, 8));

/// Mirror of zenravif's per-speed search-setting derivation.
///
/// Provenance: zenravif 0.1.3 `av1encoder.rs:1454`
/// (`SpeedTweaks::from_my_preset`), resolved values only. The
/// quantizer thresholds are fixed by zenravif design ("so these don't
/// shift when the quality curve changes"): `low_quality` ⇔
/// `quantizer > 150` (≈ Q50 and below), `high_quality` ⇔
/// `quantizer < 80` (≈ Q80 and above).
#[must_use]
pub(crate) fn speed_derived(speed: u8, quantizer: u8) -> SpeedDerived {
    let low_quality = quantizer > 150;
    let high_quality = quantizer < 80;
    let max_block_size = if high_quality { 16 } else { 64 };

    let partition_range = match speed {
        0 => (4, 64.min(max_block_size)),
        1 if low_quality => (4, 64.min(max_block_size)),
        2 if low_quality => (4, 32.min(max_block_size)),
        1..=4 => (4, 16),
        5..=8 => (8, 16),
        _ => (16, 16),
    };

    SpeedDerived {
        partition_range,
        complex_prediction_modes: false,
        sgr_complexity_full: speed <= 2,
        encode_bottomup: false,
        rdo_tx_decision: speed <= 4 && !high_quality,
        reduced_tx_set: speed == 4 || speed >= 9,
        fine_directional_intra: speed <= 6,
        fast_deblock: speed >= 7 && !high_quality,
        lrf: low_quality && speed <= 8,
        cdef: low_quality && speed <= 9,
        inter_tx_split: speed >= 9,
        tx_domain_rate: speed >= 10,
        segmentation_complex: speed <= 2,
        lru_on_skip: speed <= 1,
        non_square_partition_max: match speed {
            0..=1 => 64,
            2..=3 => 32,
            _ => 8,
        },
        min_tile_size: match speed {
            0 => 4096,
            1 => 2048,
            2 => 1024,
            3 => 512,
            4 => 256,
            _ => 128,
        } * if high_quality { 2 } else { 1 },
    }
}

/// Apply zenavif's override knobs onto the preset-derived settings —
/// the same precedence `build_ravif_encoder` → zenravif applies
/// (override patches `SpeedTweaks` after `from_my_preset`; zenravif
/// 0.1.3 `av1encoder.rs:1169`).
#[cfg_attr(not(feature = "encode-imazen"), allow(unused_variables))]
pub(crate) fn apply_overrides(derived: &mut SpeedDerived, config: &EncoderConfig) {
    #[cfg(feature = "encode-imazen")]
    {
        if let Some(v) = config.override_cdef {
            derived.cdef = v;
        }
        if let Some(v) = config.override_rdo_tx_decision {
            derived.rdo_tx_decision = v;
        }
        if let Some(v) = config.override_sgr_full {
            derived.sgr_complexity_full = v;
        }
        if let Some(v) = config.override_lru_on_skip {
            derived.lru_on_skip = v;
        }
        if let Some(v) = config.override_segmentation_complex {
            derived.segmentation_complex = v;
        }
        if let Some(v) = config.override_encode_bottomup {
            derived.encode_bottomup = v;
        }
    }
    #[cfg(feature = "__expert")]
    {
        if let Some(r) = config.override_partition_range {
            derived.partition_range = r;
        }
        if let Some(v) = config.override_complex_prediction_modes {
            derived.complex_prediction_modes = v;
        }
        if let Some(v) = config.override_lrf {
            derived.lrf = v;
        }
        if let Some(v) = config.override_fast_deblock {
            derived.fast_deblock = v;
        }
    }
}

/// Tile count for the given thread setting and image size.
///
/// Provenance: zenravif 0.1.3 `av1encoder.rs:1615` (`rav1e_config`):
/// `threads.unwrap_or_else(rayon::current_num_threads)
///     .min(width * height / min_tile_size²)`.
#[must_use]
pub(crate) fn tiles_for(
    threads: Option<usize>,
    width: u32,
    height: u32,
    min_tile_size: u16,
) -> TilesResolution {
    let cap = (width as usize * height as usize) / (min_tile_size as usize).pow(2);
    match threads {
        Some(t) => TilesResolution::Fixed(t.min(cap)),
        None => TilesResolution::MachineDependent { cap },
    }
}

// ============================================================================
// resolve_plan
// ============================================================================

impl EncoderConfig {
    /// Resolve every knob of this config against a concrete input shape.
    ///
    /// The zenavif-side decisions (bit-depth resolution, alpha-quality
    /// fallback, the QM lossless gate) come from the same functions the
    /// encoder itself runs. The zenravif-side derivations (quantizer
    /// curve, speed tables, tile formula) are provenance-tagged mirrors
    /// validated by encode in `examples/sweep_validate.rs` — see the
    /// module docs for the contract.
    ///
    /// ```
    /// # #[cfg(feature = "encode")] {
    /// use zenavif::{EncoderConfig, PlanInput};
    ///
    /// let plan = EncoderConfig::new()
    ///     .quality(80.0)
    ///     .speed(6)
    ///     .resolve_plan(PlanInput::rgb8(1024, 768));
    /// assert_eq!(plan.quantizer, 71);
    /// assert_eq!(plan.bit_depth, 8);
    /// # }
    /// ```
    #[must_use]
    pub fn resolve_plan(&self, input: PlanInput) -> EncodePlan {
        let quantizer = if self.lossless_effective() {
            0
        } else {
            quality_to_quantizer(self.quality)
        };
        // The alpha plane has its own quantizer and its own
        // speed-threshold derivation (zenravif 0.1.3 av1encoder.rs:1240
        // builds a second SpeedTweaks from the alpha quantizer).
        let alpha_quantizer = input.input_has_alpha.then(|| {
            if self.lossless_effective() {
                0
            } else {
                quality_to_quantizer(crate::encoder::effective_alpha_quality(self))
            }
        });

        // The effective preset: identical to `self.speed` except under
        // the registry-era lossless clamp (imazen/zenavif#8, see
        // `speed_effective`) — the plan mirrors what the encoder runs.
        let speed_preset = self.speed_effective();
        let mut speed = speed_derived(speed_preset, quantizer);
        apply_overrides(&mut speed, self);
        let alpha_speed = alpha_quantizer.map(|aq| {
            let mut s = speed_derived(speed_preset, aq);
            apply_overrides(&mut s, self);
            s
        });

        let bit_depth =
            match crate::encoder::resolve_bit_depth(self.bit_depth, input.input_is_16bit) {
                ravif::BitDepth::Eight => 8,
                _ => 10,
            };

        // The 16-bit entry points feed raw RGB planes with an identity
        // matrix regardless of the configured color model
        // (encoder.rs encode_rgb16/encode_rgba16).
        let color_model = if input.input_is_16bit {
            EncodeColorModel::Rgb
        } else {
            self.color_model
        };
        let matrix_coefficients_cicp = match color_model {
            EncodeColorModel::YCbCr => 6, // BT.601
            EncodeColorModel::Rgb => 0,   // Identity
        };

        EncodePlan {
            backend: self.backend,
            quantizer,
            alpha_quantizer,
            bit_depth,
            color_model,
            matrix_coefficients_cicp,
            chroma_subsampling: self.chroma_subsampling,
            pixel_range: self.pixel_range.unwrap_or(EncodePixelRange::Full),
            alpha_color_mode: self.alpha_color_mode,
            qm: self.qm_effective(),
            vaq: self.vaq_active(),
            vaq_strength: self.vaq_strength_effective(),
            tune_still_image: self.tune_still_image_effective(),
            lossless: self.lossless_effective(),
            seg_boost: self.seg_boost_effective(),
            trellis: self.trellis_effective(),
            speed_preset,
            tiles: tiles_for(self.threads, input.width, input.height, speed.min_tile_size),
            speed,
            alpha_speed,
            has_icc: self.icc_profile.is_some(),
            has_exif: self.exif.is_some(),
            has_xmp: self.xmp.is_some(),
            has_gain_map: self.gain_map.is_some(),
        }
    }

    // --- effective-value accessors -------------------------------------
    // Builds without `encode-imazen` hand zenravif a config whose
    // imazen-only fields don't exist; zenravif then encodes with QM/VAQ/
    // trellis off and psychovisual tune (zenravif 0.1.3
    // `av1encoder.rs:1666` rav1e_config cfg fallbacks). The plan reports
    // that reality unconditionally.

    pub(crate) fn qm_effective(&self) -> bool {
        #[cfg(feature = "encode-imazen")]
        {
            crate::encoder::effective_qm(self)
        }
        #[cfg(not(feature = "encode-imazen"))]
        {
            false
        }
    }

    pub(crate) fn vaq_effective(&self) -> bool {
        #[cfg(feature = "encode-imazen")]
        {
            self.enable_vaq
        }
        #[cfg(not(feature = "encode-imazen"))]
        {
            false
        }
    }

    /// Whether VAQ will actually change the encode. Under the
    /// psychovisual/still-image tunes zenravif always uses, the
    /// activity mask is computed regardless of `enable_vaq`; the knob's
    /// only incremental effect is the strength rescale, which zenrav1e
    /// explicitly skips at strength 1.0. So `with_vaq(true, 1.0)` is
    /// byte-identical to VAQ off — structurally, not just empirically.
    ///
    /// Provenance: zenrav1e `api/internal.rs:1369` (`use_activity`
    /// includes both tunes) and `:1379`
    /// (`enable_vaq && vaq_strength != 1.0` gate). Byte-proven in
    /// `sweep_validate` (first run flagged the 1.0 axis value as an
    /// inert step across 24 encodes).
    pub(crate) fn vaq_active(&self) -> bool {
        self.vaq_effective() && self.vaq_strength_effective() != 1.0
    }

    pub(crate) fn vaq_strength_effective(&self) -> f64 {
        #[cfg(feature = "encode-imazen")]
        {
            self.vaq_strength
        }
        #[cfg(not(feature = "encode-imazen"))]
        {
            1.0
        }
    }

    pub(crate) fn tune_still_image_effective(&self) -> bool {
        #[cfg(feature = "encode-imazen")]
        {
            self.tune_still_image
        }
        #[cfg(not(feature = "encode-imazen"))]
        {
            false
        }
    }

    /// The speed preset the encode will actually run: the configured
    /// speed, clamped into [`LOSSLESS_REGISTRY_SPEED_BAND`] when lossless
    /// is effective (imazen/zenavif#8).
    ///
    /// Registry zenrav1e 0.1.4's lossless path floors qindex at 1
    /// (zenrav1e#9 — every "lossless" encode is actually qi=1 lossy;
    /// fixed on zenrav1e master `c3567081`, unreleased), so the slow
    /// tier's RDO spends its time optimizing against phantom distortion.
    /// Measured on that path (`examples/lossless_speed_sweep.rs`;
    /// `benchmarks/lossless_speed_sweep_2026-06-11.tsv`, re-verified
    /// byte-identical 2026-07-23): speeds 1-4 produce +5..+19% LARGER
    /// files than speed 8 at 4.6-11x the wall time on every measured
    /// source class, speed 10 is the largest of all on 4/5 sources
    /// (up to +42%), and the slow tier's worst-case pixel error is no
    /// better (paris max_delta 8 at s1-2 vs 2 at s6+). The [6, 8] band
    /// is the empirically size-optimal region; within it the residual
    /// s6-vs-s8 size inversion is ≤ ~1.1%, and effort keeps its time
    /// semantics (s6 ≈ 1.5x s8).
    ///
    /// AT THE zenrav1e (>0.1.4) DEP BUMP: set
    /// `LOSSLESS_REGISTRY_SPEED_BAND` to `None` — the fixed encoder is
    /// bit-exact and byte-monotonic (slower = smaller;
    /// `benchmarks/lossless_speed_sweep_fixed_2026-06-11.tsv`), so the
    /// full 1..=10 range becomes meaningful again. Then re-run
    /// `examples/lossless_speed_sweep.rs`, tighten
    /// `tests/identity_roundtrip.rs` to exact, and close zenavif#8 if
    /// still open (CLAUDE.md dep-bump checklist).
    pub(crate) fn speed_effective(&self) -> u8 {
        match (self.lossless_effective(), LOSSLESS_REGISTRY_SPEED_BAND) {
            (true, Some((lo, hi))) => self.speed.clamp(lo, hi),
            _ => self.speed,
        }
    }

    pub(crate) fn lossless_effective(&self) -> bool {
        #[cfg(feature = "encode-imazen")]
        {
            self.lossless
        }
        #[cfg(not(feature = "encode-imazen"))]
        {
            false
        }
    }

    pub(crate) fn seg_boost_effective(&self) -> f64 {
        #[cfg(feature = "encode-imazen")]
        {
            self.seg_boost.unwrap_or(1.0)
        }
        #[cfg(not(feature = "encode-imazen"))]
        {
            1.0
        }
    }

    pub(crate) fn trellis_effective(&self) -> bool {
        #[cfg(feature = "encode-imazen")]
        {
            self.trellis.unwrap_or(false)
        }
        #[cfg(not(feature = "encode-imazen"))]
        {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantizer_curve_anchor_points() {
        // Anchors computed from the mirrored formula; the *encode-level*
        // pin against zenravif is sweep_validate's alias-pair check.
        assert_eq!(quality_to_quantizer(100.0), 0);
        assert_eq!(quality_to_quantizer(80.0), 71);
        assert_eq!(quality_to_quantizer(80.2), 71); // alias pair partner
        assert_eq!(quality_to_quantizer(81.0), 68); // negative control
        assert_eq!(quality_to_quantizer(70.0), 107);
        assert_eq!(quality_to_quantizer(10.0), 237);
        assert_eq!(quality_to_quantizer(1.0), 255);
        // Out-of-range clamps like zenravif's.
        assert_eq!(quality_to_quantizer(0.0), 255);
        assert_eq!(quality_to_quantizer(150.0), 0);
    }

    #[test]
    fn integer_qualities_never_alias() {
        // The curve's per-unit slope is ≥ ~2 qindex steps everywhere, so
        // every integer quality maps to a distinct quantizer — integer
        // sweep grids never produce accidental duplicate cells.
        let mut prev = quality_to_quantizer(1.0);
        for q in 2..=100 {
            let cur = quality_to_quantizer(q as f32);
            assert!(
                cur < prev,
                "quantizer not strictly decreasing at q{q}: {cur} vs {prev}"
            );
            prev = cur;
        }
    }

    #[test]
    fn speed_thresholds_match_zenravif_design() {
        // low_quality ⇔ quantizer > 150; high_quality ⇔ quantizer < 80.
        let lo = speed_derived(6, 194); // ~q30
        assert!(lo.cdef && lo.lrf);
        assert_eq!(lo.min_tile_size, 128);
        let hi = speed_derived(6, 54); // ~q85
        assert!(!hi.cdef && !hi.lrf);
        assert_eq!(hi.min_tile_size, 256); // ×2 at high quality
        assert_eq!(hi.partition_range, (8, 16));
        let s2_lo = speed_derived(2, 194);
        assert_eq!(s2_lo.partition_range, (4, 32));
        assert!(s2_lo.segmentation_complex && s2_lo.sgr_complexity_full);
    }

    #[test]
    fn tiles_resolution() {
        // 512² at min_tile 128 → cap 16.
        assert_eq!(tiles_for(Some(2), 512, 512, 128), TilesResolution::Fixed(2));
        assert_eq!(
            tiles_for(Some(32), 512, 512, 128),
            TilesResolution::Fixed(16)
        );
        assert_eq!(
            tiles_for(None, 512, 512, 128),
            TilesResolution::MachineDependent { cap: 16 }
        );
        // Tiny image: cap 0 regardless of threads.
        assert_eq!(tiles_for(Some(8), 64, 64, 128), TilesResolution::Fixed(0));
    }

    #[test]
    fn plan_resolves_through_shared_functions() {
        let cfg = EncoderConfig::new().quality(80.0).speed(6);
        let plan = cfg.resolve_plan(PlanInput::rgb8(1024, 768));
        assert_eq!(plan.quantizer, 71);
        assert_eq!(plan.alpha_quantizer, None);
        assert_eq!(plan.bit_depth, 8);
        assert_eq!(plan.matrix_coefficients_cicp, 6);
        assert_eq!(plan.speed_preset, 6);
        assert!(!plan.speed.cdef, "cdef off at high quality");

        // Alpha follows color quality when unset.
        let plan_a = cfg.resolve_plan(PlanInput::rgba8(1024, 768));
        assert_eq!(plan_a.alpha_quantizer, Some(71));
        assert!(plan_a.alpha_speed.is_some());

        // 16-bit input forces 10-bit identity-RGB.
        let plan16 = cfg.resolve_plan(PlanInput {
            width: 64,
            height: 64,
            input_is_16bit: true,
            input_has_alpha: false,
        });
        assert_eq!(plan16.bit_depth, 10);
        assert_eq!(plan16.color_model, EncodeColorModel::Rgb);
        assert_eq!(plan16.matrix_coefficients_cicp, 0);
    }

    #[cfg(feature = "encode-imazen")]
    #[test]
    fn lossless_gates_qm_and_quantizer() {
        let cfg = EncoderConfig::new().quality(50.0).with_lossless(true);
        let plan = cfg.resolve_plan(PlanInput::rgb8(256, 256));
        assert_eq!(plan.quantizer, 0);
        assert!(!plan.qm, "lossless forces QM off");
        assert!(plan.lossless);
    }

    /// imazen/zenavif#8: while the dep chain resolves the zenrav1e
    /// release whose lossless is qi=1 lossy, lossless encodes clamp
    /// their speed preset into `LOSSLESS_REGISTRY_SPEED_BAND` (both
    /// ends), lossy encodes never do, and the plan mirrors the clamp.
    /// DELETE this test together with the band const at the zenrav1e
    /// (>0.1.4) dep bump.
    #[cfg(feature = "encode-imazen")]
    #[test]
    fn lossless_clamps_speed_into_registry_band() {
        let (lo, hi) = LOSSLESS_REGISTRY_SPEED_BAND.expect(
            "band const gone: this test should have been deleted with it (dep-bump checklist)",
        );
        let input = PlanInput::rgb8(256, 256);

        // Below the band → floor.
        let slow = EncoderConfig::new().speed(1).with_lossless(true);
        assert_eq!(slow.resolve_plan(input).speed_preset, lo);
        // Above the band → ceiling.
        let fast = EncoderConfig::new().speed(10).with_lossless(true);
        assert_eq!(fast.resolve_plan(input).speed_preset, hi);
        // Interior of the band → untouched.
        let mid = EncoderConfig::new().speed(hi).with_lossless(true);
        assert_eq!(mid.resolve_plan(input).speed_preset, hi);

        // Lossy encodes never clamp.
        let lossy = EncoderConfig::new().speed(1).quality(80.0);
        assert_eq!(lossy.resolve_plan(input).speed_preset, 1);

        // The derived search settings follow the effective preset, not
        // the requested one (speed 1's bottom-up search must not leak
        // into a clamped lossless plan).
        assert_eq!(
            slow.resolve_plan(input).speed.encode_bottomup,
            EncoderConfig::new()
                .speed(lo)
                .with_lossless(true)
                .resolve_plan(input)
                .speed
                .encode_bottomup
        );
    }

    #[cfg(feature = "encode-imazen")]
    #[test]
    fn override_patches_speed_derived() {
        let base = EncoderConfig::new().quality(30.0).speed(6);
        let plan = base.clone().resolve_plan(PlanInput::rgb8(256, 256));
        assert!(plan.speed.cdef, "preset: cdef on at low quality");
        let forced = base.with_cdef(Some(false));
        let plan2 = forced.resolve_plan(PlanInput::rgb8(256, 256));
        assert!(!plan2.speed.cdef);
    }
}

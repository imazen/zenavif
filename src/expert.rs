//! Expert-only knobs for codec calibration and picker training.
//!
//! Anything in this module is **unstable**: it may change in any patch
//! release without semver justification, and is **not part of the
//! public API contract**. Reach for it only when:
//!
//! 1. Sweeping parameter combinations to feed a picker / regression /
//!    calibration training pipeline.
//! 2. Diagnosing codec behaviour by overriding speed-preset defaults.
//! 3. Wiring a future `predict` feature that selects [`InternalParams`]
//!    via a baked MLP.
//!
//! Everything in here lives behind the `__expert` cargo feature, whose
//! double-underscore signals "private — do not depend on this in
//! production code." Default builds expose only stable public knobs
//! ([`crate::EncoderConfig::quality`], [`crate::EncoderConfig::speed`],
//! etc.).
//!
//! # Where the overrides land
//!
//! [`InternalParams`] is a thin mirror of
//! [`zenravif::expert::InternalParams`]. Fields fan out one-to-one
//! through `build_ravif_encoder()` in `src/encoder.rs`, which builds a
//! `zenravif::expert::InternalParams` and forwards it via
//! `zenravif::Encoder::with_internal_params`. From there each `Some(_)`
//! replaces the value the AV1 speed preset would have picked, **after**
//! `zenrav1e::prelude::SpeedSettings::from_preset` and **after**
//! zenravif's own preset overrides in `SpeedTweaks::from_my_preset`.
//! `None` falls through to whatever the preset chose. Apply via
//! [`crate::EncoderConfig::with_internal_params`]; the call replaces
//! *all* four fields wholesale, so reset by passing
//! `InternalParams::default()`.
//!
//! For the underlying mechanism, source citations into zenrav1e, and
//! the speed-preset gating tables, see
//! [`zenravif::expert::InternalParams`] (which the zenavif wrapper
//! forwards to verbatim).

/// Expert override knobs for the AVIF encoder.
///
/// Each field is `Option<T>`: `None` (the [`Default`]) keeps the speed
/// preset's value, `Some(_)` overrides it. Apply via
/// [`crate::EncoderConfig::with_internal_params`].
///
/// `#[non_exhaustive]` — fields may be added in any patch release.
/// Construct via [`Default::default`] and field-by-field assignment;
/// callers cannot use struct-literal syntax outside this crate.
///
/// Forwards to [`zenravif::expert::InternalParams`]; consult that
/// type's docs for source-line citations into the underlying
/// zenrav1e encoder.
///
/// # Example
///
/// ```ignore
/// # #[cfg(feature = "__expert")] {
/// use zenavif::{EncoderConfig, expert::InternalParams};
///
/// let mut params = InternalParams::default();
/// params.partition_range = Some((4, 16));
/// params.lrf = Some(false);
///
/// let config = EncoderConfig::new()
///     .quality(85.0)
///     .speed(6)
///     .with_internal_params(params);
/// # }
/// ```
#[non_exhaustive]
#[derive(Default, Clone, Debug)]
pub struct InternalParams {
    /// Partition block-size search range `(min, max)` in pixels. Each
    /// bound must be one of `{4, 8, 16, 32, 64}` and `min <= max`.
    /// (zenrav1e currently rejects `128` via a `max <= 64×64` debug
    /// assert; passing `128` triggers a debug-mode panic. The wider
    /// 128 path is reserved for future AV1 large-superblock support.)
    ///
    /// **Pipeline stage:** partition / mode decision. Drives the
    /// recursive AV1 superblock split during RDO. The bounds gate
    /// `must_split` / `can_split` decisions in zenrav1e's top-down
    /// and bottom-up partition encoders: `bsize > max` forces a
    /// split; `bsize > min` allows one. The encoder never tries
    /// blocks outside the range, so this knob both caps speed and
    /// constrains the achievable RD curve.
    ///
    /// **Why override:**
    /// - **Sharp text / screen content** benefits from `Some((4, 16))`
    ///   — small blocks track glyph edges, and large blocks waste bits
    ///   on transform coefficients that the entropy coder can't reuse.
    /// - **Smooth photos at q ≥ 85** benefit from `Some((16, 64))` or
    ///   `Some((32, 64))` — the 4×4/8×8 partitions never win RDO at
    ///   high q (they pay a partition-flag cost for no distortion
    ///   improvement) and disabling them shaves encode time.
    /// - **Calibration sweeps** want `Some((4, 64))` to expose the
    ///   full RD frontier so a picker can learn where the partition
    ///   boundaries live (`128` is rejected by zenrav1e — see above).
    ///
    /// **Mechanism:** the encoder's RDO loop picks the partition
    /// shape per superblock by recursing within `[min, max]`. Setting
    /// both bounds equal (e.g. `Some((16, 16))`) forces fixed-size
    /// blocks and skips partition RDO entirely. Bounds outside the
    /// speed preset's range can both expand and contract the search.
    ///
    /// **Speed-preset interaction:** zenravif's `SpeedTweaks` clamps
    /// the upper bound to 16 at high quality and reshapes the range
    /// per speed; underneath, `SpeedSettings::from_preset` widens to
    /// `(8, 64)` at speed 3 and shrinks to `(16, 32)` / `(32, 32)` at
    /// speed 9+. See [`zenravif::expert::InternalParams::partition_range`]
    /// for exact source citations.
    pub partition_range: Option<(u8, u8)>,

    /// Override intra prediction-mode search depth.
    /// `Some(true)` = `ComplexAll` (all intra modes searched on every
    /// frame). `Some(false)` = `Simple` (reduced mode set on every
    /// frame, plus `enable_filter_intra=false` in the AV1 sequence
    /// header).
    ///
    /// **Pipeline stage:** intra prediction / mode decision. Maps to
    /// zenrav1e's `PredictionModesSetting`, consumed in two RDO
    /// shortlist sites and at sequence-header build time to decide
    /// whether `enable_filter_intra` is signalled in the bitstream
    /// at all.
    ///
    /// **Why override:**
    /// - **Calibration sweeps** that need the full intra search to
    ///   measure the upper bound of intra-only RD: `Some(true)`.
    /// - **Diagnosing the still-image guard:** zenravif forces
    ///   `Simple` for stills because `ComplexAll` triggers
    ///   `filter_intra` RDO with broken cost estimation that costs
    ///   ~12 dB PSNR at speed 1 (zenrav1e#5). `Some(true)` lets you
    ///   reproduce or verify that regression. **Production stills
    ///   should leave this at `None`** — the override exists to
    ///   expose the bug, not hide it.
    /// - **Animated sequences** where the filter-intra bug is less
    ///   pronounced and the extra modes can recover RD on textured
    ///   inter frames.
    ///
    /// **Mechanism:** `Simple` searches a 3-mode intra shortlist;
    /// `ComplexKeyframes` (the speed-preset default at speed 0..=6)
    /// searches a 7-mode list on keyframes only; `ComplexAll`
    /// searches the 7-mode list on every frame and additionally
    /// enables filter-intra mode bits in the bitstream. For inter
    /// RDO, `ComplexAll` switches from a 9-mode shortlist to the
    /// full inter-mode set.
    ///
    /// **Speed-preset interaction:** zenrav1e's preset sets
    /// `ComplexAll` at speed 0..=1, `ComplexKeyframes` at speed
    /// 2..=6, and `Simple` at speed 7+. zenravif then **forces
    /// `Simple` regardless of speed for still images**. Setting this
    /// to `Some(true)` (=`ComplexAll`) defeats that guard.
    pub complex_prediction_modes: Option<bool>,

    /// Override loop restoration filter (LRF: Wiener + Self-Guided).
    /// `Some(true)` enables Wiener/SGR search and emits restoration
    /// units in the bitstream; `Some(false)` disables both and clears
    /// `enable_restoration` in the AV1 sequence header.
    ///
    /// **Pipeline stage:** post-filter (after deblock + CDEF, before
    /// frame output). LRF runs on the reconstructed frame and stores
    /// per-restoration-unit filter parameters in the bitstream. The
    /// flag gates whether Wiener/SGR searches run at all and whether
    /// restoration unit headers are written.
    ///
    /// **Why override:**
    /// - **Noisy DSLR / film captures at low q (q ≤ 50)**: `Some(true)`
    ///   recovers measurable PSNR by smoothing reconstruction error
    ///   that survives deblock+CDEF. The preset already enables LRF
    ///   here, so the override is for sweeps that need to A/B it.
    /// - **Smooth photos at q ≥ 85**: `Some(false)` saves encode time
    ///   with no measurable RD loss — at high q the residual energy
    ///   LRF would smooth is already below quantization noise.
    /// - **Line art / pixel art / sharp text**: `Some(false)` prevents
    ///   LRF from over-softening hard edges that survived deblock.
    ///
    /// **Mechanism:** when enabled, the encoder per-frame searches
    /// Wiener filter coefficients and SGR (self-guided) parameters
    /// per restoration unit (typically 64×64 or 256×256 pixels). The
    /// cost is RDO over both filter types plus the rate of
    /// signalling the chosen coefficients. SGR search depth is
    /// independently controlled by `sgr_complexity` (not exposed
    /// here). When disabled, `enable_restoration` in the sequence
    /// header is `0` and decoders skip the post-filter stage.
    ///
    /// **Speed-preset interaction:** zenrav1e enables LRF at speed
    /// 0..=7 and disables it at speed 8+. zenravif's `SpeedTweaks`
    /// then narrows that to `low_quality && speed <= 8` — i.e., LRF
    /// is only on when the quantizer is above ~150 (≈Q50 and below)
    /// AND speed ≤ 8. At Q ≥ 85 with any speed, the preset turns
    /// LRF off; this override is the way to flip it back on.
    pub lrf: Option<bool>,

    /// Override fast vs full deblock-filter level search.
    /// `Some(true)` = closed-form q-derived deblock level (fast).
    /// `Some(false)` = full SSE-driven search across deblock levels
    /// (slow, better edge preservation).
    ///
    /// **Pipeline stage:** post-filter (deblock filter level
    /// optimization, before CDEF). The flag is consumed inside
    /// zenrav1e's `deblock_filter_optimize`, which decides per
    /// frame what loop-filter level(s) the reconstruction pass
    /// will apply.
    ///
    /// **Why override:**
    /// - **Sharp text / screen content / line art**: `Some(false)`
    ///   keeps the SSE-driven search, which finds smaller deblock
    ///   levels and preserves the hard edges the closed-form formula
    ///   would over-smooth.
    /// - **Smooth photos / video where speed matters**: `Some(true)`
    ///   skips the per-frame search and uses a precomputed level. The
    ///   formula was fit on natural images; it produces the right
    ///   answer there but can over-blur or under-blur on outliers.
    /// - **Diagnosing edge artifacts**: flipping the flag is the
    ///   fastest way to confirm whether deblock-level search is the
    ///   cause.
    ///
    /// **Mechanism:** when fast, the level is computed in closed form
    /// from the AC quantizer and frame type via 8/10/12-bpc-specific
    /// fixed-point coefficients. When slow, `sse_optimize` searches
    /// deblock levels by reconstructing each 4×4 luma block and
    /// minimizing reconstruction SSE against the source. The slow
    /// path can run dozens of trial reconstructions per frame.
    ///
    /// **Speed-preset interaction:** zenrav1e enables `fast_deblock`
    /// at speed 7+. zenravif's `SpeedTweaks` further restricts that
    /// to `speed >= 7 && !high_quality` — i.e., at Q ≥ 80 the slow
    /// search runs even at speed 10. Override `Some(true)` if you
    /// want the fast path at high q, or `Some(false)` to force the
    /// slow search at any speed for edge-sensitive content.
    pub fast_deblock: Option<bool>,
}

// ============================================================================
// SVT-AV1 still-image knobs (`Av1Backend::SvtRs`)
// ============================================================================

/// Still-image encoder knobs for the [`crate::Av1Backend::SvtRs`] backend.
///
/// **Unstable surface** — same contract as [`InternalParams`]: this module is
/// explicitly not part of the public API and exists so a sweep / picker /
/// calibration pipeline can drive parameter combinations. Apply via
/// [`crate::EncoderConfig::with_svt_params`].
///
/// Every field's [`Default`] is **what the seam configures today**, so a
/// default `SvtParams` is byte-identical to not setting one at all. The
/// defaults are SVT-AV1 v4.2.0 *mainline* defaults (tune 1 = PSNR, QM off,
/// variance boost off, sharpness 0) — deliberately **not** the upstream
/// still-image recipe, which is `--tune 3` + variance boost + QM. Measuring
/// the distance between those two is the point of the knob sweep.
///
/// Fields map one-to-one onto `svtav1_encoder::pipeline::EncodePipeline`'s
/// `hdr` (a `HdrForkConfig`) and its tile fields. Knobs that the port
/// refuses, ignores or has no consumer for are deliberately absent — see
/// `zenmetrics/benchmarks/avif_knob_dossier_2026-09-01.md` §4.2 for the
/// refused/inert inventory and §8.1 for why these nine are the ones worth
/// sweeping.
///
/// # Tune is a super-factor
///
/// `tune` 3 (IQ) and 4 (MS-SSIM) **rewrite other fields of this struct** at
/// encode time via the port's own `HdrForkConfig::apply_tune_overrides`
/// (`enable_qm`, the QM levels, `sharpness`, the variance-boost trio, and for
/// IQ also `max_tx_size` — by qp — and `screen_content_mode`). Setting those
/// fields alongside `tune = 3` does not do what it looks like: the tune wins.
/// [`Self::resolved`] applies exactly that rewrite, which is what the sweep
/// planner fingerprints, so aliased spellings collapse to one cell instead of
/// being encoded repeatedly under different names.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvtParams {
    /// `--tune`: 0 = VQ, 1 = PSNR (default), 2 = SSIM, 3 = IQ (the only mode
    /// upstream marks still-image-only), 4 = MS-SSIM. Slot 5 is
    /// `TUNE_FILM_GRAIN` in this port's fork enum — **not** mainline's VMAF —
    /// and is not swept.
    pub tune: u8,
    /// `--enable-variance-boost`: per-64×64 delta-q that lowers qindex on
    /// low-variance superblocks. The main perceptual lever that is not the
    /// quantizer. Off in SVT mainline; forced on by tune 3/4.
    pub enable_variance_boost: bool,
    /// `--variance-boost-strength`, 1..=4. Upstream: "strength 3 is best for
    /// still images". **3 and 4 saturate to the same plan**, so there are
    /// three distinct levels. Out-of-range values panic in a release build
    /// (the port guards this with `debug_assert` only), so
    /// [`Self::clamped`] pins it — apply it before handing a value to the
    /// encoder.
    pub variance_boost_strength: u8,
    /// `--variance-octile`, 1..=8 (upstream recommends 4–7): how much of a
    /// superblock must be low-contrast to be boosted. Same release-mode
    /// out-of-range panic as `variance_boost_strength`; same clamp.
    pub variance_octile: u8,
    /// `--enable-qm`: quantization matrices. **Off by default in SVT**, in
    /// contrast to libaom, which turns them on for images.
    pub enable_qm: bool,
    /// `--qm-min`, 0..=15. Applied to luma and chroma alike, matching what
    /// the port's tune-IQ override does.
    pub min_qm_level: u8,
    /// `--qm-max`, 0..=15 (15 = identity).
    pub max_qm_level: u8,
    /// `--sharpness`. **Categorical, not a linear dial** — both backends'
    /// image tunes force 7, and the underlying behaviour is a set of discrete
    /// switches rather than a smooth ramp. The port clamps to 0..=7 at use,
    /// so negatives are indistinguishable from 0.
    pub sharpness: i8,
    /// `--scm`: `None` derives the screen-content mode from the preset (the
    /// default); `Some(3)` forces the anti-alias-aware detector on at any
    /// preset, enabling palette + IntraBC. Decisive on text/UI content.
    pub force_screen_content_mode: Option<u8>,
    /// `--ac-bias`, 0.0..=8.0: RD bias toward high-frequency error (texture
    /// and grain retention). Live in mainline; default 0.0.
    pub ac_bias: f64,
    /// `--max-tx-size`, **32 or 64 only** (the port hard-refuses anything
    /// else). 32 forbids 64×64 square transforms. Tune IQ selects it *by qp*
    /// (32 at qp ≤ 45), i.e. upstream's own optimum is quality-dependent.
    pub max_tx_size: u8,
    /// `--tile-columns` as log2. Encode + decode parallelism at an efficiency
    /// cost; unlike `threads` it moves bytes, so it is a modelled axis.
    pub tile_cols_log2: u8,
    /// `--tile-rows` as log2.
    pub tile_rows_log2: u8,
}

impl Default for SvtParams {
    fn default() -> Self {
        Self {
            tune: 1,
            enable_variance_boost: false,
            variance_boost_strength: 2,
            variance_octile: 5,
            enable_qm: false,
            min_qm_level: 8,
            max_qm_level: 15,
            sharpness: 0,
            force_screen_content_mode: None,
            ac_bias: 0.0,
            max_tx_size: 64,
            tile_cols_log2: 0,
            tile_rows_log2: 0,
        }
    }
}

impl SvtParams {
    /// `true` when every field is at its [`Default`] — i.e. this config asks
    /// for exactly what the seam does with no `SvtParams` at all.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// How many fields deviate from the default, counting the
    /// variance-boost trio and the QM triple as **one** deviation each
    /// (they are one knob with a compound value, not three independent
    /// ones — a design that counted them separately would spend its
    /// interaction budget crossing a knob with itself).
    // `ac_bias` is an f64, but its values are literal grid points copied from
    // the axis definition, never computed, so exact comparison is asking the
    // right question: "is this field still the default spelling?"
    #[allow(clippy::float_cmp)]
    #[must_use]
    pub fn deviations(&self) -> u8 {
        let d = Self::default();
        u8::from(self.tune != d.tune)
            + u8::from(
                self.enable_variance_boost != d.enable_variance_boost
                    || self.variance_boost_strength != d.variance_boost_strength
                    || self.variance_octile != d.variance_octile,
            )
            + u8::from(
                self.enable_qm != d.enable_qm
                    || self.min_qm_level != d.min_qm_level
                    || self.max_qm_level != d.max_qm_level,
            )
            + u8::from(self.sharpness != d.sharpness)
            + u8::from(self.force_screen_content_mode != d.force_screen_content_mode)
            + u8::from(self.ac_bias != d.ac_bias)
            + u8::from(self.max_tx_size != d.max_tx_size)
            + u8::from(
                self.tile_cols_log2 != d.tile_cols_log2 || self.tile_rows_log2 != d.tile_rows_log2,
            )
    }

    /// Clamp the two fields the port guards with `debug_assert` only.
    ///
    /// `variance_boost_strength` indexes a `[f64; 5]` and `variance_octile`
    /// feeds `octile * SUBBLOCKS_IN_OCTILE - 1`, both behind assertions that
    /// **vanish in a release build** — and every fleet worker is a release
    /// build. The seam clamps rather than refuses so a sweep cell can never
    /// take down a worker.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.variance_boost_strength = self.variance_boost_strength.clamp(1, 4);
        self.variance_octile = self.variance_octile.clamp(1, 8);
        self.min_qm_level = self.min_qm_level.min(15);
        self.max_qm_level = self.max_qm_level.min(15);
        self.max_tx_size = if self.max_tx_size == 32 { 32 } else { 64 };
        self.sharpness = self.sharpness.clamp(0, 7);
        self
    }

    /// The configuration the encoder will actually run: [`Self::clamped`]
    /// with the port's own tune overrides applied for the given CLI-domain
    /// `qp`.
    ///
    /// This is a **transcription** of `HdrForkConfig::apply_tune_overrides`
    /// (zenav1-svt `hdr_mode.rs`), restricted to the fields this struct
    /// carries. It is kept here rather than called through the port so the
    /// sweep planner can resolve a cell without an `encode-svt-rs` build; the
    /// test `resolved_matches_the_port_tune_overrides` (behind that feature)
    /// pins the two together.
    #[must_use]
    pub fn resolved(self, qp: u8) -> Self {
        let mut r = self.clamped();
        // TUNE_IQ (3) and TUNE_MS_SSIM (4) share this block.
        if r.tune == 3 || r.tune == 4 {
            r.enable_qm = true;
            r.min_qm_level = 4;
            r.max_qm_level = 10;
            r.sharpness = 7;
            r.enable_variance_boost = true;
            r.variance_boost_strength = 3;
        }
        // IQ only, on top of the shared block.
        if r.tune == 3 {
            r.max_tx_size = if qp <= 45 { 32 } else { 64 };
            r.force_screen_content_mode = Some(3);
        }
        r
    }
}

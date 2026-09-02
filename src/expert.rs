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
// SVT-AV1 still-image knobs (`Av1Backend::Zenav1Svt`)
// ============================================================================

// `SvtParams` is DEFINED in the private, always-compiled `crate::svt_params`
// module and re-exported here, because the `zenav1-svt` seam needs the type
// with `__expert` off. Publicly it is only ever reachable as
// `zenavif::expert::SvtParams`, so this re-export is the whole public surface.
pub use crate::svt_params::SvtParams;

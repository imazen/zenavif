//! Budgeted sweep-plan construction over the AVIF encoder knob space.
//!
//! The zenavif adoption of the variant-generation patterns (see
//! `docs/VARIANT_GENERATION.md` here and the reference write-up in
//! zenjpeg's `docs/VARIANT_GENERATION.md`). This module turns the knob
//! cross-product into a **finite, auditable list of encode cells**:
//!
//! 1. **Strata** — concrete values per axis ([`SweepAxes`]; curated
//!    defaults in [`SweepAxes::rd_core`] / [`SweepAxes::modes_full`]).
//!    Invalid combinations (`EncoderConfig::validate()`) are skipped
//!    and *reported*, never silently lost.
//! 2. **Quality grid** — [`QualityGrid`]: the step-5 floor for
//!    benchmarks, denser grids for training. Low-q coverage is never
//!    thinned preferentially.
//! 3. **Fingerprint dedup** — every cell gets a byte-identity
//!    [`fingerprint`] over its *resolved* state (quantizers, the
//!    qm×lossless gate, speed-derived search settings after overrides).
//!    Aliased spellings — an override equal to its preset value, two
//!    qualities on the same quantizer, a VAQ strength with VAQ off —
//!    collapse into one encode with the merged ids recorded.
//! 4. **Budget ladder** — [`SweepBuilder::with_budget`] reduces
//!    deterministically: collapse low-tier axes one value at a time
//!    (recorded in [`SweepPlan::dropped`]), then coarsen the quality
//!    grid uniformly (endpoints kept, never below 11 points), and
//!    finally set [`SweepPlan::over_budget`] rather than sample
//!    silently. No silent caps.
//! 5. **Queue ordering** — main-effects-first: the all-defaults
//!    stratum, then every single-deviation stratum, then interaction
//!    combos, milder deviations first; quality ascending *within* a
//!    stratum so an interrupted run never strands a half-measured RD
//!    curve. [`SweepCell::deviations`] exposes the priority class.
//!
//! # Reproducibility: every cell pins `threads = Some(1)`
//!
//! zenravif derives the AV1 **tile count** from the thread setting
//! (`min(threads, w·h / min_tile_size²)`), substituting the *host's*
//! core count when threads is unset — so default-config encodes are not
//! byte-reproducible across machines. Sweep cells therefore pin
//! `threads(Some(1))`: single tile on every host, and the config-only
//! [`fingerprint`] stays dimension-independent. Parallelize sweeps
//! across cells, not within them.
//!
//! # Scalar bounds and step provenance
//!
//! | knob | bound | curated steps (modes_full) | provenance |
//! |---|---|---|---|
//! | quality | 1–100 | grids in [`QualityGrid`] | step-5 floor / training-dense per the sweep discipline |
//! | speed | 1–10 | 4, 6, 2, 8 | 4 = zenavif default; 1,2,4,6 was the axis of the committed sweeps (`benchmarks/avif_encode_fine_sweep_2026-04-16.tsv`, picker phase1a); 8 probes the fast tail. Speed 1 omitted: ~4× the cost of 2 for marginal RD movement in the 2026-04-16 sweep |
//! | qm | on/off | true, false | ~10 % BD-rate win measured on 63-image CID22 stills (ravif 7265eea benchmarks; default on) |
//! | subsampling | 444/420 | Yuv444, Yuv420 | the classic AVIF rate knob (~25–35 % on photos); previously unsweepable — zenavif only exposed 4:4:4 before this module landed |
//! | bit_depth | 8/10 | Auto(→8 for RGB8 corpora), Ten | zenravif docs recommend 10-bit even for 8-bit input; sweep measures the claim |
//! | vaq strength (SCALAR) | 0.0–4.0, **1.0 = structural no-op** | Some(0.5) (axis); probes 2.0, 3.0, 0.25 | bound = zenravif validate range. Strength 1.0 is byte-identical to VAQ off under the psychovisual/still tunes (zenrav1e `api/internal.rs:1379`; the harness's first run caught the 1.0 axis value as an inert step across 24 encodes — the axis is `Option<f64>` now so the no-op spelling can't be curated by accident). Prior finding "VAQ hurts stills" (CLAUDE.md, ravif 7265eea) — steps exist to quantify, not to endorse. 0.0 stays out pending a semantics check (untested extreme of the rescale curve). **Still-envelope equivalence**: see the seg_boost row — values interleave with seg_boost's so the joint effective ladder {0.25, 0.5, 0.75, 1.5, 2.0, 2.5, 3.0, 4.0} is alias-free |
//! | seg_boost (SCALAR) | 0.5–4.0 | 1.5, 2.5, 0.75, 4.0 | bound = zenravif validate range (1.0 = off). 1.5/2.5 validated live by `sweep_validate` 2026-06-10; 0.75 (de-boost direction) + 4.0 (validate endpoint) added 2026-06-12 (dense-sweep program). **Still-envelope equivalence (proven by encode 2026-06-12, 28/28 cells × 3 value pairs)**: `seg_boost(x)` is byte-identical to `vaq_strength(x)` at equal x on still encodes — on intra-only frames the only byte-affecting consumer of `spatiotemporal_scores` is `segmentation_scores`, and both knobs are the same log-domain exponent on that chain (zenrav1e `internal.rs:1379` applies vaq^s to spatiotemporal, then seg = spatiotemporal^boost). The fingerprint deliberately under-merges the spellings (config-only; animated/inter encodes may diverge); the curated ladders interleave values instead so no two curated cells alias |
//! | trellis | on/off | off, on | zenrav1e Viterbi DP; default off in zenravif |
//! | deep-knob probes | see [`KnobProbe`] | one axis, single-deviation by construction | each probe flips one preset-derived setting both ways; fingerprint dedup removes the spellings that equal the preset |
//! | lru_on_skip | on/off | **not curated** | byte-inert on still-image encodes at speeds 2–8 across photos/noise/checker/gradient/flat-logo, q10–q85 (28/28 comparisons, `sweep_validate` 2026-06-10). The search-on-skip-LRUs path resolves to the same restoration decisions on intra-only content; the preset only enables it at speed ≤ 1, outside the curated speed set. [`KnobProbe::LruOnSkip`] remains available for explicit speed ≤ 1 sweeps |
//! | lossless | on/off | **not curated** | a different product mode (quantizer pinned 0, QM force-off), not a point on the lossy RD curve a picker optimizes over; sweep it as its own dedicated run if needed |
//! | quantizer (direct qp) | 0–255 | **axis blocked on encoder knob** | quality is the only public rate dial: neither zenavif's `EncoderConfig` nor zenravif exposes a direct-quantizer setter (`quality_to_quantizer` is internal mediation; zenravif `av1encoder.rs` has `with_quality`/`with_alpha_quality` only). Plans cannot pin qp until zenravif grows a `with_quantizer`-class `__expert` knob. The **resolved** quantizer is already the trained-on mediator via [`SweepCell::feature_row`] (`quantizer`/`alpha_quantizer` columns), so pickers see it today |
//! | alpha_quality delta | result clamps to 1–100 | ±25, [`modes_full_alpha`](SweepAxes::modes_full_alpha) only | expressed as a **delta against the grid q** to dodge the absolute-value-vs-moving-grid trap (zenjpeg's `chroma_quality` lesson); ±25 probes both the "alpha cheaper" and "alpha cleaner" directions. Validated live on alpha content / byte-inert on RGB by the harness's RGBA leg, 2026-06-11 |
//! | alpha_color_mode | Clean/Dirty/Premultiplied | Dirty, Premultiplied probes ([`modes_full_alpha`](SweepAxes::modes_full_alpha) only) | pixel-changing on alpha content (Clean rewrites color under transparency; Premultiplied rescales). Validated live on alpha content by the harness's RGBA leg, 2026-06-11 |
//!
//! Empirical validation harness: `examples/sweep_validate.rs`. It
//! encodes the default stratum + every single-deviation stratum on
//! mixed content and fails hard on inert steps, fingerprint-contract
//! violations, and ordering breakage. Re-run it after touching the
//! curated axes, the fingerprint, or the zenravif dependency version;
//! commit the TSV next to the run date.
//!
//! The plan is **per config-cell**; multiply by corpus images and size
//! buckets with [`SweepPlan::encodes`]. Persistence of encoded bytes
//! and metric scoring belong to the harness consuming the plan, not
//! here.

#![cfg(feature = "__expert")]

use crate::encode_plan::{
    PlanInput, TilesResolution, apply_overrides, quality_to_quantizer, speed_derived,
};
use crate::encoder::{
    EncodeAlphaMode, EncodeBitDepth, EncodeChromaSubsampling, EncodeColorModel, EncodePixelRange,
    EncoderConfig,
};

// ============================================================================
// Axes
// ============================================================================

/// One single-deviation probe of a deep knob. All probes share one axis
/// so the cross product stays main-effects-shaped: a probe never
/// combines with another probe, only with the primary axes.
///
/// `Preset`-equal spellings (e.g. `Cdef(true)` at a low-quality point
/// where the preset already enables CDEF) dedupe away by fingerprint.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum KnobProbe {
    /// No probe — the primary axes as configured.
    None,
    /// Override CDEF on/off.
    Cdef(bool),
    /// Override RDO transform-type search.
    RdoTxDecision(bool),
    /// Override SGR complexity Full (16 sets) vs Reduced (8).
    SgrFull(bool),
    /// Override loop-restoration search on skip blocks. Not in the
    /// curated [`SweepAxes::modes_full`] set: byte-inert on still-image
    /// encodes at speeds 2–8 (28/28 comparisons incl. skip-heavy flat
    /// content, `sweep_validate` 2026-06-10). Probe it explicitly when
    /// sweeping speed ≤ 1, where the preset turns it on.
    LruOnSkip(bool),
    /// Override segmentation Complex (k-means) vs Simple.
    SegmentationComplex(bool),
    /// Override bottom-up partition search.
    EncodeBottomup(bool),
    /// `Tune::StillImage` instead of `Tune::Psychovisual`.
    TuneStillImage,
    /// Segmentation boost (1.0 = off; valid 0.5–4.0).
    SegBoost(f64),
    /// VAQ at the given strength (forces VAQ on; valid 0.0–4.0).
    VaqStrength(f64),
    /// Partition block-size range override (`__expert` depth).
    PartitionRange(u8, u8),
    /// Full prediction-mode search (`ComplexAll`). Reproduces the
    /// zenrav1e#5 filter-intra regression on purpose — a sweep probe,
    /// not a default candidate.
    ComplexPredictionModes(bool),
    /// Override loop restoration filter.
    Lrf(bool),
    /// Override fast vs full deblock search.
    FastDeblock(bool),
    /// Alpha quality as a **delta against the cell's quality**, clamped
    /// to 1–100. A delta rather than an absolute value: the grid moves
    /// q, so any static absolute alpha quality would be wrong at most
    /// grid points (the trap zenjpeg documents for
    /// `MozjpegRobidoux::chroma_quality`). `Delta(0.0)` is the
    /// follow-color spelling and fingerprint-aliases the no-probe cell.
    /// Only meaningful on alpha-bearing corpora — see
    /// [`SweepAxes::modes_full_alpha`].
    AlphaQualityDelta(f32),
    /// Alpha handling mode (default `UnassociatedClean`). Pixel-changing
    /// on alpha-bearing input. Only meaningful on alpha-bearing corpora.
    AlphaMode(crate::EncodeAlphaMode),
}

impl KnobProbe {
    /// Apply this probe's override onto a config — the same application
    /// the planner uses when building cells. Public so harnesses can
    /// exercise individual probes outside a full plan (e.g. the
    /// validation harness's RGBA alpha leg).
    // `InternalParams` is #[non_exhaustive]: struct-literal construction
    // (clippy's suggested `..Default::default()` form) is not available
    // outside the defining module, so Default + field assignment is the
    // only spelling — same as build_ravif_encoder's.
    #[allow(clippy::field_reassign_with_default)]
    #[must_use]
    pub fn apply(self, cfg: EncoderConfig) -> EncoderConfig {
        match self {
            Self::None => cfg,
            Self::Cdef(v) => cfg.with_cdef(Some(v)),
            Self::RdoTxDecision(v) => cfg.with_rdo_tx_decision(Some(v)),
            Self::SgrFull(v) => cfg.with_sgr_full(Some(v)),
            Self::LruOnSkip(v) => cfg.with_lru_on_skip(Some(v)),
            Self::SegmentationComplex(v) => cfg.with_segmentation_complex(Some(v)),
            Self::EncodeBottomup(v) => cfg.with_encode_bottomup(Some(v)),
            Self::TuneStillImage => cfg.with_still_image_tuning(true),
            Self::SegBoost(b) => cfg.with_seg_boost(Some(b)),
            Self::VaqStrength(s) => cfg.with_vaq(true, s),
            Self::PartitionRange(min, max) => {
                let mut params = crate::expert::InternalParams::default();
                params.partition_range = Some((min, max));
                cfg.with_internal_params(params)
            }
            Self::ComplexPredictionModes(v) => {
                let mut params = crate::expert::InternalParams::default();
                params.complex_prediction_modes = Some(v);
                cfg.with_internal_params(params)
            }
            Self::Lrf(v) => {
                let mut params = crate::expert::InternalParams::default();
                params.lrf = Some(v);
                cfg.with_internal_params(params)
            }
            Self::FastDeblock(v) => {
                let mut params = crate::expert::InternalParams::default();
                params.fast_deblock = Some(v);
                cfg.with_internal_params(params)
            }
            Self::AlphaQualityDelta(d) => {
                let aq = (cfg.quality_value() + d).clamp(1.0, 100.0);
                cfg.alpha_quality(aq)
            }
            Self::AlphaMode(m) => cfg.alpha_color_mode(m),
        }
    }

    fn label(self) -> String {
        fn b(v: bool) -> &'static str {
            if v { "1" } else { "0" }
        }
        match self {
            Self::None => String::new(),
            Self::Cdef(v) => format!("-cdef{}", b(v)),
            Self::RdoTxDecision(v) => format!("-rdotx{}", b(v)),
            Self::SgrFull(v) => format!("-sgr{}", b(v)),
            Self::LruOnSkip(v) => format!("-lru{}", b(v)),
            Self::SegmentationComplex(v) => format!("-segcx{}", b(v)),
            Self::EncodeBottomup(v) => format!("-bup{}", b(v)),
            Self::TuneStillImage => "-still".into(),
            Self::SegBoost(v) => format!("-sb{v}"),
            Self::VaqStrength(s) => format!("-vaqs{s}"),
            Self::PartitionRange(min, max) => format!("-part{min}.{max}"),
            Self::ComplexPredictionModes(v) => format!("-cpred{}", b(v)),
            Self::Lrf(v) => format!("-lrf{}", b(v)),
            Self::FastDeblock(v) => format!("-fdb{}", b(v)),
            Self::AlphaQualityDelta(d) => format!("-aqd{d}"),
            Self::AlphaMode(m) => match m {
                crate::EncodeAlphaMode::UnassociatedClean => "-aclean".into(),
                crate::EncodeAlphaMode::UnassociatedDirty => "-adirty".into(),
                crate::EncodeAlphaMode::Premultiplied => "-aprem".into(),
            },
        }
    }
}

/// Concrete values per categorical axis. The cross product of these,
/// times the quality grid, is the candidate cell set. Axis vectors are
/// ordered **most-important-first**: the budget ladder sheds from the
/// tail, so put the value you'd keep under any budget at index 0.
#[derive(Clone, Debug)]
pub struct SweepAxes {
    /// Speed presets (1–10).
    pub speeds: Vec<u8>,
    /// Quantization matrices on/off.
    pub qm: Vec<bool>,
    /// Chroma subsampling.
    pub subsampling: Vec<EncodeChromaSubsampling>,
    /// Output bit depth.
    pub bit_depths: Vec<EncodeBitDepth>,
    /// Internal color model.
    pub color_models: Vec<EncodeColorModel>,
    /// Variance adaptive quantization: `None` = off, `Some(strength)`
    /// = on at that strength. Strength 1.0 is NOT a useful "on" value —
    /// it is structurally byte-identical to off (the psychovisual/still
    /// tunes always compute the activity mask and zenrav1e skips the
    /// rescale at 1.0; zenrav1e `api/internal.rs:1379`) — the planner
    /// would just dedupe it away.
    pub vaq: Vec<Option<f64>>,
    /// Trellis quantization on/off.
    pub trellis: Vec<bool>,
    /// Deep-knob single-deviation probes.
    pub probes: Vec<KnobProbe>,
}

impl SweepAxes {
    /// The axes that move the rate-distortion front, with everything
    /// else at production defaults: speeds {4, 6, 2} × qm {on, off} ×
    /// subsampling {4:4:4, 4:2:0} × bit depth {8, 10}.
    #[must_use]
    pub fn rd_core() -> Self {
        Self {
            speeds: vec![4, 6, 2],
            qm: vec![true, false],
            subsampling: vec![
                EncodeChromaSubsampling::Yuv444,
                EncodeChromaSubsampling::Yuv420,
            ],
            bit_depths: vec![EncodeBitDepth::Auto, EncodeBitDepth::Ten],
            color_models: vec![EncodeColorModel::YCbCr],
            vaq: vec![None],
            trellis: vec![false],
            probes: vec![KnobProbe::None],
        }
    }

    /// Every user-disableable mode axis on top of
    /// [`rd_core`](Self::rd_core): the RGB color model, VAQ, trellis,
    /// speed 8, and the deep-knob probe set (each preset-derived search
    /// setting forced both ways, tune, seg-boost and VAQ-strength
    /// steps). Large — pair with [`SweepBuilder::with_budget`].
    #[must_use]
    pub fn modes_full() -> Self {
        let mut axes = Self::rd_core();
        axes.speeds.push(8);
        axes.color_models.push(EncodeColorModel::Rgb);
        // 0.5 was the strength zenravif's still_image_preset documents;
        // 2.0 lives on the probe axis. 1.0 is the structural no-op.
        axes.vaq.push(Some(0.5));
        axes.trellis.push(true);
        axes.probes = vec![
            KnobProbe::None,
            // Preset-derived search settings, both ways each. The
            // spelling matching the preset value at a given (speed, q)
            // dedupes away by fingerprint; the other one measures the
            // knob.
            KnobProbe::Cdef(true),
            KnobProbe::Cdef(false),
            KnobProbe::RdoTxDecision(true),
            KnobProbe::RdoTxDecision(false),
            KnobProbe::SgrFull(true),
            KnobProbe::SgrFull(false),
            // LruOnSkip is deliberately absent: byte-inert on
            // still-image encodes at every curated speed (28/28
            // comparisons incl. flat skip-heavy content,
            // sweep_validate 2026-06-10) — see the provenance table.
            KnobProbe::SegmentationComplex(true),
            KnobProbe::SegmentationComplex(false),
            KnobProbe::EncodeBottomup(true),
            KnobProbe::EncodeBottomup(false),
            KnobProbe::TuneStillImage,
            // Scalar steps — bounds from zenravif's validate ranges; see
            // the module-docs provenance table.
            KnobProbe::SegBoost(1.5),
            KnobProbe::SegBoost(2.5),
            // VaqStrength(0.5) would alias the vaq-axis Some(0.5)
            // stratum; only the 2.0 step adds information here.
            KnobProbe::VaqStrength(2.0),
            // __expert depth.
            KnobProbe::PartitionRange(4, 16),
            KnobProbe::PartitionRange(16, 64),
            KnobProbe::ComplexPredictionModes(true),
            KnobProbe::Lrf(true),
            KnobProbe::Lrf(false),
            KnobProbe::FastDeblock(true),
            KnobProbe::FastDeblock(false),
            // SCALAR ladder densification (dense-sweep program,
            // 2026-06-12) — appended after the established probe set so
            // the budget ladder (tail-shed) drops the newest values
            // first. The values interleave with the established steps
            // to keep the EFFECTIVE still-image dial alias-free:
            // vaq_strength(x) and seg_boost(x) are byte-identical at
            // equal x on still encodes (both reduce to the same
            // log-domain exponent on spatiotemporal_scores →
            // segmentation_scores; zenrav1e internal.rs:1379 +
            // encoder.rs apply_vaq_strength/compute_segmentation_scores
            // — proven 28/28 by the 2026-06-12 harness on three value
            // pairs), so the union ladder {0.25, 0.5v, 0.75s, 1.5s,
            // 2.0v, 2.5s, 3.0v, 4.0s} covers 8 distinct effective
            // values with zero duplicate encodes. Bounds + exclusions
            // (vaq 1.0 no-op, vaq 0.0 pending semantics) in the
            // module-docs provenance table.
            KnobProbe::VaqStrength(3.0),
            KnobProbe::SegBoost(0.75),
            KnobProbe::VaqStrength(0.25),
            KnobProbe::SegBoost(4.0),
        ];
        axes
    }

    /// [`modes_full`](Self::modes_full) plus the alpha-plane probes,
    /// for **alpha-bearing (RGBA) corpora**: alpha-quality deltas ±25
    /// against the grid q and the non-default alpha handling modes.
    ///
    /// Kept out of `modes_full` deliberately: on RGB corpora no alpha
    /// plane is emitted, every alpha probe is byte-inert there, and the
    /// validation harness would (correctly) flag them as dead steps.
    /// The harness instead validates these probes on an RGBA leg —
    /// live on alpha content, byte-inert on RGB.
    #[must_use]
    pub fn modes_full_alpha() -> Self {
        let mut axes = Self::modes_full();
        axes.probes.extend(Self::alpha_probes());
        axes
    }

    /// The curated alpha-plane probes (see
    /// [`modes_full_alpha`](Self::modes_full_alpha)).
    #[must_use]
    pub fn alpha_probes() -> Vec<KnobProbe> {
        vec![
            // Deltas, not absolutes — the grid moves q (see
            // KnobProbe::AlphaQualityDelta). ±25 spans the "alpha
            // cheaper than color" and "alpha cleaner than color"
            // directions; Delta(0.0) is the no-op spelling and would
            // dedupe away.
            KnobProbe::AlphaQualityDelta(-25.0),
            KnobProbe::AlphaQualityDelta(25.0),
            KnobProbe::AlphaMode(crate::EncodeAlphaMode::UnassociatedDirty),
            KnobProbe::AlphaMode(crate::EncodeAlphaMode::Premultiplied),
        ]
    }

    /// Dense, isolated single-axis ladders over the CONTINUOUS knobs,
    /// every CATEGORICAL axis pinned to its production default — the data
    /// a trained **scalar head** (a per-knob continuous regression in the
    /// picker pipeline) needs to fit `knob_value × quality → outcome`
    /// (VARIANT_GENERATION patterns 17–18). Unlike
    /// [`modes_full`](Self::modes_full) (which crosses every mode to map
    /// *interactions* and explodes combinatorially), this preset samples
    /// each continuous axis *densely enough to fit a curve* while leaving
    /// the others at default, so a
    /// [`SweepBuilder::with_max_deviations`]`(1)` plan is one isolated
    /// ladder per knob — not a cartesian blow-up.
    ///
    /// Continuous axes covered (bounds/provenance in the module-docs
    /// table):
    ///
    /// - **speed** — the dense `2..=10` ladder (default `4` first), every
    ///   step the user can dial. Speed is the dominant term of
    ///   [`compute_tier`] (and is INVERTED: a *higher* speed number is
    ///   *faster*/cheaper, so the dense speed ladder is also the dense
    ///   *compute* axis — each step is a distinct tier). Speed `1` is
    ///   omitted (≈4× the cost of `2` for marginal RD movement, per the
    ///   2026-04-16 sweep) and `0` is below the user range.
    /// - **VAQ strength** — the `vaq` axis (`Option<f64>`): default `None`
    ///   (off) first, then a dense strength ladder. `1.0` is excluded —
    ///   it is the structural no-op (byte-identical to off; zenrav1e
    ///   `api/internal.rs:1379`) and would dedupe back onto the default
    ///   cell, so the ladder never double-encodes the default.
    /// - **seg_boost** — the [`KnobProbe::SegBoost`] scalar: a dense
    ///   ladder spanning the de-boost (`< 1.0`) and boost (`> 1.0`)
    ///   directions. `1.0` (= off) is excluded for the same no-op reason.
    ///   Its values stay value-DISJOINT from the VAQ-strength ladder:
    ///   `seg_boost(x)` and `vaq_strength(x)` are byte-identical at equal
    ///   `x` on still encodes (the still-envelope equivalence proven by
    ///   the 2026-06-12 harness — both are the same log-domain exponent
    ///   on `spatiotemporal_scores`), so a shared value would be a
    ///   duplicate encode the fingerprint deliberately does not merge.
    ///
    /// Pair with [`SweepBuilder::with_max_deviations`]`(1)` and a dense
    /// quality grid ([`QualityGrid::TrainingDense`]) for one clean
    /// per-knob response curve across the quality range.
    #[must_use]
    pub fn scalar_dense() -> Self {
        // CATEGORICAL axes pinned to their production defaults (the
        // index-0 / default stratum): speed-default qm ON, 4:4:4, Auto
        // bit-depth, YCbCr, trellis OFF, no deep-knob probes beyond the
        // seg_boost ladder. Every cell is therefore a single deviation on
        // exactly one continuous axis.
        let mut speeds = vec![4u8];
        // Dense compute/RD ladder over the usable speed range (default 4
        // first, then the rest of 2..=10). Each value is a distinct
        // compute_tier (speed is inverted — higher = faster = lower tier).
        speeds.extend([2u8, 3, 5, 6, 7, 8, 9, 10]);

        // VAQ-strength ladder on the `vaq` axis: None (default/off) first,
        // then dense strengths. 1.0 excluded (structural no-op — aliases
        // off). Values chosen disjoint from the seg_boost ladder below.
        let mut vaq: Vec<Option<f64>> = vec![None];
        vaq.extend(
            [0.25f64, 0.5, 0.75, 1.5, 2.0, 2.5, 3.0, 4.0]
                .into_iter()
                .map(Some),
        );

        // seg_boost ladder on the probe axis: None (default/off) first,
        // then dense boosts/de-boosts. 1.0 excluded (= off). Disjoint from
        // the vaq strengths so the union effective dial is alias-free.
        let probes = vec![
            KnobProbe::None,
            KnobProbe::SegBoost(0.6),
            KnobProbe::SegBoost(0.8),
            KnobProbe::SegBoost(1.2),
            KnobProbe::SegBoost(1.6),
            KnobProbe::SegBoost(2.2),
            KnobProbe::SegBoost(2.8),
            KnobProbe::SegBoost(3.4),
            KnobProbe::SegBoost(4.0),
        ];

        Self {
            speeds,
            qm: vec![true],
            subsampling: vec![EncodeChromaSubsampling::Yuv444],
            bit_depths: vec![EncodeBitDepth::Auto],
            color_models: vec![EncodeColorModel::YCbCr],
            vaq,
            trellis: vec![false],
            probes,
        }
    }
}

// ============================================================================
// Quality grid
// ============================================================================

/// Quality grids per the sweep discipline. Low-q density is never below
/// high-q density.
#[derive(Clone, Debug)]
pub enum QualityGrid {
    /// q ∈ {1, 5, 10, …, 100} — the 21-point floor for benchmarks and
    /// anchor tables.
    Step5,
    /// Step 5 through q65, step 2 from q70 — the training-density grid
    /// (31 points).
    TrainingDense,
    /// Caller-provided points (kept in the given order, deduplicated).
    Explicit(Vec<f32>),
}

impl QualityGrid {
    /// Materialize the grid points.
    #[must_use]
    pub fn points(&self) -> Vec<f32> {
        match self {
            Self::Step5 => {
                let mut v = vec![1.0];
                v.extend((1..=20).map(|i| (i * 5) as f32));
                v
            }
            Self::TrainingDense => {
                let mut v = vec![1.0];
                v.extend((1..=13).map(|i| (i * 5) as f32)); // 5..=65
                v.extend((35..=50).map(|i| (i * 2) as f32)); // 70..=100
                v
            }
            Self::Explicit(pts) => {
                let mut v = Vec::new();
                for &p in pts {
                    if !v.contains(&p) {
                        v.push(p);
                    }
                }
                v
            }
        }
    }
}

// ============================================================================
// Plan output
// ============================================================================

/// One encode cell: a fully-built config at one quality point.
#[derive(Clone, Debug)]
pub struct SweepCell {
    /// Stable human-readable id (speed/flags/probe/q tokens).
    pub id: String,
    /// The config to encode with (quality applied, `threads` pinned to
    /// `Some(1)` for cross-machine reproducibility).
    pub config: EncoderConfig,
    /// The quality point.
    pub quality: f32,
    /// Byte-identity fingerprint of the resolved state. Cells with
    /// equal fingerprints produce identical bytes for the same input.
    pub fingerprint: u64,
    /// Ids of candidate cells merged into this one (identical
    /// fingerprints).
    pub aliases: Vec<String>,
    /// How many axes deviate from the default stratum (index 0 of every
    /// axis). 0 = the production-default cell; 1 = a main-effect probe;
    /// ≥2 = interaction combos. Cells are emitted in ascending order.
    pub deviations: u8,
}

/// Column names for [`SweepCell::feature_row`], in order.
///
/// The training-side contract for picker / MLP pipelines (zentrain):
/// one numeric column per knob, resolved state where a mediator exists
/// (`quantizer`, not just `quality`; the post-override speed-derived
/// search settings, not the `Option` override spellings). Booleans are
/// 0/1; enums are the small stable integers documented on
/// [`SweepCell::feature_row`]. Append-only across zenavif versions —
/// training data keyed by column name stays joinable.
#[must_use]
pub fn feature_columns() -> &'static [&'static str] {
    &[
        "quality",
        "quantizer",
        "alpha_quantizer",
        "speed",
        "qm",
        "vaq",
        "vaq_strength",
        "tune_still_image",
        "lossless",
        "seg_boost",
        "trellis",
        "bit_depth",
        "color_model",
        "chroma_subsampling",
        "pixel_range",
        "alpha_color_mode",
        "partition_min",
        "partition_max",
        "complex_prediction_modes",
        "sgr_complexity_full",
        "encode_bottomup",
        "rdo_tx_decision",
        "reduced_tx_set",
        "fine_directional_intra",
        "fast_deblock",
        "lrf",
        "cdef",
        "inter_tx_split",
        "tx_domain_rate",
        "segmentation_complex",
        "lru_on_skip",
        "non_square_partition_max",
        "min_tile_size",
        "tiles",
    ]
}

impl SweepCell {
    /// Numeric knob vector for ML training, in [`feature_columns`]
    /// order, resolved against `input` through the same path the
    /// encoder runs ([`EncoderConfig::resolve_plan`]).
    ///
    /// Encodings: booleans 0/1; `alpha_quantizer` −1 when the input
    /// declares no alpha; `color_model` YCbCr=0 / Rgb=1;
    /// `chroma_subsampling` 4:4:4=0 / 4:2:0=1; `pixel_range` Full=0 /
    /// Limited=1; `alpha_color_mode` Clean=0 / Dirty=1 /
    /// Premultiplied=2; `tiles` −1 when machine-dependent (never the
    /// case for planner cells, which pin `threads`).
    ///
    /// Resolved values are deliberately preferred over config
    /// spellings: a model trained on `quantizer` and the post-override
    /// search settings generalizes across the aliases the fingerprint
    /// merges, instead of learning that q 80.0 and q 80.2 are
    /// different inputs.
    #[must_use]
    pub fn feature_row(&self, input: PlanInput) -> Vec<f64> {
        let plan = self.config.resolve_plan(input);
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        let row = vec![
            f64::from(self.quality),
            f64::from(plan.quantizer),
            plan.alpha_quantizer.map_or(-1.0, f64::from),
            f64::from(plan.speed_preset),
            b(plan.qm),
            b(plan.vaq),
            plan.vaq_strength,
            b(plan.tune_still_image),
            b(plan.lossless),
            plan.seg_boost,
            b(plan.trellis),
            f64::from(plan.bit_depth),
            match plan.color_model {
                EncodeColorModel::YCbCr => 0.0,
                EncodeColorModel::Rgb => 1.0,
            },
            match plan.chroma_subsampling {
                EncodeChromaSubsampling::Yuv444 => 0.0,
                EncodeChromaSubsampling::Yuv420 => 1.0,
            },
            match plan.pixel_range {
                EncodePixelRange::Full => 0.0,
                EncodePixelRange::Limited => 1.0,
            },
            match plan.alpha_color_mode {
                EncodeAlphaMode::UnassociatedClean => 0.0,
                EncodeAlphaMode::UnassociatedDirty => 1.0,
                EncodeAlphaMode::Premultiplied => 2.0,
            },
            f64::from(plan.speed.partition_range.0),
            f64::from(plan.speed.partition_range.1),
            b(plan.speed.complex_prediction_modes),
            b(plan.speed.sgr_complexity_full),
            b(plan.speed.encode_bottomup),
            b(plan.speed.rdo_tx_decision),
            b(plan.speed.reduced_tx_set),
            b(plan.speed.fine_directional_intra),
            b(plan.speed.fast_deblock),
            b(plan.speed.lrf),
            b(plan.speed.cdef),
            b(plan.speed.inter_tx_split),
            b(plan.speed.tx_domain_rate),
            b(plan.speed.segmentation_complex),
            b(plan.speed.lru_on_skip),
            f64::from(plan.speed.non_square_partition_max),
            f64::from(plan.speed.min_tile_size),
            match plan.tiles {
                TilesResolution::Fixed(n) => n as f64,
                TilesResolution::MachineDependent { .. } => -1.0,
            },
        ];
        debug_assert_eq!(row.len(), feature_columns().len());
        row
    }
}

/// A mode axis collapsed by the budget ladder.
#[derive(Clone, Debug)]
pub struct DroppedAxis {
    /// Axis name.
    pub axis: &'static str,
    /// The values kept (Debug-rendered).
    pub kept: String,
    /// The values dropped (Debug-rendered).
    pub dropped: Vec<String>,
}

/// The finite, auditable sweep plan.
#[derive(Clone, Debug)]
pub struct SweepPlan {
    /// Deduplicated encode cells.
    pub cells: Vec<SweepCell>,
    /// Stratum ids rejected by `EncoderConfig::validate()` (e.g.
    /// 4:2:0 × RGB).
    pub invalid_skipped: Vec<String>,
    /// Cell ids dropped because their [`compute_tier`] exceeded the
    /// [`SweepBuilder::with_compute_limit`] budget — the explicit
    /// no-silent-caps report for the compute constraint (empty when no
    /// limit was set).
    pub compute_tier_skipped: Vec<String>,
    /// Mode axes collapsed to fit the budget — the explicit
    /// no-silent-caps report.
    pub dropped: Vec<DroppedAxis>,
    /// Candidate cells merged by fingerprint identity.
    pub duplicates_merged: usize,
    /// How many times the quality grid was uniformly coarsened.
    pub q_coarsenings: u32,
    /// The budget could not be met even after the full reduction
    /// ladder. The plan is complete (nothing was sampled away); the
    /// caller decides whether to spend or cut axes manually.
    pub over_budget: bool,
}

impl SweepPlan {
    /// Total encodes when this plan runs over a corpus: cells × images ×
    /// size buckets.
    #[must_use]
    pub fn encodes(&self, images: usize, size_buckets: usize) -> usize {
        self.cells.len() * images * size_buckets
    }
}

// ============================================================================
// Compute-resource tier
// ============================================================================

/// Coarse compute-cost tier of a config (`0` = cheapest). Higher tiers
/// run more encoder passes or wider searches and cost more CPU per
/// encode. Used by [`SweepBuilder::with_compute_limit`] to keep a sweep
/// inside a compute budget, and public so the fleet harness and pickers
/// can bound their candidate set the same way zenavif's `auto_tune`
/// bounds its speed range. It is an **ordinal proxy**, not a calibrated
/// millisecond estimate — compare tiers, don't read absolute cost into
/// them.
///
/// ## The AV1 `speed` dial is INVERTED
///
/// zenravif's `speed` runs `1..=10` where **higher `speed` = FASTER =
/// LESS compute** (`speed` 1 is slowest/best, 10 is fastest/worst — see
/// [`EncoderConfig::speed`]). Lower speed numbers unlock the expensive
/// searches (`sgr_complexity_full`/`segmentation_complex` at speed ≤ 2,
/// `rdo_tx_decision` at speed ≤ 4, wider partition ranges, etc. —
/// `encode_plan::speed_derived`), so a tier that *increased* with the
/// `speed` number would be exactly backwards. The tier therefore inverts
/// it: `MAX_SPEED − speed`, so slow/expensive `speed` 2 ⇒ a large base
/// tier (`8`) and fast/cheap `speed` 10 ⇒ `0`.
///
/// ## Mapping
///
/// - **speed** (dominant term): `MAX_SPEED.saturating_sub(speed)` with
///   `MAX_SPEED = 10` — so `speed` 10 → +0, 8 → +2, 4 → +6, 2 → +8.
/// - **trellis** on: **+2** — the zenrav1e Viterbi coefficient search is
///   a full extra optimization pass over every block.
/// - **QM** on: **+1** — quantization-matrix derivation/application is a
///   real (small) additional pass.
///
/// VAQ/seg_boost add no term: under the psychovisual/still tunes
/// zenravif always uses, the activity mask is computed regardless, and
/// these knobs only rescale it (no extra pass) — see
/// [`EncoderConfig::vaq`](struct.EncoderConfig.html) / the module-docs
/// table.
#[must_use]
pub fn compute_tier(config: &EncoderConfig) -> u8 {
    /// zenravif's maximum (fastest/cheapest) speed preset; the tier
    /// inverts against this so the number is monotone in CPU cost.
    const MAX_SPEED: u8 = 10;
    let mut tier = MAX_SPEED.saturating_sub(config.speed_value());
    if config.trellis_effective() {
        tier = tier.saturating_add(2);
    }
    if config.qm_effective() {
        tier = tier.saturating_add(1);
    }
    tier
}

// ============================================================================
// Builder
// ============================================================================

/// Builds a [`SweepPlan`] from axes × quality grid under an optional
/// encode-cell budget.
#[derive(Clone, Debug)]
pub struct SweepBuilder {
    axes: SweepAxes,
    grid: QualityGrid,
    budget: Option<usize>,
    compute_limit: Option<u8>,
    max_deviations: Option<u8>,
}

impl SweepBuilder {
    /// New builder over the given axes and quality grid.
    #[must_use]
    pub fn new(axes: SweepAxes, grid: QualityGrid) -> Self {
        Self {
            axes,
            grid,
            budget: None,
            compute_limit: None,
            max_deviations: None,
        }
    }

    /// Cap the number of (deduplicated) cells. The reduction ladder:
    /// collapse axes lowest-tier-first (probes, vaq, trellis,
    /// color_models, bit_depths, speeds down to two), then coarsen the
    /// quality grid (uniformly, endpoints kept, ≥ 11 points). The qm
    /// and subsampling axes are never collapsed. Every reduction is
    /// recorded.
    #[must_use]
    pub fn with_budget(mut self, max_cells: usize) -> Self {
        self.budget = Some(max_cells);
        self
    }

    /// Constrain the plan to cells whose [`compute_tier`] is `<= max_tier`,
    /// dropping the more expensive cells (recorded in
    /// [`SweepPlan::compute_tier_skipped`], never silently). This is the
    /// compute-resource constraint: a fleet with a tight CPU budget, or a
    /// picker bounding its search to "fast" configs, asks for the cheap
    /// end of the knob space. Because the speed dial is inverted (see
    /// [`compute_tier`]), this keeps the FAST/high-speed cells and drops
    /// the slow ones — `with_compute_limit(2)` keeps roughly speed ≥ 8.
    /// Composes with [`with_budget`](Self::with_budget) (the compute
    /// filter is applied first, then the budget ladder reduces whatever
    /// remains).
    #[must_use]
    pub fn with_compute_limit(mut self, max_tier: u8) -> Self {
        self.compute_limit = Some(max_tier);
        self
    }

    /// Keep only cells within `max_deviations` axes of the default
    /// stratum. `1` = main-effects only (the all-defaults cell plus every
    /// single-axis probe, no interaction combos) — the isolated-axis
    /// regime a trained **scalar head** trains on. Pair with
    /// [`SweepAxes::scalar_dense`] for dense per-knob response curves
    /// without the cartesian blow-up of the full cross.
    #[must_use]
    pub fn with_max_deviations(mut self, max_deviations: u8) -> Self {
        self.max_deviations = Some(max_deviations);
        self
    }

    /// Cross + apply the user constraints (compute-tier limit, then
    /// deviation scope). Returns `(cells, invalid_skipped,
    /// duplicates_merged, compute_tier_skipped)`.
    fn build_cells(
        &self,
        axes: &SweepAxes,
        q_points: &[f32],
    ) -> (Vec<SweepCell>, Vec<String>, usize, Vec<String>) {
        let (mut cells, invalid_skipped, duplicates_merged) = cross(axes, q_points);
        let mut compute_tier_skipped = Vec::new();
        if let Some(max_tier) = self.compute_limit {
            cells.retain(|c| {
                if compute_tier(&c.config) <= max_tier {
                    true
                } else {
                    compute_tier_skipped.push(c.id.clone());
                    false
                }
            });
        }
        if let Some(max_dev) = self.max_deviations {
            cells.retain(|c| c.deviations <= max_dev);
        }
        (
            cells,
            invalid_skipped,
            duplicates_merged,
            compute_tier_skipped,
        )
    }

    /// Build the plan.
    #[must_use]
    pub fn plan(&self) -> SweepPlan {
        let mut axes = self.axes.clone();
        let mut q_points = self.grid.points();
        let mut dropped = Vec::new();
        let mut q_coarsenings = 0u32;
        let mut over_budget = false;

        loop {
            let (cells, invalid_skipped, duplicates_merged, compute_tier_skipped) =
                self.build_cells(&axes, &q_points);

            let within = match self.budget {
                None => true,
                Some(b) => cells.len() <= b,
            };
            if within {
                return SweepPlan {
                    cells,
                    invalid_skipped,
                    compute_tier_skipped,
                    dropped,
                    duplicates_merged,
                    q_coarsenings,
                    over_budget,
                };
            }

            // Reduction ladder, one step per iteration.
            if let Some(d) = collapse_one_axis(&mut axes) {
                // Coalesce repeated single-value drops of the same axis.
                if let Some(last) = dropped.last_mut()
                    && last.axis == d.axis
                {
                    last.dropped.extend(d.dropped);
                    last.kept = d.kept;
                    continue;
                }
                dropped.push(d);
                continue;
            }
            if q_points.len() > 11 {
                q_points = coarsen_keep_endpoints(&q_points);
                q_coarsenings += 1;
                continue;
            }

            // Nothing left to reduce: report rather than sample.
            over_budget = true;
            let (cells, invalid_skipped, duplicates_merged, compute_tier_skipped) =
                self.build_cells(&axes, &q_points);
            return SweepPlan {
                cells,
                invalid_skipped,
                compute_tier_skipped,
                dropped,
                duplicates_merged,
                q_coarsenings,
                over_budget,
            };
        }
    }
}

fn collapse<T: core::fmt::Debug + Clone>(
    name: &'static str,
    v: &mut Vec<T>,
    floor: usize,
) -> Option<DroppedAxis> {
    // Shed ONE value per ladder step — the last (lowest-priority) entry —
    // so the budget is approached from above instead of overshot by
    // whole-axis removals. Axis vecs are ordered most-important-first.
    if v.len() <= floor {
        return None;
    }
    let dropped = vec![format!("{:?}", v[v.len() - 1])];
    v.truncate(v.len() - 1);
    let kept = v
        .iter()
        .map(|x| format!("{x:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(DroppedAxis {
        axis: name,
        kept,
        dropped,
    })
}

/// Collapse the lowest-tier multi-valued axis by one value.
fn collapse_one_axis(axes: &mut SweepAxes) -> Option<DroppedAxis> {
    // Tier order: cheapest-to-lose first. qm, subsampling AND color_models
    // are MANDATORY core axes — never collapsed. color_models was previously
    // collapsed to floor=1 (YCbCr only), silently dropping the appended RGB
    // color path and crippling the picker (RGB is 25-40% RD on suitable
    // content). Color mode is mandatory per the zenmetrics
    // docs/MANDATORY_SWEEP_AXES.md contract. speeds keeps at least two points
    // so RD-vs-cost stays measurable.
    collapse("probes", &mut axes.probes, 1)
        .or_else(|| collapse("vaq", &mut axes.vaq, 1))
        .or_else(|| collapse("trellis", &mut axes.trellis, 1))
        .or_else(|| collapse("bit_depths", &mut axes.bit_depths, 1))
        .or_else(|| collapse("speeds", &mut axes.speeds, 2))
}

/// Drop every second interior point (endpoints kept).
fn coarsen_keep_endpoints(points: &[f32]) -> Vec<f32> {
    let last = points.len() - 1;
    points
        .iter()
        .enumerate()
        .filter(|(i, _)| *i == 0 || *i == last || i % 2 == 0)
        .map(|(_, &p)| p)
        .collect()
}

/// One point in the categorical cross product.
#[derive(Clone, Copy)]
struct Stratum {
    speed: u8,
    qm: bool,
    subsampling: EncodeChromaSubsampling,
    bit_depth: EncodeBitDepth,
    color_model: EncodeColorModel,
    vaq: Option<f64>,
    trellis: bool,
    probe: KnobProbe,
}

impl Stratum {
    fn build_config(&self, q: f32) -> EncoderConfig {
        let cfg = EncoderConfig::new()
            .quality(q)
            .speed(self.speed)
            .bit_depth(self.bit_depth)
            .color_model(self.color_model)
            .chroma_subsampling(self.subsampling)
            // Reproducibility pin — see module docs: tile count derives
            // from the thread setting, and unset threads make encoded
            // bytes depend on the host's core count.
            .threads(Some(1))
            .with_qm(self.qm)
            .with_vaq(self.vaq.is_some(), self.vaq.unwrap_or(1.0));
        let cfg = if self.trellis {
            cfg.with_trellis(Some(true))
        } else {
            cfg
        };
        self.probe.apply(cfg)
    }

    fn id(&self) -> String {
        let mut s = format!("s{}", self.speed);
        if !self.qm {
            s.push_str("-noqm");
        }
        if self.subsampling == EncodeChromaSubsampling::Yuv420 {
            s.push_str("-420");
        }
        match self.bit_depth {
            EncodeBitDepth::Auto => {}
            EncodeBitDepth::Eight => s.push_str("-bd8"),
            EncodeBitDepth::Ten => s.push_str("-bd10"),
        }
        if self.color_model == EncodeColorModel::Rgb {
            s.push_str("-rgb");
        }
        if let Some(strength) = self.vaq {
            s.push_str(&format!("-vaq{strength}"));
        }
        if self.trellis {
            s.push_str("-trel");
        }
        s.push_str(&self.probe.label());
        s
    }
}

/// Cross axes × quality points into deduplicated, priority-ordered cells.
fn cross(axes: &SweepAxes, q_points: &[f32]) -> (Vec<SweepCell>, Vec<String>, usize) {
    // Pass 1: enumerate strata with per-axis value indices; validity is
    // quality-independent so it is checked here, once per stratum.
    struct Entry {
        stratum: Stratum,
        deviations: u8,
        idx_sum: usize,
        seq: usize,
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut invalid = Vec::new();
    let mut seq = 0usize;

    for (si, &speed) in axes.speeds.iter().enumerate() {
        for (qi, &qm) in axes.qm.iter().enumerate() {
            for (ci, &subsampling) in axes.subsampling.iter().enumerate() {
                for (bi, &bit_depth) in axes.bit_depths.iter().enumerate() {
                    for (mi, &color_model) in axes.color_models.iter().enumerate() {
                        for (vi, &vaq) in axes.vaq.iter().enumerate() {
                            for (ti, &trellis) in axes.trellis.iter().enumerate() {
                                for (pi, &probe) in axes.probes.iter().enumerate() {
                                    let idxs = [si, qi, ci, bi, mi, vi, ti, pi];
                                    let stratum = Stratum {
                                        speed,
                                        qm,
                                        subsampling,
                                        bit_depth,
                                        color_model,
                                        vaq,
                                        trellis,
                                        probe,
                                    };
                                    if stratum.build_config(75.0).validate().is_err() {
                                        invalid.push(stratum.id());
                                        continue;
                                    }
                                    entries.push(Entry {
                                        stratum,
                                        deviations: idxs.iter().filter(|&&x| x != 0).count() as u8,
                                        idx_sum: idxs.iter().sum(),
                                        seq,
                                    });
                                    seq += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Main effects before interactions; milder deviations before extreme
    // ones; nested order as the deterministic tie-break.
    entries.sort_by_key(|e| (e.deviations, e.idx_sum, e.seq));

    // Pass 2: expand quality ascending within each stratum (complete RD
    // curves — a truncated queue is safe at stratum boundaries) and
    // dedupe by resolved fingerprint. Keep-first means the merged cell
    // carries the highest-priority spelling; later aliases record the
    // exotic ones.
    let mut cells: Vec<SweepCell> = Vec::new();
    let mut by_fingerprint: std::collections::HashMap<u64, usize> =
        std::collections::HashMap::new();
    let mut merged = 0usize;

    for e in &entries {
        for &q in q_points {
            let config = e.stratum.build_config(q);
            let fingerprint = fingerprint(&config);
            let id = format!("{}_q{q}", e.stratum.id());
            if let Some(&idx) = by_fingerprint.get(&fingerprint) {
                cells[idx].aliases.push(id);
                merged += 1;
            } else {
                by_fingerprint.insert(fingerprint, cells.len());
                cells.push(SweepCell {
                    id,
                    config,
                    quality: q,
                    fingerprint,
                    aliases: Vec::new(),
                    deviations: e.deviations,
                });
            }
        }
    }
    (cells, invalid, merged)
}

// ============================================================================
// Byte-identity fingerprint
// ============================================================================

struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn u8(&mut self, v: u8) {
        self.write(&[v]);
    }
    fn u16(&mut self, v: u16) {
        self.write(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.write(&v.to_bits().to_le_bytes());
    }
    fn opt_u8(&mut self, v: Option<u8>) {
        match v {
            None => self.u8(0xff),
            Some(x) => {
                self.u8(1);
                self.u8(x);
            }
        }
    }
    fn bytes_opt(&mut self, v: Option<&[u8]>) {
        match v {
            None => self.u8(0),
            Some(b) => {
                self.u8(1);
                self.u64(b.len() as u64);
                self.write(b);
            }
        }
    }
}

/// Byte-identity fingerprint of a config's resolved state.
///
/// Two configs with equal fingerprints produce identical bytes for the
/// same input image. Built from the RESOLVED state, so it sees through
/// aliases:
///
/// - quality is fully mediated by the resolved quantizer (the mirror of
///   zenravif's curve), so q 80.0 and q 80.2 — same quantizer — merge;
/// - an override knob set to the value its speed preset already derives
///   merges with the unset spelling (the speed-derived settings are
///   hashed *after* overrides);
/// - VAQ hashes as *active* (enabled AND strength ≠ 1.0): the strength
///   is never read when VAQ is off, and the rescale is skipped at
///   strength 1.0 under the psychovisual/still tunes zenravif always
///   uses (zenrav1e `api/internal.rs:1379`; encode-validated by
///   `sweep_validate`);
/// - the CICP `matrix_coefficients` field is **excluded**: the zenravif
///   backend derives the signaled matrix from the color model, and no
///   other backend exists (the deprecated svtav1 path was the field's
///   only reader). Encode-validated by `sweep_validate`.
///
/// `threads` is hashed as configured. `None` (machine-dependent tiling)
/// never merges with anything — including another `None` spelling on a
/// different host — so don't put unpinned-thread configs in sweeps;
/// the planner pins `Some(1)` everywhere.
///
/// Every exclusion above is a *claim about encoder behavior* proven by
/// encode in `examples/sweep_validate.rs`, per the variant-generation
/// discipline. Re-validate when bumping the zenravif dependency.
#[must_use]
pub fn fingerprint(config: &EncoderConfig) -> u64 {
    let mut h = Fnv::new();

    // Backend. The deprecated Svtav1 variant still hashes distinctly:
    // conservative (validate() rejects it, so no plan carries it).
    #[allow(deprecated)]
    h.u8(match config.backend {
        crate::Av1Backend::Zenravif => 0,
        crate::Av1Backend::Svtav1 => 1,
    });

    // Resolved quantizers (quality is mediated; lossless pins 0).
    let lossless = config.lossless_effective();
    let quantizer = if lossless {
        0
    } else {
        quality_to_quantizer(config.quality)
    };
    let alpha_quantizer = if lossless {
        0
    } else {
        quality_to_quantizer(crate::encoder::effective_alpha_quality(config))
    };
    h.u8(quantizer);
    h.u8(alpha_quantizer);
    h.u8(u8::from(lossless));

    // Pixel-path knobs.
    h.u8(match config.bit_depth {
        EncodeBitDepth::Eight => 0,
        EncodeBitDepth::Ten => 1,
        // Conservative: Auto resolves per input bitness, which a
        // config-only fingerprint cannot see. Auto never merges with an
        // explicit depth even where they coincide.
        EncodeBitDepth::Auto => 2,
    });
    h.u8(match config.color_model {
        EncodeColorModel::YCbCr => 0,
        EncodeColorModel::Rgb => 1,
    });
    h.u8(match config.chroma_subsampling {
        EncodeChromaSubsampling::Yuv444 => 0,
        EncodeChromaSubsampling::Yuv420 => 1,
    });
    h.u8(match config.pixel_range {
        None | Some(crate::EncodePixelRange::Full) => 0,
        Some(crate::EncodePixelRange::Limited) => 1,
    });
    h.u8(match config.alpha_color_mode {
        crate::EncodeAlphaMode::UnassociatedClean => 0,
        crate::EncodeAlphaMode::UnassociatedDirty => 1,
        crate::EncodeAlphaMode::Premultiplied => 2,
    });

    // Coefficient-level knobs after gates. VAQ hashes its *active*
    // form: enabled-at-strength-1.0 is structurally byte-identical to
    // off (the psychovisual/still tunes always compute the activity
    // mask; zenrav1e skips the rescale at 1.0 — api/internal.rs:1379,
    // byte-proven by the harness's first run flagging the 1.0 axis
    // value as inert).
    h.u8(u8::from(config.qm_effective()));
    let vaq = config.vaq_active();
    h.u8(u8::from(vaq));
    if vaq {
        h.f64(config.vaq_strength_effective());
    }
    h.u8(u8::from(config.tune_still_image_effective()));
    h.f64(config.seg_boost_effective());
    h.u8(u8::from(config.trellis_effective()));

    // Speed preset + derived search settings after overrides. Hashing
    // the resolved values (not the Option spellings) is what merges
    // override-equals-preset aliases. The preset number itself is also
    // hashed: settings zenravif leaves to the zenrav1e preset
    // (tx_domain_distortion, motion config) are functions of it.
    h.u8(config.speed);
    for &(s, q) in &[(config.speed, quantizer), (config.speed, alpha_quantizer)] {
        let mut d = speed_derived(s, q);
        apply_overrides(&mut d, config);
        h.u8(d.partition_range.0);
        h.u8(d.partition_range.1);
        h.u8(u8::from(d.complex_prediction_modes));
        h.u8(u8::from(d.sgr_complexity_full));
        h.u8(u8::from(d.encode_bottomup));
        h.u8(u8::from(d.rdo_tx_decision));
        h.u8(u8::from(d.reduced_tx_set));
        h.u8(u8::from(d.fine_directional_intra));
        h.u8(u8::from(d.fast_deblock));
        h.u8(u8::from(d.lrf));
        h.u8(u8::from(d.cdef));
        h.u8(u8::from(d.inter_tx_split));
        h.u8(u8::from(d.tx_domain_rate));
        h.u8(u8::from(d.segmentation_complex));
        h.u8(u8::from(d.lru_on_skip));
        h.u8(d.non_square_partition_max);
        h.u16(d.min_tile_size);
    }

    // Threads → tiles. Hashed raw; see the doc comment.
    match config.threads {
        None => h.u64(u64::MAX),
        Some(t) => h.u64(t as u64),
    }

    // Container-level state: metadata bytes change the file, so hash
    // content, not presence.
    h.bytes_opt(config.exif.as_deref());
    h.bytes_opt(config.xmp.as_deref());
    h.bytes_opt(config.icc_profile.as_deref());
    h.opt_u8(config.rotation);
    h.opt_u8(config.mirror);
    match config.content_light_level {
        None => h.u8(0),
        Some((cll, fall)) => {
            h.u8(1);
            h.u16(cll);
            h.u16(fall);
        }
    }
    match config.mastering_display {
        None => h.u8(0),
        Some(md) => {
            h.u8(1);
            for (x, y) in md.primaries.iter().chain([&md.white_point]) {
                h.u16(*x);
                h.u16(*y);
            }
            h.u64(u64::from(md.max_luminance));
            h.u64(u64::from(md.min_luminance));
        }
    }
    h.opt_u8(config.color_primaries);
    h.opt_u8(config.transfer_characteristics);
    // matrix_coefficients: excluded — no available backend reads it
    // (zenravif derives the matrix from color_model; the deprecated
    // svtav1 path was the only reader). Encode-validated; see fn docs.
    match config.gain_map.as_ref() {
        None => h.u8(0),
        Some(gm) => {
            h.u8(1);
            h.u64(u64::from(gm.width));
            h.u64(u64::from(gm.height));
            h.u8(gm.bit_depth);
            h.u64(gm.av1_data.len() as u64);
            h.write(&gm.av1_data);
            h.u64(gm.metadata.len() as u64);
            h.write(&gm.metadata);
        }
    }

    h.0
}

// ============================================================================
// Cell-id grammar: parse ids back to configs (the ledger contract)
// ============================================================================

impl SweepAxes {
    /// Resolve a named plan's axes — the executor-facing entry point
    /// (`--plan name` in zenmetrics chunk mode resolves through this).
    /// Names are a wire contract: additive-only, never renamed.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "rd_core" => Some(Self::rd_core()),
            "modes_full" => Some(Self::modes_full()),
            "modes_full_alpha" => Some(Self::modes_full_alpha()),
            "scalar_dense" => Some(Self::scalar_dense()),
            _ => None,
        }
    }
}

/// Reconstruct the exact [`EncoderConfig`] a sweep cell id denotes.
///
/// `base_id` is the cell id without its `_q{q}` suffix (the split the
/// ledger stores); `quality` is the grid point. Reconstruction goes
/// through the **same** stratum builder the planner uses, so a parsed
/// config cannot drift from the planner's: equal id ⇒ equal resolved
/// state ⇒ equal [`fingerprint`].
///
/// The grammar (see `Stratum::id` / `KnobProbe::label`, which this
/// parser mirrors token for token):
///
/// ```text
/// s<speed>[-noqm][-420][-bd8|-bd10][-rgb][-vaq<f>][-trel][<probe>]
/// probe := -cdef<0|1> | -rdotx<0|1> | -sgr<0|1> | -lru<0|1>
///        | -segcx<0|1> | -bup<0|1> | -still | -sb<f> | -vaqs<f>
///        | -part<u8>.<u8> | -cpred<0|1> | -lrf<0|1> | -fdb<0|1>
///        | -aqd<signed f> | -adirty | -aprem
/// ```
///
/// Numbers render with shortest-roundtrip `Display`, so parsing is
/// lossless. Grammar evolution is additive-only: a new token's absence
/// means "default", so every stored id stays valid. There are no
/// non-self-describing cells in zenavif's planner (no opaque table
/// bytes); unknown or duplicated tokens error.
///
/// Consumers that carry the cell fingerprint alongside the id
/// (zenmetrics does) should verify `fingerprint(&config) == carried_fp`
/// after parsing — it turns any grammar drift between the declaring and
/// executing builds into a loud deterministic failure instead of a
/// silently wrong encode.
pub fn config_from_cell_id(base_id: &str, quality: f32) -> Result<EncoderConfig, String> {
    let rest = base_id
        .strip_prefix('s')
        .ok_or_else(|| format!("cell id must start with 's<speed>': {base_id}"))?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let speed: u8 = digits
        .parse()
        .map_err(|e| format!("bad speed in '{base_id}': {e}"))?;
    let mut cur = &rest[digits.len()..];

    let mut stratum = Stratum {
        speed,
        qm: true,
        subsampling: EncodeChromaSubsampling::Yuv444,
        bit_depth: EncodeBitDepth::Auto,
        color_model: EncodeColorModel::YCbCr,
        vaq: None,
        trellis: false,
        probe: KnobProbe::None,
    };

    // Scan a (possibly signed) decimal number off the front of `s`.
    fn number<'a>(s: &'a str, id: &str, what: &str) -> Result<(f64, &'a str), String> {
        let mut end = 0;
        let b = s.as_bytes();
        if end < b.len() && b[end] == b'-' {
            end += 1;
        }
        while end < b.len() && (b[end].is_ascii_digit() || b[end] == b'.') {
            end += 1;
        }
        let v: f64 = s[..end]
            .parse()
            .map_err(|e| format!("bad {what} number in '{id}': {e}"))?;
        Ok((v, &s[end..]))
    }
    // Digits only — for tokens where '.' is a separator, not a decimal
    // point (the part<min>.<max> pair). The float scanner would eat the
    // separator (caught by cell_ids_roundtrip_to_their_configs).
    fn integer<'a>(s: &'a str, id: &str, what: &str) -> Result<(u8, &'a str), String> {
        let end = s.bytes().take_while(u8::is_ascii_digit).count();
        let v: u8 = s[..end]
            .parse()
            .map_err(|e| format!("bad {what} integer in '{id}': {e}"))?;
        Ok((v, &s[end..]))
    }
    fn bool01<'a>(s: &'a str, id: &str, what: &str) -> Result<(bool, &'a str), String> {
        match s.as_bytes().first() {
            Some(b'1') => Ok((true, &s[1..])),
            Some(b'0') => Ok((false, &s[1..])),
            _ => Err(format!("expected 0|1 after {what} in '{id}'")),
        }
    }
    fn set_probe(st: &mut Stratum, p: KnobProbe, id: &str) -> Result<(), String> {
        if st.probe != KnobProbe::None {
            return Err(format!(
                "duplicate probe token in '{id}': probes are single-deviation by construction"
            ));
        }
        st.probe = p;
        Ok(())
    }

    // Longest-prefix-first where prefixes overlap (-vaqs before -vaq).
    while !cur.is_empty() {
        let Some(tok) = cur.strip_prefix('-') else {
            return Err(format!(
                "expected '-' before token at '…{cur}' in '{base_id}'"
            ));
        };
        cur = if let Some(t) = tok.strip_prefix("noqm") {
            stratum.qm = false;
            t
        } else if let Some(t) = tok.strip_prefix("420") {
            stratum.subsampling = EncodeChromaSubsampling::Yuv420;
            t
        } else if let Some(t) = tok.strip_prefix("bd10") {
            stratum.bit_depth = EncodeBitDepth::Ten;
            t
        } else if let Some(t) = tok.strip_prefix("bd8") {
            stratum.bit_depth = EncodeBitDepth::Eight;
            t
        } else if let Some(t) = tok.strip_prefix("rgb") {
            stratum.color_model = EncodeColorModel::Rgb;
            t
        } else if let Some(t) = tok.strip_prefix("vaqs") {
            let (v, t) = number(t, base_id, "vaqs")?;
            set_probe(&mut stratum, KnobProbe::VaqStrength(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("vaq") {
            let (v, t) = number(t, base_id, "vaq")?;
            stratum.vaq = Some(v);
            t
        } else if let Some(t) = tok.strip_prefix("trel") {
            stratum.trellis = true;
            t
        } else if let Some(t) = tok.strip_prefix("cdef") {
            let (v, t) = bool01(t, base_id, "cdef")?;
            set_probe(&mut stratum, KnobProbe::Cdef(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("rdotx") {
            let (v, t) = bool01(t, base_id, "rdotx")?;
            set_probe(&mut stratum, KnobProbe::RdoTxDecision(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("sgr") {
            let (v, t) = bool01(t, base_id, "sgr")?;
            set_probe(&mut stratum, KnobProbe::SgrFull(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("lru") {
            let (v, t) = bool01(t, base_id, "lru")?;
            set_probe(&mut stratum, KnobProbe::LruOnSkip(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("segcx") {
            let (v, t) = bool01(t, base_id, "segcx")?;
            set_probe(&mut stratum, KnobProbe::SegmentationComplex(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("bup") {
            let (v, t) = bool01(t, base_id, "bup")?;
            set_probe(&mut stratum, KnobProbe::EncodeBottomup(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("still") {
            set_probe(&mut stratum, KnobProbe::TuneStillImage, base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("sb") {
            let (v, t) = number(t, base_id, "sb")?;
            set_probe(&mut stratum, KnobProbe::SegBoost(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("part") {
            let (min, t) = integer(t, base_id, "part-min")?;
            let Some(t) = t.strip_prefix('.') else {
                return Err(format!("expected '.' in part token of '{base_id}'"));
            };
            let (max, t) = integer(t, base_id, "part-max")?;
            set_probe(&mut stratum, KnobProbe::PartitionRange(min, max), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("cpred") {
            let (v, t) = bool01(t, base_id, "cpred")?;
            set_probe(&mut stratum, KnobProbe::ComplexPredictionModes(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("lrf") {
            let (v, t) = bool01(t, base_id, "lrf")?;
            set_probe(&mut stratum, KnobProbe::Lrf(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("fdb") {
            let (v, t) = bool01(t, base_id, "fdb")?;
            set_probe(&mut stratum, KnobProbe::FastDeblock(v), base_id)?;
            t
        } else if let Some(t) = tok.strip_prefix("aqd") {
            let (v, t) = number(t, base_id, "aqd")?;
            set_probe(
                &mut stratum,
                KnobProbe::AlphaQualityDelta(v as f32),
                base_id,
            )?;
            t
        } else if let Some(t) = tok.strip_prefix("adirty") {
            set_probe(
                &mut stratum,
                KnobProbe::AlphaMode(EncodeAlphaMode::UnassociatedDirty),
                base_id,
            )?;
            t
        } else if let Some(t) = tok.strip_prefix("aprem") {
            set_probe(
                &mut stratum,
                KnobProbe::AlphaMode(EncodeAlphaMode::Premultiplied),
                base_id,
            )?;
            t
        } else {
            return Err(format!("unknown token '-{tok}' in cell id '{base_id}'"));
        };
    }

    Ok(stratum.build_config(quality))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_axes() -> SweepAxes {
        SweepAxes {
            speeds: vec![4],
            qm: vec![true],
            subsampling: vec![EncodeChromaSubsampling::Yuv444],
            bit_depths: vec![EncodeBitDepth::Auto],
            color_models: vec![EncodeColorModel::YCbCr],
            vaq: vec![None],
            trellis: vec![false],
            probes: vec![KnobProbe::None],
        }
    }

    #[test]
    fn default_stratum_first_and_deviations_nondecreasing() {
        let plan = SweepBuilder::new(SweepAxes::modes_full(), QualityGrid::Step5).plan();
        assert!(!plan.cells.is_empty());
        assert_eq!(plan.cells[0].deviations, 0, "all-defaults cell first");
        let mut prev = 0u8;
        for c in &plan.cells {
            assert!(c.deviations >= prev, "deviations must be non-decreasing");
            prev = c.deviations;
        }
    }

    #[test]
    fn budget_never_sheds_mandatory_color_models() {
        // Color mode (incl RGB) is MANDATORY (zenmetrics docs/MANDATORY_SWEEP_AXES.md).
        // Pre-2026-06-27 a budgeted modes_full collapsed color_models to YCbCr-only,
        // silently dropping RGB and crippling the picker. Lock that shut.
        let axes = SweepAxes::modes_full();
        assert!(
            axes.color_models
                .iter()
                .any(|c| matches!(c, EncodeColorModel::Rgb)),
            "modes_full must declare the RGB color model"
        );
        let unbudgeted = SweepBuilder::new(axes.clone(), QualityGrid::Explicit(vec![50.0])).plan();
        let plan = SweepBuilder::new(axes, QualityGrid::Explicit(vec![50.0]))
            .with_budget((unbudgeted.cells.len() / 16).max(1))
            .plan();
        // color_models must NEVER be in the dropped manifest, at any budget.
        assert!(
            !plan.dropped.iter().any(|d| d.axis == "color_models"),
            "color_models is mandatory; must never be shed. dropped: {:?}",
            plan.dropped.iter().map(|d| d.axis).collect::<Vec<_>>()
        );
        // The RGB color path must survive the budget.
        assert!(
            plan.cells.iter().any(|c| c.id.contains("rgb")),
            "RGB must survive a budgeted modes_full"
        );
    }

    #[test]
    fn quality_ascending_within_stratum_and_ids_unique() {
        let plan = SweepBuilder::new(SweepAxes::rd_core(), QualityGrid::Step5).plan();
        let mut seen = std::collections::HashSet::new();
        for c in &plan.cells {
            assert!(seen.insert(c.id.clone()), "duplicate cell id {}", c.id);
        }
        // Quality ascending within each contiguous same-base run.
        let base = |id: &str| id.rsplit_once("_q").map(|(b, _)| b.to_string()).unwrap();
        let mut prev_base = String::new();
        let mut prev_q = -1.0f32;
        for c in &plan.cells {
            let b = base(&c.id);
            if b == prev_base {
                assert!(c.quality > prev_q, "quality must ascend within {b}");
            }
            prev_base = b;
            prev_q = c.quality;
        }
    }

    #[test]
    fn quality_mediation_dedupes_equal_quantizers() {
        // q 80.0 and q 80.2 → quantizer 71 (mirror anchor); the planner
        // must merge those cells.
        let plan =
            SweepBuilder::new(tiny_axes(), QualityGrid::Explicit(vec![80.0, 80.2, 81.0])).plan();
        assert_eq!(plan.cells.len(), 2, "80.0/80.2 merge; 81.0 distinct");
        assert_eq!(plan.duplicates_merged, 1);
        assert!(plan.cells[0].aliases.iter().any(|a| a.contains("q80.2")));
    }

    #[test]
    fn override_equal_to_preset_dedupes() {
        // At q30/speed6 the preset enables CDEF (low quality), so
        // Cdef(true) aliases the no-probe stratum; Cdef(false) does not.
        let mut axes = tiny_axes();
        axes.speeds = vec![6];
        axes.probes = vec![
            KnobProbe::None,
            KnobProbe::Cdef(true),
            KnobProbe::Cdef(false),
        ];
        let plan = SweepBuilder::new(axes, QualityGrid::Explicit(vec![30.0])).plan();
        assert_eq!(plan.cells.len(), 2, "Cdef(true) must merge with preset");
        assert_eq!(plan.duplicates_merged, 1);
    }

    #[test]
    fn vaq_strength_inert_when_vaq_off() {
        let base = Stratum {
            speed: 4,
            qm: true,
            subsampling: EncodeChromaSubsampling::Yuv444,
            bit_depth: EncodeBitDepth::Auto,
            color_model: EncodeColorModel::YCbCr,
            vaq: None,
            trellis: false,
            probe: KnobProbe::None,
        };
        let a = base.build_config(50.0).with_vaq(false, 1.0);
        let b = base.build_config(50.0).with_vaq(false, 3.0);
        assert_eq!(fingerprint(&a), fingerprint(&b));
        // Enabled at strength 1.0 is the structural no-op spelling
        // (zenrav1e api/internal.rs:1379) — it must alias OFF.
        let noop = base.build_config(50.0).with_vaq(true, 1.0);
        assert_eq!(fingerprint(&a), fingerprint(&noop));
        // A strength with effect must not alias.
        let live = base.build_config(50.0).with_vaq(true, 0.5);
        assert_ne!(fingerprint(&a), fingerprint(&live));
    }

    #[test]
    fn scalar_ladders_dense_distinct_and_roundtrip() {
        // Dense-sweep program contract: the curated scalar ladders for
        // seg_boost and vaq_strength stay dense (>= 4-6 distinct values
        // per axis, both directions around the neutral point), every
        // value yields a distinct fingerprint at a fixed stratum (dedup
        // must not over-merge distinct scalar values), and each cell id
        // parses back fingerprint-identical (the resolve_verified
        // contract for arbitrary scalar values, not just registry
        // presets).
        let axes = SweepAxes::modes_full();
        let mut sb: Vec<f64> = axes
            .probes
            .iter()
            .filter_map(|p| match p {
                KnobProbe::SegBoost(v) => Some(*v),
                _ => None,
            })
            .collect();
        sb.sort_by(f64::total_cmp);
        assert_eq!(sb, vec![0.75, 1.5, 2.5, 4.0], "seg_boost ladder");
        let mut vaq: Vec<f64> = axes
            .probes
            .iter()
            .filter_map(|p| match p {
                KnobProbe::VaqStrength(v) => Some(*v),
                _ => None,
            })
            .collect();
        // Plus the vaq-axis Some(0.5) stratum.
        assert!(axes.vaq.contains(&Some(0.5)));
        vaq.sort_by(f64::total_cmp);
        assert_eq!(vaq, vec![0.25, 2.0, 3.0], "vaq_strength probe ladder");
        assert!(
            !vaq.contains(&1.0) && !vaq.contains(&0.5),
            "1.0 is the structural no-op; 0.5 would alias the vaq-axis stratum"
        );
        // Still-envelope equivalence guard: vaq_strength(x) and
        // seg_boost(x) are byte-identical at equal x on still encodes
        // (proven by encode, 2026-06-12 harness), so the two curated
        // ladders must stay value-DISJOINT — a shared value would be a
        // duplicate encode the fingerprint deliberately doesn't merge.
        let mut joint = vaq.clone();
        joint.extend(axes.vaq.iter().filter_map(|v| *v)); // axis strength(s)
        for v in &sb {
            assert!(
                !joint.contains(v),
                "seg_boost {v} duplicates a vaq_strength value — \
                 still-envelope alias (see module docs)"
            );
        }

        let base = Stratum {
            speed: 4,
            qm: true,
            subsampling: EncodeChromaSubsampling::Yuv444,
            bit_depth: EncodeBitDepth::Auto,
            color_model: EncodeColorModel::YCbCr,
            vaq: None,
            trellis: false,
            probe: KnobProbe::None,
        };
        let mut fps = std::collections::BTreeMap::new();
        fps.insert(fingerprint(&base.build_config(50.0)), "default".to_string());
        for probe in axes
            .probes
            .iter()
            .filter(|p| matches!(p, KnobProbe::SegBoost(_) | KnobProbe::VaqStrength(_)))
        {
            let s = Stratum {
                probe: *probe,
                ..base
            };
            let id = s.id();
            let fp = fingerprint(&s.build_config(50.0));
            if let Some(prev) = fps.insert(fp, id.clone()) {
                panic!("fingerprint collision between {prev} and {id}");
            }
            // Round-trip: id → config → identical fingerprint.
            let parsed = config_from_cell_id(&id, 50.0).unwrap();
            assert_eq!(fingerprint(&parsed), fp, "parser drift for {id}");
        }
        // default + 4 seg_boost + 3 vaq_strength.
        assert_eq!(fps.len(), 8, "scalar ladder size drifted");
    }

    #[test]
    fn invalid_strata_reported_not_lost() {
        let mut axes = tiny_axes();
        axes.subsampling = vec![
            EncodeChromaSubsampling::Yuv444,
            EncodeChromaSubsampling::Yuv420,
        ];
        axes.color_models = vec![EncodeColorModel::YCbCr, EncodeColorModel::Rgb];
        let plan = SweepBuilder::new(axes, QualityGrid::Explicit(vec![50.0])).plan();
        assert_eq!(
            plan.invalid_skipped.len(),
            1,
            "420×RGB must be skipped and reported: {:?}",
            plan.invalid_skipped
        );
        assert!(plan.invalid_skipped[0].contains("420"));
        assert!(plan.invalid_skipped[0].contains("rgb"));
    }

    #[test]
    fn budget_ladder_sheds_probes_first_and_records() {
        let unbudgeted = SweepBuilder::new(SweepAxes::modes_full(), QualityGrid::Step5).plan();
        let budget = unbudgeted.cells.len() / 2;
        let plan = SweepBuilder::new(SweepAxes::modes_full(), QualityGrid::Step5)
            .with_budget(budget)
            .plan();
        assert!(plan.cells.len() <= budget);
        assert!(!plan.over_budget);
        assert!(!plan.dropped.is_empty());
        assert_eq!(
            plan.dropped[0].axis, "probes",
            "ladder must shed the probe axis first"
        );
    }

    #[test]
    fn impossible_budget_reports_over_budget() {
        let plan = SweepBuilder::new(SweepAxes::rd_core(), QualityGrid::Step5)
            .with_budget(1)
            .plan();
        assert!(plan.over_budget);
        assert!(
            !plan.cells.is_empty(),
            "over-budget still returns the full reduced plan"
        );
    }

    #[test]
    fn cells_pin_threads_for_reproducibility() {
        let plan =
            SweepBuilder::new(SweepAxes::rd_core(), QualityGrid::Explicit(vec![50.0])).plan();
        for c in &plan.cells {
            assert_eq!(c.config.threads, Some(1), "cell {} must pin threads", c.id);
        }
    }

    #[test]
    fn plan_is_deterministic() {
        let a = SweepBuilder::new(SweepAxes::modes_full(), QualityGrid::Step5).plan();
        let b = SweepBuilder::new(SweepAxes::modes_full(), QualityGrid::Step5).plan();
        assert_eq!(a.cells.len(), b.cells.len());
        for (x, y) in a.cells.iter().zip(b.cells.iter()) {
            assert_eq!(x.id, y.id);
            assert_eq!(x.fingerprint, y.fingerprint);
        }
    }

    #[test]
    fn metadata_bytes_are_hashed_by_content() {
        let base = EncoderConfig::new().quality(50.0);
        let a = base.clone().exif(vec![1, 2, 3]);
        let b = base.clone().exif(vec![1, 2, 4]);
        assert_ne!(fingerprint(&a), fingerprint(&b));
        assert_ne!(fingerprint(&a), fingerprint(&base));
    }

    #[test]
    fn matrix_coefficients_excluded() {
        let base = EncoderConfig::new().quality(50.0);
        let with_mc = base.clone().matrix_coefficients(9);
        assert_eq!(
            fingerprint(&base),
            fingerprint(&with_mc),
            "no available backend reads the CICP matrix field"
        );
    }

    #[test]
    fn alpha_delta_zero_aliases_follow_color() {
        let base = Stratum {
            speed: 4,
            qm: true,
            subsampling: EncodeChromaSubsampling::Yuv444,
            bit_depth: EncodeBitDepth::Auto,
            color_model: EncodeColorModel::YCbCr,
            vaq: None,
            trellis: false,
            probe: KnobProbe::None,
        };
        let none = base.build_config(60.0);
        let delta0 = Stratum {
            probe: KnobProbe::AlphaQualityDelta(0.0),
            ..base
        }
        .build_config(60.0);
        assert_eq!(
            fingerprint(&none),
            fingerprint(&delta0),
            "Delta(0.0) is the follow-color spelling and must alias"
        );
        let delta_neg = Stratum {
            probe: KnobProbe::AlphaQualityDelta(-25.0),
            ..base
        }
        .build_config(60.0);
        assert_ne!(fingerprint(&none), fingerprint(&delta_neg));
    }

    #[test]
    fn modes_full_alpha_extends_and_validates() {
        let plan = SweepBuilder::new(
            SweepAxes::modes_full_alpha(),
            QualityGrid::Explicit(vec![30.0, 60.0]),
        )
        .plan();
        for probe_label in ["-aqd-25", "-aqd25", "-adirty", "-aprem"] {
            assert!(
                plan.cells.iter().any(|c| c.id.contains(probe_label)),
                "alpha probe {probe_label} missing from modes_full_alpha plan"
            );
        }
    }

    #[test]
    fn feature_row_matches_columns_and_resolves() {
        let plan =
            SweepBuilder::new(SweepAxes::rd_core(), QualityGrid::Explicit(vec![30.0])).plan();
        let cols = feature_columns();
        let idx = |name: &str| cols.iter().position(|c| *c == name).unwrap();

        for cell in &plan.cells {
            let row = cell.feature_row(PlanInput::rgb8(512, 512));
            assert_eq!(row.len(), cols.len(), "row/column length mismatch");
            assert_eq!(
                row[idx("quantizer")],
                f64::from(quality_to_quantizer(cell.quality)),
                "quantizer column must match the resolved mirror"
            );
            assert_eq!(
                row[idx("alpha_quantizer")],
                -1.0,
                "rgb8 input declares no alpha"
            );
            // Pinned threads ⇒ resolved tile request, never the -1
            // machine-dependent sentinel. The value is min(1, cap):
            // cap 0 at speeds whose min_tile_size exceeds 512.
            assert!(
                row[idx("tiles")] == 0.0 || row[idx("tiles")] == 1.0,
                "pinned-thread cells must resolve tiles (got {})",
                row[idx("tiles")]
            );
        }

        // A probe flips exactly its resolved column.
        let base = Stratum {
            speed: 6,
            qm: true,
            subsampling: EncodeChromaSubsampling::Yuv444,
            bit_depth: EncodeBitDepth::Auto,
            color_model: EncodeColorModel::YCbCr,
            vaq: None,
            trellis: false,
            probe: KnobProbe::None,
        };
        let mk_cell = |probe: KnobProbe, q: f32| SweepCell {
            id: "t".into(),
            config: Stratum { probe, ..base }.build_config(q),
            quality: q,
            fingerprint: 0,
            aliases: Vec::new(),
            deviations: 0,
        };
        let input = PlanInput::rgb8(512, 512);
        // q30/speed6: preset cdef ON (low quality) — forcing off flips the column.
        let on = mk_cell(KnobProbe::None, 30.0).feature_row(input);
        let off = mk_cell(KnobProbe::Cdef(false), 30.0).feature_row(input);
        assert_eq!(on[idx("cdef")], 1.0);
        assert_eq!(off[idx("cdef")], 0.0);

        // Alpha delta lands in alpha_quantizer on alpha-bearing input.
        let a = mk_cell(KnobProbe::None, 60.0).feature_row(PlanInput::rgba8(512, 512));
        let d = mk_cell(KnobProbe::AlphaQualityDelta(-25.0), 60.0)
            .feature_row(PlanInput::rgba8(512, 512));
        assert_eq!(
            a[idx("alpha_quantizer")],
            f64::from(quality_to_quantizer(60.0))
        );
        assert_eq!(
            d[idx("alpha_quantizer")],
            f64::from(quality_to_quantizer(35.0))
        );
        // Alpha mode column.
        let prem =
            mk_cell(KnobProbe::AlphaMode(EncodeAlphaMode::Premultiplied), 60.0).feature_row(input);
        assert_eq!(prem[idx("alpha_color_mode")], 2.0);
    }

    /// Pattern-7 grammar totality: every id the planner can emit —
    /// canonical AND alias spellings, across the largest axes set —
    /// parses back to a config whose resolved-state fingerprint is
    /// identical. This is the renderer/parser lockstep contract; it is
    /// what makes a ledger row regenerable years later.
    #[test]
    fn cell_ids_roundtrip_to_their_configs() {
        let plan = SweepBuilder::new(SweepAxes::modes_full_alpha(), QualityGrid::Step5).plan();
        assert!(plan.cells.len() > 1000, "expected a large plan to attack");

        let parse_q = |id: &str| -> (String, f32) {
            let (base, q) = id.rsplit_once("_q").expect("id must carry _q suffix");
            (base.to_string(), q.parse::<f32>().expect("lossless q"))
        };

        let mut checked = 0usize;
        for cell in &plan.cells {
            for id in std::iter::once(&cell.id).chain(cell.aliases.iter()) {
                let (base, q) = parse_q(id);
                let config =
                    config_from_cell_id(&base, q).unwrap_or_else(|e| panic!("grammar gap: {e}"));
                assert_eq!(
                    fingerprint(&config),
                    cell.fingerprint,
                    "parsed config diverges from planner for id '{id}'"
                );
                checked += 1;
            }
        }
        // Canonical cells + every merged alias spelling all roundtrip.
        assert_eq!(
            checked,
            plan.cells.len() + plan.cells.iter().map(|c| c.aliases.len()).sum::<usize>()
        );

        // Id uniqueness over the largest axes set (attribution depends
        // on it; collisions silently merge deltas).
        let mut seen = std::collections::HashSet::new();
        for c in &plan.cells {
            assert!(seen.insert(&c.id), "duplicate cell id {}", c.id);
        }
    }

    #[test]
    fn cell_id_parser_rejects_malformed_ids() {
        // Unknown token.
        assert!(config_from_cell_id("s4-bogus", 50.0).is_err());
        // Two probe tokens (single-deviation violated).
        assert!(config_from_cell_id("s4-cdef1-lrf0", 50.0).is_err());
        // Bad number.
        assert!(config_from_cell_id("s4-vaq", 50.0).is_err());
        // Missing speed.
        assert!(config_from_cell_id("x4", 50.0).is_err());
        // Trailing garbage.
        assert!(config_from_cell_id("s4-cdef1x", 50.0).is_err());
        // Signed alpha delta parses (the '-' inside a token).
        let cfg = config_from_cell_id("s6-aqd-25", 60.0).expect("signed delta");
        let probe_cfg = KnobProbe::AlphaQualityDelta(-25.0)
            .apply(EncoderConfig::new().quality(60.0).speed(6).threads(Some(1)));
        assert_eq!(fingerprint(&cfg), fingerprint(&probe_cfg));
    }

    #[test]
    fn fingerprint_verification_catches_drift() {
        // The consumer contract: parse, recompute, compare against the
        // carried fp. A different cell's fp must not verify.
        let a = config_from_cell_id("s4", 50.0).unwrap();
        let b = config_from_cell_id("s4-noqm", 50.0).unwrap();
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn plan_names_resolve() {
        assert!(SweepAxes::by_name("rd_core").is_some());
        assert!(SweepAxes::by_name("modes_full").is_some());
        assert!(SweepAxes::by_name("modes_full_alpha").is_some());
        assert!(SweepAxes::by_name("scalar_dense").is_some());
        assert!(SweepAxes::by_name("nonsense").is_none());
    }

    #[test]
    fn scalar_dense_is_isolated_and_speed_ladder_is_dense() {
        let plan = SweepBuilder::new(SweepAxes::scalar_dense(), QualityGrid::Explicit(vec![50.0]))
            .with_max_deviations(1)
            .plan();
        // Isolation: nothing beyond a single-axis deviation survives, so
        // every probe is a clean per-knob response point for a scalar head.
        assert!(
            plan.cells.iter().all(|c| c.deviations <= 1),
            "scalar_dense + max_deviations(1) must be main-effects only"
        );
        // Density of the COMPUTE (speed) axis: the dense speed ladder must
        // surface ≥ 6 distinct compute tiers among the plan's cells —
        // proof that the user's requested dense speed coverage is present.
        let mut tiers: Vec<u8> = plan.cells.iter().map(|c| compute_tier(&c.config)).collect();
        tiers.sort_unstable();
        tiers.dedup();
        assert!(
            tiers.len() >= 6,
            "speed ladder not dense enough: only {} distinct compute tiers {:?}",
            tiers.len(),
            tiers
        );
    }

    #[test]
    fn compute_tier_respects_speed_inversion() {
        // The AV1 speed dial is INVERTED: higher speed = faster = cheaper.
        // A fast speed (8) MUST yield a strictly smaller tier than a slow
        // speed (2). Asserted explicitly so the inversion can never
        // silently regress to "tier increases with the speed number".
        let fast = EncoderConfig::new().quality(50.0).speed(8).threads(Some(1));
        let slow = EncoderConfig::new().quality(50.0).speed(2).threads(Some(1));
        assert!(
            compute_tier(&fast) < compute_tier(&slow),
            "INVERSION BROKEN: speed 8 (fast) tier {} must be < speed 2 (slow) tier {}",
            compute_tier(&fast),
            compute_tier(&slow),
        );
        // And the cheapest preset (fastest speed, qm off, no trellis) is
        // the tier-0 floor.
        let floor = EncoderConfig::new()
            .quality(50.0)
            .speed(10)
            .threads(Some(1))
            .with_qm(false);
        assert_eq!(compute_tier(&floor), 0, "speed 10 + no-qm is the floor");
        // Trellis adds real cost on top of the speed term.
        let trel = slow.clone().with_trellis(Some(true));
        assert!(
            compute_tier(&trel) > compute_tier(&slow),
            "trellis (extra coefficient pass) must raise the tier"
        );
    }

    #[test]
    fn with_compute_limit_drops_expensive_and_reports() {
        let unlimited =
            SweepBuilder::new(SweepAxes::modes_full(), QualityGrid::Explicit(vec![75.0])).plan();
        let limited = SweepBuilder::new(SweepAxes::modes_full(), QualityGrid::Explicit(vec![75.0]))
            .with_compute_limit(3)
            .plan();
        assert!(
            !limited.cells.is_empty(),
            "tier ≤ 3 (the fast/high-speed end) cells must survive"
        );
        assert!(
            limited.cells.len() < unlimited.cells.len(),
            "the compute limit must drop the expensive (slow-speed/trellis) cells"
        );
        assert!(
            limited.cells.iter().all(|c| compute_tier(&c.config) <= 3),
            "every surviving cell must be within the compute budget"
        );
        assert!(
            !limited.compute_tier_skipped.is_empty(),
            "dropped cells must be reported, never silently capped"
        );
    }
}

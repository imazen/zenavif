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
//! | vaq strength | 0.0–4.0 | on@1.0 (axis), 0.5 / 2.0 (probes) | bound = zenravif validate range; prior finding "VAQ hurts stills" (CLAUDE.md, ravif 7265eea) — steps exist to quantify, not to endorse. First validated by `sweep_validate` 2026-06-10 |
//! | seg_boost | 0.5–4.0 | 1.5, 2.5 | bound = zenravif validate range (1.0 = off). Steps unmeasured before this module; validated for liveness (not goodness) by `sweep_validate` |
//! | trellis | on/off | off, on | zenrav1e Viterbi DP; default off in zenravif |
//! | deep-knob probes | see [`KnobProbe`] | one axis, single-deviation by construction | each probe flips one preset-derived setting both ways; fingerprint dedup removes the spellings that equal the preset |
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

use crate::encode_plan::{apply_overrides, quality_to_quantizer, speed_derived};
use crate::encoder::{EncodeBitDepth, EncodeChromaSubsampling, EncodeColorModel, EncoderConfig};

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
    /// Override loop-restoration search on skip blocks.
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
}

impl KnobProbe {
    // `InternalParams` is #[non_exhaustive]: struct-literal construction
    // (clippy's suggested `..Default::default()` form) is not available
    // outside the defining module, so Default + field assignment is the
    // only spelling — same as build_ravif_encoder's.
    #[allow(clippy::field_reassign_with_default)]
    fn apply(self, cfg: EncoderConfig) -> EncoderConfig {
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
    /// Variance adaptive quantization on/off (strength 1.0; strength
    /// steps live on the probe axis).
    pub vaq: Vec<bool>,
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
            vaq: vec![false],
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
        axes.vaq.push(true);
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
            KnobProbe::LruOnSkip(true),
            KnobProbe::LruOnSkip(false),
            KnobProbe::SegmentationComplex(true),
            KnobProbe::SegmentationComplex(false),
            KnobProbe::EncodeBottomup(true),
            KnobProbe::EncodeBottomup(false),
            KnobProbe::TuneStillImage,
            // Scalar steps — bounds from zenravif's validate ranges; see
            // the module-docs provenance table.
            KnobProbe::SegBoost(1.5),
            KnobProbe::SegBoost(2.5),
            KnobProbe::VaqStrength(0.5),
            KnobProbe::VaqStrength(2.0),
            // __expert depth.
            KnobProbe::PartitionRange(4, 16),
            KnobProbe::PartitionRange(16, 64),
            KnobProbe::ComplexPredictionModes(true),
            KnobProbe::Lrf(true),
            KnobProbe::Lrf(false),
            KnobProbe::FastDeblock(true),
            KnobProbe::FastDeblock(false),
        ];
        axes
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
// Builder
// ============================================================================

/// Builds a [`SweepPlan`] from axes × quality grid under an optional
/// encode-cell budget.
#[derive(Clone, Debug)]
pub struct SweepBuilder {
    axes: SweepAxes,
    grid: QualityGrid,
    budget: Option<usize>,
}

impl SweepBuilder {
    /// New builder over the given axes and quality grid.
    #[must_use]
    pub fn new(axes: SweepAxes, grid: QualityGrid) -> Self {
        Self {
            axes,
            grid,
            budget: None,
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

    /// Build the plan.
    #[must_use]
    pub fn plan(&self) -> SweepPlan {
        let mut axes = self.axes.clone();
        let mut q_points = self.grid.points();
        let mut dropped = Vec::new();
        let mut q_coarsenings = 0u32;
        let mut over_budget = false;

        loop {
            let (cells, invalid_skipped, duplicates_merged) = cross(&axes, &q_points);

            let within = match self.budget {
                None => true,
                Some(b) => cells.len() <= b,
            };
            if within {
                return SweepPlan {
                    cells,
                    invalid_skipped,
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
            let (cells, invalid_skipped, duplicates_merged) = cross(&axes, &q_points);
            return SweepPlan {
                cells,
                invalid_skipped,
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
    // Tier order: cheapest-to-lose first. qm and subsampling are the
    // core RD axes and are never collapsed (floor = their full size);
    // speeds keeps at least two points so RD-vs-cost stays measurable.
    collapse("probes", &mut axes.probes, 1)
        .or_else(|| collapse("vaq", &mut axes.vaq, 1))
        .or_else(|| collapse("trellis", &mut axes.trellis, 1))
        .or_else(|| collapse("color_models", &mut axes.color_models, 1))
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
    vaq: bool,
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
            .with_vaq(self.vaq, 1.0);
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
        if self.vaq {
            s.push_str("-vaq");
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
/// - `vaq_strength` is hashed only when VAQ is enabled — zenrav1e never
///   reads the strength when `enable_vaq` is false (encode-validated by
///   `sweep_validate`);
/// - the CICP `matrix_coefficients` field is **excluded**: the zenravif
///   backend derives the signaled matrix from the color model and never
///   reads the field (encode-validated by `sweep_validate`; it IS
///   hashed when the svtav1 backend is selected, which does read it).
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

    // Backend.
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

    // Coefficient-level knobs after gates.
    h.u8(u8::from(config.qm_effective()));
    let vaq = config.vaq_effective();
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
    // matrix_coefficients: read by the svtav1 backend only — excluded on
    // zenravif (encode-validated; see fn docs).
    if config.backend == crate::Av1Backend::Svtav1 {
        h.opt_u8(config.matrix_coefficients);
    }
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
            vaq: vec![false],
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
            vaq: false,
            trellis: false,
            probe: KnobProbe::None,
        };
        let a = base.build_config(50.0).with_vaq(false, 1.0);
        let b = base.build_config(50.0).with_vaq(false, 3.0);
        assert_eq!(fingerprint(&a), fingerprint(&b));
        let c = base.build_config(50.0).with_vaq(true, 1.0);
        assert_ne!(fingerprint(&a), fingerprint(&c));
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
    fn matrix_coefficients_excluded_on_zenravif() {
        let base = EncoderConfig::new().quality(50.0);
        let with_mc = base.clone().matrix_coefficients(9);
        assert_eq!(
            fingerprint(&base),
            fingerprint(&with_mc),
            "zenravif backend never reads the CICP matrix field"
        );
    }
}

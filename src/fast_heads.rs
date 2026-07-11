//! Per-image fast-tier budget heads — the FAST_TIER_PARITY_PLAN Phase P2
//! "prediction replaces search" rules (FEATURE_HINTS §E hyperparameter
//! expert, second and third deterministic descriptor heads after
//! [`crate::palette_gate`]).
//!
//! Two pure threshold rules choose per-image search budgets for the s6-class
//! fast mode, spending the expensive levers only where the measured
//! per-image response surfaces say they pay:
//!
//! # Head 1 — TX budget (`speed 6..=8`)
//!
//! | rule (in order; both gates CONJUNCTIVE on two features) | budget | measured why |
//! |---|---|---|
//! | `patch_fraction > 0.8505 && dct_compressibility_y > 100` | [`TxBudget::Largest`] | razor-edge tiled plots (7050/7052-class: repeating patches + extreme DCT α) PAY +8.2..+19.3% BD under depth-1 size-RDO — withholding is a pure win (and saves the 1.67× tx time). The dcty bound is the VAL-attribution tightening: pf-high CHART content (5343/8103, dcty ≈ 8) still wants size-RDO |
//! | `patch_fraction <= 0.8505 && dct_compressibility_y < 8.352` | [`TxBudget::Min`] | SMOOTH/easy content (dcty = libwebp α, HIGHER = harder; products/people run 3..8 vs photo median ~16): sparse-AC residuals are where tx-TYPE RDO over the reduced set pays its premium (train −2.6..−4.5 extra BD; val 9021/9631 −6.4/−4.2 clean) |
//! | otherwise | [`TxBudget::Size1`] | the landed global default (P0: 51% of the s6→s4 step at 1.67×) |
//!
//! First fit 2026-07-04 on the fastwins per-image surfaces (train26,
//! veto-adjusted BD, LOOCV leave-one-origin-out): rule −5.84 mean at 2.04×
//! vs global-size1 −5.03 at 1.67× (s6); the withhold-only form dominates
//! global-size1 on BOTH axes (−5.42 at 1.56×). s8 same shape (−6.21 at
//! 1.92× vs −4.94 at 1.43×). The conjunctive bounds above are the val
//! attribution revision (fewer fires than the train-only fit — see the
//! constants' docs). `benchmarks/hyperparam_tx_budget_2026-07-04.tsv` +
//! `rd_gap_p2heads_2026-07-04.tsv`.
//!
//! # Head 2 — partition budget (`speed == 6`)
//!
//! | rule | budget | measured why |
//! |---|---|---|
//! | `gradient_fraction_smooth < 0.4105` | [`PartitionBudget::Max32`] | flat/synthetic content (plots/clipart/products/screens) is where 32-px partitions pay (fam-7000 m32 recovery 255%); photos (high smooth-gradient fraction) measured m32-adverse in the prange re-test |
//! | otherwise | [`PartitionBudget::Ship`] | the landed P1 pruned-liveness point (r16no4_bkvg2) |
//!
//! Fit 2026-07-04 on the p1part per-image surfaces: LOOCV −5.46 mean at
//! 2.41× vs global-ship −4.73 at 2.15× and global-vg2 −5.20 at 2.46× (beats
//! the vg2 rung on both axes). The withhold side measured NO stable win
//! (partition liveness pays on 24/24 train images — unlike tx); at s8/s4 the
//! per-image head adds ~nothing over the right global rung, so this head is
//! s6-only. `benchmarks/hyperparam_partition_budget_2026-07-04.tsv`.
//!
//! # Head 3 — intra-mode budget: measured, NOT a per-image head
//!
//! The top-7-vs-top-3 keyframe intra response (ComplexKeyframes +
//! `filter_intra=Some(false)`, the zenrav1e#5-safe form) measured as a small
//! BROAD win (s6 median −0.56%, s8 −1.17%, 17/24 better, composition-stable
//! on the partition ship point) with a single +1.4 regressor — no honest
//! per-image structure at n=24. It is a global fast-mode arm candidate
//! (ravif SpeedTweaks side), not a zenavif head. The top-5 midpoint knob
//! the first fit called out now EXISTS upstream
//! (`PredictionSpeedSettings::num_modes_rdo_override`, zenrav1e@071e9844,
//! default `None` = byte-identical 7|3): the S4TIER axis measured top-5 ≈
//! 90% of top-7's median value on the s6 base (i5 −0.51 vs i7 −0.56; s8
//! −1.09 vs −1.17), and at the s4-tier MODE level top-5 DOMINATES top-7 —
//! the same parity column (+2.80/+4.14 vs +2.84/+4.04 vs cpu2iq-ai) at
//! 6.26× vs 7.61× plain-s6 solo (the composed i7 marginal, 1.22×, buys
//! nothing). The s4-tier intra arm is top-5; top-7 stays the s6/s8
//! global-arm candidate at its own tiers.
//!
//! # Release gating
//!
//! Registry `zenrav1e` 0.1.4 has none of the underlying knobs (tx-RDO
//! decouple d82c16ba, topdown_prune 725f5f71/767c8ff5 landed on master
//! post-0.1.4), so today this module only *recommends*. Forwarding needs the
//! zenrav1e release + zenravif expert passthroughs for
//! `rdo_tx_type_override`/`reduced_tx_set`/`topdown_prune`/
//! `non_square_partition_max_threshold` (additive `InternalParams` fields,
//! same shape as the landed `sb_q_scale`) — see the CLAUDE.md dep-bump
//! checklist.

/// Per-image transform-search budget for the fast tier (head 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TxBudget {
    /// Withhold tx-size RDO entirely (`TX_MODE_LARGEST`, the stock s6+
    /// table): razor-edge tiled content where size-RDO pays bytes.
    Largest,
    /// Depth-1 tx-size RDO, DCT-only — the landed global default
    /// (`S6_TX_SIZE_RDO_LIVE` arm).
    #[default]
    Size1,
    /// Size1 + tx-type RDO over the reduced set — the P0 "min" point
    /// (92% of the s6→s4 tx step at 4.6× solo).
    Min,
}

/// Per-image partition-search budget for the fast tier (head 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PartitionBudget {
    /// The landed P1 pruned-liveness point (rects live at 16×16, 4-ways
    /// SPLIT-dominant-gated, breakout + homogeneity vargate 2.0).
    #[default]
    Ship,
    /// Ship + partition max 16→32 with 4-ways fully live (the measured
    /// `r16m32_bkvg2` pareto tip) — flat/synthetic content only.
    Max32,
}

/// Head-1 withhold threshold on zenanalyze `patch_fraction` (id 23).
///
/// Above it AND above [`TX_GATE_DCT_RAZOR_MIN`], per-image tx-size RDO
/// measured as a regression (train26 razor-edge plots 7050/7052: +19.3/+8.2
/// veto-adjusted BD). The pf axis alone is NOT sufficient: the VAL
/// attribution factoring (2026-07-04) showed pf-high CHART content (5343
/// hurricane chart, 8103 bls chart, pf 0.936-0.939) still WANTS size-RDO
/// ((none,ship) cost +18.1 ssim2 on 8103 while (size1,m32) won −1.9) —
/// hence the conjunctive dcty bound below.
pub const TX_GATE_PATCH_FRACTION_LARGEST: f32 = 0.8505;

/// Head-1 withhold co-threshold on `dct_compressibility_y` (id 21 —
/// libwebp α, HIGHER = harder to compress): the razor-edge near-lossless
/// class that size-RDO genuinely harms sits at EXTREME α (7050/7052:
/// 162.9/201.6) while the pf-high content that still wants size-RDO sits
/// ≤ 12.1 (7028/5343/8103). Bound
/// placed conservatively between the clusters (post-val tightening,
/// support n=5 in the pf>0.85 band; fires strictly FEWER images than the
/// train-only fit — harm-avoiding direction). This class is owned by the
/// intraBC/near-lossless program (P3); the gate is the interim guard.
pub const TX_GATE_DCT_RAZOR_MIN: f32 = 100.0;

/// Head-1 deep threshold on zenanalyze `dct_compressibility_y` (id 21).
///
/// Below it — SMOOTH/easy content with sparse AC (α well under the photo
/// median ~16), where tx-TYPE selection over the reduced set is the
/// classic residual win — AND at `patch_fraction ≤`
/// [`TX_GATE_PATCH_FRACTION_LARGEST`], the "min" tx set pays its 2.7× solo
/// premium over size1 (train26 2000/9228/9958 −2.6..−4.5 extra BD; VAL
/// 9021/9631 −6.4/−4.2 vs global-ship, clean butteraugli). The pf cap
/// keeps pf-high chart content (5343/8103-class, dcty 8.1-8.3) in Size1 —
/// their measured-best class (VAL factoring cells).
pub const TX_GATE_DCT_COMPRESSIBILITY_MIN: f32 = 8.352;

/// Head-1 deep threshold at the **s4-equivalent tier** (requested speed
/// 4..=5): the same rule form refit at the s4-tier time budget (the
/// cpu2iq-allintra wall = 1.27× the composed s6 mode left ~27% to spend;
/// fit_s4_tier.py on the fastwins labels, LOOCV 22/24-stable at λ=0.5 and
/// 0.25). Min fires 11/24 train26 images (was 3 at the s6 bound) — the
/// mid-α band (products/people/interiors/illustrations at dcty 8.8-22.6)
/// keeps paying for tx-TYPE RDO when the budget affords its premium. The
/// razor-edge W guard is UNCHANGED (harm class is budget-independent), as
/// is the pf cap (8414/8302/8268-class screens measured min-null/harm).
pub const TX_GATE_DCT_COMPRESSIBILITY_MIN_S4TIER: f32 = 23.69;

/// Head-2 upgrade threshold on zenanalyze `gradient_fraction_smooth`
/// (id 120). Below it (flat/synthetic, not smooth-gradient photo), the
/// Max32 partition rung pays (m32 recovery 255% on fam-7000; photos
/// measured m32-adverse in the prange (4,64) re-test).
pub const PART_GATE_GRADIENT_SMOOTH_MAX32: f32 = 0.4105;

/// First encoder speed of the tx head's measured tier (fit at s6, s8).
pub const TX_HEAD_MIN_SPEED: u8 = 6;
/// Last encoder speed of the tx head's measured tier.
pub const TX_HEAD_MAX_SPEED: u8 = 8;
/// The partition head's measured speed (the Max32 rung was measured at s6
/// only; s8/s4 per-image selection measured ≈ null over the global rung).
pub const PART_HEAD_SPEED: u8 = 6;
/// First requested speed of the **s4-equivalent tier** (the composed fast
/// mode with the richer budgets; the encoder still runs the speed-6
/// mechanics — the s4-native preset measured strictly worse at this wall:
/// s4+prune vs cpu2iq-ai +4.22 ssim2 at ~10× plain-s6 vs the composed
/// mode's +2.2 at ~6×, fit_s4_tier.py / rd_gap_s4tier record).
pub const S4_TIER_MIN_SPEED: u8 = 4;
/// Last requested speed of the s4-equivalent tier.
pub const S4_TIER_MAX_SPEED: u8 = 5;

/// Head 1: per-image TX budget from two descriptor features (see module
/// docs). Requested speed 6..=8 uses the s6-tier deep bound; 4..=5 (the
/// s4-equivalent tier) the refit
/// [`TX_GATE_DCT_COMPRESSIBILITY_MIN_S4TIER`] bound. Outside 4..=8, or on
/// non-finite features, returns the tier default [`TxBudget::Size1`] —
/// which the speed table already applies, i.e. "no per-image override".
#[must_use]
pub fn tx_budget_gate(patch_fraction: f32, dct_compressibility_y: f32, speed: u8) -> TxBudget {
    let deep_bound = if (S4_TIER_MIN_SPEED..=S4_TIER_MAX_SPEED).contains(&speed) {
        TX_GATE_DCT_COMPRESSIBILITY_MIN_S4TIER
    } else if (TX_HEAD_MIN_SPEED..=TX_HEAD_MAX_SPEED).contains(&speed) {
        TX_GATE_DCT_COMPRESSIBILITY_MIN
    } else {
        return TxBudget::Size1;
    };
    let pf_high = patch_fraction.is_finite() && patch_fraction > TX_GATE_PATCH_FRACTION_LARGEST;
    let pf_low = patch_fraction.is_finite() && patch_fraction <= TX_GATE_PATCH_FRACTION_LARGEST;
    if pf_high && dct_compressibility_y.is_finite() && dct_compressibility_y > TX_GATE_DCT_RAZOR_MIN
    {
        TxBudget::Largest
    } else if pf_low && dct_compressibility_y.is_finite() && dct_compressibility_y < deep_bound {
        TxBudget::Min
    } else {
        TxBudget::Size1
    }
}

/// Head 2: per-image partition budget (s6 + the s4-equivalent tier 4..=5;
/// see module docs — the s4-tier refit kept the s6 rule: the λ=0.25
/// alternative `gfs@0.6474` was LOOCV-flat and fires Max32 onto measured
/// m32-harm content). Non-finite feature or off-tier speed →
/// [`PartitionBudget::Ship`].
#[must_use]
pub fn partition_budget_gate(gradient_fraction_smooth: f32, speed: u8) -> PartitionBudget {
    if (speed == PART_HEAD_SPEED || (S4_TIER_MIN_SPEED..=S4_TIER_MAX_SPEED).contains(&speed))
        && gradient_fraction_smooth.is_finite()
        && gradient_fraction_smooth < PART_GATE_GRADIENT_SMOOTH_MAX32
    {
        PartitionBudget::Max32
    } else {
        PartitionBudget::Ship
    }
}

/// Monotonicity head threshold on `gradient_fraction_smooth` (id 120).
///
/// Below it (synthetic / graphic / scan content, not smooth-gradient photo)
/// the armed speed-5 tier is a measured **dominated valley**: it carries
/// fine_dir's cost yet lacks BOTH the s6-8 bundle (`rdo_tx_size` + intra
/// top-k + `topdown_prune`, armed s6+) AND s9's `reduced_tx_set` /
/// `inter_tx_split`, so a faster tier Pareto-dominates it. Measured on 12/24
/// train26 renditions (inverter gfs 0.08-0.612); clean photos sit at gfs >=
/// 0.675, so the threshold is placed in the clean gap (max inverter 0.612, min
/// clean 0.675). Provenance:
/// `benchmarks/mono_rd_vs_time_2026-07-05.tsv` +
/// `scripts/rd_gap/fit_content_gates.py` (2026-07-06).
pub const MONOTONE_GATE_GRADIENT_SMOOTH_MAX: f32 = 0.64;

/// The measured dominated valley speed (see [`monotone_speed_gate`]).
pub const MONOTONE_VALLEY_SPEED: u8 = 5;

/// Remap target for the valley: the fast tier that Pareto-dominates s5 on the
/// gated content. Chosen by simulation over {s4, s6, s9}
/// (`fit_content_gates.py`): **s9 removes all 17 valley inversions with 0 new
/// inversions** and the lowest RD delta, where s4 (slow) introduced 14 new
/// s6/7/8-dominated-by-s5 orderings and s6 (itself dominated by s4 on razor
/// plots) introduced 6 new s5<s4. s9 slightly regresses 3 borderline
/// clean-synthetic misfires (<= 4.4% bytes, still monotone) — no feature in
/// {gfs, pf, dcty} separates those from true inverters. **Held-out (15
/// doccharts origins distinct from train): s5->s9 removes 9 valley inversions
/// with 0 new; gate recall 9/10** — the safe-remap property generalizes
/// (benchmarks/mono_val_labels_doccharts_2026-07-06.tsv).
pub const MONOTONE_REMAP_SPEED: u8 = 9;

/// Release gate for [`monotone_speed_gate_for_rgb8`]'s APPLICATION. The s5
/// valley exists ONLY on the armed build (the s6+ bundle arms create it); on a
/// registry build s5 is *not* dominated — measured, it often BEATS s9 (6096:
/// registry-s5 170337/90.16 vs s9 198950/89.29), so remapping there would
/// REGRESS synthetic content (+17% bytes / -0.87 ssim2). The gate fires on
/// `gfs` regardless of build, so its application must be held OFF until the
/// arms go live. Flip to `true` at the zenrav1e dep bump alongside ravif's
/// S1_DEEP/S6_*/S10_RETIER const flips (dep-bump checklist). Byte-identical
/// while `false` (the pure [`monotone_speed_gate`] logic stays unit-tested).
pub const MONOTONE_GATE_LIVE: bool = true;

/// Monotonicity head (the 2026-07-05 user directive: "make sure our image
/// analysis provides monotonic rd improvement with time"). On synthetic
/// content (`gradient_fraction_smooth < `[`MONOTONE_GATE_GRADIENT_SMOOTH_MAX`])
/// the armed speed-5 tier is a dominated valley; remap it to
/// [`MONOTONE_REMAP_SPEED`] (the measured dominator) so spending s5's time can
/// never buy a worse RD point than the faster s9. Returns `speed` unchanged off
/// the gate, on a non-finite feature, or off [`MONOTONE_VALLEY_SPEED`] — a
/// clean no-op. This is the PURE remap logic (always active for unit-testing);
/// its APPLICATION via [`monotone_speed_gate_for_rgb8`] is release-gated by
/// [`MONOTONE_GATE_LIVE`] because the valley is armed-only — on registry s5 is
/// not dominated, so applying the remap there would regress synthetic content.
#[must_use]
pub fn monotone_speed_gate(gradient_fraction_smooth: f32, speed: u8) -> u8 {
    if speed == MONOTONE_VALLEY_SPEED
        && gradient_fraction_smooth.is_finite()
        && gradient_fraction_smooth < MONOTONE_GATE_GRADIENT_SMOOTH_MAX
    {
        MONOTONE_REMAP_SPEED
    } else {
        speed
    }
}

/// The composed per-image fast-tier recommendation (both heads).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FastTierBudgets {
    /// Head-1 transform-search budget.
    pub tx: TxBudget,
    /// Head-2 partition-search budget.
    pub partition: PartitionBudget,
}

/// Run both budget heads on RGB8 pixels via zenanalyze, reusing a shared
/// [`zenanalyze_api::Offer`] when its reuse key matches (the same
/// orchestrator contract as [`crate::palette_gate::palette_gate_for_rgb8`]).
/// Any failure path returns the defaults (Size1 + Ship — the landed global
/// fast-mode configuration, i.e. a clean no-override degrade).
#[cfg(feature = "auto-tune")]
#[must_use]
pub fn fast_tier_budgets_for_rgb8(
    rgb: &[u8],
    width: u32,
    height: u32,
    offer: Option<&zenanalyze_api::Offer<'_>>,
    speed: u8,
) -> FastTierBudgets {
    use zenanalyze::feature::{AnalysisFeature, AnalysisQuery, FeatureSet};

    const FEATURES: [AnalysisFeature; 3] = [
        AnalysisFeature::PatchFraction,
        AnalysisFeature::DctCompressibilityY,
        AnalysisFeature::GradientFractionSmooth,
    ];
    let names = [FEATURES[0].name(), FEATURES[1].name(), FEATURES[2].name()];

    let mk = |pf: f32, dcty: f32, gfs: f32| FastTierBudgets {
        tx: tx_budget_gate(pf, dcty, speed),
        partition: partition_budget_gate(gfs, speed),
    };

    if let Some(offer) = offer {
        let request = zenanalyze_api::Request::new(
            &names,
            zenanalyze::analyzer_version(),
            zenanalyze::feature_defs_version(),
            0,
        );
        if let Some(values) = offer.reuse_for(&request)
            && let [pf, dcty, gfs] = values[..]
        {
            return mk(pf, dcty, gfs);
        }
    }

    if rgb.is_empty() || width == 0 || height == 0 {
        return FastTierBudgets::default();
    }
    let query = AnalysisQuery::new(
        FeatureSet::new()
            .with(FEATURES[0])
            .with(FEATURES[1])
            .with(FEATURES[2]),
    );
    let analysis = zenanalyze::analyze_features_rgb8(rgb, width, height, &query);
    match (
        analysis.get_f32(FEATURES[0]),
        analysis.get_f32(FEATURES[1]),
        analysis.get_f32(FEATURES[2]),
    ) {
        (Some(pf), Some(dcty), Some(gfs)) => mk(pf, dcty, gfs),
        _ => FastTierBudgets::default(),
    }
}

/// Extract `gradient_fraction_smooth` and apply [`monotone_speed_gate`].
/// Requests the SAME 3-feature set as [`fast_tier_budgets_for_rgb8`] so a
/// shared [`zenanalyze_api::Offer`] reuses (call this first, then the budgets
/// on the returned effective speed). Any analysis failure returns `speed`
/// unchanged — a clean no-op (identical to the gate not firing).
#[cfg(feature = "auto-tune")]
#[must_use]
pub fn monotone_speed_gate_for_rgb8(
    rgb: &[u8],
    width: u32,
    height: u32,
    offer: Option<&zenanalyze_api::Offer<'_>>,
    speed: u8,
) -> u8 {
    use zenanalyze::feature::{AnalysisFeature, AnalysisQuery, FeatureSet};

    // Release-gated: the s5 valley (hence a correct remap) exists ONLY on the
    // armed build. On registry s5 is not dominated — applying the remap there
    // regresses synthetic content, so hold it OFF until the arms flip live.
    if !MONOTONE_GATE_LIVE {
        return speed;
    }
    // Off the valley speed the gate is a no-op regardless of content — skip
    // the analysis entirely (it also can't fire, keeping this cheap).
    if speed != MONOTONE_VALLEY_SPEED {
        return speed;
    }
    const FEATURES: [AnalysisFeature; 3] = [
        AnalysisFeature::PatchFraction,
        AnalysisFeature::DctCompressibilityY,
        AnalysisFeature::GradientFractionSmooth,
    ];
    let names = [FEATURES[0].name(), FEATURES[1].name(), FEATURES[2].name()];

    if let Some(offer) = offer {
        let request = zenanalyze_api::Request::new(
            &names,
            zenanalyze::analyzer_version(),
            zenanalyze::feature_defs_version(),
            0,
        );
        if let Some(values) = offer.reuse_for(&request)
            && let [_pf, _dcty, gfs] = values[..]
        {
            return monotone_speed_gate(gfs, speed);
        }
    }

    if rgb.is_empty() || width == 0 || height == 0 {
        return speed;
    }
    let query = AnalysisQuery::new(
        FeatureSet::new()
            .with(FEATURES[0])
            .with(FEATURES[1])
            .with(FEATURES[2]),
    );
    let analysis = zenanalyze::analyze_features_rgb8(rgb, width, height, &query);
    match analysis.get_f32(FEATURES[2]) {
        Some(gfs) => monotone_speed_gate(gfs, speed),
        None => speed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotone_gate_remaps_only_synthetic_s5() {
        // Synthetic inverters (gfs < 0.64) at the valley speed remap to s9;
        // clean photos (gfs >= 0.675) keep s5. Anchors from the fit's 24
        // train26 renditions (fit_content_gates.py).
        for gfs in [0.081_f32, 0.280, 0.436, 0.612] {
            assert_eq!(monotone_speed_gate(gfs, 5), MONOTONE_REMAP_SPEED);
        }
        for gfs in [0.675_f32, 0.714, 0.842] {
            assert_eq!(monotone_speed_gate(gfs, 5), 5);
        }
        // Off the valley speed: never remap, any content.
        for s in [1u8, 2, 4, 6, 7, 8, 9, 10] {
            assert_eq!(monotone_speed_gate(0.10, s), s);
        }
        // Non-finite feature degrades to no-op (never a spurious remap).
        assert_eq!(monotone_speed_gate(f32::NAN, 5), 5);
        assert_eq!(monotone_speed_gate(f32::INFINITY, 5), 5);
    }

    #[test]
    #[cfg(feature = "auto-tune")]
    fn monotone_gate_release_held_off_on_registry() {
        // A checkerboard has low gfs, so the PURE gate would remap s5→s9. But
        // the valley is armed-only; on registry s5 is not dominated (measured:
        // it beats s9). MONOTONE_GATE_LIVE holds the APPLICATION off so the
        // remap can't ship the +17%-bytes registry regression. This asserts the
        // no-op while the flag is false (the safety-critical state today).
        if !MONOTONE_GATE_LIVE {
            let (w, h) = (256u32, 256u32);
            let screen: Vec<u8> = (0..h)
                .flat_map(|y| {
                    (0..w).flat_map(move |x| {
                        if ((x / 16) + (y / 16)) % 2 == 0 {
                            [255u8, 255, 255]
                        } else {
                            [0u8, 32, 128]
                        }
                    })
                })
                .collect();
            assert_eq!(
                monotone_speed_gate_for_rgb8(&screen, w, h, None, 5),
                5,
                "applied gate must be a no-op while MONOTONE_GATE_LIVE is false"
            );
        }
    }

    #[test]
    fn tx_gate_measured_anchors() {
        // Anchors from the fit + VAL factoring: the razor-edge pair
        // 7050/7052 (pf>.85 AND dcty>100) withholds; pf-high CHART content
        // (7028 dcty 12.1, 5343 8.1, 8103 8.3) stays Size1 — the val
        // factoring measured (size1,m32) as their best class; the min class
        // (pf<=.85, dcty 7.9/7.1/3.2 + val 9021 4.3 / 9631 1.8) deepens;
        // 8196 screenshot (pf .379, dcty 30.5) stays size1.
        for s in [6u8, 7, 8] {
            assert_eq!(tx_budget_gate(0.994, 162.9, s), TxBudget::Largest);
            assert_eq!(tx_budget_gate(0.998, 201.6, s), TxBudget::Largest);
            assert_eq!(tx_budget_gate(0.901, 12.1, s), TxBudget::Size1);
            assert_eq!(tx_budget_gate(0.939, 8.1, s), TxBudget::Size1);
            assert_eq!(tx_budget_gate(0.936, 8.3, s), TxBudget::Size1);
            assert_eq!(tx_budget_gate(0.002, 7.9, s), TxBudget::Min);
            assert_eq!(tx_budget_gate(0.582, 7.1, s), TxBudget::Min);
            assert_eq!(tx_budget_gate(0.834, 4.3, s), TxBudget::Min);
            assert_eq!(tx_budget_gate(0.379, 30.5, s), TxBudget::Size1);
            // pf-high + dcty-low: NEITHER gate (the 5343/8103 corner).
            assert_eq!(tx_budget_gate(0.9, 1.0, s), TxBudget::Size1);
        }
        // Off-tier speeds: no override.
        for s in [2u8, 3, 9, 10] {
            assert_eq!(tx_budget_gate(0.998, 201.6, s), TxBudget::Size1);
        }
        // The s4-equivalent tier (requested speed 4..=5): the W guard is
        // budget-independent (razor-edge harm class), the D bound widens
        // to 23.69 — the mid-α band that stays Size1 at s6 fires Min here
        // (1236 dcty 21.5, 5004 22.6, 1614 18.1), while the 6018-class
        // (dcty 120.2 at pf 0.33: min/full measured harm) and the
        // 9118-class (dcty 38.5) stay Size1.
        for s in [4u8, 5] {
            assert_eq!(tx_budget_gate(0.998, 201.6, s), TxBudget::Largest);
            assert_eq!(tx_budget_gate(0.000, 21.5, s), TxBudget::Min);
            assert_eq!(tx_budget_gate(0.066, 22.6, s), TxBudget::Min);
            assert_eq!(tx_budget_gate(0.006, 18.1, s), TxBudget::Min);
            assert_eq!(tx_budget_gate(0.329, 120.2, s), TxBudget::Size1);
            assert_eq!(tx_budget_gate(0.000, 38.5, s), TxBudget::Size1);
            assert_eq!(tx_budget_gate(0.000, 23.69, s), TxBudget::Size1);
        }
        // ... and the same features stay Size1 at the s6-tier bound.
        assert_eq!(tx_budget_gate(0.000, 21.5, 6), TxBudget::Size1);
        assert_eq!(tx_budget_gate(0.066, 22.6, 6), TxBudget::Size1);
    }

    #[test]
    fn partition_gate_measured_anchors() {
        // m32 class anchors (gfs 0.081/0.111/0.298) vs photo class
        // (0.716/0.783); 9868 sits just inside the fitted 0.4105 (0.4086).
        assert_eq!(partition_budget_gate(0.081, 6), PartitionBudget::Max32);
        assert_eq!(partition_budget_gate(0.4086, 6), PartitionBudget::Max32);
        assert_eq!(partition_budget_gate(0.4105, 6), PartitionBudget::Ship);
        assert_eq!(partition_budget_gate(0.716, 6), PartitionBudget::Ship);
        // s6 + the s4-equivalent tier fire; everything else ships.
        for s in [4u8, 5] {
            assert_eq!(partition_budget_gate(0.081, s), PartitionBudget::Max32);
            assert_eq!(partition_budget_gate(0.716, s), PartitionBudget::Ship);
        }
        for s in [2u8, 3, 7, 8, 9, 10] {
            assert_eq!(partition_budget_gate(0.081, s), PartitionBudget::Ship);
        }
    }

    #[test]
    fn non_finite_degrades_to_defaults() {
        // Both gates are CONJUNCTIVE on two features — any non-finite input
        // falls through to Size1 (never spend or withhold on unverified
        // content).
        assert_eq!(tx_budget_gate(f32::NAN, f32::NAN, 6), TxBudget::Size1);
        assert_eq!(tx_budget_gate(f32::NAN, 5.0, 6), TxBudget::Size1);
        assert_eq!(tx_budget_gate(0.9, f32::NAN, 6), TxBudget::Size1);
        assert_eq!(tx_budget_gate(0.2, f32::NAN, 6), TxBudget::Size1);
        assert_eq!(partition_budget_gate(f32::NAN, 6), PartitionBudget::Ship);
    }

    /// The synthetic screen/photo pair from the palette-gate tests, run
    /// through the real zenanalyze pass: tiled flat content must land in
    /// the withhold+Max32 corner; hash-noise photo content must stay at
    /// the Size1+Ship defaults.
    #[cfg(feature = "auto-tune")]
    #[test]
    fn budgets_for_synthetic_content() {
        let (w, h) = (256u32, 256u32);
        let screen: Vec<u8> = (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| {
                    if ((x / 16) + (y / 16)) % 2 == 0 {
                        [255u8, 255, 255]
                    } else {
                        [0u8, 32, 128]
                    }
                })
            })
            .collect();
        let b = fast_tier_budgets_for_rgb8(&screen, w, h, None, 6);
        // Flat 2-color tiles have HIGH patch_fraction but LOW DCT α (most
        // blocks are flat -> sparse AC), so the revised conjunctive W-gate
        // correctly does NOT withhold (the withhold class is razor-edge
        // line tilings with EXTREME α, pinned by the measured-anchor test);
        // the partition head upgrades flat content to 32-blocks.
        assert_eq!(
            b.tx,
            TxBudget::Size1,
            "flat tiles are not the razor-edge withhold class"
        );
        assert_eq!(
            b.partition,
            PartitionBudget::Max32,
            "flat content upgrades to 32-blocks"
        );

        // Photo-side probe for the PARTITION gate: gradient_fraction_smooth
        // measures smooth-gradient mass (photo bokeh/sky), so the honest
        // high-gfs synthetic is a smooth gradient (probed: gfs 0.936 vs the
        // 0.4105 threshold; hash NOISE probes at gfs 0.237 — noise is
        // genuinely Max32-side content for this gate, matching the fit's
        // "flat/synthetic vs smooth-gradient-photo" split). The gradient's
        // patch_fraction collides to 1.0 (the DC-invariant signature
        // collision documented in palette_gate tests), so only the
        // partition assertion is meaningful here.
        let photo: Vec<u8> = (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| {
                    let g = ((x + y) * 255 / 510) as u8;
                    [g, g, 255 - g]
                })
            })
            .collect();
        let b = fast_tier_budgets_for_rgb8(&photo, w, h, None, 6);
        assert_eq!(
            b.partition,
            PartitionBudget::Ship,
            "smooth-gradient (photo-like) content stays at ship"
        );
    }

    /// Offer reuse short-circuits analysis (the auto_tune orchestrator
    /// contract) — values order matches the request's feature-name order.
    #[cfg(feature = "auto-tune")]
    #[test]
    fn offer_reuse_short_circuits() {
        use zenanalyze::feature::AnalysisFeature;
        let names = [
            AnalysisFeature::PatchFraction.name(),
            AnalysisFeature::DctCompressibilityY.name(),
            AnalysisFeature::GradientFractionSmooth.name(),
        ];
        let values = [0.95f32, 150.0, 0.1];
        let offer = zenanalyze_api::Offer::new(
            &names,
            &values,
            zenanalyze::analyzer_version(),
            zenanalyze::feature_defs_version(),
            0,
        );
        let b = fast_tier_budgets_for_rgb8(&[], 0, 0, Some(&offer), 6);
        assert_eq!(b.tx, TxBudget::Largest);
        assert_eq!(b.partition, PartitionBudget::Max32);
    }
}

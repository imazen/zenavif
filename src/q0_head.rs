//! q0-prediction head for target-quality mode — predicts the *starting*
//! quality for [`crate::target_quality::encode_rgb8_with_target`]'s
//! bracketed secant search from zenanalyze content features, so the search
//! converges in fewer encode+decode+score iterations than the content-blind
//! anchor curve (`initial_guess`).
//!
//! # What it is
//!
//! A deterministic fitted-constants head (the [`crate::fast_heads`] /
//! [`crate::palette_gate`] descriptor-head pattern — no model file): a
//! robust-L1 linear model over a piecewise-linear (hinge) target basis with
//! feature×target interactions,
//!
//! ```text
//! q0 = dot(COEFS, [1, tn, h50, h60, h70, h80, h85, speed_n, logpx_n,
//!                  f_0..f_7, f_0*tn..f_7*tn, f_0*h80..f_7*h80])
//! ```
//!
//! where `tn = (t-65)/25`, `h_k = max(t-k, 0)/10`, `speed_n = (speed-5)/5`,
//! `logpx_n = (ln(px)-13)/3`, and `f_*` are the eight zenanalyze features in
//! [`Q0_FEATURES`] order (`distinct_color_bins` is `ln_1p`-transformed).
//!
//! # Fit provenance (2026-07-05)
//!
//! `scripts/hyperparam/fit_q0_head.py` on the hyperparam label store
//! (`hyperparam-labels-2026-07-03`, `q_kind = cavif_q`, zenrav1e rows only):
//! per-image q→ssim2 curves from 22 ship-of-era arms across 10 sweeps
//! (speeds 1–10, size slots 256–top), PAVA-isotonized and inverse-labeled at
//! targets {40,45,…,90} via the leftmost crossing (= the runtime's
//! "smallest file in band" policy). 5,687 labels, LSD-train fit / LSD-val
//! evaluation (14 val origins), greedy feature selection by
//! leave-one-origin-out p90 on train only.
//!
//! **Fit quality (|q0 − q*|, LSD-val): p50 2.15, p90 7.25.** The
//! pre-registered p90 ≤ 6 gate is NOT met — misses concentrate at t ≥ 85
//! where the ssim2→q inversion is ill-conditioned (curve saturation; val
//! p90 10.8/11.4 at t=85/90 vs 5.3–6.1 at t=45–70). The sanctioned MLP
//! escalation was measured and REJECTED: a 24-unit zenpredict-shape MLP
//! fits train harder (p90 4.08) and generalizes WORSE (val p90 7.70) — the
//! binding constraint is origin-level transfer, not model capacity
//! (cross-arm label-noise floor: p90 ≈ 2.1). The head ships because the
//! *actual* objective — encodes-to-converge — improves decisively in the
//! offline secant simulation on held-out val curves (verbatim
//! `search_target` port, tolerance 0.5, max 6):
//!
//! | metric | anchor-curve seed | q0-head seed |
//! |---|---|---|
//! | mean encodes | 3.75 | 2.72 |
//! | median encodes | 4 | 3 |
//! | converged ≤ 6 encodes | 688/731 | 731/731 |
//! | done in ≤ 2 encodes | 11.5% | 26.8% |
//!
//! Full tables: `benchmarks/q0_head_fit_2026-07-05.tsv`.
//!
//! # Scope
//!
//! Fitted for [`crate::TargetMetric::Ssim2`] targets in 40–90 at any speed
//! 1–10 (labels span s1–s10). Outside that target band the basis input is
//! clamped to the band edge (the search's own extrapolation handles the
//! rest). NOT fitted for zensim targets — the caller keeps the anchor curve
//! there. Features unavailable (empty pixels, analysis failure) → `None`,
//! caller falls back to the anchor curve: the head never degrades the
//! current behavior, it only re-seeds the search.

/// The eight zenanalyze features, in fit order. `distinct_color_bins`
/// (index 4) is `ln_1p`-transformed before entering the dot product.
#[cfg(feature = "auto-tune")]
pub const Q0_FEATURES: [zenanalyze::feature::AnalysisFeature; 8] = [
    zenanalyze::feature::AnalysisFeature::FlatColorBlockRatio,
    zenanalyze::feature::AnalysisFeature::PatchFraction,
    zenanalyze::feature::AnalysisFeature::Uniformity,
    zenanalyze::feature::AnalysisFeature::HighFreqEnergyRatio,
    zenanalyze::feature::AnalysisFeature::DistinctColorBins,
    zenanalyze::feature::AnalysisFeature::NoiseFloorY,
    zenanalyze::feature::AnalysisFeature::GradientFraction,
    zenanalyze::feature::AnalysisFeature::GradientFractionSmooth,
];

/// Fitted M5-l1-q-hinge-h80 coefficients (fit_q0_head.py 2026-07-05,
/// val p50/p90 = 2.15/7.25, train 1.82/6.35). Layout: [const, tn, h50,
/// h60, h70, h80, h85, speed_n, logpx_n] ++ feats[8] ++ feats*tn[8] ++
/// feats*h80[8].
const Q0_COEFS: [f32; 33] = [
    45.274_72,   // const
    16.965_99,   // tn
    1.268_446,   // h50
    2.254_219,   // h60
    3.931_834,   // h70
    -3.764_774,  // h80
    5.804_622,   // h85
    0.881_025,   // speed_n
    2.367_783,   // logpx_n
    -3.530_724,  // flat_color_block_ratio
    -11.460_023, // patch_fraction
    -14.995_291, // uniformity
    5.530_201,   // high_freq_energy_ratio
    1.207_155,   // ln_1p(distinct_color_bins)
    -8.744_402,  // noise_floor_y
    -16.150_45,  // gradient_fraction
    6.266_575,   // gradient_fraction_smooth
    -9.584_532,  // flat_color_block_ratio*tn
    -8.128_057,  // patch_fraction*tn
    5.679_198,   // uniformity*tn
    -0.871_462,  // high_freq_energy_ratio*tn
    0.584_576,   // ln_1p(distinct_color_bins)*tn
    -4.473_643,  // noise_floor_y*tn
    4.562_962,   // gradient_fraction*tn
    -10.327_946, // gradient_fraction_smooth*tn
    -1.005_466,  // flat_color_block_ratio*h80
    5.389_818,   // patch_fraction*h80
    18.228_61,   // uniformity*h80
    -1.888_892,  // high_freq_energy_ratio*h80
    0.472_011,   // ln_1p(distinct_color_bins)*h80
    3.236_563,   // noise_floor_y*h80
    1.718_392,   // gradient_fraction*h80
    -5.155_774,  // gradient_fraction_smooth*h80
];

/// Fitted target band. Basis inputs are clamped here; the search's own
/// bracketing handles targets outside it from the band-edge seed.
const Q0_T_MIN: f32 = 40.0;
const Q0_T_MAX: f32 = 90.0;

/// Pure evaluation of the fitted head on already-extracted feature values
/// (in [`Q0_FEATURES`] order, RAW — the `ln_1p` transform for
/// `distinct_color_bins` is applied here). Returns q0 clamped to
/// `[1, 100]`. `None` if any input is non-finite.
#[must_use]
pub fn predict_q0_from_features(
    features: &[f32; 8],
    target_ssim2: f32,
    speed: u8,
    pixels: u64,
) -> Option<f32> {
    if !target_ssim2.is_finite() || features.iter().any(|f| !f.is_finite()) {
        return None;
    }
    let t = target_ssim2.clamp(Q0_T_MIN, Q0_T_MAX);
    let tn = (t - 65.0) / 25.0;
    let h = |k: f32| (t - k).max(0.0) / 10.0;
    let speed_n = (f32::from(speed) - 5.0) / 5.0;
    let logpx_n = ((pixels.max(1) as f32).ln() - 13.0) / 3.0;

    // Feature vector with the documented transform applied.
    let mut fv = *features;
    fv[4] = fv[4].max(0.0).ln_1p(); // distinct_color_bins

    let mut x = [0.0f32; 33];
    x[0] = 1.0;
    x[1] = tn;
    x[2] = h(50.0);
    x[3] = h(60.0);
    x[4] = h(70.0);
    x[5] = h(80.0);
    x[6] = h(85.0);
    x[7] = speed_n;
    x[8] = logpx_n;
    let h80 = h(80.0);
    for i in 0..8 {
        x[9 + i] = fv[i];
        x[17 + i] = fv[i] * tn;
        x[25 + i] = fv[i] * h80;
    }
    let q0: f32 = Q0_COEFS.iter().zip(x.iter()).map(|(c, v)| c * v).sum();
    Some(q0.clamp(1.0, 100.0))
}

/// Predict the starting quality for an ssim2-targeted encode of RGB8
/// pixels, extracting [`Q0_FEATURES`] via zenanalyze — reusing a shared
/// [`zenanalyze_api::Offer`] when its reuse key matches (the same
/// orchestrator contract as [`crate::palette_gate::palette_gate_for_rgb8`]).
///
/// `None` on any failure path (empty input, feature extraction failure,
/// non-finite values) — the caller keeps the content-blind anchor curve, so
/// this head can only ever *re-seed* the search, never break it.
#[cfg(feature = "auto-tune")]
#[must_use]
pub fn predict_q0_for_rgb8(
    rgb: &[u8],
    width: u32,
    height: u32,
    target_ssim2: f64,
    speed: u8,
    offer: Option<&zenanalyze_api::Offer<'_>>,
) -> Option<f32> {
    use zenanalyze::feature::{AnalysisQuery, FeatureSet};

    let pixels = u64::from(width) * u64::from(height);
    let names: [&str; 8] = core::array::from_fn(|i| Q0_FEATURES[i].name());

    if let Some(offer) = offer {
        let request = zenanalyze_api::Request::new(
            &names,
            zenanalyze::analyzer_version(),
            zenanalyze::feature_defs_version(),
            0,
        );
        if let Some(values) = offer.reuse_for(&request)
            && let Ok(arr) = <[f32; 8]>::try_from(values.as_slice())
        {
            return predict_q0_from_features(&arr, target_ssim2 as f32, speed, pixels);
        }
    }

    if rgb.is_empty() || width == 0 || height == 0 {
        return None;
    }
    let mut set = FeatureSet::new();
    for f in Q0_FEATURES {
        set = set.with(f);
    }
    let analysis = zenanalyze::analyze_features_rgb8(rgb, width, height, &AnalysisQuery::new(set));
    let mut arr = [0.0f32; 8];
    for (i, f) in Q0_FEATURES.iter().enumerate() {
        arr[i] = analysis.get_f32(*f)?;
    }
    predict_q0_from_features(&arr, target_ssim2 as f32, speed, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Python-fixture golden: exact re-evaluation of the fitted formula on
    /// hand-fed feature vectors (values from fit_q0_head.py's coefficient
    /// table — guards transcription and basis-order drift).
    #[test]
    fn formula_matches_fit_fixtures() {
        // photo-like: moderate gradients, some noise, few flat blocks
        let photo = [0.05f32, 0.1, 0.2, 0.3, 5000.0, 0.02, 0.4, 0.7];
        // synthetic/plot-like: flat blocks, patches, uniform
        let synth = [0.8f32, 0.95, 0.9, 0.05, 64.0, 0.0, 0.05, 0.1];
        // Expected values computed with the Python fit code (float64) —
        // f32 evaluation stays within 1e-3.
        let cases = [
            (photo, 75.0f32, 6u8, 786_432u64, 66.207_56f32),
            (photo, 85.0, 6, 786_432, 80.558_65),
            (synth, 60.0, 2, 1_048_576, 22.967_05),
            (synth, 90.0, 8, 65_536, 71.509_16),
        ];
        for (fv, t, s, px, expect) in cases {
            let got = predict_q0_from_features(&fv, t, s, px).unwrap();
            assert!(
                (got - expect).abs() < 1e-2,
                "q0({t}, s{s}) = {got}, expected {expect}"
            );
        }
    }

    #[test]
    fn bounds_and_band_clamp() {
        let fv = [0.2f32, 0.3, 0.4, 0.2, 1000.0, 0.01, 0.3, 0.5];
        for t in [-10.0f32, 0.0, 40.0, 65.0, 90.0, 99.0, 150.0] {
            for s in [1u8, 4, 10] {
                let q0 = predict_q0_from_features(&fv, t, s, 262_144).unwrap();
                assert!((1.0..=100.0).contains(&q0), "q0({t},{s}) = {q0}");
            }
        }
        // Outside the fitted band the basis input is clamped: t=99 and
        // t=150 must produce the SAME seed (band edge).
        let a = predict_q0_from_features(&fv, 99.0, 4, 262_144).unwrap();
        let b = predict_q0_from_features(&fv, 150.0, 4, 262_144).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn non_finite_inputs_refused() {
        let mut fv = [0.2f32, 0.3, 0.4, 0.2, 1000.0, 0.01, 0.3, 0.5];
        assert!(predict_q0_from_features(&fv, f32::NAN, 4, 100).is_none());
        fv[3] = f32::INFINITY;
        assert!(predict_q0_from_features(&fv, 70.0, 4, 100).is_none());
    }

    /// Extraction path: deterministic, in-bounds, and Offer-reuse produces
    /// the identical seed as the own-analysis pass (fast_heads contract).
    #[cfg(feature = "auto-tune")]
    #[test]
    fn offer_reuse_matches_own_pass() {
        let (w, h) = (128u32, 96u32);
        let rgb: Vec<u8> = (0..(w * h * 3))
            .map(|i| (i.wrapping_mul(13) % 251) as u8)
            .collect();

        let own = predict_q0_for_rgb8(&rgb, w, h, 78.0, 6, None);
        assert!(own.is_some(), "features must extract on plain RGB8");
        let q0 = own.unwrap();
        assert!((1.0..=100.0).contains(&q0));

        // Build a matching Offer exactly as an orchestrator would.
        use zenanalyze::feature::{AnalysisQuery, FeatureSet};
        let mut set = FeatureSet::new();
        for f in Q0_FEATURES {
            set = set.with(f);
        }
        let analysis = zenanalyze::analyze_features_rgb8(&rgb, w, h, &AnalysisQuery::new(set));
        let names: Vec<&str> = Q0_FEATURES.iter().map(|f| f.name()).collect();
        let values: Vec<f32> = Q0_FEATURES
            .iter()
            .map(|f| analysis.get_f32(*f).unwrap())
            .collect();
        let offer = zenanalyze_api::Offer::new(
            &names,
            &values,
            zenanalyze::analyzer_version(),
            zenanalyze::feature_defs_version(),
            0,
        );
        assert_eq!(
            predict_q0_for_rgb8(&rgb, w, h, 78.0, 6, Some(&offer)),
            own,
            "offer reuse must produce the identical seed"
        );
    }
}

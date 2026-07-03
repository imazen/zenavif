//! Deterministic zenanalyze palette gate — the FEATURE_HINTS §E "hyperparameter
//! expert" rule that replaces zenrav1e's ported AA-aware screen-content
//! detection where downscaling has blinded it.
//!
//! # The rule
//!
//! Fire `PaletteMode::Always` iff `patch_fraction > 0.197`; otherwise keep
//! `PaletteMode::Auto` (the encoder's own per-keyframe detection). No model
//! file — a single-threshold descriptor rule, same shape as the planned
//! Yuv400-for-grayscale gate (`GrayscaleScore >= 0.99`).
//!
//! # Provenance (measured, not designed)
//!
//! * Fit: `scripts/hyperparam/fit_palette_gate.py` on the §E label store —
//!   train26 palette-ab labels @1024 (s2+s6) + WEDGE-FINDER size-transfer
//!   evidence. LOOCV −10.91 vs −11.05 fire-everything ceiling;
//!   30 fire&won / 2 fire&lost (≈0-cost); butteraugli-vetoed wins never
//!   banked. `docs/HYPERPARAM_FIRST_CUT_2026-07-03.md` rule 1.
//! * Mechanism A/B across sizes {256,512,1024} × configs {isolated rav1e CLI,
//!   shipped cavif} × val origins: `scripts/hyperparam/analyze_palette_mech_ab.py`
//!   + `benchmarks/hyperparam_palette_mech_ab_2026-07-03.tsv`. The gate's
//!   value concentrates at the ≥512/native slots where the ported detection
//!   is dead but forced palette still wins; false fires on photos measured
//!   ≈0 BD (identical or near-identical bytes — RDO simply declines palette
//!   blocks) at a bounded encode-time cost on fired files only.
//! * `patch_fraction` (zenanalyze id 23) is the strongest single
//!   screen-vs-photo discriminator (AUC 0.880) and — measured in the wedge
//!   program — keeps its screen-vs-photo separation at every rendition size,
//!   i.e. it sees "this WAS screen content" through the resample while the
//!   encoder-side detection (which sees only post-resample AA'd edges) fires
//!   on 5/16 of the ≤512 wedge cells the gate catches.
//!
//! # Release gating
//!
//! Registry `zenrav1e` 0.1.4 has NO palette tool (it landed on master
//! 2026-07-03, `zenrav1e@68a8d81f..df27117c`), so today this module only
//! *recommends*. The [`EncoderConfig::palette_preference`] plumbing applies
//! the recommendation to the actual encoder once the zenravif → zenrav1e dep
//! chain bumps past 0.1.4 (see the dep-bump checklist in CLAUDE.md "Known
//! Bugs" → palette mode).

/// Palette-mode preference for the AV1 encoder's screen-content palette tool.
///
/// Mirrors zenrav1e's `PaletteMode` (which registry builds don't expose yet —
/// see the module docs). `Auto` defers to the encoder's own
/// antialiasing-aware per-keyframe detection; `Always` arms the palette
/// search unconditionally (RDO still decides per block); `Off` disables it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PalettePreference {
    /// Encoder's own screen-content detection decides (the AV1 default).
    #[default]
    Auto,
    /// Arm the palette search on every keyframe (RDO gates per block).
    Always,
    /// Never search palette modes.
    Off,
}

/// The fitted gate threshold on zenanalyze `patch_fraction`.
///
/// Fit 2026-07-03 on the §E label store (LOOCV −10.91 vs −11.05 ceiling) and
/// re-validated by the mechanism A/B's val-origin refit (see module docs).
/// `patch_fraction` ∈ [0,1]: fraction of sampled blocks exactly matching
/// another block — photos p90 ≈ 0.037, screens p50 ≈ 0.726.
pub const PALETTE_GATE_PATCH_FRACTION: f32 = 0.197;

/// The deterministic palette gate: `patch_fraction > 0.197` → [`PalettePreference::Always`].
///
/// Degrades cleanly: a non-finite `patch_fraction` (analysis failed,
/// feature unavailable) returns [`PalettePreference::Auto`] — the encoder's
/// own detection stays in charge, which is exactly the pre-gate behavior.
#[must_use]
pub fn palette_gate(patch_fraction: f32) -> PalettePreference {
    if patch_fraction.is_finite() && patch_fraction > PALETTE_GATE_PATCH_FRACTION {
        PalettePreference::Always
    } else {
        PalettePreference::Auto
    }
}

/// Run the palette gate on RGB8 pixels via zenanalyze, reusing a shared
/// [`zenanalyze_api::Offer`] when its reuse key matches the *current*
/// zenanalyze (same `major.minor`, `feature_defs_version`, default config) —
/// the same orchestrator contract as [`crate::EncoderConfig::auto_tune`],
/// keyed by the live analyzer instead of a baked model (this rule has no
/// model file).
///
/// Any failure path — empty pixels, feature missing from the offer AND
/// unanalyzable — resolves to [`PalettePreference::Auto`] (clean degrade).
#[cfg(feature = "auto-tune")]
#[must_use]
pub fn palette_gate_for_rgb8(
    rgb: &[u8],
    width: u32,
    height: u32,
    offer: Option<&zenanalyze_api::Offer<'_>>,
) -> PalettePreference {
    use zenanalyze::feature::{AnalysisFeature, AnalysisQuery, FeatureSet};

    const FEATURE: AnalysisFeature = AnalysisFeature::PatchFraction;
    let names = [FEATURE.name()];

    // Offer reuse: valid iff produced by the same feature definitions this
    // gate's threshold was fit against (stamped by the live zenanalyze).
    if let Some(offer) = offer {
        let request = zenanalyze_api::Request::new(
            &names,
            zenanalyze::analyzer_version(),
            zenanalyze::feature_defs_version(),
            0, // canonical default analysis config
        );
        if let Some(values) = offer.reuse_for(&request) {
            if let Some(&pf) = values.first() {
                if pf.is_finite() {
                    return palette_gate(pf);
                }
            }
        }
    }

    // Own-pass: a single Tier-1 feature (≤ ~14 ms at 4 MP per the P0 cost
    // grid; sub-ms at thumbnail sizes).
    if rgb.is_empty() || width == 0 || height == 0 {
        return PalettePreference::Auto;
    }
    let query = AnalysisQuery::new(FeatureSet::new().with(FEATURE));
    let analysis = zenanalyze::analyze_features_rgb8(rgb, width, height, &query);
    match analysis.get_f32(FEATURE) {
        Some(pf) if pf.is_finite() => palette_gate(pf),
        _ => PalettePreference::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_thresholds() {
        // The rule is strict-greater at the fitted threshold.
        assert_eq!(palette_gate(0.197), PalettePreference::Auto);
        assert_eq!(palette_gate(0.198), PalettePreference::Always);
        assert_eq!(palette_gate(0.0), PalettePreference::Auto);
        assert_eq!(palette_gate(1.0), PalettePreference::Always);
        // Measured corpus anchors (imazen26_features_2026-06-23.parquet):
        // screens keep patch_fraction >= 0.44 down to 256px; photos <= 0.008.
        assert_eq!(palette_gate(0.44), PalettePreference::Always);
        assert_eq!(palette_gate(0.008), PalettePreference::Auto);
    }

    #[test]
    fn non_finite_degrades_to_auto() {
        // Any non-finite feature value is an analysis failure — Auto is the
        // safe reading (the encoder's own detection stays in charge).
        assert_eq!(palette_gate(f32::NAN), PalettePreference::Auto);
        assert_eq!(palette_gate(f32::NEG_INFINITY), PalettePreference::Auto);
        assert_eq!(palette_gate(f32::INFINITY), PalettePreference::Auto);
    }

    /// Screen-like synthetic content (repeated flat patches) fires the gate;
    /// photo-like content (smooth non-repeating gradient + noise) does not.
    /// Exercises the real zenanalyze own-pass path — no palette encoder
    /// needed at runtime, so this test is NOT release-gated.
    #[cfg(feature = "auto-tune")]
    #[test]
    fn gate_fires_on_synthetic_screen_not_on_gradient() {
        let (w, h) = (256u32, 256u32);
        // Screen-like: 16x16 tiles alternating between two flat colors —
        // maximal block-repetition (patch_fraction -> ~1).
        let screen: Vec<u8> = (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| {
                    let tile = ((x / 16) + (y / 16)) % 2;
                    if tile == 0 { [255u8, 255, 255] } else { [0u8, 32, 128] }
                })
            })
            .collect();
        assert_eq!(
            palette_gate_for_rgb8(&screen, w, h, None),
            PalettePreference::Always,
            "flat repeated tiles must fire the gate"
        );

        // Photo-like: strong aperiodic hash noise around mid-gray. The DCT
        // signature behind patch_fraction is DC-invariant and sign-based, so
        // a smooth global gradient (uniform AC sign structure) COLLIDES like
        // screen content does — genuinely photo-scoring synthetic content
        // needs per-block-unique AC structure, i.e. real texture. Probed:
        // this scores patch_fraction = 0.0000; a gradient + weak noise
        // scores 0.58 (fires, correctly — that's the 9226 smooth-gradient
        // product-shot class the A/B measured).
        let photo: Vec<u8> = (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| {
                    let v = x
                        .wrapping_mul(2_654_435_761)
                        .wrapping_add(y.wrapping_mul(2_246_822_519));
                    let v = (v ^ (v >> 15)).wrapping_mul(2_246_822_519);
                    [
                        (128 + ((v >> 8) & 0x3f) as i32 - 32) as u8,
                        (128 + ((v >> 14) & 0x3f) as i32 - 32) as u8,
                        (128 + ((v >> 20) & 0x3f) as i32 - 32) as u8,
                    ]
                })
            })
            .collect();
        assert_eq!(
            palette_gate_for_rgb8(&photo, w, h, None),
            PalettePreference::Auto,
            "smooth noisy gradient must not fire the gate"
        );
    }

    /// Degrade path: empty/degenerate input recommends Auto.
    #[cfg(feature = "auto-tune")]
    #[test]
    fn degenerate_input_degrades_to_auto() {
        assert_eq!(palette_gate_for_rgb8(&[], 0, 0, None), PalettePreference::Auto);
    }

    /// A matching Offer is reused verbatim (no own-pass): supplying a fired
    /// patch_fraction via the offer flips the recommendation without pixels.
    #[cfg(feature = "auto-tune")]
    #[test]
    fn offer_reuse_short_circuits_analysis() {
        use zenanalyze::feature::AnalysisFeature;
        let names = [AnalysisFeature::PatchFraction.name()];
        let values = [0.9f32];
        let offer = zenanalyze_api::Offer::new(
            &names,
            &values,
            zenanalyze::analyzer_version(),
            zenanalyze::feature_defs_version(),
            0,
        );
        // Pixels say "photo" (flat gray = zero repetition? flat IS repetitive —
        // use empty pixels: with a valid offer the pixels are never touched).
        assert_eq!(
            palette_gate_for_rgb8(&[], 0, 0, Some(&offer)),
            PalettePreference::Always,
            "offered patch_fraction=0.9 must fire without an analysis pass"
        );
    }
}

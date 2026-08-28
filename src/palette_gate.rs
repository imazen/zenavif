//! Deterministic zenanalyze palette gate — the FEATURE_HINTS §E "hyperparameter
//! expert" rule that replaces zenrav1e's ported AA-aware screen-content
//! detection where downscaling has blinded it.
//!
//! # The rule (speed-conditional since 2026-07-03)
//!
//! Fire `PaletteMode::Always` iff `patch_fraction` exceeds the speed tier's
//! threshold; otherwise keep `PaletteMode::Auto` (the encoder's own
//! per-keyframe detection). No model file — a per-speed-tier
//! single-threshold descriptor rule, same shape as the planned
//! Yuv400-for-grayscale gate (`GrayscaleScore >= 0.99`).
//!
//! | speed tier | threshold | provenance |
//! |---|---|---|
//! | slow, `speed <= 5` | `> 0.197` | first-cut fit + mechanism A/B (fitted and confirmed at s2; s1/s3-s5 unmeasured — the conservative fewer-fires side) |
//! | fast, `speed >= 6` | `> 0.05`  | speed-conditional A/B (fitted at s6 train, confirmed s6 val, corroborated at s8; s7/s9/s10 same-tier assumption) |
//!
//! At fast speeds the encoder's RDO search is weak enough that the palette
//! tool's exact-color path wins even on quiet illustration-ish content
//! (6600-class scans: −0.6..−2.6 BD at s6, butteraugli agreeing), while at
//! s2 the same firings measured ≈0-to-negative — the threshold is genuinely
//! speed-conditional. 0.05 keeps photos out (val photo firing rate 2.9% vs
//! 0.4% at 0.197; val photo patch_fraction p90 0.008 / p95 0.032): firing
//! costs palette search time (median 1.80× at s6 / 2.13× at s8 on fired
//! cells), so the residual −0.19 mean BD sitting below pf 0.05
//! (9094/1000-class cells overlapping the photo pf distribution) is
//! deliberately left unclaimed at the speed-oriented tier.
//! `benchmarks/hyperparam_palette_speed_ab_2026-07-03.tsv`.
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

/// The slow-tier (speed ≤ 5) gate threshold on zenanalyze `patch_fraction`.
///
/// Fit 2026-07-03 on the §E label store (LOOCV −10.91 vs −11.05 ceiling),
/// re-validated by the mechanism A/B's val-origin refit, and re-confirmed by
/// the speed-conditional A/B (s2 refit plateau keeps 0.197; lower arms add
/// nothing consistent at s2 — see module docs). `patch_fraction` ∈ [0,1]:
/// fraction of sampled blocks exactly matching another block — photos p90 ≈
/// 0.037, screens p50 ≈ 0.726.
pub const PALETTE_GATE_PATCH_FRACTION: f32 = 0.197;

/// The fast-tier (speed ≥ [`PALETTE_GATE_FAST_TIER_MIN_SPEED`]) threshold.
///
/// Speed-conditional A/B 2026-07-03: at s6 the t0.05 arm improved deploy
/// mean BD −0.047 (train) / −0.074 (val) over 0.197 with every flipped
/// winner butteraugli-clean, while keeping the photo firing rate at 2.9%.
/// The deeper refit picks (0.01-0.015) and fire-always claim another −0.19
/// mean but cross into the photo `patch_fraction` mass at 1.80×/2.13×
/// (s6/s8) fired encode cost — rejected for a speed tier. Corroborated at
/// s8 (−0.044 val, same direction — see the benchmarks TSV).
pub const PALETTE_GATE_PATCH_FRACTION_FAST: f32 = 0.05;

/// First encoder speed of the fast gate tier (rav1e/ravif speed domain,
/// 1-10). Measured at s6 + s8; the boundary sits at 6 because 5-and-below
/// was never measured to benefit from the lower threshold (s2 measured
/// against it) — the unmeasured s3-s5 default to the conservative
/// fewer-fires slow tier.
pub const PALETTE_GATE_FAST_TIER_MIN_SPEED: u8 = 6;

/// The gate threshold for an encoder speed (see the module-docs table).
#[must_use]
pub fn palette_gate_threshold(speed: u8) -> f32 {
    if speed >= PALETTE_GATE_FAST_TIER_MIN_SPEED {
        PALETTE_GATE_PATCH_FRACTION_FAST
    } else {
        PALETTE_GATE_PATCH_FRACTION
    }
}

/// The deterministic palette gate: `patch_fraction >` the speed tier's
/// threshold → [`PalettePreference::Always`] (0.197 at speed ≤ 5, 0.05 at
/// speed ≥ 6 — see the module docs for the measured provenance).
///
/// Degrades cleanly: a non-finite `patch_fraction` (analysis failed,
/// feature unavailable) returns [`PalettePreference::Auto`] — the encoder's
/// own detection stays in charge, which is exactly the pre-gate behavior.
#[must_use]
pub fn palette_gate(patch_fraction: f32, speed: u8) -> PalettePreference {
    if patch_fraction.is_finite() && patch_fraction > palette_gate_threshold(speed) {
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
/// `speed` is the encoder speed the gate's recommendation will run at
/// (rav1e/ravif 1-10) — it selects the tier threshold per the module docs;
/// [`crate::EncoderConfig::auto_tune`] passes the speed it just picked.
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
    speed: u8,
) -> PalettePreference {
    use zenanalyze::feature::{AnalysisFeature, AnalysisQuery, FeatureSet};

    const FEATURE: AnalysisFeature = AnalysisFeature::PatchFraction;
    let names = [FEATURE.name()];

    // Offer reuse: valid iff produced by the same feature definitions this
    // gate's threshold was fit against. `reuse_pinned` builds the wanted
    // identity from THIS build's code version for the feature, so a drifted
    // definition misses and we fall through to our own pass — a finer gate than
    // the whole-build `feature_defs_version` stamp it replaces.
    if let Some(offer) = offer
        && let Some(values) = crate::auto_tune::reuse_pinned(offer, &names)
        && let Some(&pf) = values.first()
        && pf.is_finite()
    {
        return palette_gate(pf, speed);
    }

    // Own-pass: a single Tier-1 feature (≤ ~14 ms at 4 MP per the P0 cost
    // grid; sub-ms at thumbnail sizes).
    if rgb.is_empty() || width == 0 || height == 0 {
        return PalettePreference::Auto;
    }
    let query = AnalysisQuery::new(FeatureSet::new().with(FEATURE));
    let analysis = zenanalyze::analyze_features_rgb8(rgb, width, height, &query);
    match analysis.get_f32(FEATURE) {
        Some(pf) if pf.is_finite() => palette_gate(pf, speed),
        _ => PalettePreference::Auto,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_thresholds_slow_tier() {
        // The rule is strict-greater at the fitted threshold; the slow tier
        // (speed <= 5) is byte-identical to the pre-speed-conditional rule.
        for speed in 1..=5u8 {
            assert_eq!(palette_gate(0.197, speed), PalettePreference::Auto);
            assert_eq!(palette_gate(0.198, speed), PalettePreference::Always);
            assert_eq!(palette_gate(0.0, speed), PalettePreference::Auto);
            assert_eq!(palette_gate(1.0, speed), PalettePreference::Always);
            // Measured corpus anchors (imazen26_features_2026-06-23.parquet):
            // screens keep patch_fraction >= 0.44 down to 256px; photos <= 0.008.
            assert_eq!(palette_gate(0.44, speed), PalettePreference::Always);
            assert_eq!(palette_gate(0.008, speed), PalettePreference::Auto);
        }
    }

    #[test]
    fn rule_thresholds_fast_tier() {
        // speed >= 6: strict-greater at 0.05.
        for speed in 6..=10u8 {
            assert_eq!(palette_gate(0.05, speed), PalettePreference::Auto);
            assert_eq!(palette_gate(0.051, speed), PalettePreference::Always);
            assert_eq!(palette_gate(0.0, speed), PalettePreference::Auto);
            // Photo p90 anchor (0.032) stays quiet even at the fast tier.
            assert_eq!(palette_gate(0.032, speed), PalettePreference::Auto);
        }
    }

    #[test]
    fn speed_conditional_band() {
        // The measured speed-conditional band (0.05, 0.197]: quiet at slow
        // speeds, fired at fast speeds — e.g. the 6600-class scans (pf
        // 0.060-0.072) whose forced palette wins only at s6.
        for pf in [0.06f32, 0.072, 0.113, 0.197] {
            assert_eq!(palette_gate(pf, 2), PalettePreference::Auto);
            assert_eq!(palette_gate(pf, 5), PalettePreference::Auto);
            assert_eq!(palette_gate(pf, 6), PalettePreference::Always);
            assert_eq!(palette_gate(pf, 8), PalettePreference::Always);
        }
        // Tier boundary is exactly at PALETTE_GATE_FAST_TIER_MIN_SPEED.
        assert_eq!(palette_gate_threshold(5), PALETTE_GATE_PATCH_FRACTION);
        assert_eq!(palette_gate_threshold(6), PALETTE_GATE_PATCH_FRACTION_FAST);
    }

    #[test]
    fn non_finite_degrades_to_auto() {
        // Any non-finite feature value is an analysis failure — Auto is the
        // safe reading (the encoder's own detection stays in charge).
        for speed in [2u8, 6] {
            assert_eq!(palette_gate(f32::NAN, speed), PalettePreference::Auto);
            assert_eq!(
                palette_gate(f32::NEG_INFINITY, speed),
                PalettePreference::Auto
            );
            assert_eq!(palette_gate(f32::INFINITY, speed), PalettePreference::Auto);
        }
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
                    if tile == 0 {
                        [255u8, 255, 255]
                    } else {
                        [0u8, 32, 128]
                    }
                })
            })
            .collect();
        for speed in [2u8, 6] {
            assert_eq!(
                palette_gate_for_rgb8(&screen, w, h, None, speed),
                PalettePreference::Always,
                "flat repeated tiles must fire the gate at every tier"
            );
        }

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
        for speed in [2u8, 6] {
            assert_eq!(
                palette_gate_for_rgb8(&photo, w, h, None, speed),
                PalettePreference::Auto,
                "aperiodic hash noise must not fire the gate at any tier"
            );
        }
    }

    /// Degrade path: empty/degenerate input recommends Auto.
    #[cfg(feature = "auto-tune")]
    #[test]
    fn degenerate_input_degrades_to_auto() {
        assert_eq!(
            palette_gate_for_rgb8(&[], 0, 0, None, 2),
            PalettePreference::Auto
        );
        assert_eq!(
            palette_gate_for_rgb8(&[], 0, 0, None, 6),
            PalettePreference::Auto
        );
    }

    /// A matching Offer is reused verbatim (no own-pass): supplying a fired
    /// patch_fraction via the offer flips the recommendation without pixels.
    #[cfg(feature = "auto-tune")]
    #[test]
    fn offer_reuse_short_circuits_analysis() {
        use zenanalyze::feature::AnalysisFeature;
        let owned = crate::auto_tune::test_offer(
            &[(AnalysisFeature::PatchFraction.name(), 0.9f32)],
            zenanalyze::analyzer_version(),
            0,
        );
        let cells: Vec<_> = owned
            .features()
            .iter()
            .map(zenanalyze_api::OwnedFeatureResult::as_ref)
            .collect();
        let offer = zenanalyze_api::Offer::new(&cells, owned.provenance());
        // Pixels say "photo" (flat gray = zero repetition? flat IS repetitive —
        // use empty pixels: with a valid offer the pixels are never touched).
        assert_eq!(
            palette_gate_for_rgb8(&[], 0, 0, Some(&offer), 2),
            PalettePreference::Always,
            "offered patch_fraction=0.9 must fire without an analysis pass"
        );
    }
}

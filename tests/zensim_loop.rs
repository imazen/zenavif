//! Contracts for the zensim-diffmap closed loop
//! (`encode_rgb8_zensim_loop`): it must converge on a requested zensim
//! score, report honestly what it could and could not apply, and never
//! claim the spatial hints reached the encoder when the release gate is
//! shut.

#![cfg(feature = "two-pass-zensim")]

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::Rgb;
use zenavif::{
    EncoderConfig, SPATIAL_HINTS_LIVE, ZensimLoopOptions, anchor_quality_for_zensim,
    encode_rgb8_zensim_loop,
};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// Deterministic pseudo-random byte without a rand dependency (same mixer
/// as `tests/target_quality.rs`).
fn mix(x: u32, y: u32, salt: u32) -> u8 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ salt;
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    (h >> 16) as u8
}

/// Structured content whose quality→score curve moves smoothly through the
/// mid range, with a deliberately NON-uniform error profile: the left half
/// is a smooth gradient (cheap, low error) and the right half is dense
/// texture (expensive, high error), so the diffmap has real spatial
/// structure for the pooling to find.
fn test_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let pixels = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let smooth = ((x * 200) / w.max(1)) as u8;
                if x * 2 < w {
                    Rgb {
                        r: smooth,
                        g: smooth.saturating_add(20),
                        b: 255 - smooth,
                    }
                } else {
                    let tex = mix(x as u32, y as u32, 11);
                    Rgb {
                        r: smooth.saturating_add(tex >> 1),
                        g: tex,
                        b: (255 - smooth).saturating_add(tex >> 2),
                    }
                }
            })
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn base_config() -> EncoderConfig {
    EncoderConfig::new().speed(8).threads(Some(1))
}

#[test]
fn converges_on_a_zensim_target() {
    let img = test_image(192, 192);
    let opts = ZensimLoopOptions {
        tolerance: 1.0,
        max_encodes: 8,
        ..Default::default()
    };
    let out = encode_rgb8_zensim_loop(img.as_ref(), &base_config(), 80.0, &opts, stop())
        .expect("zensim loop");
    assert!(
        out.converged,
        "should converge on zensim 80: score {:.2} at q {:.1} after {} encodes",
        out.score, out.quality, out.encodes
    );
    assert!((out.score - 80.0).abs() <= 1.0, "score {:.2}", out.score);
    assert!(!out.encoded.avif_file.is_empty());
    assert!((1..=8).contains(&out.encodes));
    // Pass-1 provenance must be filled in and consistent with the run.
    assert!(out.pass1_score.is_finite(), "pass1 score not recorded");
    assert!(out.pass1_quality > 0.0);
    if out.encodes == 1 {
        assert_eq!(out.quality, out.pass1_quality);
        assert!((out.score - out.pass1_score).abs() < 1e-12);
    }
}

#[test]
fn low_target_lands_no_worse_than_the_secant_baseline() {
    // The LOW-target band is the hard one, and not only because the
    // score-vs-quality curve is steepest there: quality maps to an INTEGER
    // AV1 quantizer index, so the achievable scores are a discrete lattice
    // and one step can move zensim by more than a 1.0 tolerance — measured
    // in `benchmarks/zensim_score_lattice_2026-08-06.tsv`: adjacent
    // achievable scores are 0.82–1.05 apart at the median and ~50% of the
    // gaps exceed 1.0. Asserting unconditional convergence there would be
    // asserting something the codec cannot deliver — so the contract is
    // comparative instead, which is also the actual claim of this loop: it
    // must get at least as close as the existing search, from the same
    // input, with the same budget.
    use zenavif::{TargetMetric, TargetOptions, encode_rgb8_with_target};
    let img = test_image(192, 192);
    let target = 35.0;
    let (tolerance, max_encodes) = (1.0, 8);

    let baseline = encode_rgb8_with_target(
        img.as_ref(),
        &base_config(),
        TargetMetric::Zensim(target),
        &TargetOptions {
            tolerance,
            max_encodes,
            ..Default::default()
        },
        stop(),
    )
    .expect("secant baseline");

    let out = encode_rgb8_zensim_loop(
        img.as_ref(),
        &base_config(),
        target,
        &ZensimLoopOptions {
            tolerance,
            max_encodes,
            ..Default::default()
        },
        stop(),
    )
    .expect("zensim loop");

    let (loop_err, base_err) = ((out.score - target).abs(), (baseline.score - target).abs());
    assert!(
        loop_err <= base_err + 1e-6,
        "loop landed further from {target} than the secant baseline: \
         loop {:.3} (q {:.2}, {} encodes) vs baseline {:.3} (q {:.2}, {} encodes)",
        out.score,
        out.quality,
        out.encodes,
        baseline.score,
        baseline.quality,
        baseline.encodes
    );
    // And it must never claim convergence it did not reach.
    assert_eq!(out.converged, loop_err <= tolerance);
}

#[test]
fn spatial_applied_never_overclaims() {
    // The single most important honesty property: `spatial_applied` is the
    // release gate, not a wish. While the gate is shut the loop must still
    // run and converge — it is a live global search — but must say the
    // per-superblock hints did NOT reach the encoder.
    let img = test_image(192, 192);
    let opts = ZensimLoopOptions {
        tolerance: 1.5,
        max_encodes: 6,
        ..Default::default()
    };
    let out = encode_rgb8_zensim_loop(img.as_ref(), &base_config(), 70.0, &opts, stop())
        .expect("zensim loop");
    assert_eq!(
        out.spatial_applied, SPATIAL_HINTS_LIVE,
        "spatial_applied must mirror the release gate exactly"
    );
    if !SPATIAL_HINTS_LIVE {
        assert!(
            !out.spatial_applied,
            "must not claim the gated-off hints were applied"
        );
    }
    assert!(!out.encoded.avif_file.is_empty(), "still produces an AVIF");
}

#[test]
fn spatial_map_is_computed_and_shaped_like_the_superblock_grid() {
    // Force more than one pass by asking for a tight tolerance from a
    // deliberately wrong seed, so a map is produced for pass 2.
    let (w, h) = (192usize, 128usize);
    let img = test_image(w, h);
    let opts = ZensimLoopOptions {
        tolerance: 0.25,
        max_encodes: 4,
        seed_quality: Some(20.0),
        // Explicit since 2026-08-06: the default is 0.0 now that the
        // channel is live and measured (see ZensimLoopOptions docs), so a
        // test about the map's SHAPE has to ask for a map.
        spatial_strength: 1.0,
        ..Default::default()
    };
    let out = encode_rgb8_zensim_loop(img.as_ref(), &base_config(), 85.0, &opts, stop())
        .expect("zensim loop");
    assert!(out.encodes >= 2, "seed 20 -> target 85 must need >1 pass");
    // The reported map must describe the RETURNED encode, so a run whose
    // selected encode is a later pass has to carry one.
    let map = out
        .sb_q_scale
        .expect("the returned encode was not pass 1, so it carries a map");
    assert_eq!(map.len(), w.div_ceil(64) * h.div_ceil(64), "3x2 SB grid");
    assert!(
        map.iter().all(|s| s.is_finite() && *s > 0.0),
        "every scale must be a positive finite multiplier"
    );
    // This image is half smooth / half textured, so the map must NOT be
    // uniformly neutral — otherwise the spatial signal is not being read.
    assert!(
        map.iter().any(|s| (s - 1.0).abs() > 1e-3),
        "expected spatial structure, got an all-neutral map: {map:?}"
    );
}

#[test]
fn spatial_strength_zero_disables_the_map_entirely() {
    let img = test_image(192, 128);
    let opts = ZensimLoopOptions {
        tolerance: 0.25,
        max_encodes: 4,
        seed_quality: Some(20.0),
        spatial_strength: 0.0,
        ..Default::default()
    };
    let out = encode_rgb8_zensim_loop(img.as_ref(), &base_config(), 85.0, &opts, stop())
        .expect("zensim loop");
    assert!(out.encodes >= 2);
    assert!(
        out.sb_q_scale.is_none(),
        "strength 0 must not compute a map at all"
    );
}

#[test]
fn seed_quality_override_is_the_first_pass() {
    let img = test_image(128, 128);
    let opts = ZensimLoopOptions {
        tolerance: 0.5,
        max_encodes: 1,
        seed_quality: Some(42.0),
        ..Default::default()
    };
    let out = encode_rgb8_zensim_loop(img.as_ref(), &base_config(), 90.0, &opts, stop())
        .expect("zensim loop");
    assert_eq!(out.encodes, 1);
    assert_eq!(out.pass1_quality, 42.0);
    assert_eq!(out.quality, 42.0);
    assert!(
        out.sb_q_scale.is_none(),
        "pass 1 encodes with no map (nothing had been measured yet), so the \
         reported map -- which describes the RETURNED encode -- must be None"
    );
}

#[test]
fn unreachable_target_reports_not_converged() {
    let img = test_image(128, 128);
    // Capped at quality 15 and asked for near-lossless: structurally
    // unreachable. The loop must say so rather than pretend.
    let opts = ZensimLoopOptions {
        tolerance: 0.5,
        max_encodes: 5,
        min_quality: 1.0,
        max_quality: 15.0,
        ..Default::default()
    };
    let out = encode_rgb8_zensim_loop(img.as_ref(), &base_config(), 97.0, &opts, stop())
        .expect("zensim loop");
    assert!(
        !out.converged,
        "q<=15 cannot reach zensim 97 (got {:.2})",
        out.score
    );
    assert!(out.quality <= 15.0);
    assert!(
        !out.encoded.avif_file.is_empty(),
        "still returns a valid AVIF"
    );
}

#[test]
fn empty_search_range_is_rejected() {
    let img = test_image(64, 64);
    let opts = ZensimLoopOptions {
        min_quality: 80.0,
        max_quality: 80.0,
        ..Default::default()
    };
    let err = encode_rgb8_zensim_loop(img.as_ref(), &base_config(), 70.0, &opts, stop())
        .expect_err("an empty range must be an error, not a silent single probe");
    let msg = format!("{err:?}");
    assert!(msg.contains("range"), "unexpected error: {msg}");
}

#[test]
fn handles_an_image_smaller_than_one_superblock() {
    // 40x24 is a sub-superblock, sub-pyramid-minimum frame: the pooling grid
    // is 1x1 and zensim reflect-pads internally. Nothing may panic.
    let img = test_image(40, 24);
    let opts = ZensimLoopOptions {
        tolerance: 2.0,
        max_encodes: 4,
        ..Default::default()
    };
    let out = encode_rgb8_zensim_loop(img.as_ref(), &base_config(), 75.0, &opts, stop())
        .expect("zensim loop on a tiny image");
    assert!(!out.encoded.avif_file.is_empty());
    if let Some(map) = out.sb_q_scale {
        assert_eq!(map.len(), 1, "one superblock covers the whole frame");
    }
}

#[test]
fn cancellation_is_honored() {
    use almost_enough::{Stop, Stopper};
    let img = test_image(192, 192);
    let stopper = Stopper::cancelled();
    let token = StopToken::new(stopper);
    assert!(token.check().is_err(), "token must already be fired");
    let opts = ZensimLoopOptions::default();
    let err = encode_rgb8_zensim_loop(img.as_ref(), &base_config(), 80.0, &opts, token)
        .expect_err("a fired stop token must abort the loop");
    let msg = format!("{err:?}");
    assert!(
        msg.to_lowercase().contains("cancel") || msg.to_lowercase().contains("stop"),
        "expected a cancellation error, got: {msg}"
    );
}

#[test]
fn the_anchor_curve_seeds_pass_one_in_the_right_direction() {
    // A higher target must never seed a lower quality — the seed is the
    // whole reason a one- or two-encode convergence is possible.
    let mut prev = 0.0f32;
    for t in [20.0f64, 35.0, 50.0, 65.0, 80.0, 90.0, 95.0] {
        let q = anchor_quality_for_zensim(t);
        assert!(
            q >= prev,
            "anchor went backwards at target {t}: {prev} -> {q}"
        );
        prev = q;
    }
}

// ===========================================================================
// Two-shot: precision at a FIXED budget of two encodes.
// ===========================================================================

use zenavif::{
    LatticePolicy, TwoShotOptions, anchor_quantizer_for_zensim, anchor_zensim_for_quantizer,
    encode_rgb8_zensim_two_shot, quality_for_quantizer,
};

#[test]
fn two_shot_spends_at_most_two_encodes_and_returns_the_last_one() {
    let img = test_image(192, 192);
    for target in [35.0f64, 60.0, 80.0] {
        let out = encode_rgb8_zensim_two_shot(
            img.as_ref(),
            &base_config(),
            target,
            &TwoShotOptions::default(),
            stop(),
        )
        .expect("two-shot");
        assert!(
            out.encodes <= 2,
            "budget is two encodes, spent {} at target {target}",
            out.encodes
        );
        assert!(!out.encoded.avif_file.is_empty());
        // The returned encode is the LAST one: its quality and quantizer
        // agree, and when two encodes ran it is not pass 1's.
        assert_eq!(
            zenavif::EncoderConfig::new()
                .quality(out.quality)
                .resolve_plan(zenavif::PlanInput::rgb8(192, 192))
                .quantizer,
            out.quantizer,
            "reported quality and quantizer must describe the same encode"
        );
        if out.encodes == 2 {
            assert_ne!(
                out.quantizer, out.pass1_quantizer,
                "a second encode at the same quantizer would be a duplicate"
            );
        } else {
            assert_eq!(out.quantizer, out.pass1_quantizer);
            assert_eq!(out.score, out.pass1_score);
        }
    }
}

#[test]
fn two_shot_lands_closer_than_its_own_seed() {
    // The whole point of pass 2: on content whose curve is offset from the
    // population's, the correction must actually help. Averaged over a
    // spread of targets, two-shot error must beat the open-loop seed.
    let img = test_image(192, 192);
    let (mut seed_err, mut final_err) = (0.0f64, 0.0f64);
    let targets = [30.0f64, 40.0, 50.0, 60.0, 70.0, 80.0, 88.0];
    for &t in &targets {
        let out = encode_rgb8_zensim_two_shot(
            img.as_ref(),
            &base_config(),
            t,
            &TwoShotOptions::default(),
            stop(),
        )
        .expect("two-shot");
        seed_err += (out.pass1_score - t).abs();
        final_err += (out.score - t).abs();
    }
    let n = targets.len() as f64;
    assert!(
        final_err < seed_err,
        "pass 2 must improve on the seed: mean |err| {:.3} -> {:.3}",
        seed_err / n,
        final_err / n
    );
}

#[test]
fn two_shot_lattice_policy_picks_the_requested_side() {
    // AtLeast must never choose a coarser quantizer than Nearest, and
    // AtMost never a finer one — that is what "prefer the side above /
    // below the target" means once quantizer order is accounted for.
    let img = test_image(192, 192);
    let mk = |policy| TwoShotOptions {
        policy,
        ..Default::default()
    };
    for target in [45.0f64, 65.0, 82.0] {
        let n = encode_rgb8_zensim_two_shot(
            img.as_ref(),
            &base_config(),
            target,
            &mk(LatticePolicy::Nearest),
            stop(),
        )
        .expect("nearest");
        let a = encode_rgb8_zensim_two_shot(
            img.as_ref(),
            &base_config(),
            target,
            &mk(LatticePolicy::AtLeast),
            stop(),
        )
        .expect("at least");
        let b = encode_rgb8_zensim_two_shot(
            img.as_ref(),
            &base_config(),
            target,
            &mk(LatticePolicy::AtMost),
            stop(),
        )
        .expect("at most");
        assert!(
            a.quantizer <= n.quantizer && n.quantizer <= b.quantizer,
            "target {target}: at_least {} nearest {} at_most {}",
            a.quantizer,
            n.quantizer,
            b.quantizer
        );
        // A finer quantizer is never a smaller file.
        assert!(a.encoded.avif_file.len() >= b.encoded.avif_file.len());
    }
}

#[test]
fn two_shot_never_claims_spatial_hints_it_did_not_apply() {
    let img = test_image(192, 192);
    let out = encode_rgb8_zensim_two_shot(
        img.as_ref(),
        &base_config(),
        70.0,
        &TwoShotOptions {
            spatial_strength: 1.0,
            ..Default::default()
        },
        stop(),
    )
    .expect("two-shot");
    assert_eq!(
        out.spatial_applied, SPATIAL_HINTS_LIVE,
        "spatial_applied must mirror the release gate, never the intent"
    );
    if out.encodes == 2 {
        let map = out.sb_q_scale.expect("a nonzero strength computes a map");
        assert!(map.iter().all(|v| v.is_finite() && *v > 0.0));
    }
    // The default must not compute or claim a map at all.
    let plain = encode_rgb8_zensim_two_shot(
        img.as_ref(),
        &base_config(),
        70.0,
        &TwoShotOptions::default(),
        stop(),
    )
    .expect("two-shot");
    assert!(plain.sb_q_scale.is_none());
}

#[test]
fn two_shot_respects_the_quality_range_and_rejects_an_empty_one() {
    let img = test_image(128, 128);
    let out = encode_rgb8_zensim_two_shot(
        img.as_ref(),
        &base_config(),
        97.0,
        &TwoShotOptions {
            min_quality: 1.0,
            max_quality: 20.0,
            ..Default::default()
        },
        stop(),
    )
    .expect("two-shot");
    assert!(out.quality <= 20.0, "quality {} escaped the range", out.quality);
    assert!(!out.within_tolerance, "zensim 97 is unreachable at q<=20");
    assert!(!out.encoded.avif_file.is_empty());

    let err = encode_rgb8_zensim_two_shot(
        img.as_ref(),
        &base_config(),
        70.0,
        &TwoShotOptions {
            min_quality: 80.0,
            max_quality: 80.0,
            ..Default::default()
        },
        stop(),
    )
    .expect_err("an empty range must be an error");
    assert!(format!("{err:?}").contains("range"));
}

#[test]
fn two_shot_addresses_the_full_quantizer_lattice() {
    // Pass 2 picks a QUANTIZER, so every integer quantizer it can pick has
    // to be addressable through the quality knob. This is the property the
    // whole precision argument rests on.
    for qi in [0u8, 1, 26, 71, 107, 150, 206, 254, 255] {
        let q = quality_for_quantizer(qi);
        assert_eq!(
            zenavif::EncoderConfig::new()
                .quality(q)
                .resolve_plan(zenavif::PlanInput::rgb8(64, 64))
                .quantizer,
            qi
        );
    }
    // And the anchor pair used to place pass 2 is a consistent bijection.
    for t in [25.0f64, 45.0, 65.0, 85.0] {
        let qi = anchor_quantizer_for_zensim(t);
        assert!((anchor_zensim_for_quantizer(qi) - t).abs() < 0.05);
    }
}

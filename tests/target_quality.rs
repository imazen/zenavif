//! Contracts for the `target-quality` convergence loop
//! (`encode_rgb8_with_target`): it must actually hit the requested
//! SSIMULACRA2 / zensim score on content where the score-vs-quality curve
//! covers the target, and must say so honestly when it cannot.

#![cfg(feature = "target-quality")]

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::Rgb;
use zenavif::{EncoderConfig, TargetMetric, TargetOptions, encode_rgb8_with_target};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// Deterministic pseudo-random byte without a rand dependency.
fn mix(x: u32, y: u32, salt: u32) -> u8 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ salt;
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    (h >> 16) as u8
}

/// Structured content (soft radial gradient + edges + mild texture) — the
/// quality→score curve must move smoothly through the mid range, which pure
/// noise or a flat fill would not give.
fn test_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let pixels = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let dx = x as f32 - w as f32 / 2.0;
                let dy = y as f32 - h as f32 / 2.0;
                let r = (dx * dx + dy * dy).sqrt() / (w as f32 / 2.0);
                let base = (200.0 - 120.0 * r.min(1.0)) as u8;
                let stripe = if (x / 12) % 2 == 0 { 20 } else { 0 };
                let tex = mix(x as u32, y as u32, 7) >> 4; // 0..15 texture
                Rgb {
                    r: base.saturating_add(stripe).saturating_add(tex),
                    g: base.saturating_add(tex),
                    b: (255 - base).saturating_add(stripe),
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
fn ssim2_target_converges_within_tolerance() {
    let img = test_image(192, 192);
    let opts = TargetOptions {
        tolerance: 1.0,
        max_encodes: 8,
        ..Default::default()
    };
    let out = encode_rgb8_with_target(
        img.as_ref(),
        &base_config(),
        TargetMetric::Ssim2(80.0),
        &opts,
        stop(),
    )
    .expect("targeted encode");
    assert!(
        out.converged,
        "should converge on ssim2 80: got score {:.2} at q {:.1} after {} encodes",
        out.score, out.quality, out.encodes
    );
    assert!(
        (out.score - 80.0).abs() <= 1.0,
        "score {:.2} not within tolerance",
        out.score
    );
    assert!(!out.encoded.avif_file.is_empty());
    assert!(out.encodes >= 1 && out.encodes <= 8);
}

#[test]
fn zensim_target_converges_within_tolerance() {
    let img = test_image(192, 192);
    let opts = TargetOptions {
        tolerance: 1.0,
        max_encodes: 8,
        ..Default::default()
    };
    let out = encode_rgb8_with_target(
        img.as_ref(),
        &base_config(),
        TargetMetric::Zensim(85.0),
        &opts,
        stop(),
    )
    .expect("targeted encode");
    assert!(
        out.converged,
        "should converge on zensim 85: got score {:.2} at q {:.1} after {} encodes",
        out.score, out.quality, out.encodes
    );
    assert!(
        (out.score - 85.0).abs() <= 1.0,
        "score {:.2} not within tolerance",
        out.score
    );
}

#[test]
fn unreachable_target_reports_not_converged() {
    let img = test_image(128, 128);
    // Cap the search at quality 15 and ask for near-lossless: structurally
    // unreachable, and the search must say so instead of pretending.
    let opts = TargetOptions {
        tolerance: 0.5,
        max_encodes: 5,
        min_quality: 1.0,
        max_quality: 15.0,
    };
    let out = encode_rgb8_with_target(
        img.as_ref(),
        &base_config(),
        TargetMetric::Ssim2(92.0),
        &opts,
        stop(),
    )
    .expect("targeted encode");
    assert!(
        !out.converged,
        "q<=15 cannot reach ssim2 92 (got {:.2})",
        out.score
    );
    assert!(out.quality <= 15.0);
    assert!(out.score < 91.5);
    assert!(!out.encoded.avif_file.is_empty());
}

#[test]
fn converged_result_reaches_target_band_with_smallest_file() {
    // The selection policy promises: among iterates at/above target−tol,
    // the returned encode is the lowest-quality (smallest) one. Verify the
    // returned score is inside the acceptable band, not far above it.
    let img = test_image(192, 192);
    let opts = TargetOptions {
        tolerance: 1.5,
        max_encodes: 8,
        ..Default::default()
    };
    let out = encode_rgb8_with_target(
        img.as_ref(),
        &base_config(),
        TargetMetric::Ssim2(70.0),
        &opts,
        stop(),
    )
    .expect("targeted encode");
    assert!(out.converged);
    assert!(
        out.score >= 70.0 - 1.5 && out.score <= 70.0 + 1.5,
        "score {:.2} escaped the target band",
        out.score
    );
}

#[test]
fn rgba_target_converges_and_respects_alpha_scoring() {
    use rgb::Rgba;
    // Textured color + a real alpha gradient (mirrors encode_contracts.rs's
    // alpha-image rationale: the alpha plane must carry signal).
    let (w, h) = (160usize, 160usize);
    let pixels: Vec<Rgba<u8>> = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let base = ((x * 255) / w.max(1)) as u8;
                Rgba {
                    r: base,
                    g: 255 - base,
                    b: (mix(x as u32, y as u32, 5) >> 2).saturating_add(100),
                    a: (((x + y) * 255) / (w + h - 2)) as u8,
                }
            })
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, w, h);
    let opts = TargetOptions {
        tolerance: 1.5,
        max_encodes: 8,
        ..Default::default()
    };
    let out = zenavif::encode_rgba8_with_target(
        img.as_ref(),
        &base_config(),
        TargetMetric::Ssim2(78.0),
        &opts,
        stop(),
    )
    .expect("targeted rgba encode");
    assert!(
        out.converged,
        "rgba ssim2 78: got {:.2} at q {:.1} after {} encodes",
        out.score, out.quality, out.encodes
    );
    assert!(!out.encoded.avif_file.is_empty());
    // The encode really carried an alpha payload.
    assert!(out.encoded.alpha_byte_size > 0, "alpha plane missing");
}

#[test]
fn rgb16_target_converges() {
    // 16-bit version of the structured test image (10-bit AV1 encode path).
    let (w, h) = (160usize, 160usize);
    let pixels: Vec<Rgb<u16>> = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let dx = x as f32 - w as f32 / 2.0;
                let dy = y as f32 - h as f32 / 2.0;
                let r = (dx * dx + dy * dy).sqrt() / (w as f32 / 2.0);
                let base = ((200.0 - 120.0 * r.min(1.0)) * 257.0) as u16;
                let tex = u16::from(mix(x as u32, y as u32, 7)) << 4;
                Rgb {
                    r: base.saturating_add(tex),
                    g: base,
                    b: 65535 - base,
                }
            })
        })
        .collect();
    let img = imgref::ImgVec::new(pixels, w, h);
    let opts = TargetOptions {
        tolerance: 1.5,
        max_encodes: 8,
        ..Default::default()
    };
    let out = zenavif::encode_rgb16_with_target(
        img.as_ref(),
        &base_config(),
        TargetMetric::Ssim2(80.0),
        &opts,
        stop(),
    )
    .expect("targeted rgb16 encode");
    assert!(
        out.converged,
        "rgb16 ssim2 80: got {:.2} at q {:.1} after {} encodes",
        out.score, out.quality, out.encodes
    );
    assert!(!out.encoded.avif_file.is_empty());
}

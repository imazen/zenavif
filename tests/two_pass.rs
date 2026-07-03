//! Contract tests for the butteraugli-diffmap-guided second pass.
//!
//! The suite adapts to the compile-time release-gate state
//! (`zenravif::FRAME_HINTS_LIVE`) — both branches assert real behavior of
//! the build under test, so nothing silently skips:
//! - passthrough LIVE (dev-patched zenrav1e / post-dep-bump): the two-pass
//!   drive must succeed, apply a non-neutral map, and produce a decodable
//!   pass-2 AVIF whose bytes differ from the single-pass encode.
//! - passthrough GATED (registry zenrav1e ≤ 0.1.4): the driver must fail
//!   honestly instead of silently double-encoding.
#![cfg(feature = "two-pass-butteraugli")]

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::Rgb;
use zenavif::{EncoderConfig, FRAME_HINTS_LIVE, TwoPassOptions, encode_rgb8_two_pass};

/// Synthetic photo-ish content: smooth gradient + a textured quadrant +
/// a hard-edged box, so superblocks genuinely differ in butteraugli-vs-MSE
/// behavior at moderate quality.
fn test_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let gx = (x * 255 / w) as u8;
            let gy = (y * 255 / h) as u8;
            let mut p = Rgb {
                r: gx,
                g: gy,
                b: 128u8,
            };
            if x < w / 2 && y >= h / 2 {
                // Texture: deterministic pseudo-noise.
                let n = ((x * 7 + y * 13) ^ (x * y)) as u8;
                p = Rgb {
                    r: n,
                    g: n.wrapping_add(40),
                    b: n.wrapping_mul(3),
                };
            }
            if x > w * 3 / 4 && y < h / 4 {
                p = Rgb {
                    r: 240,
                    g: 20,
                    b: 20,
                };
            }
            px.push(p);
        }
    }
    ImgVec::new(px, w, h)
}

#[test]
fn two_pass_contract_matches_gate_state() {
    let img = test_image(256, 192); // 4×3 superblocks
    let config = EncoderConfig::new().quality(55.0).speed(6);
    let stop = StopToken::new(Unstoppable);

    let result = encode_rgb8_two_pass(
        img.as_ref(),
        &config,
        &TwoPassOptions::default(),
        stop.clone(),
    );

    if FRAME_HINTS_LIVE {
        let two = result.expect("two-pass encode must succeed when the passthrough is live");
        assert!(!two.encode.avif_file.is_empty());
        assert_eq!(two.sb_q_scale.len(), 4 * 3);
        assert!(
            two.sb_q_scale.iter().any(|&s| (s - 1.0).abs() > 1e-3),
            "a mixed-content image must produce a non-neutral map: {:?}",
            two.sb_q_scale
        );
        assert!(two.pass1_bytes > 0);
        assert!(two.pass1_butteraugli_max > 0.0);
        assert!(two.pass1_butteraugli_3n > 0.0);

        // The map must actually steer the encoder: pass 2 differs from a
        // plain single-pass encode at the same config.
        let single =
            zenavif::encode_rgb8(img.as_ref(), &config, stop.clone()).expect("single-pass encode");
        assert_ne!(
            single.avif_file, two.encode.avif_file,
            "pass 2 must differ from the single-pass encode"
        );

        // And the pass-2 file must decode cleanly with our own decoder.
        let decoded = zenavif::decode(&two.encode.avif_file).expect("pass-2 decode");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (256, 192),
            "pass-2 dimensions"
        );
    } else {
        // Release-gated: the driver must refuse honestly.
        let err = result.expect_err(
            "two-pass must fail honestly while the zenravif passthrough is release-gated",
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("FRAME_HINTS_LIVE"),
            "error must name the release gate: {msg}"
        );
    }
}

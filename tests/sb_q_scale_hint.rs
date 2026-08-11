//! Contract tests for the public `EncoderConfig::with_sb_q_scale` hint hook
//! (the external-closed-loop entry the `zensim_cq_rd` harness steers with —
//! `benchmarks/zensim_avif_loop_2026-08-07.md`).
//!
//! Like `tests/two_pass.rs`, the suite adapts to the compile-time
//! release-gate state (`zenavif::FRAME_HINTS_LIVE`) — both branches assert
//! real behavior of the build under test, so nothing silently skips:
//! - passthrough LIVE (dev-patched zenrav1e / post-dep-bump): a strongly
//!   non-neutral map must change the emitted bitstream.
//! - passthrough GATED (registry zenrav1e ≤ 0.1.4): supplied maps are
//!   accepted but inert — encodes stay byte-identical — which is exactly
//!   the honest-refusal contract closed-loop callers probe against.
#![cfg(all(feature = "encode-imazen", feature = "two-pass-butteraugli"))]

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::Rgb;
use zenavif::{EncoderConfig, FRAME_HINTS_LIVE, encode_rgb8};

/// Mixed content (gradient + texture + hard edge) so superblocks would
/// genuinely respond differently to per-SB quantizer scaling when live.
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
                let n = ((x * 7 + y * 13) ^ (x * y)) as u8;
                p = Rgb {
                    r: n,
                    g: n.wrapping_add(40),
                    b: n.wrapping_mul(3),
                };
            }
            if x >= w / 2 && y >= h / 2 && (x / 8 + y / 8) % 2 == 0 {
                p = Rgb {
                    r: 255,
                    g: 255,
                    b: 255,
                };
            }
            px.push(p);
        }
    }
    ImgVec::new(px, w, h)
}

fn config() -> EncoderConfig {
    EncoderConfig::new()
        .quality(60.0)
        .speed(6)
        .bit_depth(zenavif::EncodeBitDepth::Eight)
        .threads(Some(1))
}

fn encode(img: imgref::ImgRef<'_, Rgb<u8>>, cfg: &EncoderConfig) -> Vec<u8> {
    encode_rgb8(img, cfg, StopToken::new(Unstoppable))
        .expect("encode")
        .avif_file
}

#[test]
fn builder_stores_and_exposes_the_map() {
    let map: Box<[f32]> = vec![1.0f32; 4].into_boxed_slice();
    let cfg = config().with_sb_q_scale(Some(map.clone()));
    assert_eq!(cfg.sb_q_scale_value(), Some(&map[..]));
    let cfg = cfg.with_sb_q_scale(None);
    assert_eq!(cfg.sb_q_scale_value(), None);
}

#[test]
fn hint_map_engagement_matches_the_release_gate() {
    // 128×96 = 2×2 superblocks.
    let img = test_image(128, 96);
    let base = config();

    // Determinism first: without it the byte-equality assertions below
    // are unsound.
    let a1 = encode(img.as_ref(), &base);
    let a2 = encode(img.as_ref(), &base);
    assert_eq!(a1, a2, "single-threaded encode must be deterministic");

    let neutral = base
        .clone()
        .with_sb_q_scale(Some(vec![1.0f32; 4].into_boxed_slice()));
    let steered = base
        .clone()
        .with_sb_q_scale(Some(vec![0.5f32, 2.0, 2.0, 0.5].into_boxed_slice()));
    let b = encode(img.as_ref(), &neutral);
    let c = encode(img.as_ref(), &steered);

    if FRAME_HINTS_LIVE {
        // Live passthrough: a strongly non-neutral map must move the
        // bitstream (the closed-loop engagement contract).
        assert_ne!(
            a1, c,
            "FRAME_HINTS_LIVE but a 0.5/2.0 map left the bitstream \
             byte-identical — the passthrough is a silent no-op"
        );
    } else {
        // Gated: maps are accepted but inert; encodes stay byte-identical.
        // This is the state closed-loop callers must detect and refuse on.
        assert_eq!(
            a1, b,
            "gated build: neutral map changed the bitstream — the gate is \
             not inert as documented"
        );
        assert_eq!(
            a1, c,
            "gated build: non-neutral map changed the bitstream — \
             FRAME_HINTS_LIVE is stale (flip it or fix the gate)"
        );
    }
}

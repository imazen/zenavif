//! Regression observer for zenavif's gray-as-RGB encode path
//! (imazen/zenavif#6): `encode_gray8` (the zencodec adapter's
//! `do_encode_gray8`) expands Gray → RGB{g,g,g} and encodes color.
//!
//! Measured 2026-06-11 (benchmarks/mono_encode_ab_2026-06-11.txt):
//! the expansion manufactures ZERO channel divergence (RGB→YCbCr of
//! R=G=B is exactly neutral chroma, and AV1 codes flat chroma at ~no
//! bitrate cost — settings-matched Cs400 vs Cs444 is within ±1% at
//! ≥512²). The real cost of the color path is encode TIME (chroma RDO
//! runs even when its bits are ~0: 2-3× slower than Cs400) — that is
//! what a true monochrome encode in zenravif buys.
//!
//! This example pins the no-divergence contract and tracks the size.
//!
//! ```sh
//! cargo run --release --features encode-imazen --example mono_encode_ab
//! ```

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::Rgb;
use zenavif::{EncodeChromaSubsampling, EncoderConfig};

/// Identical to make_mono_avif's gray_pattern — keep in sync.
fn gray_pattern(w: usize, h: usize) -> Vec<u8> {
    (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let g = (x * 255) / w.max(1);
                let t = ((x as f32 * 0.7).sin() * 20.0 + (y as f32 * 0.9).cos() * 20.0) as i32;
                (g as i32 + t).clamp(0, 255) as u8
            })
        })
        .collect()
}

fn main() {
    for (w, h) in [(96usize, 64usize), (512, 512), (1024, 1024), (2048, 2048)] {
        // The same Gray → RGB replication do_encode_gray8 performs.
        let rgb: Vec<Rgb<u8>> = gray_pattern(w, h)
            .into_iter()
            .map(|g| Rgb { r: g, g, b: g })
            .collect();
        let img = ImgVec::new(rgb, w, h);
        for (label, sub) in [
            ("444", EncodeChromaSubsampling::Yuv444),
            ("420", EncodeChromaSubsampling::Yuv420),
        ] {
            // quality 83.2 → quantizer 60 (ravif quality_to_quantizer
            // curve: (1 - 0.832) * 1.4 * 255 = 60).
            let cfg = EncoderConfig::new()
                .quality(83.2)
                .speed(6)
                .threads(Some(1))
                .chroma_subsampling(sub);
            let out = zenavif::encode_rgb8(img.as_ref(), &cfg, StopToken::new(Unstoppable))
                .expect("encode_rgb8");
            // Channel divergence in the decoded output = chroma wobble the
            // color path manufactured from perfectly gray input.
            let dec = zenavif::decode_with(
                &out.avif_file,
                &zenavif::DecoderConfig::new().prefer_8bit(true),
                &Unstoppable,
            )
            .expect("decode");
            let view = dec.try_as_imgref::<Rgb<u8>>().expect("rgb8");
            let mut max_spread = 0u8;
            let mut spread_px = 0usize;
            for row in view.rows() {
                for p in row {
                    let mx = p.r.max(p.g).max(p.b);
                    let mn = p.r.min(p.g).min(p.b);
                    let s = mx - mn;
                    if s > 0 {
                        spread_px += 1;
                    }
                    max_spread = max_spread.max(s);
                }
            }
            println!(
                "{w}x{h} gray-as-RGB ({label}): {} bytes | decoded channel spread: max {} on {:.1}% of pixels",
                out.avif_file.len(),
                max_spread,
                spread_px as f64 / (w * h) as f64 * 100.0
            );
        }
    }
}

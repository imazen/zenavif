//! Encode wall time, bytes and round-trip PSNR for `Av1Backend::Zenav1Aom`,
//! interleaved against the zenravif backend on identical input.
//!
//! `src/encoder_aom.rs` shipped saying "encode speed is unmeasured from this
//! seam" rather than quoting a number nobody had run. This is the harness that
//! runs it. It prints a TSV to stdout; commit the output under `benchmarks/`
//! with a `.meta` naming the commit, host and grid.
//!
//! ```text
//! cargo run --release --example aom_backend_bench \
//!   --features zenav1-aom-encode,encode > benchmarks/aom_backend_<date>.tsv
//! ```
//!
//! # Grid
//!
//! Four sizes spanning tiny to large (64², 256², 512², 1024²) so the fixed
//! per-call cost can be separated from the per-pixel cost — a "ms/MP" figure
//! without an intercept is meaningless at the small end. Six qualities
//! weighted toward the low-q range where web encoding actually lives
//! (10/25/40/60/80/95, not a high-quality-only pair). Three speeds spanning
//! the whole `--cpu-used` map (1 → 0, 5 → 4, 9 → 8).
//!
//! Both backends encode the SAME buffer in the same process, alternating, so
//! machine state is shared rather than measured separately.
//!
//! One encode per cell: this reports wall time to an order of magnitude, not a
//! regression-gate delta. Use zenbench for anything that has to detect a small
//! change.

use imgref::{Img, ImgRef, ImgVec};
use rgb::Rgb;
use std::time::Instant;
use zenavif::{Av1Backend, EncodeChromaSubsampling, EncodedImage, EncoderConfig};

fn gradient(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let mut st = 0x2545F491u32;
    let mut lcg = move || {
        st = st.wrapping_mul(1664525).wrapping_add(1013904223);
        (st >> 24) as u8
    };
    let mut b = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let g = ((x * 255) / w) as u8;
            let bl = ((y * 255) / h) as u8;
            let n = lcg() / 8;
            // Noise in luma only; per-channel noise would grade 4:2:0
            // subsampling instead of the encoder (see tests/aom_encode_backend.rs).
            let base = ((g as u16 + bl as u16) / 2) as u8;
            b.push(Rgb {
                r: g.saturating_add(n / 2),
                g: base.saturating_add(n / 2),
                b: bl.saturating_add(n / 2),
            });
        }
    }
    Img::new(b, w, h)
}

fn psnr(a: &[Rgb<u8>], b: &[Rgb<u8>]) -> f64 {
    let mut se = 0u64;
    for (p, q) in a.iter().zip(b) {
        for (x, y) in [(p.r, q.r), (p.g, q.g), (p.b, q.b)] {
            let d = i32::from(x) - i32::from(y);
            se += (d * d) as u64;
        }
    }
    if se == 0 {
        return f64::INFINITY;
    }
    10.0 * (255.0 * 255.0 / (se as f64 / (a.len() * 3) as f64)).log10()
}

fn run(img: ImgRef<'_, Rgb<u8>>, cfg: &EncoderConfig) -> (f64, EncodedImage) {
    let stop = almost_enough::StopToken::new(zenavif::Unstoppable);
    let t = Instant::now();
    let e = zenavif::encode_rgb8(img, cfg, stop).expect("encode");
    (t.elapsed().as_secs_f64() * 1000.0, e)
}

fn main() {
    println!("backend\twidth\theight\tquality\tspeed\tms\tpayload_bytes\tfile_bytes\tpsnr_db");
    for &(w, h) in &[(64usize, 64usize), (256, 256), (512, 512), (1024, 1024)] {
        let img = gradient(w, h);
        for &q in &[10.0f32, 25.0, 40.0, 60.0, 80.0, 95.0] {
            for &sp in &[1u8, 5, 9] {
                for backend in [Av1Backend::Zenav1Aom, Av1Backend::Zenravif] {
                    let cfg = EncoderConfig::new()
                        .backend(backend)
                        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
                        .quality(q)
                        .speed(sp);
                    let (ms, enc) = run(img.as_ref(), &cfg);
                    let dec = zenavif::decode(&enc.avif_file).expect("decode");
                    let out = dec.try_as_imgref::<Rgb<u8>>().expect("rgb8");
                    let p = psnr(img.buf(), out.buf());
                    let name = if backend == Av1Backend::Zenav1Aom {
                        "zenav1-aom"
                    } else {
                        "zenravif"
                    };
                    println!(
                        "{name}\t{w}\t{h}\t{q}\t{sp}\t{ms:.1}\t{}\t{}\t{p:.2}",
                        enc.color_byte_size,
                        enc.avif_file.len()
                    );
                }
            }
        }
        eprintln!("done {w}x{h}");
    }
}

//! The 8-bit byte-identity emitter: 60 cells, each printed as
//! `class \t WxH \t qN \t sN \t len \t fnv1a-64`.
//!
//! Run it once against a `git archive` of the base rev and once against the
//! working tree, and diff the two TSVs. That is what turns "8-bit output is
//! unchanged" from an argument about code paths into a measurement. Results:
//! `benchmarks/aom_bd8_identity_2026-09-03.{tsv,meta}`.
//!
//!   # base: extract the rev, point a copy of this crate's Cargo.toml at it
//!   git archive <base-rev> | tar -x -C ~/tmp/zenavif-base
//!   # head:
//!   CARGO_TARGET_DIR=../../target cargo run --release --bin bd8_anchor > head.tsv
//!
//! Quality 100 is deliberately absent from the grid: `--cq-level 0` panics
//! upstream on flat content (zenavif#45).
use imgref::{Img, ImgVec};
use rgb::Rgb;
use zenavif::{Av1Backend, EncodeChromaSubsampling, EncoderConfig};
fn stop() -> almost_enough::StopToken { almost_enough::StopToken::new(almost_enough::Unstoppable) }
fn gradient_rgb8(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let mut state = 0x2545F491u32;
    let mut lcg = move || { state = state.wrapping_mul(1664525).wrapping_add(1013904223); (state >> 24) as u8 };
    let mut buf = Vec::with_capacity(w * h);
    for y in 0..h { for x in 0..w {
        let g = ((x * 255) / w.max(1)) as u8;
        let b = ((y * 255) / h.max(1)) as u8;
        let n = lcg() / 8;
        let base = ((g as u16 + b as u16) / 2) as u8;
        buf.push(Rgb { r: g.saturating_add(n / 2), g: base.saturating_add(n / 2), b: b.saturating_add(n / 2) });
    }}
    Img::new(buf, w, h)
}
fn flat(w: usize, h: usize, v: u8) -> ImgVec<Rgb<u8>> { Img::new(vec![Rgb{r:v,g:v,b:v}; w*h], w, h) }
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes { h ^= u64::from(*b); h = h.wrapping_mul(0x1000_0000_01b3); }
    h
}
fn main() {
    let cfg = |q: f32, s: u8| EncoderConfig::new()
        .backend(Av1Backend::Zenav1Aom)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
        .quality(q).speed(s);
    for &(w, h) in &[(64usize,64usize),(33,47),(128,96),(192,64),(65,33),(256,256)] {
        for &(q, s) in &[(90.0f32, 6u8), (80.0, 1), (80.0, 10), (20.0, 6), (99.0, 4)] {
            let g = gradient_rgb8(w, h);
            let e = zenavif::encode_rgb8(g.as_ref(), &cfg(q, s), stop()).expect("enc");
            println!("grad\t{w}x{h}\tq{q}\ts{s}\t{}\t{:#018x}", e.avif_file.len(), fnv1a(&e.avif_file));
            let f = flat(w, h, 235);
            let e = zenavif::encode_rgb8(f.as_ref(), &cfg(q, s), stop()).expect("enc");
            println!("flat\t{w}x{h}\tq{q}\ts{s}\t{}\t{:#018x}", e.avif_file.len(), fnv1a(&e.avif_file));
        }
    }
}

//! Measurement harness for the zensim-diffmap closed loop
//! ([`zenavif::two_pass_zensim`]) and its fitted constants.
//!
//! Three modes, all emitting TSV on stdout (redirect to
//! `benchmarks/<name>_<date>.tsv`; write the companion `.meta` by hand):
//!
//! ```text
//! # 1. Dense quality sweep -> the score-vs-quality curves every fit reads.
//! cargo run --release --features two-pass-zensim --example zensim_loop_bench -- \
//!     sweep <manifest.txt> <speed> <sizes-comma> <q-grid-comma> [jobs]
//!
//! # 2. Real-encode A/B: the secant baseline vs the zensim loop, same images,
//! #    same targets, same tolerance.
//! cargo run --release --features two-pass-zensim --example zensim_loop_bench -- \
//!     ab <manifest.txt> <speed> <sizes-comma> <targets-comma> [tolerance] [max_encodes]
//!
//! # 3. Per-cell cost probe (one encode+decode+score at one quality).
//! cargo run --release --features two-pass-zensim --example zensim_loop_bench -- \
//!     probe <manifest.txt> <speed> <sizes-comma> <quality>
//!
//! # 4. Dense ACHIEVABLE-LATTICE sweep: every reachable quantizer index in
//! #    the band covering [score_lo, score_hi]. This is the exact set of
//! #    encodes any targeting search can land on, so it doubles as the
//! #    ground truth for offline replay of 2-shot rules.
//! cargo run --release --features two-pass-zensim --example zensim_loop_bench -- \
//!     lattice <manifest.txt> <speed> <sizes-comma> [score_lo] [score_hi]
//!
//! # 5. Real-encode 2-shot A/B: two-shot vs the loop vs the secant, all
//! #    capped at a budget of 2 encodes.
//! cargo run --release --features two-pass-zensim --example zensim_loop_bench -- \
//!     ab2 <manifest.txt> <speed> <sizes-comma> <targets-comma> [tolerance]
//! ```
//!
//! ## Why `lattice` sweeps quantizers, not qualities
//!
//! Encoded output depends on `quality` only through the **integer quantizer
//! index** it resolves to, and all 256 quantizers are addressable via
//! [`zenavif::quality_for_quantizer`] — 2.56× more distinct encodes than the
//! 100 that integer qualities reach. A quality-grid sweep therefore measures
//! a 2.56×-too-coarse caricature of the real achievable-score lattice, and
//! any "the lattice is too coarse to hit this tolerance" conclusion drawn
//! from one is correspondingly too pessimistic.
//!
//! `manifest.txt` is one source image path per line (`#` comments skipped).
//! Sizes are LONG-EDGE targets; a size larger than the source's long edge is
//! skipped rather than upscaled (upscaled sources are synthetic — no
//! high-frequency detail — and would bias every content-conditioned fit).
//! Downscaling uses Lanczos3.
//!
//! Encodes are pinned to one thread so bytes are deterministic and the
//! reported per-cell wall time is comparable across cells.
//!
//! **macOS scheduling gotcha, measured 2026-08-06:** do NOT run this under
//! `nice -n 19` on macOS. Darwin maps high nice values onto the background
//! QoS class, which confines the process to efficiency cores and throttles
//! it hard — the same `ab` workload ran at ~17% of one core under
//! `nice -n 19` and ~145% (multi-core, decode threads live) under
//! `nice -n 5`, a ~40x wall-clock difference on an otherwise idle box.
//! `nice -n 5` still yields to interactive work; `nice -n 19` turns an hour
//! of measurement into a day of it.

use std::io::{BufRead, Write};

use almost_enough::{StopToken, Unstoppable};
use imgref::{ImgRef, ImgVec};
use rgb::Rgb;
use zenavif::{
    EncoderConfig, TargetMetric, TargetOptions, ZensimLoopOptions, encode_rgb8_with_target,
    encode_rgb8_zensim_loop,
};

fn load_rgb8(path: &str) -> ImgVec<Rgb<u8>> {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("open {path}: {e}"))
        .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels: Vec<Rgb<u8>> = img
        .as_raw()
        .chunks_exact(3)
        .map(|c| Rgb::new(c[0], c[1], c[2]))
        .collect();
    ImgVec::new(pixels, w, h)
}

/// Lanczos3 downscale to `long_edge` on the longer axis, preserving aspect.
/// Returns `None` when the source is already smaller (never upscales).
fn downscale(src: &ImgVec<Rgb<u8>>, long_edge: u32) -> Option<ImgVec<Rgb<u8>>> {
    let (w, h) = (src.width() as u32, src.height() as u32);
    let src_long = w.max(h);
    if long_edge > src_long {
        return None;
    }
    if long_edge == src_long {
        return Some(src.clone());
    }
    let scale = f64::from(long_edge) / f64::from(src_long);
    let (nw, nh) = (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    );
    let mut flat = Vec::with_capacity(src.width() * src.height() * 3);
    for row in src.rows() {
        for p in row {
            flat.extend_from_slice(&[p.r, p.g, p.b]);
        }
    }
    let buf = image::RgbImage::from_raw(w, h, flat).expect("rgb buffer");
    let out = image::imageops::resize(&buf, nw, nh, image::imageops::FilterType::Lanczos3);
    let pixels: Vec<Rgb<u8>> = out
        .as_raw()
        .chunks_exact(3)
        .map(|c| Rgb::new(c[0], c[1], c[2]))
        .collect();
    Some(ImgVec::new(pixels, nw as usize, nh as usize))
}

fn read_manifest(path: &str) -> Vec<String> {
    let f = std::fs::File::open(path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    std::io::BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

fn parse_list<T: std::str::FromStr>(s: &str) -> Vec<T>
where
    T::Err: std::fmt::Debug,
{
    s.split(',').map(|v| v.trim().parse().unwrap()).collect()
}

fn base(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// One measured cell: what an encode at `quality` costs and achieves.
struct Cell {
    bytes: usize,
    score: f64,
    /// Mean of the zensim per-pixel error map — the spatial signal's
    /// magnitude, used to report the error-vs-quantizer elasticity that the
    /// loop's derived `spatial_strength` assumes.
    dm_mean: f64,
    /// Resolved AV1 quantizer index for this quality (zenavif's mirror of
    /// zenravif's quality->quantizer curve).
    qindex: u8,
    enc_ms: u128,
}

/// Encode at one **quantizer index**, then decode and score. The quantizer
/// is what the encode actually depends on, so this addresses one achievable
/// lattice point exactly.
fn encode_and_score_qi(img: ImgRef<'_, Rgb<u8>>, speed: u8, qi: u8) -> Option<Cell> {
    let c = encode_and_score(img, speed, zenavif::quality_for_quantizer(qi))?;
    debug_assert_eq!(c.qindex, qi);
    Some(c)
}

/// Encode at one quantizer with an optional per-superblock quantizer-scale
/// map, then decode and score.
fn encode_and_score_map(
    img: ImgRef<'_, Rgb<u8>>,
    speed: u8,
    qi: u8,
    map: Option<Box<[f32]>>,
) -> Option<Cell> {
    let cfg = EncoderConfig::new()
        .speed(speed)
        .quality(zenavif::quality_for_quantizer(qi))
        .threads(Some(1))
        .with_sb_q_scale(map);
    encode_and_score_cfg(img, &cfg)
}

/// The per-superblock map the two-shot's spatial term would build from an
/// un-hinted encode at `qi` — one extra encode+decode+diffmap.
fn diffmap_sb_map(
    img: ImgRef<'_, Rgb<u8>>,
    speed: u8,
    qi: u8,
    strength: f64,
    expect_sbs: usize,
) -> Option<Box<[f32]>> {
    let cfg = EncoderConfig::new()
        .speed(speed)
        .quality(zenavif::quality_for_quantizer(qi))
        .threads(Some(1));
    let enc = zenavif::encode_rgb8(img, &cfg, StopToken::new(Unstoppable)).ok()?;
    let dec = zenavif::decode_with(
        &enc.avif_file,
        &zenavif::DecoderConfig::new().prefer_8bit(true).threads(1),
        &StopToken::new(Unstoppable),
    )
    .ok()?;
    let dec_img: ImgRef<'_, Rgb<u8>> = dec.try_as_imgref::<Rgb<u8>>()?;
    let z = zensim::Zensim::new(zensim::ZensimProfile::codec_target());
    let dr = z
        .compute_with_diffmap(&img, &dec_img, zensim::DiffmapOptions::default())
        .ok()?;
    let map = zenavif::two_pass_zensim::sb_q_scale_from_diffmap(
        dr.diffmap(),
        dr.width(),
        dr.height(),
        &zenavif::TwoShotOptions {
            spatial_strength: strength,
            ..Default::default()
        },
    );
    (map.len() == expect_sbs).then_some(map)
}

/// Encode at one quality, then decode and score with the same zensim
/// profile the loop and the target search use.
fn encode_and_score(img: ImgRef<'_, Rgb<u8>>, speed: u8, quality: f32) -> Option<Cell> {
    encode_and_score_cfg(
        img,
        &EncoderConfig::new()
            .speed(speed)
            .quality(quality)
            .threads(Some(1)),
    )
}

/// Which zensim generation the harness scores with, from
/// `ZENSIM_BENCH_PROFILE` (`b`, the default, or `c`).
///
/// One switch instead of a parallel set of modes: every mode below reports
/// whatever `encode_and_score_cfg` measures, so the anchor fit, the lattice
/// geometry and the ab2 arms all follow the profile without duplicating a
/// line of sweep logic. Read once — an env lookup per cell would be free
/// next to an encode, but a mid-run change would silently mix two dials
/// into one TSV.
fn scoring_profile_is_c() -> bool {
    static IS_C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IS_C.get_or_init(|| {
        matches!(
            std::env::var("ZENSIM_BENCH_PROFILE").as_deref(),
            Ok("c") | Ok("C")
        )
    })
}

/// Encode with a fully built config, then decode and score it.
fn encode_and_score_cfg(img: ImgRef<'_, Rgb<u8>>, cfg: &EncoderConfig) -> Option<Cell> {
    let qindex = cfg
        .resolve_plan(zenavif::PlanInput::rgb8(
            img.width() as u32,
            img.height() as u32,
        ))
        .quantizer;
    let t0 = std::time::Instant::now();
    let enc = zenavif::encode_rgb8(img, cfg, StopToken::new(Unstoppable)).ok()?;
    let enc_ms = t0.elapsed().as_millis();
    let dec = zenavif::decode_with(
        &enc.avif_file,
        &zenavif::DecoderConfig::new().prefer_8bit(true).threads(1),
        &StopToken::new(Unstoppable),
    )
    .ok()?;
    let dec_img: ImgRef<'_, Rgb<u8>> = dec.try_as_imgref::<Rgb<u8>>()?;
    let z = zensim::Zensim::new(zensim::ZensimProfile::codec_target());
    let dr = z
        .compute_with_diffmap(&img, &dec_img, zensim::DiffmapOptions::default())
        .ok()?;
    let dm = dr.diffmap();
    // `dm_mean` stays the B diffmap's mean under BOTH profiles, on purpose.
    // It is the spatial signal's magnitude (the fitter's error-vs-quantizer
    // elasticity diagnostic), not the dial, and C's own spatial signal is
    // the attribution density — a different quantity in different units,
    // ~4x the cost of a score, that would make the column mean two things
    // across two files. The `zensim` column is the one that follows the
    // profile.
    let dm_mean = dm.iter().map(|&v| f64::from(v)).sum::<f64>() / dm.len().max(1) as f64;
    let score = if scoring_profile_is_c() {
        zenavif::zensim_c::ZensimC::new()
            .score(&img, &dec_img)
            .ok()?
    } else {
        dr.score()
    };
    Some(Cell {
        bytes: enc.avif_file.len(),
        score,
        dm_mean,
        qindex,
        enc_ms,
    })
}

// ---------------------------------------------------------------------------
// poolbench: where does the spatial channel's time actually go?
// ---------------------------------------------------------------------------

/// A stride-`step` p-norm pool of a full-resolution error map onto the
/// 64x64 superblock grid — the post-hoc subsampling arm.
///
/// Identical to `sb_pool::pool_pnorm` at `step == 1`; at `step == 2` it
/// visits one pixel in four, at 4 one in sixteen. The normalizer is the
/// SAMPLED count, so a uniform map still pools to exactly its own value at
/// every step and the arms stay directly comparable. There is no copy and
/// no allocation — the whole prize is `powf` calls not made.
fn pool_pnorm_strided(map: &[f32], w: usize, h: usize, exp: f64, step: usize) -> Vec<f64> {
    const SB: usize = 64;
    let (cols, rows) = (w.div_ceil(SB), h.div_ceil(SB));
    let mut out = Vec::with_capacity(cols * rows);
    for by in 0..rows {
        let y0 = by * SB;
        let y1 = (y0 + SB).min(h);
        for bx in 0..cols {
            let x0 = bx * SB;
            let x1 = (x0 + SB).min(w);
            let mut acc = 0.0f64;
            let mut n = 0usize;
            let mut y = y0;
            while y < y1 {
                let row = &map[y * w + x0..y * w + x1];
                let mut x = 0usize;
                while x < row.len() {
                    acc += f64::from(row[x]).max(0.0).powf(exp);
                    n += 1;
                    x += step;
                }
                y += step;
            }
            out.push(if n == 0 {
                0.0
            } else {
                (acc / n as f64).powf(1.0 / exp)
            });
        }
    }
    out
}

/// `pool_pnorm_strided` -> the same geomean-normalized, clamped,
/// exponentiated quantizer scales `sb_q_scale_from_diffmap` produces, so
/// the arms can be compared as MAPS and not just as timings.
fn q_scale_from_pooled(pooled: &[f64], clamp: (f64, f64), strength: f64) -> Vec<f32> {
    let (lo, hi) = clamp;
    let (mut log_sum, mut n) = (0.0f64, 0u32);
    for &e in pooled {
        if e > 1e-4 && e.is_finite() {
            log_sum += e.ln();
            n += 1;
        }
    }
    if n == 0 {
        return vec![1.0f32; pooled.len()];
    }
    let gm = (log_sum / f64::from(n)).exp();
    pooled
        .iter()
        .map(|&e| {
            if e > 1e-4 && e.is_finite() {
                ((e / gm).clamp(lo, hi)).powf(-strength) as f32
            } else {
                1.0f32
            }
        })
        .collect()
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() {
        f64::NAN
    } else {
        v[v.len() / 2]
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("");
    let manifest = args.get(2).expect("manifest arg");
    let speed: u8 = args.get(3).expect("speed arg").parse().unwrap();
    let sizes: Vec<u32> = parse_list(args.get(4).expect("sizes arg"));
    let images = read_manifest(manifest);
    let mut out = std::io::stdout().lock();

    match mode {
        "probe" => {
            let q: f32 = args.get(5).expect("quality arg").parse().unwrap();
            writeln!(
                out,
                "image\tsize\tw\th\tq\tqindex\tbytes\tzensim\tdm_mean\tenc_ms"
            )
            .unwrap();
            for path in &images {
                let src = load_rgb8(path);
                for &sz in &sizes {
                    let Some(img) = downscale(&src, sz) else {
                        continue;
                    };
                    if let Some(c) = encode_and_score(img.as_ref(), speed, q) {
                        writeln!(
                            out,
                            "{}\t{sz}\t{}\t{}\t{q}\t{}\t{}\t{:.4}\t{:.6e}\t{}",
                            base(path),
                            img.width(),
                            img.height(),
                            c.qindex,
                            c.bytes,
                            c.score,
                            c.dm_mean,
                            c.enc_ms
                        )
                        .unwrap();
                        out.flush().unwrap();
                    }
                }
            }
        }
        "sweep" => {
            let qs: Vec<f32> = parse_list(args.get(5).expect("q-grid arg"));
            writeln!(
                out,
                "image\tsize\tw\th\tq\tqindex\tbytes\tzensim\tdm_mean\tenc_ms"
            )
            .unwrap();
            for path in &images {
                let src = load_rgb8(path);
                for &sz in &sizes {
                    let Some(img) = downscale(&src, sz) else {
                        continue;
                    };
                    for &q in &qs {
                        if let Some(c) = encode_and_score(img.as_ref(), speed, q) {
                            writeln!(
                                out,
                                "{}\t{sz}\t{}\t{}\t{q}\t{}\t{}\t{:.4}\t{:.6e}\t{}",
                                base(path),
                                img.width(),
                                img.height(),
                                c.qindex,
                                c.bytes,
                                c.score,
                                c.dm_mean,
                                c.enc_ms
                            )
                            .unwrap();
                        }
                    }
                    out.flush().unwrap();
                    eprintln!("[sweep] {} @ {sz} done", base(path));
                }
            }
        }
        "hintdiag" => {
            // Does the per-SB map's CONTENT matter, or only whether it is
            // non-neutral? The hintprobe sweep came back invariant in the
            // dithered fraction, which is only consistent with the channel
            // acting as a switch. These probes separate the two: if D/E/F
            // (one / half / all superblocks at 0.5) are byte-identical to
            // each other, the content is being discarded.
            let qis: Vec<u8> = parse_list(args.get(5).expect("qindex list"));
            writeln!(out, "image\tsize\tw\th\tqi\tprobe\tsbs\tbytes\tzensim").unwrap();
            for path in &images {
                let src = load_rgb8(path);
                for &sz in &sizes {
                    let Some(img) = downscale(&src, sz) else {
                        continue;
                    };
                    let (w, h) = (img.width(), img.height());
                    let n = w.div_ceil(64) * h.div_ceil(64);
                    for &qi in &qis {
                        let probes: Vec<(&str, Option<Box<[f32]>>)> = vec![
                            ("none", None),
                            ("all_1.0", Some(vec![1.0f32; n].into_boxed_slice())),
                            (
                                "one_0.999",
                                Some(
                                    (0..n)
                                        .map(|i| if i == 0 { 0.999 } else { 1.0 })
                                        .collect::<Vec<f32>>()
                                        .into_boxed_slice(),
                                ),
                            ),
                            (
                                "one_0.5",
                                Some(
                                    (0..n)
                                        .map(|i| if i == 0 { 0.5 } else { 1.0 })
                                        .collect::<Vec<f32>>()
                                        .into_boxed_slice(),
                                ),
                            ),
                            (
                                "half_0.5",
                                Some(
                                    (0..n)
                                        .map(|i| if i * 2 < n { 0.5 } else { 1.0 })
                                        .collect::<Vec<f32>>()
                                        .into_boxed_slice(),
                                ),
                            ),
                            ("all_0.5", Some(vec![0.5f32; n].into_boxed_slice())),
                            ("all_2.0", Some(vec![2.0f32; n].into_boxed_slice())),
                            (
                                "alt_0.5_2.0",
                                Some(
                                    (0..n)
                                        .map(|i| if i % 2 == 0 { 0.5 } else { 2.0 })
                                        .collect::<Vec<f32>>()
                                        .into_boxed_slice(),
                                ),
                            ),
                            // Wrong length: must be IGNORED, i.e. identical
                            // to "none". This is the control that proves a
                            // null is a real null and not a silent drop.
                            ("wrong_len", Some(vec![0.5f32; n + 1].into_boxed_slice())),
                        ];
                        for (tag, map) in probes {
                            if let Some(c) = encode_and_score_map(img.as_ref(), speed, qi, map) {
                                writeln!(
                                    out,
                                    "{}\t{sz}\t{w}\t{h}\t{qi}\t{tag}\t{n}\t{}\t{:.4}",
                                    base(path),
                                    c.bytes,
                                    c.score
                                )
                                .unwrap();
                            }
                        }
                    }
                    out.flush().unwrap();
                    eprintln!("[hintdiag] {} @ {sz} done ({n} SBs)", base(path));
                }
            }
        }
        "hintprobe" => {
            // Can a MIXED per-superblock map place the frame score BETWEEN
            // two adjacent achievable lattice points? If so the quantizer
            // lattice stops being the precision floor.
            //
            // Two things the first version of this probe got wrong, both
            // now designed around:
            //  * AV1 codes per-SB delta_q at a RESOLUTION of 1/2/4/8
            //    quantizer indices (`variance_boost_delta_q_res_log2`,
            //    keyed on the frame's base quantizer). A nudge smaller than
            //    that resolution quantizes to zero, so the map collapses to
            //    "activated, all deltas zero" and the sweep comes back
            //    invariant in f. The scales here are large enough to
            //    survive; the f=0 arm is the activated-but-flat control
            //    that the invariant result actually was.
            //  * Activating delta-q is NOT a small perturbation: it also
            //    disables segmentation, so there is a content-dependent
            //    step between the un-hinted encode and the activated one.
            //    That step is measured separately (`activated_base`) so it
            //    is never mistaken for interpolation.
            //
            // Fractions are exact k/N over the frame's superblock count, so
            // f really is "this many superblocks", not a rounded request.
            let qis: Vec<u8> = parse_list(args.get(5).expect("qindex list"));
            let scales: Vec<f32> =
                parse_list(args.get(6).map(String::as_str).unwrap_or("0.97,0.94,0.88"));
            writeln!(
                out,
                "image\tsize\tw\th\tbase_qi\tvariant\tscale\tk\tsbs\tbytes\tzensim\tenc_ms"
            )
            .unwrap();
            for path in &images {
                let src = load_rgb8(path);
                for &sz in &sizes {
                    let Some(img) = downscale(&src, sz) else {
                        continue;
                    };
                    let (w, h) = (img.width(), img.height());
                    let nsb = w.div_ceil(64) * h.div_ceil(64);
                    let mut emit = |base_qi: u8, variant: &str, scale: f32, k: usize, c: &Cell| {
                        writeln!(
                            out,
                            "{}\t{sz}\t{w}\t{h}\t{base_qi}\t{variant}\t{scale}\t{k}\t{nsb}\t{}\t{:.4}\t{}",
                            base(path),
                            c.bytes,
                            c.score,
                            c.enc_ms
                        )
                        .unwrap();
                    };
                    for &qi in &qis {
                        // The un-hinted neighbourhood: the interval any
                        // sub-lattice claim has to be judged against.
                        for d in -2i32..=6 {
                            let q = (i32::from(qi) + d).clamp(0, 255) as u8;
                            if let Some(c) = encode_and_score_qi(img.as_ref(), speed, q) {
                                // The k column carries the ACTUAL quantizer
                                // for lattice rows, so the analysis can find
                                // the un-hinted point at `base_qi` exactly
                                // instead of inferring it from byte order
                                // (bytes are not reliably monotone in qi).
                                emit(qi, "lattice", 1.0, usize::from(q), &c);
                            }
                        }
                        for &scale in &scales {
                            // k superblocks nudged, chosen by van der Corput
                            // ordering so the subset is scattered rather
                            // than a contiguous stripe, and nested in k.
                            // Stride k so a 176-superblock cell does not
                            // cost 177 encodes per scale; k=0,1,2 are kept
                            // exactly because the activation step and the
                            // first real superblock move are the two points
                            // the whole question turns on.
                            let ks: Vec<usize> = {
                                let mut v = vec![0usize, 1, 2];
                                for i in 1..=10 {
                                    v.push(nsb * i / 10);
                                }
                                v.push(nsb);
                                v.retain(|&k| k <= nsb);
                                v.sort_unstable();
                                v.dedup();
                                v
                            };
                            for k in ks {
                                let mut order: Vec<(f64, usize)> = (0..nsb)
                                    .map(|i| {
                                        let (mut n, mut d, mut b) = (i + 1, 0.5f64, 0.0f64);
                                        while n > 0 {
                                            b += d * f64::from((n & 1) as u32);
                                            n >>= 1;
                                            d *= 0.5;
                                        }
                                        (b, i)
                                    })
                                    .collect();
                                order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                                let chosen: std::collections::HashSet<usize> =
                                    order.iter().take(k).map(|&(_, i)| i).collect();
                                // k == 0 still has to ACTIVATE delta-q, or
                                // it is a different regime from k >= 1 and
                                // the sweep is not an interpolation at all.
                                // 0.999 is below every delta_q resolution,
                                // so it activates without moving anything.
                                let map: Vec<f32> = (0..nsb)
                                    .map(|i| {
                                        if chosen.contains(&i) {
                                            scale
                                        } else if i == 0 {
                                            0.999
                                        } else {
                                            1.0
                                        }
                                    })
                                    .collect();
                                let tag = if k == 0 { "activated_base" } else { "dither" };
                                if let Some(c) = encode_and_score_map(
                                    img.as_ref(),
                                    speed,
                                    qi,
                                    Some(map.into_boxed_slice()),
                                ) {
                                    emit(qi, tag, scale, k, &c);
                                }
                            }
                        }
                        // The diffmap-derived map, for the RD question.
                        for &strength in &[0.5f64, 1.0, 2.0] {
                            if let Some(map) =
                                diffmap_sb_map(img.as_ref(), speed, qi, strength, nsb)
                                && let Some(c) =
                                    encode_and_score_map(img.as_ref(), speed, qi, Some(map))
                            {
                                emit(qi, "diffmap", strength as f32, 0, &c);
                            }
                        }
                    }
                    out.flush().unwrap();
                    eprintln!("[hintprobe] {} @ {sz} done ({nsb} SBs)", base(path));
                }
            }
        }
        "lattice" => {
            let score_lo: f64 = args.get(5).map_or(15.0, |v| v.parse().unwrap());
            let score_hi: f64 = args.get(6).map_or(95.0, |v| v.parse().unwrap());
            writeln!(
                out,
                "image\tsize\tw\th\tq\tqindex\tbytes\tzensim\tdm_mean\tenc_ms"
            )
            .unwrap();
            for path in &images {
                let src = load_rgb8(path);
                for &sz in &sizes {
                    let Some(img) = downscale(&src, sz) else {
                        continue;
                    };
                    let t_cell = std::time::Instant::now();
                    // Coarse probe first: locate the quantizer band whose
                    // scores straddle [score_lo, score_hi] without paying
                    // for the (very expensive) near-lossless end of the
                    // range on every cell.
                    let mut measured: std::collections::BTreeMap<u8, Cell> =
                        std::collections::BTreeMap::new();
                    let mut qi = 8u8;
                    loop {
                        if let Some(c) = encode_and_score_qi(img.as_ref(), speed, qi) {
                            measured.insert(qi, c);
                        }
                        let Some(next) = qi.checked_add(16) else {
                            break;
                        };
                        qi = next;
                    }
                    // Widest band that still contains the requested score
                    // window (probes are only approximately monotone, so
                    // take extremes rather than a bisection).
                    let lo_qi = measured
                        .iter()
                        .filter(|(_, c)| c.score >= score_hi)
                        .map(|(&k, _)| k)
                        .next_back()
                        .unwrap_or(8);
                    let hi_qi = measured
                        .iter()
                        .filter(|(_, c)| c.score <= score_lo)
                        .map(|(&k, _)| k)
                        .next()
                        .unwrap_or(248);
                    for qi in lo_qi..=hi_qi {
                        if measured.contains_key(&qi) {
                            continue;
                        }
                        if let Some(c) = encode_and_score_qi(img.as_ref(), speed, qi) {
                            measured.insert(qi, c);
                        }
                    }
                    for (&qi, c) in &measured {
                        writeln!(
                            out,
                            "{}\t{sz}\t{}\t{}\t{}\t{qi}\t{}\t{:.4}\t{:.6e}\t{}",
                            base(path),
                            img.width(),
                            img.height(),
                            zenavif::quality_for_quantizer(qi),
                            c.bytes,
                            c.score,
                            c.dm_mean,
                            c.enc_ms
                        )
                        .unwrap();
                    }
                    out.flush().unwrap();
                    eprintln!(
                        "[lattice] {} @ {sz}: qi {lo_qi}..={hi_qi}, {} points, {:.1}s",
                        base(path),
                        measured.len(),
                        t_cell.elapsed().as_secs_f64()
                    );
                }
            }
        }
        "ab2" => {
            // A FIXED budget of two encodes on every arm. The question is
            // only how close each lands, never how many encodes it wants,
            // so `encodes` is reported but is not the metric.
            //
            // Arms, and why each is here:
            //   twoshot          the shipped rule (quantizer-space
            //                    translate, nearest lattice point)
            //   twoshot_atleast  same, but never knowingly undershoot --
            //                    the overshoot policy, measured not assumed
            //   twoshot_spatial  same, plus the per-SB map at the strength
            //                    that measured best on RD (0.5). Says what
            //                    the now-live spatial half costs or buys
            //                    for PRECISION, separately from RD.
            //   loop2            the existing closed loop capped at 2. Its
            //                    spatial_strength defaults to 1.0 and that
            //                    is now LIVE, so this arm carries the map.
            //   loop2_nospatial  the same loop with the map suppressed --
            //                    isolates what the newly live default is
            //                    doing to the loop that did not ask for it.
            //   secant2          the baseline.
            let targets: Vec<f64> = parse_list(args.get(5).expect("targets arg"));
            let tolerance: f64 = args.get(6).map_or(0.5, |t| t.parse().unwrap());
            writeln!(
                out,
                "arm\timage\tsize\tw\th\ttarget\tachieved\terr\tencodes\tqi\tbytes\tms\tspatial\tpass1_qi\tpass1_score\tpredicted"
            )
            .unwrap();
            for path in &images {
                let src = load_rgb8(path);
                for &sz in &sizes {
                    let Some(img) = downscale(&src, sz) else {
                        continue;
                    };
                    let (w, h) = (img.width(), img.height());
                    for &target in &targets {
                        let cfg = EncoderConfig::new().speed(speed).threads(Some(1));
                        let mut row = |arm: &str,
                                       achieved: f64,
                                       encodes: u8,
                                       qi: i32,
                                       bytes: usize,
                                       ms: u128,
                                       spatial: &str,
                                       p1qi: i32,
                                       p1s: f64,
                                       pred: f64| {
                            writeln!(
                                out,
                                "{arm}\t{}\t{sz}\t{w}\t{h}\t{target}\t{achieved:.4}\t{:+.4}\t{encodes}\t{qi}\t{bytes}\t{ms}\t{spatial}\t{p1qi}\t{p1s:.4}\t{pred:.4}",
                                base(path),
                                achieved - target
                            )
                            .unwrap();
                        };

                        let two_shot_arms: [(&str, zenavif::TwoShotOptions); 3] = [
                            (
                                "twoshot",
                                zenavif::TwoShotOptions {
                                    tolerance,
                                    ..Default::default()
                                },
                            ),
                            (
                                "twoshot_atleast",
                                zenavif::TwoShotOptions {
                                    tolerance,
                                    policy: zenavif::LatticePolicy::AtLeast,
                                    ..Default::default()
                                },
                            ),
                            (
                                "twoshot_spatial",
                                zenavif::TwoShotOptions {
                                    tolerance,
                                    spatial_strength: 0.5,
                                    ..Default::default()
                                },
                            ),
                        ];
                        for (arm, opts) in two_shot_arms {
                            let t0 = std::time::Instant::now();
                            match zenavif::encode_rgb8_zensim_two_shot(
                                img.as_ref(),
                                &cfg,
                                target,
                                &opts,
                                StopToken::new(Unstoppable),
                            ) {
                                Ok(o) => row(
                                    arm,
                                    o.score,
                                    o.encodes,
                                    i32::from(o.quantizer),
                                    o.encoded.avif_file.len(),
                                    t0.elapsed().as_millis(),
                                    if o.spatial_applied && opts.spatial_strength != 0.0 {
                                        "applied"
                                    } else {
                                        "none"
                                    },
                                    i32::from(o.pass1_quantizer),
                                    o.pass1_score,
                                    o.predicted_score,
                                ),
                                Err(e) => eprintln!("FAIL {arm} {path} {sz} {target}: {e:?}"),
                            }
                        }

                        for (arm, strength) in [("loop2", 1.0f64), ("loop2_nospatial", 0.0)] {
                            let t0 = std::time::Instant::now();
                            match encode_rgb8_zensim_loop(
                                img.as_ref(),
                                &cfg,
                                target,
                                &ZensimLoopOptions {
                                    tolerance,
                                    max_encodes: 2,
                                    spatial_strength: strength,
                                    ..Default::default()
                                },
                                StopToken::new(Unstoppable),
                            ) {
                                Ok(o) => row(
                                    arm,
                                    o.score,
                                    o.encodes,
                                    -1,
                                    o.encoded.avif_file.len(),
                                    t0.elapsed().as_millis(),
                                    if o.spatial_applied && strength != 0.0 {
                                        "applied"
                                    } else {
                                        "none"
                                    },
                                    -1,
                                    o.pass1_score,
                                    f64::NAN,
                                ),
                                Err(e) => eprintln!("FAIL {arm} {path} {sz} {target}: {e:?}"),
                            }
                        }

                        let t0 = std::time::Instant::now();
                        match encode_rgb8_with_target(
                            img.as_ref(),
                            &cfg,
                            TargetMetric::Zensim(target),
                            &TargetOptions {
                                tolerance,
                                max_encodes: 2,
                                ..Default::default()
                            },
                            StopToken::new(Unstoppable),
                        ) {
                            Ok(o) => row(
                                "secant2",
                                o.score,
                                o.encodes,
                                -1,
                                o.encoded.avif_file.len(),
                                t0.elapsed().as_millis(),
                                "none",
                                -1,
                                f64::NAN,
                                f64::NAN,
                            ),
                            Err(e) => eprintln!("FAIL secant2 {path} {sz} {target}: {e:?}"),
                        }
                    }
                    out.flush().unwrap();
                    eprintln!("[ab2] {} @ {sz} done", base(path));
                }
            }
        }
        "ab" => {
            let targets: Vec<f64> = parse_list(args.get(5).expect("targets arg"));
            let tolerance: f64 = args.get(6).map_or(0.5, |t| t.parse().unwrap());
            let max_encodes: u8 = args.get(7).map_or(6, |t| t.parse().unwrap());
            writeln!(
                out,
                "arm\timage\tsize\tw\th\ttarget\tachieved\terr\tencodes\tconverged\tbytes\tms\tspatial\tseed_q\tseed_score\tanchor_q"
            )
            .unwrap();
            for path in &images {
                let src = load_rgb8(path);
                for &sz in &sizes {
                    let Some(img) = downscale(&src, sz) else {
                        continue;
                    };
                    let (w, h) = (img.width(), img.height());
                    for &target in &targets {
                        let cfg = EncoderConfig::new().speed(speed).threads(Some(1));

                        // Baseline: the existing bracketed secant search.
                        let opts = TargetOptions {
                            tolerance,
                            max_encodes,
                            ..Default::default()
                        };
                        let t0 = std::time::Instant::now();
                        let r = encode_rgb8_with_target(
                            img.as_ref(),
                            &cfg,
                            TargetMetric::Zensim(target),
                            &opts,
                            StopToken::new(Unstoppable),
                        );
                        let ms = t0.elapsed().as_millis();
                        match r {
                            Ok(o) => writeln!(
                                out,
                                "secant\t{}\t{sz}\t{w}\t{h}\t{target}\t{:.4}\t{:+.4}\t{}\t{}\t{}\t{ms}\tNA\tNA\tNA\tNA",
                                base(path),
                                o.score,
                                o.score - target,
                                o.encodes,
                                o.converged,
                                o.encoded.avif_file.len()
                            )
                            .unwrap(),
                            Err(e) => eprintln!("FAIL secant {path} {sz} {target}: {e:?}"),
                        }

                        // The zensim closed loop.
                        let lopts = ZensimLoopOptions {
                            tolerance,
                            max_encodes,
                            ..Default::default()
                        };
                        let t0 = std::time::Instant::now();
                        let r = encode_rgb8_zensim_loop(
                            img.as_ref(),
                            &cfg,
                            target,
                            &lopts,
                            StopToken::new(Unstoppable),
                        );
                        let ms = t0.elapsed().as_millis();
                        match r {
                            Ok(o) => writeln!(
                                out,
                                "zloop\t{}\t{sz}\t{w}\t{h}\t{target}\t{:.4}\t{:+.4}\t{}\t{}\t{}\t{ms}\t{}\t{:.3}\t{:.4}\t{:.3}",
                                base(path),
                                o.score,
                                o.score - target,
                                o.encodes,
                                o.converged,
                                o.encoded.avif_file.len(),
                                o.spatial_applied,
                                o.pass1_quality,
                                o.pass1_score,
                                zenavif::anchor_quality_for_zensim(target)
                            )
                            .unwrap(),
                            Err(e) => eprintln!("FAIL zloop {path} {sz} {target}: {e:?}"),
                        }
                    }
                    out.flush().unwrap();
                    eprintln!("[ab] {} @ {sz} done", base(path));
                }
            }
        }
        // Where the spatial channel's time goes, and whether pooling a
        // SUBSAMPLED map is worth anything. Arms are interleaved within
        // each rep (never all-A-then-all-B) so thermal drift cannot fake a
        // delta; medians over `reps`.
        "poolbench" => {
            let qi: u8 = args.get(5).map_or(80, |v| v.parse().unwrap());
            let reps: usize = args.get(6).map_or(7, |v| v.parse().unwrap());
            writeln!(
                out,
                "image\tsize\tw\th\tqi\tsbs\tb_score_ms\tb_score_map_ms\tb_pool1_ms\tb_pool2_ms\tb_pool4_ms\t\
                 c_score_ms\tc_steer_ms\tc_sbmeans_ms\td2_max\td2_med\td4_max\td4_med"
            )
            .unwrap();
            for path in &images {
                let src = load_rgb8(path);
                for &sz in &sizes {
                    let Some(img) = downscale(&src, sz) else {
                        continue;
                    };
                    let cfg = EncoderConfig::new()
                        .speed(speed)
                        .quality(zenavif::quality_for_quantizer(qi))
                        .threads(Some(1));
                    let Ok(enc) =
                        zenavif::encode_rgb8(img.as_ref(), &cfg, StopToken::new(Unstoppable))
                    else {
                        continue;
                    };
                    let Ok(dec) = zenavif::decode_with(
                        &enc.avif_file,
                        &zenavif::DecoderConfig::new().prefer_8bit(true).threads(1),
                        &StopToken::new(Unstoppable),
                    ) else {
                        continue;
                    };
                    let Some(dec_img) = dec.try_as_imgref::<Rgb<u8>>() else {
                        continue;
                    };
                    let z = zensim::Zensim::new(zensim::ZensimProfile::codec_target())
                        .with_parallel(false);
                    let Ok(pre) = z.precompute_reference(&img.as_ref()) else {
                        continue;
                    };
                    let dm_opts = zensim::DiffmapOptions::default();
                    // One map up front for the accuracy comparison.
                    let Ok(dr) = z.compute_with_ref_and_diffmap(&pre, &dec_img, dm_opts) else {
                        continue;
                    };
                    let (mw, mh) = (dr.width(), dr.height());
                    let opts = zenavif::TwoShotOptions {
                        spatial_strength: 1.0,
                        ..Default::default()
                    };
                    let m1 = zenavif::two_pass_zensim::sb_q_scale_from_diffmap(
                        dr.diffmap(),
                        mw,
                        mh,
                        &opts,
                    );
                    let m2 = q_scale_from_pooled(
                        &pool_pnorm_strided(dr.diffmap(), mw, mh, opts.pool_exponent, 2),
                        opts.weight_clamp,
                        opts.spatial_strength,
                    );
                    let m4 = q_scale_from_pooled(
                        &pool_pnorm_strided(dr.diffmap(), mw, mh, opts.pool_exponent, 4),
                        opts.weight_clamp,
                        opts.spatial_strength,
                    );
                    let delta = |a: &[f32], b: &[f32]| -> (f64, f64) {
                        let mut d: Vec<f64> = a
                            .iter()
                            .zip(b.iter())
                            .map(|(x, y)| f64::from(x - y).abs())
                            .collect();
                        let mx = d.iter().cloned().fold(0.0f64, f64::max);
                        (mx, median(&mut d))
                    };
                    let (d2_max, d2_med) = delta(&m1, &m2);
                    let (d4_max, d4_med) = delta(&m1, &m4);

                    let mut t_bs = Vec::new();
                    let mut t_bsm = Vec::new();
                    let mut t_p1 = Vec::new();
                    let mut t_p2 = Vec::new();
                    let mut t_p4 = Vec::new();
                    let mut t_cs = Vec::new();
                    let mut t_cst = Vec::new();
                    let mut t_csb = Vec::new();
                    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
                    for _ in 0..reps {
                        let t = std::time::Instant::now();
                        std::hint::black_box(z.compute_with_ref(&pre, &dec_img).ok());
                        t_bs.push(ms(t));

                        let t = std::time::Instant::now();
                        let r = z.compute_with_ref_and_diffmap(&pre, &dec_img, dm_opts).ok();
                        t_bsm.push(ms(t));
                        std::hint::black_box(&r);

                        let t = std::time::Instant::now();
                        std::hint::black_box(zenavif::two_pass_zensim::sb_q_scale_from_diffmap(
                            dr.diffmap(),
                            mw,
                            mh,
                            &opts,
                        ));
                        t_p1.push(ms(t));

                        let t = std::time::Instant::now();
                        std::hint::black_box(pool_pnorm_strided(
                            dr.diffmap(),
                            mw,
                            mh,
                            opts.pool_exponent,
                            2,
                        ));
                        t_p2.push(ms(t));

                        let t = std::time::Instant::now();
                        std::hint::black_box(pool_pnorm_strided(
                            dr.diffmap(),
                            mw,
                            mh,
                            opts.pool_exponent,
                            4,
                        ));
                        t_p4.push(ms(t));

                        let mut zc = zenavif::zensim_c::ZensimC::new().with_parallel(false);
                        let t = std::time::Instant::now();
                        std::hint::black_box(zc.score(&img.as_ref(), &dec_img).ok());
                        t_cs.push(ms(t));

                        let t = std::time::Instant::now();
                        let st = zc.steer(&img.as_ref(), &dec_img).ok();
                        t_cst.push(ms(t));

                        if let Some(st) = st.as_ref() {
                            let t = std::time::Instant::now();
                            std::hint::black_box(st.sb_means());
                            t_csb.push(ms(t));
                        }
                    }
                    writeln!(
                        out,
                        "{}\t{sz}\t{mw}\t{mh}\t{qi}\t{}\t{:.3}\t{:.3}\t{:.4}\t{:.4}\t{:.4}\t\
                         {:.3}\t{:.3}\t{:.4}\t{d2_max:.5}\t{d2_med:.5}\t{d4_max:.5}\t{d4_med:.5}",
                        base(path),
                        m1.len(),
                        median(&mut t_bs),
                        median(&mut t_bsm),
                        median(&mut t_p1),
                        median(&mut t_p2),
                        median(&mut t_p4),
                        median(&mut t_cs),
                        median(&mut t_cst),
                        median(&mut t_csb),
                    )
                    .unwrap();
                    out.flush().unwrap();
                    eprintln!("[poolbench] {} @ {sz} done", base(path));
                }
            }
        }
        // Does C's attribution map STEER? Matched-QUANTIZER A/B: encode
        // plain, build the per-SB map from C's own attribution density,
        // re-encode at the SAME quantizer with the map, compare bytes and
        // C score. NOT rate-matched — a map that buys score by spending
        // bytes is not a win, so both deltas are reported and neither is
        // read alone.
        "steerbench" => {
            let qis: Vec<u8> = parse_list(args.get(5).map_or("60,100,140", |v| v.as_str()));
            let strengths: Vec<f64> = parse_list(args.get(6).map_or("0.5,1.0", |v| v.as_str()));
            writeln!(
                out,
                "image\tsize\tw\th\tqi\tstrength\tsbs\tbase_bytes\tbase_c\tmap_bytes\tmap_c\t\
                 d_bytes_pct\td_c\tmap_min\tmap_max\tgrad_nz"
            )
            .unwrap();
            for path in &images {
                let src = load_rgb8(path);
                for &sz in &sizes {
                    let Some(img) = downscale(&src, sz) else {
                        continue;
                    };
                    for &qi in &qis {
                        let q = zenavif::quality_for_quantizer(qi);
                        let base_cfg = EncoderConfig::new()
                            .speed(speed)
                            .quality(q)
                            .threads(Some(1));
                        let Ok(base_enc) = zenavif::encode_rgb8(
                            img.as_ref(),
                            &base_cfg,
                            StopToken::new(Unstoppable),
                        ) else {
                            continue;
                        };
                        let Ok(dec) = zenavif::decode_with(
                            &base_enc.avif_file,
                            &zenavif::DecoderConfig::new().prefer_8bit(true).threads(1),
                            &StopToken::new(Unstoppable),
                        ) else {
                            continue;
                        };
                        let Some(dec_img) = dec.try_as_imgref::<Rgb<u8>>() else {
                            continue;
                        };
                        let mut zc = zenavif::zensim_c::ZensimC::new().with_parallel(false);
                        let Ok(steer) = zc.steer(&img.as_ref(), &dec_img) else {
                            continue;
                        };
                        let means = steer.sb_means();
                        let grad_nz = steer.gradient_nonzero();
                        let base_c = steer.score();
                        for &st in &strengths {
                            let map = zenavif::zensim_c::sb_q_scale_from_attribution(
                                &means,
                                (0.4, 2.5),
                                st,
                            );
                            let (mn, mx) = map
                                .iter()
                                .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
                            let cfg = base_cfg.clone().with_sb_q_scale(Some(map.clone()));
                            let Ok(enc) = zenavif::encode_rgb8(
                                img.as_ref(),
                                &cfg,
                                StopToken::new(Unstoppable),
                            ) else {
                                continue;
                            };
                            let Ok(d2) = zenavif::decode_with(
                                &enc.avif_file,
                                &zenavif::DecoderConfig::new().prefer_8bit(true).threads(1),
                                &StopToken::new(Unstoppable),
                            ) else {
                                continue;
                            };
                            let Some(d2img) = d2.try_as_imgref::<Rgb<u8>>() else {
                                continue;
                            };
                            let Ok(map_c) = zc.score(&img.as_ref(), &d2img) else {
                                continue;
                            };
                            let bb = base_enc.avif_file.len() as f64;
                            let mb = enc.avif_file.len() as f64;
                            writeln!(
                                out,
                                "{}\t{sz}\t{}\t{}\t{qi}\t{st}\t{}\t{}\t{base_c:.4}\t{}\t{map_c:.4}\t\
                                 {:.3}\t{:+.4}\t{mn:.4}\t{mx:.4}\t{grad_nz}",
                                base(path),
                                img.width(),
                                img.height(),
                                map.len(),
                                base_enc.avif_file.len(),
                                enc.avif_file.len(),
                                100.0 * (mb - bb) / bb,
                                map_c - base_c,
                            )
                            .unwrap();
                            out.flush().unwrap();
                        }
                    }
                    eprintln!("[steerbench] {} @ {sz} done", base(path));
                }
            }
        }
        other => {
            eprintln!("unknown mode {other:?}; expected sweep | ab | probe");
            std::process::exit(2);
        }
    }
}

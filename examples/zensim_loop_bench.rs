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
//! ```
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

/// Encode at one quality, then decode and score with the same zensim
/// profile the loop and the target search use.
fn encode_and_score(img: ImgRef<'_, Rgb<u8>>, speed: u8, quality: f32) -> Option<Cell> {
    let cfg = EncoderConfig::new()
        .speed(speed)
        .quality(quality)
        .threads(Some(1));
    let qindex = cfg
        .resolve_plan(zenavif::PlanInput::rgb8(
            img.width() as u32,
            img.height() as u32,
        ))
        .quantizer;
    let t0 = std::time::Instant::now();
    let enc = zenavif::encode_rgb8(img, &cfg, StopToken::new(Unstoppable)).ok()?;
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
    let dm_mean = dm.iter().map(|&v| f64::from(v)).sum::<f64>() / dm.len().max(1) as f64;
    Some(Cell {
        bytes: enc.avif_file.len(),
        score: dr.score(),
        dm_mean,
        qindex,
        enc_ms,
    })
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
        other => {
            eprintln!("unknown mode {other:?}; expected sweep | ab | probe");
            std::process::exit(2);
        }
    }
}

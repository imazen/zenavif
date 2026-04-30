//! Phase 2 — One-At-a-Time knob sensitivity sweep.
//!
//! For each (image, size) cell, encode at a baseline (speed=4, q=60,
//! defaults: qm=on vaq=off vaq_strength=1.0 tune_still=on) and at one
//! perturbation per knob, holding all other knobs at the baseline.
//! The Phase 1a TSV already has the baseline row for free, so this
//! harness only encodes the perturbations and joins against the
//! baseline at analysis time.
//!
//! Output TSV columns:
//!   image_path, size_class, width, height, knob, perturbation,
//!   bytes, zensim, encode_ms
//!
//! Post-process: for each knob, compute Δ% bytes vs baseline across
//! (image, size) cells. Cull rule (per docs/RAV1E_PICKER_PLAN.md):
//!   median |Δ%| < 0.5 % AND p90 |Δ%| < 1.5 %  →  drop knob
//!
//! Surviving knobs become CATEGORICAL_AXES / SCALAR_AXES additions
//! in v0.2 of training/rav1e_picker_config.py.
//!
//! Usage:
//!   cargo run --release --example phase2_oat \
//!     --features encode-imazen,encode-threading -- \
//!     --manifest ~/work/codec-corpus/picker-train/manifest.tsv \
//!     --output benchmarks/rav1e_phase2_oat_<DATE>.tsv \
//!     --sizes 64,256,1024,4096 \
//!     --max-images 50 --threads 14

use almost_enough::{StopToken, Unstoppable};
use image::{DynamicImage, GenericImageView, ImageReader, imageops::FilterType};
use imgref::{Img, ImgVec};
use rayon::prelude::*;
use rgb::{RGB8, Rgb};
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Instant;
use zensim::{Zensim, ZensimProfile};
use zensim_regress::{RegressionTolerance, check_regression};

const BASELINE_SPEED: u8 = 4;
const BASELINE_Q: f32 = 60.0;

/// Each perturbation: (knob_id, value_label, builder closure).
/// The closure takes a baseline ravif::Encoder and applies one knob
/// override on top. The knob_id and value_label land in the TSV so
/// post-process can join.
type Pert = (
    &'static str,
    &'static str,
    fn(ravif::Encoder<'_>) -> ravif::Encoder<'_>,
);

fn perturbations() -> Vec<Pert> {
    vec![
        // Macro knobs (zenavif user-visible)
        ("qm", "off", |e| e.with_qm(false)),
        ("vaq", "on_strength_1.0", |e| e.with_vaq(true, 1.0)),
        ("vaq_strength", "0.5", |e| e.with_vaq(true, 0.5)),
        ("vaq_strength", "2.0", |e| e.with_vaq(true, 2.0)),
        ("vaq_strength", "3.0", |e| e.with_vaq(true, 3.0)),
        ("tune_still", "off", |e| e.with_still_image_tuning(false)),
        ("seg_boost", "1.5", |e| e.with_seg_boost(1.5)),
        ("seg_boost", "2.0", |e| e.with_seg_boost(2.0)),
        ("trellis", "on", |e| e.with_trellis(true)),
        // Internal speed-knob overrides (Option<bool>)
        ("cdef", "off", |e| e.with_cdef(Some(false))),
        ("rdo_tx_decision", "off", |e| e.with_rdo_tx_decision(Some(false))),
        ("sgr_full", "off", |e| e.with_sgr_full(Some(false))),
        ("lru_on_skip", "on", |e| e.with_lru_on_skip(Some(true))),
        ("segmentation_complex", "on", |e| e.with_segmentation_complex(Some(true))),
        ("encode_bottomup", "on", |e| e.with_encode_bottomup(Some(true))),
        // Deep knobs (newly plumbed)
        ("partition_range", "fine_4_16", |e| e.with_partition_range(Some((4, 16)))),
        ("partition_range", "coarse_16_64", |e| e.with_partition_range(Some((16, 64)))),
        ("complex_prediction_modes", "on", |e| e.with_complex_prediction_modes(Some(true))),
        ("lrf", "off", |e| e.with_lrf(Some(false))),
        ("lrf", "on", |e| e.with_lrf(Some(true))),
        ("fast_deblock", "on", |e| e.with_fast_deblock(Some(true))),
    ]
}

#[derive(Clone, Debug)]
struct ManifestEntry {
    sha256: String,
    content_class: String,
    source: String,
    path: PathBuf,
}

#[derive(Clone, Debug)]
struct Args {
    manifest: PathBuf,
    output: PathBuf,
    sizes: Vec<u32>,
    max_images: Option<usize>,
    threads: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut manifest = None;
    let mut output = PathBuf::from("./phase2_oat.tsv");
    let mut sizes = vec![64, 256, 1024, 4096];
    let mut max_images = None;
    let mut threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);

    let raw: Vec<String> = env::args().collect();
    let mut iter = raw.iter().skip(1);
    while let Some(a) = iter.next() {
        match a.as_str() {
            "-h" | "--help" => {
                eprintln!(
                    "Usage: phase2_oat --manifest PATH --output PATH \
                     [--sizes 64,256,...] [--max-images N] [--threads N]"
                );
                std::process::exit(0);
            }
            "--manifest" => manifest = iter.next().map(PathBuf::from),
            "--output" => output = iter.next().map(PathBuf::from).ok_or("--output PATH")?,
            "--sizes" => {
                sizes = iter
                    .next()
                    .ok_or("--sizes")?
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
            }
            "--max-images" => {
                max_images = Some(
                    iter.next()
                        .ok_or("--max-images N")?
                        .parse()
                        .map_err(|e| format!("--max-images: {e}"))?,
                );
            }
            "--threads" => {
                threads = iter
                    .next()
                    .ok_or("--threads N")?
                    .parse()
                    .map_err(|e| format!("--threads: {e}"))?;
            }
            other => return Err(format!("unknown arg {other}")),
        }
    }
    Ok(Args {
        manifest: manifest.ok_or("--manifest PATH required")?,
        output,
        sizes,
        max_images,
        threads,
    })
}

fn read_manifest(path: &Path) -> Result<Vec<ManifestEntry>, String> {
    let f = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let r = BufReader::new(f);
    let mut out = Vec::new();
    let mut header_idx: std::collections::HashMap<String, usize> = Default::default();
    for (i, line) in r.lines().enumerate() {
        let line = line.map_err(|e| format!("read line {i}: {e}"))?;
        let cols: Vec<&str> = line.split('\t').collect();
        if i == 0 {
            for (idx, name) in cols.iter().enumerate() {
                header_idx.insert(name.to_string(), idx);
            }
            continue;
        }
        let get = |k: &str| {
            header_idx
                .get(k)
                .and_then(|&idx| cols.get(idx).copied())
                .unwrap_or("")
                .to_string()
        };
        let path_str = get("path");
        if path_str.is_empty() {
            continue;
        }
        out.push(ManifestEntry {
            sha256: get("sha256"),
            content_class: get("content_class"),
            source: get("source"),
            path: PathBuf::from(path_str),
        });
    }
    Ok(out)
}

fn resize_to_maxdim(src: &DynamicImage, target_maxdim: u32) -> Option<DynamicImage> {
    let (w, h) = src.dimensions();
    let cur_max = w.max(h);
    if cur_max <= target_maxdim {
        if cur_max == target_maxdim {
            return Some(src.clone());
        }
        return None;
    }
    let ratio = target_maxdim as f64 / cur_max as f64;
    let new_w = ((w as f64) * ratio).round().max(1.0) as u32;
    let new_h = ((h as f64) * ratio).round().max(1.0) as u32;
    Some(src.resize_exact(new_w, new_h, FilterType::Lanczos3))
}

fn rgb_from_dynamic(img: &DynamicImage) -> ImgVec<RGB8> {
    let (w, h) = img.dimensions();
    let rgb8 = img.to_rgb8();
    let buf: Vec<RGB8> = rgb8
        .pixels()
        .map(|p| RGB8::new(p[0], p[1], p[2]))
        .collect();
    ImgVec::new(buf, w as usize, h as usize)
}

fn size_class_label(w: u32, h: u32) -> &'static str {
    let n = (w as u64) * (h as u64);
    if n < 64 * 64 {
        "tiny"
    } else if n < 256 * 256 {
        "small"
    } else if n < 1024 * 1024 {
        "medium"
    } else {
        "large"
    }
}

fn baseline_encoder<'a>() -> ravif::Encoder<'a> {
    // Match predictor_sweep.rs's defaults: qm=on, vaq=off,
    // vaq_strength=1.0, tune_still=on. speed=4, q=60.
    ravif::Encoder::new()
        .with_quality(BASELINE_Q)
        .with_speed(BASELINE_SPEED)
        .with_bit_depth(ravif::BitDepth::Eight)
        .with_qm(true)
        .with_vaq(false, 1.0)
        .with_still_image_tuning(true)
        .with_num_threads(Some(1))
        .with_stop(StopToken::new(Unstoppable))
}

struct Outcome {
    bytes: usize,
    zensim: f32,
    encode_ms: f64,
}

fn encode_score(
    enc: ravif::Encoder<'_>,
    img: Img<&[RGB8]>,
    zensim: &Zensim,
    tol: &RegressionTolerance,
) -> Result<Outcome, String> {
    let t0 = Instant::now();
    let result = enc.encode_rgb(img).map_err(|e| format!("encode: {e}"))?;
    let encode_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let cfg = zenavif::DecoderConfig::new().prefer_8bit(true);
    let decoded = zenavif::decode_with(&result.avif_file, &cfg, &Unstoppable)
        .map_err(|e| format!("decode: {e}"))?;
    let decoded_ref = decoded
        .try_as_imgref::<Rgb<u8>>()
        .ok_or("decoded buffer not Rgb<u8>")?;
    let r = check_regression(zensim, &img, &decoded_ref, tol)
        .map_err(|e| format!("zensim: {e}"))?;

    Ok(Outcome {
        bytes: result.avif_file.len(),
        zensim: r.score() as f32,
        encode_ms,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let mut manifest = match read_manifest(&args.manifest) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };

    if let Some(n) = args.max_images
        && manifest.len() > n
    {
        manifest.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        manifest.truncate(n);
    }

    let perts = perturbations();
    eprintln!(
        "OAT: {} images × {} sizes × {} perturbations (+ baseline) = {} encodes",
        manifest.len(),
        args.sizes.len(),
        perts.len(),
        manifest.len() * args.sizes.len() * (perts.len() + 1)
    );

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok();
    }

    let writing_header = !args.output.exists();
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.output)
        .unwrap_or_else(|e| panic!("open {}: {e}", args.output.display()));
    let writer = Mutex::new(BufWriter::new(file));
    if writing_header {
        let mut w = writer.lock().unwrap();
        writeln!(
            w,
            "image_path\tsize_class\twidth\theight\tsha256\tcontent_class\tsource\tknob\tperturbation\tbytes\tzensim\tencode_ms"
        )
        .unwrap();
        w.flush().ok();
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(args.threads.max(1))
        .build()
        .expect("rayon pool");
    let zensim = Zensim::new(ZensimProfile::latest());
    let tol = RegressionTolerance::off_by_one().with_min_similarity(0.0);

    let total_done = std::sync::atomic::AtomicUsize::new(0);
    let total_failed = std::sync::atomic::AtomicUsize::new(0);

    pool.install(|| {
        manifest.par_iter().for_each(|entry| {
            let dyn_img = match ImageReader::open(&entry.path).and_then(|r| Ok(r.decode())) {
                Ok(Ok(img)) => img,
                _ => {
                    eprintln!("skip (decode fail): {}", entry.path.display());
                    return;
                }
            };

            for &target in &args.sizes {
                let Some(resized) = resize_to_maxdim(&dyn_img, target) else {
                    continue;
                };
                let img_buf: ImgVec<RGB8> = rgb_from_dynamic(&resized);
                let img = img_buf.as_ref();
                let sz = size_class_label(img.width() as u32, img.height() as u32);

                // Baseline first.
                if let Ok(o) = encode_score(baseline_encoder(), img, &zensim, &tol) {
                    let mut w = writer.lock().unwrap();
                    writeln!(
                        w,
                        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.2}",
                        entry.path.display(),
                        sz,
                        img.width(),
                        img.height(),
                        entry.sha256,
                        entry.content_class,
                        entry.source,
                        "baseline",
                        "default",
                        o.bytes,
                        o.zensim,
                        o.encode_ms,
                    )
                    .ok();
                }

                // Each perturbation.
                for (knob, value_label, build) in &perts {
                    let enc = build(baseline_encoder());
                    match encode_score(enc, img, &zensim, &tol) {
                        Ok(o) => {
                            let mut w = writer.lock().unwrap();
                            writeln!(
                                w,
                                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.6}\t{:.2}",
                                entry.path.display(),
                                sz,
                                img.width(),
                                img.height(),
                                entry.sha256,
                                entry.content_class,
                                entry.source,
                                knob,
                                value_label,
                                o.bytes,
                                o.zensim,
                                o.encode_ms,
                            )
                            .ok();
                            if total_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 50
                                == 0
                            {
                                w.flush().ok();
                                eprintln!(
                                    "[done={} failed={}]",
                                    total_done.load(std::sync::atomic::Ordering::Relaxed),
                                    total_failed.load(std::sync::atomic::Ordering::Relaxed),
                                );
                            }
                        }
                        Err(e) => {
                            total_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            eprintln!("FAIL {}/{}/{}/{}: {e}", entry.sha256, target, knob, value_label);
                        }
                    }
                }
            }
        });
    });

    let mut w = writer.lock().unwrap();
    w.flush().ok();
    drop(w);

    eprintln!(
        "\nfinal: done={} failed={}",
        total_done.load(std::sync::atomic::Ordering::Relaxed),
        total_failed.load(std::sync::atomic::Ordering::Relaxed),
    );
    ExitCode::from(0)
}

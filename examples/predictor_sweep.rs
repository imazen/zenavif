//! Multi-image, multi-size, multi-knob sweep harness for the
//! rav1e knob predictor MLP training.
//!
//! Reads a manifest TSV (`sha256, split, content_class, source, size_bytes,
//! path` columns), resizes each image to one or more target maxdim sizes
//! via Lanczos3, sweeps the configured axes through zenavif's encoder, and
//! writes one TSV row per encode with bytes, zensim, encode_ms, decode_ms,
//! and full config provenance.
//!
//! Output schema matches what `zenpicker/tools/train_hybrid.py` expects.
//!
//! Phases:
//!   1a baseline: --axes speed,q (no deep-knob axes)
//!   2 OAT       : --axes speed,q + per-knob single-axis perturbations
//!   3 LHS joint : caller passes a CSV of pre-sampled knob tuples
//!   4 full corpus: same as 3 with bigger manifest
//!
//! Resumable: if --output exists and --append is set, skip rows whose
//! (sha256, size_bucket, config_id) tuple is already in the file.
//!
//! Usage:
//!   cargo run --release --example predictor_sweep \
//!     --features encode-imazen,encode-threading -- \
//!     --manifest ~/work/codec-corpus/picker-train/manifest_v1_100.tsv \
//!     --output benchmarks/rav1e_phase1a_<DATE>.tsv \
//!     --speeds 0..=10 \
//!     --qualities 5..=100:5 \
//!     --sizes 64,256,1024,4096 \
//!     --max-images 50

use almost_enough::{StopToken, Unstoppable};
use image::{DynamicImage, GenericImageView, ImageReader, imageops::FilterType};
use imgref::{Img, ImgRef, ImgVec};
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
    speeds: Vec<u8>,
    qualities: Vec<u8>,
    sizes: Vec<u32>, // target maxdim
    max_images: Option<usize>,
    threads: usize, // rayon outer
    enc_threads: Option<usize>, // per-encode (None = use single)
    append: bool,
    qm: bool,
    vaq: bool,
    vaq_strength: f64,
    tune_still: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut manifest = None;
        let mut output = PathBuf::from("./predictor_sweep.tsv");
        let mut speeds = (0u8..=10).collect::<Vec<_>>();
        let mut qualities = (5u8..=100).step_by(5).collect::<Vec<_>>();
        let mut sizes = vec![64, 256, 1024, 4096];
        let mut max_images = None;
        let mut threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let mut enc_threads = Some(1);
        let mut append = false;
        let mut qm = true;
        let mut vaq = false;
        let mut vaq_strength = 1.0;
        let mut tune_still = true;

        let raw: Vec<String> = env::args().collect();
        let bin = raw
            .first()
            .map(|s| Path::new(s).file_name().unwrap_or_default().to_string_lossy().into_owned())
            .unwrap_or_else(|| "predictor_sweep".into());

        let mut iter = raw.iter().skip(1);
        while let Some(a) = iter.next() {
            match a.as_str() {
                "-h" | "--help" => {
                    print_help(&bin);
                    std::process::exit(0);
                }
                "--manifest" => {
                    manifest = iter.next().map(PathBuf::from);
                }
                "--output" => {
                    output = iter.next().map(PathBuf::from).ok_or("--output PATH")?;
                }
                "--speeds" => {
                    speeds = parse_int_list_u8(iter.next().ok_or("--speeds")?)?;
                }
                "--qualities" => {
                    qualities = parse_int_list_u8(iter.next().ok_or("--qualities")?)?;
                }
                "--sizes" => {
                    sizes = parse_int_list_u32(iter.next().ok_or("--sizes")?)?;
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
                "--enc-threads" => {
                    let v: i64 = iter
                        .next()
                        .ok_or("--enc-threads N")?
                        .parse()
                        .map_err(|e| format!("--enc-threads: {e}"))?;
                    enc_threads = if v <= 0 { None } else { Some(v as usize) };
                }
                "--append" => append = true,
                "--qm" => {
                    qm = iter.next().ok_or("--qm BOOL")?.parse::<bool>().map_err(|e| e.to_string())?;
                }
                "--vaq" => {
                    vaq = iter.next().ok_or("--vaq BOOL")?.parse::<bool>().map_err(|e| e.to_string())?;
                }
                "--vaq-strength" => {
                    vaq_strength = iter
                        .next()
                        .ok_or("--vaq-strength F")?
                        .parse()
                        .map_err(|e| format!("--vaq-strength: {e}"))?;
                }
                "--tune-still" => {
                    tune_still = iter
                        .next()
                        .ok_or("--tune-still BOOL")?
                        .parse::<bool>()
                        .map_err(|e| e.to_string())?;
                }
                other => return Err(format!("unknown arg {other}")),
            }
        }

        let manifest = manifest.ok_or("--manifest PATH required")?;
        Ok(Args {
            manifest,
            output,
            speeds,
            qualities,
            sizes,
            max_images,
            threads,
            enc_threads,
            append,
            qm,
            vaq,
            vaq_strength,
            tune_still,
        })
    }
}

fn print_help(bin: &str) {
    eprintln!(
        "Multi-image AVIF predictor sweep — phase 1a baseline (speed × q × size).
Writes one TSV row per encode with provenance + bytes + zensim + encode_ms.

Usage: {bin} [options]
  --manifest PATH       Manifest TSV with `path` column (required)
  --output PATH         Output TSV  [default ./predictor_sweep.tsv]
  --speeds LIST         e.g. 0..=10  or  0,4,8  [default 0..=10]
  --qualities LIST      e.g. 5..=100:5  or  10,50,90  [default 5..=100:5]
  --sizes LIST          target maxdim list, e.g. 64,256,1024,4096
  --max-images N        cap manifest at first N images (after stratify)
  --threads N           rayon outer parallelism  [default num_cpus]
  --enc-threads N       per-encode threads (-1 = auto)  [default 1]
  --append              skip rows already in --output
  --qm BOOL             [default true]
  --vaq BOOL            [default false]
  --vaq-strength F      [default 1.0]
  --tune-still BOOL     [default true]
"
    );
}

fn parse_int_list_u8(s: &str) -> Result<Vec<u8>, String> {
    parse_int_list_u32(s).map(|v| v.into_iter().map(|x| x as u8).collect())
}

fn parse_int_list_u32(s: &str) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((range, step_str)) = part.split_once(':') {
            let (lo, hi, inclusive) = parse_range(range)?;
            let step: u32 = step_str.parse().map_err(|e| format!("bad step {step_str}: {e}"))?;
            let hi = if inclusive { hi } else { hi.saturating_sub(1) };
            let mut x = lo;
            while x <= hi {
                out.push(x);
                x += step;
            }
        } else if part.contains("..") {
            let (lo, hi, inclusive) = parse_range(part)?;
            let hi = if inclusive { hi } else { hi.saturating_sub(1) };
            for x in lo..=hi {
                out.push(x);
            }
        } else {
            let v: u32 = part.parse().map_err(|e| format!("bad int {part}: {e}"))?;
            out.push(v);
        }
    }
    Ok(out)
}

fn parse_range(s: &str) -> Result<(u32, u32, bool), String> {
    let (s, inclusive) = if let Some(stripped) = s.strip_prefix("..=").map(|s| ("0", s)) {
        // unreachable — strip_prefix returns Option, this branch is for "..=N" syntax
        let _ = stripped;
        (s, true)
    } else if let Some((a, b)) = s.split_once("..=") {
        let lo: u32 = a.parse().map_err(|e| format!("bad lo {a}: {e}"))?;
        let hi: u32 = b.parse().map_err(|e| format!("bad hi {b}: {e}"))?;
        return Ok((lo, hi, true));
    } else {
        (s, false)
    };
    if let Some((a, b)) = s.split_once("..") {
        let lo: u32 = a.parse().map_err(|e| format!("bad lo {a}: {e}"))?;
        let hi: u32 = b.parse().map_err(|e| format!("bad hi {b}: {e}"))?;
        Ok((lo, hi, inclusive))
    } else {
        Err(format!("bad range {s}"))
    }
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

fn rgb_from_dynamic(img: &DynamicImage) -> ImgVec<RGB8> {
    let (w, h) = img.dimensions();
    let rgb8 = img.to_rgb8();
    let buf: Vec<RGB8> = rgb8
        .pixels()
        .map(|p| RGB8::new(p[0], p[1], p[2]))
        .collect();
    ImgVec::new(buf, w as usize, h as usize)
}

/// Resize so max(w, h) == target_maxdim. Skips upscaling. Lanczos3.
fn resize_to_maxdim(src: &DynamicImage, target_maxdim: u32) -> Option<DynamicImage> {
    let (w, h) = src.dimensions();
    let cur_max = w.max(h);
    if cur_max <= target_maxdim {
        // No upscale: skip this size variant.
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

fn read_existing_keys(path: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Ok(f) = File::open(path) else {
        return out;
    };
    let r = BufReader::new(f);
    let mut header_idx: std::collections::HashMap<String, usize> = Default::default();
    for (i, line) in r.lines().enumerate() {
        let Ok(line) = line else { break };
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
        };
        let key = format!(
            "{}|{}|{}",
            get("sha256"),
            get("size_bucket"),
            get("config_id")
        );
        out.insert(key);
    }
    out
}

fn main() -> ExitCode {
    let args = match Args::parse() {
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

    // Stratify: keep all content classes proportionally if --max-images set
    if let Some(n) = args.max_images
        && manifest.len() > n
    {
        manifest = stratified_subset(&manifest, n);
    }

    eprintln!("manifest: {} images", manifest.len());
    eprintln!(
        "axes: speeds={:?} qualities={:?} sizes={:?}",
        args.speeds, args.qualities, args.sizes
    );

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok();
    }

    let existing_keys = if args.append && args.output.exists() {
        let k = read_existing_keys(&args.output);
        eprintln!("append mode: {} existing rows", k.len());
        k
    } else {
        Default::default()
    };

    let writing_header = !args.append || !args.output.exists();
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
            "sha256\tcontent_class\tsource\tw\th\tsize_bucket\tspeed\tq\tqm\tvaq\tvaq_strength\ttune_still\tconfig_id\tbytes\tzensim\tencode_ms\tdecode_ms"
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

    let total_attempts = std::sync::atomic::AtomicUsize::new(0);
    let total_done = std::sync::atomic::AtomicUsize::new(0);
    let total_skipped = std::sync::atomic::AtomicUsize::new(0);
    let total_failed = std::sync::atomic::AtomicUsize::new(0);

    pool.install(|| {
        manifest.par_iter().for_each(|entry| {
            let dyn_img = match ImageReader::open(&entry.path)
                .and_then(|r| Ok(r.decode()))
            {
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
                let img: ImgVec<RGB8> = rgb_from_dynamic(&resized);

                for &speed in &args.speeds {
                    for &q in &args.qualities {
                        let config_id = format!(
                            "s{}_q{}_qm{}_vaq{}_strength{:.1}_tune{}",
                            speed,
                            q,
                            args.qm as u8,
                            args.vaq as u8,
                            args.vaq_strength,
                            args.tune_still as u8
                        );
                        let key = format!("{}|{}|{}", entry.sha256, target, config_id);
                        if existing_keys.contains(&key) {
                            total_skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            continue;
                        }
                        total_attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                        let r = encode_one(
                            img.as_ref(),
                            speed,
                            q as f32,
                            args.qm,
                            args.vaq,
                            args.vaq_strength,
                            args.tune_still,
                            args.enc_threads,
                            &zensim,
                            &tol,
                        );
                        match r {
                            Ok(EncodeResult {
                                bytes,
                                zensim_score,
                                encode_ms,
                                decode_ms,
                            }) => {
                                let mut w = writer.lock().unwrap();
                                writeln!(
                                    w,
                                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.2}\t{}\t{}\t{}\t{:.6}\t{:.2}\t{:.2}",
                                    entry.sha256,
                                    entry.content_class,
                                    entry.source,
                                    img.width(),
                                    img.height(),
                                    target,
                                    speed,
                                    q,
                                    args.qm as u8,
                                    args.vaq as u8,
                                    args.vaq_strength,
                                    args.tune_still as u8,
                                    config_id,
                                    bytes,
                                    zensim_score,
                                    encode_ms,
                                    decode_ms,
                                )
                                .ok();
                                if total_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 100 == 0 {
                                    w.flush().ok();
                                    eprintln!(
                                        "[done={} attempts={} skipped={} failed={}]",
                                        total_done.load(std::sync::atomic::Ordering::Relaxed),
                                        total_attempts.load(std::sync::atomic::Ordering::Relaxed),
                                        total_skipped.load(std::sync::atomic::Ordering::Relaxed),
                                        total_failed.load(std::sync::atomic::Ordering::Relaxed),
                                    );
                                }
                            }
                            Err(e) => {
                                total_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                eprintln!("FAIL {}/{}/{}: {}", entry.sha256, target, config_id, e);
                            }
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
        "\nfinal: done={} attempts={} skipped={} failed={}",
        total_done.load(std::sync::atomic::Ordering::Relaxed),
        total_attempts.load(std::sync::atomic::Ordering::Relaxed),
        total_skipped.load(std::sync::atomic::Ordering::Relaxed),
        total_failed.load(std::sync::atomic::Ordering::Relaxed),
    );
    ExitCode::from(0)
}

struct EncodeResult {
    bytes: usize,
    zensim_score: f32,
    encode_ms: f64,
    decode_ms: f64,
}

fn encode_one(
    img: Img<&[RGB8]>,
    speed: u8,
    quality: f32,
    qm: bool,
    vaq: bool,
    vaq_strength: f64,
    tune_still: bool,
    enc_threads: Option<usize>,
    zensim: &Zensim,
    tol: &RegressionTolerance,
) -> Result<EncodeResult, String> {
    let mut enc = ravif::Encoder::new()
        .with_quality(quality)
        .with_speed(speed)
        .with_bit_depth(ravif::BitDepth::Eight)
        .with_qm(qm)
        .with_vaq(vaq, vaq_strength)
        .with_still_image_tuning(tune_still)
        .with_stop(StopToken::new(Unstoppable));
    if let Some(n) = enc_threads {
        enc = enc.with_num_threads(Some(n));
    }
    let t0 = Instant::now();
    let result = enc.encode_rgb(img).map_err(|e| format!("encode: {e}"))?;
    let encode_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();
    let cfg = zenavif::DecoderConfig::new().prefer_8bit(true);
    let decoded = zenavif::decode_with(&result.avif_file, &cfg, &Unstoppable)
        .map_err(|e| format!("decode: {e}"))?;
    let decode_ms = t1.elapsed().as_secs_f64() * 1000.0;

    let decoded_ref: ImgRef<'_, Rgb<u8>> = decoded
        .try_as_imgref::<Rgb<u8>>()
        .ok_or("decoded buffer not Rgb<u8>")?;
    let r = check_regression(zensim, &img, &decoded_ref, tol)
        .map_err(|e| format!("zensim: {e}"))?;
    let score = r.score();

    Ok(EncodeResult {
        bytes: result.avif_file.len(),
        zensim_score: score as f32,
        encode_ms,
        decode_ms,
    })
}

fn stratified_subset(entries: &[ManifestEntry], n: usize) -> Vec<ManifestEntry> {
    use std::collections::HashMap;
    let mut by_class: HashMap<String, Vec<&ManifestEntry>> = HashMap::new();
    for e in entries {
        by_class.entry(e.content_class.clone()).or_default().push(e);
    }
    let total = entries.len();
    let mut out = Vec::with_capacity(n);
    for (_class, mut list) in by_class {
        let take = ((list.len() as f64 / total as f64) * n as f64).round() as usize;
        let take = take.min(list.len()).max(1);
        // Deterministic: take first `take` by sha256 sort
        list.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        for e in list.into_iter().take(take) {
            out.push(e.clone());
        }
    }
    out.sort_by(|a, b| a.sha256.cmp(&b.sha256));
    out.truncate(n);
    out
}

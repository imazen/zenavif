//! zenanalyze feature extraction harness for the rav1e knob predictor.
//!
//! Reads a manifest TSV (same format as predictor_sweep.rs), resizes each
//! image to N target maxdim sizes via Lanczos3 (matching predictor_sweep
//! exactly so the picker training can join features to encode rows), runs
//! `analyze_features_rgb8` with `FeatureSet::SUPPORTED` (~100 features) +
//! `composites` + `experimental`, and writes one TSV row per (image,
//! size_bucket).
//!
//! Output schema matches what zenpicker/tools/train_hybrid.py expects:
//!   image_path  size_class  width  height  feat_<feature>...
//!
//! Plus extra zenavif-side columns for join with predictor_sweep output:
//!   sha256  content_class  source  size_bucket
//!
//! Phase 4a usage:
//!   cargo run --release --example extract_features \
//!     --features encode-imazen -- \
//!     --manifest ~/work/codec-corpus/picker-train/manifest.tsv \
//!     --output benchmarks/rav1e_phase1a_features_<DATE>.tsv \
//!     --sizes 64,256,1024,4096 \
//!     --max-images 50

use image::{DynamicImage, GenericImageView, ImageReader, imageops::FilterType};
use rayon::prelude::*;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use zenanalyze::analyze_features_rgb8;
use zenanalyze::feature::{AnalysisFeature, AnalysisQuery, FeatureSet, FeatureValue};

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
    append: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut manifest = None;
        let mut output = PathBuf::from("./extract_features.tsv");
        let mut sizes = vec![64, 256, 1024, 4096];
        let mut max_images = None;
        let mut threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let mut append = false;

        let raw: Vec<String> = env::args().collect();
        let mut iter = raw.iter().skip(1);
        while let Some(a) = iter.next() {
            match a.as_str() {
                "-h" | "--help" => {
                    eprintln!(
                        "Usage: extract_features --manifest PATH --output PATH \
                         [--sizes 64,256,...] [--max-images N] \
                         [--threads N] [--append]"
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
                "--append" => append = true,
                other => return Err(format!("unknown arg {other}")),
            }
        }

        let manifest = manifest.ok_or("--manifest PATH required")?;
        Ok(Args {
            manifest,
            output,
            sizes,
            max_images,
            threads,
            append,
        })
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

/// Map (w, h) → "tiny"/"small"/"medium"/"large" by pixel count.
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

fn feature_value_str(
    analysis: &zenanalyze::feature::AnalysisResults,
    f: AnalysisFeature,
) -> String {
    if let Some(v) = analysis.get_f32(f) {
        format!("{v:.6}")
    } else if let Some(v) = analysis.get(f) {
        match v {
            FeatureValue::F32(x) => format!("{x:.6}"),
            FeatureValue::U32(x) => format!("{x}"),
            FeatureValue::Bool(b) => format!("{}", b as u8),
            _ => String::new(),
        }
    } else {
        String::new()
    }
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
        out.insert(format!("{}|{}", get("sha256"), get("size_bucket")));
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

    if let Some(n) = args.max_images
        && manifest.len() > n
    {
        // Same stratification as predictor_sweep.
        manifest = stratified_subset(&manifest, n);
    }

    eprintln!("manifest: {} images, sizes: {:?}", manifest.len(), args.sizes);

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok();
    }

    let cols: Vec<AnalysisFeature> = FeatureSet::SUPPORTED.iter().collect();
    eprintln!("extracting {} features per (image, size)", cols.len());

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
        write!(
            w,
            "sha256\tcontent_class\tsource\timage_path\tsize_class\tsize_bucket\twidth\theight"
        )
        .unwrap();
        for c in &cols {
            write!(w, "\tfeat_{}", c.name()).unwrap();
        }
        writeln!(w).unwrap();
        w.flush().ok();
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(args.threads.max(1))
        .build()
        .expect("rayon pool");

    let query = AnalysisQuery::new(FeatureSet::SUPPORTED);
    let total_done = std::sync::atomic::AtomicUsize::new(0);
    let total_skipped = std::sync::atomic::AtomicUsize::new(0);
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
                let key = format!("{}|{}", entry.sha256, target);
                if existing_keys.contains(&key) {
                    total_skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    continue;
                }

                let Some(resized) = resize_to_maxdim(&dyn_img, target) else {
                    continue;
                };
                let (w, h) = resized.dimensions();
                let rgb8 = resized.to_rgb8();
                let rgb_bytes = rgb8.as_raw();

                let row = analyze_features_rgb8(rgb_bytes, w, h, &query);
                let size_class = size_class_label(w, h);

                let mut w_lock = writer.lock().unwrap();
                write!(
                    w_lock,
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    entry.sha256,
                    entry.content_class,
                    entry.source,
                    entry.path.display(),
                    size_class,
                    target,
                    w,
                    h
                )
                .ok();
                for c in &cols {
                    write!(w_lock, "\t{}", feature_value_str(&row, *c)).ok();
                }
                writeln!(w_lock).ok();
                if total_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 10 == 0 {
                    w_lock.flush().ok();
                    eprintln!(
                        "[done={} skipped={} failed={}]",
                        total_done.load(std::sync::atomic::Ordering::Relaxed),
                        total_skipped.load(std::sync::atomic::Ordering::Relaxed),
                        total_failed.load(std::sync::atomic::Ordering::Relaxed),
                    );
                }
            }
        });
    });

    let mut w = writer.lock().unwrap();
    w.flush().ok();
    drop(w);

    eprintln!(
        "\nfinal: done={} skipped={} failed={}",
        total_done.load(std::sync::atomic::Ordering::Relaxed),
        total_skipped.load(std::sync::atomic::Ordering::Relaxed),
        total_failed.load(std::sync::atomic::Ordering::Relaxed),
    );
    ExitCode::from(0)
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
        list.sort_by(|a, b| a.sha256.cmp(&b.sha256));
        for e in list.into_iter().take(take) {
            out.push(e.clone());
        }
    }
    out.sort_by(|a, b| a.sha256.cmp(&b.sha256));
    out.truncate(n);
    out
}

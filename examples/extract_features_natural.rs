//! Sidecar to `extract_features` for the multi-axis corpus
//! (`/mnt/v/output/codec-corpus-2026-05-01-multiaxis/manifest.tsv`).
//!
//! The standard `extract_features` example resamples each source image
//! to a fixed `--sizes` list via Lanczos3. The multi-axis corpus is the
//! opposite: each row is *already* sized to a chosen target (grayscale
//! 47x47, wide-aspect 2048x113, etc.). Resizing them again would lose
//! the very signal the corpus was built to surface.
//!
//! This example reads the new manifest schema:
//!   relative_path  bytes  width  height  axis_class  source  description
//!
//! decodes each image at its natural dimensions, runs
//! `analyze_features_rgb8` (FeatureSet::SUPPORTED + composites +
//! experimental, matching `extract_features` exactly), and writes the
//! same TSV columns produced by `extract_features` so the two outputs
//! can be `cat`-merged for downstream `correlation_cleanup.py` /
//! `train_hybrid.py` consumption.
//!
//! `image_path`  is `<corpus_root>/<relative_path>`.
//! `size_class`  is `tiny|small|medium|large` per pixel count.
//! `size_bucket` is `max(width, height)` (the natural maxdim).
//! `sha256`      is left blank — the new manifest doesn't carry one;
//!               correlation analysis doesn't need it.
//! `content_class` is the `axis_class` column.
//! `source`      is the `source` column.
//!
//! Usage:
//!   cargo run --release --example extract_features_natural \
//!     --features encode-imazen -- \
//!     --manifest /mnt/v/output/codec-corpus-2026-05-01-multiaxis/manifest.tsv \
//!     --corpus-root /mnt/v/output/codec-corpus-2026-05-01-multiaxis \
//!     --output benchmarks/zenavif_features_expanded_2026-05-02.tsv

use image::{GenericImageView, ImageReader};
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
    relative_path: String,
    axis_class: String,
    source: String,
    #[allow(dead_code)]
    width: u32,
    #[allow(dead_code)]
    height: u32,
}

#[derive(Clone, Debug)]
struct Args {
    manifest: PathBuf,
    corpus_root: PathBuf,
    output: PathBuf,
    threads: usize,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut manifest = None;
        let mut corpus_root = None;
        let mut output = PathBuf::from("./extract_features_natural.tsv");
        let mut threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);

        let raw: Vec<String> = env::args().collect();
        let mut iter = raw.iter().skip(1);
        while let Some(a) = iter.next() {
            match a.as_str() {
                "-h" | "--help" => {
                    eprintln!(
                        "Usage: extract_features_natural --manifest PATH \
                         --corpus-root PATH --output PATH [--threads N]"
                    );
                    std::process::exit(0);
                }
                "--manifest" => manifest = iter.next().map(PathBuf::from),
                "--corpus-root" => corpus_root = iter.next().map(PathBuf::from),
                "--output" => output = iter.next().map(PathBuf::from).ok_or("--output PATH")?,
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
            corpus_root: corpus_root.ok_or("--corpus-root PATH required")?,
            output,
            threads,
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
        let relative_path = get("relative_path");
        if relative_path.is_empty() {
            continue;
        }
        let width: u32 = get("width").parse().unwrap_or(0);
        let height: u32 = get("height").parse().unwrap_or(0);
        out.push(ManifestEntry {
            relative_path,
            axis_class: get("axis_class"),
            source: get("source"),
            width,
            height,
        });
    }
    Ok(out)
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

fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let manifest = match read_manifest(&args.manifest) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };

    eprintln!(
        "manifest: {} images, corpus_root: {}",
        manifest.len(),
        args.corpus_root.display()
    );

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok();
    }

    let cols: Vec<AnalysisFeature> = FeatureSet::SUPPORTED.iter().collect();
    eprintln!("extracting {} features per image", cols.len());

    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&args.output)
        .unwrap_or_else(|e| panic!("open {}: {e}", args.output.display()));
    let writer = Mutex::new(BufWriter::new(file));
    {
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
    let total_failed = std::sync::atomic::AtomicUsize::new(0);

    pool.install(|| {
        manifest.par_iter().for_each(|entry| {
            let full = args.corpus_root.join(&entry.relative_path);
            let dyn_img = match ImageReader::open(&full).map(|r| r.decode()) {
                Ok(Ok(img)) => img,
                _ => {
                    total_failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    eprintln!("skip (decode fail): {}", full.display());
                    return;
                }
            };

            let (w, h) = dyn_img.dimensions();
            let rgb8 = dyn_img.to_rgb8();
            let rgb_bytes = rgb8.as_raw();

            let row = analyze_features_rgb8(rgb_bytes, w, h, &query);
            let size_class = size_class_label(w, h);
            let size_bucket = w.max(h);

            let mut w_lock = writer.lock().unwrap();
            // sha256 left empty — manifest doesn't ship one; correlation
            // analysis joins on (image_path, size_bucket) instead.
            write!(
                w_lock,
                "\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                entry.axis_class, entry.source, entry.relative_path, size_class, size_bucket, w, h
            )
            .ok();
            for c in &cols {
                write!(w_lock, "\t{}", feature_value_str(&row, *c)).ok();
            }
            writeln!(w_lock).ok();
            if total_done
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .is_multiple_of(25)
            {
                w_lock.flush().ok();
                eprintln!(
                    "[done={} failed={}]",
                    total_done.load(std::sync::atomic::Ordering::Relaxed),
                    total_failed.load(std::sync::atomic::Ordering::Relaxed),
                );
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

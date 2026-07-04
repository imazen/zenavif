//! WEDGE-FINDER corpus materializer: crops/resizes of imazen-26 natives with
//! EXACTLY the conventions of zenanalyze's `extract_features_imazen26_crops`
//! (the code that built `/mnt/v/output/imazen-26-features/imazen26_features_2026-06-23.parquet`),
//! so every encode cell joins to its precomputed feature row:
//!
//!   - crop windows computed on the NATIVE image: `round(W·f) × round(H·f)`
//!     at center/tl/tr/bl/br (we materialize full + the 4 c50 quadrants);
//!   - then Lanczos3 `resize_exact` to `round(dim·target/maxdim)` (sRGB space,
//!     image crate 0.25.10 — same version the extractor locked), downscale-only
//!     (`target >= maxdim` ⇒ the `native` size_class);
//!   - features computed on `to_rgb8()` of the result — we save exactly that
//!     buffer as the encode PNG and recompute zenanalyze features on it for
//!     join verification.
//!
//! Grid (WEDGE-FINDER program, budgeted):
//!   - full crop at size classes {256, 512, 1024, 2048-or-native-if-smaller}
//!   - c50_{tl,tr,bl,br} at size class 1024 (or native crop when the crop's
//!     maxdim ≤ 1024) — local-content wedges at one size only.
//!
//! Usage:
//!   cargo run --release --example wedge_corpus -- \
//!     --picks /mnt/v/output/rd-gap-wedge-2026-07-03/picks_k16.json \
//!     --outdir /mnt/v/output/rd-gap-wedge-2026-07-03
//!
//! Outputs into --outdir:
//!   png/<stem>.<crop>.<sclass>.png    RGB8 encode corpus
//!   corpus_map.tsv                    file → origin/crop/size_class/w/h/family join map
//!   sample_wedge_all.tsv              harness TSV (image w h family), desc-pixel order
//!   sample_wedge_cpu0.tsv             full-crop {512, top-size} subset for the cpu0 arm
//!   verify_features.tsv               recomputed zenanalyze features per materialized file

use image::{DynamicImage, GenericImageView, ImageReader, imageops::FilterType};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use zenanalyze::analyze_features_rgb8;
use zenanalyze::feature::{AnalysisFeature, AnalysisQuery, FeatureSet, FeatureValue};

const FULL_SIZES: &[u32] = &[256, 512, 1024];
const TOP_SIZE: u32 = 2048;
const CROP_SIZE: u32 = 1024;

struct Pick {
    image_path: PathBuf,
    content_class: String,
    origin_id: String,
}

fn parse_picks(path: &Path) -> Result<Vec<Pick>, String> {
    // Tiny hand-rolled JSON field scraper (flat, known schema) — avoids a dep.
    let s = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut picks = Vec::new();
    let mut ip: Option<String> = None;
    let mut cc: Option<String> = None;
    let mut oid: Option<String> = None;
    for line in s.lines() {
        let l = line.trim();
        let grab = |l: &str, key: &str| -> Option<String> {
            let pat = format!("\"{key}\":");
            l.strip_prefix(&pat).map(|rest| {
                rest.trim()
                    .trim_end_matches(',')
                    .trim_matches('"')
                    .to_string()
            })
        };
        if let Some(v) = grab(l, "image_path") {
            ip = Some(v);
        } else if let Some(v) = grab(l, "content_class") {
            cc = Some(v);
        } else if let Some(v) = grab(l, "origin_id") {
            oid = Some(v);
        }
        if let (Some(i), Some(c), Some(o)) = (&ip, &cc, &oid) {
            picks.push(Pick {
                image_path: PathBuf::from(i),
                content_class: c.clone(),
                origin_id: o.clone(),
            });
            ip = None;
            cc = None;
            oid = None;
        }
    }
    if picks.is_empty() {
        return Err("no picks parsed".into());
    }
    Ok(picks)
}

/// EXACT mirror of extract_features_imazen26_crops::crop_variants (fraction 0.5),
/// filtered to full + the 4 quadrants.
fn crop_variants(src: &DynamicImage) -> Vec<(String, DynamicImage)> {
    let (w, h) = src.dimensions();
    let f = 0.5f64;
    let cw = ((w as f64 * f).round() as u32).clamp(1, w);
    let ch = ((h as f64 * f).round() as u32).clamp(1, h);
    let mut v = vec![("full".to_string(), src.clone())];
    let positions: [(&str, u32, u32); 4] = [
        ("tl", 0, 0),
        ("tr", w - cw, 0),
        ("bl", 0, h - ch),
        ("br", w - cw, h - ch),
    ];
    for (pos, x, y) in positions {
        v.push((format!("c50_{pos}"), src.crop_imm(x, y, cw, ch)));
    }
    v
}

/// EXACT mirror of extract_features_imazen26_crops::resize_to_maxdim.
fn resize_to_maxdim(src: &DynamicImage, target: u32) -> DynamicImage {
    let (w, h) = src.dimensions();
    if target == 0 || w.max(h) <= target {
        return src.clone();
    }
    let ratio = target as f64 / w.max(h) as f64;
    let nw = ((w as f64) * ratio).round().max(1.0) as u32;
    let nh = ((h as f64) * ratio).round().max(1.0) as u32;
    src.resize_exact(nw, nh, FilterType::Lanczos3)
}

fn feature_value_str(a: &zenanalyze::feature::AnalysisResults, f: AnalysisFeature) -> String {
    if let Some(v) = a.get_f32(f) {
        if v.is_nan() {
            return String::new();
        }
        return format!("{v:.6}");
    }
    match a.get(f) {
        Some(FeatureValue::F32(x)) if !x.is_nan() => format!("{x:.6}"),
        Some(FeatureValue::U32(x)) => format!("{x}"),
        Some(FeatureValue::Bool(b)) => format!("{}", b as u8),
        _ => String::new(),
    }
}

fn family_of(content_class: &str) -> String {
    content_class
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect()
}

fn main() -> ExitCode {
    let mut picks_path = None;
    let mut outdir = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--picks" => picks_path = args.next().map(PathBuf::from),
            "--outdir" => outdir = args.next().map(PathBuf::from),
            other => {
                eprintln!("unknown arg {other}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(picks_path), Some(outdir)) = (picks_path, outdir) else {
        eprintln!("--picks and --outdir required (see file header)");
        return ExitCode::from(2);
    };
    let picks = match parse_picks(&picks_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    eprintln!("{} picks", picks.len());
    let pngdir = outdir.join("png");
    fs::create_dir_all(&pngdir).expect("mkdir png/");

    let query = AnalysisQuery::new(FeatureSet::SUPPORTED);
    let cols: Vec<AnalysisFeature> = FeatureSet::SUPPORTED.iter().collect();

    let mut map = String::from(
        "file\torigin_path\torigin_id\tcontent_class\tfamily\tcrop_label\tsize_class\twidth\theight\n",
    );
    // (pixels, sample-row) for descending-pixel scheduling order
    let mut sample_rows: Vec<(u64, String)> = Vec::new();
    let mut cpu0_rows: Vec<(u64, String)> = Vec::new();
    let mut verify = String::from("file\torigin_path\tcrop_label\tsize_class\twidth\theight");
    for c in &cols {
        let _ = write!(verify, "\tfeat_{}", c.name());
    }
    verify.push('\n');

    for p in &picks {
        let img = match ImageReader::open(&p.image_path)
            .map_err(|e| e.to_string())
            .and_then(|r| r.decode().map_err(|e| e.to_string()))
        {
            Ok(i) => i,
            Err(e) => {
                eprintln!("DECODE FAIL {}: {e}", p.image_path.display());
                return ExitCode::from(1);
            }
        };
        let stem = p
            .image_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches(".png")
            .to_string();
        let family = family_of(&p.content_class);
        for (crop_label, variant) in crop_variants(&img) {
            let (vw, vh) = variant.dimensions();
            let vmax = vw.max(vh);
            // 0 encodes "native" (extractor convention)
            let targets: Vec<u32> = if crop_label == "full" {
                let mut t: Vec<u32> = FULL_SIZES.iter().copied().filter(|&t| t < vmax).collect();
                t.push(if TOP_SIZE < vmax { TOP_SIZE } else { 0 });
                t
            } else {
                vec![if CROP_SIZE < vmax { CROP_SIZE } else { 0 }]
            };
            for target in targets {
                let resized = resize_to_maxdim(&variant, target);
                let (rw, rh) = resized.dimensions();
                let rgb8 = resized.to_rgb8();
                let size_class = if target == 0 {
                    "native".to_string()
                } else {
                    target.to_string()
                };
                let ftag = if target == 0 {
                    "native".to_string()
                } else {
                    format!("s{target}")
                };
                let fname = format!("{stem}.{crop_label}.{ftag}.png");
                let fpath = pngdir.join(&fname);
                rgb8.save(&fpath).expect("save png");
                let _ = writeln!(
                    map,
                    "{fname}\t{}\t{}\t{}\t{family}\t{crop_label}\t{size_class}\t{rw}\t{rh}",
                    p.image_path.display(),
                    p.origin_id,
                    p.content_class
                );
                let px = rw as u64 * rh as u64;
                let srow = format!("{}\t{rw}\t{rh}\t{family}", fpath.display());
                sample_rows.push((px, srow.clone()));
                if crop_label == "full" {
                    let top_slot = (target == TOP_SIZE) || (target == 0);
                    if size_class == "512" || top_slot {
                        cpu0_rows.push((px, srow));
                    }
                }
                // join verification: recompute features on the exact saved buffer
                let row = analyze_features_rgb8(rgb8.as_raw(), rw, rh, &query);
                let _ = write!(
                    verify,
                    "{fname}\t{}\t{crop_label}\t{size_class}\t{rw}\t{rh}",
                    p.image_path.display()
                );
                for c in &cols {
                    let _ = write!(verify, "\t{}", feature_value_str(&row, *c));
                }
                verify.push('\n');
                eprintln!("  {fname} ({rw}x{rh})");
            }
        }
    }

    fs::write(outdir.join("corpus_map.tsv"), map).expect("write corpus_map");
    fs::write(outdir.join("verify_features.tsv"), verify).expect("write verify_features");
    for (name, mut rows) in [
        ("sample_wedge_all.tsv", sample_rows),
        ("sample_wedge_cpu0.tsv", cpu0_rows),
    ] {
        rows.sort_by_key(|r| std::cmp::Reverse(r.0));
        let f = File::create(outdir.join(name)).expect("create sample tsv");
        let mut w = BufWriter::new(f);
        writeln!(w, "image\tw\th\tfamily").unwrap();
        for (_, r) in &rows {
            writeln!(w, "{r}").unwrap();
        }
        w.flush().unwrap();
        eprintln!("wrote {name} ({} rows)", rows.len());
    }
    ExitCode::from(0)
}

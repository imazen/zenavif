use image::GenericImageView;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn find_avif_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_avif_files(&path));
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("avif")) {
                files.push(path);
            }
        }
    }
    files
}

#[derive(Default)]
struct Stats {
    total: u32,
    exact_match: u32,
    close_match: u32,
    minor_mismatch: u32,
    major_mismatch: u32,
    dimension_mismatch: u32,
    zenavif_fail: u32,
    libavif_missing: u32,
    sum_max_err: u64,
    sum_avg_err: f64,
    compared: u32,
}

fn compare_pixels(zenavif_rgb: &[u8], ref_rgb: &[u8], width: u32, height: u32) -> (f64, u8, f64, u64) {
    // Returns (psnr, max_error, avg_error, wrong_pixels)
    let total_pixels = (width as u64) * (height as u64);
    let total_samples = total_pixels * 3;
    let mut sum_sq_err: f64 = 0.0;
    let mut sum_abs_err: f64 = 0.0;
    let mut max_err: u8 = 0;
    let mut wrong_pixels: u64 = 0;

    let len = zenavif_rgb.len().min(ref_rgb.len());
    for i in (0..len).step_by(3) {
        if i + 2 >= len {
            break;
        }
        let mut pixel_wrong = false;
        for c in 0..3 {
            let a = zenavif_rgb[i + c] as i16;
            let b = ref_rgb[i + c] as i16;
            let diff = (a - b).unsigned_abs() as u8;
            if diff > 0 {
                pixel_wrong = true;
                sum_sq_err += (diff as f64) * (diff as f64);
                sum_abs_err += diff as f64;
                if diff > max_err {
                    max_err = diff;
                }
            }
        }
        if pixel_wrong {
            wrong_pixels += 1;
        }
    }

    let mse = sum_sq_err / total_samples as f64;
    let psnr = if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0_f64 * 255.0 / mse).log10()
    };
    let avg_err = sum_abs_err / total_samples as f64;

    (psnr, max_err, avg_err, wrong_pixels)
}

/// CPU feature level names and their corresponding flag masks (x86_64)
fn cpu_levels() -> Vec<(&'static str, u32)> {
    vec![
        // v3 = AVX2 + FMA + BMI etc (bits 0-3 set)
        ("v3-avx2", u32::MAX),
        // v2 = SSE4.2 (bits 0-2 set, AVX2 masked out)
        ("v2-sse4", 0b0111),
        // scalar = no SIMD
        ("scalar", 0),
    ]
}

fn run_comparison(
    files: &[PathBuf],
    input_dir: &Path,
    ref_dir: &Path,
    cpu_mask: u32,
    level_name: &str,
) -> (Stats, Vec<String>) {
    let config = zenavif::DecoderConfig::new()
        .threads(1)
        .cpu_flags_mask(cpu_mask);

    let mut stats = Stats::default();
    let mut mismatches: Vec<String> = Vec::new();
    let total = files.len();

    for (i, path) in files.iter().enumerate() {
        stats.total += 1;

        if (i + 1) % 100 == 0 || i + 1 == total {
            eprint!(
                "\r  [{level_name}] [{}/{}] exact={} close={} minor={} major={} fail={}  ",
                i + 1,
                total,
                stats.exact_match,
                stats.close_match,
                stats.minor_mismatch,
                stats.major_mismatch,
                stats.zenavif_fail,
            );
            std::io::stderr().flush().ok();
        }

        let stem = path.file_stem().unwrap_or_default();
        let ref_png = ref_dir.join(stem).with_extension("png");

        if !ref_png.exists() {
            stats.libavif_missing += 1;
            continue;
        }

        let data = match fs::read(path) {
            Ok(d) => d,
            Err(_) => {
                stats.zenavif_fail += 1;
                continue;
            }
        };

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            zenavif::decode_with(&data, &config, &zenavif::Unstoppable)
        }));

        let img = match result {
            Ok(Ok(img)) => img,
            _ => {
                stats.zenavif_fail += 1;
                continue;
            }
        };

        let (z_width, z_height, z_rgb) = match &img {
            zenavif::DecodedImage::Rgb8(buf) => {
                let w = buf.width() as u32;
                let h = buf.height() as u32;
                let mut rgb = Vec::with_capacity((w * h * 3) as usize);
                for row in buf.rows() {
                    for px in row {
                        rgb.push(px.r);
                        rgb.push(px.g);
                        rgb.push(px.b);
                    }
                }
                (w, h, rgb)
            }
            zenavif::DecodedImage::Rgba8(buf) => {
                let w = buf.width() as u32;
                let h = buf.height() as u32;
                let mut rgb = Vec::with_capacity((w * h * 3) as usize);
                for row in buf.rows() {
                    for px in row {
                        rgb.push(px.r);
                        rgb.push(px.g);
                        rgb.push(px.b);
                    }
                }
                (w, h, rgb)
            }
            _ => {
                stats.libavif_missing += 1;
                continue;
            }
        };

        let ref_img = match image::open(&ref_png) {
            Ok(img) => img,
            Err(_) => {
                stats.libavif_missing += 1;
                continue;
            }
        };
        let (r_width, r_height) = ref_img.dimensions();

        if z_width != r_width || z_height != r_height {
            stats.dimension_mismatch += 1;
            let rel = path.strip_prefix(input_dir).unwrap_or(path);
            mismatches.push(format!(
                "DIM\t{}\tzenavif={}x{}\tlibavif={}x{}",
                rel.display(), z_width, z_height, r_width, r_height
            ));
            continue;
        }

        let ref_rgb: Vec<u8> = ref_img.to_rgb8().into_raw();
        let (psnr, max_err, avg_err, wrong_pixels) =
            compare_pixels(&z_rgb, &ref_rgb, z_width, z_height);

        stats.compared += 1;
        stats.sum_max_err += max_err as u64;
        stats.sum_avg_err += avg_err;

        let rel = path.strip_prefix(input_dir).unwrap_or(path);
        if max_err == 0 {
            stats.exact_match += 1;
        } else if max_err <= 2 {
            stats.close_match += 1;
            mismatches.push(format!(
                "CLOSE\t{}\tmax_err={}\tavg_err={:.4}\twrong={}/{}\tpsnr={:.1}",
                rel.display(), max_err, avg_err, wrong_pixels,
                (z_width as u64) * (z_height as u64), psnr
            ));
        } else if max_err <= 10 {
            stats.minor_mismatch += 1;
            mismatches.push(format!(
                "MINOR\t{}\tmax_err={}\tavg_err={:.4}\twrong={}/{}\tpsnr={:.1}",
                rel.display(), max_err, avg_err, wrong_pixels,
                (z_width as u64) * (z_height as u64), psnr
            ));
        } else {
            stats.major_mismatch += 1;
            mismatches.push(format!(
                "MAJOR\t{}\tmax_err={}\tavg_err={:.4}\twrong={}/{}\tpsnr={:.1}",
                rel.display(), max_err, avg_err, wrong_pixels,
                (z_width as u64) * (z_height as u64), psnr
            ));
        }
    }
    eprintln!();

    (stats, mismatches)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Parse --level arg first (so it doesn't get consumed as positional)
    let level_filter = args.iter()
        .position(|a| a == "--level")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    // Collect positional args (skip program name and --level/value pairs)
    let positional: Vec<&str> = args.iter().skip(1)
        .filter(|a| *a != "--level")
        .filter(|a| level_filter.map_or(true, |lf| a.as_str() != lf))
        .map(|s| s.as_str())
        .collect();

    let input_dir = positional.first().map(|s| Path::new(*s))
        .unwrap_or(Path::new("/mnt/v/datasets/scraping/avif"));
    let ref_dir = positional.get(1).map(|s| Path::new(*s))
        .unwrap_or(Path::new("/mnt/v/output/zenavif/libavif-refs"));
    let report_dir = Path::new("/mnt/v/output/zenavif");

    let all_levels = cpu_levels();
    let levels: Vec<_> = match level_filter {
        Some("all") | None => all_levels,
        Some(name) => all_levels.into_iter()
            .filter(|(n, _)| n.starts_with(name))
            .collect(),
    };

    if levels.is_empty() {
        eprintln!("Unknown level. Available: v3, v2, scalar, all");
        std::process::exit(1);
    }

    let mut files = find_avif_files(input_dir);
    files.sort();
    println!("Found {} AVIF files in {}", files.len(), input_dir.display());
    println!("Reference dir: {}", ref_dir.display());
    println!("Testing {} CPU level(s): {}", levels.len(),
        levels.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));
    println!();

    let start = Instant::now();

    for (level_name, cpu_mask) in &levels {
        println!("=== Testing: {} (mask=0x{:x}) ===", level_name, cpu_mask);
        let level_start = Instant::now();

        let (stats, mismatches) = run_comparison(&files, input_dir, ref_dir, *cpu_mask, level_name);

        let elapsed = level_start.elapsed();
        println!("  Results ({:.1}s):", elapsed.as_secs_f64());
        println!("    Total:              {}", stats.total);
        println!("    Compared:           {}", stats.compared);
        println!("    Exact match:        {} ({:.1}%)", stats.exact_match,
            if stats.compared > 0 { stats.exact_match as f64 / stats.compared as f64 * 100.0 } else { 0.0 });
        println!("    Close (err<=2):     {} ({:.1}%)", stats.close_match,
            if stats.compared > 0 { stats.close_match as f64 / stats.compared as f64 * 100.0 } else { 0.0 });
        println!("    Minor (err<=10):    {} ({:.1}%)", stats.minor_mismatch,
            if stats.compared > 0 { stats.minor_mismatch as f64 / stats.compared as f64 * 100.0 } else { 0.0 });
        println!("    Major (err>10):     {} ({:.1}%)", stats.major_mismatch,
            if stats.compared > 0 { stats.major_mismatch as f64 / stats.compared as f64 * 100.0 } else { 0.0 });
        println!("    Dim mismatch:       {}", stats.dimension_mismatch);
        println!("    Decode fail:        {}", stats.zenavif_fail);
        println!("    No reference:       {}", stats.libavif_missing);
        if stats.compared > 0 {
            println!("    Avg max error:      {:.3}", stats.sum_max_err as f64 / stats.compared as f64);
            println!("    Avg pixel error:    {:.6}", stats.sum_avg_err / stats.compared as f64);
        }

        if !mismatches.is_empty() {
            let mut by_cat: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for m in &mismatches {
                let cat = m.split('\t').next().unwrap_or("?").to_string();
                by_cat.entry(cat).or_default().push(m.clone());
            }

            for (cat, items) in &by_cat {
                println!("    --- {cat} ({} files) ---", items.len());
                for item in items.iter().take(5) {
                    println!("      {item}");
                }
                if items.len() > 5 {
                    println!("      ... and {} more", items.len() - 5);
                }
            }

            // Write per-level report
            let report_path = report_dir.join(format!("comparison-{}.txt", level_name));
            if let Ok(mut report) = fs::File::create(&report_path) {
                for m in &mismatches {
                    writeln!(report, "{m}").ok();
                }
                println!("    Full report: {}", report_path.display());
            }
        }
        println!();
    }

    println!("Total time: {:.1}s", start.elapsed().as_secs_f64());
}

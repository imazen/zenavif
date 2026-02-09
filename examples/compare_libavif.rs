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
    close_match: u32, // max error <= 2
    minor_mismatch: u32, // max error <= 10
    major_mismatch: u32, // max error > 10
    dimension_mismatch: u32,
    zenavif_fail: u32,
    libavif_missing: u32,
}

fn compare_pixels(zenavif_rgb: &[u8], ref_rgb: &[u8], width: u32, height: u32) -> (f64, u8, u64) {
    // Returns (psnr, max_error, wrong_pixels)
    let total_pixels = (width as u64) * (height as u64);
    let total_samples = total_pixels * 3; // RGB
    let mut sum_sq_err: f64 = 0.0;
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

    (psnr, max_err, wrong_pixels)
}

fn main() {
    let input_dir = Path::new("/mnt/v/datasets/scraping/avif");
    let ref_dir = Path::new("/mnt/v/output/zenavif/libavif-refs");
    let report_path = Path::new("/mnt/v/output/zenavif/comparison-report.txt");

    let mut files = find_avif_files(input_dir);
    files.sort();
    let total = files.len();
    println!("Found {total} AVIF files to compare");

    let config = zenavif::DecoderConfig::new().threads(1);
    let mut stats = Stats::default();
    let mut mismatches: Vec<String> = Vec::new();

    let start = Instant::now();

    for (i, path) in files.iter().enumerate() {
        stats.total += 1;

        if (i + 1) % 100 == 0 || i + 1 == total {
            eprint!(
                "\r[{}/{}] exact={} close={} minor={} major={} dimm={} fail={} noref={}  ",
                i + 1,
                total,
                stats.exact_match,
                stats.close_match,
                stats.minor_mismatch,
                stats.major_mismatch,
                stats.dimension_mismatch,
                stats.zenavif_fail,
                stats.libavif_missing
            );
            std::io::stderr().flush().ok();
        }

        // Find corresponding reference PNG (refs are flat by filename)
        let rel = path.strip_prefix(input_dir).unwrap_or(path);
        let stem = path.file_stem().unwrap_or_default();
        let ref_png = ref_dir
            .join(stem)
            .with_extension("png");

        if !ref_png.exists() {
            stats.libavif_missing += 1;
            continue;
        }

        // Decode with zenavif
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

        // Extract RGB8 pixels from zenavif output
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
                // 16-bit: skip for now
                stats.libavif_missing += 1;
                continue;
            }
        };

        // Load reference PNG
        let ref_img = match image::open(&ref_png) {
            Ok(img) => img,
            Err(_) => {
                stats.libavif_missing += 1;
                continue;
            }
        };
        let (r_width, r_height) = ref_img.dimensions();

        // Check dimensions
        if z_width != r_width || z_height != r_height {
            stats.dimension_mismatch += 1;
            mismatches.push(format!(
                "DIM\t{}\tzenavif={}x{}\tlibavif={}x{}",
                rel.display(),
                z_width,
                z_height,
                r_width,
                r_height
            ));
            continue;
        }

        // Convert reference to RGB bytes
        let ref_rgb_img = ref_img.to_rgb8();
        let ref_rgb: Vec<u8> = ref_rgb_img.into_raw();

        // Compare
        let (psnr, max_err, wrong_pixels) = compare_pixels(&z_rgb, &ref_rgb, z_width, z_height);

        if max_err == 0 {
            stats.exact_match += 1;
        } else if max_err <= 2 {
            stats.close_match += 1;
            mismatches.push(format!(
                "CLOSE\t{}\tmax_err={}\twrong={}/{}\tpsnr={:.1}",
                rel.display(),
                max_err,
                wrong_pixels,
                (z_width as u64) * (z_height as u64),
                psnr
            ));
        } else if max_err <= 10 {
            stats.minor_mismatch += 1;
            mismatches.push(format!(
                "MINOR\t{}\tmax_err={}\twrong={}/{}\tpsnr={:.1}",
                rel.display(),
                max_err,
                wrong_pixels,
                (z_width as u64) * (z_height as u64),
                psnr
            ));
        } else {
            stats.major_mismatch += 1;
            mismatches.push(format!(
                "MAJOR\t{}\tmax_err={}\twrong={}/{}\tpsnr={:.1}",
                rel.display(),
                max_err,
                wrong_pixels,
                (z_width as u64) * (z_height as u64),
                psnr
            ));
        }
    }

    let elapsed = start.elapsed();
    eprintln!();

    println!();
    println!("=== Comparison Results ({:.1}s) ===", elapsed.as_secs_f64());
    println!("Total files:          {}", stats.total);
    println!("Exact match:          {} ({:.1}%)", stats.exact_match, stats.exact_match as f64 / stats.total as f64 * 100.0);
    println!("Close (max err <=2):  {} ({:.1}%)", stats.close_match, stats.close_match as f64 / stats.total as f64 * 100.0);
    println!("Minor (max err <=10): {} ({:.1}%)", stats.minor_mismatch, stats.minor_mismatch as f64 / stats.total as f64 * 100.0);
    println!("Major (max err >10):  {} ({:.1}%)", stats.major_mismatch, stats.major_mismatch as f64 / stats.total as f64 * 100.0);
    println!("Dimension mismatch:   {}", stats.dimension_mismatch);
    println!("Zenavif decode fail:  {}", stats.zenavif_fail);
    println!("Libavif ref missing:  {}", stats.libavif_missing);

    if !mismatches.is_empty() {
        // Group by category
        let mut by_cat: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for m in &mismatches {
            let cat = m.split('\t').next().unwrap_or("?").to_string();
            by_cat.entry(cat).or_default().push(m.clone());
        }

        println!();
        for (cat, items) in &by_cat {
            println!("--- {cat} ({} files) ---", items.len());
            for item in items.iter().take(10) {
                println!("  {item}");
            }
            if items.len() > 10 {
                println!("  ... and {} more", items.len() - 10);
            }
        }

        // Write full report
        let mut report = fs::File::create(report_path).expect("create report");
        for m in &mismatches {
            writeln!(report, "{m}").expect("write report");
        }
        println!("\nFull report: {}", report_path.display());
    }
}

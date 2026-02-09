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

fn main() {
    let input_dir = Path::new("/mnt/v/datasets/scraping/avif");
    let fail_dir = Path::new("/mnt/v/output/zenavif/parse-failures");

    let mut files = find_avif_files(input_dir);
    files.sort();

    let total = files.len();
    println!("Found {total} AVIF files");

    let config = zenavif::DecoderConfig::new().threads(1);
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut errors: Vec<(PathBuf, String)> = Vec::new();

    let start = Instant::now();

    for (i, path) in files.iter().enumerate() {
        if (i + 1) % 100 == 0 || i + 1 == total {
            eprint!("\r[{}/{}] passed={passed} failed={failed}  ", i + 1, total);
            std::io::stderr().flush().ok();
        }

        let data = match fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                failed += 1;
                errors.push((path.clone(), format!("read error: {e}")));
                copy_failure(path, input_dir, fail_dir);
                continue;
            }
        };

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            zenavif::decode_with(&data, &config, &zenavif::Unstoppable)
        }));

        match result {
            Ok(Ok(_img)) => {
                passed += 1;
            }
            Ok(Err(e)) => {
                failed += 1;
                errors.push((path.clone(), format!("{e}")));
                copy_failure(path, input_dir, fail_dir);
            }
            Err(panic_info) => {
                failed += 1;
                let err_str = if let Some(s) = panic_info.downcast_ref::<String>() {
                    format!("PANIC: {s}")
                } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                    format!("PANIC: {s}")
                } else {
                    "PANIC: unknown".to_string()
                };
                errors.push((path.clone(), err_str));
                copy_failure(path, input_dir, fail_dir);
            }
        }
    }

    let elapsed = start.elapsed();
    eprintln!();
    println!();
    println!("=== Results ===");
    println!("Total:  {total}");
    println!("Passed: {passed} ({:.1}%)", passed as f64 / total as f64 * 100.0);
    println!("Failed: {failed} ({:.1}%)", failed as f64 / total as f64 * 100.0);
    println!("Time:   {:.1}s", elapsed.as_secs_f64());
    println!();

    if !errors.is_empty() {
        // Group errors by error message
        let mut by_error: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for (path, err) in &errors {
            let filename = path
                .strip_prefix(input_dir)
                .unwrap_or(path)
                .display()
                .to_string();
            by_error.entry(err.clone()).or_default().push(filename);
        }

        println!("=== Errors by category ===");
        for (err, files) in &by_error {
            println!("\n[{}] ({} files)", err, files.len());
            for f in files.iter().take(5) {
                println!("  {f}");
            }
            if files.len() > 5 {
                println!("  ... and {} more", files.len() - 5);
            }
        }

        // Write full error log
        let log_path = fail_dir.join("errors.txt");
        let mut log = fs::File::create(&log_path).expect("create error log");
        for (path, err) in &errors {
            writeln!(log, "{}\t{err}", path.display()).expect("write error log");
        }
        println!("\nFull error log: {}", log_path.display());
    }

    println!("Failed files copied to: {}", fail_dir.display());
}

fn copy_failure(path: &Path, input_dir: &Path, fail_dir: &Path) {
    let rel = path.strip_prefix(input_dir).unwrap_or(path);
    let dest = fail_dir.join(rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::copy(path, &dest).ok();
}

use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::time::Instant;
use zenpixels::PixelDescriptor;

fn zenavif_output_dir() -> String {
    std::env::var("ZENAVIF_OUTPUT_DIR").unwrap_or_else(|_| "/mnt/v/output/zenavif".into())
}

fn main() {
    let error_log_str = format!("{}/parse-failures/errors.txt", zenavif_output_dir());
    let error_log = Path::new(&error_log_str);
    let data = fs::read_to_string(error_log).expect("read error log");

    let paths: Vec<&str> = data.lines().filter_map(|l| l.split('\t').next()).collect();
    let total = paths.len();
    println!("Retrying {total} previously failed files");

    let config = zenavif::DecoderConfig::new().threads(1);
    let mut passed = 0u32;
    let mut failed = 0u32;

    let start = Instant::now();

    for path_str in paths.iter() {
        let path = Path::new(path_str);
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        let data = match fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                failed += 1;
                println!("  FAIL {name}: read error: {e}");
                continue;
            }
        };

        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            zenavif::decode_with(&data, &config, &zenavif::Unstoppable)
        }));

        match result {
            Ok(Ok(img)) => {
                passed += 1;
                let desc = img.descriptor();
                let dims = if desc.layout_compatible(PixelDescriptor::RGB8) {
                    format!("{}x{} rgb8", img.width(), img.height())
                } else if desc.layout_compatible(PixelDescriptor::RGBA8) {
                    format!("{}x{} rgba8", img.width(), img.height())
                } else if desc.layout_compatible(PixelDescriptor::RGB16) {
                    format!("{}x{} rgb16", img.width(), img.height())
                } else if desc.layout_compatible(PixelDescriptor::RGBA16) {
                    format!("{}x{} rgba16", img.width(), img.height())
                } else {
                    format!("{}x{} {:?}", img.width(), img.height(), desc)
                };
                println!("  OK   {name}: {dims}");
            }
            Ok(Err(e)) => {
                failed += 1;
                println!("  FAIL {name}: {e}");
            }
            Err(panic_info) => {
                failed += 1;
                let err = if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    "unknown panic".to_string()
                };
                println!("  PANIC {name}: {err}");
            }
        }
    }

    let elapsed = start.elapsed();
    println!();
    println!("=== Retry Results ===");
    println!("Total:  {total}");
    println!("Passed: {passed}");
    println!("Failed: {failed}");
    println!("Time:   {:.1}s", elapsed.as_secs_f64());
}

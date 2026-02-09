use std::fs;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::time::Instant;

fn main() {
    let error_log = Path::new("/mnt/v/output/zenavif/parse-failures/errors.txt");
    let data = fs::read_to_string(error_log).expect("read error log");

    let paths: Vec<&str> = data.lines().filter_map(|l| l.split('\t').next()).collect();
    let total = paths.len();
    println!("Retrying {total} previously failed files");

    let config = zenavif::DecoderConfig::new().threads(1);
    let mut passed = 0u32;
    let mut failed = 0u32;

    let start = Instant::now();

    for (i, path_str) in paths.iter().enumerate() {
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
                let dims = match &img {
                    zenavif::DecodedImage::Rgb8(i) => format!("{}x{} rgb8", i.width(), i.height()),
                    zenavif::DecodedImage::Rgba8(i) => {
                        format!("{}x{} rgba8", i.width(), i.height())
                    }
                    zenavif::DecodedImage::Rgb16(i) => {
                        format!("{}x{} rgb16", i.width(), i.height())
                    }
                    zenavif::DecodedImage::Rgba16(i) => {
                        format!("{}x{} rgba16", i.width(), i.height())
                    }
                    other => format!("{:?}", std::mem::discriminant(other)),
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

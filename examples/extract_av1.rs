//! Extract raw AV1 OBU payloads from AVIF files for rav1d-safe testing
//!
//! Usage: cargo run --features managed --example extract_av1 -- <avif-file> <output-dir>

use enough::Unstoppable;
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <avif-file-or-dir> <output-dir>", args[0]);
        std::process::exit(1);
    }

    let input = Path::new(&args[1]);
    let output_dir = Path::new(&args[2]);
    fs::create_dir_all(output_dir).expect("Failed to create output dir");

    let files: Vec<_> = if input.is_dir() {
        fs::read_dir(input)
            .expect("Failed to read dir")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("avif"))
            .collect()
    } else {
        vec![input.to_path_buf()]
    };

    for path in &files {
        let stem = path.file_stem().unwrap().to_string_lossy();
        let data = fs::read(path).expect("Failed to read file");

        let config = zenavif_parse::DecodeConfig::default().lenient(true);
        let parser =
            match zenavif_parse::AvifParser::from_owned_with_config(data, &config, &Unstoppable) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("  SKIP {}: parse error: {}", stem, e);
                    continue;
                }
            };

        // Extract primary AV1 data
        match parser.primary_data() {
            Ok(primary) => {
                let out_path = output_dir.join(format!("{}.obu", stem));
                fs::write(&out_path, primary.as_ref()).expect("Failed to write");
                eprintln!(
                    "  OK   {} -> {} ({} bytes)",
                    stem,
                    out_path.display(),
                    primary.len()
                );
            }
            Err(e) => {
                eprintln!("  SKIP {}: primary data error: {}", stem, e);
            }
        }
    }
}

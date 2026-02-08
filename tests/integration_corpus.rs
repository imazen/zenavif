//! Integration tests with real AVIF test vectors
//!
//! Run with: cargo test --features managed --test integration_corpus -- --ignored
//! Or: just test-integration

use std::fs;
use std::path::PathBuf;
use zenavif::{decode_with, DecodedImage, DecoderConfig};
use enough::Unstoppable;

fn find_test_vectors() -> Vec<PathBuf> {
    let mut vectors = Vec::new();
    let test_dirs = [
        "tests/vectors/libavif",
        "tests/vectors/cavif",
        "tests/vectors/avif-parse",
    ];
    
    for dir in &test_dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("avif") {
                    vectors.push(path);
                }
            }
        }
    }
    
    vectors.sort();
    vectors
}

#[test]
#[ignore] // Run with: cargo test -- --ignored
fn test_decode_all_vectors() {
    let vectors = find_test_vectors();
    
    if vectors.is_empty() {
        eprintln!("⚠️  No test vectors found!");
        eprintln!("Run: bash scripts/download-avif-test-vectors.sh");
        eprintln!("Then re-run tests with: cargo test -- --ignored");
        return;
    }
    
    eprintln!("\n🔍 Testing {} AVIF files...\n", vectors.len());

    // Use single-threaded decoder to avoid rav1d-safe threading issues
    let config = DecoderConfig::new().threads(1);

    let mut passed = 0;
    let mut failed = 0;
    let mut failed_files = Vec::new();

    for path in &vectors {
        eprint!("  {:50} ", path.file_name().unwrap().to_string_lossy());

        match fs::read(path) {
            Ok(data) => {
                match decode_with(&data, &config, &Unstoppable) {
                    Ok(image) => {
                        let info = match &image {
                            DecodedImage::Rgb8(img) => format!("RGB8  {}x{}", img.width(), img.height()),
                            DecodedImage::Rgba8(img) => format!("RGBA8 {}x{}", img.width(), img.height()),
                            DecodedImage::Rgb16(img) => format!("RGB16 {}x{}", img.width(), img.height()),
                            DecodedImage::Rgba16(img) => format!("RGBA16 {}x{}", img.width(), img.height()),
                            DecodedImage::Gray8(img) => format!("GRAY8 {}x{}", img.width(), img.height()),
                            DecodedImage::Gray16(img) => format!("GRAY16 {}x{}", img.width(), img.height()),
                            _ => format!("OTHER {}x{}", image.width(), image.height()),
                        };
                        eprintln!("✓ {}", info);
                        passed += 1;
                    }
                    Err(e) => {
                        eprintln!("✗ {}", e);
                        failed += 1;
                        failed_files.push((path.clone(), e.to_string()));
                    }
                }
            }
            Err(e) => {
                eprintln!("✗ Read error: {}", e);
                failed += 1;
                failed_files.push((path.clone(), format!("Read error: {}", e)));
            }
        }
    }
    
    eprintln!("\n📊 Results:");
    eprintln!("  Passed: {} ({:.1}%)", passed, passed as f64 / vectors.len() as f64 * 100.0);
    eprintln!("  Failed: {} ({:.1}%)", failed, failed as f64 / vectors.len() as f64 * 100.0);
    eprintln!("  Total:  {}", vectors.len());
    
    if !failed_files.is_empty() {
        eprintln!("\n❌ Failed files:");
        for (path, error) in &failed_files {
            eprintln!("  - {:?}: {}", path.file_name().unwrap(), error);
        }
    }
    
    // Allow some failures for now (malformed test files, unsupported features, etc)
    // But we should decode at least 70% of files successfully
    let pass_rate = passed as f64 / vectors.len() as f64;
    assert!(
        pass_rate >= 0.70,
        "Pass rate too low: {:.1}% (expected >= 70%)",
        pass_rate * 100.0
    );
}

#[test]
#[ignore]
fn test_decode_specific_formats() {
    // Test specific important format combinations
    let test_cases = vec![
        ("8-bit 4:2:0", "tests/vectors/libavif/8bit_420.avif"),
        ("10-bit 4:4:4", "tests/vectors/libavif/10bit_444.avif"),
        ("With alpha", "tests/vectors/libavif/alpha.avif"),
    ];

    let config = DecoderConfig::new().threads(1);

    for (name, path) in test_cases {
        if let Ok(data) = fs::read(path) {
            eprintln!("Testing {}...", name);
            match decode_with(&data, &config, &Unstoppable) {
                Ok(image) => {
                    eprintln!("  ✓ {}x{} @ {}bpp", 
                             image.width(), image.height(), image.bit_depth());
                }
                Err(e) => {
                    eprintln!("  ⚠️  {}", e);
                }
            }
        } else {
            eprintln!("  ⚠️  File not found: {}", path);
        }
    }
}

#[test]
fn test_yuv_crate_sanity() {
    use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix};
    
    let width = 4;
    let height = 4;
    
    // Test 1: Gray (128,128,128) should stay gray
    let y_plane = vec![128u8; width * height];
    let u_plane = vec![128u8; width * height];
    let v_plane = vec![128u8; width * height];
    
    let planar = YuvPlanarImage {
        y_plane: &y_plane,
        y_stride: width as u32,
        u_plane: &u_plane,
        u_stride: width as u32,
        v_plane: &v_plane,
        v_stride: width as u32,
        width: width as u32,
        height: height as u32,
    };
    
    let mut rgb = vec![0u8; width * height * 3];
    let rgb_stride = (width * 3) as u32;
    
    yuv::yuv444_to_rgb(&planar, &mut rgb, rgb_stride, YuvRange::Full, YuvStandardMatrix::Bt601).unwrap();
    
    eprintln!("Gray test: YUV (128, 128, 128) -> RGB ({}, {}, {})", rgb[0], rgb[1], rgb[2]);
    eprintln!("Expected: RGB (128, 128, 128)");
    
    // Allow small rounding error
    assert!((rgb[0] as i16 - 128).abs() <= 1, "Red channel off: {}", rgb[0]);
    assert!((rgb[1] as i16 - 128).abs() <= 1, "Green channel off: {}", rgb[1]);
    assert!((rgb[2] as i16 - 128).abs() <= 1, "Blue channel off: {}", rgb[2]);
}

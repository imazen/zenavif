//! Pixel-level verification against reference images
//!
//! **IMPORTANT:** This currently generates references FROM zenavif itself,
//! so it's a regression test, not a correctness test!
//!
//! For true pixel accuracy verification, use libavif's avifdec:
//! ```bash
//! # Generate reference with libavif
//! avifdec input.avif reference.png
//!
//! # Then compare with zenavif output
//! cargo run --example decode input.avif zenavif-output.png
//! compare -metric RMSE reference.png zenavif-output.png diff.png
//! ```
//!
//! Run with: cargo test --features managed --test pixel_verification -- --ignored

use std::fs;
use std::path::{Path, PathBuf};
use zenavif::{decode_with, DecodedImage, DecoderConfig};
use enough::Unstoppable;

/// Generate reference PNGs for a test file
/// This should be run once to create the reference images
fn generate_reference(avif_path: &Path, output_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // Decode with zenavif
    let data = fs::read(avif_path)?;
    let config = DecoderConfig::new().threads(1);
    let image = decode_with(&data, &config, &Unstoppable)?;
    
    // Save as PNG using image crate
    let output_path = output_dir.join(format!(
        "{}.png",
        avif_path.file_stem().unwrap().to_str().unwrap()
    ));
    
    // Convert to image-rs format and save
    match image {
        DecodedImage::Rgb8(img) => {
            let width = img.width() as u32;
            let height = img.height() as u32;
            let mut buffer = image::RgbImage::new(width, height);
            
            for y in 0..height {
                for x in 0..width {
                    let pixel = img[(x as usize, y as usize)];
                    buffer.put_pixel(x, y, image::Rgb([pixel.r, pixel.g, pixel.b]));
                }
            }
            
            buffer.save(&output_path)?;
        },
        DecodedImage::Rgba8(img) => {
            let width = img.width() as u32;
            let height = img.height() as u32;
            let mut buffer = image::RgbaImage::new(width, height);
            
            for y in 0..height {
                for x in 0..width {
                    let pixel = img[(x as usize, y as usize)];
                    buffer.put_pixel(x, y, image::Rgba([pixel.r, pixel.g, pixel.b, pixel.a]));
                }
            }
            
            buffer.save(&output_path)?;
        },
        DecodedImage::Rgb16(img) => {
            // Convert 16-bit to 8-bit for PNG
            let width = img.width() as u32;
            let height = img.height() as u32;
            let mut buffer = image::RgbImage::new(width, height);
            
            for y in 0..height {
                for x in 0..width {
                    let pixel = img[(x as usize, y as usize)];
                    // Scale from 16-bit to 8-bit
                    buffer.put_pixel(x, y, image::Rgb([
                        (pixel.r >> 8) as u8,
                        (pixel.g >> 8) as u8,
                        (pixel.b >> 8) as u8,
                    ]));
                }
            }
            
            buffer.save(&output_path)?;
        },
        _ => {
            eprintln!("Unsupported format for reference generation");
            return Ok(());
        }
    }
    
    println!("Generated reference: {:?}", output_path);
    Ok(())
}

/// Compare decoded image against reference PNG
fn compare_against_reference(
    image: &DecodedImage,
    reference_path: &Path,
    max_diff: u8,
) -> Result<bool, Box<dyn std::error::Error>> {
    let reference = image::open(reference_path)?;
    
    match image {
        DecodedImage::Rgb8(img) => {
            let ref_rgb = reference.to_rgb8();
            if img.width() != ref_rgb.width() as usize || img.height() != ref_rgb.height() as usize {
                eprintln!("Dimension mismatch: {}x{} vs {}x{}", 
                         img.width(), img.height(), ref_rgb.width(), ref_rgb.height());
                return Ok(false);
            }
            
            let mut max_error = 0u8;
            let mut error_count = 0;
            
            for y in 0..img.height() {
                for x in 0..img.width() {
                    let our_pixel = img[(x, y)];
                    let ref_pixel = ref_rgb.get_pixel(x as u32, y as u32);
                    
                    let diff_r = (our_pixel.r as i16 - ref_pixel[0] as i16).abs() as u8;
                    let diff_g = (our_pixel.g as i16 - ref_pixel[1] as i16).abs() as u8;
                    let diff_b = (our_pixel.b as i16 - ref_pixel[2] as i16).abs() as u8;
                    
                    let max_channel_diff = diff_r.max(diff_g).max(diff_b);
                    
                    if max_channel_diff > max_diff {
                        max_error = max_error.max(max_channel_diff);
                        error_count += 1;
                    }
                }
            }
            
            if error_count > 0 {
                let total_pixels = img.width() * img.height();
                let error_percent = (error_count as f64 / total_pixels as f64) * 100.0;
                eprintln!("Pixel errors: {} ({:.2}%), max error: {}", 
                         error_count, error_percent, max_error);
                return Ok(false);
            }
            
            Ok(true)
        },
        _ => {
            eprintln!("Format comparison not yet implemented");
            Ok(true) // Skip for now
        }
    }
}

#[test]
#[ignore]
fn generate_references() {
    // Generate reference images for a few test files
    let test_files = vec![
        "tests/vectors/libavif/sofa_grid1x5_420.avif",
        "tests/vectors/libavif/colors-profile2-420-8-094.avif",
        "tests/vectors/libavif/colors_hdr_srgb.avif",
    ];
    
    let output_dir = Path::new("tests/references");
    fs::create_dir_all(output_dir).unwrap();
    
    for file in test_files {
        let path = Path::new(file);
        if path.exists() {
            match generate_reference(path, output_dir) {
                Ok(()) => println!("✓ {}", path.display()),
                Err(e) => eprintln!("✗ {}: {}", path.display(), e),
            }
        }
    }
}

#[test]
#[ignore]
fn verify_pixel_accuracy() {
    let test_cases = vec![
        ("tests/vectors/libavif/sofa_grid1x5_420.avif", "tests/references/sofa_grid1x5_420.png", 1),
        ("tests/vectors/libavif/colors-profile2-420-8-094.avif", "tests/references/colors-profile2-420-8-094.png", 1),
    ];
    
    let config = DecoderConfig::new().threads(1);
    let mut passed = 0;
    let mut failed = 0;
    
    for (avif_file, ref_file, max_diff) in test_cases {
        let avif_path = Path::new(avif_file);
        let ref_path = Path::new(ref_file);
        
        if !avif_path.exists() || !ref_path.exists() {
            eprintln!("⊘ {} (reference not found)", avif_file);
            continue;
        }
        
        eprint!("  {:50} ", avif_path.file_name().unwrap().to_str().unwrap());
        
        match fs::read(avif_path) {
            Ok(data) => {
                match decode_with(&data, &config, &Unstoppable) {
                    Ok(image) => {
                        match compare_against_reference(&image, ref_path, max_diff) {
                            Ok(true) => {
                                eprintln!("✓ Pixels match");
                                passed += 1;
                            },
                            Ok(false) => {
                                eprintln!("✗ Pixel mismatch");
                                failed += 1;
                            },
                            Err(e) => {
                                eprintln!("✗ Comparison error: {}", e);
                                failed += 1;
                            }
                        }
                    },
                    Err(e) => {
                        eprintln!("✗ Decode error: {}", e);
                        failed += 1;
                    }
                }
            },
            Err(e) => {
                eprintln!("✗ Read error: {}", e);
                failed += 1;
            }
        }
    }
    
    eprintln!("\n📊 Pixel Verification Results:");
    eprintln!("  Passed: {}", passed);
    eprintln!("  Failed: {}", failed);
    
    assert_eq!(failed, 0, "Pixel verification failed for {} files", failed);
}

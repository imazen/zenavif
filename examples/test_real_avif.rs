//! Test decoder with real AVIF file

use zenavif::{decode, DecodedImage};
use std::fs;

fn main() {
    let test_file = "tests/vectors/libavif/kodim03_yuv420_8bpc.avif";
    
    if !std::path::Path::new(test_file).exists() {
        println!("Test file not found: {}", test_file);
        return;
    }
    
    let data = fs::read(test_file).expect("Failed to read test file");
    
    match decode(&data) {
        Ok(image) => {
            println!("✓ Successfully decoded {}", test_file);
            
            match &image {
                DecodedImage::Rgb8(img) => {
                    println!("  Size: {}x{}", img.width(), img.height());
                    let first = img.buf()[0];
                    println!("  First pixel: R={}, G={}, B={}", first.r, first.g, first.b);
                },
                DecodedImage::Rgba8(img) => {
                    println!("  Size: {}x{}", img.width(), img.height());
                    let first = img.buf()[0];
                    println!("  First pixel: R={}, G={}, B={}, A={}", 
                             first.r, first.g, first.b, first.a);
                },
                _ => {
                    println!("  Format: 16-bit");
                }
            }
        },
        Err(e) => {
            println!("✗ Failed to decode: {:?}", e);
        }
    }
}

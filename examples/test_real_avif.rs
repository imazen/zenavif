//! Test decoder with real AVIF file

#[path = "../tests/support/pixels.rs"]
mod pixel_support;
#[allow(unused_imports)]
use pixel_support::{BufTestExt as _, SpecTestExt as _};
use std::fs;
use zenavif::decode;

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

            let desc = image.format();
            if desc.layout == zenpixels::SampleLayout::RGB8 {
                let img = image.as_view().as_imgref::<rgb::Rgb<u8>>().unwrap();
                println!("  Size: {}x{}", img.width(), img.height());
                let first = img.buf()[0];
                println!("  First pixel: R={}, G={}, B={}", first.r, first.g, first.b);
            } else if desc.layout == zenpixels::SampleLayout::RGBA8 {
                let img = image.as_view().as_imgref::<rgb::Rgba<u8>>().unwrap();
                println!("  Size: {}x{}", img.width(), img.height());
                let first = img.buf()[0];
                println!(
                    "  First pixel: R={}, G={}, B={}, A={}",
                    first.r, first.g, first.b, first.a
                );
            } else {
                println!("  Format: 16-bit");
            }
        }
        Err(e) => {
            println!("✗ Failed to decode: {:?}", e);
        }
    }
}

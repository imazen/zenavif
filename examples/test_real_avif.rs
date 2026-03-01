//! Test decoder with real AVIF file

use std::fs;
use zenavif::decode;
use zencodec_types::PixelDescriptor;

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

            let desc = image.descriptor();
            if desc.layout_compatible(&PixelDescriptor::RGB8) {
                let img = image.try_as_imgref::<rgb::Rgb<u8>>().unwrap();
                println!("  Size: {}x{}", img.width(), img.height());
                let first = img.buf()[0];
                println!("  First pixel: R={}, G={}, B={}", first.r, first.g, first.b);
            } else if desc.layout_compatible(&PixelDescriptor::RGBA8) {
                let img = image.try_as_imgref::<rgb::Rgba<u8>>().unwrap();
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

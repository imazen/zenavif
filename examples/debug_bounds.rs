//! Debug example to investigate bounds check panic in rav1d-safe

use enough::Unstoppable;
use zenavif::{DecoderConfig, decode_with};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = "tests/vectors/libavif/color_nogrid_alpha_nogrid_gainmap_grid.avif";
    let data = std::fs::read(path)?;

    println!("File size: {} bytes", data.len());
    println!("Attempting to decode (this may panic with bounds check error)...\n");

    let config = DecoderConfig::new().threads(1);
    match decode_with(&data, &config, &Unstoppable) {
        Ok(image) => {
            println!(
                "✓ SUCCESS! Decoded image: {}x{}",
                image.width(),
                image.height()
            );
            let desc = image.spec();
            let fmt = if desc.layout == zenpixels::SampleLayout::RGB8 {
                "RGB8"
            } else if desc.layout == zenpixels::SampleLayout::RGBA8 {
                "RGBA8"
            } else if desc.layout == zenpixels::SampleLayout::RGB16 {
                "RGB16"
            } else if desc.layout == zenpixels::SampleLayout::RGBA16 {
                "RGBA16"
            } else {
                "Other"
            };
            println!("  Format: {fmt}");
        }
        Err(e) => {
            eprintln!("✗ ERROR: {:?}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

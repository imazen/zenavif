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
            use zenavif::PixelData;
            println!(
                "✓ SUCCESS! Decoded image: {}x{}",
                image.width(),
                image.height()
            );
            match image {
                PixelData::Rgb8(_) => println!("  Format: RGB8"),
                PixelData::Rgba8(_) => println!("  Format: RGBA8"),
                PixelData::Rgb16(_) => println!("  Format: RGB16"),
                PixelData::Rgba16(_) => println!("  Format: RGBA16"),
                _ => println!("  Format: Other"),
            }
        }
        Err(e) => {
            eprintln!("✗ ERROR: {:?}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

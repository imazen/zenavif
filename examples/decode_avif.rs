//! Example: Decode an AVIF file and save as PNG

use rgb::ComponentBytes;
use std::fs::File;
use std::io::BufWriter;
use zenavif::{PixelData, decode};

fn main() {
    // Read input file
    let input_path = std::env::args()
        .nth(1)
        .expect("Usage: decode_avif <input.avif> <output.png>");
    let output_path = std::env::args()
        .nth(2)
        .expect("Usage: decode_avif <input.avif> <output.png>");

    let data = std::fs::read(&input_path).expect("Failed to read input file");

    // Decode AVIF
    let image = decode(&data).expect("Failed to decode AVIF");

    // Get dimensions
    let width = image.width() as u32;
    let height = image.height() as u32;

    // Create output file
    let file = File::create(&output_path).expect("Failed to create output file");
    let writer = BufWriter::new(file);

    // Write PNG
    let mut encoder = png::Encoder::new(writer, width, height);

    match &image {
        PixelData::Rgb8(img) => {
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("Failed to write PNG header");
            let pixels: Vec<u8> = img.buf().iter().flat_map(|px| [px.r, px.g, px.b]).collect();
            writer
                .write_image_data(&pixels)
                .expect("Failed to write PNG data");
        }
        PixelData::Rgba8(img) => {
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("Failed to write PNG header");
            let pixels: Vec<u8> = img
                .buf()
                .iter()
                .flat_map(|px| [px.r, px.g, px.b, px.a])
                .collect();
            writer
                .write_image_data(&pixels)
                .expect("Failed to write PNG data");
        }
        PixelData::Rgb16(img) => {
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().expect("Failed to write PNG header");
            let pixels: Vec<u8> = img
                .buf()
                .iter()
                .flat_map(|px| [px.r, px.g, px.b])
                .flat_map(|v| v.to_be_bytes())
                .collect();
            writer
                .write_image_data(&pixels)
                .expect("Failed to write PNG data");
        }
        PixelData::Rgba16(img) => {
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().expect("Failed to write PNG header");
            let pixels: Vec<u8> = img
                .buf()
                .iter()
                .flat_map(|px| [px.r, px.g, px.b, px.a])
                .flat_map(|v| v.to_be_bytes())
                .collect();
            writer
                .write_image_data(&pixels)
                .expect("Failed to write PNG data");
        }
        PixelData::Gray8(img) => {
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("Failed to write PNG header");
            writer
                .write_image_data(img.buf().as_bytes())
                .expect("Failed to write PNG data");
        }
        PixelData::Gray16(img) => {
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().expect("Failed to write PNG header");
            let pixels: Vec<u8> = img
                .buf()
                .iter()
                .flat_map(|v| v.value().to_be_bytes())
                .collect();
            writer
                .write_image_data(&pixels)
                .expect("Failed to write PNG data");
        }
        _ => panic!("Unexpected image type"),
    }

    println!("Decoded {}x{} image to {}", width, height, output_path);
}

//! Example: Decode an AVIF file and save as PNG

use std::fs::File;
use std::io::BufWriter;
use zencodec_types::PixelDescriptor;
use zenavif::decode;

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

    let desc = image.descriptor();
    if desc.layout_compatible(&PixelDescriptor::RGB8) {
        let img = image.try_as_imgref::<rgb::Rgb<u8>>().unwrap();
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("Failed to write PNG header");
        let pixels: Vec<u8> = img.buf().iter().flat_map(|px| [px.r, px.g, px.b]).collect();
        writer
            .write_image_data(&pixels)
            .expect("Failed to write PNG data");
    } else if desc.layout_compatible(&PixelDescriptor::RGBA8) {
        let img = image.try_as_imgref::<rgb::Rgba<u8>>().unwrap();
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
    } else if desc.layout_compatible(&PixelDescriptor::RGB16) {
        let img = image.try_as_imgref::<rgb::Rgb<u16>>().unwrap();
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().expect("Failed to write PNG header");
        let pixels: Vec<u8> = img
            .buf()
            .iter()
            .flat_map(|px| [px.r, px.g, px.b])
            .flat_map(|v: u16| v.to_be_bytes())
            .collect();
        writer
            .write_image_data(&pixels)
            .expect("Failed to write PNG data");
    } else if desc.layout_compatible(&PixelDescriptor::RGBA16) {
        let img = image.try_as_imgref::<rgb::Rgba<u16>>().unwrap();
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().expect("Failed to write PNG header");
        let pixels: Vec<u8> = img
            .buf()
            .iter()
            .flat_map(|px| [px.r, px.g, px.b, px.a])
            .flat_map(|v: u16| v.to_be_bytes())
            .collect();
        writer
            .write_image_data(&pixels)
            .expect("Failed to write PNG data");
    } else if desc.layout_compatible(&PixelDescriptor::GRAY8) {
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("Failed to write PNG header");
        let slice = image.as_slice();
        let mut data = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            data.extend_from_slice(slice.row(y));
        }
        writer
            .write_image_data(&data)
            .expect("Failed to write PNG data");
    } else if desc.layout_compatible(&PixelDescriptor::GRAY16) {
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().expect("Failed to write PNG header");
        let slice = image.as_slice();
        let mut data = Vec::with_capacity((width * height * 2) as usize);
        for y in 0..height {
            let row = slice.row(y);
            for chunk in row.chunks_exact(2) {
                let v = u16::from_ne_bytes([chunk[0], chunk[1]]);
                data.extend_from_slice(&v.to_be_bytes());
            }
        }
        writer
            .write_image_data(&data)
            .expect("Failed to write PNG data");
    } else {
        panic!("Unexpected image type: {:?}", desc);
    }

    println!("Decoded {}x{} image to {}", width, height, output_path);
}

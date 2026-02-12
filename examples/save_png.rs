use std::fs;

fn main() {
    let avif_path = std::env::args()
        .nth(1)
        .expect("usage: save_png <avif-path> <output-png>");
    let png_path = std::env::args()
        .nth(2)
        .expect("usage: save_png <avif-path> <output-png>");

    let data = fs::read(&avif_path).unwrap();
    let config = zenavif::DecoderConfig::new().threads(1);
    let img = zenavif::decode_with(&data, &config, &zenavif::Unstoppable).unwrap();

    match &img {
        zenavif::DecodedImage::Rgb8(buf) => {
            let w = buf.width() as u32;
            let h = buf.height() as u32;
            let mut rgb_data = Vec::with_capacity((w * h * 3) as usize);
            for row in buf.rows() {
                for px in row {
                    rgb_data.push(px.r);
                    rgb_data.push(px.g);
                    rgb_data.push(px.b);
                }
            }
            image::save_buffer(&png_path, &rgb_data, w, h, image::ColorType::Rgb8).unwrap();
            println!("Saved RGB8 {}x{} to {}", w, h, png_path);
        }
        zenavif::DecodedImage::Rgba8(buf) => {
            let w = buf.width() as u32;
            let h = buf.height() as u32;
            let mut rgba_data = Vec::with_capacity((w * h * 4) as usize);
            for row in buf.rows() {
                for px in row {
                    rgba_data.push(px.r);
                    rgba_data.push(px.g);
                    rgba_data.push(px.b);
                    rgba_data.push(px.a);
                }
            }
            image::save_buffer(&png_path, &rgba_data, w, h, image::ColorType::Rgba8).unwrap();
            println!("Saved RGBA8 {}x{} to {}", w, h, png_path);
        }
        other => {
            println!("Unsupported format: {:?}", std::mem::discriminant(other));
        }
    }
}

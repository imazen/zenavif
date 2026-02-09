use std::fs;

fn main() {
    let path = std::env::args().nth(1).expect("usage: inspect_pixels <avif-path>");
    let data = fs::read(&path).unwrap();
    let config = zenavif::DecoderConfig::new().threads(1);
    let img = zenavif::decode_with(&data, &config, &zenavif::Unstoppable).unwrap();

    match &img {
        zenavif::DecodedImage::Rgb8(buf) => {
            println!("RGB8 {}x{}", buf.width(), buf.height());
            for y in 0..3.min(buf.height()) {
                for x in 0..3.min(buf.width()) {
                    let px = buf.rows().nth(y).unwrap()[x];
                    print!("({},{},{}) ", px.r, px.g, px.b);
                }
                println!();
            }
            let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
            let mut count = 0u64;
            for row in buf.rows() {
                for px in row {
                    sr += px.r as u64;
                    sg += px.g as u64;
                    sb += px.b as u64;
                    count += 1;
                }
            }
            println!(
                "Mean: R={:.1} G={:.1} B={:.1}",
                sr as f64 / count as f64,
                sg as f64 / count as f64,
                sb as f64 / count as f64
            );
        }
        zenavif::DecodedImage::Rgba8(buf) => {
            println!("RGBA8 {}x{}", buf.width(), buf.height());
            for y in 0..3.min(buf.height()) {
                for x in 0..3.min(buf.width()) {
                    let px = buf.rows().nth(y).unwrap()[x];
                    print!("({},{},{},{}) ", px.r, px.g, px.b, px.a);
                }
                println!();
            }
            let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
            let mut count = 0u64;
            for row in buf.rows() {
                for px in row {
                    sr += px.r as u64;
                    sg += px.g as u64;
                    sb += px.b as u64;
                    count += 1;
                }
            }
            println!(
                "Mean: R={:.1} G={:.1} B={:.1}",
                sr as f64 / count as f64,
                sg as f64 / count as f64,
                sb as f64 / count as f64
            );
        }
        other => {
            println!("Other format: {:?}", std::mem::discriminant(other));
        }
    }
}

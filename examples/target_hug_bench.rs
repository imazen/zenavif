//! Measure how tightly `encode_rgb8_with_target` hugs an SSIMULACRA2 goal on
//! real corpus images: achieved-vs-target error distribution + encode counts.
//!
//!   cargo run --release --features target-quality --example target_hug_bench -- \
//!       <corpus.tsv> <speed> <targets-comma> [tolerance] [max_encodes]
//!
//! corpus.tsv: the rd_gap sample format (image\tw\th\tfamily header).
//! Emits a TSV row per (image, target): achieved, err, encodes, converged, bytes.

use std::io::BufRead;

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::Rgb;
use zenavif::{EncoderConfig, TargetMetric, TargetOptions, encode_rgb8_with_target};

fn load_png_rgb8(path: &str) -> ImgVec<Rgb<u8>> {
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).expect("open png"),
    ));
    let mut reader = decoder.read_info().expect("png info");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("png size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    let (w, h) = (info.width as usize, info.height as usize);
    let pixels: Vec<Rgb<u8>> = match info.color_type {
        png::ColorType::Rgb => buf[..w * h * 3]
            .chunks_exact(3)
            .map(|c| Rgb { r: c[0], g: c[1], b: c[2] })
            .collect(),
        png::ColorType::Rgba => buf[..w * h * 4]
            .chunks_exact(4)
            .map(|c| Rgb { r: c[0], g: c[1], b: c[2] })
            .collect(),
        png::ColorType::Grayscale => buf[..w * h]
            .iter()
            .map(|&g| Rgb { r: g, g, b: g })
            .collect(),
        other => panic!("unsupported png color type {other:?} for {path}"),
    };
    ImgVec::new(pixels, w, h)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let corpus = args.get(1).expect("corpus.tsv arg");
    let speed: u8 = args.get(2).expect("speed arg").parse().unwrap();
    let targets: Vec<f64> = args
        .get(3)
        .expect("targets arg")
        .split(',')
        .map(|t| t.parse().unwrap())
        .collect();
    let tolerance: f64 = args.get(4).map_or(0.5, |t| t.parse().unwrap());
    let max_encodes: u8 = args.get(5).map_or(6, |t| t.parse().unwrap());

    let file = std::fs::File::open(corpus).expect("open corpus tsv");
    let images: Vec<String> = std::io::BufReader::new(file)
        .lines()
        .skip(1)
        .filter_map(|l| l.ok())
        .filter_map(|l| l.split('\t').next().map(str::to_owned))
        .collect();

    println!("image\ttarget\tachieved\terr\tencodes\tconverged\tbytes\tms");
    for img_path in &images {
        let img = load_png_rgb8(img_path);
        for &target in &targets {
            let cfg = EncoderConfig::new().speed(speed).threads(Some(1));
            let opts = TargetOptions {
                tolerance,
                max_encodes,
                ..Default::default()
            };
            let t0 = std::time::Instant::now();
            let out = encode_rgb8_with_target(
                img.as_ref(),
                &cfg,
                TargetMetric::Ssim2(target),
                &opts,
                StopToken::new(Unstoppable),
            );
            let ms = t0.elapsed().as_millis();
            match out {
                Ok(o) => {
                    let base = std::path::Path::new(img_path)
                        .file_name()
                        .unwrap()
                        .to_string_lossy();
                    println!(
                        "{base}\t{target}\t{:.3}\t{:+.3}\t{}\t{}\t{}\t{ms}",
                        o.score,
                        o.score - target,
                        o.encodes,
                        o.converged,
                        o.encoded.avif_file.len(),
                    );
                }
                Err(e) => eprintln!("FAIL {img_path} target {target}: {e:?}"),
            }
        }
    }
}

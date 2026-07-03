//! Harness cell for the butteraugli two-pass A/B (`scripts/rd_gap`):
//! encodes ONE PNG at one (quality, speed) with either the plain single-pass
//! path or the diffmap-guided two-pass driver, writes the AVIF, and prints a
//! machine-readable stats line. Both arms share every other setting, so the
//! toggle is the only variable.
//!
//! Usage:
//!   two_pass_cell <in.png> <out.avif> <quality> <speed> <single|twopass> [strength]
//!
//! Stdout (tab-separated):
//!   mode<TAB>bytes<TAB>enc_ms<TAB>pass1_bytes<TAB>pass1_ba3n<TAB>pass1_bamax
//!   (pass1_* are "NA" in single mode)

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::Rgb;
use zenavif::{EncoderConfig, TwoPassOptions, encode_rgb8, encode_rgb8_two_pass};

fn load_png_rgb(path: &std::path::Path) -> ImgVec<Rgb<u8>> {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
        .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels: Vec<Rgb<u8>> = img
        .pixels()
        .map(|p| Rgb {
            r: p.0[0],
            g: p.0[1],
            b: p.0[2],
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 6 {
        eprintln!(
            "usage: two_pass_cell <in.png> <out.avif> <quality> <speed> <single|twopass> [strength]"
        );
        std::process::exit(2);
    }
    let input = std::path::Path::new(&args[1]);
    let output = &args[2];
    let quality: f32 = args[3].parse().expect("quality");
    let speed: u8 = args[4].parse().expect("speed");
    let mode = args[5].as_str();
    let strength: f64 = args.get(6).map(|s| s.parse().expect("strength")).unwrap_or(1.0);

    let img = load_png_rgb(input);
    // 8-bit forced: the rd_gap harness decodes with `save_png` (RGB8-only),
    // matching the cavif cells' `--depth 8` (fair + symmetric with libaom).
    let config = EncoderConfig::new()
        .quality(quality)
        .speed(speed)
        .bit_depth(zenavif::EncodeBitDepth::Eight);
    let stop = StopToken::new(Unstoppable);

    let t0 = std::time::Instant::now();
    match mode {
        "single" => {
            let enc = encode_rgb8(img.as_ref(), &config, stop).expect("single-pass encode");
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            std::fs::write(output, &enc.avif_file).expect("write avif");
            println!("single\t{}\t{ms:.1}\tNA\tNA\tNA", enc.avif_file.len());
        }
        "twopass" => {
            let options = TwoPassOptions {
                strength,
                ..Default::default()
            };
            let two = encode_rgb8_two_pass(img.as_ref(), &config, &options, stop)
                .expect("two-pass encode");
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            std::fs::write(output, &two.encode.avif_file).expect("write avif");
            println!(
                "twopass\t{}\t{ms:.1}\t{}\t{:.6}\t{:.6}",
                two.encode.avif_file.len(),
                two.pass1_bytes,
                two.pass1_butteraugli_3n,
                two.pass1_butteraugli_max
            );
        }
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(2);
        }
    }
}

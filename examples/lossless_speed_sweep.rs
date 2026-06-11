//! Lossless speed sweep — reproduces imazen/zenavif#8 (slow speeds
//! producing LARGER lossless output on most content) directly through
//! zenavif's own encode path, no external wrappers.
//!
//! Sources are corpus vectors decoded to RGB8, spanning content classes:
//! photo (kodim03/kodim23), detailed photo (paris), screen/text
//! (colors_text), flat synthetic (colors), illustration-ish line work
//! (draw_points).
//!
//! ```sh
//! cargo run --release --features encode-imazen --example lossless_speed_sweep
//! ```

use almost_enough::{StopToken, Unstoppable};
use rgb::Rgb;
use zenavif::{EncodeColorModel, EncoderConfig};

const SOURCES: &[(&str, &str)] = &[
    (
        "photo-natural",
        "tests/vectors/libavif/kodim23_yuv420_8bpc.avif",
    ),
    ("photo", "tests/vectors/libavif/kodim03_yuv420_8bpc.avif"),
    (
        "photo-detailed",
        "tests/vectors/libavif/paris_icc_exif_xmp.avif",
    ),
    (
        "screen-text",
        "tests/vectors/libavif/colors_text_sdr_srgb.avif",
    ),
    (
        "synthetic-flat",
        "tests/vectors/libavif/colors_sdr_srgb.avif",
    ),
    ("line-art", "tests/vectors/libavif/draw_points_idat.avif"),
];

const SPEEDS: &[u8] = &[1, 2, 4, 6, 8, 10];

fn main() {
    println!(
        "source\tclass\twidth\theight\tspeed\tbytes\tms\tmismatched_px\tmismatched_pct\tmax_delta"
    );
    for (class, path) in SOURCES {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skip {path}: {e}");
                continue;
            }
        };
        let dec = zenavif::decode_with(
            &data,
            &zenavif::DecoderConfig::new().prefer_8bit(true),
            &Unstoppable,
        )
        .expect("decode source");
        let Some(img) = dec.try_as_imgref::<Rgb<u8>>() else {
            eprintln!("skip {path}: not RGB8");
            continue;
        };
        // Re-own to a tight buffer.
        let owned: Vec<Rgb<u8>> = img.pixels().collect();
        let img = imgref::ImgVec::new(owned, img.width(), img.height());

        for &speed in SPEEDS {
            let cfg = EncoderConfig::new()
                .speed(speed)
                .threads(Some(1))
                .color_model(EncodeColorModel::Rgb)
                .with_lossless(true);
            let t0 = std::time::Instant::now();
            let out = zenavif::encode_rgb8(img.as_ref(), &cfg, StopToken::new(Unstoppable))
                .expect("encode");
            let ms = t0.elapsed().as_millis();
            // "Lossless" exactness vs source (zenrav1e#9): if slow
            // speeds are closer to exact, the #8 size inversion is RDO
            // buying down phantom distortion that exact reconstruction
            // would eliminate.
            let rt = zenavif::decode_with(
                &out.avif_file,
                &zenavif::DecoderConfig::new().prefer_8bit(true),
                &Unstoppable,
            )
            .expect("roundtrip decode");
            let rt_img = rt.try_as_imgref::<Rgb<u8>>().expect("rgb8 roundtrip");
            let mut mismatched = 0usize;
            let mut max_delta = 0i32;
            for (src_row, got_row) in img.rows().zip(rt_img.rows()) {
                for (s, g) in src_row.iter().zip(got_row) {
                    let d = (i32::from(s.r) - i32::from(g.r))
                        .abs()
                        .max((i32::from(s.g) - i32::from(g.g)).abs())
                        .max((i32::from(s.b) - i32::from(g.b)).abs());
                    if d > 0 {
                        mismatched += 1;
                    }
                    max_delta = max_delta.max(d);
                }
            }
            let total = img.width() * img.height();
            println!(
                "{path}\t{class}\t{}\t{}\t{speed}\t{}\t{ms}\t{mismatched}\t{:.2}\t{max_delta}",
                img.width(),
                img.height(),
                out.avif_file.len(),
                mismatched as f64 / total as f64 * 100.0
            );
        }
    }
}

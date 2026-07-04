//! 12-bit encode probe: does the ravif → zenrav1e chain produce valid
//! 12-bit AV1, and can zenavif + aomdec decode it?
//!
//! zenavif's public API caps at 10-bit (EncodeBitDepth has no Twelve);
//! this probes the underlying ravif capability to document the honest
//! 12-bit verdict. Usage:
//!   cargo run --release --features encode --example twelvebit_probe [out.avif]

use almost_enough::Unstoppable;

fn main() {
    let (w, h) = (64usize, 48usize);
    // 12-bit GBR planes (identity matrix), full range: gradient + speculars.
    let max12 = 4095u16;
    let pixels: Vec<[u16; 3]> = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let v = (x * 4095 / (w - 1)) as u16;
                let spec = if x % 13 == 0 && y % 5 == 0 { max12 } else { v };
                // [g, b, r] plane order per identity-matrix convention
                [spec, v / 2, (y * 4095 / (h - 1)) as u16]
            })
        })
        .collect();

    let enc = ravif::Encoder::new()
        .with_quality(90.0)
        .with_speed(8)
        .with_bit_depth(ravif::BitDepth::Twelve);
    let result = enc.encode_raw_planes_12_bit(
        w,
        h,
        pixels,
        None::<std::iter::Empty<u16>>,
        ravif::PixelRange::Full,
        ravif::MatrixCoefficients::Identity,
    );
    let encoded = match result {
        Ok(e) => {
            println!("ENCODE: ok ({} bytes)", e.avif_file.len());
            e
        }
        Err(e) => {
            println!("ENCODE: FAILED — {e}");
            std::process::exit(1);
        }
    };

    if let Some(path) = std::env::args().nth(1) {
        std::fs::write(&path, &encoded.avif_file).unwrap();
        println!("wrote {path}");
    }

    // Parse: container-level bit depth
    match zenavif_parse::AvifParser::from_bytes(&encoded.avif_file) {
        Ok(parser) => {
            let md = parser
                .primary_data()
                .ok()
                .and_then(|d| zenavif_parse::AV1Metadata::parse_av1_bitstream(&d).ok());
            match md {
                Some(m) => println!(
                    "PARSE: ok — seq header says bit_depth={} profile={} mono={}",
                    m.bit_depth, m.seq_profile, m.monochrome
                ),
                None => println!("PARSE: container ok, AV1 seq header parse FAILED"),
            }
        }
        Err(e) => println!("PARSE: FAILED — {e}"),
    }

    // Decode with zenavif (rav1d-safe managed)
    let dcfg = zenavif::DecoderConfig::new().prefer_8bit(false);
    match zenavif::ManagedAvifDecoder::new(&encoded.avif_file, &dcfg) {
        Ok(mut dec) => match dec.decode_full(&Unstoppable) {
            Ok((pixels, info)) => {
                println!(
                    "DECODE(rav1d-safe): ok — info.bit_depth={} format={:?} {}x{}",
                    info.bit_depth,
                    pixels.descriptor().pixel_format(),
                    info.width,
                    info.height
                );
            }
            Err(e) => println!("DECODE(rav1d-safe): FAILED — {e}"),
        },
        Err(e) => println!("DECODE(rav1d-safe): open FAILED — {e}"),
    }
}

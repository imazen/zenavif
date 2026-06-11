//! Generate genuine monochrome (Cs400) AVIF test fixtures.
//!
//! Encodes deterministic gray patterns with zenrav1e in true monochrome
//! mode (no chroma planes) and wraps them with `Aviffy::set_monochrome`.
//! Used to produce the fixtures committed at
//! `zenavif/tests/vectors/zenavif/` for GRAY8/GRAY16 decode tests
//! (imazen/zenavif#5) — regenerate with:
//!
//! ```sh
//! cargo run --example make_mono_avif -- <output-dir>
//! ```

use zenrav1e::color::PixelRange;
use zenrav1e::config::SpeedSettings;
use zenrav1e::prelude::*;

/// Deterministic gray pattern: gradient + sine texture, full 0..=255 sweep.
fn gray_pattern(w: usize, h: usize) -> Vec<u8> {
    (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let g = (x * 255) / w.max(1);
                let t = ((x as f32 * 0.7).sin() * 20.0 + (y as f32 * 0.9).cos() * 20.0) as i32;
                (g as i32 + t).clamp(0, 255) as u8
            })
        })
        .collect()
}

fn encode_mono<P: Pixel + Default>(
    w: usize,
    h: usize,
    bit_depth: usize,
    pixel_range: PixelRange,
    gray8: &[u8],
) -> Vec<u8> {
    // The encoder treats input samples as already being in the signaled
    // range — compress full-range source into 16..=235 for Limited so a
    // range-expanding decode reproduces the source.
    let ranged: Vec<u8>;
    let gray8 = if pixel_range == PixelRange::Limited {
        ranged = gray8
            .iter()
            .map(|&g| 16 + ((u16::from(g) * 219 + 127) / 255) as u8)
            .collect();
        &ranged
    } else {
        gray8
    };
    let enc = EncoderConfig {
        width: w,
        height: h,
        bit_depth,
        chroma_sampling: ChromaSampling::Cs400,
        pixel_range,
        still_picture: true,
        quantizer: 60,
        min_quantizer: 0,
        speed_settings: SpeedSettings::from_preset(6),
        ..Default::default()
    };
    let cfg = Config::new().with_encoder_config(enc).with_threads(1);
    let mut ctx: Context<P> = cfg.new_context().expect("context");
    let mut frame = ctx.new_frame();
    // Monochrome: only the luma plane carries data. Feed the 8-bit
    // pattern shifted into the target depth.
    if bit_depth == 8 {
        frame.planes[0].copy_from_raw_u8(gray8, w, 1);
    } else {
        let shifted: Vec<u8> = gray8
            .iter()
            .flat_map(|&g| {
                let v = (u16::from(g)) << (bit_depth - 8);
                v.to_le_bytes()
            })
            .collect();
        frame.planes[0].copy_from_raw_u8(&shifted, w * 2, 2);
    }
    ctx.send_frame(frame).expect("send_frame");
    ctx.flush();
    let mut out = Vec::new();
    loop {
        match ctx.receive_packet() {
            Ok(pkt) => out.extend_from_slice(&pkt.data),
            Err(EncoderStatus::Encoded) => continue,
            Err(EncoderStatus::LimitReached) => break,
            Err(e) => panic!("receive_packet: {e:?}"),
        }
    }
    out
}

fn write_fixture(dir: &str, name: &str, w: u32, h: u32, depth: u8, av1: &[u8]) {
    let avif = zenavif_serialize::Aviffy::new()
        .set_monochrome(true)
        .to_vec(av1, None, w, h, depth);
    let path = format!("{dir}/{name}");
    std::fs::write(&path, &avif).expect("write fixture");
    println!("{path}: {} bytes ({w}x{h}, {depth}-bit mono)", avif.len());
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&dir).expect("create output dir");

    let (w, h) = (96usize, 64usize);
    let pat = gray_pattern(w, h);

    let av1 = encode_mono::<u8>(w, h, 8, PixelRange::Full, &pat);
    write_fixture(&dir, "mono_gradient_8b_full.avif", 96, 64, 8, &av1);

    let av1 = encode_mono::<u8>(w, h, 8, PixelRange::Limited, &pat);
    write_fixture(&dir, "mono_gradient_8b_limited.avif", 96, 64, 8, &av1);

    let av1 = encode_mono::<u16>(w, h, 10, PixelRange::Full, &pat);
    write_fixture(&dir, "mono_gradient_10b_full.avif", 96, 64, 10, &av1);

    // Odd dimensions exercise the decoder's coded-size → display-size crop.
    let (cw, ch) = (5usize, 3usize);
    let pat = gray_pattern(cw, ch);
    let av1 = encode_mono::<u8>(cw, ch, 8, PixelRange::Full, &pat);
    write_fixture(&dir, "mono_5x3_8b_full.avif", 5, 3, 8, &av1);
}

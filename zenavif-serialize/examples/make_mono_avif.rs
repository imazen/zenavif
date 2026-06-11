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

/// Encode the same gray content the way a gray-unaware path does:
/// replicated into Y of a color encode with neutral chroma planes — what
/// "expand gray to RGB then encode color" costs at the AV1 layer.
fn encode_gray_as_color(
    w: usize,
    h: usize,
    chroma: ChromaSampling,
    gray8: &[u8],
) -> Vec<u8> {
    let enc = EncoderConfig {
        width: w,
        height: h,
        bit_depth: 8,
        chroma_sampling: chroma,
        pixel_range: PixelRange::Full,
        still_picture: true,
        quantizer: 60,
        min_quantizer: 0,
        speed_settings: SpeedSettings::from_preset(6),
        ..Default::default()
    };
    let cfg = Config::new().with_encoder_config(enc).with_threads(1);
    let mut ctx: Context<u8> = cfg.new_context().expect("context");
    let mut frame = ctx.new_frame();
    // Gray expanded to YCbCr: Y = g (BT.601 luma of R=G=B equals g),
    // chroma planes neutral (128).
    frame.planes[0].copy_from_raw_u8(gray8, w, 1);
    for p in 1..3 {
        let cfgp = frame.planes[p].cfg.clone();
        let (cw, ch) = ((w + cfgp.xdec) >> cfgp.xdec, (h + cfgp.ydec) >> cfgp.ydec);
        let neutral = vec![128u8; cw * ch];
        frame.planes[p].copy_from_raw_u8(&neutral, cw, 1);
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

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&dir).expect("create output dir");

    let (w, h) = (96usize, 64usize);
    let pat = gray_pattern(w, h);

    // Size A/B for imazen/zenavif#6: true Cs400 monochrome vs the same
    // content through color encodes with neutral chroma (the cost of a
    // gray-unaware "expand to RGB" path), identical quantizer/speed.
    // Multiple sizes — tiny images are dominated by fixed header costs.
    for (aw, ah) in [(96usize, 64usize), (512, 512), (1024, 1024), (2048, 2048)] {
        let apat = gray_pattern(aw, ah);
        let t0 = std::time::Instant::now();
        let mono = encode_mono::<u8>(aw, ah, 8, PixelRange::Full, &apat);
        let t_mono = t0.elapsed();
        let t0 = std::time::Instant::now();
        let c420 = encode_gray_as_color(aw, ah, ChromaSampling::Cs420, &apat);
        let t_420 = t0.elapsed();
        let t0 = std::time::Instant::now();
        let c444 = encode_gray_as_color(aw, ah, ChromaSampling::Cs444, &apat);
        let t_444 = t0.elapsed();
        println!(
            "size A/B {aw}x{ah} (q60 s6 still, 1 thread): Cs400 = {} B {:.0?} | Cs420 = {} B ({:+.1}%) {:.0?} | Cs444 = {} B ({:+.1}%) {:.0?}",
            mono.len(),
            t_mono,
            c420.len(),
            (c420.len() as f64 - mono.len() as f64) / mono.len() as f64 * 100.0,
            t_420,
            c444.len(),
            (c444.len() as f64 - mono.len() as f64) / mono.len() as f64 * 100.0,
            t_444
        );
    }

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

    // ICC-carrying mono variants for color-context class-gate tests:
    // minimal structurally-tagged profiles (header class at 16..20,
    // 'acsp' at 36..40, 132+ bytes — what class gates inspect; not
    // CMS-usable). RGB-class on a mono image is the spec-questionable
    // in-the-wild case; GRAY-class is the MIAF-correct pairing.
    let (w, h) = (96usize, 64usize);
    let pat = gray_pattern(w, h);
    let av1 = encode_mono::<u8>(w, h, 8, PixelRange::Full, &pat);
    for (name, class) in [
        ("mono_gradient_8b_rgbicc.avif", *b"RGB "),
        ("mono_gradient_8b_grayicc.avif", *b"GRAY"),
    ] {
        let mut icc = vec![0u8; 144];
        icc[0..4].copy_from_slice(&144u32.to_be_bytes());
        icc[16..20].copy_from_slice(&class);
        icc[36..40].copy_from_slice(b"acsp");
        let avif = zenavif_serialize::Aviffy::new()
            .set_monochrome(true)
            .set_icc_profile(icc)
            .to_vec(&av1, None, 96, 64, 8);
        let path = format!("{dir}/{name}");
        std::fs::write(&path, &avif).expect("write fixture");
        println!("{path}: {} bytes (96x64 mono + {} ICC)", avif.len(), name);
    }
}

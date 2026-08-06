//! Encode one 16-bit PNG (PQ HDR, cICP-tagged) to a 10-bit AVIF cell.
//! Usage: hdr_encode_cell <in.png> <out.avif> <quality> [speed]
//! Echoes the PNG's cICP (falls back to BT.709+PQ full range); clli is
//! MEASURED from the pixels (zenpixels CllMeasure — appendix AA), mdcv is
//! the conventional declared mastering display.

use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::Rgb;

fn main() {
    let mut args = std::env::args().skip(1);
    let (inp, outp) = (args.next().expect("in.png"), args.next().expect("out.avif"));
    let quality: f32 = args.next().expect("quality").parse().unwrap();
    let speed: u8 = args.next().map(|s| s.parse().unwrap()).unwrap_or(8);

    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&inp).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.bit_depth, png::BitDepth::Sixteen, "need 16-bit PNG");
    assert_eq!(info.color_type, png::ColorType::Rgb, "need RGB PNG");
    let (w, h) = (info.width as usize, info.height as usize);
    // PNG 16-bit is big-endian
    let pixels: Vec<Rgb<u16>> = buf[..w * h * 6]
        .chunks_exact(6)
        .map(|c| Rgb {
            r: u16::from_be_bytes([c[0], c[1]]),
            g: u16::from_be_bytes([c[2], c[3]]),
            b: u16::from_be_bytes([c[4], c[5]]),
        })
        .collect();
    let img = Img::new(pixels, w, h);

    // cICP from the PNG if present (P/T/M/full), else BT.709+PQ.
    let cicp = reader.info().coding_independent_code_points;
    let (cp, tc) = cicp
        .map(|c| (c.color_primaries, c.transfer_function))
        .unwrap_or((1, 16));

    // MEASURE the content light level from the pixels (campaign appendix AA:
    // stamping configured CLL is usually wrong about actual content — this
    // example used to write a hardcoded 1000/250 on every file). PQ code
    // values → absolute nits via the ST-2084 EOTF, then the zenpixels
    // measurement owner (`CllMeasure::measure_max`, MaxRGB per CTA-861.3).
    // Non-PQ inputs (tc != 16) keep no CLL rather than a guessed one.
    let measured_cll = (tc == 16).then(|| {
        use zenpixels_convert::hdr::measure::{CllMeasure, LightLevelMethod};
        // PQ-normalized linear: 1.0 = 10 000 cd/m² — anchor the measurement
        // with a 10 000-nit diffuse-white scale so the readout IS cd/m².
        let mut lin = Vec::with_capacity(w * h * 3);
        for p in img.buf().iter() {
            for v in [p.r, p.g, p.b] {
                lin.push(linear_srgb::tf::pq_to_linear(f32::from(v) / 65535.0));
            }
        }
        let bytes: Vec<u8> = lin.iter().flat_map(|f| f.to_ne_bytes()).collect();
        let px = zenpixels::PixelSlice::new(
            &bytes,
            w as u32,
            h as u32,
            w * 12,
            zenpixels::PixelDescriptor::RGBF32_LINEAR,
        )
        .expect("measurement slice");
        zenpixels::hdr::ContentLightLevel::measure_max(
            px,
            zenpixels::hdr::DiffuseWhite::new(10_000.0),
            LightLevelMethod::MaxRgb,
        )
        .expect("linear f32 slice is measurable")
    });

    let mut config = zenavif::EncoderConfig::new()
        .quality(quality)
        .speed(speed)
        .color_primaries(cp)
        .transfer_characteristics(tc)
        // The mastering display stays DECLARED (SMPTE ST 2086 describes the
        // display the content was mastered on — not measurable from pixels;
        // BT.2020/D65 at the 10 000-nit PQ container ceiling is the
        // conventional stand-in when the true mastering display is unknown).
        .mastering_display(zenavif::MasteringDisplayConfig {
            // G,B,R wire order (BT.2020 primaries), D65, 1000/0.005 cd/m²
            primaries: [(8500, 39850), (6550, 2300), (35400, 14600)],
            white_point: (15635, 16450),
            max_luminance: 10_000_000,
            min_luminance: 50,
        });
    if let Some(cll) = measured_cll {
        config = config.content_light_level(
            cll.max_content_light_level,
            cll.max_frame_average_light_level,
        );
    }
    let enc = zenavif::encode_rgb16(img.as_ref(), &config, StopToken::new(Unstoppable)).unwrap();
    std::fs::write(&outp, &enc.avif_file).unwrap();

    // Self-check: zenavif decode + metadata echo.
    let dcfg = zenavif::DecoderConfig::new().prefer_8bit(false);
    let mut d = zenavif::ManagedAvifDecoder::new(&enc.avif_file, &dcfg).unwrap();
    let (px, di) = d.decode_full(&Unstoppable).unwrap();
    assert_eq!(di.bit_depth, 10);
    assert_eq!(di.color_primaries.0, cp);
    assert_eq!(di.transfer_characteristics.0, tc);
    assert_eq!(
        di.content_light_level.is_some(),
        measured_cll.is_some(),
        "clli present exactly when measured (PQ input)"
    );
    if let (Some(echoed), Some(measured)) = (di.content_light_level, measured_cll) {
        assert_eq!(
            (echoed.max_content_light_level, echoed.max_pic_average_light_level),
            (
                measured.max_content_light_level,
                measured.max_frame_average_light_level
            ),
            "container must echo the MEASURED content light level"
        );
    }
    assert!(di.mastering_display.is_some());
    assert_eq!((di.width as usize, di.height as usize), (w, h));
    let _ = px;
    println!(
        "OK {} {}x{} q{} s{} -> {} bytes (cicp {}/{})",
        outp,
        w,
        h,
        quality,
        speed,
        enc.avif_file.len(),
        cp,
        tc
    );
}

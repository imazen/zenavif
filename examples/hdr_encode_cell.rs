//! Encode one 16-bit PNG (PQ HDR, cICP-tagged) to a 10-bit AVIF cell.
//! Usage: hdr_encode_cell <in.png> <out.avif> <quality> [speed]
//! Echoes the PNG's cICP (falls back to BT.709+PQ full range) and stamps
//! test clli/mdcv so conformance cells also carry HDR metadata.

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

    let config = zenavif::EncoderConfig::new()
        .quality(quality)
        .speed(speed)
        .color_primaries(cp)
        .transfer_characteristics(tc)
        .content_light_level(1000, 250)
        .mastering_display(zenavif::MasteringDisplayConfig {
            // G,B,R wire order (BT.2020 primaries), D65, 1000/0.005 cd/m²
            primaries: [(8500, 39850), (6550, 2300), (35400, 14600)],
            white_point: (15635, 16450),
            max_luminance: 10_000_000,
            min_luminance: 50,
        });
    let enc = zenavif::encode_rgb16(img.as_ref(), &config, StopToken::new(Unstoppable)).unwrap();
    std::fs::write(&outp, &enc.avif_file).unwrap();

    // Self-check: zenavif decode + metadata echo.
    let dcfg = zenavif::DecoderConfig::new().prefer_8bit(false);
    let mut d = zenavif::ManagedAvifDecoder::new(&enc.avif_file, &dcfg).unwrap();
    let (px, di) = d.decode_full(&Unstoppable).unwrap();
    assert_eq!(di.bit_depth, 10);
    assert_eq!(di.color_primaries.0, cp);
    assert_eq!(di.transfer_characteristics.0, tc);
    assert!(di.content_light_level.is_some());
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

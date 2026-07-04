//! Decode a gain-mapped AVIF and re-encode it preserving map + metadata +
//! alt colr — the roundtrip the contract tests assert, as a file for
//! external-tool cross-validation (avifgainmaputil printmetadata).
//! Usage: gainmap_reencode <in.avif> <out.avif>
use almost_enough::{StopToken, Unstoppable};

fn main() {
    let mut args = std::env::args().skip(1);
    let (inp, outp) = (args.next().expect("in"), args.next().expect("out"));
    let data = std::fs::read(&inp).unwrap();
    let dcfg = zenavif::DecoderConfig::new().prefer_8bit(false);
    let mut dec = zenavif::ManagedAvifDecoder::new(&data, &dcfg).unwrap();
    let (pixels, info) = dec.decode_full(&Unstoppable).unwrap();
    let gm = info.gain_map.as_ref().expect("input has a gain map");
    let md = zenavif_parse::AV1Metadata::parse_av1_bitstream(&gm.gain_map_data).unwrap();
    let mut config = zenavif::EncoderConfig::new()
        .quality(85.0)
        .speed(8)
        .color_primaries(info.color_primaries.0)
        .transfer_characteristics(info.transfer_characteristics.0)
        .with_gain_map(
            gm.gain_map_data.clone(),
            md.max_frame_width.get(),
            md.max_frame_height.get(),
            md.bit_depth,
            gm.metadata.to_bytes(),
        );
    match &gm.alt_color_info {
        Some(zenavif_parse::ColorInformation::Nclx {
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            full_range,
        }) => {
            config = config.with_gain_map_alt_color(
                *color_primaries as u8,
                *transfer_characteristics as u8,
                *matrix_coefficients as u8,
                *full_range,
            );
        }
        Some(zenavif_parse::ColorInformation::IccProfile(icc)) => {
            config = config.with_gain_map_alt_icc(icc.clone());
        }
        None => {}
    }
    if let Some(icc) = &info.icc_profile {
        config = config.icc_profile(icc.clone());
    }
    let stop = StopToken::new(Unstoppable);
    let enc = if info.bit_depth > 8 {
        zenavif::encode_rgb16(pixels.try_as_imgref().unwrap(), &config, stop).unwrap()
    } else {
        zenavif::encode_rgb8(pixels.try_as_imgref().unwrap(), &config, stop).unwrap()
    };
    std::fs::write(&outp, &enc.avif_file).unwrap();
    println!("{} -> {} ({} bytes)", inp, outp, enc.avif_file.len());
}

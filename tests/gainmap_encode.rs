//! Tests for gain map embedding through the zenavif encode pipeline.
//!
//! Verifies that gain map data set via `EncoderConfig::with_gain_map()` is
//! correctly threaded through ravif to zenavif-serialize and produces valid
//! AVIF files with `tmap` derived image items.

#![cfg(feature = "encode")]

use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::Rgb;
use zenavif::{EncoderConfig, encode_rgb8};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// Build a minimal ISO 21496-1 binary metadata blob for testing.
///
/// Single-channel, use_base_colour_space=true, base headroom 0/1, alt headroom 1/1.
fn make_test_tmap_metadata() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(0); // version
    buf.extend_from_slice(&0u16.to_be_bytes()); // minimum_version
    buf.extend_from_slice(&0u16.to_be_bytes()); // writer_version
    // flags: is_multichannel=false (bit 7), use_base_colour_space=true (bit 6)
    buf.push(0b0100_0000);

    // base_hdr_headroom = 0/1
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    // alternate_hdr_headroom = 1/1
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());

    // 1 channel (not multichannel):
    // gain_map_min = 0/1
    buf.extend_from_slice(&0i32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    // gain_map_max = 1/1
    buf.extend_from_slice(&1i32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    // gamma = 1/1
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    // base_offset = 0/1
    buf.extend_from_slice(&0i32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    // alternate_offset = 0/1
    buf.extend_from_slice(&0i32.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());

    buf
}

/// Create a simple 16x16 RGB8 test image.
fn make_rgb8_image() -> Img<Vec<Rgb<u8>>> {
    let mut pixels = Vec::with_capacity(16 * 16);
    for y in 0..16u8 {
        for x in 0..16u8 {
            pixels.push(Rgb {
                r: x * 16,
                g: y * 16,
                b: 128,
            });
        }
    }
    Img::new(pixels, 16, 16)
}

/// Encode a real (tiny) gain-map AV1 payload and return (obu, w, h, depth).
///
/// The container mux derives the gain-map item's `av1C` from the payload's
/// sequence header and validates declared dims/depth against it, so tests
/// must carry a real bitstream (a hand-rolled byte blob no longer encodes).
fn make_real_gain_map() -> (Vec<u8>, u32, u32, u8) {
    let (w, h) = (16usize, 12usize);
    let pixels: Vec<Rgb<u8>> = (0..w * h)
        .map(|i| {
            let v = ((i * 255) / (w * h)) as u8;
            Rgb { r: v, g: v, b: v }
        })
        .collect();
    let img = Img::new(pixels, w, h);
    let cfg = EncoderConfig::new().quality(85.0).speed(10);
    let encoded = encode_rgb8(img.as_ref(), &cfg, stop()).expect("map encode");
    let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).expect("map parses");
    let obu = parser.primary_data().expect("map primary").into_owned();
    let md = zenavif_parse::AV1Metadata::parse_av1_bitstream(&obu).expect("map seq header");
    (
        obu,
        md.max_frame_width.get(),
        md.max_frame_height.get(),
        md.bit_depth,
    )
}

#[test]
fn encode_with_gain_map_produces_valid_avif() {
    let img = make_rgb8_image();
    let metadata = make_test_tmap_metadata();

    // Real (tiny) AV1 payload: the encode path parses the payload's
    // sequence header to derive the muxed av1C and validate dims/depth.
    let (gain_map_av1, gw, gh, gdepth) = make_real_gain_map();

    let config = EncoderConfig::new().quality(80.0).speed(10).with_gain_map(
        gain_map_av1.clone(),
        gw,
        gh,
        gdepth,
        metadata.clone(),
    );

    let encoded =
        encode_rgb8(img.as_ref(), &config, stop()).expect("encode with gain map should succeed");

    assert!(!encoded.avif_file.is_empty());
    assert!(encoded.color_byte_size > 0);

    // Parse with zenavif-parse and verify the gain map is present
    let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file)
        .expect("output should be valid AVIF");

    let gm_meta = parser
        .gain_map_metadata()
        .expect("gain map metadata should be present");
    assert!(!gm_meta.is_multichannel);
    assert!(gm_meta.use_base_colour_space);
    assert_eq!(gm_meta.base_hdr_headroom_n, 0);
    assert_eq!(gm_meta.base_hdr_headroom_d, 1);
    assert_eq!(gm_meta.alternate_hdr_headroom_n, 1);
    assert_eq!(gm_meta.alternate_hdr_headroom_d, 1);

    // Gain map AV1 data should round-trip exactly
    let gm_data = parser
        .gain_map_data()
        .expect("gain map data should be present")
        .expect("gain map data should resolve");
    assert_eq!(gm_data.as_ref(), &gain_map_av1[..]);
}

#[test]
fn encode_without_gain_map_has_none() {
    let img = make_rgb8_image();
    let config = EncoderConfig::new().quality(80.0).speed(10);

    let encoded = encode_rgb8(img.as_ref(), &config, stop()).expect("encode should succeed");

    let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file)
        .expect("output should be valid AVIF");

    assert!(
        parser.gain_map_metadata().is_none(),
        "image without gain map should have no tmap"
    );
    assert!(parser.gain_map_data().is_none());
}

#[test]
fn encode_gain_map_roundtrip_through_decode() {
    let img = make_rgb8_image();
    let metadata = make_test_tmap_metadata();
    let (gain_map_av1, gw, gh, gdepth) = make_real_gain_map();

    let config = EncoderConfig::new().quality(80.0).speed(10).with_gain_map(
        gain_map_av1.clone(),
        gw,
        gh,
        gdepth,
        metadata.clone(),
    );

    let encoded = encode_rgb8(img.as_ref(), &config, stop()).expect("encode should succeed");

    // Decode through zenavif's managed decoder and verify gain map comes through
    let decoder =
        zenavif::ManagedAvifDecoder::new(&encoded.avif_file, &zenavif::DecoderConfig::default())
            .expect("decoder should open encoded file");

    let info = decoder.probe_info().expect("probe should succeed");
    let gm = info
        .gain_map
        .as_ref()
        .expect("gain map should be present after decode");

    // Metadata fields should match what we encoded
    assert!(!gm.metadata.is_multichannel);
    assert!(gm.metadata.use_base_colour_space);
    assert_eq!(gm.metadata.base_hdr_headroom_n, 0);
    assert_eq!(gm.metadata.base_hdr_headroom_d, 1);
    assert_eq!(gm.metadata.alternate_hdr_headroom_n, 1);
    assert_eq!(gm.metadata.alternate_hdr_headroom_d, 1);

    // Channel params (parser always expands to 3 channels, copying ch0 for single-channel)
    assert_eq!(gm.metadata.channels.len(), 3);
    let ch = &gm.metadata.channels[0];
    assert_eq!(ch.gain_map_min_n, 0);
    assert_eq!(ch.gain_map_min_d, 1);
    assert_eq!(ch.gain_map_max_n, 1);
    assert_eq!(ch.gain_map_max_d, 1);

    // AV1 data should match exactly
    assert_eq!(gm.gain_map_data, gain_map_av1);
}

#[test]
fn encode_gain_map_with_exif_and_icc() {
    let img = make_rgb8_image();
    let metadata = make_test_tmap_metadata();
    let (gain_map_av1, gw, gh, gdepth) = make_real_gain_map();

    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(10)
        .exif(vec![0xFF, 0xD8, 0x00, 0x00])
        .icc_profile(vec![0x00, 0x00, 0x02, 0x0C]) // minimal ICC header
        .with_gain_map(gain_map_av1.clone(), gw, gh, gdepth, metadata);

    let encoded = encode_rgb8(img.as_ref(), &config, stop())
        .expect("encode with gain map + metadata should succeed");

    let parser = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file)
        .expect("output should be valid AVIF");

    // Gain map present
    assert!(parser.gain_map_metadata().is_some());
    let gm_data = parser.gain_map_data().unwrap().unwrap();
    assert_eq!(gm_data.as_ref(), &gain_map_av1[..]);

    // EXIF also present
    assert!(parser.exif().is_some());
}

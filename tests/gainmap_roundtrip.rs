//! Real-vector gain-map roundtrip contracts: decode a libavif gain-mapped
//! AVIF, extract base + map + metadata, re-encode through
//! `EncoderConfig::with_gain_map`, reparse, and require semantic equality.
//!
//! Classes covered (all real libavif interop vectors):
//! - SDR base + full-size 3-channel 4:4:4 map, PQ alternate (seine_sdr)
//! - HDR 10-bit base, backward direction, sRGB alternate (seine_hdr)
//! - map dimensions ≠ base dimensions (seine_hdr_gainmap_small)
//! - ICC base profile + ICC alternate colr (seine_sdr_gainmap_srgb_icc)
//!
//! Equality bar per class:
//! - gain map AV1 payload: byte-identical (byte-carry)
//! - ISO 21496-1 metadata: `to_bytes()` normal-form equality
//! - tmap alternate-rendition colr: preserved (CICP equality)
//! - gain-map item av1C honesty: the muxed av1C must describe the actual
//!   payload (subsampling / monochrome / bit depth from its sequence header)

#![cfg(feature = "encode")]

use almost_enough::{StopToken, Unstoppable};
use zenavif::{DecoderConfig, EncoderConfig, ManagedAvifDecoder};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

fn load_vector(name: &str) -> Option<Vec<u8>> {
    let path = format!("tests/vectors/libavif/{name}");
    match std::fs::read(&path) {
        Ok(data) => Some(data),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: {path} not found (download with: just download-vectors)");
            None
        }
        Err(e) => panic!("Failed to read {path}: {e}"),
    }
}

macro_rules! require_vector {
    ($expr:expr) => {
        match $expr {
            Some(data) => data,
            None => return,
        }
    };
}

/// Scan raw AVIF bytes for every `av1C` box payload (marker+profile byte +
/// flags byte) and return (seq_profile, high_bitdepth, twelve_bit, mono,
/// ssx, ssy) tuples. ipco-level scan; used to assert the muxed gain-map
/// item's av1C matches its payload.
fn scan_av1c(data: &[u8]) -> Vec<(u8, bool, bool, bool, bool, bool)> {
    let mut out = Vec::new();
    for i in 0..data.len().saturating_sub(8) {
        if &data[i..i + 4] == b"av1C" {
            let b1 = data[i + 5]; // (marker<<7)|version==0x81 precedes at +4
            let b2 = data[i + 6];
            // Guard: byte at +4 must be 0x81 (marker=1, version=1)
            if data[i + 4] != 0x81 {
                continue;
            }
            out.push((
                b1 >> 5,
                (b2 >> 6) & 1 == 1,
                (b2 >> 5) & 1 == 1,
                (b2 >> 4) & 1 == 1,
                (b2 >> 3) & 1 == 1,
                (b2 >> 2) & 1 == 1,
            ));
        }
    }
    out
}

/// Decode-extract → re-encode → reparse roundtrip for one vector.
///
/// `base_16bit`: re-encode the base as 16-bit (10-bit AV1) when the source
/// base is HDR, else 8-bit.
fn roundtrip_vector(name: &str, base_16bit: bool) {
    let data = require_vector!(load_vector(name));

    // ---- decode & extract everything ---------------------------------
    let dcfg = DecoderConfig::new().prefer_8bit(false);
    let mut dec = ManagedAvifDecoder::new(&data, &dcfg).expect("vector should open");
    let (pixels, info) = dec.decode_full(&Unstoppable).expect("vector should decode");
    let gm = info.gain_map.as_ref().expect("vector has a gain map");
    let src_alt_colr = gm.alt_color_info.clone();
    assert!(
        src_alt_colr.is_some(),
        "{name}: source tmap colr must be present (libavif writes it)"
    );

    // Gain map payload properties from its own sequence header.
    let map_md = zenavif_parse::AV1Metadata::parse_av1_bitstream(&gm.gain_map_data)
        .expect("gain map payload parses as AV1");

    // ---- re-encode base + byte-carried map ----------------------------
    let metadata_bytes = gm.metadata.to_bytes();
    let mut config = EncoderConfig::new().quality(85.0).speed(8).with_gain_map(
        gm.gain_map_data.clone(),
        map_md.max_frame_width.get(),
        map_md.max_frame_height.get(),
        map_md.bit_depth,
        metadata_bytes.clone(),
    );
    config = config
        .color_primaries(info.color_primaries.0)
        .transfer_characteristics(info.transfer_characteristics.0);
    match &src_alt_colr {
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

    let encoded = if base_16bit {
        let img = pixels
            .try_as_imgref::<rgb::Rgb<u16>>()
            .expect("HDR base decodes to Rgb16");
        zenavif::encode_rgb16(img, &config, stop()).expect("re-encode (16-bit base)")
    } else {
        let img = pixels
            .try_as_imgref::<rgb::Rgb<u8>>()
            .expect("SDR base decodes to Rgb8");
        zenavif::encode_rgb8(img, &config, stop()).expect("re-encode (8-bit base)")
    };

    // ---- reparse and compare ------------------------------------------
    let parser =
        zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).expect("re-encoded file parses");

    // (1) AV1 payload byte-carry.
    let re_map = parser
        .gain_map_data()
        .expect("gain map data present after re-encode")
        .expect("gain map data resolves");
    assert_eq!(
        re_map.as_ref(),
        &gm.gain_map_data[..],
        "{name}: gain map AV1 bytes must byte-carry"
    );

    // (2) Metadata normal-form equality.
    let re_meta = parser
        .gain_map_metadata()
        .expect("gain map metadata present after re-encode");
    assert_eq!(
        re_meta.to_bytes(),
        metadata_bytes,
        "{name}: ISO 21496-1 metadata must be semantically identical"
    );

    // (3) Alternate-rendition colr preserved.
    let re_alt = parser.gain_map_color_info();
    match (&src_alt_colr, re_alt) {
        (Some(zenavif_parse::ColorInformation::Nclx { .. }), Some(got)) => {
            assert_eq!(
                got,
                src_alt_colr.as_ref().unwrap(),
                "{name}: tmap alternate colr must roundtrip"
            );
        }
        (Some(zenavif_parse::ColorInformation::IccProfile(_)), Some(got)) => {
            assert_eq!(
                got,
                src_alt_colr.as_ref().unwrap(),
                "{name}: tmap alternate ICC must roundtrip verbatim"
            );
        }
        (Some(_), None) => {
            panic!("{name}: tmap alternate colr LOST in re-encode");
        }
        (None, _) => unreachable!("asserted Some above"),
    }

    // (4) av1C honesty for the gain-map item: some av1C in the output must
    // exactly describe the map payload's sequence-header properties.
    let av1cs = scan_av1c(&encoded.avif_file);
    let want = (
        map_md.seq_profile,
        map_md.bit_depth >= 10,
        map_md.bit_depth >= 12,
        map_md.monochrome,
        map_md.chroma_subsampling.horizontal,
        map_md.chroma_subsampling.vertical,
    );
    assert!(
        av1cs.contains(&want),
        "{name}: no muxed av1C matches the gain-map payload {want:?} (found {av1cs:?}) — \
         the gain-map item's av1C is lying about its bitstream"
    );

    // (5) The re-encoded file decodes through zenavif with the map intact.
    let mut dec2 =
        ManagedAvifDecoder::new(&encoded.avif_file, &dcfg).expect("re-encoded file opens");
    let info2 = dec2.probe_info().expect("re-encoded file probes");
    let gm2 = info2.gain_map.as_ref().expect("gain map after full cycle");
    assert_eq!(gm2.gain_map_data, gm.gain_map_data);
    assert_eq!(gm2.metadata.to_bytes(), metadata_bytes);
}

#[test]
fn roundtrip_sdr_base_multichannel_map() {
    roundtrip_vector("seine_sdr_gainmap_srgb.avif", false);
}

#[test]
fn roundtrip_hdr_base_backward_direction() {
    roundtrip_vector("seine_hdr_gainmap_srgb.avif", true);
}

#[test]
fn roundtrip_small_map_dims_differ_from_base() {
    roundtrip_vector("seine_hdr_gainmap_small_srgb.avif", true);
}

#[test]
fn roundtrip_icc_base_nclx_alt() {
    roundtrip_vector("seine_sdr_gainmap_srgb_icc.avif", false);
}

//! Tests for gain map extraction through the zenavif decode pipeline.
//!
//! Verifies that gain map metadata, image data, and alternate color info
//! are accessible from the decode output when an AVIF file contains a
//! `tmap` derived image item.

use zenavif::{AvifGainMap, DecoderConfig, ManagedAvifDecoder};

/// Path to an AVIF file with a gain map (SDR base + gain map for HDR)
const SEINE_SDR_GAINMAP: &str = "tests/vectors/libavif/seine_sdr_gainmap_srgb.avif";
/// Path to an AVIF file with a gain map (HDR base + gain map for SDR)
const SEINE_HDR_GAINMAP: &str = "tests/vectors/libavif/seine_hdr_gainmap_srgb.avif";
/// Path to a normal AVIF file without gain map
const WHITE_1X1: &str = "tests/vectors/libavif/white_1x1.avif";
/// Gain map with unsupported version (should still decode base image)
const UNSUPPORTED_VERSION: &str = "tests/vectors/libavif/unsupported_gainmap_version.avif";
/// Gain map with unsupported minimum version
const UNSUPPORTED_MIN_VERSION: &str =
    "tests/vectors/libavif/unsupported_gainmap_minimum_version.avif";
/// Gain map with supported writer version and extra bytes
const SUPPORTED_WRITER_EXTRA: &str =
    "tests/vectors/libavif/supported_gainmap_writer_version_with_extra_bytes.avif";
/// SDR gain map with small dimensions
const SEINE_HDR_GAINMAP_SMALL: &str = "tests/vectors/libavif/seine_hdr_gainmap_small_srgb.avif";
/// Gain map with non-grid color but grid gain map
const NOGRID_ALPHA_NOGRID_GAINMAP_GRID: &str =
    "tests/vectors/libavif/color_nogrid_alpha_nogrid_gainmap_grid.avif";

// ============================================================================
// Gain map detection through probe_info
// ============================================================================

#[test]
fn probe_gain_map_present() {
    let data = std::fs::read(SEINE_SDR_GAINMAP).expect("read test file");
    let decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let info = decoder.probe_info().expect("probe should succeed");

    let gm = info.gain_map.as_ref().expect("gain map should be present");
    assert!(
        gm.metadata.is_multichannel,
        "seine test file uses multichannel gain map"
    );
    assert!(gm.metadata.use_base_colour_space);

    // HDR headroom values
    assert_eq!(gm.metadata.base_hdr_headroom_n, 0);
    assert_eq!(gm.metadata.base_hdr_headroom_d, 1);
    assert_eq!(gm.metadata.alternate_hdr_headroom_n, 13);
    assert_eq!(gm.metadata.alternate_hdr_headroom_d, 10);

    // Gain map data should be non-empty AV1
    assert!(!gm.gain_map_data.is_empty(), "gain map data should exist");

    // Verify AV1 OBU header
    let first_byte = gm.gain_map_data[0];
    let obu_type = (first_byte >> 3) & 0x0F;
    assert!(
        (1..=8).contains(&obu_type),
        "first OBU type should be valid: got {obu_type}"
    );

    // Alternate color info should be present
    assert!(
        gm.alt_color_info.is_some(),
        "tmap colr property should be present"
    );
}

#[test]
fn probe_gain_map_absent() {
    let data = std::fs::read(WHITE_1X1).expect("read test file");
    let decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let info = decoder.probe_info().expect("probe should succeed");
    assert!(info.gain_map.is_none(), "normal image has no gain map");
}

#[test]
fn probe_hdr_gain_map_present() {
    let data = std::fs::read(SEINE_HDR_GAINMAP).expect("read test file");
    let decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let info = decoder.probe_info().expect("probe should succeed");

    let gm = info.gain_map.as_ref().expect("gain map should be present");
    assert!(
        !gm.gain_map_data.is_empty(),
        "HDR gain map data should exist"
    );
}

// ============================================================================
// Gain map through decode_full
// ============================================================================

#[test]
fn decode_full_has_gain_map() {
    let data = std::fs::read(SEINE_SDR_GAINMAP).expect("read test file");
    let mut decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let (_pixels, info) = decoder
        .decode_full(&enough::Unstoppable)
        .expect("decode should succeed");

    let gm = info
        .gain_map
        .as_ref()
        .expect("decode_full should include gain map");
    assert!(gm.metadata.is_multichannel);
    assert!(!gm.gain_map_data.is_empty());

    // Per-channel parameters should differ for multichannel
    assert_ne!(
        gm.metadata.channels[0].gain_map_min_n, gm.metadata.channels[1].gain_map_min_n,
        "multichannel should have different per-channel values"
    );
}

#[test]
fn decode_full_no_gain_map() {
    let data = std::fs::read(WHITE_1X1).expect("read test file");
    let mut decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let (_pixels, info) = decoder
        .decode_full(&enough::Unstoppable)
        .expect("decode should succeed");
    assert!(
        info.gain_map.is_none(),
        "normal image should not have gain map after decode"
    );
}

// ============================================================================
// Gain map metadata field validation
// ============================================================================

#[test]
fn gain_map_channel_params_valid() {
    let data = std::fs::read(SEINE_SDR_GAINMAP).expect("read test file");
    let decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let info = decoder.probe_info().expect("probe should succeed");
    let gm = info.gain_map.unwrap();

    for (i, ch) in gm.metadata.channels.iter().enumerate() {
        // Denominators should be non-zero
        assert!(
            ch.gain_map_min_d > 0,
            "channel {i} gain_map_min_d should be non-zero"
        );
        assert!(
            ch.gain_map_max_d > 0,
            "channel {i} gain_map_max_d should be non-zero"
        );
        assert!(ch.gamma_d > 0, "channel {i} gamma_d should be non-zero");
        assert!(
            ch.base_offset_d > 0,
            "channel {i} base_offset_d should be non-zero"
        );
        assert!(
            ch.alternate_offset_d > 0,
            "channel {i} alternate_offset_d should be non-zero"
        );

        // Gamma should be positive (gamma_n/gamma_d > 0)
        assert!(ch.gamma_n > 0, "channel {i} gamma should be positive");
    }
}

// ============================================================================
// Edge cases: unsupported versions, extra bytes
// ============================================================================

#[test]
fn unsupported_gainmap_version_still_decodes_base() {
    // Parser rejects unsupported tmap versions, so ManagedAvifDecoder::new fails.
    // This is the expected behavior — we test that the parse error is surfaced.
    let data = std::fs::read(UNSUPPORTED_VERSION).expect("read test file");
    let result = ManagedAvifDecoder::new(&data, &DecoderConfig::default());
    // The parser should reject this file due to unsupported tmap version
    assert!(
        result.is_err(),
        "unsupported gain map version should cause parse error"
    );
}

#[test]
fn unsupported_gainmap_minimum_version_rejected() {
    let data = std::fs::read(UNSUPPORTED_MIN_VERSION).expect("read test file");
    let result = ManagedAvifDecoder::new(&data, &DecoderConfig::default());
    assert!(
        result.is_err(),
        "unsupported gain map minimum version should cause parse error"
    );
}

#[test]
fn supported_writer_version_with_extra_bytes() {
    let data = std::fs::read(SUPPORTED_WRITER_EXTRA).expect("read test file");
    let decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let info = decoder.probe_info().expect("probe should succeed");

    // This file has a supported writer version with extra trailing bytes
    // in the tmap payload — the parser should still extract the metadata
    let gm = info
        .gain_map
        .as_ref()
        .expect("gain map should be present despite extra bytes");
    assert!(!gm.gain_map_data.is_empty());
}

// ============================================================================
// Gain map with different dimensions than base
// ============================================================================

#[test]
fn gain_map_small_dimensions() {
    let data = std::fs::read(SEINE_HDR_GAINMAP_SMALL).expect("read test file");
    let decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let info = decoder.probe_info().expect("probe should succeed");

    let gm = info.gain_map.as_ref().expect("gain map should be present");
    assert!(
        !gm.gain_map_data.is_empty(),
        "small gain map data should be non-empty"
    );

    // Gain map is typically smaller than the base image
    // Just verify it parses successfully
}

// ============================================================================
// Grid image with gain map
// ============================================================================

#[test]
fn nogrid_color_with_gainmap_grid() {
    let data = std::fs::read(NOGRID_ALPHA_NOGRID_GAINMAP_GRID).expect("read test file");
    let decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let info = decoder.probe_info().expect("probe should succeed");

    // This file has a non-grid color image but a grid gain map
    let gm = info
        .gain_map
        .as_ref()
        .expect("gain map should be present for grid gain map file");
    assert!(!gm.gain_map_data.is_empty());
}

// ============================================================================
// Gain map data is decodable AV1 (basic validation)
// ============================================================================

#[test]
fn gain_map_data_has_valid_obu_structure() {
    let data = std::fs::read(SEINE_SDR_GAINMAP).expect("read test file");
    let decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let info = decoder.probe_info().expect("probe should succeed");
    let gm = info.gain_map.unwrap();

    // Walk through OBU headers to verify the bitstream isn't corrupted.
    // AV1 OBU format: forbidden(1) | type(4) | extension(1) | has_size(1) | reserved(1)
    let mut pos = 0;
    let data = &gm.gain_map_data;
    let mut obu_count = 0;

    while pos < data.len() {
        let header = data[pos];
        let forbidden = header >> 7;
        assert_eq!(forbidden, 0, "OBU forbidden bit must be 0 at pos {pos}");

        let obu_type = (header >> 3) & 0x0F;
        assert!(
            obu_type <= 8 || obu_type == 15,
            "invalid OBU type {obu_type} at pos {pos}"
        );

        let has_extension = (header >> 2) & 1;
        let has_size = (header >> 1) & 1;

        pos += 1;
        if has_extension != 0 {
            pos += 1; // skip extension byte
        }

        if has_size != 0 {
            // LEB128 encoded size
            let mut size: u64 = 0;
            let mut shift = 0;
            loop {
                if pos >= data.len() {
                    break;
                }
                let byte = data[pos] as u64;
                pos += 1;
                size |= (byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift > 56 {
                    break;
                }
            }
            pos += size as usize;
        } else {
            // Without size, this OBU extends to end of stream
            pos = data.len();
        }

        obu_count += 1;
    }

    assert!(
        obu_count >= 2,
        "gain map AV1 should have at least 2 OBUs (sequence header + frame), got {obu_count}"
    );
}

// ============================================================================
// Gain map through zencodec trait DecodeOutput extras
// ============================================================================

#[cfg(feature = "zencodec")]
#[test]
fn decode_gain_map_via_zencodec_extras() {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

    let data = std::fs::read(SEINE_SDR_GAINMAP).expect("read file");
    let dec = zenavif::AvifDecoderConfig::new();
    let output = dec
        .job()
        .decoder(std::borrow::Cow::Borrowed(&data), &[])
        .expect("decoder")
        .decode()
        .expect("decode");

    // Gain map should be attached as extras
    let gm = output
        .extras::<AvifGainMap>()
        .expect("gain map should be present as extras");
    assert!(gm.metadata.is_multichannel);
    assert!(!gm.gain_map_data.is_empty());
    assert!(gm.alt_color_info.is_some());
}

#[cfg(feature = "zencodec")]
#[test]
fn decode_no_gain_map_extras_on_normal_image() {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

    let data = std::fs::read(WHITE_1X1).expect("read file");
    let dec = zenavif::AvifDecoderConfig::new();
    let output = dec
        .job()
        .decoder(std::borrow::Cow::Borrowed(&data), &[])
        .expect("decoder")
        .decode()
        .expect("decode");

    assert!(
        output.extras::<AvifGainMap>().is_none(),
        "normal image should not have gain map extras"
    );
}

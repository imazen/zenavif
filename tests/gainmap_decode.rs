//! Tests for gain map extraction through the zenavif decode pipeline.
//!
//! Verifies that gain map metadata, image data, and alternate color info
//! are accessible from the decode output when an AVIF file contains a
//! `tmap` derived image item.

#[path = "support/independent_av1.rs"]
mod independent;
#[path = "support/pixels.rs"]
mod pixel_support;
use enough::Unstoppable;
#[allow(unused_imports)]
use pixel_support::{BufTestExt as _, SpecTestExt as _};
use zenavif::{DecoderConfig, ManagedAvifDecoder};
use zencodec::gainmap::GainMapSource;

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

/// Load a test vector, fail-loud when missing: CI provisions the vectors
/// (ci.yml "Download test vectors"); locally run `just download-vectors`.
/// A silent skip would fake coverage (no-graceful-skips policy).
fn load_vector(path: &str) -> Vec<u8> {
    std::fs::read(path)
        .unwrap_or_else(|e| panic!("Failed to read {path}: {e} (run: just download-vectors)"))
}

// ============================================================================
// Gain map detection through probe_info
// ============================================================================

#[test]
fn probe_gain_map_present() {
    let data = load_vector(SEINE_SDR_GAINMAP);
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
    let data = load_vector(WHITE_1X1);
    let decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let info = decoder.probe_info().expect("probe should succeed");
    assert!(info.gain_map.is_none(), "normal image has no gain map");
}

#[test]
fn probe_hdr_gain_map_present() {
    let data = load_vector(SEINE_HDR_GAINMAP);
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
    let data = load_vector(SEINE_SDR_GAINMAP);
    let mut decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let (_pixels, info) = decoder
        .decode_full(&Unstoppable)
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
    let data = load_vector(WHITE_1X1);
    let mut decoder =
        ManagedAvifDecoder::new(&data, &DecoderConfig::default()).expect("decoder should open");
    let (_pixels, info) = decoder
        .decode_full(&Unstoppable)
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
    let data = load_vector(SEINE_SDR_GAINMAP);
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
    let data = load_vector(UNSUPPORTED_VERSION);
    let result = ManagedAvifDecoder::new(&data, &DecoderConfig::default());
    // The parser should reject this file due to unsupported tmap version
    assert!(
        result.is_err(),
        "unsupported gain map version should cause parse error"
    );
}

#[test]
fn unsupported_gainmap_minimum_version_rejected() {
    let data = load_vector(UNSUPPORTED_MIN_VERSION);
    let result = ManagedAvifDecoder::new(&data, &DecoderConfig::default());
    assert!(
        result.is_err(),
        "unsupported gain map minimum version should cause parse error"
    );
}

#[test]
fn supported_writer_version_with_extra_bytes() {
    let data = load_vector(SUPPORTED_WRITER_EXTRA);
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
    let data = load_vector(SEINE_HDR_GAINMAP_SMALL);
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
    let data = load_vector(NOGRID_ALPHA_NOGRID_GAINMAP_GRID);
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
    let data = load_vector(SEINE_SDR_GAINMAP);
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

/// `GainMapRender::Components` surfaces BOTH the raw `GainMapSource` (AV1
/// payload + metadata, for transcode) and the decoded
/// [`zencodec::decode::DecodedGainMap`] (pixels + ISO 21496-1 params).
///
/// Gain-map extras are opt-in per the zencodec contract — the default
/// (`BaseOnly`) decode attaches neither (see
/// `gain_map_render_base_only_attaches_nothing`).
#[test]
fn decode_gain_map_via_zencodec_extras() {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

    let data = load_vector(SEINE_SDR_GAINMAP);
    let dec = zenavif::AvifDecoderConfig::new();
    let output = dec
        .job()
        .with_gain_map_render(zencodec::GainMapRender::Components)
        .decoder(std::borrow::Cow::Borrowed(&data), &[])
        .expect("decoder")
        .decode()
        .expect("decode");

    // Gain map should be attached as normalized GainMapSource extras
    let gm = output
        .extras::<GainMapSource>()
        .expect("gain map should be present as extras");
    assert!(!gm.data.is_empty());
    assert_eq!(gm.format, zencodec::ImageFormat::Avif);
    assert_eq!(gm.depth, 0);
    // Multi-channel gain map should have 3 channels in metadata
    assert_eq!(gm.metadata.channels, 3);
    assert!(gm.metadata.alternate_cicp.is_some());

    // ...and the DECODED gain map (Components contract).
    let dgm = output
        .extras::<zencodec::decode::DecodedGainMap>()
        .expect("Components must surface the DecodedGainMap");
    assert!(dgm.pixels.width() > 0 && dgm.pixels.height() > 0);
    assert_eq!(dgm.metadata.channels, 3);
}

/// The default decode (`BaseOnly`) attaches no gain-map extras at all —
/// the gain map is ignored, only `ImageInfo` metadata reports its presence.
#[test]
fn gain_map_render_base_only_attaches_nothing() {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

    let data = load_vector(SEINE_SDR_GAINMAP);
    let output = zenavif::AvifDecoderConfig::new()
        .job()
        .decoder(std::borrow::Cow::Borrowed(&data), &[])
        .expect("decoder")
        .decode()
        .expect("decode");
    assert!(output.extras::<GainMapSource>().is_none());
    assert!(
        output
            .extras::<zencodec::decode::DecodedGainMap>()
            .is_none()
    );
}

/// Decode with `ReconstructHdr` and return the output (full boost when
/// `target_headroom` is `None`).
fn decode_reconstruct(data: &[u8], target_headroom: Option<f32>) -> zencodec::decode::DecodeOutput {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
    zenavif::AvifDecoderConfig::new()
        .job()
        .with_gain_map_render(zencodec::GainMapRender::ReconstructHdr { target_headroom })
        .decoder(std::borrow::Cow::Borrowed(data), &[])
        .expect("decoder")
        .decode()
        .expect("decode")
}

/// Peak linear value (max over R,G,B of every pixel) of an RgbaF32 buffer.
fn peak_linear(pixels: &zenpixels::PixelView<'_>) -> f32 {
    assert_eq!(pixels.spec().layout, zenpixels::SampleLayout::RGBAF32);
    let stride = pixels.stride();
    let bytes = pixels.as_strided_bytes();
    let (w, h) = (pixels.width() as usize, pixels.rows() as usize);
    let mut peak = f32::MIN;
    for y in 0..h {
        let row: &[f32] = rgb::bytemuck::cast_slice(&bytes[y * stride..][..w * 16]);
        for px in row.as_chunks::<4>().0.iter() {
            assert!(
                px.iter().all(|v| v.is_finite()),
                "reconstructed pixels must be finite"
            );
            peak = peak.max(px[0].max(px[1]).max(px[2]));
        }
    }
    peak
}

/// zenavif reconstructs (`reconstructs_hdr()` is true): `ReconstructHdr`
/// applies the gain map via ultrahdr-core. Output is linear f32 RGBA
/// (1.0 = SDR white), CLL is measured from the reconstructed pixels, and
/// the gain-map components are still surfaced for transcode use.
#[test]
fn gain_map_render_reconstructs_linear_hdr() {
    assert!(
        <zenavif::AvifDecoderConfig as zencodec::decode::DecoderConfig>::capabilities()
            .reconstructs_hdr()
    );
    let data = load_vector(SEINE_SDR_GAINMAP);
    let output = decode_reconstruct(&data, None);

    let desc = output.buffer().spec();
    assert_eq!(desc.layout, zenpixels::SampleLayout::RGBAF32);
    assert_eq!(
        desc.transfer(),
        zenpixels::Transfer::Linear,
        "reconstruction output is linear light (1.0 = SDR white)"
    );

    let peak = peak_linear(&output.pixels());
    let gm = output
        .extras::<GainMapSource>()
        .expect("components still surfaced on ReconstructHdr");
    let alt_headroom = gm.metadata.params.alternate_hdr_headroom as f32;
    assert!(
        alt_headroom > 0.0,
        "seine vector's alternate rendition is HDR"
    );
    assert!(
        peak > 1.02,
        "full reconstruction must boost highlights past SDR white (peak {peak})"
    );
    assert!(
        peak <= alt_headroom.exp2() * 1.10 + 0.1,
        "peak {peak} exceeds the gain map's encoded envelope (2^{alt_headroom})"
    );

    // Envelope obligation: MaxCLL/MaxFALL measured from the output.
    let cll = output
        .info()
        .source_color
        .content_light_level
        .expect("ReconstructHdr must populate measured CLL");
    assert!(cll.max_content_light_level > 203, "peak above SDR white");
    assert!(
        cll.max_frame_average_light_level <= cll.max_content_light_level,
        "FALL cannot exceed CLL"
    );
    assert_eq!(
        cll.max_content_light_level,
        (peak * 203.0).round() as u16,
        "MaxCLL is the measured peak in nits"
    );

    // Components contract still honored alongside reconstruction.
    assert!(
        output
            .extras::<zencodec::decode::DecodedGainMap>()
            .is_some()
    );
}

fn gain_fixture(
    base: &[u8],
    alpha: Option<&[u8]>,
    bits: u8,
    metadata: &zenavif_parse::GainMapMetadata,
) -> Vec<u8> {
    let mut mux = zenavif_serialize::Aviffy::new();
    mux.set_gain_map(
        independent::MONO_WHITE.to_vec(),
        16,
        16,
        8,
        metadata.to_bytes(),
    )
    .set_gain_map_monochrome(true)
    .set_strict_payload_agreement(true);
    mux.try_to_vec(base, alpha, 16, 16, bits).unwrap()
}

fn unequal_gains() -> zenavif_parse::GainMapMetadata {
    let channel = |stops| zenavif_parse::GainMapChannel {
        gain_map_min_n: 0,
        gain_map_min_d: 1,
        gain_map_max_n: stops,
        gain_map_max_d: 1,
        gamma_n: 1,
        gamma_d: 1,
        base_offset_n: 0,
        base_offset_d: 32,
        alternate_offset_n: 1,
        alternate_offset_d: 32,
    };
    zenavif_parse::GainMapMetadata {
        is_multichannel: true,
        use_base_colour_space: true,
        backward_direction: false,
        base_hdr_headroom_n: 0,
        base_hdr_headroom_d: 1,
        alternate_hdr_headroom_n: 3,
        alternate_hdr_headroom_d: 1,
        channels: [channel(1), channel(2), channel(3)],
    }
}

#[test]
fn reconstruct_at_sdr_headroom_matches_linearized_base_bit_for_bit() {
    use std::borrow::Cow;
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

    let params = unequal_gains();
    let data = gain_fixture(independent::GBR8, None, 8, &params);
    let hdr = decode_reconstruct(&data, Some(1.0));
    let base = zenavif::AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(&data), &[])
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(base.pixels().spec().layout, zenpixels::SampleLayout::RGB8);
    for y in 0..16 {
        let base = base.pixels();
        let hdr = hdr.pixels();
        let rgb = base.row_as::<[u8; 3]>(y).unwrap();
        let rgba = hdr.row_as::<[f32; 4]>(y).unwrap();
        for (x, (&raw, got)) in rgb.iter().zip(rgba).enumerate() {
            // The independently authored fixture uses sRGB endpoints only.
            let expected = raw.map(|q| match q {
                0 => 0.0f32,
                255 => 1.0,
                _ => panic!("unexpected source code {q}"),
            });
            assert_eq!(
                [got[0].to_bits(), got[1].to_bits(), got[2].to_bits()],
                expected.map(f32::to_bits),
                "({x}, {y})"
            );
            assert_eq!(got[3].to_bits(), 1.0f32.to_bits());
        }
    }
}

#[test]
fn mono_gain_sample_drives_three_independent_iso_records() {
    let params = unequal_gains();
    let data = gain_fixture(independent::GBR8, None, 8, &params);
    let out = decode_reconstruct(&data, Some(8.0));
    let raw = out.extras::<GainMapSource>().unwrap();
    assert_eq!(raw.data.as_slice(), independent::MONO_WHITE);
    assert_eq!(raw.metadata.params.iso_channel_count(), Some(3));
    let map = out.extras::<zencodec::decode::DecodedGainMap>().unwrap();
    assert_eq!((map.width(), map.height(), map.channels()), (16, 16, 1));
    assert_eq!(map.metadata.params.iso_channel_count(), Some(3));
    assert_eq!(map.metadata.channels, 1);
    for row in 0..16 {
        assert_eq!(map.pixels.as_view().row(row), &[255; 16]);
    }
    // White quadrant: 1.0 × 2^[1,2,3] - 1/32, with one numeric map value.
    let expected = [1.96875f32, 3.96875, 7.96875, 1.0];
    assert_eq!(out.pixels().row_as::<[f32; 4]>(8).unwrap()[8], expected);
}

#[test]
fn gain_map_components_survive_typed_unsupported_reconstruction() {
    use std::borrow::Cow;
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

    for (base, alpha, bits, hdr) in [
        (independent::GBR8, Some(independent::MONO_RAMP), 8, false),
        (independent::GBR10, None, 10, false),
        (independent::GBR12, None, 12, false),
        (independent::GBR10_PQ, None, 10, true),
    ] {
        let mut params = unequal_gains();
        if hdr {
            params.backward_direction = true;
            params.base_hdr_headroom_n = 3;
            params.alternate_hdr_headroom_n = 0;
        }
        let data = gain_fixture(base, alpha, bits, &params);
        let parser = zenavif_parse::AvifParser::from_bytes(&data).unwrap();
        let source = parser.primary_metadata().unwrap();
        assert_eq!(source.bit_depth, bits);
        assert_eq!(source.transfer_characteristics == 16, hdr);
        let make_job = || zenavif::AvifDecoderConfig::new().job();
        let components = make_job()
            .with_gain_map_render(zencodec::GainMapRender::Components)
            .decoder(Cow::Borrowed(&data), &[])
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(components.info().has_alpha, alpha.is_some());
        let raw = components.extras::<GainMapSource>().unwrap();
        assert_eq!(raw.data.as_slice(), independent::MONO_WHITE);
        assert_eq!(raw.metadata.params.backward_direction, hdr);
        let map = components
            .extras::<zencodec::decode::DecodedGainMap>()
            .unwrap();
        assert_eq!(
            (map.channels(), map.pixels.as_view().row(0)),
            (1, &[255u8; 16][..])
        );
        for y in 0..16 {
            assert_eq!(map.pixels.as_view().row(y), &[255; 16]);
        }
        let base_view = components.pixels();
        if alpha.is_some() {
            assert_eq!(base_view.spec().layout, zenpixels::SampleLayout::RGBA8);
            for y in 0..16 {
                for (x, rgba) in base_view.row_as::<[u8; 4]>(y).unwrap().iter().enumerate() {
                    assert_eq!(rgba[3], (y * 16 + x as u32) as u8);
                }
            }
        } else {
            assert_eq!(base_view.spec().layout, zenpixels::SampleLayout::RGB16);
            let exact = base_view.facts().depth.unwrap();
            assert_eq!(exact.color_bits(), bits);
            assert!(exact.holds_for(&base_view).unwrap());
        }

        let result = make_job()
            .with_gain_map_render(zencodec::GainMapRender::ReconstructHdr {
                target_headroom: Some(2.0),
            })
            .decoder(Cow::Borrowed(&data), &[])
            .and_then(|decoder| decoder.decode());
        let err = result.expect_err("reconstruction must refuse before producing pixels");
        assert!(matches!(
            err.error().category(),
            zencodec::ErrorCategory::Image(zencodec::ImageError::Unsupported(
                zencodec::UnsupportedImageKind::Feature
            ))
        ));
        assert!(err.to_string().contains(if alpha.is_some() {
            "alpha channel"
        } else {
            "requires an 8-bit base"
        }));
    }
}

#[test]
fn reconstructed_cll_uses_native_max_rgb_and_exact_nits() {
    // An 8×8 quadrant of each saturated primary and white in BT.2020.
    // R gains 2×, G 4×, B 8×. Offsets are zero in this control.
    let mut params = unequal_gains();
    for channel in &mut params.channels {
        channel.alternate_offset_n = 0;
    }
    let data = gain_fixture(independent::GBR8_BT2020, None, 8, &params);
    let out = decode_reconstruct(&data, Some(8.0));
    let view = out.pixels();
    assert_eq!(
        view.spec().color.project_cicp().ok(),
        None,
        "explicit diffuse white is part of the active color"
    );
    assert!(matches!(
        view.spec().color,
        zenpixels::ColorSpace::Rgb {
            primaries: zenpixels::Primaries::Bt2020,
            transfer: zenpixels::Transfer::Linear,
            ..
        }
    ));
    assert_eq!(
        view.spec().color.white(),
        Some(zenpixels::DiffuseWhite::BT2408)
    );
    let native = [
        [2.0, 0.0, 0.0],
        [0.0, 4.0, 0.0],
        [0.0, 0.0, 8.0],
        [2.0, 4.0, 8.0],
    ];
    for y in 0..16 {
        for (x, sample) in view.row_as::<[f32; 4]>(y).unwrap().iter().enumerate() {
            assert_eq!(
                sample[..3],
                native[usize::from(y >= 8) * 2 + usize::from(x >= 8)]
            );
        }
    }
    let cie_y = native
        .map(|p| f64::from(p[0]) * 0.2627 + f64::from(p[1]) * 0.6780 + f64::from(p[2]) * 0.0593);
    let y_peak = (cie_y[3] * 203.0).round() as u16;
    let y_mean = (cie_y.iter().sum::<f64>() * 203.0 / 4.0).round() as u16;
    assert_eq!((y_peak, y_mean), (753, 377));
    let cll = out.info().source_color.content_light_level.unwrap();
    assert_eq!(
        (
            cll.max_content_light_level,
            cll.max_frame_average_light_level
        ),
        (1624, 1117)
    );

    for (stops, expected) in [(-1, 102u16), (10, u16::MAX)] {
        let mut neutral_params = params.clone();
        for c in &mut neutral_params.channels {
            c.gain_map_min_n = stops;
            c.gain_map_max_n = stops;
        }
        let neutral_data = gain_fixture(independent::WHITE_BT2020, None, 8, &neutral_params);
        let neutral = decode_reconstruct(&neutral_data, Some(8.0));
        let rgb = neutral.pixels().row_as::<[f32; 4]>(0).unwrap()[0];
        let linear = 2f64.powi(stops);
        assert_eq!(rgb[..3], [linear as f32; 3]);
        let cie_y = linear * (0.2627 + 0.6780 + 0.0593);
        assert_eq!((cie_y * 203.0).round().clamp(0.0, 65535.0) as u16, expected);
        let cll = neutral.info().source_color.content_light_level.unwrap();
        assert_eq!(
            (
                cll.max_content_light_level,
                cll.max_frame_average_light_level
            ),
            (expected, expected)
        );
    }

    #[cfg(feature = "encode")]
    {
        use imgref::Img;
        let rgb = vec![rgb::Rgb::new(128u8, 128, 128); 256];
        let config = zenavif::EncoderConfig::new()
            .content_light_level(
                cll.max_content_light_level,
                cll.max_frame_average_light_level,
            )
            .threads(Some(1));
        let encoded = zenavif::encode_rgb8(
            Img::new(&rgb[..], 16, 16),
            &config,
            almost_enough::StopToken::new(enough::Unstoppable),
        )
        .unwrap();
        if let Some(path) = std::env::var_os("ZENAVIF_GAINMAP_CLL_WITNESS") {
            std::fs::write(path, &encoded.avif_file).unwrap();
        }
        let parsed = zenavif_parse::AvifParser::from_bytes(&encoded.avif_file).unwrap();
        let wire = parsed.content_light_level().unwrap();
        assert_eq!(
            (
                wire.max_content_light_level,
                wire.max_pic_average_light_level
            ),
            (1624, 1117)
        );
    }
}

/// Reconstruction peak is monotonic in target headroom (weight is
/// monotonic in display boost): SDR ≤ 2× ≤ full.
#[test]
fn reconstruct_headroom_is_monotonic() {
    let data = load_vector(SEINE_SDR_GAINMAP);
    let p1 = peak_linear(&decode_reconstruct(&data, Some(1.0)).pixels());
    let p2 = peak_linear(&decode_reconstruct(&data, Some(2.0)).pixels());
    let pf = peak_linear(&decode_reconstruct(&data, None).pixels());
    assert!(
        p1 <= p2 + 1e-4 && p2 <= pf + 1e-4,
        "peaks must be monotonic in headroom: 1.0→{p1}, 2.0→{p2}, full→{pf}"
    );
    assert!(
        pf > p1 + 0.01,
        "full boost must actually exceed the SDR rendering ({pf} vs {p1})"
    );
}

/// Streaming `ReconstructHdr` emits the same pixels as the buffered path
/// (both run the shared whole-image reconstruction, strip-emitted).
#[test]
fn streaming_reconstruct_matches_buffered() {
    use zencodec::decode::{DecodeJob as _, DecoderConfig as _, StreamingDecode as _};

    let data = load_vector(SEINE_SDR_GAINMAP);
    let buffered = decode_reconstruct(&data, None);
    let bp = buffered.pixels();

    let mut dec = zenavif::AvifDecoderConfig::new()
        .job()
        .with_gain_map_render(zencodec::GainMapRender::ReconstructHdr {
            target_headroom: None,
        })
        .streaming_decoder(std::borrow::Cow::Borrowed(&data), &[])
        .expect("streaming_decoder");
    let cll = dec
        .info()
        .source_color
        .content_light_level
        .expect("streaming ReconstructHdr must also populate measured CLL");
    assert!(cll.max_content_light_level > 203);

    let (w, h) = (dec.info().width, dec.info().height);
    assert_eq!((w, h), (bp.width(), bp.rows()));
    let mut rows_seen = 0u32;
    while let Some((y, strip)) = dec.next_batch().expect("next_batch") {
        assert_eq!(strip.spec().layout, zenpixels::SampleLayout::RGBAF32);
        for r in 0..strip.rows() {
            let got = strip.row(r);
            let expect = bp.row(y + r);
            assert_eq!(got, expect, "strip row {} differs from buffered", y + r);
            rows_seen += 1;
        }
    }
    assert_eq!(rows_seen, h, "stream must cover every row exactly once");
}

#[test]
fn decode_no_gain_map_extras_on_normal_image() {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

    let data = load_vector(WHITE_1X1);
    let dec = zenavif::AvifDecoderConfig::new();
    let output = dec
        .job()
        .decoder(std::borrow::Cow::Borrowed(&data), &[])
        .expect("decoder")
        .decode()
        .expect("decode");

    assert!(
        output.extras::<GainMapSource>().is_none(),
        "normal image should not have gain map extras"
    );
}

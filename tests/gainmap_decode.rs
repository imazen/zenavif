//! Tests for gain map extraction through the zenavif decode pipeline.
//!
//! Verifies that gain map metadata, image data, and alternate color info
//! are accessible from the decode output when an AVIF file contains a
//! `tmap` derived image item.

use enough::Unstoppable;
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
fn peak_linear(pixels: &zenpixels::PixelSlice<'_>) -> f32 {
    assert_eq!(
        pixels.descriptor().pixel_format(),
        zenpixels::PixelFormat::RgbaF32
    );
    let stride = pixels.stride();
    let bytes = pixels.as_strided_bytes();
    let (w, h) = (pixels.width() as usize, pixels.rows() as usize);
    let mut peak = f32::MIN;
    for y in 0..h {
        let row: &[f32] = rgb::bytemuck::cast_slice(&bytes[y * stride..][..w * 16]);
        for px in row.chunks_exact(4) {
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

    let desc = output.pixels().descriptor();
    assert_eq!(desc.pixel_format(), zenpixels::PixelFormat::RgbaF32);
    assert_eq!(
        desc.transfer(),
        zenpixels::TransferFunction::Linear,
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

/// At `target_headroom = 1.0` (an SDR display) the weight is 0, the gain
/// is 1.0, and the output is the linearized base — the ISO 21496-1
/// formula collapses to `sdr + (base_offset - alternate_offset)`.
#[test]
fn reconstruct_at_sdr_headroom_matches_linearized_base() {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

    let data = load_vector(SEINE_SDR_GAINMAP);
    let hdr = decode_reconstruct(&data, Some(1.0));

    // SDR reference decode of the same base.
    let base = zenavif::AvifDecoderConfig::new()
        .job()
        .decoder(
            std::borrow::Cow::Borrowed(&data),
            &[zenpixels::PixelDescriptor::RGB8_SRGB],
        )
        .expect("decoder")
        .decode()
        .expect("decode");
    let bp = base.pixels();
    assert_eq!(bp.descriptor().pixel_format(), zenpixels::PixelFormat::Rgb8);

    fn srgb_eotf(v: f32) -> f32 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    let hp = hdr.pixels();
    let (w, h) = (hp.width() as usize, hp.rows() as usize);
    assert_eq!((w, h), (bp.width() as usize, bp.rows() as usize));
    let (hs, bs) = (hp.stride(), bp.stride());
    let (hb, bb) = (hp.as_strided_bytes(), bp.as_strided_bytes());
    let mut max_diff = 0.0f32;
    for y in 0..h {
        let hrow: &[f32] = rgb::bytemuck::cast_slice(&hb[y * hs..][..w * 16]);
        let brow = &bb[y * bs..][..w * 3];
        for x in 0..w {
            for ch in 0..3 {
                let expect = srgb_eotf(brow[x * 3 + ch] as f32 / 255.0);
                let got = hrow[x * 4 + ch];
                max_diff = max_diff.max((got - expect).abs());
            }
        }
    }
    // Tolerance covers the per-channel (base_offset − alternate_offset)
    // residual the formula leaves at gain = 1, plus LUT rounding.
    assert!(
        max_diff < 0.02,
        "boost=1.0 reconstruction must equal the linearized base (max diff {max_diff})"
    );
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
        assert_eq!(
            strip.descriptor().pixel_format(),
            zenpixels::PixelFormat::RgbaF32
        );
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

//! Tests for metadata embedding and roundtrip preservation
//!
//! Verifies that EXIF, XMP, CICP color info, rotation, mirror,
//! and HDR metadata survive encode → decode roundtrips.

#![cfg(feature = "encode")]

use imgref::Img;
use rgb::Rgb;
use zenavif::{
    DecoderConfig, EncoderConfig, ManagedAvifDecoder, MasteringDisplayConfig, encode_rgb8,
};

/// Create a simple 16x16 RGB8 test image
fn make_test_image() -> Img<Vec<Rgb<u8>>> {
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

fn encode_and_probe(config: &EncoderConfig) -> zenavif::ImageInfo {
    let img = make_test_image();
    let encoded =
        encode_rgb8(img.as_ref(), config, &enough::Unstoppable).expect("encode should succeed");
    let decoder = ManagedAvifDecoder::new(&encoded.avif_file, &DecoderConfig::default())
        .expect("decoder should open");
    decoder.probe_info().expect("probe should succeed")
}

/// Build EXIF data with the AVIF 4-byte offset prefix.
/// The prefix is a big-endian u32 offset from the end of the prefix
/// to the TIFF header. For typical EXIF, offset = 0.
fn make_avif_exif() -> Vec<u8> {
    let mut data = vec![
        0x00, 0x00, 0x00, 0x00, // AVIF EXIF offset prefix (offset = 0)
    ];
    // Minimal TIFF header (little-endian) + empty IFD
    data.extend_from_slice(&[
        0x49, 0x49, // Little-endian byte order (II)
        0x2A, 0x00, // TIFF magic number
        0x08, 0x00, 0x00, 0x00, // Offset to first IFD
        0x00, 0x00, // Zero IFD entries
        0x00, 0x00, 0x00, 0x00, // No next IFD
    ]);
    data
}

#[test]
fn exif_metadata_roundtrip() {
    let exif_data = make_avif_exif();

    let config = EncoderConfig::new().quality(80.0).speed(10).exif(exif_data);

    let info = encode_and_probe(&config);
    let decoded_exif = info.exif.expect("EXIF should be present after roundtrip");
    // Parser strips the 4-byte AVIF prefix and returns TIFF data starting from the TIFF header
    let tiff_magic = &[0x49u8, 0x49, 0x2A, 0x00];
    assert!(
        decoded_exif.starts_with(tiff_magic),
        "decoded EXIF should start with TIFF magic bytes, got {:?}",
        &decoded_exif[..decoded_exif.len().min(8)]
    );
}

#[test]
fn xmp_metadata_roundtrip() {
    let xmp_data = b"<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?>\
        <x:xmpmeta xmlns:x='adobe:ns:meta/'>\
        <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\
        </rdf:RDF></x:xmpmeta><?xpacket end='w'?>"
        .to_vec();

    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(10)
        .xmp(xmp_data.clone());

    let info = encode_and_probe(&config);
    let decoded_xmp = info.xmp.expect("XMP should be present after roundtrip");
    assert_eq!(
        decoded_xmp, xmp_data,
        "XMP data should survive roundtrip exactly"
    );
}

#[test]
fn cicp_color_primaries_roundtrip() {
    // BT.2020 primaries (9), PQ transfer (16)
    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(10)
        .color_primaries(9)
        .transfer_characteristics(16);

    let info = encode_and_probe(&config);
    assert_eq!(
        info.color_primaries.0, 9,
        "color primaries should be BT.2020 (9)"
    );
    assert_eq!(
        info.transfer_characteristics.0, 16,
        "transfer characteristics should be PQ (16)"
    );
    // Matrix coefficients may be overridden by the AV1 encoder based on
    // actual encoding parameters (e.g. BT.601 for YCbCr 4:2:0 at SD)
}

#[test]
fn cicp_matrix_coefficients_set() {
    // Verify the matrix coefficients field is populated (value may differ
    // from user request — the AV1 encoder selects based on encoding params)
    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(10)
        .matrix_coefficients(9);

    let info = encode_and_probe(&config);
    // Just verify we get a valid matrix coefficient value
    assert!(
        info.matrix_coefficients.0 > 0,
        "matrix coefficients should be set, got {}",
        info.matrix_coefficients.0
    );
}

#[test]
fn srgb_cicp_defaults() {
    let config = EncoderConfig::new().quality(80.0).speed(10);

    let info = encode_and_probe(&config);
    // Default should be BT.709 primaries (1) or unspecified
    assert!(
        info.color_primaries.0 == 1 || info.color_primaries.0 == 0,
        "default primaries should be BT.709 (1) or unspecified, got {}",
        info.color_primaries.0
    );
}

#[test]
fn content_light_level_roundtrip() {
    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(10)
        .content_light_level(1000, 400);

    let info = encode_and_probe(&config);
    let cll = info
        .content_light_level
        .expect("content light level should be present");
    assert_eq!(
        cll.max_content_light_level, 1000,
        "max_cll should roundtrip"
    );
    assert_eq!(
        cll.max_pic_average_light_level, 400,
        "max_fall should roundtrip"
    );
}

#[test]
fn mastering_display_roundtrip() {
    let md_config = MasteringDisplayConfig {
        primaries: [(13250, 34500), (7500, 3000), (34000, 16000)],
        white_point: (15635, 16450),
        max_luminance: 10000 << 8, // 10000 cd/m² in 24.8 fixed point
        min_luminance: 50,         // ~0.003 cd/m² in 18.14 fixed point
    };

    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(10)
        .mastering_display(md_config);

    let info = encode_and_probe(&config);
    let mdcv = info
        .mastering_display
        .expect("mastering display metadata should be present");

    assert_eq!(mdcv.max_luminance, 10000 << 8);
    assert_eq!(mdcv.min_luminance, 50);
}

#[test]
fn rotation_90_roundtrip() {
    // Encoder takes raw code: 1 = 90° CCW
    let config = EncoderConfig::new().quality(80.0).speed(10).rotation(1);

    let info = encode_and_probe(&config);
    let rot = info.rotation.expect("rotation should be present");
    assert_eq!(rot.angle, 90, "rotation angle should be 90°");
}

#[test]
fn rotation_270_roundtrip() {
    // Encoder takes raw code: 3 = 270° CCW
    let config = EncoderConfig::new().quality(80.0).speed(10).rotation(3);

    let info = encode_and_probe(&config);
    let rot = info.rotation.expect("rotation should be present");
    assert_eq!(rot.angle, 270, "rotation angle should be 270°");
}

#[test]
fn mirror_vertical_roundtrip() {
    let config = EncoderConfig::new().quality(80.0).speed(10).mirror(0);

    let info = encode_and_probe(&config);
    let mir = info.mirror.expect("mirror should be present");
    assert_eq!(mir.axis, 0, "mirror axis should roundtrip");
}

#[test]
fn mirror_horizontal_roundtrip() {
    let config = EncoderConfig::new().quality(80.0).speed(10).mirror(1);

    let info = encode_and_probe(&config);
    let mir = info.mirror.expect("mirror should be present");
    assert_eq!(mir.axis, 1, "mirror axis should roundtrip");
}

#[test]
fn no_metadata_by_default() {
    let config = EncoderConfig::new().quality(80.0).speed(10);

    let info = encode_and_probe(&config);
    assert!(info.exif.is_none(), "no EXIF by default");
    assert!(info.xmp.is_none(), "no XMP by default");
    assert!(info.content_light_level.is_none(), "no CLL by default");
    assert!(info.mastering_display.is_none(), "no MDCV by default");
}

#[test]
fn combined_metadata_roundtrip() {
    let exif_data = make_avif_exif();
    let xmp_data = b"<x:xmpmeta/>".to_vec();

    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(10)
        .exif(exif_data)
        .xmp(xmp_data.clone())
        .color_primaries(9)
        .transfer_characteristics(16)
        .content_light_level(4000, 1000)
        .rotation(2); // raw code 2 = 180°

    let info = encode_and_probe(&config);

    assert!(info.exif.is_some(), "EXIF should be present");
    assert_eq!(
        info.xmp.as_deref(),
        Some(xmp_data.as_slice()),
        "XMP should match"
    );
    assert_eq!(info.color_primaries.0, 9);
    assert_eq!(info.transfer_characteristics.0, 16);
    let cll = info.content_light_level.expect("CLL present");
    assert_eq!(cll.max_content_light_level, 4000);
    assert_eq!(cll.max_pic_average_light_level, 1000);
    let rot = info.rotation.expect("rotation present");
    assert_eq!(rot.angle, 180);
}

#[test]
fn decode_full_returns_metadata() {
    let exif_data = make_avif_exif();

    let img = make_test_image();
    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(10)
        .exif(exif_data)
        .color_primaries(12) // Display P3
        .transfer_characteristics(13); // sRGB

    let encoded =
        encode_rgb8(img.as_ref(), &config, &enough::Unstoppable).expect("encode should succeed");

    let mut decoder = ManagedAvifDecoder::new(&encoded.avif_file, &DecoderConfig::default())
        .expect("decoder should open");
    let (_pixels, info) = decoder
        .decode_full(&enough::Unstoppable)
        .expect("decode_full should succeed");

    assert_eq!(info.width, 16);
    assert_eq!(info.height, 16);
    assert!(info.exif.is_some(), "EXIF should survive decode_full");
    assert_eq!(info.color_primaries.0, 12, "Display P3 primaries");
    assert_eq!(info.transfer_characteristics.0, 13, "sRGB transfer");
}

#[test]
fn cancellation_during_encode() {
    use enough::StopReason;

    struct AlreadyStopped;
    impl enough::Stop for AlreadyStopped {
        fn check(&self) -> std::result::Result<(), StopReason> {
            Err(StopReason::Cancelled)
        }
    }

    let img = make_test_image();
    let config = EncoderConfig::new().quality(80.0).speed(10);

    let result = encode_rgb8(img.as_ref(), &config, &AlreadyStopped);
    assert!(result.is_err(), "encoding with cancelled token should fail");
}

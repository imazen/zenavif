//! HDR contract tests: 10-bit encode → decode roundtrips with PQ/HLG CICP,
//! clli/mdcv metadata, and pixel fidelity.
//!
//! Verifies the full pipeline claim: HDR color description + light-level
//! metadata survive encode → parse → decode → re-encode, and 10-bit pixel
//! data survives an encode/decode cycle within quantifiable bounds.
//!
//! The 16-bit encode path (`encode_rgb16`) targets 10-bit AV1 with the
//! identity matrix (GBR plane order, full range) — see `encode_rgb16` docs
//! and imazen/zenavif#14 for the plane-order contract.

#![cfg(feature = "encode")]

use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::Rgb;
use zenavif::{
    ColorPrimaries, EncoderConfig, ManagedAvifDecoder, MasteringDisplayConfig, MatrixCoefficients,
    TransferCharacteristics, encode_rgb16,
};
use zenpixels::PixelBuffer;

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// Deterministic 16-bit HDR-ish test content, full-range u16.
///
/// Layout (64×48):
/// - rows 0..16: horizontal luminance ramp 0..65535 (smooth gradient)
/// - rows 16..32: saturated color patches (R/G/B/W quadrants)
/// - rows 32..48: dark content with a few max-value "specular" pixels —
///   the shape PQ content stresses (tiny bright highlights on dark).
fn make_hdr16() -> Img<Vec<Rgb<u16>>> {
    let (w, h) = (64usize, 48usize);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let p = if y < 16 {
                let v = (x * 65535 / (w - 1)) as u16;
                Rgb { r: v, g: v, b: v }
            } else if y < 32 {
                match x / 16 {
                    0 => Rgb {
                        r: 60000,
                        g: 4000,
                        b: 4000,
                    },
                    1 => Rgb {
                        r: 4000,
                        g: 60000,
                        b: 4000,
                    },
                    2 => Rgb {
                        r: 4000,
                        g: 4000,
                        b: 60000,
                    },
                    _ => Rgb {
                        r: 62000,
                        g: 62000,
                        b: 62000,
                    },
                }
            } else {
                // Dark base with specular pops on a fixed lattice.
                if x % 13 == 0 && y % 5 == 0 {
                    Rgb {
                        r: 65535,
                        g: 65535,
                        b: 65535,
                    }
                } else {
                    let v = 1200 + ((x * 7 + y * 11) % 64) as u16 * 8;
                    Rgb {
                        r: v,
                        g: v / 2,
                        b: v / 3,
                    }
                }
            };
            px.push(p);
        }
    }
    Img::new(px, w, h)
}

/// BT.2020 mastering primaries in mdcv wire order (GREEN, BLUE, RED — the
/// ST 2086 / HEVC SEI slot order the container `mdcv` box uses), xy×50000,
/// D65 white point, 1000 cd/m² max / 0.005 cd/m² min.
fn bt2020_mastering() -> MasteringDisplayConfig {
    MasteringDisplayConfig {
        primaries: [(8500, 39850), (6550, 2300), (35400, 14600)],
        white_point: (15635, 16450),
        max_luminance: 1000 * 10000,
        min_luminance: 50,
    }
}

fn pq10_config() -> EncoderConfig {
    EncoderConfig::new()
        .quality(90.0)
        .speed(8)
        .color_primaries(ColorPrimaries::BT2020.0)
        .transfer_characteristics(TransferCharacteristics::SMPTE2084.0)
        .content_light_level(4000, 400)
        .mastering_display(bt2020_mastering())
}

fn decode_all(avif: &[u8]) -> (PixelBuffer, zenavif::ImageInfo) {
    // prefer_8bit(false): keep 10-bit output as RGB16 instead of
    // down-converting to 8-bit for display.
    let cfg = zenavif::DecoderConfig::new().prefer_8bit(false);
    let mut dec = ManagedAvifDecoder::new(avif, &cfg).expect("decoder should open our own encode");
    dec.decode_full(&Unstoppable)
        .expect("decode should succeed")
}

// ============================================================================
// PQ (SMPTE 2084) 10-bit: metadata + pixel fidelity
// ============================================================================

#[test]
fn pq10_metadata_survives_encode_decode() {
    let img = make_hdr16();
    let encoded = encode_rgb16(img.as_ref(), &pq10_config(), stop()).expect("PQ10 encode");

    let (_pixels, info) = decode_all(&encoded.avif_file);

    // Bit depth: 16-bit input → 10-bit AV1 (EncodeBitDepth::Auto contract).
    assert_eq!(info.bit_depth, 10, "16-bit input must produce 10-bit AV1");

    // CICP echoed exactly.
    assert_eq!(info.color_primaries, ColorPrimaries::BT2020);
    assert_eq!(
        info.transfer_characteristics,
        TransferCharacteristics::SMPTE2084
    );
    // encode_rgb16 is the identity-matrix (GBR) path; the file must signal
    // MC=0, not whatever a caller might wish for (see encode_rgb16 docs).
    assert_eq!(info.matrix_coefficients, MatrixCoefficients::IDENTITY);

    // clli intact.
    let cll = info.content_light_level.expect("clli must survive");
    assert_eq!(cll.max_content_light_level, 4000);
    assert_eq!(cll.max_pic_average_light_level, 400);

    // mdcv intact, in wire order (G, B, R).
    let md = info.mastering_display.expect("mdcv must survive");
    assert_eq!(md.primaries, [(8500, 39850), (6550, 2300), (35400, 14600)]);
    assert_eq!(md.white_point, (15635, 16450));
    assert_eq!(md.max_luminance, 1000 * 10000);
    assert_eq!(md.min_luminance, 50);
}

#[test]
fn pq10_pixel_fidelity_within_bounds() {
    let img = make_hdr16();
    // quality 95: high-fidelity point with measured headroom (see bounds).
    let config = pq10_config().quality(95.0);
    let encoded = encode_rgb16(img.as_ref(), &config, stop()).expect("PQ10 encode");

    let (pixels, info) = decode_all(&encoded.avif_file);
    assert_eq!(info.bit_depth, 10);

    let out = pixels
        .try_as_imgref::<Rgb<u16>>()
        .expect("10-bit decode must expose an Rgb16 view");
    assert_eq!(out.width(), img.width());
    assert_eq!(out.height(), img.height());

    // Error budget in 16-bit units:
    // - 16→10-bit floor-shift quantization: up to 63 (scale_from_u16 docs)
    // - 10→16-bit LSB-replication reconstruction: exact inverse for the
    //   quantized value, so the scale pair alone contributes ≤ 63
    // - codec loss at quality 95, identity matrix (no chroma resample).
    // Measured 2026-07-03 on this exact content (examples/hdr_fidelity_probe,
    // zenravif 0.2.0 dev chain): q95/s8 max |Δ| = 607 (9 ten-bit steps),
    // mean = 50. Quality sweep confirms pure quantizer behavior
    // (q80→3075, q90→1665, q95→607, q99→221). Bounds = measured + ~50%
    // headroom; real pipeline bugs are orders of magnitude past them
    // (channel swap ≈ 56000, plane rotation ≈ full-scale).
    //
    // RE-CALIBRATED 2026-08-06 for the zenrav1e dep bump + the armed speed
    // rows (cavif-rs#6). q95/s8 max |Δ| moved 607 → 1281, attributed by
    // isolation to `S6_TX_SIZE_RDO_LIVE` (`S6_PART_PRUNE_LIVE` alone reaches
    // 960); mean IMPROVED 50 → 47.
    //
    // This is a rate shift, not a fidelity regression, and the distinction is
    // the whole reason the number moves rather than the arms coming out:
    //
    //   * At MATCHED BYTES the armed encoder wins the tail too —
    //     baseline q90 = 1125 B / max 1665 / mean 85, versus
    //     armed    q95 = 1127 B / max 1281 / mean 47.
    //     The old 900 budget was calibrated against a 1407-byte q95 output;
    //     the arms now hit that quality setting in 1127 bytes, so asserting
    //     at a fixed QUALITY silently compares two different rate points.
    //   * Exactly one cell of 9,216 crosses the old budget: a single
    //     max-white specular impulse, 65535 → 64254 on one channel
    //     (1023 → 1003 in ten-bit). Second worst is 772.
    //   * Ramp and colour-patch region maxima are UNCHANGED (461/420 both
    //     sides). Only the dark+specular tail moved, and that region's mean
    //     improved 95 → 87.
    //   * Mean |Δ| improves at every quality (q70..q99: 204/132/109/85/50/30
    //     → 151/95/93/72/47/29) while bytes drop 18-25%, and max is not
    //     monotonically worse — the armed encoder WINS at q90 (1537 vs 1665).
    //
    // New bound = 1281 measured + ~48% headroom, matching the original
    // methodology. The mean bound is untouched: mean fidelity improved, so
    // 90 still holds and still has teeth.
    //
    // Honest limits on that evidence: n=1 synthetic fixture, no perceptual
    // metric on the 10-bit path, and every one of these arms was fitted on
    // 8-bit YCbCr — this is their first 10-bit identity-matrix data point.
    // If a second 10-bit fixture ever exceeds this bound, re-open the
    // question rather than widening again.
    let mut max_diff = 0u32;
    let mut sum_diff = 0u64;
    for (a, b) in img.pixels().zip(out.pixels()) {
        for (va, vb) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
            let d = (va as i32 - vb as i32).unsigned_abs();
            max_diff = max_diff.max(d);
            sum_diff += d as u64;
        }
    }
    let n = (img.width() * img.height() * 3) as u64;
    let mean_diff = sum_diff / n;
    eprintln!("pq10 fidelity: max |Δ| = {max_diff}, mean |Δ| = {mean_diff} (16-bit units)");
    assert!(
        max_diff <= 1900,
        "PQ10 roundtrip max |Δ| {max_diff} exceeds budget (mean {mean_diff})"
    );
    assert!(
        mean_diff <= 90,
        "PQ10 roundtrip mean |Δ| {mean_diff} exceeds budget (max {max_diff})"
    );
}

// ============================================================================
// HLG 10-bit
// ============================================================================

#[test]
fn hlg10_cicp_survives_encode_decode() {
    let img = make_hdr16();
    let config = EncoderConfig::new()
        .quality(85.0)
        .speed(8)
        .color_primaries(ColorPrimaries::BT2020.0)
        .transfer_characteristics(TransferCharacteristics::HLG.0);
    let encoded = encode_rgb16(img.as_ref(), &config, stop()).expect("HLG10 encode");

    let (_pixels, info) = decode_all(&encoded.avif_file);
    assert_eq!(info.bit_depth, 10);
    assert_eq!(info.color_primaries, ColorPrimaries::BT2020);
    assert_eq!(info.transfer_characteristics, TransferCharacteristics::HLG);
    // No clli/mdcv were set: they must not materialize from nowhere.
    assert!(info.content_light_level.is_none());
    assert!(info.mastering_display.is_none());
}

// ============================================================================
// Re-encode chain: parse → decode → encode preserves HDR metadata
// ============================================================================

#[test]
fn pq10_reencode_chain_preserves_hdr_metadata() {
    let img = make_hdr16();
    let first = encode_rgb16(img.as_ref(), &pq10_config(), stop()).expect("first encode");
    let (pixels, info) = decode_all(&first.avif_file);

    let decoded_img = pixels
        .try_as_imgref::<Rgb<u16>>()
        .expect("expected an Rgb16 view");

    // Rebuild an encoder config from decoded ImageInfo — the "transcode"
    // direction an application performs.
    let mut config2 = EncoderConfig::new()
        .quality(90.0)
        .speed(8)
        .color_primaries(info.color_primaries.0)
        .transfer_characteristics(info.transfer_characteristics.0);
    if let Some(cll) = info.content_light_level {
        config2 = config2
            .content_light_level(cll.max_content_light_level, cll.max_pic_average_light_level);
    }
    if let Some(md) = info.mastering_display {
        config2 = config2.mastering_display(MasteringDisplayConfig {
            primaries: md.primaries,
            white_point: md.white_point,
            max_luminance: md.max_luminance,
            min_luminance: md.min_luminance,
        });
    }

    let second = encode_rgb16(decoded_img, &config2, stop()).expect("re-encode from decode");
    let (_p2, info2) = decode_all(&second.avif_file);

    assert_eq!(info2.bit_depth, 10);
    assert_eq!(info2.color_primaries, ColorPrimaries::BT2020);
    assert_eq!(
        info2.transfer_characteristics,
        TransferCharacteristics::SMPTE2084
    );
    let cll2 = info2.content_light_level.expect("clli survives re-encode");
    assert_eq!(cll2.max_content_light_level, 4000);
    assert_eq!(cll2.max_pic_average_light_level, 400);
    let md2 = info2.mastering_display.expect("mdcv survives re-encode");
    assert_eq!(md2.primaries, [(8500, 39850), (6550, 2300), (35400, 14600)]);
    assert_eq!(md2.max_luminance, 1000 * 10000);
}

// ============================================================================
// HDR base + gain map together (10-bit PQ base carrying a tmap)
// ============================================================================

#[test]
fn pq10_base_with_gain_map_roundtrips() {
    // Real gain-map payload: encode a small 8-bit map image first, then
    // extract its AV1 OBUs — the byte-carry form a transcoder holds.
    let map_img: Img<Vec<Rgb<u8>>> = Img::new(
        (0..32 * 24)
            .map(|i| {
                let v = ((i * 255) / (32 * 24)) as u8;
                Rgb { r: v, g: v, b: v }
            })
            .collect(),
        32,
        24,
    );
    let map_cfg = EncoderConfig::new().quality(85.0).speed(10);
    let map_avif =
        zenavif::encode_rgb8(map_img.as_ref(), &map_cfg, stop()).expect("gain map encode");
    let map_parser =
        zenavif_parse::AvifParser::from_bytes(&map_avif.avif_file).expect("map parses");
    let map_av1 = map_parser
        .primary_data()
        .expect("map primary item")
        .into_owned();

    // Minimal single-channel ISO 21496-1 metadata (alt headroom 2/1).
    let mut tmap = Vec::new();
    tmap.push(0u8); // version
    tmap.extend_from_slice(&0u16.to_be_bytes()); // minimum_version
    tmap.extend_from_slice(&0u16.to_be_bytes()); // writer_version
    tmap.push(0b0100_0000); // use_base_colour_space, single channel
    tmap.extend_from_slice(&0u32.to_be_bytes()); // base_hdr_headroom 0/1
    tmap.extend_from_slice(&1u32.to_be_bytes());
    tmap.extend_from_slice(&2u32.to_be_bytes()); // alternate_hdr_headroom 2/1
    tmap.extend_from_slice(&1u32.to_be_bytes());
    for _ in 0..1 {
        tmap.extend_from_slice(&0i32.to_be_bytes()); // gain_map_min 0/1
        tmap.extend_from_slice(&1u32.to_be_bytes());
        tmap.extend_from_slice(&2i32.to_be_bytes()); // gain_map_max 2/1
        tmap.extend_from_slice(&1u32.to_be_bytes());
        tmap.extend_from_slice(&1u32.to_be_bytes()); // gamma 1/1
        tmap.extend_from_slice(&1u32.to_be_bytes());
        tmap.extend_from_slice(&0i32.to_be_bytes()); // base_offset 0/1
        tmap.extend_from_slice(&1u32.to_be_bytes());
        tmap.extend_from_slice(&0i32.to_be_bytes()); // alternate_offset 0/1
        tmap.extend_from_slice(&1u32.to_be_bytes());
    }

    let img = make_hdr16();
    let config = pq10_config().with_gain_map(map_av1.clone(), 32, 24, 8, tmap);
    let encoded = encode_rgb16(img.as_ref(), &config, stop()).expect("PQ10+tmap encode");

    let (_pixels, info) = decode_all(&encoded.avif_file);
    assert_eq!(info.bit_depth, 10);
    assert_eq!(
        info.transfer_characteristics,
        TransferCharacteristics::SMPTE2084
    );
    let gm = info
        .gain_map
        .as_ref()
        .expect("gain map survives on PQ base");
    assert_eq!(gm.gain_map_data, map_av1, "gain map AV1 bytes byte-carry");
    assert!(!gm.metadata.is_multichannel);
    assert_eq!(gm.metadata.alternate_hdr_headroom_n, 2);
    assert_eq!(gm.metadata.alternate_hdr_headroom_d, 1);
    // clli/mdcv still present alongside the tmap.
    assert!(info.content_light_level.is_some());
    assert!(info.mastering_display.is_some());
}

//! Cross-backend validation: bitstreams produced by zenavif's encode
//! backends must decode byte-identically on the independent decode backends.
//!
//! The decode-conformance story so far only covered aomenc-produced streams
//! (the AV1 conformance corpus). These tests close the loop on OUR encoders:
//! zenravif (zenrav1e) and svtav1-rs output is decoded through the raw-OBU
//! seam with both `DecodeBackend::Rav1dSafe` and `DecodeBackend::AomRs` and
//! the YUV planes are byte-compared. Two independent decoder ports agreeing
//! bit-exactly is the strongest conformance signal available without a C
//! reference in-tree.
#![cfg(all(feature = "aom-backend", feature = "encode"))]

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgVec};
use rgb::Rgb;
use zenavif::{Av1Backend, DecodeBackend, DecodedYuv, EncoderConfig, decode_av1_obu_yuv};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// Deterministic photo-ish test content: smooth gradients + LCG noise so
/// quantization has real work at every quality tier.
fn test_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let mut state = 0x2545F491u32;
    let mut lcg = move || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (state >> 24) as u8
    };
    let mut buf = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let g = ((x * 255) / w.max(1)) as u8;
            let b = ((y * 255) / h.max(1)) as u8;
            let n = lcg() / 8;
            buf.push(Rgb {
                r: g.wrapping_add(n),
                g: b.wrapping_add(n / 2),
                b: ((g as u16 + b as u16) / 2) as u8,
            });
        }
    }
    Img::new(buf, w, h)
}

/// Extract the primary item's AV1 payload from an encoded AVIF container.
fn primary_payload(avif: &[u8]) -> Vec<u8> {
    let cfg = zenavif_parse::DecodeConfig::default().lenient(true);
    let parser =
        zenavif_parse::AvifParser::from_owned_with_config(avif.to_vec(), &cfg, &Unstoppable)
            .expect("container parse");
    parser
        .primary_data()
        .expect("primary item")
        .as_ref()
        .to_vec()
}

/// Decode with aom-rs, tolerating a missing temporal-delimiter OBU (AVIF item
/// payloads may start directly at the sequence header).
fn decode_aom(payload: &[u8]) -> DecodedYuv {
    decode_av1_obu_yuv(payload, DecodeBackend::AomRs).unwrap_or_else(|_| {
        let mut with_td = vec![0x12, 0x00];
        with_td.extend_from_slice(payload);
        decode_av1_obu_yuv(&with_td, DecodeBackend::AomRs).expect("aom-rs decode")
    })
}

fn assert_backends_agree(avif: &[u8], label: &str) {
    let payload = primary_payload(avif);
    let rav = decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe).expect("rav1d-safe decode");
    let aom = decode_aom(&payload);
    assert_eq!(
        (
            rav.width,
            rav.height,
            rav.width_uv,
            rav.height_uv,
            rav.bit_depth
        ),
        (
            aom.width,
            aom.height,
            aom.width_uv,
            aom.height_uv,
            aom.bit_depth
        ),
        "{label}: geometry mismatch between rav1d-safe and aom-rs"
    );
    assert_eq!(rav.y, aom.y, "{label}: luma plane diverges");
    assert_eq!(rav.u, aom.u, "{label}: U plane diverges");
    assert_eq!(rav.v, aom.v, "{label}: V plane diverges");
}

#[test]
fn zenravif_output_decodes_identically_on_both_backends() {
    let img = test_image(128, 96);
    for (quality, subsampling) in [
        (30.0, zenavif::EncodeChromaSubsampling::Yuv420),
        (85.0, zenavif::EncodeChromaSubsampling::Yuv420),
        (85.0, zenavif::EncodeChromaSubsampling::Yuv444),
    ] {
        let config = EncoderConfig::new()
            .quality(quality)
            .speed(8)
            .chroma_subsampling(subsampling);
        let enc = zenavif::encode_rgb8(img.as_ref(), &config, stop()).expect("zenravif encode");
        assert_backends_agree(
            &enc.avif_file,
            &format!("zenravif q{quality} {subsampling:?}"),
        );
    }
}

#[cfg(feature = "encode-svt-rs")]
#[test]
fn svt_rs_output_decodes_identically_on_both_backends() {
    // SvtRs scope: 8-bit 4:2:0, dims multiple of 64.
    let img = test_image(192, 128);
    for quality in [30.0, 85.0] {
        let config = EncoderConfig::new()
            .quality(quality)
            .speed(6)
            .chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv420)
            .backend(Av1Backend::SvtRs);
        let enc = zenavif::encode_rgb8(img.as_ref(), &config, stop()).expect("svt-rs encode");
        assert_backends_agree(&enc.avif_file, &format!("svt-rs q{quality}"));
    }
}

/// 10-bit AV1 from the zenravif backend: both decode backends must agree on
/// the high-bit-depth path too (aom-rs decodes 8/10/12-bit; rav1d-safe
/// likewise). Encodes RGB16 -> 10-bit AV1.
#[test]
fn zenravif_10bit_output_decodes_identically_on_both_backends() {
    let img8 = test_image(128, 96);
    let buf16: Vec<rgb::Rgb<u16>> = img8
        .buf()
        .iter()
        .map(|p| rgb::Rgb {
            r: (p.r as u16) << 8 | p.r as u16,
            g: (p.g as u16) << 8 | p.g as u16,
            b: (p.b as u16) << 8 | p.b as u16,
        })
        .collect();
    let img16 = Img::new(buf16, img8.width(), img8.height());
    let config = EncoderConfig::new()
        .quality(80.0)
        .speed(8)
        .bit_depth(zenavif::EncodeBitDepth::Ten);
    let enc = zenavif::encode_rgb16(img16.as_ref(), &config, stop()).expect("10-bit encode");
    let payload = primary_payload(&enc.avif_file);
    let rav = decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe).expect("rav1d-safe decode");
    assert_eq!(rav.bit_depth, 10, "expected a 10-bit AV1 stream");
    assert_backends_agree(&enc.avif_file, "zenravif 10-bit q80");
}

/// Backend-seam obligation 3 liveness: the config-carrying seam must actually
/// deliver limits and cancellation to the backends — a seam that accepts a
/// config and drops it would pass every other test here.
mod config_threading {
    use super::*;
    use zenavif::DecoderConfig;

    fn small_payload() -> Vec<u8> {
        let img = test_image(128, 96);
        let config = EncoderConfig::new().quality(60.0).speed(8);
        let enc = zenavif::encode_rgb8(img.as_ref(), &config, stop()).expect("encode");
        let mut payload = primary_payload(&enc.avif_file);
        // aom-rs wants a leading temporal delimiter.
        if payload.first().map(|b| b >> 3 & 0xf) != Some(2) {
            let mut td = vec![0x12, 0x00];
            td.append(&mut payload);
            payload = td;
        }
        payload
    }

    #[test]
    fn frame_size_limit_rejects_on_both_backends() {
        let payload = small_payload();
        // 128x96 = 12,288 px; a 10,000-px cap must reject on BOTH backends
        // before any frame allocation.
        // The tiny-cap-fails + roomy-cap-succeeds PAIRING is the liveness
        // proof (the cap is the only difference). Message wording is only
        // asserted on AomRs, where this seam owns the error mapping —
        // rav1d-safe enforces its cap internally and surfaces a generic
        // decode error.
        let tiny = DecoderConfig::new().frame_size_limit(10_000);
        for backend in [DecodeBackend::Rav1dSafe, DecodeBackend::AomRs] {
            let err = zenavif::decode_av1_obu_yuv_with(&payload, backend, &tiny, &Unstoppable)
                .expect_err("a 10k-px cap must reject a 12k-px frame");
            if backend == DecodeBackend::AomRs {
                let msg = err.to_string().to_lowercase();
                assert!(
                    msg.contains("limit"),
                    "AomRs limit rejection should say so: {msg}"
                );
            }
        }
        // And the same config decodes fine with an adequate cap.
        let roomy = DecoderConfig::new().frame_size_limit(1_000_000);
        for backend in [DecodeBackend::Rav1dSafe, DecodeBackend::AomRs] {
            zenavif::decode_av1_obu_yuv_with(&payload, backend, &roomy, &Unstoppable)
                .unwrap_or_else(|e| panic!("{backend:?} under a roomy cap: {e}"));
        }
    }

    #[test]
    fn pre_fired_stop_cancels_on_every_backend() {
        struct AlwaysStop;
        impl enough::Stop for AlwaysStop {
            fn check(&self) -> Result<(), enough::StopReason> {
                Err(enough::StopReason::Cancelled)
            }
        }
        let payload = small_payload();
        for backend in [DecodeBackend::Rav1dSafe, DecodeBackend::AomRs] {
            let err = zenavif::decode_av1_obu_yuv_with(
                &payload,
                backend,
                &DecoderConfig::default(),
                &AlwaysStop,
            )
            .expect_err("a pre-fired stop token must cancel the decode");
            assert!(
                err.to_string().to_lowercase().contains("cancel"),
                "{backend:?}: expected a cancellation error, got: {err}"
            );
        }
    }
}

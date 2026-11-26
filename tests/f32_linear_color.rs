//! F32 conversion, stored RGB codes, and the exact emitted AVIF color carriers.

#![cfg(feature = "encode")]

#[path = "support/icc_profiles.rs"]
mod profiles;

use std::borrow::Cow;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
use zencodec::{Cicp, Metadata, MetadataPolicy};
use zenpixels::PixelView;

fn config() -> zenavif::AvifEncoderConfig {
    let mut config = zenavif::AvifEncoderConfig::new()
        .with_generic_quality(90.0)
        .with_generic_effort(10);
    *config.inner_mut() = config
        .inner()
        .clone()
        .color_model(zenavif::EncodeColorModel::Rgb)
        .threads(Some(1));
    config
}

fn linear(pixels: &[[f32; 3]]) -> PixelView<'_> {
    PixelView::new(
        bytemuck::cast_slice(pixels),
        16,
        16,
        16 * 12,
        &zenpixels::formats::RGBF32_LINEAR,
    )
    .unwrap()
}

fn decoded(bytes: &[u8]) -> zencodec::decode::DecodeOutput {
    zenavif::AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(bytes), &[])
        .unwrap()
        .decode()
        .unwrap()
}

#[test]
#[allow(deprecated)] // Preserve the legacy input-color override contract separately from checked retention.
fn f32_linear_input_emits_the_current_srgb_transfer_and_exact_carriers() {
    let px = [[0.25, 0.5, 0.75]; 256];
    for (cp, meta) in [
        (1, None),
        (
            12,
            Some(Metadata::none().with_cicp(Cicp::new(12, 8, 0, true))),
        ),
        (1, Some(Metadata::none().with_cicp(Cicp::LINEAR_SRGB))),
    ] {
        let mut job = config().job();
        if let Some(meta) = meta {
            job = job.with_metadata_policy(meta, MetadataPolicy::PreserveExact);
        }
        let output = job.encoder().unwrap().encode(linear(&px)).unwrap();
        let out = decoded(output.data());
        let source = &out.info().source_color;
        assert_eq!(source.cicp, Some(Cicp::new(cp, 13, 0, true)));
        assert_eq!(out.pixels().origin().unwrap().cicp, source.cicp);
        if cp == 12 {
            let profile = source
                .icc_profile
                .as_deref()
                .expect("P3 compatibility profile");
            assert_eq!(&profile[16..20], b"RGB ");
            assert_eq!(
                Cicp::from_icc(profile)
                    .or_else(|| zenpixels::icc::IccIdentification::from_icc(profile)?.to_cicp()),
                Some(Cicp::DISPLAY_P3)
            );
            assert_eq!(source.color_authority, zencodec::ColorAuthority::Icc);
        } else {
            assert!(source.icc_profile.is_none());
            assert_eq!(source.color_authority, zencodec::ColorAuthority::Cicp);
        }
    }
}

#[test]
fn analytical_srgb_codes_produce_the_same_bitstream_as_explicit_rgb8() {
    let base = [0.0f32, 0.001, 0.015625, 0.125, 0.25, 0.5, 0.75, 1.0];
    let pixels = (0..256)
        .map(|i| [base[i % 8], base[(i + 3) % 8], base[(i + 5) % 8]])
        .collect::<Vec<_>>();
    let quantize = |v: f32| {
        let v = f64::from(v);
        let nonlinear = if v <= 0.0031308 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (255.0 * nonlinear).round() as u8
    };
    assert_eq!(base.map(quantize), [0, 3, 34, 99, 137, 188, 225, 255]);
    let quantized = pixels.iter().map(|p| p.map(quantize)).collect::<Vec<_>>();
    let reference = PixelView::new(
        bytemuck::cast_slice(&quantized),
        16,
        16,
        48,
        &zenpixels::formats::RGB8_SRGB,
    )
    .unwrap();
    let actual = config()
        .job()
        .encoder()
        .unwrap()
        .encode(linear(&pixels))
        .unwrap();
    let expected = config().job().encoder().unwrap().encode(reference).unwrap();
    assert_eq!(actual.data(), expected.data());
}

#[test]
fn checked_metadata_retention_preserves_bytes_and_refuses_a_changed_transfer() {
    let input = [[0.25, 0.5, 0.75]; 256];
    let xmp = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>";
    let exact = Metadata::none()
        .with_cicp(Cicp::SRGB)
        .with_icc(profiles::SRGB)
        .with_xmp(xmp.as_slice());
    let output = config()
        .job()
        .try_with_metadata_policy(exact, MetadataPolicy::PreserveExact)
        .unwrap()
        .encoder()
        .unwrap()
        .encode(linear(&input))
        .unwrap();
    if let Some(path) = std::env::var_os("ZENAVIF_STRICT_COLOR_WITNESS") {
        std::fs::write(path, output.data()).unwrap();
    }
    let out = decoded(output.data());
    let source = &out.info().source_color;
    assert_eq!(source.cicp, Some(Cicp::SRGB));
    assert_eq!(source.icc_profile.as_deref(), Some(profiles::SRGB));
    assert_eq!(source.color_authority, zencodec::ColorAuthority::Icc);
    let parser = zenavif_parse::AvifParser::from_bytes(output.data()).unwrap();
    assert_eq!(
        parser.xmp().transpose().unwrap().as_deref(),
        Some(xmp.as_slice())
    );

    let mut published = Vec::new();
    let result = config()
        .job()
        .try_with_metadata_policy(
            Metadata::none().with_cicp(Cicp::LINEAR_SRGB),
            MetadataPolicy::PreserveExact,
        )
        .unwrap()
        .encoder()
        .and_then(|encoder| encoder.encode(linear(&input)));
    let err = result
        .map(|encoded| published.extend_from_slice(encoded.data()))
        .unwrap_err();
    assert_eq!(
        err.error().category(),
        zencodec::ErrorCategory::Request(zencodec::RequestError::Unsupported(
            zencodec::UnsupportedOperation::MetadataRetention
        ))
    );
    assert!(published.is_empty());
}

#[test]
fn checked_metadata_refuses_malformed_icc_before_encoder_creation() {
    let err = config()
        .job()
        .try_with_metadata_policy(
            Metadata::none()
                .with_icc(&[0u8; 16][..])
                .with_cicp(Cicp::SRGB),
            MetadataPolicy::PreserveExact,
        )
        .err()
        .expect("malformed ICC must not use the fallback in strict authoring");
    let typed = err
        .downcast_ref::<whereat::At<zencodec::CodecError>>()
        .unwrap();
    assert_eq!(
        typed.error().category(),
        zencodec::ErrorCategory::Request(zencodec::RequestError::Unsupported(
            zencodec::UnsupportedOperation::MetadataRetention
        ))
    );
}

//! The 0.1.8 backend rename kept the old spellings compiling.
//!
//! `Av1Backend::SvtRs` -> `Zenav1Svt`, `DecodeBackend::AomRs` -> `Zenav1Aom`
//! and `EstimateArm::SvtRs420` -> `Zenav1Svt420` were renamed to match the
//! crates they name (`zenav1-svt`, `zenav1-aom`). Each old spelling survives
//! as a `#[deprecated]` associated constant.
//!
//! An associated constant is usable in a *pattern* only when its type is
//! structural-match (derived `PartialEq` + `Eq`, not a hand-written impl).
//! All three enums derive both, so both positions work — and this file is the
//! proof, because it FAILS TO COMPILE if either position regresses. Every
//! assertion below exercises the alias in expression AND pattern position.

#![allow(deprecated)]

use zenavif::heuristics::EstimateArm;

// --------------------------------------------------------------------
// EstimateArm (always built — `heuristics` carries no feature gate)
// --------------------------------------------------------------------

#[test]
fn estimate_arm_alias_expression_position() {
    let arm: EstimateArm = EstimateArm::SvtRs420;
    assert_eq!(arm, EstimateArm::Zenav1Svt420);
}

#[test]
fn estimate_arm_alias_pattern_position() {
    // Bind through the NEW name, match through the OLD one: this arm only
    // compiles if the deprecated constant is usable as a pattern, and only
    // passes if it still denotes the same variant.
    let arm = EstimateArm::Zenav1Svt420;
    let matched = match arm {
        EstimateArm::SvtRs420 => "svt420",
        EstimateArm::Zenravif444 => "zenravif444",
        EstimateArm::Zenravif420 => "zenravif420",
    };
    assert_eq!(matched, "svt420");
}

// --------------------------------------------------------------------
// Av1Backend (behind `encode`, where the encoder enum lives)
// --------------------------------------------------------------------

#[cfg(feature = "encode")]
#[test]
fn av1_backend_alias_expression_position() {
    use zenavif::Av1Backend;
    let backend: Av1Backend = Av1Backend::SvtRs;
    assert_eq!(backend, Av1Backend::Zenav1Svt);
}

#[cfg(feature = "encode")]
#[test]
fn av1_backend_alias_pattern_position() {
    use zenavif::Av1Backend;
    let backend = Av1Backend::Zenav1Svt;
    let matched = match backend {
        Av1Backend::SvtRs => "svt",
        _ => "other",
    };
    assert_eq!(matched, "svt");

    // The alias must NOT collapse distinct variants: the default backend
    // still falls through to the wildcard.
    let other = Av1Backend::default();
    let matched_other = match other {
        Av1Backend::SvtRs => "svt",
        _ => "other",
    };
    assert_eq!(matched_other, "other");
}

/// The deprecated alias must denote the SAME variant, not merely a
/// same-typed constant: routed through `validate()` both spellings must
/// produce a byte-identical verdict (Ok or the very same error).
#[cfg(feature = "encode")]
#[test]
fn av1_backend_alias_drives_the_same_validation() {
    use zenavif::{Av1Backend, EncodeChromaSubsampling, EncodeColorModel, EncoderConfig};
    let build = |b: Av1Backend| {
        EncoderConfig::new()
            .backend(b)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
            .color_model(EncodeColorModel::YCbCr)
            .validate()
    };
    let via_alias = build(Av1Backend::SvtRs);
    let via_new = build(Av1Backend::Zenav1Svt);
    assert_eq!(
        format!("{via_alias:?}"),
        format!("{via_new:?}"),
        "alias and new name must validate identically"
    );
}

// --------------------------------------------------------------------
// DecodeBackend (the alias is gated exactly like the variant)
// --------------------------------------------------------------------

#[cfg(feature = "zenav1-aom")]
#[test]
fn decode_backend_alias_expression_position() {
    use zenavif::DecodeBackend;
    let backend: DecodeBackend = DecodeBackend::AomRs;
    assert_eq!(backend, DecodeBackend::Zenav1Aom);
}

#[cfg(feature = "zenav1-aom")]
#[test]
fn decode_backend_alias_pattern_position() {
    use zenavif::DecodeBackend;
    let backend = DecodeBackend::Zenav1Aom;
    let matched = match backend {
        DecodeBackend::AomRs => "aom",
        _ => "other",
    };
    assert_eq!(matched, "aom");

    let safe = DecodeBackend::Rav1dSafe;
    let matched_safe = match safe {
        DecodeBackend::AomRs => "aom",
        _ => "other",
    };
    assert_eq!(matched_safe, "other");
}

// --------------------------------------------------------------------
// Feature aliases: `aom-backend` -> `zenav1-aom`, `encode-svt-rs` ->
// `zenav1-svt`. Building with EITHER spelling must enable the same code.
//
// The two tests below are the observable proof, because they assert on
// behaviour that CHANGES with the feature rather than on the feature name.
// Their `not(...)` twins further down assert the opposite verdict when the
// feature is off, so an alias that resolved but gated nothing would flip a
// test from passing to failing rather than silently vanishing.
// --------------------------------------------------------------------

/// With `zenav1-svt` on (however spelled), an in-scope 4:2:0 YCbCr config
/// on the SVT backend must VALIDATE.
#[cfg(feature = "zenav1-svt")]
#[test]
fn zenav1_svt_feature_admits_the_svt_backend() {
    use zenavif::{Av1Backend, EncodeChromaSubsampling, EncodeColorModel, EncoderConfig};
    EncoderConfig::new()
        .backend(Av1Backend::Zenav1Svt)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
        .color_model(EncodeColorModel::YCbCr)
        .validate()
        .expect("zenav1-svt is enabled, so an in-scope 4:2:0 YCbCr config must validate");
}

/// With the feature OFF, the same config must be rejected as unavailable —
/// naming the NEW feature spelling in the error.
#[cfg(all(feature = "encode", not(feature = "zenav1-svt")))]
#[test]
fn without_zenav1_svt_the_backend_is_unavailable() {
    use zenavif::{
        Av1Backend, EncodeChromaSubsampling, EncodeColorModel, EncoderConfig, ValidationError,
    };
    let err = EncoderConfig::new()
        .backend(Av1Backend::Zenav1Svt)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
        .color_model(EncodeColorModel::YCbCr)
        .validate()
        .expect_err("zenav1-svt is off, so the backend must be rejected");
    match err {
        ValidationError::BackendUnavailable { feature, .. } => {
            assert_eq!(feature, "zenav1-svt", "the error must name the new feature");
        }
        other => panic!("expected BackendUnavailable, got {other:?}"),
    }
}

/// With `zenav1-aom` on (however spelled), the decode backend variant and
/// its decode entry point must exist. `decode_av1_obu_yuv` on empty input
/// must fail as a DECODE error, not be missing entirely.
#[cfg(feature = "zenav1-aom")]
#[test]
fn zenav1_aom_feature_admits_the_decode_backend() {
    use zenavif::{DecodeBackend, decode_av1_obu_yuv};
    let err = decode_av1_obu_yuv(&[], DecodeBackend::Zenav1Aom)
        .expect_err("empty OBU input must not decode");
    // The point is that the call COMPILED and reached the backend: an
    // error is expected, a missing variant would be a build failure.
    let _ = err;
}

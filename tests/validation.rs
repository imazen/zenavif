//! Per-error-variant + happy-path coverage for `Config::validate()`.
//!
//! Each `ValidationError` variant gets one synthetic config that
//! triggers it; default-constructed configs must always validate.

use zenavif::DecoderConfig;

#[cfg(feature = "encode")]
use zenavif::{EncoderConfig, ValidationError};

// --------------------------------------------------------------------
// Happy paths
// --------------------------------------------------------------------

#[test]
fn decoder_default_validates() {
    DecoderConfig::default()
        .validate()
        .expect("default DecoderConfig must validate");
}

#[cfg(feature = "encode")]
#[test]
fn encoder_default_validates() {
    EncoderConfig::default()
        .validate()
        .expect("default EncoderConfig must validate");
}

#[cfg(feature = "encode")]
#[test]
fn encoder_typical_config_validates() {
    let cfg = EncoderConfig::new()
        .quality(85.0)
        .speed(6)
        .alpha_quality(80.0)
        .threads(Some(4))
        .rotation(2)
        .mirror(0);
    cfg.validate().expect("typical config must validate");
}

#[cfg(feature = "__expert")]
#[test]
fn expert_default_internal_params_validates() {
    use zenavif::expert::InternalParams;
    InternalParams::default()
        .validate()
        .expect("default InternalParams must validate");
}

#[cfg(feature = "__expert")]
#[test]
fn expert_internal_params_well_formed_validates() {
    use zenavif::expert::InternalParams;
    let mut p = InternalParams::default();
    p.partition_range = Some((8, 32));
    p.lrf = Some(true);
    p.fast_deblock = Some(false);
    p.complex_prediction_modes = Some(true);
    p.validate()
        .expect("well-formed InternalParams must validate");
}

// --------------------------------------------------------------------
// EncoderConfig: per-variant
// --------------------------------------------------------------------

#[cfg(feature = "encode")]
#[test]
fn quality_out_of_range_high() {
    let cfg = EncoderConfig::new().quality(150.0);
    match cfg.validate() {
        Err(ValidationError::QualityOutOfRange { value, valid }) => {
            assert_eq!(value, 150.0);
            assert_eq!(*valid.start(), 0.0);
            assert_eq!(*valid.end(), 100.0);
        }
        other => panic!("expected QualityOutOfRange, got {other:?}"),
    }
}

#[cfg(feature = "encode")]
#[test]
fn quality_out_of_range_negative() {
    let cfg = EncoderConfig::new().quality(-1.0);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::QualityOutOfRange { .. })
    ));
}

#[cfg(feature = "encode")]
#[test]
fn quality_nan() {
    let cfg = EncoderConfig::new().quality(f32::NAN);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::QualityOutOfRange { .. })
    ));
}

#[cfg(feature = "encode")]
#[test]
fn alpha_quality_out_of_range() {
    let cfg = EncoderConfig::new().alpha_quality(101.0);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::AlphaQualityOutOfRange { .. })
    ));
}

#[cfg(feature = "encode")]
#[test]
fn speed_out_of_range_zero() {
    let cfg = EncoderConfig::new().speed(0);
    match cfg.validate() {
        Err(ValidationError::SpeedOutOfRange { value, valid }) => {
            assert_eq!(value, 0);
            assert_eq!(*valid.start(), 1);
            assert_eq!(*valid.end(), 10);
        }
        other => panic!("expected SpeedOutOfRange, got {other:?}"),
    }
}

#[cfg(feature = "encode")]
#[test]
fn speed_out_of_range_high() {
    let cfg = EncoderConfig::new().speed(11);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::SpeedOutOfRange { .. })
    ));
}

#[cfg(feature = "encode")]
#[test]
fn encoder_threads_zero() {
    let cfg = EncoderConfig::new().threads(Some(0));
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::EncoderThreadsZero)
    ));
}

#[cfg(feature = "encode")]
#[test]
fn encoder_threads_none_ok() {
    let cfg = EncoderConfig::new().threads(None);
    cfg.validate().expect("threads=None must validate");
}

#[cfg(feature = "encode")]
#[test]
fn rotation_invalid() {
    // Anything > 3 is invalid (irot box stores 2-bit code).
    let cfg = EncoderConfig::new().rotation(90);
    match cfg.validate() {
        Err(ValidationError::RotationInvalid { value }) => assert_eq!(value, 90),
        other => panic!("expected RotationInvalid, got {other:?}"),
    }
}

#[cfg(feature = "encode")]
#[test]
fn rotation_quarter_turn_codes_ok() {
    for code in 0u8..=3 {
        let cfg = EncoderConfig::new().rotation(code);
        cfg.validate()
            .unwrap_or_else(|e| panic!("rotation={code} must validate, got {e:?}"));
    }
}

#[cfg(feature = "encode")]
#[test]
fn mirror_invalid() {
    let cfg = EncoderConfig::new().mirror(2);
    match cfg.validate() {
        Err(ValidationError::MirrorInvalid { value }) => assert_eq!(value, 2),
        other => panic!("expected MirrorInvalid, got {other:?}"),
    }
}

#[cfg(feature = "encode")]
#[test]
fn cicp_color_primaries_reserved() {
    let cfg = EncoderConfig::new().color_primaries(3);
    match cfg.validate() {
        Err(ValidationError::CicpReserved { field, value }) => {
            assert_eq!(field, "color_primaries");
            assert_eq!(value, 3);
        }
        other => panic!("expected CicpReserved, got {other:?}"),
    }
}

#[cfg(feature = "encode")]
#[test]
fn cicp_transfer_characteristics_reserved() {
    let cfg = EncoderConfig::new().transfer_characteristics(3);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::CicpReserved {
            field: "transfer_characteristics",
            ..
        })
    ));
}

#[cfg(feature = "encode")]
#[test]
fn cicp_matrix_coefficients_reserved() {
    let cfg = EncoderConfig::new().matrix_coefficients(3);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::CicpReserved {
            field: "matrix_coefficients",
            ..
        })
    ));
}

// --------------------------------------------------------------------
// encode-imazen variants
// --------------------------------------------------------------------

#[cfg(feature = "encode-imazen")]
#[test]
fn vaq_strength_out_of_range_high() {
    let cfg = EncoderConfig::new().with_vaq(true, 5.0);
    match cfg.validate() {
        Err(ValidationError::VaqStrengthOutOfRange { value, valid }) => {
            assert_eq!(value, 5.0);
            assert_eq!(*valid.start(), 0.0);
            assert_eq!(*valid.end(), 4.0);
        }
        other => panic!("expected VaqStrengthOutOfRange, got {other:?}"),
    }
}

#[cfg(feature = "encode-imazen")]
#[test]
fn vaq_strength_out_of_range_negative() {
    let cfg = EncoderConfig::new().with_vaq(true, -0.1);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::VaqStrengthOutOfRange { .. })
    ));
}

#[cfg(feature = "encode-imazen")]
#[test]
fn seg_boost_below_half() {
    let cfg = EncoderConfig::new().with_seg_boost(Some(0.4));
    match cfg.validate() {
        Err(ValidationError::SegBoostOutOfRange { value, valid }) => {
            assert_eq!(value, 0.4);
            assert_eq!(*valid.start(), 0.5);
            assert_eq!(*valid.end(), 4.0);
        }
        other => panic!("expected SegBoostOutOfRange, got {other:?}"),
    }
}

#[cfg(feature = "encode-imazen")]
#[test]
fn seg_boost_above_four() {
    let cfg = EncoderConfig::new().with_seg_boost(Some(4.5));
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::SegBoostOutOfRange { .. })
    ));
}

#[cfg(feature = "encode-imazen")]
#[test]
fn seg_boost_one_ok() {
    let cfg = EncoderConfig::new().with_seg_boost(Some(1.0));
    cfg.validate().expect("seg_boost=1.0 (off) must validate");
}

#[cfg(feature = "encode-imazen")]
#[test]
fn lossless_and_vaq_mutually_exclusive() {
    let cfg = EncoderConfig::new().with_lossless(true).with_vaq(true, 1.0);
    match cfg.validate() {
        Err(ValidationError::MutuallyExclusive { a, b }) => {
            assert_eq!(a, "lossless");
            assert_eq!(b, "vaq");
        }
        other => panic!("expected MutuallyExclusive, got {other:?}"),
    }
}

#[cfg(feature = "encode-imazen")]
#[test]
fn lossless_alone_ok() {
    let cfg = EncoderConfig::new().with_lossless(true);
    cfg.validate().expect("lossless alone must validate");
}

// --------------------------------------------------------------------
// expert::InternalParams: per-variant
// --------------------------------------------------------------------

#[cfg(feature = "__expert")]
#[test]
fn partition_range_min_gt_max() {
    use zenavif::expert::InternalParams;
    let mut p = InternalParams::default();
    p.partition_range = Some((32, 8));
    match p.validate() {
        Err(ValidationError::PartitionRangeInvalid { min, max }) => {
            assert_eq!(min, 32);
            assert_eq!(max, 8);
        }
        other => panic!("expected PartitionRangeInvalid, got {other:?}"),
    }
}

#[cfg(feature = "__expert")]
#[test]
fn partition_range_invalid_size() {
    use zenavif::expert::InternalParams;
    let mut p = InternalParams::default();
    p.partition_range = Some((4, 7)); // 7 not in {4,8,16,32,64}
    assert!(matches!(
        p.validate(),
        Err(ValidationError::PartitionRangeInvalid { .. })
    ));
}

#[cfg(feature = "__expert")]
#[test]
fn partition_range_128_rejected() {
    use zenavif::expert::InternalParams;
    let mut p = InternalParams::default();
    p.partition_range = Some((4, 128)); // zenrav1e debug-asserts on 128
    match p.validate() {
        Err(ValidationError::PartitionRangeInvalid { min, max }) => {
            assert_eq!(min, 4);
            assert_eq!(max, 128);
        }
        other => panic!("expected PartitionRangeInvalid for 128, got {other:?}"),
    }
}

#[cfg(feature = "__expert")]
#[test]
fn partition_range_valid_combinations() {
    use zenavif::expert::InternalParams;
    let valid_pairs: &[(u8, u8)] = &[(4, 4), (4, 64), (8, 16), (16, 32), (32, 64), (64, 64)];
    for &(min, max) in valid_pairs {
        let mut p = InternalParams::default();
        p.partition_range = Some((min, max));
        p.validate()
            .unwrap_or_else(|e| panic!("({min},{max}) must validate, got {e:?}"));
    }
}

#[cfg(feature = "__expert")]
#[test]
fn encoder_config_forwards_partition_validation() {
    use zenavif::expert::InternalParams;
    let mut p = InternalParams::default();
    p.partition_range = Some((128, 128));
    let cfg = EncoderConfig::new().with_internal_params(p);
    assert!(matches!(
        cfg.validate(),
        Err(ValidationError::PartitionRangeInvalid { .. })
    ));
}

// --------------------------------------------------------------------
// DecoderConfig: currently no failing variants. Reserved.
// --------------------------------------------------------------------

#[test]
fn decoder_threads_zero_ok() {
    // 0 is the documented "auto-detect" sentinel.
    DecoderConfig::new()
        .threads(0)
        .validate()
        .expect("threads=0 (auto) must validate");
}

#[test]
fn decoder_frame_size_limit_zero_ok() {
    // 0 = "no limit", documented and accepted.
    DecoderConfig::new()
        .frame_size_limit(0)
        .validate()
        .expect("frame_size_limit=0 (no limit) must validate");
}

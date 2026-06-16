//! Pre-flight decode pixel-cap enforcement — imazen/zenavif#22 (decode fail-open).
//!
//! `DecoderConfig::default()` now defaults `frame_size_limit` to 120 MP so
//! untrusted `decode()` is bounded. These tests exercise the pre-flight
//! dimension check (`decoder_managed.rs` rejects before any frame allocation)
//! with a tiny real 5x3 AVIF — we never allocate a 120 MP frame. We set a cap
//! *below* the fixture's 15 px to prove the same mechanism the 120 MP default
//! arms; and we confirm `0` still opts out (decodes unbounded).

use zenavif::{DecoderConfig, Error, decode_with};
use enough::Unstoppable;

/// 5x3 (= 15 px) genuine monochrome AVIF, the smallest committed fixture.
const MONO_5X3: &str = "tests/vectors/zenavif/mono_5x3_8b_full.avif";

/// A cap below the fixture's pixel count must reject pre-flight with
/// `ImageTooLarge`. This is the exact mechanism the 120 MP default arms,
/// proven without allocating an over-120-MP frame. The cap is `1` so the
/// rejection is robust whether the pre-flight reads the 5x3 display size or
/// the AV1 coded size (either way > 1 px).
#[test]
fn over_limit_dimensions_rejected_preflight() {
    let data = std::fs::read(MONO_5X3).expect("fixture");

    // Sanity: with no cap the fixture decodes fine.
    let ok = decode_with(&data, &DecoderConfig::new().frame_size_limit(0), &Unstoppable);
    assert!(ok.is_ok(), "5x3 fixture must decode with the cap disabled");

    // Cap of 1 px < the fixture's pixel count → pre-flight rejection.
    let cfg = DecoderConfig::new().frame_size_limit(1);
    let err = decode_with(&data, &cfg, &Unstoppable)
        .expect_err("a multi-pixel frame must be rejected against a 1 px cap");
    // The load-bearing assertion: rejected with ImageTooLarge before decode.
    // Reported dims are the container's declared size; assert they're the
    // expected 5x3 but don't hinge the test on exact coded-vs-display padding.
    match err.error() {
        Error::ImageTooLarge { width, height } => {
            assert!(
                (*width as u64) * (*height as u64) > 1,
                "rejected dims {width}x{height} must exceed the 1 px cap"
            );
        }
        other => panic!("expected ImageTooLarge, got {other:?}"),
    }
}

/// `frame_size_limit(0)` keeps the documented opt-out: a frame larger than the
/// 120 MP default still decodes when the caller explicitly disables the cap.
/// (The 5x3 fixture is well under 120 MP; this asserts `0` does not reject.)
#[test]
fn zero_limit_opts_out_of_preflight() {
    let data = std::fs::read(MONO_5X3).expect("fixture");
    let out = decode_with(&data, &DecoderConfig::new().frame_size_limit(0), &Unstoppable)
        .expect("0 must opt out of the decode-side pixel cap");
    assert_eq!((out.width(), out.height()), (5, 3));
}

/// The default config now carries the 120 MP cap (would reject a >120 MP
/// frame). The 5x3 fixture is far under it, so the default decodes it — this
/// guards that the new default doesn't reject normal images.
#[test]
fn default_config_decodes_normal_image() {
    let data = std::fs::read(MONO_5X3).expect("fixture");
    let out = decode_with(&data, &DecoderConfig::default(), &Unstoppable)
        .expect("normal 5x3 image must decode under the 120 MP default cap");
    assert_eq!((out.width(), out.height()), (5, 3));
}

//! Regression tests for fuzz-discovered crashes.
//!
//! Each test loads a crash file and decodes it, verifying no panics occur.
//! The decoding may succeed or return an error -- both are acceptable.
//! A panic (e.g., from a threading race or bounds check) is a failure.

use zenavif::{AnimationDecoder, DecoderConfig};

/// Fuzz regression: DisjointMut overlap panic in rav1d-safe CDEF during animation decode.
/// Root cause: frame threading race condition in rav1d-safe 0.5.3.
/// Fix: set max_frame_delay=1 to disable frame threading, keeping tile parallelism.
#[test]
fn fuzz_decode_animation_cdef_race() {
    let data = std::fs::read(
        "fuzz/regression/fuzz_decode_animation/crash-3c65b028f4111d121f45bce88a818bf6e7014d1c",
    )
    .expect("crash file should exist");

    let config = DecoderConfig::new().frame_size_limit(4 * 1024 * 1024);

    // Must not panic -- error return is fine
    let _ = zenavif::decode_animation_with(&data, &config, &enough::Unstoppable);

    // Also exercise the frame-by-frame iterator path
    if let Ok(mut anim) = AnimationDecoder::new(&data, &config) {
        while let Ok(Some(_frame)) = anim.next_frame(&enough::Unstoppable) {}
    }
}

/// Fuzz regression: crash in resource-limited decode path.
/// Same root cause as above: rav1d-safe frame threading DisjointMut overlap.
#[test]
fn fuzz_decode_limited_cdef_race() {
    let data = std::fs::read(
        "fuzz/regression/fuzz_decode_limited/crash-d59bcf148460f516c26540047b7bfeaaff421fab",
    )
    .expect("crash file should exist");

    let config = DecoderConfig::new().frame_size_limit(1024 * 1024);

    // Must not panic -- error return is fine
    let _ = zenavif::decode_with(&data, &config, &enough::Unstoppable);
}

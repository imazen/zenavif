//! Regression: error traces (`whereat::At<Error>`) must survive the
//! `decoder_managed` decode boundary instead of being dropped.
//!
//! Prior to the fix, several boundaries inside the streaming/sink decode
//! path decomposed an `At<Error>` with `.map_err(|e| e.decompose().0)?`,
//! which throws away the heap trace and returns the bare `Error`. After the
//! fix those sites use `.at()?`, which preserves the inner trace and adds a
//! frame at the propagation site.
//!
//! These tests drive a corrupt/invalid AVIF through the public decode entry
//! point so an `At` error propagates up through `decoder_managed`, then assert
//! the trace is non-empty (`frame_count() >= 1`). A dropped trace would report
//! `frame_count() == 0`.

use enough::Unstoppable;
use zenavif::{DecoderConfig, Error, decode_with};

/// Smallest committed genuine AVIF (5x3 monochrome). We corrupt copies of it
/// so the container still parses far enough to construct the decoder, then the
/// error surfaces from deeper in the pipeline.
const MONO_5X3: &str = "tests/vectors/zenavif/mono_5x3_8b_full.avif";

/// Corrupting the trailing AV1 payload (after the container boxes) makes the
/// AV1 decode stage fail. That error is born inside `decoder_managed`'s decode
/// pipeline and must arrive at the caller carrying a non-empty trace.
#[test]
fn av1_decode_error_preserves_trace() {
    let mut data = std::fs::read(MONO_5X3).expect("fixture");

    // Sanity: the pristine fixture decodes.
    assert!(
        decode_with(
            &data,
            &DecoderConfig::new().frame_size_limit(0),
            &Unstoppable
        )
        .is_ok(),
        "pristine 5x3 fixture must decode"
    );

    // Flip the last 40 bytes (the compressed AV1 data) to break the bitstream
    // without disturbing the leading ISOBMFF boxes the parser reads.
    let n = data.len();
    for b in &mut data[n.saturating_sub(40)..] {
        *b ^= 0xFF;
    }

    let err = decode_with(
        &data,
        &DecoderConfig::new().frame_size_limit(0),
        &Unstoppable,
    )
    .expect_err("a corrupt AV1 payload must fail to decode");

    // Load-bearing assertion: the trace crossed the decoder_managed boundary
    // intact. A decompose()-style drop would leave the trace empty.
    assert!(
        err.frame_count() >= 1,
        "decode error must carry a non-empty trace across the decoder boundary, \
         got frame_count={} (err: {err})",
        err.frame_count()
    );

    // It must be an AV1 decode-stage error, confirming we exercised the
    // decode pipeline (not just a container parse rejection). Since the
    // ErrorCategory reshape, a corrupted bitstream classifies precisely as
    // `Malformed` (recovered from rav1d-safe's real cause via
    // `error_from_rav1d`) rather than the old catch-all `Decode` — accept
    // any of the decode-stage-originating variants, just not `Parse`.
    assert!(
        matches!(
            err.error(),
            Error::Malformed(_) | Error::UnexpectedEof(_) | Error::Decode { .. }
        ),
        "expected an AV1 decode-stage error (Malformed/UnexpectedEof/Decode), got {:?}",
        err.error()
    );
}

/// A truncated container fails at parse time. That `At<Error>` is started in
/// `decoder_managed` and likewise must keep a non-empty trace.
#[test]
fn parse_error_preserves_trace() {
    let data = std::fs::read(MONO_5X3).expect("fixture");
    let truncated = &data[..data.len() / 2];

    let err = decode_with(truncated, &DecoderConfig::new(), &Unstoppable)
        .expect_err("a truncated container must fail to parse");

    assert!(
        err.frame_count() >= 1,
        "parse error must carry a non-empty trace, got frame_count={} (err: {err})",
        err.frame_count()
    );
    assert!(
        matches!(err.error(), Error::Parse(_)),
        "expected Parse error, got {:?}",
        err.error()
    );
}

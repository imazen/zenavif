//! Coverage for the zenavif zencodec adapter.
//!
//! Currently focused on orientation handling: the adapter honors
//! [`zencodec::OrientationHint`] the same way zenjpeg and heic do, so the
//! codecs report orientation consistently. zenavif's orientation source is the
//! container's `irot`/`imir` transform boxes (not EXIF); the native decoder
//! leaves pixels in stored orientation, so the adapter is what applies the
//! transform when the caller asks for it.

use std::borrow::Cow;

use zenavif::AvifDecoderConfig;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _, StreamingDecode as _};
use zenpixels::{Orientation, PixelBuffer, PixelDescriptor, PixelSlice};
use zenpixels_convert::PixelBufferConvertTypedExt as _;

// ── Fixtures (link-u/avif-sample-images) ────────────────────────────────────
//
// `kimono.avif` is the upright reference (722×1024, no transform).
// `kimono.rotate90.avif` carries `irot` angle=270 → intrinsic `Rotate90`
// (axis-swapping): stored (coded) dims 1024×722, display dims 722×1024.
// `kimono.mirror-horizontal.avif` carries `imir` axis=1 → intrinsic `FlipV`
// (a pure flip, no axis swap): stored dims == display dims == 722×1024.

const UPRIGHT: &str = "tests/vectors/link-u/kimono.avif";
const ROT90: &str = "tests/vectors/link-u/kimono.rotate90.avif";
const MIRROR_H: &str = "tests/vectors/link-u/kimono.mirror-horizontal.avif";

const UPRIGHT_DIMS: (u32, u32) = (722, 1024);
const ROT90_STORED: (u32, u32) = (1024, 722);
const ROT90_DISPLAY: (u32, u32) = (722, 1024);
/// Intrinsic orientation of `kimono.rotate90.avif` (irot angle=270).
const ROT90_INTRINSIC: Orientation = Orientation::Rotate90;

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn decode_correct(data: &[u8]) -> zencodec::decode::DecodeOutput {
    AvifDecoderConfig::new()
        .with_orientation(zencodec::OrientationHint::Correct)
        .job()
        .decoder(Cow::Borrowed(data), &[PixelDescriptor::RGB8_SRGB])
        .expect("decoder")
        .decode()
        .expect("decode Correct")
}

fn decode_preserve(data: &[u8]) -> zencodec::decode::DecodeOutput {
    AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(data), &[PixelDescriptor::RGB8_SRGB])
        .expect("decoder")
        .decode()
        .expect("decode Preserve")
}

/// Crude mean-absolute-difference over RGB8 pixels of two equally-sized buffers.
/// The rotated and upright kimono fixtures are *separately* lossy-encoded files,
/// so even at matching orientation they differ by ~2.8/255 (codec noise). The
/// correct bake lands there; every wrong axis-swapping orientation
/// (Rotate270/Transpose/Transverse) scrambles the content to 40-55/255, and the
/// non-swapping ones mismatch dims entirely — measured. A threshold between
/// those clusters proves the bake applies the *right* transform without pulling
/// in a perceptual-metric dependency.
fn mean_abs_diff_rgb8(a: &PixelBuffer, b: &PixelBuffer) -> f64 {
    assert_eq!((a.width(), a.height()), (b.width(), b.height()));
    let a = a.to_rgb8();
    let b = b.to_rgb8();
    let ar = a.as_imgref();
    let br = b.as_imgref();
    let mut sum = 0u64;
    let mut n = 0u64;
    for (ra, rb) in ar.rows().zip(br.rows()) {
        for (pa, pb) in ra.iter().zip(rb.iter()) {
            sum += (pa.r as i32 - pb.r as i32).unsigned_abs() as u64;
            sum += (pa.g as i32 - pb.g as i32).unsigned_abs() as u64;
            sum += (pa.b as i32 - pb.b as i32).unsigned_abs() as u64;
            n += 3;
        }
    }
    sum as f64 / n as f64
}

// ── Probe: Preserve (default) vs Correct ────────────────────────────────────

#[test]
fn orientation_preserve_default_reports_stored_dims_and_tag() {
    let data = read(ROT90);
    // Default config == OrientationHint::Preserve.
    let info = AvifDecoderConfig::new().job().probe(&data).expect("probe");
    assert_eq!(
        (info.width, info.height),
        ROT90_STORED,
        "Preserve must report stored (coded, pre-rotation) dims"
    );
    assert!(
        info.orientation.swaps_axes() && !info.orientation.is_identity(),
        "Preserve must report the intrinsic 90/270 orientation tag, got {:?}",
        info.orientation
    );
    assert_eq!(info.orientation, ROT90_INTRINSIC);
    assert_eq!(
        (info.display_width(), info.display_height()),
        ROT90_DISPLAY,
        "display_width/height must yield the upright dims under Preserve"
    );
}

#[test]
fn orientation_correct_reports_display_dims_and_identity() {
    let data = read(ROT90);
    let info = AvifDecoderConfig::new()
        .with_orientation(zencodec::OrientationHint::Correct)
        .job()
        .probe(&data)
        .expect("probe");
    assert_eq!(
        (info.width, info.height),
        ROT90_DISPLAY,
        "Correct must report display (post-rotation) dims"
    );
    assert_eq!(
        info.orientation,
        Orientation::Identity,
        "Correct must report Identity — orientation is baked into the pixels"
    );
    assert_eq!((info.display_width(), info.display_height()), ROT90_DISPLAY);
}

#[test]
fn orientation_mirror_preserve_reports_flip_tag_without_swapping_dims() {
    // imir axis=1 is a pure flip (no axis swap): stored dims == display dims,
    // but Preserve must still report a non-identity flip orientation.
    let data = read(MIRROR_H);
    let info = AvifDecoderConfig::new().job().probe(&data).expect("probe");
    assert!(
        !info.orientation.is_identity() && !info.orientation.swaps_axes(),
        "imir Preserve must report a pure flip (non-identity, non-swapping), got {:?}",
        info.orientation
    );
    assert_eq!((info.width, info.height), UPRIGHT_DIMS);
    // A pure flip does not swap dims, so display dims equal stored dims.
    assert_eq!(info.display_width(), info.width);
    assert_eq!(info.display_height(), info.height);
}

// ── output_info mirrors the probe reporting ─────────────────────────────────

#[test]
fn orientation_output_info_matches_probe_for_both_hints() {
    let data = read(ROT90);

    let preserve = AvifDecoderConfig::new()
        .job()
        .output_info(&data)
        .expect("output_info Preserve");
    assert_eq!((preserve.width, preserve.height), ROT90_STORED);
    assert_eq!(
        preserve.orientation_applied,
        Orientation::Identity,
        "Preserve applies no orientation — caller orients"
    );

    let correct = AvifDecoderConfig::new()
        .with_orientation(zencodec::OrientationHint::Correct)
        .job()
        .output_info(&data)
        .expect("output_info Correct");
    assert_eq!((correct.width, correct.height), ROT90_DISPLAY);
    assert_eq!(
        correct.orientation_applied, ROT90_INTRINSIC,
        "Correct bakes the intrinsic orientation"
    );
}

// ── Single-image decode (Decode::decode) ────────────────────────────────────

#[test]
fn orientation_decode_dims_match_probe_for_both_hints() {
    let data = read(ROT90);

    // Preserve (default): decoded pixels stay in stored orientation, and the
    // output ImageInfo dims match the pixels.
    let preserve = decode_preserve(&data);
    assert_eq!(
        (preserve.pixels().width(), preserve.pixels().rows()),
        ROT90_STORED,
        "Preserve decode must output stored-orientation pixels"
    );
    assert_eq!(
        (preserve.info().width, preserve.info().height),
        ROT90_STORED,
        "Preserve decode ImageInfo dims must match the decoded pixels"
    );
    assert!(
        preserve.info().orientation.swaps_axes(),
        "Preserve decode must tag the intrinsic orientation"
    );
    assert_eq!(
        (
            preserve.info().display_width(),
            preserve.info().display_height()
        ),
        ROT90_DISPLAY,
    );

    // Correct: decoded pixels are baked upright.
    let correct = decode_correct(&data);
    assert_eq!(
        (correct.pixels().width(), correct.pixels().rows()),
        ROT90_DISPLAY,
        "Correct decode must output display-orientation (upright) pixels"
    );
    assert_eq!(
        (correct.info().width, correct.info().height),
        ROT90_DISPLAY,
        "Correct decode ImageInfo dims must match the baked pixels"
    );
    assert_eq!(correct.info().orientation, Orientation::Identity);
}

/// The load-bearing correctness check: a `Correct` decode of the rotated file
/// must produce pixels that match the upright reference. This proves the bake
/// applies the *right* transform in the *right* direction (a no-op or a wrong
/// rotation would blow past the threshold).
#[test]
fn orientation_correct_bakes_pixels_to_match_upright_reference() {
    let rotated = decode_correct(&read(ROT90));
    let upright = decode_preserve(&read(UPRIGHT));

    let rotated_buf = rotated.into_buffer();
    let upright_buf = upright.into_buffer();
    assert_eq!(
        (rotated_buf.width(), rotated_buf.height()),
        UPRIGHT_DIMS,
        "baked output must have upright dims"
    );
    let mad = mean_abs_diff_rgb8(&rotated_buf, &upright_buf);
    // Correct bake measures ~2.8 (inter-file lossy noise); nearest wrong
    // orientation is ~41. 5.0 sits cleanly between with ~8x margin either side.
    assert!(
        mad < 5.0,
        "Correct-baked rotated image must match the upright reference \
         (mean abs diff {mad:.3}/255 — a wrong/no-op bake measures 40+)"
    );
}

// ── Streaming decode bakes orientation too ──────────────────────────────────

/// Collect a streaming decode into one buffer.
fn stream_collect(data: &[u8], hint: zencodec::OrientationHint) -> (PixelBuffer, Orientation) {
    let mut dec = AvifDecoderConfig::new()
        .with_orientation(hint)
        .job()
        .streaming_decoder(Cow::Borrowed(data), &[PixelDescriptor::RGB8_SRGB])
        .expect("streaming_decoder");
    let reported = dec.info().orientation;
    let (w, h) = (dec.info().width, dec.info().height);
    // kimono is 8-bit and we asked for RGB8_SRGB, so the strips are RGB8.
    let mut out = PixelBuffer::new(w, h, PixelDescriptor::RGB8_SRGB);
    {
        let mut om = out.as_slice_mut();
        while let Some((y, strip)) = dec.next_batch().expect("next_batch") {
            let strip: PixelSlice<'_> = strip;
            for row in 0..strip.rows() {
                om.row_mut(y + row).copy_from_slice(strip.row(row));
            }
        }
    }
    (out, reported)
}

#[test]
fn orientation_streaming_correct_matches_single_decode() {
    let data = read(ROT90);
    let (streamed, reported) = stream_collect(&data, zencodec::OrientationHint::Correct);
    assert_eq!(
        (streamed.width(), streamed.height()),
        ROT90_DISPLAY,
        "streaming Correct must emit display-orientation dims"
    );
    assert_eq!(
        reported,
        Orientation::Identity,
        "streaming Correct must report Identity on info()"
    );

    // Streamed bake must equal the one-shot bake.
    let single = decode_correct(&data).into_buffer();
    let mad = mean_abs_diff_rgb8(&streamed, &single);
    assert!(
        mad < 0.001,
        "streaming and one-shot Correct decode must agree byte-for-byte (mad {mad})"
    );
}

#[test]
fn orientation_streaming_preserve_keeps_stored_dims() {
    let data = read(ROT90);
    let (streamed, reported) = stream_collect(&data, zencodec::OrientationHint::Preserve);
    assert_eq!(
        (streamed.width(), streamed.height()),
        ROT90_STORED,
        "streaming Preserve must emit stored-orientation dims"
    );
    assert!(
        reported.swaps_axes(),
        "streaming Preserve must report the intrinsic tag"
    );
}

// ── Pattern B: ErrorCategory + codec name survive Dyn dispatch (envelope) ────

/// The load-bearing Pattern-B check: drive a malformed AVIF through
/// [`DynDecoderConfig`](zencodec::decode::DynDecoderConfig), erase the error to
/// `BoxedError` (`Box<dyn Error + Send + Sync>`), and prove a generic consumer
/// still recovers the real [`ErrorCategory`](zencodec::ErrorCategory) **and** the
/// originating codec name. Under Pattern A (`type Error = At<native>`), the boxed
/// error carries no `CodecError` envelope, so `error_category()` / `codec_error()`
/// return `None`; under Pattern B (this crate now) they return `Some(..)`.
#[test]
fn dyn_dispatch_recovers_category_and_codec_name() {
    use zencodec::decode::DynDecoderConfig;
    use zencodec::{CodecError, CodecErrorExt, ErrorCategory};

    // Clearly-malformed input: an `ftyp avif` lead-in followed by garbage, with
    // no valid box structure after it — long enough that the container parser
    // actually inspects it rather than rejecting on length alone.
    let bytes: &[u8] =
        b"\x00\x00\x00\x18ftypavifmif1miafMA1B garbage-not-a-real-avif-container-payload!!!!";

    // Typed (Pattern B) path: the genuine category zenavif assigns to this input.
    // Derived at runtime so the assertion is robust to the exact parse variant,
    // while still proving it is a real classification (not the `Internal`
    // catch-all). In practice malformed containers land on `MalformedImage` or
    // `UnexpectedEof`.
    let typed = AvifDecoderConfig::new()
        .job()
        .probe(bytes)
        .expect_err("malformed input must fail to probe");
    let expected = typed.error().category();
    assert!(
        !matches!(expected, ErrorCategory::Internal(_)),
        "a malformed container must get a real input classification, not the \
         Internal catch-all (got {expected:?})"
    );
    assert_eq!(typed.error().codec(), Some("zenavif"));

    // Dyn-dispatch path: erase to `BoxedError` and prove the category + codec
    // name survive `Box<dyn Error>` erasure.
    let dyn_cfg: &dyn DynDecoderConfig = &AvifDecoderConfig::new();
    let erased = dyn_cfg
        .dyn_job()
        .probe(bytes)
        .expect_err("malformed input must fail through dyn dispatch");
    assert_eq!(
        erased.error_category(),
        Some(expected),
        "ErrorCategory must survive Box<dyn Error> erasure (None under Pattern A)"
    );
    assert_eq!(
        erased.codec_error().and_then(CodecError::codec),
        Some("zenavif"),
        "codec name must survive Box<dyn Error> erasure"
    );
}

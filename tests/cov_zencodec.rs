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

// ── Threading policy (src/codec/threads.rs) ─────────────────────────────────
//
// `policy_to_threads` measured 0 of 6 regions covered in EVERY feature combo
// (cargo-llvm-cov 0.8.7, 2026-08-11; docs/TEST_COVERAGE.md): the default
// `ResourceLimits` threading is `Parallel`, which `effective_config` skips on
// purpose, so no test ever lowered a policy to a concrete thread count. The
// contract that matters to a caller is that asking for `Sequential` changes
// how the work is scheduled and NOT what comes out.

/// Decoded pixels must be byte-identical under every threading policy, and a
/// `Sequential` request must be honoured rather than rejected.
///
/// Fixture: the committed `kodim03_yuv420_8bpc.avif` (768×512 4:2:0 8-bit) —
/// large enough for rav1d-safe to use more than one tile worker at the
/// `Parallel` (auto) default, so the two arms really are different schedules.
#[test]
fn decode_is_byte_identical_under_every_threading_policy() {
    const KODIM: &str = "tests/vectors/libavif/kodim03_yuv420_8bpc.avif";
    let bytes = read(KODIM);

    let decode_under = |policy: zencodec::ThreadingPolicy| -> Vec<u8> {
        let limits = zencodec::ResourceLimits::default().with_threading(policy);
        let out = AvifDecoderConfig::new()
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(&bytes), &[PixelDescriptor::RGB8_SRGB])
            .expect("decoder")
            .decode()
            .unwrap_or_else(|e| panic!("decode under {policy:?}: {e:?}"));
        let p = out.pixels();
        let (w, h, stride) = (p.width() as usize, p.rows() as usize, p.stride());
        let b = p.as_strided_bytes();
        (0..h)
            .flat_map(|y| b[y * stride..][..w * 3].to_vec())
            .collect()
    };

    let parallel = decode_under(zencodec::ThreadingPolicy::Parallel);
    assert!(!parallel.is_empty(), "decode produced no pixels");
    let sequential = decode_under(zencodec::ThreadingPolicy::Sequential);
    assert_eq!(
        sequential.len(),
        parallel.len(),
        "Sequential and Parallel produced different buffer sizes"
    );
    assert!(
        sequential == parallel,
        "Sequential decode differs from Parallel at byte {} — thread count must \
         never change pixels",
        sequential
            .iter()
            .zip(parallel.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(0)
    );

    // The deprecated legacy variants must still decode (they lower to "auto",
    // and the deprecation warning belongs at the construction site, not here).
    #[allow(deprecated)]
    let legacy = decode_under(zencodec::ThreadingPolicy::Balanced);
    assert_eq!(legacy, parallel, "a legacy policy must not change pixels");
}

// ── Row-sink decode (src/decoder_managed/sink.rs) ───────────────────────────
//
// `decode_to_sink` is the streaming counterpart of the buffered decode: it
// converts strip by strip so nothing holds a full RGB image. It measured
// 114/416 regions (27.4%) in the broadest feature combo (cargo-llvm-cov
// 0.8.7, 2026-08-11; docs/TEST_COVERAGE.md) — every test took the buffered
// path. Two code paths for one operation, so the invariant to pin is that they
// produce the SAME pixels (CLAUDE.md: "if two code paths for the same
// operation produce different output, that is a bug in one of them").

/// A sink that reassembles the strips into one contiguous image, checking the
/// protocol as it goes (one `begin`, sequential strips, one `finish`).
struct CollectSink {
    desc: Option<PixelDescriptor>,
    total: (u32, u32),
    /// Reassembled tightly-packed image.
    image: Vec<u8>,
    /// Buffer handed to the codec for the current strip.
    strip: Vec<u8>,
    /// Geometry of the strip the codec currently owns.
    pending: Option<(u32, u32, u32, usize)>,
    strips: usize,
    began: bool,
    finished: bool,
}

impl CollectSink {
    fn new() -> Self {
        Self {
            desc: None,
            total: (0, 0),
            image: Vec::new(),
            strip: Vec::new(),
            pending: None,
            strips: 0,
            began: false,
            finished: false,
        }
    }

    /// Copy the strip the codec just filled into the reassembled image.
    fn flush_pending(&mut self) {
        if let Some((y, h, w, stride)) = self.pending.take() {
            let bpp = self.desc.expect("descriptor from begin").bytes_per_pixel();
            let row_bytes = w as usize * bpp;
            for r in 0..h as usize {
                let src = &self.strip[r * stride..][..row_bytes];
                let off = (y as usize + r) * row_bytes;
                self.image[off..off + row_bytes].copy_from_slice(src);
            }
        }
    }

    fn bpp(&self) -> usize {
        self.desc.expect("descriptor").bytes_per_pixel()
    }
}

impl zencodec::decode::DecodeRowSink for CollectSink {
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Result<(), zencodec::decode::SinkError> {
        assert!(!self.began, "begin called twice");
        self.began = true;
        self.total = (width, height);
        self.desc = Some(descriptor);
        self.image = vec![0u8; width as usize * height as usize * descriptor.bytes_per_pixel()];
        Ok(())
    }

    fn provide_next_buffer(
        &mut self,
        y: u32,
        height: u32,
        width: u32,
        descriptor: PixelDescriptor,
    ) -> Result<zenpixels::PixelSliceMut<'_>, zencodec::decode::SinkError> {
        assert!(self.began, "provide_next_buffer before begin");
        self.flush_pending();
        assert_eq!(
            width, self.total.0,
            "strip width must match the width announced to begin()"
        );
        assert!(
            y + height <= self.total.1,
            "strip {y}..{} runs past the {} announced rows",
            y + height,
            self.total.1
        );
        let stride = width as usize * descriptor.bytes_per_pixel();
        self.strip.clear();
        self.strip.resize(height as usize * stride, 0);
        self.pending = Some((y, height, width, stride));
        self.strips += 1;
        Ok(
            zenpixels::PixelSliceMut::new(&mut self.strip, width, height, stride, descriptor)
                .expect("strip buffer sized from the codec's own geometry"),
        )
    }

    fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
        assert!(!self.finished, "finish called twice");
        self.flush_pending();
        self.finished = true;
        Ok(())
    }
}

/// Tightly-packed copy of a decoded buffer's bytes.
fn packed_bytes(out: &zencodec::decode::DecodeOutput) -> (Vec<u8>, usize, usize, usize) {
    let p = out.pixels();
    let (w, h, stride) = (p.width() as usize, p.rows() as usize, p.stride());
    let bpp = p.descriptor().bytes_per_pixel();
    let b = p.as_strided_bytes();
    let packed = (0..h)
        .flat_map(|y| b[y * stride..][..w * bpp].to_vec())
        .collect();
    (packed, w, h, bpp)
}

/// Row-sink decode must reassemble to byte-identical pixels vs the buffered
/// decode on a colour 4:2:0 photo — the case where the sink really does
/// convert strip-by-strip (8 strips for this 768x512 fixture).
#[test]
fn row_sink_decode_is_byte_identical_to_buffered_decode() {
    const KODIM: &str = "tests/vectors/libavif/kodim03_yuv420_8bpc.avif";
    let bytes = read(KODIM);

    let buffered = AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(&bytes), &[])
        .expect("decoder")
        .decode()
        .expect("buffered decode");
    let (reference, bw, bh, bpp) = packed_bytes(&buffered);

    let mut sink = CollectSink::new();
    let info = AvifDecoderConfig::new()
        .job()
        .push_decoder(Cow::Borrowed(&bytes), &mut sink, &[])
        .expect("row-sink decode");
    assert!(sink.began && sink.finished, "sink protocol incomplete");
    assert_eq!(
        (info.width as usize, info.height as usize),
        (bw, bh),
        "row-sink OutputInfo dims differ from the buffered decode"
    );
    assert_eq!(
        sink.bpp(),
        bpp,
        "row-sink pixel format differs from the buffered decode"
    );
    assert_eq!(
        sink.image.len(),
        reference.len(),
        "reassembled size differs"
    );
    if sink.image != reference {
        let at = sink
            .image
            .iter()
            .zip(reference.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        panic!(
            "row-sink decode differs from buffered decode at byte {at} (sink {}, buffered \
             {}) — the strip path and the whole-image path disagree on pixels",
            sink.image[at], reference[at]
        );
    }
    assert!(
        sink.strips > 1,
        "the 8-bit colour sink path delivered {} strip(s) — it is supposed to convert \
         strip-wise, so this test would not be measuring the strip path at all",
        sink.strips
    );
}

/// Monochrome: all three decode entry points of the same adapter — buffered
/// `decode()`, `streaming_decoder()` and the `push_decoder()` row sink — must
/// emit the SAME pixel format and the SAME bytes for the same input and the
/// same `preferred`.
///
/// This was imazen/zenavif#35: the row sink derived its descriptor from bit
/// depth + alpha alone, never consulted `preferred` and never called
/// `set_native_gray`, so a caller asking for Gray8 got Rgb8 — 3x the bytes in
/// a layout no other path emitted, while `DecodeCapabilities` advertises
/// `native_gray`. It was a format gap rather than a wrong-pixel gap (the
/// triples were neutral), so this test used to pin content parity and flip
/// itself to byte identity once the descriptors converged. They have
/// converged, so the weaker property is gone: this now asserts strict format
/// equality plus byte identity, three ways, and cannot pass on neutral-RGB
/// output any more.
///
/// MEASURED after the fix (6 mono fixtures x 4 `preferred` lists): the row
/// sink matches streaming in 24/24 cells and buffered in 22/24. The 2 misses
/// are a different, pre-existing bug — on the 10-bit fixture at
/// `preferred = [Gray8]` or `[Rgb8]`, buffered downconverts to 8-bit per
/// `negotiate_format` while BOTH streaming and the row sink stay 16-bit — so
/// the `preferred` list here deliberately admits either depth
/// (`[Gray8, Gray16]`), which all three paths agree on.
#[test]
fn row_sink_mono_matches_buffered_and_streaming_byte_for_byte() {
    for path in [
        "tests/vectors/zenavif/mono_gradient_8b_full.avif",
        "tests/vectors/zenavif/mono_gradient_8b_limited.avif",
        "tests/vectors/zenavif/mono_gradient_10b_full.avif",
        // Gray-class ICC: the collapse is allowed, so this must be Gray too.
        "tests/vectors/zenavif/mono_gradient_8b_grayicc.avif",
        // RGB-class ICC that cannot describe a Gray layout: all three paths
        // must agree on suppressing the collapse and staying RGB.
        "tests/vectors/zenavif/mono_gradient_8b_rgbicc.avif",
    ] {
        let bytes = read(path);
        // Ask for gray explicitly — exactly the request the sink path dropped.
        let pref = [PixelDescriptor::GRAY8_SRGB, PixelDescriptor::GRAY16_SRGB];

        let buffered = AvifDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(&bytes), &pref)
            .expect("decoder")
            .decode()
            .unwrap_or_else(|e| panic!("buffered decode of {path}: {e:?}"));
        let (reference, _bw, _bh, _bpp) = packed_bytes(&buffered);
        let ref_desc = buffered.pixels().descriptor();

        let mut sink = CollectSink::new();
        let reported = AvifDecoderConfig::new()
            .job()
            .push_decoder(Cow::Borrowed(&bytes), &mut sink, &pref)
            .unwrap_or_else(|e| panic!("row-sink decode of {path}: {e:?}"));
        let sink_desc = sink.desc.expect("sink descriptor");

        assert_eq!(
            sink_desc.pixel_format(),
            ref_desc.pixel_format(),
            "{path}: row-sink format {:?} != buffered {:?} — zenavif#35 has regressed; the \
             sink is ignoring `preferred` again",
            sink_desc.pixel_format(),
            ref_desc.pixel_format()
        );
        assert_eq!(
            sink.image, reference,
            "{path}: formats agree but the row-sink pixels differ from the buffered pixels"
        );

        // The returned OutputInfo must describe what the sink was actually
        // handed — the second half of #35 was a re-derived descriptor that
        // could silently disagree with the pixels.
        assert_eq!(
            reported.native_format.pixel_format(),
            sink_desc.pixel_format(),
            "{path}: push_decoder reported {:?} but handed the sink {:?}",
            reported.native_format.pixel_format(),
            sink_desc.pixel_format()
        );

        // Third path: streaming must agree with both.
        let mut stream = AvifDecoderConfig::new()
            .job()
            .streaming_decoder(Cow::Borrowed(&bytes), &pref)
            .unwrap_or_else(|e| panic!("streaming decode of {path}: {e:?}"));
        let mut streamed: Vec<u8> = Vec::new();
        let mut stream_desc = None;
        while let Some((_y, strip)) = stream.next_batch().expect("next_batch") {
            let strip: PixelSlice<'_> = strip;
            let d = strip.descriptor();
            stream_desc = Some(d);
            let row_bytes = strip.width() as usize * d.bytes_per_pixel();
            for row in 0..strip.rows() {
                streamed.extend_from_slice(&strip.row(row)[..row_bytes]);
            }
        }
        let stream_desc = stream_desc.expect("streaming emitted no strips");
        assert_eq!(
            stream_desc.pixel_format(),
            sink_desc.pixel_format(),
            "{path}: streaming format {:?} != row-sink {:?}",
            stream_desc.pixel_format(),
            sink_desc.pixel_format()
        );
        assert_eq!(
            streamed, sink.image,
            "{path}: streaming pixels differ from the row-sink pixels"
        );
    }
}

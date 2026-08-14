//! Cross-path negotiation matrix: `preferred` x input CICP x depth x alpha,
//! swept over every public decode entry point.
//!
//! This exists because zenavif#39 found a *panic* reachable from untrusted
//! input through an ordinary caller preference: `preferred = [Rgba8]` over an
//! HDR PQ file drove `negotiate_format` into `PixelBuffer::to_rgba8()`, whose
//! `RowConverter` has no plan for PQ -> sRGB without a peak-luminance
//! parameter, and which signals that by `expect`-ing. The file's own CICP
//! selects the arm, so the caller cannot avoid it by validating its own input.
//!
//! A single regression test for that one cell would not have found it, and
//! would not find the next one. The deliverable is the *product*: every
//! committed fixture (spanning 8/10/12-bit, mono/colour, alpha/no-alpha,
//! sRGB/PQ/HLG/unspecified transfer, grid/non-grid) crossed with every
//! `preferred` list the adapter advertises support for, crossed with all five
//! decode entry points.
//!
//! Related: #36 (streaming paths never ran `negotiate_format`), #37 (CICP
//! transfer lost from the streaming descriptors), #38 (row sink ignored
//! `ReconstructHdr`).

use std::borrow::Cow;
use std::panic::AssertUnwindSafe;

use rgb::{Rgb, Rgba};
use zenavif::AvifDecoderConfig;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _, StreamingDecode as _};
use zenpixels::{PixelDescriptor, PixelSlice};

// ── Fixture set ─────────────────────────────────────────────────────────────
//
// Chosen to cover the axes that select conversion arms, not to be exhaustive
// over the corpus. `depth` / `alpha` / `transfer` in the comments are what the
// container signals, verified by the `matrix_axes_are_actually_covered` test
// below rather than trusted from these notes.

/// `(path, label)` — every one is committed in-tree (no downloader needed).
const FIXTURES: &[(&str, &str)] = &[
    // 8-bit, sRGB, no alpha — the ordinary case.
    ("tests/vectors/libavif/kodim03_yuv420_8bpc.avif", "sdr8"),
    ("tests/vectors/libavif/colors_sdr_srgb.avif", "sdr8_colors"),
    // 8-bit with alpha.
    ("tests/vectors/libavif/alpha_noispe.avif", "alpha8"),
    // 10-bit HDR PQ, three primary sets. These are the #39 trigger.
    ("tests/vectors/libavif/colors_hdr_p3.avif", "pq_p3"),
    ("tests/vectors/libavif/colors_hdr_rec2020.avif", "pq_2020"),
    ("tests/vectors/libavif/colors_hdr_srgb.avif", "pq_srgb"),
    (
        "tests/vectors/libavif/cosmos1650_yuv444_10bpc_p3pq.avif",
        "pq_444_10b",
    ),
    // HDR, wide gamut, non-PQ curve.
    ("tests/vectors/libavif/seine_hdr_rec2020.avif", "hdr_2020"),
    (
        "tests/vectors/libavif/colors_wcg_hdr_rec2020.avif",
        "wcg_hdr",
    ),
    // 12-bit.
    ("tests/vectors/libavif/weld_sato_12B_8B_q0.avif", "b12"),
    // Monochrome, 8- and 10-bit, plus the ICC-class variants that decide
    // whether the gray collapse is allowed at all.
    ("tests/vectors/zenavif/mono_gradient_8b_full.avif", "mono8"),
    (
        "tests/vectors/zenavif/mono_gradient_10b_full.avif",
        "mono10",
    ),
    (
        "tests/vectors/zenavif/mono_gradient_8b_rgbicc.avif",
        "mono8_rgbicc",
    ),
    (
        "tests/vectors/zenavif/mono_gradient_8b_grayicc.avif",
        "mono8_grayicc",
    ),
    // Grid (the strip paths take a different branch entirely).
    ("tests/vectors/libavif/sofa_grid1x5_420.avif", "grid"),
    (
        "tests/vectors/libavif/color_grid_alpha_nogrid.avif",
        "grid_alpha",
    ),
    // Degenerate size.
    ("tests/vectors/libavif/white_1x1.avif", "px1"),
    // ICC-carrying colour image.
    ("tests/vectors/libavif/paris_icc_exif_xmp.avif", "icc8"),
];

/// Every descriptor the adapter lists in `supported_decode_descriptors`, each
/// on its own, plus the multi-entry lists a real caller writes. `[]` (no
/// preference) is the native-format control.
fn preferences() -> Vec<(&'static str, Vec<PixelDescriptor>)> {
    use PixelDescriptor as D;
    vec![
        ("[]", vec![]),
        ("[Rgb8]", vec![D::RGB8_SRGB]),
        ("[Rgba8]", vec![D::RGBA8_SRGB]),
        ("[Rgb16]", vec![D::RGB16_SRGB]),
        ("[Rgba16]", vec![D::RGBA16_SRGB]),
        ("[Gray8]", vec![D::GRAY8_SRGB]),
        ("[Gray16]", vec![D::GRAY16_SRGB]),
        ("[Gray8,Gray16]", vec![D::GRAY8_SRGB, D::GRAY16_SRGB]),
        ("[Rgb8,Rgba8]", vec![D::RGB8_SRGB, D::RGBA8_SRGB]),
        ("[Gray8,Rgb8]", vec![D::GRAY8_SRGB, D::RGB8_SRGB]),
        // A 16-bit-first list on an 8-bit source: negotiation must decline
        // the upscale and fall through rather than fabricate depth.
        ("[Rgb16,Rgb8]", vec![D::RGB16_SRGB, D::RGB8_SRGB]),
    ]
}

/// The five decode entry points a caller can reach with a `preferred` list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Path {
    /// `Decode::decode` — the buffered reference.
    Buffered,
    /// `DecodeJob::streaming_decoder` + `next_batch`.
    Streaming,
    /// `DecodeJob::push_decoder` into a `DecodeRowSink`.
    RowSink,
    /// Buffered with `OrientationHint::Correct` (the bake branch).
    BufferedBaked,
    /// Streaming with `OrientationHint::Correct` (the separate bake branch).
    StreamingBaked,
}

const PATHS: &[Path] = &[
    Path::Buffered,
    Path::Streaming,
    Path::RowSink,
    Path::BufferedBaked,
    Path::StreamingBaked,
];

impl Path {
    fn label(self) -> &'static str {
        match self {
            Path::Buffered => "buffered",
            Path::Streaming => "streaming",
            Path::RowSink => "rowsink",
            Path::BufferedBaked => "buffered+bake",
            Path::StreamingBaked => "streaming+bake",
        }
    }

    fn config(self) -> AvifDecoderConfig {
        let c = AvifDecoderConfig::new();
        match self {
            Path::BufferedBaked | Path::StreamingBaked => {
                c.with_orientation(zencodec::OrientationHint::Correct)
            }
            _ => c,
        }
    }
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// What one matrix cell produced: the descriptor the path reported, plus the
/// tightly-packed pixel bytes so paths can be compared for pixel identity.
struct Cell {
    desc: PixelDescriptor,
    bytes: Vec<u8>,
    dims: (u32, u32),
}

/// A sink that reassembles strips into one contiguous image.
struct CollectSink {
    desc: Option<PixelDescriptor>,
    total: (u32, u32),
    image: Vec<u8>,
    strip: Vec<u8>,
    pending: Option<(u32, u32, u32, usize)>,
}

impl CollectSink {
    fn new() -> Self {
        Self {
            desc: None,
            total: (0, 0),
            image: Vec::new(),
            strip: Vec::new(),
            pending: None,
        }
    }

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
}

impl zencodec::decode::DecodeRowSink for CollectSink {
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Result<(), zencodec::decode::SinkError> {
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
        self.flush_pending();
        if self.desc.is_none() {
            self.desc = Some(descriptor);
        }
        if self.image.is_empty() {
            self.total = (width, self.total.1.max(y + height));
            self.image =
                vec![0u8; width as usize * self.total.1 as usize * descriptor.bytes_per_pixel()];
        }
        let stride = width as usize * descriptor.bytes_per_pixel();
        self.strip.clear();
        self.strip.resize(height as usize * stride, 0);
        self.pending = Some((y, height, width, stride));
        Ok(
            zenpixels::PixelSliceMut::new(&mut self.strip, width, height, stride, descriptor)
                .expect("strip buffer sized from the codec's own geometry"),
        )
    }

    fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
        self.flush_pending();
        Ok(())
    }
}

/// Run one matrix cell. `Err` is a *typed* decode error (fine — the point of
/// the sweep is that failures arrive as `Err`, not as an unwind).
fn run_cell(path: Path, bytes: &[u8], pref: &[PixelDescriptor]) -> Result<Cell, String> {
    match path {
        Path::Buffered | Path::BufferedBaked => {
            let out = path
                .config()
                .job()
                .decoder(Cow::Borrowed(bytes), pref)
                .map_err(|e| format!("{e}"))?
                .decode()
                .map_err(|e| format!("{e}"))?;
            let p = out.pixels();
            let (w, h, stride) = (p.width() as usize, p.rows() as usize, p.stride());
            let bpp = p.descriptor().bytes_per_pixel();
            let raw = p.as_strided_bytes();
            let packed = (0..h)
                .flat_map(|y| raw[y * stride..][..w * bpp].to_vec())
                .collect();
            Ok(Cell {
                desc: p.descriptor(),
                bytes: packed,
                dims: (w as u32, h as u32),
            })
        }
        Path::Streaming | Path::StreamingBaked => {
            let mut dec = path
                .config()
                .job()
                .streaming_decoder(Cow::Borrowed(bytes), pref)
                .map_err(|e| format!("{e}"))?;
            let mut image: Vec<u8> = Vec::new();
            let mut desc = None;
            let mut w = 0u32;
            let mut rows = 0u32;
            while let Some((_y, strip)) = dec.next_batch().map_err(|e| format!("{e}"))? {
                let strip: PixelSlice<'_> = strip;
                let d = strip.descriptor();
                desc = Some(d);
                w = strip.width();
                rows += strip.rows();
                let row_bytes = strip.width() as usize * d.bytes_per_pixel();
                for row in 0..strip.rows() {
                    image.extend_from_slice(&strip.row(row)[..row_bytes]);
                }
            }
            let desc = desc.ok_or_else(|| "streaming emitted no strips".to_string())?;
            Ok(Cell {
                desc,
                bytes: image,
                dims: (w, rows),
            })
        }
        Path::RowSink => {
            let mut sink = CollectSink::new();
            let info = path
                .config()
                .job()
                .push_decoder(Cow::Borrowed(bytes), &mut sink, pref)
                .map_err(|e| format!("{e}"))?;
            let desc = sink
                .desc
                .ok_or_else(|| "sink got no descriptor".to_string())?;
            Ok(Cell {
                desc,
                bytes: sink.image,
                dims: (info.width, info.height),
            })
        }
    }
}

/// Outcome of one cell, flattened for reporting.
enum Outcome {
    Ok(Cell),
    /// A typed decode error. The message is kept so a failing sweep can be
    /// diagnosed from its output without a rerun, even though the assertions
    /// only count these.
    Err(#[allow(dead_code)] String),
    Panic(String),
}

/// Run a cell with unwinds caught, so one panicking combination does not stop
/// the sweep. The panic hook is silenced for the duration: a sweep with
/// hundreds of cells would otherwise bury its own report in backtraces.
fn probe(path: Path, bytes: &[u8], pref: &[PixelDescriptor]) -> Outcome {
    let prev = std::panic::take_hook();
    let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let sink = std::sync::Arc::clone(&captured);
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut s) = sink.lock() {
            *s = format!("{}", info);
        }
    }));
    let r = std::panic::catch_unwind(AssertUnwindSafe(|| run_cell(path, bytes, pref)));
    std::panic::set_hook(prev);
    match r {
        Ok(Ok(cell)) => Outcome::Ok(cell),
        Ok(Err(e)) => Outcome::Err(e),
        Err(_) => Outcome::Panic(
            captured
                .lock()
                .map(|s| s.clone())
                .unwrap_or_else(|_| "<panic message unavailable>".into()),
        ),
    }
}

// ── The sweep ───────────────────────────────────────────────────────────────

/// **No combination of (`preferred`, input CICP, depth, alpha, entry point)
/// may unwind.** Decode input is untrusted and the CICP that selects a
/// conversion arm is read out of the file, so a panic here is reachable by a
/// hostile (or merely unusual) AVIF plus an ordinary caller preference.
///
/// MEASURED 2026-08-13 on `main` @ 2ba3e766: **72 of 990** cells panicked
/// before the fix — 6 HDR fixtures x 4 preference lists x 3 entry points —
/// every one of them `RowConverter: no conversion path` raised by
/// `PixelBuffer::to_rgb8()` / `to_rgba8()` inside `negotiate_format`. (The
/// issue reported one cell; the arm is far wider than that.) Failures are
/// allowed to be `Err` — this asserts only that they are not unwinds.
///
/// A returned cell is also checked for self-consistency: the descriptor a
/// path reports must actually describe the bytes it produced. A path that
/// "succeeds" while announcing a format it did not write is the #35/#38 shape
/// of bug, and would otherwise slip past a pure no-panic sweep.
#[test]
fn no_preferred_cicp_depth_alpha_combination_panics() {
    let prefs = preferences();
    let mut panics: Vec<String> = Vec::new();
    let mut inconsistent: Vec<String> = Vec::new();
    let mut cells = 0usize;
    let mut errs = 0usize;

    for (fixture, label) in FIXTURES {
        let bytes = read(fixture);
        for (pref_label, pref) in &prefs {
            for &path in PATHS {
                cells += 1;
                match probe(path, &bytes, pref) {
                    Outcome::Ok(cell) => {
                        let expected = cell.dims.0 as usize
                            * cell.dims.1 as usize
                            * cell.desc.bytes_per_pixel();
                        if cell.bytes.len() != expected {
                            inconsistent.push(format!(
                                "  {label:<14} {pref_label:<15} {:<15} reported {:?} at {:?} \
                                 = {expected} bytes but produced {}",
                                path.label(),
                                cell.desc.pixel_format(),
                                cell.dims,
                                cell.bytes.len()
                            ));
                        }
                    }
                    Outcome::Err(_) => errs += 1,
                    Outcome::Panic(msg) => {
                        let first = msg.lines().next().unwrap_or("").to_string();
                        panics.push(format!(
                            "  {label:<14} {pref_label:<15} {:<15} PANIC: {first}",
                            path.label()
                        ));
                    }
                }
            }
        }
    }

    assert!(
        inconsistent.is_empty(),
        "{} of {cells} cells reported a descriptor that does not describe the bytes they \
         wrote:\n{}",
        inconsistent.len(),
        inconsistent.join("\n")
    );

    assert!(
        panics.is_empty(),
        "{} of {cells} decode cells PANICKED ({errs} returned a typed error, which is \
         fine). Untrusted input must never unwind the caller:\n{}",
        panics.len(),
        panics.join("\n")
    );
    assert!(cells > 900, "sweep degenerated to {cells} cells");
}

/// The sweep is only worth its runtime if the fixtures really do span the axes
/// it claims to cover. Pin that: without a PQ fixture the #39 arm is never
/// entered, and the sweep above would pass while testing nothing.
#[test]
fn matrix_axes_are_actually_covered() {
    use zenpixels::TransferFunction as T;
    let mut transfers: Vec<T> = Vec::new();
    let mut depths = std::collections::BTreeSet::new();
    let mut with_alpha = 0usize;
    let mut mono = 0usize;

    for (fixture, label) in FIXTURES {
        let bytes = read(fixture);
        let out = AvifDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(&bytes), &[])
            .and_then(|d| d.decode());
        let Ok(out) = out else {
            panic!("{label}: native decode failed — fixture unusable for the matrix");
        };
        let d = out.pixels().descriptor();
        if !transfers.contains(&d.transfer) {
            transfers.push(d.transfer);
        }
        depths.insert(d.channel_type().byte_size());
        if d.alpha.is_some() {
            with_alpha += 1;
        }
        if d.layout() == zenpixels::ChannelLayout::Gray {
            mono += 1;
        }
    }

    assert!(
        transfers.contains(&T::Pq),
        "no fixture decodes to a PQ descriptor — the #39 conversion arm is unreachable \
         and the panic sweep proves nothing. Got: {transfers:?}"
    );
    assert!(
        transfers.len() >= 3,
        "only {} distinct transfer functions across the fixture set: {transfers:?}",
        transfers.len()
    );
    assert!(
        depths.contains(&1) && depths.contains(&2),
        "fixtures must span 8-bit and >8-bit native output; got byte sizes {depths:?}"
    );
    assert!(with_alpha > 0, "no alpha-carrying fixture in the matrix");
    assert!(mono > 0, "no monochrome-native fixture in the matrix");
}

// ── #39, second surface: the `decode_into_*` convenience methods ────────────
//
// These take no `preferred` list at all — they decode natively and then
// convert to a fixed 8-bit target — so the audit that swept the negotiation
// layer would not have looked at them. They shared the same defect and the
// same root cause: `PixelBuffer::to_rgb8()` / `to_rgba8()` `expect` on
// `RowConverter::new`, and the source descriptor comes from the file's CICP.
// A caller has no way to steer around this one.

/// Run `f`, reporting whether it unwound.
fn caught(f: impl FnOnce() -> bool) -> Result<bool, ()> {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(AssertUnwindSafe(f));
    std::panic::set_hook(prev);
    r.map_err(|_| ())
}

/// None of the five `decode_into_*` entry points may unwind on any committed
/// fixture, whatever its CICP, depth or alpha.
#[test]
fn decode_into_convenience_methods_never_panic_on_any_fixture() {
    let mut panics: Vec<String> = Vec::new();
    let mut cells = 0usize;

    for (fixture, label) in FIXTURES {
        let bytes = read(fixture);
        // The walkers convert the WHOLE decoded buffer before clamping to the
        // destination, so a small destination still exercises the conversion.
        type IntoCase = (&'static str, Box<dyn Fn(&[u8]) -> bool>);
        let cases: Vec<IntoCase> = vec![
            (
                "rgb8",
                Box::new(|b: &[u8]| {
                    let mut d = vec![Rgb::<u8> { r: 0, g: 0, b: 0 }; 64];
                    AvifDecoderConfig::new()
                        .decode_into_rgb8(b, imgref::ImgRefMut::new(&mut d, 8, 8))
                        .is_ok()
                }),
            ),
            (
                "rgba8",
                Box::new(|b: &[u8]| {
                    let mut d = vec![
                        Rgba::<u8> {
                            r: 0,
                            g: 0,
                            b: 0,
                            a: 0
                        };
                        64
                    ];
                    AvifDecoderConfig::new()
                        .decode_into_rgba8(b, imgref::ImgRefMut::new(&mut d, 8, 8))
                        .is_ok()
                }),
            ),
            (
                "rgb_f32",
                Box::new(|b: &[u8]| {
                    let mut d = vec![
                        Rgb::<f32> {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0
                        };
                        64
                    ];
                    AvifDecoderConfig::new()
                        .decode_into_rgb_f32(b, imgref::ImgRefMut::new(&mut d, 8, 8))
                        .is_ok()
                }),
            ),
            (
                "rgba_f32",
                Box::new(|b: &[u8]| {
                    let mut d = vec![
                        Rgba::<f32> {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0
                        };
                        64
                    ];
                    AvifDecoderConfig::new()
                        .decode_into_rgba_f32(b, imgref::ImgRefMut::new(&mut d, 8, 8))
                        .is_ok()
                }),
            ),
            (
                "gray_f32",
                Box::new(|b: &[u8]| {
                    let mut d = vec![rgb::Gray::<f32>(0.0); 64];
                    AvifDecoderConfig::new()
                        .decode_into_gray_f32(b, imgref::ImgRefMut::new(&mut d, 8, 8))
                        .is_ok()
                }),
            ),
        ];

        for (name, run) in cases {
            cells += 1;
            if caught(|| run(&bytes)).is_err() {
                panics.push(format!("  {label:<14} decode_into_{name}"));
            }
        }
    }

    assert_eq!(cells, FIXTURES.len() * 5, "sweep did not run every method");
    assert!(
        panics.is_empty(),
        "{} of {cells} `decode_into_*` calls PANICKED — these take no `preferred` list, so \
         a caller cannot avoid the arm; the conversion must return a typed error \
         (zenavif#39, second surface):\n{}",
        panics.len(),
        panics.join("\n")
    );
}

/// The refusal must be a *typed, categorised* error, not a wrong picture.
/// Without this the test above could be satisfied by silently writing garbage.
#[test]
fn decode_into_refuses_hdr_with_an_unsupported_error() {
    let bytes = read("tests/vectors/libavif/colors_hdr_p3.avif");
    let mut d = vec![Rgb::<u8> { r: 9, g: 9, b: 9 }; 64];
    let err = AvifDecoderConfig::new()
        .decode_into_rgb8(&bytes, imgref::ImgRefMut::new(&mut d, 8, 8))
        .expect_err(
            "an HDR PQ source has no plan to 8-bit sRGB without a tone-mapping decision, so \
             decode_into_rgb8 must refuse rather than invent one",
        );
    assert_eq!(
        err.error().category(),
        zencodec::ErrorCategory::Image(zencodec::ImageError::Unsupported(
            zencodec::UnsupportedImageKind::Feature
        )),
        "the refusal must carry an Unsupported category a caller can branch on, got: {err}"
    );
    assert!(
        d.iter().all(|p| p.r == 9 && p.g == 9 && p.b == 9),
        "the destination buffer must be left untouched when the conversion is refused"
    );

    // An SDR source of the same shape must still succeed — otherwise this
    // test would pass against a `decode_into_rgb8` that refuses everything.
    let sdr = read("tests/vectors/libavif/kodim03_yuv420_8bpc.avif");
    let mut d2 = vec![Rgb::<u8> { r: 9, g: 9, b: 9 }; 64];
    AvifDecoderConfig::new()
        .decode_into_rgb8(&sdr, imgref::ImgRefMut::new(&mut d2, 8, 8))
        .expect("an ordinary 8-bit sRGB AVIF must still decode into RGB8");
    assert!(
        d2.iter().any(|p| p.r != 9 || p.g != 9 || p.b != 9),
        "the SDR arm wrote nothing — the liveness half of this test is dead"
    );
}

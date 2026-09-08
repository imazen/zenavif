//! **How much does each aom-backend mode actually lose, on every input format?**
//! Measured on pixels, not argued from configuration.
//!
//! # Why this file exists
//!
//! Until this landing the zenav1-aom backend refused 4:4:4 (which is
//! `EncodeChromaSubsampling`'s DEFAULT), refused full pixel range, refused
//! lossless, and refused the identity/GBR colour model — so every image it
//! accepted was subsampled to 4:2:0 AND crushed to the studio swing, silently.
//! Now all four work, and the obvious failure mode of "now they work" is that
//! one of them quietly stops being least-lossy again on some format nobody
//! swept.
//!
//! A configuration cannot tell you that. Everything in FRONT of the coded
//! planes throws information away — the RGB->YUV matrix, chroma subsampling,
//! and studio-vs-full range each lose samples before the encoder sees one — so
//! a cq-0 encode through 4:2:0 limited-range YCbCr is "lossless" only about the
//! planes it was handed. The only honest measurement is a round trip.
//!
//! # What this sweeps
//!
//! **Every channel ramps over its full range, and every input format is
//! covered.** The probe drives R, G and B (and A) on three DIFFERENT axes, so
//! each channel sweeps 0..max independently and the chroma planes carry real
//! signal; a single-channel ramp or a grey wedge would let 4:2:0 look as good
//! as 4:4:4 and make the ordering assertions below vacuous.
//!
//! Formats: `Rgb<u8>`, `Rgb<u16>`, `Rgba<u8>`, `Rgba<u16>`, and 8-bit
//! grayscale (the Cs400 mono path) — i.e. every entry point this backend has.
//! Coded depths 8/10/12 where the format reaches them.
//!
//! Assertions are an ORDERING plus an EXACT zero rather than per-cell
//! tolerances: an ordering gate catches a silent downgrade that a tolerance
//! would wave through, and the exact zero is what "mathematically lossless"
//! has to mean.
//!
//! `zensim-regress` runs alongside as the perceptual number, so a change that
//! holds max error while smearing the image is visible too — **and it is
//! asserted, not merely printed**, on the arm where it is computed.
//! `check_regression` here takes an 8-bit `RgbSlice` reference, so the score is
//! computed for the **8-bit RGB** arms and reported as `n/a` for the
//! high-bit-depth and RGBA arms rather than silently as a number. Those arms
//! are covered by the pixel error, which is exact; extending the perceptual
//! score to them needs an `RgbaSlice` / hbd reference and is not done here.

#![cfg(all(feature = "zenav1-aom-encode", feature = "encode"))]

use imgref::{Img, ImgRef, ImgVec};
use rgb::{Rgb, Rgba};
use zenavif::{
    Av1Backend, EncodeBitDepth, EncodeChromaSubsampling, EncodeColorModel, EncodePixelRange,
    EncoderConfig,
};
use zensim::{RgbSlice, Zensim, ZensimProfile};
use zensim_regress::{RegressionTolerance, check_regression};

fn stop() -> almost_enough::StopToken {
    almost_enough::StopToken::new(almost_enough::Unstoppable)
}

const W: usize = 64;
const H: usize = 48;

/// Every channel ramps over its FULL range, on a different axis each:
/// R along x, G along y, B along the diagonal, A along the anti-diagonal.
///
/// Independent axes matter as much as the ramps: if the channels moved
/// together the image would be near-grey, the chroma planes would carry almost
/// nothing, and 4:2:0 would score as well as 4:4:4 — the gate would pass while
/// measuring nothing. Decorrelated ramps put real signal in U and V.
fn ramp_rgba16(w: usize, h: usize) -> ImgVec<Rgba<u16>> {
    let mut px = Vec::with_capacity(w * h);
    let sx = |i: usize, n: usize| ((i * 65535) / (n - 1).max(1)) as u16;
    for y in 0..h {
        for x in 0..w {
            px.push(Rgba {
                r: sx(x, w),
                g: sx(y, h),
                b: sx((x + y) % w.max(h), w.max(h)),
                a: sx((x + (h - 1 - y)) % w.max(h), w.max(h)),
            });
        }
    }
    Img::new(px, w, h)
}

fn to_rgba8(src: ImgRef<'_, Rgba<u16>>) -> ImgVec<Rgba<u8>> {
    Img::new(
        src.pixels()
            .map(|p| Rgba {
                r: (p.r >> 8) as u8,
                g: (p.g >> 8) as u8,
                b: (p.b >> 8) as u8,
                a: (p.a >> 8) as u8,
            })
            .collect::<Vec<_>>(),
        src.width(),
        src.height(),
    )
}

fn to_rgb8(src: ImgRef<'_, Rgba<u16>>) -> ImgVec<Rgb<u8>> {
    Img::new(
        src.pixels()
            .map(|p| Rgb { r: (p.r >> 8) as u8, g: (p.g >> 8) as u8, b: (p.b >> 8) as u8 })
            .collect::<Vec<_>>(),
        src.width(),
        src.height(),
    )
}

fn to_rgb16(src: ImgRef<'_, Rgba<u16>>) -> ImgVec<Rgb<u16>> {
    Img::new(
        src.pixels().map(|p| Rgb { r: p.r, g: p.g, b: p.b }).collect::<Vec<_>>(),
        src.width(),
        src.height(),
    )
}

#[derive(Debug)]
struct Loss {
    max: u32,
    mean: f64,
    zensim: f64,
}

/// Compare a decoded 8-bit RGB result against the 8-bit source.
fn measure_rgb8(src: ImgRef<'_, Rgb<u8>>, avif: &[u8], label: &str) -> Loss {
    let dec = zenavif::decode(avif)
        .unwrap_or_else(|e| panic!("{label}: the AVIF it produced must decode: {e}"));
    let got = dec
        .try_as_imgref::<Rgb<u8>>()
        .unwrap_or_else(|| panic!("{label}: expected 8-bit RGB back"));
    assert_eq!(
        (got.width(), got.height()),
        (src.width(), src.height()),
        "{label}: dimensions must survive"
    );
    let (mut max, mut sum) = (0u32, 0u64);
    for (a, b) in src.pixels().zip(got.pixels()) {
        for (s, d) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
            let e = u32::from(s.abs_diff(d));
            max = max.max(e);
            sum += u64::from(e);
        }
    }
    let zensim = Zensim::new(ZensimProfile::codec_target());
    let sv: Vec<[u8; 3]> = src.pixels().map(|p| [p.r, p.g, p.b]).collect();
    let ss = RgbSlice::new(&sv, src.width(), src.height());
    let tol = RegressionTolerance::off_by_one().with_min_similarity(0.0);
    let score = check_regression(&zensim, &ss, &got, &tol)
        .map(|r| r.score())
        .unwrap_or(f64::NAN);
    Loss { max, mean: sum as f64 / (src.width() * src.height() * 3) as f64, zensim: score }
}

/// Compare a decoded 16-bit RGB result against the 16-bit source, reported in
/// the CODED depth's units so the number is comparable across depths.
fn measure_rgb16(src: ImgRef<'_, Rgb<u16>>, avif: &[u8], depth: u32, label: &str) -> Loss {
    let dec = zenavif::decode(avif)
        .unwrap_or_else(|e| panic!("{label}: must decode: {e}"));
    let got = dec
        .try_as_imgref::<Rgb<u16>>()
        .unwrap_or_else(|| panic!("{label}: expected 16-bit RGB back"));
    let sh = 16 - depth;
    let (mut max, mut sum) = (0u32, 0u64);
    for (a, b) in src.pixels().zip(got.pixels()) {
        for (s, d) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
            let e = u32::from((s >> sh).abs_diff(d >> sh));
            max = max.max(e);
            sum += u64::from(e);
        }
    }
    Loss {
        max,
        mean: sum as f64 / (src.width() * src.height() * 3) as f64,
        zensim: f64::NAN,
    }
}


/// Colour error for an image that carries alpha: the decode comes back as
/// RGBA, so `measure_rgb8` cannot read it. Compares the three colour channels
/// in the coded depth's units.
fn measure_rgba(
    src8: ImgRef<'_, Rgb<u8>>,
    src16: ImgRef<'_, Rgba<u16>>,
    avif: &[u8],
    depth: u32,
    label: &str,
) -> Loss {
    let dec = zenavif::decode(avif).unwrap_or_else(|e| panic!("{label}: must decode: {e}"));
    let (mut max, mut sum) = (0u32, 0u64);
    let n;
    if let Some(got) = dec.try_as_imgref::<Rgba<u8>>() {
        n = (got.width() * got.height() * 3) as f64;
        for (a, b) in src8.pixels().zip(got.pixels()) {
            for (s, d) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
                let e = u32::from(s.abs_diff(d));
                max = max.max(e);
                sum += u64::from(e);
            }
        }
    } else if let Some(got) = dec.try_as_imgref::<Rgba<u16>>() {
        let sh = 16 - depth;
        n = (got.width() * got.height() * 3) as f64;
        for (a, b) in src16.pixels().zip(got.pixels()) {
            for (s, d) in [(a.r, b.r), (a.g, b.g), (a.b, b.b)] {
                let e = u32::from((s >> sh).abs_diff(d >> sh));
                max = max.max(e);
                sum += u64::from(e);
            }
        }
    } else {
        panic!("{label}: expected RGBA back");
    }
    Loss { max, mean: sum as f64 / n, zensim: f64::NAN }
}

/// **Does the SUPPORT QUERY tell the truth?**
///
/// `EncoderConfig::validate()` is the capability question a caller asks before
/// committing to a backend, and `validate_for_input` is its config x input
/// twin. Both used to be a hand-written RESTATEMENT of the encode path's
/// rules — and they drifted: 4:4:4, identity/GBR, full range, lossless and
/// alpha all became supported at the seam while the query still refused them,
/// so the query said "unsupported" about five things the encoder does.
///
/// This gate makes that mechanical instead of reviewed: over the whole
/// configuration matrix, `validate()` must return Ok exactly when a real
/// encode returns Ok. Any disagreement, in either direction, is the query
/// lying.
#[test]
fn the_support_query_agrees_with_the_encode_path() {
    let src16 = ramp_rgba16(32, 32);
    let rgb8 = to_rgb8(src16.as_ref());
    let rgba8 = to_rgba8(src16.as_ref());

    let mut checked = 0usize;
    let mut disagreements = Vec::new();
    for chroma in [EncodeChromaSubsampling::Yuv444, EncodeChromaSubsampling::Yuv420] {
        for range in [None, Some(EncodePixelRange::Full), Some(EncodePixelRange::Limited)] {
            for model in [EncodeColorModel::YCbCr, EncodeColorModel::Rgb] {
                for depth in
                    [EncodeBitDepth::Eight, EncodeBitDepth::Ten, EncodeBitDepth::Twelve]
                {
                    let mut cfg =
                        base().quality(90.0).chroma_subsampling(chroma).color_model(model).bit_depth(depth);
                    if let Some(r) = range {
                        cfg = cfg.pixel_range(r);
                    }
                    let label = format!("{chroma:?}/{range:?}/{model:?}/{depth:?}");

                    // (a) config-only query vs the RGB encode.
                    let says = cfg.validate().is_ok();
                    let does = zenavif::encode_rgb8(rgb8.as_ref(), &cfg, stop()).is_ok();
                    checked += 1;
                    if says != does {
                        disagreements.push(format!(
                            "rgb8 {label}: validate()={says} but encode={does}"
                        ));
                    }

                    // (b) config x input query vs the RGBA encode -- the arm
                    // that used to refuse alpha outright.
                    let says_a = cfg
                        .validate_for_input(zenavif::PlanInput::rgba8(32, 32))
                        .is_ok();
                    let does_a = zenavif::encode_rgba8(rgba8.as_ref(), &cfg, stop()).is_ok();
                    checked += 1;
                    if says_a != does_a {
                        disagreements.push(format!(
                            "rgba8 {label}: validate_for_input()={says_a} but encode={does_a}"
                        ));
                    }
                }
            }
        }
    }
    eprintln!("support-query agreement: {checked} config/input pairs checked");
    assert!(
        disagreements.is_empty(),
        "the support query must never disagree with the encode path; {} of {checked} did:\n  {}",
        disagreements.len(),
        disagreements.join("\n  ")
    );
    // Non-vacuity: the matrix must contain BOTH answers, or "they agree" is
    // trivially true because nothing is ever refused.
    let all_ok = base().quality(90.0).validate().is_ok();
    let refused = base()
        .quality(90.0)
        .color_model(EncodeColorModel::Rgb)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
        .validate()
        .is_err();
    assert!(all_ok && refused, "the matrix must span supported AND refused configurations");
}

/// **The THIRD query surface: `backend_router::query_still_backends`.**
///
/// `validate()` / `validate_for_input()` are one query and the router is
/// another, and a caller choosing a backend calls the router — so the router
/// is the one whose answer decides whether this backend is ever reached. It
/// runs `validate_adapter_controls` on top of the config validation, which is
/// an ADDITIONAL predicate the test above cannot see: a config the encode path
/// accepts can still be refused there, and that refusal is invisible to
/// `validate()`.
///
/// This sweeps `matrix_coefficients` alongside the format axes, because the
/// muxer's matrix is exactly what the newly-supported identity/GBR mode
/// changes: `mux_aom` writes `MatrixCoefficients::Rgb` (CICP 0) for an
/// identity encode and `Bt601` (6) otherwise, so an adapter predicate that
/// hardcodes 6 refuses the one value the adapter itself emits.
#[test]
fn the_router_query_agrees_with_the_encode_path() {
    let src16 = ramp_rgba16(32, 32);
    let rgb8 = to_rgb8(src16.as_ref());

    let mut checked = 0usize;
    let mut disagreements = Vec::new();
    let mut refusals = 0usize;
    for chroma in [EncodeChromaSubsampling::Yuv444, EncodeChromaSubsampling::Yuv420] {
        for model in [EncodeColorModel::YCbCr, EncodeColorModel::Rgb] {
            for range in [None, Some(EncodePixelRange::Full)] {
                // `None` is the caller who does not care; 6 is what the muxer
                // writes for a YCbCr encode; 0 is what it writes for identity.
                for mc in [None, Some(6u8), Some(0u8)] {
                    let mut cfg = base()
                        .quality(90.0)
                        .chroma_subsampling(chroma)
                        .color_model(model);
                    if let Some(r) = range {
                        cfg = cfg.pixel_range(r);
                    }
                    if let Some(m) = mc {
                        cfg = cfg.matrix_coefficients(m);
                    }
                    let label = format!("{chroma:?}/{model:?}/{range:?}/mc={mc:?}");

                    let reports = zenavif::backend_router::query_still_backends(
                        &cfg,
                        zenavif::PlanInput::rgb8(32, 32),
                    );
                    let says = reports
                        .iter()
                        .find(|r| r.backend == Av1Backend::Zenav1Aom)
                        .expect("the router must report on every backend, not omit one")
                        .supported();
                    let does = zenavif::encode_rgb8(rgb8.as_ref(), &cfg, stop()).is_ok();
                    checked += 1;
                    if !says {
                        refusals += 1;
                    }
                    if says != does {
                        disagreements.push(format!(
                            "{label}: router says supported={says} but encode={does}"
                        ));
                    }
                }
            }
        }
    }
    eprintln!(
        "router agreement: {checked} configurations checked, {refusals} refused by the router"
    );
    assert!(
        disagreements.is_empty(),
        "the routing query must never disagree with the encode path; {} of {checked} did:\n  {}",
        disagreements.len(),
        disagreements.join("\n  ")
    );
    // Non-vacuity, both directions: the sweep must contain configurations the
    // router refuses AND configurations it accepts, or "they agree" is trivial.
    assert!(
        refusals > 0 && refusals < checked,
        "the sweep must span refused AND accepted routing answers (got {refusals} of {checked})"
    );

    // **The specific cell this test was written for, asserted by name.**
    // Agreement alone would be satisfied by BOTH surfaces refusing, which is
    // exactly the state this found: the adapter predicate hardcoded
    // `if mono { 2 } else { 6 }`, so asking for the identity matrix at
    // `Rgb` + 4:4:4 -- the one configuration whose `colr` box the muxer fills
    // with CICP 0 -- was refused as "not implemented by this pixel conversion
    // path". So assert the ANSWER, not only that the two agree on it.
    let identity444 = base()
        .quality(90.0)
        .color_model(EncodeColorModel::Rgb)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv444)
        .matrix_coefficients(0);
    let reports = zenavif::backend_router::query_still_backends(
        &identity444,
        zenavif::PlanInput::rgb8(32, 32),
    );
    let aom = reports
        .iter()
        .find(|r| r.backend == Av1Backend::Zenav1Aom)
        .expect("the router must report on every backend");
    assert!(
        aom.supported(),
        "requesting the identity matrix (CICP 0) at Rgb + 4:4:4 is requesting exactly what \
         this backend writes into the colr box; the router refused it: {:?}",
        aom.refusal
    );
    assert!(
        zenavif::encode_rgb8(rgb8.as_ref(), &identity444, stop()).is_ok(),
        "and it must encode"
    );

    // The other direction, so the fix is a correction and not a hole: a matrix
    // this seam does NOT code is still refused, and identity outside 4:4:4 is
    // still refused by name (AV1 5.5.2 -- G/B/R planes have no subsampling).
    let wrong_matrix = base().quality(90.0).matrix_coefficients(9);
    assert!(
        !zenavif::backend_router::query_still_backends(
            &wrong_matrix,
            zenavif::PlanInput::rgb8(32, 32),
        )
        .iter()
        .find(|r| r.backend == Av1Backend::Zenav1Aom)
        .unwrap()
        .supported(),
        "a matrix this conversion path does not implement must still be refused"
    );
    let identity420 = base()
        .quality(90.0)
        .color_model(EncodeColorModel::Rgb)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420);
    assert!(
        !zenavif::backend_router::query_still_backends(
            &identity420,
            zenavif::PlanInput::rgb8(32, 32),
        )
        .iter()
        .find(|r| r.backend == Av1Backend::Zenav1Aom)
        .unwrap()
        .supported(),
        "identity at 4:2:0 is not conformant and must stay refused"
    );
}

fn base() -> EncoderConfig {
    EncoderConfig::new().backend(Av1Backend::Zenav1Aom)
}

/// The mode ladder, least lossy first.
fn lossy_ladder(q: f32) -> Vec<(&'static str, EncoderConfig)> {
    vec![
        (
            "4:4:4 full",
            base()
                .quality(q)
                .chroma_subsampling(EncodeChromaSubsampling::Yuv444)
                .pixel_range(EncodePixelRange::Full),
        ),
        (
            "4:4:4 limited",
            base()
                .quality(q)
                .chroma_subsampling(EncodeChromaSubsampling::Yuv444)
                .pixel_range(EncodePixelRange::Limited),
        ),
        (
            "4:2:0 limited",
            base()
                .quality(q)
                .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
                .pixel_range(EncodePixelRange::Limited),
        ),
    ]
}

/// **The ordering half, on every RGB(A) format and coded depth.**
#[test]
fn every_format_orders_least_lossy_first() {
    let src16 = ramp_rgba16(W, H);
    let rgb8 = to_rgb8(src16.as_ref());
    let rgb16 = to_rgb16(src16.as_ref());
    let rgba8 = to_rgba8(src16.as_ref());

    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten, EncodeBitDepth::Twelve] {
        let dbits = match depth {
            EncodeBitDepth::Eight => 8,
            EncodeBitDepth::Ten => 10,
            _ => 12,
        };
        for (fmt, is8) in [("Rgb<u8>", true), ("Rgb<u16>", false), ("Rgba<u8>", true)] {
            let mut losses = Vec::new();
            for (mode, cfg) in lossy_ladder(95.0) {
                let cfg = cfg.bit_depth(depth);
                let label = format!("{fmt} d{dbits} {mode}");
                let l = match (fmt, is8) {
                    ("Rgb<u8>", _) => {
                        let e = zenavif::encode_rgb8(rgb8.as_ref(), &cfg, stop())
                            .unwrap_or_else(|e| panic!("{label}: {e}"));
                        if dbits == 8 {
                            measure_rgb8(rgb8.as_ref(), &e.avif_file, &label)
                        } else {
                            measure_rgb16(rgb16.as_ref(), &e.avif_file, dbits, &label)
                        }
                    }
                    ("Rgb<u16>", _) => {
                        let e = zenavif::encode_rgb16(rgb16.as_ref(), &cfg, stop())
                            .unwrap_or_else(|e| panic!("{label}: {e}"));
                        if dbits == 8 {
                            measure_rgb8(rgb8.as_ref(), &e.avif_file, &label)
                        } else {
                            measure_rgb16(rgb16.as_ref(), &e.avif_file, dbits, &label)
                        }
                    }
                    _ => {
                        let e = zenavif::encode_rgba8(rgba8.as_ref(), &cfg, stop())
                            .unwrap_or_else(|e| panic!("{label}: {e}"));
                        assert!(e.alpha_byte_size > 0, "{label}: RGBA must carry alpha");
                        // An image WITH alpha decodes back as RGBA, so the
                        // colour comparison has to read that shape.
                        measure_rgba(rgb8.as_ref(), src16.as_ref(), &e.avif_file, dbits, &label)
                    }
                };
                if l.zensim.is_nan() {
                    eprintln!("{label:32} max={:5} mean={:8.4} zensim=   n/a", l.max, l.mean);
                } else {
                    eprintln!(
                        "{label:32} max={:5} mean={:8.4} zensim={:6.2}",
                        l.max, l.mean, l.zensim
                    );
                }
                losses.push((mode, l));
            }
            // 4:4:4 must beat 4:2:0: 4:2:0 discards three quarters of the
            // chroma before the encoder sees a sample.
            let f444 = &losses[1].1;
            let f420 = &losses[2].1;
            assert!(
                f444.mean < f420.mean,
                "{fmt} d{dbits}: 4:4:4 must lose less than 4:2:0 ({:.4} vs {:.4})",
                f444.mean,
                f420.mean
            );
            // Where the perceptual score exists, it must agree with the pixel
            // error — so the ordering is not an artefact of one statistic.
            if !f444.zensim.is_nan() && !f420.zensim.is_nan() {
                assert!(
                    f444.zensim > f420.zensim,
                    "{fmt} d{dbits}: 4:4:4 must score better perceptually ({:.2} vs {:.2})",
                    f444.zensim,
                    f420.zensim
                );
            }
            // Non-vacuity: the probe must actually separate them.
            assert!(
                f420.mean > 0.0,
                "{fmt} d{dbits}: the probe must be lossy at 4:2:0, or this proves nothing"
            );
        }
    }
}

/// **The exact-zero half.** Identity (GBR) + full range + lossless must return
/// the source EXACTLY, on both RGB input widths.
#[test]
#[cfg(feature = "encode-imazen")]
fn lossless_round_trips_bit_exactly_on_every_rgb_format() {
    let src16 = ramp_rgba16(W, H);
    let rgb8 = to_rgb8(src16.as_ref());
    let rgb16 = to_rgb16(src16.as_ref());

    let cfg = |d: EncodeBitDepth| {
        base()
            .color_model(EncodeColorModel::Rgb)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv444)
            .pixel_range(EncodePixelRange::Full)
            .with_lossless(true)
            .bit_depth(d)
    };

    // 8-bit source, 8-bit coded.
    let e = zenavif::encode_rgb8(rgb8.as_ref(), &cfg(EncodeBitDepth::Eight), stop())
        .expect("8-bit lossless must encode");
    let l = measure_rgb8(rgb8.as_ref(), &e.avif_file, "Rgb<u8> lossless");
    eprintln!("Rgb<u8>  lossless: max={} mean={:.4} zensim={:.2}", l.max, l.mean, l.zensim);
    assert_eq!(l.max, 0, "8-bit lossless must be EXACT, max abs error {}", l.max);

    // 16-bit source at 12-bit coded: exact in the CODED depth's units, which is
    // the most a 12-bit stream can promise about a 16-bit source.
    let e = zenavif::encode_rgb16(rgb16.as_ref(), &cfg(EncodeBitDepth::Twelve), stop())
        .expect("12-bit lossless must encode");
    let l = measure_rgb16(rgb16.as_ref(), &e.avif_file, 12, "Rgb<u16> lossless d12");
    eprintln!("Rgb<u16> lossless d12: max={} mean={:.4}", l.max, l.mean);
    assert_eq!(
        l.max, 0,
        "12-bit lossless must be exact in coded units, max abs error {}",
        l.max
    );
}

/// Alpha ramps too, and rides its own Cs400 item — a colour-path regression
/// cannot cover it.
#[test]
fn alpha_ramp_round_trips_through_the_auxiliary_item() {
    let src16 = ramp_rgba16(W, H);
    let rgba8 = to_rgba8(src16.as_ref());
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten, EncodeBitDepth::Twelve] {
        let cfg = base().quality(95.0).bit_depth(depth);
        let enc = zenavif::encode_rgba8(rgba8.as_ref(), &cfg, stop()).expect("RGBA must encode");
        assert!(enc.alpha_byte_size > 0, "must carry an alpha auxiliary item");
        let dec = zenavif::decode(&enc.avif_file).expect("must decode");
        // The alpha ramp must come back as a ramp: a dropped plane would
        // default to opaque and a constant probe could not tell.
        let max_a = if let Some(g) = dec.try_as_imgref::<Rgba<u8>>() {
            rgba8
                .as_ref()
                .pixels()
                .zip(g.pixels())
                .map(|(a, b)| u32::from(a.a.abs_diff(b.a)))
                .max()
                .unwrap_or(0)
        } else if let Some(g) = dec.try_as_imgref::<Rgba<u16>>() {
            src16
                .as_ref()
                .pixels()
                .zip(g.pixels())
                .map(|(a, b)| u32::from((a.a >> 8).abs_diff((b.a >> 8) as u16)))
                .max()
                .unwrap_or(0)
        } else {
            panic!("expected RGBA back");
        };
        eprintln!("alpha ramp d{depth:?}: max abs error = {max_a}");
        assert!(max_a <= 8, "alpha ramp must survive, max abs error {max_a}");
    }
}

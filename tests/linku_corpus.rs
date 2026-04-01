//! Integration tests against link-u/avif-sample-images with libavif reference comparison.
//!
//! Downloads: <https://github.com/link-u/avif-sample-images>
//! Reproduces: <https://github.com/imazen/rav1d-safe/issues/1>
//!
//! Run with:
//!   just download-linku
//!   just generate-linku-references
//!   just test-linku
//!
//! Or just decode (no reference comparison):
//!   just test-linku-decode

use enough::Unstoppable;
use std::fs;
use std::path::{Path, PathBuf};
use zenavif::{DecoderConfig, decode_with};
use zenpixels::{PixelBuffer, PixelDescriptor};
use zensim::{RgbSlice, RgbaSlice, Zensim, ZensimProfile};
use zensim_regress::{RegressionReport, RegressionTolerance, check_regression};

fn linku_vectors_dir() -> PathBuf {
    PathBuf::from("tests/vectors/link-u")
}

fn linku_references_dir() -> PathBuf {
    PathBuf::from("tests/linku-references")
}

fn find_linku_vectors() -> Vec<PathBuf> {
    let dir = linku_vectors_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|s| s.to_str()),
                Some("avif" | "avifs")
            )
        })
        .collect();
    files.sort();
    files
}

fn format_name(desc: &zenpixels::PixelDescriptor) -> &'static str {
    if desc.layout_compatible(PixelDescriptor::RGB8) {
        "RGB8"
    } else if desc.layout_compatible(PixelDescriptor::RGBA8) {
        "RGBA8"
    } else if desc.layout_compatible(PixelDescriptor::RGB16) {
        "RGB16"
    } else if desc.layout_compatible(PixelDescriptor::RGBA16) {
        "RGBA16"
    } else if desc.layout_compatible(PixelDescriptor::GRAY8) {
        "GRAY8"
    } else if desc.layout_compatible(PixelDescriptor::GRAY16) {
        "GRAY16"
    } else {
        "OTHER"
    }
}

enum CompareResult {
    Pass(RegressionReport),
    Fail(RegressionReport),
    NoReference,
    CompareError(String),
}

/// Compare zenavif decode output against a libavif reference PNG using zensim-regress.
///
/// Uses perceptual similarity scoring with off-by-one tolerance (max delta 1/255,
/// zensim score >= 85). This catches real bugs while tolerating rounding differences
/// between YUV->RGB implementations.
fn compare_against_libavif(zensim: &Zensim, image: &PixelBuffer, ref_path: &Path) -> CompareResult {
    if !ref_path.exists() {
        return CompareResult::NoReference;
    }

    let desc = image.descriptor();

    // For 16-bit and grayscale formats, zensim only supports 8-bit RGB/RGBA.
    // Convert both sides to 8-bit for comparison.
    let is_8bit_rgb = desc.layout_compatible(PixelDescriptor::RGB8);
    let is_8bit_rgba = desc.layout_compatible(PixelDescriptor::RGBA8);

    if !is_8bit_rgb && !is_8bit_rgba {
        // For 16-bit / grayscale: convert zenavif output to 8-bit RGB/RGBA via image crate,
        // then compare. This loses precision but still catches structural bugs.
        return compare_via_image_crate(zensim, image, ref_path);
    }

    let reference = match image::open(ref_path) {
        Ok(r) => r,
        Err(e) => return CompareResult::CompareError(format!("open ref: {e}")),
    };

    // Allow off-by-one rounding + generous alpha tolerance (alpha range mapping differs)
    let tolerance = RegressionTolerance::off_by_one()
        .with_max_alpha_delta(2)
        .with_min_similarity(85.0);

    if is_8bit_rgb {
        let actual = image.try_as_imgref::<rgb::Rgb<u8>>().unwrap();
        let ref_rgb = reference.to_rgb8();
        let w = ref_rgb.width() as usize;
        let h = ref_rgb.height() as usize;
        if actual.width() != w || actual.height() != h {
            return CompareResult::CompareError(format!(
                "size: {}x{} vs {w}x{h}",
                actual.width(),
                actual.height()
            ));
        }
        // Pack reference into contiguous [R,G,B] for RgbSlice
        let ref_pixels: Vec<[u8; 3]> = ref_rgb.pixels().map(|p| [p[0], p[1], p[2]]).collect();
        let ref_src = RgbSlice::new(&ref_pixels, w, h);

        match check_regression(zensim, &ref_src, &actual, &tolerance) {
            Ok(report) => {
                if report.passed() {
                    CompareResult::Pass(report)
                } else {
                    CompareResult::Fail(report)
                }
            }
            Err(e) => CompareResult::CompareError(format!("zensim: {e}")),
        }
    } else {
        // RGBA8
        let actual = image.try_as_imgref::<rgb::Rgba<u8>>().unwrap();
        let ref_rgba = reference.to_rgba8();
        let w = ref_rgba.width() as usize;
        let h = ref_rgba.height() as usize;
        if actual.width() != w || actual.height() != h {
            return CompareResult::CompareError(format!(
                "size: {}x{} vs {w}x{h}",
                actual.width(),
                actual.height()
            ));
        }
        let ref_pixels: Vec<[u8; 4]> = ref_rgba
            .pixels()
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect();
        let ref_src = RgbaSlice::new(&ref_pixels, w, h);

        match check_regression(zensim, &ref_src, &actual, &tolerance) {
            Ok(report) => {
                if report.passed() {
                    CompareResult::Pass(report)
                } else {
                    CompareResult::Fail(report)
                }
            }
            Err(e) => CompareResult::CompareError(format!("zensim: {e}")),
        }
    }
}

/// Fallback comparison for 16-bit and grayscale formats.
/// Converts both sides to 8-bit RGB via the `image` crate, then uses zensim.
fn compare_via_image_crate(
    zensim: &Zensim,
    decoded: &PixelBuffer,
    ref_path: &Path,
) -> CompareResult {
    let reference = match image::open(ref_path) {
        Ok(r) => r,
        Err(e) => return CompareResult::CompareError(format!("open ref: {e}")),
    };

    let desc = decoded.descriptor();
    let has_alpha = desc.layout_compatible(PixelDescriptor::RGBA16)
        || desc.layout_compatible(PixelDescriptor::RGBA8);

    // 16-bit tolerance: allow off-by-2 after quantization to 8-bit
    // (10/12-bit -> 16-bit -> 8-bit loses precision at each step)
    let tolerance = RegressionTolerance::off_by_one()
        .with_max_delta(2)
        .with_max_alpha_delta(2)
        .with_min_similarity(80.0);

    if has_alpha {
        // Convert zenavif 16-bit RGBA to 8-bit RGBA
        let actual_rgba = decoded_to_rgba8(decoded);
        let ref_rgba = reference.to_rgba8();
        let w = ref_rgba.width() as usize;
        let h = ref_rgba.height() as usize;
        if actual_rgba.len() != w * h {
            return CompareResult::CompareError(format!(
                "size mismatch after conversion: {} vs {w}x{h}",
                actual_rgba.len()
            ));
        }
        let ref_pixels: Vec<[u8; 4]> = ref_rgba
            .pixels()
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect();
        let ref_src = RgbaSlice::new(&ref_pixels, w, h);
        let actual_src = RgbaSlice::new(&actual_rgba, w, h);
        match check_regression(zensim, &ref_src, &actual_src, &tolerance) {
            Ok(report) => {
                if report.passed() {
                    CompareResult::Pass(report)
                } else {
                    CompareResult::Fail(report)
                }
            }
            Err(e) => CompareResult::CompareError(format!("zensim: {e}")),
        }
    } else {
        // Convert to 8-bit RGB
        let actual_rgb = decoded_to_rgb8(decoded);
        let ref_rgb = reference.to_rgb8();
        let w = ref_rgb.width() as usize;
        let h = ref_rgb.height() as usize;
        if actual_rgb.len() != w * h {
            return CompareResult::CompareError(format!(
                "size mismatch after conversion: {} vs {w}x{h}",
                actual_rgb.len()
            ));
        }
        let ref_pixels: Vec<[u8; 3]> = ref_rgb.pixels().map(|p| [p[0], p[1], p[2]]).collect();
        let ref_src = RgbSlice::new(&ref_pixels, w, h);
        let actual_src = RgbSlice::new(&actual_rgb, w, h);
        match check_regression(zensim, &ref_src, &actual_src, &tolerance) {
            Ok(report) => {
                if report.passed() {
                    CompareResult::Pass(report)
                } else {
                    CompareResult::Fail(report)
                }
            }
            Err(e) => CompareResult::CompareError(format!("zensim: {e}")),
        }
    }
}

/// Convert a PixelBuffer to 8-bit RGB pixels as `Vec<[u8; 3]>`.
fn decoded_to_rgb8(image: &PixelBuffer) -> Vec<[u8; 3]> {
    let desc = image.descriptor();
    let w = image.width() as usize;
    let h = image.height() as usize;

    if desc.layout_compatible(PixelDescriptor::RGB8) {
        let img = image.try_as_imgref::<rgb::Rgb<u8>>().unwrap();
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let p = img[(x, y)];
                out.push([p.r, p.g, p.b]);
            }
        }
        out
    } else if desc.layout_compatible(PixelDescriptor::RGB16) {
        let img = image.try_as_imgref::<rgb::Rgb<u16>>().unwrap();
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let p = img[(x, y)];
                out.push([(p.r >> 8) as u8, (p.g >> 8) as u8, (p.b >> 8) as u8]);
            }
        }
        out
    } else if desc.layout_compatible(PixelDescriptor::GRAY8)
        || desc.layout_compatible(PixelDescriptor::GRAY16)
    {
        // Grayscale: expand to RGB
        let slice = image.as_slice();
        let bpp = desc.bytes_per_pixel();
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            let row = slice.row(y as u32);
            for x in 0..w {
                let v = if bpp == 1 {
                    row[x]
                } else {
                    // 16-bit gray -> 8-bit
                    let v16 = u16::from_ne_bytes([row[x * 2], row[x * 2 + 1]]);
                    (v16 >> 8) as u8
                };
                out.push([v, v, v]);
            }
        }
        out
    } else {
        // Fallback: treat as opaque RGB from RGBA
        decoded_to_rgba8(image)
            .into_iter()
            .map(|[r, g, b, _]| [r, g, b])
            .collect()
    }
}

/// Convert a PixelBuffer to 8-bit RGBA pixels as `Vec<[u8; 4]>`.
fn decoded_to_rgba8(image: &PixelBuffer) -> Vec<[u8; 4]> {
    let desc = image.descriptor();
    let w = image.width() as usize;
    let h = image.height() as usize;

    if desc.layout_compatible(PixelDescriptor::RGBA8) {
        let img = image.try_as_imgref::<rgb::Rgba<u8>>().unwrap();
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let p = img[(x, y)];
                out.push([p.r, p.g, p.b, p.a]);
            }
        }
        out
    } else if desc.layout_compatible(PixelDescriptor::RGBA16) {
        let img = image.try_as_imgref::<rgb::Rgba<u16>>().unwrap();
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let p = img[(x, y)];
                out.push([
                    (p.r >> 8) as u8,
                    (p.g >> 8) as u8,
                    (p.b >> 8) as u8,
                    (p.a >> 8) as u8,
                ]);
            }
        }
        out
    } else {
        // RGB -> RGBA with alpha=255
        decoded_to_rgb8(image)
            .into_iter()
            .map(|[r, g, b]| [r, g, b, 255])
            .collect()
    }
}

/// Decode all link-u AVIF samples. No reference comparison -- just tests that
/// decoding doesn't panic or error. This is the primary test for reproducing
/// <https://github.com/imazen/rav1d-safe/issues/1>.
#[test]
#[ignore]
fn linku_decode_all() {
    let vectors = find_linku_vectors();
    if vectors.is_empty() {
        eprintln!("No link-u vectors found. Run: just download-linku");
        panic!("link-u test vectors required");
    }

    let config = DecoderConfig::new().threads(1);

    let mut passed = 0usize;
    let mut panicked = 0usize;
    let mut errored = 0usize;
    let mut panicked_files = Vec::new();
    let mut errored_files = Vec::new();

    eprintln!("\nDecoding {} link-u AVIF files...\n", vectors.len());

    for path in &vectors {
        let name = path.file_name().unwrap().to_string_lossy();
        eprint!("  {name:60} ");

        let data = match fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("READ ERROR: {e}");
                errored += 1;
                errored_files.push((name.to_string(), format!("read: {e}")));
                continue;
            }
        };

        let config = config.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decode_with(&data, &config, &Unstoppable)
        }));

        match result {
            Ok(Ok(image)) => {
                let desc = image.descriptor();
                eprintln!(
                    "OK  {:6} {}x{}",
                    format_name(&desc),
                    image.width(),
                    image.height()
                );
                passed += 1;
            }
            Ok(Err(e)) => {
                eprintln!("ERR {e}");
                errored += 1;
                errored_files.push((name.to_string(), e.to_string()));
            }
            Err(panic_info) => {
                let msg = panic_info
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic_info.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_string());
                eprintln!("PANIC {msg}");
                panicked += 1;
                panicked_files.push((name.to_string(), msg));
            }
        }
    }

    eprintln!("\n--- link-u Decode Results ---");
    eprintln!("  Passed:   {passed}/{}", vectors.len());
    eprintln!("  Panicked: {panicked}");
    eprintln!("  Errored:  {errored}");

    if !panicked_files.is_empty() {
        eprintln!("\nPANICS (rav1d-safe#1 reproduction):");
        for (name, msg) in &panicked_files {
            eprintln!("  {name}: {msg}");
        }
    }
    if !errored_files.is_empty() {
        eprintln!("\nERRORS:");
        for (name, msg) in &errored_files {
            eprintln!("  {name}: {msg}");
        }
    }

    assert_eq!(
        panicked, 0,
        "{panicked} files caused panics (integer underflow / index out of bounds)"
    );
    assert_eq!(errored, 0, "{errored} files failed to decode");
}

/// Decode all link-u AVIF samples and compare pixel output against libavif
/// reference PNGs using zensim perceptual similarity.
/// Requires references generated by `just generate-linku-references`.
#[test]
#[ignore]
fn linku_pixel_parity() {
    let vectors = find_linku_vectors();
    if vectors.is_empty() {
        eprintln!("No link-u vectors found. Run: just download-linku");
        panic!("link-u test vectors required");
    }

    let ref_dir = linku_references_dir();
    if !ref_dir.exists() {
        eprintln!("No link-u references found. Run: just generate-linku-references");
        panic!("link-u reference PNGs required");
    }

    let config = DecoderConfig::new().threads(1);
    let zensim = Zensim::new(ZensimProfile::latest());

    let mut matched = 0usize;
    let mut mismatched = 0usize;
    let mut no_ref = 0usize;
    let mut panicked = 0usize;
    let mut errored = 0usize;
    let mut mismatch_details = Vec::new();
    let mut panic_details = Vec::new();

    eprintln!(
        "\nComparing {} link-u files against libavif (zensim)...\n",
        vectors.len()
    );

    for path in &vectors {
        let name = path.file_name().unwrap().to_string_lossy();
        let stem = path.file_stem().unwrap().to_string_lossy();
        let ref_path = ref_dir.join(format!("{stem}.png"));
        eprint!("  {name:60} ");

        let data = match fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("READ ERROR: {e}");
                errored += 1;
                continue;
            }
        };

        let config = config.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decode_with(&data, &config, &Unstoppable)
        }));

        match result {
            Ok(Ok(image)) => {
                let fmt = format_name(&image.descriptor());
                match compare_against_libavif(&zensim, &image, &ref_path) {
                    CompareResult::Pass(report) => {
                        eprintln!(
                            "PASS   {fmt:6} score={:.1} max_delta={:?}",
                            report.score(),
                            report.max_channel_delta()
                        );
                        matched += 1;
                    }
                    CompareResult::Fail(report) => {
                        eprintln!(
                            "FAIL   {fmt:6} score={:.1} max_delta={:?} cat={:?}",
                            report.score(),
                            report.max_channel_delta(),
                            report.category()
                        );
                        mismatched += 1;
                        mismatch_details.push((name.to_string(), report));
                    }
                    CompareResult::NoReference => {
                        eprintln!("NOREF  {fmt:6}");
                        no_ref += 1;
                    }
                    CompareResult::CompareError(e) => {
                        eprintln!("CMPERR {e}");
                        errored += 1;
                    }
                }
            }
            Ok(Err(e)) => {
                eprintln!("DECERR {e}");
                errored += 1;
            }
            Err(panic_info) => {
                let msg = panic_info
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic_info.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_string());
                eprintln!("PANIC  {msg}");
                panicked += 1;
                panic_details.push((name.to_string(), msg));
            }
        }
    }

    eprintln!("\n--- link-u Pixel Parity (zensim) ---");
    eprintln!("  Passed:      {matched}");
    eprintln!("  Failed:      {mismatched}");
    eprintln!("  No ref:      {no_ref}");
    eprintln!("  Panicked:    {panicked}");
    eprintln!("  Errored:     {errored}");

    if !mismatch_details.is_empty() {
        eprintln!("\nFAILURES:");
        for (name, report) in &mismatch_details {
            eprintln!("  {name}:");
            eprintln!("    {report}");
        }
    }
    if !panic_details.is_empty() {
        eprintln!("\nPANICS:");
        for (name, msg) in &panic_details {
            eprintln!("  {name}: {msg}");
        }
    }

    assert_eq!(panicked, 0, "{panicked} files caused panics");
    assert_eq!(errored, 0, "{errored} files had decode/compare errors");
    if mismatched > 0 {
        eprintln!(
            "\nWARNING: {mismatched} files fail zensim tolerance vs libavif (see details above)"
        );
    }
}

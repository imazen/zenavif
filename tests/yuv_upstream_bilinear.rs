//! Pins the upstream `yuv` behaviour that let `src/yuv_bilinear_fix.rs` retire.
//!
//! From 0.8.12 through 0.8.16, `yuv420_*_bilinear` / `i0xx_*_bilinear` paired
//! luma row-pairs against an overlapping chroma window
//! (`windows(u_stride * 2).step_by(u_stride)`). For an even-height image whose
//! chroma planes are exactly `ceil(h/2)` rows that yields `ceil(h/2) - 1`
//! windows for `h/2` luma pairs, so the zip dropped the LAST luma row pair and
//! the bottom two output rows stayed unwritten. zenavif carried a completion
//! wrapper for it, plus a reverse tripwire that failed once upstream fixed the
//! bug.
//!
//! 0.8.17 fixes it, the wrapper is retired, and this is the tripwire's positive
//! replacement: retiring a guard without leaving something behind to catch a
//! regression would trade a known defect for an unwatched one.
//!
//! Retirement evidence: `benchmarks/yuv_bilinear_retirement_2026-08-29.{tsv,meta}`
//! — 2,304 cells across all eight converter variants the decoder uses, both
//! height parities, 2x2 to 2048x2048, two ranges, three matrices, three content
//! classes. Wrapper-on-0.8.15 and direct-on-0.8.17 were byte-identical in every
//! cell, while the unrepaired 0.8.15 differed in 1,836 of them.

use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix};

/// 4:2:0 planar gradient with flat mid chroma — the shape the original
/// reproduction used.
fn gradient_planar(w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; w * h];
    for yy in 0..h {
        for xx in 0..w {
            // +1 so no sample is 0: an all-zero row must mean "unwritten",
            // never "legitimately black", or this test could pass vacuously.
            y[yy * w + xx] = (((xx + yy) * 254) / (w + h) + 1) as u8;
        }
    }
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    (y, vec![128u8; cw * ch], vec![128u8; cw * ch])
}

/// Every output row must be written on even-height 4:2:0 bilinear input.
///
/// This is the assertion the retired wrapper existed to make true. If it ever
/// fails, upstream has regressed and the completion wrapper must come back —
/// its implementation is in this repo's history at `src/yuv_bilinear_fix.rs`.
#[test]
fn upstream_writes_every_row_on_even_height_420_bilinear() {
    for (w, h) in [(64usize, 64usize), (128, 128), (66, 4), (64, 2), (320, 240)] {
        let (y, u, v) = gradient_planar(w, h);
        let cw = w.div_ceil(2);
        let planar = YuvPlanarImage {
            y_plane: &y,
            y_stride: w as u32,
            u_plane: &u,
            u_stride: cw as u32,
            v_plane: &v,
            v_stride: cw as u32,
            width: w as u32,
            height: h as u32,
        };
        let mut out = vec![0u8; w * h * 4];
        yuv::yuv420_to_rgba_bilinear(
            &planar,
            &mut out,
            (w * 4) as u32,
            YuvRange::Full,
            YuvStandardMatrix::Bt601,
        )
        .expect("conversion failed");

        // Check BOTH bottom rows: the defect dropped the whole last luma pair,
        // so asserting only the final row would miss a half-regression.
        for row in [h - 2, h - 1] {
            let start = row * w * 4;
            let slice = &out[start..start + w * 4];
            assert!(
                slice.iter().any(|&b| b != 0),
                "{w}x{h}: output row {row} is entirely unwritten — upstream yuv \
                 has regressed the even-height 4:2:0 bilinear row-pair bug. \
                 Restore the completion wrapper (src/yuv_bilinear_fix.rs in \
                 this repo's history) and re-pin `yuv`."
            );
            let (pixels, rest) = slice.as_chunks::<4>();
            debug_assert!(
                rest.is_empty(),
                "row length must be a whole number of RGBA pixels"
            );
            assert!(
                pixels.iter().all(|px| px[3] == 255),
                "{w}x{h}: output row {row} has unwritten alpha"
            );
        }
    }
}

/// Odd heights were never affected — they have a dedicated clamp path upstream.
/// Kept so a future "fix" that breaks the odd-height tail is caught here too.
#[test]
fn upstream_writes_every_row_on_odd_height_420_bilinear() {
    for (w, h) in [(64usize, 63usize), (65, 65), (66, 5)] {
        let (y, u, v) = gradient_planar(w, h);
        let cw = w.div_ceil(2);
        let planar = YuvPlanarImage {
            y_plane: &y,
            y_stride: w as u32,
            u_plane: &u,
            u_stride: cw as u32,
            v_plane: &v,
            v_stride: cw as u32,
            width: w as u32,
            height: h as u32,
        };
        let mut out = vec![0u8; w * h * 3];
        yuv::yuv420_to_rgb_bilinear(
            &planar,
            &mut out,
            (w * 3) as u32,
            YuvRange::Full,
            YuvStandardMatrix::Bt601,
        )
        .expect("conversion failed");
        let start = (h - 1) * w * 3;
        assert!(
            out[start..start + w * 3].iter().any(|&b| b != 0),
            "{w}x{h}: final odd-height row unwritten"
        );
    }
}

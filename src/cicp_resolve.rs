//! H.273 matrix-coefficient resolution for decode.
//!
//! Consumer-side implementation of the `Cicp::resolve_matrix` contract
//! specified in zenpixels#36 (this crate is the gating "real codec
//! consumer" there). The logic here is spec-identical and migrates to
//! `zenpixels::Cicp::resolve_matrix` when that lands in a released
//! zenpixels — keep the two in lockstep until then.
//!
//! Resolution and support are distinct steps, per imazen/zenavif#15:
//!
//! 1. **Resolve** the signaled code point to a concrete recipe:
//!    self-contained codes pass through; MC=12/13 derive their
//!    coefficients from the colour primaries (never blind-copied);
//!    MC=2/reserved apply a valid hint (the container's `nclx` matrix,
//!    which the AV1-bitstream-authoritative path otherwise discards)
//!    or error.
//! 2. **Map** the resolved recipe onto the conversions this decoder
//!    actually implements — and error loudly on resolved-but-
//!    unimplemented math (YCgCo, BT.2020-CL, Y′D′zD′x, ICtCp, and
//!    derived KR/KB pairs with no matching table). A wrong matrix is
//!    silent chroma corruption on every pixel; an error is honest.
//!
//! MC=0 (Identity) is not a matrix at all: planes are already G,B,R
//! and conversion is a reorder + range expansion. Callers branch to
//! the identity path when this module returns
//! [`ResolvedMatrix::Identity`].

use crate::Result;
use crate::error::Error;
use whereat::at;

/// The AVIF specification's default matrix coefficients when the file
/// carries no signaling at all (CICP 1/13/**6**, full range). Used as
/// the last hint in the resolution chain for an unspecified bitstream
/// MC with no container `nclx` — a documented spec default, distinct
/// from guessing on an *explicit* `nclx` MC=2.
pub(crate) const AVIF_DEFAULT_MC: u8 = 6;

/// Whether `mc` can serve as a resolution hint (it must itself be a
/// non-unspecified, non-reserved code point).
pub(crate) fn is_resolvable_hint(mc: u8) -> bool {
    !matches!(mc, 2 | 3 | 15..)
}

/// A resolved, implementable conversion recipe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ResolvedMatrix {
    /// MC=0: planes are G,B,R — reorder + range scale, no matrix math.
    Identity,
    /// BT.709 (MC=1, or derived from CP=1).
    Bt709,
    /// BT.601 family (MC=5/6).
    Bt601,
    /// US FCC (MC=4) — exact KR=0.30/KB=0.11 via the yuv crate.
    Fcc,
    /// BT.2020 non-constant luminance (MC=9, or derived from CP=9).
    Bt2020Ncl,
    /// SMPTE 240M (MC=7).
    Smpte240,
    /// Chromaticity-derived NCL (MC=12) whose coefficients match no
    /// named table — decoded exactly via the yuv crate's custom-KR/KB
    /// path (e.g. P3 primaries: KR≈0.229, KB≈0.079).
    Derived {
        /// Derived KR (red luminance weight).
        kr: f32,
        /// Derived KB (blue luminance weight).
        kb: f32,
    },
}

impl ResolvedMatrix {
    /// Map to the `yuv` crate's matrix (16-bit / fallback paths).
    /// `None` = identity (caller takes the reorder path).
    pub(crate) fn to_yuv_std(self) -> Option<yuv::YuvStandardMatrix> {
        use yuv::YuvStandardMatrix as M;
        match self {
            Self::Identity => None,
            Self::Bt709 => Some(M::Bt709),
            Self::Bt601 => Some(M::Bt601),
            Self::Fcc => Some(M::Fcc),
            Self::Bt2020Ncl => Some(M::Bt2020),
            Self::Smpte240 => Some(M::Smpte240),
            Self::Derived { kr, kb } => Some(M::Custom(kr, kb)),
        }
    }

    /// Map to the in-house kernel matrix. `None` = identity only (which
    /// has no matrix math); every real matrix — including FCC, SMPTE 240M
    /// and chromaticity-derived — maps, via explicit (Kr, Kb) where no
    /// named variant exists (H.273 table 4 values).
    pub(crate) fn to_our(self) -> Option<crate::yuv_convert::YuvMatrix> {
        use crate::yuv_convert::YuvMatrix as M;
        match self {
            Self::Identity => None,
            Self::Bt709 => Some(M::Bt709),
            Self::Bt601 => Some(M::Bt601),
            Self::Bt2020Ncl => Some(M::Bt2020),
            Self::Fcc => Some(M::Custom { kr: 0.30, kb: 0.11 }),
            Self::Smpte240 => Some(M::Custom {
                kr: 0.212,
                kb: 0.087,
            }),
            Self::Derived { kr, kb } => Some(M::Custom { kr, kb }),
        }
    }
}

/// Resolve signaled H.273 code points to an implementable recipe.
///
/// * `mc` — matrix coefficients from the authoritative source (the AV1
///   bitstream sequence header, per the existing decode precedence).
/// * `cp` — effective colour primaries (container `nclx` if present,
///   else bitstream), used for MC=12/13 derivation.
/// * `hint_mc` — the container `nclx` matrix code, consulted only when
///   `mc` is 2 (unspecified) or reserved.
pub(crate) fn resolve(mc: u8, cp: u8, hint_mc: Option<u8>) -> Result<ResolvedMatrix> {
    match mc {
        0 => Ok(ResolvedMatrix::Identity),
        1 => Ok(ResolvedMatrix::Bt709),
        4 => Ok(ResolvedMatrix::Fcc),
        5 | 6 => Ok(ResolvedMatrix::Bt601),
        7 => Ok(ResolvedMatrix::Smpte240),
        9 => Ok(ResolvedMatrix::Bt2020Ncl),
        8 => Err(at!(Error::Unsupported(
            "matrix_coefficients=8 (YCgCo) is not implemented; refusing to mis-decode as YCbCr"
        ))),
        10 => Err(at!(Error::Unsupported(
            "matrix_coefficients=10 (BT.2020 constant-luminance) changes the decode math; \
             refusing to NCL-decode it"
        ))),
        11 => Err(at!(Error::Unsupported(
            "matrix_coefficients=11 (SMPTE ST 2085 Y'D'zD'x) is not implemented"
        ))),
        14 => Err(at!(Error::Unsupported(
            "matrix_coefficients=14 (ICtCp) is not implemented; refusing to mis-decode as YCbCr"
        ))),
        12 | 13 => {
            if mc == 13 {
                // Derivation may succeed, but constant-luminance math
                // is a different decode path entirely.
                return Err(at!(Error::Unsupported(
                    "matrix_coefficients=13 (chromaticity-derived constant-luminance) is not \
                     implemented; refusing to NCL-decode it"
                )));
            }
            let (kr, kb) = derive_kr_kb(cp)?;
            Ok(match_known_krkb(kr, kb).unwrap_or(ResolvedMatrix::Derived {
                kr: kr as f32,
                kb: kb as f32,
            }))
        }
        // 2 = unspecified; 3 and 15.. are reserved. A valid hint (the
        // container nclx matrix) resolves them; otherwise this is an
        // honest error, never a silent BT.601 guess.
        2 | 3 | 15.. => match hint_mc {
            Some(h) if h != 2 && h != 3 && h < 15 => resolve(h, cp, None),
            _ => Err(at!(Error::Unsupported(
                "matrix_coefficients is unspecified/reserved and no container nclx hint is \
                 available; cannot pick a YUV matrix without silent chroma corruption"
            ))),
        },
    }
}

/// Derive (KR, KB) from colour primaries per H.273 §8.3: the luminance
/// row of the RGB→XYZ matrix built from the primaries' chromaticities
/// and white point.
fn derive_kr_kb(cp: u8) -> Result<(f64, f64)> {
    // (xR, yR, xG, yG, xB, yB, xW, yW) per H.273 Table 2.
    let c: [f64; 8] = match cp {
        1 => [0.640, 0.330, 0.300, 0.600, 0.150, 0.060, 0.3127, 0.3290], // BT.709
        4 => [0.670, 0.330, 0.210, 0.710, 0.140, 0.080, 0.3101, 0.3162], // BT.470M / C
        5 => [0.640, 0.330, 0.290, 0.600, 0.150, 0.060, 0.3127, 0.3290], // BT.470BG
        6 | 7 => [0.630, 0.340, 0.310, 0.595, 0.155, 0.070, 0.3127, 0.3290], // 170M/240M
        8 => [0.681, 0.319, 0.243, 0.692, 0.145, 0.049, 0.3100, 0.3160], // Film (C)
        9 => [0.708, 0.292, 0.170, 0.797, 0.131, 0.046, 0.3127, 0.3290], // BT.2020
        11 => [0.680, 0.320, 0.265, 0.690, 0.150, 0.060, 0.3140, 0.3510], // P3-DCI
        12 => [0.680, 0.320, 0.265, 0.690, 0.150, 0.060, 0.3127, 0.3290], // P3-D65
        22 => [0.630, 0.340, 0.295, 0.605, 0.155, 0.077, 0.3127, 0.3290], // EBU 3213-E
        _ => {
            return Err(at!(Error::Unsupported(
                "matrix_coefficients=12/13 require colour primaries with known chromaticities \
                 to derive the matrix; the signaled primaries are unspecified/reserved"
            )));
        }
    };
    let [xr, yr, xg, yg, xb, yb, xw, yw] = c;
    // z = 1 - x - y; scale each primary's XYZ column so the white point
    // maps to Y = 1; KR/KG/KB are the Y-row entries.
    let (zr, zg, zb, zw) = (1.0 - xr - yr, 1.0 - xg - yg, 1.0 - xb - yb, 1.0 - xw - yw);
    // Solve [Xr Xg Xb; Yr Yg Yb; Zr Zg Zb] * s = white_XYZ with
    // primary columns (x,y,z)/y and white (xw,yw,zw)/yw.
    let (wx, wz) = (xw / yw, zw / yw);
    // Columns before scaling: (x/y, 1, z/y) per primary.
    let (ar, ag, ab) = (xr / yr, xg / yg, xb / yb);
    let (cr, cg, cb) = (zr / yr, zg / yg, zb / yb);
    // 3x3 solve via Cramer's rule for s = (sr, sg, sb):
    // [ar ag ab][sr]   [wx]
    // [1  1  1 ][sg] = [1 ]
    // [cr cg cb][sb]   [wz]
    let det = ar * (cb - cg) - ag * (cb - cr) + ab * (cg - cr);
    if det.abs() < 1e-12 {
        return Err(at!(Error::Unsupported(
            "degenerate colour primaries; cannot derive matrix coefficients"
        )));
    }
    let det_r = wx * (cb - cg) - ag * (cb - wz) + ab * (cg - wz);
    let det_g = ar * (cb - wz) - wx * (cb - cr) + ab * (wz - cr);
    let det_b = ar * (wz - cg) - ag * (wz - cr) + wx * (cg - cr);
    // KR/KG/KB = Y-row = the scale factors themselves (Y component of
    // each scaled column is s_i * 1).
    let (kr, kg, kb) = (det_r / det, det_g / det, det_b / det);
    debug_assert!((kr + kg + kb - 1.0).abs() < 1e-9);
    Ok((kr, kb))
}

/// Match derived (KR, KB) against the implemented tables.
fn match_known_krkb(kr: f64, kb: f64) -> Option<ResolvedMatrix> {
    const EPS: f64 = 2e-3;
    let close = |a: f64, b: f64| (a - b).abs() < EPS;
    if close(kr, 0.2126) && close(kb, 0.0722) {
        Some(ResolvedMatrix::Bt709)
    } else if close(kr, 0.299) && close(kb, 0.114) {
        Some(ResolvedMatrix::Bt601)
    } else if close(kr, 0.2627) && close(kb, 0.0593) {
        Some(ResolvedMatrix::Bt2020Ncl)
    } else if close(kr, 0.212) && close(kb, 0.087) {
        Some(ResolvedMatrix::Smpte240)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_contained_codes_pass_through() {
        assert_eq!(resolve(0, 1, None).unwrap(), ResolvedMatrix::Identity);
        assert_eq!(resolve(1, 1, None).unwrap(), ResolvedMatrix::Bt709);
        assert_eq!(resolve(4, 1, None).unwrap(), ResolvedMatrix::Fcc);
        assert_eq!(resolve(5, 1, None).unwrap(), ResolvedMatrix::Bt601);
        assert_eq!(resolve(6, 1, None).unwrap(), ResolvedMatrix::Bt601);
        assert_eq!(resolve(7, 1, None).unwrap(), ResolvedMatrix::Smpte240);
        assert_eq!(resolve(9, 9, None).unwrap(), ResolvedMatrix::Bt2020Ncl);
    }

    #[test]
    fn unimplemented_math_errors_loudly() {
        for mc in [8u8, 10, 11, 14] {
            let err = resolve(mc, 1, None).unwrap_err().to_string();
            assert!(
                err.contains(&format!("matrix_coefficients={mc}")),
                "error must name the code point: {err}"
            );
        }
    }

    #[test]
    fn unspecified_uses_hint_else_errors() {
        // MC=2 with a valid container hint resolves through the hint.
        assert_eq!(resolve(2, 1, Some(1)).unwrap(), ResolvedMatrix::Bt709);
        assert_eq!(resolve(2, 1, Some(0)).unwrap(), ResolvedMatrix::Identity);
        // Hint that is itself unspecified/reserved does not count.
        assert!(resolve(2, 1, Some(2)).is_err());
        assert!(resolve(2, 1, Some(3)).is_err());
        // No hint: loud error naming the situation.
        let err = resolve(2, 1, None).unwrap_err().to_string();
        assert!(err.contains("unspecified"), "got {err}");
        // Reserved codes behave like unspecified.
        assert_eq!(resolve(3, 1, Some(9)).unwrap(), ResolvedMatrix::Bt2020Ncl);
        assert!(resolve(15, 1, None).is_err());
        assert!(resolve(255, 1, None).is_err());
    }

    #[test]
    fn mc12_derives_from_primaries() {
        // CP=1 (BT.709 primaries) → 709 coefficients.
        assert_eq!(resolve(12, 1, None).unwrap(), ResolvedMatrix::Bt709);
        // CP=9 (BT.2020) → 2020-NCL coefficients (the canonical pair).
        assert_eq!(resolve(12, 9, None).unwrap(), ResolvedMatrix::Bt2020Ncl);
        // CP=12 (P3-D65) derives KR≈0.229/KB≈0.079 — decoded exactly
        // via the yuv crate's custom-coefficient path.
        match resolve(12, 12, None).unwrap() {
            ResolvedMatrix::Derived { kr, kb } => {
                assert!((kr - 0.2290).abs() < 2e-3, "P3 KR derived {kr}");
                assert!((kb - 0.0793).abs() < 2e-3, "P3 KB derived {kb}");
            }
            other => panic!("expected Derived for P3, got {other:?}"),
        }
        // CP unspecified cannot derive.
        assert!(resolve(12, 2, None).is_err());
    }

    #[test]
    fn mc13_constant_luminance_rejected_even_when_derivable() {
        let err = resolve(13, 9, None).unwrap_err().to_string();
        assert!(err.contains("13"), "got {err}");
    }

    #[test]
    fn derivation_matches_h273_published_values() {
        let (kr, kb) = derive_kr_kb(1).unwrap();
        assert!((kr - 0.2126).abs() < 5e-4, "709 KR derived {kr}");
        assert!((kb - 0.0722).abs() < 5e-4, "709 KB derived {kb}");
        let (kr, kb) = derive_kr_kb(9).unwrap();
        assert!((kr - 0.2627).abs() < 5e-4, "2020 KR derived {kr}");
        assert!((kb - 0.0593).abs() < 5e-4, "2020 KB derived {kb}");
    }
}

//! NEON unpremultiply for 8-bit RGBA rows.
//!
//! `convert::unpremultiply8` divides by the pixel's own alpha — a runtime
//! value — and there is no SIMD integer divide, so the scalar loop cannot
//! vectorize no matter what the compiler does. This runs once per row on every
//! alpha-bearing AVIF, in both the buffered (`convert.rs`) and streaming
//! (`strip_convert.rs`) paths.
//!
//! `vld4q_u8` is what makes a vector version possible: it deinterleaves RGBA
//! into four planes in one instruction, so the alpha for a whole 16-pixel group
//! is already a vector and no per-pixel shuffling is needed. The same kernel
//! shape measured 2.7x in zenresize.
//!
//! # Exactness
//!
//! Bit-identical to the scalar formula, not an approximation. `num = c*255 +
//! a/2 <= 65152` and `a <= 255` are both integers exactly representable in f32,
//! so `vdivq_f32` returns the correctly rounded quotient. Truncating that could
//! only disagree with integer floor if rounding crossed an integer, but a
//! non-integral `num/a` sits at least `1/a >= 1/255` from the nearest integer
//! while its half-ULP is at most `65025 * 2^-24`, about 1000x smaller.
//!
//! Two branch cases from the scalar loop:
//! * `a == 255` needs no special case — `(c*255 + 127)/255 == c` exactly.
//! * `a == 0` divides by zero, so it is selected away. NOTE this crate leaves
//!   such pixels UNCHANGED (unlike zenresize, which zeroes RGB), so the select
//!   restores the original channel rather than zero.
//!
//! The tests below enumerate the complete (channel, alpha) domain rather than
//! sampling it.

#![allow(clippy::needless_range_loop)]

use rgb::Rgba;
use rgb::prelude::*;

#[cfg(target_arch = "aarch64")]
use archmage::prelude::*;

/// Widen a `u8x16` into four `u32x4` groups (lossless).
#[cfg(target_arch = "aarch64")]
#[archmage::rite]
fn widen_u8x16(_t: NeonToken, v: uint8x16_t) -> [uint32x4_t; 4] {
    let lo16 = vmovl_u8(vget_low_u8(v));
    let hi16 = vmovl_u8(vget_high_u8(v));
    [
        vmovl_u16(vget_low_u16(lo16)),
        vmovl_u16(vget_high_u16(lo16)),
        vmovl_u16(vget_low_u16(hi16)),
        vmovl_u16(vget_high_u16(hi16)),
    ]
}

/// Narrow four `u32x4` groups (all values <= 255) back into a `u8x16`.
#[cfg(target_arch = "aarch64")]
#[archmage::rite]
fn narrow_u32x4x4(_t: NeonToken, g: [uint32x4_t; 4]) -> uint8x16_t {
    let n0 = vcombine_u16(vmovn_u32(g[0]), vmovn_u32(g[1]));
    let n1 = vcombine_u16(vmovn_u32(g[2]), vmovn_u32(g[3]));
    vcombine_u8(vmovn_u16(n0), vmovn_u16(n1))
}

/// One 4-lane group: `min(255, (c*255 + a/2) / a)`, leaving `c` where `a == 0`.
#[cfg(target_arch = "aarch64")]
#[archmage::rite]
fn unpremul_group(_t: NeonToken, c: uint32x4_t, a: uint32x4_t) -> uint32x4_t {
    let num = vaddq_u32(vmulq_u32(c, vdupq_n_u32(255)), vshrq_n_u32::<1>(a));
    let q = vdivq_f32(vcvtq_f32_u32(num), vcvtq_f32_u32(a));
    let r = vminq_u32(vcvtq_u32_f32(q), vdupq_n_u32(255));
    // a == 0: this crate's scalar loop leaves the pixel untouched.
    vbslq_u32(vceqq_u32(a, vdupq_n_u32(0)), c, r)
}

/// NEON row kernel: 16 pixels per iteration, scalar remainder for the tail.
#[cfg(target_arch = "aarch64")]
#[archmage::arcane]
pub(crate) fn unpremultiply8_neon(token: NeonToken, row: &mut [Rgba<u8>]) {
    const PX: usize = 16;
    let bytes: &mut [u8] = bytemuck::cast_slice_mut(row);
    let full = bytes.len() / (PX * 4) * (PX * 4);
    let (body, tail) = bytes.split_at_mut(full);

    for chunk in body.chunks_exact_mut(PX * 4) {
        let block: &mut [u8; PX * 4] = chunk.try_into().unwrap();
        let p = vld4q_u8(block);
        let a_groups = widen_u8x16(token, p.3);

        let mut planes = [p.0, p.1, p.2];
        for plane in planes.iter_mut() {
            let c = widen_u8x16(token, *plane);
            *plane = narrow_u32x4x4(
                token,
                [
                    unpremul_group(token, c[0], a_groups[0]),
                    unpremul_group(token, c[1], a_groups[1]),
                    unpremul_group(token, c[2], a_groups[2]),
                    unpremul_group(token, c[3], a_groups[3]),
                ],
            );
        }
        vst4q_u8(block, uint8x16x4_t(planes[0], planes[1], planes[2], p.3));
    }

    if !tail.is_empty() {
        unpremultiply8_scalar(bytemuck::cast_slice_mut(tail));
    }
}

/// The original scalar loop. Kept as the reference and the non-aarch64 path.
pub(crate) fn unpremultiply8_scalar(img_row: &mut [Rgba<u8>]) {
    for px in img_row.iter_mut() {
        if px.a != 255 && px.a != 0 {
            *px.rgb_mut() = px
                .rgb()
                .map(|c| ((c as u16 * 255 + px.a as u16 / 2) / px.a as u16).min(255) as u8);
        }
    }
}

/// Dispatch: NEON where available, otherwise the scalar loop unchanged.
#[inline]
pub fn unpremultiply8_dispatch(img_row: &mut [Rgba<u8>]) {
    #[cfg(target_arch = "aarch64")]
    {
        use archmage::SimdToken;
        if let Some(t) = NeonToken::summon() {
            unpremultiply8_neon(t, img_row);
            return;
        }
    }
    unpremultiply8_scalar(img_row);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The original scalar body, transcribed verbatim as the oracle.
    fn reference(c: u8, a: u8) -> u8 {
        if a != 255 && a != 0 {
            ((c as u16 * 255 + a as u16 / 2) / a as u16).min(255) as u8
        } else {
            c
        }
    }

    fn expect_of(row: &[Rgba<u8>]) -> Vec<Rgba<u8>> {
        row.iter()
            .map(|p| Rgba {
                r: reference(p.r, p.a),
                g: reference(p.g, p.a),
                b: reference(p.b, p.a),
                a: p.a,
            })
            .collect()
    }

    /// All 256 channel values x all 256 alphas. The domain is small enough to
    /// enumerate, so a pass here is a proof rather than a sample — which is the
    /// bar a floating-point substitution for integer division should have to
    /// clear.
    #[test]
    fn exact_over_complete_domain() {
        let mut row: Vec<Rgba<u8>> = Vec::with_capacity(256 * 256);
        for a in 0..=255u16 {
            for c in 0..=255u16 {
                // Distinct value per channel so the three lanes are not all
                // testing the same number.
                row.push(Rgba {
                    r: c as u8,
                    g: (255 - c) as u8,
                    b: ((c * 7) % 256) as u8,
                    a: a as u8,
                });
            }
        }
        let expect = expect_of(&row);
        let mut got = row.clone();
        unpremultiply8_dispatch(&mut got);
        for (i, (g, e)) in got.iter().zip(expect.iter()).enumerate() {
            assert_eq!(
                (g.r, g.g, g.b, g.a),
                (e.r, e.g, e.b, e.a),
                "pixel {i} diverged; input {:?}",
                row[i]
            );
        }
        assert_eq!(got.len(), 256 * 256, "domain not fully enumerated");
    }

    /// The NEON kernel takes 16 pixels per `vld4q_u8`; the exhaustive row above
    /// is a multiple of 16 and never reaches the scalar remainder, so cover
    /// every tail length explicitly.
    #[test]
    fn tail_lengths_exact() {
        for px in 0..=40usize {
            let row: Vec<Rgba<u8>> = (0..px)
                .map(|i| Rgba {
                    r: (i * 13 % 256) as u8,
                    g: (i * 91 % 256) as u8,
                    b: 200,
                    a: ((i * 37) % 256) as u8,
                })
                .collect();
            let expect = expect_of(&row);
            let mut got = row.clone();
            unpremultiply8_dispatch(&mut got);
            for (i, (g, e)) in got.iter().zip(expect.iter()).enumerate() {
                assert_eq!(
                    (g.r, g.g, g.b, g.a),
                    (e.r, e.g, e.b, e.a),
                    "tail len {px}, pixel {i}"
                );
            }
        }
    }

    /// This crate leaves `a == 0` pixels UNCHANGED (zenresize zeroes RGB), and
    /// `a == 255` must be identity. Both are branch cases in the scalar loop
    /// and selects in the kernel, so pin them directly.
    #[test]
    fn edge_alpha_semantics() {
        let row = vec![
            Rgba { r: 10, g: 20, b: 30, a: 0 },
            Rgba { r: 40, g: 50, b: 60, a: 255 },
        ];
        let mut got = row.clone();
        unpremultiply8_dispatch(&mut got);
        assert_eq!(
            (got[0].r, got[0].g, got[0].b, got[0].a),
            (10, 20, 30, 0),
            "a == 0 must leave the pixel untouched"
        );
        assert_eq!(
            (got[1].r, got[1].g, got[1].b, got[1].a),
            (40, 50, 60, 255),
            "a == 255 must be identity"
        );
    }
}

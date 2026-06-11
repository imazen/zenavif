//! Identity (MC=0 / GBR) roundtrip contracts — the lockstep gate for
//! imazen/zenavif#14 (16-bit encode plane order) and #15 (decode
//! identity passthrough). Each bug alone made the other invisible in
//! self-roundtrips; these tests pin both at once.
//!
//! Tolerance note: the asserts allow |delta| ≤ 2 per channel because
//! zenrav1e's `with_lossless` is currently not bit-exact
//! (imazen/zenrav1e#9 — ±2 scatter on ~28 % of pixels; measured here).
//! That tolerance still catches the bugs these tests gate: a channel
//! rotation or a YCbCr matrix applied to GBR planes produces deltas in
//! the tens-to-hundreds. Tighten to exact equality when zenrav1e#9 is
//! fixed.

#![cfg(feature = "encode-imazen")]

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::{RGB16, Rgb};
use zenavif::{EncodeColorModel, EncoderConfig};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

fn mix(x: u32, y: u32, salt: u32) -> u8 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ salt;
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    (h >> 16) as u8
}

/// Expand a 10-bit value to 16 bits the way the decoder does
/// (LSB replication) — these values roundtrip the encoder's
/// truncating 16→10 scale exactly.
fn expand10(v10: u16) -> u16 {
    (v10 << 6) | (v10 >> 4)
}

#[test]
fn identity_8bit_lossless_roundtrip_is_pixel_exact() {
    let (w, h) = (64usize, 64usize);
    let pixels: Vec<Rgb<u8>> = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| Rgb {
                r: mix(x as u32, y as u32, 1),
                g: mix(x as u32, y as u32, 2),
                b: mix(x as u32, y as u32, 3),
            })
        })
        .collect();
    let img = ImgVec::new(pixels, w, h);

    let cfg = EncoderConfig::new()
        .speed(6)
        .threads(Some(1))
        .color_model(EncodeColorModel::Rgb)
        .with_lossless(true);
    let enc = zenavif::encode_rgb8(img.as_ref(), &cfg, stop()).expect("encode");

    let dec_cfg = zenavif::DecoderConfig::new().prefer_8bit(true);
    let out = zenavif::decode_with(&enc.avif_file, &dec_cfg, &Unstoppable).expect("decode");
    let got = out.try_as_imgref::<Rgb<u8>>().expect("rgb8 view");

    assert_eq!(got.width(), w);
    assert_eq!(got.height(), h);
    for y in 0..h {
        for x in 0..w {
            let (e, g) = (img.buf()[y * w + x], got.buf()[y * got.stride() + x]);
            let d = (i32::from(e.r) - i32::from(g.r))
                .abs()
                .max((i32::from(e.g) - i32::from(g.g)).abs())
                .max((i32::from(e.b) - i32::from(g.b)).abs());
            assert!(
                d <= 2, // zenrav1e#9; rotation/matrix bugs produce d in the tens+
                "identity roundtrip delta {d} at ({x},{y}): encoded {e:?}, decoded {g:?} — \
                 channel rotation or matrix math on the identity path"
            );
        }
    }
}

#[test]
fn identity_16bit_lossless_roundtrip_is_pixel_exact() {
    let (w, h) = (64usize, 64usize);
    // Values exact under the 16→10→16 scale pair (truncate then
    // LSB-replicate), spread across the range and per-channel distinct
    // so any plane rotation breaks equality.
    let pixels: Vec<RGB16> = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| RGB16 {
                r: expand10(u16::from(mix(x as u32, y as u32, 1)) * 4 % 1024),
                g: expand10(u16::from(mix(x as u32, y as u32, 2)) * 4 % 1024),
                b: expand10(u16::from(mix(x as u32, y as u32, 3)) * 4 % 1024),
            })
        })
        .collect();
    let img = ImgVec::new(pixels, w, h);

    let cfg = EncoderConfig::new()
        .speed(6)
        .threads(Some(1))
        .with_lossless(true);
    let enc = zenavif::encode_rgb16(img.as_ref(), &cfg, stop()).expect("encode");

    let dec_cfg = zenavif::DecoderConfig::new().prefer_8bit(false);
    let out = zenavif::decode_with(&enc.avif_file, &dec_cfg, &Unstoppable).expect("decode");
    let got = out.try_as_imgref::<Rgb<u16>>().expect("rgb16 view");

    assert_eq!(got.width(), w);
    assert_eq!(got.height(), h);
    for y in 0..h {
        for x in 0..w {
            let (e, g) = (img.buf()[y * w + x], got.buf()[y * got.stride() + x]);
            // Compare in the native 10-bit domain (>>6 inverts the
            // LSB-replication expansion exactly for replicated values).
            let d10 = |a: u16, b: u16| (i32::from(a >> 6) - i32::from(b >> 6)).abs();
            let d = d10(e.r, g.r).max(d10(e.g, g.g)).max(d10(e.b, g.b));
            assert!(
                d <= 2, // zenrav1e#9 (±2 ten-bit steps); rotation = hundreds
                "16-bit identity roundtrip 10-bit delta {d} at ({x},{y}): encoded {e:?}, \
                 decoded {g:?} — this is exactly the #14 plane-rotation failure mode"
            );
        }
    }
}

/// Lossy sanity for the same paths: dominant channels land in the
/// right slots without requiring exact equality (catches gross
/// rotations even if lossless ever regresses to near-lossless).
#[test]
fn identity_lossy_channels_stay_in_their_slots() {
    let (w, h) = (64usize, 64usize);
    let red8: Vec<Rgb<u8>> = vec![
        Rgb {
            r: 200,
            g: 10,
            b: 30
        };
        w * h
    ];
    let img = ImgVec::new(red8, w, h);
    let cfg = EncoderConfig::new()
        .quality(95.0)
        .speed(8)
        .threads(Some(1))
        .color_model(EncodeColorModel::Rgb);
    let enc = zenavif::encode_rgb8(img.as_ref(), &cfg, stop()).expect("encode");
    let out = zenavif::decode_with(
        &enc.avif_file,
        &zenavif::DecoderConfig::new().prefer_8bit(true),
        &Unstoppable,
    )
    .expect("decode");
    let got = out.try_as_imgref::<Rgb<u8>>().expect("rgb8");
    let p = got.buf()[(h / 2) * got.stride() + w / 2];
    assert!(
        p.r > 150 && p.g < 80 && p.b < 80,
        "red must stay red on the identity path; got {p:?} (the pre-fix decoder \
         returned R=111 G=0 B=0 here via BT.601-on-GBR)"
    );

    let red16: Vec<RGB16> = vec![
        RGB16 {
            r: 200 * 257,
            g: 10 * 257,
            b: 30 * 257,
        };
        w * h
    ];
    let img16 = ImgVec::new(red16, w, h);
    let cfg16 = EncoderConfig::new().quality(95.0).speed(8).threads(Some(1));
    let enc16 = zenavif::encode_rgb16(img16.as_ref(), &cfg16, stop()).expect("encode16");
    let out16 = zenavif::decode_with(
        &enc16.avif_file,
        &zenavif::DecoderConfig::new().prefer_8bit(true),
        &Unstoppable,
    )
    .expect("decode16");
    let got16 = out16.try_as_imgref::<Rgb<u8>>().expect("rgb8 of 16");
    let p = got16.buf()[(h / 2) * got16.stride() + w / 2];
    assert!(
        p.r > 150 && p.g < 80 && p.b < 80,
        "red must stay red on the 16-bit path; got {p:?} (the pre-fix encoder \
         wrote RGB plane order, decoding as R=63 G=255 B=0 here)"
    );
}

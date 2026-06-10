//! Encode-level contracts that the type-level API promises.
//!
//! These pin behavior that documentation alone cannot: each test
//! encodes real pixels and asserts byte-level facts. They are the
//! integration-test complement of `examples/sweep_validate.rs` (which
//! needs the full `__expert` feature set and a corpus); the contracts
//! here hold with plain `encode`.

#![cfg(feature = "encode")]

use almost_enough::{StopToken, Unstoppable};
use imgref::ImgVec;
use rgb::{Rgb, Rgba};
use zenavif::{EncoderConfig, PlanInput, encode_rgb8, encode_rgba8};

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

/// Deterministic pseudo-random byte without a rand dependency.
fn mix(x: u32, y: u32, salt: u32) -> u8 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ salt;
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    (h >> 16) as u8
}

/// A 96×96 RGBA image with textured color and a non-trivial alpha
/// gradient — the alpha plane must carry real signal so its quantizer
/// affects output bytes.
fn rgba_test_image() -> ImgVec<Rgba<u8>> {
    let (w, h) = (96usize, 96usize);
    let pixels = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| Rgba {
                r: mix(x as u32, y as u32, 1),
                g: mix(x as u32, y as u32, 2),
                b: mix(x as u32, y as u32, 3),
                // Diagonal gradient with texture: non-opaque nearly
                // everywhere, so the alpha plane is emitted and has
                // entropy worth quantizing.
                a: (((x + y) * 255 / (w + h - 2)) as u8) ^ (mix(x as u32, y as u32, 4) >> 3),
            })
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn rgb_test_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let pixels = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| Rgb {
                r: mix(x as u32, y as u32, 1),
                g: mix(x as u32, y as u32, 2),
                b: mix(x as u32, y as u32, 3),
            })
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

/// Contract: `alpha_quality` unset ⇒ the alpha plane is encoded at the
/// color quality — i.e. byte-identical to setting it explicitly.
///
/// zenravif's own default would pin the alpha quantizer to the
/// quality-80 equivalent regardless of the color quality (zenravif
/// 0.1.3 `av1encoder.rs`: `Default` sets `alpha_quantizer:
/// quality_to_quantizer(80.)` and `with_quality` never touches it);
/// zenavif forwards the fallback explicitly. The third encode pins the
/// fix's observable effect: at quality 30, the unset spelling must NOT
/// match an explicit `alpha_quality(80.0)` — which is exactly what it
/// silently was before the forwarding fix.
#[test]
fn alpha_quality_unset_follows_color_quality() {
    let img = rgba_test_image();
    let base = EncoderConfig::new().quality(30.0).speed(8).threads(Some(1));

    let unset = encode_rgba8(img.as_ref(), &base, stop()).expect("encode unset");
    let explicit = encode_rgba8(img.as_ref(), &base.clone().alpha_quality(30.0), stop())
        .expect("encode explicit");
    let q80 =
        encode_rgba8(img.as_ref(), &base.clone().alpha_quality(80.0), stop()).expect("encode aq80");

    assert!(
        unset.alpha_byte_size > 0,
        "test image must emit an alpha plane (got {} alpha bytes)",
        unset.alpha_byte_size
    );
    assert_eq!(
        unset.avif_file, explicit.avif_file,
        "alpha_quality unset must encode identically to alpha_quality(quality)"
    );
    assert_ne!(
        unset.avif_file, q80.avif_file,
        "alpha_quality unset must NOT behave like alpha_quality(80) — \
         that was the pre-fix zenravif default"
    );

    // The static plan agrees with the encode-level contract.
    let plan = base.resolve_plan(PlanInput::rgba8(96, 96));
    assert_eq!(plan.alpha_quantizer, Some(plan.quantizer));
}

/// Contract: distinct qualities mapping to the same resolved quantizer
/// are byte-identical (quality is fully mediated by the quantizer), and
/// a quality on a different quantizer differs. This is the encode-level
/// pin for the mirrored quality→quantizer curve that `resolve_plan` and
/// the sweep fingerprint rely on.
#[test]
fn quality_is_mediated_by_quantizer() {
    let img = rgb_test_image(96, 96);
    let cfg = |q: f32| EncoderConfig::new().quality(q).speed(8).threads(Some(1));

    let a = encode_rgb8(img.as_ref(), &cfg(80.0), stop()).expect("q80.0");
    let b = encode_rgb8(img.as_ref(), &cfg(80.2), stop()).expect("q80.2");
    let c = encode_rgb8(img.as_ref(), &cfg(81.0), stop()).expect("q81.0");

    // Mirror says: 80.0 → 71, 80.2 → 71, 81.0 → 68.
    let p = |q: f32| cfg(q).resolve_plan(PlanInput::rgb8(96, 96)).quantizer;
    assert_eq!(p(80.0), p(80.2));
    assert_ne!(p(80.0), p(81.0));

    assert_eq!(
        a.avif_file, b.avif_file,
        "equal resolved quantizer must produce identical bytes"
    );
    assert_ne!(
        a.avif_file, c.avif_file,
        "different resolved quantizer must produce different bytes"
    );
}

/// Contract: the chroma-subsampling knob is live — 4:2:0 must change
/// (and on textured content shrink) the encoded bytes vs 4:4:4.
#[test]
fn chroma_subsampling_is_live() {
    use zenavif::EncodeChromaSubsampling;
    let img = rgb_test_image(96, 96);
    let base = EncoderConfig::new().quality(60.0).speed(8).threads(Some(1));

    let cs444 = encode_rgb8(img.as_ref(), &base, stop()).expect("444");
    let cs420 = encode_rgb8(
        img.as_ref(),
        &base
            .clone()
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420),
        stop(),
    )
    .expect("420");

    assert_ne!(cs444.avif_file, cs420.avif_file);
    assert!(
        cs420.avif_file.len() < cs444.avif_file.len(),
        "4:2:0 must shrink chroma-textured content ({} vs {})",
        cs420.avif_file.len(),
        cs444.avif_file.len()
    );
}

/// validate() rejects the combinations the encoder would otherwise
/// remap silently or reject only at encode time.
#[test]
#[allow(deprecated)] // deliberately exercises the deprecated Svtav1 variant
fn validate_rejects_dead_and_contradictory_combos() {
    use zenavif::{Av1Backend, EncodeChromaSubsampling, EncodeColorModel};

    // The deprecated Svtav1 backend exists in no build: encode_rgb8
    // would silently serve the request with zenravif.
    let svt = EncoderConfig::new().backend(Av1Backend::Svtav1);
    assert!(
        svt.validate().is_err(),
        "deprecated Svtav1 must fail validate"
    );

    // 4:2:0 has no defined meaning for identity-matrix RGB.
    let rgb420 = EncoderConfig::new()
        .color_model(EncodeColorModel::Rgb)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420);
    assert!(rgb420.validate().is_err(), "Rgb × Yuv420 must fail");

    // The 16-bit entry points force the identity matrix, so 4:2:0 is
    // invalid for them even with the YCbCr color model configured.
    let ycc420 = EncoderConfig::new().chroma_subsampling(EncodeChromaSubsampling::Yuv420);
    assert!(ycc420.validate().is_ok());
    assert!(
        ycc420
            .validate_for_input(PlanInput {
                width: 64,
                height: 64,
                input_is_16bit: true,
                input_has_alpha: false,
            })
            .is_err(),
        "Yuv420 × 16-bit input must fail validate_for_input"
    );

    let ok = EncoderConfig::new().quality(75.0).speed(4);
    assert!(ok.validate().is_ok());
}

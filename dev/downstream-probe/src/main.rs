//! An OUT-OF-CRATE consumer of zenavif 0.2.0, standing in for `cargo
//! semver-checks` (which cannot run on this crate) and for the real
//! downstream repos (which cannot be edited).
//!
//! It exercises (a) the `zencodec` surface imageflow actually uses, (b) the
//! `EncoderConfig` builder surface, and (c) exhaustive matches on every enum
//! that became `#[non_exhaustive]` in 0.2.0 — those are the break, and this is
//! where it must show up.
use imgref::{Img, ImgVec};
use rgb::Rgb;
use zenavif::{
    Av1Backend, EncodeAlphaMode, EncodeBitDepth, EncodeChromaSubsampling, EncodeColorModel,
    EncodePixelRange, EncoderConfig,
};

fn stop() -> almost_enough::StopToken {
    almost_enough::StopToken::new(almost_enough::Unstoppable)
}

fn grad(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let mut b = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            b.push(Rgb {
                r: ((x * 255) / w.max(1)) as u8,
                g: ((y * 255) / h.max(1)) as u8,
                b: (((x + y) * 127) / (w + h).max(1)) as u8,
            });
        }
    }
    Img::new(b, w, h)
}

fn main() {
    // (a) The zencodec surface imageflow drives.
    let _dec = zenavif::AvifDecoderConfig::new();
    let _enc = zenavif::AvifEncoderConfig::new();

    // (c) Exhaustive matches on the newly `#[non_exhaustive]` enums. Each
    // wildcard arm below is the break a downstream consumer must add.
    let d = EncodeBitDepth::Twelve;
    let depth_name = match d {
        EncodeBitDepth::Eight => "8",
        EncodeBitDepth::Ten => "10",
        EncodeBitDepth::Twelve => "12",
        EncodeBitDepth::Auto => "auto",
        _ => "future",
    };
    let cm = match EncodeColorModel::default() {
        EncodeColorModel::YCbCr => "ycbcr",
        EncodeColorModel::Rgb => "rgb",
        _ => "future",
    };
    let ss = match EncodeChromaSubsampling::Yuv420 {
        EncodeChromaSubsampling::Yuv444 => "444",
        EncodeChromaSubsampling::Yuv420 => "420",
        _ => "future",
    };
    let am = match EncodeAlphaMode::default() {
        EncodeAlphaMode::UnassociatedClean => "clean",
        EncodeAlphaMode::UnassociatedDirty => "dirty",
        EncodeAlphaMode::Premultiplied => "premul",
        _ => "future",
    };
    // EncodePixelRange is NOT non_exhaustive: this match has no wildcard and
    // MUST still compile. If it stops compiling, the 0.2.0 note claiming the
    // domain is closed is wrong.
    let pr = match EncodePixelRange::Limited {
        EncodePixelRange::Full => "full",
        EncodePixelRange::Limited => "limited",
    };
    println!("enums: {depth_name} {cm} {ss} {am} {pr}");

    // (b) The builder surface, at every depth the aom backend codes.
    let img = grad(64, 64);
    for depth in [
        EncodeBitDepth::Eight,
        EncodeBitDepth::Ten,
        EncodeBitDepth::Twelve,
    ] {
        let cfg = EncoderConfig::new()
            .backend(Av1Backend::Zenav1Aom)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
            .bit_depth(depth)
            .quality(90.0)
            .speed(6);
        cfg.validate().expect("aom config must validate at 8/10/12");
        let out = zenavif::encode_rgb8(img.as_ref(), &cfg, stop())
            .unwrap_or_else(|e| panic!("{depth:?}: {e}"));
        let back = zenavif::decode(&out.avif_file).expect("decode");
        println!(
            "aom {depth:?}: {} bytes -> {}x{}",
            out.avif_file.len(),
            back.width(),
            back.height()
        );
    }

    // Twelve on the zenravif backend must be an honest refusal naming the one
    // backend that codes it, not a silent 10-bit encode.
    let e = zenavif::encode_rgb8(
        img.as_ref(),
        &EncoderConfig::new()
            .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
            .bit_depth(EncodeBitDepth::Twelve),
        stop(),
    )
    .expect_err("zenravif must refuse 12-bit");
    let msg = format!("{e}");
    assert!(msg.contains("Zenav1Aom"), "refusal must name the backend: {msg}");
    assert!(
        !msg.contains("zenav1-svt"),
        "an aom/zenravif refusal must not name the zenav1-svt feature: {msg}"
    );
    println!("zenravif 12-bit refusal: {msg}");
    println!("OK");
}

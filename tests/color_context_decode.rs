//! Decoded buffers are self-describing: the zencodec adapter attaches
//! the authoritative source color as a `zenpixels::ColorContext`
//! (drop-dupe authority via `SourceColor::to_color_context`, class-gated
//! so an RGB-class ICC never rides a Gray buffer). Conversions and the
//! load-bearing reduction propagate it; HDR reconstruction output is
//! deliberately context-free (no SDR profile describes linear f32).

#![cfg(feature = "zencodec")]

use std::borrow::Cow;
use zencodec::decode::{Decode as _, DecodeJob as _, DecodeOutput, DecoderConfig as _};
use zenpixels::{ChannelLayout, PixelDescriptor, PixelFormat};

fn decode_pref(data: &[u8], preferred: &[PixelDescriptor]) -> DecodeOutput {
    zenavif::AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(data), preferred)
        .expect("decoder")
        .decode()
        .expect("decode")
}

/// ICC-carrying source (paris): the buffer context holds the RGB-class
/// profile and — per the drop-dupe authority rules (ICC > nclx, MIAF) —
/// no duplicate CICP.
#[test]
fn icc_source_attaches_rgb_class_profile() {
    let data = std::fs::read("tests/vectors/libavif/paris_icc_exif_xmp.avif").expect("vector");
    let out = decode_pref(&data, &[]);
    let p = out.pixels();
    let ctx = p.color_context().expect("ICC source must yield a context");
    let icc = ctx.icc.as_deref().expect("ICC must ride the buffer");
    assert_eq!(&icc[16..20], b"RGB ", "RGB-class profile on RGB pixels");
    assert!(
        ctx.cicp.is_none(),
        "Icc authority drops the duplicate CICP (drop-dupe contract)"
    );
    // And it must match what ImageInfo reports as the source profile.
    let info_icc = out
        .info()
        .source_color
        .icc_profile
        .as_deref()
        .expect("info carries the source ICC too");
    assert_eq!(icc, info_icc);
}

/// CICP-only source (kodim03 nclx): the context carries the raw H.273
/// code points — including values the descriptor enums fold away.
#[test]
fn cicp_source_attaches_raw_code_points() {
    let data = std::fs::read("tests/vectors/libavif/kodim03_yuv420_8bpc.avif").expect("vector");
    let out = decode_pref(&data, &[]);
    let ctx = out
        .pixels()
        .color_context()
        .cloned()
        .expect("CICP source must yield a context");
    assert!(ctx.icc.is_none());
    let cicp = ctx.cicp.expect("cicp carried");
    let info_cicp = out.info().source_color.cicp.expect("info cicp");
    assert_eq!(
        (cicp.color_primaries, cicp.transfer_characteristics),
        (
            info_cicp.color_primaries,
            info_cicp.transfer_characteristics
        ),
        "buffer context must carry the same raw code points as the info"
    );
}

/// Native-gray decode: the context survives onto the Gray8 buffer
/// (CICP-only here — the fixtures carry no ICC, so nothing to class-gate).
#[test]
fn native_gray_output_carries_context() {
    let data = std::fs::read("tests/vectors/zenavif/mono_gradient_8b_full.avif").expect("fixture");
    let out = decode_pref(&data, &[]);
    let p = out.pixels();
    assert_eq!(p.descriptor().pixel_format(), PixelFormat::Gray8);
    let ctx = p.color_context().expect("gray output keeps its context");
    assert!(
        ctx.icc.is_none(),
        "no ICC in the fixture; class gate must not invent one"
    );
    assert!(ctx.cicp.is_some(), "raw CICP rides the gray buffer");
}

/// Format negotiation propagates the context through conversions
/// (to_rgba8 path on a CICP-only source).
#[test]
fn negotiated_conversion_propagates_context() {
    let data = std::fs::read("tests/vectors/libavif/kodim03_yuv420_8bpc.avif").expect("vector");
    let out = decode_pref(&data, &[PixelDescriptor::RGBA8_SRGB]);
    let p = out.pixels();
    assert_eq!(p.descriptor().pixel_format(), PixelFormat::Rgba8);
    assert!(
        p.color_context().is_some(),
        "RGB→RGBA negotiation must propagate the color context"
    );
}

/// 10-bit mono with a GRAY8 preference: the context survives the
/// Gray16 → Gray8 downscale (`downscale_to_8bit` rebuilds the buffer).
#[test]
fn gray_downscale_preserves_context() {
    let data = std::fs::read("tests/vectors/zenavif/mono_gradient_10b_full.avif").expect("fixture");
    let out = decode_pref(&data, &[PixelDescriptor::GRAY8_SRGB]);
    let p = out.pixels();
    assert_eq!(p.descriptor().pixel_format(), PixelFormat::Gray8);
    assert!(
        p.color_context().is_some(),
        "context must survive the 16→8 bit downscale rebuild"
    );
}

/// HDR reconstruction output carries a synthesized LINEAR CICP — source
/// primaries (raw code point), H.273 transfer 8, identity matrix, full
/// range. The source's SDR signaling (ICC or transfer) must not carry
/// over, but the linear output IS describable and self-description
/// shouldn't stop at the descriptor's folded enums.
#[test]
fn reconstructed_hdr_carries_linear_cicp() {
    let data = std::fs::read("tests/vectors/libavif/seine_sdr_gainmap_srgb.avif").expect("vector");
    let out = zenavif::AvifDecoderConfig::new()
        .job()
        .with_gain_map_render(zencodec::GainMapRender::ReconstructHdr {
            target_headroom: None,
        })
        .decoder(Cow::Borrowed(&data), &[])
        .expect("decoder")
        .decode()
        .expect("decode");
    let p = out.pixels();
    assert_eq!(p.descriptor().pixel_format(), PixelFormat::RgbaF32);
    let ctx = p.color_context().expect("linear CICP context");
    assert!(ctx.icc.is_none(), "no SDR ICC may ride linear f32 output");
    let cicp = ctx.cicp.expect("cicp");
    assert_eq!(cicp.transfer_characteristics, 8, "H.273 linear transfer");
    assert_eq!(cicp.matrix_coefficients, 0, "identity matrix for RGB data");
    assert!(cicp.full_range);
    let src_cp = out
        .info()
        .source_color
        .cicp
        .expect("source cicp")
        .color_primaries;
    assert_eq!(cicp.color_primaries, src_cp, "source primaries carried raw");
}

/// Streaming strips carry the same class-gated context as the buffered
/// decode — the scratch strip buffer is rebuilt per batch, so the
/// context is re-attached at emission (this was a drop bug, not a
/// design gap).
#[test]
fn streaming_strips_carry_context() {
    use zencodec::decode::{DecodeJob as _, StreamingDecode as _};

    let data = std::fs::read("tests/vectors/zenavif/mono_gradient_8b_full.avif").expect("fixture");
    let buffered_ctx = decode_pref(&data, &[])
        .pixels()
        .color_context()
        .cloned()
        .expect("buffered ctx");

    let mut dec = zenavif::AvifDecoderConfig::new()
        .job()
        .streaming_decoder(Cow::Borrowed(&data), &[])
        .expect("streaming_decoder");
    let mut strips = 0;
    while let Some((_, strip)) = dec.next_batch().expect("next_batch") {
        assert_eq!(strip.descriptor().layout(), ChannelLayout::Gray);
        let ctx = strip
            .color_context()
            .expect("every emitted strip carries the context");
        assert_eq!(ctx.cicp, buffered_ctx.cicp, "strip ctx == buffered ctx");
        assert_eq!(ctx.icc, buffered_ctx.icc);
        strips += 1;
    }
    assert!(strips > 0);
}

/// Mono file carrying an RGB-class ICC (spec-questionable but in the
/// wild): native gray is DECLINED so the profile — the most accurate
/// color description present — stays attached to pixels it validly
/// describes. A gray preference is then honestly suppressed by the
/// load-bearing ICC rules (underivable without icc-db), not faked.
#[test]
fn mono_with_rgb_class_icc_stays_rgb() {
    let data =
        std::fs::read("tests/vectors/zenavif/mono_gradient_8b_rgbicc.avif").expect("fixture");
    let out = decode_pref(&data, &[]);
    let p = out.pixels();
    assert_eq!(
        p.descriptor().pixel_format(),
        PixelFormat::Rgb8,
        "RGB-class ICC must keep the RGB layout it describes"
    );
    let ctx = p.color_context().expect("ctx");
    let icc = ctx.icc.as_deref().expect("profile rides the RGB pixels");
    assert_eq!(&icc[16..20], b"RGB ");

    // Gray preference: suppression, never a mislabeled collapse.
    let out = decode_pref(&data, &[PixelDescriptor::GRAY8_SRGB]);
    assert_ne!(
        out.pixels().descriptor().layout(),
        ChannelLayout::Gray,
        "underivable RGB-class ICC suppresses the gray collapse"
    );
}

/// Mono file with a MIAF-correct GRAY-class ICC: native gray proceeds
/// and the profile rides the gray buffer.
#[test]
fn mono_with_gray_class_icc_decodes_native_gray() {
    let data =
        std::fs::read("tests/vectors/zenavif/mono_gradient_8b_grayicc.avif").expect("fixture");
    let out = decode_pref(&data, &[]);
    let p = out.pixels();
    assert_eq!(p.descriptor().pixel_format(), PixelFormat::Gray8);
    let ctx = p.color_context().expect("ctx");
    let icc = ctx
        .icc
        .as_deref()
        .expect("GRAY-class profile rides gray pixels");
    assert_eq!(&icc[16..20], b"GRAY");
}

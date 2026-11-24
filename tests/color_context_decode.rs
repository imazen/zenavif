//! The active rendered color and the source-file claims occupy different carriers.
use std::borrow::Cow;
use zencodec::decode::{Decode as _, DecodeJob as _, DecodeOutput, DecoderConfig as _};
use zenpixels::{ChannelOrder, ColorSpace, ImageSpec, OriginKind, SampleLayout, Transfer};

fn decode_pref(data: &[u8], preferred: &[ImageSpec]) -> DecodeOutput {
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
    let p = out.buffer();
    let ColorSpace::Icc { profile, .. } = &p.spec().color else {
        panic!("ICC source must drive active pixel color");
    };
    let icc = profile.as_ref();
    assert_eq!(&icc[16..20], b"RGB ", "RGB-class profile on RGB pixels");
    assert!(
        p.spec().color.to_cicp().is_none(),
        "active ICC has no second color authority"
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
    let ctx = out.buffer().origin();
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
    let p = out.buffer();
    assert_eq!(p.spec().layout, SampleLayout::GRAY8);
    let ctx = p.origin();
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
    let out = decode_pref(&data, &[ImageSpec::rgba8_srgb()]);
    let p = out.buffer();
    assert_eq!(p.spec().layout, SampleLayout::RGBA8);
    assert!(
        p.origin().kind != OriginKind::Assumed,
        "RGB→RGBA negotiation must propagate the color context"
    );
}

/// 10-bit mono with a GRAY8 preference: the context survives the
/// Gray16 → Gray8 downscale (`downscale_to_8bit` rebuilds the buffer).
#[test]
fn gray_downscale_preserves_context() {
    let data = std::fs::read("tests/vectors/zenavif/mono_gradient_10b_full.avif").expect("fixture");
    let out = decode_pref(&data, &[ImageSpec::gray8_srgb()]);
    let p = out.buffer();
    assert_eq!(p.spec().layout, SampleLayout::GRAY8);
    assert!(
        p.origin().kind != OriginKind::Assumed,
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
    let p = out.buffer();
    assert_eq!(p.spec().layout, SampleLayout::RGBAF32);
    assert!(
        !matches!(p.spec().color, ColorSpace::Icc { .. }),
        "no SDR ICC may describe linear f32 output"
    );
    let cicp = p.spec().color.to_cicp().expect("active linear CICP");
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
    let buffered = decode_pref(&data, &[]);
    let spec = buffered.buffer().spec();

    let mut dec = zenavif::AvifDecoderConfig::new()
        .job()
        .streaming_decoder(Cow::Borrowed(&data), &[])
        .expect("streaming_decoder");
    let mut strips = 0;
    while let Some((_, strip)) = dec.next_batch().expect("next_batch") {
        assert_eq!(strip.spec().layout.order, ChannelOrder::Gray);
        assert_eq!(strip.spec(), spec, "streaming active color equals buffered");
        assert_eq!(strip.hdr(), buffered.buffer().hdr());
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
    let p = out.buffer();
    assert_eq!(
        p.spec().layout,
        SampleLayout::RGB8,
        "RGB-class ICC must keep the RGB layout it describes"
    );
    let ColorSpace::Icc { profile: icc, .. } = &p.spec().color else {
        panic!("profile rides the RGB pixels");
    };
    assert_eq!(&icc[16..20], b"RGB ");

    // Gray preference: suppression, never a mislabeled collapse.
    let out = decode_pref(&data, &[ImageSpec::gray8_srgb()]);
    assert_ne!(
        out.pixels().spec().layout.order,
        ChannelOrder::Gray,
        "underivable RGB-class ICC suppresses the gray collapse"
    );
}

/// Mono file with a REAL, identifiable RGB-class profile (Display P3 —
/// the common in-the-wild pairing): native gray proceeds, the profile's
/// CICP is derived (P3-D65 primaries + sRGB transfer — white point and
/// transfer remain fully meaningful for single-channel data), and the
/// gray buffer gets a CICP-only context. No RGB expansion needed to
/// honor the profile.
#[test]
fn mono_with_derivable_rgb_icc_decodes_native_gray_with_cicp() {
    let data = std::fs::read("tests/vectors/zenavif/mono_gradient_8b_p3icc.avif").expect("fixture");
    let out = decode_pref(&data, &[]);
    let p = out.buffer();
    assert_eq!(
        p.spec().layout,
        SampleLayout::GRAY8,
        "derivable RGB profile must not force RGB expansion"
    );
    assert!(matches!(
        p.spec().color,
        ColorSpace::Gray {
            transfer: Transfer::Srgb,
            ..
        }
    ));
    let original = p
        .origin()
        .icc
        .as_deref()
        .expect("keeps original ICC in provenance");
    let cicp = zenpixels::icc::IccIdentification::from_icc(original)
        .unwrap()
        .to_cicp()
        .unwrap();
    assert_eq!(
        (cicp.color_primaries, cicp.transfer_characteristics),
        (12, 13),
        "Display P3 identification: P3-D65 primaries + sRGB transfer"
    );
    // The source profile stays available for provenance on the info.
    assert!(out.info().source_color.icc_profile.is_some());
}

/// Mono file with a MIAF-correct GRAY-class ICC: native gray proceeds
/// and the profile rides the gray buffer.
#[test]
fn mono_with_gray_class_icc_decodes_native_gray() {
    let data =
        std::fs::read("tests/vectors/zenavif/mono_gradient_8b_grayicc.avif").expect("fixture");
    let out = decode_pref(&data, &[]);
    let p = out.buffer();
    assert_eq!(p.spec().layout, SampleLayout::GRAY8);
    let ColorSpace::Icc {
        profile: icc,
        channels: 1,
        ..
    } = &p.spec().color
    else {
        panic!("GRAY-class profile rides gray pixels");
    };
    assert_eq!(&icc[16..20], b"GRAY");
}

#[test]
fn shared_png_encoder_reads_avif_pixel_view_without_repacking() {
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};

    let data = std::fs::read("tests/vectors/libavif/kodim03_yuv420_8bpc.avif").unwrap();
    let decoded = decode_pref(&data, &[]);
    let input = decoded.pixels();
    let encoded = zenpng::PngEncoderConfig::default()
        .job()
        .encoder()
        .unwrap()
        .encode(input.clone())
        .unwrap();
    let mut png = png::Decoder::new(std::io::Cursor::new(encoded.data()))
        .read_info()
        .unwrap();
    let mut bytes = vec![0; png.output_buffer_size().unwrap()];
    let info = png.next_frame(&mut bytes).unwrap();
    assert_eq!((info.width, info.height), (input.width(), input.rows()));
    assert_eq!(
        &bytes[..info.buffer_size()],
        input.try_contiguous_bytes().unwrap().as_ref()
    );
}

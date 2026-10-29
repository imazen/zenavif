//! A still encode writes upright pixels and orientation 1 rather than
//! `irot`/`imir`: several readers ignore `irot`, and `imir`'s axis reads
//! opposite ways across HEIF editions (imazen/zencodec#132). Checked pixel by
//! pixel for every orientation, through the lossless mode.

#![cfg(all(feature = "encode", feature = "encode-imazen"))]

use std::borrow::Cow;

use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
use zencodec::{Metadata, MetadataPolicy, Orientation};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 7;
const H: u32 = 4;

fn source() -> Vec<u8> {
    (0..W * H)
        .flat_map(|i| [(i * 9) as u8, (255 - i * 7) as u8, 100])
        .collect()
}

#[allow(deprecated)] // the only EXIF writer zencodec 0.1.26 has
fn exif_with(o: Orientation) -> Vec<u8> {
    let mut e = zencodec::exif::Exif::new(zencodec::exif::TextEncoding::Ascii);
    e.set_orientation(o);
    e.to_bytes()
}

#[test]
fn stills_are_written_upright_with_orientation_1() {
    let px = source();
    for o in [
        Orientation::FlipH,
        Orientation::Rotate180,
        Orientation::FlipV,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ] {
        let xmp = format!(r#"<x tiff:Orientation="{}"/>"#, o.to_exif());
        let meta = Metadata::none()
            .with_orientation(o)
            .with_exif(exif_with(o))
            .with_xmp(xmp.into_bytes());
        let slice = PixelSlice::new(&px, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap();
        let bytes = zenavif::AvifEncoderConfig::new()
            .with_lossless(true)
            .job()
            .with_metadata_policy(meta, MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap()
            .encode(slice)
            .unwrap()
            .into_vec();

        // Preserve (the default) reports what the file says, unbaked.
        let out = zenavif::AvifDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(&bytes), &[PixelDescriptor::RGB8_SRGB])
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(
            out.info().orientation,
            Orientation::Identity,
            "{o:?}: no irot/imir"
        );
        let (dw, dh) = o.output_dimensions(W, H);
        let p = out.pixels();
        assert_eq!((p.width(), p.rows()), (dw, dh), "{o:?}: upright dims");
        for y in 0..H {
            for x in 0..W {
                let (dx, dy) = o.forward_map(x, y, W, H);
                let s = ((y * W + x) * 3) as usize;
                let row = p.row(dy);
                let d = dx as usize * 3;
                // zenrav1e's lossless doesn't reach qindex 0 yet (zenrav1e#9:
                // |delta| <= 2); neighbouring source pixels differ by >= 7, so
                // any misplaced pixel still fails.
                let worst = row[d..d + 3]
                    .iter()
                    .zip(&px[s..s + 3])
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(worst <= 2, "{o:?}: pixel ({x},{y}) off by {worst}");
            }
        }
        let meta = out.metadata();
        assert_eq!(
            zencodec::helpers::parse_exif_orientation(meta.exif.as_deref().expect("EXIF kept")),
            Some(Orientation::Identity),
            "{o:?}: EXIF tag rewritten"
        );
        let xmp = meta.xmp.expect("XMP kept");
        assert_eq!(
            &*xmp, br#"<x tiff:Orientation="1"/>"#,
            "{o:?}: XMP rewritten"
        );
    }
}

// Explicit representation is independent of the compatibility default above.
#[test]
fn explicit_stored_orientation_keeps_samples_and_geometry_typed_and_erased() {
    use zencodec::encode::EncodeOrientation;
    let px = source();
    let stride = W as usize * 3 + 6;
    let mut padded = vec![231; stride * H as usize];
    for y in 0..H as usize {
        padded[y * stride..y * stride + W as usize * 3]
            .copy_from_slice(&px[y * W as usize * 3..(y + 1) * W as usize * 3]);
    }
    let slice = PixelSlice::new(&padded, W, H, stride, PixelDescriptor::RGB8_SRGB).unwrap();
    let mut baseline = None;
    for o in [
        Orientation::Identity,
        Orientation::FlipH,
        Orientation::Rotate180,
        Orientation::FlipV,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ] {
        let meta = Metadata::none().with_orientation(o).with_exif(exif_with(o));
        let mut config = zenavif::AvifEncoderConfig::new().with_lossless(true);
        // An explicit common request must replace conflicting native overrides,
        // including when metadata requests identity and therefore has no boxes.
        *config.inner_mut() = config.inner().clone().rotation(2).mirror(1);
        // Set the mode before metadata: subsequent metadata must retain it.
        let typed = config
            .clone()
            .job()
            .try_with_orientation_mode(EncodeOrientation::PreserveStored)
            .unwrap()
            .with_metadata_policy(meta.clone(), MetadataPolicy::PreserveExact)
            .encoder()
            .unwrap()
            .encode(slice.clone())
            .unwrap()
            .into_vec();
        let mut erased = config.job().into_dyn_job();
        erased.set_metadata_policy(meta, MetadataPolicy::PreserveExact);
        erased
            .try_set_orientation_mode(EncodeOrientation::PreserveStored)
            .unwrap();
        let dynamic = erased
            .into_encoder()
            .unwrap()
            .encode(slice.clone())
            .unwrap()
            .into_vec();
        assert_eq!(
            typed, dynamic,
            "{o:?}: erasure preserves exact configured operation"
        );
        let codecs = zencodec::CodecSet::new()
            .with_send_encoder(zenavif::AvifEncoderConfig::new().with_lossless(true));
        let request = zencodec::operation::EncodeRequest::new(zencodec::ImageFormat::Avif)
            .with_orientation_mode(EncodeOrientation::PreserveStored);
        let mut regular = request.prepare(&codecs).unwrap().pop().unwrap().into_job();
        regular.set_metadata_policy(
            Metadata::none().with_orientation(o).with_exif(exif_with(o)),
            MetadataPolicy::PreserveExact,
        );
        assert_eq!(
            regular
                .into_encoder()
                .unwrap()
                .encode(slice.clone())
                .unwrap()
                .into_vec(),
            typed
        );
        let mut send = request
            .prepare_send(&codecs)
            .unwrap()
            .pop()
            .unwrap()
            .into_job();
        send.set_metadata_policy(
            Metadata::none().with_orientation(o).with_exif(exif_with(o)),
            MetadataPolicy::PreserveExact,
        );
        assert_eq!(
            send.into_encoder()
                .unwrap()
                .encode(slice.clone())
                .unwrap()
                .into_vec(),
            typed
        );
        let out = zenavif::AvifDecoderConfig::new()
            .job()
            .decoder(Cow::Borrowed(&typed), &[PixelDescriptor::RGB8_SRGB])
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(out.info().orientation, o);
        assert_eq!((out.pixels().width(), out.pixels().rows()), (W, H));
        let bytes: Vec<u8> = (0..H).flat_map(|y| out.pixels().row(y).to_vec()).collect();
        if let Some(expected) = &baseline {
            assert_eq!(&bytes, expected, "{o:?}: stored samples changed");
        } else {
            baseline = Some(bytes);
        }
        let parsed = zenavif_parse::AvifParser::from_bytes(&typed).unwrap();
        let coded = parsed.primary_metadata().unwrap();
        assert_eq!(
            (coded.max_frame_width.get(), coded.max_frame_height.get()),
            (W, H)
        );
    }
}

#[test]
fn explicit_bake_retains_default_still_bytes_and_refuses_animation() {
    use zencodec::encode::EncodeOrientation;
    let px = source();
    let slice = PixelSlice::new(&px, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap();
    let config = zenavif::AvifEncoderConfig::new().with_lossless(true);
    let meta = Metadata::none().with_orientation(Orientation::Rotate90);
    let expected = config
        .clone()
        .job()
        .with_metadata_policy(meta.clone(), MetadataPolicy::PreserveExact)
        .encoder()
        .unwrap()
        .encode(slice.clone())
        .unwrap()
        .into_vec();
    let actual = config
        .clone()
        .job()
        .with_metadata_policy(meta, MetadataPolicy::PreserveExact)
        .try_with_orientation_mode(EncodeOrientation::Bake)
        .unwrap()
        .encoder()
        .unwrap()
        .encode(slice.clone())
        .unwrap()
        .into_vec();
    assert_eq!(actual, expected);
    let result = config
        .job()
        .try_with_orientation_mode(EncodeOrientation::Bake)
        .unwrap()
        .animation_frame_encoder();
    let error = match result {
        Ok(_) => panic!("animation baking is unsupported"),
        Err(error) => error,
    };
    assert_eq!(
        zencodec::find_cause::<zencodec::UnsupportedOperation>(&error),
        Some(&zencodec::UnsupportedOperation::EncodeOrientation)
    );
}

#[test]
fn retained_orientation_plan_matches_native_start_and_bytes() {
    use zencodec::encode::{EncodeDeclaration, EncodeOrientation, OrientationPlan};
    let pixels = source();
    let config = zenavif::AvifEncoderConfig::new().with_lossless(true);
    for orientation in [
        Orientation::Identity,
        Orientation::FlipH,
        Orientation::Rotate180,
        Orientation::FlipV,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ] {
        for mode in [EncodeOrientation::PreserveStored, EncodeOrientation::Bake] {
            let job = || {
                config
                    .clone()
                    .job()
                    .with_metadata_policy(
                        Metadata::none().with_orientation(orientation),
                        MetadataPolicy::PreserveExact,
                    )
                    .try_with_orientation_mode(mode)
                    .unwrap()
            };
            let declaration = EncodeDeclaration::new(W, H, PixelDescriptor::RGB8_SRGB);
            let expected = match mode {
                EncodeOrientation::PreserveStored => OrientationPlan::preserved(orientation),
                EncodeOrientation::Bake => OrientationPlan::baked(orientation),
                _ => unreachable!(),
            };
            let retained = job();
            let plan = retained.plan_encode(&declaration).unwrap();
            assert_eq!(plan.orientation, Some(expected));
            let mut encoder = retained.encoder().unwrap();
            let started = encoder.begin_declared(&declaration).unwrap();
            assert_eq!(started, plan);
            let slice = || {
                PixelSlice::new(&pixels, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap()
            };
            let actual = encoder.encode(slice()).unwrap().into_vec();
            let ordinary = job().encoder().unwrap().encode(slice()).unwrap().into_vec();
            assert_eq!(actual, ordinary, "{orientation:?} {mode:?}");
            let decoded = zenavif::AvifDecoderConfig::new()
                .job()
                .decoder(Cow::Borrowed(&actual), &[PixelDescriptor::RGB8_SRGB])
                .unwrap()
                .decode()
                .unwrap();
            assert_eq!(
                (decoded.pixels().width(), decoded.pixels().rows()),
                expected.stored_dimensions(W, H)
            );
            assert_eq!(decoded.info().orientation, expected.output);
        }
    }
}

#[test]
fn declared_encoding_refuses_changed_geometry_and_descriptor() {
    use zencodec::encode::{EncodeDeclaration, EncodePlanError};
    let declaration = EncodeDeclaration::new(W, H, PixelDescriptor::RGB8_SRGB);
    for (width, height, descriptor, channels) in [
        (W - 1, H, PixelDescriptor::RGB8_SRGB, 3),
        (W, H - 1, PixelDescriptor::RGB8_SRGB, 3),
        (W, H, PixelDescriptor::RGBA8_SRGB, 4),
    ] {
        let mut encoder = zenavif::AvifEncoderConfig::new().job().encoder().unwrap();
        encoder.begin_declared(&declaration).unwrap();
        let pixels = vec![0; (width * height * channels) as usize];
        let slice = PixelSlice::new(
            &pixels,
            width,
            height,
            (width * channels) as usize,
            descriptor,
        )
        .unwrap();
        let error = encoder.encode(slice).unwrap_err();
        assert_eq!(
            zencodec::find_cause::<EncodePlanError>(&error),
            Some(&EncodePlanError::Changed)
        );
    }
}

#[test]
fn unsupported_declarations_refuse_without_poisoning_startup() {
    use zencodec::encode::{EncodeDeclaration, EncodePlanError};
    let valid = EncodeDeclaration::new(W, H, PixelDescriptor::RGB8_SRGB);
    let context = zenpixels::ColorContext::from_icc(vec![42; 128]);
    let unsupported = [
        valid.with_color(&context),
        EncodeDeclaration::new(W, H, PixelDescriptor::RGB16_SRGB),
    ];
    for declaration in unsupported {
        let job = zenavif::AvifEncoderConfig::new().job();
        let error = job.plan_encode(&declaration).unwrap_err();
        assert_eq!(
            zencodec::find_cause::<zencodec::UnsupportedOperation>(error.as_ref()),
            Some(&zencodec::UnsupportedOperation::EncodePlanning)
        );
        let mut encoder = job.encoder().unwrap();
        let error = encoder.begin_declared(&declaration).unwrap_err();
        assert_eq!(
            zencodec::find_cause::<zencodec::UnsupportedOperation>(&error),
            Some(&zencodec::UnsupportedOperation::EncodePlanning)
        );
        encoder.begin_declared(&valid).unwrap();
        let error = encoder.begin_declared(&valid).unwrap_err();
        assert_eq!(
            zencodec::find_cause::<EncodePlanError>(&error),
            Some(&EncodePlanError::Changed)
        );
        let pixels = source();
        encoder
            .encode(
                PixelSlice::new(&pixels, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap(),
            )
            .unwrap();
    }
}

#[test]
fn preflight_keeps_resource_category_and_original_limit() {
    use zencodec::encode::EncodeDeclaration;
    use zencodec::{
        CategorizedError as _, ErrorCategory, LimitExceeded, LimitKind, ResourceError,
        ResourceLimits,
    };
    let declaration = EncodeDeclaration::new(W, H, PixelDescriptor::RGB8_SRGB);
    let job = zenavif::AvifEncoderConfig::new()
        .job()
        .with_limits(ResourceLimits::none().with_max_width(W - 1));
    let error = job.plan_encode(&declaration).unwrap_err();
    let envelope = zencodec::find_cause::<zencodec::CodecError>(error.as_ref()).unwrap();
    assert_eq!(
        envelope.category(),
        ErrorCategory::Resource(ResourceError::Limits(LimitKind::Width))
    );
    assert_eq!(
        zencodec::find_cause::<LimitExceeded>(error.as_ref()),
        Some(&LimitExceeded::Width {
            actual: W,
            max: W - 1
        })
    );
    let mut encoder = job.encoder().unwrap();
    let error = encoder.begin_declared(&declaration).unwrap_err();
    assert_eq!(
        error.category(),
        ErrorCategory::Resource(ResourceError::Limits(LimitKind::Width))
    );
    assert_eq!(
        zencodec::find_cause::<LimitExceeded>(&error),
        Some(&LimitExceeded::Width {
            actual: W,
            max: W - 1
        })
    );
}

#[test]
fn exact_depth_and_late_color_are_not_silently_ignored() {
    use zencodec::encode::{EncodeDeclaration, EncodePlanError};
    use zenpixels::{ChannelType, SignalRange, sample::ExactDepth};
    let valid = EncodeDeclaration::new(W, H, PixelDescriptor::RGB8_SRGB);
    let claimed =
        valid.with_exact_depth(ExactDepth::new(ChannelType::U8, SignalRange::Full, 8, 8).unwrap());
    let job = zenavif::AvifEncoderConfig::new().job();
    assert!(job.plan_encode(&claimed).is_err());
    let mut encoder = job.encoder().unwrap();
    let error = encoder.begin_declared(&claimed).unwrap_err();
    assert_eq!(
        zencodec::find_cause::<zencodec::UnsupportedOperation>(&error),
        Some(&zencodec::UnsupportedOperation::EncodePlanning)
    );
    encoder.begin_declared(&valid).unwrap();
    let pixels = source();
    let input = PixelSlice::new(&pixels, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB)
        .unwrap()
        .with_color_context(std::sync::Arc::new(zenpixels::ColorContext::from_icc(
            vec![42; 128],
        )));
    let error = encoder.encode(input).unwrap_err();
    assert_eq!(
        zencodec::find_cause::<EncodePlanError>(&error),
        Some(&EncodePlanError::Changed)
    );
}

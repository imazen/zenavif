//! Run in this standalone workspace to avoid feature unification with PNG dev dependencies.

#[path = "../../../../tests/support/independent_av1.rs"]
mod independent;
#[path = "../../../../tests/support/icc_profiles.rs"]
mod profiles;

use std::{borrow::Cow, path::Path};
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
use zenpixels::{Cicp, ColorSpace, IccColorSpace, ImageSpec, SampleLayout, Transfer};
use zenpixels_convert::icc_profiles::{DISPLAY_P3_V4, SynthesizedIcc};

fn check(profile: &[u8], class: IccColorSpace, expected: &str) {
    assert_eq!(IccColorSpace::from_icc(profile), Some(class));
    assert_eq!(&profile[36..40], b"acsp");
    let input = zenavif_serialize::Aviffy::new()
        .set_icc_profile(profile.to_vec())
        .try_to_vec(independent::MONO_RAMP, None, 16, 16, 8)
        .unwrap();
    let result = zenavif::AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(&input), &[ImageSpec::GRAY8_SRGB])
        .unwrap()
        .decode()
        .unwrap();
    let view = result.pixels();
    let origin = view.origin().unwrap();
    assert_eq!(origin.icc.as_deref(), Some(profile));
    assert_eq!(origin.cicp, Some(Cicp::new(2, 2, 2, true)));
    assert_eq!(
        result.info().source_color.icc_profile.as_deref(),
        Some(profile)
    );
    match (expected, &view.spec().color) {
        (
            "rgb",
            ColorSpace::Icc {
                profile: active,
                channels: 3,
                ..
            },
        ) => {
            assert_eq!(active.as_ref(), profile);
            assert_eq!(view.spec().layout, SampleLayout::RGB8);
        }
        (
            "gray-original",
            ColorSpace::Icc {
                profile: active,
                channels: 1,
                ..
            },
        ) => {
            assert_eq!(IccColorSpace::from_icc(active), Some(IccColorSpace::Gray));
            assert_eq!(active.as_ref(), profile);
        }
        (
            "gray-synthesized",
            ColorSpace::Icc {
                profile: active,
                channels: 1,
                ..
            },
        ) => {
            assert_eq!(IccColorSpace::from_icc(active), Some(IccColorSpace::Gray));
            assert_ne!(active.as_ref(), profile);
        }
        (
            "pq-fallback",
            ColorSpace::Gray {
                transfer: Transfer::Pq,
                ..
            },
        )
        | (
            "srgb-fallback",
            ColorSpace::Gray {
                transfer: Transfer::Srgb,
                ..
            },
        ) => {}
        (_, wrong) => panic!("expected {expected}, got {wrong:?}"),
    }
    let db = cfg!(feature = "icc-db");
    for y in 0..16 {
        if expected == "rgb" {
            let want = (0..16).map(|x| [(16 * y + x) as u8; 3]).collect::<Vec<_>>();
            assert_eq!(view.row_as::<[u8; 3]>(y).unwrap(), want);
        } else {
            assert_eq!(view.spec().layout, SampleLayout::GRAY8);
            let want = (0..16).map(|x| (16 * y + x) as u8).collect::<Vec<_>>();
            assert_eq!(view.row(y), want);
        }
    }
    println!("icc-db={db} {class:?} → {expected}, 256 exact codes and exact origin");
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .expect("pass export-pq or verify, followed by a profile path");
    let profile_path = args.next().expect("pass a profile path");
    assert!(args.next().is_none());
    let path = Path::new(&profile_path);
    if mode == "export-pq" {
        let SynthesizedIcc::Profile(rgb) = SynthesizedIcc::for_cicp(Cicp::BT2100_PQ_RGB) else {
            panic!("export-pq requires icc-db");
        };
        std::fs::write(path, rgb.as_ref()).unwrap();
        return;
    }
    assert_eq!(mode, "verify");
    let pq = std::fs::read(path).unwrap();
    let db = cfg!(feature = "icc-db");
    check(
        &pq,
        IccColorSpace::Rgb,
        if db {
            "gray-synthesized"
        } else {
            "pq-fallback"
        },
    );
    check(
        DISPLAY_P3_V4,
        IccColorSpace::Rgb,
        if db {
            "gray-synthesized"
        } else {
            "srgb-fallback"
        },
    );
    check(profiles::SRGB, IccColorSpace::Rgb, "srgb-fallback");
    check(profiles::GRAY, IccColorSpace::Gray, "gray-original");
    check(profiles::UNIDENTIFIED_RGB, IccColorSpace::Rgb, "rgb");
}

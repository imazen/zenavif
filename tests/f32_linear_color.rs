//! f32 input that isn't PQ/HLG is stored as 8-bit sRGB. The file must say
//! sRGB: an nclx (or a profile synthesized from one) that kept the input's
//! "linear" makes readers decode the sRGB samples as linear.

#![cfg(feature = "encode")]

use std::borrow::Cow;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
use zencodec::{Cicp, Metadata, MetadataPolicy};
use zenpixels::{PixelDescriptor, PixelSlice};

fn encode(meta: Option<Metadata>) -> Vec<u8> {
    let (w, h) = (8u32, 8u32);
    let floats: Vec<f32> = (0..w * h * 3).map(|i| (i % 7) as f32 / 7.0).collect();
    let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let slice = PixelSlice::new(&bytes, w, h, w as usize * 12, PixelDescriptor::RGBF32_LINEAR)
        .unwrap();
    let mut job = zenavif::AvifEncoderConfig::new()
        .with_generic_quality(90.0)
        .with_generic_effort(10)
        .job();
    if let Some(m) = meta {
        job = job.with_metadata_policy(m, MetadataPolicy::PreserveExact);
    }
    job.encoder().unwrap().encode(slice).unwrap().into_vec()
}

fn declared(bytes: &[u8]) -> (Option<Cicp>, Option<zenpixels::icc::IccIdentification>) {
    let out = zenavif::AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(bytes), &[])
        .unwrap()
        .decode()
        .unwrap();
    let c = &out.info().source_color;
    (
        c.cicp,
        c.icc_profile
            .as_deref()
            .and_then(zenpixels::icc::identify_common),
    )
}

#[test]
fn f32_linear_input_is_declared_srgb() {
    for (how, meta) in [
        ("no metadata", None),
        (
            "linear P3 CICP",
            Some(Metadata::none().with_cicp(Cicp::new(12, 8, 0, true))),
        ),
        (
            "linear BT.709 CICP",
            Some(Metadata::none().with_cicp(Cicp::new(1, 8, 0, true))),
        ),
    ] {
        let (cicp, icc) = declared(&encode(meta.clone()));
        let cicp = cicp.expect("nclx");
        assert_eq!(cicp.transfer_characteristics, 13, "{how}: nclx transfer");
        let want_primaries = meta.and_then(|m| m.cicp).map_or(1, |c| c.color_primaries);
        assert_eq!(cicp.color_primaries, want_primaries, "{how}: primaries kept");
        if let Some(id) = icc {
            assert_eq!(
                id.to_cicp().map(|c| c.transfer_characteristics),
                Some(13),
                "{how}: a profile next to the nclx says sRGB too"
            );
        }
    }
}

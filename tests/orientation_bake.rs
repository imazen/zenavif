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

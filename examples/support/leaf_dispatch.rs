//! Bounded committed fixtures, parsing and real streaming decode consumers.
#![forbid(unsafe_code)]
use enough::Unstoppable;
use std::{
    borrow::Cow,
    io::{Cursor, Read},
};
use zencodec::decode::{DecodeJob as _, DecoderConfig as _, StreamingDecode as _};

pub fn fixtures() -> [(&'static str, &'static [u8]); 4] {
    [
        (
            "photo420",
            include_bytes!("../../tests/vectors/libavif/kodim03_yuv420_8bpc.avif"),
        ),
        (
            "pixi",
            include_bytes!("../../tests/vectors/libavif/extended_pixi.avif"),
        ),
        (
            "orientation",
            include_bytes!("../../tests/vectors/libavif/clap_irot_imir_non_essential.avif"),
        ),
        (
            "animation_metadata",
            include_bytes!("../../zenavif-parse/tests/colors-animated-8bpc-alpha-exif-xmp.avif"),
        ),
    ]
}

pub struct ShortReader<'a>(pub &'a [u8]);
impl Read for ShortReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = out.len().min(3).min(self.0.len());
        out[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

pub fn parse(data: &[u8], kind: usize) -> zenavif_parse::AvifParser<'static> {
    match kind {
        0 => zenavif_parse::AvifParser::from_reader(&mut Cursor::new(data)).unwrap(),
        1 => zenavif_parse::AvifParser::from_reader(&mut &*data).unwrap(),
        _ => zenavif_parse::AvifParser::from_reader(&mut ShortReader(data)).unwrap(),
    }
}

pub fn metadata(p: &zenavif_parse::AvifParser<'_>) -> Vec<u8> {
    use std::fmt::Write as _;
    let mut s = String::new();
    macro_rules! field {
        ($e:expr) => {
            writeln!(s, "{}={:?}", stringify!($e), $e).unwrap()
        };
    }
    field!(p.major_brand());
    field!(p.compatible_brands());
    field!(p.primary_metadata());
    field!(p.alpha_metadata());
    field!(p.spatial_extents());
    field!(p.av1_config());
    field!(p.nclx_color_info());
    field!(p.color_info());
    field!(p.rotation());
    field!(p.mirror());
    field!(p.clean_aperture());
    field!(p.pixel_aspect_ratio());
    field!(p.content_light_level());
    field!(p.mastering_display());
    field!(p.content_colour_volume());
    field!(p.ambient_viewing());
    field!(p.operating_point());
    field!(p.layer_selector());
    field!(p.layered_image_indexing());
    field!(p.grid_config());
    field!(p.animation_info());
    field!(p.premultiplied_alpha());
    field!(p.exif());
    field!(p.xmp());
    field!(p.primary_data());
    field!(p.alpha_data());
    for i in 0..p.grid_tile_count() {
        field!(p.tile_data(i));
    }
    s.into_bytes()
}

pub fn decode(data: &[u8]) -> Vec<u8> {
    zenavif::decode_with(
        data,
        &zenavif::DecoderConfig::new().threads(1),
        &Unstoppable,
    )
    .unwrap()
    .copy_to_contiguous_bytes()
}

pub fn stream(data: &[u8]) -> Vec<u8> {
    let mut decoder = zenavif::AvifDecoderConfig::new()
        .with_threads(1)
        .job()
        .streaming_decoder(Cow::Borrowed(data), &[])
        .unwrap();
    let mut bytes = Vec::new();
    let mut rows = 0;
    while let Some((y, strip)) = decoder.next_batch().unwrap() {
        assert_eq!(y, rows);
        for row in 0..strip.rows() {
            bytes.extend_from_slice(strip.row(row));
        }
        rows += strip.rows();
    }
    assert_eq!(rows, decoder.info().height);
    bytes
}

#[cfg(feature = "_dev")]
pub fn yuv_witnesses() -> Vec<(String, Vec<u8>)> {
    use rgb::Rgba;
    use zenavif::yuv_convert::{self as yuv, YuvMatrix, YuvRange};
    let mut cases = Vec::new();
    for width in [1usize, 7, 8, 9, 15, 16, 17, 31, 32, 33, 65, 257] {
        let height = 17usize;
        for sampling in [420, 422, 444] {
            let cw = if sampling == 444 {
                width
            } else {
                width.div_ceil(2)
            };
            let ch = if sampling == 420 {
                height.div_ceil(2)
            } else {
                height
            };
            let ys = width + 7;
            let us = cw + 5;
            let vs = cw + 9;
            let plane = |n: usize, seed: usize| {
                (0..n)
                    .map(|i| ((i * 71 + seed) % 256) as u8)
                    .collect::<Vec<_>>()
            };
            let yp = plane(ys * height, 19);
            let up = plane(us * ch, 37);
            let vp = plane(vs * ch, 91);
            for range in [YuvRange::Full, YuvRange::Limited] {
                for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
                    let convert = |start, rows, out: &mut [Rgba<u8>]| match sampling {
                        420 => yuv::yuv420_to_rgba8_strip(
                            &yp, ys, &up, us, &vp, vs, width, height, start, rows, range, matrix,
                            out,
                        ),
                        422 => yuv::yuv422_to_rgba8_strip(
                            &yp, ys, &up, us, &vp, vs, width, start, rows, range, matrix, out,
                        ),
                        _ => yuv::yuv444_to_rgba8_strip(
                            &yp, ys, &up, us, &vp, vs, width, start, rows, range, matrix, out,
                        ),
                    };
                    let mut full = vec![Rgba::default(); width * height];
                    convert(0, height, &mut full);
                    let mut split = vec![Rgba::default(); width * height];
                    for start in (0..height).step_by(3) {
                        let rows = (height - start).min(3);
                        convert(start, rows, &mut split[start * width..][..rows * width]);
                    }
                    assert_eq!(
                        full, split,
                        "strip boundary {sampling}/{width}/{range:?}/{matrix:?}"
                    );
                    cases.push((
                        format!("yuv-{sampling}-{width}-{range:?}-{matrix:?}"),
                        bytemuck::cast_slice(&full).to_vec(),
                    ));
                }
            }
        }
    }
    cases
}

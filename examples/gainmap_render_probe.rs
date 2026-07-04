//! Probe GainMapRender::ReconstructHdr behavior across gain-map vector
//! classes (SDR-base forward, HDR-base backward, small map, ICC base).
//! Prints peak linear values and CLL at several target headrooms.
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
use zencodec::gainmap::GainMapSource;

fn peak_linear(pixels: &zenpixels::PixelSlice<'_>) -> f32 {
    assert_eq!(
        pixels.descriptor().pixel_format(),
        zenpixels::PixelFormat::RgbaF32
    );
    let stride = pixels.stride();
    let bytes = pixels.as_strided_bytes();
    let (w, h) = (pixels.width() as usize, pixels.rows() as usize);
    let mut peak = f32::MIN;
    for y in 0..h {
        let row: &[f32] = rgb::bytemuck::cast_slice(&bytes[y * stride..][..w * 16]);
        for px in row.chunks_exact(4) {
            assert!(px.iter().all(|v| v.is_finite()), "non-finite pixel");
            peak = peak.max(px[0].max(px[1]).max(px[2]));
        }
    }
    peak
}

fn probe(name: &str) {
    let path = format!("tests/vectors/libavif/{name}");
    let data = std::fs::read(&path).expect("vector");
    for th in [Some(1.0f32), Some(2.0), None] {
        let out = zenavif::AvifDecoderConfig::new()
            .job()
            .with_gain_map_render(zencodec::GainMapRender::ReconstructHdr {
                target_headroom: th,
            })
            .decoder(std::borrow::Cow::Borrowed(&data), &[])
            .and_then(|d| d.decode());
        match out {
            Ok(o) => {
                let peak = peak_linear(&o.pixels());
                let cll = o
                    .info()
                    .source_color
                    .content_light_level
                    .map(|c| c.max_content_light_level)
                    .unwrap_or(0);
                let gm = o.extras::<GainMapSource>();
                let (bh, ah, bd) = gm
                    .map(|g| {
                        (
                            g.metadata.params.base_hdr_headroom,
                            g.metadata.params.alternate_hdr_headroom,
                            g.metadata.params.backward_direction,
                        )
                    })
                    .unwrap_or((-1.0, -1.0, false));
                println!(
                    "{name} th={th:?}: peak={peak:.4} cll={cll} base_hr={bh:.3} alt_hr={ah:.3} backward={bd}"
                );
            }
            Err(e) => println!("{name} th={th:?}: ERROR {e}"),
        }
    }
}

fn main() {
    probe("seine_sdr_gainmap_srgb.avif");
    probe("seine_hdr_gainmap_srgb.avif");
    probe("seine_hdr_gainmap_small_srgb.avif");
    probe("seine_sdr_gainmap_srgb_icc.avif");
    probe("seine_sdr_gainmap_gammazero.avif");
}

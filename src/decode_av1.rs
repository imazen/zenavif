//! Raw AV1 OBU bitstream decoding
//!
//! Decodes raw AV1 OBU byte sequences to pixels without requiring an AVIF
//! container. This is used for AVIF gain map images, which are stored as
//! raw AV1 bitstreams inside the AVIF container but outside the normal
//! primary/alpha item structure.

#![deny(unsafe_code)]

use crate::error::{Error, Result};
use rav1d_safe::src::managed::{
    Decoder as Rav1dDecoder, Frame, MatrixCoefficients as Rav1dMatrixCoefficients, PixelLayout,
    Planes, Settings,
};
use rgb::Rgb;
use whereat::at;
use yuv::{YuvGrayImage, YuvPlanarImage, YuvRange, YuvStandardMatrix};

/// Convert rav1d matrix coefficients to yuv crate's YuvStandardMatrix
fn to_yuv_matrix(mc: Rav1dMatrixCoefficients) -> YuvStandardMatrix {
    match mc {
        Rav1dMatrixCoefficients::BT709 => YuvStandardMatrix::Bt709,
        Rav1dMatrixCoefficients::BT601 => YuvStandardMatrix::Bt601,
        Rav1dMatrixCoefficients::BT2020NCL => YuvStandardMatrix::Bt2020,
        _ => YuvStandardMatrix::Bt601,
    }
}

/// Decode a raw AV1 OBU bitstream to pixels.
///
/// This decodes AV1 data that is not wrapped in an AVIF container,
/// such as AVIF gain map images stored as raw AV1 bitstreams.
///
/// Returns `(pixel_data, width, height, channels)` where channels is
/// 1 for grayscale (monochrome AV1) or 3 for RGB.
///
/// For 10-bit or 12-bit AV1 streams, values are scaled down to 8-bit.
///
/// # Errors
///
/// Returns an error if the data is not valid AV1, if decoding fails,
/// or if the decoded frame cannot be converted to pixels.
///
/// # Example
///
/// ```no_run
/// let av1_obu_data: &[u8] = &[/* raw AV1 OBU bytes */];
/// let (pixels, width, height, channels) = zenavif::decode_av1_obu(av1_obu_data).unwrap();
/// if channels == 1 {
///     println!("Grayscale {}x{}", width, height);
/// } else {
///     println!("RGB {}x{}", width, height);
/// }
/// ```
pub fn decode_av1_obu(data: &[u8]) -> Result<(Vec<u8>, u32, u32, u8)> {
    if data.is_empty() {
        return Err(at!(Error::Decode {
            code: -1,
            msg: "empty AV1 OBU data",
        }));
    }

    let mut settings = Settings::default();
    settings.threads = 1;

    let mut decoder = Rav1dDecoder::with_settings(settings).map_err(|_e| {
        at!(Error::Decode {
            code: -1,
            msg: "failed to create AV1 decoder",
        })
    })?;

    let frame = decode_single_frame(&mut decoder, data)?;

    let bit_depth = frame.bit_depth();
    let layout = frame.pixel_layout();

    let color_info = frame.color_info();
    let yuv_range = if matches!(
        color_info.color_range,
        rav1d_safe::src::managed::ColorRange::Full
    ) {
        YuvRange::Full
    } else {
        YuvRange::Limited
    };
    let matrix = to_yuv_matrix(color_info.matrix_coefficients);

    match layout {
        PixelLayout::I400 => convert_monochrome(&frame, bit_depth, yuv_range, matrix),
        _ => convert_to_rgb(&frame, bit_depth, yuv_range, matrix),
    }
}

/// Decode a single frame from AV1 OBU data, handling progressive/multi-layer
/// streams by flushing the decoder if needed.
fn decode_single_frame(decoder: &mut Rav1dDecoder, data: &[u8]) -> Result<Frame> {
    match decoder.decode(data) {
        Ok(Some(frame)) => {
            let _ = decoder.flush();
            Ok(frame)
        }
        Ok(None) => {
            // Progressive/multi-layer: flush to get the composed frame
            let frames = decoder.flush().map_err(|_e| {
                at!(Error::Decode {
                    code: -1,
                    msg: "failed to flush AV1 decoder",
                })
            })?;
            frames.into_iter().last().ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "AV1 decoder produced no frames",
                })
            })
        }
        Err(_e) => Err(at!(Error::Decode {
            code: -1,
            msg: "failed to decode AV1 OBU data",
        })),
    }
}

/// Convert a monochrome (I400) frame to grayscale u8 pixels.
fn convert_monochrome(
    frame: &Frame,
    bit_depth: u8,
    yuv_range: YuvRange,
    matrix: YuvStandardMatrix,
) -> Result<(Vec<u8>, u32, u32, u8)> {
    let width = frame.width();
    let height = frame.height();
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| at!(Error::OutOfMemory))?;

    if bit_depth == 8 {
        let Planes::Depth8(planes) = frame.planes() else {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "expected 8-bit planes for 8-bit frame",
            }));
        };

        let y_view = planes.y();

        // For monochrome, we need Y-to-gray conversion respecting range
        // Use yuv crate's yuv400_to_rgb which produces R=G=B=luma, then
        // extract just one channel.
        let mut rgb_out = vec![Rgb { r: 0u8, g: 0, b: 0 }; pixel_count];
        let rgb_stride = width * 3;
        let gray = YuvGrayImage {
            y_plane: y_view.as_slice(),
            y_stride: y_view.stride() as u32,
            width,
            height,
        };
        yuv::yuv400_to_rgb(
            &gray,
            rgb::bytemuck::cast_slice_mut(rgb_out.as_mut_slice()),
            rgb_stride,
            yuv_range,
            matrix,
        )
        .map_err(|e| at!(Error::ColorConversion(e)))?;

        // Extract just the R channel (R=G=B for monochrome)
        let gray_pixels: Vec<u8> = rgb_out.iter().map(|px| px.r).collect();
        Ok((gray_pixels, width, height, 1))
    } else {
        let Planes::Depth16(planes) = frame.planes() else {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "expected 16-bit planes for high-bit-depth frame",
            }));
        };

        let y_view = planes.y();
        let pixel_count_u = width as usize * height as usize;

        // Convert 10/12-bit Y plane through yuv crate, then scale to 8-bit
        let mut rgb16_out = vec![Rgb::<u16> { r: 0, g: 0, b: 0 }; pixel_count_u];
        let rgb_stride = width * 3;
        let gray = YuvGrayImage {
            y_plane: y_view.as_slice(),
            y_stride: y_view.stride() as u32,
            width,
            height,
        };

        match bit_depth {
            10 => yuv::y010_to_rgb10(
                &gray,
                rgb::bytemuck::cast_slice_mut(rgb16_out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            12 => yuv::y012_to_rgb12(
                &gray,
                rgb::bytemuck::cast_slice_mut(rgb16_out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            _ => yuv::y016_to_rgb16(
                &gray,
                rgb::bytemuck::cast_slice_mut(rgb16_out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
        }
        .map_err(|e| at!(Error::ColorConversion(e)))?;

        // Scale 10/12/16-bit down to 8-bit, extracting just the R channel
        let shift = bit_depth.saturating_sub(8);
        let gray_pixels: Vec<u8> = rgb16_out
            .iter()
            .map(|px| (px.r >> shift).min(255) as u8)
            .collect();
        Ok((gray_pixels, width, height, 1))
    }
}

/// Convert a YUV frame (I420/I422/I444) to RGB u8 pixels.
fn convert_to_rgb(
    frame: &Frame,
    bit_depth: u8,
    yuv_range: YuvRange,
    matrix: YuvStandardMatrix,
) -> Result<(Vec<u8>, u32, u32, u8)> {
    let width = frame.width();
    let height = frame.height();
    let layout = frame.pixel_layout();
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| at!(Error::OutOfMemory))?;

    if bit_depth == 8 {
        let Planes::Depth8(planes) = frame.planes() else {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "expected 8-bit planes for 8-bit frame",
            }));
        };

        let y_view = planes.y();
        let u_view = planes.u().ok_or_else(|| {
            at!(Error::Decode {
                code: -1,
                msg: "missing U chroma plane",
            })
        })?;
        let v_view = planes.v().ok_or_else(|| {
            at!(Error::Decode {
                code: -1,
                msg: "missing V chroma plane",
            })
        })?;

        let planar = YuvPlanarImage {
            y_plane: y_view.as_slice(),
            y_stride: y_view.stride() as u32,
            u_plane: u_view.as_slice(),
            u_stride: u_view.stride() as u32,
            v_plane: v_view.as_slice(),
            v_stride: v_view.stride() as u32,
            width,
            height,
        };

        let mut out = vec![Rgb { r: 0u8, g: 0, b: 0 }; pixel_count];
        let rgb_stride = width * 3;

        match layout {
            PixelLayout::I420 => yuv::yuv420_to_rgb_bilinear(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            PixelLayout::I422 => yuv::yuv422_to_rgb_bilinear(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            PixelLayout::I444 => yuv::yuv444_to_rgb(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            PixelLayout::I400 => unreachable!("monochrome handled separately"),
        }
        .map_err(|e| at!(Error::ColorConversion(e)))?;

        let bytes: Vec<u8> = rgb::bytemuck::cast_vec(out);
        Ok((bytes, width, height, 3))
    } else {
        let Planes::Depth16(planes) = frame.planes() else {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "expected 16-bit planes for high-bit-depth frame",
            }));
        };

        let y_view = planes.y();
        let u_view = planes.u().ok_or_else(|| {
            at!(Error::Decode {
                code: -1,
                msg: "missing U chroma plane",
            })
        })?;
        let v_view = planes.v().ok_or_else(|| {
            at!(Error::Decode {
                code: -1,
                msg: "missing V chroma plane",
            })
        })?;

        let planar = YuvPlanarImage {
            y_plane: y_view.as_slice(),
            y_stride: y_view.stride() as u32,
            u_plane: u_view.as_slice(),
            u_stride: u_view.stride() as u32,
            v_plane: v_view.as_slice(),
            v_stride: v_view.stride() as u32,
            width,
            height,
        };

        let mut out = vec![Rgb::<u16> { r: 0, g: 0, b: 0 }; pixel_count];
        let rgb_stride = width * 3;

        match (layout, bit_depth) {
            (PixelLayout::I420, 10) => yuv::i010_to_rgb10_bilinear(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            (PixelLayout::I420, 12) => yuv::i012_to_rgb12_bilinear(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            (PixelLayout::I420, _) => yuv::i016_to_rgb16_bilinear(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            (PixelLayout::I422, 10) => yuv::i210_to_rgb10(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            (PixelLayout::I422, 12) => yuv::i212_to_rgb12(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            (PixelLayout::I422, _) => yuv::i216_to_rgb16(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            (PixelLayout::I444, 10) => yuv::i410_to_rgb10(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            (PixelLayout::I444, 12) => yuv::i412_to_rgb12(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            (PixelLayout::I444, _) => yuv::i416_to_rgb16(
                &planar,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            ),
            (PixelLayout::I400, _) => unreachable!("monochrome handled separately"),
        }
        .map_err(|e| at!(Error::ColorConversion(e)))?;

        // Scale 10/12/16-bit RGB down to 8-bit
        let shift = bit_depth.saturating_sub(8);
        let mut bytes = Vec::with_capacity(pixel_count * 3);
        for px in &out {
            bytes.push((px.r >> shift).min(255) as u8);
            bytes.push((px.g >> shift).min(255) as u8);
            bytes.push((px.b >> shift).min(255) as u8);
        }
        Ok((bytes, width, height, 3))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_data_returns_error() {
        let result = decode_av1_obu(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_data_returns_error() {
        let result = decode_av1_obu(&[0x00, 0x01, 0x02, 0x03]);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_obu_returns_error() {
        // A valid OBU header byte (temporal delimiter) but truncated
        let result = decode_av1_obu(&[0x12, 0x00]);
        assert!(result.is_err());
    }

    #[test]
    fn random_bytes_return_error() {
        let garbage: Vec<u8> = (0..256).map(|i| (i * 37 + 13) as u8).collect();
        let result = decode_av1_obu(&garbage);
        assert!(result.is_err());
    }

    /// Extract the gain map AV1 data from a test AVIF file and decode it
    /// using `decode_av1_obu`. This exercises the real use case: AVIF gain
    /// maps are stored as raw AV1 bitstreams.
    #[test]
    fn decode_gain_map_from_avif_test_file() {
        let avif_path = "tests/vectors/libavif/seine_sdr_gainmap_srgb.avif";
        let avif_data = match std::fs::read(avif_path) {
            Ok(data) => data,
            Err(_) => {
                eprintln!("skipping: test vector not found at {avif_path}");
                return;
            }
        };

        // Parse the AVIF to extract the gain map AV1 data
        let config = crate::config::DecoderConfig::default();
        let decoder = crate::decoder_managed::ManagedAvifDecoder::new(&avif_data, &config)
            .expect("should parse AVIF");
        let info = decoder.probe_info().expect("should probe");
        let gm = info.gain_map.expect("seine test file should have gain map");
        let av1_data = &gm.gain_map_data;
        assert!(
            !av1_data.is_empty(),
            "gain map AV1 data should be non-empty"
        );

        // Decode the raw AV1 OBU data
        let (pixels, width, height, channels) =
            decode_av1_obu(av1_data).expect("should decode gain map AV1 data");

        assert!(width > 0, "decoded width should be positive");
        assert!(height > 0, "decoded height should be positive");
        assert!(
            channels == 1 || channels == 3,
            "channels should be 1 (gray) or 3 (RGB), got {channels}"
        );

        let expected_len = width as usize * height as usize * channels as usize;
        assert_eq!(
            pixels.len(),
            expected_len,
            "pixel data length should match width*height*channels: {width}x{height}x{channels} = {expected_len}, got {}",
            pixels.len()
        );

        // Verify pixel values are not all zero (actual image content)
        let nonzero_count = pixels.iter().filter(|&&p| p != 0).count();
        assert!(
            nonzero_count > 0,
            "decoded gain map should have non-zero pixel values"
        );
    }
}

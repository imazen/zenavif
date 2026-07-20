//! Raw AV1 OBU bitstream decoding
//!
//! Decodes raw AV1 OBU byte sequences to pixels without requiring an AVIF
//! container. This is used for AVIF gain map images, which are stored as
//! raw AV1 bitstreams inside the AVIF container but outside the normal
//! primary/alpha item structure.

#![deny(unsafe_code)]

use crate::error::{Error, Result};
use rav1d_safe::src::managed::{Decoder as Rav1dDecoder, Frame, PixelLayout, Planes, Settings};
use rgb::Rgb;
use whereat::at;

// The former blind `to_yuv_matrix` (`_ => Bt601`) is replaced by
// `crate::cicp_resolve::resolve` — raw OBU streams carry no container,
// so there is no nclx hint here; unspecified/unimplemented matrices
// error honestly (imazen/zenavif#15).

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
        crate::yuv_convert::YuvRange::Full
    } else {
        crate::yuv_convert::YuvRange::Limited
    };
    let mc = color_info.matrix_coefficients as u8;
    let cp = color_info.primaries as u8;
    // Raw OBUs have no container; these streams come out of AVIF files
    // (gain maps, auxiliaries), so the AVIF default disambiguates an
    // unspecified MC exactly as it would for the embedding file.
    let hint = Some(crate::cicp_resolve::AVIF_DEFAULT_MC);
    match layout {
        PixelLayout::I400 => {
            // Monochrome has no chroma to matrix; no matrix input needed.
            convert_monochrome(&frame, bit_depth, yuv_range)
        }
        PixelLayout::I444
            if matches!(
                crate::cicp_resolve::resolve(mc, cp, hint),
                Ok(crate::cicp_resolve::ResolvedMatrix::Identity)
            ) =>
        {
            convert_identity_to_rgb(&frame, bit_depth, yuv_range)
        }
        _ => {
            let resolved = crate::cicp_resolve::resolve(mc, cp, hint)?;
            let matrix = resolved.to_our().ok_or_else(|| {
                at!(Error::Unsupported(
                    "identity (MC=0) requires 4:4:4 chroma; subsampled identity has no \
                     defined reconstruction"
                ))
            })?;
            convert_to_rgb(&frame, bit_depth, yuv_range, matrix)
        }
    }
}

/// 3-bytes-per-pixel output length with an explicit overflow check.
///
/// On 32-bit targets (i686, wasm32) a pixel count can survive the
/// width×height `checked_mul` and still wrap on `* 3`; a wrapped length
/// would under-allocate and then panic on the first out-of-bounds row
/// (zenavif#18). Also reachable on 64-bit with pathological pixel counts.
fn rgb_byte_len(pixel_count: usize) -> Result<usize> {
    pixel_count
        .checked_mul(3)
        .ok_or_else(|| at!(Error::OutOfMemory))
}

/// Identity (MC=0) conversion for raw OBU decode: planes are G,B,R —
/// reorder + range expansion + (for 10/12-bit) downscale to 8-bit,
/// matching this function family's 8-bit output contract.
fn convert_identity_to_rgb(
    frame: &Frame,
    bit_depth: u8,
    yuv_range: crate::yuv_convert::YuvRange,
) -> Result<(Vec<u8>, u32, u32, u8)> {
    let width = frame.width();
    let height = frame.height();
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| at!(Error::OutOfMemory))?;
    let limited = matches!(yuv_range, crate::yuv_convert::YuvRange::Limited);
    // Full-image output buffer sized from the (untrusted) decoded frame
    // dimensions → fallible by default (zenavif#21).
    let mut out = crate::alloc_util::alloc_filled(
        crate::alloc_util::AllocPref::CodecDefault,
        true,
        0u8,
        rgb_byte_len(pixel_count)?,
    )?;

    // Map a native-depth sample to full-range u8.
    let to8 = |v: u32| -> u8 {
        if limited {
            let smin = 16u32 << (bit_depth - 8);
            let span = 219u32 << (bit_depth - 8);
            let c = v.saturating_sub(smin).min(span);
            ((c * 255 + span / 2) / span) as u8
        } else {
            (v >> (bit_depth - 8)) as u8
        }
    };

    macro_rules! reorder {
        ($planes:expr) => {{
            let g = $planes.y();
            let b = $planes.u().ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "identity content missing plane 1 (B)",
                })
            })?;
            let r = $planes.v().ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "identity content missing plane 2 (R)",
                })
            })?;
            for (row_idx, ((g_row, b_row), r_row)) in g
                .rows()
                .zip(b.rows())
                .zip(r.rows())
                .enumerate()
                .take(height as usize)
            {
                let out_row = &mut out[row_idx * width as usize * 3..][..width as usize * 3];
                for x in 0..width as usize {
                    out_row[x * 3] = to8(r_row[x] as u32);
                    out_row[x * 3 + 1] = to8(g_row[x] as u32);
                    out_row[x * 3 + 2] = to8(b_row[x] as u32);
                }
            }
        }};
    }
    match frame.planes() {
        Planes::Depth8(p) => reorder!(p),
        Planes::Depth16(p) => reorder!(p),
    }

    Ok((out, width, height, 3))
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

/// Convert a monochrome (I400) frame to grayscale u8 pixels via the
/// in-house mono kernel (16-bit output is scaled down to 8-bit).
fn convert_monochrome(
    frame: &Frame,
    bit_depth: u8,
    yuv_range: crate::yuv_convert::YuvRange,
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
        let mut gray = crate::alloc_util::alloc_filled(
            crate::alloc_util::AllocPref::CodecDefault,
            true,
            rgb::Gray::<u8>::new(0),
            pixel_count,
        )?;
        crate::yuv_convert::yuv400_to_rgbx_strip::<u8, rgb::Gray<u8>>(
            y_view.as_slice(),
            y_view.stride(),
            width as usize,
            0,
            height as usize,
            yuv_range,
            8,
            &mut gray,
        );
        Ok((rgb::bytemuck::cast_vec(gray), width, height, 1))
    } else {
        let Planes::Depth16(planes) = frame.planes() else {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "expected 16-bit planes for high-bit-depth frame",
            }));
        };
        let y_view = planes.y();
        let mut gray = crate::alloc_util::alloc_filled(
            crate::alloc_util::AllocPref::CodecDefault,
            true,
            rgb::Gray::<u16>::new(0),
            pixel_count,
        )?;
        crate::yuv_convert::yuv400_to_rgbx_strip::<u16, rgb::Gray<u16>>(
            y_view.as_slice(),
            y_view.stride(),
            width as usize,
            0,
            height as usize,
            yuv_range,
            bit_depth,
            &mut gray,
        );
        // Scale 10/12/16-bit down to 8-bit.
        let shift = bit_depth.saturating_sub(8);
        let mut gray_pixels: Vec<u8> = crate::alloc_util::vec_with_capacity(
            crate::alloc_util::AllocPref::CodecDefault,
            true,
            pixel_count,
        )?;
        gray_pixels.extend(gray.iter().map(|px| (px.value() >> shift).min(255) as u8));
        Ok((gray_pixels, width, height, 1))
    }
}

/// Convert a YUV frame (I420/I422/I444) to RGB u8 pixels via the in-house
/// unified kernels (16-bit output is scaled down to 8-bit — this function
/// family's output contract).
fn convert_to_rgb(
    frame: &Frame,
    bit_depth: u8,
    yuv_range: crate::yuv_convert::YuvRange,
    matrix: crate::yuv_convert::YuvMatrix,
) -> Result<(Vec<u8>, u32, u32, u8)> {
    use crate::yuv_convert::ChromaSubsampling as Cs;
    let width = frame.width();
    let height = frame.height();
    let layout = frame.pixel_layout();
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| at!(Error::OutOfMemory))?;
    let sampling = match layout {
        PixelLayout::I420 => Cs::Cs420,
        PixelLayout::I422 => Cs::Cs422,
        PixelLayout::I444 => Cs::Cs444,
        PixelLayout::I400 => unreachable!("monochrome handled separately"),
    };
    let (w, h) = (width as usize, height as usize);

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
        let mut out = crate::alloc_util::alloc_filled(
            crate::alloc_util::AllocPref::CodecDefault,
            true,
            Rgb { r: 0u8, g: 0, b: 0 },
            pixel_count,
        )?;
        match sampling {
            Cs::Cs420 => crate::yuv_convert::yuv420_to_rgb8_strip(
                y_view.as_slice(),
                y_view.stride(),
                u_view.as_slice(),
                u_view.stride(),
                v_view.as_slice(),
                v_view.stride(),
                w,
                h,
                0,
                h,
                yuv_range,
                matrix,
                &mut out,
            ),
            Cs::Cs422 => crate::yuv_convert::yuv422_to_rgb8_strip(
                y_view.as_slice(),
                y_view.stride(),
                u_view.as_slice(),
                u_view.stride(),
                v_view.as_slice(),
                v_view.stride(),
                w,
                0,
                h,
                yuv_range,
                matrix,
                &mut out,
            ),
            Cs::Cs444 => crate::yuv_convert::yuv444_to_rgb8_strip(
                y_view.as_slice(),
                y_view.stride(),
                u_view.as_slice(),
                u_view.stride(),
                v_view.as_slice(),
                v_view.stride(),
                w,
                0,
                h,
                yuv_range,
                matrix,
                &mut out,
            ),
        }
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
        let mut out = crate::alloc_util::alloc_filled(
            crate::alloc_util::AllocPref::CodecDefault,
            true,
            Rgb::<u16> { r: 0, g: 0, b: 0 },
            pixel_count,
        )?;
        crate::yuv_convert::yuv16_to_rgbx_strip::<Rgb<u16>>(
            sampling,
            y_view.as_slice(),
            y_view.stride(),
            u_view.as_slice(),
            u_view.stride(),
            v_view.as_slice(),
            v_view.stride(),
            w,
            h,
            0,
            h,
            yuv_range,
            matrix,
            bit_depth,
            &mut out,
        );

        // Scale 10/12/16-bit RGB down to 8-bit
        let shift = bit_depth.saturating_sub(8);
        let mut bytes: Vec<u8> = crate::alloc_util::vec_with_capacity(
            crate::alloc_util::AllocPref::CodecDefault,
            true,
            rgb_byte_len(pixel_count)?,
        )?;
        for px in &out {
            bytes.push((px.r >> shift).min(255) as u8);
            bytes.push((px.g >> shift).min(255) as u8);
            bytes.push((px.b >> shift).min(255) as u8);
        }
        Ok((bytes, width, height, 3))
    }
}

// ===========================================================================
// DECODE-BENCH FORK: second decode backend (aom-rs) behind a common YUV seam.
//
// Both backends receive the IDENTICAL raw AV1 OBU temporal-unit bytes and
// return the same `DecodedYuv` shape (tight, unpadded u16 planes) — "one
// frontend, two backends", so an apples-to-apples decode-speed comparison
// isolates the decode kernel. The aom-rs backend covers the KEY-frame / intra
// scope (AVIF stills are single KEY frames) and is byte-identical to libaom on
// the AV1 conformance corpus.
// ===========================================================================

/// A decoded AV1 frame as tight (unpadded) YUV planes, `u16` per sample at
/// every bit depth. The field shape mirrors aom-rs's
/// `aom_decode::frame::FrameDecode` so the two backends are directly
/// comparable.
#[derive(Clone, Debug)]
pub struct DecodedYuv {
    /// Cropped luma, tight `width`-strided rows.
    pub y: Vec<u16>,
    /// Cropped chroma (empty when monochrome), tight `width_uv`-strided.
    pub u: Vec<u16>,
    pub v: Vec<u16>,
    pub width: usize,
    pub height: usize,
    pub width_uv: usize,
    pub height_uv: usize,
    pub bit_depth: i32,
    pub monochrome: bool,
    pub subsampling_x: usize,
    pub subsampling_y: usize,
}

impl DecodedYuv {
    /// Total decoded luma pixels (the throughput unit for the decode bench).
    pub fn luma_pixels(&self) -> usize {
        self.width * self.height
    }
}

/// Which AV1 decode kernel to run behind zenavif's raw-OBU decode seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeBackend {
    /// The default pure-Rust rav1d-safe managed decoder (full AV1 profile).
    Rav1dSafe,
    /// The aom-rs pure-Rust decoder (KEY-frame / intra scope; byte-identical
    /// to libaom on the AV1 conformance corpus). Requires the `aom-backend`
    /// feature.
    #[cfg(feature = "aom-backend")]
    AomRs,
    /// The rav1d FFI decoder — upstream rav1d with its full hand-written
    /// assembly (unsafe). Requires the `unsafe-asm` feature. This is the
    /// asm-speed reference arm of the decode benchmark.
    #[cfg(feature = "unsafe-asm")]
    Rav1dFfi,
}

/// Decode a raw AV1 OBU temporal unit to tight YUV planes via the selected
/// backend. Both backends receive the IDENTICAL OBU bytes and produce the same
/// [`DecodedYuv`] shape — this is the "one frontend, two backends" seam the
/// decode benchmark drives (only the decode kernel differs).
pub fn decode_av1_obu_yuv(data: &[u8], backend: DecodeBackend) -> Result<DecodedYuv> {
    match backend {
        DecodeBackend::Rav1dSafe => decode_av1_obu_yuv_rav1d(data),
        #[cfg(feature = "aom-backend")]
        DecodeBackend::AomRs => decode_av1_obu_yuv_aomrs(data),
        #[cfg(feature = "unsafe-asm")]
        DecodeBackend::Rav1dFfi => crate::decoder::decode_obu_yuv_ffi(data),
    }
}

/// rav1d-safe backend: managed decode, then copy the decoded planes into tight
/// (unpadded) `u16` buffers (widening 8-bit samples) so the output matches the
/// aom-rs backend's shape exactly.
pub fn decode_av1_obu_yuv_rav1d(data: &[u8]) -> Result<DecodedYuv> {
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

    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let bit_depth = frame.bit_depth() as i32;
    let layout = frame.pixel_layout();
    let monochrome = matches!(layout, PixelLayout::I400);
    let (subsampling_x, subsampling_y) = match layout {
        PixelLayout::I400 | PixelLayout::I420 => (1, 1),
        PixelLayout::I422 => (1, 0),
        PixelLayout::I444 => (0, 0),
    };

    let (y, u, v, width_uv, height_uv) = match frame.planes() {
        Planes::Depth8(p) => {
            let yv = p.y();
            let mut y = Vec::with_capacity(yv.width() * yv.height());
            for row in yv.rows() {
                y.extend(row.iter().map(|&s| s as u16));
            }
            match (p.u(), p.v()) {
                (Some(up), Some(vp)) => {
                    let (wu, hu) = (up.width(), up.height());
                    let mut u = Vec::with_capacity(wu * hu);
                    for row in up.rows() {
                        u.extend(row.iter().map(|&s| s as u16));
                    }
                    let mut v = Vec::with_capacity(vp.width() * vp.height());
                    for row in vp.rows() {
                        v.extend(row.iter().map(|&s| s as u16));
                    }
                    (y, u, v, wu, hu)
                }
                _ => (y, Vec::new(), Vec::new(), 0, 0),
            }
        }
        Planes::Depth16(p) => {
            let yv = p.y();
            let mut y = Vec::with_capacity(yv.width() * yv.height());
            for row in yv.rows() {
                y.extend_from_slice(row);
            }
            match (p.u(), p.v()) {
                (Some(up), Some(vp)) => {
                    let (wu, hu) = (up.width(), up.height());
                    let mut u = Vec::with_capacity(wu * hu);
                    for row in up.rows() {
                        u.extend_from_slice(row);
                    }
                    let mut v = Vec::with_capacity(vp.width() * vp.height());
                    for row in vp.rows() {
                        v.extend_from_slice(row);
                    }
                    (y, u, v, wu, hu)
                }
                _ => (y, Vec::new(), Vec::new(), 0, 0),
            }
        }
    };

    Ok(DecodedYuv {
        y,
        u,
        v,
        width,
        height,
        width_uv,
        height_uv,
        bit_depth,
        monochrome,
        subsampling_x,
        subsampling_y,
    })
}

/// aom-rs backend: pure-Rust KEY-frame decode, byte-identical to libaom on the
/// AV1 conformance corpus. Output is already tight `u16` planes.
#[cfg(feature = "aom-backend")]
pub fn decode_av1_obu_yuv_aomrs(data: &[u8]) -> Result<DecodedYuv> {
    let fd = aom_decode::frame::decode_frame_obus(data).map_err(|_e| {
        at!(Error::Decode {
            code: -1,
            msg: "aom-rs backend rejected the AV1 OBU frame (KEY/intra scope only)",
        })
    })?;
    Ok(DecodedYuv {
        y: fd.y,
        u: fd.u,
        v: fd.v,
        width: fd.width,
        height: fd.height,
        width_uv: fd.width_uv,
        height_uv: fd.height_uv,
        bit_depth: fd.bit_depth,
        monochrome: fd.monochrome,
        subsampling_x: fd.subsampling_x,
        subsampling_y: fd.subsampling_y,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_data_returns_error() {
        let result = decode_av1_obu(&[]);
        assert!(result.is_err());
    }

    /// zenavif#18 (3): the `pixel_count * 3` RGB byte length must be a
    /// checked multiply. On i686/wasm32 a width×height that survives the
    /// pixel-count check can still wrap on `* 3`; the same expression is
    /// reachable on 64-bit with a pathological pixel count. A wrapped
    /// length would under-allocate and panic on the first row write.
    #[test]
    fn rgb_byte_len_overflow_is_an_error_not_a_wrap() {
        // Overflows `* 3` on every pointer width.
        assert!(rgb_byte_len(usize::MAX / 2).is_err());
        // The largest representable AV1 frame (65536×65536) must stay valid
        // on 64-bit. On 32-bit targets the pixel count itself overflows
        // (`checked_mul` returns None) and is caught before rgb_byte_len.
        if let Some(px) = 65536usize.checked_mul(65536) {
            assert_eq!(rgb_byte_len(px).unwrap(), px * 3);
        }
        assert_eq!(rgb_byte_len(4).unwrap(), 12);
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
        // Fail-loud: CI provisions the vectors (see ci.yml "Download test
        // vectors"); locally run `just download-vectors` first. A silent
        // skip here would fake coverage (no-graceful-skips policy).
        let avif_data = std::fs::read(avif_path)
            .unwrap_or_else(|e| panic!("read {avif_path}: {e} (run: just download-vectors)"));

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

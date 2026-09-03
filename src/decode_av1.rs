//! Raw AV1 OBU bitstream decoding
//!
//! Decodes raw AV1 OBU byte sequences to pixels without requiring an AVIF
//! container. This is used for AVIF gain map images, which are stored as
//! raw AV1 bitstreams inside the AVIF container but outside the normal
//! primary/alpha item structure.

#![deny(unsafe_code)]

use crate::error::{Error, Result, error_from_rav1d};
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
    decode_av1_obu_with_config(data, &crate::DecoderConfig::default())
}

/// [`decode_av1_obu`] with the decode backend + caps taken from a
/// [`crate::DecoderConfig`] — the gain-map decode path honors
/// `decode_backend` like the primary/alpha item decodes do.
pub(crate) fn decode_av1_obu_with_config(
    data: &[u8],
    #[cfg_attr(not(feature = "zenav1-aom"), allow(unused_variables))] config: &crate::DecoderConfig,
) -> Result<(Vec<u8>, u32, u32, u8)> {
    #[cfg(feature = "zenav1-aom")]
    if config.decode_backend == crate::DecodeBackend::Zenav1Aom {
        return decode_av1_obu_aom_8bit(data, config);
    }
    if data.is_empty() {
        return Err(at!(Error::UnexpectedEof("empty AV1 OBU data")));
    }

    let mut settings = Settings::default();
    settings.threads = 1;

    let mut decoder = Rav1dDecoder::with_settings(settings).map_err(|e| {
        e.map_error(|re| error_from_rav1d(re, "failed to create AV1 decoder"))
            .at()
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
            let b = $planes
                .u()
                .ok_or_else(|| at!(Error::Malformed("identity content missing plane 1 (B)")))?;
            let r = $planes
                .v()
                .ok_or_else(|| at!(Error::Malformed("identity content missing plane 2 (R)")))?;
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
            let frames = decoder.flush().map_err(|e| {
                e.map_error(|re| error_from_rav1d(re, "failed to flush AV1 decoder"))
                    .at()
            })?;
            frames.into_iter().last().ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "AV1 decoder produced no frames",
                })
            })
        }
        Err(e) => Err(e
            .map_error(|re| error_from_rav1d(re, "failed to decode AV1 OBU data"))
            .at()),
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
        let u_view = planes
            .u()
            .ok_or_else(|| at!(Error::Malformed("missing U chroma plane")))?;
        let v_view = planes
            .v()
            .ok_or_else(|| at!(Error::Malformed("missing V chroma plane")))?;
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
        let u_view = planes
            .u()
            .ok_or_else(|| at!(Error::Malformed("missing U chroma plane")))?;
        let v_view = planes
            .v()
            .ok_or_else(|| at!(Error::Malformed("missing V chroma plane")))?;
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
// DECODE-BENCH FORK: second decode backend (zenav1-aom) behind a common YUV seam.
//
// Both backends receive the IDENTICAL raw AV1 OBU temporal-unit bytes and
// return the same `DecodedYuv` shape (tight, unpadded u16 planes) — "one
// frontend, two backends", so an apples-to-apples decode-speed comparison
// isolates the decode kernel. The zenav1-aom backend covers the KEY-frame / intra
// scope (AVIF stills are single KEY frames) and is byte-identical to libaom on
// the AV1 conformance corpus.
// ===========================================================================

/// A decoded AV1 frame as tight (unpadded) YUV planes, `u16` per sample at
/// every bit depth. The field shape mirrors zenav1-aom's
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
/// `#[non_exhaustive]`: same reasoning as [`crate::Av1Backend`] — this enum
/// already went `Rav1dSafe` -> `+Zenav1Aom` -> `+Rav1dFfi` and will keep growing,
/// so a new decode backend should not break every downstream match.
#[non_exhaustive]
pub enum DecodeBackend {
    /// The default pure-Rust rav1d-safe managed decoder (full AV1 profile).
    Rav1dSafe,
    /// The zenav1-aom pure-Rust decoder (KEY-frame / intra scope; byte-identical
    /// to libaom on the AV1 conformance corpus). Requires the `zenav1-aom`
    /// feature.
    #[cfg(feature = "zenav1-aom")]
    Zenav1Aom,
    /// The rav1d FFI decoder — upstream rav1d with its full hand-written
    /// assembly (unsafe). Requires the `unsafe-asm` feature. This is the
    /// asm-speed reference arm of the decode benchmark.
    #[cfg(feature = "unsafe-asm")]
    Rav1dFfi,
}

impl DecodeBackend {
    /// Deprecated spelling of [`DecodeBackend::Zenav1Aom`].
    ///
    /// The backend crate was renamed `aom-rs` -> `zenav1-aom`, so this
    /// variant name no longer matches anything that exists. Kept as an
    /// associated constant so existing consumers keep compiling in both
    /// expression and pattern position (the enum derives `PartialEq` /
    /// `Eq`, so the constant is structural-match and is usable in a
    /// `match` arm).
    #[cfg(feature = "zenav1-aom")]
    #[deprecated(
        since = "0.1.8",
        note = "renamed to `DecodeBackend::Zenav1Aom` to match the zenav1-aom crate; \
                the alias's removal is DEFERRED past 0.2.0 — a live consumer \
                was still building against the old spelling"
    )]
    #[allow(non_upper_case_globals)]
    pub const AomRs: Self = Self::Zenav1Aom;
}

/// Decode a raw AV1 OBU temporal unit to tight YUV planes via the selected
/// backend. Both backends receive the IDENTICAL OBU bytes and produce the same
/// [`DecodedYuv`] shape — this is the "one frontend, two backends" seam the
/// decode benchmark drives (only the decode kernel differs).
///
/// Uses default limits and no cancellation; the config-carrying twin is
/// [`decode_av1_obu_yuv_with`].
pub fn decode_av1_obu_yuv(data: &[u8], backend: DecodeBackend) -> Result<DecodedYuv> {
    decode_av1_obu_yuv_with(
        data,
        backend,
        &crate::DecoderConfig::default(),
        &enough::Unstoppable,
    )
}

/// [`decode_av1_obu_yuv`] with resource limits and cancellation threaded
/// through to the backend (backend-seam obligation 3: a capability the
/// backend accepts must be consumed, not dropped at the seam).
///
/// What each backend receives:
/// * **Rav1dSafe** — `config.frame_size_limit` via the managed
///   `Settings::frame_size_limit` (header-rejected before frame allocation);
///   `stop` is polled at the phase boundary (the managed single-frame call
///   has no in-loop hook on this seam).
/// * **Zenav1Aom** — the full upstream `DecodeConfig`: `frame_size_limit` as
///   `DecodeLimits::max_pixels`, `stop` polled in-loop at SB-row/tile/frame
///   cadence, and `config.alloc_pref` as the `AllocMode`
///   (`CodecDefault`/`Fallible` → fallible pre-flight — untrusted-input
///   default — `Infallible` → single fast allocation).
/// * **Rav1dFfi** — nothing (legacy C seam accepts no config); `stop` is
///   polled once before the call.
pub fn decode_av1_obu_yuv_with(
    data: &[u8],
    backend: DecodeBackend,
    config: &crate::DecoderConfig,
    stop: &(impl enough::Stop + ?Sized),
) -> Result<DecodedYuv> {
    stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
    match backend {
        DecodeBackend::Rav1dSafe => decode_av1_obu_yuv_rav1d_with(data, config),
        #[cfg(feature = "zenav1-aom")]
        DecodeBackend::Zenav1Aom => decode_av1_obu_yuv_aomrs_with(data, config, stop),
        #[cfg(feature = "unsafe-asm")]
        DecodeBackend::Rav1dFfi => crate::decoder::decode_obu_yuv_ffi(data),
    }
}

/// rav1d-safe backend: managed decode, then copy the decoded planes into tight
/// (unpadded) `u16` buffers (widening 8-bit samples) so the output matches the
/// zenav1-aom backend's shape exactly.
fn decode_av1_obu_yuv_rav1d_with(data: &[u8], config: &crate::DecoderConfig) -> Result<DecodedYuv> {
    if data.is_empty() {
        return Err(at!(Error::Decode {
            code: -1,
            msg: "empty AV1 OBU data",
        }));
    }
    let mut settings = Settings::default();
    settings.threads = 1;
    // Same pre-allocation pixel cap the container decode path enforces
    // (`DecoderConfig::frame_size_limit`; 0 = opt-out).
    settings.frame_size_limit = config.frame_size_limit;
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

/// zenav1-aom backend: pure-Rust KEY-frame decode, byte-identical to libaom on the
/// AV1 conformance corpus. Output is already tight `u16` planes.
///
/// Every `aom_decode::DecodeError` variant maps onto the matching zenavif
/// [`Error`] variant so the failure category survives to
/// `CategorizedError::category()` (backend-seam obligation 1 — no flattening
/// to one opaque code).
/// aom-backed raw-OBU decode to the [`decode_av1_obu`] 8-bit output contract
/// (`(pixels, w, h, channels)`; 10/12-bit scaled down to 8-bit). Serves the
/// gain-map decode path when `decode_backend == Zenav1Aom`. Same CICP handling
/// as the rav1d arm: raw OBUs carry no container, so an unspecified MC
/// disambiguates via the AVIF default.
#[cfg(feature = "zenav1-aom")]
fn decode_av1_obu_aom_8bit(
    data: &[u8],
    config: &crate::DecoderConfig,
) -> Result<(Vec<u8>, u32, u32, u8)> {
    use crate::yuv_convert as yc;
    let aom_config = aom_config_from(config);
    let fd = aom_decode::frame::decode_frame_obus_with(data, &aom_config).map_err(map_aom_error)?;
    let (w, h) = (fd.width, fd.height);
    let bd = fd.bit_depth as u8;
    let shift = bd.saturating_sub(8);
    let px = w.checked_mul(h).ok_or_else(|| at!(Error::OutOfMemory))?;
    let range = if fd.full_range {
        yc::YuvRange::Full
    } else {
        yc::YuvRange::Limited
    };
    if fd.monochrome {
        let mut gray = vec![rgb::Gray::<u16>::new(0); px];
        yc::yuv400_to_rgbx_strip::<u16, rgb::Gray<u16>>(&fd.y, w, w, 0, h, range, bd, &mut gray);
        let out: Vec<u8> = gray.iter().map(|g| (g.value() >> shift) as u8).collect();
        return Ok((out, w as u32, h as u32, 1));
    }
    let hint = Some(crate::cicp_resolve::AVIF_DEFAULT_MC);
    let resolved =
        crate::cicp_resolve::resolve(fd.matrix_coefficients as u8, fd.color_primaries as u8, hint)?;
    if matches!(resolved, crate::cicp_resolve::ResolvedMatrix::Identity) {
        if (fd.subsampling_x, fd.subsampling_y) != (0, 0) {
            return Err(at!(Error::Unsupported(
                "identity (MC=0) requires 4:4:4 chroma; subsampled identity has no \
                 defined reconstruction"
            )));
        }
        // Planes are (G,B,R); expand limited range then scale to 8-bit.
        let limited = !fd.full_range;
        let max = (1u32 << bd) - 1;
        let smin = 16u32 << shift;
        let span = 219u32 << shift;
        let to8 = |v: u16| -> u8 {
            if limited {
                let c = (v as u32).saturating_sub(smin).min(span);
                ((c * 255 + span / 2) / span) as u8
            } else {
                (((v as u32) * 255 + max / 2) / max) as u8
            }
        };
        let mut out = crate::alloc_util::alloc_filled(
            config.alloc_pref,
            true,
            0u8,
            px.checked_mul(3).ok_or_else(|| at!(Error::OutOfMemory))?,
        )?;
        for i in 0..px {
            out[i * 3] = to8(fd.v[i]);
            out[i * 3 + 1] = to8(fd.y[i]);
            out[i * 3 + 2] = to8(fd.u[i]);
        }
        return Ok((out, w as u32, h as u32, 3));
    }
    let matrix = resolved
        .to_our()
        .expect("identity handled above; every real matrix maps in-house");
    let sampling = match (fd.subsampling_x, fd.subsampling_y) {
        (0, 0) => yc::ChromaSubsampling::Cs444,
        (1, 0) => yc::ChromaSubsampling::Cs422,
        _ => yc::ChromaSubsampling::Cs420,
    };
    let mut wide = vec![rgb::Rgb::<u16> { r: 0, g: 0, b: 0 }; px];
    yc::yuv16_to_rgbx_strip::<rgb::Rgb<u16>>(
        sampling,
        &fd.y,
        w,
        &fd.u,
        fd.width_uv,
        &fd.v,
        fd.width_uv,
        w,
        h,
        0,
        h,
        range,
        matrix,
        bd,
        &mut wide,
    );
    let mut out = crate::alloc_util::alloc_filled(
        config.alloc_pref,
        true,
        0u8,
        px.checked_mul(3).ok_or_else(|| at!(Error::OutOfMemory))?,
    )?;
    for (i, p) in wide.iter().enumerate() {
        out[i * 3] = (p.r >> shift) as u8;
        out[i * 3 + 1] = (p.g >> shift) as u8;
        out[i * 3 + 2] = (p.b >> shift) as u8;
    }
    Ok((out, w as u32, h as u32, 3))
}

/// Map every `aom_decode::DecodeError` variant onto the matching zenavif
/// [`Error`] variant (backend-seam obligation 1 — the failure category
/// survives to `CategorizedError::category()`). Shared by the raw-OBU seam
/// and the aom-backed product decode path.
#[cfg(feature = "zenav1-aom")]
pub(crate) fn map_aom_error(e: aom_decode::DecodeError) -> whereat::At<Error> {
    use aom_decode::DecodeError as AomError;
    match e {
        AomError::Truncated(_) => at!(Error::Decode {
            code: -2,
            msg: "zenav1-aom: truncated AV1 OBU stream",
        }),
        AomError::Malformed(_) => at!(Error::Decode {
            code: -3,
            msg: "zenav1-aom: malformed AV1 bitstream",
        }),
        AomError::UnsupportedType(_) => at!(Error::Unsupported(
            "zenav1-aom: AV1 stream type outside this backend's envelope"
        )),
        AomError::UnsupportedFeature(m) => at!(Error::Unsupported(m)),
        AomError::LimitExceeded { kind, actual, max } => at!(Error::ResourceLimit(format!(
            "zenav1-aom decode limit: {} = {actual} > {max}",
            kind.as_str()
        ))),
        AomError::AllocFailed { .. } => at!(Error::OutOfMemory),
        AomError::Cancelled(reason) => at!(Error::Cancelled(reason)),
        AomError::Internal(_) => at!(Error::Decode {
            code: -4,
            msg: "zenav1-aom: internal decoder invariant failure",
        }),
        // `DecodeError` is #[non_exhaustive]; future variants degrade to the
        // generic decode bucket rather than failing the build.
        _ => at!(Error::Decode {
            code: -1,
            msg: "zenav1-aom: decode failed",
        }),
    }
}

/// Build the upstream `DecodeConfig` from zenavif's decode caps: the pixel
/// cap (0 = opt-out -> None = upstream's own 2^28 default) and the
/// allocation mode (CodecDefault/Fallible -> fallible pre-flight, the
/// untrusted-input default; Infallible -> fast path). The stop token is
/// attached at the call site (lifetime-bound).
#[cfg(feature = "zenav1-aom")]
pub(crate) fn aom_config_from<'a>(config: &crate::DecoderConfig) -> aom_decode::DecodeConfig<'a> {
    let mut limits = aom_decode::DecodeLimits::new();
    limits.max_pixels = (config.frame_size_limit > 0).then_some(u64::from(config.frame_size_limit));
    let alloc = match config.alloc_pref {
        crate::alloc_util::AllocPref::Infallible => aom_decode::AllocMode::Infallible,
        _ => aom_decode::AllocMode::Fallible,
    };
    aom_decode::DecodeConfig::new()
        .with_limits(limits)
        .with_alloc(alloc)
}

#[cfg(feature = "zenav1-aom")]
fn decode_av1_obu_yuv_aomrs_with(
    data: &[u8],
    config: &crate::DecoderConfig,
    stop: &(impl enough::Stop + ?Sized),
) -> Result<DecodedYuv> {
    // `&stop` (not `stop`): `&S: Stop` for any `S: Stop + ?Sized`, and the
    // sized reference coerces to the config's `&dyn Stop`.
    let aom_config = aom_config_from(config).with_stop(&stop);
    let fd = aom_decode::frame::decode_frame_obus_with(data, &aom_config).map_err(map_aom_error)?;
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

    /// The raw-OBU MONOCHROME arm (`convert_monochrome`) must produce the
    /// same gray values as the container decode path.
    ///
    /// Two implementations of one conversion: `decode_av1_obu` reorders and
    /// range-expands the I400 plane itself, while the product path goes
    /// through `decoder_managed`. `convert_monochrome` measured **0
    /// executions** across the whole feature matrix (cargo-llvm-cov,
    /// 2026-08-11; docs/TEST_COVERAGE.md) — the mono tests all enter through
    /// the container. Fail-loud on the fixture: it is committed under
    /// tests/vectors/zenavif/ (no graceful skips).
    #[test]
    fn raw_obu_mono_matches_the_container_path() {
        for (name, depth) in [
            ("mono_gradient_8b_full.avif", 8u8),
            ("mono_gradient_8b_limited.avif", 8),
            ("mono_gradient_10b_full.avif", 10),
            ("mono_5x3_8b_full.avif", 8),
        ] {
            let path = format!(
                "{}/tests/vectors/zenavif/{name}",
                env!("CARGO_MANIFEST_DIR")
            );
            let file = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
            let parser = zenavif_parse::AvifParser::from_bytes(&file)
                .unwrap_or_else(|e| panic!("parse {name}: {e:?}"));
            let payload = parser
                .primary_data()
                .unwrap_or_else(|e| panic!("primary item of {name}: {e:?}"));

            // Raw-OBU arm: 1 channel out for monochrome content.
            let (raw, w, h, channels) = decode_av1_obu(&payload)
                .unwrap_or_else(|e| panic!("raw OBU decode of {name}: {e:?}"));
            assert_eq!(channels, 1, "{name}: mono content must decode to 1 channel");
            assert_eq!(
                raw.len(),
                w as usize * h as usize,
                "{name}: gray plane size"
            );

            // Product path, forced to 8-bit so both sides are u8 gray.
            let cfg = crate::config::DecoderConfig::new().prefer_8bit(true);
            let buf = crate::decode_with(&file, &cfg, &enough::Unstoppable)
                .unwrap_or_else(|e| panic!("container decode of {name}: {e:?}"));
            // Native Gray8 by default; the Rgb8 arm keeps the test valid if
            // the default output preference ever changes.
            let container: Vec<u8> = if let Some(g) = buf.try_as_imgref::<rgb::Gray<u8>>() {
                assert_eq!((g.width(), g.height()), (w as usize, h as usize));
                (0..h as usize)
                    .flat_map(|y| {
                        let row = &g.buf()[y * g.stride()..][..w as usize];
                        row.iter().map(|p| p.value()).collect::<Vec<_>>()
                    })
                    .collect()
            } else {
                let c = buf
                    .try_as_imgref::<rgb::Rgb<u8>>()
                    .unwrap_or_else(|| panic!("{name}: neither Gray8 nor Rgb8 output"));
                assert_eq!((c.width(), c.height()), (w as usize, h as usize));
                (0..h as usize)
                    .flat_map(|y| {
                        let row = &c.buf()[y * c.stride()..][..w as usize];
                        row.iter().map(|p| p.r).collect::<Vec<_>>()
                    })
                    .collect()
            };
            for (i, (&cv, &rv)) in container.iter().zip(raw.iter()).enumerate() {
                assert_eq!(
                    cv,
                    rv,
                    "{name} (depth {depth}): raw-OBU gray {rv} != container gray {cv} at \
                     ({x},{y}) — the two mono paths disagree",
                    x = i % w as usize,
                    y = i / w as usize
                );
            }
        }
    }

    /// 4:2:2 decode, both plumbings, exact agreement.
    ///
    /// Nothing in the suite decodes a 4:2:2 AVIF: the in-repo encoder can only
    /// emit 4:4:4 and 4:2:0 (`EncodeChromaSubsampling`), so the 4:2:2 dispatch
    /// arms of `decoder_managed/plane_convert.rs` (`:388-395`, `:454-464`) and
    /// of `convert_to_rgb` here measured cold in every feature combo
    /// (cargo-llvm-cov, 2026-08-11; docs/TEST_COVERAGE.md). 4:2:2 is
    /// horizontal-only chroma upsampling — its own kernel, its own edge clamp.
    ///
    /// The two paths reach the same kernel family through different plumbing
    /// (raw-OBU strip entry + own buffer vs. the managed decoder's plane views
    /// and full-image entry), so byte identity is the invariant: a divergence
    /// is a bug in one of them. Uses 8-bit fixtures deliberately — at 10 bits
    /// the two paths narrow to 8 with different rounding, which is a separate
    /// question from the chroma math.
    ///
    /// Fixtures are the link-u corpus (`just download-linku`; CI provisions
    /// them). Fail-loud, no graceful skip.
    #[test]
    fn raw_obu_422_matches_the_container_path() {
        for name in [
            "fox.profile2.8bpc.yuv422.avif",
            "fox.profile2.8bpc.yuv422.odd-width.avif",
            "fox.profile2.8bpc.yuv422.odd-width.odd-height.avif",
            "hato.profile2.8bpc.yuv422.avif",
        ] {
            let path = format!("{}/tests/vectors/link-u/{name}", env!("CARGO_MANIFEST_DIR"));
            let file = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("read {path}: {e} (run: just download-linku)"));
            let parser = zenavif_parse::AvifParser::from_bytes(&file)
                .unwrap_or_else(|e| panic!("parse {name}: {e:?}"));
            let payload = parser
                .primary_data()
                .unwrap_or_else(|e| panic!("primary item of {name}: {e:?}"));

            let (raw, w, h, channels) = decode_av1_obu(&payload)
                .unwrap_or_else(|e| panic!("raw OBU decode of {name}: {e:?}"));
            assert_eq!(channels, 3, "{name}: 4:2:2 colour content decodes to RGB");

            let cfg = crate::config::DecoderConfig::new().prefer_8bit(true);
            let buf = crate::decode_with(&file, &cfg, &enough::Unstoppable)
                .unwrap_or_else(|e| panic!("container decode of {name}: {e:?}"));
            let img = buf
                .try_as_imgref::<rgb::Rgb<u8>>()
                .unwrap_or_else(|| panic!("{name}: expected Rgb8 output"));
            assert_eq!(
                (img.width(), img.height()),
                (w as usize, h as usize),
                "{name}: dimensions differ between the two paths"
            );
            for y in 0..h as usize {
                let row = &img.buf()[y * img.stride()..][..w as usize];
                for (x, px) in row.iter().enumerate() {
                    let i = (y * w as usize + x) * 3;
                    assert_eq!(
                        (px.r, px.g, px.b),
                        (raw[i], raw[i + 1], raw[i + 2]),
                        "{name}: 4:2:2 raw-OBU vs container decode disagree at ({x},{y}) \
                         — the two 4:2:2 plumbings produce different pixels"
                    );
                }
            }
        }
    }

    /// The raw-OBU IDENTITY (MC=0 / GBR) arm must put the planes back in RGB
    /// order.
    ///
    /// `convert_identity_to_rgb` measured **0 executions** across the whole
    /// feature matrix: tests/identity_roundtrip.rs covers the container path
    /// only, so nothing checked the raw-OBU plane reorder — a rotation there
    /// silently swaps channels. Tolerance is the same ±2 as
    /// tests/identity_roundtrip.rs (zenrav1e#9: `with_lossless` is not yet
    /// bit-exact); the paired swap assertion below proves that tolerance
    /// cannot absorb a channel rotation.
    #[cfg(feature = "encode-imazen")]
    #[test]
    fn raw_obu_identity_reorders_gbr_planes_to_rgb() {
        let (w, h) = (32usize, 32usize);
        let px: Vec<rgb::Rgb<u8>> = (0..h)
            .flat_map(|y| {
                (0..w).map(move |x| {
                    let mix = |salt: u32| -> u8 {
                        let mut v = (x as u32)
                            .wrapping_mul(0x9E37_79B9)
                            .wrapping_add((y as u32).wrapping_mul(0x85EB_CA6B))
                            ^ salt;
                        v ^= v >> 13;
                        (v.wrapping_mul(0xC2B2_AE35) >> 16) as u8
                    };
                    rgb::Rgb {
                        r: mix(1),
                        g: mix(2),
                        b: mix(3),
                    }
                })
            })
            .collect();
        let img = imgref::ImgVec::new(px.clone(), w, h);
        let cfg = crate::EncoderConfig::new()
            .speed(6)
            .threads(Some(1))
            .color_model(crate::EncodeColorModel::Rgb)
            .with_lossless(true);
        let enc = crate::encode_rgb8(
            img.as_ref(),
            &cfg,
            almost_enough::StopToken::new(almost_enough::Unstoppable),
        )
        .expect("identity lossless encode");
        let parser = zenavif_parse::AvifParser::from_bytes(&enc.avif_file).expect("parse");
        let payload = parser.primary_data().expect("primary item");

        let (raw, gw, gh, channels) = decode_av1_obu(&payload).expect("raw OBU identity decode");
        assert_eq!((gw as usize, gh as usize), (w, h));
        assert_eq!(channels, 3, "identity content decodes to RGB");

        let mut worst = 0i32;
        let mut worst_swapped = 0i32;
        for i in 0..w * h {
            let (e, g) = (px[i], &raw[i * 3..i * 3 + 3]);
            let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
            worst = worst.max(d(e.r, g[0]).max(d(e.g, g[1])).max(d(e.b, g[2])));
            // Same comparison with R and B swapped: what a plane rotation
            // would look like. Keeps the ±2 tolerance honest.
            worst_swapped = worst_swapped.max(d(e.b, g[0]).max(d(e.r, g[2])));
        }
        assert!(
            worst <= 2,
            "raw-OBU identity decode is off by {worst} (zenrav1e#9 allows 2) — plane \
             reorder or a YCbCr matrix applied to GBR planes"
        );
        assert!(
            worst_swapped > 8,
            "the R/B-swapped comparison is only off by {worst_swapped}, so this test \
             could not tell a channel rotation from a correct decode"
        );
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

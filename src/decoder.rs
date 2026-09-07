//! AVIF decoder implementation wrapping rav1d

#![allow(unsafe_code)]

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_without_frame_returns_error() {
        // A valid temporal delimiter OBU has no picture. End-of-input must
        // return an error, not poll EAGAIN forever.
        let mut decoder = Rav1dDecoder::new(&DecoderConfig::new().threads(1)).unwrap();
        assert!(decoder.decode(&[0x12, 0]).is_err());
    }
}

use crate::config::DecoderConfig;
use crate::convert::{add_alpha8, add_alpha16, downscale_to_8bit, scale_pixels_to_u16};
use crate::error::{Error, Result};
use crate::image::{
    ChromaSampling, ColorPrimaries, ColorRange, ImageInfo, MatrixCoefficients,
    TransferCharacteristics,
};
use crate::yuv_convert::{YuvMatrix, YuvRange as KernelRange};
use enough::Stop;
use rgb::{Rgb, Rgba};
use whereat::at;
use yuv::YuvRange;
use zenpixels::PixelBuffer;

// Conditionally import from rav1d or rav1d-safe based on feature
#[cfg(feature = "unsafe-asm")]
use rav1d::include::dav1d::data::Dav1dData;
#[cfg(feature = "unsafe-asm")]
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
#[cfg(feature = "unsafe-asm")]
use rav1d::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444, Dav1dPixelLayout, Rav1dSequenceHeader,
};
#[cfg(feature = "unsafe-asm")]
use rav1d::include::dav1d::picture::Dav1dPicture;
#[cfg(feature = "unsafe-asm")]
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};

#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::include::dav1d::data::Dav1dData;
#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444, Dav1dPixelLayout, Rav1dSequenceHeader,
};
#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::include::dav1d::picture::Dav1dPicture;
#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};
use std::ffi::c_int;
use std::ptr::NonNull;

/// Own the input reference even when sending fails or leaves data pending.
struct DecoderPacket(Dav1dData);

impl Drop for DecoderPacket {
    fn drop(&mut self) {
        // SAFETY: the data was initialized by dav1d_data_create; unref also
        // accepts the empty value left after dav1d_send_data consumes it.
        unsafe { dav1d_data_unref(NonNull::new(&mut self.0)) };
    }
}

/// Internal rav1d context wrapper with automatic cleanup
struct Rav1dDecoder {
    ctx: Option<Dav1dContext>,
}

impl Rav1dDecoder {
    /// Create a new rav1d decoder with the given configuration
    fn new(config: &DecoderConfig) -> Result<Self> {
        let mut settings = std::mem::MaybeUninit::<Dav1dSettings>::uninit();

        // SAFETY: dav1d_default_settings initializes the settings struct
        unsafe {
            dav1d_default_settings(NonNull::new(settings.as_mut_ptr()).unwrap());
        }

        let mut settings = unsafe { settings.assume_init() };
        settings.n_threads = config.threads as c_int;
        settings.apply_grain = config.apply_grain as c_int;
        settings.frame_size_limit = config.frame_size_limit;

        let mut ctx: Option<Dav1dContext> = None;

        // SAFETY: dav1d_open creates a new decoder context
        let result = unsafe {
            dav1d_open(
                NonNull::new(&mut ctx),
                NonNull::new(&mut settings).map(|p| p.cast()),
            )
        };

        if result.0 < 0 {
            return Err(at!(Error::Decode {
                code: result.0,
                msg: "failed to open decoder",
            }));
        }

        Ok(Self { ctx })
    }

    /// Decode AV1 data and return the picture
    fn decode(&mut self, data: &[u8]) -> Result<DecodedPicture> {
        // EAGAIN is 11 on Linux, 35 on macOS - rav1d uses -EAGAIN for "try again"
        #[cfg(target_os = "linux")]
        const EAGAIN: c_int = -11;
        #[cfg(target_os = "macos")]
        const EAGAIN: c_int = -35;
        #[cfg(target_os = "windows")]
        const EAGAIN: c_int = -11; // Windows doesn't use EAGAIN but use same value

        let ctx = self.ctx.ok_or_else(|| {
            at!(Error::Decode {
                code: -1,
                msg: "decoder context is null",
            })
        })?;

        // rav1d may retain the packet after returning a picture. Give it an
        // owned reference instead of wrapping the caller's borrowed slice with
        // a no-op free callback. Release our reference on every return path.
        let mut packet = DecoderPacket(Dav1dData::default());
        // SAFETY: packet.0 is valid writable storage for the initialized data.
        let dst = unsafe { dav1d_data_create(NonNull::new(&mut packet.0), data.len()) };
        if dst.is_null() {
            return Err(at!(Error::OutOfMemory));
        }
        // SAFETY: dav1d_data_create allocated data.len() bytes, disjoint from
        // the caller's slice. rav1d owns their lifetime through its references.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len()) };
        let dav1d_data = &mut packet.0;

        // Send data to decoder in a loop until all data is consumed
        // SAFETY: ctx is valid and dav1d_data has been initialized
        loop {
            let result = unsafe { dav1d_send_data(Some(ctx), NonNull::new(&mut *dav1d_data)) };

            if result.0 == 0 {
                // All data consumed
                break;
            } else if result.0 == EAGAIN {
                // Output queue is full, need to drain pictures first
                // For single-frame AVIF this shouldn't happen, but handle it
                let mut picture = Dav1dPicture::default();
                let pic_result =
                    unsafe { dav1d_get_picture(Some(ctx), NonNull::new(&mut picture)) };
                if pic_result.0 == 0 {
                    // Got a picture while draining
                    return Ok(DecodedPicture { picture });
                }
                return Err(at!(Error::Decode {
                    code: pic_result.0,
                    msg: "decoder could not drain pending input",
                }));
            } else if result.0 < 0 {
                return Err(at!(Error::Decode {
                    code: result.0,
                    msg: "failed to send data to decoder",
                }));
            }

            // If data.sz == 0, we're done
            if dav1d_data.sz == 0 {
                break;
            }
        }

        // The first get_picture call enables draining; a second call waits
        // for delayed frame-thread output (rav1d 1.1.0 rav1d_get_picture).
        // EAGAIN after that means more input is required, not asynchronous
        // work to busy-poll. This API has already supplied the whole packet.
        let mut picture = Dav1dPicture::default();
        for attempt in 0..2 {
            // SAFETY: ctx is live and picture is valid writable storage.
            let result = unsafe { dav1d_get_picture(Some(ctx), NonNull::new(&mut picture)) };
            if result.0 == 0 {
                return Ok(DecodedPicture { picture });
            }
            if result.0 != EAGAIN || attempt == 1 {
                return Err(at!(Error::Decode {
                    code: result.0,
                    msg: "AV1 packet did not produce a picture",
                }));
            }
        }
        unreachable!("the second drain attempt always returns")
    }
}

impl Drop for Rav1dDecoder {
    fn drop(&mut self) {
        if self.ctx.is_some() {
            // SAFETY: ctx is valid
            unsafe {
                dav1d_close(NonNull::new(&mut self.ctx));
            }
        }
    }
}

/// Wrapper around Dav1dPicture that handles cleanup
struct DecodedPicture {
    picture: Dav1dPicture,
}

impl DecodedPicture {
    /// Get image dimensions
    fn dimensions(&self) -> (u32, u32) {
        (self.picture.p.w as u32, self.picture.p.h as u32)
    }

    /// Get bit depth
    fn bit_depth(&self) -> u8 {
        self.picture.p.bpc as u8
    }

    /// Get pixel layout
    fn layout(&self) -> Dav1dPixelLayout {
        self.picture.p.layout
    }

    /// Get sequence header reference
    fn seq_hdr(&self) -> Option<&Rav1dSequenceHeader> {
        // SAFETY: seq_hdr_ref contains a reference to the sequence header
        // that is valid while picture is alive
        self.picture.seq_hdr_ref.as_ref().map(|arc| {
            // SAFETY: RawArc is valid while picture owns it
            // DRav1d derefs to the Rav1d type
            unsafe { &**arc.as_ref() }
        })
    }

    /// Extract Y plane data as a contiguous Vec with stride = width (copies the data)
    fn y_plane_u8(&self) -> Option<(Vec<u8>, usize, usize, usize)> {
        let (w, h) = self.dimensions();
        let stride = self.picture.stride[0] as usize;
        let data_ptr = self.picture.data[0]?;

        let mut pixels = Vec::with_capacity(w as usize * h as usize);
        for row in 0..h as usize {
            // SAFETY: data pointer is valid for stride * height bytes
            let row_start = unsafe { data_ptr.as_ptr().cast::<u8>().add(row * stride) };
            let row_slice = unsafe { std::slice::from_raw_parts(row_start, w as usize) };
            pixels.extend_from_slice(row_slice);
        }

        Some((pixels, w as usize, h as usize, stride))
    }

    /// Extract Y plane data as 16-bit contiguous Vec (copies the data)
    fn y_plane_u16(&self) -> Option<(Vec<u16>, usize, usize, usize)> {
        let (w, h) = self.dimensions();
        let stride = self.picture.stride[0] as usize;
        let data_ptr = self.picture.data[0]?;

        let mut pixels = Vec::with_capacity(w as usize * h as usize);
        for row in 0..h as usize {
            // SAFETY: data pointer is valid for stride * height bytes
            let row_start = unsafe { data_ptr.as_ptr().cast::<u8>().add(row * stride) };
            let row_slice =
                unsafe { std::slice::from_raw_parts(row_start.cast::<u16>(), w as usize) };
            pixels.extend_from_slice(row_slice);
        }

        Some((pixels, w as usize, h as usize, stride / 2))
    }

    /// Extract all YUV planes as 8-bit with stride = width (copies the data)
    fn yuv_planes_u8(&self) -> Option<YuvPlanes8> {
        let (w, h) = self.dimensions();
        let layout = self.layout();

        let y_stride = self.picture.stride[0] as usize;
        let uv_stride = self.picture.stride[1] as usize;

        let y_ptr = self.picture.data[0]?;
        let u_ptr = self.picture.data[1];
        let v_ptr = self.picture.data[2];

        // Calculate chroma dimensions based on layout
        let (chroma_w, chroma_h) = match layout {
            DAV1D_PIXEL_LAYOUT_I444 => (w as usize, h as usize),
            DAV1D_PIXEL_LAYOUT_I422 => ((w as usize).div_ceil(2), h as usize),
            DAV1D_PIXEL_LAYOUT_I420 => ((w as usize).div_ceil(2), (h as usize).div_ceil(2)),
            DAV1D_PIXEL_LAYOUT_I400 => (0, 0), // Monochrome
            _ => return None,
        };

        // Copy Y plane
        let mut y_data = Vec::with_capacity(w as usize * h as usize);
        for row in 0..h as usize {
            let row_start = unsafe { y_ptr.as_ptr().cast::<u8>().add(row * y_stride) };
            let row_slice = unsafe { std::slice::from_raw_parts(row_start, w as usize) };
            y_data.extend_from_slice(row_slice);
        }

        // Copy U and V planes if present
        let (u_data, v_data) = if layout != DAV1D_PIXEL_LAYOUT_I400 {
            let u_ptr = u_ptr?;
            let v_ptr = v_ptr?;

            let mut u_data = Vec::with_capacity(chroma_w * chroma_h);
            let mut v_data = Vec::with_capacity(chroma_w * chroma_h);

            for row in 0..chroma_h {
                let u_row_start = unsafe { u_ptr.as_ptr().cast::<u8>().add(row * uv_stride) };
                let v_row_start = unsafe { v_ptr.as_ptr().cast::<u8>().add(row * uv_stride) };

                let u_row = unsafe { std::slice::from_raw_parts(u_row_start, chroma_w) };
                let v_row = unsafe { std::slice::from_raw_parts(v_row_start, chroma_w) };

                u_data.extend_from_slice(u_row);
                v_data.extend_from_slice(v_row);
            }

            (u_data, v_data)
        } else {
            (Vec::new(), Vec::new())
        };

        Some(YuvPlanes8 {
            y: y_data,
            u: u_data,
            v: v_data,
            width: w as usize,
            height: h as usize,
            chroma_width: chroma_w,
            chroma_height: chroma_h,
            layout,
        })
    }

    /// Extract all YUV planes as 16-bit with stride = width (copies the data)
    fn yuv_planes_u16(&self) -> Option<YuvPlanes16> {
        let (w, h) = self.dimensions();
        let layout = self.layout();

        let y_stride = self.picture.stride[0] as usize / 2; // In u16 units
        let uv_stride = self.picture.stride[1] as usize / 2;

        let y_ptr = self.picture.data[0]?;
        let u_ptr = self.picture.data[1];
        let v_ptr = self.picture.data[2];

        // Calculate chroma dimensions based on layout
        let (chroma_w, chroma_h) = match layout {
            DAV1D_PIXEL_LAYOUT_I444 => (w as usize, h as usize),
            DAV1D_PIXEL_LAYOUT_I422 => ((w as usize).div_ceil(2), h as usize),
            DAV1D_PIXEL_LAYOUT_I420 => ((w as usize).div_ceil(2), (h as usize).div_ceil(2)),
            DAV1D_PIXEL_LAYOUT_I400 => (0, 0),
            _ => return None,
        };

        // Copy Y plane
        let mut y_data = Vec::with_capacity(w as usize * h as usize);
        for row in 0..h as usize {
            let row_start = unsafe { y_ptr.as_ptr().cast::<u16>().add(row * y_stride) };
            let row_slice = unsafe { std::slice::from_raw_parts(row_start, w as usize) };
            y_data.extend_from_slice(row_slice);
        }

        // Copy U and V planes if present
        let (u_data, v_data) = if layout != DAV1D_PIXEL_LAYOUT_I400 {
            let u_ptr = u_ptr?;
            let v_ptr = v_ptr?;

            let mut u_data = Vec::with_capacity(chroma_w * chroma_h);
            let mut v_data = Vec::with_capacity(chroma_w * chroma_h);

            for row in 0..chroma_h {
                let u_row_start = unsafe { u_ptr.as_ptr().cast::<u16>().add(row * uv_stride) };
                let v_row_start = unsafe { v_ptr.as_ptr().cast::<u16>().add(row * uv_stride) };

                let u_row = unsafe { std::slice::from_raw_parts(u_row_start, chroma_w) };
                let v_row = unsafe { std::slice::from_raw_parts(v_row_start, chroma_w) };

                u_data.extend_from_slice(u_row);
                v_data.extend_from_slice(v_row);
            }

            (u_data, v_data)
        } else {
            (Vec::new(), Vec::new())
        };

        Some(YuvPlanes16 {
            y: y_data,
            u: u_data,
            v: v_data,
            width: w as usize,
            height: h as usize,
            chroma_width: chroma_w,
            chroma_height: chroma_h,
            layout,
        })
    }
}

impl Drop for DecodedPicture {
    fn drop(&mut self) {
        // SAFETY: picture was initialized by dav1d_get_picture
        unsafe {
            dav1d_picture_unref(NonNull::new(&mut self.picture));
        }
    }
}

/// 8-bit YUV plane data (contiguous, stride = width)
struct YuvPlanes8 {
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    width: usize,
    height: usize,
    chroma_width: usize,
    #[allow(dead_code)]
    chroma_height: usize,
    layout: Dav1dPixelLayout,
}

impl YuvPlanes8 {
    fn chroma_sampling(&self) -> ChromaSampling {
        match self.layout {
            DAV1D_PIXEL_LAYOUT_I444 => ChromaSampling::Cs444,
            DAV1D_PIXEL_LAYOUT_I422 => ChromaSampling::Cs422,
            DAV1D_PIXEL_LAYOUT_I420 => ChromaSampling::Cs420,
            DAV1D_PIXEL_LAYOUT_I400 => ChromaSampling::Monochrome,
            _ => ChromaSampling::Cs420,
        }
    }
}

/// 16-bit YUV plane data (contiguous, stride = width)
struct YuvPlanes16 {
    y: Vec<u16>,
    u: Vec<u16>,
    v: Vec<u16>,
    width: usize,
    height: usize,
    chroma_width: usize,
    #[allow(dead_code)]
    chroma_height: usize,
    layout: Dav1dPixelLayout,
}

impl YuvPlanes16 {
    fn chroma_sampling(&self) -> ChromaSampling {
        match self.layout {
            DAV1D_PIXEL_LAYOUT_I444 => ChromaSampling::Cs444,
            DAV1D_PIXEL_LAYOUT_I422 => ChromaSampling::Cs422,
            DAV1D_PIXEL_LAYOUT_I420 => ChromaSampling::Cs420,
            DAV1D_PIXEL_LAYOUT_I400 => ChromaSampling::Monochrome,
            _ => ChromaSampling::Cs420,
        }
    }
}

/// The same range convention used by the managed and raw-OBU kernels.
fn kernel_range(range: YuvRange) -> KernelRange {
    match range {
        YuvRange::Full => KernelRange::Full,
        YuvRange::Limited => KernelRange::Limited,
    }
}

/// Convert rav1d color range to yuv crate's YuvRange
fn to_yuv_range(color_range: u8) -> YuvRange {
    if color_range != 0 {
        YuvRange::Full
    } else {
        YuvRange::Limited
    }
}

/// Convert rav1d color range to zenavif ColorRange
fn to_color_range(color_range: u8) -> ColorRange {
    if color_range != 0 {
        ColorRange::Full
    } else {
        ColorRange::Limited
    }
}

/// Decode a raw AV1 OBU temporal unit to tight `u16` YUV planes via the
/// rav1d FFI decoder (upstream rav1d with its full hand-written asm when the
/// `unsafe-asm` feature selects the `rav1d` crate).
///
/// This is the `Av1Backend::Rav1dFfi` arm of the raw-OBU decode seam in
/// `decode_av1.rs` — single-threaded (threads=1) to match the seam's other
/// backends, output shape identical to theirs.
pub(crate) fn decode_obu_yuv_ffi(data: &[u8]) -> Result<crate::decode_av1::DecodedYuv> {
    use crate::decode_av1::DecodedYuv;

    if data.is_empty() {
        return Err(at!(Error::Decode {
            code: -1,
            msg: "empty AV1 OBU data",
        }));
    }
    let config = DecoderConfig::default().threads(1);
    let mut decoder = Rav1dDecoder::new(&config)?;
    let picture = decoder.decode(data)?;

    let (w, h) = picture.dimensions();
    let bit_depth = picture.bit_depth() as i32;
    let layout = picture.layout();
    let monochrome = layout == DAV1D_PIXEL_LAYOUT_I400;
    let (subsampling_x, subsampling_y) = match layout {
        DAV1D_PIXEL_LAYOUT_I422 => (1, 0),
        DAV1D_PIXEL_LAYOUT_I444 => (0, 0),
        // I420 and I400 both signal 1,1 in the sequence header.
        _ => (1, 1),
    };

    let (y, u, v, width_uv, height_uv) = if bit_depth > 8 {
        let p = picture.yuv_planes_u16().ok_or_else(|| {
            at!(Error::Decode {
                code: -1,
                msg: "rav1d FFI picture had no plane data",
            })
        })?;
        (p.y, p.u, p.v, p.chroma_width, p.chroma_height)
    } else {
        let p = picture.yuv_planes_u8().ok_or_else(|| {
            at!(Error::Decode {
                code: -1,
                msg: "rav1d FFI picture had no plane data",
            })
        })?;
        (
            p.y.iter().map(|&s| s as u16).collect(),
            p.u.iter().map(|&s| s as u16).collect(),
            p.v.iter().map(|&s| s as u16).collect(),
            p.chroma_width,
            p.chroma_height,
        )
    };

    Ok(DecodedYuv {
        y,
        u,
        v,
        width: w as usize,
        height: h as usize,
        width_uv,
        height_uv,
        bit_depth,
        monochrome,
        subsampling_x,
        subsampling_y,
    })
}

/// AVIF decoder
pub struct AvifDecoder {
    parser: zenavif_parse::AvifParser<'static>,
    config: DecoderConfig,
    info: ImageInfo,
}

impl AvifDecoder {
    /// Create a new AVIF decoder from raw data
    ///
    /// This parses the AVIF container but does not decode the AV1 data yet.
    pub fn new(data: &[u8], config: &DecoderConfig) -> Result<Self> {
        // Zero-copy AvifParser — primary/alpha data returned as Cow::Borrowed.
        //
        // STRICT container validation, deliberately. `DecodeConfig::default()`
        // is strict; this call site must not re-enable `lenient(true)`.
        //
        // History, so it is not re-broken: this line used to read
        // `DecodeConfig::default().lenient(true)`, justified by the comment
        // "Use lenient parsing to handle files with non-critical validation
        // issues". Commit 0a6606a replaced that comment with a note about
        // zero-copy parsing but kept the `.lenient(true)`, so the reason was
        // gone while the behaviour stayed. What it silently bought was four
        // downgraded container conformance checks — non-zero reserved flags,
        // and three `essential`-flag rules, the worst of which lets an item
        // carrying an *unknown property marked essential* decode with nothing
        // but a log line, even though such an item is by definition unusable.
        //
        // Measured before removing it: of the 227 AVIF files in this repo's
        // corpus, exactly two needed anything from leniency, and both are now
        // handled precisely inside zenavif-parse (`read_pixi` for the extended
        // `pixi` form, and the mislabelled-essential warning for a supported
        // property). See `tests/parser_leniency_scope.rs`.
        let mut parse_config = zenavif_parse::DecodeConfig::default();
        // Forward resource limits to the parser when configured.
        if let Some(mem) = config.parser_peak_memory_limit {
            parse_config = parse_config.with_peak_memory_limit(mem);
        }
        if let Some(mp) = config.parser_total_megapixels_limit {
            parse_config = parse_config.with_total_megapixels_limit(mp);
        }
        if let Some(frames) = config.parser_max_animation_frames {
            parse_config = parse_config.with_max_animation_frames(frames);
        }
        let parser = zenavif_parse::AvifParser::from_owned_with_config(
            data.to_vec(),
            &parse_config,
            &enough::Unstoppable,
        )
        .map_err(|e| e.map_error(Error::Parse))?;

        // Extract metadata from the parsed AVIF. Like the default rav1d-safe
        // backend (decoder_managed), tolerate a metadata-parse failure here: a
        // corrupt AV1 payload is a decode-stage failure, so deferring lets the
        // error surface from the decode pipeline (carrying its whereat trace)
        // instead of as an eager container-parse rejection at construction.
        // See tests/whereat_trace_preservation.rs.
        let info = match parser.primary_metadata() {
            Ok(metadata) => {
                // Reject oversized frames up front when a limit is configured.
                if config.frame_size_limit > 0 {
                    let total_pixels = metadata
                        .max_frame_width
                        .get()
                        .saturating_mul(metadata.max_frame_height.get());
                    if total_pixels > config.frame_size_limit {
                        return Err(at!(Error::ImageTooLarge {
                            width: metadata.max_frame_width.get(),
                            height: metadata.max_frame_height.get(),
                        }));
                    }
                }

                let cs = metadata.chroma_subsampling;
                let chroma_sampling = if cs.horizontal && cs.vertical {
                    ChromaSampling::Cs420
                } else if cs.horizontal {
                    ChromaSampling::Cs422
                } else {
                    ChromaSampling::Cs444
                };

                let (color_primaries, transfer_characteristics) = match parser.nclx_color_info() {
                    Some(zenavif_parse::ColorInformation::Nclx {
                        color_primaries,
                        transfer_characteristics,
                        ..
                    }) => (
                        ColorPrimaries(*color_primaries as u8),
                        TransferCharacteristics(*transfer_characteristics as u8),
                    ),
                    _ => (
                        ColorPrimaries(metadata.color_primaries),
                        TransferCharacteristics(metadata.transfer_characteristics),
                    ),
                };
                let icc_profile = match parser.color_info() {
                    Some(zenavif_parse::ColorInformation::IccProfile(icc)) => Some(icc.clone()),
                    _ => None,
                };
                ImageInfo {
                    width: metadata.max_frame_width.get(),
                    height: metadata.max_frame_height.get(),
                    bit_depth: metadata.bit_depth,
                    has_alpha: parser.alpha_data().is_some(),
                    premultiplied_alpha: parser.premultiplied_alpha(),
                    monochrome: metadata.monochrome,
                    color_primaries,
                    transfer_characteristics,
                    matrix_coefficients: MatrixCoefficients(metadata.matrix_coefficients),
                    color_range: if metadata.full_range {
                        ColorRange::Full
                    } else {
                        ColorRange::Limited
                    },
                    chroma_sampling,
                    icc_profile,
                    rotation: None,
                    mirror: None,
                    clean_aperture: None,
                    pixel_aspect_ratio: None,
                    content_light_level: None,
                    mastering_display: None,
                    exif: None,
                    xmp: None,
                    gain_map: None,
                    depth_map: None,
                }
            }
            // Corrupt/unreadable metadata: defer to decode(), which produces the
            // real decode-stage error (with its trace) when the payload is fed
            // to the AV1 decoder.
            Err(_) => ImageInfo::default(),
        };

        Ok(Self {
            parser,
            config: config.clone(),
            info,
        })
    }

    /// Get image metadata
    pub fn info(&self) -> &ImageInfo {
        &self.info
    }

    /// Decode the AVIF image
    pub fn decode(&mut self, stop: &(impl Stop + ?Sized)) -> Result<PixelBuffer> {
        // Check for cancellation before starting decode
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        if let Some(grid) = self.parser.grid_config() {
            // A grid descriptor is container syntax, never an AV1 packet.
            // Match the current managed path's explicit transparency limit.
            if self.parser.alpha_data().is_some() || self.parser.has_alpha_aux_items() {
                return Err(at!(Error::Unsupported(
                    "grid AVIF with alpha auxiliary items: alpha-grid stitching is not implemented"
                )));
            }
            let (width, height) = (grid.output_width, grid.output_height);
            if self.config.frame_size_limit > 0
                && u64::from(width) * u64::from(height) > u64::from(self.config.frame_size_limit)
            {
                return Err(at!(Error::ImageTooLarge { width, height }));
            }
            let count = usize::from(grid.rows) * usize::from(grid.columns);
            if count == 0 || count != self.parser.grid_tile_count() {
                return Err(at!(Error::Malformed(
                    "tile count does not match grid dimensions"
                )));
            }
            let mut tiles =
                crate::alloc_util::vec_with_capacity(self.config.alloc_pref, true, count)?;
            for i in 0..count {
                stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
                let data = self
                    .parser
                    .tile_data(i)
                    .map_err(|e| e.map_error(Error::Parse))?;
                tiles.push(self.decode_coded_item(&data, stop)?);
            }
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
            return crate::decoder_managed::grid::stitch_tile_images(
                tiles,
                usize::from(grid.columns),
                width as usize,
                height as usize,
                self.config.alloc_pref,
            );
        }
        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| e.map_error(Error::Parse))?;
        self.decode_coded_item(&primary_data, stop)
    }

    fn decode_coded_item(&self, data: &[u8], stop: &(impl Stop + ?Sized)) -> Result<PixelBuffer> {
        let mut decoder = Rav1dDecoder::new(&self.config)?;
        let color_picture = decoder.decode(data)?;

        // Check for cancellation after color decode
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Get color info from sequence header
        let seq_hdr = color_picture.seq_hdr();
        let yuv_range = seq_hdr
            .map(|h| to_yuv_range(h.color_range))
            .unwrap_or(YuvRange::Limited);
        let _color_range = seq_hdr
            .map(|h| to_color_range(h.color_range))
            .unwrap_or(ColorRange::Limited);

        // Resolve the same H.273 recipe as the managed decoder, including
        // container hints for unspecified matrices and derived coefficients.
        let mut primaries = seq_hdr.map(|h| h.pri.0).unwrap_or(2);
        let mut hint = crate::cicp_resolve::AVIF_DEFAULT_MC;
        if let Some(zenavif_parse::ColorInformation::Nclx {
            color_primaries,
            matrix_coefficients,
            ..
        }) = self.parser.nclx_color_info()
        {
            primaries = *color_primaries as u8;
            if crate::cicp_resolve::is_resolvable_hint(*matrix_coefficients as u8) {
                hint = *matrix_coefficients as u8;
            }
        }
        let resolved = crate::cicp_resolve::resolve(
            seq_hdr.map(|h| h.mtrx.0).unwrap_or(2),
            primaries,
            Some(hint),
        )?;
        let is_identity = matches!(resolved, crate::cicp_resolve::ResolvedMatrix::Identity);
        // Identity is dispatched separately and does not use this matrix.
        let matrix = resolved.to_our().unwrap_or(YuvMatrix::Bt601);

        let bit_depth = color_picture.bit_depth();
        let has_alpha = self.parser.alpha_data().is_some();

        // Convert through the same kernels as managed and raw-OBU decoding
        let mut image = if bit_depth == 8 {
            let planes = color_picture.yuv_planes_u8().ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "failed to extract YUV planes",
                })
            })?;

            match planes.chroma_sampling() {
                ChromaSampling::Monochrome => {
                    self.convert_mono8(&planes, yuv_range, matrix, has_alpha)?
                }
                ChromaSampling::Cs444 if is_identity => {
                    self.convert_identity8(&planes, yuv_range, has_alpha)?
                }
                _ if is_identity => {
                    return Err(at!(Error::Unsupported(
                        "matrix_coefficients=0 (identity/GBR) requires 4:4:4 chroma; \
                         subsampled identity has no defined reconstruction"
                    )));
                }
                _ => self.convert_yuv8(&planes, yuv_range, matrix, has_alpha)?,
            }
        } else {
            let planes = color_picture.yuv_planes_u16().ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "failed to extract YUV planes",
                })
            })?;

            match planes.chroma_sampling() {
                ChromaSampling::Monochrome => {
                    self.convert_mono16(&planes, yuv_range, matrix, bit_depth, has_alpha)?
                }
                ChromaSampling::Cs444 if is_identity => {
                    self.convert_identity16(&planes, yuv_range, bit_depth, has_alpha)?
                }
                _ if is_identity => {
                    return Err(at!(Error::Unsupported(
                        "matrix_coefficients=0 (identity/GBR) requires 4:4:4 chroma; \
                         subsampled identity has no defined reconstruction"
                    )));
                }
                _ => self.convert_yuv16(&planes, yuv_range, matrix, bit_depth, has_alpha)?,
            }
        };

        // Drop color picture before decoding alpha
        drop(color_picture);

        // Check for cancellation before alpha decode
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // add_alpha16 scales the alpha samples itself and unpremultiplies
        // against full-range RGB. Expand color before attaching alpha.
        if bit_depth > 8 && bit_depth < 16 {
            scale_pixels_to_u16(&mut image, bit_depth);
        }

        // Decode alpha channel if present
        if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| e.map_error(Error::Parse))?;
            let alpha_picture = decoder.decode(&alpha_data)?;

            let alpha_color_range = alpha_picture
                .seq_hdr()
                .map(|h| to_color_range(h.color_range))
                .unwrap_or(ColorRange::Limited);

            let alpha_bit_depth = alpha_picture.bit_depth();
            let premultiplied = self.parser.premultiplied_alpha();

            if alpha_bit_depth == 8 {
                let (y_data, width, height, _) = alpha_picture.y_plane_u8().ok_or_else(|| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "failed to extract alpha plane",
                    })
                })?;

                add_alpha8(
                    &mut image,
                    y_data.chunks(width),
                    width,
                    height,
                    alpha_color_range,
                    premultiplied,
                )?;
            } else {
                let (y_data, width, height, _) = alpha_picture.y_plane_u16().ok_or_else(|| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "failed to extract alpha plane",
                    })
                })?;

                add_alpha16(
                    &mut image,
                    y_data.chunks(width),
                    width,
                    height,
                    alpha_color_range,
                    alpha_bit_depth,
                    premultiplied,
                )?;
            }
        }

        if self.config.prefer_8bit && bit_depth > 8 {
            image = downscale_to_8bit(image);
        }

        Ok(image)
    }

    fn convert_mono8(
        &self,
        planes: &YuvPlanes8,
        yuv_range: YuvRange,
        _matrix: YuvMatrix,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let (width, height) = (planes.width, planes.height);
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let range = kernel_range(yuv_range);
        if has_alpha {
            let mut out = crate::alloc_util::alloc_filled(
                self.config.alloc_pref,
                true,
                Rgba::<u8>::default(),
                pixel_count,
            )?;
            crate::yuv_convert::yuv400_to_rgbx_strip::<u8, Rgba<u8>>(
                &planes.y, width, width, 0, height, range, 8, &mut out,
            );
            PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map(Into::into)
                .map_err(|_| at!(Error::OutOfMemory))
        } else {
            let mut out = crate::alloc_util::alloc_filled(
                self.config.alloc_pref,
                true,
                Rgb::<u8>::default(),
                pixel_count,
            )?;
            crate::yuv_convert::yuv400_to_rgbx_strip::<u8, Rgb<u8>>(
                &planes.y, width, width, 0, height, range, 8, &mut out,
            );
            PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map(Into::into)
                .map_err(|_| at!(Error::OutOfMemory))
        }
    }

    fn convert_mono16(
        &self,
        planes: &YuvPlanes16,
        yuv_range: YuvRange,
        _matrix: YuvMatrix,
        bit_depth: u8,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let (width, height) = (planes.width, planes.height);
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let range = kernel_range(yuv_range);
        if has_alpha {
            let mut out = crate::alloc_util::alloc_filled(
                self.config.alloc_pref,
                true,
                Rgba::<u16>::default(),
                pixel_count,
            )?;
            crate::yuv_convert::yuv400_to_rgbx_strip::<u16, Rgba<u16>>(
                &planes.y, width, width, 0, height, range, bit_depth, &mut out,
            );
            PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map(Into::into)
                .map_err(|_| at!(Error::OutOfMemory))
        } else {
            let mut out = crate::alloc_util::alloc_filled(
                self.config.alloc_pref,
                true,
                Rgb::<u16>::default(),
                pixel_count,
            )?;
            crate::yuv_convert::yuv400_to_rgbx_strip::<u16, Rgb<u16>>(
                &planes.y, width, width, 0, height, range, bit_depth, &mut out,
            );
            PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map(Into::into)
                .map_err(|_| at!(Error::OutOfMemory))
        }
    }

    fn convert_yuv8(
        &self,
        planes: &YuvPlanes8,
        yuv_range: YuvRange,
        matrix: YuvMatrix,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let (width, height) = (planes.width, planes.height);
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let range = kernel_range(yuv_range);
        if has_alpha {
            let mut out = crate::alloc_util::alloc_filled(
                self.config.alloc_pref,
                true,
                Rgba::<u8>::default(),
                pixel_count,
            )?;
            match planes.chroma_sampling() {
                ChromaSampling::Cs420 => crate::yuv_convert::yuv420_to_rgba8_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    height,
                    0,
                    height,
                    range,
                    matrix,
                    &mut out,
                ),
                ChromaSampling::Cs422 => crate::yuv_convert::yuv422_to_rgba8_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    0,
                    height,
                    range,
                    matrix,
                    &mut out,
                ),
                ChromaSampling::Cs444 => crate::yuv_convert::yuv444_to_rgba8_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    0,
                    height,
                    range,
                    matrix,
                    &mut out,
                ),
                ChromaSampling::Monochrome => {
                    return Err(at!(Error::Malformed(
                        "monochrome frame reached planar conversion"
                    )));
                }
            }
            PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map(Into::into)
                .map_err(|_| at!(Error::OutOfMemory))
        } else {
            let mut out = crate::alloc_util::alloc_filled(
                self.config.alloc_pref,
                true,
                Rgb::<u8>::default(),
                pixel_count,
            )?;
            match planes.chroma_sampling() {
                ChromaSampling::Cs420 => crate::yuv_convert::yuv420_to_rgb8_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    height,
                    0,
                    height,
                    range,
                    matrix,
                    &mut out,
                ),
                ChromaSampling::Cs422 => crate::yuv_convert::yuv422_to_rgb8_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    0,
                    height,
                    range,
                    matrix,
                    &mut out,
                ),
                ChromaSampling::Cs444 => crate::yuv_convert::yuv444_to_rgb8_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    0,
                    height,
                    range,
                    matrix,
                    &mut out,
                ),
                ChromaSampling::Monochrome => {
                    return Err(at!(Error::Malformed(
                        "monochrome frame reached planar conversion"
                    )));
                }
            }
            PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map(Into::into)
                .map_err(|_| at!(Error::OutOfMemory))
        }
    }

    fn convert_identity8(
        &self,
        planes: &YuvPlanes8,
        yuv_range: YuvRange,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let (width, height) = (planes.width, planes.height);
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let limited = matches!(yuv_range, YuvRange::Limited);
        // H.273 full-range-flag: limited identity uses the luma range (16–235)
        // on all three planes.
        let map = |v: u8| -> u8 {
            if limited {
                let c = u32::from(v.saturating_sub(16)).min(219);
                ((c * 255 + 109) / 219) as u8
            } else {
                v
            }
        };
        // planes.y = G, planes.u = B, planes.v = R.
        if has_alpha {
            let mut out: Vec<Rgba<u8>> = Vec::with_capacity(pixel_count);
            for i in 0..pixel_count {
                out.push(Rgba {
                    r: map(planes.v[i]),
                    g: map(planes.y[i]),
                    b: map(planes.u[i]),
                    a: 255,
                });
            }
            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        } else {
            let mut out: Vec<Rgb<u8>> = Vec::with_capacity(pixel_count);
            for i in 0..pixel_count {
                out.push(Rgb {
                    r: map(planes.v[i]),
                    g: map(planes.y[i]),
                    b: map(planes.u[i]),
                });
            }
            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        }
    }

    /// MC=0 (Identity / GBR), 10/12/16-bit, 4:4:4. Output is native bit depth
    /// (scaled to full u16 by the caller). Mirrors `convert_16bit_identity`.
    fn convert_identity16(
        &self,
        planes: &YuvPlanes16,
        yuv_range: YuvRange,
        bit_depth: u8,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let (width, height) = (planes.width, planes.height);
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let limited = matches!(yuv_range, YuvRange::Limited);
        let max = (1u32 << bit_depth) - 1;
        let smin = 16u32 << (bit_depth - 8);
        let span = 219u32 << (bit_depth - 8);
        let map = |v: u16| -> u16 {
            if limited {
                let c = u32::from(v).saturating_sub(smin).min(span);
                ((c * max + span / 2) / span) as u16
            } else {
                v
            }
        };
        if has_alpha {
            let mut out: Vec<Rgba<u16>> = Vec::with_capacity(pixel_count);
            for i in 0..pixel_count {
                out.push(Rgba {
                    r: map(planes.v[i]),
                    g: map(planes.y[i]),
                    b: map(planes.u[i]),
                    a: max as u16,
                });
            }
            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        } else {
            let mut out: Vec<Rgb<u16>> = Vec::with_capacity(pixel_count);
            for i in 0..pixel_count {
                out.push(Rgb {
                    r: map(planes.v[i]),
                    g: map(planes.y[i]),
                    b: map(planes.u[i]),
                });
            }
            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        }
    }

    fn convert_yuv16(
        &self,
        planes: &YuvPlanes16,
        yuv_range: YuvRange,
        matrix: YuvMatrix,
        bit_depth: u8,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let (width, height) = (planes.width, planes.height);
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let range = kernel_range(yuv_range);
        if has_alpha {
            let mut out = crate::alloc_util::alloc_filled(
                self.config.alloc_pref,
                true,
                Rgba::<u16>::default(),
                pixel_count,
            )?;
            match planes.chroma_sampling() {
                ChromaSampling::Cs420 => crate::yuv_convert::yuv420_to_rgba16_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    height,
                    0,
                    height,
                    range,
                    matrix,
                    bit_depth,
                    &mut out,
                ),
                ChromaSampling::Cs422 => crate::yuv_convert::yuv422_to_rgba16_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    0,
                    height,
                    range,
                    matrix,
                    bit_depth,
                    &mut out,
                ),
                ChromaSampling::Cs444 => crate::yuv_convert::yuv444_to_rgba16_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    0,
                    height,
                    range,
                    matrix,
                    bit_depth,
                    &mut out,
                ),
                ChromaSampling::Monochrome => {
                    return Err(at!(Error::Malformed(
                        "monochrome frame reached planar conversion"
                    )));
                }
            }
            PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map(Into::into)
                .map_err(|_| at!(Error::OutOfMemory))
        } else {
            let mut out = crate::alloc_util::alloc_filled(
                self.config.alloc_pref,
                true,
                Rgb::<u16>::default(),
                pixel_count,
            )?;
            match planes.chroma_sampling() {
                ChromaSampling::Cs420 => crate::yuv_convert::yuv420_to_rgb16_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    height,
                    0,
                    height,
                    range,
                    matrix,
                    bit_depth,
                    &mut out,
                ),
                ChromaSampling::Cs422 => crate::yuv_convert::yuv422_to_rgb16_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    0,
                    height,
                    range,
                    matrix,
                    bit_depth,
                    &mut out,
                ),
                ChromaSampling::Cs444 => crate::yuv_convert::yuv444_to_rgb16_strip(
                    &planes.y,
                    width,
                    &planes.u,
                    planes.chroma_width,
                    &planes.v,
                    planes.chroma_width,
                    width,
                    0,
                    height,
                    range,
                    matrix,
                    bit_depth,
                    &mut out,
                ),
                ChromaSampling::Monochrome => {
                    return Err(at!(Error::Malformed(
                        "monochrome frame reached planar conversion"
                    )));
                }
            }
            PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map(Into::into)
                .map_err(|_| at!(Error::OutOfMemory))
        }
    }
}

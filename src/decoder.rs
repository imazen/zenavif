//! AVIF decoder implementation wrapping rav1d

#![allow(unsafe_code)]

use crate::config::DecoderConfig;
use crate::convert::{add_alpha8, add_alpha16, downscale_to_8bit, scale_pixels_to_u16};
use crate::error::{Error, Result};
use crate::image::{
    ChromaSampling, ColorPrimaries, ColorRange, ImageInfo, MatrixCoefficients,
    TransferCharacteristics,
};
use enough::Stop;
use rgb::{Rgb, Rgba};
use whereat::at;
use yuv::{YuvGrayImage, YuvPlanarImage, YuvRange, YuvStandardMatrix};
use zenpixels::PixelBuffer;

// Conditionally import from rav1d or rav1d-safe based on feature
#[cfg(feature = "unsafe-asm")]
use rav1d::include::dav1d::data::Dav1dData;
#[cfg(feature = "unsafe-asm")]
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
#[cfg(feature = "unsafe-asm")]
use rav1d::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444, Dav1dPixelLayout, Rav1dMatrixCoefficients, Rav1dSequenceHeader,
};
#[cfg(feature = "unsafe-asm")]
use rav1d::include::dav1d::picture::Dav1dPicture;
#[cfg(feature = "unsafe-asm")]
use rav1d::src::lib::{
    dav1d_close, dav1d_data_wrap, dav1d_default_settings, dav1d_get_picture, dav1d_open,
    dav1d_picture_unref, dav1d_send_data,
};
#[cfg(feature = "unsafe-asm")]
use rav1d::src::send_sync_non_null::SendSyncNonNull;

#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::include::dav1d::data::Dav1dData;
#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444, Dav1dPixelLayout, Rav1dMatrixCoefficients, Rav1dSequenceHeader,
};
#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::include::dav1d::picture::Dav1dPicture;
#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::src::lib::{
    dav1d_close, dav1d_data_wrap, dav1d_default_settings, dav1d_get_picture, dav1d_open,
    dav1d_picture_unref, dav1d_send_data,
};
#[cfg(not(feature = "unsafe-asm"))]
use rav1d_safe::src::send_sync_non_null::SendSyncNonNull;
use std::ffi::c_int;
use std::ffi::c_void;
use std::ptr::NonNull;

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

        // Wrap the input data
        let mut dav1d_data = Dav1dData::default();

        // We need to keep the data alive for the duration of decode.
        // We pass a null free callback since we manage the lifetime ourselves.
        unsafe extern "C" fn null_free(_data: *const u8, _cookie: Option<SendSyncNonNull<c_void>>) {
        }

        // SAFETY: dav1d_data_wrap wraps the data pointer
        let result = unsafe {
            dav1d_data_wrap(
                NonNull::new(&mut dav1d_data),
                NonNull::new(data.as_ptr() as *mut u8),
                data.len(),
                Some(null_free),
                None,
            )
        };

        if result.0 < 0 {
            return Err(at!(Error::Decode {
                code: result.0,
                msg: "failed to wrap data",
            }));
        }

        // Send data to decoder in a loop until all data is consumed
        // SAFETY: ctx is valid and dav1d_data has been initialized
        loop {
            let result = unsafe { dav1d_send_data(Some(ctx), NonNull::new(&mut dav1d_data)) };

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
                // Otherwise continue trying to send
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

        // Get the decoded picture - keep trying if EAGAIN
        let mut picture = Dav1dPicture::default();
        loop {
            // SAFETY: ctx is valid and picture is initialized
            let result = unsafe { dav1d_get_picture(Some(ctx), NonNull::new(&mut picture)) };

            if result.0 == 0 {
                return Ok(DecodedPicture { picture });
            } else if result.0 == EAGAIN {
                // No picture ready yet, this can happen if decoding is async
                // For single-frame this shouldn't loop forever
                std::thread::yield_now();
                continue;
            } else {
                return Err(at!(Error::Decode {
                    code: result.0,
                    msg: "failed to get picture",
                }));
            }
        }
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

/// Convert rav1d matrix coefficients to yuv crate's YuvStandardMatrix
fn to_yuv_matrix(mc: Rav1dMatrixCoefficients) -> YuvStandardMatrix {
    match mc {
        Rav1dMatrixCoefficients::BT709 => YuvStandardMatrix::Bt709,
        Rav1dMatrixCoefficients::FCC => YuvStandardMatrix::Fcc,
        Rav1dMatrixCoefficients::BT470BG => YuvStandardMatrix::Bt470_6,
        Rav1dMatrixCoefficients::BT601 => YuvStandardMatrix::Bt601,
        Rav1dMatrixCoefficients::SMPTE240 => YuvStandardMatrix::Smpte240,
        Rav1dMatrixCoefficients::BT2020_NCL | Rav1dMatrixCoefficients::BT2020_CL => {
            YuvStandardMatrix::Bt2020
        }
        _ => YuvStandardMatrix::Bt601, // Default fallback
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
    let mut config = DecoderConfig::default();
    config.threads = 1;
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

                ImageInfo {
                    width: metadata.max_frame_width.get(),
                    height: metadata.max_frame_height.get(),
                    bit_depth: metadata.bit_depth,
                    has_alpha: parser.alpha_data().is_some(),
                    premultiplied_alpha: parser.premultiplied_alpha(),
                    monochrome: metadata.monochrome,
                    // Color info will be determined from decoded sequence header
                    color_primaries: ColorPrimaries::default(),
                    transfer_characteristics: TransferCharacteristics::default(),
                    matrix_coefficients: MatrixCoefficients::default(),
                    color_range: ColorRange::default(),
                    chroma_sampling,
                    icc_profile: None,
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

        // Create decoder and decode the color image
        let mut decoder = Rav1dDecoder::new(&self.config)?;

        // Decode color image
        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| e.map_error(Error::Parse))?;
        let color_picture = decoder.decode(&primary_data)?;

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

        let matrix = seq_hdr
            .map(|h| to_yuv_matrix(h.mtrx))
            .unwrap_or(YuvStandardMatrix::Bt601);

        // MC=0 (Identity / GBR): the planes are already G,B,R and the decode is
        // a reorder + range expansion, NOT a YUV matrix. `to_yuv_matrix`
        // collapses MC=0 into its Bt601 fallback, so detect identity from the
        // raw code point here and branch to the identity path (4:4:4 only).
        // This is imazen/zenavif#15 for the unsafe-asm backend — the default
        // rav1d-safe path already does this via `cicp_resolve` + the identity
        // converter; without it, GBR planes were silently BT.601-decoded.
        let is_identity = seq_hdr
            .map(|h| matches!(h.mtrx, Rav1dMatrixCoefficients::IDENTITY))
            .unwrap_or(false);

        let bit_depth = color_picture.bit_depth();
        let has_alpha = self.parser.alpha_data().is_some();

        // Convert to RGB using bulk yuv crate functions
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

        // Scale 10/12-bit output to full u16 range
        if bit_depth > 8 && bit_depth < 16 {
            scale_pixels_to_u16(&mut image, bit_depth);
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
        matrix: YuvStandardMatrix,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let width = planes.width;
        let height = planes.height;
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;

        let gray = YuvGrayImage {
            y_plane: &planes.y,
            y_stride: width as u32,
            width: width as u32,
            height: height as u32,
        };

        if has_alpha {
            let mut out = vec![
                Rgba {
                    r: 0u8,
                    g: 0,
                    b: 0,
                    a: 255
                };
                pixel_count
            ];
            let rgb_stride = width as u32 * 4;
            yuv::yuv400_to_rgba(
                &gray,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            )
            .map_err(|e| at!(Error::ColorConversion(e)))?;
            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        } else {
            let mut out = vec![Rgb { r: 0u8, g: 0, b: 0 }; pixel_count];
            let rgb_stride = width as u32 * 3;
            yuv::yuv400_to_rgb(
                &gray,
                rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                rgb_stride,
                yuv_range,
                matrix,
            )
            .map_err(|e| at!(Error::ColorConversion(e)))?;
            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        }
    }

    fn convert_mono16(
        &self,
        planes: &YuvPlanes16,
        yuv_range: YuvRange,
        matrix: YuvStandardMatrix,
        bit_depth: u8,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let width = planes.width;
        let height = planes.height;
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;

        let gray = YuvGrayImage {
            y_plane: &planes.y,
            y_stride: width as u32,
            width: width as u32,
            height: height as u32,
        };

        if has_alpha {
            let mut out = vec![
                Rgba {
                    r: 0u16,
                    g: 0,
                    b: 0,
                    a: 0xFFFF
                };
                pixel_count
            ];
            let rgb_stride = width as u32 * 4;
            match bit_depth {
                10 => yuv::y010_to_rgba10(
                    &gray,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                12 => yuv::y012_to_rgba12(
                    &gray,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                _ => yuv::y016_to_rgba16(
                    &gray,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
            }
            .map_err(|e| at!(Error::ColorConversion(e)))?;
            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        } else {
            let mut out = vec![
                Rgb {
                    r: 0u16,
                    g: 0,
                    b: 0
                };
                pixel_count
            ];
            let rgb_stride = width as u32 * 3;
            match bit_depth {
                10 => yuv::y010_to_rgb10(
                    &gray,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                12 => yuv::y012_to_rgb12(
                    &gray,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                _ => yuv::y016_to_rgb16(
                    &gray,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
            }
            .map_err(|e| at!(Error::ColorConversion(e)))?;
            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        }
    }

    fn convert_yuv8(
        &self,
        planes: &YuvPlanes8,
        yuv_range: YuvRange,
        matrix: YuvStandardMatrix,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let width = planes.width;
        let height = planes.height;
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;

        let planar = YuvPlanarImage {
            y_plane: &planes.y,
            y_stride: width as u32,
            u_plane: &planes.u,
            u_stride: planes.chroma_width as u32,
            v_plane: &planes.v,
            v_stride: planes.chroma_width as u32,
            width: width as u32,
            height: height as u32,
        };

        if has_alpha {
            let mut out = vec![
                Rgba {
                    r: 0u8,
                    g: 0,
                    b: 0,
                    a: 255
                };
                pixel_count
            ];
            let rgb_stride = width as u32 * 4;
            match planes.chroma_sampling() {
                ChromaSampling::Cs420 => crate::yuv_bilinear_fix::yuv420_bilinear_complete(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    4,
                    |p, o, s| yuv::yuv420_to_rgba_bilinear(p, o, s, yuv_range, matrix),
                ),
                ChromaSampling::Cs422 => yuv::yuv422_to_rgba_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                ChromaSampling::Cs444 => yuv::yuv444_to_rgba(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                ChromaSampling::Monochrome => {
                    return Err(at!(Error::Decode {
                        code: -1,
                        msg: "Monochrome should not reach chroma conversion",
                    }));
                }
            }
            .map_err(|e| at!(Error::ColorConversion(e)))?;

            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        } else {
            let mut out = vec![Rgb { r: 0u8, g: 0, b: 0 }; pixel_count];
            let rgb_stride = width as u32 * 3;
            match planes.chroma_sampling() {
                ChromaSampling::Cs420 => crate::yuv_bilinear_fix::yuv420_bilinear_complete(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    3,
                    |p, o, s| yuv::yuv420_to_rgb_bilinear(p, o, s, yuv_range, matrix),
                ),
                ChromaSampling::Cs422 => yuv::yuv422_to_rgb_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                ChromaSampling::Cs444 => yuv::yuv444_to_rgb(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                ChromaSampling::Monochrome => {
                    return Err(at!(Error::Decode {
                        code: -1,
                        msg: "Monochrome should not reach chroma conversion",
                    }));
                }
            }
            .map_err(|e| at!(Error::ColorConversion(e)))?;

            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        }
    }

    /// MC=0 (Identity / GBR), 8-bit, 4:4:4. The planes are G (plane 0), B
    /// (plane 1), R (plane 2); decode is a reorder plus optional limited→full
    /// range expansion. Mirrors the default backend's `convert_8bit_identity`.
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
        matrix: YuvStandardMatrix,
        bit_depth: u8,
        has_alpha: bool,
    ) -> Result<PixelBuffer> {
        let width = planes.width;
        let height = planes.height;
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;

        let planar = YuvPlanarImage {
            y_plane: &planes.y,
            y_stride: width as u32,
            u_plane: &planes.u,
            u_stride: planes.chroma_width as u32,
            v_plane: &planes.v,
            v_stride: planes.chroma_width as u32,
            width: width as u32,
            height: height as u32,
        };

        if has_alpha {
            let mut out = vec![
                Rgba {
                    r: 0u16,
                    g: 0,
                    b: 0,
                    a: 0xFFFF
                };
                pixel_count
            ];
            let rgb_stride = width as u32 * 4;
            match (planes.chroma_sampling(), bit_depth) {
                (ChromaSampling::Cs420, 10) => crate::yuv_bilinear_fix::yuv420_bilinear_complete(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    4,
                    |p, o, s| yuv::i010_to_rgba10_bilinear(p, o, s, yuv_range, matrix),
                ),
                (ChromaSampling::Cs420, 12) => crate::yuv_bilinear_fix::yuv420_bilinear_complete(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    4,
                    |p, o, s| yuv::i012_to_rgba12_bilinear(p, o, s, yuv_range, matrix),
                ),
                (ChromaSampling::Cs420, _) => crate::yuv_bilinear_fix::yuv420_bilinear_complete(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    4,
                    |p, o, s| yuv::i016_to_rgba16_bilinear(p, o, s, yuv_range, matrix),
                ),
                (ChromaSampling::Cs422, 10) => yuv::i210_to_rgba10(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs422, 12) => yuv::i212_to_rgba12(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs422, _) => yuv::i216_to_rgba16(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs444, 10) => yuv::i410_to_rgba(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs444, 12) => yuv::i412_to_rgba12(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs444, _) => yuv::i416_to_rgba16(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Monochrome, _) => {
                    return Err(at!(Error::Decode {
                        code: -1,
                        msg: "Monochrome should not reach chroma conversion",
                    }));
                }
            }
            .map_err(|e| at!(Error::ColorConversion(e)))?;

            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        } else {
            let mut out = vec![
                Rgb {
                    r: 0u16,
                    g: 0,
                    b: 0
                };
                pixel_count
            ];
            let rgb_stride = width as u32 * 3;
            match (planes.chroma_sampling(), bit_depth) {
                (ChromaSampling::Cs420, 10) => crate::yuv_bilinear_fix::yuv420_bilinear_complete(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    3,
                    |p, o, s| yuv::i010_to_rgb10_bilinear(p, o, s, yuv_range, matrix),
                ),
                (ChromaSampling::Cs420, 12) => crate::yuv_bilinear_fix::yuv420_bilinear_complete(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    3,
                    |p, o, s| yuv::i012_to_rgb12_bilinear(p, o, s, yuv_range, matrix),
                ),
                (ChromaSampling::Cs420, _) => crate::yuv_bilinear_fix::yuv420_bilinear_complete(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    3,
                    |p, o, s| yuv::i016_to_rgb16_bilinear(p, o, s, yuv_range, matrix),
                ),
                (ChromaSampling::Cs422, 10) => yuv::i210_to_rgb10(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs422, 12) => yuv::i212_to_rgb12(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs422, _) => yuv::i216_to_rgb16(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs444, 10) => yuv::i410_to_rgb10(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs444, 12) => yuv::i412_to_rgb12(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs444, _) => yuv::i416_to_rgb16(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Monochrome, _) => {
                    return Err(at!(Error::Decode {
                        code: -1,
                        msg: "Monochrome should not reach chroma conversion",
                    }));
                }
            }
            .map_err(|e| at!(Error::ColorConversion(e)))?;

            Ok(PixelBuffer::from_pixels(out, width as u32, height as u32)
                .map_err(|_| at!(Error::OutOfMemory))?
                .into())
        }
    }
}

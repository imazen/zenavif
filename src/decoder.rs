//! AVIF decoder implementation wrapping rav1d

use crate::chroma::{yuv_420, yuv_422, yuv_444};
use crate::config::DecoderConfig;
use crate::convert::{add_alpha8, add_alpha16};
use crate::error::{Error, Result};
use crate::image::{
    ChromaSampling, ColorPrimaries, ColorRange, DecodedImage, ImageInfo, MatrixCoefficients,
    TransferCharacteristics,
};
use enough::Stop;
use imgref::ImgVec;

// Conditionally import from rav1d or rav1d-safe based on feature
#[cfg(feature = "asm")]
use rav1d::include::dav1d::data::Dav1dData;
#[cfg(feature = "asm")]
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
#[cfg(feature = "asm")]
use rav1d::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444, Dav1dPixelLayout, Rav1dMatrixCoefficients, Rav1dSequenceHeader,
};
#[cfg(feature = "asm")]
use rav1d::include::dav1d::picture::Dav1dPicture;
#[cfg(feature = "asm")]
use rav1d::src::lib::{
    dav1d_close, dav1d_data_wrap, dav1d_default_settings, dav1d_get_picture, dav1d_open,
    dav1d_picture_unref, dav1d_send_data,
};
#[cfg(feature = "asm")]
use rav1d::src::send_sync_non_null::SendSyncNonNull;

#[cfg(feature = "safe-simd")]
use rav1d_safe::include::dav1d::data::Dav1dData;
#[cfg(feature = "safe-simd")]
use rav1d_safe::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
#[cfg(feature = "safe-simd")]
use rav1d_safe::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444, Dav1dPixelLayout, Rav1dMatrixCoefficients, Rav1dSequenceHeader,
};
#[cfg(feature = "safe-simd")]
use rav1d_safe::include::dav1d::picture::Dav1dPicture;
#[cfg(feature = "safe-simd")]
use rav1d_safe::src::lib::{
    dav1d_close, dav1d_data_wrap, dav1d_default_settings, dav1d_get_picture, dav1d_open,
    dav1d_picture_unref, dav1d_send_data,
};
#[cfg(feature = "safe-simd")]
use rav1d_safe::src::send_sync_non_null::SendSyncNonNull;
use rgb::Rgba;
use rgb::prelude::*;
use std::ffi::c_int;
use std::ffi::c_void;
use std::ptr::NonNull;
use whereat::at;
use yuv::YUV;
use yuv::color::{Depth, Range};
use yuv::convert::RGBConvert;

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
            return Err(at(Error::Decode {
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
            at(Error::Decode {
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
            return Err(at(Error::Decode {
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
                return Err(at(Error::Decode {
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
                return Err(at(Error::Decode {
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

    /// Extract Y plane data as a Vec (copies the data)
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

    /// Extract Y plane data as 16-bit (copies the data)
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

    /// Extract all YUV planes as 8-bit
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

    /// Extract all YUV planes as 16-bit
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

/// 8-bit YUV plane data
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
    fn y_rows(&self) -> impl Iterator<Item = &[u8]> {
        self.y.chunks(self.width)
    }

    fn u_rows(&self) -> impl Iterator<Item = &[u8]> {
        if self.chroma_width == 0 {
            return [].chunks(1);
        }
        self.u.chunks(self.chroma_width)
    }

    fn v_rows(&self) -> impl Iterator<Item = &[u8]> {
        if self.chroma_width == 0 {
            return [].chunks(1);
        }
        self.v.chunks(self.chroma_width)
    }

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

/// 16-bit YUV plane data
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
    fn y_rows(&self) -> impl Iterator<Item = &[u16]> {
        self.y.chunks(self.width)
    }

    fn u_rows(&self) -> impl Iterator<Item = &[u16]> {
        if self.chroma_width == 0 {
            return [].chunks(1);
        }
        self.u.chunks(self.chroma_width)
    }

    fn v_rows(&self) -> impl Iterator<Item = &[u16]> {
        if self.chroma_width == 0 {
            return [].chunks(1);
        }
        self.v.chunks(self.chroma_width)
    }

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

/// Convert rav1d matrix coefficients to yuv crate's
fn to_yuv_matrix(mc: Rav1dMatrixCoefficients) -> yuv::color::MatrixCoefficients {
    match mc {
        Rav1dMatrixCoefficients::IDENTITY => yuv::color::MatrixCoefficients::Identity,
        Rav1dMatrixCoefficients::BT709 => yuv::color::MatrixCoefficients::BT709,
        Rav1dMatrixCoefficients::FCC => yuv::color::MatrixCoefficients::FCC,
        Rav1dMatrixCoefficients::BT470BG => yuv::color::MatrixCoefficients::BT470BG,
        Rav1dMatrixCoefficients::BT601 => yuv::color::MatrixCoefficients::BT601,
        Rav1dMatrixCoefficients::SMPTE240 => yuv::color::MatrixCoefficients::SMPTE240,
        Rav1dMatrixCoefficients::SMPTE_YCGCO => yuv::color::MatrixCoefficients::YCgCo,
        Rav1dMatrixCoefficients::BT2020_NCL => yuv::color::MatrixCoefficients::BT2020NCL,
        Rav1dMatrixCoefficients::BT2020_CL => yuv::color::MatrixCoefficients::BT2020CL,
        _ => yuv::color::MatrixCoefficients::BT601, // Default fallback
    }
}

/// AVIF decoder
pub struct AvifDecoder {
    avif_data: avif_parse::AvifData,
    config: DecoderConfig,
    info: ImageInfo,
}

impl AvifDecoder {
    /// Create a new AVIF decoder from raw data
    ///
    /// This parses the AVIF container but does not decode the AV1 data yet.
    pub fn new(data: &[u8], config: &DecoderConfig) -> Result<Self> {
        // Use lenient parsing to handle files with non-critical validation issues
        let options = avif_parse::ParseOptions { lenient: true };
        let avif_data = avif_parse::read_avif_with_options(&mut &data[..], &options).map_err(|e| at(Error::Parse(e)))?;

        // Extract metadata from the parsed AVIF
        let metadata = avif_data
            .primary_item_metadata()
            .map_err(|e| at(Error::Parse(e)))?;

        let chroma_sampling = match metadata.chroma_subsampling {
            (false, false) => ChromaSampling::Cs444,
            (true, false) => ChromaSampling::Cs422,
            (true, true) => ChromaSampling::Cs420,
            _ => ChromaSampling::Cs420,
        };

        let info = ImageInfo {
            width: metadata.max_frame_width.get(),
            height: metadata.max_frame_height.get(),
            bit_depth: metadata.bit_depth,
            has_alpha: avif_data.alpha_item.is_some(),
            premultiplied_alpha: avif_data.premultiplied_alpha,
            monochrome: metadata.monochrome,
            // Color info will be determined from decoded sequence header
            color_primaries: ColorPrimaries::default(),
            transfer_characteristics: TransferCharacteristics::default(),
            matrix_coefficients: MatrixCoefficients::default(),
            color_range: ColorRange::default(),
            chroma_sampling,
        };

        // Check frame size limit
        if config.frame_size_limit > 0 {
            let total_pixels = info.width.saturating_mul(info.height);
            if total_pixels > config.frame_size_limit {
                return Err(at(Error::ImageTooLarge {
                    width: info.width,
                    height: info.height,
                }));
            }
        }

        Ok(Self {
            avif_data,
            config: config.clone(),
            info,
        })
    }

    /// Get image metadata
    pub fn info(&self) -> &ImageInfo {
        &self.info
    }

    /// Decode the AVIF image
    pub fn decode(&mut self, stop: &impl Stop) -> Result<DecodedImage> {
        // Check for cancellation before starting decode
        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Create decoder and decode the color image
        let mut decoder = Rav1dDecoder::new(&self.config)?;

        // Decode color image
        let color_picture = decoder.decode(&self.avif_data.primary_item)?;

        // Check for cancellation after color decode
        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Get color info from sequence header
        let seq_hdr = color_picture.seq_hdr();
        let range = seq_hdr
            .map(|h| {
                if h.color_range != 0 {
                    Range::Full
                } else {
                    Range::Limited
                }
            })
            .unwrap_or(Range::Limited);

        let matrix = seq_hdr
            .map(|h| to_yuv_matrix(h.mtrx))
            .unwrap_or(yuv::color::MatrixCoefficients::BT601);

        let bit_depth = color_picture.bit_depth();
        let has_alpha = self.avif_data.alpha_item.is_some();

        // Convert to RGB
        let mut image = if bit_depth == 8 {
            let planes = color_picture
                .yuv_planes_u8()
                .ok_or_else(|| at(Error::Unsupported("failed to extract YUV planes")))?;

            match planes.chroma_sampling() {
                ChromaSampling::Monochrome => {
                    self.convert_mono8(&planes, range, matrix, has_alpha)?
                }
                _ => self.convert_yuv8(&planes, range, matrix, has_alpha)?,
            }
        } else {
            let planes = color_picture
                .yuv_planes_u16()
                .ok_or_else(|| at(Error::Unsupported("failed to extract YUV planes")))?;

            let depth = match bit_depth {
                10 => Depth::Depth10,
                12 => Depth::Depth12,
                _ => Depth::Depth16,
            };

            match planes.chroma_sampling() {
                ChromaSampling::Monochrome => {
                    self.convert_mono16(&planes, range, matrix, depth, has_alpha)?
                }
                _ => self.convert_yuv16(&planes, range, matrix, depth, has_alpha)?,
            }
        };

        // Drop color picture before decoding alpha
        drop(color_picture);

        // Check for cancellation before alpha decode
        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Decode alpha channel if present
        if let Some(alpha_data) = &self.avif_data.alpha_item {
            let alpha_picture = decoder.decode(alpha_data)?;

            let alpha_range = alpha_picture
                .seq_hdr()
                .map(|h| {
                    if h.color_range != 0 {
                        Range::Full
                    } else {
                        Range::Limited
                    }
                })
                .unwrap_or(Range::Limited);

            let alpha_bit_depth = alpha_picture.bit_depth();

            // Alpha uses Identity matrix
            if alpha_bit_depth == 8 {
                let (y_data, width, height, _) = alpha_picture
                    .y_plane_u8()
                    .ok_or_else(|| at(Error::Unsupported("failed to extract alpha plane")))?;

                let conv =
                    RGBConvert::<u8>::new(alpha_range, yuv::color::MatrixCoefficients::Identity)
                        .map_err(|e| at(Error::ColorConversion(e)))?;

                add_alpha8(
                    &mut image,
                    y_data.chunks(width),
                    width,
                    height,
                    conv,
                    self.avif_data.premultiplied_alpha,
                )?;
            } else {
                let depth = match alpha_bit_depth {
                    10 => Depth::Depth10,
                    12 => Depth::Depth12,
                    _ => Depth::Depth16,
                };

                let (y_data, width, height, _) = alpha_picture
                    .y_plane_u16()
                    .ok_or_else(|| at(Error::Unsupported("failed to extract alpha plane")))?;

                let conv = RGBConvert::<u16>::new(
                    alpha_range,
                    yuv::color::MatrixCoefficients::Identity,
                    depth,
                )
                .map_err(|e| at(Error::ColorConversion(e)))?;

                add_alpha16(
                    &mut image,
                    y_data.chunks(width),
                    width,
                    height,
                    conv,
                    self.avif_data.premultiplied_alpha,
                )?;
            }
        }

        Ok(image)
    }

    fn convert_mono8(
        &self,
        planes: &YuvPlanes8,
        range: Range,
        matrix: yuv::color::MatrixCoefficients,
        has_alpha: bool,
    ) -> Result<DecodedImage> {
        let mc = if matrix == yuv::color::MatrixCoefficients::BT601 {
            yuv::color::MatrixCoefficients::Identity
        } else {
            matrix
        };

        let conv = RGBConvert::<u8>::new(range, mc).map_err(|e| at(Error::ColorConversion(e)))?;

        let width = planes.width;
        let height = planes.height;

        if has_alpha {
            let mut out = Vec::with_capacity(width * height);
            for row in planes.y_rows() {
                for &y in row {
                    let g = conv.to_luma(y);
                    out.push(Rgba::new(g, g, g, 0));
                }
            }
            Ok(DecodedImage::Rgba8(ImgVec::new(out, width, height)))
        } else {
            let mut out = Vec::with_capacity(width * height);
            for row in planes.y_rows() {
                for &y in row {
                    out.push(conv.to_luma(y));
                }
            }
            Ok(DecodedImage::Gray8(ImgVec::new(out, width, height)))
        }
    }

    fn convert_mono16(
        &self,
        planes: &YuvPlanes16,
        range: Range,
        matrix: yuv::color::MatrixCoefficients,
        depth: Depth,
        has_alpha: bool,
    ) -> Result<DecodedImage> {
        let mc = if matrix == yuv::color::MatrixCoefficients::BT601 {
            yuv::color::MatrixCoefficients::Identity
        } else {
            matrix
        };

        let conv =
            RGBConvert::<u16>::new(range, mc, depth).map_err(|e| at(Error::ColorConversion(e)))?;

        let width = planes.width;
        let height = planes.height;

        if has_alpha {
            let mut out = Vec::with_capacity(width * height);
            for row in planes.y_rows() {
                for &y in row {
                    let g = conv.to_luma(y);
                    out.push(Rgba::new(g, g, g, 0));
                }
            }
            Ok(DecodedImage::Rgba16(ImgVec::new(out, width, height)))
        } else {
            let mut out = Vec::with_capacity(width * height);
            for row in planes.y_rows() {
                for &y in row {
                    out.push(conv.to_luma(y));
                }
            }
            Ok(DecodedImage::Gray16(ImgVec::new(out, width, height)))
        }
    }

    fn convert_yuv8(
        &self,
        planes: &YuvPlanes8,
        range: Range,
        matrix: yuv::color::MatrixCoefficients,
        has_alpha: bool,
    ) -> Result<DecodedImage> {
        let conv =
            RGBConvert::<u8>::new(range, matrix).map_err(|e| at(Error::ColorConversion(e)))?;

        let width = planes.width;
        let height = planes.height;

        let px_iter: Box<dyn Iterator<Item = YUV<u8>>> = match planes.chroma_sampling() {
            ChromaSampling::Cs444 => {
                Box::new(yuv_444(planes.y_rows(), planes.u_rows(), planes.v_rows()))
            }
            ChromaSampling::Cs422 => {
                Box::new(yuv_422(planes.y_rows(), planes.u_rows(), planes.v_rows()))
            }
            ChromaSampling::Cs420 => {
                Box::new(yuv_420(planes.y_rows(), planes.u_rows(), planes.v_rows()))
            }
            ChromaSampling::Monochrome => unreachable!(),
        };

        if has_alpha {
            let mut out = Vec::with_capacity(width * height);
            out.extend(px_iter.map(|px| conv.to_rgb(px).with_alpha(0)));
            Ok(DecodedImage::Rgba8(ImgVec::new(out, width, height)))
        } else {
            let mut out = Vec::with_capacity(width * height);
            out.extend(px_iter.map(|px| conv.to_rgb(px)));
            Ok(DecodedImage::Rgb8(ImgVec::new(out, width, height)))
        }
    }

    fn convert_yuv16(
        &self,
        planes: &YuvPlanes16,
        range: Range,
        matrix: yuv::color::MatrixCoefficients,
        depth: Depth,
        has_alpha: bool,
    ) -> Result<DecodedImage> {
        let conv = RGBConvert::<u16>::new(range, matrix, depth)
            .map_err(|e| at(Error::ColorConversion(e)))?;

        let width = planes.width;
        let height = planes.height;

        let px_iter: Box<dyn Iterator<Item = YUV<u16>>> = match planes.chroma_sampling() {
            ChromaSampling::Cs444 => {
                Box::new(yuv_444(planes.y_rows(), planes.u_rows(), planes.v_rows()))
            }
            ChromaSampling::Cs422 => {
                Box::new(yuv_422(planes.y_rows(), planes.u_rows(), planes.v_rows()))
            }
            ChromaSampling::Cs420 => {
                Box::new(yuv_420(planes.y_rows(), planes.u_rows(), planes.v_rows()))
            }
            ChromaSampling::Monochrome => unreachable!(),
        };

        if has_alpha {
            let mut out = Vec::with_capacity(width * height);
            out.extend(px_iter.map(|px| conv.to_rgb(px).with_alpha(0)));
            Ok(DecodedImage::Rgba16(ImgVec::new(out, width, height)))
        } else {
            let mut out = Vec::with_capacity(width * height);
            out.extend(px_iter.map(|px| conv.to_rgb(px)));
            Ok(DecodedImage::Rgb16(ImgVec::new(out, width, height)))
        }
    }
}

//! AVIF decoder implementation wrapping rav1d

#![allow(unsafe_code)]

use crate::config::DecoderConfig;
use crate::convert::{add_alpha8, add_alpha16, scale_pixels_to_u16};
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
        // Use zero-copy AvifParser — primary/alpha data returned as Cow::Borrowed
        let parse_config = zenavif_parse::DecodeConfig::default().lenient(true);
        let parser = zenavif_parse::AvifParser::from_owned_with_config(
            data.to_vec(),
            &parse_config,
            &enough::Unstoppable,
        )
        .map_err(|e| at!(Error::Parse(e)))?;

        // Extract metadata from the parsed AVIF
        let metadata = parser
            .primary_metadata()
            .map_err(|e| at!(Error::Parse(e)))?;

        let cs = metadata.chroma_subsampling;
        let chroma_sampling = if cs.horizontal && cs.vertical {
            ChromaSampling::Cs420
        } else if cs.horizontal {
            ChromaSampling::Cs422
        } else {
            ChromaSampling::Cs444
        };

        let has_alpha = parser.alpha_data().is_some();
        let info = ImageInfo {
            width: metadata.max_frame_width.get(),
            height: metadata.max_frame_height.get(),
            bit_depth: metadata.bit_depth,
            has_alpha,
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
        };

        // Check frame size limit
        if config.frame_size_limit > 0 {
            let total_pixels = info.width.saturating_mul(info.height);
            if total_pixels > config.frame_size_limit {
                return Err(at!(Error::ImageTooLarge {
                    width: info.width,
                    height: info.height,
                }));
            }
        }

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
            .map_err(|e| at!(Error::Parse(e)))?;
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

        let bit_depth = color_picture.bit_depth();
        let has_alpha = self.parser.alpha_data().is_some();

        // Convert to RGB using bulk yuv crate functions
        let mut image = if bit_depth == 8 {
            let planes = color_picture
                .yuv_planes_u8()
                .ok_or_else(|| at!(Error::Unsupported("failed to extract YUV planes")))?;

            match planes.chroma_sampling() {
                ChromaSampling::Monochrome => {
                    self.convert_mono8(&planes, yuv_range, matrix, has_alpha)?
                }
                _ => self.convert_yuv8(&planes, yuv_range, matrix, has_alpha)?,
            }
        } else {
            let planes = color_picture
                .yuv_planes_u16()
                .ok_or_else(|| at!(Error::Unsupported("failed to extract YUV planes")))?;

            match planes.chroma_sampling() {
                ChromaSampling::Monochrome => {
                    self.convert_mono16(&planes, yuv_range, matrix, bit_depth, has_alpha)?
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
            let alpha_data = alpha_result.map_err(|e| at!(Error::Parse(e)))?;
            let alpha_picture = decoder.decode(&alpha_data)?;

            let alpha_color_range = alpha_picture
                .seq_hdr()
                .map(|h| to_color_range(h.color_range))
                .unwrap_or(ColorRange::Limited);

            let alpha_bit_depth = alpha_picture.bit_depth();
            let premultiplied = self.parser.premultiplied_alpha();

            if alpha_bit_depth == 8 {
                let (y_data, width, height, _) = alpha_picture
                    .y_plane_u8()
                    .ok_or_else(|| at!(Error::Unsupported("failed to extract alpha plane")))?;

                add_alpha8(
                    &mut image,
                    y_data.chunks(width),
                    width,
                    height,
                    alpha_color_range,
                    premultiplied,
                )?;
            } else {
                let (y_data, width, height, _) = alpha_picture
                    .y_plane_u16()
                    .ok_or_else(|| at!(Error::Unsupported("failed to extract alpha plane")))?;

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
                ChromaSampling::Cs420 => yuv::yuv420_to_rgba_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
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
                ChromaSampling::Cs420 => yuv::yuv420_to_rgb_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
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
                (ChromaSampling::Cs420, 10) => yuv::i010_to_rgba10_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs420, 12) => yuv::i012_to_rgba12_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs420, _) => yuv::i016_to_rgba16_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
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
                (ChromaSampling::Cs420, 10) => yuv::i010_to_rgb10_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs420, 12) => yuv::i012_to_rgb12_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
                ),
                (ChromaSampling::Cs420, _) => yuv::i016_to_rgb16_bilinear(
                    &planar,
                    rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                    rgb_stride,
                    yuv_range,
                    matrix,
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

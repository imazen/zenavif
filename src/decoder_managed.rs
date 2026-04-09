//! AVIF decoder implementation using rav1d-safe managed API
//!
//! This module provides a 100% safe implementation using the managed API.
//! No unsafe code required!

#![deny(unsafe_code)]

use crate::config::DecoderConfig;
use crate::convert::{add_alpha8, add_alpha16, scale_pixels_to_u16};
use crate::error::{Error, Result};
use crate::image::{
    ChromaSampling, ColorPrimaries, ColorRange, DecodedAnimation, DecodedAnimationInfo,
    DecodedFrame, ImageInfo, MatrixCoefficients, TransferCharacteristics,
};
use crate::yuv_convert::{self, YuvMatrix as OurYuvMatrix, YuvRange as OurYuvRange};
use enough::Stop;
use rgb::{Rgb, Rgba};
use whereat::at;
use yuv::{YuvGrayImage, YuvPlanarImage, YuvRange, YuvStandardMatrix};
use zenpixels::{PixelBuffer, PixelDescriptor};

// Import managed API from rav1d-safe
use rav1d_safe::src::managed::{
    ColorPrimaries as Rav1dColorPrimaries, ColorRange as Rav1dColorRange, Decoder as Rav1dDecoder,
    Frame, MatrixCoefficients as Rav1dMatrixCoefficients, PixelLayout, Planes, Settings,
    TransferCharacteristics as Rav1dTransferCharacteristics,
};

/// Convert rav1d-safe ColorPrimaries to zenavif ColorPrimaries
fn convert_color_primaries(pri: Rav1dColorPrimaries) -> ColorPrimaries {
    match pri {
        Rav1dColorPrimaries::BT709 => ColorPrimaries::BT709,
        Rav1dColorPrimaries::BT2020 => ColorPrimaries::BT2020,
        Rav1dColorPrimaries::BT601 => ColorPrimaries::BT601,
        Rav1dColorPrimaries::SMPTE240 => ColorPrimaries::SMPTE240,
        _ => ColorPrimaries::UNKNOWN,
    }
}

/// Convert rav1d-safe TransferCharacteristics to zenavif
fn convert_transfer(trc: Rav1dTransferCharacteristics) -> TransferCharacteristics {
    match trc {
        Rav1dTransferCharacteristics::BT709 => TransferCharacteristics::BT709,
        Rav1dTransferCharacteristics::SMPTE2084 => TransferCharacteristics::SMPTE2084,
        Rav1dTransferCharacteristics::HLG => TransferCharacteristics::HLG,
        Rav1dTransferCharacteristics::SRGB => TransferCharacteristics::SRGB,
        _ => TransferCharacteristics::UNKNOWN,
    }
}

/// Convert rav1d-safe MatrixCoefficients to zenavif
fn convert_matrix(mtrx: Rav1dMatrixCoefficients) -> MatrixCoefficients {
    match mtrx {
        Rav1dMatrixCoefficients::Identity => MatrixCoefficients::IDENTITY,
        Rav1dMatrixCoefficients::BT709 => MatrixCoefficients::BT709,
        Rav1dMatrixCoefficients::BT2020NCL => MatrixCoefficients::BT2020_NCL,
        Rav1dMatrixCoefficients::BT601 => MatrixCoefficients::BT601,
        _ => MatrixCoefficients::UNKNOWN,
    }
}

/// Convert rav1d-safe ColorRange to zenavif
fn convert_color_range(range: Rav1dColorRange) -> ColorRange {
    match range {
        Rav1dColorRange::Limited => ColorRange::Limited,
        Rav1dColorRange::Full => ColorRange::Full,
    }
}

/// Convert zenavif MatrixCoefficients to yuv crate's YuvStandardMatrix
fn to_yuv_matrix(mc: MatrixCoefficients) -> YuvStandardMatrix {
    match mc {
        MatrixCoefficients::BT709 => YuvStandardMatrix::Bt709,
        MatrixCoefficients::BT601 | MatrixCoefficients::BT470BG | MatrixCoefficients::FCC => {
            YuvStandardMatrix::Bt601
        }
        MatrixCoefficients::BT2020_NCL | MatrixCoefficients::BT2020_CL => YuvStandardMatrix::Bt2020,
        MatrixCoefficients::SMPTE240 => YuvStandardMatrix::Smpte240,
        _ => YuvStandardMatrix::Bt601,
    }
}

/// Convert zenavif MatrixCoefficients to our YuvMatrix
fn to_our_yuv_matrix(mc: MatrixCoefficients) -> OurYuvMatrix {
    match mc {
        MatrixCoefficients::BT709 => OurYuvMatrix::Bt709,
        MatrixCoefficients::BT601 | MatrixCoefficients::BT470BG | MatrixCoefficients::FCC => {
            OurYuvMatrix::Bt601
        }
        MatrixCoefficients::BT2020_NCL | MatrixCoefficients::BT2020_CL => OurYuvMatrix::Bt2020,
        _ => OurYuvMatrix::Bt601, // Default to BT.601 for unknown
    }
}

/// Convert zenavif ColorRange to our YuvRange
fn to_our_yuv_range(cr: ColorRange) -> OurYuvRange {
    match cr {
        ColorRange::Limited => OurYuvRange::Limited,
        ColorRange::Full => OurYuvRange::Full,
    }
}

/// Convert zenavif ColorRange to yuv crate's YuvRange
fn to_yuv_range(range: ColorRange) -> YuvRange {
    match range {
        ColorRange::Full => YuvRange::Full,
        ColorRange::Limited => YuvRange::Limited,
    }
}

/// Convert rav1d-safe PixelLayout to zenavif ChromaSampling
fn convert_chroma_sampling(layout: PixelLayout) -> ChromaSampling {
    match layout {
        PixelLayout::I400 => ChromaSampling::Monochrome,
        PixelLayout::I420 => ChromaSampling::Cs420,
        PixelLayout::I422 => ChromaSampling::Cs422,
        PixelLayout::I444 => ChromaSampling::Cs444,
    }
}

/// Managed decoder wrapper - 100% safe!
pub struct ManagedAvifDecoder {
    decoder: Rav1dDecoder,
    parser: zenavif_parse::AvifParser<'static>,
}

impl ManagedAvifDecoder {
    /// Create new decoder with AVIF data and configuration
    pub fn new(data: &[u8], config: &DecoderConfig) -> Result<Self> {
        // Use zero-copy AvifParser — primary/alpha data returned as Cow::Borrowed
        let mut parse_config = zenavif_parse::DecodeConfig::default().lenient(true);
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
        .map_err(|e| at!(Error::from(e)))?;

        let mut settings = Settings::default();
        settings.threads = config.threads;
        settings.max_frame_delay = 1; // disable frame threading (DisjointMut race in rav1d CDEF)
        settings.apply_grain = config.apply_grain;
        settings.frame_size_limit = config.frame_size_limit;

        let decoder = Rav1dDecoder::with_settings(settings).map_err(|_e| {
            at!(Error::Decode {
                code: -1,
                msg: "Failed to create decoder",
            })
        })?;

        // Validate dimensions against frame_size_limit before any decode work
        if config.frame_size_limit > 0 {
            let (width, height) = if let Some(grid) = parser.grid_config() {
                (grid.output_width, grid.output_height)
            } else if let Ok(meta) = parser.primary_metadata() {
                (meta.max_frame_width.get(), meta.max_frame_height.get())
            } else {
                (0, 0) // unknown dimensions, skip check
            };
            let total_pixels = width.saturating_mul(height);
            if total_pixels > config.frame_size_limit {
                return Err(at!(Error::ImageTooLarge { width, height }));
            }
        }

        Ok(Self { decoder, parser })
    }

    /// Decode a single AV1 frame, handling progressive/multi-layer streams transparently.
    ///
    /// If the decoder buffers data internally (returns `Ok(None)`), flushes to retrieve
    /// the composed frame. Always flushes afterward to reset state, so sequential calls
    /// (e.g. primary then alpha) work without the caller needing to manage decoder state.
    ///
    /// Takes `decoder` explicitly to avoid borrowing `self` (which would conflict
    /// with borrows of `self.parser` for data access).
    fn decode_frame(
        decoder: &mut Rav1dDecoder,
        data: &[u8],
        context: &'static str,
    ) -> Result<Frame> {
        // Send data and try to get a frame immediately
        let frame = match decoder.decode(data) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                // Progressive/multi-layer: flush to get the composed frame
                let frames = decoder.flush().map_err(|_e| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "Failed to flush decoder",
                    })
                })?;
                frames.into_iter().last().ok_or_else(|| {
                    at!(Error::Decode {
                        code: -1,
                        msg: context,
                    })
                })?
            }
            Err(_e) => {
                return Err(at!(Error::Decode {
                    code: -1,
                    msg: context,
                }));
            }
        };
        // Reset decoder state so the next decode_frame call starts clean
        // (e.g. primary → alpha without cross-contamination)
        let _ = decoder.flush();
        Ok(frame)
    }

    /// Decode the primary image and optionally alpha channel
    pub fn decode(&mut self, stop: &(impl Stop + ?Sized)) -> Result<PixelBuffer> {
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Check if this is a grid image (tiled/multi-frame)
        if self.parser.grid_config().is_some() {
            return self.decode_grid(stop);
        }

        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| at!(Error::from(e)))?;
        let primary_frame = Self::decode_frame(
            &mut self.decoder,
            &primary_data,
            "Failed to decode primary frame",
        )?;

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let alpha_frame = if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| at!(Error::from(e)))?;
            Some(Self::decode_frame(
                &mut self.decoder,
                &alpha_data,
                "Failed to decode alpha frame",
            )?)
        } else {
            None
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let (pixels, _info) = self.convert_to_image(primary_frame, alpha_frame, stop)?;
        Ok(pixels)
    }

    /// Decode the primary image and return both pixels and metadata.
    pub fn decode_full(&mut self, stop: &(impl Stop + ?Sized)) -> Result<(PixelBuffer, ImageInfo)> {
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        if self.parser.grid_config().is_some() {
            let pixels = self.decode_grid(stop)?;
            let info = self.probe_info()?;
            return Ok((pixels, info));
        }

        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| at!(Error::from(e)))?;
        let primary_frame = Self::decode_frame(
            &mut self.decoder,
            &primary_data,
            "Failed to decode primary frame",
        )?;

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let alpha_frame = if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| at!(Error::from(e)))?;
            Some(Self::decode_frame(
                &mut self.decoder,
                &alpha_data,
                "Failed to decode alpha frame",
            )?)
        } else {
            None
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        self.convert_to_image(primary_frame, alpha_frame, stop)
    }

    /// Decode frames and return a StripConverter for cache-optimal streaming.
    ///
    /// For 8-bit color images, the decoded YUV frames are held in memory and
    /// converted strip-by-strip on demand. For 16-bit or monochrome, falls back
    /// to full-frame conversion (same allocation as `decode_full`).
    ///
    /// Returns `(StripConverter, ImageInfo)`.
    // WIP: will be wired up as the streaming decode entry point
    #[allow(dead_code)]
    pub(crate) fn decode_to_strip_converter(
        &mut self,
        stop: &(impl Stop + ?Sized),
    ) -> Result<(crate::strip_convert::StripConverter, ImageInfo)> {
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| at!(Error::from(e)))?;
        let primary_frame = Self::decode_frame(
            &mut self.decoder,
            &primary_data,
            "Failed to decode primary frame",
        )?;

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let alpha_frame = if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| at!(Error::from(e)))?;
            Some(Self::decode_frame(
                &mut self.decoder,
                &alpha_data,
                "Failed to decode alpha frame",
            )?)
        } else {
            None
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let info = self.build_image_info(&primary_frame, alpha_frame.is_some())?;

        let bit_depth = primary_frame.bit_depth();
        let layout = primary_frame.pixel_layout();
        let chroma_sampling = convert_chroma_sampling(layout);
        let buffer_width = primary_frame.width() as usize;
        let buffer_height = primary_frame.height() as usize;
        let display_width = info.width as usize;
        let display_height = info.height as usize;

        let can_strip = bit_depth == 8
            && !matches!(chroma_sampling, ChromaSampling::Monochrome)
            && buffer_width == display_width
            && buffer_height == display_height;

        let converter = if can_strip {
            let alpha_range = alpha_frame
                .as_ref()
                .map(|f| convert_color_range(f.color_info().color_range))
                .unwrap_or(ColorRange::Full);

            let descriptor = if alpha_frame.is_some() {
                PixelDescriptor::RGBA8_SRGB
            } else {
                PixelDescriptor::RGB8_SRGB
            };

            crate::strip_convert::StripConverter::new(
                primary_frame,
                alpha_frame,
                chroma_sampling,
                to_our_yuv_range(info.color_range),
                to_our_yuv_matrix(info.matrix_coefficients),
                alpha_range,
                self.parser.premultiplied_alpha(),
                display_width,
                display_height,
                buffer_width,
                buffer_height,
                descriptor,
            )
        } else {
            // Fallback: full conversion for 16-bit, monochrome, or cropped images
            let (pixels, _) = self.convert_to_image(primary_frame, alpha_frame, stop)?;
            crate::strip_convert::StripConverter::new_from_pixels(pixels)
        };

        Ok((converter, info))
    }

    /// Build ImageInfo from a decoded primary frame and parser metadata.
    ///
    /// Factored out of `convert_to_image` for reuse by `decode_to_strip_converter`.
    // WIP: used by decode_to_strip_converter above
    #[allow(dead_code)]
    fn build_image_info(&self, primary: &Frame, has_alpha: bool) -> Result<ImageInfo> {
        let width = primary.width() as usize;
        let height = primary.height() as usize;
        let bit_depth = primary.bit_depth();
        let layout = primary.pixel_layout();

        let av1_color = primary.color_info();
        let matrix_coefficients = convert_matrix(av1_color.matrix_coefficients);
        let color_range = convert_color_range(av1_color.color_range);

        let (color_primaries, transfer_characteristics, icc_profile) =
            match self.parser.color_info() {
                Some(zenavif_parse::ColorInformation::Nclx {
                    color_primaries: cp,
                    transfer_characteristics: tc,
                    ..
                }) => (
                    ColorPrimaries(*cp as u8),
                    TransferCharacteristics(*tc as u8),
                    None,
                ),
                Some(zenavif_parse::ColorInformation::IccProfile(icc)) => (
                    convert_color_primaries(av1_color.primaries),
                    convert_transfer(av1_color.transfer_characteristics),
                    Some(icc.clone()),
                ),
                None => (
                    convert_color_primaries(av1_color.primaries),
                    convert_transfer(av1_color.transfer_characteristics),
                    None,
                ),
            };

        Ok(ImageInfo {
            width: width as u32,
            height: height as u32,
            bit_depth,
            has_alpha,
            premultiplied_alpha: self.parser.premultiplied_alpha(),
            monochrome: matches!(layout, PixelLayout::I400),
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            color_range,
            chroma_sampling: convert_chroma_sampling(layout),
            icc_profile,
            rotation: self.parser.rotation().cloned(),
            mirror: self.parser.mirror().cloned(),
            clean_aperture: self.parser.clean_aperture().cloned(),
            pixel_aspect_ratio: self.parser.pixel_aspect_ratio().cloned(),
            content_light_level: self.parser.content_light_level().cloned(),
            mastering_display: self.parser.mastering_display().cloned(),
            exif: self
                .parser
                .exif()
                .and_then(|r| r.ok())
                .map(|c| c.into_owned()),
            xmp: self
                .parser
                .xmp()
                .and_then(|r| r.ok())
                .map(|c| c.into_owned()),
            gain_map: self.extract_gain_map(),
            // Depth map extraction requires zenavif-parse > 0.4.0 (not yet published).
            depth_map: None,
        })
    }

    /// Probe image metadata without decoding pixels.
    ///
    /// Uses the AVIF container parser and AV1 sequence header to extract
    /// dimensions, color info, ICC profile, EXIF, XMP, orientation, and HDR metadata.
    /// Does NOT do full AV1 frame decoding.
    pub fn probe_info(&self) -> Result<ImageInfo> {
        // Get dimensions from grid config or AV1 sequence header
        let (width, height) = if let Some(grid) = self.parser.grid_config() {
            (grid.output_width, grid.output_height)
        } else {
            let meta = self
                .parser
                .primary_metadata()
                .map_err(|e| at!(Error::from(e)))?;
            (meta.max_frame_width.get(), meta.max_frame_height.get())
        };

        let has_alpha = self.parser.alpha_metadata().is_some();

        // AV1 config for bit depth
        let bit_depth = self.parser.av1_config().map(|c| c.bit_depth).unwrap_or(8);

        // CICP from container (colr box) or AV1 config fallback
        let (
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            color_range,
            icc_profile,
        ) = match self.parser.color_info() {
            Some(zenavif_parse::ColorInformation::Nclx {
                color_primaries: cp,
                transfer_characteristics: tc,
                matrix_coefficients: mc,
                full_range,
            }) => (
                ColorPrimaries(*cp as u8),
                TransferCharacteristics(*tc as u8),
                MatrixCoefficients(*mc as u8),
                if *full_range {
                    ColorRange::Full
                } else {
                    ColorRange::Limited
                },
                None,
            ),
            Some(zenavif_parse::ColorInformation::IccProfile(icc)) => (
                ColorPrimaries::BT709,
                TransferCharacteristics::SRGB,
                MatrixCoefficients::BT601,
                ColorRange::Full,
                Some(icc.clone()),
            ),
            None => (
                ColorPrimaries::BT709,
                TransferCharacteristics::SRGB,
                MatrixCoefficients::BT601,
                ColorRange::Full,
                None,
            ),
        };

        let chroma_sampling = self
            .parser
            .av1_config()
            .map(|c| {
                if c.monochrome {
                    ChromaSampling::Monochrome
                } else if c.chroma_subsampling_x != 0 && c.chroma_subsampling_y != 0 {
                    ChromaSampling::Cs420
                } else if c.chroma_subsampling_x != 0 {
                    ChromaSampling::Cs422
                } else {
                    ChromaSampling::Cs444
                }
            })
            .unwrap_or(ChromaSampling::Cs420);

        Ok(ImageInfo {
            width,
            height,
            bit_depth,
            has_alpha,
            premultiplied_alpha: self.parser.premultiplied_alpha(),
            monochrome: chroma_sampling == ChromaSampling::Monochrome,
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            color_range,
            chroma_sampling,
            icc_profile,
            rotation: self.parser.rotation().cloned(),
            mirror: self.parser.mirror().cloned(),
            clean_aperture: self.parser.clean_aperture().cloned(),
            pixel_aspect_ratio: self.parser.pixel_aspect_ratio().cloned(),
            content_light_level: self.parser.content_light_level().cloned(),
            mastering_display: self.parser.mastering_display().cloned(),
            exif: self
                .parser
                .exif()
                .and_then(|r| r.ok())
                .map(|c| c.into_owned()),
            xmp: self
                .parser
                .xmp()
                .and_then(|r| r.ok())
                .map(|c| c.into_owned()),
            gain_map: self.extract_gain_map(),
            // Depth map extraction requires zenavif-parse > 0.4.0 (not yet published).
            depth_map: None,
        })
    }

    /// Decode an animated AVIF, returning all frames with timing info.
    ///
    /// Returns [`Error::Unsupported`] if the file is not animated.
    /// Each frame's AV1 color (and optional alpha) data is decoded through
    /// rav1d and converted to RGB/RGBA at the source bit depth.
    ///
    /// For memory-efficient frame-by-frame decoding, use [`AnimationDecoder`]
    /// instead.
    ///
    /// Color and alpha tracks use separate decoder instances because
    /// inter-predicted frames depend on prior reference frames within
    /// the same track.
    pub fn decode_animation(&mut self, stop: &(impl Stop + ?Sized)) -> Result<DecodedAnimation> {
        // AnimationDecoder can't reuse our parser (it owns its own),
        // so we implement the loop directly here to avoid a redundant parse.
        let anim_info = self
            .parser
            .animation_info()
            .ok_or_else(|| at!(Error::Unsupported("not an animated AVIF")))?;

        let mut alpha_decoder = if anim_info.has_alpha {
            let mut settings = Settings::default();
            settings.threads = 0;
            settings.max_frame_delay = 1; // disable frame threading (DisjointMut race in rav1d CDEF)
            Some(Rav1dDecoder::with_settings(settings).map_err(|_e| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Failed to create alpha decoder",
                })
            })?)
        } else {
            None
        };

        let frame_count = anim_info.frame_count;
        let mut frames = Vec::with_capacity(frame_count);

        for i in 0..frame_count {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

            let frame_ref = self.parser.frame(i).map_err(|e| at!(Error::from(e)))?;

            let primary_frame = Self::decode_anim_frame(
                &mut self.decoder,
                &frame_ref.data,
                "Failed to decode animation frame",
            )?;

            let alpha_frame = match (&mut alpha_decoder, &frame_ref.alpha_data) {
                (Some(dec), Some(alpha_data)) => Some(Self::decode_anim_frame(
                    dec,
                    alpha_data,
                    "Failed to decode animation alpha frame",
                )?),
                _ => None,
            };

            let (pixels, _info) = self.convert_to_image(primary_frame, alpha_frame, stop)?;

            frames.push(DecodedFrame {
                pixels,
                duration_ms: frame_ref.duration_ms,
            });
        }

        Ok(DecodedAnimation {
            frames,
            info: DecodedAnimationInfo {
                frame_count,
                loop_count: anim_info.loop_count,
                has_alpha: anim_info.has_alpha,
                timescale: anim_info.timescale,
            },
        })
    }

    /// Decode a single frame within an animation sequence.
    ///
    /// Unlike [`decode_frame`], this does NOT flush the decoder, preserving
    /// reference frames needed by subsequent inter-predicted frames.
    fn decode_anim_frame(
        decoder: &mut Rav1dDecoder,
        data: &[u8],
        context: &'static str,
    ) -> Result<Frame> {
        match decoder.decode(data) {
            Ok(Some(frame)) => return Ok(frame),
            Ok(None) => {}
            Err(_e) => {
                return Err(at!(Error::Decode {
                    code: -1,
                    msg: context,
                }));
            }
        }

        // Frame not returned immediately — drain via get_frame
        for _ in 0..10_000 {
            match decoder.get_frame() {
                Ok(Some(frame)) => return Ok(frame),
                Ok(None) => std::thread::yield_now(),
                Err(_e) => break,
            }
        }

        Err(at!(Error::Decode {
            code: -1,
            msg: context,
        }))
    }

    /// Decode a grid-based AVIF (tiled image)
    fn decode_grid(&mut self, stop: &(impl Stop + ?Sized)) -> Result<PixelBuffer> {
        let grid_config = self
            .parser
            .grid_config()
            .ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Expected grid config but found none",
                })
            })?
            .clone();

        // Decode all tiles
        let mut tile_frames = Vec::new();
        for i in 0..self.parser.grid_tile_count() {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

            let tile_data = self.parser.tile_data(i).map_err(|e| at!(Error::from(e)))?;
            let frame =
                Self::decode_frame(&mut self.decoder, &tile_data, "Failed to decode grid tile")?;

            tile_frames.push(frame);
        }

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Stitch tiles together
        self.stitch_tiles(tile_frames, &grid_config, stop)
    }

    /// Stitch decoded tile frames into a single image
    fn stitch_tiles(
        &self,
        tiles: Vec<Frame>,
        grid_config: &zenavif_parse::GridConfig,
        stop: &(impl Stop + ?Sized),
    ) -> Result<PixelBuffer> {
        if tiles.is_empty() {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "No tiles to stitch",
            }));
        }

        let rows = grid_config.rows as usize;
        let cols = grid_config.columns as usize;

        if tiles.len() != rows * cols {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "Tile count doesn't match grid dimensions",
            }));
        }

        // Get dimensions from first tile (all tiles should be same size)
        let tile_width = tiles[0].width() as usize;
        let tile_height = tiles[0].height() as usize;

        // Calculate output dimensions
        let output_width = if grid_config.output_width > 0 {
            grid_config.output_width as usize
        } else {
            tile_width * cols
        };
        let output_height = if grid_config.output_height > 0 {
            grid_config.output_height as usize
        } else {
            tile_height * rows
        };

        // Convert each tile to RGB/RGBA
        let mut tile_images = Vec::new();
        for tile in tiles {
            let (img, _info) = self.convert_to_image(tile, None, stop)?;
            tile_images.push(img);
        }

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Stitch tiles using byte-level row access (format-agnostic)
        let descriptor = tile_images[0].descriptor();
        let bpp = descriptor.bytes_per_pixel();
        let alloc_size = output_width
            .checked_mul(output_height)
            .and_then(|n| n.checked_mul(bpp))
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let data = vec![0u8; alloc_size];
        let mut output =
            PixelBuffer::from_vec(data, output_width as u32, output_height as u32, descriptor)
                .map_err(|_| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "failed to create output buffer for grid stitch",
                    })
                })?;

        for (tile_idx, tile) in tile_images.iter().enumerate() {
            let row = tile_idx / cols;
            let col = tile_idx % cols;
            let tile_w = tile.width() as usize;
            let tile_h = tile.height() as usize;
            let dst_x = col * tile_w;
            let dst_y = row * tile_h;

            let tile_slice = tile.as_slice();
            let mut out_slice = output.as_slice_mut();
            for y in 0..tile_h.min(output_height - dst_y) {
                let src = tile_slice.row(y as u32);
                let copy_w = tile_w.min(output_width - dst_x);
                let copy_bytes = copy_w * bpp;
                let dst_row = out_slice.row_mut((dst_y + y) as u32);
                let dst_start = dst_x * bpp;
                dst_row[dst_start..dst_start + copy_bytes].copy_from_slice(&src[..copy_bytes]);
            }
        }

        Ok(output)
    }

    /// Crop an image to the specified dimensions
    fn crop_image(image: PixelBuffer, width: usize, height: usize) -> Result<PixelBuffer> {
        let descriptor = image.descriptor();
        let bpp = descriptor.bytes_per_pixel();
        let src_w = image.width() as usize;
        let src_h = image.height() as usize;
        let copy_w = width.min(src_w);
        let copy_bytes = copy_w * bpp;

        let alloc_size = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(bpp))
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let mut data = vec![0u8; alloc_size];
        let src = image.as_slice();
        for y in 0..height.min(src_h) {
            let src_row = src.row(y as u32);
            let dst_start = y * width * bpp;
            data[dst_start..dst_start + copy_bytes].copy_from_slice(&src_row[..copy_bytes]);
        }

        PixelBuffer::from_vec(data, width as u32, height as u32, descriptor).map_err(|_| {
            at!(Error::Decode {
                code: -1,
                msg: "failed to create cropped buffer",
            })
        })
    }

    fn convert_to_image(
        &self,
        primary: Frame,
        alpha: Option<Frame>,
        stop: &(impl Stop + ?Sized),
    ) -> Result<(PixelBuffer, ImageInfo)> {
        let width = primary.width() as usize;
        let height = primary.height() as usize;
        let bit_depth = primary.bit_depth();
        let layout = primary.pixel_layout();

        let av1_color = primary.color_info();
        let has_alpha = alpha.is_some();

        // CICP precedence (per MIAF ISO 23000-22 Amd 2):
        //   container colr box > AV1 bitstream > AVIF defaults (1/13/6/full)
        //
        // Matrix coefficients and color range always come from AV1 bitstream
        // because they govern YUV→RGB conversion before any ICC profile applies.
        let matrix_coefficients = convert_matrix(av1_color.matrix_coefficients);
        let color_range = convert_color_range(av1_color.color_range);

        let (color_primaries, transfer_characteristics, icc_profile) =
            match self.parser.color_info() {
                Some(zenavif_parse::ColorInformation::Nclx {
                    color_primaries: cp,
                    transfer_characteristics: tc,
                    ..
                }) => (
                    ColorPrimaries(*cp as u8),
                    TransferCharacteristics(*tc as u8),
                    None,
                ),
                Some(zenavif_parse::ColorInformation::IccProfile(icc)) => {
                    // ICC overrides CP and TC for color management, but we
                    // still populate those fields from AV1 as a fallback
                    (
                        convert_color_primaries(av1_color.primaries),
                        convert_transfer(av1_color.transfer_characteristics),
                        Some(icc.clone()),
                    )
                }
                None => (
                    convert_color_primaries(av1_color.primaries),
                    convert_transfer(av1_color.transfer_characteristics),
                    None,
                ),
            };

        let info = ImageInfo {
            width: width as u32,
            height: height as u32,
            bit_depth,
            has_alpha,
            premultiplied_alpha: self.parser.premultiplied_alpha(),
            monochrome: matches!(layout, PixelLayout::I400),
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            color_range,
            chroma_sampling: convert_chroma_sampling(layout),
            icc_profile,
            rotation: self.parser.rotation().cloned(),
            mirror: self.parser.mirror().cloned(),
            clean_aperture: self.parser.clean_aperture().cloned(),
            pixel_aspect_ratio: self.parser.pixel_aspect_ratio().cloned(),
            content_light_level: self.parser.content_light_level().cloned(),
            mastering_display: self.parser.mastering_display().cloned(),
            exif: self
                .parser
                .exif()
                .and_then(|r| r.ok())
                .map(|c| c.into_owned()),
            xmp: self
                .parser
                .xmp()
                .and_then(|r| r.ok())
                .map(|c| c.into_owned()),
            gain_map: self.extract_gain_map(),
            // Depth map extraction requires zenavif-parse > 0.4.0 (not yet published).
            depth_map: None,
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let info_clone = info.clone();
        let pixels = match bit_depth {
            8 => self.convert_8bit(primary, alpha, info, stop),
            10 | 12 => self.convert_16bit(primary, alpha, info, stop),
            _ => Err(at!(Error::Decode {
                code: -1,
                msg: "Unsupported bit depth",
            })),
        }?;
        Ok((pixels, info_clone))
    }

    /// Convert 8-bit frame to RGB using yuv crate bulk conversion (zero-copy)
    fn convert_8bit(
        &self,
        primary: Frame,
        alpha: Option<Frame>,
        info: ImageInfo,
        stop: &(impl Stop + ?Sized),
    ) -> Result<PixelBuffer> {
        let Planes::Depth8(planes) = primary.planes() else {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "Expected 8-bit planes",
            }));
        };

        // Use buffer dimensions for YUV conversion (actual buffer size)
        // Then crop to displayed dimensions if needed
        let buffer_width = planes.y().width();
        let buffer_height = planes.y().height();
        let display_width = info.width as usize;
        let display_height = info.height as usize;
        let needs_crop = buffer_width != display_width || buffer_height != display_height;
        let has_alpha = alpha.is_some();
        let yuv_range = to_yuv_range(info.color_range);
        let matrix = to_yuv_matrix(info.matrix_coefficients);
        let buffer_pixel_count = buffer_width
            .checked_mul(buffer_height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;

        let mut image = match info.chroma_sampling {
            ChromaSampling::Monochrome => {
                let y_view = planes.y();
                let gray = YuvGrayImage {
                    y_plane: y_view.as_slice(),
                    y_stride: y_view.stride() as u32,
                    width: buffer_width as u32,
                    height: buffer_height as u32,
                };

                if has_alpha {
                    let mut out = vec![
                        Rgba {
                            r: 0u8,
                            g: 0,
                            b: 0,
                            a: 255
                        };
                        buffer_pixel_count
                    ];
                    let rgb_stride = buffer_width as u32 * 4;
                    yuv::yuv400_to_rgba(
                        &gray,
                        rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                        rgb_stride,
                        yuv_range,
                        matrix,
                    )
                    .map_err(|e| at!(Error::ColorConversion(e)))?;
                    PixelBuffer::from_pixels(out, buffer_width as u32, buffer_height as u32)
                        .map_err(|_| at!(Error::OutOfMemory))?
                        .into()
                } else {
                    let mut out = vec![Rgb { r: 0u8, g: 0, b: 0 }; buffer_pixel_count];
                    let rgb_stride = buffer_width as u32 * 3;
                    yuv::yuv400_to_rgb(
                        &gray,
                        rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                        rgb_stride,
                        yuv_range,
                        matrix,
                    )
                    .map_err(|e| at!(Error::ColorConversion(e)))?;
                    PixelBuffer::from_pixels(out, buffer_width as u32, buffer_height as u32)
                        .map_err(|_| at!(Error::OutOfMemory))?
                        .into()
                }
            }
            sampling => {
                let y_view = planes.y();
                let u_view = planes.u().ok_or_else(|| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "Missing U plane",
                    })
                })?;
                let v_view = planes.v().ok_or_else(|| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "Missing V plane",
                    })
                })?;

                let planar = YuvPlanarImage {
                    y_plane: y_view.as_slice(),
                    y_stride: y_view.stride() as u32,
                    u_plane: u_view.as_slice(),
                    u_stride: u_view.stride() as u32,
                    v_plane: v_view.as_slice(),
                    v_stride: v_view.stride() as u32,
                    width: buffer_width as u32,
                    height: buffer_height as u32,
                };

                if has_alpha {
                    // Decode YUV directly to RGBA to avoid an intermediate RGB
                    // allocation. Uses yuv crate bilinear functions for 420/422
                    // (matching our custom YUV module's chroma upsampling quality)
                    // and standard function for 444 (no upsampling needed).
                    let mut out = vec![
                        Rgba {
                            r: 0u8,
                            g: 0,
                            b: 0,
                            a: 255
                        };
                        buffer_pixel_count
                    ];
                    let rgb_stride = buffer_width as u32 * 4;
                    match sampling {
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

                    PixelBuffer::from_pixels(out, buffer_width as u32, buffer_height as u32)
                        .map_err(|_| at!(Error::OutOfMemory))?
                        .into()
                } else {
                    let our_range = to_our_yuv_range(info.color_range);
                    let our_matrix = to_our_yuv_matrix(info.matrix_coefficients);

                    let result = match sampling {
                        ChromaSampling::Cs420 => yuv_convert::yuv420_to_rgb8(
                            y_view.as_slice(),
                            y_view.stride(),
                            u_view.as_slice(),
                            u_view.stride(),
                            v_view.as_slice(),
                            v_view.stride(),
                            buffer_width,
                            buffer_height,
                            our_range,
                            our_matrix,
                        ),
                        ChromaSampling::Cs422 => yuv_convert::yuv422_to_rgb8(
                            y_view.as_slice(),
                            y_view.stride(),
                            u_view.as_slice(),
                            u_view.stride(),
                            v_view.as_slice(),
                            v_view.stride(),
                            buffer_width,
                            buffer_height,
                            our_range,
                            our_matrix,
                        ),
                        ChromaSampling::Cs444 => yuv_convert::yuv444_to_rgb8(
                            y_view.as_slice(),
                            y_view.stride(),
                            u_view.as_slice(),
                            u_view.stride(),
                            v_view.as_slice(),
                            v_view.stride(),
                            buffer_width,
                            buffer_height,
                            our_range,
                            our_matrix,
                        ),
                        ChromaSampling::Monochrome => {
                            return Err(at!(Error::Decode {
                                code: -1,
                                msg: "Monochrome should not reach chroma conversion",
                            }));
                        }
                    };

                    PixelBuffer::from_imgvec(result).into()
                }
            }
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Crop to display dimensions if needed
        if needs_crop {
            image = Self::crop_image(image, display_width, display_height)?;
        }

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            let Planes::Depth8(alpha_planes) = alpha_frame.planes() else {
                return Err(at!(Error::Decode {
                    code: -1,
                    msg: "Expected 8-bit alpha plane",
                }));
            };

            let alpha_range = convert_color_range(alpha_frame.color_info().color_range);

            add_alpha8(
                &mut image,
                alpha_planes.y().rows(),
                display_width,
                display_height,
                alpha_range,
                self.parser.premultiplied_alpha(),
            )?;
        }

        Ok(image)
    }

    /// Convert 10/12-bit frame to RGB using yuv crate bulk conversion (zero-copy)
    fn convert_16bit(
        &self,
        primary: Frame,
        alpha: Option<Frame>,
        info: ImageInfo,
        stop: &(impl Stop + ?Sized),
    ) -> Result<PixelBuffer> {
        let Planes::Depth16(planes) = primary.planes() else {
            return Err(at!(Error::Decode {
                code: -1,
                msg: "Expected 16-bit planes",
            }));
        };

        // Use buffer dimensions for YUV conversion (actual buffer size)
        // Then crop to displayed dimensions if needed
        let buffer_width = planes.y().width();
        let buffer_height = planes.y().height();
        let display_width = info.width as usize;
        let display_height = info.height as usize;
        let needs_crop = buffer_width != display_width || buffer_height != display_height;
        let has_alpha = alpha.is_some();
        let yuv_range = to_yuv_range(info.color_range);
        let matrix = to_yuv_matrix(info.matrix_coefficients);
        let buffer_pixel_count = buffer_width
            .checked_mul(buffer_height)
            .ok_or_else(|| at!(Error::OutOfMemory))?;

        let mut image = match info.chroma_sampling {
            ChromaSampling::Monochrome => {
                let y_view = planes.y();
                let gray = YuvGrayImage {
                    y_plane: y_view.as_slice(),
                    y_stride: y_view.stride() as u32,
                    width: buffer_width as u32,
                    height: buffer_height as u32,
                };

                if has_alpha {
                    let mut out = vec![
                        Rgba {
                            r: 0u16,
                            g: 0,
                            b: 0,
                            a: 0xFFFF
                        };
                        buffer_pixel_count
                    ];
                    let rgb_stride = buffer_width as u32 * 4;
                    match info.bit_depth {
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
                    PixelBuffer::from_pixels(out, buffer_width as u32, buffer_height as u32)
                        .map_err(|_| at!(Error::OutOfMemory))?
                        .into()
                } else {
                    let mut out = vec![
                        Rgb {
                            r: 0u16,
                            g: 0,
                            b: 0
                        };
                        buffer_pixel_count
                    ];
                    let rgb_stride = buffer_width as u32 * 3;
                    match info.bit_depth {
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
                    PixelBuffer::from_pixels(out, buffer_width as u32, buffer_height as u32)
                        .map_err(|_| at!(Error::OutOfMemory))?
                        .into()
                }
            }
            sampling => {
                let y_view = planes.y();
                let u_view = planes.u().ok_or_else(|| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "Missing U plane",
                    })
                })?;
                let v_view = planes.v().ok_or_else(|| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "Missing V plane",
                    })
                })?;

                let planar = YuvPlanarImage {
                    y_plane: y_view.as_slice(),
                    y_stride: y_view.stride() as u32,
                    u_plane: u_view.as_slice(),
                    u_stride: u_view.stride() as u32,
                    v_plane: v_view.as_slice(),
                    v_stride: v_view.stride() as u32,
                    width: buffer_width as u32,
                    height: buffer_height as u32,
                };

                if has_alpha {
                    let mut out = vec![
                        Rgba {
                            r: 0u16,
                            g: 0,
                            b: 0,
                            a: 0xFFFF
                        };
                        buffer_pixel_count
                    ];
                    let rgb_stride = buffer_width as u32 * 4;
                    match (info.bit_depth, sampling) {
                        (10, ChromaSampling::Cs420) => yuv::i010_to_rgba10(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (10, ChromaSampling::Cs422) => yuv::i210_to_rgba10(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (10, ChromaSampling::Cs444) => yuv::i410_to_rgba10(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs420) => yuv::i012_to_rgba12(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs422) => yuv::i212_to_rgba12(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs444) => yuv::i412_to_rgba12(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs420) => yuv::i016_to_rgba16(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs422) => yuv::i216_to_rgba16(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs444) => yuv::i416_to_rgba16(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Monochrome) => {
                            return Err(at!(Error::Decode {
                                code: -1,
                                msg: "Monochrome should not reach chroma conversion",
                            }));
                        }
                    }
                    .map_err(|e| at!(Error::ColorConversion(e)))?;
                    PixelBuffer::from_pixels(out, buffer_width as u32, buffer_height as u32)
                        .map_err(|_| at!(Error::OutOfMemory))?
                        .into()
                } else {
                    let mut out = vec![
                        Rgb {
                            r: 0u16,
                            g: 0,
                            b: 0
                        };
                        buffer_pixel_count
                    ];
                    let rgb_stride = buffer_width as u32 * 3;
                    match (info.bit_depth, sampling) {
                        (10, ChromaSampling::Cs420) => yuv::i010_to_rgb10(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (10, ChromaSampling::Cs422) => yuv::i210_to_rgb10(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (10, ChromaSampling::Cs444) => yuv::i410_to_rgb10(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs420) => yuv::i012_to_rgb12(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs422) => yuv::i212_to_rgb12(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs444) => yuv::i412_to_rgb12(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs420) => yuv::i016_to_rgb16(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs422) => yuv::i216_to_rgb16(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs444) => yuv::i416_to_rgb16(
                            &planar,
                            rgb::bytemuck::cast_slice_mut(out.as_mut_slice()),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Monochrome) => {
                            return Err(at!(Error::Decode {
                                code: -1,
                                msg: "Monochrome should not reach chroma conversion",
                            }));
                        }
                    }
                    .map_err(|e| at!(Error::ColorConversion(e)))?;
                    PixelBuffer::from_pixels(out, buffer_width as u32, buffer_height as u32)
                        .map_err(|_| at!(Error::OutOfMemory))?
                        .into()
                }
            }
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Scale from native bit depth (e.g. 0–1023 for 10-bit) to full u16 (0–65535).
        // Must happen before alpha attachment so unpremultiply uses correct 16-bit range.
        scale_pixels_to_u16(&mut image, info.bit_depth);

        // Crop to display dimensions if needed
        if needs_crop {
            image = Self::crop_image(image, display_width, display_height)?;
        }

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            let Planes::Depth16(alpha_planes) = alpha_frame.planes() else {
                return Err(at!(Error::Decode {
                    code: -1,
                    msg: "Expected 16-bit alpha plane",
                }));
            };

            let alpha_range = convert_color_range(alpha_frame.color_info().color_range);

            add_alpha16(
                &mut image,
                alpha_planes.y().rows(),
                display_width,
                display_height,
                alpha_range,
                info.bit_depth,
                self.parser.premultiplied_alpha(),
            )?;
        }

        Ok(image)
    }

    /// Animation metadata from the AVIF container, if this is an animated AVIF.
    #[allow(dead_code)] // Used by codec.rs when `zencodec` feature is enabled.
    pub(crate) fn animation_info(&self) -> Option<zenavif_parse::AnimationInfo> {
        self.parser.animation_info()
    }

    /// Extract the gain map from the AVIF container, if present.
    ///
    /// Bundles gain_map_metadata, gain_map_data, and gain_map_color_info from
    /// the parser into a single [`AvifGainMap`](crate::image::AvifGainMap).
    fn extract_gain_map(&self) -> Option<crate::image::AvifGainMap> {
        let metadata = self.parser.gain_map_metadata()?.clone();
        let data = self.parser.gain_map_data()?.ok()?.into_owned();
        let alt_color_info = self.parser.gain_map_color_info().cloned();
        Some(crate::image::AvifGainMap {
            metadata,
            gain_map_data: data,
            alt_color_info,
        })
    }

    /// Whether this image is a grid (tiled) image.
    #[allow(dead_code)]
    pub(crate) fn is_grid(&self) -> bool {
        self.parser.grid_config().is_some()
    }

    /// Grid configuration, if this is a grid image.
    #[allow(dead_code)]
    pub(crate) fn grid_config(&self) -> Option<zenavif_parse::GridConfig> {
        self.parser.grid_config().cloned()
    }

    /// Decode one tile-row of a grid image, returning converted pixel buffers.
    ///
    /// Each tile is decoded from AV1 and color-converted before the next,
    /// so peak memory is one raw Frame + one converted PixelBuffer per tile.
    #[allow(dead_code)]
    pub(crate) fn decode_tile_row(
        &mut self,
        grid_row: usize,
        cols: usize,
        stop: &(impl Stop + ?Sized),
    ) -> Result<Vec<PixelBuffer>> {
        let mut row_tiles = Vec::with_capacity(cols);
        for col in 0..cols {
            let tile_idx = grid_row * cols + col;
            let tile_data = self
                .parser
                .tile_data(tile_idx)
                .map_err(|e| at!(Error::from(e)))?;
            let frame =
                Self::decode_frame(&mut self.decoder, &tile_data, "Failed to decode grid tile")?;
            let (pixels, _info) = self.convert_to_image(frame, None, stop)?;
            row_tiles.push(pixels);
        }
        Ok(row_tiles)
    }

    /// Decode with row-level streaming to a sink.
    ///
    /// For grid images, processes one tile-row at a time: decode tiles,
    /// convert to RGB, stitch into the sink buffer, drop frames.
    ///
    /// For single 8-bit color images, the decoded YUV frame is converted
    /// strip-by-strip directly into the sink's buffers. This eliminates the
    /// full RGB allocation and keeps the working set in L2 cache.
    ///
    /// For 16-bit/monochrome images, falls back to full-frame conversion.
    #[cfg(feature = "zencodec")]
    pub fn decode_to_sink(
        &mut self,
        stop: &(impl Stop + ?Sized),
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<ImageInfo> {
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        if self.parser.grid_config().is_some() {
            return self.decode_grid_to_sink(stop, sink);
        }

        // Single image: strip conversion, then copy rows to sink
        let (converter, info) = self.decode_to_strip_converter(stop)?;
        let width = converter.display_width() as u32;
        let height = converter.display_height() as u32;
        let desc = converter.descriptor();
        let strip_h = converter.optimal_strip_height();
        let bpp = desc.bytes_per_pixel();

        sink.begin(width, height, desc)
            .map_err(|e| at!(Error::Encode(e.to_string())))?;

        // Reusable strip buffer for conversion
        let mut strip_pixels = PixelBuffer::new(width, strip_h as u32, desc);

        let mut y_offset = 0usize;
        while y_offset < height as usize {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

            let h = strip_h.min(height as usize - y_offset);

            // Resize strip buffer for the last (possibly shorter) strip
            if h < strip_h {
                strip_pixels = PixelBuffer::new(width, h as u32, desc);
            }

            converter
                .convert_strip(y_offset, h, &mut strip_pixels)
                .map_err(|e| e.decompose().0)?;

            // Copy converted rows to sink buffer
            let mut sink_buf = sink
                .provide_next_buffer(y_offset as u32, h as u32, width, desc)
                .map_err(|e| at!(Error::Encode(e.to_string())))?;

            let src = strip_pixels.as_slice();
            let row_bytes = width as usize * bpp;
            for row in 0..h {
                let dst_row = sink_buf.row_mut(row as u32);
                let src_row = src.row(row as u32);
                dst_row[..row_bytes].copy_from_slice(&src_row[..row_bytes]);
            }

            y_offset += h;
        }

        sink.finish()
            .map_err(|e| at!(Error::Encode(e.to_string())))?;

        Ok(info)
    }

    /// Stream a grid image tile-row by tile-row to a sink.
    #[cfg(feature = "zencodec")]
    fn decode_grid_to_sink(
        &mut self,
        stop: &(impl Stop + ?Sized),
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<ImageInfo> {
        let grid_config = self
            .parser
            .grid_config()
            .ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Expected grid config but found none",
                })
            })?
            .clone();

        let grid_rows = grid_config.rows as usize;
        let cols = grid_config.columns as usize;
        let output_width = grid_config.output_width as usize;
        let output_height = grid_config.output_height as usize;

        let mut y_offset = 0u32;
        let mut began = false;

        for grid_row in 0..grid_rows {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

            // Decode and convert tiles for this row one at a time.
            // Each tile is decoded then converted before the next, so at most
            // one raw Frame + one converted PixelBuffer per tile is live.
            let mut row_tiles: Vec<PixelBuffer> = Vec::with_capacity(cols);
            for col in 0..cols {
                let tile_idx = grid_row * cols + col;
                let tile_data = self
                    .parser
                    .tile_data(tile_idx)
                    .map_err(|e| at!(Error::from(e)))?;
                let frame = Self::decode_frame(
                    &mut self.decoder,
                    &tile_data,
                    "Failed to decode grid tile",
                )?;
                let (pixels, _info) = self.convert_to_image(frame, None, stop)?;
                row_tiles.push(pixels);
            }

            // Get descriptor and tile height from the first tile
            let desc = row_tiles[0].descriptor();
            let bpp = desc.bytes_per_pixel();
            let tile_h = row_tiles[0].height() as usize;

            // Last tile-row may be clipped to output dimensions
            let strip_h = tile_h.min(output_height.saturating_sub(y_offset as usize));
            if strip_h == 0 {
                break;
            }

            // Signal begin on the first strip
            if !began {
                sink.begin(output_width as u32, output_height as u32, desc)
                    .map_err(|e| at!(Error::Encode(e.to_string())))?;
                began = true;
            }

            // Provide buffer from sink and stitch tiles into it
            let mut sink_buf = sink
                .provide_next_buffer(y_offset, strip_h as u32, output_width as u32, desc)
                .map_err(|e| at!(Error::Encode(e.to_string())))?;
            for py in 0..strip_h {
                let dst_row = sink_buf.row_mut(py as u32);
                let mut x_offset = 0usize;
                for tile in &row_tiles {
                    let tile_w = tile.width() as usize;
                    let actual_w = tile_w.min(output_width.saturating_sub(x_offset));
                    if actual_w == 0 {
                        continue;
                    }
                    let tile_slice = tile.as_slice();
                    let src = tile_slice.row(py as u32);
                    let copy_bytes = actual_w * bpp;
                    let dst_start = x_offset * bpp;
                    dst_row[dst_start..dst_start + copy_bytes].copy_from_slice(&src[..copy_bytes]);
                    x_offset += tile_w;
                }
            }

            y_offset += strip_h as u32;
        }

        if began {
            sink.finish()
                .map_err(|e| at!(Error::Encode(e.to_string())))?;
        }

        self.probe_info()
    }
}

/// Frame-by-frame animation decoder.
///
/// Yields one [`DecodedFrame`] at a time instead of loading the entire
/// animation into memory, making it suitable for large animations.
///
/// # Example
///
/// ```no_run
/// use zenavif::{AnimationDecoder, DecoderConfig};
/// use enough::Unstoppable;
///
/// let data = std::fs::read("animation.avif").unwrap();
/// let mut decoder = AnimationDecoder::new(&data, &DecoderConfig::default()).unwrap();
/// while let Some(frame) = decoder.next_frame(&Unstoppable).unwrap() {
///     println!("frame {}x{}, {}ms", frame.pixels.width(), frame.pixels.height(), frame.duration_ms);
/// }
/// ```
pub struct AnimationDecoder {
    /// Underlying decoder (owns parser + color decoder)
    inner: ManagedAvifDecoder,
    /// Separate decoder for the alpha track (inter-prediction needs its own state)
    alpha_decoder: Option<Rav1dDecoder>,
    /// Animation metadata
    info: DecodedAnimationInfo,
    /// Index of the next frame to decode
    frame_index: usize,
}

impl AnimationDecoder {
    /// Create a new frame-by-frame animation decoder.
    ///
    /// Returns [`Error::Unsupported`] if the file is not animated.
    pub fn new(data: &[u8], config: &DecoderConfig) -> Result<Self> {
        let inner = ManagedAvifDecoder::new(data, config)?;

        let anim_info = inner
            .parser
            .animation_info()
            .ok_or_else(|| at!(Error::Unsupported("not an animated AVIF")))?;

        let alpha_decoder = if anim_info.has_alpha {
            let mut settings = Settings::default();
            settings.threads = config.threads;
            settings.max_frame_delay = 1; // disable frame threading (DisjointMut race in rav1d CDEF)
            Some(Rav1dDecoder::with_settings(settings).map_err(|_e| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Failed to create alpha decoder",
                })
            })?)
        } else {
            None
        };

        let info = DecodedAnimationInfo {
            frame_count: anim_info.frame_count,
            loop_count: anim_info.loop_count,
            has_alpha: anim_info.has_alpha,
            timescale: anim_info.timescale,
        };

        Ok(Self {
            inner,
            alpha_decoder,
            info,
            frame_index: 0,
        })
    }

    /// Animation metadata (frame count, loop count, etc.).
    pub fn info(&self) -> &DecodedAnimationInfo {
        &self.info
    }

    /// Decode and return the next frame, or `None` if all frames have been decoded.
    pub fn next_frame(&mut self, stop: &(impl Stop + ?Sized)) -> Result<Option<DecodedFrame>> {
        if self.frame_index >= self.info.frame_count {
            return Ok(None);
        }

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let frame_ref = self
            .inner
            .parser
            .frame(self.frame_index)
            .map_err(|e| at!(Error::from(e)))?;

        let primary_frame = ManagedAvifDecoder::decode_anim_frame(
            &mut self.inner.decoder,
            &frame_ref.data,
            "Failed to decode animation frame",
        )?;

        let alpha_frame = match (&mut self.alpha_decoder, &frame_ref.alpha_data) {
            (Some(dec), Some(alpha_data)) => Some(ManagedAvifDecoder::decode_anim_frame(
                dec,
                alpha_data,
                "Failed to decode animation alpha frame",
            )?),
            _ => None,
        };

        let (pixels, _info) = self
            .inner
            .convert_to_image(primary_frame, alpha_frame, stop)?;

        let duration_ms = frame_ref.duration_ms;
        self.frame_index += 1;

        Ok(Some(DecodedFrame {
            pixels,
            duration_ms,
        }))
    }

    /// Number of frames remaining (not yet decoded).
    pub fn remaining_frames(&self) -> usize {
        self.info.frame_count.saturating_sub(self.frame_index)
    }

    /// Index of the next frame that will be decoded (0-based).
    pub fn frame_index(&self) -> usize {
        self.frame_index
    }
}

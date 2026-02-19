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
use imgref::ImgVec;
use rgb::{ComponentBytes, ComponentSlice, Rgb, Rgba};
use whereat::at;
use yuv::{YuvGrayImage, YuvPlanarImage, YuvRange, YuvStandardMatrix};
use zencodec_types::PixelData;

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
        let parse_config = zenavif_parse::DecodeConfig::default().lenient(true);
        let parser = zenavif_parse::AvifParser::from_owned_with_config(
            data.to_vec(),
            &parse_config,
            &enough::Unstoppable,
        )
        .map_err(|e| at(Error::from(e)))?;

        let settings = Settings {
            threads: config.threads,
            apply_grain: config.apply_grain,
            frame_size_limit: config.frame_size_limit,
            ..Default::default()
        };

        let decoder = Rav1dDecoder::with_settings(settings).map_err(|_e| {
            at(Error::Decode {
                code: -1,
                msg: "Failed to create decoder",
            })
        })?;

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
                    at(Error::Decode {
                        code: -1,
                        msg: "Failed to flush decoder",
                    })
                })?;
                frames.into_iter().last().ok_or_else(|| {
                    at(Error::Decode {
                        code: -1,
                        msg: context,
                    })
                })?
            }
            Err(_e) => {
                return Err(at(Error::Decode {
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
    pub fn decode(&mut self, stop: &(impl Stop + ?Sized)) -> Result<PixelData> {
        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Check if this is a grid image (tiled/multi-frame)
        if self.parser.grid_config().is_some() {
            return self.decode_grid(stop);
        }

        let primary_data = self.parser.primary_data().map_err(|e| at(Error::from(e)))?;
        let primary_frame = Self::decode_frame(
            &mut self.decoder,
            &primary_data,
            "Failed to decode primary frame",
        )?;

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        let alpha_frame = if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| at(Error::from(e)))?;
            Some(Self::decode_frame(
                &mut self.decoder,
                &alpha_data,
                "Failed to decode alpha frame",
            )?)
        } else {
            None
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        let (pixels, _info) = self.convert_to_image(primary_frame, alpha_frame, stop)?;
        Ok(pixels)
    }

    /// Decode the primary image and return both pixels and metadata.
    pub fn decode_full(&mut self, stop: &(impl Stop + ?Sized)) -> Result<(PixelData, ImageInfo)> {
        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        if self.parser.grid_config().is_some() {
            let pixels = self.decode_grid(stop)?;
            let info = self.probe_info()?;
            return Ok((pixels, info));
        }

        let primary_data = self.parser.primary_data().map_err(|e| at(Error::from(e)))?;
        let primary_frame = Self::decode_frame(
            &mut self.decoder,
            &primary_data,
            "Failed to decode primary frame",
        )?;

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        let alpha_frame = if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| at(Error::from(e)))?;
            Some(Self::decode_frame(
                &mut self.decoder,
                &alpha_data,
                "Failed to decode alpha frame",
            )?)
        } else {
            None
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        self.convert_to_image(primary_frame, alpha_frame, stop)
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
                .map_err(|e| at(Error::from(e)))?;
            (
                meta.max_frame_width.get() as u32,
                meta.max_frame_height.get() as u32,
            )
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
            .ok_or_else(|| at(Error::Unsupported("not an animated AVIF")))?;

        let mut alpha_decoder = if anim_info.has_alpha {
            let settings = Settings {
                threads: 1,
                ..Default::default()
            };
            Some(Rav1dDecoder::with_settings(settings).map_err(|_e| {
                at(Error::Decode {
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
            stop.check().map_err(|e| at(Error::Cancelled(e)))?;

            let frame_ref = self.parser.frame(i).map_err(|e| at(Error::from(e)))?;

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
                return Err(at(Error::Decode {
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

        Err(at(Error::Decode {
            code: -1,
            msg: context,
        }))
    }

    /// Decode a grid-based AVIF (tiled image)
    fn decode_grid(&mut self, stop: &(impl Stop + ?Sized)) -> Result<PixelData> {
        let grid_config = self
            .parser
            .grid_config()
            .expect("grid_config should be Some")
            .clone();

        // Decode all tiles
        let mut tile_frames = Vec::new();
        for i in 0..self.parser.grid_tile_count() {
            stop.check().map_err(|e| at(Error::Cancelled(e)))?;

            let tile_data = self.parser.tile_data(i).map_err(|e| at(Error::from(e)))?;
            let frame =
                Self::decode_frame(&mut self.decoder, &tile_data, "Failed to decode grid tile")?;

            tile_frames.push(frame);
        }

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Stitch tiles together
        self.stitch_tiles(tile_frames, &grid_config, stop)
    }

    /// Stitch decoded tile frames into a single image
    fn stitch_tiles(
        &self,
        tiles: Vec<Frame>,
        grid_config: &zenavif_parse::GridConfig,
        stop: &(impl Stop + ?Sized),
    ) -> Result<PixelData> {
        if tiles.is_empty() {
            return Err(at(Error::Decode {
                code: -1,
                msg: "No tiles to stitch",
            }));
        }

        let rows = grid_config.rows as usize;
        let cols = grid_config.columns as usize;

        if tiles.len() != rows * cols {
            return Err(at(Error::Decode {
                code: -1,
                msg: "Tile count doesn't match grid dimensions",
            }));
        }

        // Get dimensions from first tile (all tiles should be same size)
        let tile_width = tiles[0].width() as usize;
        let tile_height = tiles[0].height() as usize;
        let _bit_depth = tiles[0].bit_depth();
        let _layout = tiles[0].pixel_layout();

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

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Stitch tiles based on bit depth and alpha
        match &tile_images[0] {
            PixelData::Rgb8(_) => {
                self.stitch_rgb8(tile_images, rows, cols, output_width, output_height)
            }
            PixelData::Rgba8(_) => {
                self.stitch_rgba8(tile_images, rows, cols, output_width, output_height)
            }
            PixelData::Rgb16(_) => {
                self.stitch_rgb16(tile_images, rows, cols, output_width, output_height)
            }
            PixelData::Rgba16(_) => {
                self.stitch_rgba16(tile_images, rows, cols, output_width, output_height)
            }
            PixelData::Gray8(_) => {
                self.stitch_gray8(tile_images, rows, cols, output_width, output_height)
            }
            PixelData::Gray16(_) => {
                self.stitch_gray16(tile_images, rows, cols, output_width, output_height)
            }
            _ => Err(at(Error::Unsupported(
                "unsupported pixel format for grid stitching",
            ))),
        }
    }

    /// Stitch RGB8 tiles into final image
    fn stitch_rgb8(
        &self,
        tiles: Vec<PixelData>,
        _rows: usize,
        cols: usize,
        width: usize,
        height: usize,
    ) -> Result<PixelData> {
        use rgb::RGB8;
        let mut output = imgref::ImgVec::new(vec![RGB8::default(); width * height], width, height);

        for (tile_idx, tile) in tiles.iter().enumerate() {
            if let PixelData::Rgb8(tile_img) = tile {
                let row = tile_idx / cols;
                let col = tile_idx % cols;
                let tile_w = tile_img.width();
                let tile_h = tile_img.height();
                let dst_x = col * tile_w;
                let dst_y = row * tile_h;

                // Copy tile data to output
                for y in 0..tile_h.min(height - dst_y) {
                    for x in 0..tile_w.min(width - dst_x) {
                        output[(dst_x + x, dst_y + y)] = tile_img[(x, y)];
                    }
                }
            }
        }

        Ok(PixelData::Rgb8(output))
    }

    /// Stitch RGBA8 tiles into final image
    fn stitch_rgba8(
        &self,
        tiles: Vec<PixelData>,
        _rows: usize,
        cols: usize,
        width: usize,
        height: usize,
    ) -> Result<PixelData> {
        use rgb::RGBA8;
        let mut output = imgref::ImgVec::new(vec![RGBA8::default(); width * height], width, height);

        for (tile_idx, tile) in tiles.iter().enumerate() {
            if let PixelData::Rgba8(tile_img) = tile {
                let row = tile_idx / cols;
                let col = tile_idx % cols;
                let tile_w = tile_img.width();
                let tile_h = tile_img.height();
                let dst_x = col * tile_w;
                let dst_y = row * tile_h;

                for y in 0..tile_h.min(height - dst_y) {
                    for x in 0..tile_w.min(width - dst_x) {
                        output[(dst_x + x, dst_y + y)] = tile_img[(x, y)];
                    }
                }
            }
        }

        Ok(PixelData::Rgba8(output))
    }

    /// Stitch RGB16 tiles into final image
    fn stitch_rgb16(
        &self,
        tiles: Vec<PixelData>,
        _rows: usize,
        cols: usize,
        width: usize,
        height: usize,
    ) -> Result<PixelData> {
        use rgb::RGB16;
        let mut output = imgref::ImgVec::new(vec![RGB16::default(); width * height], width, height);

        for (tile_idx, tile) in tiles.iter().enumerate() {
            if let PixelData::Rgb16(tile_img) = tile {
                let row = tile_idx / cols;
                let col = tile_idx % cols;
                let tile_w = tile_img.width();
                let tile_h = tile_img.height();
                let dst_x = col * tile_w;
                let dst_y = row * tile_h;

                for y in 0..tile_h.min(height - dst_y) {
                    for x in 0..tile_w.min(width - dst_x) {
                        output[(dst_x + x, dst_y + y)] = tile_img[(x, y)];
                    }
                }
            }
        }

        Ok(PixelData::Rgb16(output))
    }

    /// Stitch RGBA16 tiles into final image
    fn stitch_rgba16(
        &self,
        tiles: Vec<PixelData>,
        _rows: usize,
        cols: usize,
        width: usize,
        height: usize,
    ) -> Result<PixelData> {
        use rgb::RGBA16;
        let mut output =
            imgref::ImgVec::new(vec![RGBA16::default(); width * height], width, height);

        for (tile_idx, tile) in tiles.iter().enumerate() {
            if let PixelData::Rgba16(tile_img) = tile {
                let row = tile_idx / cols;
                let col = tile_idx % cols;
                let tile_w = tile_img.width();
                let tile_h = tile_img.height();
                let dst_x = col * tile_w;
                let dst_y = row * tile_h;

                for y in 0..tile_h.min(height - dst_y) {
                    for x in 0..tile_w.min(width - dst_x) {
                        output[(dst_x + x, dst_y + y)] = tile_img[(x, y)];
                    }
                }
            }
        }

        Ok(PixelData::Rgba16(output))
    }

    /// Stitch Gray8 tiles into final image
    fn stitch_gray8(
        &self,
        tiles: Vec<PixelData>,
        _rows: usize,
        cols: usize,
        width: usize,
        height: usize,
    ) -> Result<PixelData> {
        let mut output =
            imgref::ImgVec::new(vec![rgb::Gray::new(0u8); width * height], width, height);

        for (tile_idx, tile) in tiles.iter().enumerate() {
            if let PixelData::Gray8(tile_img) = tile {
                let row = tile_idx / cols;
                let col = tile_idx % cols;
                let tile_w = tile_img.width();
                let tile_h = tile_img.height();
                let dst_x = col * tile_w;
                let dst_y = row * tile_h;

                for y in 0..tile_h.min(height - dst_y) {
                    for x in 0..tile_w.min(width - dst_x) {
                        output[(dst_x + x, dst_y + y)] = tile_img[(x, y)];
                    }
                }
            }
        }

        Ok(PixelData::Gray8(output))
    }

    /// Stitch Gray16 tiles into final image
    fn stitch_gray16(
        &self,
        tiles: Vec<PixelData>,
        _rows: usize,
        cols: usize,
        width: usize,
        height: usize,
    ) -> Result<PixelData> {
        let mut output =
            imgref::ImgVec::new(vec![rgb::Gray::new(0u16); width * height], width, height);

        for (tile_idx, tile) in tiles.iter().enumerate() {
            if let PixelData::Gray16(tile_img) = tile {
                let row = tile_idx / cols;
                let col = tile_idx % cols;
                let tile_w = tile_img.width();
                let tile_h = tile_img.height();
                let dst_x = col * tile_w;
                let dst_y = row * tile_h;

                for y in 0..tile_h.min(height - dst_y) {
                    for x in 0..tile_w.min(width - dst_x) {
                        output[(dst_x + x, dst_y + y)] = tile_img[(x, y)];
                    }
                }
            }
        }

        Ok(PixelData::Gray16(output))
    }

    /// Convert rav1d Frame to zenavif PixelData
    /// Crop an image to the specified dimensions
    fn crop_image(image: PixelData, width: usize, height: usize) -> Result<PixelData> {
        match image {
            PixelData::Rgb8(img) => {
                let mut cropped = vec![rgb::RGB8::default(); width * height];
                for y in 0..height.min(img.height()) {
                    for x in 0..width.min(img.width()) {
                        cropped[y * width + x] = img[(x, y)];
                    }
                }
                Ok(PixelData::Rgb8(ImgVec::new(cropped, width, height)))
            }
            PixelData::Rgba8(img) => {
                let mut cropped = vec![rgb::RGBA8::default(); width * height];
                for y in 0..height.min(img.height()) {
                    for x in 0..width.min(img.width()) {
                        cropped[y * width + x] = img[(x, y)];
                    }
                }
                Ok(PixelData::Rgba8(ImgVec::new(cropped, width, height)))
            }
            PixelData::Rgb16(img) => {
                let mut cropped = vec![rgb::RGB16::default(); width * height];
                for y in 0..height.min(img.height()) {
                    for x in 0..width.min(img.width()) {
                        cropped[y * width + x] = img[(x, y)];
                    }
                }
                Ok(PixelData::Rgb16(ImgVec::new(cropped, width, height)))
            }
            PixelData::Rgba16(img) => {
                let mut cropped = vec![rgb::RGBA16::default(); width * height];
                for y in 0..height.min(img.height()) {
                    for x in 0..width.min(img.width()) {
                        cropped[y * width + x] = img[(x, y)];
                    }
                }
                Ok(PixelData::Rgba16(ImgVec::new(cropped, width, height)))
            }
            PixelData::Gray8(img) => {
                let mut cropped = vec![rgb::Gray::new(0u8); width * height];
                for y in 0..height.min(img.height()) {
                    for x in 0..width.min(img.width()) {
                        cropped[y * width + x] = img[(x, y)];
                    }
                }
                Ok(PixelData::Gray8(ImgVec::new(cropped, width, height)))
            }
            PixelData::Gray16(img) => {
                let mut cropped = vec![rgb::Gray::new(0u16); width * height];
                for y in 0..height.min(img.height()) {
                    for x in 0..width.min(img.width()) {
                        cropped[y * width + x] = img[(x, y)];
                    }
                }
                Ok(PixelData::Gray16(ImgVec::new(cropped, width, height)))
            }
            _ => Err(at(Error::Unsupported(
                "unsupported pixel format for cropping",
            ))),
        }
    }

    fn convert_to_image(
        &self,
        primary: Frame,
        alpha: Option<Frame>,
        stop: &(impl Stop + ?Sized),
    ) -> Result<(PixelData, ImageInfo)> {
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
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        let info_clone = info.clone();
        let pixels = match bit_depth {
            8 => self.convert_8bit(primary, alpha, info, stop),
            10 | 12 => self.convert_16bit(primary, alpha, info, stop),
            _ => Err(at(Error::Decode {
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
    ) -> Result<PixelData> {
        let Planes::Depth8(planes) = primary.planes() else {
            return Err(at(Error::Decode {
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
        let buffer_pixel_count = buffer_width * buffer_height;

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
                        out.as_mut_slice().as_bytes_mut(),
                        rgb_stride,
                        yuv_range,
                        matrix,
                    )
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    PixelData::Rgba8(ImgVec::new(out, buffer_width, buffer_height))
                } else {
                    let mut out = vec![Rgb { r: 0u8, g: 0, b: 0 }; buffer_pixel_count];
                    let rgb_stride = buffer_width as u32 * 3;
                    yuv::yuv400_to_rgb(
                        &gray,
                        out.as_mut_slice().as_bytes_mut(),
                        rgb_stride,
                        yuv_range,
                        matrix,
                    )
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    PixelData::Rgb8(ImgVec::new(out, buffer_width, buffer_height))
                }
            }
            sampling => {
                let y_view = planes.y();
                let u_view = planes.u().ok_or_else(|| {
                    at(Error::Decode {
                        code: -1,
                        msg: "Missing U plane",
                    })
                })?;
                let v_view = planes.v().ok_or_else(|| {
                    at(Error::Decode {
                        code: -1,
                        msg: "Missing V plane",
                    })
                })?;

                #[allow(unused_variables)]
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
                    // Use our accurate custom YUV to RGB conversion, then add alpha
                    let our_range = to_our_yuv_range(info.color_range);
                    let our_matrix = to_our_yuv_matrix(info.matrix_coefficients);

                    let rgb_result = match sampling {
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
                        ChromaSampling::Monochrome => unreachable!(),
                    };

                    // Convert RGB to RGBA (with alpha=255 default)
                    let rgba_buf: Vec<Rgba<u8>> = rgb_result
                        .buf()
                        .iter()
                        .map(|rgb| Rgba {
                            r: rgb.r,
                            g: rgb.g,
                            b: rgb.b,
                            a: 255,
                        })
                        .collect();

                    PixelData::Rgba8(ImgVec::new(rgba_buf, buffer_width, buffer_height))
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
                        ChromaSampling::Monochrome => unreachable!(),
                    };

                    PixelData::Rgb8(result)
                }
            }
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        // Crop to display dimensions if needed
        if needs_crop {
            image = Self::crop_image(image, display_width, display_height)?;
        }

        // Handle alpha channel if present
        if let Some(alpha_frame) = alpha {
            let Planes::Depth8(alpha_planes) = alpha_frame.planes() else {
                return Err(at(Error::Decode {
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
    ) -> Result<PixelData> {
        let Planes::Depth16(planes) = primary.planes() else {
            return Err(at(Error::Decode {
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
        let buffer_pixel_count = buffer_width * buffer_height;

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
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        12 => yuv::y012_to_rgba12(
                            &gray,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        _ => yuv::y016_to_rgba16(
                            &gray,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    PixelData::Rgba16(ImgVec::new(out, buffer_width, buffer_height))
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
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        12 => yuv::y012_to_rgb12(
                            &gray,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        _ => yuv::y016_to_rgb16(
                            &gray,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    PixelData::Rgb16(ImgVec::new(out, buffer_width, buffer_height))
                }
            }
            sampling => {
                let y_view = planes.y();
                let u_view = planes.u().ok_or_else(|| {
                    at(Error::Decode {
                        code: -1,
                        msg: "Missing U plane",
                    })
                })?;
                let v_view = planes.v().ok_or_else(|| {
                    at(Error::Decode {
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
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (10, ChromaSampling::Cs422) => yuv::i210_to_rgba10(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (10, ChromaSampling::Cs444) => yuv::i410_to_rgba10(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs420) => yuv::i012_to_rgba12(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs422) => yuv::i212_to_rgba12(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs444) => yuv::i412_to_rgba12(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs420) => yuv::i016_to_rgba16(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs422) => yuv::i216_to_rgba16(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs444) => yuv::i416_to_rgba16(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Monochrome) => unreachable!(),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    PixelData::Rgba16(ImgVec::new(out, buffer_width, buffer_height))
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
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (10, ChromaSampling::Cs422) => yuv::i210_to_rgb10(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (10, ChromaSampling::Cs444) => yuv::i410_to_rgb10(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs420) => yuv::i012_to_rgb12(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs422) => yuv::i212_to_rgb12(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (12, ChromaSampling::Cs444) => yuv::i412_to_rgb12(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs420) => yuv::i016_to_rgb16(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs422) => yuv::i216_to_rgb16(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Cs444) => yuv::i416_to_rgb16(
                            &planar,
                            out.as_mut_slice().as_mut_slice(),
                            rgb_stride,
                            yuv_range,
                            matrix,
                        ),
                        (_, ChromaSampling::Monochrome) => unreachable!(),
                    }
                    .map_err(|e| at(Error::ColorConversion(e)))?;
                    PixelData::Rgb16(ImgVec::new(out, buffer_width, buffer_height))
                }
            }
        };

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

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
                return Err(at(Error::Decode {
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
            .ok_or_else(|| at(Error::Unsupported("not an animated AVIF")))?;

        let alpha_decoder = if anim_info.has_alpha {
            let settings = Settings {
                threads: 1,
                ..Default::default()
            };
            Some(Rav1dDecoder::with_settings(settings).map_err(|_e| {
                at(Error::Decode {
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

        stop.check().map_err(|e| at(Error::Cancelled(e)))?;

        let frame_ref = self
            .inner
            .parser
            .frame(self.frame_index)
            .map_err(|e| at(Error::from(e)))?;

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

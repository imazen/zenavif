//! Metadata derivation: everything that answers a question about the image
//! without producing pixels.
//!
//! H.273 matrix resolution, [`ImageInfo`] construction (both from a decoded
//! frame and decode-free via [`ManagedAvifDecoder::probe_info`]), gain-map
//! extraction, and the small container-query accessors.

use super::ManagedAvifDecoder;
use super::cicp_map::{
    convert_chroma_sampling, convert_color_primaries, convert_color_range, convert_matrix,
    convert_transfer,
};
use crate::cicp_resolve::{self, ResolvedMatrix};
use crate::error::{Error, Result};
use crate::image::{
    ChromaSampling, ColorPrimaries, ColorRange, ImageInfo, MatrixCoefficients,
    TransferCharacteristics,
};
use rav1d_safe::src::managed::{Frame, PixelLayout};

/// Which independently coded image owns the properties used for conversion.
#[derive(Clone, Copy)]
pub(super) enum MetadataSource {
    Primary,
    Animation,
}

impl ManagedAvifDecoder {
    pub(super) fn color_info_for(
        &self,
        source: MetadataSource,
    ) -> Option<&zenavif_parse::ColorInformation> {
        match source {
            MetadataSource::Primary => self.parser.color_info(),
            MetadataSource::Animation => self.parser.animation_color_info(),
        }
    }

    pub(super) fn nclx_info_for(
        &self,
        source: MetadataSource,
    ) -> Option<&zenavif_parse::ColorInformation> {
        match source {
            MetadataSource::Primary => self.parser.nclx_color_info(),
            MetadataSource::Animation => self.parser.animation_nclx_color_info(),
        }
    }

    pub(super) fn color_fields_for(
        &self,
        source: MetadataSource,
        fallback_primaries: ColorPrimaries,
        fallback_transfer: TransferCharacteristics,
    ) -> (ColorPrimaries, TransferCharacteristics, Option<Vec<u8>>) {
        let (primaries, transfer) = match self.nclx_info_for(source) {
            Some(zenavif_parse::ColorInformation::Nclx {
                color_primaries,
                transfer_characteristics,
                ..
            }) => (
                ColorPrimaries(*color_primaries as u8),
                TransferCharacteristics(*transfer_characteristics as u8),
            ),
            _ => (fallback_primaries, fallback_transfer),
        };
        let icc = match self.color_info_for(source) {
            Some(zenavif_parse::ColorInformation::IccProfile(icc)) => Some(icc.clone()),
            _ => None,
        };
        (primaries, transfer, icc)
    }

    pub(super) fn exif_for(&self, source: MetadataSource) -> Result<Option<Vec<u8>>> {
        let value = match source {
            MetadataSource::Primary => self.parser.exif(),
            MetadataSource::Animation => self.parser.animation_exif(),
        };
        value
            .transpose()
            .map(|v| v.map(|v| v.into_owned()))
            .map_err(|e| e.map_error(Error::Parse))
    }

    pub(super) fn xmp_for(&self, source: MetadataSource) -> Result<Option<Vec<u8>>> {
        let value = match source {
            MetadataSource::Primary => self.parser.xmp(),
            MetadataSource::Animation => self.parser.animation_xmp(),
        };
        value
            .transpose()
            .map(|v| v.map(|v| v.into_owned()))
            .map_err(|e| e.map_error(Error::Parse))
    }

    pub(super) fn spatial_for(&self, source: MetadataSource) -> crate::AnimationSpatialMetadata {
        match source {
            MetadataSource::Primary => crate::AnimationSpatialMetadata {
                rotation: self.parser.rotation().copied(),
                mirror: self.parser.mirror().copied(),
                clean_aperture: self.parser.clean_aperture().copied(),
                pixel_aspect_ratio: self.parser.pixel_aspect_ratio().copied(),
            },
            MetadataSource::Animation => self
                .parser
                .animation_info()
                .map(|a| a.spatial)
                .unwrap_or_default(),
        }
    }

    pub(super) fn premultiplied_for(&self, source: MetadataSource) -> bool {
        match source {
            MetadataSource::Primary => self.parser.premultiplied_alpha(),
            MetadataSource::Animation => {
                self.parser.animation_premultiplied_alpha().unwrap_or(false)
            }
        }
    }

    /// Resolve the H.273 matrix for conversion, honestly.
    ///
    /// `info.matrix_coefficients` carries the *signaled* AV1-bitstream
    /// code (kept raw for metadata passthrough); `info.color_primaries`
    /// already carries the container-precedence effective primaries.
    /// The container `nclx` matrix — discarded by the bitstream-
    /// authoritative precedence — is consulted only as the hint for
    /// MC=2/reserved, per the zenpixels#36 resolution contract.
    pub(super) fn resolved_matrix_for(&self, info: &ImageInfo) -> Result<ResolvedMatrix> {
        self.resolved_matrix_for_source(info, MetadataSource::Primary)
    }

    pub(super) fn resolved_matrix_for_source(
        &self,
        info: &ImageInfo,
        source: MetadataSource,
    ) -> Result<ResolvedMatrix> {
        // Hint chain for an unspecified/reserved bitstream MC, per the
        // documented AVIF precedence ("container colr > AV1 bitstream >
        // AVIF defaults 1/13/6"): a *valid* container `nclx` matrix
        // first (its MC is otherwise discarded by the bitstream-
        // authoritative precedence), else the AVIF-spec default —
        // including when the nclx itself says MC=2, which the av1-avif
        // guidance disambiguates to the defaults exactly like absent
        // signaling (and which real ICC-centric writers emit). A spec
        // default is documented disambiguation, not a guess; the
        // honest-error class stays with genuinely unimplemented math
        // (YCgCo/CL/ICtCp/underivable MC=12).
        let hint = match self.nclx_info_for(source) {
            Some(zenavif_parse::ColorInformation::Nclx {
                matrix_coefficients,
                ..
            }) if cicp_resolve::is_resolvable_hint(*matrix_coefficients as u8) => {
                Some(*matrix_coefficients as u8)
            }
            _ => Some(cicp_resolve::AVIF_DEFAULT_MC),
        };
        cicp_resolve::resolve(info.matrix_coefficients.0, info.color_primaries.0, hint)
    }

    /// Build ImageInfo from a decoded primary frame and parser metadata.
    ///
    /// Factored out of `convert_to_image` for reuse by `decode_to_strip_converter`.
    // WIP: used by decode_to_strip_converter above
    #[allow(dead_code)]
    pub(super) fn build_image_info(&self, primary: &Frame, has_alpha: bool) -> Result<ImageInfo> {
        let width = primary.width() as usize;
        let height = primary.height() as usize;
        let bit_depth = primary.bit_depth();
        let layout = primary.pixel_layout();

        let av1_color = primary.color_info();
        let matrix_coefficients = convert_matrix(av1_color.matrix_coefficients);
        let color_range = convert_color_range(av1_color.color_range);

        let (color_primaries, transfer_characteristics, icc_profile) = self.color_fields_for(
            MetadataSource::Primary,
            convert_color_primaries(av1_color.primaries),
            convert_transfer(av1_color.transfer_characteristics),
        );

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

    /// Opt in to native grayscale output for alpha-free monochrome
    /// images (zencodec adapter negotiation; see `convert_*_monochrome_gray`).
    pub(crate) fn set_native_gray(&mut self, on: bool) {
        self.native_gray = on;
    }

    /// Probe image metadata without decoding pixels.
    ///
    /// Uses the AVIF container parser and AV1 sequence header to extract
    /// dimensions, color info, ICC profile, EXIF, XMP, orientation, and HDR metadata.
    /// Does NOT do full AV1 frame decoding.
    pub fn probe_info(&self) -> Result<ImageInfo> {
        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| e.map_error(Error::Parse))?;
        if primary_data.is_empty() && self.parser.animation_info().is_some() {
            return self.probe_animation_info();
        }
        // Get dimensions from grid config or AV1 sequence header
        let (width, height) = if let Some(grid) = self.parser.grid_config() {
            (grid.output_width, grid.output_height)
        } else {
            let meta = zenavif_parse::AV1Metadata::parse_av1_bitstream(&primary_data)
                .map_err(|e| e.map_error(Error::Parse))?;
            (meta.max_frame_width.get(), meta.max_frame_height.get())
        };

        let has_alpha = self.parser.alpha_metadata().is_some();

        // AV1 config for bit depth
        let bit_depth = self.parser.av1_config().map(|c| c.bit_depth).unwrap_or(8);

        let (color_primaries, transfer_characteristics, icc_profile) = self.color_fields_for(
            MetadataSource::Primary,
            ColorPrimaries::BT709,
            TransferCharacteristics::SRGB,
        );
        let (matrix_coefficients, color_range) = match self.parser.nclx_color_info() {
            Some(zenavif_parse::ColorInformation::Nclx {
                matrix_coefficients,
                full_range,
                ..
            }) => (
                MatrixCoefficients(*matrix_coefficients as u8),
                if *full_range {
                    ColorRange::Full
                } else {
                    ColorRange::Limited
                },
            ),
            _ => (MatrixCoefficients::BT601, ColorRange::Full),
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

    /// Probe the animation color track independently of its optional poster.
    pub fn probe_animation_info(&self) -> Result<ImageInfo> {
        let source = MetadataSource::Animation;
        let track = self
            .parser
            .animation_info()
            .ok_or_else(|| whereat::at!(Error::InvalidParameters("not an animated AVIF".into())))?;
        let frame = self
            .parser
            .frame(0)
            .map_err(|e| e.map_error(Error::Parse))?;
        let meta = zenavif_parse::AV1Metadata::parse_av1_bitstream(&frame.data)
            .map_err(|e| e.map_error(Error::Parse))?;
        let (width, height) = (meta.max_frame_width.get(), meta.max_frame_height.get());
        let has_alpha = track.has_alpha;
        let bit_depth = meta.bit_depth;

        let (color_primaries, transfer_characteristics, icc_profile) = self.color_fields_for(
            source,
            ColorPrimaries(meta.color_primaries),
            TransferCharacteristics(meta.transfer_characteristics),
        );
        let matrix_coefficients = MatrixCoefficients(meta.matrix_coefficients);
        let color_range = if meta.full_range {
            ColorRange::Full
        } else {
            ColorRange::Limited
        };

        let chroma_sampling = if meta.monochrome {
            ChromaSampling::Monochrome
        } else if meta.chroma_subsampling.horizontal && meta.chroma_subsampling.vertical {
            ChromaSampling::Cs420
        } else if meta.chroma_subsampling.horizontal {
            ChromaSampling::Cs422
        } else {
            ChromaSampling::Cs444
        };

        Ok(ImageInfo {
            width,
            height,
            bit_depth,
            has_alpha,
            premultiplied_alpha: self.premultiplied_for(source),
            monochrome: chroma_sampling == ChromaSampling::Monochrome,
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            color_range,
            chroma_sampling,
            icc_profile,
            rotation: track.spatial.rotation,
            mirror: track.spatial.mirror,
            clean_aperture: track.spatial.clean_aperture,
            pixel_aspect_ratio: track.spatial.pixel_aspect_ratio,
            content_light_level: track.hdr.content_light_level,
            mastering_display: track.hdr.mastering_display,
            exif: self.exif_for(source)?,
            xmp: self.xmp_for(source)?,
            gain_map: self.extract_gain_map(),
            // Depth map extraction requires zenavif-parse > 0.4.0 (not yet published).
            depth_map: None,
        })
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
    pub(super) fn extract_gain_map(&self) -> Option<crate::image::AvifGainMap> {
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
}

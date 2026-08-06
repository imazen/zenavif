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

impl ManagedAvifDecoder {
    /// Resolve the H.273 matrix for conversion, honestly.
    ///
    /// `info.matrix_coefficients` carries the *signaled* AV1-bitstream
    /// code (kept raw for metadata passthrough); `info.color_primaries`
    /// already carries the container-precedence effective primaries.
    /// The container `nclx` matrix — discarded by the bitstream-
    /// authoritative precedence — is consulted only as the hint for
    /// MC=2/reserved, per the zenpixels#36 resolution contract.
    pub(super) fn resolved_matrix_for(&self, info: &ImageInfo) -> Result<ResolvedMatrix> {
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
        let hint = match self.parser.color_info() {
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
        // Get dimensions from grid config or AV1 sequence header
        let (width, height) = if let Some(grid) = self.parser.grid_config() {
            (grid.output_width, grid.output_height)
        } else {
            let meta = self
                .parser
                .primary_metadata()
                .map_err(|e| e.map_error(Error::Parse))?;
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

//! [`AvifEncoder`] — the single-image encoder: config assembly, the pre-flight
//! limit checks + memory-budget thread pin, descriptor to CICP / bit-depth
//! propagation, the per-pixel-format encode helpers, and the
//! [`zencodec::encode::Encoder`] boundary.

use std::sync::Arc;

use crate::pixel::ImageSpecExt as _;
use rgb::{Rgb, Rgba};
use whereat::{At, at};
use zencodec::encode::EncodeOutput;
use zencodec::{CodecError, ImageFormat, ResourceLimits};
use zenpixels::{ImageSpec, PixelView};

use super::threads::fit_encode_threads_to_memory;
use crate::error::Error;

#[cfg(test)]
mod tests;

/// Single-image AVIF encoder.
#[cfg(feature = "encode")]
pub struct AvifEncoder {
    pub(super) planned_orientation: Option<zencodec::encode::OrientationPlan>,
    pub(super) declaration: Option<zencodec::encode::EncodeDeclaration>,
    pub(super) config: crate::EncoderConfig,
    /// The job's control. The native encoders poll it for cancellation only.
    pub(super) pulse: Option<zencodec::SharedPulse>,
    pub(super) exif: Option<Arc<[u8]>>,
    pub(super) icc_profile: Option<Arc<[u8]>>,
    pub(super) xmp: Option<Arc<[u8]>>,
    pub(super) limits: ResourceLimits,
    /// CICP resolved by [`resolve_color_emit`] in `encoder()` (caller-supplied
    /// CICP, possibly derived from an ICC). When set, it is the authority — the
    /// pixel-descriptor color in `apply_descriptor_color` only fills axes this
    /// leaves *unspecified*, so a caller's CICP is never clobbered.
    pub(super) caller_cicp: Option<zencodec::Cicp>,
    /// Record of a memory-budget thread reduction made by `checked_config`
    /// (reductions are never silent). Attached to the [`EncodeOutput`] as a
    /// `String` extra in `make_output`.
    pub(super) threads_note: Option<String>,
    /// Applied to the pixels before encoding; the file says orientation 1.
    pub(super) orientation: zencodec::Orientation,
}

#[cfg(feature = "encode")]
impl AvifEncoder {
    fn validate_declared(
        &self,
        width: u32,
        height: u32,
        descriptor: ImageSpec,
        has_color: bool,
    ) -> Result<(), At<CodecError>> {
        if let Some(declaration) = &self.declaration
            && (width != declaration.width
                || height != declaration.height
                || descriptor != declaration.descriptor
                || has_color)
        {
            return Err(super::declared::refusal(
                zencodec::encode::EncodePlanError::Changed,
            ));
        }
        Ok(())
    }

    fn build_config(&self) -> crate::EncoderConfig {
        let mut cfg = self.config.clone();
        if let Some(ref exif) = self.exif {
            cfg = cfg.exif(exif.to_vec());
        }
        if let Some(ref icc) = self.icc_profile {
            cfg = cfg.icc_profile(icc.to_vec());
        }
        if let Some(ref xmp) = self.xmp {
            cfg = cfg.xmp(xmp.to_vec());
        }
        cfg
    }

    /// Pre-flight resource checks + memory-adaptive thread fit, returning the
    /// final native config for this encode (any budget-driven thread pin
    /// applied on top of [`build_config`](Self::build_config)).
    ///
    /// Checks, in order: dimension caps; the raw input-buffer size against
    /// `max_memory_bytes` (cheap floor, kept from the pre-calibrated era);
    /// then the CALIBRATED thread-aware peak estimate at the fitted thread
    /// count via [`fit_encode_threads_to_memory`] — which reduces the thread
    /// count to fit the budget and errors only when even the single-threaded
    /// estimate does not fit. Any reduction is recorded in
    /// [`threads_note`](Self::threads_note) (surfaced by `make_output`).
    fn checked_config(
        &mut self,
        w: usize,
        h: usize,
        bpp: u64,
    ) -> Result<crate::EncoderConfig, At<Error>> {
        self.limits
            .check_dimensions(w as u32, h as u32)
            .map_err(|_| {
                at!(Error::ImageTooLarge {
                    width: w as u32,
                    height: h as u32,
                })
            })?;
        let estimated_mem = w as u64 * h as u64 * bpp;
        self.limits
            .check_memory(estimated_mem)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;
        let (pin, note) = fit_encode_threads_to_memory(
            &self.limits,
            &self.config,
            w as u32,
            h as u32,
            bpp as u8,
        )?;
        self.threads_note = note;
        let mut cfg = self.build_config();
        if let Some(n) = pin {
            cfg = cfg.threads(Some(n));
        }
        Ok(cfg)
    }

    fn make_output(&self, data: Vec<u8>) -> Result<EncodeOutput, At<Error>> {
        self.limits
            .check_output_size(data.len() as u64)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;
        let mut out = EncodeOutput::new(data, ImageFormat::Avif);
        if let Some(ref note) = self.threads_note {
            // Reductions are never silent: readable by the caller via
            // `output.extras::<String>()` (no zenavif-specific type — this
            // crate adds no public API for it).
            out = out.with_extras(note.clone());
        }
        Ok(out)
    }

    /// An owned cancellation view for the native encoder threads; the shared
    /// pulse itself stays with this encoder.
    fn stop_token(&self) -> almost_enough::StopToken {
        match &self.pulse {
            Some(p) => almost_enough::StopToken::new(p.clone()),
            None => almost_enough::StopToken::new(enough::Unstoppable),
        }
    }

    /// Fill in CICP color axes from the pixel descriptor for the axes a
    /// caller-supplied CICP left *unspecified*. A `Metadata`-set CICP (resolved
    /// in `encoder()` into [`caller_cicp`](Self::caller_cicp) and already lowered
    /// onto the config) is the authority and is never overwritten here — fixing
    /// the prior bug where the descriptor unconditionally clobbered the caller's
    /// primaries/transfer (and never set a matching matrix). For HDR transfers
    /// (PQ/HLG), also switches to 10-bit encoding depth.
    fn apply_descriptor_color(&mut self, desc: &ImageSpec) {
        use zenpixels::{Primaries, Transfer};

        let transfer = desc.transfer();
        let primaries = desc.primaries();

        // f32 input that isn't PQ/HLG is stored as 8-bit sRGB
        // (`linear_to_srgb_u8`), so the file's transfer is sRGB whatever the
        // input's was.
        let stored_srgb = desc.layout.sample == zenpixels::SampleType::F32
            && !matches!(transfer, Transfer::Pq | Transfer::Hlg(_));

        // Map transfer function to CICP transfer_characteristics
        let tc = match transfer {
            _ if stored_srgb => Some(13),
            Transfer::Pq => Some(16u8),
            Transfer::Hlg(_) => Some(18),
            Transfer::Bt709 => Some(1),
            Transfer::Srgb => Some(13),
            Transfer::Linear => Some(8),
            _ => None,
        };

        // Map color primaries to CICP color_primaries
        let cp = match primaries {
            Primaries::Bt2020 => Some(9u8),
            Primaries::DisplayP3 => Some(12),
            Primaries::Bt709 => Some(1),
            _ => None,
        };

        // Which axes did the caller's CICP already pin? An axis is "specified"
        // only if caller_cicp is present AND the code point is not the H.273
        // unspecified/reserved sentinel (primaries/transfer: 0 reserved, 2
        // unspecified). Matrix 0 is Identity — a real value, not unspecified.
        // A caller's "linear" described the f32 input, not the stored sRGB:
        // move it to sRGB, and re-derive a profile that was synthesized from
        // it (a caller's own ICC can't be re-derived and is kept).
        if stored_srgb
            && let Some(c) = self.caller_cicp
            && c.transfer_characteristics == 8
        {
            let fixed =
                zencodec::Cicp::new(c.color_primaries, 13, c.matrix_coefficients, c.full_range);
            let (_, synthesized) = super::encode_job::resolve_avif_color(Some(c), None, None);
            if self.icc_profile.is_some() && self.icc_profile == synthesized {
                self.icc_profile = super::encode_job::resolve_avif_color(Some(fixed), None, None).1;
            }
            self.caller_cicp = Some(fixed);
            self.config = self.config.clone().transfer_characteristics(13);
        }
        let caller = self.caller_cicp;
        let cp_specified = caller.is_some_and(|c| !matches!(c.color_primaries, 0 | 2));
        let tc_specified = caller.is_some_and(|c| !matches!(c.transfer_characteristics, 0 | 2));

        // Fill only the unspecified axes from the descriptor, so the caller's
        // CICP wins on the axes it pinned.
        if let Some(tc_val) = tc
            && !tc_specified
        {
            self.config = self.config.clone().transfer_characteristics(tc_val);
        }
        if let Some(cp_val) = cp
            && !cp_specified
        {
            self.config = self.config.clone().color_primaries(cp_val);
        }

        // The descriptor describes RGB input pixels. Its identity matrix is
        // not a request for identity-coded AV1: the backend chooses the matrix
        // of its RGB-to-YUV conversion. Keep any explicit native config value.

        // For PQ/HLG, switch to 10-bit depth (the native HDR depth for AV1)
        if matches!(transfer, Transfer::Pq | Transfer::Hlg(_)) {
            self.config = self.config.clone().bit_depth(crate::EncodeBitDepth::Ten);
        }

        // Map narrow signal range to limited pixel range
        if desc.range() == zenpixels::SignalRange::Narrow {
            self.config = self
                .config
                .clone()
                .pixel_range(crate::EncodePixelRange::Limited);
        }
    }

    /// Convert f32 RGB pixels to u16 and encode via the 16-bit path.
    /// Used for HDR (PQ/HLG) f32 data that would be corrupted by linear_to_srgb_u8().
    fn encode_f32_as_u16_rgb(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 6)?; // 6 bytes per u16 RGB pixel
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        let rgb: Vec<Rgb<u16>> = raw
            .as_chunks::<12>()
            .0
            .iter()
            .map(|c| {
                let r = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let g = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                let b = f32::from_le_bytes([c[8], c[9], c[10], c[11]]);
                Rgb {
                    r: (r.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    g: (g.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    b: (b.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(rgb, w, h);
        let result = crate::encode_rgb16(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    /// Convert f32 RGBA pixels to u16 and encode via the 16-bit path.
    /// Used for HDR (PQ/HLG) f32 data that would be corrupted by linear_to_srgb_u8().
    fn encode_f32_as_u16_rgba(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 8)?; // 8 bytes per u16 RGBA pixel
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        let rgba: Vec<Rgba<u16>> = raw
            .as_chunks::<16>()
            .0
            .iter()
            .map(|c| {
                let r = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let g = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                let b = f32::from_le_bytes([c[8], c[9], c[10], c[11]]);
                let a = f32::from_le_bytes([c[12], c[13], c[14], c[15]]);
                Rgba {
                    r: (r.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    g: (g.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    b: (b.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                    a: (a.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(rgba, w, h);
        let result = crate::encode_rgba16(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    // ── Per-format encode helpers ──────────────────────────────────────

    fn do_encode_rgb8(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 3)?;
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        let rgb: &[Rgb<u8>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgb, w, h);
        let result = crate::encode_rgb8(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba8(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 4)?;
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        let rgba: &[Rgba<u8>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgba, w, h);
        let result = crate::encode_rgba8(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_gray8(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 1)?;
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        #[cfg(feature = "encode-mono")]
        {
            // True monochrome AV1 (Cs400): luma-only bitstream, no chroma
            // RDO — measured 2-3x faster at output-byte parity vs the RGB
            // expansion (imazen/zenavif#6).
            let img = imgref::ImgRef::new(&raw, w, h);
            let result = crate::encode_gray8(img, &cfg, stop)?;
            self.make_output(result.avif_file)
        }
        #[cfg(not(feature = "encode-mono"))]
        {
            // Gray → RGB for encoding. RGB→YCbCr of R=G=B is exactly
            // neutral chroma, so this is pixel-safe — just slower (chroma
            // RDO still runs). The `encode-mono` feature routes through
            // zenravif's Cs400 path instead.
            let rgb: Vec<Rgb<u8>> = raw.iter().map(|&g| Rgb { r: g, g, b: g }).collect();
            let img = imgref::ImgVec::new(rgb, w, h);
            let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
            self.make_output(result.avif_file)
        }
    }

    fn do_encode_rgb_f32(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 12)?;
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        let rgb: Vec<Rgb<u8>> = raw
            .as_chunks::<12>()
            .0
            .iter()
            .map(|c| {
                let r = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let g = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                let b = f32::from_le_bytes([c[8], c[9], c[10], c[11]]);
                Rgb {
                    r: linear_to_srgb_u8(r.clamp(0.0, 1.0)),
                    g: linear_to_srgb_u8(g.clamp(0.0, 1.0)),
                    b: linear_to_srgb_u8(b.clamp(0.0, 1.0)),
                }
            })
            .collect();
        let img = imgref::ImgVec::new(rgb, w, h);
        let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba_f32(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 16)?;
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        let rgba: Vec<Rgba<u8>> = raw
            .as_chunks::<16>()
            .0
            .iter()
            .map(|c| {
                let r = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let g = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                let b = f32::from_le_bytes([c[8], c[9], c[10], c[11]]);
                let a = f32::from_le_bytes([c[12], c[13], c[14], c[15]]);
                Rgba {
                    r: linear_to_srgb_u8(r.clamp(0.0, 1.0)),
                    g: linear_to_srgb_u8(g.clamp(0.0, 1.0)),
                    b: linear_to_srgb_u8(b.clamp(0.0, 1.0)),
                    a: (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(rgba, w, h);
        let result = crate::encode_rgba8(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_gray_f32(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 4)?;
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        let rgb: Vec<Rgb<u8>> = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                let v = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let s = linear_to_srgb_u8(v.clamp(0.0, 1.0));
                Rgb { r: s, g: s, b: s }
            })
            .collect();
        let img = imgref::ImgVec::new(rgb, w, h);
        let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgb16(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 6)?;
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        let rgb: &[Rgb<u16>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgb, w, h);
        let result = crate::encode_rgb16(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba16(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 8)?;
        let stop = self.stop_token();
        let raw = pixels
            .try_contiguous_bytes()
            .map_err(|_| at!(Error::OutOfMemory))?;
        let rgba: &[Rgba<u16>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgba, w, h);
        let result = crate::encode_rgba16(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }
}

#[cfg(feature = "encode")]
impl zencodec::encode::Encoder for AvifEncoder {
    fn begin_declared(
        &mut self,
        declaration: &zencodec::encode::EncodeDeclaration,
    ) -> Result<zencodec::encode::EncodePlan, Self::Error> {
        if self.declaration.is_some() {
            return Err(super::declared::refusal(
                zencodec::encode::EncodePlanError::Changed,
            ));
        }
        if !super::declared::color_supported(&self.config)
            || self.icc_profile.is_some()
            || self.caller_cicp.is_some_and(|c| c != zencodec::Cicp::SRGB)
        {
            return Err(super::declared::refusal(
                zencodec::UnsupportedOperation::EncodePlanning,
            ));
        }
        let plan = super::declared::plan(
            declaration,
            self.planned_orientation,
            self.orientation,
            &self.limits,
        )?;
        self.declaration = Some(
            zencodec::encode::EncodeDeclaration::new(
                declaration.width,
                declaration.height,
                declaration.descriptor.clone(),
            )
            .with_access(declaration.access),
        );
        Ok(plan)
    }

    type Error = At<CodecError>;

    fn reject(op: zencodec::UnsupportedOperation) -> At<CodecError> {
        // Bare native error exiting the trait boundary: `.into()` routes through
        // `From<Error> for At<CodecError>`, locating + enveloping in one step.
        Error::UnsupportedOperation(op).into()
    }

    fn encode_srgba8(
        self,
        data: &mut [u8],
        make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> Result<EncodeOutput, At<CodecError>> {
        self.validate_declared(width, height, ImageSpec::rgba8_srgb(), false)?;
        self.encode_srgba8_inner(data, make_opaque, width, height, stride_pixels)
            .map_err(zencodec::CodecError::of)
    }

    fn encode(self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<CodecError>> {
        self.validate_declared(pixels.width(), pixels.rows(), pixels.spec().clone(), false)?;
        self.encode_inner(pixels).map_err(zencodec::CodecError::of)
    }
}

#[cfg(feature = "encode")]
impl AvifEncoder {
    fn encode_srgba8_inner(
        mut self,
        data: &mut [u8],
        make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> Result<EncodeOutput, At<Error>> {
        if self.orientation != zencodec::Orientation::Identity {
            let o = self.orientation;
            self.orientation = zencodec::Orientation::Identity;
            let src = PixelView::new(
                data,
                width,
                height,
                stride_pixels as usize * 4,
                ImageSpec::rgba8_srgb(),
            )
            .map_err(|e| at!(Error::InvalidParameters(format!("pixel buffer: {e}"))))?;
            let upright = zenpixels_convert::orient::apply_orientation(&src, o)
                .map_err(|_| at!(Error::OutOfMemory))?;
            let (ow, oh) = (upright.width(), upright.height());
            let mut bytes = upright
                .try_into_vec::<u8>()
                .map_err(|_| at!(Error::OutOfMemory))?;
            return self.encode_srgba8_inner(&mut bytes, make_opaque, ow, oh, ow);
        }
        let w = width as usize;
        let h = height as usize;
        let stride = stride_pixels as usize;
        let cfg = self.checked_config(w, h, 4)?;
        let stop = self.stop_token();

        if make_opaque {
            // Strip alpha: RGBA → RGB in-place, then encode RGB
            let mut rgb = Vec::with_capacity(w * h);
            for y in 0..h {
                let row_start = y * stride * 4;
                let row = &data[row_start..row_start + w * 4];
                for px in row.as_chunks::<4>().0.iter() {
                    rgb.push(Rgb {
                        r: px[0],
                        g: px[1],
                        b: px[2],
                    });
                }
            }
            let img = imgref::ImgVec::new(rgb, w, h);
            let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
            self.make_output(result.avif_file)
        } else {
            // Zero-copy RGBA path — bytemuck cast the contiguous region
            if stride == w {
                let pixel_bytes = &data[..w * h * 4];
                let rgba: &[Rgba<u8>] = bytemuck::cast_slice(pixel_bytes);
                let img = imgref::Img::new(rgba, w, h);
                let result = crate::encode_rgba8(img, &cfg, stop)?;
                self.make_output(result.avif_file)
            } else {
                // Strided: use ImgRef with stride
                let total_pixels = (h - 1) * stride + w;
                let pixel_bytes = &data[..total_pixels * 4];
                let rgba: &[Rgba<u8>] = bytemuck::cast_slice(pixel_bytes);
                let img = imgref::Img::new_stride(rgba, w, h, stride);
                let result = crate::encode_rgba8(img, &cfg, stop)?;
                self.make_output(result.avif_file)
            }
        }
    }

    fn encode_inner(mut self, pixels: PixelView<'_>) -> Result<EncodeOutput, At<Error>> {
        use zenpixels::SampleLayout;

        if self.orientation != zencodec::Orientation::Identity {
            let upright = zenpixels_convert::orient::apply_orientation(&pixels, self.orientation)
                .map_err(|_| at!(Error::OutOfMemory))?;
            self.orientation = zencodec::Orientation::Identity;
            return self.encode_inner(upright.as_view());
        }

        // Propagate HDR color metadata from pixel descriptor to encoder config
        let desc = pixels.spec();
        self.apply_descriptor_color(desc);

        // For f32 pixels with HDR transfer (PQ/HLG), convert to u16 and use 16-bit
        // path to preserve HDR data. The default f32 path uses linear_to_srgb_u8()
        // which would silently destroy HDR values.
        let is_hdr_transfer = matches!(
            desc.transfer(),
            zenpixels::Transfer::Pq | zenpixels::Transfer::Hlg(_)
        );

        match desc.layout {
            SampleLayout::RGBF32 if is_hdr_transfer => {
                return self.encode_f32_as_u16_rgb(pixels);
            }
            SampleLayout::RGBAF32 if is_hdr_transfer => {
                return self.encode_f32_as_u16_rgba(pixels);
            }
            _ => {}
        }

        match desc.layout {
            SampleLayout::RGB8 => self.do_encode_rgb8(pixels),
            SampleLayout::RGBA8 => self.do_encode_rgba8(pixels),
            SampleLayout::GRAY8 => self.do_encode_gray8(pixels),
            SampleLayout::RGB16 => self.do_encode_rgb16(pixels),
            SampleLayout::RGBA16 => self.do_encode_rgba16(pixels),
            SampleLayout::RGBF32 => self.do_encode_rgb_f32(pixels),
            SampleLayout::RGBAF32 => self.do_encode_rgba_f32(pixels),
            SampleLayout::GRAYF32 => self.do_encode_gray_f32(pixels),
            SampleLayout::BGRA8 => {
                // Swizzle BGRA → RGBA and encode
                let raw = pixels
                    .try_contiguous_bytes()
                    .map_err(|_| at!(Error::OutOfMemory))?;
                let w = pixels.width() as usize;
                let h = pixels.rows() as usize;
                let cfg = self.checked_config(w, h, 4)?;
                let stop = self.stop_token();
                let rgba: Vec<Rgba<u8>> = raw
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| Rgba {
                        r: c[2],
                        g: c[1],
                        b: c[0],
                        a: c[3],
                    })
                    .collect();
                let img = imgref::ImgVec::new(rgba, w, h);
                let result = crate::encode_rgba8(img.as_ref(), &cfg, stop)?;
                self.make_output(result.avif_file)
            }
            SampleLayout::RGBX8 => {
                // Byte 3 is padding — strip to 3-channel RGB.
                let raw = pixels
                    .try_contiguous_bytes()
                    .map_err(|_| at!(Error::OutOfMemory))?;
                let w = pixels.width() as usize;
                let h = pixels.rows() as usize;
                let cfg = self.checked_config(w, h, 3)?;
                let stop = self.stop_token();
                let rgb: Vec<Rgb<u8>> = raw
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| Rgb {
                        r: c[0],
                        g: c[1],
                        b: c[2],
                    })
                    .collect();
                let img = imgref::ImgVec::new(rgb, w, h);
                let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
                self.make_output(result.avif_file)
            }
            SampleLayout::BGRX8 => {
                // BGRA layout with padding byte — swap B↔R and strip to RGB.
                let raw = pixels
                    .try_contiguous_bytes()
                    .map_err(|_| at!(Error::OutOfMemory))?;
                let w = pixels.width() as usize;
                let h = pixels.rows() as usize;
                let cfg = self.checked_config(w, h, 3)?;
                let stop = self.stop_token();
                let rgb: Vec<Rgb<u8>> = raw
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| Rgb {
                        r: c[2],
                        g: c[1],
                        b: c[0],
                    })
                    .collect();
                let img = imgref::ImgVec::new(rgb, w, h);
                let result = crate::encode_rgb8(img.as_ref(), &cfg, stop)?;
                self.make_output(result.avif_file)
            }
            _ => Err(at!(Error::UnsupportedOperation(
                zencodec::UnsupportedOperation::PixelFormat,
            ))),
        }
    }
}

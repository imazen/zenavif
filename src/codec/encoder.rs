//! [`AvifEncoder`] — the single-image encoder: config assembly, the pre-flight
//! limit checks + memory-budget thread pin, descriptor to CICP / bit-depth
//! propagation, the per-pixel-format encode helpers, and the
//! [`zencodec::encode::Encoder`] boundary.

use std::sync::Arc;

use rgb::{Rgb, Rgba};
use whereat::{At, at};
use zencodec::encode::EncodeOutput;
use zencodec::{CodecError, ImageFormat, ResourceLimits};
use zenpixels::{PixelDescriptor, PixelSlice};

use super::threads::fit_encode_threads_to_memory;
use crate::error::Error;

#[cfg(test)]
mod tests;

/// Single-image AVIF encoder.
#[cfg(feature = "encode")]
pub struct AvifEncoder {
    pub(super) config: crate::EncoderConfig,
    pub(super) stop: Option<zencodec::StopToken>,
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
}

#[cfg(feature = "encode")]
impl AvifEncoder {
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

    fn stop_token(&self) -> almost_enough::StopToken {
        match &self.stop {
            Some(s) => s.clone(),
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
    fn apply_descriptor_color(&mut self, desc: PixelDescriptor) {
        use zenpixels::{ColorPrimaries, TransferFunction};

        let transfer = desc.transfer;
        let primaries = desc.primaries;

        // Map transfer function to CICP transfer_characteristics
        let tc = match transfer {
            TransferFunction::Pq => Some(16u8),
            TransferFunction::Hlg => Some(18),
            TransferFunction::Bt709 => Some(1),
            TransferFunction::Srgb => Some(13),
            TransferFunction::Linear => Some(8),
            _ => None,
        };

        // Map color primaries to CICP color_primaries
        let cp = match primaries {
            ColorPrimaries::Bt2020 => Some(9u8),
            ColorPrimaries::DisplayP3 => Some(12),
            ColorPrimaries::Bt709 => Some(1),
            _ => None,
        };

        // Which axes did the caller's CICP already pin? An axis is "specified"
        // only if caller_cicp is present AND the code point is not the H.273
        // unspecified/reserved sentinel (primaries/transfer: 0 reserved, 2
        // unspecified). Matrix 0 is Identity — a real value, not unspecified.
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

        // Keep the matrix consistent with the primaries/transfer we just wrote.
        // When the caller supplied no CICP at all, its matrix wasn't applied in
        // encoder(), so derive one from the descriptor here (RGB content ⇒
        // Identity/0, matching `Cicp::from_descriptor`) so the config carries a
        // coherent triple (informational — no available backend reads the
        // matrix field). When the caller DID supply a CICP, its matrix is
        // already on the config; leave it alone.
        if caller.is_none() && (cp.is_some() || tc.is_some()) {
            let mc = zenpixels::Cicp::from_descriptor(&desc)
                .map(|c| c.matrix_coefficients)
                .unwrap_or(0);
            self.config = self.config.clone().matrix_coefficients(mc);
        }

        // For PQ/HLG, switch to 10-bit depth (the native HDR depth for AV1)
        if matches!(transfer, TransferFunction::Pq | TransferFunction::Hlg) {
            self.config = self.config.clone().bit_depth(crate::EncodeBitDepth::Ten);
        }

        // Map narrow signal range to limited pixel range
        if desc.signal_range == zenpixels::SignalRange::Narrow {
            self.config = self
                .config
                .clone()
                .pixel_range(crate::EncodePixelRange::Limited);
        }
    }

    /// Convert f32 RGB pixels to u16 and encode via the 16-bit path.
    /// Used for HDR (PQ/HLG) f32 data that would be corrupted by linear_to_srgb_u8().
    fn encode_f32_as_u16_rgb(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 6)?; // 6 bytes per u16 RGB pixel
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
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
    fn encode_f32_as_u16_rgba(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 8)?; // 8 bytes per u16 RGBA pixel
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
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

    fn do_encode_rgb8(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 3)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: &[Rgb<u8>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgb, w, h);
        let result = crate::encode_rgb8(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba8(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 4)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgba: &[Rgba<u8>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgba, w, h);
        let result = crate::encode_rgba8(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_gray8(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 1)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
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

    fn do_encode_rgb_f32(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 12)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
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

    fn do_encode_rgba_f32(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 16)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
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

    fn do_encode_gray_f32(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use linear_srgb::default::linear_to_srgb_u8;
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 4)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
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

    fn do_encode_rgb16(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 6)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgb: &[Rgb<u16>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgb, w, h);
        let result = crate::encode_rgb16(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }

    fn do_encode_rgba16(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        let w = pixels.width() as usize;
        let h = pixels.rows() as usize;
        let cfg = self.checked_config(w, h, 8)?;
        let stop = self.stop_token();
        let raw = pixels.contiguous_bytes();
        let rgba: &[Rgba<u16>] = bytemuck::cast_slice(&raw);
        let img = imgref::Img::new(rgba, w, h);
        let result = crate::encode_rgba16(img, &cfg, stop)?;
        self.make_output(result.avif_file)
    }
}

#[cfg(feature = "encode")]
impl zencodec::encode::Encoder for AvifEncoder {
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
        self.encode_srgba8_inner(data, make_opaque, width, height, stride_pixels)
            .map_err(zencodec::CodecError::of)
    }

    fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<CodecError>> {
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

    fn encode_inner(mut self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, At<Error>> {
        use zenpixels::PixelFormat;

        // Propagate HDR color metadata from pixel descriptor to encoder config
        let desc = pixels.descriptor();
        self.apply_descriptor_color(desc);

        // For f32 pixels with HDR transfer (PQ/HLG), convert to u16 and use 16-bit
        // path to preserve HDR data. The default f32 path uses linear_to_srgb_u8()
        // which would silently destroy HDR values.
        let is_hdr_transfer = matches!(
            desc.transfer,
            zenpixels::TransferFunction::Pq | zenpixels::TransferFunction::Hlg
        );

        match desc.pixel_format() {
            PixelFormat::RgbF32 if is_hdr_transfer => {
                return self.encode_f32_as_u16_rgb(pixels);
            }
            PixelFormat::RgbaF32 if is_hdr_transfer => {
                return self.encode_f32_as_u16_rgba(pixels);
            }
            _ => {}
        }

        match desc.pixel_format() {
            PixelFormat::Rgb8 => self.do_encode_rgb8(pixels),
            PixelFormat::Rgba8 => self.do_encode_rgba8(pixels),
            PixelFormat::Gray8 => self.do_encode_gray8(pixels),
            PixelFormat::Rgb16 => self.do_encode_rgb16(pixels),
            PixelFormat::Rgba16 => self.do_encode_rgba16(pixels),
            PixelFormat::RgbF32 => self.do_encode_rgb_f32(pixels),
            PixelFormat::RgbaF32 => self.do_encode_rgba_f32(pixels),
            PixelFormat::GrayF32 => self.do_encode_gray_f32(pixels),
            PixelFormat::Bgra8 => {
                // Swizzle BGRA → RGBA and encode
                let raw = pixels.contiguous_bytes();
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
            PixelFormat::Rgbx8 => {
                // Byte 3 is padding — strip to 3-channel RGB.
                let raw = pixels.contiguous_bytes();
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
            PixelFormat::Bgrx8 => {
                // BGRA layout with padding byte — swap B↔R and strip to RGB.
                let raw = pixels.contiguous_bytes();
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

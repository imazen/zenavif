//! [`AvifDecoderConfig`] — the [`zencodec::decode::DecoderConfig`] adapter: the
//! inherent builders, the `decode_into_*` convenience methods with their row
//! walkers, and the supported-descriptor / capability statics.

use std::borrow::Cow;

use rgb::{Rgb, Rgba};
use whereat::At;
use zencodec::decode::DecodeOutput;
use zencodec::{CodecError, ImageFormat, ImageInfo, ResourceLimits};
use zenpixels::{PixelBuffer, PixelDescriptor};
use zenpixels_convert::PixelBufferConvertTypedExt as _;

use super::decode_job::AvifDecodeJob;

/// AVIF decoder configuration implementing [`zencodec::decode::DecoderConfig`].
#[derive(Clone, Debug)]
pub struct AvifDecoderConfig {
    pub(super) inner: crate::DecoderConfig,
    /// When true, gain map and depth map data will be attached to
    /// `DecodeOutput` extras. Default: false.
    extract_gain_map: bool,
    /// How to handle the image's stored orientation (`irot`/`imir`).
    ///
    /// Default: [`OrientationHint::Preserve`](zencodec::OrientationHint::Preserve)
    /// — the decoder does **not** bake the orientation into the pixels: decode
    /// returns pixels in stored orientation and [`ImageInfo`] reports the
    /// stored dims + the intrinsic [`Orientation`](zencodec::Orientation) tag.
    /// With [`Correct`](zencodec::OrientationHint::Correct) the decoder applies
    /// the orientation and reports display dimensions with
    /// [`Orientation::Identity`](zencodec::Orientation::Identity).
    ///
    /// Set via [`with_orientation`](Self::with_orientation) or
    /// [`DecodeJob::with_orientation`](zencodec::decode::DecodeJob::with_orientation).
    pub(super) orientation: zencodec::OrientationHint,
}

impl AvifDecoderConfig {
    /// Create a default AVIF decoder config.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: crate::DecoderConfig::new(),
            extract_gain_map: false,
            orientation: zencodec::OrientationHint::Preserve,
        }
    }

    /// Set how the decoder handles the image's stored orientation
    /// (`irot`/`imir`). See [`orientation`](Self::orientation) for semantics.
    /// Default [`OrientationHint::Preserve`](zencodec::OrientationHint::Preserve).
    #[must_use]
    pub fn with_orientation(mut self, hint: zencodec::OrientationHint) -> Self {
        self.orientation = hint;
        self
    }

    /// Set resource limits.
    #[must_use]
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        if let Some(max_pixels) = limits.max_pixels {
            self.inner = self
                .inner
                .frame_size_limit(max_pixels.min(u32::MAX as u64) as u32);
        }
        if let Some(max_w) = limits.max_width
            && let Some(max_h) = limits.max_height
        {
            let max = max_w as u64 * max_h as u64;
            self.inner = self.inner.frame_size_limit(max.min(u32::MAX as u64) as u32);
        }
        self
    }

    /// Access the underlying [`crate::DecoderConfig`].
    /// Set the number of decode threads (0 = auto).
    #[must_use]
    pub fn with_threads(mut self, threads: u32) -> Self {
        self.inner = self.inner.threads(threads);
        self
    }

    /// Apply film grain synthesis during decode.
    #[must_use]
    pub fn with_film_grain(mut self, apply: bool) -> Self {
        self.inner = self.inner.apply_grain(apply);
        self
    }

    /// Enable extraction of gain map and depth map data into `DecodeOutput`
    /// extras.
    ///
    /// When enabled, the gain map is converted to a normalized
    /// [`GainMapSource`](zencodec::gainmap::GainMapSource) and attached to
    /// decode output so callers can retrieve it via
    /// `output.extras::<zencodec::gainmap::GainMapSource>()`.
    /// Depth maps (when available) are attached as
    /// [`AvifDepthMap`](crate::AvifDepthMap). Default: **false**.
    ///
    /// `ImageInfo` metadata (`supplements`, `GainMapPresence`) is always
    /// populated from container probe data regardless of this flag.
    #[must_use]
    pub fn with_extract_gain_map(mut self, extract: bool) -> Self {
        self.extract_gain_map = extract;
        self
    }

    /// Access the underlying [`crate::DecoderConfig`].
    #[must_use]
    pub fn inner(&self) -> &crate::DecoderConfig {
        &self.inner
    }

    /// Mutable access to the underlying [`crate::DecoderConfig`].
    pub fn inner_mut(&mut self) -> &mut crate::DecoderConfig {
        &mut self.inner
    }

    /// Convenience: decode image with this config.
    pub fn decode(&self, data: &[u8]) -> Result<DecodeOutput, At<CodecError>> {
        use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        self.clone()
            .job()
            .decoder(Cow::Borrowed(data), &[])?
            .decode()
    }

    /// Convenience: probe image header with this config.
    pub fn probe_header(&self, data: &[u8]) -> Result<ImageInfo, At<CodecError>> {
        use zencodec::decode::{DecodeJob as _, DecoderConfig as _};
        self.clone().job().probe(data)
    }

    /// Convenience: probe full image metadata (may be expensive).
    pub fn probe_full(&self, data: &[u8]) -> Result<ImageInfo, At<CodecError>> {
        use zencodec::decode::{DecodeJob as _, DecoderConfig as _};
        self.clone().job().probe_full(data)
    }

    /// Convenience: decode into a pre-allocated RGB8 buffer.
    pub fn decode_into_rgb8(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgb<u8>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        map_rgb8_rows(&output.into_buffer(), &mut dst, |s, d| {
            d.copy_from_slice(s);
        });
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated RGBA8 buffer.
    pub fn decode_into_rgba8(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgba<u8>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        let output = self.decode(data)?;
        let info = output.info().clone();
        map_rgba8_rows(&output.into_buffer(), &mut dst, |s, d| {
            d.copy_from_slice(s);
        });
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated RGB f32 buffer.
    pub fn decode_into_rgb_f32(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgb<f32>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        map_rgb8_rows(&output.into_buffer(), &mut dst, |s, d| {
            for (px, out) in s.iter().zip(d.iter_mut()) {
                *out = Rgb {
                    r: srgb_u8_to_linear(px.r),
                    g: srgb_u8_to_linear(px.g),
                    b: srgb_u8_to_linear(px.b),
                };
            }
        });
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated RGBA f32 buffer.
    pub fn decode_into_rgba_f32(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, Rgba<f32>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        map_rgba8_rows(&output.into_buffer(), &mut dst, |s, d| {
            for (px, out) in s.iter().zip(d.iter_mut()) {
                *out = Rgba {
                    r: srgb_u8_to_linear(px.r),
                    g: srgb_u8_to_linear(px.g),
                    b: srgb_u8_to_linear(px.b),
                    a: px.a as f32 / 255.0,
                };
            }
        });
        Ok(info)
    }

    /// Convenience: decode into a pre-allocated Gray f32 buffer.
    pub fn decode_into_gray_f32(
        &self,
        data: &[u8],
        mut dst: imgref::ImgRefMut<'_, rgb::Gray<f32>>,
    ) -> Result<ImageInfo, At<CodecError>> {
        use linear_srgb::default::srgb_u8_to_linear;
        let output = self.decode(data)?;
        let info = output.info().clone();
        // BT.709 luma coefficients in linear light
        let (kr, kb) =
            crate::yuv_convert::matrix_coefficients(crate::yuv_convert::YuvMatrix::Bt709);
        let kg = 1.0 - kr - kb;
        map_rgb8_rows(&output.into_buffer(), &mut dst, |s, d| {
            for (px, out) in s.iter().zip(d.iter_mut()) {
                let r = srgb_u8_to_linear(px.r);
                let g = srgb_u8_to_linear(px.g);
                let b = srgb_u8_to_linear(px.b);
                *out = rgb::Gray(kr * r + kg * g + kb * b);
            }
        });
        Ok(info)
    }
}

/// Borrow a decoded buffer as `ImgRef<Rgb<u8>>`, converting only when it isn't
/// already RGB8 sRGB — this skips the redundant identity convert + full-image
/// allocation an unconditional `to_rgb8()` would do. Rows are walked in
/// O(height) (the prior `nth(y)` per-row lookup was O(height²)). `f` writes
/// each output row over the overlapping `min(width)`×`min(height)` region.
fn map_rgb8_rows<D>(
    buffer: &PixelBuffer,
    dst: &mut imgref::ImgRefMut<'_, D>,
    mut f: impl FnMut(&[Rgb<u8>], &mut [D]),
) {
    let converted;
    // The borrow-free fast path needs BOTH an exact descriptor match and a
    // stride that is a whole number of pixels (`try_as_imgref`'s second
    // condition). An odd-stride RGB8 buffer satisfies the first and not the
    // second, so fall back to the owning conversion instead of panicking.
    let src = if let Some(view) = buffer
        .try_as_imgref::<Rgb<u8>>()
        .filter(|_| buffer.descriptor() == PixelDescriptor::RGB8_SRGB)
    {
        view
    } else {
        converted = buffer.to_rgb8();
        converted.as_imgref()
    };
    let w = dst.width().min(src.width());
    let h = dst.height().min(src.height());
    for (src_row, dst_row) in src.rows().zip(dst.rows_mut()).take(h) {
        f(&src_row[..w], &mut dst_row[..w]);
    }
}

/// Like [`map_rgb8_rows`] but views the buffer as RGBA8 (preserving alpha).
fn map_rgba8_rows<D>(
    buffer: &PixelBuffer,
    dst: &mut imgref::ImgRefMut<'_, D>,
    mut f: impl FnMut(&[Rgba<u8>], &mut [D]),
) {
    let converted;
    // Same fast-path/fallback split as `map_rgb8_rows` — see the note there.
    let src = if let Some(view) = buffer
        .try_as_imgref::<Rgba<u8>>()
        .filter(|_| buffer.descriptor() == PixelDescriptor::RGBA8_SRGB)
    {
        view
    } else {
        converted = buffer.to_rgba8();
        converted.as_imgref()
    };
    let w = dst.width().min(src.width());
    let h = dst.height().min(src.height());
    for (src_row, dst_row) in src.rows().zip(dst.rows_mut()).take(h) {
        f(&src_row[..w], &mut dst_row[..w]);
    }
}

impl Default for AvifDecoderConfig {
    fn default() -> Self {
        Self::new()
    }
}

static DECODE_DESCRIPTORS: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::RGB16_SRGB,
    PixelDescriptor::RGBA16_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::GRAY16_SRGB,
];

static AVIF_DECODE_CAPABILITIES: zencodec::decode::DecodeCapabilities =
    zencodec::decode::DecodeCapabilities::new()
        .with_icc(true)
        .with_exif(true)
        .with_xmp(true)
        .with_cicp(true)
        .with_stop(true)
        .with_animation(true)
        .with_cheap_probe(true)
        .with_streaming(true)
        .with_hdr(true)
        .with_gain_map(true)
        .with_reconstructs_hdr(true)
        .with_native_gray(true)
        .with_native_16bit(true)
        .with_native_alpha(true)
        .with_enforces_max_pixels(true)
        .with_enforces_max_memory(true)
        .with_enforces_max_input_bytes(true)
        .with_threads_supported_range(1, 256);

impl zencodec::decode::DecoderConfig for AvifDecoderConfig {
    type Error = At<CodecError>;
    type Job<'a> = AvifDecodeJob;

    fn formats() -> &'static [ImageFormat] {
        &[ImageFormat::Avif]
    }

    fn supported_descriptors() -> &'static [PixelDescriptor] {
        DECODE_DESCRIPTORS
    }

    fn capabilities() -> &'static zencodec::decode::DecodeCapabilities {
        &AVIF_DECODE_CAPABILITIES
    }

    /// Core-adjusted resource estimate via zencodec's unified `estimate` API.
    ///
    /// Mirrors [`estimate_encode_resources`](AvifEncoderConfig::estimate_encode_resources):
    /// delegates to the crate's calibrated [`crate::heuristics::estimate_decode`]
    /// (peak memory / time, keyed on the decoded output bytes-per-pixel) and maps
    /// the local [`crate::heuristics::ThreadingInfo`] onto the shared
    /// [`zencodec::estimate::ThreadingInformation`] only here at the boundary, so
    /// the local heuristics stay decoupled from the optional `zencodec`
    /// dependency.
    ///
    /// The estimate covers the *whole* decode: the AV1 frame/tile working set
    /// (owned by `rav1d-safe`) plus zenavif's own RGB(A) output buffer. AVIF
    /// decode is only partly parallel (tile decode parallelises; the YUV→RGB
    /// conversion does not), so the [`ThreadingInformation`] knee is
    /// conservative — see [`crate::heuristics::decode_threading_info`].
    fn estimate_decode_resources(
        &self,
        image: &zencodec::estimate::ImageCharacteristics,
        compute: &zencodec::estimate::ComputeEnvironment,
    ) -> zencodec::estimate::ResourceEstimate {
        use zencodec::estimate::{ResourceEstimate, ThreadingInformation};
        let bpp = image.descriptor().bytes_per_pixel() as u8;
        let lti = crate::heuristics::decode_threading_info(image.pixels());
        let ti = if lti.parallel {
            ThreadingInformation::parallel(lti.max_useful_threads)
        } else {
            ThreadingInformation::SERIAL
        };
        match crate::heuristics::estimate_decode(image.width(), image.height(), bpp) {
            Some(e) => ResourceEstimate::new(e.peak_memory_bytes, e.time_ms as u64)
                .with_threading(ti)
                .at_cores(compute.cores()),
            None => ResourceEstimate::conservative(image).at_cores(compute.cores()),
        }
    }

    fn job<'a>(self) -> Self::Job<'a> {
        let extract_gain_map = self.extract_gain_map;
        let orientation = self.orientation;
        AvifDecodeJob {
            config: self,
            stop: None,
            limits: ResourceLimits::none(),
            start_frame_index: 0,
            policy: None,
            extract_gain_map,
            gain_map_render: zencodec::GainMapRender::default(),
            orientation,
        }
    }
}

#[cfg(test)]
mod tests {
    /// `AllocPreference` must not change decoded pixels: decoding the same
    /// AVIF under `Fallible` (the `try_reserve` path), `Infallible` (the
    /// `vec!` path), and the default (`CodecDefault`) must produce
    /// byte-identical output. Uses a real 8-bit 4:2:0 photo fixture, which
    /// exercises the big full-image RGB output buffer + the per-row scratch.
    mod alloc_pref_decode {
        use super::super::{AvifDecoderConfig, DecodeOutput};
        use alloc::borrow::Cow;
        use whereat::At;
        use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
        use zencodec::{AllocPreference, CodecError, ResourceLimits};

        extern crate alloc;

        /// A real, committed 8-bit 4:2:0 AVIF (kodim03). The default-features
        /// (decode-only) build can decode it without the `encode` feature.
        const KODIM03: &[u8] =
            include_bytes!("../../tests/vectors/libavif/kodim03_yuv420_8bpc.avif");

        fn decode_bytes(
            encoded: &[u8],
            pref: Option<AllocPreference>,
        ) -> Result<Vec<u8>, At<CodecError>> {
            let job = AvifDecoderConfig::new().job();
            let job = match pref {
                Some(p) => {
                    job.with_limits(ResourceLimits::none().with_prefer_fallible_allocations(p))
                }
                None => job,
            };
            let out: DecodeOutput = job.decoder(Cow::Borrowed(encoded), &[])?.decode()?;
            Ok(out.into_buffer().copy_to_contiguous_bytes())
        }

        #[test]
        fn fallible_alloc_decode_matches_default() {
            let default = decode_bytes(KODIM03, None).expect("default decode");
            let fallible =
                decode_bytes(KODIM03, Some(AllocPreference::Fallible)).expect("fallible decode");
            let infallible = decode_bytes(KODIM03, Some(AllocPreference::Infallible))
                .expect("infallible decode");
            assert!(!default.is_empty(), "decode produced no pixels");
            assert_eq!(
                default, fallible,
                "Fallible decode must be byte-identical to the default decode"
            );
            assert_eq!(
                default, infallible,
                "Infallible decode must be byte-identical to the default decode"
            );
        }
    }
}

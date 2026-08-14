//! [`AvifDecodeJob`] — the per-operation decode job: limits/policy lowering
//! into a native [`crate::DecoderConfig`], the input-size and decode-limit
//! checks, probe / output-info, and construction of each decoder flavour
//! (buffered, streaming, animation, row-sink).

use std::borrow::Cow;
use std::sync::Arc;

use enough::Stop;
use whereat::{At, at};
use zencodec::{CodecError, ImageInfo, ImageSequence, ResourceLimits};
use zenpixels::PixelDescriptor;

use super::anim_decoder::AvifAnimationFrameDecoder;
use super::color::{
    attach_source_color_context, color_context_for_layout, icc_allows_native_gray,
    native_source_color, set_cicp_on_pixels,
};
use super::decode_config::AvifDecoderConfig;
use super::decoder::AvifDecoder;
use super::gain_map::reconstruct_hdr_pixels;
use super::info::{apply_decode_policy, convert_native_info};
use super::negotiate::{
    apply_strip_reduction, negotiate_format, negotiate_strip_descriptor, wants_gray_output,
};
use super::orientation::{
    apply_reported_orientation, bake_orientation, intrinsic_orientation,
    reported_dims_and_orientation, will_auto_orient,
};
use super::streaming::AvifStreamingDecoder;
use super::threads::policy_to_threads;
use crate::error::Error;

/// Per-operation AVIF decode job.
pub struct AvifDecodeJob {
    pub(super) config: AvifDecoderConfig,
    pub(super) stop: Option<zencodec::StopToken>,
    pub(super) limits: ResourceLimits,
    pub(super) start_frame_index: u32,
    pub(super) policy: Option<zencodec::decode::DecodePolicy>,
    /// When true, attach gain map and depth map data to `DecodeOutput` extras.
    /// Default: false. Metadata (supplements, `GainMapPresence`) is always
    /// populated regardless of this flag.
    pub(super) extract_gain_map: bool,
    /// Gain-map rendition intent (zencodec 0.1.21). `Components` decodes
    /// the gain-map AV1 payload into a
    /// [`zencodec::decode::DecodedGainMap`]; `ReconstructHdr` additionally
    /// applies it (ultrahdr-core) producing linear f32 HDR pixels — see
    /// `with_gain_map_render`. Default `BaseOnly`.
    pub(super) gain_map_render: zencodec::GainMapRender,
    /// How to handle the image's stored orientation (`irot`/`imir`).
    /// Default [`OrientationHint::Preserve`](zencodec::OrientationHint::Preserve).
    pub(super) orientation: zencodec::OrientationHint,
}

impl AvifDecodeJob {
    fn effective_config(&self) -> crate::DecoderConfig {
        let mut cfg = self.config.inner.clone();
        if let Some(max_pixels) = self.limits.max_pixels {
            cfg = cfg.frame_size_limit(max_pixels.min(u32::MAX as u64) as u32);
        }
        // Apply threading policy from ResourceLimits.
        // Skip Parallel — it means "use the ambient pool", so keep the codec's
        // own default (which is 1 thread to avoid the rav1d-safe DisjointMut
        // race condition).
        if !matches!(self.limits.threading(), zencodec::ThreadingPolicy::Parallel) {
            let threads = policy_to_threads(self.limits.threading());
            cfg = cfg.threads(threads);
        }
        // Forward resource limits to the container parser.
        if let Some(mem) = self.limits.max_memory_bytes {
            cfg.parser_peak_memory_limit = Some(mem);
        }
        if let Some(px) = self.limits.max_pixels {
            // Convert pixels to megapixels (round up to avoid zero).
            let mp = px.div_ceil(1_000_000).min(u32::MAX as u64) as u32;
            cfg.parser_total_megapixels_limit = Some(mp);
        }
        if let Some(frames) = self.limits.max_frames {
            cfg.parser_max_animation_frames = Some(frames);
        }
        // Honor the allocation-fallibility preference for zenavif's own decode
        // buffers (output / grid / crop / per-row scratch). `CodecDefault` (the
        // default) keeps each site's own default fallibility. The AV1 frame/tile
        // buffers are owned by `rav1d-safe` and are not affected.
        cfg.alloc_pref = self.limits.prefer_fallible_allocations.into();
        cfg
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

    /// Check input data size against limits.
    fn check_input_size(&self, data: &[u8]) -> Result<(), At<Error>> {
        self.limits
            .check_input_size(data.len() as u64)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;
        Ok(())
    }

    /// Check decoded image dimensions and estimated memory against limits.
    fn check_decode_limits(&self, info: &crate::image::ImageInfo) -> Result<(), At<Error>> {
        self.limits
            .check_dimensions(info.width, info.height)
            .map_err(|_| {
                at!(Error::ImageTooLarge {
                    width: info.width,
                    height: info.height,
                })
            })?;
        // Estimate output memory: width * height * max_bpp (4 bytes for RGBA8, 8 for RGBA16)
        let bpp: u64 = if info.bit_depth > 8 {
            if info.has_alpha { 8 } else { 6 }
        } else if info.has_alpha {
            4
        } else {
            3
        };
        let estimated_mem = info.width as u64 * info.height as u64 * bpp;
        self.limits
            .check_memory(estimated_mem)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;
        Ok(())
    }
}

impl<'a> zencodec::decode::DecodeJob<'a> for AvifDecodeJob {
    type Error = At<CodecError>;
    type Dec = AvifDecoder<'a>;
    type StreamDec = AvifStreamingDecoder;
    type AnimationFrameDec = AvifAnimationFrameDecoder;

    fn with_stop(mut self, stop: zencodec::StopToken) -> Self {
        self.stop = Some(stop);
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    fn with_start_frame_index(mut self, index: u32) -> Self {
        self.start_frame_index = index;
        self
    }

    fn with_policy(mut self, policy: zencodec::decode::DecodePolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// `ReconstructHdr` applies the gain map to the SDR base via
    /// ultrahdr-core (`DecodeCapabilities::reconstructs_hdr()` is `true`):
    /// the output is linear f32 RGBA (1.0 = SDR white / 203 nits) in the
    /// base image's primaries, MaxCLL/MaxFALL measured from the
    /// reconstructed pixels, and the gain-map components still surfaced
    /// for transcode use. Files without a gain map decode as honest SDR.
    /// Alpha-carrying or >8-bit bases are refused loudly — use
    /// [`GainMapRender::Components`] and apply downstream for those.
    fn with_gain_map_render(mut self, render: zencodec::GainMapRender) -> Self {
        self.gain_map_render = render;
        self
    }

    fn with_orientation(mut self, hint: zencodec::OrientationHint) -> Self {
        self.orientation = hint;
        self.config.orientation = hint;
        self
    }

    fn probe(&self, data: &[u8]) -> Result<ImageInfo, At<CodecError>> {
        self.probe_inner(data).map_err(zencodec::CodecError::of)
    }

    fn output_info(&self, data: &[u8]) -> Result<zencodec::decode::OutputInfo, At<CodecError>> {
        self.output_info_inner(data)
            .map_err(zencodec::CodecError::of)
    }

    fn push_decoder(
        self,
        data: Cow<'a, [u8]>,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
        preferred: &[PixelDescriptor],
    ) -> Result<zencodec::decode::OutputInfo, At<CodecError>> {
        // Bake path: orientation isn't row-local, so a true row-streamed sink
        // would emit pixels in stored orientation. Route through a full decode
        // (which bakes the buffer upright and reports display dims) and copy the
        // baked rows to the sink. `Preserve` (default) keeps the native, low-
        // memory streaming sink unchanged.
        //
        // `copy_decode_to_sink` is generic over `Self`, so it already returns
        // `Self::Error` (= `At<CodecError>`); the sink-error wrap bridges a bare
        // native `Error` into the envelope via `.into()`. The non-bake path keeps
        // its verbatim `At<Error>` body in `push_decoder_inner` and is re-wrapped
        // once at this boundary.
        //
        // `ReconstructHdr` takes the same route, for the same reason: applying
        // a gain map is whole-image (the map is sampled at an image-to-map
        // scale ratio), so it is not strip-local either. Routing it through
        // the buffered decode makes the row sink agree with the other two
        // paths *by construction* — including the refusal, since
        // `reconstruct_hdr_pixels` rejects 10/12-bit and alpha-carrying bases.
        //
        // zenavif#38: `push_decoder_inner` had no gain-map arm at all. It
        // neither reconstructed nor refused — it returned `Ok` with the SDR
        // base while buffered and streaming both returned `Unsupported`. An
        // error is recoverable; a wrong rendition reported as success is not
        // detectable by the caller at all.
        if will_auto_orient(self.orientation)
            || matches!(
                self.gain_map_render,
                zencodec::GainMapRender::ReconstructHdr { .. }
            )
        {
            return zencodec::helpers::copy_decode_to_sink(self, data, sink, preferred, |e| {
                Error::Io(e.to_string()).into()
            });
        }
        self.push_decoder_inner(data, sink, preferred)
            .map_err(zencodec::CodecError::of)
    }

    fn decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifDecoder<'a>, At<CodecError>> {
        self.decoder_inner(data, preferred)
            .map_err(zencodec::CodecError::of)
    }

    fn streaming_decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifStreamingDecoder, At<CodecError>> {
        self.streaming_decoder_inner(data, preferred)
            .map_err(zencodec::CodecError::of)
    }

    fn animation_frame_decoder(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifAnimationFrameDecoder, At<CodecError>> {
        self.animation_frame_decoder_inner(data, preferred)
            .map_err(zencodec::CodecError::of)
    }
}

impl AvifDecodeJob {
    fn probe_inner(&self, data: &[u8]) -> Result<ImageInfo, At<Error>> {
        let decoder = crate::ManagedAvifDecoder::new(data, &self.config.inner)?;
        let native_info = decoder.probe_info()?;
        // `convert_native_info` reports the Preserve view (stored dims +
        // intrinsic tag); rewrite to display dims + Identity on the bake path.
        let mut info = apply_reported_orientation(
            convert_native_info(&native_info),
            &native_info,
            self.orientation,
        );
        // Detect animation from the container's track structure.
        if let Some(anim) = decoder.animation_info() {
            info = info.with_sequence(ImageSequence::Animation {
                frame_count: Some(anim.frame_count as u32),
                loop_count: Some(anim.loop_count),
                random_access: true,
            });
        }
        if let Ok(probe) = crate::detect::probe(data) {
            info = info.with_source_encoding_details(probe);
        }
        if let Some(ref policy) = self.policy {
            apply_decode_policy(&mut info, policy);
        }
        Ok(info)
    }

    fn output_info_inner(&self, data: &[u8]) -> Result<zencodec::decode::OutputInfo, At<Error>> {
        let decoder = crate::ManagedAvifDecoder::new(data, &self.config.inner)?;
        let native_info = decoder.probe_info()?;
        let mut desc = if native_info.bit_depth > 8 {
            if native_info.has_alpha {
                PixelDescriptor::RGBA16_SRGB
            } else {
                PixelDescriptor::RGB16_SRGB
            }
        } else if native_info.has_alpha {
            PixelDescriptor::RGBA8_SRGB
        } else {
            PixelDescriptor::RGB8_SRGB
        };
        // Override TF and primaries from CICP if available.
        if let Some(tf) =
            zenpixels::TransferFunction::from_cicp(native_info.transfer_characteristics.0)
        {
            desc = desc.with_transfer(tf);
        }
        if let Some(p) = zenpixels::ColorPrimaries::from_cicp(native_info.color_primaries.0) {
            desc = desc.with_primaries(p);
        }
        // Report the post-orientation output dims + what the decoder bakes:
        // `Correct` bakes the intrinsic orientation (output = display dims,
        // `orientation_applied` = intrinsic); `Preserve` bakes nothing
        // (output = stored dims, `orientation_applied` = Identity, caller orients).
        let (w, h, _) = reported_dims_and_orientation(&native_info, self.orientation);
        let orientation_applied = if will_auto_orient(self.orientation) {
            intrinsic_orientation(&native_info)
        } else {
            zencodec::Orientation::Identity
        };
        Ok(zencodec::decode::OutputInfo::full_decode(w, h, desc)
            .with_orientation_applied(orientation_applied))
    }

    fn decoder_inner<'a>(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifDecoder<'a>, At<Error>> {
        self.check_input_size(&data)?;
        let cfg = self.effective_config();
        Ok(AvifDecoder {
            config: cfg,
            stop: self.stop,
            data,
            preferred: preferred.to_vec(),
            limits: self.limits,
            policy: self.policy,
            extract_gain_map: self.extract_gain_map,
            gain_map_render: self.gain_map_render,
            orientation: self.orientation,
        })
    }

    fn streaming_decoder_inner<'a>(
        mut self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifStreamingDecoder, At<Error>> {
        self.check_input_size(&data)?;
        let cfg = self.effective_config();
        let stop_token = self
            .stop
            .take()
            .unwrap_or_else(|| zencodec::StopToken::new(enough::Unstoppable));

        let mut decoder = crate::ManagedAvifDecoder::new(&data, &cfg)?;
        let native_info = decoder.probe_info()?;
        self.check_decode_limits(&native_info)?;

        // Native grayscale opt-in (zenavif#5) — mirrors the buffered
        // decode: alpha-free monochrome, not reconstructing, not a grid
        // (the grid branch below stitches RGB tiles), gray negotiated.
        // The monochrome strip path runs through full conversion +
        // `StripConverter::new_from_pixels`, which carries the gray
        // descriptor into the emitted strips.
        let mono_source = native_info.monochrome && !native_info.has_alpha;
        let reconstructing = matches!(
            self.gain_map_render,
            zencodec::GainMapRender::ReconstructHdr { .. }
        ) && native_info.gain_map.is_some();
        if mono_source
            && !reconstructing
            && !decoder.is_grid()
            && icc_allows_native_gray(&native_info)
            && wants_gray_output(preferred)
        {
            decoder.set_native_gray(true);
        }

        // ReconstructHdr path: gain-map application is whole-image (the
        // map is sampled at an image-to-map scale ratio), so decode the
        // full buffer, reconstruct, then emit fixed-height strips —
        // same shape as the orientation-bake path below. Files without
        // a gain map fall through to honest SDR streaming.
        if let zencodec::GainMapRender::ReconstructHdr { target_headroom } = self.gain_map_render
            && native_info.gain_map.is_some()
        {
            let (pixels, native_info) = decoder.decode_full(&stop_token)?;
            let pixels = set_cicp_on_pixels(pixels, &native_info);
            let pixels = attach_source_color_context(pixels, &native_info);
            let (hdr, (max_cll, max_fall)) = reconstruct_hdr_pixels(
                pixels,
                &native_info,
                target_headroom,
                self.config.inner(),
                &stop_token,
            )?;
            let (baked, _orientation, w, h) = bake_orientation(hdr, &native_info, self.orientation);
            let strip_descriptor = baked.descriptor();
            let mut info = apply_reported_orientation(
                convert_native_info(&native_info),
                &native_info,
                self.orientation,
            );
            // Measured envelope of the reconstructed pixels (zencodec
            // contract: MaxCLL/MaxFALL are measured; mastering display
            // passes through unchanged).
            info =
                info.with_content_light_level(zencodec::ContentLightLevel::new(max_cll, max_fall));
            let strip_height = 64u32.min(h.max(1));
            return Ok(AvifStreamingDecoder {
                info,
                y_offset: 0,
                output_width: w,
                output_height: h,
                decoder: None,
                stop: stop_token,
                grid_rows: 0,
                grid_cols: 0,
                current_grid_row: 0,
                strip_descriptor,
                strip_buffer: None,
                strip_converter: None,
                strip_height,
                strip_color_context: baked.color_context().cloned(),
                baked: Some(baked),
                // The bake paths materialise the whole image and have already
                // run `negotiate_format` (HDR reconstruct) or produce a
                // rendition negotiation does not apply to; their strips are
                // slices of a finished buffer.
                output_descriptor: None,
                output_buffer: None,
            });
        }

        // Bake path: orientation is not strip-local (transposes need the whole
        // image), so decode + bake the full buffer once and emit it in
        // fixed-height strips. Mirrors `Decode::decode`'s buffer pipeline. The
        // preserve path (default) falls through to the low-memory grid / strip-
        // converter streaming below, unchanged.
        if will_auto_orient(self.orientation) {
            let (pixels, native_info) = decoder.decode_full(&stop_token)?;
            let pixels = set_cicp_on_pixels(pixels, &native_info);
            let pixels = attach_source_color_context(pixels, &native_info);
            let pixels = negotiate_format(
                pixels,
                preferred,
                native_info.monochrome && !native_info.has_alpha,
            );
            let (baked, _orientation, w, h) =
                bake_orientation(pixels, &native_info, self.orientation);
            let strip_descriptor = baked.descriptor();
            let info = apply_reported_orientation(
                convert_native_info(&native_info),
                &native_info,
                self.orientation,
            );
            // Emit in cache-friendly fixed-height strips (or the whole image if
            // it's short). The baked buffer is contiguous, so a strip is just a
            // row-range view re-copied into a small buffer.
            let strip_height = 64u32.min(h.max(1));
            return Ok(AvifStreamingDecoder {
                info,
                y_offset: 0,
                output_width: w,
                output_height: h,
                decoder: None,
                stop: stop_token,
                grid_rows: 0,
                grid_cols: 0,
                current_grid_row: 0,
                strip_descriptor,
                strip_buffer: None,
                strip_converter: None,
                strip_height,
                strip_color_context: baked.color_context().cloned(),
                baked: Some(baked),
                // The bake paths materialise the whole image and have already
                // run `negotiate_format` (HDR reconstruct) or produce a
                // rendition negotiation does not apply to; their strips are
                // slices of a finished buffer.
                output_descriptor: None,
                output_buffer: None,
            });
        }

        let info = convert_native_info(&native_info);

        if decoder.is_grid() {
            let grid = decoder.grid_config().ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "grid_config missing after is_grid()",
                })
            })?;
            let output_width = grid.output_width;
            let output_height = grid.output_height;

            let base_desc = if native_info.bit_depth > 8 {
                if native_info.has_alpha {
                    PixelDescriptor::RGBA16_SRGB
                } else {
                    PixelDescriptor::RGB16_SRGB
                }
            } else if native_info.has_alpha {
                PixelDescriptor::RGBA8_SRGB
            } else {
                PixelDescriptor::RGB8_SRGB
            };

            // Apply CICP metadata to descriptor. No format negotiation for
            // the grid path — tiles produce native format and we stitch raw bytes.
            let mut strip_descriptor = base_desc;
            if let Some(tf) =
                zenpixels::TransferFunction::from_cicp(native_info.transfer_characteristics.0)
            {
                strip_descriptor = strip_descriptor.with_transfer(tf);
            }
            if let Some(p) = zenpixels::ColorPrimaries::from_cicp(native_info.color_primaries.0) {
                strip_descriptor = strip_descriptor.with_primaries(p);
            }

            return Ok(AvifStreamingDecoder {
                info,
                y_offset: 0,
                output_width,
                output_height,
                decoder: Some(decoder),
                stop: stop_token,
                grid_rows: grid.rows as u32,
                grid_cols: grid.columns as u32,
                current_grid_row: 0,
                strip_descriptor,
                strip_buffer: None,
                strip_converter: None,
                strip_height: 0,
                strip_color_context: color_context_for_layout(
                    &native_source_color(&native_info),
                    strip_descriptor.layout(),
                ),
                baked: None,
                // The grid path stitches native-format tiles; the reduction
                // `preferred` asked for is applied to each stitched strip
                // (zenavif#36 — it used to be dropped entirely here).
                output_descriptor: negotiate_strip_descriptor(strip_descriptor, preferred),
                output_buffer: None,
            });
        }

        // Non-grid: decode YUV, set up strip converter for on-demand conversion.
        // Use the frame-era info the converter returns (not the probe-era
        // one): the buffered path attaches its context from decode_full's
        // info, and strips must describe pixels identically.
        let (converter, frame_native) = decoder.decode_to_strip_converter(&stop_token)?;
        // Already carries the container's CICP: `decode_to_strip_converter`
        // stamps the descriptor it mints (zenavif#37). Re-deriving it here
        // from probe-era info would be wrong — the comment below on
        // `frame_native` is the same trap.
        let desc = converter.descriptor();
        let w = converter.display_width() as u32;
        let h = converter.display_height() as u32;
        let strip_h = converter.optimal_strip_height() as u32;

        Ok(AvifStreamingDecoder {
            info,
            y_offset: 0,
            output_width: w,
            output_height: h,
            decoder: None,
            stop: stop_token,
            grid_rows: 0,
            grid_cols: 0,
            current_grid_row: 0,
            strip_descriptor: desc,
            strip_buffer: None,
            strip_converter: Some(converter),
            strip_height: strip_h,
            strip_color_context: color_context_for_layout(
                &native_source_color(&frame_native),
                desc.layout(),
            ),
            baked: None,
            output_descriptor: negotiate_strip_descriptor(desc, preferred),
            output_buffer: None,
        })
    }

    fn animation_frame_decoder_inner<'a>(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<AvifAnimationFrameDecoder, At<Error>> {
        // Reject animation when policy disallows it.
        if let Some(ref policy) = self.policy
            && !policy.resolve_animation(true)
        {
            return Err(at!(Error::UnsupportedOperation(
                zencodec::UnsupportedOperation::AnimationDecode,
            )));
        }
        self.check_input_size(&data)?;
        let cfg = self.effective_config();

        // Probe metadata before creating animation decoder (both parse the container,
        // but ManagedAvifDecoder gives us the native ImageInfo for conversion).
        let probe_dec = crate::ManagedAvifDecoder::new(&data, &cfg)?;
        let native_info = probe_dec.probe_info()?;
        self.check_decode_limits(&native_info)?;
        drop(probe_dec);

        let anim_dec = crate::AnimationDecoder::new(&data, &cfg)?;
        let anim_info = anim_dec.info().clone();

        // `convert_native_info` reports the Preserve view (stored dims +
        // intrinsic tag); the bake path rewrites the canvas to display dims +
        // Identity, matching the per-frame buffers `render_next_frame` bakes.
        let mut base_info = apply_reported_orientation(
            convert_native_info(&native_info),
            &native_info,
            self.orientation,
        )
        .with_sequence(ImageSequence::Animation {
            frame_count: Some(anim_info.frame_count as u32),
            loop_count: Some(anim_info.loop_count),
            random_access: true,
        });
        // Attach source encoding details to the shared animation ImageInfo.
        if let Ok(probe) = crate::detect::probe(&data) {
            base_info = base_info.with_source_encoding_details(probe);
        }
        if let Some(ref policy) = self.policy {
            apply_decode_policy(&mut base_info, policy);
        }

        // Resolve the orientation to bake into each frame: the intrinsic
        // transform on the bake path, `Identity` (no-op) on the preserve path.
        let bake_to = if will_auto_orient(self.orientation) {
            intrinsic_orientation(&native_info)
        } else {
            zencodec::Orientation::Identity
        };

        Ok(AvifAnimationFrameDecoder {
            anim_decoder: anim_dec,
            index: 0,
            frames_decoded: 0,
            start_frame_index: self.start_frame_index,
            info: Arc::new(base_info),
            total_frames: anim_info.frame_count as u32,
            loop_count: anim_info.loop_count,
            preferred: preferred.to_vec(),
            current_frame: None,
            limits: self.limits,
            accumulated_ms: 0,
            bake_to,
        })
    }

    fn push_decoder_inner(
        self,
        data: Cow<'_, [u8]>,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
        preferred: &[PixelDescriptor],
    ) -> Result<zencodec::decode::OutputInfo, At<Error>> {
        self.check_input_size(&data)?;
        let cfg = self.effective_config();
        let stop: &dyn Stop = match &self.stop {
            Some(s) => s,
            None => &enough::Unstoppable,
        };
        let mut decoder = crate::ManagedAvifDecoder::new(&data, &cfg)?;
        let probe_info = decoder.probe_info()?;
        self.check_decode_limits(&probe_info)?;

        // Native grayscale opt-in (zenavif#5, gap closed in zenavif#35) —
        // the SAME gate as the buffered (`AvifDecoder::decode`) and streaming
        // (`streaming_decoder_inner`) paths, so all three honour a Gray
        // preference identically: alpha-free monochrome, not reconstructing,
        // not a grid (the grid sink stitches RGB tiles), ICC-compatible.
        // Without this the row sink dropped `preferred` on the floor and
        // always expanded mono to RGB, at 3x the bytes and a descriptor no
        // other path emitted.
        let mono_source = probe_info.monochrome && !probe_info.has_alpha;
        let reconstructing = matches!(
            self.gain_map_render,
            zencodec::GainMapRender::ReconstructHdr { .. }
        ) && probe_info.gain_map.is_some();
        if mono_source
            && !reconstructing
            && !decoder.is_grid()
            && icc_allows_native_gray(&probe_info)
            && wants_gray_output(preferred)
        {
            decoder.set_native_gray(true);
        }

        // Report the descriptor the sink was ACTUALLY handed rather than
        // re-deriving one from bit depth + alpha. The re-derived guess was the
        // second half of zenavif#35: it could not describe a gray output, and
        // any future format decision inside `decode_to_sink` would silently
        // desync `OutputInfo` from the pixels again. Recording makes the two
        // incapable of disagreeing.
        //
        // The same wrapper now also carries the CICP stamp (#37) and the
        // `preferred` reduction (#36) — deliberately in one place, because
        // the descriptor it announces to the caller's sink and the descriptor
        // it reports in `OutputInfo` are then literally the same value.
        let mut recorder = NegotiatingSink {
            inner: sink,
            preferred,
            resolved: None,
            scratch: None,
            pending: None,
        };
        let native_info = decoder.decode_to_sink(stop, &mut recorder)?;
        let recorded = recorder
            .resolved
            .map(|(native, target)| target.unwrap_or(native));

        // Fallback only for a decode that emitted no strips at all (so the
        // sink was never told a format): keep the historical derivation.
        let desc = recorded.unwrap_or(if native_info.bit_depth > 8 {
            if native_info.has_alpha {
                PixelDescriptor::RGBA16_SRGB
            } else {
                PixelDescriptor::RGB16_SRGB
            }
        } else if native_info.has_alpha {
            PixelDescriptor::RGBA8_SRGB
        } else {
            PixelDescriptor::RGB8_SRGB
        });
        Ok(zencodec::decode::OutputInfo::full_decode(
            native_info.width,
            native_info.height,
            desc,
        ))
    }
}

/// Wrapping [`DecodeRowSink`](zencodec::decode::DecodeRowSink) that makes the
/// row-sink path describe and negotiate its output like the other two.
///
/// It does three things the bare sink could not, all of which have to happen
/// together or the descriptor and the pixels drift apart:
///
/// * **records** the format actually handed downstream, so `push_decoder`
///   reports it instead of re-deriving a guess (zenavif#35);
/// * **stamps** the container's CICP onto that format, so PQ pixels are not
///   handed over labelled `transfer: Unknown` (zenavif#37);
/// * **applies** the `preferred` reduction per strip, so a caller asking for
///   `Rgb8` is not silently handed `Rgb16` or `Rgba8` (zenavif#36).
///
/// When no reduction is negotiated the codec still writes *directly* into the
/// caller's buffer — the stamp changes only the description, never the bytes
/// per pixel — so the low-memory, zero-copy property of the sink path is
/// preserved for the common case. A strip is only staged through `scratch`
/// when a real conversion was asked for and is available.
struct NegotiatingSink<'s> {
    inner: &'s mut dyn zencodec::decode::DecodeRowSink,
    preferred: &'s [PixelDescriptor],
    /// `(stamped native, negotiated target)`, decided from the first
    /// descriptor the codec announces. `None` until then.
    resolved: Option<(PixelDescriptor, Option<PixelDescriptor>)>,
    /// Native-format staging buffer, only allocated on the converting path.
    scratch: Option<zenpixels::PixelBuffer>,
    /// Geometry of the strip currently staged in `scratch`, awaiting
    /// conversion into the caller's sink.
    pending: Option<(u32, u32, u32)>,
}

impl NegotiatingSink<'_> {
    /// Decide the output format once, from the codec's native descriptor
    /// (which already carries the container's CICP — `decode_to_sink` stamps
    /// every descriptor it announces).
    fn resolve(&mut self, native: PixelDescriptor) -> (PixelDescriptor, Option<PixelDescriptor>) {
        if let Some(r) = self.resolved {
            return r;
        }
        let target = negotiate_strip_descriptor(native, self.preferred);
        let r = (native, target);
        self.resolved = Some(r);
        r
    }

    /// The format the caller's sink sees.
    fn out_descriptor(&mut self, native: PixelDescriptor) -> PixelDescriptor {
        let (stamped, target) = self.resolve(native);
        target.unwrap_or(stamped)
    }

    /// Convert the staged strip and hand it to the caller's sink.
    fn flush_pending(&mut self) -> Result<(), zencodec::decode::SinkError> {
        let Some((y, h, w)) = self.pending.take() else {
            return Ok(());
        };
        let Some(staged) = self.scratch.take() else {
            return Ok(());
        };
        let Some((_stamped, Some(target))) = self.resolved else {
            return Ok(());
        };
        let converted = apply_strip_reduction(staged, target).ok_or_else(|| {
            zencodec::decode::SinkError::from(
                "negotiated strip conversion failed after its plan was probed",
            )
        })?;
        let mut dst = self.inner.provide_next_buffer(y, h, w, target)?;
        let src = converted.as_slice();
        let row_bytes = w as usize * target.bytes_per_pixel();
        for row in 0..h {
            dst.row_mut(row)[..row_bytes].copy_from_slice(&src.row(row)[..row_bytes]);
        }
        Ok(())
    }
}

impl zencodec::decode::DecodeRowSink for NegotiatingSink<'_> {
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Result<(), zencodec::decode::SinkError> {
        let out = self.out_descriptor(descriptor);
        self.inner.begin(width, height, out)
    }

    fn provide_next_buffer(
        &mut self,
        y: u32,
        height: u32,
        width: u32,
        descriptor: PixelDescriptor,
    ) -> Result<zenpixels::PixelSliceMut<'_>, zencodec::decode::SinkError> {
        // `begin` is skipped when a decode produces no strips, and the grid
        // sink calls it lazily; resolve from whichever arrives first.
        let (stamped, target) = self.resolve(descriptor);
        self.flush_pending()?;
        match target {
            // No reduction: the codec writes straight into the caller's
            // buffer, exactly as before — only the announced descriptor
            // gained its CICP.
            None => self.inner.provide_next_buffer(y, height, width, stamped),
            // Reduction: stage the strip in the native format, convert on the
            // next call (or at `finish`).
            Some(_) => {
                self.pending = Some((y, height, width));
                self.scratch = Some(zenpixels::PixelBuffer::new(width, height, stamped));
                Ok(self
                    .scratch
                    .as_mut()
                    .expect("scratch was just set")
                    .as_slice_mut()
                    .erase())
            }
        }
    }

    fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
        self.flush_pending()?;
        self.inner.finish()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "encode")]
    use super::*;
    #[cfg(feature = "encode")]
    use crate::codec::encode_config::AvifEncoderConfig;
    #[cfg(feature = "encode")]
    use rgb::Rgb;

    #[cfg(feature = "encode")]
    #[test]
    fn four_layer_decode_flow() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200
            };
            8 * 8
        ];
        let img = imgref::ImgVec::new(pixels, 8, 8);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        let config = AvifDecoderConfig::new();
        let decoded = config
            .job()
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(decoded.width(), 8);
        assert_eq!(decoded.height(), 8);
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_max_input_bytes_rejects() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        // First encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();
        assert!(encoded.data().len() > 100);

        // Decode with max_input_bytes=100 should fail
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_input_bytes(100);
        let result = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .and_then(|dec| dec.decode());
        assert!(
            result.is_err(),
            "decode should fail with max_input_bytes=100"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_max_width_rejects() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        // Encode a 32x32 image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // Decode with max_width=10 should fail for a 32-wide image
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none()
            .with_max_width(10)
            .with_max_height(10);
        let result = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .and_then(|dec| dec.decode());
        assert!(
            result.is_err(),
            "decode should fail with max_width=10 for 32px image"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_max_memory_rejects() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        // Encode a 32x32 image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // 100 bytes of memory is not enough to decode a 32x32 image
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_memory(100);
        let result = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[])
            .and_then(|dec| dec.decode());
        assert!(
            result.is_err(),
            "decode should fail with max_memory_bytes=100"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_push_decoder_checks_input_size() {
        use zencodec::decode::{DecodeJob, DecodeRowSink, DecoderConfig, SinkError};
        use zenpixels::PixelSliceMut;

        struct DiscardSink {
            buf: Vec<u8>,
        }
        impl DecodeRowSink for DiscardSink {
            fn provide_next_buffer(
                &mut self,
                _y: u32,
                height: u32,
                width: u32,
                descriptor: PixelDescriptor,
            ) -> Result<PixelSliceMut<'_>, SinkError> {
                let bpp = descriptor.bytes_per_pixel();
                let stride = width as usize * bpp;
                let needed = height as usize * stride;
                self.buf.resize(needed, 0);
                Ok(
                    PixelSliceMut::new(&mut self.buf, width, height, stride, descriptor)
                        .expect("buffer sized correctly"),
                )
            }
        }

        // Encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // push_decoder with max_input_bytes=100 should fail
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_input_bytes(100);
        let mut sink = DiscardSink { buf: Vec::new() };
        let result = config.job().with_limits(limits).push_decoder(
            Cow::Borrowed(encoded.data()),
            &mut sink,
            &[],
        );
        assert!(
            result.is_err(),
            "push_decoder should fail with max_input_bytes=100"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_streaming_checks_input_size() {
        use zencodec::decode::{DecodeJob, DecoderConfig};

        // Encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // streaming_decoder with max_input_bytes=100 should fail
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_input_bytes(100);
        let result = config
            .job()
            .with_limits(limits)
            .streaming_decoder(Cow::Borrowed(encoded.data()), &[]);
        assert!(
            result.is_err(),
            "streaming_decoder should fail with max_input_bytes=100"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_memory_limit_produces_resource_limit_error() {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

        // Encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // Set max_memory to 1 byte — decode should fail with ResourceLimit, not Encode
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_memory(1);
        let decoder = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[]);
        // The limit may be checked at decoder() or decode() stage
        let result = match decoder {
            Err(e) => Err(e),
            Ok(dec) => dec.decode().map(|_| ()),
        };
        assert!(result.is_err(), "expected error from memory limit");
        let err = result.err().unwrap();
        // Pattern B (envelope): the coarse category is read off `At<CodecError>`
        // directly (native `Error::ResourceLimit` maps to `Resource(Limits(Memory))`),
        // and the native detail stays reachable via downcast — faithful to the
        // original `matches!(.., Error::ResourceLimit(_))` intent.
        assert_eq!(
            err.error().category(),
            zencodec::ErrorCategory::Resource(zencodec::ResourceError::Limits(
                zencodec::LimitKind::Memory
            )),
            "expected a memory Resource(Limits) category, got: {err}"
        );
        assert!(
            matches!(
                err.error().detail().and_then(|d| d.downcast_ref::<Error>()),
                Some(Error::ResourceLimit(_))
            ),
            "envelope must carry the native Error::ResourceLimit detail, got: {err}"
        );
    }

    #[cfg(feature = "encode")]
    #[test]
    fn decode_input_size_limit_produces_resource_limit_error() {
        use zencodec::decode::{DecodeJob, DecoderConfig};

        // Encode a valid image
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 100,
                g: 150,
                b: 200,
            };
            32 * 32
        ];
        let img = imgref::ImgVec::new(pixels, 32, 32);
        let encoded = AvifEncoderConfig::new()
            .with_quality(80.0)
            .encode_rgb8(img.as_ref())
            .unwrap();

        // Set max_input_bytes to 1 — decode should fail with ResourceLimit, not Encode
        let config = AvifDecoderConfig::new();
        let limits = ResourceLimits::none().with_max_input_bytes(1);
        let result = config
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(encoded.data()), &[]);
        assert!(result.is_err(), "expected error from memory limit");
        let err = result.err().unwrap();
        // Pattern B (envelope): the coarse category is read off `At<CodecError>`
        // directly (native `Error::ResourceLimit` maps to `Resource(Limits(Memory))`),
        // and the native detail stays reachable via downcast — faithful to the
        // original `matches!(.., Error::ResourceLimit(_))` intent.
        assert_eq!(
            err.error().category(),
            zencodec::ErrorCategory::Resource(zencodec::ResourceError::Limits(
                zencodec::LimitKind::Memory
            )),
            "expected a memory Resource(Limits) category, got: {err}"
        );
        assert!(
            matches!(
                err.error().detail().and_then(|d| d.downcast_ref::<Error>()),
                Some(Error::ResourceLimit(_))
            ),
            "envelope must carry the native Error::ResourceLimit detail, got: {err}"
        );
    }
}

//! The decoder handle itself: construction, the rav1d frame-decode
//! plumbing, and the whole-image entry points.
//!
//! `decode_frame` is the single place the caller's cancellation token is
//! installed on (and removed from) the rav1d-safe decoder; every other
//! module reaches AV1 frames through it.

use crate::config::DecoderConfig;
use crate::error::{Error, Result, error_from_rav1d};
use crate::image::ImageInfo;
use enough::Stop;
use rav1d_safe::src::managed::{Decoder as Rav1dDecoder, Frame, Settings};
use whereat::at;
use zenpixels::PixelBuffer;

/// Managed decoder wrapper - 100% safe!
pub struct ManagedAvifDecoder {
    pub(super) decoder: Rav1dDecoder,
    // TODO(whereat): every `self.parser.<m>(...)` (and the `AvifParser::from_owned_with_config`
    // constructor) is currently consumed via `.map_err(|e| at!(Error::from(e)))?`. That is
    // correct today: zenavif-parse (0.6.x) returns a *bare* `Error`, so `at!` is what starts
    // the trace. When zenavif-parse begins returning `At<Error>` (the planned >=0.6.3
    // trace-carrying release), switch those boundaries to `.map_err_at(Error::from)?` so the
    // parser-side trace is preserved instead of discarded and re-started here. This mirrors the
    // way ravif consumes the same parser. Sites, by sibling module:
    //   from_owned_with_config + primary_data + alpha_data (decoder, sink, aom),
    //   primary_metadata (metadata), frame (animation), tile_data (grid, sink, aom).
    pub(super) parser: zenavif_parse::AvifParser<'static>,
    pub(super) prefer_8bit: bool,
    /// When true, alpha-free monochrome images decode to native Gray8 /
    /// Gray16 buffers (1 channel) instead of expanding to RGB. Off by
    /// default — opted in by the zencodec adapter's format negotiation
    /// (imazen/zenavif#5).
    pub(super) native_gray: bool,
    /// Allocation-fallibility preference for zenavif's own decode buffers.
    /// Threaded from [`DecoderConfig::alloc_pref`](crate::DecoderConfig) so the
    /// big untrusted-sized output / grid / crop buffers and the per-row scratch
    /// honor `Fallible` / `Infallible` overrides. `CodecDefault` keeps each
    /// site's own default.
    pub(super) alloc_pref: crate::alloc_util::AllocPref,
    /// Which AV1 kernel serves item decodes ([`DecoderConfig::decode_backend`]).
    /// Full decode caps (limits/stop/alloc) are re-threaded per item decode.
    /// (Only read by the `aom-backend` routing; the rav1d path bakes its caps
    /// into `Settings` at construction.)
    #[cfg_attr(not(feature = "aom-backend"), allow(dead_code))]
    pub(super) decode_backend: crate::DecodeBackend,
    /// Retained caps for the aom-backed item decodes.
    #[cfg_attr(not(feature = "aom-backend"), allow(dead_code))]
    pub(super) frame_size_limit: u32,
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
        .map_err(|e| e.map_error(Error::Parse))?;

        let mut settings = Settings::default();
        settings.threads = config.threads;
        settings.apply_grain = config.apply_grain;
        settings.frame_size_limit = config.frame_size_limit;

        let decoder = Rav1dDecoder::with_settings(settings).map_err(|e| {
            e.map_error(|re| error_from_rav1d(re, "Failed to create decoder"))
                .at()
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

        Ok(Self {
            decoder,
            parser,
            prefer_8bit: config.prefer_8bit,
            native_gray: false,
            alloc_pref: config.alloc_pref,
            decode_backend: config.decode_backend,
            frame_size_limit: config.frame_size_limit,
        })
    }

    /// Decode a single AV1 frame, handling progressive/multi-layer streams transparently.
    ///
    /// If the decoder buffers data internally (returns `Ok(None)`), flushes to retrieve
    /// the composed frame. Always flushes afterward to reset state, so sequential calls
    /// (e.g. primary then alpha) work without the caller needing to manage decoder state.
    ///
    /// Takes `decoder` explicitly to avoid borrowing `self` (which would conflict
    /// with borrows of `self.parser` for data access).
    ///
    /// This is the one place that installs the caller's cancellation token on
    /// the rav1d-safe decoder, so every caller gets in-flight cancellation from
    /// passing `stop` — see [`crate::cancel`] for why a borrowed token needs
    /// relaying and what it costs (nothing, for an `Unstoppable` caller). The
    /// token is uninstalled again before returning: the relay stops tracking
    /// the caller the moment this call ends, so leaving it attached would give
    /// the decoder a token frozen at whatever it last read.
    pub(super) fn decode_frame(
        decoder: &mut Rav1dDecoder,
        data: &[u8],
        context: &'static str,
        stop: &(impl Stop + ?Sized),
    ) -> Result<Frame> {
        crate::cancel::with_relayed_stop(stop, |token| {
            let relayed = token.is_some();
            if relayed {
                decoder.set_stop(token);
            }
            let out = Self::decode_frame_inner(decoder, data, context);
            if relayed {
                decoder.set_stop(None);
            }
            out
        })
    }

    /// The decode itself, with no cancellation plumbing — see [`Self::decode_frame`].
    fn decode_frame_inner(
        decoder: &mut Rav1dDecoder,
        data: &[u8],
        context: &'static str,
    ) -> Result<Frame> {
        // Send data and try to get a frame immediately
        let frame = match decoder.decode(data) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                // Progressive/multi-layer: flush to get the composed frame
                let frames = decoder.flush().map_err(|e| {
                    e.map_error(|re| error_from_rav1d(re, "Failed to flush decoder"))
                        .at()
                })?;
                frames.into_iter().last().ok_or_else(|| {
                    at!(Error::Decode {
                        code: -1,
                        msg: context,
                    })
                })?
            }
            Err(e) => {
                return Err(e.map_error(|re| error_from_rav1d(re, context)).at());
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

        #[cfg(feature = "aom-backend")]
        if self.decode_backend == crate::DecodeBackend::AomRs {
            return self.decode_full_aom(stop).map(|(pixels, _info)| pixels);
        }

        // Check if this is a grid image (tiled/multi-frame)
        if self.parser.grid_config().is_some() {
            return self.decode_grid(stop);
        }

        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| e.map_error(Error::Parse))?;
        let primary_frame = Self::decode_frame(
            &mut self.decoder,
            &primary_data,
            "Failed to decode primary frame",
            stop,
        )?;

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let alpha_frame = if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| e.map_error(Error::Parse))?;
            Some(Self::decode_frame(
                &mut self.decoder,
                &alpha_data,
                "Failed to decode alpha frame",
                stop,
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

        #[cfg(feature = "aom-backend")]
        if self.decode_backend == crate::DecodeBackend::AomRs {
            return self.decode_full_aom(stop);
        }

        if self.parser.grid_config().is_some() {
            let pixels = self.decode_grid(stop)?;
            let info = self.probe_info()?;
            return Ok((pixels, info));
        }

        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| e.map_error(Error::Parse))?;
        let primary_frame = Self::decode_frame(
            &mut self.decoder,
            &primary_data,
            "Failed to decode primary frame",
            stop,
        )?;

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let alpha_frame = if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| e.map_error(Error::Parse))?;
            Some(Self::decode_frame(
                &mut self.decoder,
                &alpha_data,
                "Failed to decode alpha frame",
                stop,
            )?)
        } else {
            None
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        self.convert_to_image(primary_frame, alpha_frame, stop)
    }
}

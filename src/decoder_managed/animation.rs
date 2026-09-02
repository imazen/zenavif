//! Animated AVIF decoding.
//!
//! Two entry points: [`ManagedAvifDecoder::decode_animation`] eagerly decodes
//! every frame, while [`AnimationDecoder`] yields them one at a time. Both
//! share `decode_anim_frame`, the non-flushing single-frame decode that keeps
//! the reference frames inter prediction needs.

use super::ManagedAvifDecoder;
use crate::config::DecoderConfig;
use crate::error::{Error, Result, error_from_rav1d};
use crate::image::{DecodedAnimation, DecodedAnimationInfo, DecodedFrame};
use enough::Stop;
use rav1d_safe::src::managed::{Decoder as Rav1dDecoder, Frame, Settings};
use whereat::at;

impl ManagedAvifDecoder {
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
            .ok_or_else(|| at!(Error::InvalidParameters("not an animated AVIF".into())))?;

        let mut alpha_decoder = if anim_info.has_alpha {
            let mut settings = Settings::default();
            settings.threads = 0;
            Some(Rav1dDecoder::with_settings(settings).map_err(|e| {
                e.map_error(|re| error_from_rav1d(re, "Failed to create alpha decoder"))
                    .at()
            })?)
        } else {
            None
        };

        let frame_count = anim_info.frame_count;
        // The frame count comes from the (untrusted) container sample
        // tables; reserve fallibly so an absurd declared count degrades to
        // a graceful error instead of an allocator abort (zenavif#21).
        let mut frames = crate::alloc_util::vec_with_capacity(self.alloc_pref, true, frame_count)?;

        for i in 0..frame_count {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

            let frame_ref = self
                .parser
                .frame(i)
                .map_err(|e| e.map_error(Error::Parse))?;

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
            Err(e) => {
                return Err(e.map_error(|re| error_from_rav1d(re, context)).at());
            }
        }

        // Frame not returned immediately — drain via get_frame. Capture the
        // last real error (instead of discarding it, per `error_from_rav1d`)
        // so a genuine rav1d-safe fault classifies correctly; exhausting all
        // 10,000 polls without an error or a frame is its own (rare, internal)
        // condition, distinct from any classified cause.
        let mut last_err = None;
        for _ in 0..10_000 {
            match decoder.get_frame() {
                Ok(Some(frame)) => return Ok(frame),
                Ok(None) => std::thread::yield_now(),
                Err(e) => {
                    last_err = Some(e);
                    break;
                }
            }
        }

        Err(match last_err {
            Some(e) => e.map_error(|re| error_from_rav1d(re, context)).at(),
            None => at!(Error::Decode {
                code: -1,
                msg: context,
            }),
        })
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
/// One eagerly decoded aom animation frame: (color, optional alpha),
/// consumed exactly once in display order.
#[cfg(feature = "zenav1-aom")]
type AomAnimFrame = (
    aom_decode::frame::FrameDecode,
    Option<aom_decode::frame::FrameDecode>,
);

pub struct AnimationDecoder {
    /// Underlying decoder (owns parser + color decoder)
    inner: ManagedAvifDecoder,
    /// Separate decoder for the alpha track (inter-prediction needs its own state)
    alpha_decoder: Option<Rav1dDecoder>,
    /// Animation metadata
    info: DecodedAnimationInfo,
    /// Index of the next frame to decode
    frame_index: usize,
    /// Eagerly decoded frames for the aom backend (`decode_frames` needs the
    /// whole temporal-unit stream for DPB/CDF state; per-sample incremental
    /// decode is a future upstream API). `None` on the rav1d path.
    #[cfg(feature = "zenav1-aom")]
    aom_frames: Option<Vec<Option<AomAnimFrame>>>,
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
            .ok_or_else(|| at!(Error::InvalidParameters("not an animated AVIF".into())))?;

        let alpha_decoder = if anim_info.has_alpha {
            let mut settings = Settings::default();
            settings.threads = config.threads;
            Some(Rav1dDecoder::with_settings(settings).map_err(|e| {
                e.map_error(|re| error_from_rav1d(re, "Failed to create alpha decoder"))
                    .at()
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

        #[cfg(feature = "zenav1-aom")]
        let aom_frames = if config.decode_backend == crate::DecodeBackend::Zenav1Aom {
            Some(Self::decode_all_frames_aom(&inner, &info)?)
        } else {
            None
        };

        Ok(Self {
            inner,
            alpha_decoder,
            info,
            frame_index: 0,
            #[cfg(feature = "zenav1-aom")]
            aom_frames,
        })
    }

    /// Eagerly decode the whole animation through zenav1-aom: the color (and
    /// alpha) tracks' temporal units are concatenated per track and decoded
    /// in one `decode_frames` pass, because inter frames carry DPB/CDF state
    /// across samples. Memory is `frame_count` decoded frames up front —
    /// bounded by the parser's animation caps; the rav1d path remains the
    /// streaming choice.
    #[cfg(feature = "zenav1-aom")]
    fn decode_all_frames_aom(
        inner: &ManagedAvifDecoder,
        info: &DecodedAnimationInfo,
    ) -> Result<Vec<Option<AomAnimFrame>>> {
        let config = crate::DecoderConfig {
            frame_size_limit: inner.frame_size_limit,
            alloc_pref: inner.alloc_pref,
            ..crate::DecoderConfig::default()
        };
        let aom_config = crate::decode_av1::aom_config_from(&config);
        let mut color_stream = Vec::new();
        let mut alpha_stream: Option<Vec<u8>> = info.has_alpha.then(Vec::new);
        for i in 0..info.frame_count {
            let fr = inner
                .parser
                .frame(i)
                .map_err(|e| e.map_error(Error::Parse))?;
            color_stream.extend_from_slice(&fr.data);
            if let (Some(buf), Some(ad)) = (alpha_stream.as_mut(), fr.alpha_data.as_ref()) {
                buf.extend_from_slice(ad);
            }
        }
        let color = aom_decode::frame::decode_frames_with(&color_stream, &aom_config)
            .map_err(crate::decode_av1::map_aom_error)?;
        if color.len() != info.frame_count {
            return Err(at!(Error::Unsupported(
                "zenav1-aom decoded a different frame count than the container declares \
                 (unshown-frame packing outside the current envelope)"
            )));
        }
        let alpha = match alpha_stream {
            Some(stream) => {
                let a = aom_decode::frame::decode_frames_with(&stream, &aom_config)
                    .map_err(crate::decode_av1::map_aom_error)?;
                if a.len() != info.frame_count {
                    return Err(at!(Error::Unsupported(
                        "zenav1-aom alpha track frame count does not match the container"
                    )));
                }
                Some(a)
            }
            None => None,
        };
        let mut alpha_iter = alpha.map(|v| v.into_iter());
        Ok(color
            .into_iter()
            .map(|c| {
                Some((
                    c,
                    alpha_iter
                        .as_mut()
                        .map(|it| it.next().expect("len checked")),
                ))
            })
            .collect())
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

        #[cfg(feature = "zenav1-aom")]
        if let Some(frames) = self.aom_frames.as_mut() {
            let (fd, fd_alpha) = frames[self.frame_index]
                .take()
                .expect("animation frames are consumed exactly once in order");
            let duration_ms = self
                .inner
                .parser
                .frame(self.frame_index)
                .map_err(|e| e.map_error(Error::Parse))?
                .duration_ms;
            let (pixels, _info) = self.inner.convert_aom_to_image(fd, fd_alpha, stop)?;
            self.frame_index += 1;
            return Ok(Some(DecodedFrame {
                pixels,
                duration_ms,
            }));
        }

        let frame_ref = self
            .inner
            .parser
            .frame(self.frame_index)
            .map_err(|e| e.map_error(Error::Parse))?;

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

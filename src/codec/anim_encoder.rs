//! [`AvifAnimationFrameEncoder`] and its `BufferedFrame` buffer: frame push
//! and validation, the memory-budget thread fit at finish time, and the
//! dispatch to the native `encode_animation_*` entry points.

use enough::Stop;
use rgb::{Rgb, Rgba};
use whereat::{At, at};
use zencodec::encode::EncodeOutput;
use zencodec::{CodecError, ImageFormat, ResourceLimits};
use zenpixels::PixelSlice;

use super::threads::fit_encode_threads_to_memory;
use crate::error::Error;

/// Buffered frame for animation encoding.
#[cfg(feature = "encode")]
pub(super) enum BufferedFrame {
    Rgb8 {
        pixels: imgref::ImgVec<Rgb<u8>>,
        duration_ticks: u32,
    },
    Rgba8 {
        pixels: imgref::ImgVec<Rgba<u8>>,
        duration_ticks: u32,
    },
    Rgb16 {
        pixels: imgref::ImgVec<Rgb<u16>>,
        duration_ticks: u32,
    },
    Rgba16 {
        pixels: imgref::ImgVec<Rgba<u16>>,
        duration_ticks: u32,
    },
}

impl BufferedFrame {
    fn duration(&self) -> u32 {
        match self {
            Self::Rgb8 { duration_ticks, .. }
            | Self::Rgba8 { duration_ticks, .. }
            | Self::Rgb16 { duration_ticks, .. }
            | Self::Rgba16 { duration_ticks, .. } => *duration_ticks,
        }
    }
    fn duration_mut(&mut self) -> &mut u32 {
        match self {
            Self::Rgb8 { duration_ticks, .. }
            | Self::Rgba8 { duration_ticks, .. }
            | Self::Rgb16 { duration_ticks, .. }
            | Self::Rgba16 { duration_ticks, .. } => duration_ticks,
        }
    }
}

/// Full-frame animation encoder for AVIF.
///
/// Buffers frames and encodes them all on [`finish()`](zencodec::encode::AnimationFrameEncoder::finish).
/// All frames must have the same dimensions and pixel format.
#[cfg(feature = "encode")]
pub struct AvifAnimationFrameEncoder {
    pub(super) config: crate::EncoderConfig,
    pub(super) stop: Option<zencodec::StopToken>,
    pub(super) frames: Vec<BufferedFrame>,
    pub(super) pixel_format: Option<zenpixels::PixelFormat>,
    pub(super) canvas_width: Option<u32>,
    pub(super) canvas_height: Option<u32>,
    pub(super) limits: ResourceLimits,
    /// Number of frames pushed so far, for max_frames enforcement.
    pub(super) frame_count: u32,
    pub(super) loop_count: Option<u32>,
    pub(super) timescale: u32,
    pub(super) retained_bytes: u64,
    pub(super) descriptor: Option<zenpixels::PixelDescriptor>,
    pub(super) color_context: Option<std::sync::Arc<zenpixels::ColorContext>>,
}

#[cfg(feature = "encode")]
impl AvifAnimationFrameEncoder {
    /// Append a frame with an exact positive tick duration.
    ///
    /// `timescale` is ticks per second. Mixed clocks are rescaled exactly to
    /// their least common multiple. An unrepresentable u32 clock or sample
    /// duration returns an error before appending the frame. The trait's
    /// `push_frame` method continues to accept milliseconds.
    pub fn push_frame_ticks(
        &mut self,
        pixels: PixelSlice<'_>,
        duration_ticks: u32,
        timescale: u32,
        stop: Option<&dyn Stop>,
    ) -> Result<(), At<CodecError>> {
        self.push_frame_ticks_inner(pixels, duration_ticks, timescale, stop)
            .map_err(zencodec::CodecError::of)
    }

    fn push_frame_ticks_inner(
        &mut self,
        pixels: PixelSlice<'_>,
        duration_ticks: u32,
        timescale: u32,
        stop: Option<&dyn Stop>,
    ) -> Result<(), At<Error>> {
        if let Some(s) = stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }
        if let Some(s) = &self.stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }
        if timescale == 0 || duration_ticks == 0 {
            return Err(at!(Error::InvalidState(
                "animation timing must be positive".into()
            )));
        }
        let clock = if self.frames.is_empty() {
            timescale
        } else {
            let (mut a, mut b) = (self.timescale, timescale);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            self.timescale.checked_mul(timescale / a).ok_or_else(|| {
                at!(Error::InvalidState(
                    "animation timescale exceeds u32".into()
                ))
            })?
        };
        let incoming = duration_ticks
            .checked_mul(clock / timescale)
            .ok_or_else(|| {
                at!(Error::InvalidState(
                    "animation sample duration exceeds u32".into()
                ))
            })?;
        let factor = if self.frames.is_empty() {
            1
        } else {
            clock / self.timescale
        };
        if factor != 1 {
            for frame in &self.frames {
                frame.duration().checked_mul(factor).ok_or_else(|| {
                    at!(Error::InvalidState(
                        "animation sample duration exceeds u32".into()
                    ))
                })?;
            }
        }
        if let Some(max_ms) = self.limits.max_animation_ms {
            let total = self
                .frames
                .iter()
                .map(|f| u128::from(f.duration()) * u128::from(factor))
                .sum::<u128>()
                + u128::from(incoming);
            if total * 1000 > u128::from(max_ms) * u128::from(clock) {
                return Err(at!(Error::ResourceLimit(
                    "animation duration limit exceeded".into()
                )));
            }
        }
        let previous_len = self.frames.len();
        self.push_frame_inner(pixels, incoming, stop)?;
        if factor != 1 {
            for frame in &mut self.frames[..previous_len] {
                *frame.duration_mut() *= factor;
            }
        }
        self.timescale = clock;
        Ok(())
    }

    fn stop_token(&self) -> almost_enough::StopToken {
        match &self.stop {
            Some(s) => s.clone(),
            None => almost_enough::StopToken::new(enough::Unstoppable),
        }
    }
}

#[cfg(feature = "encode")]
impl zencodec::encode::AnimationFrameEncoder for AvifAnimationFrameEncoder {
    type Error = At<CodecError>;

    fn reject(op: zencodec::UnsupportedOperation) -> At<CodecError> {
        // Bare native error exiting the trait boundary: `.into()` routes through
        // `From<Error> for At<CodecError>`, locating + enveloping in one step.
        Error::UnsupportedOperation(op).into()
    }

    fn push_frame(
        &mut self,
        pixels: PixelSlice<'_>,
        duration_ms: u32,
        stop: Option<&dyn Stop>,
    ) -> Result<(), At<CodecError>> {
        self.push_frame_ticks(pixels, duration_ms, 1000, stop)
    }

    fn push_frame_timed(
        &mut self,
        pixels: PixelSlice<'_>,
        duration: zencodec::animation::FrameDuration,
        stop: Option<&dyn Stop>,
    ) -> Result<(), At<CodecError>> {
        let ticks = u32::try_from(duration.numerator())
            .map_err(|_| Self::reject(zencodec::UnsupportedOperation::AnimationTiming))?;
        self.push_frame_ticks(pixels, ticks, duration.denominator(), stop)
    }

    fn finish(self, stop: Option<&dyn Stop>) -> Result<EncodeOutput, At<CodecError>> {
        self.finish_inner(stop).map_err(zencodec::CodecError::of)
    }
}

#[cfg(feature = "encode")]
impl AvifAnimationFrameEncoder {
    fn push_frame_inner(
        &mut self,
        pixels: PixelSlice<'_>,
        duration_ticks: u32,
        stop: Option<&dyn Stop>,
    ) -> Result<(), At<Error>> {
        if let Some(s) = stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }
        if let Some(s) = &self.stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }
        let (w, h) = (pixels.width(), pixels.rows());
        let desc = pixels.descriptor();
        let fmt = desc.pixel_format();
        if w == 0
            || h == 0
            || desc.signal_range != zenpixels::SignalRange::Full
            || matches!(desc.alpha(), Some(zenpixels::AlphaMode::Premultiplied))
            || !matches!(
                fmt,
                zenpixels::PixelFormat::Rgb8
                    | zenpixels::PixelFormat::Rgba8
                    | zenpixels::PixelFormat::Rgb16
                    | zenpixels::PixelFormat::Rgba16
            )
        {
            return Err(at!(Error::UnsupportedOperation(
                zencodec::UnsupportedOperation::PixelFormat
            )));
        }
        if self.canvas_width.is_some_and(|v| v != w) || self.canvas_height.is_some_and(|v| v != h) {
            return Err(at!(Error::InvalidState(
                "frame dimensions do not match the canvas".into()
            )));
        }
        if self.descriptor.is_some_and(|d| d != desc)
            || (self.descriptor.is_some() && self.color_context.as_ref() != pixels.color_context())
        {
            return Err(at!(Error::InvalidState(
                "animation frames must share precision and color interpretation".into()
            )));
        }
        let count = self
            .frame_count
            .checked_add(1)
            .ok_or_else(|| at!(Error::ResourceLimit("frame count overflow".into())))?;
        let bytes = u64::from(w) * u64::from(h) * desc.bytes_per_pixel() as u64;
        let retained = self
            .retained_bytes
            .checked_add(bytes)
            .ok_or_else(|| at!(Error::ResourceLimit("animation size overflow".into())))?;
        self.limits
            .check_dimensions(w, h)
            .map_err(|e| at!(Error::ResourceLimit(e.to_string())))?;
        self.limits
            .check_frames(count)
            .map_err(|e| at!(Error::ResourceLimit(e.to_string())))?;
        self.limits
            .check_memory(retained)
            .map_err(|e| at!(Error::ResourceLimit(e.to_string())))?;
        self.limits
            .check_total_pixels(u64::from(w) * u64::from(h) * u64::from(count))
            .map_err(|e| at!(Error::ResourceLimit(e.to_string())))?;
        let mut config = self.config.clone();
        if self.frames.is_empty() {
            if let Some(context) = pixels.color_context() {
                if config.icc_profile.is_none() {
                    config.icc_profile = context.icc.as_ref().map(|v| v.to_vec());
                }
                if let Some(cicp) = context.cicp {
                    config.color_primaries = config.color_primaries.or(Some(cicp.color_primaries));
                    config.transfer_characteristics = config
                        .transfer_characteristics
                        .or(Some(cicp.transfer_characteristics));
                }
            }
            config.color_primaries = config.color_primaries.or(desc.primaries.to_cicp());
            config.transfer_characteristics = config
                .transfer_characteristics
                .or(desc.transfer().to_cicp());
            if config.icc_profile.is_none()
                && (config.color_primaries.is_none() || config.transfer_characteristics.is_none())
            {
                return Err(at!(Error::InvalidState(
                    "animation color interpretation must be explicit".into()
                )));
            }
        }
        let job = self.stop.as_ref();
        let u16_at = |p: &[u8], i| u16::from_ne_bytes([p[i], p[i + 1]]);
        let (wu, hu) = (w as usize, h as usize);
        let frame = match fmt {
            zenpixels::PixelFormat::Rgb8 => BufferedFrame::Rgb8 {
                pixels: imgref::ImgVec::new(
                    copy_animation_pixels(&pixels, 3, job, stop, |p| Rgb {
                        r: p[0],
                        g: p[1],
                        b: p[2],
                    })?,
                    wu,
                    hu,
                ),
                duration_ticks,
            },
            zenpixels::PixelFormat::Rgba8 => BufferedFrame::Rgba8 {
                pixels: imgref::ImgVec::new(
                    copy_animation_pixels(&pixels, 4, job, stop, |p| Rgba {
                        r: p[0],
                        g: p[1],
                        b: p[2],
                        a: p[3],
                    })?,
                    wu,
                    hu,
                ),
                duration_ticks,
            },
            zenpixels::PixelFormat::Rgb16 => BufferedFrame::Rgb16 {
                pixels: imgref::ImgVec::new(
                    copy_animation_pixels(&pixels, 6, job, stop, |p| Rgb {
                        r: u16_at(p, 0),
                        g: u16_at(p, 2),
                        b: u16_at(p, 4),
                    })?,
                    wu,
                    hu,
                ),
                duration_ticks,
            },
            zenpixels::PixelFormat::Rgba16 => BufferedFrame::Rgba16 {
                pixels: imgref::ImgVec::new(
                    copy_animation_pixels(&pixels, 8, job, stop, |p| Rgba {
                        r: u16_at(p, 0),
                        g: u16_at(p, 2),
                        b: u16_at(p, 4),
                        a: u16_at(p, 6),
                    })?,
                    wu,
                    hu,
                ),
                duration_ticks,
            },
            _ => unreachable!(),
        };
        self.frames.try_reserve(1).map_err(|_| {
            at!(Error::ResourceLimit(
                "animation frame allocation failed".into()
            ))
        })?;
        self.frames.push(frame);
        self.canvas_width = Some(w);
        self.canvas_height = Some(h);
        self.frame_count = count;
        self.pixel_format = Some(fmt);
        self.descriptor = Some(desc);
        self.color_context = pixels.color_context().cloned();
        self.retained_bytes = retained;
        self.config = config;
        Ok(())
    }

    fn finish_inner(mut self, stop: Option<&dyn Stop>) -> Result<EncodeOutput, At<Error>> {
        if let Some(s) = stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }
        if let Some(ref s) = self.stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }

        if self.frames.is_empty() {
            return Err(at!(Error::InvalidState("no frames to encode".into())));
        }

        // Memory-adaptive concurrency: canvas dimensions and pixel format are
        // known once frames exist, so fit the encoder thread count to the
        // memory budget here (the animation-job counterpart of the still
        // path's `checked_config`). The estimate covers the per-frame AV1
        // encoder working set — the dominant cost; the buffered input frames
        // were checked per push against the raw-size limit above. Errors when
        // even the single-threaded estimate does not fit the budget.
        let mut threads_note = None;
        if let (Some(w), Some(h), Some(fmt)) =
            (self.canvas_width, self.canvas_height, self.pixel_format)
        {
            let bpp: u8 = match fmt {
                zenpixels::PixelFormat::Rgb8 => 3,
                zenpixels::PixelFormat::Rgba8 => 4,
                zenpixels::PixelFormat::Rgb16 => 6,
                // Rgba16 (push_frame_inner rejects everything else).
                _ => 8,
            };
            let mut working_limits = self.limits;
            if let Some(max) = working_limits.max_memory_bytes {
                working_limits.max_memory_bytes = Some(max.saturating_sub(self.retained_bytes));
            }
            let (pin, note) =
                fit_encode_threads_to_memory(&working_limits, &self.config, w, h, bpp)?;
            if let Some(n) = pin {
                self.config = self.config.clone().threads(Some(n));
            }
            threads_note = note;
        }

        let stop_token = self.stop_token();

        let mut avif_file = match self.frames[0] {
            BufferedFrame::Rgb8 { .. } => {
                let anim_frames: Vec<crate::TimedAnimationFrame<Rgb<u8>>> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgb8 {
                            pixels,
                            duration_ticks,
                        } => crate::TimedAnimationFrame {
                            pixels,
                            duration_ticks,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result = crate::encoder::encode_animation_rgb8_timed_with_stop(
                    &anim_frames,
                    self.timescale,
                    &self.config,
                    stop_token.clone(),
                    stop,
                )?;
                result.avif_file
            }
            BufferedFrame::Rgba8 { .. } => {
                let anim_frames: Vec<crate::TimedAnimationFrame<Rgba<u8>>> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgba8 {
                            pixels,
                            duration_ticks,
                        } => crate::TimedAnimationFrame {
                            pixels,
                            duration_ticks,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result = crate::encoder::encode_animation_rgba8_timed_with_stop(
                    &anim_frames,
                    self.timescale,
                    &self.config,
                    stop_token.clone(),
                    stop,
                )?;
                result.avif_file
            }
            BufferedFrame::Rgb16 { .. } => {
                let anim_frames: Vec<crate::TimedAnimationFrame<Rgb<u16>>> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgb16 {
                            pixels,
                            duration_ticks,
                        } => crate::TimedAnimationFrame {
                            pixels,
                            duration_ticks,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result = crate::encoder::encode_animation_rgb16_timed_with_stop(
                    &anim_frames,
                    self.timescale,
                    &self.config,
                    stop_token.clone(),
                    stop,
                )?;
                result.avif_file
            }
            BufferedFrame::Rgba16 { .. } => {
                let anim_frames: Vec<crate::TimedAnimationFrame<Rgba<u16>>> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgba16 {
                            pixels,
                            duration_ticks,
                        } => crate::TimedAnimationFrame {
                            pixels,
                            duration_ticks,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result = crate::encoder::encode_animation_rgba16_timed_with_stop(
                    &anim_frames,
                    self.timescale,
                    &self.config,
                    stop_token.clone(),
                    stop,
                )?;
                result.avif_file
            }
        };

        if let Some(count) = self.loop_count {
            super::animation_repetition::set_loop_count(&mut avif_file, count)?;
        }

        self.limits
            .check_output_size(avif_file.len() as u64)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

        let mut out = EncodeOutput::new(avif_file, ImageFormat::Avif);
        if let Some(note) = threads_note {
            // Reductions are never silent: readable via `extras::<String>()`.
            out = out.with_extras(note);
        }
        Ok(out)
    }
}

fn copy_animation_pixels<T>(
    pixels: &PixelSlice<'_>,
    bpp: usize,
    job: Option<&zencodec::StopToken>,
    call: Option<&dyn Stop>,
    convert: impl Fn(&[u8]) -> T,
) -> Result<Vec<T>, At<Error>> {
    let count = (pixels.width() as usize)
        .checked_mul(pixels.rows() as usize)
        .ok_or_else(|| at!(Error::ResourceLimit("frame size overflow".into())))?;
    let mut output = Vec::new();
    output.try_reserve_exact(count).map_err(|_| {
        at!(Error::ResourceLimit(
            "animation pixel allocation failed".into()
        ))
    })?;
    for y in 0..pixels.rows() {
        if let Some(stop) = job {
            stop.check().map_err(|e| at!(Error::from(e)))?;
        }
        if let Some(stop) = call {
            stop.check().map_err(|e| at!(Error::from(e)))?;
        }
        output.extend(pixels.row(y).chunks_exact(bpp).map(&convert));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::decode_config::AvifDecoderConfig;
    use crate::codec::encode_config::AvifEncoderConfig;
    use std::borrow::Cow;

    #[cfg(feature = "encode")]
    #[test]
    fn avif_animation_frame_encoder_implements_trait() {
        fn _assert_trait<T: zencodec::encode::AnimationFrameEncoder + Send + 'static>() {}
        _assert_trait::<super::AvifAnimationFrameEncoder>();
    }

    #[cfg(feature = "encode")]
    #[test]
    fn zencodec_animation_encode_decode_roundtrip() {
        use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};
        use zencodec::encode::{AnimationFrameEncoder, EncodeJob, EncoderConfig};

        let config = AvifEncoderConfig::new()
            .with_generic_quality(80.0)
            .with_generic_effort(0);
        let mut enc = config
            .job()
            .with_canvas_size(64, 64)
            .with_loop_count(Some(0))
            .animation_frame_encoder()
            .expect("animation_frame_encoder should succeed");

        // Push 3 solid-color RGB8 frames
        let colors: [Rgb<u8>; 3] = [
            Rgb { r: 255, g: 0, b: 0 },
            Rgb { r: 0, g: 255, b: 0 },
            Rgb { r: 0, g: 0, b: 255 },
        ];
        for color in &colors {
            let pixels: Vec<Rgb<u8>> = vec![*color; 64 * 64];
            let img = imgref::ImgVec::new(pixels, 64, 64);
            let ps = PixelSlice::from(img.as_ref()).erase();
            enc.push_frame(ps, 100, None).unwrap();
        }

        let output = enc.finish(None).expect("animation finish should succeed");
        assert!(!output.is_empty(), "encoded animation should not be empty");

        // Decode via zencodec animation frame decoder
        let dec_config = AvifDecoderConfig::new();
        let mut decoder = dec_config
            .job()
            .animation_frame_decoder(Cow::Borrowed(output.data()), &[])
            .expect("should decode the animated AVIF");

        assert_eq!(decoder.frame_count(), Some(3));
        let mut count = 0u32;
        while let Ok(Some(_frame)) = decoder.render_next_frame(None) {
            count += 1;
        }
        assert_eq!(count, 3, "should decode exactly 3 frames");
    }
}

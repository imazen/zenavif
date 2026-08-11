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
        duration_ms: u32,
    },
    Rgba8 {
        pixels: imgref::ImgVec<Rgba<u8>>,
        duration_ms: u32,
    },
    Rgb16 {
        pixels: imgref::ImgVec<Rgb<u16>>,
        duration_ms: u32,
    },
    Rgba16 {
        pixels: imgref::ImgVec<Rgba<u16>>,
        duration_ms: u32,
    },
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
}

#[cfg(feature = "encode")]
impl AvifAnimationFrameEncoder {
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
        self.push_frame_inner(pixels, duration_ms, stop)
            .map_err(zencodec::CodecError::of)
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
        duration_ms: u32,
        stop: Option<&dyn Stop>,
    ) -> Result<(), At<Error>> {
        // Check cancellation (combine per-call + owned stop)
        if let Some(s) = stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }
        if let Some(ref s) = self.stop {
            s.check().map_err(|e| at!(Error::from(e)))?;
        }

        let w = pixels.width();
        let h = pixels.rows();

        // Validate canvas dimensions match
        match (self.canvas_width, self.canvas_height) {
            (Some(cw), Some(ch)) if cw != w || ch != h => {
                return Err(at!(Error::InvalidState(format!(
                    "frame dimensions {}x{} don't match canvas {}x{}",
                    w, h, cw, ch
                ))));
            }
            (None, None) => {
                self.canvas_width = Some(w);
                self.canvas_height = Some(h);
            }
            _ => {}
        }

        // Check resource limits
        let desc = pixels.descriptor();
        let bpp = desc.bytes_per_pixel() as u64;
        self.limits.check_dimensions(w, h).map_err(|_| {
            at!(Error::ImageTooLarge {
                width: w,
                height: h,
            })
        })?;
        self.limits
            .check_memory(w as u64 * h as u64 * bpp)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

        // Enforce max_frames limit.
        self.frame_count += 1;
        self.limits
            .check_frames(self.frame_count)
            .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

        let fmt = desc.pixel_format();

        // Validate consistent pixel format across frames
        if let Some(expected) = self.pixel_format {
            if fmt != expected {
                return Err(at!(Error::InvalidState(format!(
                    "pixel format mismatch: first frame was {expected:?}, this frame is {fmt:?}"
                ))));
            }
        } else {
            self.pixel_format = Some(fmt);
        }

        let raw = pixels.contiguous_bytes();
        let wu = w as usize;
        let hu = h as usize;

        let frame = match fmt {
            zenpixels::PixelFormat::Rgb8 => {
                let rgb: Vec<Rgb<u8>> = bytemuck::cast_slice(&raw).to_vec();
                BufferedFrame::Rgb8 {
                    pixels: imgref::ImgVec::new(rgb, wu, hu),
                    duration_ms,
                }
            }
            zenpixels::PixelFormat::Rgba8 => {
                let rgba: Vec<Rgba<u8>> = bytemuck::cast_slice(&raw).to_vec();
                BufferedFrame::Rgba8 {
                    pixels: imgref::ImgVec::new(rgba, wu, hu),
                    duration_ms,
                }
            }
            zenpixels::PixelFormat::Rgb16 => {
                let rgb: Vec<Rgb<u16>> = bytemuck::cast_slice(&raw).to_vec();
                BufferedFrame::Rgb16 {
                    pixels: imgref::ImgVec::new(rgb, wu, hu),
                    duration_ms,
                }
            }
            zenpixels::PixelFormat::Rgba16 => {
                let rgba: Vec<Rgba<u16>> = bytemuck::cast_slice(&raw).to_vec();
                BufferedFrame::Rgba16 {
                    pixels: imgref::ImgVec::new(rgba, wu, hu),
                    duration_ms,
                }
            }
            _ => {
                return Err(at!(Error::UnsupportedOperation(
                    zencodec::UnsupportedOperation::PixelFormat,
                )));
            }
        };

        self.frames.push(frame);
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
            let (pin, note) = fit_encode_threads_to_memory(&self.limits, &self.config, w, h, bpp)?;
            if let Some(n) = pin {
                self.config = self.config.clone().threads(Some(n));
            }
            threads_note = note;
        }

        let stop_token = self.stop_token();

        let avif_file = match self.frames[0] {
            BufferedFrame::Rgb8 { .. } => {
                let anim_frames: Vec<crate::AnimationFrame> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgb8 {
                            pixels,
                            duration_ms,
                        } => crate::AnimationFrame {
                            pixels,
                            duration_ms,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result =
                    crate::encode_animation_rgb8(&anim_frames, &self.config, stop_token.clone())?;
                result.avif_file
            }
            BufferedFrame::Rgba8 { .. } => {
                let anim_frames: Vec<crate::AnimationFrameRgba> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgba8 {
                            pixels,
                            duration_ms,
                        } => crate::AnimationFrameRgba {
                            pixels,
                            duration_ms,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result =
                    crate::encode_animation_rgba8(&anim_frames, &self.config, stop_token.clone())?;
                result.avif_file
            }
            BufferedFrame::Rgb16 { .. } => {
                let anim_frames: Vec<crate::AnimationFrame16> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgb16 {
                            pixels,
                            duration_ms,
                        } => crate::AnimationFrame16 {
                            pixels,
                            duration_ms,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result =
                    crate::encode_animation_rgb16(&anim_frames, &self.config, stop_token.clone())?;
                result.avif_file
            }
            BufferedFrame::Rgba16 { .. } => {
                let anim_frames: Vec<crate::AnimationFrameRgba16> = self
                    .frames
                    .into_iter()
                    .map(|f| match f {
                        BufferedFrame::Rgba16 {
                            pixels,
                            duration_ms,
                        } => crate::AnimationFrameRgba16 {
                            pixels,
                            duration_ms,
                        },
                        _ => unreachable!(),
                    })
                    .collect();
                let result =
                    crate::encode_animation_rgba16(&anim_frames, &self.config, stop_token.clone())?;
                result.avif_file
            }
        };

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

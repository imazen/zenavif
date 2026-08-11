//! [`AvifAnimationFrameDecoder`] — lazy per-frame animation decode with
//! frame-count / duration limit enforcement, colour-context attach, format
//! negotiation and per-frame orientation bake.

use std::sync::Arc;

use whereat::{At, ResultAtExt as _, at};
use zencodec::{AnimationFrame, CodecError, ImageInfo, ResourceLimits};
use zenpixels::{PixelBuffer, PixelDescriptor};

use super::color::attach_color_context_class_gated;
use super::negotiate::negotiate_format;
use crate::error::Error;

// `animated_avif_animation_frame_decoder_roundtrip` names this through `super`.
#[cfg(test)]
use super::decode_config::AvifDecoderConfig;

/// Animation AVIF full-frame decoder.
///
/// Lazily decodes frames on demand. The `AnimationFrameDecoder` trait doesn't pass
/// a stop token per-call, so per-frame cancellation is not available
/// through this interface (use the native `AnimationDecoder` API for that).
pub struct AvifAnimationFrameDecoder {
    pub(super) anim_decoder: crate::AnimationDecoder,
    pub(super) index: usize,
    /// Number of frames decoded so far (including skipped ones).
    pub(super) frames_decoded: u32,
    /// Skip frames before this index. Frames are still decoded to maintain
    /// correct compositing state, but not yielded to the caller.
    pub(super) start_frame_index: u32,
    pub(super) info: Arc<ImageInfo>,
    pub(super) total_frames: u32,
    /// Animation loop count (0 = infinite, n = play n times).
    pub(super) loop_count: u32,
    pub(super) preferred: Vec<PixelDescriptor>,
    /// Holds the current frame's pixels so `render_next_frame` can return
    /// a borrowing `AnimationFrame<'_>`.
    pub(super) current_frame: Option<PixelBuffer>,
    /// Resource limits for frame count and animation duration enforcement.
    pub(super) limits: ResourceLimits,
    /// Accumulated animation duration in milliseconds across all decoded frames.
    pub(super) accumulated_ms: u64,
    /// Orientation to bake into every frame: the intrinsic `irot`/`imir`
    /// transform on the bake path (`OrientationHint::bakes()`), or `Identity`
    /// (no-op) on the preserve path (the default). Applied after format
    /// negotiation, before the frame is yielded.
    pub(super) bake_to: zencodec::Orientation,
}

impl zencodec::decode::AnimationFrameDecoder for AvifAnimationFrameDecoder {
    type Error = At<CodecError>;

    fn wrap_sink_error(err: zencodec::decode::SinkError) -> Self::Error {
        // Bare native error -> envelope via the `From<Error> for At<CodecError>` bridge.
        Error::ResourceLimit(err.to_string()).into()
    }

    fn info(&self) -> &ImageInfo {
        &self.info
    }

    fn frame_count(&self) -> Option<u32> {
        Some(self.total_frames)
    }

    fn loop_count(&self) -> Option<u32> {
        Some(self.loop_count)
    }

    fn render_next_frame(
        &mut self,
        stop: Option<&dyn zencodec::enough::Stop>,
    ) -> Result<Option<AnimationFrame<'_>>, At<CodecError>> {
        self.render_next_frame_inner(stop)
            .map_err(zencodec::CodecError::of)
    }

    fn render_next_frame_to_sink(
        &mut self,
        stop: Option<&dyn zencodec::enough::Stop>,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<Option<zencodec::decode::OutputInfo>, Self::Error> {
        zencodec::helpers::copy_frame_to_sink(self, stop, sink)
    }
}

impl AvifAnimationFrameDecoder {
    fn render_next_frame_inner(
        &mut self,
        stop: Option<&dyn zencodec::enough::Stop>,
    ) -> Result<Option<AnimationFrame<'_>>, At<Error>> {
        let stop: &dyn zencodec::enough::Stop = stop.unwrap_or(&enough::Unstoppable);
        loop {
            let frame = self.anim_decoder.next_frame(stop).at()?;
            let Some(frame) = frame else {
                return Ok(None);
            };
            let frame_index = self.frames_decoded;
            self.frames_decoded += 1;

            // Enforce max_frames limit (counts all decoded frames, including skipped).
            self.limits
                .check_frames(self.frames_decoded)
                .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

            // Accumulate and enforce max_animation_ms.
            self.accumulated_ms += frame.duration_ms as u64;
            self.limits
                .check_animation_ms(self.accumulated_ms)
                .map_err(|e| at!(Error::ResourceLimit(format!("{e}"))))?;

            // Skip frames before the requested start index. We must still
            // decode them to maintain correct compositing state, but we
            // don't yield them to the caller.
            if frame_index < self.start_frame_index {
                continue;
            }

            // Animation frames stay on the RGB path (native-gray opt-in is
            // still-image only); no gray claims here.
            let pixels = attach_color_context_class_gated(frame.pixels, &self.info.source_color);
            let pixels = negotiate_format(pixels, &self.preferred, false);
            // Bake orientation into the frame on the bake path; `Identity` is a
            // no-op (preserve path keeps stored-orientation pixels unchanged).
            let pixels = if self.bake_to.is_identity() {
                pixels
            } else {
                zenpixels_convert::orient::apply_orientation(pixels.as_slice(), self.bake_to)
            };
            let idx = self.index as u32;
            self.index += 1;
            let duration_ms = frame.duration_ms;
            self.current_frame = Some(pixels);
            let slice = self.current_frame.as_ref().unwrap().as_slice().erase();
            return Ok(Some(AnimationFrame::new(slice, duration_ms, idx)));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn avif_animation_frame_decoder_implements_trait() {
        // AvifAnimationFrameDecoder implements AnimationFrameDecoder which includes loop_count()
        fn _assert_trait<T: zencodec::decode::AnimationFrameDecoder>() {}
        _assert_trait::<super::AvifAnimationFrameDecoder>();
    }

    #[test]
    fn animated_avif_animation_frame_decoder_roundtrip() {
        use super::AvifDecoderConfig;
        use std::borrow::Cow;
        use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecoderConfig};

        // This fixture lives in the codec-corpus repo, which CI does not
        // clone. Per the no-graceful-skips policy the skip decision belongs
        // to the CALLER: environments without the corpus declare it via
        // ZENAVIF_NO_CODEC_CORPUS=1 (set in ci.yml); everywhere else a
        // missing file is a loud failure, never a silent pass. (The old
        // `../codec-corpus` + exists()-return silently no-oped everywhere:
        // the corpus checkout lives one level further up.)
        if std::env::var_os("ZENAVIF_NO_CODEC_CORPUS").is_some() {
            return;
        }
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let rel = "codec-corpus/avif-conformance/valid/2.avif";
        let path = [manifest.join("..").join(rel), manifest.join("../..").join(rel)]
            .into_iter()
            .find(|p| p.exists())
            .unwrap_or_else(|| {
                panic!(
                    "codec-corpus fixture {rel} not found beside {} (clone                      imazen/codec-corpus at ~/work/, or set                      ZENAVIF_NO_CODEC_CORPUS=1 to declare it absent)",
                    manifest.display()
                )
            });
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        let config = AvifDecoderConfig::new();
        let mut decoder = config
            .job()
            .animation_frame_decoder(Cow::Borrowed(&data), &[])
            .expect("animation_frame_decoder should succeed for animated AVIF");

        // Verify metadata
        let info = decoder.info();
        assert!(info.is_animation(), "should be detected as animation");
        assert!(
            info.width > 0 && info.height > 0,
            "dimensions must be nonzero"
        );

        // frame_count and loop_count should be populated
        let frame_count = decoder.frame_count();
        assert!(
            frame_count.is_some(),
            "frame_count should be Some for animated AVIF"
        );
        let total = frame_count.unwrap();
        assert!(
            total >= 2,
            "animated AVIF should have at least 2 frames, got {total}"
        );

        let loop_count = decoder.loop_count();
        assert!(
            loop_count.is_some(),
            "loop_count should be Some for animated AVIF"
        );

        // Decode all frames
        let mut frames_decoded = 0u32;
        loop {
            match decoder.render_next_frame(None) {
                Ok(Some(frame)) => {
                    assert_eq!(frame.frame_index(), frames_decoded);
                    let pixels = frame.pixels();
                    assert!(
                        pixels.width() > 0 && pixels.rows() > 0,
                        "frame {} pixels should have nonzero dimensions",
                        frames_decoded
                    );
                    frames_decoded += 1;
                }
                Ok(None) => break,
                Err(e) => panic!("render_next_frame failed at frame {frames_decoded}: {e}"),
            }
        }

        assert_eq!(
            frames_decoded, total,
            "should decode exactly {total} frames, got {frames_decoded}"
        );
    }
}

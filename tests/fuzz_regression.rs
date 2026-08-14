//! Replay seed inputs from `fuzz/regression/` through every fuzz target
//! entry point. Shared scaffolding lives in `zen-fuzz-regress`.

use zenutils_fuzz::RegressionSuite;

#[test]
fn fuzz_regression() {
    RegressionSuite::new("fuzz/regression")
        .target("decode", |input| {
            // Mirrors fuzz_decode.rs: 4 MP frame_size_limit.
            let config = zenavif::DecoderConfig::new().frame_size_limit(4 * 1024 * 1024);
            let _ = zenavif::decode_with(input, &config, &enough::Unstoppable);
        })
        .target("decode_limited", |input| {
            // Mirrors fuzz_decode_limited.rs: tight 1 MP cap.
            let config = zenavif::DecoderConfig::new().frame_size_limit(1024 * 1024);
            let _ = zenavif::decode_with(input, &config, &enough::Unstoppable);
        })
        .target("decode_animation", |input| {
            // Mirrors fuzz_decode_animation.rs.
            let config = zenavif::DecoderConfig::new().frame_size_limit(4 * 1024 * 1024);
            let _ = zenavif::decode_animation_with(input, &config, &enough::Unstoppable);
            if let Ok(mut anim) = zenavif::AnimationDecoder::new(input, &config) {
                while let Ok(Some(_frame)) = anim.next_frame(&enough::Unstoppable) {}
            }
        })
        .target("probe", |input| {
            // Mirrors fuzz_probe.rs: lightweight container parse + probe_info.
            let config = zenavif::DecoderConfig::new();
            if let Ok(decoder) = zenavif::ManagedAvifDecoder::new(input, &config) {
                let _ = decoder.probe_info();
            }
        })
        .target("decode_negotiate", |input| {
            // Mirrors fuzz_decode_negotiate.rs: the zencodec adapter with a
            // caller `preferred` list, on all three entry points. The fuzz
            // target steers the preference and the path from two in-band
            // bytes; a replay seed is a whole file, so sweep the steering
            // instead of consuming it.
            use std::borrow::Cow;
            use zencodec::decode::{
                Decode as _, DecodeJob as _, DecoderConfig as _, StreamingDecode as _,
            };
            use zenpixels::PixelDescriptor;

            let limits = zencodec::ResourceLimits::none().with_max_pixels(1 << 20);
            for pref in [
                &[][..],
                &[PixelDescriptor::RGB8_SRGB][..],
                &[PixelDescriptor::RGBA8_SRGB][..],
                &[PixelDescriptor::GRAY8_SRGB][..],
            ] {
                let cfg = zenavif::AvifDecoderConfig::new();
                if let Ok(dec) = cfg
                    .clone()
                    .job()
                    .with_limits(limits)
                    .decoder(Cow::Borrowed(input), pref)
                {
                    let _ = dec.decode();
                }
                if let Ok(mut dec) = cfg
                    .clone()
                    .job()
                    .with_limits(limits)
                    .streaming_decoder(Cow::Borrowed(input), pref)
                {
                    for _ in 0..4096 {
                        match dec.next_batch() {
                            Ok(Some(_)) => {}
                            _ => break,
                        }
                    }
                }
                let mut sink = ScratchSink { buf: Vec::new() };
                let _ = cfg.job().with_limits(limits).push_decoder(
                    Cow::Borrowed(input),
                    &mut sink,
                    pref,
                );
            }
        })
        .run();
}

/// Minimal sink for the `decode_negotiate` replay: hands back a correctly
/// sized scratch buffer and discards it. Mirrors the one in
/// `fuzz/fuzz_targets/fuzz_decode_negotiate.rs`.
struct ScratchSink {
    buf: Vec<u8>,
}

impl zencodec::decode::DecodeRowSink for ScratchSink {
    fn provide_next_buffer(
        &mut self,
        _y: u32,
        height: u32,
        width: u32,
        descriptor: zenpixels::PixelDescriptor,
    ) -> Result<zenpixels::PixelSliceMut<'_>, zencodec::decode::SinkError> {
        let stride = (width as usize).saturating_mul(descriptor.bytes_per_pixel());
        let needed = (height as usize).saturating_mul(stride);
        self.buf.clear();
        self.buf.resize(needed, 0);
        zenpixels::PixelSliceMut::new(&mut self.buf, width, height, stride, descriptor)
            .map_err(|_| zencodec::decode::SinkError::from("scratch buffer rejected"))
    }
}

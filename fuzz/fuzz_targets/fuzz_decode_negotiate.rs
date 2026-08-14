#![no_main]

use std::borrow::Cow;

use libfuzzer_sys::fuzz_target;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _, StreamingDecode as _};
use zenpixels::{PixelDescriptor, PixelSliceMut};

// Format-negotiation fuzzer: arbitrary bytes decoded through the **zencodec
// adapter** with a caller `preferred` list, on all three entry points.
//
// The other decode fuzzers all call `zenavif::decode_with`, the native entry
// point, which takes no `preferred` list and never reaches
// `src/codec/negotiate.rs`. That left the entire negotiation and
// format-conversion layer unfuzzed — which is how zenavif#39 (an `expect` on
// `RowConverter::new` reachable from an HDR CICP plus an ordinary `[Rgba8]`
// preference) survived. The arm is selected by the *file's own* CICP, so it
// is squarely an untrusted-input surface and belongs here.
//
// Any panic or abort is a bug; every failure must arrive as `Err`.
fuzz_target!(|data: &[u8]| {
    // First two bytes steer the negotiation; the rest is the image. Keeping
    // the steering in-band lets the corpus evolve interesting (bytes,
    // preference, path) triples together rather than fixing one shape.
    let (choice, image) = match data.split_first_chunk::<2>() {
        Some((head, rest)) => (*head, rest),
        None => return,
    };

    const D: &[PixelDescriptor] = &[
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGBA8_SRGB,
        PixelDescriptor::RGB16_SRGB,
        PixelDescriptor::RGBA16_SRGB,
        PixelDescriptor::GRAY8_SRGB,
        PixelDescriptor::GRAY16_SRGB,
    ];
    // Build a 0..=2-entry preference list from the first byte's nibbles, so
    // both single-preference and fallthrough orderings get exercised.
    let mut preferred: Vec<PixelDescriptor> = Vec::new();
    let n = (choice[0] >> 6) as usize; // 0..=3 entries, capped below
    for k in 0..n.min(2) {
        preferred.push(D[((choice[0] as usize >> (k * 3)) + k) % D.len()]);
    }

    let cfg = || {
        let c = zenavif::AvifDecoderConfig::new();
        if choice[1] & 0x40 != 0 {
            c.with_orientation(zencodec::OrientationHint::Correct)
        } else {
            c
        }
    };
    // Keep the fuzzer's memory bounded — an unbounded decode is the other
    // fuzzers' job, not this one's.
    let limits = zencodec::ResourceLimits::none().with_max_pixels(1 << 20);

    match choice[1] % 3 {
        0 => {
            if let Ok(dec) = cfg()
                .job()
                .with_limits(limits)
                .decoder(Cow::Borrowed(image), &preferred)
            {
                let _ = dec.decode();
            }
        }
        1 => {
            if let Ok(mut dec) = cfg()
                .job()
                .with_limits(limits)
                .streaming_decoder(Cow::Borrowed(image), &preferred)
            {
                // Bound the pull loop: a corrupt stream must not be able to
                // turn this into an infinite fuzz iteration.
                for _ in 0..4096 {
                    match dec.next_batch() {
                        Ok(Some(_)) => {}
                        _ => break,
                    }
                }
            }
        }
        _ => {
            let mut sink = ScratchSink { buf: Vec::new() };
            let _ = cfg().job().with_limits(limits).push_decoder(
                Cow::Borrowed(image),
                &mut sink,
                &preferred,
            );
        }
    }
});

// Minimal sink: hands back a correctly-sized scratch buffer and discards it.
struct ScratchSink {
    buf: Vec<u8>,
}

impl zencodec::decode::DecodeRowSink for ScratchSink {
    fn provide_next_buffer(
        &mut self,
        _y: u32,
        height: u32,
        width: u32,
        descriptor: PixelDescriptor,
    ) -> Result<PixelSliceMut<'_>, zencodec::decode::SinkError> {
        let stride = (width as usize).saturating_mul(descriptor.bytes_per_pixel());
        let needed = (height as usize).saturating_mul(stride);
        self.buf.clear();
        self.buf.resize(needed, 0);
        PixelSliceMut::new(&mut self.buf, width, height, stride, descriptor)
            .map_err(|_| zencodec::decode::SinkError::from("scratch buffer rejected"))
    }
}

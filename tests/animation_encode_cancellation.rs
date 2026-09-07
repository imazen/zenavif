#![cfg(feature = "encode")]

use almost_enough::{Stop, StopReason, StopToken};
use imgref::Img;
use rgb::{Rgb, Rgba};
use std::sync::atomic::{AtomicUsize, Ordering};
use zenavif::*;

struct StopAfterEntry(AtomicUsize);
impl Stop for StopAfterEntry {
    fn check(&self) -> core::result::Result<(), StopReason> {
        if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
            Ok(())
        } else {
            Err(StopReason::Cancelled)
        }
    }
}

#[test]
fn zenravif_animation_propagates_cancellation_after_public_entry() {
    let config = EncoderConfig::new()
        .backend(Av1Backend::Zenravif)
        .speed(10)
        .bit_depth(EncodeBitDepth::Eight);
    for alpha in [false, true] {
        // The public entry check succeeds. Cancellation must reach the owner,
        // then return through error_from_ravif as the canonical Cancelled error.
        let stop = StopToken::new(StopAfterEntry(AtomicUsize::new(0)));
        let result = if alpha {
            encode_animation_rgba8(
                &[AnimationFrameRgba {
                    pixels: Img::new(vec![Rgba::new(40, 90, 180, 128); 32 * 32], 32, 32),
                    duration_ms: 20,
                }],
                &config,
                stop,
            )
        } else {
            encode_animation_rgb8(
                &[AnimationFrame {
                    pixels: Img::new(vec![Rgb::new(40, 90, 180); 32 * 32], 32, 32),
                    duration_ms: 20,
                }],
                &config,
                stop,
            )
        };
        assert!(
            matches!(
                result.as_ref().map_err(|e| e.error()),
                Err(Error::Cancelled(_))
            ),
            "alpha={alpha}: cancellation was lost: {:?}",
            result.map(|_| ())
        );
    }
}

#![cfg(feature = "encode-threading")]

use almost_enough::{Stop, StopReason, StopToken};
use imgref::Img;
use rgb::Rgba;
use std::sync::{Arc, Mutex};
use zenavif::*;

struct ObserveThreads(Arc<Mutex<Vec<usize>>>);
impl Stop for ObserveThreads {
    fn check(&self) -> core::result::Result<(), StopReason> {
        if rayon::current_thread_index().is_some() {
            let count = rayon::current_num_threads();
            let mut seen = self.0.lock().unwrap();
            if !seen.contains(&count) {
                seen.push(count);
            }
        }
        Ok(())
    }
}

#[test]
fn zenravif_animation_uses_requested_workers_for_color_and_alpha() {
    let frames = [20, 30].map(|duration_ms| AnimationFrameRgba {
        pixels: Img::new(
            (0..65 * 67)
                .map(|i| Rgba::new((i * 17) as u8, (i * 31) as u8, (i * 7) as u8, 128))
                .collect::<Vec<_>>(),
            65,
            67,
        ),
        duration_ms,
    });
    for depth in [EncodeBitDepth::Eight, EncodeBitDepth::Ten] {
        let mut baseline = None;
        for threads in [1, 2] {
            let observed = Arc::new(Mutex::new(Vec::new()));
            let config = EncoderConfig::new()
                .backend(Av1Backend::Zenravif)
                .speed(10)
                .bit_depth(depth)
                .threads(Some(threads));
            let encoded = encode_animation_rgba8(
                &frames,
                &config,
                StopToken::new(ObserveThreads(observed.clone())),
            )
            .unwrap();
            assert_eq!(
                *observed.lock().unwrap(),
                vec![threads],
                "depth={depth:?}: worker setting did not reach every coding context"
            );
            if let Some(ref baseline) = baseline {
                assert_eq!(&encoded.avif_file, baseline);
            } else {
                baseline = Some(encoded.avif_file);
            }
        }
    }
}

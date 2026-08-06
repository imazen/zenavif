//! Measure cooperative-cancellation latency on the decode path.
//!
//! What "latency" means here: the wall time from `StopSource::cancel()` to the
//! decode call returning. That is the number a server actually cares about —
//! how long a worker keeps burning CPU on a request the client already
//! abandoned.
//!
//! The run is a genuine before/after in ONE binary. `ZENAVIF_CANCEL_RELAY=0`
//! reproduces the pre-relay behavior (the caller's token never reaches
//! rav1d-safe, so it is polled only at phase boundaries and a frame decode runs
//! to completion); the default reproduces the shipped behavior. Run it twice —
//! the flag is read once per process — so both legs decode the same bytes, at
//! the same size, on the same machine.
//!
//! ```text
//! cargo run --release --example cancel_latency --features encode
//! cargo run --release --example cancel_latency --features encode,aom-backend
//! ```
//!
//! Reported per leg: n, min / p50 / p90 / p99 / max cancellation latency, plus
//! an uncancelled control decode so the latency can be read against the length
//! of the operation being interrupted.

use std::time::{Duration, Instant};

/// Decoder threads for every leg. `ZENAVIF_DECODE_THREADS` overrides it; 0 is
/// rav1d-safe's auto-detect. Exposed because tile threading is the axis the
/// known `DisjointMut` worker race lives on, and a latency harness that cannot
/// vary it cannot tell a cancellation result from a decode failure.
fn decode_threads() -> u32 {
    std::env::var("ZENAVIF_DECODE_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn decoder_config() -> DecoderConfig {
    DecoderConfig::default().threads(decode_threads())
}

use almost_enough::StopSource;
use zenavif::{DecoderConfig, EncoderConfig};

/// Cancel points are swept across the decode rather than fixed, so a lucky
/// alignment with a phase boundary cannot flatter the result.
const TRIALS: usize = 40;

fn main() {
    // Sweep sizes: cancellation latency is floored by the decoder's own check
    // spacing (one superblock row), and an sbrow's WALL time scales with width
    // and thread count — so a single size cannot answer "is it under 5 ms?".
    // Sizes follow the repo's sweep discipline: tiny, small, medium, large.
    let sizes: Vec<(u32, u32)> = vec![(64, 64), (256, 256), (1024, 1024), (2048, 2048), (3840, 2160)];
    let relay_on = !matches!(
        std::env::var("ZENAVIF_CANCEL_RELAY").as_deref(),
        Ok("0") | Ok("off")
    );
    let threads = decode_threads();

    eprintln!(
        "# zenavif cancellation latency — relay={} threads={} trials={}",
        if relay_on { "on" } else { "off" },
        threads,
        TRIALS
    );
    println!("width\theight\trelay\tthreads\tcontrol_ms\tn\tmin_ms\tp50_ms\tp90_ms\tp99_ms\tmax_ms");

    for (w, h) in sizes {
        let avif = match build_source(w, h) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{w}x{h}: encode failed: {e}");
                continue;
            }
        };
        let control = match control_decode(&avif) {
            Some(d) => d,
            None => {
                // A failed decode returns in microseconds; timing it would
                // silently report a fantastic cancellation latency. Skip loudly.
                eprintln!("{w}x{h}: control decode FAILED — no latency reported for this cell");
                continue;
            }
        };
        let lat = sample(&avif, control);
        if lat.is_empty() {
            eprintln!("{w}x{h}: no trial cancelled in time (decode outran every cancel point)");
            continue;
        }
        let q = |p: f64| ms(lat[((lat.len() - 1) as f64 * p).round() as usize]);
        println!(
            "{w}\t{h}\t{}\t{threads}\t{:.3}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}",
            if relay_on { "on" } else { "off" },
            ms(control),
            lat.len(),
            ms(lat[0]),
            q(0.50),
            q(0.90),
            q(0.99),
            ms(lat[lat.len() - 1]),
        );
    }
}

/// Encode a detailed synthetic image. Detail matters: a flat image encodes to
/// almost nothing and decodes too fast to interrupt, which would measure the
/// harness rather than the decoder.
fn build_source(w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut px = Vec::with_capacity((w as usize) * (h as usize));
    for y in 0..h {
        for x in 0..w {
            // Cheap high-frequency pattern — no smooth gradients, which the
            // repo's benchmarking rules ban as unrepresentative content.
            let r = ((x * 7 + y * 3) % 251) as u8;
            let g = ((x ^ y) % 253) as u8;
            let b = ((x.wrapping_mul(y) >> 3) % 249) as u8;
            px.push(rgb::Rgb { r, g, b });
        }
    }
    let img = imgref::Img::new(px, w as usize, h as usize);
    let cfg = EncoderConfig::default().speed(10);
    zenavif::encode_rgb8(img.as_ref(), &cfg, almost_enough::StopToken::new(enough::Unstoppable))
        .map(|e| e.avif_file)
        .map_err(|e| e.to_string())
}

fn control_decode(avif: &[u8]) -> Option<Duration> {
    let cfg = decoder_config();
    // One warm-up (allocator + code paths), then the measured run.
    // The result is CHECKED, not discarded: a decode that errors returns in
    // microseconds, and timing that instead of a real decode would silently
    // turn this whole harness into a measurement of the error path.
    if let Err(e) = zenavif::decode_with(avif, &cfg, &enough::Unstoppable) {
        eprintln!("control decode FAILED: {e}");
        return None;
    }
    let t = Instant::now();
    let out = zenavif::decode_with(avif, &cfg, &enough::Unstoppable);
    let d = t.elapsed();
    out.ok().map(|_| d)
}

/// Collect cancellation latencies, sweeping the cancel point across the decode
/// so the sample covers early/middle/late cancellation rather than one
/// privileged instant. Returned sorted.
fn sample(avif: &[u8], control: Duration) -> Vec<Duration> {
    let mut lat = Vec::with_capacity(TRIALS);
    for i in 0..TRIALS {
        let frac = (i as f64 + 0.5) / TRIALS as f64;
        if let Some(d) = one_trial(avif, control.mul_f64(frac * 0.9)) {
            lat.push(d);
        }
    }
    lat.sort_unstable();
    lat
}

/// One trial: start a decode, cancel it `at` into the decode, return the time
/// from cancel to return. `None` if the decode finished before the cancel
/// landed (that trial measures nothing about cancellation).
fn one_trial(avif: &[u8], at: Duration) -> Option<Duration> {
    let source = StopSource::new();
    let cfg = decoder_config();

    std::thread::scope(|s| {
        let canceller = s.spawn(|| {
            std::thread::sleep(at);
            let fired = Instant::now();
            source.cancel();
            fired
        });

        let result = zenavif::decode_with(avif, &cfg, &source.as_ref());
        let returned = Instant::now();
        let fired = canceller.join().ok()?;

        match result {
            // Cancellation observed: this is the number we came for.
            Err(_) if returned > fired => Some(returned.duration_since(fired)),
            _ => None,
        }
    })
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

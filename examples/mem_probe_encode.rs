//! Encode peak-memory probe — one AVIF encode, report measured peak RSS (VmHWM).
//!
//! The ENCODE counterpart to `examples/heaptrack_decode.rs` (decode side) and a
//! raw-`.bin` sibling of `examples/avif_probe.rs` (the calibration harness that
//! produced the constants in `heuristics.rs`). Used by the heaptrack / VmHWM
//! sweep to calibrate the encode peak-memory model
//! (`heuristics::estimate_encode`, surfaced as the zencodec
//! `estimate_encode_resources`) against measured reality, *per effort level*
//! (the AV1 `speed` preset, the dominant cost knob), instead of the current
//! sub-linear `ENCODE_FIXED + bpp·pixels` guess.
//!
//!   cargo build -p zenavif --release --features encode --example mem_probe_encode
//!   GLIBC_TUNABLES=glibc.malloc.mmap_threshold=131072 \
//!     ./target/release/examples/mem_probe_encode <rgb8.bin> <w> <h> <avif> <speed 0..10> <quality>
//!   heaptrack ./target/release/examples/mem_probe_encode ...   # allocator peak heap
//!
//! One encode per process — peak RSS is a per-process high-water mark, so the
//! input must come from a cheap file read (raw RGB8 bin), never an in-process
//! decode (whose own peak would pollute VmHWM above the encode peak).
//!
//! TSV row:
//!   w  h  pixels  mode  speed  quality  out_bytes  pre_rss_kb  vmhwm_kb  marginal_kb
//!
//! `est` mode (7th arg `est`): prints the codec's CURRENT model prediction for
//! this cell from `zenavif::heuristics::estimate_encode` (no encode), so model
//! vs measured can be compared in the same harness.
//!
//! ## Effort axis = AV1 `speed` (0..=10)
//!
//! 0 = slowest/densest search (most memory + by FAR the most time),
//! 10 = fastest. AVIF encode memory IS effort-dependent: the measured marginal
//! working set is ~38 B/px at speed 4 vs ~46 B/px at speed 10 (denser search at
//! low speed actually holds *less* peak — RDO is depth-first, the fast modes
//! buffer more), and `heuristics::estimate_encode` clamps the time curve to the
//! speed-4 anchor below 4 (speeds 1–3 unmeasured; speed 0 is NOT a real AV1
//! preset — zenravif/`speed_value` ultimately maps it through the AV1 range).
//! Representative levels to sweep: **10 (fast/default-ish), 6 (mid), 2 (slow)**.
//!   // VERIFY: AVOID speed 0/1 at 4096² — single-thread AV1 search there is
//!   // minutes-long. Run large sizes only at speed >=6 unless the parent's
//!   // resource cap explicitly budgets the time. The probe pins threads(Some(1))
//!   // so the peak is the clean single-thread working set; with N threads the AV1
//!   // tile contexts add ~mem_bytes_per_thread each (see encode_threading_info).

use std::hint::black_box;

use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::Rgb;
use zenavif::EncoderConfig;

/// A `/proc/self/status` field in KiB (e.g. `VmRSS:`, `VmHWM:`).
fn status_kb(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(field))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 7 {
        eprintln!(
            "usage: mem_probe_encode <rgb8.bin> <w> <h> <avif> <speed 0..10> <quality> [est]"
        );
        std::process::exit(2);
    }
    let path = &a[1];
    let w: u32 = a[2].parse().expect("w");
    let h: u32 = a[3].parse().expect("h");
    // 4th arg is the output mode tag. Only `avif` is meaningful here; it is
    // accepted (and echoed in the TSV) to keep the arg shape uniform with the
    // other codecs' probes, which take a subsampling/mode token in this slot.
    let mode = match a[4].as_str() {
        "avif" => a[4].clone(),
        other => panic!("mode must be avif, got {other}"),
    };
    let speed: u8 = a[5].parse().expect("speed");
    let quality: f32 = a[6].parse().expect("quality");

    // `est` mode: print what the CURRENT model predicts for this cell (min /
    // typical / max peak + time), no encode — so model vs measured can be
    // compared without an encode polluting VmHWM. RGB8 input ⇒ input_bpp = 3.
    if a.get(7).map(String::as_str) == Some("est") {
        let pixels = (w as u64) * (h as u64);
        let input_bpp: u8 = 3; // VERIFY: RGB8 packed; rgba=4, rgb16=6, rgba16=8.
        let est = zenavif::heuristics::estimate_encode(w, h, input_bpp, speed);
        let (min, typ, max, t) = est
            .map(|e| {
                (
                    e.peak_memory_bytes_min / 1024,
                    e.peak_memory_bytes / 1024,
                    e.peak_memory_bytes_max / 1024,
                    e.time_ms,
                )
            })
            .unwrap_or((0, 0, 0, 0.0));
        println!(
            "{w}\t{h}\t{pixels}\t{mode}\t{speed}\t{quality}\tEST\tmin_kb={min}\ttyp_kb={typ}\tmax_kb={max}\tmin_bpp={:.2}\ttyp_bpp={:.2}\tmax_bpp={:.2}\test_time_ms={t:.1}",
            (min * 1024) as f64 / pixels as f64,
            (typ * 1024) as f64 / pixels as f64,
            (max * 1024) as f64 / pixels as f64,
        );
        return;
    }

    let data = std::fs::read(path).expect("read rgb8.bin");
    assert_eq!(
        data.len(),
        (w as usize) * (h as usize) * 3,
        "bin size {} != w*h*3 {}",
        data.len(),
        (w as usize) * (h as usize) * 3
    );

    // Single-thread so the high-water mark is the clean per-pixel working set
    // the model targets (matches the `avif_probe` calibration, which pinned
    // threads=1). The AV1 `speed` preset is the effort axis.
    let config = EncoderConfig::new()
        .quality(quality)
        .speed(speed)
        .threads(Some(1));

    // Pack the raw RGB8 bytes into the `Rgb<u8>` buffer the encoder takes.
    // VERIFY: this allocation (w*h*3 B) lands AFTER `pre` below, so it is part
    // of `marginal` — matching `avif_probe.rs` (which captures its baseline
    // `b0` *before* the equivalent `Vec<Rgb<u8>>` collect). Keeping it on the
    // measured side is deliberate so this probe stays comparable to the
    // constants already baked into `heuristics.rs`.
    let pre = status_kb("VmRSS:");

    let px: Vec<Rgb<u8>> = data
        .chunks_exact(3)
        .map(|c| Rgb {
            r: c[0],
            g: c[1],
            b: c[2],
        })
        .collect();
    let img = Img::new(px, w as usize, h as usize);

    let out = zenavif::encode_rgb8(img.as_ref(), &config, StopToken::new(Unstoppable))
        .expect("encode_rgb8");

    // High-water mark immediately after encode — VmHWM is monotonic, so it
    // reflects the peak *during* the encode.
    let peak = status_kb("VmHWM:");

    let pixels = (w as u64) * (h as u64);
    println!(
        "{w}\t{h}\t{pixels}\t{mode}\t{speed}\t{quality}\t{}\t{pre}\t{peak}\t{}",
        out.avif_file.len(),
        peak.saturating_sub(pre)
    );
    black_box(&out.avif_file);
}

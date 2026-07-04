//! zenavif#30 futex-hang repro/stress loop.
//!
//! ~2% of two-pass conformance cells hung forever in `futex_` under 10-way
//! process parallelism (issue #30, docs/DIFFMAP_TWO_PASS.md "Known bug").
//! Root cause (found 2026-07-03 with this harness): a rav1d-safe tile worker
//! panicked on a real `overlapping DisjointMut` race — the loop filter's
//! tile-threading compact-COW guards covered tap rows CDEF legitimately
//! touches — and the dead worker's task could never complete, so
//! `rav1d_decode_frame`'s completion wait blocked forever. Fixed in
//! rav1d-safe (the guards now match dav1d's write/read sets exactly, and a
//! worker death fails the decode instead of wedging). Both halves are
//! regression-tested upstream; this example remains the zenavif-stack loop
//! that found it and re-verifies the product path end to end.
//!
//! The cell flow is encode -> decode (rav1d-safe, ~n_cpu tile workers, fresh
//! pool per decode) -> butteraugli (global rayon pool) -> encode. The
//! encodes are single-threaded in the two-pass build (no `encode-threading`),
//! so the threaded components are decode + butteraugli. The race needs heavy
//! scheduling pressure: run ~10+ instances in parallel (see #30 for the
//! original harness shape).
//!
//! Modes:
//! - `fast`: encode once up front, then loop { decode + butteraugli } —
//!   maximum race dice-rolls per second on the two threaded components.
//! - `full`: loop { encode + decode + butteraugli + encode } — the exact
//!   two-pass cell shape (minus the FrameHints handoff, which is pure data).
//! - `decode`: loop { decode } only — isolates rav1d-safe.
//! - `butter`: loop { butteraugli } only — isolates the rayon pool.
//!
//! Prints a heartbeat line per iteration (stdout, flushed); a driving
//! harness detects a process whose heartbeat stops while CPU stays ~0.
//!
//! Usage:
//!   hang_stress <in.png> <iters> <fast|full|decode|butter> [quality] [speed] [420|444]

use almost_enough::{StopToken, Unstoppable};
use imgref::{ImgRef, ImgVec};
use rgb::Rgb;
use std::io::Write as _;
use zenavif::{DecoderConfig, EncoderConfig, encode_rgb8};

fn load_png_rgb(path: &std::path::Path) -> ImgVec<Rgb<u8>> {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
        .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels: Vec<Rgb<u8>> = img
        .pixels()
        .map(|p| Rgb {
            r: p.0[0],
            g: p.0[1],
            b: p.0[2],
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn encode(img: ImgRef<'_, Rgb<u8>>, config: &EncoderConfig, stop: &StopToken) -> Vec<u8> {
    encode_rgb8(img, config, stop.clone())
        .expect("encode")
        .avif_file
}

/// Decode with zenavif's own decoder — same call shape as
/// `two_pass.rs::encode_rgb8_two_pass` (rav1d-safe managed, threads auto).
fn decode(avif: &[u8], stop: &StopToken) -> ImgVec<Rgb<u8>> {
    let dec_config = DecoderConfig::new().prefer_8bit(true);
    let decoded = zenavif::decode_with(avif, &dec_config, stop).expect("decode");
    let img = decoded
        .try_as_imgref::<Rgb<u8>>()
        .expect("decode not RGB8-viewable");
    let mut pixels = Vec::with_capacity(img.width() * img.height());
    for row in img.rows() {
        pixels.extend_from_slice(row);
    }
    ImgVec::new(pixels, img.width(), img.height())
}

/// Butteraugli diffmap — same params as `two_pass.rs::compute_error_map`.
fn butter(src: ImgRef<'_, Rgb<u8>>, dec: ImgRef<'_, Rgb<u8>>) -> (f64, f64) {
    let params = butteraugli::ButteraugliParams::new()
        .with_hf_asymmetry(1.0)
        .with_intensity_target(80.0)
        .with_compute_diffmap(true);
    let ba = butteraugli::butteraugli(src, dec, &params).expect("butteraugli");
    assert!(ba.diffmap.is_some(), "no diffmap");
    (ba.score, ba.pnorm_3)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "usage: hang_stress <in.png> <iters> <fast|full|decode|butter> [quality] [speed] [420|444]"
        );
        std::process::exit(2);
    }
    let input = std::path::Path::new(&args[1]);
    let iters: u64 = args[2].parse().expect("iters");
    let mode = args[3].as_str();
    let quality: f32 = args
        .get(4)
        .map(|s| s.parse().expect("quality"))
        .unwrap_or(50.0);
    let speed: u8 = args.get(5).map(|s| s.parse().expect("speed")).unwrap_or(2);
    let chroma = match args.get(6).map(String::as_str) {
        None | Some("444") => zenavif::EncodeChromaSubsampling::Yuv444,
        Some("420") => zenavif::EncodeChromaSubsampling::Yuv420,
        Some(other) => panic!("bad chroma {other} (420|444)"),
    };

    let img = load_png_rgb(input);
    let config = EncoderConfig::new()
        .quality(quality)
        .speed(speed)
        .bit_depth(zenavif::EncodeBitDepth::Eight)
        .chroma_subsampling(chroma);
    let stop = StopToken::new(Unstoppable);

    let heartbeat = |i: u64, note: &str| {
        println!("iter {i} {note}");
        std::io::stdout().flush().unwrap();
    };

    match mode {
        "fast" => {
            let avif = encode(img.as_ref(), &config, &stop);
            heartbeat(0, &format!("setup encode {} bytes", avif.len()));
            for i in 1..=iters {
                let dec = decode(&avif, &stop);
                let (s, p3) = butter(img.as_ref(), dec.as_ref());
                heartbeat(i, &format!("ok {s:.4} {p3:.4}"));
            }
        }
        "full" => {
            for i in 1..=iters {
                let avif = encode(img.as_ref(), &config, &stop);
                let dec = decode(&avif, &stop);
                let (s, p3) = butter(img.as_ref(), dec.as_ref());
                let avif2 = encode(img.as_ref(), &config, &stop);
                heartbeat(
                    i,
                    &format!("ok {s:.4} {p3:.4} {}b {}b", avif.len(), avif2.len()),
                );
            }
        }
        "decode" => {
            let avif = encode(img.as_ref(), &config, &stop);
            heartbeat(0, &format!("setup encode {} bytes", avif.len()));
            for i in 1..=iters {
                let dec = decode(&avif, &stop);
                heartbeat(i, &format!("ok {}x{}", dec.width(), dec.height()));
            }
        }
        "butter" => {
            let avif = encode(img.as_ref(), &config, &stop);
            let dec = decode(&avif, &stop);
            heartbeat(0, "setup done");
            for i in 1..=iters {
                let (s, p3) = butter(img.as_ref(), dec.as_ref());
                heartbeat(i, &format!("ok {s:.4} {p3:.4}"));
            }
        }
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(2);
        }
    }
    println!("done");
}

//! What the AVIF container costs on top of the AV1 payload.
//!
//! The sweep discipline asks for `total = alpha + beta * pixels` on the BYTE
//! axis as well as the time axis, and for the intercept to be reported. For a
//! shipped AVIF the intercept has two parts: the AV1 bitstream's own fixed
//! headers (measured by `scripts/encode_rd/bytes_model.py` over `bytes_av1`)
//! and the ISOBMFF boxes this crate writes around them. This measures the
//! second part with the muxer zenavif actually ships, not an estimate.
//!
//! It muxes an existing AV1 payload — no encoding — so it can be run over the
//! artifacts an RD sweep already persisted.
//!
//!     cargo run -p zenavif-serialize --example container_cost -- <w> <h> <file.obu|file.ivf>...
//!
//! Prints one TSV row per input: payload bytes, AVIF bytes, the difference, and
//! the difference expressed as bpp at that resolution — which is the number
//! that matters, because a fixed box overhead is a large bitrate on a thumbnail
//! and nothing at 4K.

use std::path::Path;

/// Strip an IVF wrapper if present; a bare OBU stream passes through.
/// Mirrors `run_grid.py`'s `payload_bytes`, deliberately.
fn payload(d: &[u8]) -> Vec<u8> {
    if d.len() >= 32 && &d[0..4] == b"DKIF" {
        let mut off = 32usize;
        let mut out = Vec::new();
        while off + 12 <= d.len() {
            let sz = u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]]) as usize;
            off += 12;
            if off + sz > d.len() {
                break;
            }
            out.extend_from_slice(&d[off..off + sz]);
            off += sz;
        }
        return out;
    }
    d.to_vec()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: container_cost <width> <height> <bitstream>...");
        std::process::exit(2);
    }
    let w: u32 = args[0].parse().expect("width");
    let h: u32 = args[1].parse().expect("height");

    println!("file\tw\th\tpayload_B\tavif_B\tcontainer_B\tcontainer_bpp");
    for f in &args[2..] {
        let raw = match std::fs::read(Path::new(f)) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skip {f}: {e}");
                continue;
            }
        };
        let p = payload(&raw);
        let mut out = Vec::new();
        // Defaults only: no ICC, no EXIF, no XMP, no alpha, no gain map. That is
        // the floor of what a still AVIF costs — anything a caller attaches is
        // additive on top and is its own measurement.
        if let Err(e) = zenavif_serialize::serialize(&mut out, &p, None, w, h, 8) {
            eprintln!("skip {f}: mux failed: {e}");
            continue;
        }
        let overhead = out.len() as i64 - p.len() as i64;
        let bpp = overhead as f64 * 8.0 / (w as f64 * h as f64);
        println!(
            "{}\t{w}\t{h}\t{}\t{}\t{overhead}\t{bpp:.5}",
            Path::new(f).file_name().unwrap_or_default().to_string_lossy(),
            p.len(),
            out.len()
        );
    }
}

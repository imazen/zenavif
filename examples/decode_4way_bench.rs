//! Apples-to-apples AV1 KEY-frame decode benchmark: **zenav1-aom vs rav1d-safe**,
//! both driven through zenavif's raw-OBU decode seam
//! ([`zenavif::decode_av1_obu_yuv`]) so only the decode kernel differs.
//!
//! # Same scope (critical)
//!
//! Both decoders receive the **identical** raw AV1 OBU bytes of **frame 0 (the
//! first, KEY frame)** of each vector, demuxed once by the shared IVF reader
//! here, and produce the same tight-YUV [`zenavif::DecodedYuv`] output. First-
//! KEY-frame-for-both is the only honest common scope because the zenav1-aom
//! decoder is KEY-frame / intra-only (an AVIF still IS a single KEY frame).
//!
//! The two Rust decoders are measured **interleaved** (round-robin) by zenbench
//! so shared-box load cancels out — that interleaved ratio is the trustworthy
//! headline number. The C references (aomdec, dav1d) are timed separately, as
//! back-to-back processes, and are reported with a cross-harness caveat.
//!
//! # Correctness gate
//!
//! Before timing, each cell is decoded by both backends and the YUV planes are
//! compared byte-for-byte. A divergence is a real correctness finding (both are
//! conformant AV1 decoders on these streams), reported per cell — never
//! silently dropped.
//!
//! Usage:
//!   decode_4way_bench <corpus_dir> <out_csv>
//! (defaults: /root/zenav1-aom/conformance/data  /tmp/decode_rust.csv)

use std::fs;
use std::path::PathBuf;
use zenavif::{DecodeBackend, DecodedYuv, decode_av1_obu_yuv};
use zenbench::prelude::*;

/// One benchmark cell: a single KEY-frame AV1 stream.
struct Cell {
    label: String,
    w: usize,
    h: usize,
    /// Raw OBU bytes of frame 0 (the KEY frame), demuxed from the IVF.
    obu: Vec<u8>,
    note: &'static str,
    /// Correctness: do the two backends agree byte-for-byte?
    correctness: String,
}

/// IVF display dims (header bytes 12..16, little-endian).
fn ivf_hdr_dims(data: &[u8]) -> (usize, usize) {
    (
        u16::from_le_bytes([data[12], data[13]]) as usize,
        u16::from_le_bytes([data[14], data[15]]) as usize,
    )
}

/// Split an IVF container into per-frame temporal-unit payloads (raw OBU bytes)
/// and return the FIRST one (frame 0 = the KEY frame).
fn ivf_first_tu(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 32 || &data[0..4] != b"DKIF" {
        return None;
    }
    let hdr_len = u16::from_le_bytes([data[6], data[7]]) as usize;
    let mut off = hdr_len;
    if off + 12 > data.len() {
        return None;
    }
    let sz = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]) as usize;
    off += 12; // 4-byte size + 8-byte timestamp
    if off + sz > data.len() {
        return None;
    }
    Some(data[off..off + sz].to_vec())
}

/// (label, filename, note). Small conformance cells (352x288) under-weight
/// fixed overhead; the 2K/4K photographic mosaics are the headline stills-decode
/// workload; the 1920x1080 intrabc vector is real conformance content at 2K.
fn cell_defs() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "small-352x288-q00",
            "av1-1-b8-00-quantizer-00.ivf",
            "conformance KEY frame0 (KEY,INTER vector; frame0 only)",
        ),
        (
            "small-352x288-q32",
            "av1-1-b8-00-quantizer-32.ivf",
            "conformance KEY frame0",
        ),
        (
            "small-352x288-q63",
            "av1-1-b8-00-quantizer-63.ivf",
            "conformance KEY frame0 (aggressive q; >64-block path)",
        ),
        (
            "small-640x360-b10-q00",
            "av1-1-b10-00-quantizer-00.ivf",
            "conformance 10-bit KEY frame0 (high-bitdepth path)",
        ),
        (
            "small-640x360-b10-q32",
            "av1-1-b10-00-quantizer-32.ivf",
            "conformance 10-bit KEY frame0 (high-bitdepth path)",
        ),
        (
            "2K-1920x1080-conf-intrabc",
            "av1-1-b8-16-intra_only-intrabc-extreme-dv.ivf",
            "conformance intra KEY frame0 (screen-content/intrabc, real bitstream)",
        ),
        (
            "2K-1920x1080-photo-cq20",
            "mosaic-2k-cq20.ivf",
            "aomenc allintra KEY, photographic gb82 25-tile mosaic",
        ),
        (
            "2K-1920x1080-photo-cq40",
            "mosaic-2k-cq40.ivf",
            "aomenc allintra KEY, photographic gb82 25-tile mosaic",
        ),
        (
            "4K-3840x2160-photo-cq20",
            "mosaic-4k-cq20.ivf",
            "aomenc allintra KEY, photographic gb82 25-tile mosaic",
        ),
        (
            "4K-3840x2160-photo-cq40",
            "mosaic-4k-cq40.ivf",
            "aomenc allintra KEY, photographic gb82 25-tile mosaic",
        ),
    ]
}

/// Byte-compare two decodes; return a short correctness verdict.
fn compare(a: &DecodedYuv, r: &DecodedYuv) -> String {
    if a.width != r.width || a.height != r.height {
        return format!(
            "DIM-MISMATCH aom {}x{} vs rav1d {}x{}",
            a.width, a.height, r.width, r.height
        );
    }
    let first_diff = |x: &[u16], y: &[u16]| -> Option<usize> {
        if x.len() != y.len() {
            return Some(usize::MAX);
        }
        x.iter().zip(y).position(|(p, q)| p != q)
    };
    if let Some(i) = first_diff(&a.y, &r.y) {
        return format!("LUMA-DIVERGE at {i}");
    }
    if let Some(i) = first_diff(&a.u, &r.u) {
        return format!("U-DIVERGE at {i}");
    }
    if let Some(i) = first_diff(&a.v, &r.v) {
        return format!("V-DIVERGE at {i}");
    }
    "byte-identical(aom==rav1d)".to_string()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| "/root/zenav1-aom/conformance/data".to_string()),
    );
    let out_csv = PathBuf::from(
        args.get(2)
            .cloned()
            .unwrap_or_else(|| "/tmp/decode_rust.csv".to_string()),
    );

    // ---- Load cells + correctness gate -----------------------------------
    let mut cells: Vec<Cell> = Vec::new();
    for (label, fname, note) in cell_defs() {
        let path = dir.join(fname);
        let data = match fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("!! cell {label}: cannot read {path:?}: {e} — SKIPPED");
                continue;
            }
        };
        let (w, h) = ivf_hdr_dims(&data);
        let obu = match ivf_first_tu(&data) {
            Some(t) => t,
            None => {
                eprintln!("!! cell {label}: not a valid IVF ({path:?}) — SKIPPED");
                continue;
            }
        };

        // Correctness: decode both backends once, compare.
        let a = decode_av1_obu_yuv(&obu, DecodeBackend::Zenav1Aom);
        let r = decode_av1_obu_yuv(&obu, DecodeBackend::Rav1dSafe);
        #[allow(unused_mut)]
        let mut correctness = match (&a, &r) {
            (Ok(a), Ok(r)) => compare(a, r),
            (Err(_), _) => "zenav1-aom REJECTED frame".to_string(),
            (_, Err(_)) => "rav1d-safe REJECTED frame".to_string(),
        };
        // Third arm (rav1d FFI, full asm): must byte-agree with rav1d-safe.
        #[cfg(feature = "unsafe-asm")]
        {
            let f = decode_av1_obu_yuv(&obu, DecodeBackend::Rav1dFfi);
            correctness = match (&f, &r) {
                (Ok(f), Ok(r)) if compare(f, r) == "byte-identical(aom==rav1d)" => {
                    format!("{correctness}+ffi-identical")
                }
                (Ok(f), Ok(r)) => format!("{correctness}; ffi: {}", compare(f, r)),
                (Err(_), _) => format!("{correctness}; rav1d-ffi REJECTED frame"),
                _ => correctness,
            };
        }
        eprintln!("cell {label:28} {w}x{h}  {correctness}");
        cells.push(Cell {
            label: label.to_string(),
            w,
            h,
            obu,
            note,
            correctness,
        });
    }
    if cells.is_empty() {
        eprintln!("no cells loaded; nothing to benchmark");
        std::process::exit(1);
    }

    // ---- Interleaved Rust-pair benchmark (gate disabled: run under load; --
    // ---- zenbench interleaving cancels the load bias) --------------------
    let result = zenbench::run_gated(GateConfig::disabled(), |suite| {
        for cell in &cells {
            let px = (cell.w * cell.h) as u64;
            let label = cell.label.clone();
            let obu_a = cell.obu.clone();
            let obu_r = cell.obu.clone();
            #[cfg(feature = "unsafe-asm")]
            let obu_f = cell.obu.clone();
            suite.group(label, move |g| {
                g.throughput(Throughput::Elements(px));
                g.bench("zenav1-aom", move |b| {
                    b.iter(|| {
                        let d = decode_av1_obu_yuv(&obu_a, DecodeBackend::Zenav1Aom).unwrap();
                        black_box(d.y.len())
                    })
                });
                g.bench("rav1d-safe", move |b| {
                    b.iter(|| {
                        let d = decode_av1_obu_yuv(&obu_r, DecodeBackend::Rav1dSafe).unwrap();
                        black_box(d.y.len())
                    })
                });
                #[cfg(feature = "unsafe-asm")]
                {
                    let obu_f = obu_f.clone();
                    g.bench("rav1d-ffi-asm", move |b| {
                        b.iter(|| {
                            let d = decode_av1_obu_yuv(&obu_f, DecodeBackend::Rav1dFfi).unwrap();
                            black_box(d.y.len())
                        })
                    });
                }
            });
        }
    });

    // ---- Extract + write CSV (Rust rows) ---------------------------------
    let mut csv = String::new();
    csv.push_str(
        "cell,width,height,megapixels,decoder,scope,min_ms_per_frame,mean_ms_per_frame,\
         mpx_s_min,mpx_s_mean,correctness,note\n",
    );
    // Human summary to stderr.
    eprintln!(
        "\n{:<28} {:>9} {:>12} {:>12}   aom/rav1d",
        "cell", "decoder", "mean ms/fr", "Mpx/s(mean)"
    );
    for cell in &cells {
        let comp = result
            .comparisons
            .iter()
            .find(|c| c.group_name == cell.label);
        let (mut aom_mean, mut rav_mean, mut ffi_mean) = (f64::NAN, f64::NAN, f64::NAN);
        if let Some(comp) = comp {
            let px = (cell.w * cell.h) as f64;
            for b in &comp.benchmarks {
                let min_ns = b.summary.min;
                let mean_ns = b.summary.mean;
                let mpx_min = px * 1e3 / min_ns; // px / (min_ns*1e-9) / 1e6
                let mpx_mean = px * 1e3 / mean_ns;
                let mp = px / 1e6;
                csv.push_str(&format!(
                    "{},{},{},{:.4},{},first-KEY,{:.4},{:.4},{:.2},{:.2},{},{}\n",
                    cell.label,
                    cell.w,
                    cell.h,
                    mp,
                    b.name,
                    min_ns / 1e6,
                    mean_ns / 1e6,
                    mpx_min,
                    mpx_mean,
                    cell.correctness,
                    cell.note,
                ));
                if b.name == "zenav1-aom" {
                    aom_mean = mean_ns;
                }
                if b.name == "rav1d-safe" {
                    rav_mean = mean_ns;
                }
                if b.name == "rav1d-ffi-asm" {
                    ffi_mean = mean_ns;
                }
            }
        }
        let ratio = aom_mean / rav_mean;
        let px = (cell.w * cell.h) as f64;
        eprintln!(
            "{:<28} {:>9} {:>12.3} {:>12.2}   {:.3}x",
            cell.label,
            "zenav1-aom",
            aom_mean / 1e6,
            px * 1e3 / aom_mean,
            ratio
        );
        eprintln!(
            "{:<28} {:>9} {:>12.3} {:>12.2}",
            "",
            "rav1d-safe",
            rav_mean / 1e6,
            px * 1e3 / rav_mean
        );
        if ffi_mean.is_finite() {
            eprintln!(
                "{:<28} {:>9} {:>12.3} {:>12.2}   safe/asm {:.3}x",
                "",
                "ffi-asm",
                ffi_mean / 1e6,
                px * 1e3 / ffi_mean,
                rav_mean / ffi_mean
            );
        }
    }

    fs::write(&out_csv, &csv).expect("write csv");
    eprintln!("\nRust-pair CSV written: {out_csv:?}");
    eprintln!(
        "(headline = interleaved zenav1-aom÷rav1d-safe ratio; C references appended separately)"
    );
}

//! Decode an IVF-contained (or bare-OBU) AV1 still frame via rav1d-safe and
//! dump the raw planar pixels — byte-compatible with
//! `aomdec --rawvideo -o out.raw in.ivf`, so `md5sum` of the two outputs is
//! a decoder byte-agreement check (the conformance protocol for
//! palette-armed and HDR encodes: aomdec must decode cleanly AND byte-agree
//! with rav1d-safe; see docs/RD_GAP_VS_LIBAOM.md "IMPLEMENTED 2026-07-03:
//! palette mode").
//!
//! Usage: ivf_raw <in.ivf|in.obu> <out.raw>
//!
//! Output layout: Y rows (visible width), then U, then V (subsampled dims
//! ceil(w/2) x ceil(h/2) for I420; full for I444; absent for I400).
//! 8-bit: 1 byte/sample; 10/12-bit: 2 bytes/sample little-endian
//! (matches aomdec's high-bitdepth `--rawvideo` output on LE hosts).

use rav1d_safe::src::managed::{Decoder, Frame, PixelLayout, Planes, Settings};
use std::io::Write;
use std::process::ExitCode;

fn decode_all(data: &[u8]) -> Result<Frame, String> {
    let mut settings = Settings::default();
    settings.threads = 1;
    let mut dec = Decoder::with_settings(settings).map_err(|e| format!("decoder init: {e:?}"))?;

    let mut frames: Vec<Frame> = Vec::new();
    let mut feed = |payload: &[u8], frames: &mut Vec<Frame>| -> Result<(), String> {
        match dec.decode(payload) {
            Ok(Some(f)) => {
                frames.push(f);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(e) => Err(format!("decode: {e:?}")),
        }
    };

    if data.len() >= 32 && &data[0..4] == b"DKIF" {
        // IVF: 32-byte file header, then per frame: u32le size + u64le pts.
        let mut off = 32usize;
        while off + 12 <= data.len() {
            let sz = u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                as usize;
            off += 12;
            if off + sz > data.len() {
                return Err("truncated IVF frame".into());
            }
            feed(&data[off..off + sz], &mut frames)?;
            off += sz;
        }
    } else {
        feed(data, &mut frames)?;
    }
    // Drain delayed frames.
    if let Ok(mut fl) = dec.flush() {
        frames.append(&mut fl);
    }
    frames
        .into_iter()
        .last()
        .ok_or_else(|| "no frames decoded".into())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("usage: ivf_raw <in.ivf|in.obu> <out.raw>");
        return ExitCode::FAILURE;
    };
    let data = match std::fs::read(input) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("read {input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let frame = match decode_all(&data) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("FAIL {input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let layout = frame.pixel_layout();
    let (cw, ch) = match layout {
        PixelLayout::I400 => (0, 0),
        PixelLayout::I420 => (w.div_ceil(2), h.div_ceil(2)),
        PixelLayout::I422 => (w.div_ceil(2), h),
        PixelLayout::I444 => (w, h),
    };

    // 8-bit: 1 byte/sample. 10/12-bit: 2 bytes/sample little-endian —
    // matching `aomdec --rawvideo` high-bitdepth output on LE hosts.
    let mut out = Vec::with_capacity((w * h + 2 * cw * ch) * 2);
    match frame.planes() {
        Planes::Depth8(planes) => {
            for row in planes.y().rows().take(h) {
                out.extend_from_slice(&row[..w]);
            }
            if cw > 0 {
                for view in [planes.u(), planes.v()] {
                    let Some(p) = view else {
                        eprintln!("FAIL {input}: missing chroma plane for {layout:?}");
                        return ExitCode::FAILURE;
                    };
                    for row in p.rows().take(ch) {
                        out.extend_from_slice(&row[..cw]);
                    }
                }
            }
        }
        Planes::Depth16(planes) => {
            for row in planes.y().rows().take(h) {
                for &v in &row[..w] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            if cw > 0 {
                for view in [planes.u(), planes.v()] {
                    let Some(p) = view else {
                        eprintln!("FAIL {input}: missing chroma plane for {layout:?}");
                        return ExitCode::FAILURE;
                    };
                    for row in p.rows().take(ch) {
                        for &v in &row[..cw] {
                            out.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                }
            }
        }
    }
    let mut f = match std::fs::File::create(output) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("create {output}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = f.write_all(&out) {
        eprintln!("write {output}: {e}");
        return ExitCode::FAILURE;
    }
    println!("{}x{} {:?} {} bytes", w, h, layout, out.len());
    ExitCode::SUCCESS
}

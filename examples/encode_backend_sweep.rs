//! Cross-backend encode sweep: zenravif (zenrav1e) vs svtav1-rs, RD + speed.
//!
//! For every (image, size, speed, quality, backend) cell this harness:
//!  1. encodes RGB8 -> AVIF through `zenavif::EncoderConfig` with the
//!     backend selected via `.backend(Av1Backend::..)` (identical frontend,
//!     different AV1 encoder — the "one frontend, two backends" discipline
//!     the decode seam already uses);
//!  2. decodes the AVIF back with zenavif's rav1d-safe decoder and scores
//!     SSIMULACRA2 against the source;
//!  3. (feature `aom-backend`) extracts the AV1 OBU payload and decodes it
//!     with BOTH `DecodeBackend::Rav1dSafe` and `DecodeBackend::AomRs`,
//!     byte-comparing the planes — every encoded cell doubles as a
//!     cross-decoder conformance cell on bitstreams *our* encoders produced
//!     (the conformance corpus only ever covered aomenc output);
//!  4. writes one TSV row.
//!
//! With `--reps N` (N > 1) the encode is repeated N times per cell with the
//! backends interleaved (A,B,A,B..) and the minimum wall time recorded —
//! run that mode with `--threads 1` on a quiet box for timing headlines.
//! The default parallel mode is for RD curves; its encode_ms column is
//! load-polluted and labeled as such in the meta file.
//!
//! Usage:
//! ```text
//! cargo run --release --example encode_backend_sweep \
//!   --features encode,encode-svt-rs,aom-backend -- \
//!   --corpus /root/codec-corpus/CID22/CID22-512/validation \
//!   --images 8 --sizes 256,512,1024 --qualities 5..=100:5 \
//!   --speeds 6 --threads 8 --out /tmp/backend_sweep.tsv
//! ```

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgRef, ImgVec};
use rgb::Rgb;

use zenavif::EncodeChromaSubsampling;
use zenavif::{Av1Backend, EncoderConfig};

/// One comparison arm: a backend at a fixed chroma subsampling. Both
/// backends run 4:2:0 for the apples-to-apples RD comparison (SvtRs encodes
/// 4:2:0 only); the zenravif 4:4:4 arm is its shipped default, kept as a
/// reference curve.
#[derive(Clone, Copy)]
struct Arm {
    label: &'static str,
    backend: Av1Backend,
    subsampling: EncodeChromaSubsampling,
}

#[derive(Clone)]
struct Args {
    corpus: PathBuf,
    images: usize,
    sizes: Vec<usize>,
    qualities: Vec<f32>,
    speeds: Vec<u8>,
    reps: usize,
    threads: usize,
    out: PathBuf,
}

fn parse_qualities(s: &str) -> Result<Vec<f32>, String> {
    if let Some((range, step)) = s.split_once(':') {
        let (a, b) = range
            .split_once("..=")
            .ok_or("quality range must be a..=b:step")?;
        let (a, b, step): (u32, u32, u32) = (
            a.parse().map_err(|_| "bad range start")?,
            b.parse().map_err(|_| "bad range end")?,
            step.parse().map_err(|_| "bad step")?,
        );
        Ok((a..=b).step_by(step as usize).map(|q| q as f32).collect())
    } else {
        s.split(',')
            .map(|q| {
                q.trim()
                    .parse::<f32>()
                    .map_err(|_| "bad quality".to_string())
            })
            .collect()
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        corpus: PathBuf::from("/root/codec-corpus/CID22/CID22-512/validation"),
        images: 8,
        sizes: vec![256, 512, 1024],
        qualities: parse_qualities("5..=100:5").unwrap(),
        speeds: vec![6],
        reps: 1,
        threads: 8,
        out: PathBuf::from("/tmp/backend_sweep.tsv"),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--corpus" => args.corpus = PathBuf::from(val()?),
            "--images" => args.images = val()?.parse().map_err(|_| "bad --images")?,
            "--sizes" => {
                args.sizes = val()?
                    .split(',')
                    .map(|s| s.trim().parse().map_err(|_| "bad size".to_string()))
                    .collect::<Result<_, _>>()?
            }
            "--qualities" => args.qualities = parse_qualities(&val()?)?,
            "--speeds" => {
                args.speeds = val()?
                    .split(',')
                    .map(|s| s.trim().parse().map_err(|_| "bad speed".to_string()))
                    .collect::<Result<_, _>>()?
            }
            "--reps" => args.reps = val()?.parse().map_err(|_| "bad --reps")?,
            "--threads" => args.threads = val()?.parse().map_err(|_| "bad --threads")?,
            "--out" => args.out = PathBuf::from(val()?),
            other => return Err(format!("unknown arg {other}")),
        }
    }
    Ok(args)
}

fn load_rgb8(path: &PathBuf) -> Result<ImgVec<Rgb<u8>>, String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let buf: Vec<Rgb<u8>> = rgb
        .pixels()
        .map(|p| Rgb {
            r: p.0[0],
            g: p.0[1],
            b: p.0[2],
        })
        .collect();
    Ok(Img::new(buf, w, h))
}

/// Center-crop to `size` x `size` (caller guarantees the source is larger).
fn center_crop(src: ImgRef<'_, Rgb<u8>>, size: usize) -> ImgVec<Rgb<u8>> {
    let x0 = (src.width() - size) / 2;
    let y0 = (src.height() - size) / 2;
    let mut buf = Vec::with_capacity(size * size);
    for row in src.rows().skip(y0).take(size) {
        buf.extend_from_slice(&row[x0..x0 + size]);
    }
    Img::new(buf, size, size)
}

/// 2x2 mosaic of four equally-sized tiles.
fn mosaic_2x2(tiles: &[ImgVec<Rgb<u8>>]) -> ImgVec<Rgb<u8>> {
    let t = tiles[0].width();
    let side = t * 2;
    let mut buf = vec![Rgb { r: 0, g: 0, b: 0 }; side * side];
    for (i, tile) in tiles.iter().take(4).enumerate() {
        let (tx, ty) = ((i % 2) * t, (i / 2) * t);
        for (y, row) in tile.rows().enumerate() {
            let dst = (ty + y) * side + tx;
            buf[dst..dst + t].copy_from_slice(row);
        }
    }
    Img::new(buf, side, side)
}

fn ssim2(a: ImgRef<'_, Rgb<u8>>, b: ImgRef<'_, Rgb<u8>>) -> Result<f64, String> {
    let tri = |src: ImgRef<'_, Rgb<u8>>| -> ImgVec<[u8; 3]> {
        let mut out = Vec::with_capacity(src.width() * src.height());
        for row in src.rows() {
            out.extend(row.iter().map(|p| [p.r, p.g, p.b]));
        }
        Img::new(out, src.width(), src.height())
    };
    let (ta, tb) = (tri(a), tri(b));
    fast_ssim2::compute_ssimulacra2(ta.as_ref(), tb.as_ref()).map_err(|e| e.to_string())
}

/// Cross-decoder gate: decode the AV1 payload with rav1d-safe and aom-rs
/// through the seam and byte-compare the planes. Returns a status string for
/// the TSV (`identical`, `aom-unsupported:<..>`, or a divergence label).
#[cfg(feature = "aom-backend")]
fn aom_cross_gate(avif: &[u8]) -> String {
    use zenavif::{DecodeBackend, decode_av1_obu_yuv};
    // Lenient on purpose: a corpus file with a container quirk should still
    // yield a measurement cell rather than dropping out of the sweep. Production
    // decode is strict -- see tests/parser_leniency_scope.rs.
    let cfg = zenavif_parse::DecodeConfig::default().lenient(true);
    let parser = match zenavif_parse::AvifParser::from_owned_with_config(
        avif.to_vec(),
        &cfg,
        &Unstoppable,
    ) {
        Ok(p) => p,
        Err(e) => return format!("container-parse-error:{e}"),
    };
    let payload = match parser.primary_data() {
        Ok(p) => p.as_ref().to_vec(),
        Err(e) => return format!("no-primary-item:{e}"),
    };
    let rav = match decode_av1_obu_yuv(&payload, DecodeBackend::Rav1dSafe) {
        Ok(d) => d,
        Err(e) => return format!("rav1d-error:{e}"),
    };
    // aom-decode wants a full temporal unit; AVIF item payloads may omit the
    // temporal delimiter OBU — retry with one prepended before failing.
    let aom = match decode_av1_obu_yuv(&payload, DecodeBackend::AomRs) {
        Ok(d) => d,
        Err(_) => {
            let mut with_td = vec![0x12, 0x00];
            with_td.extend_from_slice(&payload);
            match decode_av1_obu_yuv(&with_td, DecodeBackend::AomRs) {
                Ok(d) => d,
                Err(e) => return format!("aom-error:{e}"),
            }
        }
    };
    if (rav.width, rav.height, rav.width_uv, rav.height_uv)
        != (aom.width, aom.height, aom.width_uv, aom.height_uv)
    {
        return "DIM-MISMATCH".to_string();
    }
    if rav.y != aom.y {
        return "LUMA-DIVERGE".to_string();
    }
    if rav.u != aom.u || rav.v != aom.v {
        return "CHROMA-DIVERGE".to_string();
    }
    "identical".to_string()
}

#[cfg(not(feature = "aom-backend"))]
fn aom_cross_gate(_avif: &[u8]) -> String {
    "aom-backend-off".to_string()
}

struct Cell {
    image: String,
    size_label: String,
    speed: u8,
    quality: f32,
}

fn run_cell(
    reference: ImgRef<'_, Rgb<u8>>,
    cell: &Cell,
    arm: Arm,
    reps: usize,
) -> Result<String, String> {
    let config = EncoderConfig::new()
        .quality(cell.quality)
        .speed(cell.speed)
        .chroma_subsampling(arm.subsampling)
        .backend(arm.backend);
    let mut best_ms = f64::MAX;
    let mut encoded = None;
    for _ in 0..reps.max(1) {
        let t0 = Instant::now();
        let enc = zenavif::encode_rgb8(reference, &config, StopToken::new(Unstoppable))
            .map_err(|e| format!("encode: {e}"))?;
        best_ms = best_ms.min(t0.elapsed().as_secs_f64() * 1e3);
        encoded = Some(enc);
    }
    let enc = encoded.unwrap();
    let bytes = enc.avif_file.len();

    let t1 = Instant::now();
    let decoded = zenavif::decode(&enc.avif_file).map_err(|e| format!("decode: {e}"))?;
    let decode_ms = t1.elapsed().as_secs_f64() * 1e3;
    let dec_img: ImgRef<'_, Rgb<u8>> = decoded
        .try_as_imgref::<Rgb<u8>>()
        .ok_or("decoded image not RGB8-viewable")?;
    let score = ssim2(reference, dec_img)?;
    let gate = aom_cross_gate(&enc.avif_file);

    Ok(format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{:.1}\t{}\t{:.2}\t{:.2}\t{:.4}\t{}",
        cell.image,
        cell.size_label,
        reference.width(),
        reference.height(),
        arm.label,
        cell.speed,
        cell.quality,
        bytes,
        best_ms,
        decode_ms,
        score,
        gate,
    ))
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("arg error: {e}");
            std::process::exit(2);
        }
    };

    let mut paths: Vec<PathBuf> = fs::read_dir(&args.corpus)
        .expect("corpus dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
        .collect();
    paths.sort();
    paths.truncate(args.images.max(4));

    let sources: Vec<(String, ImgVec<Rgb<u8>>)> = paths
        .iter()
        .map(|p| {
            (
                p.file_stem().unwrap().to_string_lossy().to_string(),
                load_rgb8(p).expect("load png"),
            )
        })
        .collect();

    // Build the (reference image, cell metadata) work list.
    let mut work: Vec<(ImgVec<Rgb<u8>>, Cell)> = Vec::new();
    for size in &args.sizes {
        match *size {
            1024 => {
                // 2x2 mosaics of consecutive 512 sources: one per 4 images.
                for (mi, group) in sources.chunks(4).enumerate() {
                    if group.len() < 4 {
                        continue;
                    }
                    let tiles: Vec<ImgVec<Rgb<u8>>> =
                        group.iter().map(|(_, im)| im.clone()).collect();
                    let mosaic = mosaic_2x2(&tiles);
                    for &speed in &args.speeds {
                        for &q in &args.qualities {
                            work.push((
                                mosaic.clone(),
                                Cell {
                                    image: format!("mosaic{mi}"),
                                    size_label: "1024".into(),
                                    speed,
                                    quality: q,
                                },
                            ));
                        }
                    }
                }
            }
            s if s <= 512 => {
                for (name, im) in &sources {
                    let var = if s == 512 {
                        im.clone()
                    } else {
                        center_crop(im.as_ref(), s)
                    };
                    for &speed in &args.speeds {
                        for &q in &args.qualities {
                            work.push((
                                var.clone(),
                                Cell {
                                    image: name.clone(),
                                    size_label: s.to_string(),
                                    speed,
                                    quality: q,
                                },
                            ));
                        }
                    }
                }
            }
            other => eprintln!("skipping unsupported size {other}"),
        }
    }

    let backends: Vec<Arm> = {
        // `mut` only when a cfg-gated arm below pushes onto it; without those
        // features the vec is complete as written and clippy rejects the mut.
        #[allow(unused_mut)]
        let mut b = vec![
            Arm {
                label: "zenravif-420",
                backend: Av1Backend::Zenravif,
                subsampling: EncodeChromaSubsampling::Yuv420,
            },
            Arm {
                label: "zenravif-444",
                backend: Av1Backend::Zenravif,
                subsampling: EncodeChromaSubsampling::Yuv444,
            },
        ];
        #[cfg(feature = "encode-svt-rs")]
        b.push(Arm {
            label: "svt-rs-420",
            backend: Av1Backend::SvtRs,
            subsampling: EncodeChromaSubsampling::Yuv420,
        });
        b
    };

    eprintln!(
        "sweep: {} cells x {} backends ({} rows), {} threads, reps={}",
        work.len(),
        backends.len(),
        work.len() * backends.len(),
        args.threads,
        args.reps
    );

    let header = "image\tsize\twidth\theight\tbackend\tspeed\tquality\tbytes\tencode_ms\tdecode_ms\tssim2\taom_gate";
    let rows: Vec<String> = if args.threads <= 1 || args.reps > 1 {
        // Serial, backend-interleaved within each cell (timing mode).
        let mut rows = Vec::new();
        for (reference, cell) in &work {
            for &arm in &backends {
                match run_cell(reference.as_ref(), cell, arm, args.reps) {
                    Ok(row) => rows.push(row),
                    Err(e) => eprintln!(
                        "CELLFAIL {} {} {} q{}: {e}",
                        cell.image, cell.size_label, arm.label, cell.quality
                    ),
                }
            }
        }
        rows
    } else {
        use rayon::prelude::*;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(args.threads)
            .stack_size(32 * 1024 * 1024)
            .build()
            .expect("rayon pool");
        pool.install(|| {
            work.par_iter()
                .flat_map(|(reference, cell)| {
                    backends
                        .iter()
                        .filter_map(|&arm| match run_cell(reference.as_ref(), cell, arm, 1) {
                            Ok(row) => Some(row),
                            Err(e) => {
                                eprintln!(
                                    "CELLFAIL {} {} {} q{}: {e}",
                                    cell.image, cell.size_label, arm.label, cell.quality
                                );
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        })
    };

    let mut f = fs::File::create(&args.out).expect("open out");
    writeln!(f, "{header}").unwrap();
    for r in &rows {
        writeln!(f, "{r}").unwrap();
    }
    let n_ident = rows.iter().filter(|r| r.ends_with("identical")).count();
    eprintln!(
        "wrote {} rows to {} ({} aom-gate identical)",
        rows.len(),
        args.out.display(),
        n_ident
    );
}

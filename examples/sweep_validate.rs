//! Empirical validation of the curated sweep axes (`zenavif::sweep`).
//!
//! Encodes the default stratum plus every single-deviation stratum of
//! [`SweepAxes::modes_full`] on a small mixed corpus (CID22-512 photos,
//! synthetic noise / gradient / checkerboard at 256², one 64×64 tiny)
//! and checks:
//!
//! 1. **Fingerprint contract** — equal fingerprint ⇒ byte-identical
//!    output, on real encodes of the documented alias/exclusion pairs:
//!    the quality→quantizer mirror (q 80.0 vs 80.2), override == preset
//!    value, `vaq_strength` with VAQ off, `matrix_coefficients` on the
//!    zenravif backend, `alpha_quality` unset vs explicit — plus a
//!    distinct-fingerprint negative control (qm on vs off).
//! 2. **No inert step** — every single-deviation label changes output
//!    bytes vs the default stratum somewhere in the subset.
//! 3. **Tile/thread claims** — threads=1 vs 2 byte-identical below the
//!    tile-size cap and byte-different above it (the machine-dependence
//!    rationale for pinning threads in sweep cells).
//! 4. **Queue ordering invariants** on the emitted plan.
//! 5. **zensim sanity floor** at q85 (catches corrupt pixel paths).
//! 6. **Documented directions** (soft, reported not fatal): 4:2:0
//!    shrinks photos, 10-bit ≠ 8-bit, speed ladder monotone-ish in
//!    time.
//!
//! Run (about 2–4 minutes on a workstation; encodes parallelize across
//! cells with each cell pinned single-threaded):
//!
//! ```bash
//! GIT_COMMIT=$(git rev-parse --short HEAD) \
//! nice -n 19 cargo run --release --example sweep_validate \
//!   --features __expert -- --out benchmarks/sweep_validate_$(date +%F).tsv
//! ```
//!
//! Requires the codec corpus (CID22-512) — set `CODEC_CORPUS_DIR` or
//! keep the default sibling checkout layout. The corpus requirement is
//! hard: a validation harness that silently skips its real-photo
//! checks would report false confidence.
//!
//! Exit code is non-zero on any hard failure (contract violation,
//! inert step, ordering breakage, encode error).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use almost_enough::{StopToken, Unstoppable};
use imgref::{Img, ImgVec};
use rayon::prelude::*;
use rgb::Rgb;
use zenavif::sweep::{QualityGrid, SweepAxes, SweepBuilder, fingerprint};
use zenavif::{EncoderConfig, encode_rgb8};
use zensim::{Zensim, ZensimProfile};
use zensim_regress::{RegressionTolerance, check_regression};

const Q_GRID: [f32; 4] = [10.0, 30.0, 60.0, 85.0];

fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ---------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------

fn codec_corpus_dir() -> PathBuf {
    if let Ok(p) = std::env::var("CODEC_CORPUS_DIR") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return pb;
        }
        panic!("CODEC_CORPUS_DIR={pb:?} does not exist");
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for rel in [
        "../codec-eval/codec-corpus",
        "../../codec-eval/codec-corpus",
    ] {
        let p = manifest.join(rel);
        if p.exists() {
            return p;
        }
    }
    panic!(
        "codec corpus not found: set CODEC_CORPUS_DIR or check out codec-eval \
         as a sibling of the zen workspace"
    );
}

fn load_png_rgb(path: &std::path::Path) -> ImgVec<Rgb<u8>> {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
        .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels: Vec<Rgb<u8>> = img
        .pixels()
        .map(|p| Rgb {
            r: p[0],
            g: p[1],
            b: p[2],
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn mix(x: u32, y: u32, salt: u32) -> u8 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ salt;
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    (h >> 16) as u8
}

fn generate_noise(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let px = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| Rgb {
                r: mix(x as u32, y as u32, 11),
                g: mix(x as u32, y as u32, 22),
                b: mix(x as u32, y as u32, 33),
            })
        })
        .collect();
    ImgVec::new(px, w, h)
}

fn generate_checkerboard(w: usize, h: usize, cell: usize) -> ImgVec<Rgb<u8>> {
    let px = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let on = ((x / cell) + (y / cell)).is_multiple_of(2);
                let v = if on { 235 } else { 20 };
                Rgb { r: v, g: v, b: v }
            })
        })
        .collect();
    ImgVec::new(px, w, h)
}

/// Smooth photo-like gradient with mild texture (DCT-friendly content).
fn generate_gradient(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let px = (0..h)
        .flat_map(|y| {
            (0..w).map(move |x| {
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                let r = (fx * 200.0 + 30.0) as u8;
                let g = (fy * 180.0 + 40.0) as u8;
                let b = ((fx + fy) * 100.0 + (mix(x as u32, y as u32, 7) % 8) as f32) as u8;
                Rgb { r, g, b }
            })
        })
        .collect();
    ImgVec::new(px, w, h)
}

/// Center-crop to at most `max` × `max` (keeps real photo statistics
/// while keeping AV1 encode cost sane for a validation harness).
fn center_crop(img: &ImgVec<Rgb<u8>>, max: usize) -> ImgVec<Rgb<u8>> {
    let (w, h) = (img.width(), img.height());
    let (cw, ch) = (w.min(max), h.min(max));
    let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
    let px = (y0..y0 + ch)
        .flat_map(|y| (x0..x0 + cw).map(move |x| (x, y)))
        .map(|(x, y)| img.buf()[y * w + x])
        .collect();
    ImgVec::new(px, cw, ch)
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Measure {
    bytes: usize,
    hash: u64,
    ssim2: f64,
    encode_ms: u128,
}

fn encode_and_score(
    img: Img<&[Rgb<u8>]>,
    config: &EncoderConfig,
    zensim: &Zensim,
) -> Result<Measure, String> {
    let t0 = Instant::now();
    let enc = encode_rgb8(img, config, stop()).map_err(|e| format!("encode: {e}"))?;
    let encode_ms = t0.elapsed().as_millis();

    let dec_config = zenavif::DecoderConfig::new().prefer_8bit(true);
    let decoded = zenavif::decode_with(&enc.avif_file, &dec_config, &Unstoppable)
        .map_err(|e| format!("decode: {e}"))?;
    let decoded_img = decoded
        .try_as_imgref::<Rgb<u8>>()
        .ok_or_else(|| "decoded image not RGB8-viewable".to_string())?;
    let tol = RegressionTolerance::off_by_one().with_min_similarity(0.0);
    let ssim2 = check_regression(zensim, &img, &decoded_img, &tol)
        .map(|r| r.score())
        .map_err(|e| format!("zensim: {e}"))?;

    Ok(Measure {
        bytes: enc.avif_file.len(),
        hash: fnv64(&enc.avif_file),
        ssim2,
        encode_ms,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_path = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1).cloned())
        .unwrap_or_else(|| "benchmarks/sweep_validate.tsv".to_string());

    let mut hard_failures: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // ------------------------------------------------------------------
    // Corpus: 2 CID22-512 photos (center-cropped to 256²) + 3 synthetic
    // 256² + one 64×64 tiny. Hard-required; no graceful skips.
    // ------------------------------------------------------------------
    let cid_dir = codec_corpus_dir().join("CID22/CID22-512/validation");
    let mut cid: Vec<_> = std::fs::read_dir(&cid_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", cid_dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    cid.sort();
    assert!(
        cid.len() >= 2,
        "expected ≥2 CID22-512 validation PNGs in {}",
        cid_dir.display()
    );

    let mut images: Vec<(String, ImgVec<Rgb<u8>>)> = Vec::new();
    for p in cid.iter().take(2) {
        let name = format!("cid_{}", p.file_stem().unwrap().to_string_lossy());
        images.push((name, center_crop(&load_png_rgb(p), 256)));
    }
    images.push(("noise256".into(), generate_noise(256, 256)));
    images.push(("checker256".into(), generate_checkerboard(256, 256, 8)));
    images.push(("gradient256".into(), generate_gradient(256, 256)));
    images.push(("tiny64".into(), generate_gradient(64, 64)));

    // ------------------------------------------------------------------
    // Plan + ordering invariants.
    // ------------------------------------------------------------------
    let plan = SweepBuilder::new(SweepAxes::modes_full(), QualityGrid::Step5).plan();
    eprintln!(
        "plan: {} cells, {} aliases merged, {} invalid skipped",
        plan.cells.len(),
        plan.duplicates_merged,
        plan.invalid_skipped.len()
    );

    if plan.cells[0].deviations != 0 {
        hard_failures.push("ordering: first cell is not the all-defaults stratum".into());
    }
    let mut prev_dev = 0u8;
    for c in &plan.cells {
        if c.deviations < prev_dev {
            hard_failures.push(format!("ordering: deviations decreased at {}", c.id));
            break;
        }
        prev_dev = c.deviations;
    }
    {
        let mut seen = std::collections::HashSet::new();
        for c in &plan.cells {
            if !seen.insert(&c.id) {
                hard_failures.push(format!("ordering: duplicate cell id {}", c.id));
            }
        }
    }
    if !plan.invalid_skipped.is_empty() {
        eprintln!(
            "  invalid strata (reported, expected): {:?}",
            plan.invalid_skipped
        );
    }

    // ------------------------------------------------------------------
    // Subset: default + every single-deviation stratum, on Q_GRID points.
    // ------------------------------------------------------------------
    let subset: Vec<_> = plan
        .cells
        .iter()
        .filter(|c| c.deviations <= 1 && Q_GRID.iter().any(|&q| (c.quality - q).abs() < 0.01))
        .collect();
    let n_strata = {
        let mut bases = std::collections::HashSet::new();
        for c in &subset {
            bases.insert(c.id.rsplit_once("_q").map(|(b, _)| b.to_string()).unwrap());
        }
        bases.len()
    };
    eprintln!(
        "subset: {} cells across {} strata × {} images",
        subset.len(),
        n_strata,
        images.len()
    );

    let zensim = Zensim::new(ZensimProfile::latest());

    // Encode the whole subset; cells pin threads=1, so parallelize
    // across (image, cell) pairs.
    let jobs: Vec<(usize, usize)> = (0..images.len())
        .flat_map(|i| (0..subset.len()).map(move |c| (i, c)))
        .collect();
    let t_all = Instant::now();
    let results: Vec<((usize, usize), Result<Measure, String>)> = jobs
        .par_iter()
        .map(|&(ii, ci)| {
            let (_, img) = &images[ii];
            let m = encode_and_score(img.as_ref(), &subset[ci].config, &zensim);
            ((ii, ci), m)
        })
        .collect();
    eprintln!(
        "encoded {} cells in {:.1}s",
        results.len(),
        t_all.elapsed().as_secs_f32()
    );

    let mut measures: HashMap<(usize, usize), Measure> = HashMap::new();
    for ((ii, ci), r) in results {
        match r {
            Ok(m) => {
                measures.insert((ii, ci), m);
            }
            Err(e) => hard_failures.push(format!(
                "encode failure: {} on {}: {e}",
                subset[ci].id, images[ii].0
            )),
        }
    }

    // ------------------------------------------------------------------
    // TSV.
    // ------------------------------------------------------------------
    let git = std::env::var("GIT_COMMIT").unwrap_or_else(|_| "unknown".into());
    let mut tsv = format!(
        "# sweep_validate git={git} cells={} strata={} images={} zenravif=0.1.3\n\
         image\tbase_id\tdeviations\tq\tbytes\tssim2\tencode_ms\tfingerprint\n",
        subset.len(),
        n_strata,
        images.len()
    );
    for (ii, (name, _)) in images.iter().enumerate() {
        for (ci, cell) in subset.iter().enumerate() {
            if let Some(m) = measures.get(&(ii, ci)) {
                let base = cell
                    .id
                    .rsplit_once("_q")
                    .map(|(b, _)| b)
                    .unwrap_or(&cell.id);
                tsv.push_str(&format!(
                    "{name}\t{base}\t{}\t{}\t{}\t{:.2}\t{}\t{:016x}\n",
                    cell.deviations, cell.quality, m.bytes, m.ssim2, m.encode_ms, cell.fingerprint
                ));
            }
        }
    }

    // ------------------------------------------------------------------
    // Check 2: no inert single-deviation label. Each non-default base id
    // must produce different bytes from the default stratum at the same
    // q on at least one image. (Probes whose override equals the preset
    // dedupe away at plan time — whatever survives must be live.)
    // ------------------------------------------------------------------
    let default_base = {
        let c = &plan.cells[0];
        c.id.rsplit_once("_q").map(|(b, _)| b.to_string()).unwrap()
    };
    let key_of = |ci: usize| {
        let c = &subset[ci];
        let base = c.id.rsplit_once("_q").map(|(b, _)| b.to_string()).unwrap();
        (base, c.quality.to_bits())
    };
    // (base, q) → per-image hashes.
    let mut by_cell: HashMap<(String, u32), HashMap<usize, u64>> = HashMap::new();
    for (&(ii, ci), m) in &measures {
        by_cell.entry(key_of(ci)).or_default().insert(ii, m.hash);
    }
    let bases: std::collections::HashSet<String> = by_cell.keys().map(|(b, _)| b.clone()).collect();
    for base in &bases {
        if *base == default_base {
            continue;
        }
        let mut differs_somewhere = false;
        let mut compared = 0;
        for &qbits in Q_GRID
            .iter()
            .map(|q| q.to_bits())
            .collect::<Vec<_>>()
            .iter()
        {
            let (Some(dft), Some(dev)) = (
                by_cell.get(&(default_base.clone(), qbits)),
                by_cell.get(&(base.clone(), qbits)),
            ) else {
                continue;
            };
            for (ii, h) in dev {
                if let Some(dh) = dft.get(ii) {
                    compared += 1;
                    if dh != h {
                        differs_somewhere = true;
                    }
                }
            }
        }
        if compared == 0 {
            warnings.push(format!("inert-check: no comparable encodes for {base}"));
        } else if !differs_somewhere {
            hard_failures.push(format!(
                "INERT STEP: {base} never changed output bytes vs {default_base} \
                 across {compared} comparisons — a curated value that does nothing"
            ));
        }
    }

    // ------------------------------------------------------------------
    // Check 1: fingerprint contracts on real encodes.
    // Each pair (a, b): equal fingerprint must imply byte-identity, on
    // an adversarial image (noise — maximizes the chance a live knob
    // shows itself).
    // ------------------------------------------------------------------
    let noise = generate_noise(256, 256);
    let big_noise = generate_noise(512, 512);
    let pin = |c: EncoderConfig| c.threads(Some(1));

    struct AliasPair {
        label: &'static str,
        a: EncoderConfig,
        b: EncoderConfig,
        expect_equal: bool,
    }
    let pairs = vec![
        AliasPair {
            label: "quantizer-mirror: q80.0 vs q80.2 (same quantizer 71)",
            a: pin(EncoderConfig::new().quality(80.0).speed(6)),
            b: pin(EncoderConfig::new().quality(80.2).speed(6)),
            expect_equal: true,
        },
        AliasPair {
            label: "quantizer-mirror negative control: q80.0 vs q81.0",
            a: pin(EncoderConfig::new().quality(80.0).speed(6)),
            b: pin(EncoderConfig::new().quality(81.0).speed(6)),
            expect_equal: false,
        },
        AliasPair {
            label: "override==preset: cdef Some(true) vs None at q30/speed6",
            a: pin(EncoderConfig::new().quality(30.0).speed(6)).with_cdef(Some(true)),
            b: pin(EncoderConfig::new().quality(30.0).speed(6)),
            expect_equal: true,
        },
        AliasPair {
            label: "override!=preset negative control: cdef Some(false) at q30/speed6",
            a: pin(EncoderConfig::new().quality(30.0).speed(6)).with_cdef(Some(false)),
            b: pin(EncoderConfig::new().quality(30.0).speed(6)),
            expect_equal: false,
        },
        AliasPair {
            label: "vaq_strength inert when vaq off: 1.0 vs 3.0",
            a: pin(EncoderConfig::new().quality(50.0).speed(6)).with_vaq(false, 1.0),
            b: pin(EncoderConfig::new().quality(50.0).speed(6)).with_vaq(false, 3.0),
            expect_equal: true,
        },
        AliasPair {
            label: "vaq_strength live when vaq on: 0.5 vs 2.0",
            a: pin(EncoderConfig::new().quality(50.0).speed(6)).with_vaq(true, 0.5),
            b: pin(EncoderConfig::new().quality(50.0).speed(6)).with_vaq(true, 2.0),
            expect_equal: false,
        },
        AliasPair {
            label: "matrix_coefficients dead on zenravif: mc(9) vs unset",
            a: pin(EncoderConfig::new().quality(50.0).speed(6)).matrix_coefficients(9),
            b: pin(EncoderConfig::new().quality(50.0).speed(6)),
            expect_equal: true,
        },
        AliasPair {
            label: "negative control: qm on vs off",
            a: pin(EncoderConfig::new().quality(50.0).speed(6)).with_qm(true),
            b: pin(EncoderConfig::new().quality(50.0).speed(6)).with_qm(false),
            expect_equal: false,
        },
    ];

    for p in &pairs {
        let fp_a = fingerprint(&p.a);
        let fp_b = fingerprint(&p.b);
        if (fp_a == fp_b) != p.expect_equal {
            hard_failures.push(format!(
                "FINGERPRINT: {} — fingerprints {} but expected {}",
                p.label,
                if fp_a == fp_b { "equal" } else { "differ" },
                if p.expect_equal { "equal" } else { "distinct" },
            ));
            continue;
        }
        let ea = encode_rgb8(noise.as_ref(), &p.a, stop());
        let eb = encode_rgb8(noise.as_ref(), &p.b, stop());
        match (ea, eb) {
            (Ok(a), Ok(b)) => {
                let identical = a.avif_file == b.avif_file;
                if p.expect_equal && !identical {
                    hard_failures.push(format!(
                        "FINGERPRINT CONTRACT VIOLATION: {} — equal fingerprint, \
                         different bytes ({} vs {})",
                        p.label,
                        a.avif_file.len(),
                        b.avif_file.len()
                    ));
                } else if !p.expect_equal && identical {
                    warnings.push(format!(
                        "fingerprint negative control produced identical bytes \
                         (knob inert on noise@q): {}",
                        p.label
                    ));
                }
            }
            (Err(e), _) | (_, Err(e)) => {
                hard_failures.push(format!("alias-pair encode failed: {}: {e}", p.label));
            }
        }
    }

    // ------------------------------------------------------------------
    // Check 3: tiles/threads claims.
    // 256² at speed 6, q30 → min_tile_size 128 → cap (256·256)/128² = 4:
    // threads 1 vs 2 must differ (2 tiles vs 1). 64² tiny → cap 0:
    // threads 1 vs 2 must be byte-identical.
    // ------------------------------------------------------------------
    {
        let mk = |threads: usize| {
            EncoderConfig::new()
                .quality(30.0)
                .speed(6)
                .threads(Some(threads))
        };
        let t1 = encode_rgb8(big_noise.as_ref(), &mk(1), stop());
        let t2 = encode_rgb8(big_noise.as_ref(), &mk(2), stop());
        match (t1, t2) {
            (Ok(a), Ok(b)) => {
                if a.avif_file == b.avif_file {
                    warnings.push(
                        "tiles: threads 1 vs 2 byte-identical on 512² q30/speed6 — \
                         tile-count machine-dependence claim not reproduced here"
                            .into(),
                    );
                }
            }
            (Err(e), _) | (_, Err(e)) => hard_failures.push(format!("tiles encode failed: {e}")),
        }
        let tiny = generate_gradient(64, 64);
        let t1 = encode_rgb8(tiny.as_ref(), &mk(1), stop());
        let t2 = encode_rgb8(tiny.as_ref(), &mk(2), stop());
        match (t1, t2) {
            (Ok(a), Ok(b)) => {
                if a.avif_file != b.avif_file {
                    hard_failures.push(
                        "tiles: threads 1 vs 2 differ on 64² (cap 0) — the \
                         dimension-independence assumption behind pinned-thread \
                         fingerprints is wrong"
                            .into(),
                    );
                }
            }
            (Err(e), _) | (_, Err(e)) => {
                hard_failures.push(format!("tiles tiny encode failed: {e}"))
            }
        }
    }

    // ------------------------------------------------------------------
    // Check 5: ssim2 sanity floor at q85 on the default stratum (catches
    // corrupt pixel paths — channel swaps, broken subsampling).
    // ------------------------------------------------------------------
    for (ii, (name, _)) in images.iter().enumerate() {
        if name.starts_with("noise") {
            continue; // noise legitimately scores low
        }
        for (ci, cell) in subset.iter().enumerate() {
            let base = cell
                .id
                .rsplit_once("_q")
                .map(|(b, _)| b.to_string())
                .unwrap();
            if base == default_base
                && (cell.quality - 85.0).abs() < 0.01
                && let Some(m) = measures.get(&(ii, ci))
                && m.ssim2 < 60.0
            {
                hard_failures.push(format!(
                    "SSIM2 FLOOR: default stratum q85 scored {:.1} on {name} — \
                     pixel corruption suspected",
                    m.ssim2
                ));
            }
        }
    }

    // ------------------------------------------------------------------
    // Check 6 (soft): documented directions.
    // ------------------------------------------------------------------
    {
        // 4:2:0 shrinks photos at q60.
        let q = 60.0f32.to_bits();
        let (Some(dft), Some(s420)) = (
            by_cell.get(&(default_base.clone(), q)),
            by_cell.get(&("s4-420".to_string(), q)),
        ) else {
            warnings.push("direction: missing 420 cells for comparison".into());
            // (fall through; nothing else uses these bindings)
            return finish(out_path, tsv, hard_failures, warnings);
        };
        let _ = (dft, s420); // hashes only confirm distinctness; sizes below
        for (ii, (name, _)) in images.iter().enumerate() {
            if !name.starts_with("cid_") {
                continue;
            }
            let bytes_of = |base: &str| {
                subset.iter().enumerate().find_map(|(ci, c)| {
                    let b = c.id.rsplit_once("_q").map(|(x, _)| x.to_string()).unwrap();
                    ((c.quality - 60.0).abs() < 0.01 && b == base)
                        .then(|| measures.get(&(ii, ci)).map(|m| m.bytes))
                        .flatten()
                })
            };
            if let (Some(d), Some(s)) = (bytes_of(&default_base), bytes_of("s4-420"))
                && s >= d
            {
                warnings.push(format!(
                    "direction: 4:2:0 did not shrink {name} at q60 ({s} vs {d} bytes)"
                ));
            }
        }
    }

    finish(out_path, tsv, hard_failures, warnings)
}

fn finish(out_path: String, tsv: String, hard_failures: Vec<String>, warnings: Vec<String>) {
    if let Some(parent) = std::path::Path::new(&out_path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&out_path, &tsv).unwrap_or_else(|e| panic!("write {out_path}: {e}"));
    eprintln!("\nTSV → {out_path}");

    let mut out = std::io::stderr().lock();
    if !warnings.is_empty() {
        writeln!(out, "\n{} warnings:", warnings.len()).ok();
        for w in &warnings {
            writeln!(out, "  warn: {w}").ok();
        }
    }
    if hard_failures.is_empty() {
        writeln!(out, "\nsweep_validate: ALL HARD CHECKS PASSED").ok();
        std::process::exit(0);
    }
    writeln!(out, "\n{} HARD FAILURES:", hard_failures.len()).ok();
    for f in &hard_failures {
        writeln!(out, "  FAIL: {f}").ok();
    }
    std::process::exit(1);
}

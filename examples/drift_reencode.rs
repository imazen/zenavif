//! P0 label-drift re-encode harness (docs/FEATURE_HINTS_PLAN.md, P0.2).
//!
//! The canonical picker dataset (`canonical-picker-2026-07-01-zensimA/zenavif_lossy`)
//! was encoded on pre-parity zenrav1e `22a58d58`. This harness re-encodes a
//! stratified sample of its cells — the EXACT planner cells, matched by
//! `(cell, q)` and verified byte-for-byte on the stored plan fingerprint —
//! against whatever zenrav1e tree the current build resolves (swapped between
//! legs via the ravif dev-patch), then decodes + scores ssim2 exactly the way
//! the sweep did (`decode_with` threads(1) → RowConverter → RGB8 →
//! `fast_ssim2::compute_ssimulacra2`).
//!
//! Plan reproduction is pinned by the run's own `manifests/box-0.plan.json`:
//! `modes_full`, budget 400, q-grid {5,15,30,50,70,85,95} → 336 cells, all
//! probe/vaq/trellis axes budget-dropped. Any fingerprint mismatch aborts the
//! run (the planner no longer reproduces the dataset's configs — that itself
//! would be a P0 finding).
//!
//! Usage (requires `--features __expert`):
//!   drift_reencode --sample <tsv> --out <tsv> --leg <label> [--jobs N]
//!                  [--encoded-dir <dir>] [--verify-only]
//!
//! Sample TSV comes from `scripts/rd_gap/sample_drift_cells.py`; column 1 is
//! the absolute source PNG path (sync.sh ships those to the box verbatim).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use imgref::ImgRef;
use rgb::Rgb;
use zenavif::sweep::{QualityGrid, SweepAxes, SweepBuilder, SweepCell};
use zenavif::DecoderConfig;

const Q_GRID: [f32; 7] = [5.0, 15.0, 30.0, 50.0, 70.0, 85.0, 95.0];
const PLAN_BUDGET: usize = 400;

#[derive(Clone)]
struct SampleRow {
    local_png: String,
    cell: String,
    fp: String,
    q: f32,
    rest: Vec<String>, // stored_* + identity columns, passed through
}

fn main() {
    let mut sample = None;
    let mut out = None;
    let mut leg = "unnamed".to_string();
    let mut jobs = std::thread::available_parallelism().map_or(8, |n| n.get());
    let mut encoded_dir: Option<PathBuf> = None;
    let mut verify_only = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--sample" => sample = args.next(),
            "--out" => out = args.next(),
            "--leg" => leg = args.next().expect("--leg <label>"),
            "--jobs" => jobs = args.next().expect("--jobs N").parse().expect("jobs"),
            "--encoded-dir" => encoded_dir = args.next().map(PathBuf::from),
            "--verify-only" => verify_only = true,
            other => panic!("unknown arg {other:?}"),
        }
    }
    let sample = sample.expect("--sample <tsv> required");

    // ---- 1. Rebuild the canonical plan and index by (cell base id, q). ----
    let plan = SweepBuilder::new(
        SweepAxes::modes_full(),
        QualityGrid::Explicit(Q_GRID.to_vec()),
    )
    .with_budget(PLAN_BUDGET)
    .plan();
    eprintln!(
        "plan: {} cells (dup_merged={} invalid={} over_budget={})",
        plan.cells.len(),
        plan.duplicates_merged,
        plan.invalid_skipped.len(),
        plan.over_budget
    );
    let mut by_cell_q: HashMap<(String, i64), &SweepCell> = HashMap::new();
    for c in &plan.cells {
        let base = c.id.rfind("_q").map(|at| &c.id[..at]).unwrap_or(&c.id);
        let key = (base.to_string(), (c.quality * 100.0).round() as i64);
        if by_cell_q.insert(key, c).is_some() {
            panic!("duplicate (cell,q) in plan: {}", c.id);
        }
    }

    // ---- 2. Load the sample and verify every fingerprint. ----
    let text = std::fs::read_to_string(&sample).expect("read sample tsv");
    let mut lines = text.lines();
    let header = lines.next().expect("tsv header");
    let rows: Vec<SampleRow> = lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            SampleRow {
                local_png: f[0].to_string(),
                cell: f[1].to_string(),
                fp: f[2].to_string(),
                q: f[3].parse().expect("q"),
                rest: f[4..].iter().map(|s| s.to_string()).collect(),
            }
        })
        .collect();

    let mut fp_mismatch = 0usize;
    let mut missing = 0usize;
    for r in &rows {
        let key = (r.cell.clone(), (r.q * 100.0).round() as i64);
        match by_cell_q.get(&key) {
            None => {
                missing += 1;
                eprintln!("NO PLAN CELL for ({}, q{})", r.cell, r.q);
            }
            Some(c) => {
                let fp = format!("{:016x}", c.fingerprint);
                if fp != r.fp {
                    fp_mismatch += 1;
                    eprintln!(
                        "FP MISMATCH ({}, q{}): plan {} != stored {}",
                        r.cell, r.q, fp, r.fp
                    );
                }
            }
        }
    }
    eprintln!(
        "sample: {} rows, {} plan-missing, {} fp-mismatch",
        rows.len(),
        missing,
        fp_mismatch
    );
    assert_eq!(
        missing + fp_mismatch,
        0,
        "planner does not reproduce the dataset's cells — DO NOT trust re-encodes"
    );
    if verify_only {
        println!("VERIFY OK: {} rows all match plan fingerprints", rows.len());
        return;
    }
    let out = out.expect("--out <tsv> required");
    if let Some(d) = &encoded_dir {
        std::fs::create_dir_all(d).expect("create encoded dir");
    }

    // ---- 3. Re-encode + decode + score, in parallel. 32 MB stacks: rayon ----
    // work-stealing stacks whole encode/decode task contexts on one worker
    // stack (project CLAUDE.md, sweep_validate gotcha).
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .stack_size(32 * 1024 * 1024)
        .build()
        .expect("rayon pool");

    // Cache decoded sources (8 distinct images, 42 rows each).
    let mut png_cache: HashMap<String, (Vec<u8>, usize, usize)> = HashMap::new();
    for r in &rows {
        if !png_cache.contains_key(&r.local_png) {
            let img = image::open(&r.local_png)
                .unwrap_or_else(|e| panic!("open {}: {e}", r.local_png))
                .to_rgb8();
            let (w, h) = (img.width() as usize, img.height() as usize);
            png_cache.insert(r.local_png.clone(), (img.into_raw(), w, h));
        }
    }

    let results: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::with_capacity(rows.len()));
    let done = std::sync::atomic::AtomicUsize::new(0);
    let t0 = Instant::now();
    pool.scope(|s| {
        for (i, r) in rows.iter().enumerate() {
            let results = &results;
            let by_cell_q = &by_cell_q;
            let png_cache = &png_cache;
            let done = &done;
            let leg = leg.as_str();
            let encoded_dir = encoded_dir.as_deref();
            let n_rows = rows.len();
            s.spawn(move |_| {
                let line = run_row(r, by_cell_q, png_cache, leg, encoded_dir);
                results.lock().unwrap().push((i, line));
                let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if d % 25 == 0 || d == n_rows {
                    eprintln!(
                        "[{leg}] {d}/{n_rows} done, {:.1}s elapsed",
                        t0.elapsed().as_secs_f64()
                    );
                }
            });
        }
    });

    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(i, _)| *i);
    let mut f = std::fs::File::create(&out).expect("create out tsv");
    writeln!(f, "{header}\tleg\tnew_bytes\tnew_encode_ms\tnew_decode_ms\tnew_ssim2\terr").unwrap();
    for (_, line) in &results {
        writeln!(f, "{line}").unwrap();
    }
    let errs = results.iter().filter(|(_, l)| !l.ends_with('\t')).count();
    println!(
        "[{leg}] wrote {} rows to {} ({} with err) in {:.1}s",
        results.len(),
        out,
        errs,
        t0.elapsed().as_secs_f64()
    );
}

fn run_row(
    r: &SampleRow,
    by_cell_q: &HashMap<(String, i64), &SweepCell>,
    png_cache: &HashMap<String, (Vec<u8>, usize, usize)>,
    leg: &str,
    encoded_dir: Option<&std::path::Path>,
) -> String {
    let passthrough = format!(
        "{}\t{}\t{}\t{}\t{}",
        r.local_png,
        r.cell,
        r.fp,
        r.q,
        r.rest.join("\t")
    );
    match encode_score(r, by_cell_q, png_cache, leg, encoded_dir) {
        Ok((bytes, enc_ms, dec_ms, ssim2)) => format!(
            "{passthrough}\t{leg}\t{bytes}\t{enc_ms:.3}\t{dec_ms:.3}\t{ssim2:.6}\t"
        ),
        Err(e) => format!("{passthrough}\t{leg}\t0\t0\t0\tnan\t{}", e.replace('\t', " ")),
    }
}

fn encode_score(
    r: &SampleRow,
    by_cell_q: &HashMap<(String, i64), &SweepCell>,
    png_cache: &HashMap<String, (Vec<u8>, usize, usize)>,
    _leg: &str,
    encoded_dir: Option<&std::path::Path>,
) -> Result<(usize, f64, f64, f64), String> {
    let cell = by_cell_q
        .get(&(r.cell.clone(), (r.q * 100.0).round() as i64))
        .ok_or("plan cell vanished")?;
    let (raw, w, h) = png_cache.get(&r.local_png).ok_or("png not cached")?;
    let pixels: &[Rgb<u8>] = bytemuck::cast_slice(raw);
    let img = ImgRef::new(pixels, *w, *h);

    // Encode — the same call shape as zenmetrics' PlannedConfig::encode_bytes.
    let t = Instant::now();
    let encoded = zenavif::encode_rgb8(
        img,
        &cell.config,
        almost_enough::StopToken::new(enough::Unstoppable),
    )
    .map_err(|e| format!("encode: {e}"))?;
    let enc_ms = t.elapsed().as_secs_f64() * 1e3;
    let avif = encoded.avif_file;

    if let Some(d) = encoded_dir {
        let variant = r.rest.get(5).map(String::as_str).unwrap_or("unknown");
        let name = format!("{variant}__{}_q{}.avif", r.cell, r.q as i64);
        std::fs::write(d.join(name), &avif).map_err(|e| format!("persist: {e}"))?;
    }

    // Decode + narrow to RGB8 exactly like zenmetrics' decode_avif +
    // pixel_slice_to_rgb8 (threads(1); RowConverter to RGB8_SRGB).
    let t = Instant::now();
    let buf = zenavif::decode_with(&avif, &DecoderConfig::new().threads(1), &enough::Unstoppable)
        .map_err(|e| format!("decode: {e}"))?;
    let dec = {
        use zenpixels::PixelDescriptor;
        use zenpixels_convert::converter::RowConverter;
        let slice = buf.as_slice();
        let (dw, dh) = (slice.width(), slice.rows());
        if dw as usize != *w || dh as usize != *h {
            return Err(format!("decode size {dw}x{dh} != source {w}x{h}"));
        }
        let mut dst = vec![0u8; *w * 3 * *h];
        let mut conv = RowConverter::new(slice.descriptor(), PixelDescriptor::RGB8_SRGB)
            .map_err(|e| format!("plan convert: {e}"))?;
        conv.convert_rows(
            slice.as_strided_bytes(),
            slice.stride(),
            &mut dst,
            *w * 3,
            dw,
            dh,
        )
        .map_err(|e| format!("convert: {e}"))?;
        dst
    };
    let dec_ms = t.elapsed().as_secs_f64() * 1e3;

    let a: &[[u8; 3]] = bytemuck::cast_slice(raw);
    let b: &[[u8; 3]] = bytemuck::cast_slice(&dec);
    let ssim2 = fast_ssim2::compute_ssimulacra2(ImgRef::new(a, *w, *h), ImgRef::new(b, *w, *h))
        .map_err(|e| format!("ssim2: {e}"))?;

    Ok((avif.len(), enc_ms, dec_ms, ssim2))
}

//! Zensim target-hitting CQ loop harness — the AVIF analogue of
//! jxl-encoder's `zensim_diffmap_rd` beats-butter series. **Protocol,
//! constants, gates, seeds, and the measured smoke:
//! `benchmarks/zensim_avif_loop_2026-08-07.md`** (appendix AC.4).
//!
//! Per cell: seed encode → independent decode → complete candidate surface
//! score and current attribution → the adopted jxl
//! controller (pure proportional, exp 1.0 / per-step clamp 2.0). Arms:
//! `baseline` (controller only), `h3-mag` (per-64px-SB `query_rect`
//! magnitude steering via `EncoderConfig::with_sb_q_scale`; PANICS if
//! the hint passthrough is a no-op), `outer` (zensim-judged CQ bisection
//! comparator). `--iters K` = K steps after the seed = K+1 encodes per
//! cell (series budget parity). The in-loop score runs on the ACTUAL
//! decoded bitstream ⇒ `achieved_inloop == achieved_decoded`
//! structurally (both columns kept for schema parity).
//!
//! CLI (mirrors zensim_diffmap_rd): `--corpus-file` (path\tname\tclass
//! TSV) `--zensim-targets 70,80,88` `--arms baseline,h3-mag|outer`
//! `--bake <exact-artifact-path>` `--iters K` `--label L` `--out-dir D`.
//! `AVIF_ZENSIM_*` env knobs + defaults: the study doc. Outputs:
//! `target_ab_<label>.tsv` (jxl series schema — readable by
//! `analyze_23shot.cells_stats`; seed_d → seed_cq) + a per-iteration
//! `trace_<label>.tsv`.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use almost_enough::{StopToken, Unstoppable};
use rgb::Rgb;
use zenavif::{DecoderConfig, EncoderConfig, FRAME_HINTS_LIVE, decode_with, encode_rgb8};
// Scalar scoring and current spatial maps share the complete candidate surface.
use zenpredict_serving::Model;
use zensim::{BakeScorer, PrecomputedReference, RgbSlice};

const SB: usize = 64;
const CQ_MIN: f64 = 1.0;
const CQ_MAX: f64 = 255.0;
/// Accumulated per-SB quantizer-scale bounds (registered; the two-pass
/// driver's λ-domain clamp [0.4, 2.5] maps to q-scale ≈ [0.63, 1.58] —
/// ours is symmetric and slightly wider).
const SB_SCALE_MIN: f32 = 0.5;
const SB_SCALE_MAX: f32 = 2.0;

/// Registered seed CQ per target (2026-08-07, from the 10-point
/// city.png probe curve in the study doc's SEEDS section; coarse by
/// design — the controller owns convergence).
fn seed_cq_for_target(t: f64) -> f64 {
    if let Ok(s) = std::env::var("AVIF_ZENSIM_SEED_CQ")
        && let Ok(v) = s.parse::<f64>()
    {
        return v.clamp(CQ_MIN, CQ_MAX);
    }
    if t >= 88.0 {
        40.0
    } else if t >= 80.0 {
        90.0
    } else {
        125.0
    }
}

/// Mirror of zenravif's private `quality_to_quantizer` (av1encoder.rs).
/// Kept honest by the roundtrip self-check in `main`; drift vs the real
/// mapping would only degrade seed-table legibility (the feedback
/// controller needs monotonicity alone), never correctness.
fn quality_to_qindex(quality: f32) -> u8 {
    let q = quality.clamp(1., 100.) / 100.;
    let x = if q >= 0.70 {
        (1. - q) * 1.4
    } else if q > 0.10 {
        0.42 + (0.70 - q) * 0.85
    } else {
        0.93 + (0.10 - q) * 0.78
    };
    (x.min(1.0) * 255.).round() as u8
}

/// Inverse of [`quality_to_qindex`]: continuous CQ (AV1 qindex domain,
/// [1, 255]) → the zenavif `quality` dial value that realizes it.
fn cq_to_quality(cq: f64) -> f32 {
    let x = cq.clamp(0.0, 255.0) / 255.0;
    let q = if x <= 0.42 {
        1.0 - x / 1.4
    } else if x <= 0.93 {
        0.70 - (x - 0.42) / 0.85
    } else {
        0.10 - (x - 0.93) / 0.78
    };
    (q * 100.0).clamp(1.0, 100.0) as f32
}

/// Exact artifact identity, with no global mount or width inference.
fn load_model(path: &str) -> Model {
    assert!(
        !path.starts_with("profile:"),
        "pass an exact bake file, not a mutable profile alias"
    );
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("bake {path}: {e}"));
    Model::from_bytes(&bytes).unwrap_or_else(|e| panic!("bake {path}: {e}"))
}

fn served_score(scorer: &mut BakeScorer<'_>, source: &RgbSlice<'_>, decoded: &RgbSlice<'_>) -> f64 {
    let score = scorer
        .compute(source, decoded, Some("avif"))
        .expect("complete candidate scoring")
        .score();
    assert!(score.is_finite(), "nonfinite candidate score");
    score
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1" | "true" | "yes")
    )
}

fn load_rgb8(path: &str) -> (Vec<[u8; 3]>, usize, usize) {
    let img = image::open(path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    assert!(
        matches!(img.color(), image::ColorType::Rgb8 | image::ColorType::L8),
        "this research driver requires opaque 8-bit RGB or grayscale input: {path}"
    );
    let img = img.to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px: Vec<[u8; 3]> = img.pixels().map(|p| [p.0[0], p.0[1], p.0[2]]).collect();
    (px, w, h)
}

fn corpus_from_file(path: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for line in std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("corpus file {path}: {e}"))
        .lines()
    {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut f = line.split('\t');
        let p = f.next().unwrap_or("").to_string();
        let name = f.next().map(|s| s.to_string()).unwrap_or_else(|| {
            std::path::Path::new(&p)
                .file_stem()
                .expect("corpus path stem")
                .to_string_lossy()
                .into_owned()
        });
        let class = f.next().unwrap_or("image").to_string();
        if !p.is_empty() {
            out.push((name, class, p));
        }
    }
    assert!(!out.is_empty(), "corpus file {path} has no rows");
    out
}

struct EncodeSettings {
    speed: u8,
    threads: usize,
}

fn base_config(s: &EncodeSettings) -> EncoderConfig {
    // Registered constants (study doc): 4:4:4, 8-bit, single-threaded
    // (deterministic + box-load courtesy).
    EncoderConfig::new()
        .backend(zenavif::Av1Backend::Zenravif)
        .speed(s.speed)
        .bit_depth(zenavif::EncodeBitDepth::Eight)
        .chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv444)
        .threads(Some(s.threads))
}

/// `[[u8; 3]]` and `[Rgb<u8>]` are layout-identical — reslice, no copy.
fn as_rgb_slice(px: &[[u8; 3]]) -> &[Rgb<u8>] {
    use rgb::FromSlice;
    px.as_flattened().as_rgb()
}

fn encode_at(
    px: &[[u8; 3]],
    w: usize,
    h: usize,
    cq: f64,
    sb_map: Option<Box<[f32]>>,
    s: &EncodeSettings,
) -> Vec<u8> {
    let img = imgref::ImgRef::new(as_rgb_slice(px), w, h);
    let cfg = base_config(s)
        .quality(cq_to_quality(cq))
        .with_sb_q_scale(sb_map);
    encode_rgb8(img, &cfg, StopToken::new(Unstoppable))
        .unwrap_or_else(|e| panic!("encode failed at cq {cq:.1}: {e}"))
        .avif_file
}

fn decode_rgb8(avif: &[u8], w: usize, h: usize) -> Vec<[u8; 3]> {
    let cfg = DecoderConfig::new().prefer_8bit(true);
    let decoded = decode_with(avif, &cfg, &StopToken::new(Unstoppable))
        .unwrap_or_else(|e| panic!("decode failed: {e}"));
    let img = decoded
        .try_as_imgref::<Rgb<u8>>()
        .expect("decode not RGB8-viewable");
    assert_eq!((img.width(), img.height()), (w, h), "decode dims mismatch");
    let mut out = Vec::with_capacity(w * h);
    for row in img.rows() {
        for p in row {
            out.push([p.r, p.g, p.b]);
        }
    }
    out
}

/// The engagement probe (G-AV1): per-SB hints must actually reach the
/// bitstream — same-map determinism first (else the differ-assert is
/// unsound), then PANIC unless a strongly non-neutral map moves bytes.
fn assert_hint_engagement(px: &[[u8; 3]], w: usize, h: usize, s: &EncodeSettings) {
    // Runtime read of the compile-time gate (a const assert would forbid
    // COMPILING gated — the refusal belongs to the arm being requested).
    if !FRAME_HINTS_LIVE {
        panic!(
            "h3-mag requested but zenavif::FRAME_HINTS_LIVE == false: the per-SB \
             FrameHints passthrough is release-gated off (registry zenrav1e 0.1.4 \
             has no FrameHints; it lands past 0.1.4, rev c4047cec) — the hint API \
             would be a silent no-op. Unblock: zenravif dep bump + flip \
             FRAME_HINTS_LIVE + uncomment the hinted send (ravif repo, \
             av1encoder.rs)."
        );
    }
    let (cw, ch) = (w.min(128), h.min(128));
    let mut crop = Vec::with_capacity(cw * ch);
    for y in 0..ch {
        crop.extend_from_slice(&px[y * w..y * w + cw]);
    }
    let cols = cw.div_ceil(SB);
    let rows = ch.div_ceil(SB);
    let neutral: Box<[f32]> = vec![1.0f32; cols * rows].into_boxed_slice();
    let steer: Box<[f32]> = (0..cols * rows)
        .map(|i| if i % 2 == 0 { 0.5f32 } else { 2.0 })
        .collect();
    let a = encode_at(&crop, cw, ch, 100.0, Some(neutral.clone()), s);
    let b = encode_at(&crop, cw, ch, 100.0, Some(neutral), s);
    assert_eq!(a, b, "probe unsound: same-map encodes nondeterministic");
    let c = encode_at(&crop, cw, ch, 100.0, Some(steer), s);
    assert_ne!(
        a, c,
        "h3-mag engagement probe FAILED: a 0.5/2.0 per-SB map left the bitstream \
         byte-identical despite FRAME_HINTS_LIVE == true (stale gate constant or \
         broken plumbing) — refusing an arm that steers nothing"
    );
}

struct CellResult {
    achieved: f64,
    iters_used: usize,
    bytes: Vec<u8>,
    encode_ms: f64,
    loop_ms: f64,
    seed_cq: f64,
}

struct TraceCtx<'a> {
    path: &'a std::path::Path,
    id: String,
}

impl TraceCtx<'_> {
    fn map(&self, iter: usize, score: f64, tiles: &[f64]) {
        use std::io::Write;
        let path = self.path.with_extension("maps.jsonl");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("map trace");
        writeln!(
            file,
            "{}",
            serde_json::json!({"trace_id":self.id,"iter":iter,"score":score,"tiles":tiles})
        )
        .expect("write map trace");
    }
    #[allow(clippy::too_many_arguments)]
    fn line(
        &self,
        iter: usize,
        cq: f64,
        score: f64,
        bytes: usize,
        sb_min: f32,
        sb_max: f32,
        iter_ms: f64,
    ) {
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path)
        {
            let qi = quality_to_qindex(cq_to_quality(cq));
            let _ = writeln!(
                f,
                "{}\t{iter}\t{cq:.2}\t{qi}\t{score:.4}\t{bytes}\t{sb_min:.4}\t{sb_max:.4}\t{iter_ms:.1}",
                self.id
            );
        }
    }
}

/// Emitted iterate: min |err|, ties to the LATEST (the jxl emit-best
/// tie rule) when `emit_best`; else the last.
fn emit_from(iterates: Vec<(f64, f64, Vec<u8>)>, emit_best: bool) -> (f64, Vec<u8>) {
    assert!(!iterates.is_empty(), "at least one encode ran");
    if emit_best {
        let mut bi = 0usize;
        for (i, (err, ..)) in iterates.iter().enumerate() {
            if *err <= iterates[bi].0 {
                bi = i;
            }
        }
        let (_, score, bytes) = iterates.into_iter().nth(bi).expect("index in range");
        (score, bytes)
    } else {
        let (_, score, bytes) = iterates.into_iter().next_back().expect("non-empty");
        (score, bytes)
    }
}

/// Controller step shared by both inner arms (the adopted jxl template
/// mirrored into the quantizer domain, qf ∝ 1/q): g > 1 ⇒ too lossy ⇒
/// more bits ⇒ LOWER CQ, i.e.
/// `next_cq = cq · clamp((target_loss/achieved_loss)^exp, 1/clamp, clamp)`
/// — pure proportional in the score-error → log-quantizer domain.
fn controller_step(cq: f64, score: f64, target: f64, exp: f64, clamp: f64) -> f64 {
    let achieved_loss = (100.0 - score).max(0.05);
    let target_loss = (100.0 - target).max(0.05);
    let g = ((achieved_loss / target_loss).powf(exp)).clamp(1.0 / clamp, clamp);
    (cq / g).clamp(CQ_MIN, CQ_MAX)
}

#[allow(clippy::too_many_arguments)]
fn run_inner_cell<'m>(
    scorer: &mut BakeScorer<'m>,
    map_scorer: Option<&mut BakeScorer<'m>>,
    px: &[[u8; 3]],
    w: usize,
    h: usize,
    pre: &PrecomputedReference,
    target: f64,
    steer: bool,
    encodes: usize,
    settings: &EncodeSettings,
    trace: &TraceCtx<'_>,
    emit_best: bool,
) -> CellResult {
    let ctrl_exp = env_f64("AVIF_ZENSIM_CTRL_EXP", 1.0);
    let ctrl_clamp = env_f64("AVIF_ZENSIM_CTRL_CLAMP", 2.0);
    let attr_bin = env_f64("ZENSIM_ATTR_BIN", 8.0).max(1.0) as usize;
    let h3_gain = env_f64("AVIF_ZENSIM_H3_GAIN", 10.0) as f32;
    let factor_max = env_f64("AVIF_ZENSIM_FACTOR_MAX", 1.15) as f32;
    // AVIF_ZENSIM_H3_RULE=zerosum selects the redistribution-only steering rule.
    let h3_rule_zerosum = std::env::var("AVIF_ZENSIM_H3_RULE").as_deref() == Ok("zerosum");

    let ref_slice = RgbSlice::new(px, w, h);
    let sb_cols = w.div_ceil(SB);
    let sb_rows = h.div_ceil(SB);
    let n_sb = sb_cols * sb_rows;
    let mut sb_scale = vec![1.0f32; n_sb];
    let mut map_scorer = map_scorer;
    let mut session = zensim::Fused944Session::new();

    let seed_cq = seed_cq_for_target(target);
    let mut cq = seed_cq;
    let mut encode_ms = 0.0f64;
    let mut loop_ms = 0.0f64;
    // (|err|, score, bytes) per iterate.
    let mut iterates: Vec<(f64, f64, Vec<u8>)> = Vec::with_capacity(encodes);

    for iter in 0..encodes {
        let t_it = Instant::now();
        // Current comparison i supplies hints to complete encode i+1.
        let hints: Option<Box<[f32]>> = if steer && iter > 0 && sb_scale.iter().any(|&v| v != 1.0) {
            Some(sb_scale.clone().into_boxed_slice())
        } else {
            None
        };
        let t_enc = Instant::now();
        let bytes = encode_at(px, w, h, cq, hints, settings);
        encode_ms += t_enc.elapsed().as_secs_f64() * 1e3;

        let t_loop = Instant::now();
        let dec = decode_rgb8(&bytes, w, h);
        let dec_slice = RgbSlice::new(&dec, w, h);

        let mut tile_q: Option<Vec<f64>> = None;
        let score = if steer {
            let map_owner = map_scorer.as_deref_mut().unwrap_or(&mut *scorer);
            let spatial = map_owner
                .compute_with_ref_and_attribution(
                    &ref_slice,
                    pre,
                    &dec_slice,
                    Some("avif"),
                    &mut session,
                    attr_bin,
                )
                .expect("complete candidate attribution");
            assert!(
                spatial.unsupported_feature_ids().is_empty() && !spatial.has_corruption_gate(),
                "candidate has unsupported spatial terms or a discontinuous corruption gate: {:?}",
                spatial.unsupported_feature_ids()
            );
            let mut q = Vec::with_capacity(n_sb);
            for sby in 0..sb_rows {
                for sbx in 0..sb_cols {
                    let (x, y) = (sbx * SB, sby * SB);
                    q.push(spatial.attribution().query_rect(x, y, x + SB, y + SB));
                }
            }
            assert!(q.iter().all(|v| v.is_finite()), "nonfinite SB attribution");
            trace.map(iter, spatial.result().score(), &q);
            tile_q = Some(q);
            if map_scorer.is_some() {
                served_score(scorer, &ref_slice, &dec_slice)
            } else {
                spatial.result().score()
            }
        } else {
            served_score(scorer, &ref_slice, &dec_slice)
        };
        loop_ms += t_loop.elapsed().as_secs_f64() * 1e3;

        let err = (score - target).abs();
        let (mn, mx) = sb_scale
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| {
                (a.min(v), b.max(v))
            });
        trace.line(
            iter,
            cq,
            score,
            bytes.len(),
            mn,
            mx,
            t_it.elapsed().as_secs_f64() * 1e3,
        );
        iterates.push((err, score, bytes));
        if iter + 1 == encodes {
            break;
        }

        // Controller (shared by both inner arms).
        cq = controller_step(cq, score, target, ctrl_exp, ctrl_clamp);

        // h3-mag: step ∝ query_rect × gain, per-step clamped, DIVIDING
        // the quantizer scale (+ = wants bits = finer; the jxl qf rule
        // inverted), accumulated, then mean-renormalized to the
        // controller's base CQ.
        if let Some(q) = &tile_q {
            if h3_rule_zerosum {
                // ZEROSUM rule (bug-fix candidate, 2026-08-29): the legacy rule's
                // arithmetic-mean renorm AFTER clamping, accumulated across
                // iterations, systematically COARSENS the neutral majority when
                // attribution is concentrated (screen content: a few clamped-fine
                // tiles drag the mean below 1, dividing everyone else above 1,
                // compounding per iteration — measured: sc_gui@t80 −28% bytes,
                // −11 score; photo median exactly 0.0). This rule is log-domain
                // ZERO-SUM redistribution, recomputed FRESH each iteration:
                //   s_i = exp(−g·(tq_i − mean(tq))), then clamp. Mean-rate stays
                // the controller's job; the map only redistributes.
                let mtq = q.iter().sum::<f64>() / q.len() as f64;
                for (scale, &tq) in sb_scale.iter_mut().zip(q.iter()) {
                    let step = (-(h3_gain as f64) * (tq - mtq)).exp() as f32;
                    let step = step.clamp(1.0 / factor_max, factor_max);
                    *scale = step.clamp(SB_SCALE_MIN, SB_SCALE_MAX);
                }
            } else {
                for (scale, &tq) in sb_scale.iter_mut().zip(q.iter()) {
                    let factor = (1.0 + h3_gain * tq as f32).clamp(1.0 / factor_max, factor_max);
                    *scale = (*scale / factor).clamp(SB_SCALE_MIN, SB_SCALE_MAX);
                }
                let mean = sb_scale.iter().map(|&v| v as f64).sum::<f64>() / n_sb as f64;
                if mean > 1e-6 {
                    for v in sb_scale.iter_mut() {
                        *v = (*v as f64 / mean) as f32;
                    }
                }
            }
        }
    }

    let iters_used = iterates.len();
    let (achieved, bytes) = emit_from(iterates, emit_best);
    CellResult {
        achieved,
        iters_used,
        bytes,
        encode_ms,
        loop_ms,
        seed_cq,
    }
}

/// The comparator: zensim-judged CQ BISECTION, one full re-encode per
/// step. Bracket [1, 255], first probe at the shared seed CQ, integer
/// midpoints after; judge = the same complete candidate surface.
#[allow(clippy::too_many_arguments)]
fn run_outer_cell(
    scorer: &mut BakeScorer<'_>,
    px: &[[u8; 3]],
    w: usize,
    h: usize,
    target: f64,
    encodes: usize,
    settings: &EncodeSettings,
    trace: &TraceCtx<'_>,
    emit_best: bool,
) -> CellResult {
    let ref_slice = RgbSlice::new(px, w, h);
    let seed_cq = seed_cq_for_target(target);
    let (mut lo, mut hi) = (CQ_MIN, CQ_MAX);
    let mut cq = seed_cq;
    let mut encode_ms = 0.0f64;
    let mut loop_ms = 0.0f64;
    let mut iterates: Vec<(f64, f64, Vec<u8>)> = Vec::with_capacity(encodes);

    for j in 0..encodes {
        let t_it = Instant::now();
        let t_enc = Instant::now();
        let bytes = encode_at(px, w, h, cq, None, settings);
        encode_ms += t_enc.elapsed().as_secs_f64() * 1e3;
        let t_loop = Instant::now();
        let dec = decode_rgb8(&bytes, w, h);
        let dec_slice = RgbSlice::new(&dec, w, h);
        let judged = served_score(scorer, &ref_slice, &dec_slice);
        loop_ms += t_loop.elapsed().as_secs_f64() * 1e3;
        let err = (judged - target).abs();
        trace.line(
            j,
            cq,
            judged,
            bytes.len(),
            1.0,
            1.0,
            t_it.elapsed().as_secs_f64() * 1e3,
        );
        iterates.push((err, judged, bytes));
        if j + 1 == encodes {
            break;
        }
        if judged < target {
            // Too lossy: the answer lies at a lower CQ.
            hi = cq;
        } else {
            lo = cq;
        }
        cq = ((lo + hi) / 2.0).round().clamp(CQ_MIN, CQ_MAX);
    }

    let iters_used = iterates.len();
    let (achieved, bytes) = emit_from(iterates, emit_best);
    CellResult {
        achieved,
        iters_used,
        bytes,
        encode_ms,
        loop_ms,
        seed_cq,
    }
}

fn main() {
    // CQ⇄quality self-check: the inverse must realize the intended
    // qindex within ±1 across the dial (guards mirror drift).
    for cq in 1..=255u32 {
        let realized = i32::from(quality_to_qindex(cq_to_quality(f64::from(cq))));
        assert!(
            (realized - cq as i32).abs() <= 1,
            "cq_to_quality roundtrip drift at {cq}: realized {realized}"
        );
    }

    let mut label = "avif_cq".to_string();
    let mut out_dir = PathBuf::from("/mnt/v/output/zensim/avif-loop-2026-08-07");
    let mut iters: usize = 3;
    let mut corpus_file: Option<String> = None;
    let mut targets: Vec<f64> = vec![70.0, 80.0, 88.0];
    let mut arms: Vec<String> = vec!["baseline".into(), "h3-mag".into()];
    let mut bake = "/mnt/v/output/zensim/bakes/sota944/bakes/W10L9_s4003_packed.bin".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--label" => label = args.next().expect("--label value"),
            "--out-dir" => out_dir = PathBuf::from(args.next().expect("--out-dir value")),
            "--iters" => {
                iters = args.next().and_then(|s| s.parse().ok()).expect("--iters N");
            }
            "--corpus-file" => corpus_file = args.next(),
            "--zensim-targets" => {
                targets = args
                    .next()
                    .expect("--zensim-targets list")
                    .split(',')
                    .map(|x| x.trim().parse().expect("finite target score"))
                    .collect();
            }
            "--arms" => {
                arms = args
                    .next()
                    .expect("--arms list")
                    .split(',')
                    .map(|x| x.trim().to_string())
                    .collect();
            }
            "--bake" => bake = args.next().expect("--bake value"),
            other => panic!("unknown flag {other}"),
        }
    }
    assert!(
        !targets.is_empty() && targets.iter().all(|x| x.is_finite()),
        "targets must be nonempty and finite"
    );
    assert!(!arms.is_empty(), "at least one arm is required");
    let mut unique_targets = std::collections::HashSet::new();
    assert!(
        targets.iter().all(|t| unique_targets.insert(t.to_string())),
        "duplicate target score"
    );
    for arm in &arms {
        assert!(
            matches!(arm.as_str(), "baseline" | "h3-mag" | "outer"),
            "--arms '{arm}' is not a known arm (baseline|h3-mag|outer) — \
             refusing the silent fall-through"
        );
    }
    let corpus = corpus_from_file(
        corpus_file
            .as_deref()
            .expect("--corpus-file is required (path\\tname\\tclass rows)"),
    );
    let settings = EncodeSettings {
        speed: env_f64("AVIF_ZENSIM_SPEED", 6.0) as u8,
        threads: 1,
    };
    let model = load_model(&bake);
    let mut scorer = BakeScorer::new(&model).expect("candidate admission");
    let map_model = std::env::var("AVIF_ZENSIM_MAP_BAKE")
        .ok()
        .map(|path| load_model(&path));
    let mut map_scorer = map_model
        .as_ref()
        .map(|m| BakeScorer::new(m).expect("map candidate admission"));
    let encodes = iters.checked_add(1).expect("encode count overflow");
    let emit_best = env_flag("AVIF_ZENSIM_EMIT_BEST");

    let decoded_dir = out_dir.join("decoded");
    let ref_dir = out_dir.join("ref");
    fs::create_dir_all(&decoded_dir).expect("out dir");
    fs::create_dir_all(&ref_dir).expect("ref dir");
    let manifest_path = out_dir.join(format!("target_ab_{label}.tsv"));
    let trace_path = out_dir.join(format!("trace_{label}.tsv"));
    let mut manifest = String::from(
        "image\tclass\ttarget\tarm\tbake\tseed_cq\tachieved_inloop\titers_used\tachieved_decoded\tabs_err\tbytes\tencode_ms\tloop_ms\tms_per_compare\tterminal_verify_ms\tcell_total_ms\tscalar_comparisons\tmap_evaluations\n",
    );
    eprintln!(
        "[zensim_cq_rd] label={label} bake={bake} (complete candidate surface) corpus={} arms={arms:?} \
         targets={targets:?} encodes/cell={encodes} speed={} FRAME_HINTS_LIVE={FRAME_HINTS_LIVE}",
        corpus.len(),
        settings.speed,
    );

    let mut hint_probe_done = false;
    for (name, class, path) in &corpus {
        let (px, w, h) = load_rgb8(path);
        let ref_png = ref_dir.join(format!("{name}.png"));
        if !ref_png.exists() {
            let flat: Vec<u8> = px.iter().flat_map(|p| p.iter().copied()).collect();
            image::RgbImage::from_raw(w as u32, h as u32, flat)
                .expect("ref from_raw")
                .save(&ref_png)
                .expect("save ref");
        }
        let pre = map_scorer
            .as_ref()
            .unwrap_or(&scorer)
            .precompute_reference(&RgbSlice::new(&px, w, h))
            .expect("precompute reference");
        for &t in &targets {
            for arm in &arms {
                if arm == "h3-mag" && !hint_probe_done {
                    assert_hint_engagement(&px, w, h, &settings);
                    hint_probe_done = true;
                    eprintln!("[zensim_cq_rd] h3-mag hint engagement probe PASSED");
                }
                let trace = TraceCtx {
                    path: &trace_path,
                    id: format!("{label}|{name}|{class}|{t}|{arm}"),
                };
                let t_cell = Instant::now();
                let res = match arm.as_str() {
                    "outer" => run_outer_cell(
                        &mut scorer,
                        &px,
                        w,
                        h,
                        t,
                        encodes,
                        &settings,
                        &trace,
                        emit_best,
                    ),
                    inner => run_inner_cell(
                        &mut scorer,
                        map_scorer.as_mut(),
                        &px,
                        w,
                        h,
                        &pre,
                        t,
                        inner == "h3-mag",
                        encodes,
                        &settings,
                        &trace,
                        emit_best,
                    ),
                };
                let cell_ms = t_cell.elapsed().as_secs_f64() * 1e3;
                let err = (res.achieved - t).abs();
                let ms_per_compare = res.loop_ms / res.iters_used.max(1) as f64;
                let dist_png = decoded_dir.join(format!("{label}__{name}__t{t}__{arm}.png"));
                let t_verify = Instant::now();
                let dec = decode_rgb8(&res.bytes, w, h);
                let verified = served_score(
                    &mut scorer,
                    &RgbSlice::new(&px, w, h),
                    &RgbSlice::new(&dec, w, h),
                );
                assert!(
                    (verified - res.achieved).abs() <= 1e-8,
                    "emitted bitstream score mismatch"
                );
                let terminal_verify_ms = t_verify.elapsed().as_secs_f64() * 1e3;
                let cell_total_ms = t_cell.elapsed().as_secs_f64() * 1e3;
                let scalar_comparisons = res.iters_used
                    + 1
                    + usize::from(arm == "h3-mag" && map_scorer.is_some()) * res.iters_used;
                let map_evaluations = usize::from(arm == "h3-mag") * res.iters_used;
                let flat: Vec<u8> = dec.iter().flat_map(|p| p.iter().copied()).collect();
                image::RgbImage::from_raw(w as u32, h as u32, flat)
                    .expect("dec from_raw")
                    .save(&dist_png)
                    .expect("save decoded");
                if env_flag("AVIF_ZENSIM_SAVE_AVIF") {
                    let avif = decoded_dir.join(format!("{label}__{name}__t{t}__{arm}.avif"));
                    fs::write(&avif, &res.bytes).expect("save avif");
                }
                manifest.push_str(&format!(
                    "{name}\t{class}\t{t}\t{arm}\t{bake}\t{:.0}\t{:.3}\t{}\t{:.3}\t{err:.3}\t{}\t{:.1}\t{:.1}\t{ms_per_compare:.1}\t{terminal_verify_ms:.3}\t{cell_total_ms:.3}\t{scalar_comparisons}\t{map_evaluations}\n",
                    res.seed_cq,
                    res.achieved,
                    res.iters_used,
                    verified,
                    res.bytes.len(),
                    res.encode_ms,
                    res.loop_ms,
                ));
                eprintln!(
                    "  [{label}] {name} t={t} {arm}: achieved={:.2} err={err:.2} \
                     encodes={} bytes={} cell={cell_ms:.0}ms",
                    res.achieved,
                    res.iters_used,
                    res.bytes.len(),
                );
            }
        }
    }
    fs::write(&manifest_path, &manifest).expect("write manifest");
    eprintln!("[zensim_cq_rd] wrote {}", manifest_path.display());
}

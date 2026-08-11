//! Zensim target-hitting CQ loop harness — the AVIF analogue of
//! jxl-encoder's `zensim_diffmap_rd` beats-butter series. **Protocol,
//! constants, gates, seeds, and the measured smoke:
//! `benchmarks/zensim_avif_loop_2026-08-07.md`** (appendix AC.4).
//!
//! Per cell: seed encode at the registered seed CQ → decode → folded-944
//! features on (ref, decoded) → mounted-bake forward → the adopted jxl
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
//! `--bake <path>|profile:c` `--iters K` `--label L` `--out-dir D`.
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
// `ZensimProfile::C` and the folded-944 surface come from the plain `zensim`
// dep, which on this branch is git `main` with `custom-profiles` +
// `feature-regime-v2` enabled. The renamed `zensim03` alias this file used to
// need existed only while the main dep was pinned to registry 0.2.4.
use zensim::{PrecomputedReference, RgbSlice, Zensim, ZensimProfile};

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

/// One bake per process (matches the jxl harness' OnceLock contract).
static BAKE_BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
fn bake_bytes() -> &'static [u8] {
    BAKE_BYTES.get().expect("bake bytes loaded").as_slice()
}
static SCORE_PROFILE: std::sync::OnceLock<(ZensimProfile, usize)> = std::sync::OnceLock::new();

/// Smallest-first width probe (the jxl `rd_infer_n_inputs` rule: the
/// forward accepts any width ≥ the bake's caller width, so the FIRST
/// accepted width is tight; a PRUNED bake probes at its CALLER width —
/// the shipped C bake is 944 caller / 667 internal).
fn probe_caller_width(profile: ZensimProfile) -> usize {
    let feats = vec![0.0f64; 944];
    for n in [156usize, 228, 300, 372, 720, 924, 944] {
        if zensim::score_features_with_profile(profile, &feats[..n], 64, 64).is_ok() {
            return n;
        }
    }
    0
}

fn score_profile(bake_arg: &str) -> (ZensimProfile, usize) {
    *SCORE_PROFILE.get_or_init(|| {
        let profile = if let Some(name) = bake_arg.strip_prefix("profile:") {
            match name {
                "c" => ZensimProfile::C,
                other => panic!("unsupported judge profile:{other} (only profile:c)"),
            }
        } else {
            let bytes = std::fs::read(bake_arg).unwrap_or_else(|e| panic!("bake {bake_arg}: {e}"));
            BAKE_BYTES.set(bytes).expect("bake set once");
            let params = zensim::profile::ProfileParams::builder()
                .mlp(bake_bytes)
                .skip_score_mapping(true)
                .extrapolate_score(true)
                .extended_features(true)
                .compute_iw_features(true)
                .build();
            let params: &'static zensim::profile::ProfileParams = Box::leak(Box::new(params));
            ZensimProfile::Custom {
                params,
                name: "avif-cq-rd-bake",
            }
        };
        let n_in = probe_caller_width(profile);
        assert!(
            n_in != 0,
            "bake {bake_arg}: forward accepts no probed feature width — refusing \
             (a silent mount would emit seed-quality bitstreams)"
        );
        // Folded-class only (the `--regime 944` known-bug class):
        assert!(
            n_in >= 720,
            "bake {bake_arg}: caller width {n_in} < 720 — folded-944 features \
             zero f156-371, silently mis-scoring a {n_in}-class bake (zensim \
             CLAUDE.md known bug); use the jxl-series 372-class route instead"
        );
        (profile, n_in)
    })
}

/// Map-side profile for extraction + the fused attribution walk (the jxl
/// `rd_attr_map_profile` shape: no MLP, all basic features, default walk).
static MAP_PROFILE: std::sync::OnceLock<ZensimProfile> = std::sync::OnceLock::new();
fn map_profile() -> ZensimProfile {
    *MAP_PROFILE.get_or_init(|| {
        let params = zensim::profile::ProfileParams::builder()
            .skip_score_mapping(true)
            .extrapolate_score(true)
            .extended_features(true)
            .build();
        let params: &'static zensim::profile::ProfileParams = Box::leak(Box::new(params));
        ZensimProfile::Custom {
            params,
            name: "avif-cq-rd-map",
        }
    })
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
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("open {path}: {e}"))
        .to_rgb8();
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

/// The mounted bake's forward over caller-width-sized features.
fn forward(profile: ZensimProfile, n_in: usize, feats: &[f64], w: usize, h: usize) -> f64 {
    let take = n_in.min(feats.len());
    zensim::score_features_with_profile(profile, &feats[..take], w as u32, h as u32)
        .expect("forward failed after a passing mount probe (wiring bug)")
}

/// Shared per-compare scoring: folded-944 extraction on (ref, decoded) +
/// the mounted bake's forward, sized by the bake's CALLER width.
fn folded_score(
    z: &Zensim,
    profile: ZensimProfile,
    n_in: usize,
    ref_slice: &RgbSlice<'_>,
    dec_slice: &RgbSlice<'_>,
    w: usize,
    h: usize,
) -> (f64, Vec<f64>) {
    let v2 = z
        .compute_folded720_append2_features(ref_slice, dec_slice)
        .expect("folded-944 extraction failed (loud by design)");
    let feats = v2.features();
    let sc = forward(profile, n_in, feats, w, h);
    (sc, feats[..n_in.min(feats.len())].to_vec())
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
fn run_inner_cell(
    z: &Zensim,
    profile: ZensimProfile,
    n_in: usize,
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

    let ref_slice = RgbSlice::new(px, w, h);
    let sb_cols = w.div_ceil(SB);
    let sb_rows = h.div_ceil(SB);
    let n_sb = sb_cols * sb_rows;
    let mut sb_scale = vec![1.0f32; n_sb];
    let mut grad: Option<Vec<f64>> = None;
    let mut session = zensim::Fused944Session::new();

    let seed_cq = seed_cq_for_target(target);
    let mut cq = seed_cq;
    let mut encode_ms = 0.0f64;
    let mut loop_ms = 0.0f64;
    // (|err|, score, bytes) per iterate.
    let mut iterates: Vec<(f64, f64, Vec<u8>)> = Vec::with_capacity(encodes);

    for iter in 0..encodes {
        let t_it = Instant::now();
        // Hints first land on encode 2 (the jxl timeline: the seed
        // compare derives the gradient, encode 1's compare the first
        // map; redistribution at iteration i shapes i+1).
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

        // Score (+ attribution on steered iterations 1+); iter 0 is
        // always the plain extraction (the fused entry needs the grad).
        let mut tile_q: Option<Vec<f64>> = None;
        let score = if let (true, Some(s)) = (steer, grad.as_deref()) {
            let (_res, v2, attr) = z
                .compute_folded944_score_and_attribution_binned(
                    &ref_slice,
                    pre,
                    &dec_slice,
                    s,
                    &mut session,
                    attr_bin,
                )
                .expect("fused folded-944 compare failed (loud by design)");
            let sc = forward(profile, n_in, v2.features(), w, h);
            // Per-64px-SB magnitude signal: query_rect in pixel coords
            // (bin-exact for 64px rects at bin 8; edges clamp exactly).
            let mut q = Vec::with_capacity(n_sb);
            for sby in 0..sb_rows {
                for sbx in 0..sb_cols {
                    let x0 = sbx * SB;
                    let y0 = sby * SB;
                    q.push(attr.query_rect(x0, y0, x0 + SB, y0 + SB));
                }
            }
            tile_q = Some(q);
            sc
        } else {
            let (sc, feats) = folded_score(z, profile, n_in, &ref_slice, &dec_slice, w, h);
            if steer && grad.is_none() {
                let s = zensim::score_features_fd_gradient_with_profile(
                    profile, &feats, w as u32, h as u32,
                )
                .expect("FD gradient failed");
                let nonzero = s.iter().filter(|&&g| g != 0.0).count();
                assert!(
                    nonzero > 0,
                    "h3-mag gradient identically zero — steering could never engage"
                );
                grad = Some(s);
            }
            sc
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
/// midpoints after; judge = the same folded-944 forward.
#[allow(clippy::too_many_arguments)]
fn run_outer_cell(
    z: &Zensim,
    profile: ZensimProfile,
    n_in: usize,
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
        let (judged, _) = folded_score(z, profile, n_in, &ref_slice, &dec_slice, w, h);
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
                    .filter_map(|x| x.trim().parse().ok())
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
    let (profile, n_in) = score_profile(&bake);
    let z = Zensim::new(map_profile()).with_parallel(false);
    let encodes = iters + 1;
    let emit_best = env_flag("AVIF_ZENSIM_EMIT_BEST");

    let decoded_dir = out_dir.join("decoded");
    let ref_dir = out_dir.join("ref");
    fs::create_dir_all(&decoded_dir).expect("out dir");
    fs::create_dir_all(&ref_dir).expect("ref dir");
    let manifest_path = out_dir.join(format!("target_ab_{label}.tsv"));
    let trace_path = out_dir.join(format!("trace_{label}.tsv"));
    let mut manifest = String::from(
        "image\tclass\ttarget\tarm\tbake\tseed_cq\tachieved_inloop\titers_used\tachieved_decoded\tabs_err\tbytes\tencode_ms\tloop_ms\tms_per_compare\n",
    );
    eprintln!(
        "[zensim_cq_rd] label={label} bake={bake} (caller width {n_in}) corpus={} arms={arms:?} \
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
        let pre = z
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
                    id: format!("{label}|{name}|{class}|{t:.0}|{arm}"),
                };
                let t_cell = Instant::now();
                let res = match arm.as_str() {
                    "outer" => run_outer_cell(
                        &z, profile, n_in, &px, w, h, t, encodes, &settings, &trace, emit_best,
                    ),
                    inner => run_inner_cell(
                        &z,
                        profile,
                        n_in,
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
                let dist_png = decoded_dir.join(format!("{label}__{name}__t{t:.0}__{arm}.png"));
                let dec = decode_rgb8(&res.bytes, w, h);
                let flat: Vec<u8> = dec.iter().flat_map(|p| p.iter().copied()).collect();
                image::RgbImage::from_raw(w as u32, h as u32, flat)
                    .expect("dec from_raw")
                    .save(&dist_png)
                    .expect("save decoded");
                if env_flag("AVIF_ZENSIM_SAVE_AVIF") {
                    let avif = decoded_dir.join(format!("{label}__{name}__t{t:.0}__{arm}.avif"));
                    fs::write(&avif, &res.bytes).expect("save avif");
                }
                manifest.push_str(&format!(
                    "{name}\t{class}\t{t:.0}\t{arm}\t{bake}\t{:.0}\t{:.3}\t{}\t{:.3}\t{err:.3}\t{}\t{:.1}\t{:.1}\t{ms_per_compare:.1}\n",
                    res.seed_cq,
                    res.achieved,
                    res.iters_used,
                    res.achieved,
                    res.bytes.len(),
                    res.encode_ms,
                    res.loop_ms,
                ));
                eprintln!(
                    "  [{label}] {name} t={t:.0} {arm}: achieved={:.2} err={err:.2} \
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

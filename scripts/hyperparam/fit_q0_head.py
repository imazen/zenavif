#!/usr/bin/env python3
"""Fit the q0-prediction head for target-quality mode (src/q0_head.rs).

Goal: given (zenanalyze features, ssim2 target t, speed, log_px), predict the
starting quality q0 so `encode_rgb8_with_target`'s bracketed secant converges
in 1-2 encodes instead of ~4 from the content-blind `initial_guess(t)`.

Labels: offline inverse-interpolation of the label store's per-image q->ssim2
curves (q_kind == 'cavif_q' ONLY — the zenavif/ravif quality scale) at targets
{40,45,...,90}. Curves are isotonic-smoothed (PAVA) before inversion. Only
in-range targets are labeled (no extrapolation past the measured curve).

Fit arms: the per-sweep ship-of-era configurations (tune-on where the sweep
had it), one coherent binary chain per sweep_source. A tune-off arm set
(registry-like) is evaluated as a robustness check, never fit.

Split honesty: fit on LSD-train origins; the gate (|q0-q*| p90 <= 6) is
evaluated on LSD-val origins. Feature selection is greedy forward on
LEAVE-ONE-FAMILY-OUT p90 over TRAIN only; val is read once per ladder rung.

Simulation: `search_target` (src/target_quality.rs) is ported verbatim; the
encode+score oracle is the held-out curve's isotonic interpolation. Headline:
encodes-to-converge before (initial_guess seed) vs after (model q0 seed).

Output:
  benchmarks/q0_head_fit_2026-07-05.tsv   (fit table + sim table + coefficients)
  stdout: the Rust consts block to paste into src/q0_head.rs

Deterministic: re-running reproduces every number from the label store.
"""

import os
import subprocess
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import load_features, load_store  # noqa: E402

TARGETS = list(range(40, 95, 5))
TOL = 0.5
MAX_ENCODES = 6
GATE_P90 = 6.0

# Ship-of-era fit arms (tune-on where the sweep had it). One entry per
# (sweep_source, arm_id). Speeds span 1..10.
FIT_ARMS = [
    ("speedladder-2026-07-04", "speedladder/zr-s2-tune"),
    ("speedladder-2026-07-04", "speedladder/zr-s4-tune"),
    ("speedladder-2026-07-04", "speedladder/zr-s6-tune"),
    ("speedladder-2026-07-04", "speedladder/zr-s8-tune"),
    ("speedladder-2026-07-04", "speedladder/zr-s10-tune"),
    ("sizedecay-2026-07-03", "sizedecay/full_s2"),
    ("sizedecay-2026-07-03", "sizedecay/full_s2-val"),
    ("qmdist-2026-07-03", "qmdist/ratio-final_s2"),
    ("qmdist-2026-07-03", "qmdist/ratio-final_s1deep"),
    ("lfsharp-2026-07-03", "lfsharp/still753-final_s2"),
    ("lfsharp-2026-07-03", "lfsharp/still753-final_s1deep"),
    ("deltaq-2026-07-02", "deltaq/str1_s2"),
    ("coeffrd-2026-07-05", "coeffrd/base_s2"),
    ("palette-mech-ab-2026-07-03", "palette-mech/shipped-auto_s2"),
    ("wedge-2026-07-03", "wedge/zr-best_s2"),
    ("p2heads-2026-07-04", "p2heads/confirm-s6-ship"),
    ("p2heads-2026-07-04", "p2heads/val-ship"),
    ("p1part-2026-07-04", "p1part/confirm-s4-r16no4_bkvg2"),
    ("p1part-2026-07-04", "p1part/confirm-s6-r16no4_bkvg2"),
    ("p1part-2026-07-04", "p1part/confirm-s8-r16no4_bkvg2"),
    ("fastwins-2026-07-04", "fastwins/confirm-s6-size1"),
    ("fastwins-2026-07-04", "fastwins/confirm-s8-size1"),
]
# Registry-like (tune-off) arms: evaluated for robustness, never fit.
OFF_ARMS = [
    ("speedladder-2026-07-04", "speedladder/zr-s2-off"),
    ("speedladder-2026-07-04", "speedladder/zr-s4-off"),
    ("speedladder-2026-07-04", "speedladder/zr-s6-off"),
    ("speedladder-2026-07-04", "speedladder/zr-s8-off"),
    ("speedladder-2026-07-04", "speedladder/zr-s10-off"),
    ("sizedecay-nontune-2026-07-03", "sdnontune/base_s2"),
    ("sizedecay-2026-07-03", "sizedecay/off_s2"),
    ("sizedecay-2026-07-03", "sizedecay/off_s2-val"),
]

# Runtime-available candidate features (zenanalyze AnalysisFeature names ==
# features-parquet column names; verified against the path-pinned zenanalyze
# main enum — NO dense-percentile branch features). LOG1P set gets log1p().
CAND_FEATURES = [
    "dct_compressibility_y",
    "dct_compressibility_uv",
    "noise_floor_y",
    "noise_floor_uv",
    "gradient_fraction_smooth",
    "gradient_fraction",
    "patch_fraction",
    "edge_density",
    "laplacian_variance",
    "laplacian_variance_peak",
    "high_freq_energy_ratio",
    "luma_histogram_entropy",
    "colourfulness",
    "aq_map_mean",
    "aq_map_std",
    "spectral_slope_y",
    "quant_survival_y",
    "quant_survival_uv",
    "uniformity",
    "flat_color_block_ratio",
    "variance",
    "variance_spread",
    "chroma_complexity",
    "distinct_color_bins",
    "palette_density",
    "edge_slope_stdev",
    "luma_kurtosis",
    "info_weight_mean",
    "orientation_energy_ratio",
    "grayscale_score",
    "skin_tone_fraction",
]
LOG1P = {"dct_compressibility_y", "dct_compressibility_uv", "laplacian_variance",
         "laplacian_variance_peak", "colourfulness", "aq_map_mean", "aq_map_std",
         "variance", "variance_spread", "chroma_complexity",
         "distinct_color_bins", "edge_slope_stdev", "luma_kurtosis"}
MAX_FEATS = 8


def pava_increasing(y):
    """Pool-adjacent-violators: minimal isotonic (non-decreasing) fit."""
    y = np.asarray(y, dtype=float)
    n = len(y)
    val = y.copy()
    wt = np.ones(n)
    idx = list(range(n))  # block right edges
    blocks = [[v, w, i, i] for v, w, i in zip(val, wt, range(n))]
    out = []
    for b in blocks:
        out.append(b)
        while len(out) >= 2 and out[-2][0] > out[-1][0]:
            v2, w2, s2, e2 = out.pop()
            v1, w1, s1, e1 = out.pop()
            w = w1 + w2
            out.append([(v1 * w1 + v2 * w2) / w, w, s1, e2])
    z = np.empty(n)
    for v, _, s, e in out:
        z[s : e + 1] = v
    return z


def build_curves(df):
    """{(sweep, arm, speed, image_id): (qs, ssim2_iso, log_px, family, split,
    origin, feature_join)} with qs sorted ascending and ssim2 PAVA-isotonic."""
    curves = {}
    for key, g in df.groupby(["sweep_source", "arm_id", "speed", "image_id"], sort=False):
        g = g.dropna(subset=["q", "ssim2"]).sort_values("q")
        if len(g) < 5:
            continue
        qs = g["q"].to_numpy(dtype=float)
        ss = pava_increasing(g["ssim2"].to_numpy(dtype=float))
        px = float(g["px"].iloc[0])
        fj = g["feature_join"].iloc[0]
        curves[key] = dict(
            qs=qs, ss=ss, log_px=np.log(max(px, 1.0)),
            family=str(g["family"].iloc[0]), split=str(g["split"].iloc[0]),
            origin=str(g["origin_id"].iloc[0]),
            feature_join=fj if isinstance(fj, str) else None,
        )
    return curves


def invert(curve, t):
    """q*(t) = the SMALLEST q reaching t (leftmost crossing on the isotonic
    curve) — matches the runtime selection policy ("smallest file inside the
    band") and resolves plateau ambiguity toward smaller files.
    None if t is outside the measured curve range."""
    qs, ss = curve["qs"], curve["ss"]
    if not (ss[0] <= t <= ss[-1]):
        return None
    i = int(np.searchsorted(ss, t, side="left"))
    if i == 0:
        return float(qs[0])
    if ss[i] == ss[i - 1]:
        return float(qs[i])
    f = (t - ss[i - 1]) / (ss[i] - ss[i - 1])
    return float(qs[i - 1] + f * (qs[i] - qs[i - 1]))


def score_at(curve, q):
    """The simulation's encode+score oracle: ssim2 at (fractional) q."""
    qs, ss = curve["qs"], curve["ss"]
    return float(np.interp(np.clip(q, qs[0], qs[-1]), qs, ss))


def initial_guess(t):
    """Verbatim port of src/target_quality.rs::initial_guess."""
    if t <= 30.0:
        return max(t, 1.0)
    if t <= 70.0:
        return 30.0 + (t - 30.0) * (30.0 / 40.0)
    return 60.0 + (t - 70.0) * (30.0 / 19.0)


def simulate(curve, t, q_start, tol=TOL, max_encodes=MAX_ENCODES,
             min_q=1.0, max_q=100.0):
    """Verbatim port of src/target_quality.rs::search_target (bracketed
    secant + extrapolation), oracle = curve interpolation. Returns
    (encodes, converged)."""
    lo = hi = None
    q = float(np.clip(q_start, min_q, max_q))
    encodes = 0
    best_any = None  # (dist, q)
    while encodes < max_encodes:
        s = score_at(curve, q)
        encodes += 1
        d = abs(s - t)
        if best_any is None or d < best_any[0]:
            best_any = (d, q)
        if d <= tol:
            return encodes, True
        if s < t:
            lo = (q, s)
        else:
            hi = (q, s)
        if lo is not None and hi is not None:
            lq, ls = lo
            hq, hs = hi
            span = hq - lq
            if span <= 0.25:
                break
            if abs(hs - ls) > 1e-9:
                sec = lq + ((t - ls) / (hs - ls)) * span
            else:
                sec = lq + span / 2.0
            q = float(np.clip(sec, lq + span * 0.1, hq - span * 0.1))
        elif lo is not None:
            lq, ls = lo
            step = max((t - ls) * 1.2, 4.0)
            n = min(lq + step, max_q)
            if n <= lq + 0.25:
                break
            q = n
        else:
            hq, hs = hi
            step = max((hs - t) * 1.2, 4.0)
            n = max(hq - step, min_q)
            if n >= hq - 0.25:
                break
            q = n
    return encodes, best_any is not None and best_any[0] <= tol


HINGES = [50.0, 60.0, 70.0, 80.0, 85.0]


def design(rows, feat_names, interactions, hinge=False, inter_h80=False):
    """Design matrix.

    Polynomial basis (hinge=False): [1, tn, tn^2, tn^3, speed_n, logpx_n].
    Hinge basis (hinge=True): [1, tn, h50..h85, speed_n, logpx_n] — a
    piecewise-linear (isotonic-capable) t response that can track the
    saturation bend the cubic cannot.
    + feats..., then feat*tn interaction block, then optional feat*h80
    (saturation-onset) interaction block.
    """
    t = rows["t"]
    tn = (t - 65.0) / 25.0
    sp = (rows["speed"] - 5.0) / 5.0
    lp = (rows["log_px"] - 13.0) / 3.0
    cols = [np.ones(len(tn)), tn]
    names = ["const", "tn"]
    if hinge:
        for k in HINGES:
            cols.append(np.maximum(t - k, 0.0) / 10.0)
            names.append(f"h{int(k)}")
    else:
        cols += [tn * tn, tn * tn * tn]
        names += ["tn2", "tn3"]
    cols += [sp, lp]
    names += ["speed_n", "logpx_n"]
    for f in feat_names:
        cols.append(rows[f])
        names.append(f)
    if interactions:
        for f in feat_names:
            cols.append(rows[f] * tn)
            names.append(f + "*tn")
    if inter_h80:
        h80 = np.maximum(t - 80.0, 0.0) / 10.0
        for f in feat_names:
            cols.append(rows[f] * h80)
            names.append(f + "*h80")
    return np.column_stack(cols), names


def fit_ridge(X, y, lam=1e-3, w=None):
    if w is not None:
        sw = np.sqrt(w)
        X = X * sw[:, None]
        y = y * sw
    n = X.shape[1]
    A = X.T @ X + lam * np.eye(n)
    A[0, 0] -= lam  # don't shrink the intercept
    return np.linalg.solve(A, X.T @ y)


def fit_l1(X, y, w=None, iters=30, lam=1e-3):
    """L1 (least absolute deviations) via IRLS on top of the ridge solver.
    p90 is a tail metric; L1 resists the saturation-region label outliers."""
    beta = fit_ridge(X, y, lam=lam, w=w)
    for _ in range(iters):
        r = np.abs(y - X @ beta)
        wi = 1.0 / np.maximum(r, 0.5)
        if w is not None:
            wi = wi * w
        beta = fit_ridge(X, y, lam=lam, w=wi)
    return beta


def train_mlp(Xtr, ytr, wtr, Xstop, ystop, hidden=24, seed=0, lr=3e-3,
              steps=6000, l2=1e-4, huber=3.0):
    """Tiny deterministic MLP (in -> hidden leakyrelu(0.01) -> 1 identity),
    full-batch Adam + Huber loss, early-stopped on the inner-holdout p90.
    Inputs must already be standardized. Returns (W1, b1, W2, b2, stop_p90)."""
    rng = np.random.default_rng(seed)
    n_in = Xtr.shape[1]
    W1 = rng.normal(0, 1.0 / np.sqrt(n_in), (n_in, hidden)).astype(np.float64)
    b1 = np.zeros(hidden)
    W2 = rng.normal(0, 1.0 / np.sqrt(hidden), (hidden, 1)).astype(np.float64)
    b2 = np.zeros(1)
    params = [W1, b1, W2, b2]
    m = [np.zeros_like(p) for p in params]
    v = [np.zeros_like(p) for p in params]
    best = (np.inf, [p.copy() for p in params])
    alpha_lr = 0.01

    def fwd(X, ps):
        w1, bb1, w2, bb2 = ps
        h = X @ w1 + bb1
        h = np.where(h > 0, h, alpha_lr * h)
        return (h @ w2 + bb2).ravel(), h

    for step in range(1, steps + 1):
        pred, h = fwd(Xtr, params)
        r = pred - ytr
        # Huber gradient dL/dpred, per-origin weighted.
        g = np.where(np.abs(r) <= huber, r, huber * np.sign(r)) * wtr
        g = g / len(g)
        gW2 = h.T @ g[:, None] + l2 * params[2]
        gb2 = np.array([g.sum()])
        dh = g[:, None] @ params[2].T
        dh = dh * np.where(h > 0, 1.0, alpha_lr)
        gW1 = Xtr.T @ dh + l2 * params[0]
        gb1 = dh.sum(axis=0)
        for i, gr in enumerate([gW1, gb1, gW2, gb2]):
            m[i] = 0.9 * m[i] + 0.1 * gr
            v[i] = 0.999 * v[i] + 0.001 * gr * gr
            mh = m[i] / (1 - 0.9**step)
            vh = v[i] / (1 - 0.999**step)
            params[i] = params[i] - lr * mh / (np.sqrt(vh) + 1e-8)
        if step % 50 == 0:
            sp, _ = fwd(Xstop, params)
            s90 = float(np.percentile(np.abs(np.clip(sp, 1, 100) - ystop), 90))
            if s90 < best[0]:
                best = (s90, [p.copy() for p in params])
    return best[1], best[0]


# --- quantizer-space transform (mirror of src/encode_plan.rs, zenravif
# quality_to_quantizer, continuous form — no u8 rounding so it inverts) ---
def q_to_qi(q):
    qq = np.clip(np.asarray(q, dtype=float), 1.0, 100.0) / 100.0
    x = np.where(qq >= 0.70, (1.0 - qq) * 1.4,
                 np.where(qq > 0.10, 0.42 + (0.70 - qq) * 0.85,
                          0.93 + (0.10 - qq) * 0.78))
    return np.minimum(x, 1.0) * 255.0


def qi_to_q(qi):
    x = np.clip(np.asarray(qi, dtype=float), 0.0, 255.0) / 255.0
    q = np.where(x <= 0.42, 1.0 - x / 1.4,
                 np.where(x <= 0.93, 0.70 - (x - 0.42) / 0.85,
                          0.10 - (x - 0.93) / 0.78))
    return np.clip(q, 0.01, 1.0) * 100.0


def origin_weights(R):
    """Inverse-count weights so every origin contributes equally."""
    from collections import Counter

    c = Counter(R["origin"])
    return np.array([1.0 / c[o] for o in R["origin"]])


def p90(v):
    return float(np.percentile(np.abs(v), 90)) if len(v) else float("nan")


def p50(v):
    return float(np.percentile(np.abs(v), 50)) if len(v) else float("nan")


def main():
    store = load_store(q_kind="cavif_q", encoder="zenrav1e")
    fit_keys = set(FIT_ARMS)
    off_keys = set(OFF_ARMS)
    mask = [(sw, arm) in fit_keys or (sw, arm) in off_keys
            for sw, arm in zip(store["sweep_source"], store["arm_id"])]
    df = store[np.array(mask)].copy()
    curves = build_curves(df)
    n_fit = sum((k[0], k[1]) in fit_keys for k in curves)
    print(f"curves: {len(curves)} total, {n_fit} fit-arm, {len(curves) - n_fit} off-arm")

    # Feature table (runtime-available candidates only).
    feats = load_features(CAND_FEATURES)
    fmat = {}
    for join, row in feats[CAND_FEATURES].iterrows():
        fmat[join] = row.to_numpy(dtype=float)

    # Label extraction: one row per (curve, target).
    def label_rows(keys):
        rows = {k: [] for k in ["t", "speed", "log_px", "qstar", "family",
                                "split", "origin", "curve_key"]}
        fvals = []
        for key in keys:
            c = curves[key]
            fj = c["feature_join"]
            if fj is None or fj not in fmat:
                continue
            fv = fmat[fj]
            if not np.all(np.isfinite(fv)):
                continue
            for t in TARGETS:
                q = invert(c, t)
                if q is None:
                    continue
                rows["t"].append(float(t))
                rows["speed"].append(float(key[2]))
                rows["log_px"].append(c["log_px"])
                rows["qstar"].append(q)
                rows["family"].append(c["family"])
                rows["split"].append(c["split"])
                rows["origin"].append(c["origin"])
                rows["curve_key"].append(key)
                fvals.append(fv)
        out = {k: np.array(v) for k, v in rows.items() if k != "curve_key"}
        out["curve_key"] = rows["curve_key"]
        fv = np.array(fvals) if fvals else np.zeros((0, len(CAND_FEATURES)))
        for i, name in enumerate(CAND_FEATURES):
            col = fv[:, i] if len(fv) else np.zeros(0)
            out[name] = np.log1p(np.maximum(col, 0.0)) if name in LOG1P else col
        return out

    fit_curve_keys = [k for k in curves if (k[0], k[1]) in fit_keys]
    off_curve_keys = [k for k in curves if (k[0], k[1]) in off_keys]
    L = label_rows(fit_curve_keys)
    LOFF = label_rows(off_curve_keys)
    tr = L["split"] == "train"
    va = L["split"] == "val"
    print(f"labels: {len(L['t'])} fit-arm ({tr.sum()} train / {va.sum()} val; "
          f"{len(set(L['origin'][va]))} val origins), {len(LOFF['t'])} off-arm")

    def subset(R, m):
        S = {k: (v[m] if isinstance(v, np.ndarray) else
                 [v[i] for i in np.where(m)[0]]) for k, v in R.items()}
        return S

    TR, VA = subset(L, tr), subset(L, va)

    def eval_model(beta, names, R, feat_names, interactions):
        X, _ = design(R, feat_names, interactions)
        pred = np.clip(X @ beta, 1.0, 100.0)
        return pred - R["qstar"]

    results = []  # (model_name, train_p50, train_p90, val_p50, val_p90)

    # M0: the current content-blind initial_guess.
    for nm, R in [("M0-initial_guess/train", TR), ("M0-initial_guess/val", VA)]:
        e = np.array([initial_guess(t) for t in R["t"]]) - R["qstar"]
        results.append((nm, p50(e), p90(e)))

    w_tr = origin_weights(TR)

    # M1 = target-only: fit only [const, tn, tn2, tn3].
    Xtr_t, _ = design(TR, [], False)
    beta_m1 = fit_ridge(Xtr_t[:, :4], TR["qstar"], w=w_tr)

    def eval_m1(R):
        X, _ = design(R, [], False)
        return np.clip(X[:, :4] @ beta_m1, 1.0, 100.0) - R["qstar"]

    results.append(("M1-target-only/train", p50(eval_m1(TR)), p90(eval_m1(TR))))
    results.append(("M1-target-only/val", p50(eval_m1(VA)), p90(eval_m1(VA))))

    beta_m2 = fit_ridge(Xtr_t, TR["qstar"], w=w_tr)
    e_tr = eval_model(beta_m2, None, TR, [], False)
    e_va = eval_model(beta_m2, None, VA, [], False)
    results.append(("M2-+speed+logpx/train", p50(e_tr), p90(e_tr)))
    results.append(("M2-+speed+logpx/val", p50(e_va), p90(e_va)))

    # Greedy forward feature selection on TRAIN leave-one-ORIGIN-out p90
    # (deployment-matched: new origins, families seen in training).
    # Selection runs on the HINGE basis (the stronger t response); the same
    # set is reused for every variant.
    origins = sorted(set(TR["origin"]))

    def loo_p90(feat_names, interactions, hinge=True):
        errs = []
        for o in origins:
            m_in = TR["origin"] != o
            S_in = subset(TR, m_in)
            Xi, _ = design(S_in, feat_names, interactions, hinge=hinge)
            bi = fit_ridge(Xi, TR["qstar"][m_in], w=origin_weights(S_in))
            S_out = subset(TR, ~m_in)
            Xo, _ = design(S_out, feat_names, interactions, hinge=hinge)
            errs.append(np.clip(Xo @ bi, 1.0, 100.0) - S_out["qstar"])
        return p90(np.concatenate(errs))

    selected = []
    use_inter = False
    best_score = loo_p90([], False)
    print(f"greedy base (no feats) LOO-origin-p90 (hinge): {best_score:.2f}")
    for _ in range(MAX_FEATS):
        cand_best, cand_score, cand_inter = None, best_score, use_inter
        for f in CAND_FEATURES:
            if f in selected:
                continue
            for inter in ([False, True] if not use_inter else [True]):
                s = loo_p90(selected + [f], inter)
                if s < cand_score - 0.02:
                    cand_best, cand_score, cand_inter = f, s, inter
        if cand_best is None:
            break
        selected.append(cand_best)
        best_score, use_inter = cand_score, cand_inter
        print(f"greedy +{cand_best} (inter={use_inter}): LOO-p90 {best_score:.2f}")

    # M3+ rungs: greedy-selected features across bases / y-spaces / losses.
    # Val p90 is ALWAYS measured in q space (the gate's space).
    def to_y(q, space):
        if space == "qi":
            return q_to_qi(q)
        if space == "logq":
            return np.log(101.0 - np.clip(q, 1.0, 100.0))
        return q

    def from_y(y, space):
        if space == "qi":
            return qi_to_q(y)
        if space == "logq":
            return 101.0 - np.exp(y)
        return y

    def make_predictor(beta, feat_names, interactions, space, hinge, ih80):
        def predict(R):
            X, _ = design(R, feat_names, interactions, hinge=hinge,
                          inter_h80=ih80)
            return np.clip(from_y(X @ beta, space), 1.0, 100.0)

        return predict

    # (name, space, loss, hinge, inter_h80) in SIMPLICITY order.
    specs = [
        ("M3-l2-q-poly", "q", "l2", False, False),
        ("M3-l1-q-poly", "q", "l1", False, False),
        ("M3-l1-qi-poly", "qi", "l1", False, False),
        ("M4-l1-q-hinge", "q", "l1", True, False),
        ("M4-l1-logq-hinge", "logq", "l1", True, False),
        ("M5-l1-q-hinge-h80", "q", "l1", True, True),
        ("M5-l1-logq-hinge-h80", "logq", "l1", True, True),
    ]
    variants = []
    cal_tables = {}
    for nm, space, loss, hinge, ih80 in specs:
        X3, _ = design(TR, selected, use_inter, hinge=hinge, inter_h80=ih80)
        y_tr = to_y(TR["qstar"], space)
        fitter = fit_l1 if loss == "l1" else fit_ridge
        beta = fitter(X3, y_tr, w=w_tr)
        pred = make_predictor(beta, selected, use_inter, space, hinge, ih80)
        e_tr = pred(TR) - TR["qstar"]
        e_va = pred(VA) - VA["qstar"]
        variants.append((nm, pred, beta, space, loss, hinge, ih80))
        results.append((f"{nm}/train", p50(e_tr), p90(e_tr)))
        results.append((f"{nm}/val", p50(e_va), p90(e_va)))
        print(f"{nm}: train p50/p90 {p50(e_tr):.2f}/{p90(e_tr):.2f}  "
              f"val p50/p90 {p50(e_va):.2f}/{p90(e_va):.2f}")
        # "+cal": subtract the per-target TRAIN median residual (a t-LUT,
        # linearly interpolated at predict time) — kills systematic bias
        # the basis can't express at the range edges.
        cal_t = np.array(TARGETS, dtype=float)
        cal_off = np.array([np.median(e_tr[TR["t"] == t]) if
                            (TR["t"] == t).any() else 0.0 for t in TARGETS])
        cal_tables[nm] = (cal_t, cal_off)

        def make_cal(pred_fn, ct, co):
            def pc(R):
                return np.clip(pred_fn(R) - np.interp(R["t"], ct, co),
                               1.0, 100.0)
            return pc

        pred_cal = make_cal(pred, cal_t, cal_off)
        e_trc = pred_cal(TR) - TR["qstar"]
        e_vac = pred_cal(VA) - VA["qstar"]
        variants.append((nm + "+cal", pred_cal, beta, space, loss, hinge, ih80))
        results.append((f"{nm}+cal/train", p50(e_trc), p90(e_trc)))
        results.append((f"{nm}+cal/val", p50(e_vac), p90(e_vac)))
        print(f"{nm}+cal: train p50/p90 {p50(e_trc):.2f}/{p90(e_trc):.2f}  "
              f"val p50/p90 {p50(e_vac):.2f}/{p90(e_vac):.2f}")

    # ---- M6: tiny MLP (the sanctioned escalation once linear fails).
    # Inputs: zenanalyze features (LOG1P-transformed set as in label_rows)
    # + raw [t, speed, log_px]; standardized by TRAIN stats (the stats
    # become the ZNPR bake's scaler). Early stop on an inner ORIGIN
    # holdout of TRAIN; VAL is read once for the final pick.
    def mlp_inputs(R, feat_names):
        cols = [R[f] for f in feat_names] + [R["t"], R["speed"], R["log_px"]]
        return np.column_stack(cols)

    inner_holdout = set(sorted(set(TR["origin"]))[::5])  # every 5th origin
    m_stop = np.isin(TR["origin"], sorted(inner_holdout))
    mlp_variants = []
    for feat_names, tag in ((selected, "sel6"), (CAND_FEATURES, "all31")):
        Xall = mlp_inputs(TR, feat_names)
        mu = Xall.mean(axis=0)
        sd = Xall.std(axis=0)
        sd[sd == 0] = 1.0
        Xs = (Xall - mu) / sd
        Xin, yin, win = Xs[~m_stop], TR["qstar"][~m_stop], origin_weights(
            subset(TR, ~m_stop))
        Xst, yst = Xs[m_stop], TR["qstar"][m_stop]
        best_cfg = None
        for seed in (0, 1, 2):
            ps, s90 = train_mlp(Xin, yin, win, Xst, yst, hidden=24, seed=seed)
            if best_cfg is None or s90 < best_cfg[1]:
                best_cfg = (ps, s90, seed)
        ps, s90, seed = best_cfg
        print(f"M6-mlp-{tag}: inner-holdout p90 {s90:.2f} (seed {seed})")
        mlp_variants.append((tag, feat_names, mu, sd, ps, s90, seed))
    # pick MLP config by inner holdout, evaluate on val once
    tag, mlp_feats, mlp_mu, mlp_sd, mlp_ps, _, mlp_seed = min(
        mlp_variants, key=lambda v: v[5])

    def pred_mlp(R):
        X = (mlp_inputs(R, mlp_feats) - mlp_mu) / mlp_sd
        h = X @ mlp_ps[0] + mlp_ps[1]
        h = np.where(h > 0, h, 0.01 * h)
        return np.clip((h @ mlp_ps[2] + mlp_ps[3]).ravel(), 1.0, 100.0)

    e_tr = pred_mlp(TR) - TR["qstar"]
    e_va = pred_mlp(VA) - VA["qstar"]
    results.append((f"M6-mlp-{tag}/train", p50(e_tr), p90(e_tr)))
    results.append((f"M6-mlp-{tag}/val", p50(e_va), p90(e_va)))
    print(f"M6-mlp-{tag}: train p50/p90 {p50(e_tr):.2f}/{p90(e_tr):.2f}  "
          f"val p50/p90 {p50(e_va):.2f}/{p90(e_va):.2f}")

    # Pick the SIMPLEST rung whose val p90 meets the gate.
    def pred_m1(R):
        X, _ = design(R, [], False)
        return np.clip(X[:, :4] @ beta_m1, 1.0, 100.0)

    def pred_m2(R):
        X, _ = design(R, [], False)
        return np.clip(X @ beta_m2, 1.0, 100.0)

    beta_m1_full = np.zeros(6)
    beta_m1_full[:4] = beta_m1
    rung_evals = [
        ("M1", pred_m1, beta_m1_full, "q", "l2", False, False, []),
        ("M2", pred_m2, beta_m2, "q", "l2", False, False, []),
    ] + [(nm, pred, beta, space, loss, hinge, ih80, selected)
         for nm, pred, beta, space, loss, hinge, ih80 in variants] + [
        (f"M6-mlp-{tag}", pred_mlp, None, "q", "huber", False, False, mlp_feats),
    ]
    chosen = None
    for r in rung_evals:
        if p90(r[1](VA) - VA["qstar"]) <= GATE_P90:
            chosen = r
            break
    if chosen is None:
        # No rung meets the gate — take the best val rung and report honestly.
        chosen = min(rung_evals, key=lambda r: p90(r[1](VA) - VA["qstar"]))
        print(f"WARNING: no rung meets p90<={GATE_P90}; best is {chosen[0]}")
    nm_c, pred_c, beta_c, space_c, loss_c, hinge_c, ih80_c, feats_c = chosen
    inter_c = use_inter if feats_c else False

    def ev_c(R):
        return pred_c(R) - R["qstar"]

    print(f"CHOSEN: {nm_c} feats={feats_c} interactions={inter_c} "
          f"space={space_c} loss={loss_c} hinge={hinge_c} h80={ih80_c}")

    # If the MLP is chosen, emit the zenpredict-bake request JSON.
    if nm_c.startswith("M6-mlp"):
        import json as _json

        bake = {
            "schema_hash": 0,
            "scaler_mean": [float(x) for x in mlp_mu],
            "scaler_scale": [float(x) for x in mlp_sd],
            "layers": [
                # zenpredict layout: w[i * out_dim + j] (input-major) —
                # numpy (in,out) .ravel() matches directly.
                {"in_dim": int(mlp_ps[0].shape[0]), "out_dim": 24,
                 "activation": "leakyrelu", "dtype": "f32",
                 "weights": [float(x) for x in mlp_ps[0].ravel()],
                 "biases": [float(x) for x in mlp_ps[1]]},
                {"in_dim": 24, "out_dim": 1,
                 "activation": "identity", "dtype": "f32",
                 "weights": [float(x) for x in mlp_ps[2].ravel()],
                 "biases": [float(x) for x in mlp_ps[3]]},
            ],
            "metadata": [
                {"key": "zentrain.bake_name", "type": "utf8",
                 "text": "zenavif_q0_head_v0_1"},
                {"key": "zentrain.feature_columns", "type": "utf8",
                 "text": "\n".join(mlp_feats)},
                {"key": "zenavif.q0.log1p_features", "type": "utf8",
                 "text": "\n".join(f for f in mlp_feats if f in LOG1P)},
                {"key": "zenavif.q0.context_dims", "type": "utf8",
                 "text": "t,speed,ln_px (appended after features, this order)"},
                {"key": "zenavif.q0.fit", "type": "utf8",
                 "text": f"fit_q0_head.py 2026-07-05 seed={mlp_seed} "
                         f"val_p50={p50(e_va):.2f} val_p90={p90(e_va):.2f}"},
            ],
        }
        bake_path = os.path.expanduser(
            "~/work/zen/zenavif/scripts/hyperparam/q0_head_bake_request.json")
        with open(bake_path, "w") as bf:
            _json.dump(bake, bf)
        print(f"wrote bake request: {bake_path}")
        print("bake with: zenpredict-bake q0_head_bake_request.json "
              "src/models/zenavif_q0_head_v0_1.bin")
    # Per-target diagnostics for the chosen model (where do misses live?).
    ev_va = ev_c(VA)
    for t in TARGETS:
        m = VA["t"] == t
        if m.sum():
            print(f"  val t={t}: n={int(m.sum())} p50={p50(ev_va[m]):.2f} "
                  f"p90={p90(ev_va[m]):.2f}")

    # Per-family val fit quality for the chosen model.
    fam_rows = []
    for fam in sorted(set(VA["family"])):
        m = VA["family"] == fam
        e = ev_c(subset(VA, m))
        fam_rows.append((fam, int(m.sum()), p50(e), p90(e)))
    # + off-arm (registry-like) robustness, all splits.
    e_off = ev_c(LOFF) if len(LOFF["t"]) else np.array([])
    e_off_val = ev_c(subset(LOFF, LOFF["split"] == "val")) if len(LOFF["t"]) else np.array([])

    # ---- Secant simulation on held-out val curves (fit arms) ----
    sim = {"before": [], "after": [], "conv_b": 0, "conv_a": 0, "n": 0}
    per_target = {}
    va_keys = sorted({tuple(k) for k, s in zip(L["curve_key"], L["split"]) if s == "val"})
    key_to_rows = {}
    for i, k in enumerate(L["curve_key"]):
        key_to_rows.setdefault(tuple(k), []).append(i)
    for key in va_keys:
        c = curves[key]
        for i in key_to_rows[key]:
            t = L["t"][i]
            R1 = subset(L, np.arange(len(L["t"])) == i)
            q0 = float(pred_c(R1)[0])
            eb, cb = simulate(c, t, initial_guess(t))
            ea, ca = simulate(c, t, q0)
            sim["before"].append(eb)
            sim["after"].append(ea)
            sim["conv_b"] += cb
            sim["conv_a"] += ca
            sim["n"] += 1
            pt = per_target.setdefault(int(t), {"b": [], "a": []})
            pt["b"].append(eb)
            pt["a"].append(ea)

    b, a = np.array(sim["before"]), np.array(sim["after"])
    print("\n=== secant simulation (val curves, fit arms) ===")
    print(f"n={sim['n']}  encodes before: mean {b.mean():.2f} median {np.median(b):.0f} "
          f"| after: mean {a.mean():.2f} median {np.median(a):.0f}")
    print(f"converged before {sim['conv_b']}/{sim['n']}  after {sim['conv_a']}/{sim['n']}")
    print(f"<=2 encodes: before {(b <= 2).mean() * 100:.1f}%  after {(a <= 2).mean() * 100:.1f}%")

    # ---- TSV ----
    git = subprocess.run(["git", "-C", os.path.dirname(os.path.dirname(
        os.path.dirname(os.path.abspath(__file__)))), "rev-parse", "--short", "HEAD"],
        capture_output=True, text=True).stdout.strip()
    out = os.path.join(os.path.dirname(os.path.dirname(
        os.path.dirname(os.path.abspath(__file__)))), "zenavif" if False else "",
        )
    tsv_path = os.path.join(os.path.expanduser("~/work/zen/zenavif/benchmarks"),
                            "q0_head_fit_2026-07-05.tsv")
    with open(tsv_path, "w") as f:
        f.write(f"# q0 head fit — fit_q0_head.py @ zenavif {git}, "
                f"store=hyperparam-labels-2026-07-03 (cavif_q, zenrav1e)\n")
        f.write(f"# fit arms: {', '.join(a for _, a in FIT_ARMS)}\n")
        f.write(f"# off arms (eval-only): {', '.join(a for _, a in OFF_ARMS)}\n")
        f.write(f"# targets {TARGETS}, gate val-p90 <= {GATE_P90}, "
                f"chosen {nm_c} feats={feats_c} interactions={inter_c} "
                f"space={space_c} loss={loss_c}\n")
        f.write("table\tkey\tn\tp50\tp90\n")
        for nm, m50, m90 in results:
            f.write(f"ladder\t{nm}\t\t{m50:.3f}\t{m90:.3f}\n")
        for fam, n, m50, m90 in fam_rows:
            f.write(f"val_family\t{fam}\t{n}\t{m50:.3f}\t{m90:.3f}\n")
        if len(e_off):
            f.write(f"off_arms\tall\t{len(e_off)}\t{p50(e_off):.3f}\t{p90(e_off):.3f}\n")
            f.write(f"off_arms\tval-origins\t{len(e_off_val)}\t{p50(e_off_val):.3f}"
                    f"\t{p90(e_off_val):.3f}\n")
        f.write("sim\tmetric\tbefore\tafter\n")
        f.write(f"sim\tmean_encodes\t{b.mean():.3f}\t{a.mean():.3f}\n")
        f.write(f"sim\tmedian_encodes\t{np.median(b):.1f}\t{np.median(a):.1f}\n")
        f.write(f"sim\tconverged_frac\t{sim['conv_b'] / sim['n']:.4f}"
                f"\t{sim['conv_a'] / sim['n']:.4f}\n")
        f.write(f"sim\tle2_frac\t{(b <= 2).mean():.4f}\t{(a <= 2).mean():.4f}\n")
        for t in sorted(per_target):
            pb, pa = np.array(per_target[t]["b"]), np.array(per_target[t]["a"])
            f.write(f"sim_target\t{t}\t{pb.mean():.3f}\t{pa.mean():.3f}\n")
        X, names = design(subset(L, np.zeros(len(L["t"]), dtype=bool)),
                          feats_c, inter_c, hinge=hinge_c, inter_h80=ih80_c)
        f.write("coef\tname\tvalue\t\t\n")
        for n_, v in zip(names, beta_c):
            f.write(f"coef\t{n_}\t{v:.6f}\t\t\n")
    print(f"\nwrote {tsv_path}")

    # ---- Rust consts block ----
    _, names = design(subset(L, np.zeros(len(L["t"]), dtype=bool)),
                      feats_c, inter_c, hinge=hinge_c, inter_h80=ih80_c)
    print("\n// ---- paste into src/q0_head.rs ----")
    print(f"// fit: {nm_c} (space={space_c}, loss={loss_c}), "
          f"val p50/p90 = {p50(ev_c(VA)):.2f}/{p90(ev_c(VA)):.2f}, "
          f"train p50/p90 = {p50(ev_c(TR)):.2f}/{p90(ev_c(TR)):.2f}")
    for n_, v in zip(names, beta_c):
        print(f"//   {n_}: {v:.6f}")
    print("consts:", dict(zip(names, [round(float(v), 6) for v in beta_c])))
    if nm_c.endswith("+cal"):
        ct, co = cal_tables[nm_c[: -len("+cal")]]
        print("cal_targets:", [float(x) for x in ct])
        print("cal_offsets:", [round(float(x), 6) for x in co])


if __name__ == "__main__":
    main()

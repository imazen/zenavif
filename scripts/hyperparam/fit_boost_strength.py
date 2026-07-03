#!/usr/bin/env python3
"""Threshold-rule first cut: per-image variance-boost strength (FEATURE_HINTS §E head,
wedge #2 / deltaq follow-up).

Labels: deltaq-2026-07-02 strength arms {0,1,2,3,4.5,6} on train26 (24 TRAIN-LSD
origins x 12-pt Q, s2+Tune::Ssimulacra2). Per-image direct BD vs the strength-0
arm on ssim2 + butteraugli(3n,max), bd_arm.py conventions.

Objective (matches the program's pre-registered aggregate rule): minimize mean
per-image ssim2 BD of the realized policy; the POLICY's median ba3n must stay
<= +1.0% and median bamax <= +1.5% vs boost-off. Per-image-veto labeling is
also reported (conservative view) but not the fit objective.

Class ceiling 2.0: strengths >= 3 are per-image butteraugli-vetoed on 1/3+ of
the corpus and non-monotone across strength (5048 str2 vetoed but str3 clean);
a 24-point fit has no business predicting them. str2 already captures most of
the 5004-class headroom (-15.04 of -17.99).

Honesty: all 24 origins are TRAIN under the LSD rule — NO val labels exist for
this head. LOOCV (leave-one-origin-out, full threshold search re-run per fold)
is the stand-in generalization estimate; a val-origin strength sweep is the
data need before landing anything.
"""

import itertools
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import load_features, load_store, to_tsv, veto_table  # noqa: E402

OUT_TSV = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "../../benchmarks/hyperparam_boost_rule_2026-07-03.tsv")

# Pre-registered rule-feature shortlist: the three the task names + the three
# strongest |rho| str1-gain correlates from the exploratory scan.
SHORTLIST = ["aq_map_std", "gradient_fraction_smooth", "luma_histogram_entropy",
             "noise_floor_y", "flat_color_block_ratio", "spectral_slope_y"]
ARMS = {1.0: "deltaq/str1_s2", 2.0: "deltaq/str2_s2", 3.0: "deltaq/str3_s2",
        4.5: "deltaq/str4.5_s2", 6.0: "deltaq/str6_s2"}
CLASSES = [0.0, 1.0, 2.0]
BASE = "deltaq/str0_s2"
SRC = "deltaq-2026-07-02"
VETO_3N, VETO_MAX = 1.0, 1.5


def build_matrix():
    store = load_store(sweep_source=SRC, corpus="train26")
    feats = load_features(SHORTLIST)
    meta = (store[["image_id", "origin_id", "content_class", "feature_join"]]
            .drop_duplicates().set_index("image_id"))
    M = pd.DataFrame(index=sorted(meta.index))
    for c in ("ssim2", "ba3n", "bamax"):
        M[f"s0_{c}"] = 0.0
    for s, arm in ARMS.items():
        t = veto_table(store, SRC, BASE, arm)
        M[f"s{s:g}_ssim2"] = t["bd_ssim2"]
        M[f"s{s:g}_ba3n"] = t["bd_ba3n"]
        M[f"s{s:g}_bamax"] = t["bd_bamax"]
    M["origin_id"] = meta["origin_id"]
    M["content_class"] = meta["content_class"]
    for f in SHORTLIST:
        M[f] = meta["feature_join"].map(feats[f])
    return M


class Vec:
    """Vectorized policy evaluation over one image subset."""

    def __init__(self, M, idx):
        self.idx = list(idx)
        self.n = len(self.idx)
        sub = M.loc[self.idx]
        self.S = {s: sub[f"s{s:g}_ssim2"].to_numpy() for s in CLASSES}
        self.B3 = {s: sub[f"s{s:g}_ba3n"].to_numpy() for s in CLASSES}
        self.BM = {s: sub[f"s{s:g}_bamax"].to_numpy() for s in CLASSES}
        self.F = {f: sub[f].to_numpy() for f in SHORTLIST}

    def eval_assign(self, cls_idx):
        """cls_idx: (..., n) integer array indexing CLASSES. Returns
        (mean_ssim2, clean) arrays over the leading axes."""
        Sm = np.stack([self.S[c] for c in CLASSES])       # (3, n)
        B3m = np.stack([self.B3[c] for c in CLASSES])
        BMm = np.stack([self.BM[c] for c in CLASSES])
        s = np.take_along_axis(np.broadcast_to(Sm, cls_idx.shape[:-1] + Sm.shape),
                               cls_idx[..., None, :], axis=-2)[..., 0, :]
        b3 = np.take_along_axis(np.broadcast_to(B3m, cls_idx.shape[:-1] + B3m.shape),
                                cls_idx[..., None, :], axis=-2)[..., 0, :]
        bm = np.take_along_axis(np.broadcast_to(BMm, cls_idx.shape[:-1] + BMm.shape),
                                cls_idx[..., None, :], axis=-2)[..., 0, :]
        # nanmedian: butteraugli BD is undefined (frontier <4 pts / no overlap on
        # the -log quality axis) for 5004 (all arms) + 8302 (some arms) — those
        # cells are excluded from the veto median and counted in the TSV.
        clean = (np.nanmedian(b3, axis=-1) <= VETO_3N) & (np.nanmedian(bm, axis=-1) <= VETO_MAX)
        return s.mean(axis=-1), clean


def thresholds(v):
    u = np.unique(v)
    return (u[:-1] + u[1:]) / 2.0


def fit_rules(M, idx):
    """One pass over the constrained rule space on rows idx.
    Returns {family: (mean_bd, rule_dict)} best butteraugli-clean rule each."""
    V = Vec(M, idx)
    ci = {c: i for i, c in enumerate(CLASSES)}
    best = {}

    def consider(kind, mean_bd, rule):
        if kind not in best or mean_bd < best[kind][0]:
            best[kind] = (mean_bd, rule)

    # A: constant
    for s in CLASSES:
        m, clean = V.eval_assign(np.full((1, V.n), ci[s]))
        if clean[0]:
            consider("const", float(m[0]), dict(kind="const", strength=s, desc=f"always {s:g}"))

    # B: one feature, one threshold, 2 classes
    for f in SHORTLIST:
        ts = thresholds(V.F[f])
        if len(ts) == 0:
            continue
        above = V.F[f][None, :] > ts[:, None]              # (T, n)
        for lo, hi in itertools.combinations(CLASSES, 2):
            for a, b in ((lo, hi), (hi, lo)):
                cls = np.where(above, ci[b], ci[a])
                m, clean = V.eval_assign(cls)
                m = np.where(clean, m, np.inf)
                j = int(np.argmin(m))
                if np.isfinite(m[j]):
                    consider("1f", float(m[j]),
                             dict(kind="1f", f=f, t=float(ts[j]), below=a, above=b,
                                  desc=f"{f} > {ts[j]:.4g} -> {b:g} else {a:g}"))

    # C: fa > ta -> 0 ; elif fb > tb -> 2 ; else 1
    for fa, fb in itertools.product(SHORTLIST, SHORTLIST):
        ta = thresholds(V.F[fa])
        tb = thresholds(V.F[fb])
        if len(ta) == 0 or len(tb) == 0:
            continue
        va = V.F[fa][None, :] > ta[:, None]                # (Ta, n)
        vb = V.F[fb][None, :] > tb[:, None]                # (Tb, n)
        cls = np.where(va[:, None, :], ci[0.0],
                       np.where(vb[None, :, :], ci[2.0], ci[1.0]))  # (Ta, Tb, n)
        m, clean = V.eval_assign(cls)
        m = np.where(clean, m, np.inf)
        j = int(np.argmin(m))
        ja, jb = np.unravel_index(j, m.shape)
        if np.isfinite(m[ja, jb]):
            consider("2f", float(m[ja, jb]),
                     dict(kind="2f", fa=fa, ta=float(ta[ja]), fb=fb, tb=float(tb[jb]),
                          desc=f"{fa} > {ta[ja]:.4g} -> 0 elif {fb} > {tb[jb]:.4g} -> 2 else 1"))
    return best


def apply_rule(rule, M, idx):
    sub = M.loc[idx]
    if rule["kind"] == "const":
        return {i: rule["strength"] for i in idx}
    if rule["kind"] == "1f":
        return {i: (rule["above"] if sub.loc[i, rule["f"]] > rule["t"] else rule["below"])
                for i in idx}
    return {i: (0.0 if sub.loc[i, rule["fa"]] > rule["ta"]
                else (2.0 if sub.loc[i, rule["fb"]] > rule["tb"] else 1.0))
            for i in idx}


def realized(M, pred):
    return pd.DataFrame({c: np.array([M.loc[i, f"s{p:g}_{c}"] for i, p in pred.items()])
                         for c in ("ssim2", "ba3n", "bamax")}, index=list(pred))


def eval_policy(M, pred):
    r = realized(M, pred)
    ok = (np.nanmedian(r["ba3n"]) <= VETO_3N) and (np.nanmedian(r["bamax"]) <= VETO_MAX)
    return r["ssim2"].mean(), r["ssim2"].median(), ok, r


def main():
    M = build_matrix()
    idx = list(M.index)

    def oracle(classes, per_image_veto):
        pred = {}
        for i in idx:
            best_s, best_v = 0.0, 0.0
            for s in classes:
                v = M.loc[i, f"s{s:g}_ssim2"]
                if per_image_veto and (M.loc[i, f"s{s:g}_ba3n"] > VETO_3N
                                       or M.loc[i, f"s{s:g}_bamax"] > VETO_MAX):
                    continue
                if v < best_v:
                    best_s, best_v = s, v
            pred[i] = best_s
        return pred

    print("=== per-image variance-boost strength: label structure (train26, n=24, ALL train-split) ===")
    for name, pred in [
        ("global str1 (shipped)", {i: 1.0 for i in idx}),
        ("oracle {0,1,2} obj-ssim2", oracle([1.0, 2.0], False)),
        ("oracle all-strengths obj-ssim2", oracle([1.0, 2.0, 3.0, 4.5, 6.0], False)),
        ("oracle all-strengths per-image-veto", oracle([1.0, 2.0, 3.0, 4.5, 6.0], True)),
    ]:
        m, med, ok, r = eval_policy(M, pred)
        print(f"  {name:<38} mean {m:+.3f} median {med:+.3f} "
              f"ba3n_med {r['ba3n'].median():+.3f} bamax_med {r['bamax'].median():+.3f} "
              f"{'CLEAN' if ok else 'VETOED'}")

    fams = fit_rules(M, idx)
    print("\n=== best rule per family (fit on all 24 = resubstitution; butteraugli-clean policies only) ===")
    for kind in ("const", "1f", "2f"):
        if kind in fams:
            m, r = fams[kind]
            print(f"  [{kind}] mean {m:+.3f}  {r['desc']}")

    print("\n=== LOOCV (full threshold search re-run per left-out origin) ===")
    loo = {}
    for kind in ("1f", "2f"):
        preds = {}
        for leave in idx:
            tr = [i for i in idx if i != leave]
            res = fit_rules(M, tr)
            preds[leave] = (apply_rule(res[kind][1], M, [leave])[leave]
                            if kind in res else 1.0)
        m, med, ok, r = eval_policy(M, preds)
        loo[kind] = (m, med, ok, preds)
        print(f"  [{kind}] LOOCV mean {m:+.3f} median {med:+.3f} "
              f"ba3n_med {r['ba3n'].median():+.3f} bamax_med {r['bamax'].median():+.3f} "
              f"{'CLEAN' if ok else 'VETOED'}")
    g_m, g_med, _, _ = eval_policy(M, {i: 1.0 for i in idx})
    print(f"  vs global-1.0 mean {g_m:+.3f} median {g_med:+.3f}")

    kind = min(loo, key=lambda k: loo[k][0])
    m, r = fams[kind]
    preds_final = apply_rule(r, M, idx)
    loocv_preds = loo[kind][3]
    print(f"\nWINNING FAMILY: [{kind}] rule (refit on all 24): {r['desc']}")

    rr = realized(M, preds_final)
    out = M.copy()
    out["rule_strength"] = pd.Series(preds_final)
    out["rule_ssim2"] = rr["ssim2"]
    out["loocv_strength"] = pd.Series(loocv_preds)
    out["loocv_ssim2"] = realized(M, loocv_preds)["ssim2"]

    print("\n=== fam-9226 members (the wedge #2 family) ===")
    for i in idx:
        if "9226" in str(M.loc[i, "content_class"]):
            print(f"  {M.loc[i, 'origin_id']}: str1 {M.loc[i, 's1_ssim2']:+.2f} "
                  f"rule(s={preds_final[i]:g}) {rr.loc[i, 'ssim2']:+.2f} "
                  f"loocv(s={loocv_preds[i]:g}) {out.loc[i, 'loocv_ssim2']:+.2f}")

    hdr = [
        "hyperparam-expert first cut: per-image variance-boost strength rule (FEATURE_HINTS section E; wedge #2)",
        "labels: deltaq-2026-07-02 strength arms on train26 (24 TRAIN-LSD origins, s2+tune, 12-pt Q); store: /mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet",
        "BD convention: bd_arm.py direct per-image vs strength-0 arm; butteraugli negated -log quality axis",
        f"shortlist (pre-registered): {','.join(SHORTLIST)}; classes {CLASSES} (>=3 excluded: per-image-veto unstable)",
        "resubstitution best per family: " + "; ".join(
            f"[{k}] mean {v[0]:+.3f} {v[1]['desc']}" for k, v in fams.items()),
        "LOOCV: " + "; ".join(
            f"[{k}] mean {v[0]:+.3f} median {v[1]:+.3f} {'CLEAN' if v[2] else 'VETOED'}"
            for k, v in loo.items()),
        f"global-1.0: mean {g_m:+.3f} median {g_med:+.3f}; winning family [{kind}]",
        "sX_<m> columns = direct BD of strength X vs 0; features from imazen26_features_2026-06-23 (derived join; vips-vs-Lanczos rendition caveat in store manifest)",
        "NO VAL LABELS EXIST for this head (all 24 origins are LSD-train); LOOCV is the generalization stand-in",
    ]
    to_tsv(out, os.path.abspath(OUT_TSV), hdr)


if __name__ == "__main__":
    main()

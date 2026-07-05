#!/usr/bin/env python3
"""P3-residual refit: per-image variance-boost strength head, second cut.

Context (docs/RD_GAP_VS_LIBAOM.md "Near-lossless rescans residual" +
FAST_TIER_PARITY_PLAN s4-tier verdict): the s4-tier residual named an iq-AQ
class — 1236 interiors (+17.2), 9094 illustrations (+7.4/+2.5), 6018 1-bit
scans (+7.04 composed) vs aom-cpu2iq-ai — where aom's deltaq machinery boosts
DEEPER per SB ({36,64} qindex spread vs our {42,61} on 6018). The first-cut
head (fit_boost_strength.py, 2026-07-03) was parked: "LOOCV ~= global at n=24,
fam-9226 not strength-shaped". The NEW evidence names a different firing class
(deep-AQ content), so this refit:

  1. MINES the enlarged label store (62,562 rows) for the iq-AQ-ness label:
     per-image BD of aom-cpu2iq-ai vs aom-cpu2def-ai (speedladder source) —
     the def->iq delta IS the AQ-machinery value per image (the two arms
     differ only in tune). Correlates it against all zenanalyze features
     (AqMap stats + the WEDGE_MAP correlates included) to pick deep-AQ
     separators. Feature selection uses this AUXILIARY label, not the fit
     target — legitimate under the LSD discipline.
  2. REFITS the strength rule with classes {0,1,2,3,4.5} (first cut capped at
     2.0) under a PER-CELL fire-conservative veto: a deviation from the
     shipped 1.0 is only assignable on train when that image-arm's butteraugli
     BDs are clean (ba3n <= max(1.0, ba3n@str1), bamax <= max(1.5, bamax@str1)
     — deviations may never create butteraugli damage the ship point doesn't
     already have). NaN butteraugli (frontier-undefined: 5004-class) is
     allowed-but-counted, matching the first cut's veto-median exclusion.
  3. Evaluates VAL transfer when the valstr-* sweep source exists in the store
     (strengths {0,1,2,3,4.5} on the 14 held-out origins — the data gap the
     first cut named). Until then: LOOCV + the honest gap statement.

Ship bar (pre-registered): LOOCV vetoadj mean must beat global-1.0 by >= 0.5
BD points AND the winning rule's train assignments must be per-cell clean AND
the val re-eval (when labels exist) must not regress the val median vs
global-1.0. Otherwise: honest negative.

BD conventions imported from bd_arm.py via hp_common (do not re-implement).
"""

import itertools
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import (load_features, load_store, to_tsv,  # noqa: E402
                       veto_table)

OUT_TSV = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "../../benchmarks/hyperparam_boost_refit_2026-07-04.tsv")

# ---- labels -----------------------------------------------------------------
SRC = "deltaq-2026-07-02"
BASE = "deltaq/str0_s2"
ARMS = {1.0: "deltaq/str1_s2", 2.0: "deltaq/str2_s2", 3.0: "deltaq/str3_s2",
        4.5: "deltaq/str4.5_s2", 6.0: "deltaq/str6_s2"}
CLASSES = [0.0, 1.0, 2.0, 3.0, 4.5]   # 6.0 excluded: nowhere better than 4.5
SL_SRC = "speedladder-2026-07-04"
IQ_ARM, DEF_ARM = "speedladder/aom-cpu2iq-ai", "speedladder/aom-cpu2def-ai"
VALSTR_SRC = "valstr-2026-07-04"      # section 3 auto-detects this source
VETO_3N, VETO_MAX = 1.0, 1.5
SHIP = 1.0
SHIP_BAR_MARGIN = 0.5                 # LOOCV mean must beat global-1.0 by this

# Pre-registered shortlist: first-cut six + AqMap family + WEDGE_MAP correlates
# (docs/WEDGE_MAP_2026-07-03.md: entropy -0.75, palette_density -0.71,
# distinct_color_bins -0.71, patch_fraction +0.67, flat_color +0.60,
# grayscale +0.61). Final rule features additionally filtered by the def->iq
# mining below (top-|rho| auxiliary-label separators).
SHORTLIST = [
    # first cut
    "aq_map_std", "gradient_fraction_smooth", "luma_histogram_entropy",
    "noise_floor_y", "flat_color_block_ratio", "spectral_slope_y",
    # AqMap family (the diagnosis's named stat class)
    "aq_map_mean", "aq_map_p5", "aq_map_p50", "aq_map_p95",
    # wedge correlates not already present
    "palette_density", "distinct_color_bins", "patch_fraction",
    "grayscale_score",
]


def build_matrix():
    store = load_store(sweep_source=SRC, corpus="train26")
    feats = load_features(SHORTLIST)
    meta = (store[["image_id", "origin_id", "content_class", "family",
                   "feature_join"]]
            .drop_duplicates().set_index("image_id"))
    M = pd.DataFrame(index=sorted(meta.index))
    for c in ("ssim2", "ba3n", "bamax"):
        M[f"s0_{c}"] = 0.0
    for s, arm in ARMS.items():
        t = veto_table(store, SRC, BASE, arm)
        M[f"s{s:g}_ssim2"] = t["bd_ssim2"]
        M[f"s{s:g}_ba3n"] = t["bd_ba3n"]
        M[f"s{s:g}_bamax"] = t["bd_bamax"]
    for c in ("origin_id", "content_class", "family"):
        M[c] = meta[c]
    for f in SHORTLIST:
        M[f] = meta["feature_join"].map(feats[f])
    return M


def mine_def_to_iq():
    """Section 1: the iq-AQ-ness auxiliary label + feature correlation scan.

    IN-BAND ONLY (ssim2 >= 60 AND bpp >= 0.05): aom's cq56/63 tail cells
    score pathological ssim2 on flat-out blur (1236 def q63 = 45.0 ssim2 at
    0.072 bpp AFTER 10.3 at q56; 9100 def q63 = 60.2 at 0.019 bpp;
    5004/7058 same shape) and the monotone-frontier hull keeps those
    fake-great points, flipping the whole-curve BD sign on exactly the
    interiors/illustrations the diagnosis names. The joint floor removes
    them; the surviving band (~ssim2 60-93) is where the s4-tier residuals
    and the web operating range actually live."""
    store = load_store(sweep_source=SL_SRC, corpus="train26")
    store = store[(store["ssim2"] >= 60.0) & (store["bpp"] >= 0.05)]
    t = veto_table(store, SL_SRC, DEF_ARM, IQ_ARM)
    t = t.rename(columns={"bd_ssim2": "iq_vs_def_ssim2",
                          "bd_ba3n": "iq_vs_def_ba3n",
                          "bd_bamax": "iq_vs_def_bamax"})
    feats = load_features(None)  # all features
    meta = (store[["image_id", "feature_join", "origin_id"]]
            .drop_duplicates().set_index("image_id"))
    fj = meta["feature_join"]
    print("=== 1. iq-AQ-ness label: aom cpu2iq-ai vs cpu2def-ai per image "
          "(negative = iq's AQ machinery pays) ===")
    lab = t["iq_vs_def_ssim2"].dropna().sort_values()
    for i, v in lab.items():
        print(f"  {str(meta.loc[i, 'origin_id']):>6} {v:+8.2f}")
    rho = {}
    fnames = [c for c in feats.columns
              if c not in ("image_path", "crop_label", "size_class", "width",
                           "height", "content_class")]
    for f in fnames:
        x = fj.map(feats[f]).astype(float)
        y = t["iq_vs_def_ssim2"]
        ok = x.notna() & y.notna()
        if ok.sum() >= 12 and x[ok].nunique() > 3:
            rho[f] = pd.Series(x[ok]).corr(pd.Series(y[ok]), method="spearman")
    top = sorted(rho.items(), key=lambda kv: -abs(kv[1]))[:14]
    print("\n  top-|rho| feature correlates of the def->iq delta (spearman):")
    for f, r in top:
        print(f"    {f:<32} {r:+.3f}")
    return t, dict(top)


# ---- fire-conservative assignability ----------------------------------------
def allowed_mask(M):
    """allowed[i, k]: may class CLASSES[k] be assigned to image i?
    Ship (1.0) always allowed. Deviations must not push butteraugli past the
    veto thresholds unless str1 already sits past them (no NEW damage).
    NaN butteraugli = allowed (frontier-undefined; counted separately)."""
    n = len(M.index)
    A = np.zeros((n, len(CLASSES)), dtype=bool)
    nan_allowed = 0
    for r, i in enumerate(M.index):
        b3_ship = M.loc[i, "s1_ba3n"]
        bm_ship = M.loc[i, "s1_bamax"]
        cap3 = max(VETO_3N, b3_ship) if np.isfinite(b3_ship) else VETO_3N
        capm = max(VETO_MAX, bm_ship) if np.isfinite(bm_ship) else VETO_MAX
        for k, s in enumerate(CLASSES):
            if s == SHIP:
                A[r, k] = True
                continue
            b3 = M.loc[i, f"s{s:g}_ba3n"]
            bm = M.loc[i, f"s{s:g}_bamax"]
            if not np.isfinite(b3) or not np.isfinite(bm):
                A[r, k] = True
                nan_allowed += 1
                continue
            A[r, k] = (b3 <= cap3) and (bm <= capm)
    return A, nan_allowed


class Vec:
    def __init__(self, M, idx, feats):
        self.idx = list(idx)
        self.n = len(self.idx)
        sub = M.loc[self.idx]
        self.S = np.stack([sub[f"s{s:g}_ssim2"].to_numpy() for s in CLASSES])
        self.B3 = np.stack([sub[f"s{s:g}_ba3n"].to_numpy() for s in CLASSES])
        self.BM = np.stack([sub[f"s{s:g}_bamax"].to_numpy() for s in CLASSES])
        A, _ = allowed_mask(M)
        rows = [M.index.get_loc(i) for i in self.idx]
        self.A = A[rows].T                       # (C, n)
        self.F = {f: sub[f].to_numpy() for f in feats}
        self.feats = feats

    def eval_assign(self, cls_idx):
        """cls_idx: (..., n) int array into CLASSES. Returns (mean, ok) where
        ok = every assignment allowed (fire-conservative on the fit rows)."""
        shp = cls_idx.shape[:-1]
        S = np.broadcast_to(self.S, shp + self.S.shape)
        A = np.broadcast_to(self.A, shp + self.A.shape)
        s = np.take_along_axis(S, cls_idx[..., None, :], axis=-2)[..., 0, :]
        a = np.take_along_axis(A, cls_idx[..., None, :], axis=-2)[..., 0, :]
        return s.mean(axis=-1), a.all(axis=-1)


def thresholds(v):
    u = np.unique(v[np.isfinite(v)])
    return (u[:-1] + u[1:]) / 2.0


def fit_rules(M, idx, feats):
    """Constrained rule families, veto-aware. Returns best per family."""
    V = Vec(M, idx, feats)
    ci = {c: i for i, c in enumerate(CLASSES)}
    best = {}

    def consider(kind, mean_bd, rule):
        if kind not in best or mean_bd < best[kind][0]:
            best[kind] = (mean_bd, rule)

    # const
    for s in CLASSES:
        m, ok = V.eval_assign(np.full((1, V.n), ci[s]))
        if ok[0]:
            consider("const", float(m[0]),
                     dict(kind="const", strength=s, desc=f"always {s:g}"))

    # 1f: f > t -> b else a, all ordered class pairs
    for f in feats:
        ts = thresholds(V.F[f])
        if len(ts) == 0:
            continue
        above = V.F[f][None, :] > ts[:, None]
        for a, b in itertools.permutations(CLASSES, 2):
            cls = np.where(above, ci[b], ci[a])
            m, ok = V.eval_assign(cls)
            m = np.where(ok, m, np.inf)
            j = int(np.argmin(m))
            if np.isfinite(m[j]):
                consider("1f", float(m[j]),
                         dict(kind="1f", f=f, t=float(ts[j]), below=a, above=b,
                              desc=f"{f} > {ts[j]:.4g} -> {b:g} else {a:g}"))

    # 2f-conj: (fa dir_a ta) AND (fb dir_b tb) -> deep else SHIP
    # (the palette-gate / tx-D conjunctive pattern; deep may also be 0.0)
    for fa, fb in itertools.combinations(feats, 2):
        ta = thresholds(V.F[fa])
        tb = thresholds(V.F[fb])
        if len(ta) == 0 or len(tb) == 0:
            continue
        for da, db in itertools.product((1, -1), repeat=2):
            va = (V.F[fa][None, :] * da) > (ta[:, None] * da)
            vb = (V.F[fb][None, :] * db) > (tb[:, None] * db)
            hit = va[:, None, :] & vb[None, :, :]          # (Ta, Tb, n)
            for deep in CLASSES:
                if deep == SHIP:
                    continue
                cls = np.where(hit, ci[deep], ci[SHIP])
                m, ok = V.eval_assign(cls)
                m = np.where(ok, m, np.inf)
                j = int(np.argmin(m))
                ja, jb = np.unravel_index(j, m.shape)
                if np.isfinite(m[ja, jb]):
                    sa = ">" if da == 1 else "<"
                    sb = ">" if db == 1 else "<"
                    consider("2fconj", float(m[ja, jb]),
                             dict(kind="2fconj", fa=fa, ta=float(ta[ja]), da=da,
                                  fb=fb, tb=float(tb[jb]), db=db, deep=deep,
                                  desc=(f"{fa} {sa} {ta[ja]:.4g} && {fb} {sb} "
                                        f"{tb[jb]:.4g} -> {deep:g} else 1")))

    # 2f-3class: fa > ta -> cA elif fb > tb -> cB else cC
    for fa, fb in itertools.product(feats, feats):
        if fa == fb:
            continue
        ta = thresholds(V.F[fa])
        tb = thresholds(V.F[fb])
        if len(ta) == 0 or len(tb) == 0:
            continue
        va = V.F[fa][None, :] > ta[:, None]
        vb = V.F[fb][None, :] > tb[:, None]
        for cA, cB, cC in itertools.permutations(CLASSES, 3):
            cls = np.where(va[:, None, :], ci[cA],
                           np.where(vb[None, :, :], ci[cB], ci[cC]))
            m, ok = V.eval_assign(cls)
            m = np.where(ok, m, np.inf)
            j = int(np.argmin(m))
            ja, jb = np.unravel_index(j, m.shape)
            if np.isfinite(m[ja, jb]):
                consider("2f3c", float(m[ja, jb]),
                         dict(kind="2f3c", fa=fa, ta=float(ta[ja]),
                              fb=fb, tb=float(tb[jb]), cA=cA, cB=cB, cC=cC,
                              desc=(f"{fa} > {ta[ja]:.4g} -> {cA:g} elif "
                                    f"{fb} > {tb[jb]:.4g} -> {cB:g} else {cC:g}")))
    return best


def apply_rule(rule, M, idx):
    sub = M.loc[idx]
    out = {}
    for i in idx:
        k = rule["kind"]
        if k == "const":
            out[i] = rule["strength"]
        elif k == "1f":
            out[i] = rule["above"] if sub.loc[i, rule["f"]] > rule["t"] else rule["below"]
        elif k == "2fconj":
            ha = (sub.loc[i, rule["fa"]] * rule["da"]) > (rule["ta"] * rule["da"])
            hb = (sub.loc[i, rule["fb"]] * rule["db"]) > (rule["tb"] * rule["db"])
            out[i] = rule["deep"] if (ha and hb) else SHIP
        else:
            if sub.loc[i, rule["fa"]] > rule["ta"]:
                out[i] = rule["cA"]
            elif sub.loc[i, rule["fb"]] > rule["tb"]:
                out[i] = rule["cB"]
            else:
                out[i] = rule["cC"]
    return out


def realized(M, pred):
    return pd.DataFrame(
        {c: np.array([M.loc[i, f"s{p:g}_{c}"] for i, p in pred.items()])
         for c in ("ssim2", "ba3n", "bamax")}, index=list(pred))


def eval_policy(M, pred, label):
    r = realized(M, pred)
    veto = ((r["ba3n"] > VETO_3N) | (r["bamax"] > VETO_MAX)) & \
        r["ba3n"].notna() & r["bamax"].notna()
    vetoadj = r["ssim2"].where(~veto, r["ssim2"].clip(lower=0.0))
    print(f"  {label:<46} mean {r['ssim2'].mean():+.3f} "
          f"med {r['ssim2'].median():+.3f} | vetoadj mean {vetoadj.mean():+.3f} "
          f"med {vetoadj.median():+.3f} | vetoes {int(veto.sum())} | "
          f"ba3n_med {r['ba3n'].median():+.3f} bamax_med {r['bamax'].median():+.3f}")
    return dict(mean=r["ssim2"].mean(), med=r["ssim2"].median(),
                vmean=vetoadj.mean(), vmed=vetoadj.median(),
                vetoes=int(veto.sum()), table=r)


def main():
    M = build_matrix()
    idx = list(M.index)
    iq_tbl, top_rho = mine_def_to_iq()

    # feature set for the fit: pre-registered shortlist + any top-8 def->iq
    # correlate already in the parquet (auxiliary-label selection)
    fit_feats = list(SHORTLIST)
    for f in list(top_rho)[:8]:
        if f not in fit_feats and f in M.columns:
            fit_feats.append(f)
    # (features outside SHORTLIST are not in M; reload if needed)
    extra = [f for f in list(top_rho)[:8] if f not in M.columns]
    if extra:
        feats2 = load_features(extra)
        store = load_store(sweep_source=SRC, corpus="train26")
        meta = (store[["image_id", "feature_join"]]
                .drop_duplicates().set_index("image_id"))
        for f in extra:
            M[f] = meta["feature_join"].map(feats2[f])
            fit_feats.append(f) if f not in fit_feats else None
    fit_feats = [f for f in dict.fromkeys(fit_feats) if M[f].notna().sum() >= 20]
    print(f"\n=== 2. refit: classes {CLASSES}, fire-conservative per-cell veto, "
          f"{len(fit_feats)} features ===")
    A, nan_ct = allowed_mask(M)
    print(f"  assignable cells: {int(A.sum())}/{A.size} "
          f"(NaN-butteraugli allowed: {nan_ct})")

    def oracle_allowed():
        pred = {}
        for r, i in enumerate(idx):
            ks = [k for k in range(len(CLASSES)) if A[r, k]]
            pred[i] = CLASSES[min(ks, key=lambda k: (
                M.loc[i, f"s{CLASSES[k]:g}_ssim2"]
                if np.isfinite(M.loc[i, f"s{CLASSES[k]:g}_ssim2"]) else np.inf))]
        return pred

    stats = {}
    stats["ship"] = eval_policy(M, {i: SHIP for i in idx}, "global str1 (shipped)")
    stats["oracle"] = eval_policy(M, oracle_allowed(),
                                  "oracle (allowed cells only)")

    fams = fit_rules(M, idx, fit_feats)
    print("\n  best rule per family (resubstitution, all-24):")
    for kind, (m, r) in sorted(fams.items()):
        print(f"    [{kind}] mean {m:+.3f}  {r['desc']}")

    print("\n=== LOOCV (full threshold re-search per left-out origin) ===")
    loo = {}
    for kind in ("1f", "2fconj", "2f3c"):
        preds = {}
        for leave in idx:
            tr = [i for i in idx if i != leave]
            res = fit_rules(M, tr, fit_feats)
            preds[leave] = (apply_rule(res[kind][1], M, [leave])[leave]
                            if kind in res else SHIP)
        loo[kind] = eval_policy(M, preds, f"[{kind}] LOOCV")
        loo[kind]["preds"] = preds

    kind = min(loo, key=lambda k: loo[k]["vmean"])
    m, rule = fams[kind]
    preds_final = apply_rule(rule, M, idx)
    print(f"\nWINNING FAMILY: [{kind}]  rule (refit on all 24): {rule['desc']}")
    ship_bar = stats["ship"]["vmean"] - SHIP_BAR_MARGIN
    verdict = ("SHIP-CANDIDATE (pending val labels)"
               if loo[kind]["vmean"] <= ship_bar else
               "HONEST NEGATIVE: LOOCV does not clear global-1.0 by the margin")
    print(f"VERDICT vs bar (global vetoadj mean {stats['ship']['vmean']:+.3f} "
          f"- {SHIP_BAR_MARGIN}): {verdict}")

    print("\n  the named iq-AQ class under the final rule:")
    for i in idx:
        oid = str(M.loc[i, "origin_id"])
        if oid in ("1236", "9100", "9118", "6018", "6096"):
            print(f"    {oid}: rule s={preds_final[i]:g} "
                  f"(ssim2 {M.loc[i, f's{preds_final[i]:g}_ssim2']:+.2f}) "
                  f"loocv s={loo[kind]['preds'][i]:g}")

    # ---- section 3: val labels, if the sweep has landed ----
    val_note = "NO VAL LABELS YET (valstr sweep pending); LOOCV is the stand-in"
    try:
        vs = load_store(sweep_source=VALSTR_SRC)
    except Exception:
        vs = pd.DataFrame()
    if not vs.empty:
        print(f"\n=== 3. VAL transfer ({VALSTR_SRC}) ===")
        feats_all = load_features(fit_feats)
        vmeta = (vs[["image_id", "origin_id", "feature_join"]]
                 .drop_duplicates().set_index("image_id"))
        VM = pd.DataFrame(index=sorted(vmeta.index))
        for s, arm in ARMS.items():
            if s not in CLASSES:
                continue
            t = veto_table(vs, VALSTR_SRC, BASE.replace("deltaq", "valstr"),
                           arm.replace("deltaq", "valstr"))
            if t is None or t.empty:
                continue
            VM[f"s{s:g}_ssim2"] = t["bd_ssim2"]
            VM[f"s{s:g}_ba3n"] = t["bd_ba3n"]
            VM[f"s{s:g}_bamax"] = t["bd_bamax"]
        for c in ("ssim2", "ba3n", "bamax"):
            VM[f"s0_{c}"] = 0.0
        VM["origin_id"] = vmeta["origin_id"]
        for f in fit_feats:
            VM[f] = vmeta["feature_join"].map(feats_all[f])
        vpred = apply_rule(rule, VM, list(VM.index))
        vstats = eval_policy(VM, vpred, "final rule on VAL")
        sstats = eval_policy(VM, {i: SHIP for i in VM.index}, "global str1 on VAL")
        val_note = (f"VAL: rule vetoadj mean {vstats['vmean']:+.3f} vs ship "
                    f"{sstats['vmean']:+.3f}; vetoes {vstats['vetoes']}")
        print(f"  -> {val_note}")

    rr = realized(M, preds_final)
    out = M.copy()
    out["rule_strength"] = pd.Series(preds_final)
    out["rule_ssim2"] = rr["ssim2"]
    out["loocv_strength"] = pd.Series(loo[kind]["preds"])
    out["iq_vs_def_ssim2"] = iq_tbl["iq_vs_def_ssim2"]

    hdr = [
        "P3-residual boost-strength head REFIT (iq-AQ class: 1236/9094/6018)",
        f"labels: {SRC} strength arms (train26, s2+tune, 12q) vs {BASE}; "
        f"aux label: {IQ_ARM} vs {DEF_ARM} ({SL_SRC})",
        f"classes {CLASSES}; fire-conservative per-cell veto "
        f"(no new butteraugli damage vs str1; NaN allowed+counted)",
        "def->iq top correlates: " + "; ".join(
            f"{f} {r:+.3f}" for f, r in list(top_rho.items())[:8]),
        "resubstitution: " + "; ".join(
            f"[{k}] {v[0]:+.3f} {v[1]['desc']}" for k, v in sorted(fams.items())),
        "LOOCV vetoadj mean/med: " + "; ".join(
            f"[{k}] {v['vmean']:+.3f}/{v['vmed']:+.3f} vetoes {v['vetoes']}"
            for k, v in loo.items()),
        f"global-1.0 vetoadj mean {stats['ship']['vmean']:+.3f}; "
        f"oracle-allowed {stats['oracle']['vmean']:+.3f}; winner [{kind}]; {verdict}",
        val_note,
    ]
    to_tsv(out, os.path.abspath(OUT_TSV), hdr)


if __name__ == "__main__":
    main()

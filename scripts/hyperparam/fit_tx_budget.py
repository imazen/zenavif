#!/usr/bin/env python3
"""Per-image TX-BUDGET head (FAST_TIER_PARITY P2 head 1; FEATURE_HINTS §E).

The FASTWINS P0 decomposition measured the s4->s6 rdo_tx cliff per image at
s6/s8: {size1 (tx-size RDO depth-1, DCT-only), min (size1 + reduced types),
full, size2, type, typred, red} vs the stock TX_MODE_LARGEST base. Global
verdicts: size1 = 51% of the whole s6->s4 step at 1.67x solo (landed
release-gated); min = 92% at 4.57x (P1 seed, unshipped). But the per-image
response is wildly heterogeneous: fam-8100 screens get ~0 from size1 and
REGRESS under min; fam-7000 near-lossless plots pay bytes under size-RDO;
interiors/food/nature leave half the step on the table unless min runs.

This head picks {none | size1 | min} PER IMAGE from cheap zenanalyze
features so the 1.67-4.6x is spent only where it pays.

Labels: fastwins-2026-07-04 (train26, 24 TRAIN-LSD origins, coarse 6-q,
tune-ss2 + palette auto, s6 rides stock table + tx arm; PALCONF-clean).
Per-image direct BD vs same-speed base (bd_arm.py hull convention), both
butteraugli norms as veto. Costs: measured SOLO wall ratios (JOBS=1,
RD_CACHE=off) from benchmarks/rd_gap_fastwins_2026-07-04.tsv — s6
size1 1.67x / min 4.57x; s8 size1 1.43x / min 3.37x. Per-image contended
(JOBS=24) enc_ms ratios reported as corroboration only.

Honesty: all labeled origins are TRAIN-LSD; LOOCV (leave-one-origin-out) is
the generalization stand-in; VAL evidence = firing-rate sanity on val
feature rows until the box val confirm runs. The tx response was measured
WITHOUT the P1 partition arms (fastwins predates them); interaction with
the partition ship point is measured separately (intrax box run).
"""

import itertools
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import (arm_bd_per_image, load_features, load_store,  # noqa: E402
                       print_dist, split_of, to_tsv, veto_table)

OUT_TSV = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "../../benchmarks/hyperparam_tx_budget_2026-07-04.tsv")

SRC = "fastwins-2026-07-04"

# Pre-registered candidate features (mechanism-driven, chosen BEFORE fitting):
# withhold side (screen/plot razor-edge content where size-RDO pays ~0 or
# regresses): patch_fraction, flat_color_block_ratio, entropy(low);
# deep side (texture that TX_MODE_LARGEST butchers and depth-1 size RDO only
# half-fixes): high_freq_energy_ratio, edge_density, laplacian_variance_p50,
# dct_compressibility_y, gradient_fraction_smooth.
SHORTLIST = [
    "patch_fraction", "flat_color_block_ratio", "luma_histogram_entropy",
    "high_freq_energy_ratio", "edge_density", "laplacian_variance_p50",
    "dct_compressibility_y", "gradient_fraction_smooth",
]
# Features where the "screen-like / withhold" side is LOW.
LOWSIDE = {"luma_histogram_entropy", "laplacian_variance_p50", "edge_density",
           "high_freq_energy_ratio"}

# Measured SOLO wall ratios vs same-speed base (fastwins TSV solo section).
SOLO_COST = {
    6: {"none": 1.0, "size1": 1.67, "min": 4.57},
    8: {"none": 1.0, "size1": 1.43, "min": 3.37},
}
MENU = ["none", "size1", "min"]
ARM_IDS = {6: {"size1": "fastwins/s6-size1", "min": "fastwins/s6-min",
               "size2": "fastwins/s6-size2", "full": "fastwins/s6-full"},
           8: {"size1": "fastwins/s8-size1", "min": "fastwins/s8-min"}}
LAMBDAS = [0.0, 0.25, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 1e9]


def veto_adj(vt):
    """Palette-gate veto convention: a butteraugli-adverse arm never banks
    its ssim2 win (bd := max(bd, 0)); NaN butteraugli (degenerate frontier)
    does not veto but is flagged by the caller."""
    bd = vt["bd_ssim2"].to_numpy(float)
    ba3 = vt["bd_ba3n"].to_numpy(float)
    bam = vt["bd_bamax"].to_numpy(float)
    vet = (np.nan_to_num(ba3, nan=-np.inf) > 1.0) | \
          (np.nan_to_num(bam, nan=-np.inf) > 1.5)
    return np.where(vet, np.maximum(bd, 0.0), bd), vet


def rdpar_ratio(store, speed, arm, base):
    """Per-image contended enc_ms ratio (sum over common q). Corroboration."""
    df = store[(store["sweep_source"] == SRC) & (store["speed"] == speed)]
    a = df[df["arm_id"] == arm].groupby(["image_id", "q"])["enc_ms"].sum()
    b = df[df["arm_id"] == base].groupby(["image_id", "q"])["enc_ms"].sum()
    j = pd.concat({"a": a, "b": b}, axis=1).dropna()
    g = j.groupby(level=0).sum()
    return (g["a"] / g["b"]).rename("rdpar")


def byte_ratio_pct(store, speed, arm, base):
    """Mean same-q byte delta % per image — the honest label when the BD hull
    degenerates (near-lossless razor-edge content where ssim2 saturates on
    both arms; fam-7000's '+2..18% bytes on ~3KB files' failure mode)."""
    df = store[(store["sweep_source"] == SRC) & (store["speed"] == speed)]
    a = df[df["arm_id"] == arm].groupby(["image_id", "q"])["bytes"].sum()
    b = df[df["arm_id"] == base].groupby(["image_id", "q"])["bytes"].sum()
    j = pd.concat({"a": a, "b": b}, axis=1).dropna()
    r = (j["a"] / j["b"] - 1.0) * 100.0
    return r.groupby(level=0).mean().rename("byte_pct")


def labels_for_speed(store, feats, speed):
    base = f"fastwins/s{speed}-base"
    meta = (store[store["sweep_source"] == SRC]
            [["image_id", "origin_id", "content_class", "feature_join"]]
            .drop_duplicates().set_index("image_id"))
    T = pd.DataFrame(index=sorted(
        store.loc[(store["sweep_source"] == SRC)
                  & (store["arm_id"] == base), "image_id"].unique()))
    T.index.name = "image_id"
    for c in ["origin_id", "content_class", "feature_join"]:
        T[c] = meta[c]
    T["family"] = T["content_class"].str.extract(r"^(\d+)")
    for arm, aid in ARM_IDS[speed].items():
        vt = veto_table(store, SRC, base, aid)
        adj, vet = veto_adj(vt)
        T[f"bd_{arm}"] = vt["bd_ssim2"]
        T[f"ba3n_{arm}"] = vt["bd_ba3n"]
        T[f"bamax_{arm}"] = vt["bd_bamax"]
        T[f"adj_{arm}"] = pd.Series(adj, index=vt.index)
        T[f"veto_{arm}"] = pd.Series(vet, index=vt.index)
        T[f"rdpar_{arm}"] = rdpar_ratio(store, speed, aid, base)
        # Degenerate-frontier fallback: NaN BD -> same-q byte delta % (never
        # 0 — those are exactly the razor-edge cells where size-RDO pays
        # bytes at saturated quality). Positive fallback = regression.
        bp = byte_ratio_pct(store, speed, aid, base)
        nanmask = T[f"adj_{arm}"].isna()
        if nanmask.any():
            T.loc[nanmask, f"adj_{arm}"] = bp.reindex(T.index)[nanmask]
            T.loc[nanmask, f"bd_{arm}"] = bp.reindex(T.index)[nanmask]
    for f in SHORTLIST:
        T[f] = [feats.loc[fj, f] if fj in feats.index else np.nan
                for fj in T["feature_join"]]
    return T


def oracle_choice(T, speed, lam):
    """argmin over the menu of veto-adjusted bd + lam*(solo_cost-1)."""
    costs = SOLO_COST[speed]
    obj = np.stack([np.zeros(len(T)) if m == "none"
                    else T[f"adj_{m}"].fillna(0.0).to_numpy()
                    for m in MENU]) \
        + lam * (np.array([[costs[m]] for m in MENU]) - 1.0)
    return np.array(MENU)[obj.argmin(axis=0)]


def realized(T, speed, choice):
    costs = SOLO_COST[speed]
    bd = np.array([0.0 if c == "none" else T[f"adj_{c}"].iloc[i]
                   for i, c in enumerate(choice)], float)
    bd = np.nan_to_num(bd, nan=0.0)
    t = np.array([costs[c] for c in choice], float)
    return bd, t


def rule_space(T):
    """Stacked 2-threshold rules over the pre-registered shortlist:
       gate W (withhold -> none)  : fw on its screen-side of tw
       gate D (deep -> min)       : fd on its texture-side of td
       else -> size1.
    Also 1-gate degenerate forms (W only / D only)."""
    def taus(f):
        u = np.unique(T[f].dropna().to_numpy())
        return (u[:-1] + u[1:]) / 2.0

    def pred(f, t, side):
        v = T[f].to_numpy(float)
        # side 'screen': fire where screen-like; side 'texture': texture-like.
        low = f in LOWSIDE
        if side == "screen":
            return (v < t) if low else (v > t)
        return (v > t) if low else (v < t)

    yield "always-size1", np.zeros(len(T), bool), np.zeros(len(T), bool), ()
    for fw in SHORTLIST:
        for tw in taus(fw):
            w = pred(fw, tw, "screen")
            yield (f"W[{fw}@{tw:.4g}]", w, np.zeros(len(T), bool), ((fw, tw, "W"),))
    for fd in SHORTLIST:
        for td in taus(fd):
            d = pred(fd, td, "texture")
            yield (f"D[{fd}@{td:.4g}]", np.zeros(len(T), bool), d, ((fd, td, "D"),))
    for fw, fd in itertools.product(SHORTLIST, SHORTLIST):
        if fw == fd:
            continue
        for tw in taus(fw):
            w = pred(fw, tw, "screen")
            for td in taus(fd):
                d = pred(fd, td, "texture") & ~w  # W wins ties
                yield (f"W[{fw}@{tw:.4g}] D[{fd}@{td:.4g}]", w, d,
                       ((fw, tw, "W"), (fd, td, "D")))


def rule_to_choice(w, d):
    c = np.full(len(w), "size1", dtype=object)
    c[d] = "min"
    c[w] = "none"
    return c


def fit_rule(T, speed, lam):
    best = None
    for desc, w, d, spec in rule_space(T):
        bd, t = realized(T, speed, rule_to_choice(w, d))
        obj = (bd + lam * (t - 1.0)).mean()
        key = (obj, len(spec))
        if best is None or key < best[0]:
            best = (key, desc, spec, w, d)
    return best[1:]  # desc, spec, w, d


def apply_spec(spec, df):
    w = np.zeros(len(df), bool)
    d = np.zeros(len(df), bool)
    for f, t, kind in spec:
        v = df[f].to_numpy(float)
        low = f in LOWSIDE
        if kind == "W":
            w |= (v < t) if low else (v > t)
        else:
            d |= (v > t) if low else (v < t)
    d &= ~w
    return rule_to_choice(w, d)


def main():
    store = load_store()
    feats = load_features(SHORTLIST)

    for speed in (6, 8):
        T = labels_for_speed(store, feats, speed)
        print(f"\n================ s{speed} ================")
        print("=== per-image response (veto-adjusted ssim2 BD vs base; "
              "solo costs size1 "
              f"{SOLO_COST[speed]['size1']}x min {SOLO_COST[speed]['min']}x) ===")
        cols = [c for c in T.columns if c.startswith(("bd_", "adj_", "veto_", "rdpar_"))]
        show = T[["origin_id", "family"] + cols].copy()
        with pd.option_context("display.width", 250, "display.max_columns", 99):
            print(show.sort_values("family").round(2).to_string())

        # --- global arms vs oracle frontier ---
        print("\n=== lambda frontier: per-image ORACLE vs global arms "
              "(mean veto-adj BD / mean solo cost) ===")
        for m in [x for x in MENU if x != "none"]:
            bd = T[f"adj_{m}"].fillna(0.0)
            print(f"  global-{m:6s}: bd mean {bd.mean():+.3f} med {bd.median():+.3f}"
                  f"  cost {SOLO_COST[speed][m]:.2f}x")
        frontier_rows = []
        for lam in LAMBDAS:
            ch = oracle_choice(T, speed, lam)
            bd, t = realized(T, speed, ch)
            n = {m: int((ch == m).sum()) for m in MENU}
            frontier_rows.append((lam, bd.mean(), np.median(bd), t.mean(), n))
            print(f"  oracle lam={lam:<5g}: bd mean {bd.mean():+.3f} med "
                  f"{np.median(bd):+.3f}  cost {t.mean():.2f}x  mix {n}")

        # --- fit rules at operating lambdas ---
        for lam in (0.5, 1.0, 2.0):
            desc, spec, w, d = fit_rule(T, speed, lam)
            ch = apply_spec(spec, T)
            bd, t = realized(T, speed, ch)
            och = oracle_choice(T, speed, lam)
            obd, ot = realized(T, speed, och)
            # LOOCV by origin
            loo_bd, loo_t = np.zeros(len(T)), np.zeros(len(T))
            for org in T["origin_id"].unique():
                mask = (T["origin_id"] == org).to_numpy()
                _, sp, _, _ = fit_rule(T[~mask], speed, lam)
                chh = apply_spec(sp, T[mask])
                b, tt = realized(T[mask], speed, chh)
                loo_bd[mask], loo_t[mask] = b, tt
            print(f"\n  --- rule @ lam={lam} ---")
            print(f"  RULE: {desc}")
            mix = {m: int((ch == m).sum()) for m in MENU}
            print(f"  fit     : bd mean {bd.mean():+.3f} med {np.median(bd):+.3f} "
                  f"cost {t.mean():.2f}x mix {mix}")
            print(f"  oracle  : bd mean {obd.mean():+.3f} med {np.median(obd):+.3f} "
                  f"cost {ot.mean():.2f}x")
            print(f"  LOOCV   : bd mean {loo_bd.mean():+.3f} med {np.median(loo_bd):+.3f} "
                  f"cost {loo_t.mean():.2f}x")
            gl = T[f"adj_size1"].fillna(0.0)
            print(f"  vs global-size1: fit {bd.mean() - gl.mean():+.3f} mean BD at "
                  f"{t.mean() - SOLO_COST[speed]['size1']:+.2f}x cost; "
                  f"LOOCV {loo_bd.mean() - gl.mean():+.3f}")
            for m in ("none", "min"):
                sel = T.index[ch == m]
                if len(sel):
                    print(f"  {m:5s} <- " + ", ".join(
                        f"{T.loc[i, 'origin_id']}({T.loc[i, 'adj_size1']:+.1f}s1/"
                        f"{T.loc[i, 'adj_min']:+.1f}mn)" for i in sel))
            missed = T.index[(T["adj_size1"] > 0.5) & (ch != "none")]
            if len(missed):
                print("  regressors NOT withheld: " + ", ".join(
                    f"{T.loc[i, 'origin_id']}({T.loc[i, 'adj_size1']:+.1f})"
                    for i in missed))
            if lam == 1.0:
                T["rule_choice"] = ch
                loo_ch = np.full(len(T), "", object)
                for org in T["origin_id"].unique():
                    mask = (T["origin_id"] == org).to_numpy()
                    _, sp, _, _ = fit_rule(T[~mask], speed, lam)
                    loo_ch[mask] = apply_spec(sp, T[mask])
                T["loocv_choice"] = loo_ch
                keep = (speed, desc, spec, T.copy())

        if speed == 6:
            s6_keep = keep
        else:
            s8_keep = keep

    # --- val firing-rate sanity on the lam=1.0 s6 rule ---
    speed, desc, spec, T6 = s6_keep
    print("\n=== val-origin choice rates for the s6 lam=1.0 rule "
          "(feature rows only; no RD labels) ===")
    allf = load_features(SHORTLIST).reset_index()
    allf["base"] = allf["image_path"].str.rsplit("/", n=1).str[-1]
    allf["split"] = allf["base"].map(split_of)
    va = allf[(allf["split"] == "val") & (allf["crop_label"] == "full")
              & allf["size_class"].isin(["1024", "native"])].copy()
    va_choice = apply_spec(spec, va)
    va["choice"] = va_choice
    va["cgroup"] = va["content_class"].str.extract(r"^(\d+)")
    tab = va.pivot_table(index="cgroup", columns="choice", values="base",
                         aggfunc="count", fill_value=0)
    print(tab.to_string())

    hdr = [
        "P2 head 1: per-image TX budget {none|size1|min} at s6/s8 (FAST_TIER_PARITY_PLAN P2; FEATURE_HINTS sect E)",
        "labels: fastwins-2026-07-04 per-image direct BD vs same-speed base (train26, coarse 6q, tune-ss2+palette auto)",
        "veto convention: arm bd := max(bd,0) when its butteraugli_3n > +1.0 or _max > +1.5 (never bank a gamed win)",
        f"solo costs: s6 size1 1.67x min 4.57x; s8 size1 1.43x min 3.37x (fastwins solo section)",
        f"s6 RULE @ lam=1.0: {s6_keep[1]}",
        f"s8 RULE @ lam=1.0: {s8_keep[1]}",
        "columns: per-image bd/ba3n/bamax/veto-adj bd per arm + contended rdpar ratios + features + rule/LOOCV choices",
    ]
    out = pd.concat({"s6": s6_keep[3], "s8": s8_keep[3]}, names=["speed_tier"])
    to_tsv(out, os.path.abspath(OUT_TSV), hdr)


if __name__ == "__main__":
    main()

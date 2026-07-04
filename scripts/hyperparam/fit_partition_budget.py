#!/usr/bin/env python3
"""Per-image PARTITION-BUDGET head (FAST_TIER_PARITY P2 head 2; §E).

P1PART measured the partition-liveness/pruning response per image at s4-s8
(vs the s6+size1 / s8+size1 / stock-s4 bases): {pr1 (margin-pruned lite),
ship = r16no4_bkvg2 (rects live @16, 4-ways SPLIT-dominant-gated, breakout +
homogeneity vargate 2.0), vg2 = r16_bkvg2 (4-ways fully live), m32 =
r16m32_bkvg2 (+ partition max 16->32)}. Global verdicts: ship −2.91 med at
2.16x solo (landed release-gated); m32 is the pareto tip (−3.89 at 2.93x,
104% of the remaining step) recorded as the P2 per-image target.

This head picks a rung PER IMAGE: withhold (off) where partitions pay ~0,
ship as default, m32 where the per-image surface says large-block liveness
pays (fam-7000 plots: m32 recovery 255%).

Labels: p1part-2026-07-04 (train26, coarse 6-q, tune-ss2+palette auto,
PALCONF-clean). Costs: measured SOLO ratios from the p1part TSV solo
section. Same veto + degenerate-frontier conventions as fit_tx_budget.py.
"""

import itertools
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import (load_features, load_store, split_of,  # noqa: E402
                       to_tsv, veto_table)

OUT_TSV = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "../../benchmarks/hyperparam_partition_budget_2026-07-04.tsv")

SRC = "p1part-2026-07-04"

# Pre-registered candidates (mechanism-driven): rect partitions pay on
# directional/edge structure; m32 (large blocks) pays on big flat/screen
# regions; withhold candidates are the smooth-gradient classes where the
# measured recovery is smallest (9094: m32 10%).
SHORTLIST = [
    "edge_density", "orientation_energy_ratio", "patch_fraction",
    "flat_color_block_ratio", "uniformity", "aq_map_std",
    "luma_histogram_entropy", "gradient_fraction_smooth",
]
# Features where the WITHHOLD side is LOW (smooth/quiet content).
LOWSIDE = {"edge_density", "aq_map_std", "luma_histogram_entropy",
           "orientation_energy_ratio"}

# Menus per speed: rung -> (arm_id suffix, measured solo ratio vs base).
MENUS = {
    6: {"off": (None, 1.0), "pr1": ("r16_pr1", 1.866),
        "ship": ("r16no4_bkvg2", 2.155), "vg2": ("r16_bkvg2", 2.457),
        "m32": ("r16m32_bkvg2", 2.934)},
    8: {"off": (None, 1.0), "pr1": ("r16_pr1", 1.822),
        "ship": ("r16no4_bkvg2", 2.077), "vg2": ("r16_bkvg2", 2.323)},
    4: {"off": (None, 1.0), "pr1": ("r16_pr1", 1.446),
        "ship": ("r16no4_bkvg2", 1.749), "vg2": ("r16_bkvg2", 2.058)},
}
# 3-class rule rungs per speed: W -> off, D -> hi rung, else ship.
HI_RUNG = {6: "m32", 8: "vg2", 4: "vg2"}
LAMBDAS = [0.0, 0.25, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 1e9]


def veto_adj(vt):
    bd = vt["bd_ssim2"].to_numpy(float)
    ba3 = vt["bd_ba3n"].to_numpy(float)
    bam = vt["bd_bamax"].to_numpy(float)
    vet = (np.nan_to_num(ba3, nan=-np.inf) > 1.0) | \
          (np.nan_to_num(bam, nan=-np.inf) > 1.5)
    return np.where(vet, np.maximum(bd, 0.0), bd), vet


def byte_ratio_pct(store, speed, arm, base):
    df = store[(store["sweep_source"] == SRC) & (store["speed"] == speed)]
    a = df[df["arm_id"] == arm].groupby(["image_id", "q"])["bytes"].sum()
    b = df[df["arm_id"] == base].groupby(["image_id", "q"])["bytes"].sum()
    j = pd.concat({"a": a, "b": b}, axis=1).dropna()
    r = (j["a"] / j["b"] - 1.0) * 100.0
    return r.groupby(level=0).mean().rename("byte_pct")


def labels_for_speed(store, feats, speed):
    base = f"p1part/s{speed}-base"
    src = store[store["sweep_source"] == SRC]
    meta = (src[["image_id", "origin_id", "content_class", "feature_join"]]
            .drop_duplicates().set_index("image_id"))
    T = pd.DataFrame(index=sorted(
        src.loc[src["arm_id"] == base, "image_id"].unique()))
    T.index.name = "image_id"
    for c in ["origin_id", "content_class", "feature_join"]:
        T[c] = meta[c]
    T["family"] = T["content_class"].str.extract(r"^(\d+)")
    for rung, (suffix, _cost) in MENUS[speed].items():
        if suffix is None:
            continue
        aid = f"p1part/s{speed}-{suffix}"
        vt = veto_table(store, SRC, base, aid)
        adj, vet = veto_adj(vt)
        T[f"bd_{rung}"] = vt["bd_ssim2"]
        T[f"ba3n_{rung}"] = vt["bd_ba3n"]
        T[f"bamax_{rung}"] = vt["bd_bamax"]
        T[f"adj_{rung}"] = pd.Series(adj, index=vt.index)
        T[f"veto_{rung}"] = pd.Series(vet, index=vt.index)
        bp = byte_ratio_pct(store, speed, aid, base)
        nanmask = T[f"adj_{rung}"].isna()
        if nanmask.any():
            T.loc[nanmask, f"adj_{rung}"] = bp.reindex(T.index)[nanmask]
            T.loc[nanmask, f"bd_{rung}"] = bp.reindex(T.index)[nanmask]
    for f in SHORTLIST:
        T[f] = [feats.loc[fj, f] if fj in feats.index else np.nan
                for fj in T["feature_join"]]
    return T


def menu_arrays(T, speed):
    rungs = list(MENUS[speed].keys())
    bd = np.stack([np.zeros(len(T)) if MENUS[speed][r][0] is None
                   else T[f"adj_{r}"].fillna(0.0).to_numpy() for r in rungs])
    cost = np.array([MENUS[speed][r][1] for r in rungs])
    return rungs, bd, cost


def oracle_choice(T, speed, lam):
    rungs, bd, cost = menu_arrays(T, speed)
    obj = bd + lam * (cost[:, None] - 1.0)
    return np.array(rungs)[obj.argmin(axis=0)]


def realized(T, speed, choice):
    out_bd = np.zeros(len(T))
    out_t = np.zeros(len(T))
    for i, c in enumerate(choice):
        out_t[i] = MENUS[speed][c][1]
        if MENUS[speed][c][0] is not None:
            v = T[f"adj_{c}"].iloc[i]
            out_bd[i] = 0.0 if pd.isna(v) else v
    return out_bd, out_t


def rule_space(T, speed):
    def taus(f):
        u = np.unique(T[f].dropna().to_numpy())
        return (u[:-1] + u[1:]) / 2.0

    def pred(f, t, side):
        v = T[f].to_numpy(float)
        low = f in LOWSIDE
        if side == "withhold":
            return (v < t) if low else (v > t)
        return (v > t) if low else (v < t)

    z = np.zeros(len(T), bool)
    yield "always-ship", z, z, ()
    for fw in SHORTLIST:
        for tw in taus(fw):
            yield (f"W[{fw}@{tw:.4g}]", pred(fw, tw, "withhold"), z,
                   ((fw, tw, "W"),))
    for fd in SHORTLIST:
        for td in taus(fd):
            yield (f"U[{fd}@{td:.4g}]", z, pred(fd, td, "upgrade"),
                   ((fd, td, "U"),))
    for fw, fd in itertools.product(SHORTLIST, SHORTLIST):
        if fw == fd:
            continue
        for tw in taus(fw):
            w = pred(fw, tw, "withhold")
            for td in taus(fd):
                u = pred(fd, td, "upgrade") & ~w
                yield (f"W[{fw}@{tw:.4g}] U[{fd}@{td:.4g}]", w, u,
                       ((fw, tw, "W"), (fd, td, "U")))


def rule_to_choice(w, u, speed):
    c = np.full(len(w), "ship", dtype=object)
    c[u] = HI_RUNG[speed]
    c[w] = "off"
    return c


def fit_rule(T, speed, lam):
    best = None
    for desc, w, u, spec in rule_space(T, speed):
        bd, t = realized(T, speed, rule_to_choice(w, u, speed))
        obj = (bd + lam * (t - 1.0)).mean()
        key = (obj, len(spec))
        if best is None or key < best[0]:
            best = (key, desc, spec)
    return best[1], best[2]


def apply_spec(spec, df, speed):
    w = np.zeros(len(df), bool)
    u = np.zeros(len(df), bool)
    for f, t, kind in spec:
        v = df[f].to_numpy(float)
        low = f in LOWSIDE
        if kind == "W":
            w |= (v < t) if low else (v > t)
        else:
            u |= (v > t) if low else (v < t)
    u &= ~w
    return rule_to_choice(w, u, speed)


def main():
    store = load_store()
    feats = load_features(SHORTLIST)
    keeps = {}

    for speed in (6, 8, 4):
        T = labels_for_speed(store, feats, speed)
        rungs = list(MENUS[speed].keys())
        print(f"\n================ s{speed} (base = s{speed}"
              f"{'+size1' if speed != 4 else ' stock'}) ================")
        cols = [c for c in T.columns
                if c.startswith(("bd_", "adj_", "veto_"))]
        with pd.option_context("display.width", 260, "display.max_columns", 99):
            print(T[["origin_id", "family"] + cols].sort_values("family")
                  .round(2).to_string())

        print("\n=== lambda frontier: ORACLE vs global rungs "
              "(mean veto-adj BD / mean solo cost) ===")
        for r in rungs:
            if MENUS[speed][r][0] is None:
                continue
            bd = T[f"adj_{r}"].fillna(0.0)
            print(f"  global-{r:5s}: bd mean {bd.mean():+.3f} med "
                  f"{bd.median():+.3f}  cost {MENUS[speed][r][1]:.2f}x")
        for lam in LAMBDAS:
            ch = oracle_choice(T, speed, lam)
            bd, t = realized(T, speed, ch)
            n = {r: int((ch == r).sum()) for r in rungs if (ch == r).any()}
            print(f"  oracle lam={lam:<5g}: bd mean {bd.mean():+.3f} med "
                  f"{np.median(bd):+.3f}  cost {t.mean():.2f}x  mix {n}")

        for lam in (0.5, 1.0, 2.0):
            desc, spec = fit_rule(T, speed, lam)
            ch = apply_spec(spec, T, speed)
            bd, t = realized(T, speed, ch)
            och = oracle_choice(T, speed, lam)
            obd, ot = realized(T, speed, och)
            loo_bd, loo_t = np.zeros(len(T)), np.zeros(len(T))
            loo_ch = np.full(len(T), "", object)
            for org in T["origin_id"].unique():
                mask = (T["origin_id"] == org).to_numpy()
                _, sp = fit_rule(T[~mask], speed, lam)
                chh = apply_spec(sp, T[mask], speed)
                b, tt = realized(T[mask], speed, chh)
                loo_bd[mask], loo_t[mask], loo_ch[mask] = b, tt, chh
            gl = T["adj_ship"].fillna(0.0)
            print(f"\n  --- rule @ lam={lam} (W->off, U->{HI_RUNG[speed]}, "
                  f"else ship) ---")
            print(f"  RULE: {desc}")
            mix = {r: int((ch == r).sum()) for r in rungs if (ch == r).any()}
            print(f"  fit     : bd mean {bd.mean():+.3f} med {np.median(bd):+.3f} "
                  f"cost {t.mean():.2f}x mix {mix}")
            print(f"  oracle  : bd mean {obd.mean():+.3f} med {np.median(obd):+.3f} "
                  f"cost {ot.mean():.2f}x")
            print(f"  LOOCV   : bd mean {loo_bd.mean():+.3f} med "
                  f"{np.median(loo_bd):+.3f} cost {loo_t.mean():.2f}x")
            print(f"  vs global-ship: fit {bd.mean() - gl.mean():+.3f} mean BD at "
                  f"{t.mean() - MENUS[speed]['ship'][1]:+.2f}x cost; "
                  f"LOOCV {loo_bd.mean() - gl.mean():+.3f}")
            for r in ("off", HI_RUNG[speed]):
                sel = T.index[ch == r]
                if len(sel):
                    print(f"  {r:4s} <- " + ", ".join(
                        f"{T.loc[i, 'origin_id']}"
                        f"({T.loc[i, 'adj_ship']:+.1f}sh"
                        + (f"/{T.loc[i, 'adj_' + HI_RUNG[speed]]:+.1f}hi)"
                           if MENUS[speed][HI_RUNG[speed]][0] else ")")
                        for i in sel))
            if lam == 1.0:
                T["rule_choice"] = ch
                T["loocv_choice"] = loo_ch
                keeps[speed] = (desc, spec, T.copy())

    # val firing-rate sanity for the s6 lam=1.0 rule
    desc6, spec6, _T6 = keeps[6]
    print("\n=== val-origin choice rates for the s6 lam=1.0 rule "
          "(feature rows only; no RD labels) ===")
    allf = load_features(SHORTLIST).reset_index()
    allf["base"] = allf["image_path"].str.rsplit("/", n=1).str[-1]
    allf["split"] = allf["base"].map(split_of)
    va = allf[(allf["split"] == "val") & (allf["crop_label"] == "full")
              & allf["size_class"].isin(["1024", "native"])].copy()
    va["choice"] = apply_spec(spec6, va, 6)
    va["cgroup"] = va["content_class"].str.extract(r"^(\d+)")
    print(va.pivot_table(index="cgroup", columns="choice", values="base",
                         aggfunc="count", fill_value=0).to_string())

    hdr = [
        "P2 head 2: per-image PARTITION budget at s4-s8 (FAST_TIER_PARITY_PLAN P2; FEATURE_HINTS sect E)",
        "labels: p1part-2026-07-04 per-image direct BD vs same-tier base (train26, coarse 6q; s6/s8 bases ride P0 size1)",
        "menus: s6 {off 1.0, pr1 1.866, ship=r16no4_bkvg2 2.155, vg2=r16_bkvg2 2.457, m32=r16m32_bkvg2 2.934}; "
        "s8 {off, pr1 1.822, ship 2.077, vg2 2.323}; s4 {off, pr1 1.446, ship 1.749, vg2 2.058} (solo ratios)",
        "veto: bd := max(bd,0) when arm butteraugli_3n > +1.0 or _max > +1.5; NaN hull -> same-q byte%",
        f"s6 RULE @ lam=1.0: {keeps[6][0]}",
        f"s8 RULE @ lam=1.0: {keeps[8][0]}",
        f"s4 RULE @ lam=1.0: {keeps[4][0]}",
    ]
    out = pd.concat({f"s{s}": keeps[s][2] for s in (6, 8, 4)},
                    names=["speed_tier"])
    to_tsv(out, os.path.abspath(OUT_TSV), hdr)


if __name__ == "__main__":
    main()

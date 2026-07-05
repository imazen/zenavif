#!/usr/bin/env python3
"""S4-tier operating point design (FAST_TIER_PARITY_PLAN, the last open column).

The composed s6 fast mode + global intra7 (p2heads) sits at ~5.11x plain-s6
solo (~5.24 s/MP) and still trails aom cpu2iq-allintra (6.47x plain-s6,
1.27x our wall) by +4.40 ssim2 / +4.04 ba3n photos median. This script mines
the committed fit-TSV labels + the label store + raw timing cells OFFLINE to
design the s4-tier operating point:

  A. residual map: per-image composed-v2+i7 BD vs cpu2iq-ai / cpu2def-ai;
  B. wall calibration: additive cost model vs the measured per-class walls;
  C. knapsack lambda scan over the composable menu
       tx   {none < size1 < min < full}   (labels: hyperparam_tx_budget TSV)
       part {ship < vg2 < m32}            (labels: hyperparam_partition TSV)
     with per-image measured i7 wall multipliers, budget = cpu2iq-ai wall;
  D. rule refits at s4-tier lambda via the existing fit modules (deployable
     H1_s4/H2_s4, LOOCV) + comparison to the oracle map;
  E. projected column after v3 (labels-additive; the honest structural
     residue preview) on BOTH metrics;
  F. emit per-class sample TSVs for the box confirm chain.

IMPORTANT honesty notes:
  * per-image min/full solo costs are DESIGN-ONLY priors (contended rdpar x
    1.10 inflation); the box chain re-measures the final point solo. full's
    solo ratio was never measured (no timing_w2_s6_full) -- box adds it.
  * tx labels ride the stock s6 base (no partition arms); partition labels
    ride size1. Composition validated only at the measured p2heads classes;
    fresh (tx,part) combos are the box confirm's job.
  * W-class + fam-7000 images stay LOCKED to their v2 classes (measured
    12q factoring cells beat the coarse labels there).
"""

import argparse
import os
import sys

import numpy as np
import pandas as pd

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "../rd_gap"))
from bd_arm import bd_rate, frontier  # noqa: E402
from hp_common import load_features, load_store  # noqa: E402
import fit_tx_budget as ftx  # noqa: E402
import fit_partition_budget as fpb  # noqa: E402

RAW_P2 = "/mnt/v/output/zenavif/p2heads-20260704"
BENCH = os.path.join(HERE, "../../benchmarks")
CLASSES = ["none_ship", "none_m32", "size1_ship", "size1_m32", "min_ship", "min_m32"]
TX_ORDER = ["none", "size1", "min", "full"]
PART_ORDER = ["ship", "vg2", "m32"]
# additive solo marginals in plain-s6 units (measured solo sections)
TX_COST = {"none": 0.0, "size1": 0.67, "min": 3.57}   # full: per-image prior
PART_COST = {"ship": 1.155, "vg2": 1.457, "m32": 1.934}
BUDGET_X = 6.47  # cpu2iq-ai wall / plain-s6-tune wall (ladder solo medians)


def pts_of(g, metric):
    out = []
    for v, bpp in zip(pd.to_numeric(g[metric], errors="coerce"),
                      pd.to_numeric(g["bpp"], errors="coerce")):
        if not np.isfinite(v) or not np.isfinite(bpp) or bpp <= 0:
            continue
        if metric.startswith("butteraugli"):
            if v <= 0:
                continue
            v = -np.log(v)
        out.append((float(v), float(bpp)))
    return out


def per_image_bd(base_df, arm_df, metric, key="image_id"):
    out = {}
    for img in sorted(set(base_df[key]) & set(arm_df[key])):
        bd = bd_rate(frontier(pts_of(arm_df[arm_df[key] == img], metric)),
                     frontier(pts_of(base_df[base_df[key] == img], metric)))
        if bd is not None:
            out[img] = bd
    return pd.Series(out, dtype=float)


def load_solo(name):
    p = os.path.join(RAW_P2, name)
    if not os.path.exists(p):
        return None
    t = pd.read_csv(p, sep="\t")
    t["enc_ms"] = pd.to_numeric(t["enc_ms"], errors="coerce")
    s = t.groupby("image")["enc_ms"].sum()
    s.index = [os.path.basename(i) for i in s.index]
    return s


def read_fit_tsv(name):
    return pd.read_csv(os.path.join(BENCH, name), sep="\t", comment="#")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--emit-samples", default=None)
    args = ap.parse_args()

    store = load_store()
    t26 = store[store["corpus"] == "train26"]
    fam = (t26[["image_id", "content_class"]].drop_duplicates()
           .set_index("image_id")["content_class"].str.extract(r"^(\d+)")[0])

    # ---- composed-v2(+i7) cells + current classes --------------------------
    def composed_cells(prefix, remap):
        parts = []
        for cls in CLASSES:
            a = t26[t26["arm_id"] == f"p2heads/{prefix}-{cls}"].copy()
            if len(a):
                a["p2class"] = cls
                parts.append(a)
        comp = pd.concat(parts, ignore_index=True)
        rx = t26[t26["arm_id"] == remap].copy()
        rx["p2class"] = "size1_m32"
        return pd.concat([comp[~comp["image_id"].str.contains("7028")], rx],
                         ignore_index=True)

    comp_i7 = composed_cells("composedi7", "p2heads/rx-7028-size1-m32-i7")
    cls_of = (comp_i7[["image_id", "p2class"]].drop_duplicates()
              .set_index("image_id")["p2class"])
    refs = {r: t26[t26["arm_id"] == f"speedladder/{r}"]
            for r in ["aom-cpu2iq-ai", "aom-cpu2def-ai"]}

    # ---- A. residual map ----------------------------------------------------
    print("=" * 110)
    print("A. RESIDUAL: composed-v2+i7 vs cpu2iq-ai (photos = non-7000)")
    print("=" * 110)
    R = pd.DataFrame({
        "fam": fam, "cls": cls_of,
        "iq_s2": per_image_bd(refs["aom-cpu2iq-ai"], comp_i7, "ssim2"),
        "iq_b3": per_image_bd(refs["aom-cpu2iq-ai"], comp_i7, "butteraugli_3n"),
        "def_s2": per_image_bd(refs["aom-cpu2def-ai"], comp_i7, "ssim2"),
        "def_b3": per_image_bd(refs["aom-cpu2def-ai"], comp_i7, "butteraugli_3n"),
    })
    ph = R[(R["fam"] != "7000") & R["iq_s2"].notna()]
    print(f"photos n={len(ph)}: vs cpu2iq {ph['iq_s2'].median():+.2f}/{ph['iq_b3'].median():+.2f}"
          f"   vs cpu2def {ph['def_s2'].median():+.2f}/{ph['def_b3'].median():+.2f}")

    # ---- labels from the committed fit TSVs ---------------------------------
    TX = read_fit_tsv("hyperparam_tx_budget_2026-07-04.tsv")
    TX = TX[TX["speed_tier"] == "s6"].set_index("image_id")
    PT = read_fit_tsv("hyperparam_partition_budget_2026-07-04.tsv")
    PT = PT[PT["speed_tier"] == "s6"].set_index("image_id")

    def tx_val(img, arm):  # adj BD vs stock s6 base (veto-adjusted)
        if arm == "none":
            return 0.0
        v = TX.at[img, f"adj_{arm}"] if img in TX.index else np.nan
        return float(v) if np.isfinite(v) else np.nan

    def tx_b3(img, arm):
        if arm == "none":
            return 0.0
        v = TX.at[img, f"ba3n_{arm}"] if img in TX.index else np.nan
        return float(v) if np.isfinite(v) else 0.0

    def pt_val(img, arm):
        v = PT.at[img, f"adj_{arm}"] if img in PT.index else np.nan
        return float(v) if np.isfinite(v) else np.nan

    def pt_b3(img, arm):
        v = PT.at[img, f"ba3n_{arm}"] if img in PT.index else np.nan
        return float(v) if np.isfinite(v) else 0.0

    # ---- walls ---------------------------------------------------------------
    plain = load_solo("p2t_s6_plain.tsv")
    cwall, cwall7 = {}, {}
    for cls in CLASSES:
        s = load_solo(f"p2t_c_{cls}.tsv")
        if s is not None:
            cwall.update(s.to_dict())
        s = load_solo(f"p2ti7_c_{cls}.tsv")
        if s is not None:
            cwall7.update(s.to_dict())
    cwall = pd.Series(cwall)
    cwall7 = pd.Series(cwall7)
    i7m = (cwall7 / cwall)

    # ---- B. cost-model calibration -------------------------------------------
    print("\n" + "=" * 110)
    print("B. WALL CALIBRATION: measured class wall (no-i7) vs additive model, per current class")
    print("=" * 110)
    cal = {}
    for img in cls_of.index:
        b = os.path.basename(img)
        if b not in plain.index or b not in cwall.index:
            continue
        tx, part = cls_of[img].rsplit("_", 1)
        pred = 1.0 + TX_COST.get(tx, 0.0) + PART_COST[part]
        meas = cwall[b] / plain[b]
        cal[b] = (cls_of[img], pred, meas, meas / pred)
    C = pd.DataFrame(cal, index=["cls", "pred", "meas", "ratio"]).T
    C[["pred", "meas", "ratio"]] = C[["pred", "meas", "ratio"]].astype(float)
    print(C.groupby("cls")[["pred", "meas", "ratio"]].agg(["mean", "count"]).round(2).to_string())
    calib = C.groupby("cls")["ratio"].mean().to_dict()
    gcal = float(C["ratio"].mean())
    print(f"global calibration factor (meas/pred): {gcal:.3f}")

    # per-image design costs for tx arms (contended rdpar prior; DESIGN-ONLY)
    def tx_cost_i(img, arm):
        if arm in TX_COST:
            base = TX_COST[arm]
            if arm == "min" and img in TX.index and np.isfinite(TX.at[img, "rdpar_min"]):
                return max(0.0, float(TX.at[img, "rdpar_min"]) * 1.10 - 1.0)
            if arm == "size1" and img in TX.index and np.isfinite(TX.at[img, "rdpar_size1"]):
                return max(0.0, float(TX.at[img, "rdpar_size1"]) * 1.10 - 1.0)
            return base
        # full: contended rdpar x 1.10 (prior; NO measured solo yet)
        if img in TX.index and np.isfinite(TX.at[img, "rdpar_full"]):
            return max(0.0, float(TX.at[img, "rdpar_full"]) * 1.10 - 1.0)
        return 6.0

    def unit_wall(img, tx, part, with_i7=True):
        m = float(i7m.get(img, np.nan))
        if not np.isfinite(m):
            m = 1.45
        u = (1.0 + tx_cost_i(img, tx) + PART_COST[part]) * gcal
        return u * (m if with_i7 else 1.0)

    # ---- C. knapsack lambda scan ---------------------------------------------
    print("\n" + "=" * 110)
    print("C. KNAPSACK over {tx x part} upgrades from v2 classes (i7 on; budget 6.47x plain)")
    print("=" * 110)
    imgs = [i for i in cls_of.index]
    basewall = plain
    total_plain = sum(basewall.get(os.path.basename(i), np.nan) for i in imgs)
    LOCKED = {i for i in imgs if fam.get(i) == "7000"}
    # tx harm caps from labels: never offer min/full where their adj >= 0 or veto
    def menu_for(img):
        tx0, p0 = cls_of[img].rsplit("_", 1)
        if img in LOCKED:
            return [(tx0, p0)]
        cands = []
        for tx in TX_ORDER[TX_ORDER.index(tx0):]:
            if tx in ("min", "full"):
                v = tx_val(img, tx)
                vetoed = bool(TX.at[img, f"veto_{tx}"]) if img in TX.index else True
                if not np.isfinite(v) or v >= 0 or vetoed:
                    continue
            for part in PART_ORDER[PART_ORDER.index(p0):]:
                cands.append((tx, part))
        return cands or [(tx0, p0)]

    def value_of(img, tx, part):
        tx0, p0 = cls_of[img].rsplit("_", 1)
        dv = (0.0 if tx == tx0 else (tx_val(img, tx) - (tx_val(img, tx0) if tx0 != "none" else 0.0)))
        dp = (0.0 if part == p0 else (pt_val(img, part) - pt_val(img, p0)))
        if not np.isfinite(dv):
            dv = 0.0
        if not np.isfinite(dp):
            dp = 0.0
        return dv + dp

    def b3_of(img, tx, part):
        tx0, p0 = cls_of[img].rsplit("_", 1)
        dv = (0.0 if tx == tx0 else (tx_b3(img, tx) - (tx_b3(img, tx0) if tx0 != "none" else 0.0)))
        dp = (0.0 if part == p0 else (pt_b3(img, part) - pt_b3(img, p0)))
        return dv + dp

    maps = {}
    for lam in [2.0, 1.0, 0.6, 0.4, 0.25, 0.1, 0.0]:
        picks = {}
        for img in imgs:
            tx0, p0 = cls_of[img].rsplit("_", 1)
            w0 = unit_wall(img, tx0, p0)
            best, bobj = (tx0, p0), 0.0
            for tx, part in menu_for(img):
                obj = value_of(img, tx, part) + lam * (unit_wall(img, tx, part) - w0)
                if obj < bobj - 1e-9:
                    best, bobj = (tx, part), obj
            picks[img] = best
        wall = sum(unit_wall(i, *picks[i]) * basewall.get(os.path.basename(i), np.nan)
                   for i in imgs) / total_plain
        val = sum(value_of(i, *picks[i]) for i in imgs)
        ups = {i: p for i, p in picks.items() if p != tuple(cls_of[i].rsplit("_", 1))}
        print(f"lam={lam:4.2f}: upgrades={len(ups):2d} est wall {wall:.2f}x "
              f"sum adjBD {val:+.1f} mean {val / len(imgs):+.2f}")
        maps[lam] = picks
        if lam in (0.6, 0.4, 0.25):
            for i, p in sorted(ups.items()):
                print(f"    {os.path.basename(i)[:46]:46s} {cls_of[i]:>11} -> {p[0]}_{p[1]:<5} "
                      f"dv={value_of(i, *p):+5.2f} b3={b3_of(i, *p):+5.2f} "
                      f"dwall={(unit_wall(i, *p) - unit_wall(i, *cls_of[i].rsplit('_', 1))):+4.1f}u")

    # ---- D. rule refits at s4-tier lambda ------------------------------------
    print("\n" + "=" * 110)
    print("D. RULE REFITS (deployable head forms, LOOCV) at s4-tier lambdas")
    print("=" * 110)
    feats_tx = load_features(ftx.SHORTLIST)
    feats_pt = load_features(fpb.SHORTLIST)
    Ttx = ftx.labels_for_speed(store, feats_tx, 6)
    Tpt = fpb.labels_for_speed(store, feats_pt, 6)
    for lam in (0.5, 0.25, 0.1):
        desc, spec, w, d = ftx.fit_rule(Ttx, 6, lam)
        ch = ftx.apply_spec(spec, Ttx)
        bd, t = ftx.realized(Ttx, 6, ch)
        loo_bd, loo_t = np.zeros(len(Ttx)), np.zeros(len(Ttx))
        loo_ch = np.empty(len(Ttx), dtype=object)
        for org in Ttx["origin_id"].unique():
            mask = (Ttx["origin_id"] == org).to_numpy()
            _, sp, _, _ = ftx.fit_rule(Ttx[~mask], 6, lam)
            chh = ftx.apply_spec(sp, Ttx[mask])
            b, tt = ftx.realized(Ttx[mask], 6, chh)
            loo_bd[mask], loo_t[mask] = b, tt
            loo_ch[mask] = chh
        mix = {m: int((ch == m).sum()) for m in ftx.MENU}
        print(f"  H1_s4 tx @lam={lam}: {desc}  mix={mix}  fit mean {bd.mean():+.2f}@{t.mean():.2f}x  "
              f"LOOCV mean {loo_bd.mean():+.2f}@{loo_t.mean():.2f}x  "
              f"loocv-agree {(loo_ch == ch).sum()}/{len(ch)}")
    for lam in (0.5, 0.25, 0.1):
        desc, spec = fpb.fit_rule(Tpt, 6, lam)
        ch = fpb.apply_spec(spec, Tpt, 6)
        bd, t = fpb.realized(Tpt, 6, ch)
        loo_bd = np.zeros(len(Tpt))
        loo_ch = np.empty(len(Tpt), dtype=object)
        for org in Tpt["origin_id"].unique():
            mask = (Tpt["origin_id"] == org).to_numpy()
            _, sp = fpb.fit_rule(Tpt[~mask], 6, lam)
            chh = fpb.apply_spec(sp, Tpt[mask], 6)
            b, tt = fpb.realized(Tpt[mask], 6, chh)
            loo_bd[mask] = b
            loo_ch[mask] = chh
        mix = {m: int((ch == m).sum()) for m in fpb.MENUS[6]}
        print(f"  H2_s4 part @lam={lam}: {desc}  mix={mix}  fit mean {bd.mean():+.2f}@{t.mean():.2f}x  "
              f"LOOCV mean {loo_bd.mean():+.2f}  loocv-agree {(loo_ch == ch).sum()}/{len(ch)}")

    # ---- E. projected column -------------------------------------------------
    print("\n" + "=" * 110)
    print("E. PROJECTED COLUMN at the knapsack maps (labels-additive; box confirms)")
    print("=" * 110)
    for lam in (0.6, 0.4, 0.25):
        picks = maps[lam]
        proj = {}
        for img in ph.index:
            if img not in cls_of.index:
                continue
            proj[img] = R.at[img, "iq_s2"] + value_of(img, *picks[img])
        pr = pd.Series(proj)
        wall = sum(unit_wall(i, *picks[i]) * basewall.get(os.path.basename(i), np.nan)
                   for i in imgs) / total_plain
        print(f"lam={lam}: projected photos median vs cpu2iq {pr.median():+.2f} "
              f"(from {ph['iq_s2'].median():+.2f}) at est wall {wall:.2f}x "
              f"[cpu2iq budget 6.47x]")
    print("\nper-image projected (lam=0.4): resid -> proj  (upgrade)")
    picks = maps[0.4]
    for img in ph.sort_values("iq_s2", ascending=False).index:
        if img not in cls_of.index:
            continue
        p = picks[img]
        print(f"  {os.path.basename(img)[:46]:46s} {R.at[img, 'iq_s2']:+6.2f} -> "
              f"{R.at[img, 'iq_s2'] + value_of(img, *p):+6.2f}  "
              f"{cls_of[img]:>11} -> {p[0]}_{p[1]}")

    # ---- F. emit -------------------------------------------------------------
    if args.emit_samples:
        os.makedirs(args.emit_samples, exist_ok=True)
        picks = maps[0.4]
        meta = t26[["image_id", "image_path", "w", "h", "content_class"]].drop_duplicates() \
            if "image_path" in t26.columns else None
        groups = {}
        for img, (tx, part) in picks.items():
            groups.setdefault(f"{tx}_{part}", []).append(img)
        for cls, lst in sorted(groups.items()):
            print(f"  v3 class {cls}: {len(lst)}")
            for i in lst:
                print(f"      {os.path.basename(i)}")


if __name__ == "__main__":
    main()

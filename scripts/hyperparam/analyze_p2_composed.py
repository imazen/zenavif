#!/usr/bin/env python3
"""P2HEADS analysis: head-3 intra response + composed fast-mode verdict.

Inputs: the fetched chain_p2heads.sh OUTDIR (default
/mnt/v/output/zenavif/p2heads-20260704). Produces:

1. Head 3 (intra-mode budget): per-image BD of intra7 (top-7 keyframe intra
   RDO, filter_intra off) vs the s6+size1 base, and of intra7+ship vs ship
   (composition check), s6 + s8. Threshold-rule fit only if the response has
   per-image structure worth a head (report either way).
2. Composed fast-mode: per-class rows merged into one arm; per-image BD vs
   p2_conf_s6_base (s6+size1) and vs p2_conf_s6_ship (global ship point);
   family table; butteraugli veto columns.
3. VAL transfer: composed vs ship on the 14 VAL-LSD origins (held-out).
4. Parity scoreboard: composed + global-ship vs the cached aom-allintra refs
   (label store speedladder arms cpu4def/cpu4iq/cpu6iq-ai), photos median
   (= train26 minus fam-7000, the p1part ladder convention).
5. Solo timing: composed mean wall vs plain s6 / size1 / global ship.
"""

import argparse
import csv
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "../rd_gap"))
from bd_arm import LOWER_BETTER, bd_rate, frontier  # noqa: E402
from hp_common import load_store, print_dist  # noqa: E402

METRICS = ["ssim2", "butteraugli_3n", "butteraugli_max"]
CLASSES = ["none_ship", "none_m32", "size1_ship", "size1_m32", "min_ship", "min_m32"]


def load_tsv(path):
    rows = []
    with open(path) as f:
        for r in csv.DictReader(f, delimiter="\t"):
            rows.append(r)
    df = pd.DataFrame(rows)
    note_fams(df)
    return df


def pts(df, img, metric):
    g = df[df["image"] == img]
    out = []
    for _, r in g.iterrows():
        try:
            v, bpp = float(r[metric]), float(r["bpp"])
        except (ValueError, KeyError):
            continue
        if not np.isfinite(v) or not np.isfinite(bpp) or bpp <= 0:
            continue
        if metric in LOWER_BETTER:
            if v <= 0:
                continue
            v = -np.log(v)
        out.append((v, bpp))
    return out


def per_image_bd(base_df, arm_df, metric):
    out = {}
    for img in sorted(base_df["image"].unique()):
        if img not in set(arm_df["image"]):
            continue
        bd = bd_rate(frontier(pts(arm_df, img, metric)),
                     frontier(pts(base_df, img, metric)))
        if bd is not None:
            out[img] = bd
    return pd.Series(out)


def veto_frame(base_df, arm_df):
    d = {}
    for m, n in zip(METRICS, ["bd_ssim2", "bd_ba3n", "bd_bamax"]):
        d[n] = per_image_bd(base_df, arm_df, m)
    T = pd.DataFrame(d)
    vet = (T["bd_ba3n"].fillna(-np.inf) > 1.0) | (T["bd_bamax"].fillna(-np.inf) > 1.5)
    T["adj"] = np.where(vet, np.maximum(T["bd_ssim2"], 0.0), T["bd_ssim2"])
    T["veto"] = vet
    return T


_FAM = {}


def note_fams(df):
    """Record the sample TSVs' own family column (authoritative)."""
    if "family" in df.columns:
        for img, fam in zip(df["image"], df["family"]):
            _FAM[img] = fam
            _FAM[os.path.basename(img)] = fam


def fam_of(img):
    return _FAM.get(img, _FAM.get(os.path.basename(img), "?"))


def summarize(name, T, fams=None, section=None):
    v = T["adj"].dropna()
    print(f"  {name:28s} n={len(v):2d} med {v.median():+7.3f} mean {v.mean():+7.3f} "
          f"better {(v < 0).sum()}/{len(v)} vetoed {int(T['veto'].sum())}")
    if section:
        emit(section, name, "ssim2_vetoadj", len(v), v.median(), v.mean(),
             f"{(v < 0).sum()}/{len(v)}", f"vetoed={int(T['veto'].sum())}")
        for col, mn in (("bd_ba3n", "butteraugli_3n"), ("bd_bamax", "butteraugli_max")):
            b = T[col].dropna()
            if len(b):
                emit(section, name, mn, len(b), b.median(), b.mean(),
                     f"{(b < 0).sum()}/{len(b)}")


TSV_ROWS = []
TSV_HEADER = [
    "P2HEADS (FAST_TIER_PARITY_PLAN Phase P2) -- 2026-07-04 -- per-image hyperparameter heads: "
    "tx budget + partition budget (frozen threshold rules) + intra-mode-budget axis + composed fast mode",
    "Box zenavif-sweep-1 (ccx63 48c, FROM_SNAPSHOT=auto); harness scripts/rd_gap/chain_p2heads.sh; "
    "analyzer scripts/hyperparam/analyze_p2_composed.py; fits scripts/hyperparam/fit_{tx,partition}_budget.py",
    "Code: zenrav1e master 39f0ecdd (INCLUDES one-sided margin fix 767c8ff5) via ravif--p2heads devpatch "
    "(p1part passthroughs + ZENRAVIF_REDUCED_TX + ZENRAVIF_INTRA_MODES; box cavif sha256/16 bd0b33d2ec5ef156). "
    "INCIDENT: a first pass ran a stale workspace (e944ea71, symmetric margins) -- caught by 6/144 "
    "byte-continuity vs p1part ship cells, wiped, re-run; base cells byte-match p1part 144/144 on both passes",
    "Frozen rules: tx = {patch_fraction>0.8505 -> LARGEST | dct_compressibility_y<8.352 -> MIN | else SIZE1} "
    "(s6-s8); partition = {gradient_fraction_smooth<0.4105 -> M32(r16m32_bkvg2) | else SHIP(r16no4_bkvg2)} (s6)",
    "All arms train26/val14 tune-ss2 + palette auto, --threads 1, BUTTER on, PALCONF=1 (0 CELLFAIL/CONFFAIL); "
    "composed = per-(tx,part)-class 12q sub-runs merged; intra7 = ComplexKeyframes + filter_intra=Some(false); "
    "BD per-image monotone-frontier hull (bd_arm.py), vetoadj = max(bd,0) when arm ba3n>+1.0 or bamax>+1.5",
    "parity rows: per-image BD vs the CACHED speedladder aom-allintra refs (photos = t26 minus fam-7000, n=20); "
    "timing rows: solo JOBS=1 RD_CACHE=off q{40,65,85} wall ratios vs plain s6-tune",
]


def emit(section, name, metric, n, med, mean, better, extra=""):
    TSV_ROWS.append((section, name, metric, n, f"{med:+.4f}", f"{mean:+.4f}",
                     better, extra))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("outdir", nargs="?",
                    default="/mnt/v/output/zenavif/p2heads-20260704")
    ap.add_argument("--tsv", default=None,
                    help="write the summary TSV (benchmarks record)")
    args = ap.parse_args()
    d = args.outdir

    # ---------- head 3: intra budget ----------
    print("=== HEAD 3: intra-mode budget (top-7 vs forced top-3) ===")
    tables = {}
    for tag, base, arm in [
        ("s6 intra7 vs size1-base", "p2_s6_base.tsv", "p2_s6_intra7.tsv"),
        ("s6 intra7 ON SHIP vs ship", "p2_s6_ship.tsv", "p2_s6_intra7ship.tsv"),
        ("s8 intra7 vs size1-base", "p2_s8_base.tsv", "p2_s8_intra7.tsv"),
    ]:
        bp, ap_ = os.path.join(d, base), os.path.join(d, arm)
        if not (os.path.exists(bp) and os.path.exists(ap_)):
            print(f"  MISSING {base} / {arm}")
            continue
        T = veto_frame(load_tsv(bp), load_tsv(ap_))
        tables[tag] = T
        summarize(tag, T, section="intra")
    if "s6 intra7 vs size1-base" in tables:
        T = tables["s6 intra7 vs size1-base"]
        T2 = T.copy()
        T2["family"] = [fam_of(i) for i in T2.index]
        print("\n  per-image s6 intra7 (adj BD, worst->best):")
        for img, r in T2.sort_values("adj", ascending=False).iterrows():
            print(f"    {r['family']:>4} {img[:58]:60s} {r['adj']:+7.3f}"
                  f"{' VETO' if r['veto'] else ''}")

    # ---------- composed ----------
    if not os.path.exists(os.path.join(d, "p2c_min_m32.tsv")):
        print("\n(composed TSVs not present yet — intra-only analysis)")
        return
    print("\n=== COMPOSED fast mode (per-image heads 1+2), s6, 12q ===")
    base = load_tsv(os.path.join(d, "p2_conf_s6_base.tsv"))
    ship = load_tsv(os.path.join(d, "p2_conf_s6_ship.tsv"))
    comp_parts = []
    for cls in CLASSES:
        p = os.path.join(d, f"p2c_{cls}.tsv")
        if os.path.exists(p):
            t = load_tsv(p)
            t["p2class"] = cls
            comp_parts.append(t)
    comp = pd.concat(comp_parts, ignore_index=True)
    print(f"  composed rows {len(comp)} over {comp['image'].nunique()} images "
          f"({', '.join(f'{c}:{(comp.p2class == c).sum() // 12}' for c in CLASSES if (comp.p2class == c).any())})")

    Tc = veto_frame(base, comp)
    Ts = veto_frame(base, ship)
    Tcs = veto_frame(ship, comp)
    summarize("composed vs s6+size1 base", Tc, section="composed")
    summarize("global-ship vs s6+size1", Ts, section="composed")
    summarize("composed vs global-ship", Tcs, section="composed")

    # ---- rules v2 (post-val attribution): swap 7028 -> (size1,m32) ----
    rx = os.path.join(d, "p2rx_7028_size1_m32.tsv")
    comp_v2 = None
    if os.path.exists(rx):
        rx_t = load_tsv(rx)
        rx_t["p2class"] = "size1_m32"
        comp_v2 = pd.concat(
            [comp[~comp["image"].str.contains("7028")], rx_t], ignore_index=True)
        Tc2 = veto_frame(base, comp_v2)
        Tcs2 = veto_frame(ship, comp_v2)
        summarize("composed-v2 vs s6+size1 base", Tc2, section="composed")
        summarize("composed-v2 vs global-ship", Tcs2, section="composed")

    fam = pd.DataFrame({
        "family": [fam_of(i) for i in Tc.index],
        "composed": Tc["adj"], "ship": Ts["adj"].reindex(Tc.index),
    })
    print("\n  per-family (median adj BD vs s6+size1 base):")
    ftab = fam.groupby("family").agg(n=("composed", "size"),
                                     composed=("composed", "median"),
                                     ship=("ship", "median"))
    ftab["delta"] = ftab["composed"] - ftab["ship"]
    print(ftab.round(2).to_string())

    # ---------- val transfer ----------
    print("\n=== VAL transfer (14 VAL-LSD origins, s6, 12q) ===")
    vb = os.path.join(d, "p2v_base.tsv")
    if os.path.exists(vb) and not os.path.exists(os.path.join(d, "p2v_ship.tsv")):
        print("  (val leg incomplete — skipping)")
        vb = "/nonexistent"
    if os.path.exists(vb):
        vbase = load_tsv(vb)
        vship = load_tsv(os.path.join(d, "p2v_ship.tsv"))
        vparts = []
        for cls in CLASSES:
            p = os.path.join(d, f"p2vc_{cls}.tsv")
            if os.path.exists(p):
                t = load_tsv(p)
                t["p2class"] = cls
                vparts.append(t)
        vcomp = pd.concat(vparts, ignore_index=True)
        Tvc = veto_frame(vbase, vcomp)
        Tvs = veto_frame(vbase, vship)
        Tvcs = veto_frame(vship, vcomp)
        summarize("VAL composed vs base", Tvc, section="val")
        summarize("VAL global-ship vs base", Tvs, section="val")
        summarize("VAL composed vs global-ship", Tvcs, section="val")
        # rules v2: 5343 + 8103 -> (size1,m32) (measured factoring cells)
        vx = os.path.join(d, "p2vx_size1_m32.tsv")
        if os.path.exists(vx):
            vx_t = load_tsv(vx)
            vx_t["p2class"] = "size1_m32"
            vcomp_v2 = pd.concat(
                [vcomp[~vcomp["image"].str.contains("5343|8103")], vx_t],
                ignore_index=True)
            Tvc2 = veto_frame(vbase, vcomp_v2)
            Tvcs2 = veto_frame(vship, vcomp_v2)
            summarize("VAL composed-v2 vs base", Tvc2, section="val")
            summarize("VAL composed-v2 vs global-ship", Tvcs2, section="val")
        print("\n  VAL per-image composed-vs-ship (adj; negative = head beats global):")
        for img, r in Tvcs.sort_values("adj").iterrows():
            cls = vcomp.loc[vcomp["image"] == img, "p2class"].iloc[0]
            print(f"    {fam_of(img):>4} {cls:11s} {img[:48]:50s} {r['adj']:+7.3f}"
                  f"{' VETO' if r['veto'] else ''}")

    # ---------- parity scoreboard vs cached aom refs ----------
    print("\n=== PARITY: vs cached aom-allintra refs (photos = t26 minus fam-7000) ===")
    store = load_store(sweep_source="speedladder-2026-07-04")
    refs = {}
    for ref in ["aom-cpu4def-ai", "aom-cpu4iq-ai", "aom-cpu6iq-ai", "aom-cpu6def-ai"]:
        r = store[store["arm_id"] == f"speedladder/{ref}"]
        refs[ref] = r
    # map store image_id -> chain image basename (same basenames)
    parity_arms = [("composed", comp), ("global-ship", ship)]
    if comp_v2 is not None:
        parity_arms.insert(1, ("composed-v2", comp_v2))
    for arm_name, arm_df in parity_arms:
        print(f"  --- {arm_name} ---")
        for ref, rdf in refs.items():
            per = {}
            for img in sorted(arm_df["image"].unique()):
                b = os.path.basename(img)
                if fam_of(b) == "7000":
                    continue
                rg = rdf[rdf["image_id"] == b]
                if rg.empty:
                    continue
                rpts = [(float(v), float(bpp)) for v, bpp in
                        zip(rg["ssim2"], rg["bpp"])
                        if np.isfinite(v) and np.isfinite(bpp) and bpp > 0]
                bd = bd_rate(frontier(pts(arm_df, img, "ssim2")), frontier(rpts))
                if bd is not None:
                    per[b] = bd
            v = pd.Series(per)
            if len(v):
                print(f"    vs {ref:15s} ssim2: n={len(v):2d} med {v.median():+7.2f} "
                      f"mean {v.mean():+7.2f} better {(v < 0).sum()}/{len(v)}")
                emit("parity", f"{arm_name}_vs_{ref}", "ssim2", len(v),
                     v.median(), v.mean(), f"{(v < 0).sum()}/{len(v)}")
            # butteraugli_3n leg
            per = {}
            for img in sorted(arm_df["image"].unique()):
                b = os.path.basename(img)
                if fam_of(b) == "7000":
                    continue
                rg = rdf[rdf["image_id"] == b]
                if rg.empty:
                    continue
                rpts = []
                for v_, bpp in zip(rg["butteraugli_3n"], rg["bpp"]):
                    v_, bpp = float(v_), float(bpp)
                    if np.isfinite(v_) and v_ > 0 and np.isfinite(bpp) and bpp > 0:
                        rpts.append((-np.log(v_), bpp))
                bd = bd_rate(frontier(pts(arm_df, img, "butteraugli_3n")),
                             frontier(rpts))
                if bd is not None:
                    per[b] = bd
            v = pd.Series(per)
            if len(v):
                print(f"    vs {ref:15s} ba3n : n={len(v):2d} med {v.median():+7.2f} "
                      f"mean {v.mean():+7.2f} better {(v < 0).sum()}/{len(v)}")
                emit("parity", f"{arm_name}_vs_{ref}", "butteraugli_3n", len(v),
                     v.median(), v.mean(), f"{(v < 0).sum()}/{len(v)}")

    # ---------- solo timing ----------
    print("\n=== SOLO timing (JOBS=1 RD_CACHE=off, q{40,65,85}; wall enc_ms sums) ===")
    def solo_sum(name):
        p = os.path.join(d, name)
        if not os.path.exists(p):
            return None
        t = load_tsv(p)
        t["enc_ms"] = pd.to_numeric(t["enc_ms"], errors="coerce")
        return t.groupby("image")["enc_ms"].sum()

    plain = solo_sum("p2t_s6_plain.tsv")
    s1ship = solo_sum("p2t_s6_size1ship.tsv")
    ctot = {}
    for cls in CLASSES:
        s = solo_sum(f"p2t_c_{cls}.tsv")
        if s is not None:
            ctot.update(s.to_dict())
    comp_s = pd.Series(ctot)
    if plain is not None:
        common = plain.index.intersection(comp_s.index)
        print(f"  plain s6 total          {plain[common].sum() / 1000:8.1f} s over {len(common)} images")
        print(f"  composed total          {comp_s[common].sum() / 1000:8.1f} s  "
              f"ratio {comp_s[common].sum() / plain[common].sum():.3f}x vs plain s6")
        if s1ship is not None:
            print(f"  global size1+ship total {s1ship[common].sum() / 1000:8.1f} s  "
                  f"ratio {s1ship[common].sum() / plain[common].sum():.3f}x vs plain s6")
        r = (comp_s[common] / plain[common]).sort_values()
        print("  per-image composed/plain ratio: "
              f"min {r.iloc[0]:.2f} med {r.median():.2f} max {r.iloc[-1]:.2f}")
    for nm in ["p2t_size1.tsv", "p2t_intra7.tsv", "p2t_intra7ship.tsv"]:
        sr = solo_sum(nm)
        if sr is not None and plain is not None:
            com = sr.index.intersection(plain.index)
            if len(com):
                r = sr[com].sum() / plain[com].sum()
                print(f"  {nm:22s} ratio vs plain (4-img): {r:.3f}x")
                emit("timing", nm.replace(".tsv", ""), "solo_ratio_vs_plain",
                     len(com), r, r, "-")
    if plain is not None and len(comp_s):
        common = plain.index.intersection(comp_s.index)
        emit("timing", "composed", "solo_ratio_vs_plain", len(common),
             comp_s[common].sum() / plain[common].sum(),
             (comp_s[common] / plain[common]).median(), "-")
        if s1ship is not None:
            emit("timing", "global_size1ship", "solo_ratio_vs_plain", len(common),
                 s1ship[common].sum() / plain[common].sum(),
                 (s1ship[common] / plain[common]).median(), "-")

    if args.tsv:
        with open(args.tsv, "w") as f:
            for ln in TSV_HEADER:
                f.write(f"# {ln}\n")
            f.write("section\tname\tmetric\tn\tmedian\tmean\tbetter\textra\n")
            for r in TSV_ROWS:
                f.write("\t".join(str(x) for x in r) + "\n")
        print(f"\nwrote {args.tsv}")


if __name__ == "__main__":
    main()

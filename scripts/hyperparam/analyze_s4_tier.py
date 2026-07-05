#!/usr/bin/env python3
"""S4TIER analysis: the last fast-tier parity column (FAST_TIER_PARITY_PLAN).

Inputs: the fetched chain_s4tier.sh OUTDIR (default
/mnt/v/output/zenavif/s4tier-20260704) + the p2heads raw dir (12q base/ship
+ v2 composed cells, byte-continuity-licensed by the cont phase) + the label
store (cached aom-allintra refs).

Produces:
 0. cont gate: per-(image,q) byte equality of the re-encoded s6+size1 base
    and i7 arms vs the p2heads run (new zenrav1e binary, knob-off identity).
 1. i5 axis: top-5 vs top-3 vs top-7 on s6 base/ship + s8 (coarse).
 2. filters probe: CDEF/LRF hi-q forced-on vs ship (coarse).
 3. composed v3 (+i7 / +i5): BD vs the p2heads 12q base + ship, parity
    scoreboard vs the cached aom refs (photos median, ssim2 + ba3n), family
    table, and the oracle-extras variant (full-tx swaps).
 4. solo timing: wall ratios vs plain-s6 (re-measured), s/MP, the cpu2iq
    budget line.
 5. val transfer (+i7 / +i5).
 6. --tsv: the benchmarks record.
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
from hp_common import load_store  # noqa: E402

METRICS = ["ssim2", "butteraugli_3n", "butteraugli_max"]
CLASSES = ["none_ship", "size1_ship", "size1_m32", "min_ship", "min_m32"]
RAW_P2 = "/mnt/v/output/zenavif/p2heads-20260704"
FULL_SWAP = {"8414": "full_ship", "6606": "full_ship", "5048": "full_ship",
             "9074": "full_m32", "9868": "full_m32"}

_FAM = {}


def load_tsv(path):
    rows = []
    with open(path) as f:
        for r in csv.DictReader(f, delimiter="\t"):
            rows.append(r)
    df = pd.DataFrame(rows)
    if "family" in df.columns:
        for img, fam in zip(df["image"], df["family"]):
            _FAM[img] = fam
            _FAM[os.path.basename(img)] = fam
    return df


def fam_of(img):
    return _FAM.get(img, _FAM.get(os.path.basename(img), "?"))


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
    return pd.Series(out, dtype=float)


def veto_frame(base_df, arm_df):
    d = {}
    for m, n in zip(METRICS, ["bd_ssim2", "bd_ba3n", "bd_bamax"]):
        d[n] = per_image_bd(base_df, arm_df, m)
    T = pd.DataFrame(d)
    vet = (T["bd_ba3n"].fillna(-np.inf) > 1.0) | (T["bd_bamax"].fillna(-np.inf) > 1.5)
    T["adj"] = np.where(vet, np.maximum(T["bd_ssim2"], 0.0), T["bd_ssim2"])
    T["veto"] = vet
    return T


TSV_ROWS = []


def emit(section, name, metric, n, med, mean, better, extra=""):
    TSV_ROWS.append((section, name, metric, n,
                     f"{med:+.4f}" if isinstance(med, float) else med,
                     f"{mean:+.4f}" if isinstance(mean, float) else mean,
                     better, extra))


def summarize(name, T, section=None):
    v = T["adj"].dropna()
    print(f"  {name:34s} n={len(v):2d} med {v.median():+7.3f} mean {v.mean():+7.3f} "
          f"better {(v < 0).sum()}/{len(v)} vetoed {int(T['veto'].sum())}")
    if section:
        emit(section, name, "ssim2_vetoadj", len(v), v.median(), v.mean(),
             f"{(v < 0).sum()}/{len(v)}", f"vetoed={int(T['veto'].sum())}")
        for col, mn in (("bd_ba3n", "butteraugli_3n"), ("bd_bamax", "butteraugli_max")):
            b = T[col].dropna()
            if len(b):
                emit(section, name, mn, len(b), b.median(), b.mean(),
                     f"{(b < 0).sum()}/{len(b)}")


def merge_classes(d, pattern, swap_full=False):
    parts = []
    for cls in CLASSES + (["full_ship", "full_m32"] if swap_full else []):
        p = os.path.join(d, pattern.format(cls=cls))
        if os.path.exists(p):
            t = load_tsv(p)
            t["p2class"] = cls
            parts.append(t)
    if not parts:
        return None
    comp = pd.concat(parts, ignore_index=True)
    if swap_full:
        # oracle variant: the 5 full-extra images ride their full_* cells
        keep = []
        for _, r in comp.iterrows():
            pre = os.path.basename(r["image"]).split("_")[0]
            if pre in FULL_SWAP:
                keep.append(r["p2class"] == FULL_SWAP[pre])
            else:
                keep.append(not r["p2class"].startswith("full"))
        comp = comp[pd.Series(keep, index=comp.index)]
    else:
        comp = comp[~comp["p2class"].str.startswith("full")]
    return comp


def solo_sum(d, name):
    p = os.path.join(d, name)
    if not os.path.exists(p):
        return None
    t = load_tsv(p)
    t["enc_ms"] = pd.to_numeric(t["enc_ms"], errors="coerce")
    s = t.groupby("image")["enc_ms"].sum()
    s.index = [os.path.basename(i) for i in s.index]
    return s


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("outdir", nargs="?", default="/mnt/v/output/zenavif/s4tier-20260704")
    ap.add_argument("--tsv", default=None)
    args = ap.parse_args()
    d = args.outdir

    # ---------- 0. continuity gate ----------
    print("=== 0. BYTE-CONTINUITY GATE (new zenrav1e chain, knob-off identity) ===")
    for new, old, tag in [("s4_cont_base.tsv", "p2_s6_base.tsv", "s6+size1 base"),
                          ("s4_cont_intra7.tsv", "p2_s6_intra7.tsv", "s6+size1+i7")]:
        pn, po = os.path.join(d, new), os.path.join(RAW_P2, old)
        if not (os.path.exists(pn) and os.path.exists(po)):
            print(f"  MISSING {new} / {old}")
            continue
        a = load_tsv(pn).set_index(["image", "q"])["bytes"]
        b = load_tsv(po).set_index(["image", "q"])["bytes"]
        j = pd.concat({"a": a, "b": b}, axis=1).dropna()
        same = int((j["a"] == j["b"]).sum())
        print(f"  {tag:20s}: {same}/{len(j)} cells byte-identical")
        emit("cont", tag, "bytes_identical", len(j), float(same), float(same),
             f"{same}/{len(j)}")
        if same != len(j):
            bad = j[j["a"] != j["b"]]
            print(f"    MISMATCHES:\n{bad.head(10).to_string()}")

    # ---------- 1. i5 axis ----------
    print("\n=== 1. THE TOP-5 KNOB (i5) vs i3 / i7 (coarse) ===")
    base = load_tsv(os.path.join(d, "s4_cont_base.tsv"))
    i7 = load_tsv(os.path.join(d, "s4_cont_intra7.tsv"))
    axes = {}
    for tag, b, arm_p in [
        ("s6 i5 vs i3-base", base, "s4_s6_i5.tsv"),
        ("s6 i7 vs i3-base", base, None),  # from cont cells
        ("s8 i5 vs i3-base", None, "s4_s8_i5.tsv"),
    ]:
        if arm_p is None:
            T = veto_frame(base, i7)
        else:
            p = os.path.join(d, arm_p)
            if not os.path.exists(p):
                continue
            if tag.startswith("s8"):
                b8 = os.path.join(RAW_P2, "p2_s8_base.tsv")
                if not os.path.exists(b8):
                    continue
                T = veto_frame(load_tsv(b8), load_tsv(p))
            else:
                T = veto_frame(b, load_tsv(p))
        axes[tag] = T
        summarize(tag, T, section="i5axis")
    p8 = os.path.join(RAW_P2, "p2_s8_intra7.tsv")
    if os.path.exists(p8):
        summarize("s8 i7 vs i3-base (p2heads)", veto_frame(
            load_tsv(os.path.join(RAW_P2, "p2_s8_base.tsv")), load_tsv(p8)),
            section="i5axis")
    ship_new = os.path.join(d, "s4_s6_i5ship.tsv")
    if os.path.exists(ship_new):
        ship_old = load_tsv(os.path.join(RAW_P2, "p2_s6_ship.tsv"))
        summarize("s6 i5+ship vs ship", veto_frame(ship_old, load_tsv(ship_new)),
                  section="i5axis")
        summarize("s6 i7+ship vs ship (p2heads)", veto_frame(
            ship_old, load_tsv(os.path.join(RAW_P2, "p2_s6_intra7ship.tsv"))),
            section="i5axis")

    # ---------- 2. filters probe ----------
    print("\n=== 2. HI-Q FILTER PROBE (CDEF / LRF forced on, vs size1+ship) ===")
    ship12 = load_tsv(os.path.join(RAW_P2, "p2_s6_ship.tsv"))
    for name, tag in [("s4_s6_cdef.tsv", "s6 CDEF-on vs ship"),
                      ("s4_s6_lrf.tsv", "s6 LRF-on vs ship")]:
        p = os.path.join(d, name)
        if not os.path.exists(p):
            continue
        T = veto_frame(ship12, load_tsv(p))
        summarize(tag, T, section="filters")
        Tf = T.copy()
        Tf["family"] = [fam_of(i) for i in Tf.index]
        best = Tf.sort_values("adj").head(5)
        for img, r in best.iterrows():
            print(f"      {r['family']:>4} {os.path.basename(img)[:52]:54s} {r['adj']:+7.2f}")

    # ---------- 3. composed v3 ----------
    print("\n=== 3. COMPOSED v3 (12q) — +i7 / +i5 / oracle-extras ===")
    base12 = load_tsv(os.path.join(RAW_P2, "p2_conf_s6_base.tsv"))
    ship12q = load_tsv(os.path.join(RAW_P2, "p2_conf_s6_ship.tsv"))
    arms = {}
    for tag, pattern, swap in [("v3+i7", "s4c_{cls}_i7.tsv", False),
                               ("v3+i5", "s4c_{cls}_i5.tsv", False),
                               ("v3+i7+fullx", "s4c_{cls}_i7.tsv", True)]:
        comp = merge_classes(d, pattern, swap_full=False)
        if swap:
            # merge in the oraclex cells
            parts = [comp] if comp is not None else []
            for cls in ["full_ship", "full_m32"]:
                p = os.path.join(d, f"s4x_{cls}_i7.tsv")
                if os.path.exists(p):
                    t = load_tsv(p)
                    t["p2class"] = cls
                    parts.append(t)
            if len(parts) < 2:
                continue
            comp = pd.concat(parts, ignore_index=True)
            keep = []
            for _, r in comp.iterrows():
                pre = os.path.basename(r["image"]).split("_")[0]
                if pre in FULL_SWAP:
                    keep.append(r["p2class"] == FULL_SWAP[pre])
                else:
                    keep.append(not r["p2class"].startswith("full"))
            comp = comp[pd.Series(keep, index=comp.index)]
        if comp is None or not len(comp):
            continue
        arms[tag] = comp
        print(f"  {tag}: {len(comp)} rows over {comp['image'].nunique()} images")
        summarize(f"{tag} vs s6+size1 base", veto_frame(base12, comp), section="composed")
        summarize(f"{tag} vs global-ship", veto_frame(ship12q, comp), section="composed")

    # v2 composed+i7 continuity row (p2heads cells vs same base)
    # ---------- parity scoreboard ----------
    print("\n=== PARITY vs cached aom-allintra refs (photos = non-7000) ===")
    store = load_store(sweep_source="speedladder-2026-07-04")
    store = store[store["corpus"] == "train26"]
    refs = {}
    for ref in ["aom-cpu2iq-ai", "aom-cpu2def-ai", "aom-cpu4iq-ai", "aom-cpu4def-ai"]:
        refs[ref] = store[store["arm_id"] == f"speedladder/{ref}"]

    def ref_pts(rg, metric):
        out = []
        for v, bpp in zip(pd.to_numeric(rg[metric], errors="coerce"),
                          pd.to_numeric(rg["bpp"], errors="coerce")):
            if not np.isfinite(v) or not np.isfinite(bpp) or bpp <= 0:
                continue
            if metric.startswith("butteraugli"):
                if v <= 0:
                    continue
                v = -np.log(v)
            out.append((float(v), float(bpp)))
        return out

    for arm_name, arm_df in arms.items():
        print(f"  --- {arm_name} ---")
        for ref, rdf in refs.items():
            for metric in ["ssim2", "butteraugli_3n"]:
                per = {}
                for img in sorted(arm_df["image"].unique()):
                    b = os.path.basename(img)
                    if fam_of(b) == "7000":
                        continue
                    rg = rdf[rdf["image_id"] == b]
                    if rg.empty:
                        continue
                    bd = bd_rate(frontier(pts(arm_df, img, metric)),
                                 frontier(ref_pts(rg, metric)))
                    if bd is not None:
                        per[b] = bd
                v = pd.Series(per)
                if len(v):
                    print(f"    vs {ref:15s} {metric:14s}: n={len(v):2d} med {v.median():+7.2f} "
                          f"mean {v.mean():+7.2f} better {(v < 0).sum()}/{len(v)}")
                    emit("parity", f"{arm_name}_vs_{ref}", metric, len(v),
                         v.median(), v.mean(), f"{(v < 0).sum()}/{len(v)}")
        if arm_name == "v3+i7":
            per = {}
            for img in sorted(arm_df["image"].unique()):
                b = os.path.basename(img)
                if fam_of(b) == "7000":
                    continue
                rg = refs["aom-cpu2iq-ai"][refs["aom-cpu2iq-ai"]["image_id"] == b]
                if rg.empty:
                    continue
                bd = bd_rate(frontier(pts(arm_df, img, "ssim2")),
                             frontier(ref_pts(rg, "ssim2")))
                if bd is not None:
                    per[b] = bd
            v = pd.Series(per).sort_values(ascending=False)
            print("    per-image vs cpu2iq-ai (ssim2, worst->best):")
            for b, bd in v.items():
                print(f"      {fam_of(b):>4} {b[:56]:58s} {bd:+7.2f}")

    # ---------- 4. timing ----------
    print("\n=== 4. SOLO TIMING (JOBS=1 RD_CACHE=off q{40,65,85}) ===")
    plain = solo_sum(d, "s4t_plain.tsv")
    if plain is not None:
        tot = {}
        for tag, pattern in [("v3+i7", "s4t_{cls}_i7.tsv"), ("v3+i5", "s4t_{cls}_i5.tsv")]:
            w = {}
            for cls in CLASSES:
                s = solo_sum(d, pattern.format(cls=cls))
                if s is not None:
                    w.update(s.to_dict())
            w = pd.Series(w)
            com = plain.index.intersection(w.index)
            if len(com):
                r = w[com].sum() / plain[com].sum()
                tot[tag] = (r, len(com))
                print(f"  {tag:12s}: {r:.3f}x plain-s6 over {len(com)} images "
                      f"(~{r * 1.026:.2f} s/MP at the ladder plain median 1026 ms/MP)")
                emit("timing", tag, "solo_ratio_vs_plain", len(com), r, r, "-")
        # oracle-extras variant: swap the 5 full images' walls
        w7 = {}
        for cls in CLASSES:
            s = solo_sum(d, f"s4t_{cls}_i7.tsv")
            if s is not None:
                w7.update(s.to_dict())
        for cls in ["full_ship", "full_m32"]:
            s = solo_sum(d, f"s4t_{cls}_i7.tsv")
            if s is not None:
                w7.update(s.to_dict())
        w7 = pd.Series(w7)
        com = plain.index.intersection(w7.index)
        if len(com):
            r = w7[com].sum() / plain[com].sum()
            print(f"  v3+i7+fullx : {r:.3f}x plain-s6 ({len(com)} images)")
            emit("timing", "v3+i7+fullx", "solo_ratio_vs_plain", len(com), r, r, "-")
        cd = solo_sum(d, "s4t_cdef4.tsv")
        sh = solo_sum(d, "s4t_ship4.tsv")
        if cd is not None and sh is not None:
            com = cd.index.intersection(sh.index)
            print(f"  CDEF marginal on ship (4-img): {cd[com].sum() / sh[com].sum():.3f}x")
            emit("timing", "cdef_marginal_on_ship", "solo_ratio", len(com),
                 cd[com].sum() / sh[com].sum(), cd[com].sum() / sh[com].sum(), "-")
        print(f"  budget line: aom cpu2iq-ai 6639 ms/MP = 6.47x ladder plain-s6 "
              f"(1026); cpu2def-ai 4707 = 4.59x")

    # ---------- 5. val ----------
    print("\n=== 5. VAL transfer (12q) ===")
    vb = os.path.join(d, "s4v_base.tsv")
    if os.path.exists(vb):
        vbase = load_tsv(vb)
        for tag, pattern in [("val v3+i7", "s4v_{cls}_i7.tsv"),
                             ("val v3+i5", "s4v_{cls}_i5.tsv")]:
            comp = merge_classes(d, pattern)
            if comp is None:
                continue
            summarize(f"{tag} vs val-base(size1)", veto_frame(vbase, comp), section="val")
        # v2 composed reference on val (p2heads cells)
        vparts = []
        for cls in ["none_ship", "none_m32", "size1_ship", "size1_m32", "min_ship", "min_m32"]:
            p = os.path.join(RAW_P2, f"p2vi7_{cls}.tsv")
            if os.path.exists(p):
                vparts.append(load_tsv(p))
        if vparts:
            v2 = pd.concat(vparts, ignore_index=True)
            vx7 = os.path.join(RAW_P2, "p2rx_valx2_size1_m32_i7.tsv")
            if os.path.exists(vx7):
                t = load_tsv(vx7)
                v2 = pd.concat([v2[~v2["image"].str.contains("5343|8103")], t],
                               ignore_index=True)
            summarize("val v2+i7 vs val-base (p2heads)", veto_frame(vbase, v2),
                      section="val")

    if args.tsv:
        hdr = [
            "S4TIER (FAST_TIER_PARITY_PLAN, the last open fast-tier column) -- 2026-07-04 -- "
            "s4-tier composed operating point: v3 rules (tx D bound 23.69) + intra top-5/top-7 axis "
            "+ hi-q CDEF/LRF probe + full-tx oracle extras",
            "Box zenavif-sweep-1 (ccx63 48c, FROM_SNAPSHOT=auto); harness scripts/rd_gap/chain_s4tier.sh; "
            "analyzer scripts/hyperparam/analyze_s4_tier.py; design scripts/hyperparam/fit_s4_tier.py; "
            "samples scripts/hyperparam/emit_s4tier_samples.py",
            "Code: zenrav1e master 0d392334 (071e9844 num_modes_rdo_override knob + fmt) via ravif--s4tier "
            "devpatch (p2heads passthroughs + ZENRAVIF_INTRA_MODES=5 + ZENRAVIF_CDEF/LRF)",
            "v3 rules: tx {pf>0.8505 && dcty>100 -> none | pf<=0.8505 && dcty<23.69 -> min | size1}; "
            "partition {gfs<0.4105 -> m32 | ship}; intra arm global (i7 vs the new i5 knob); "
            "full-tx = oracle extras only (no honest gate at n=24)",
            "All arms tune-ss2 + palette auto, --threads 1, BUTTER on, PALCONF=1; BD per-image "
            "monotone-frontier hull (bd_arm.py); vetoadj = max(bd,0) when arm ba3n>+1.0 or bamax>+1.5; "
            "parity rows vs CACHED speedladder aom-allintra refs (photos = t26 minus fam-7000)",
        ]
        with open(args.tsv, "w") as f:
            for ln in hdr:
                f.write(f"# {ln}\n")
            f.write("section\tname\tmetric\tn\tmedian\tmean\tbetter\textra\n")
            for r in TSV_ROWS:
                f.write("\t".join(str(x) for x in r) + "\n")
        print(f"\nwrote {args.tsv}")


if __name__ == "__main__":
    main()

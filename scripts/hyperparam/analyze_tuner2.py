#!/usr/bin/env python3
"""TUNER2 chain analysis (P3 residuals: iq-AQ boost head + 6096 dead-zone).

Inputs: the chain_tuner2.sh output TSVs (fetched under
scripts/rd_gap/remote/results/<run>/), the label store (cached base/ref
curves), and the run_gap TSV schema (image/w/h/family/encoder/fmt/q/bytes/
bpp/ssim2/enc_ms/butteraugli_3n/butteraugli_max).

Sections:
  cont    byte-continuity: t2_cont8 rows vs the store's
          speedladder/zr-s2-tune rows (bytes must be identical per cell —
          proves the store rows are same-binary base curves).
  valstr  per-image BD tables of strength {1,2,3,4.5} vs strength-0 on the
          val origins, both metrics + veto flags (the head's VAL labels).
  deep    per-image BD of each deep arm vs the store base (coarse grid,
          store rows restricted to the same 6 qs), t26; firing-class and
          photo slices separated.
  dz      same for the QROUND arms, plus the 6096-class focus band and a
          bytes-at-matched-q inflation table (the no-skip mechanism is
          visible as bytes ratio at fixed q).

BD conventions from bd_arm.py via hp_common. Emits one combined TSV under
benchmarks/ when --emit is passed.
"""

import argparse
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import (LOWER_BETTER, bd_rate, frontier, load_store,  # noqa: E402
                       print_dist, to_tsv)

QCOARSE = [30, 50, 60, 75, 85, 95]
FIRING = ("6018", "6096", "1236", "9100", "9118", "6606", "5048",
          "6091", "6621", "9165")  # deep-AQ diagnosis class + val members
VETO_3N, VETO_MAX = 1.0, 1.5


def read_gap_tsv(path):
    df = pd.read_csv(path, sep="\t")
    df["oid"] = df["image"].str.split("_").str[0]
    return df


def pts(g, metric):
    out = []
    for v, bpp in zip(g[metric].to_numpy(), g["bpp"].to_numpy()):
        if not np.isfinite(v) or not np.isfinite(bpp) or bpp <= 0:
            continue
        if metric in LOWER_BETTER or metric.startswith("butteraugli"):
            if v <= 0:
                continue
            v = -np.log(v)
        out.append((float(v), float(bpp)))
    return out


def bd_tables(test_df, base_df, metrics=("ssim2", "butteraugli_3n", "butteraugli_max")):
    rows = []
    for img in sorted(set(test_df["image"]) & set(base_df["image"])):
        gt = test_df[test_df["image"] == img]
        gb = base_df[base_df["image"] == img]
        r = {"image": img, "oid": img.split("_")[0]}
        for m in metrics:
            key = {"ssim2": "bd_ssim2", "butteraugli_3n": "bd_ba3n",
                   "butteraugli_max": "bd_bamax"}[m]
            try:
                r[key] = bd_rate(frontier(pts(gt, m)), frontier(pts(gb, m)))
            except Exception:
                r[key] = np.nan
        rows.append(r)
    return pd.DataFrame(rows).set_index("image")


def summarize(name, t, firing_only=False):
    if t is None or t.empty:
        print(f"  {name}: NO DATA")
        return
    sel = t[t["oid"].isin(FIRING)] if firing_only else t
    v = sel["bd_ssim2"].dropna()
    veto = ((sel["bd_ba3n"] > VETO_3N) | (sel["bd_bamax"] > VETO_MAX)).sum()
    lbl = f"{name}{' [firing]' if firing_only else ''}"
    print(f"  {lbl:<38} n={len(v)} med {v.median():+.3f} mean {v.mean():+.3f} "
          f"win {(v < 0).sum()}/{len(v)} | ba3n_med {sel['bd_ba3n'].median():+.3f} "
          f"bamax_med {sel['bd_bamax'].median():+.3f} | per-img vetoes {int(veto)}")


def store_base_t26(qs=None):
    """speedladder/zr-s2-tune rows as a run_gap-shaped base frame."""
    st = load_store(sweep_source="speedladder-2026-07-04", corpus="train26",
                    arm_id="speedladder/zr-s2-tune")
    df = pd.DataFrame({
        "image": st["image_id"], "q": st["q"], "bytes": st["bytes"],
        "bpp": st["bpp"], "ssim2": st["ssim2"],
        "butteraugli_3n": st["butteraugli_3n"],
        "butteraugli_max": st["butteraugli_max"],
    })
    if qs:
        df = df[df["q"].isin(qs)]
    df["oid"] = df["image"].str.split("_").str[0]
    return df


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("rundir", help="fetched chain output dir (t2_*.tsv)")
    ap.add_argument("--emit", help="combined per-image TSV out path")
    args = ap.parse_args()
    rd = args.rundir

    def load(name):
        p = os.path.join(rd, name)
        return read_gap_tsv(p) if os.path.exists(p) else None

    # ---- cont ----
    cont = load("t2_cont8.tsv")
    if cont is not None:
        base = store_base_t26()
        m = cont.merge(base, on=["image", "q"], suffixes=("_new", "_ref"))
        bad = m[m["bytes_new"] != m["bytes_ref"]]
        print(f"=== cont: {len(m)} joined cells, byte-mismatches: {len(bad)} ===")
        if len(bad):
            print(bad[["image", "q", "bytes_new", "bytes_ref"]].head(20).to_string())
            print("  CONTINUITY FAILED — store rows are NOT valid base curves; "
                  "deep/dz BDs below are cross-binary (interpret with care)")
        else:
            print("  byte-continuity PASS — store zr-s2-tune rows are "
                  "same-binary base curves")

    # ---- valstr ----
    val = {s: load(f"t2_valstr_{s}.tsv") for s in ("0.0", "1.0", "2.0", "3.0", "4.5")}
    if val["0.0"] is not None:
        print("\n=== valstr: strength BD vs strength-0 (14 val origins, 12q) ===")
        tables = {}
        for s in ("1.0", "2.0", "3.0", "4.5"):
            if val[s] is None:
                continue
            t = bd_tables(val[s], val["0.0"])
            tables[s] = t
            summarize(f"str{s} vs str0", t)
            summarize(f"str{s} vs str0", t, firing_only=True)
        if "1.0" in tables:
            print("\n  per-image (vs str0): the val deep-AQ probes")
            for oid in ("6091", "9165", "6621", "8103", "5343"):
                row = []
                for s, t in tables.items():
                    hit = t[t["oid"] == oid]
                    if not hit.empty:
                        r = hit.iloc[0]
                        veto = "V" if (r["bd_ba3n"] > VETO_3N or r["bd_bamax"] > VETO_MAX) else " "
                        row.append(f"s{s}: {r['bd_ssim2']:+6.2f}{veto}")
                if row:
                    print(f"    {oid}: " + "  ".join(row))
        if args.emit and tables:
            out = None
            for s, t in tables.items():
                t2 = t.rename(columns={c: f"s{s}_{c}" for c in t.columns if c != "oid"})
                out = t2 if out is None else out.join(
                    t2[[c for c in t2.columns if c != "oid"]], how="outer")
            to_tsv(out, args.emit, [
                "TUNER2 valstr: per-image strength BD vs strength-0 (val origins, s2+tune, 12q)",
                "same-binary arms (ravif--tuner2 devpatch chain); veto thresholds ba3n>+1.0 bamax>+1.5",
            ])

    # ---- deep ----
    base6 = store_base_t26(QCOARSE)
    for arm, fname in (("deep 3.0:4", "t2_deep_3.0_4.tsv"),
                       ("deep 4.5:4", "t2_deep_4.5_4.tsv")):
        t = load(fname)
        if t is None:
            continue
        tb = bd_tables(t, base6)
        print(f"\n=== {arm} vs store base (t26 coarse) ===")
        summarize(arm, tb)
        summarize(arm, tb, firing_only=True)
        photos = tb[~tb["oid"].isin(FIRING)]
        summarize(arm + " [photos/rest]", photos)
        worst = tb["bd_ssim2"].astype(float).nlargest(3)
        print("    worst-3 ssim2:", "; ".join(f"{tb.loc[i,'oid']} {v:+.2f}" for i, v in worst.items()))

    # ---- dz ----
    for arm, fname in (("QROUND=118", "t2_dz_118.tsv"), ("QROUND=128", "t2_dz_128.tsv"),
                       ("QROUND=118 full", "t2_dzfull_118.tsv"), ("QROUND=128 full", "t2_dzfull_128.tsv")):
        t = load(fname)
        if t is None:
            continue
        qs = sorted(t["q"].unique())
        baseq = store_base_t26(qs)
        tb = bd_tables(t, baseq)
        print(f"\n=== {arm} vs store base (t26) ===")
        summarize(arm, tb)
        summarize(arm, tb, firing_only=True)
        photos = tb[~tb["oid"].isin(FIRING)]
        summarize(arm + " [photos/rest]", photos)
        # 6096-class focus: bytes inflation at matched q + top-band detail
        for oid in ("6096", "6018"):
            g = t[t["oid"] == oid].sort_values("q")
            gb = baseq[baseq["oid"] == oid].sort_values("q")
            if g.empty or gb.empty:
                continue
            j = g.merge(gb, on="q", suffixes=("_arm", "_base"))
            ratio = (j["bytes_arm"] / j["bytes_base"]).round(3).tolist()
            s2d = (j["ssim2_arm"] - j["ssim2_base"]).round(2).tolist()
            print(f"    {oid} bytes ratio by q {list(j['q'])}: {ratio}")
            print(f"    {oid} ssim2 delta at matched q: {s2d}")
        w = tb["bd_ssim2"].astype(float).nlargest(3)
        print("    worst-3 ssim2:", "; ".join(f"{tb.loc[i,'oid']} {v:+.2f}" for i, v in w.items()))

    print("\n(veto convention: per-image ba3n>+1.0 or bamax>+1.5 flags; "
          "policy decisions use the standing bank-0 rule)")


if __name__ == "__main__":
    main()

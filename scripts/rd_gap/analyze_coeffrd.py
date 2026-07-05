#!/usr/bin/env python3
"""COEFF_RD_STACK coarse/full analysis per DECISION_RULE_COEFFRD.md:
per-family medians FIRST, cluster-mass-weighted aggregates, butteraugli
veto columns, photos-merit path. Reuses bd_arm.py's loaders.

Usage:
  analyze_coeffrd.py BASE.tsv ARM.tsv [ARM2.tsv ...]
"""
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bd_arm import bd_rate, frontier, load  # noqa: E402

# DECISION_RULE_COEFFRD.md families + cluster masses (train26 manifest).
FAMILIES = {
    "photos": {"1236": 56, "1438": 43, "1614": 100, "2000": 21, "5004": 14, "5048": 45},
    "products": {"9958": 117, "9868": 109, "9678": 84, "9228": 50, "9074": 50},
    "screens": {"8268": 115, "8196": 78, "8302": 50, "8414": 8},
    "illus9094": {"9100": 38, "9118": 17},
    "scans": {"6096": 32, "6018": 12, "6606": 2},
    "plots": {"7028": 25, "7058": 14, "7052": 1, "7050": 1},
}
ORIGIN_MASS = {o: m for fam in FAMILIES.values() for o, m in fam.items()}
ORIGIN_FAM = {o: f for f, fam in FAMILIES.items() for o in fam}


def origin_of(img: str) -> str:
    return os.path.basename(img).split("_")[0]


def wmedian(vals, weights):
    order = np.argsort(vals)
    v, w = np.asarray(vals)[order], np.asarray(weights, dtype=float)[order]
    c = np.cumsum(w)
    return float(v[np.searchsorted(c, 0.5 * c[-1])])


def per_image_bd(base_path, arm_path, metric):
    base, base_ms = load(base_path, metric)
    arm, arm_ms = load(arm_path, metric)
    out = {}
    for img in sorted(base):
        if img not in arm:
            continue
        bd = bd_rate(frontier(arm[img]), frontier(base[img]))
        if bd is not None:
            out[img] = bd
    # solo-sum time ratio over common images
    common = [i for i in out if base_ms.get(i) and arm_ms.get(i)]
    tr = None
    if common:
        b = sum(sum(base_ms[i]) for i in common)
        a = sum(sum(arm_ms[i]) for i in common)
        tr = a / b if b > 0 else None
    return out, tr


def main() -> int:
    base = sys.argv[1]
    arms = sys.argv[2:]
    for arm_path in arms:
        name = os.path.basename(arm_path).replace(".tsv", "")
        bds = {m: per_image_bd(base, arm_path, m) for m in
               ("ssim2", "butteraugli_3n", "butteraugli_max")}
        ss2, tr = bds["ssim2"]
        ba3, _ = bds["butteraugli_3n"]
        bam, _ = bds["butteraugli_max"]
        if not ss2:
            print(f"{name}: NO DATA")
            continue

        print(f"\n=== {name}  (n={len(ss2)}, time x{tr:.2f})" if tr else f"\n=== {name} (n={len(ss2)})")
        # Per-family medians FIRST.
        veto_fail = []
        for fam, members in FAMILIES.items():
            imgs = [i for i in ss2 if origin_of(i) in members]
            if not imgs:
                continue
            fs = np.median([ss2[i] for i in imgs])
            f3 = np.median([ba3[i] for i in imgs if i in ba3]) if any(i in ba3 for i in imgs) else float("nan")
            fm = np.median([bam[i] for i in imgs if i in bam]) if any(i in bam for i in imgs) else float("nan")
            flag = " VETO" if (f3 > 0.50 or fm > 0.50) else ""
            if flag:
                veto_fail.append(fam)
            print(f"  {fam:10s} n={len(imgs)}  ssim2 {fs:+7.2f}  ba3n {f3:+7.2f}  bamax {fm:+7.2f}{flag}")
        # Aggregates: mass-weighted median + plain.
        imgs = list(ss2)
        w = [ORIGIN_MASS.get(origin_of(i), 1) for i in imgs]
        s = [ss2[i] for i in imgs]
        ws = wmedian(s, w)
        w3 = wmedian([ba3.get(i, 0.0) for i in imgs], w)
        wm = wmedian([bam.get(i, 0.0) for i in imgs], w)
        better = sum(1 for i in imgs
                     if ss2[i] < 0 and not (ba3.get(i, 0) > 1.0))  # fire-conservative veto
        print(f"  {'MASS-WMED':10s} ssim2 {ws:+7.2f}  ba3n {w3:+7.2f}  bamax {wm:+7.2f}"
              f"   plain med {np.median(s):+6.2f} mean {np.mean(s):+6.2f}"
              f"   better(vetoed) {better}/{len(imgs)}")
        # Rule flags.
        bar = ws <= -0.30 and w3 <= 0.30 and wm <= 0.30 and not veto_fail
        pm = FAMILIES["photos"]
        pimgs = [i for i in ss2 if origin_of(i) in pm]
        pmed = np.median([ss2[i] for i in pimgs]) if pimgs else float("nan")
        p3 = np.median([ba3[i] for i in pimgs if i in ba3]) if pimgs else float("nan")
        others_ok = all(
            np.median([ss2[i] for i in ss2 if origin_of(i) in mem]) <= 0.30
            for f2, mem in FAMILIES.items() if f2 != "photos"
            and any(origin_of(i) in mem for i in ss2)
        )
        photos_merit = pmed <= -0.30 and p3 <= 0.30 and others_ok
        print(f"  RULE: mass-bar={'PASS' if bar else 'fail'}  photos-merit={'PASS' if photos_merit else 'fail'}")
        # Worst/best three images for the record.
        top = sorted(ss2.items(), key=lambda kv: kv[1])
        for tag, rows in (("best", top[:3]), ("worst", top[-3:])):
            for i, v in rows:
                print(f"    {tag:5s} {v:+8.2f}  ba3n {ba3.get(i, float('nan')):+6.2f}  {os.path.basename(i)[:60]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

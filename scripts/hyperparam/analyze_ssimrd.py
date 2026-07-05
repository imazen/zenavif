#!/usr/bin/env python3
"""SSIMRD program analysis: per-family FIRST + cluster-mass-weighted
aggregates (the 2026-07-05 evaluation-policy correction: train26 is a
DIVERSE k-means subset — one pick per cluster regardless of mass — so
unweighted all-24 medians over-weight rare classes and dilute
photo-dominant effects; photos-only merit is a keepable verdict).

Reuses bd_arm.py's frontier/BD-rate math verbatim (import) so numbers are
directly comparable with every prior program's verdicts.

Usage:
  analyze_ssimrd.py BASE.tsv ARM.tsv [ARM2.tsv ...]
      per-arm: per-family medians (ssim2 + butteraugli 3n/max), the
      mass-weighted aggregate, per-image detail, fire-conservative vetoes.
  analyze_ssimrd.py --vs-aom AOMREF.tsv BASE.tsv ARM.tsv --images 1236,6018,...
      class movement: BD(zr vs aomref) per target image, base vs arm.
"""
import argparse
import json
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "rd_gap"))
import bd_arm  # noqa: E402  (frontier, bd_rate, load)

MANIFEST = "/mnt/v/output/rd-gap-train26-2026-07-02/_MANIFEST.json"

# Family assignment by origin-id prefix (DECISION_RULE_SSIMRD.md).
FAMILY_OF_PREFIX = [
    (("1", "2", "5004", "5048", "50"), "photos"),
    (("5343", "53"), "charts"),
    (("6",), "scans"),
    (("7",), "plots"),
    (("8",), "screenshots"),
    (("90", "96", "98", "99", "92"), "products-gen"),
    (("91",), "illustrations"),
]


def origin_of(image_name):
    return image_name.split("_", 1)[0]


def family_of(image_name):
    o = origin_of(image_name)
    # longest-prefix match
    best = ("", "other")
    for prefixes, fam in FAMILY_OF_PREFIX:
        for p in prefixes:
            if o.startswith(p) and len(p) > len(best[0]):
                best = (p, fam)
    return best[1]


def cluster_weights():
    try:
        m = json.load(open(MANIFEST))
        return {p["origin_id"]: float(p["cluster_size"]) for p in m["picks"]}
    except Exception:
        return {}


def weighted_median(vals, weights):
    order = np.argsort(vals)
    v = np.asarray(vals)[order]
    w = np.asarray(weights)[order]
    c = np.cumsum(w)
    return float(v[np.searchsorted(c, 0.5 * c[-1])])


def per_image_bd(base_tsv, arm_tsv, metric):
    base, _ = bd_arm.load(base_tsv, metric)
    arm, _ = bd_arm.load(arm_tsv, metric)
    out = {}
    for img in sorted(base):
        if img not in arm:
            continue
        bd = bd_arm.bd_rate(bd_arm.frontier(arm[img]), bd_arm.frontier(base[img]))
        if bd is not None:
            out[img] = bd
    return out


def analyze_arm(base_tsv, arm_tsv, wts, verbose=False):
    ss2 = per_image_bd(base_tsv, arm_tsv, "ssim2")
    b3 = per_image_bd(base_tsv, arm_tsv, "butteraugli_3n")
    bmax = per_image_bd(base_tsv, arm_tsv, "butteraugli_max")
    fams = {}
    rows = []
    for img, bd in ss2.items():
        fam = family_of(img)
        o = origin_of(img)
        w = wts.get(o, 1.0)
        rows.append((img, fam, o, w, bd, b3.get(img), bmax.get(img)))
        fams.setdefault(fam, []).append((w, bd, b3.get(img), bmax.get(img)))

    print(f"\n=== {os.path.basename(arm_tsv)} vs {os.path.basename(base_tsv)} ===")
    print(f"{'family':<14} {'n':>2} {'ss2 med':>8} {'ss2 wmed':>9} "
          f"{'ba3n med':>9} {'bamax med':>10} {'better':>6}")
    allw, allv, allb3, allbm = [], [], [], []
    for fam in sorted(fams):
        entries = fams[fam]
        w = [e[0] for e in entries]
        v = [e[1] for e in entries]
        e3 = [e[2] for e in entries if e[2] is not None]
        em = [e[3] for e in entries if e[3] is not None]
        med = float(np.median(v))
        wmed = weighted_median(v, w)
        med3 = float(np.median(e3)) if e3 else float("nan")
        medm = float(np.median(em)) if em else float("nan")
        better = sum(1 for x in v if x < 0)
        print(f"{fam:<14} {len(v):>2} {med:>+8.2f} {wmed:>+9.2f} "
              f"{med3:>+9.2f} {medm:>+10.2f} {better:>3}/{len(v)}")
        allw += w; allv += v; allb3 += e3; allbm += em
    wmed_all = weighted_median(allv, allw)
    wmed_b3 = weighted_median(allb3, [1.0] * len(allb3)) if allb3 else float("nan")
    # mass-weighted butteraugli: rebuild with weights aligned
    b3w = [(wts.get(origin_of(i), 1.0), b3[i]) for i in b3]
    bmw = [(wts.get(origin_of(i), 1.0), bmax[i]) for i in bmax]
    wmed_b3 = weighted_median([x[1] for x in b3w], [x[0] for x in b3w]) if b3w else float("nan")
    wmed_bm = weighted_median([x[1] for x in bmw], [x[0] for x in bmw]) if bmw else float("nan")
    vetoes = [i for i in ss2 if ss2[i] < 0 and b3.get(i) is not None and b3[i] > 1.0]
    print(f"{'MASS-WEIGHTED':<14} {len(allv):>2} {'':>8} {wmed_all:>+9.2f} "
          f"{wmed_b3:>+9.2f} {wmed_bm:>+10.2f}   vetoes={len(vetoes)}")
    if vetoes:
        print(f"  vetoed (ss2<0 but ba3n>+1.0): {[origin_of(i) for i in vetoes]}")
    if verbose:
        for img, fam, o, w, bd, v3, vm in sorted(rows, key=lambda r: r[4]):
            s3 = f"{v3:+.2f}" if v3 is not None else "NA"
            sm = f"{vm:+.2f}" if vm is not None else "NA"
            print(f"  {bd:+8.2f}  ba3n {s3:>8}  bamax {sm:>8}  w={w:<5.0f} {fam:<13} {img[:60]}")
    return {"wmed_ss2": wmed_all, "wmed_b3": wmed_b3, "wmed_bmax": wmed_bm}


def vs_aom(aom_tsv, zr_tsvs, images, wts):
    print(f"\n=== class movement vs {os.path.basename(aom_tsv)} (BD zr-vs-aom, + = zr worse) ===")
    hdr = f"{'origin':<8}" + "".join(f" {os.path.basename(t)[:24]:>26}" for t in zr_tsvs)
    print(hdr)
    aom, _ = bd_arm.load(aom_tsv, "ssim2", encoder="libaom")
    per_tsv = []
    for t in zr_tsvs:
        zr, _ = bd_arm.load(t, "ssim2")
        per_tsv.append(zr)
    for target in images:
        row = f"{target:<8}"
        for zr in per_tsv:
            img_a = next((i for i in aom if i.startswith(target)), None)
            img_z = next((i for i in zr if i.startswith(target)), None)
            if img_a and img_z:
                bd = bd_arm.bd_rate(bd_arm.frontier(zr[img_z]), bd_arm.frontier(aom[img_a]))
                row += f" {bd:>+26.2f}" if bd is not None else f" {'NA':>26}"
            else:
                row += f" {'--':>26}"
        print(row)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("tsvs", nargs="+")
    ap.add_argument("--vs-aom", metavar="AOMREF")
    ap.add_argument("--images", default="1236,6018,9100,9118,9165,6091")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()
    wts = cluster_weights()
    if args.vs_aom:
        vs_aom(args.vs_aom, args.tsvs, args.images.split(","), wts)
        return
    base = args.tsvs[0]
    for arm in args.tsvs[1:]:
        analyze_arm(base, arm, wts, args.verbose)


if __name__ == "__main__":
    main()

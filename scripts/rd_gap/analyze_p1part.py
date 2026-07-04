#!/usr/bin/env python3
"""P1PART analysis (FAST_TIER_PARITY_PLAN P1 lever 1): arm summaries + the
per-family partition-recovery table.

For each arm TSV in OUTDIR (p1_s{6,8,4}_<arm>.tsv), computes direct-isolation
BD vs the same-speed base across all three metrics (bd_arm.py machinery), plus
a per-family slice keyed on the TSV's family column, plus — for s6 — each
family's recovery fraction of the s6→s4 step (BD(p1_s6_base → p1_s4_base)).

Usage: analyze_p1part.py OUTDIR [--speeds 6 8 4] [--full-prefix p1]
"""
import argparse
import collections
import csv
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bd_arm import LOWER_BETTER, bd_rate, frontier, load  # noqa: E402

METRICS = ["ssim2", "butteraugli_3n", "butteraugli_max"]


def img_families(path):
  fam = {}
  with open(path) as f:
    for r in csv.DictReader(f, delimiter="\t"):
      fam[r["image"]] = r.get("family", "?")
  return fam


def per_image_bd(base_path, arm_path, metric):
  base, base_ms = load(base_path, metric)
  arm, arm_ms = load(arm_path, metric)
  per_img = {}
  for img in sorted(base):
    if img not in arm:
      continue
    bd = bd_rate(frontier(arm[img]), frontier(base[img]))
    if bd is not None:
      per_img[img] = bd
  tsum = None
  common = [i for i in per_img if base_ms.get(i) and arm_ms.get(i)]
  if common:
    b = sum(sum(base_ms[i]) for i in common)
    a = sum(sum(arm_ms[i]) for i in common)
    if b > 0:
      tsum = a / b
  return per_img, tsum


def summarize(vals):
  v = np.array(list(vals))
  return float(np.median(v)), float(v.mean()), int((v < 0).sum()), len(v)


def main():
  ap = argparse.ArgumentParser()
  ap.add_argument("outdir")
  ap.add_argument("--speeds", nargs="*", default=["6", "8", "4"])
  ap.add_argument("--prefix", default="p1")
  ap.add_argument("--per-family-metric", default="ssim2")
  args = ap.parse_args()

  # s6->s4 step per family (the P0 convention: recovery denominators).
  s6_base = os.path.join(args.outdir, f"{args.prefix}_s6_base.tsv")
  s4_base = os.path.join(args.outdir, f"{args.prefix}_s4_base.tsv")
  step_by_fam = {}
  fam_of = {}
  if os.path.exists(s6_base) and os.path.exists(s4_base):
    fam_of = img_families(s6_base)
    step_img, _ = per_image_bd(s6_base, s4_base, args.per_family_metric)
    by_fam = collections.defaultdict(list)
    for img, bd in step_img.items():
      by_fam[fam_of.get(img, "?")].append(bd)
    step_by_fam = {f: float(np.median(v)) for f, v in by_fam.items()}

  for sp in args.speeds:
    base = os.path.join(args.outdir, f"{args.prefix}_s{sp}_base.tsv")
    if not os.path.exists(base):
      continue
    arms = sorted(
      f for f in os.listdir(args.outdir)
      if f.startswith(f"{args.prefix}_s{sp}_") and f.endswith(".tsv")
      and f != os.path.basename(base)
    )
    print(f"\n=== s{sp} arms vs {os.path.basename(base)} (coarse grid) ===")
    print("arm\tmetric\tn\tmedian_bd%\tmean_bd%\tbetter\ttime_ratio_rdpar")
    fam_rows = {}
    for a in arms:
      name = a[len(f"{args.prefix}_s{sp}_"):-4]
      for m in METRICS:
        per_img, tsum = per_image_bd(base, os.path.join(args.outdir, a), m)
        if not per_img:
          print(f"{name}\t{m}\t0\tNA\tNA\tNA\tNA")
          continue
        med, mean, better, n = summarize(per_img.values())
        ts = f"{tsum:.3f}" if tsum else "NA"
        print(f"{name}\t{m}\t{n}\t{med:+.4f}\t{mean:+.4f}\t{better}/{n}\t{ts}")
        if m == args.per_family_metric:
          fam_rows[name] = per_img
    # Per-family slice (median per family, chosen metric).
    if fam_rows:
      fams = fam_of or img_families(base)
      fam_names = sorted(set(fams.values()))
      print(f"\n--- s{sp} per-family median {args.per_family_metric} BD"
            f"{' + s6->s4 step recovery' if sp == '6' and step_by_fam else ''} ---")
      hdr = "family\tn" + ("\ts4step_bd" if sp == "6" and step_by_fam else "")
      for name in fam_rows:
        hdr += f"\t{name}"
      if sp == "6" and step_by_fam:
        hdr += "".join(f"\t{name}_recov" for name in fam_rows)
      print(hdr)
      for fam in fam_names:
        imgs = [i for i, f in fams.items() if f == fam]
        row = [fam, str(len(imgs))]
        if sp == "6" and step_by_fam:
          row.append(f"{step_by_fam.get(fam, float('nan')):+.2f}")
        recs = []
        for name, per_img in fam_rows.items():
          v = [per_img[i] for i in imgs if i in per_img]
          med = float(np.median(v)) if v else float("nan")
          row.append(f"{med:+.2f}")
          if sp == "6" and step_by_fam:
            st = step_by_fam.get(fam)
            recs.append(
              f"{100.0 * med / st:.0f}%" if st and st < 0 and v else "NA")
        row.extend(recs)
        print("\t".join(row))
      # ALL row
      row = ["ALL", str(len(fams))]
      if sp == "6" and step_by_fam:
        allstep = float(np.median(list(step_by_fam.values())))
        row.append(f"{allstep:+.2f}")
      recs = []
      for name, per_img in fam_rows.items():
        med = float(np.median(list(per_img.values())))
        row.append(f"{med:+.2f}")
        if sp == "6" and step_by_fam:
          recs.append(f"{100.0 * med / allstep:.0f}%" if allstep < 0 else "NA")
      row.extend(recs)
      print("\t".join(row))


if __name__ == "__main__":
  main()

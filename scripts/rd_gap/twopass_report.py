#!/usr/bin/env python3
"""Two-pass butteraugli A/B report: per-corpus BD summary + per-family and
per-image breakdowns from the {corpus}_{single,twopass}.tsv pairs that
run_2p_ab.sh produces. butteraugli-3n is the TARGET, ssim2 the VETO (roles
inverted vs the tune-ss2 program).

Usage: twopass_report.py DIR [--corpora train26,legacy] [--tsv OUT.tsv]
DIR holds {corpus}_single.tsv / {corpus}_twopass.tsv.
"""
import argparse
import collections
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bd_arm import bd_rate, frontier, load  # noqa: E402

METRICS = ["butteraugli_3n", "butteraugli_max", "ssim2"]
OUTLIERS = {  # the o_9051 class: best-arm-still-loses photos (RD_GAP doc)
    "o_3003", "o_3008", "o_5004", "o_6629", "o_6632", "o_9051", "o_9077",
}


def per_image_bd(base_path, arm_path, metric):
  base, base_ms = load(base_path, metric, "zenavif-2p")
  arm, arm_ms = load(arm_path, metric, "zenavif-2p")
  out = {}
  for img in sorted(base):
    if img not in arm:
      continue
    bd = bd_rate(frontier(arm[img]), frontier(base[img]))
    if bd is not None:
      out[img] = bd
  tsum = None
  common = [i for i in out if base_ms.get(i) and arm_ms.get(i)]
  if common:
    b = sum(sum(base_ms[i]) for i in common)
    a = sum(sum(arm_ms[i]) for i in common)
    if b > 0:
      tsum = a / b
  return out, tsum


def family_of(img, families):
  return families.get(img, "?")


def load_families(tsv_paths):
  fam = {}
  for p in tsv_paths:
    if not os.path.exists(p):
      continue
    with open(p) as f:
      next(f)
      for line in f:
        c = line.rstrip("\n").split("\t")
        if len(c) >= 4:
          fam[c[0]] = c[3]
  return fam


def summarize(vals):
  a = np.array(list(vals))
  return f"{np.median(a):+.4f}\t{a.mean():+.4f}\t{int((a < 0).sum())}/{len(a)}"


def main():
  ap = argparse.ArgumentParser()
  ap.add_argument("dir")
  ap.add_argument("--corpora", default="train26,legacy")
  ap.add_argument("--tsv", help="also write a machine-readable summary TSV")
  args = ap.parse_args()

  fam = load_families([
    args.dir + "/train26_single.tsv",
    args.dir + "/legacy_single.tsv",
  ])
  rows = []
  for corpus in args.corpora.split(","):
    base = f"{args.dir}/{corpus}_single.tsv"
    arm = f"{args.dir}/{corpus}_twopass.tsv"
    if not (os.path.exists(base) and os.path.exists(arm)):
      print(f"## {corpus}: MISSING TSVs, skipped")
      continue
    print(f"\n## {corpus}: twopass vs single (negative = twopass wins)")
    print("scope\tmetric\tn\tmedian_bd%\tmean_bd%\tbetter\ttime_ratio")
    per_metric = {}
    for m in METRICS:
      pi, tr = per_image_bd(base, arm, m)
      per_metric[m] = (pi, tr)
      trs = f"{tr:.3f}" if tr else "NA"
      print(f"all\t{m}\t{len(pi)}\t{summarize(pi.values())}\t{trs}")
      rows.append((corpus, "all", m, len(pi), np.median(list(pi.values())),
                   np.mean(list(pi.values())), tr))
      photos = {k: v for k, v in pi.items()
                if not family_of(k, fam).startswith("7")}
      if photos and len(photos) != len(pi):
        print(f"photos\t{m}\t{len(photos)}\t{summarize(photos.values())}\t{trs}")
        rows.append((corpus, "photos", m, len(photos),
                     np.median(list(photos.values())),
                     np.mean(list(photos.values())), tr))

    # per-family medians on the target metric
    pi3, _ = per_metric["butteraugli_3n"]
    by_fam = collections.defaultdict(list)
    for img, bd in pi3.items():
      by_fam[family_of(img, fam)].append(bd)
    print("per-family butteraugli_3n medians:")
    for f_, v in sorted(by_fam.items()):
      print(f"  fam {f_}\tn={len(v)}\t{np.median(v):+.2f}%")

    # per-image, worst-first on the target, with ssim2 veto column
    piss, _ = per_metric["ssim2"]
    print("per-image (ba3n / ssim2):")
    for img, bd in sorted(pi3.items(), key=lambda kv: kv[1]):
      ss = piss.get(img)
      sss = f"{ss:+.2f}" if ss is not None else "NA"
      mark = " <== o_9051-class" if any(img.startswith(o) for o in OUTLIERS) else ""
      print(f"  {bd:+8.2f}% / {sss:>8}%  {img}{mark}")

  if args.tsv:
    with open(args.tsv, "w") as f:
      f.write("corpus\tscope\tmetric\tn\tmedian_bd%\tmean_bd%\ttime_ratio\n")
      for r in rows:
        tr = f"{r[6]:.3f}" if r[6] else "NA"
        f.write(f"{r[0]}\t{r[1]}\t{r[2]}\t{r[3]}\t{r[4]:+.4f}\t{r[5]:+.4f}\t{tr}\n")
    print(f"\nwrote {args.tsv}")


if __name__ == "__main__":
  main()

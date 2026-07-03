#!/usr/bin/env python3
"""Direct arm-vs-baseline BD-rate between two rd_gap TSVs (same encoder, same
grid, different config arms), per image and summarized.

This is the "direct isolation" number the tune-ss2 program's verdicts use:
both TSVs come from the SAME binary with one gated knob toggled, so the BD
delta is attributable to the knob alone. Works on any quality metric column:
ssim2 (higher=better) or butteraugli 3-norm/max (lower=better; negated into a
quality axis, the metric-gaming veto protocol from TUNE_SSIMULACRA2_PLAN.md).

Usage:
  bd_arm.py BASE.tsv ARM.tsv [--metric ssim2|butteraugli_3n|butteraugli_max]
  bd_arm.py BASE.tsv ARM.tsv --all     # one row per metric, the veto view

Input columns (run_gap.sh):
  image w h family encoder fmt q bytes bpp ssim2 enc_ms butteraugli_3n butteraugli_max

Negative BD-rate = the arm needs fewer bits at matched quality (arm wins).
"""
import argparse
import collections
import csv
import math
import sys

import numpy as np

LOWER_BETTER = {"butteraugli_3n", "butteraugli_max"}


def load(path, metric, encoder="zenrav1e", photos_only=False):
  rows = collections.defaultdict(list)
  enc_ms = collections.defaultdict(list)
  with open(path) as f:
    for r in csv.DictReader(f, delimiter="\t"):
      if r["encoder"] != encoder:
        continue
      if photos_only and r.get("family", "").startswith("7"):
        continue
      try:
        v = float(r[metric])
        bpp = float(r["bpp"])
      except (ValueError, KeyError):
        continue  # NA cells (butteraugli off, failed decode)
      if metric in LOWER_BETTER:
        if v <= 0:
          continue
        v = -math.log(v)  # quality axis: higher = better
      rows[r["image"]].append((v, bpp))
      try:
        enc_ms[r["image"]].append(float(r["enc_ms"]))
      except ValueError:
        pass
  return rows, enc_ms


def frontier(points):
  bybpp = sorted(points, key=lambda p: (p[1], -p[0]))
  front, best = [], -1e18
  for s, b in bybpp:
    if s > best:
      front.append((s, b))
      best = s
  front.sort(key=lambda p: p[0])
  return front


def bd_rate(test, ref):
  """BD-rate of test vs ref over overlapping quality. + = test needs MORE bits."""

  def prep(f):
    seen = {}
    for s, b in f:
      seen[round(s, 6)] = np.log(b)
    xs = sorted(seen)
    return np.array(xs), np.array([seen[x] for x in xs])

  x1, y1 = prep(ref)
  x2, y2 = prep(test)
  if len(x1) < 4 or len(x2) < 4:
    return None
  lo, hi = max(x1.min(), x2.min()), min(x1.max(), x2.max())
  if hi <= lo:
    return None
  gg = np.linspace(lo, hi, 200)
  trapz = getattr(np, "trapezoid", None) or np.trapz
  avg = (
    trapz(np.interp(gg, x2, y2), gg) - trapz(np.interp(gg, x1, y1), gg)
  ) / (hi - lo)
  return (np.exp(avg) - 1.0) * 100.0


def one_metric(base_path, arm_path, metric, verbose, enc_base="zenrav1e",
               enc_arm="zenrav1e", photos_only=False):
  base, base_ms = load(base_path, metric, enc_base, photos_only)
  arm, arm_ms = load(arm_path, metric, enc_arm, photos_only)
  per_img = {}
  for img in sorted(base):
    if img not in arm:
      continue
    bd = bd_rate(frontier(arm[img]), frontier(base[img]))
    if bd is not None:
      per_img[img] = bd
  if not per_img:
    return None
  vals = np.array(list(per_img.values()))
  better = int((vals < 0).sum())
  tsum = None
  common = [i for i in per_img if base_ms.get(i) and arm_ms.get(i)]
  if common:
    b = sum(sum(base_ms[i]) for i in common)
    a = sum(sum(arm_ms[i]) for i in common)
    if b > 0:
      tsum = a / b
  if verbose:
    for img, bd in sorted(per_img.items(), key=lambda kv: kv[1]):
      print(f"  {bd:+8.2f}%  {img}")
  return {
    "metric": metric,
    "n": len(vals),
    "median": float(np.median(vals)),
    "mean": float(vals.mean()),
    "better": better,
    "time_ratio": tsum,
  }


def main():
  ap = argparse.ArgumentParser()
  ap.add_argument("base")
  ap.add_argument("arm")
  ap.add_argument("--metric", default="ssim2")
  ap.add_argument("--all", action="store_true", help="ssim2 + both butteraugli norms")
  ap.add_argument("--per-image", action="store_true")
  ap.add_argument("--encoder-base", default="zenrav1e", help="encoder column filter for BASE (e.g. libaom for tier refs)")
  ap.add_argument("--encoder-arm", default="zenrav1e")
  ap.add_argument("--photos-only", action="store_true", help="drop family 7xxx plots (the tier-table convention)")
  args = ap.parse_args()

  metrics = ["ssim2", "butteraugli_3n", "butteraugli_max"] if args.all else [args.metric]
  print(f"# arm={args.arm} vs base={args.base}")
  print("metric\tn\tmedian_bd%\tmean_bd%\tbetter\ttime_ratio")
  for m in metrics:
    r = one_metric(
      args.base, args.arm, m, args.per_image and m == "ssim2",
      args.encoder_base, args.encoder_arm, args.photos_only,
    )
    if r is None:
      print(f"{m}\t0\tNA\tNA\tNA\tNA")
      continue
    t = f"{r['time_ratio']:.3f}" if r["time_ratio"] else "NA"
    print(
      f"{m}\t{r['n']}\t{r['median']:+.4f}\t{r['mean']:+.4f}\t"
      f"{r['better']}/{r['n']}\t{t}"
    )


if __name__ == "__main__":
  main()

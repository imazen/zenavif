#!/usr/bin/env python3
"""Phase-1 D-diagnostic #1: does the encoder's D currency track ssim2 better
than raw pixel MSE? (Pre-registered rule: DECISION_RULE_DFIT1.md — read it
before interpreting output.)

Joins a scored corpus manifest (image, quantizer, ssim2, mse) to
d_aggregates.tsv (d_surviving, scopes) and reports:
  1. cross-image Pearson r per quantizer: ssim2 vs log10(D_surv/px) and
     ssim2 vs log10(MSE)  — the gating test (>= 4/5 quantizers)
  2. within-image across-q Pearson r (reported, not gating)

Usage: fit_trace_d_metric.py TRACE_DIR [--pixels-from-manifest]
Pixel counts come from the source images named in the manifest (via the
train26 sample TSV convention `..._s1024.png` renditions living in the trace
dir's manifest rows is NOT enough — pass the corpus sample TSV):
  fit_trace_d_metric.py TRACE_DIR --sample sample_images_train26.tsv
"""
import argparse
import csv
import math
import os
import sys


def pearson(xs, ys):
  n = len(xs)
  if n < 3:
    return float("nan")
  mx, my = sum(xs) / n, sum(ys) / n
  sx = math.sqrt(sum((x - mx) ** 2 for x in xs))
  sy = math.sqrt(sum((y - my) ** 2 for y in ys))
  if sx == 0 or sy == 0:
    return float("nan")
  return sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / (sx * sy)


def main():
  ap = argparse.ArgumentParser(description=__doc__)
  ap.add_argument("trace_dir")
  ap.add_argument("--sample", required=True,
                  help="corpus TSV (image w h family) for pixel counts")
  args = ap.parse_args()

  # image basename -> pixels (even-cropped like the dump)
  px = {}
  for r in csv.DictReader(open(args.sample), delimiter="\t"):
    base = os.path.basename(r["image"]).removesuffix(".png")
    w, h = int(r["w"]) & ~1, int(r["h"]) & ~1
    px[base] = w * h

  man = {}
  for r in csv.DictReader(open(os.path.join(args.trace_dir, "manifest.tsv")),
                          delimiter="\t"):
    if r["ssim2"] == "NA" or r.get("mse", "NA") == "NA":
      continue
    man[(r["image"], r["quantizer"])] = (float(r["ssim2"]), float(r["mse"]))

  rows = []
  for r in csv.DictReader(
      open(os.path.join(args.trace_dir, "d_aggregates.tsv")), delimiter="\t"):
    key = (r["image"], r["quantizer"])
    if key not in man or r["image"] not in px:
      continue
    ssim2, mse = man[key]
    d_surv = float(r["d_surviving"])
    if d_surv <= 0 or mse <= 0:
      continue
    rows.append({
      "image": r["image"], "q": int(r["quantizer"]), "ssim2": ssim2,
      "logd": math.log10(d_surv / px[r["image"]]),
      "logmse": math.log10(mse),
    })
  if not rows:
    sys.exit("no joinable rows (did the corpus carry ssim2+mse?)")

  qs = sorted({r["q"] for r in rows})
  print(f"# rows={len(rows)} images={len({r['image'] for r in rows})} qs={qs}")
  print("# TEST 1 — cross-image per quantizer (gating: |r_D|>|r_MSE| at >=4/5)")
  print("q\tn\tr_ssim2_vs_logD\tr_ssim2_vs_logMSE\tD_wins")
  d_wins = 0
  for q in qs:
    sub = [r for r in rows if r["q"] == q]
    rd = pearson([r["logd"] for r in sub], [r["ssim2"] for r in sub])
    rm = pearson([r["logmse"] for r in sub], [r["ssim2"] for r in sub])
    win = abs(rd) > abs(rm)
    d_wins += win
    print(f"{q}\t{len(sub)}\t{rd:+.4f}\t{rm:+.4f}\t{'D' if win else 'MSE'}")
  verdict = "D BEATS MSE" if d_wins >= 4 else "D DOES NOT BEAT MSE"
  print(f"# VERDICT (pre-registered): {verdict} ({d_wins}/{len(qs)} quantizers)")

  print("# TEST 2 — within-image across q (reported, not gating)")
  imgs = sorted({r["image"] for r in rows})
  rds, rms = [], []
  for im in imgs:
    sub = sorted((r for r in rows if r["image"] == im), key=lambda r: r["q"])
    rd = pearson([r["logd"] for r in sub], [r["ssim2"] for r in sub])
    rm = pearson([r["logmse"] for r in sub], [r["ssim2"] for r in sub])
    if not math.isnan(rd):
      rds.append(abs(rd))
    if not math.isnan(rm):
      rms.append(abs(rm))
  print(f"mean |r| across-q: logD {sum(rds)/len(rds):.4f}  "
        f"logMSE {sum(rms)/len(rms):.4f}  (n={len(rds)} images)")


if __name__ == "__main__":
  main()

#!/usr/bin/env python3
"""Phase-1 D-diagnostic #4: kernel-ingredient fit.
Rule (read FIRST): DECISION_RULE_DFIT4.md.

Global linear least squares:
  log10(p3) ~ a*log10(mse) + b*log10(1+src_var) + c*log10(1+src_grad)
             + d*(src_luma/255) + e
LOOCV over origins; statistic = held-out per-encode |Pearson r| of prediction
vs actual, averaged over encodes (comparable to DFIT3's 0.8355 MSE baseline).

Usage: fit_trace_d_kernel.py TRACE_DIR
"""
import csv
import math
import os
import sys

import numpy as np


def load_tiles(d):
  """-> list of dicts: image, q, y (log p3), x (feature vec)."""
  rows = []
  for r in csv.DictReader(open(os.path.join(d, "manifest.tsv")),
                          delimiter="\t"):
    p = os.path.join(d, f"sbmap_{r['image']}_s{r['speed']}_q{r['quantizer']}.tsv")
    if not os.path.exists(p):
      continue
    for t in csv.DictReader(open(p), delimiter="\t"):
      p3, mse = float(t["p3"]), float(t["mse"])
      if p3 <= 0 or mse <= 0:
        continue
      rows.append({
        "image": r["image"], "q": r["quantizer"],
        "y": math.log10(p3),
        "x": [
          math.log10(mse),
          math.log10(1.0 + float(t["src_var"])),
          math.log10(1.0 + float(t["src_grad"])),
          float(t["src_luma"]) / 255.0,
          1.0,
        ],
      })
  return rows


def fit(rows):
  x = np.array([r["x"] for r in rows])
  y = np.array([r["y"] for r in rows])
  coef, *_ = np.linalg.lstsq(x, y, rcond=None)
  return coef


def per_encode_r(rows, coef):
  """mean per-encode |Pearson r| of prediction vs actual."""
  by_enc = {}
  for r in rows:
    by_enc.setdefault((r["image"], r["q"]), []).append(r)
  rs = []
  for enc_rows in by_enc.values():
    if len(enc_rows) < 20:
      continue
    x = np.array([r["x"] for r in enc_rows])
    y = np.array([r["y"] for r in enc_rows])
    pred = x @ coef
    if pred.std() == 0 or y.std() == 0:
      continue
    rs.append(abs(float(np.corrcoef(pred, y)[0, 1])))
  return sum(rs) / len(rs), len(rs)


def main():
  d = sys.argv[1]
  rows = load_tiles(d)
  images = sorted({r["image"] for r in rows})
  print(f"# tiles={len(rows)} images={len(images)}")

  full_coef = fit(rows)
  names = ["log_mse", "log_var", "log_grad", "luma", "intercept"]
  print("# full-fit coefficients:",
        ", ".join(f"{n}={c:+.4f}" for n, c in zip(names, full_coef)))
  r_full, n_enc = per_encode_r(rows, full_coef)
  print(f"# full-fit mean per-encode |r| = {r_full:.4f} over {n_enc} encodes")

  # MSE-alone (per-encode correlation of log mse vs log p3) for the record.
  mse_only = fit([{**r, "x": [r["x"][0], 1.0]} for r in rows])
  r_mse, _ = per_encode_r(
    [{**r, "x": [r["x"][0], 1.0]} for r in rows], mse_only)
  print(f"# mse-alone mean per-encode |r| = {r_mse:.4f}")

  held = []
  for im in images:
    train = [r for r in rows if r["image"] != im]
    test = [r for r in rows if r["image"] == im]
    coef = fit(train)
    r_im, _ = per_encode_r(test, coef)
    held.append(r_im)
  loocv = sum(held) / len(held)
  print(f"# LOOCV held-out mean per-encode |r| = {loocv:.4f} "
        f"(folds min {min(held):.4f} max {max(held):.4f})")

  if loocv >= 0.90:
    v = "INGREDIENTS FOUND — activity-normalized error is the kernel shape"
  elif loocv >= 0.86:
    v = "PARTIAL — features help; sub-tile/frequency features next before encoder work"
  else:
    v = "INSUFFICIENT — per-block in-encoder features needed"
  print(f"# VERDICT (pre-registered): {v}")


if __name__ == "__main__":
  main()

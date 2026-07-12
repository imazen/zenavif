#!/usr/bin/env python3
"""Phase-1 D-diagnostic #6: the source-only sensitivity field.
Rule (read FIRST): DECISION_RULE_DFIT6.md.

Stage 1: log10(p3) ~ a*log10(mse) + b (global).
Stage 2: residual ~ source-only tile features + 3x3 neighborhood means +
frame means (9 features + intercept, global LSQ).
Statistic: per-encode |r| of (stage1+stage2) prediction vs actual, LOOCV by
origin. Baseline: stage-1 alone (DFIT4 mse-alone = 0.8280).

Usage: fit_sensitivity_field.py TRACE_DIR
"""
import csv
import math
import os
import sys

import numpy as np


def load(d):
  encodes = []
  for r in csv.DictReader(open(os.path.join(d, "manifest.tsv")),
                          delimiter="\t"):
    p = os.path.join(d, f"sbmap_{r['image']}_s{r['speed']}_q{r['quantizer']}.tsv")
    if not os.path.exists(p):
      continue
    tiles = {}
    for t in csv.DictReader(open(p), delimiter="\t"):
      p3, mse = float(t["p3"]), float(t["mse"])
      if p3 <= 0 or mse <= 0:
        continue
      tiles[(int(t["sb_x"]), int(t["sb_y"]))] = {
        "y": math.log10(p3),
        "logmse": math.log10(mse),
        "v": math.log10(1.0 + float(t["src_var"])),
        "g": math.log10(1.0 + float(t["src_grad"])),
        "l": float(t["src_luma"]) / 255.0,
      }
    if len(tiles) < 20:
      continue
    # frame means + 3x3 neighborhood means (source-only context)
    fv = sum(t["v"] for t in tiles.values()) / len(tiles)
    fg = sum(t["g"] for t in tiles.values()) / len(tiles)
    fl = sum(t["l"] for t in tiles.values()) / len(tiles)
    rows = []
    for (tx, ty), t in tiles.items():
      nb = [tiles[(tx + dx, ty + dy)]
            for dx in (-1, 0, 1) for dy in (-1, 0, 1)
            if (tx + dx, ty + dy) in tiles]
      nv = sum(x["v"] for x in nb) / len(nb)
      ng = sum(x["g"] for x in nb) / len(nb)
      nl = sum(x["l"] for x in nb) / len(nb)
      rows.append({
        "y": t["y"], "logmse": t["logmse"],
        "feat": [t["v"], t["g"], t["l"], nv, ng, nl, fv, fg, fl],
      })
    encodes.append({"image": r["image"], "rows": rows})
  return encodes


def lstsq(x, y):
  coef, *_ = np.linalg.lstsq(np.asarray(x), np.asarray(y), rcond=None)
  return coef


def per_encode_r(encodes, pred_fn):
  rs = []
  for e in encodes:
    y = np.array([r["y"] for r in e["rows"]])
    p = np.array([pred_fn(r) for r in e["rows"]])
    if y.std() == 0 or p.std() == 0:
      continue
    rs.append(abs(float(np.corrcoef(p, y)[0, 1])))
  return sum(rs) / len(rs)


def fit_two_stage(rows):
  x1 = [[r["logmse"], 1.0] for r in rows]
  y = [r["y"] for r in rows]
  c1 = lstsq(x1, y)
  resid = [r["y"] - (c1[0] * r["logmse"] + c1[1]) for r in rows]
  x2 = [r["feat"] + [1.0] for r in rows]
  c2 = lstsq(x2, resid)
  return c1, c2


def main():
  d = sys.argv[1]
  encodes = load(d)
  n = sum(len(e["rows"]) for e in encodes)
  print(f"# encodes={len(encodes)} tiles={n}")
  all_rows = [r for e in encodes for r in e["rows"]]

  c1, c2 = fit_two_stage(all_rows)
  s1 = per_encode_r(encodes, lambda r: c1[0] * r["logmse"] + c1[1])
  s12 = per_encode_r(
    encodes,
    lambda r: c1[0] * r["logmse"] + c1[1]
    + float(np.dot(c2, r["feat"] + [1.0])))
  print(f"# full-fit: stage1(mse-alone) |r|={s1:.4f}  +field |r|={s12:.4f}  "
        f"(field delta {s12 - s1:+.4f})")
  names = ["v", "g", "l", "nb_v", "nb_g", "nb_l", "fr_v", "fr_g", "fr_l", "1"]
  print("# field coefs:",
        ", ".join(f"{k}={c:+.4f}" for k, c in zip(names, c2)))

  images = sorted({e["image"] for e in encodes})
  held = []
  for im in images:
    train = [r for e in encodes if e["image"] != im for r in e["rows"]]
    test = [e for e in encodes if e["image"] == im]
    tc1, tc2 = fit_two_stage(train)
    held.append(per_encode_r(
      test,
      lambda r: tc1[0] * r["logmse"] + tc1[1]
      + float(np.dot(tc2, r["feat"] + [1.0]))))
  loocv = sum(held) / len(held)
  print(f"# LOOCV held-out mean per-encode |r| = {loocv:.4f} "
        f"(folds {min(held):.4f}..{max(held):.4f})")
  if loocv >= 0.90:
    v = "FIELD FOUND"
  elif loocv >= 0.86:
    v = "PARTIAL FIELD -> learned-field escalation registered"
  else:
    v = "NO FIELD at this capacity -> learned predictor or decode-side info"
  print(f"# VERDICT (pre-registered): {v}")


if __name__ == "__main__":
  main()

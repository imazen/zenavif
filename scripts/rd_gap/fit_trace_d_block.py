#!/usr/bin/env python3
"""Phase-1 D-diagnostic #5: committed-block level. Rule: DECISION_RULE_DFIT5.md.

Part 1 — per-encode |r| across committed blocks: winner-D vs block-butteraugli-p3,
against block-MSE vs the same target.
Part 2 — kernel candidate: global linear model with per-bsize intercepts,
LOOCV by origin, same statistic.

Usage: fit_trace_d_block.py TRACE_DIR
"""
import csv
import math
import os
import sys

import numpy as np

N_BSIZE = 22


def winner_d(path):
  """trace -> {(bo_x, bo_y, bsize): surviving winner D} (committed only)."""
  scope_best = {}
  last_decision = {}
  commits = []
  with open(path) as f:
    header = f.readline().rstrip("\n").split("\t")
    ix = {k: i for i, k in enumerate(header)}
    for line in f:
      c = line.rstrip("\n").split("\t")
      row = int(c[ix["row"]])
      key = (c[ix["bo_x"]], c[ix["bo_y"]], c[ix["bsize"]])
      if row == 3:
        commits.append(key)
        continue
      seq = int(c[ix["block_seq"]])
      if seq == 0:
        continue
      if row == 2:
        last_decision[key] = seq
      else:
        cost = float(c[ix["rd_cost"]])
        cur = scope_best.get(seq)
        if cur is None or cost < cur[0]:
          scope_best[seq] = (cost, float(c[ix["distortion"]]))
  out = {}
  for key in commits:
    seq = last_decision.get(key)
    if seq is None:
      continue
    b = scope_best.get(seq)
    if b is not None:
      out[key] = b[1]
  return out


def pear(a, b):
  a, b = np.asarray(a), np.asarray(b)
  if len(a) < 3 or a.std() == 0 or b.std() == 0:
    return float("nan")
  return float(np.corrcoef(a, b)[0, 1])


def main():
  d = sys.argv[1]
  encodes = []  # per encode: image, rows list of (y, x_features, bsize, logd, logmse)
  for r in csv.DictReader(open(os.path.join(d, "manifest.tsv")),
                          delimiter="\t"):
    bm = os.path.join(d, f"blockmap_{r['image']}_s{r['speed']}_q{r['quantizer']}.tsv")
    if not os.path.exists(bm):
      continue
    dmap = winner_d(os.path.join(d, r["trace"]))
    rows = []
    for t in csv.DictReader(open(bm), delimiter="\t"):
      key = (t["bo_x"], t["bo_y"], t["bsize"])
      dw = dmap.get(key)
      p3, mse = float(t["ba_p3"]), float(t["mse"])
      if dw is None or dw <= 0 or p3 <= 0 or mse <= 0:
        continue
      rows.append({
        "y": math.log10(p3),
        "logmse": math.log10(mse),
        "logd": math.log10(dw),
        "bsize": int(t["bsize"]),
        "feat": [
          math.log10(mse),
          math.log10(1.0 + float(t["src_var"])),
          math.log10(1.0 + float(t["src_grad"])),
          float(t["src_luma"]) / 255.0,
        ],
      })
    if len(rows) >= 50:
      encodes.append({"image": r["image"], "rows": rows})
  n_blocks = sum(len(e["rows"]) for e in encodes)
  print(f"# encodes={len(encodes)} blocks={n_blocks}")

  # Part 1 — D vs MSE at block level.
  rd, rm = [], []
  for e in encodes:
    y = [r["y"] for r in e["rows"]]
    rd.append(abs(pear([r["logd"] for r in e["rows"]], y)))
    rm.append(abs(pear([r["logmse"] for r in e["rows"]], y)))
  md, mm = sum(rd) / len(rd), sum(rm) / len(rm)
  wins = sum(1 for a, b in zip(rd, rm) if a > b)
  print(f"# PART 1: winner-D {md:.4f} vs block-MSE {mm:.4f}; "
        f"D wins {wins}/{len(encodes)}; gap {md - mm:+.4f}")

  def design(rows):
    x = np.zeros((len(rows), 4 + N_BSIZE))
    y = np.zeros(len(rows))
    for i, r in enumerate(rows):
      x[i, :4] = r["feat"]
      x[i, 4 + r["bsize"]] = 1.0
      y[i] = r["y"]
    return x, y

  def fit(rows):
    x, y = design(rows)
    coef, *_ = np.linalg.lstsq(x, y, rcond=None)
    return coef

  def per_encode_r(encs, coef):
    rs = []
    for e in encs:
      x, y = design(e["rows"])
      pred = x @ coef
      v = pear(pred, y)
      if not math.isnan(v):
        rs.append(abs(v))
    return sum(rs) / len(rs)

  all_rows = [r for e in encodes for r in e["rows"]]
  coef = fit(all_rows)
  print(f"# PART 2 full-fit coefs: mse={coef[0]:+.4f} var={coef[1]:+.4f} "
        f"grad={coef[2]:+.4f} luma={coef[3]:+.4f}")
  print(f"# full-fit mean per-encode |r| = {per_encode_r(encodes, coef):.4f}")

  images = sorted({e["image"] for e in encodes})
  held = []
  for im in images:
    train = [r for e in encodes if e["image"] != im for r in e["rows"]]
    test = [e for e in encodes if e["image"] == im]
    held.append(per_encode_r(test, fit(train)))
  loocv = sum(held) / len(held)
  print(f"# PART 2 LOOCV held-out mean per-encode |r| = {loocv:.4f} "
        f"(folds {min(held):.4f}..{max(held):.4f})")
  if loocv >= 0.90:
    v = "INGREDIENTS FOUND at block level"
  elif loocv >= 0.86:
    v = "PARTIAL at block level"
  else:
    v = "INSUFFICIENT — offline-feature path exhausted; in-encoder/learned predictor next"
  print(f"# VERDICT (pre-registered): {v}")


if __name__ == "__main__":
  main()

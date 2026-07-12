#!/usr/bin/env python3
"""Phase-1 D-diagnostic #3: per-tile D vs per-tile butteraugli vs per-tile MSE.
Rule (read FIRST): DECISION_RULE_DFIT3.md.

Per encode: join surviving winner-D (trace scopes, tile = bo*4 // SB) to the
sbmap tiles; correlate log10(tile ΣD) and log10(tile MSE) against
log10(tile butteraugli p3) ACROSS the encode's tiles; aggregate per-encode |r|
over the corpus. Secondary: DFIT2 bsize weights as a third predictor.

Usage: fit_trace_d_sb.py TRACE_DIR [--sb 64]
"""
import argparse
import csv
import math
import os
import sys

DFIT2_WEIGHTS = {7: 5.0, 8: 0.2, 9: 0.05, 12: 2.0}  # full-fit, rest 0


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


def tile_d(path, sb):
  """trace -> {(tx,ty): (sum surviving winner D, weighted sum)}."""
  scope_best = {}
  last_decision = {}
  surviving = set()
  with open(path) as f:
    header = f.readline().rstrip("\n").split("\t")
    ix = {k: i for i, k in enumerate(header)}
    for line in f:
      c = line.rstrip("\n").split("\t")
      row = int(c[ix["row"]])
      key = (c[ix["bo_x"]], c[ix["bo_y"]], c[ix["bsize"]])
      if row == 3:
        seq = last_decision.get(key)
        if seq is not None:
          surviving.add(seq)
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
          scope_best[seq] = (
            cost, float(c[ix["distortion"]]),
            int(c[ix["bo_x"]]), int(c[ix["bo_y"]]), int(c[ix["bsize"]]),
          )
  out = {}
  for seq in surviving:
    b = scope_best.get(seq)
    if b is None:
      continue
    _, d, bx, by, bsize = b
    t = ((bx * 4) // sb, (by * 4) // sb)
    s, ws = out.get(t, (0.0, 0.0))
    out[t] = (s + d, ws + DFIT2_WEIGHTS.get(bsize, 0.0) * d)
  return out


def main():
  ap = argparse.ArgumentParser(description=__doc__)
  ap.add_argument("trace_dir")
  ap.add_argument("--sb", type=int, default=64)
  args = ap.parse_args()
  d = args.trace_dir

  rows = list(csv.DictReader(open(os.path.join(d, "manifest.tsv")),
                             delimiter="\t"))
  r_d, r_mse, r_wd = [], [], []
  used = 0
  for r in rows:
    sbmap_path = os.path.join(
      d, f"sbmap_{r['image']}_s{r['speed']}_q{r['quantizer']}.tsv")
    if not os.path.exists(sbmap_path):
      continue
    tiles = {}
    for t in csv.DictReader(open(sbmap_path), delimiter="\t"):
      tiles[(int(t["sb_x"]), int(t["sb_y"]))] = (
        float(t["p3"]), float(t["mse"]))
    dmap = tile_d(os.path.join(d, r["trace"]), args.sb)
    ba, mse, dd, wd = [], [], [], []
    for t, (p3, m) in tiles.items():
      td, twd = dmap.get(t, (0.0, 0.0))
      if p3 <= 0 or m <= 0 or td <= 0:
        continue
      ba.append(math.log10(p3))
      mse.append(math.log10(m))
      dd.append(math.log10(td))
      wd.append(math.log10(twd) if twd > 0 else math.log10(td) - 3.0)
    if len(ba) < 20:
      continue
    used += 1
    r_d.append(abs(pearson(dd, ba)))
    r_mse.append(abs(pearson(mse, ba)))
    r_wd.append(abs(pearson(wd, ba)))
  if not used:
    sys.exit("no encodes joined (sbmaps present?)")

  md, mm, mw = (sum(r_d) / used, sum(r_mse) / used, sum(r_wd) / used)
  wins = sum(1 for a, b in zip(r_d, r_mse) if a > b)
  print(f"# encodes={used}  target=log tile-butteraugli-p3  (per-encode |r| means)")
  print(f"raw tile-D    : {md:.4f}")
  print(f"tile-MSE      : {mm:.4f}")
  print(f"DFIT2-wtd D   : {mw:.4f}   (secondary)")
  print(f"D wins on {wins}/{used} encodes; gap = {md - mm:+.4f}")
  if md - mm > 0.02 and wins * 2 > used:
    v = "D-BETTER per-tile -> kernel locally perceptual; refit = CROSS-CONTENT NORMALIZATION"
  elif mm - md > 0.02 and (used - wins) * 2 > used:
    v = "D-WORSE per-tile -> kernel fails at its own granularity; refit = NEW KERNEL vs tile metrics"
  else:
    v = "TIE per-tile -> kernel adds nothing over pixel error locally; refit = NEW KERNEL"
  print(f"# VERDICT (pre-registered): {v}")


if __name__ == "__main__":
  main()

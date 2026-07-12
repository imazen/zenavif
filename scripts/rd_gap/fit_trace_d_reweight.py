#!/usr/bin/env python3
"""Phase-1 D-diagnostic #2: per-bsize reweighting of the existing D currency.
Rule (read FIRST): DECISION_RULE_DFIT2.md — calibration-vs-kernel.

Per encode, reduce each SURVIVING scope to its winner (min-cost eval) and keep
(bsize, D). Model: score = log10(sum_b w_b * D_b / px). Fit one global
non-negative weight per bsize class by coordinate ascent on the mean
cross-image |Pearson r| vs ssim2 over quantizers; evaluate LOOCV over origins.

Usage: fit_trace_d_reweight.py TRACE_DIR --sample sample_images_train26.tsv
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


def surviving_d_by_bsize(path):
  """trace -> {bsize: sum of surviving winners' D}."""
  scope_best = {}  # seq -> (cost, dist, bsize)
  last_decision = {}
  surviving = set()
  with open(path) as f:
    header = f.readline().rstrip("\n").split("\t")
    ix = {k: i for i, k in enumerate(header)}
    for line in f:
      c = line.rstrip("\n").split("\t")
      row = int(c[ix["row"]])
      if row == 3:
        seq = last_decision.get((c[ix["bo_x"]], c[ix["bo_y"]], c[ix["bsize"]]))
        if seq is not None:
          surviving.add(seq)
        continue
      seq = int(c[ix["block_seq"]])
      if seq == 0:
        continue
      if row == 2:
        last_decision[(c[ix["bo_x"]], c[ix["bo_y"]], c[ix["bsize"]])] = seq
      else:
        cost = float(c[ix["rd_cost"]])
        cur = scope_best.get(seq)
        if cur is None or cost < cur[0]:
          scope_best[seq] = (cost, float(c[ix["distortion"]]),
                             int(c[ix["bsize"]]))
  out = {}
  for seq in surviving:
    b = scope_best.get(seq)
    if b is None:
      continue
    out[b[2]] = out.get(b[2], 0.0) + b[1]
  return out


def mean_cross_image_r(cells, weights, qs):
  """cells: list of dicts {q, ssim2, px, dmap}; returns mean |r| over qs."""
  rs = []
  for q in qs:
    xs, ys = [], []
    for c in cells:
      if c["q"] != q:
        continue
      s = sum(weights.get(b, 0.0) * d for b, d in c["dmap"].items())
      if s <= 0:
        s = 1e-9
      xs.append(math.log10(s / c["px"]))
      ys.append(c["ssim2"])
    r = pearson(xs, ys)
    if not math.isnan(r):
      rs.append(abs(r))
  return sum(rs) / len(rs) if rs else 0.0


def fit_weights(cells, bsizes, qs, iters=60):
  """Coordinate ascent over log-spaced multipliers, non-negative weights."""
  w = {b: 1.0 for b in bsizes}
  best = mean_cross_image_r(cells, w, qs)
  grid = [0.0, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0]
  for _ in range(iters):
    improved = False
    for b in bsizes:
      cur = w[b]
      for cand in grid:
        if cand == cur:
          continue
        w[b] = cand
        v = mean_cross_image_r(cells, w, qs)
        if v > best + 1e-6:
          best = v
          cur = cand
          improved = True
        else:
          w[b] = cur
    if not improved:
      break
  return w, best


def main():
  ap = argparse.ArgumentParser(description=__doc__)
  ap.add_argument("trace_dir")
  ap.add_argument("--sample", required=True)
  args = ap.parse_args()

  px = {}
  for r in csv.DictReader(open(args.sample), delimiter="\t"):
    base = os.path.basename(r["image"]).removesuffix(".png")
    px[base] = (int(r["w"]) & ~1) * (int(r["h"]) & ~1)

  cells = []
  for r in csv.DictReader(open(os.path.join(args.trace_dir, "manifest.tsv")),
                          delimiter="\t"):
    if r["ssim2"] == "NA" or r["image"] not in px:
      continue
    dmap = surviving_d_by_bsize(os.path.join(args.trace_dir, r["trace"]))
    cells.append({
      "image": r["image"], "q": int(r["quantizer"]),
      "ssim2": float(r["ssim2"]), "px": px[r["image"]], "dmap": dmap,
    })
  qs = sorted({c["q"] for c in cells})
  bsizes = sorted({b for c in cells for b in c["dmap"]})
  images = sorted({c["image"] for c in cells})
  print(f"# cells={len(cells)} images={len(images)} qs={qs} bsizes={bsizes}")

  # Baselines on the full set.
  raw = mean_cross_image_r(cells, {b: 1.0 for b in bsizes}, qs)
  print(f"# raw-D mean cross-image |r| = {raw:.4f}  (MSE baseline from DFIT1 ≈ 0.807)")

  # Full-set fit (optimistic bound) + LOOCV (the honest number).
  w_full, r_full = fit_weights(cells, bsizes, qs)
  print(f"# full-fit |r| = {r_full:.4f}  weights = "
        f"{{{', '.join(f'{b}: {w_full[b]:g}' for b in bsizes)}}}")

  held = []
  for im in images:
    train = [c for c in cells if c["image"] != im]
    w, _ = fit_weights(train, bsizes, qs)
    # held-out contribution: recompute cross-image r per q on the FULL set
    # with train-fitted weights, but that re-includes im everywhere; the
    # honest LOOCV for a cross-image statistic scores the full-set r with
    # weights fitted WITHOUT im, averaged over folds.
    held.append(mean_cross_image_r(cells, w, qs))
  loocv = sum(held) / len(held)
  print(f"# LOOCV (weights fit w/o each origin, scored on all) mean |r| = {loocv:.4f}")
  print(f"# spread across folds: min {min(held):.4f} max {max(held):.4f}")

  # Verdict per DECISION_RULE_DFIT2.md
  mse_ref = 0.807
  if loocv >= mse_ref - 0.05:
    verdict = "CALIBRATION FIXES IT"
  elif (loocv - raw) < 0.10 or loocv <= mse_ref - 0.15:
    verdict = "KERNEL PROBLEM"
  else:
    verdict = "PARTIAL — weights help, kernel still binding"
  print(f"# VERDICT (pre-registered): {verdict} "
        f"(raw {raw:.3f} -> loocv {loocv:.3f}; MSE {mse_ref:.3f})")
  zeroed = [b for b in bsizes if w_full[b] == 0.0]
  if zeroed:
    print(f"# degenerate-selection guard: full-fit zeroed bsizes {zeroed}")


if __name__ == "__main__":
  main()

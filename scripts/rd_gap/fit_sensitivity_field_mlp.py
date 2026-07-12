#!/usr/bin/env python3
"""Phase-1 D-diagnostic #7: the learned sensitivity field (tiny MLP,
multi-scale). Rule (read FIRST): DECISION_RULE_DFIT7.md.

Features per 64px tile (~20): {v,g,l} at 64px + sub-tile mean&max of {v,g}
from the 32px and 16px sbmaps + 3x3 neighborhood means (64px) + frame means.
Stage-1 mse-alone; stage-2 residual via a 1-hidden-layer tanh MLP (<=16
units, L2, fixed seed, full-batch Adam). LOOCV by origin.

Usage: fit_sensitivity_field_mlp.py TRACE_DIR [--hidden 12] [--l2 1e-3]
"""
import argparse
import csv
import math
import os

import numpy as np


def read_sbmap(path):
  tiles = {}
  for t in csv.DictReader(open(path), delimiter="\t"):
    tiles[(int(t["sb_x"]), int(t["sb_y"]))] = t
  return tiles


def load(d):
  encodes = []
  for r in csv.DictReader(open(os.path.join(d, "manifest.tsv")),
                          delimiter="\t"):
    tag = f"{r['image']}_s{r['speed']}_q{r['quantizer']}"
    p64 = os.path.join(d, f"sbmap_{tag}.tsv")
    p32 = os.path.join(d, f"sbmap32_{tag}.tsv")
    p16 = os.path.join(d, f"sbmap16_{tag}.tsv")
    if not all(os.path.exists(p) for p in (p64, p32, p16)):
      continue
    t64, t32, t16 = read_sbmap(p64), read_sbmap(p32), read_sbmap(p16)

    def sub_stats(sub, tx, ty, factor):
      vs, gs = [], []
      for dy in range(factor):
        for dx in range(factor):
          t = sub.get((tx * factor + dx, ty * factor + dy))
          if t is not None:
            vs.append(math.log10(1 + float(t["src_var"])))
            gs.append(math.log10(1 + float(t["src_grad"])))
      if not vs:
        return [0.0, 0.0, 0.0, 0.0]
      return [np.mean(vs), np.max(vs), np.mean(gs), np.max(gs)]

    base = {}
    for (tx, ty), t in t64.items():
      p3, mse = float(t["p3"]), float(t["mse"])
      if p3 <= 0 or mse <= 0:
        continue
      base[(tx, ty)] = {
        "y": math.log10(p3), "logmse": math.log10(mse),
        "v": math.log10(1 + float(t["src_var"])),
        "g": math.log10(1 + float(t["src_grad"])),
        "l": float(t["src_luma"]) / 255.0,
        "s32": sub_stats(t32, tx, ty, 2),
        "s16": sub_stats(t16, tx, ty, 4),
      }
    if len(base) < 20:
      continue
    fv = np.mean([b["v"] for b in base.values()])
    fg = np.mean([b["g"] for b in base.values()])
    fl = np.mean([b["l"] for b in base.values()])
    rows = []
    for (tx, ty), b in base.items():
      nb = [base[(tx + dx, ty + dy)]
            for dx in (-1, 0, 1) for dy in (-1, 0, 1)
            if (tx + dx, ty + dy) in base]
      feat = ([b["v"], b["g"], b["l"]]
              + b["s32"] + b["s16"]
              + [np.mean([x["v"] for x in nb]),
                 np.mean([x["g"] for x in nb]),
                 np.mean([x["l"] for x in nb]),
                 fv, fg, fl])
      rows.append({"y": b["y"], "logmse": b["logmse"],
                   "feat": np.array(feat)})
    encodes.append({"image": r["image"], "rows": rows})
  return encodes


def stage1(rows):
  x = np.array([[r["logmse"], 1.0] for r in rows])
  y = np.array([r["y"] for r in rows])
  c, *_ = np.linalg.lstsq(x, y, rcond=None)
  return c


class Mlp:
  def __init__(self, nin, hidden, l2, seed=7):
    rng = np.random.default_rng(seed)
    self.w1 = rng.normal(0, 1.0 / math.sqrt(nin), (nin, hidden))
    self.b1 = np.zeros(hidden)
    self.w2 = rng.normal(0, 1.0 / math.sqrt(hidden), hidden)
    self.b2 = 0.0
    self.l2 = l2

  def forward(self, x):
    h = np.tanh(x @ self.w1 + self.b1)
    return h @ self.w2 + self.b2, h

  def train(self, x, y, iters=800, lr=0.02):
    m = [np.zeros_like(p) for p in (self.w1, self.b1, self.w2)] + [0.0]
    v = [np.zeros_like(p) for p in (self.w1, self.b1, self.w2)] + [0.0]
    b1m, b2m, eps = 0.9, 0.999, 1e-8
    n = len(y)
    for t in range(1, iters + 1):
      pred, h = self.forward(x)
      err = pred - y
      gw2 = h.T @ err / n + self.l2 * self.w2
      gb2 = err.mean()
      dh = np.outer(err, self.w2) * (1 - h * h)
      gw1 = x.T @ dh / n + self.l2 * self.w1
      gb1 = dh.mean(axis=0)
      for i, (p, g) in enumerate(zip(
          (self.w1, self.b1, self.w2), (gw1, gb1, gw2))):
        m[i] = b1m * m[i] + (1 - b1m) * g
        v[i] = b2m * v[i] + (1 - b2m) * g * g
        p -= lr * (m[i] / (1 - b1m**t)) / (np.sqrt(v[i] / (1 - b2m**t)) + eps)
      m[3] = b1m * m[3] + (1 - b1m) * gb2
      v[3] = b2m * v[3] + (1 - b2m) * gb2 * gb2
      self.b2 -= lr * (m[3] / (1 - b1m**t)) / (math.sqrt(v[3] / (1 - b2m**t)) + eps)


def per_encode_r(encodes, c1, mlp, mu, sd):
  rs = []
  for e in encodes:
    x = np.array([r["feat"] for r in e["rows"]])
    xn = (x - mu) / sd
    resid_pred, _ = mlp.forward(xn)
    pred = np.array([c1[0] * r["logmse"] + c1[1] for r in e["rows"]]) + resid_pred
    y = np.array([r["y"] for r in e["rows"]])
    if y.std() == 0 or pred.std() == 0:
      continue
    rs.append(abs(float(np.corrcoef(pred, y)[0, 1])))
  return sum(rs) / len(rs)


def fit_all(rows, hidden, l2):
  c1 = stage1(rows)
  x = np.array([r["feat"] for r in rows])
  mu, sd = x.mean(axis=0), x.std(axis=0) + 1e-9
  xn = (x - mu) / sd
  resid = np.array([r["y"] - (c1[0] * r["logmse"] + c1[1]) for r in rows])
  mlp = Mlp(x.shape[1], hidden, l2)
  mlp.train(xn, resid)
  return c1, mlp, mu, sd


def main():
  ap = argparse.ArgumentParser(description=__doc__)
  ap.add_argument("trace_dir")
  ap.add_argument("--hidden", type=int, default=12)
  ap.add_argument("--l2", type=float, default=1e-3)
  args = ap.parse_args()
  encodes = load(args.trace_dir)
  n = sum(len(e["rows"]) for e in encodes)
  nf = len(encodes[0]["rows"][0]["feat"])
  print(f"# encodes={len(encodes)} tiles={n} features={nf} "
        f"hidden={args.hidden} l2={args.l2}")

  all_rows = [r for e in encodes for r in e["rows"]]
  c1, mlp, mu, sd = fit_all(all_rows, args.hidden, args.l2)
  r_full = per_encode_r(encodes, c1, mlp, mu, sd)
  print(f"# full-fit (train) mean per-encode |r| = {r_full:.4f}")

  images = sorted({e["image"] for e in encodes})
  held = []
  for im in images:
    train = [r for e in encodes if e["image"] != im for r in e["rows"]]
    test = [e for e in encodes if e["image"] == im]
    tc1, tm, tmu, tsd = fit_all(train, args.hidden, args.l2)
    held.append(per_encode_r(test, tc1, tm, tmu, tsd))
  loocv = sum(held) / len(held)
  print(f"# LOOCV held-out mean per-encode |r| = {loocv:.4f} "
        f"(folds {min(held):.4f}..{max(held):.4f})")

  if loocv >= 0.90:
    v = "LEARNED FIELD FOUND"
  elif loocv >= 0.86:
    v = "PARTIAL -> corpus-widening before bake"
  elif r_full >= 0.90:
    v = "TRANSFER-BOUND (as predicted) -> widen origins at same capacity"
  else:
    v = "CAPACITY-IRRELEVANT -> decode-side or transform-domain features"
  print(f"# VERDICT (pre-registered): {v}")


if __name__ == "__main__":
  main()

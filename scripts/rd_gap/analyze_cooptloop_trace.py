#!/usr/bin/env python3
"""Offline analyzer for zenrav1e `cooptloop_trace` TSV dumps (COOPT Phase 0).

Input: the TSV written by `cooptloop_trace::dump_tsv` (schema: block_seq bo_x
bo_y bsize row lambda rate_bits distortion rd_cost mode tx_size skip; row 0/1 =
currency evals, 2 = block decisions).

Reports the trace's shape and the two joins the Phase-1 fits build on:
  - per-scope chosen-vs-evaluated: the winner's rd_cost against the min /
    2nd-min currency cost inside its scope (the runner-up gap distribution —
    how contested decisions are, where λ-perturbations would flip choices);
  - the currency composition: λ spread (how many distinct λ regimes one encode
    runs), rate/distortion magnitude ranges per row kind.

Usage: analyze_cooptloop_trace.py TRACE.tsv [--json]
"""
import argparse
import collections
import json
import sys

import numpy as np


def load(path):
  rows = {"evals": [], "decisions": []}
  with open(path) as f:
    header = f.readline().rstrip("\n").split("\t")
    expect = [
      "block_seq", "bo_x", "bo_y", "bsize", "row", "lambda", "rate_bits",
      "distortion", "rd_cost", "mode", "tx_size", "skip",
    ]
    if header != expect:
      sys.exit(f"schema mismatch: {header}")
    for line in f:
      c = line.rstrip("\n").split("\t")
      seq, bsize, kind = int(c[0]), int(c[3]), int(c[4])
      lam, rate, dist, cost = (float(c[5]), float(c[6]), float(c[7]),
                               float(c[8]))
      if kind in (0, 1):
        rows["evals"].append((seq, bsize, kind, lam, rate, dist, cost))
      elif kind == 2:
        rows["decisions"].append(
          (seq, bsize, lam, cost, int(c[9]), int(c[10]), int(c[11])))
      elif kind == 3:
        rows.setdefault("commits", []).append((c[1], c[2], c[3]))
      else:
        sys.exit(f"unknown row kind {kind}")
  return rows


def analyze(rows):
  evals, decisions = rows["evals"], rows["decisions"]
  out = {
    "n_evals": len(evals),
    "n_decisions": len(decisions),
    "n_commits": len(rows.get("commits", [])),
  }

  # Currency shape.
  lams = np.array([e[3] for e in evals])
  out["lambda"] = {
    "distinct": int(len(np.unique(np.round(lams, 9)))),
    "min": float(lams.min()) if len(lams) else None,
    "max": float(lams.max()) if len(lams) else None,
  }
  out["scaled_eval_fraction"] = (
    float(sum(1 for e in evals if e[2] == 1) / len(evals)) if evals else None)

  # Scope join: per-decision runner-up gap.
  by_scope = collections.defaultdict(list)
  for e in evals:
    if e[0] > 0:
      by_scope[e[0]].append(e[6])
  gaps = []
  contested = 0
  for (seq, _bsize, _lam, _cost, _m, _t, _s) in decisions:
    costs = sorted(by_scope.get(seq, []))
    if len(costs) >= 2:
      # relative runner-up gap on the scope's own scale
      denom = max(costs[0], 1.0)
      gap = (costs[1] - costs[0]) / denom
      gaps.append(gap)
      if gap < 0.02:
        contested += 1
  if gaps:
    g = np.array(gaps)
    out["runner_up_gap"] = {
      "n": len(g),
      "p25": float(np.percentile(g, 25)),
      "p50": float(np.percentile(g, 50)),
      "p75": float(np.percentile(g, 75)),
      "contested_lt_2pct": contested,
    }

  # Decision composition.
  out["decisions_by_bsize"] = dict(
    collections.Counter(str(d[1]) for d in decisions))
  out["skip_fraction"] = (
    float(sum(1 for d in decisions if d[6] == 1) / len(decisions))
    if decisions else None)
  out["evals_per_scope_p50"] = (
    float(np.median([len(v) for v in by_scope.values()])) if by_scope else None)
  return out


def main():
  ap = argparse.ArgumentParser(description=__doc__)
  ap.add_argument("trace")
  ap.add_argument("--json", action="store_true")
  args = ap.parse_args()
  res = analyze(load(args.trace))
  if args.json:
    print(json.dumps(res, indent=2, sort_keys=True))
    return
  print(f"evals={res['n_evals']}  decisions={res['n_decisions']}  "
        f"scaled_frac={res['scaled_eval_fraction']:.3f}")
  print(f"lambda: {res['lambda']['distinct']} distinct in "
        f"[{res['lambda']['min']:.3g}, {res['lambda']['max']:.3g}]")
  if "runner_up_gap" in res:
    r = res["runner_up_gap"]
    print(f"runner-up gap (rel): p25={r['p25']:.4f} p50={r['p50']:.4f} "
          f"p75={r['p75']:.4f}; contested(<2%)={r['contested_lt_2pct']}/{r['n']}")
  print(f"evals/scope p50 = {res['evals_per_scope_p50']}; "
        f"skip_frac = {res['skip_fraction']:.3f}")
  print("decisions by bsize:", res["decisions_by_bsize"])


if __name__ == "__main__":
  main()

#!/usr/bin/env python3
"""COOPT Phase-1 D-leg opener: per-encode aggregates from decision traces.

For each trace in a gen_trace_corpus.sh output dir, reduce every decision
scope to its winner (the min-cost currency eval inside the scope) and sum the
winner (distortion, rate) over the frame:

  D_total = sum over scopes of argmin-cost eval's distortion
  R_total = sum over scopes of argmin-cost eval's rate_bits

plus join diagnostics (how often the min-eval cost matches the decision row's
rd_cost within tolerance — the winner-identification quality the D-fit rests
on). Emits one row per trace into d_aggregates.tsv beside the manifest.

The actual D-vs-metric regression (does D_total predict ssim2 across q better
than SSE?) needs decoded-quality scores per trace — generate the corpus with
`--ivf-out` wired (cooptloop_trace_dump) and score the IVFs, then join on
(image, quantizer). Until then this script's output is the trace-side half.

Usage: fit_trace_d.py TRACE_DIR
"""
import csv
import os
import sys


def reduce_trace(path):
  """One pass: per-scope min-cost eval (D, R); decision rows' rd_cost."""
  scope_best = {}  # seq -> (cost, dist, rate)
  decisions = {}  # seq -> rd_cost
  with open(path) as f:
    header = f.readline().rstrip("\n").split("\t")
    ix = {k: i for i, k in enumerate(header)}
    for line in f:
      c = line.rstrip("\n").split("\t")
      seq = int(c[ix["block_seq"]])
      if seq == 0:
        continue
      row = int(c[ix["row"]])
      cost = float(c[ix["rd_cost"]])
      if row == 2:
        decisions[seq] = cost
      else:
        dist = float(c[ix["distortion"]])
        rate = float(c[ix["rate_bits"]])
        cur = scope_best.get(seq)
        if cur is None or cost < cur[0]:
          scope_best[seq] = (cost, dist, rate)
  d_total = sum(v[1] for v in scope_best.values())
  r_total = sum(v[2] for v in scope_best.values())
  # Join quality: decision rd_cost vs the scope's min eval cost. The decision
  # cost composes luma+chroma+mode-rate, so exact equality is not expected —
  # report the fraction within 25% as a coarse identification diagnostic.
  matched = 0
  joinable = 0
  for seq, rd in decisions.items():
    best = scope_best.get(seq)
    if best is None or rd <= 0.0:
      continue
    joinable += 1
    if abs(best[0] - rd) <= 0.25 * rd:
      matched += 1
  return {
    "scopes": len(scope_best),
    "decisions": len(decisions),
    "d_total": d_total,
    "r_total_bits": r_total,
    "join_within_25pct": matched,
    "joinable": joinable,
  }


def main():
  if len(sys.argv) != 2:
    sys.exit(__doc__)
  d = sys.argv[1]
  manifest = os.path.join(d, "manifest.tsv")
  out_path = os.path.join(d, "d_aggregates.tsv")
  rows = list(csv.DictReader(open(manifest), delimiter="\t"))
  with open(out_path, "w") as out:
    out.write(
      "image\tfamily\tspeed\tquantizer\tscopes\tdecisions\td_total\t"
      "r_total_bits\tencoded_bytes\tjoin_within_25pct\tjoinable\n")
    for r in rows:
      agg = reduce_trace(os.path.join(d, r["trace"]))
      out.write(
        f'{r["image"]}\t{r["family"]}\t{r["speed"]}\t{r["quantizer"]}\t'
        f'{agg["scopes"]}\t{agg["decisions"]}\t{agg["d_total"]:.0f}\t'
        f'{agg["r_total_bits"]:.0f}\t{r["bytes"]}\t'
        f'{agg["join_within_25pct"]}\t{agg["joinable"]}\n')
      print(f'{r["image"]} q{r["quantizer"]}: D={agg["d_total"]:.3g} '
            f'R={agg["r_total_bits"]:.3g}b enc={r["bytes"]}B '
            f'join={agg["join_within_25pct"]}/{agg["joinable"]}')
  print(f"-> {out_path}")


if __name__ == "__main__":
  main()

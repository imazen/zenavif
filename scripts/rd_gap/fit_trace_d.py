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
  """One pass: per-scope min-cost eval (D, R); decisions; SURVIVING scopes.

  Survivors: commit rows (row 3, emitted by the final encode pass) join to
  the LAST decision scope at the same (bo, bsize) — valid under the corpus'
  threads=1 ordering. Without this filter the frame-level sums count every
  partition-leaf the search evaluated and discarded (measured 5.9x rate
  over-count, 2026-07-11).
  """
  scope_best = {}  # seq -> (cost, dist, rate)
  decisions = {}  # seq -> rd_cost
  last_decision = {}  # (bo_x, bo_y, bsize) -> seq
  surviving = set()
  commits = 0
  with open(path) as f:
    header = f.readline().rstrip("\n").split("\t")
    ix = {k: i for i, k in enumerate(header)}
    for line in f:
      c = line.rstrip("\n").split("\t")
      row = int(c[ix["row"]])
      if row == 3:
        commits += 1
        key = (c[ix["bo_x"]], c[ix["bo_y"]], c[ix["bsize"]])
        seq = last_decision.get(key)
        if seq is not None:
          surviving.add(seq)
        continue
      seq = int(c[ix["block_seq"]])
      if seq == 0:
        continue
      cost = float(c[ix["rd_cost"]])
      if row == 2:
        decisions[seq] = cost
        key = (c[ix["bo_x"]], c[ix["bo_y"]], c[ix["bsize"]])
        last_decision[key] = seq
      else:
        dist = float(c[ix["distortion"]])
        rate = float(c[ix["rate_bits"]])
        cur = scope_best.get(seq)
        if cur is None or cost < cur[0]:
          scope_best[seq] = (cost, dist, rate)
  d_total = sum(v[1] for v in scope_best.values())
  r_total = sum(v[2] for v in scope_best.values())
  d_surv = sum(v[1] for s, v in scope_best.items() if s in surviving)
  r_surv = sum(v[2] for s, v in scope_best.items() if s in surviving)
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
    "commits": commits,
    "surviving": len(surviving),
    "d_total": d_total,
    "r_total_bits": r_total,
    "d_surviving": d_surv,
    "r_surviving_bits": r_surv,
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
      "image\tfamily\tspeed\tquantizer\tscopes\tdecisions\tcommits\t"
      "surviving\td_total\tr_total_bits\td_surviving\tr_surviving_bits\t"
      "encoded_bytes\tjoin_within_25pct\tjoinable\n")
    for r in rows:
      agg = reduce_trace(os.path.join(d, r["trace"]))
      out.write(
        f'{r["image"]}\t{r["family"]}\t{r["speed"]}\t{r["quantizer"]}\t'
        f'{agg["scopes"]}\t{agg["decisions"]}\t{agg["commits"]}\t'
        f'{agg["surviving"]}\t{agg["d_total"]:.0f}\t'
        f'{agg["r_total_bits"]:.0f}\t{agg["d_surviving"]:.0f}\t'
        f'{agg["r_surviving_bits"]:.0f}\t{r["bytes"]}\t'
        f'{agg["join_within_25pct"]}\t{agg["joinable"]}\n')
      eb = float(r["bytes"]) if r["bytes"] not in ("NA", "") else 0.0
      ratio = (agg["r_surviving_bits"] / 8.0 / eb) if eb else float("nan")
      print(f'{r["image"]} q{r["quantizer"]}: commits={agg["commits"]} '
            f'surv={agg["surviving"]}/{agg["scopes"]} '
            f'Rsurv/8={agg["r_surviving_bits"]/8.0:.0f}B vs enc={r["bytes"]}B '
            f'(x{ratio:.2f}) join={agg["join_within_25pct"]}/{agg["joinable"]}')
  print(f"-> {out_path}")


if __name__ == "__main__":
  main()

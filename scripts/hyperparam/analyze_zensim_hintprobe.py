#!/usr/bin/env python3
"""What is the live per-superblock quantizer-hint channel worth?

Reads `zensim_loop_bench hintprobe` TSV and answers three separate
questions that are easy to conflate:

1. ACTIVATION STEP. Switching delta-q on is not a small perturbation --
   it also disables segmentation -- so there is a jump between the
   un-hinted encode at a quantizer and the "activated but flat" encode at
   the same quantizer. Measured against the un-hinted lattice, because a
   two-shot pass 1 is un-hinted and pass 2 would be paying this jump
   blind.

2. SUB-LATTICE INTERPOLATION. Inside the activated regime, does dithering
   k of N superblocks to a finer quantizer move the score smoothly
   between the endpoints? Two things must hold for a search to use it:
   the sweep must be MONOTONE in k (else a one-step placement cannot aim)
   and its per-step granularity must be finer than the un-hinted lattice
   gap it is supposed to refine (else it buys nothing).

3. RD. Does the diffmap-derived map buy a better score at the same bytes?
   Judged against the un-hinted bytes-vs-score curve interpolated to the
   map's own byte count -- comparing scores at different byte counts
   would be meaningless.

Usage: analyze_zensim_hintprobe.py probe.tsv [probe2.tsv ...]
"""

from __future__ import annotations

import csv
import math
import statistics
import sys
from collections import defaultdict


def pct(v, p):
    if not v:
        return float("nan")
    s = sorted(v)
    k = (len(s) - 1) * p / 100.0
    lo, hi = math.floor(k), math.ceil(k)
    return s[int(k)] if lo == hi else s[lo] + (s[hi] - s[lo]) * (k - lo)


def lattice_by_qi(cell, base_qi):
    """Un-hinted lattice rows keyed by their actual quantizer index.

    Newer harness runs put the true quantizer in the `k` column. Older ones
    left it at 0, but emitted the rows in a fixed `base_qi - 2 ..= base_qi + 6`
    order, so position recovers it exactly. Byte order does NOT -- bytes are
    not reliably monotone in the quantizer, which is precisely the kind of
    near-miss that makes an activation step look bigger or smaller than it is.
    """
    rows = cell.get("lattice", [])
    if any(int(r["k"]) != 0 for r in rows):
        return {int(r["k"]): (int(r["bytes"]), float(r["zensim"])) for r in rows}
    out = {}
    for i, r in enumerate(rows):
        qi = base_qi - 2 + i
        if 0 <= qi <= 255:
            out[qi] = (int(r["bytes"]), float(r["zensim"]))
    return out


def main(paths):
    rows = []
    for p in paths:
        with open(p) as f:
            rows.extend(list(csv.DictReader(f, delimiter="\t")))
    cells = defaultdict(lambda: defaultdict(list))
    for r in rows:
        cells[(r["image"], int(r["size"]), int(r["base_qi"]))][r["variant"]].append(r)

    print(f"# hintprobe cells: {len(cells)}  rows: {len(rows)}")
    print("# NOTE: 'lattice' rows are UN-HINTED encodes at neighbouring quantizers.")
    print()

    # ---- 1. activation step -------------------------------------------
    act_ds, act_db, gaps = [], [], []
    for (img, sz, qi), v in cells.items():
        lat = lattice_by_qi(v, qi)
        if qi not in lat or not v.get("activated_base"):
            continue
        base_b, base_s = lat[qi]
        for r in v["activated_base"]:
            act_ds.append(float(r["zensim"]) - base_s)
            act_db.append((int(r["bytes"]) - base_b) / max(1, base_b))
        ordered = [lat[k] for k in sorted(lat)]
        for a, b in zip(ordered, ordered[1:]):
            gaps.append(abs(a[1] - b[1]))

    if act_ds:
        print("# --- 1. ACTIVATION STEP (un-hinted -> activated-but-flat, same quantizer) ---")
        print(f"#   score delta : median {statistics.median(act_ds):+.4f}  "
              f"p10 {pct(act_ds, 10):+.4f}  p90 {pct(act_ds, 90):+.4f}  "
              f"min {min(act_ds):+.4f}  max {max(act_ds):+.4f}")
        print(f"#   |score delta|: median {statistics.median([abs(x) for x in act_ds]):.4f}  "
              f"p90 {pct([abs(x) for x in act_ds], 90):.4f}")
        print(f"#   bytes delta : median {statistics.median(act_db):+.2%}  "
              f"p90 {pct(act_db, 90):+.2%}  max {max(act_db):+.2%}")
        print(f"#   local un-hinted lattice gap for comparison: "
              f"median {statistics.median(gaps):.4f}  p90 {pct(gaps, 90):.4f}")
        big = sum(abs(x) > statistics.median(gaps) for x in act_ds) / len(act_ds)
        print(f"#   activation step exceeds the median lattice gap on {big:.1%} of cells")
        print()

    # ---- 2. sub-lattice interpolation ---------------------------------
    print("# --- 2. SUB-LATTICE INTERPOLATION (dither k of N superblocks) ---")
    print("# cells with a single superblock are excluded: nothing to dither.")
    print("size\tscale\tn\tspan_med\tstep_med\tmono_frac\tmax_reversal\tlat_gap_med\tstep<gap")
    by = defaultdict(list)
    for (img, sz, qi), v in cells.items():
        if not v.get("dither"):
            continue
        nsb_cell = int(v["dither"][0]["sbs"])
        if nsb_cell < 2:
            continue  # one superblock cannot be dithered at all
        lat = {k: sc for k, (_, sc) in lattice_by_qi(v, qi).items()}
        ordered = [lat[k] for k in sorted(lat)]
        lgap = statistics.median([abs(a - b) for a, b in zip(ordered, ordered[1:])]) \
            if len(ordered) > 1 else float("nan")
        for scale in sorted({float(r["scale"]) for r in v["dither"]}):
            base = [r for r in v.get("activated_base", []) if float(r["scale"]) == scale]
            pts = sorted((int(r["k"]), float(r["zensim"]))
                         for r in v["dither"] if float(r["scale"]) == scale)
            if not pts:
                continue
            seq = ([(0, float(base[0]["zensim"]))] if base else []) + pts
            span = seq[-1][1] - seq[0][1]
            steps = [seq[i + 1][1] - seq[i][1] for i in range(len(seq) - 1)]
            # monotone in the direction the span goes
            sign = 1.0 if span >= 0 else -1.0
            good = sum(1 for d in steps if d * sign >= -1e-9)
            rev = max((-d * sign for d in steps), default=0.0)
            nsb = int(v["dither"][0]["sbs"])
            by[(sz, scale)].append(
                (span, abs(span) / max(1, nsb), good / max(1, len(steps)), rev, lgap)
            )
    for k in sorted(by):
        sz, scale = k
        v = by[k]
        stepmed = statistics.median([x[1] for x in v])
        gapmed = statistics.median([x[4] for x in v if not math.isnan(x[4])] or [float("nan")])
        print(f"{sz}\t{scale}\t{len(v)}\t{statistics.median([x[0] for x in v]):+.4f}\t"
              f"{stepmed:.4f}\t{statistics.median([x[2] for x in v]):.1%}\t"
              f"{statistics.median([x[3] for x in v]):.4f}\t{gapmed:.4f}\t"
              f"{'YES' if stepmed < gapmed else 'no'}")
    print("#   span      = score(k=N) - score(activated, k=0)")
    print("#   step      = |span| / N, the granularity a dither search could aim with")
    print("#   mono_frac = fraction of adjacent-k steps going the span's own direction")
    print("#   max_rev   = largest step AGAINST the span direction (the aiming noise)")
    print()

    # ---- 3. RD of the diffmap-derived map ------------------------------
    print("# --- 3. RD: diffmap-derived map vs the un-hinted bytes/score curve ---")
    print("strength\tn\tmed_dscore_at_matched_bytes\tp10\tp90\twin_frac")
    rd = defaultdict(list)
    for (img, sz, qi), v in cells.items():
        lat = sorted({(int(r["bytes"]), float(r["zensim"])) for r in v.get("lattice", [])})
        if len(lat) < 2:
            continue
        for r in v.get("diffmap", []):
            # A one-superblock frame has a map of [1.0] after geomean
            # normalisation -- inert by construction, so it contributes a
            # guaranteed exact zero and would drag the median to 0.
            if int(r["sbs"]) < 2:
                continue
            b = int(r["bytes"])
            if b < lat[0][0] or b > lat[-1][0]:
                continue  # outside the measured curve: cannot judge honestly
            # linear interpolation of the un-hinted curve at these bytes
            for (b0, s0), (b1, s1) in zip(lat, lat[1:]):
                if b0 <= b <= b1:
                    ref = s0 if b1 == b0 else s0 + (b - b0) / (b1 - b0) * (s1 - s0)
                    rd[float(r["scale"])].append(float(r["zensim"]) - ref)
                    break
    for s in sorted(rd):
        v = rd[s]
        print(f"{s}\t{len(v)}\t{statistics.median(v):+.4f}\t{pct(v, 10):+.4f}\t"
              f"{pct(v, 90):+.4f}\t{sum(x > 0 for x in v) / len(v):.1%}")
    inert = sum(1 for v in cells.values() for r in v.get("diffmap", []) if int(r["sbs"]) < 2)
    print(f"#   {inert} diffmap rows were on single-superblock frames (map is inert there) "
          "and were excluded")
    skipped = (sum(len(v.get("diffmap", [])) for v in cells.values())
               - sum(len(v) for v in rd.values()) - inert)
    print(f"#   {skipped} diffmap rows fell outside the measured byte range and were NOT judged")
    print("#   (a score compared at a different byte count is not an RD comparison)")


if __name__ == "__main__":
    main(sys.argv[1:])

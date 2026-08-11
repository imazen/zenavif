#!/usr/bin/env python3
"""Between-run reproducibility check for the encode RD harness.

Within-run spread (`wall_spread_pct`) says how much five back-to-back repeats
of one cell disagree. It does NOT say whether the whole instrument lands in the
same place when you come back an hour later with a different box temperature,
a different page-cache state and different neighbours. That is the number that
decides whether a measured A/B difference is real, so it is measured separately:

    python3 run_grid.py ... --out runA.tsv
    python3 run_grid.py ... --out runB.tsv      # same grid, later
    python3 reproducibility.py runA.tsv runB.tsv

Reports, per arm, the distribution of `wall_ms_med(B)/wall_ms_med(A)` over the
cells the two runs share, and — separately, because it must be exactly 1.000 —
whether the two runs produced the same bytes for the same cell.
"""

from __future__ import annotations

import math
import sys
from collections import defaultdict


def read(path):
    rows, hdr = [], None
    with open(path) as fh:
        for line in fh:
            if line.startswith("#"):
                continue
            f = line.rstrip("\n").split("\t")
            if hdr is None:
                hdr = f
                continue
            rows.append(dict(zip(hdr, f)))
    return rows


def key(r):
    return (r["image"], r["size_tag"], r["arm"], r["ladder"], r["rate"])


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    A = {key(r): r for r in read(sys.argv[1]) if not r.get("fail")}
    B = {key(r): r for r in read(sys.argv[2]) if not r.get("fail")}
    shared = sorted(set(A) & set(B))
    print(f"run A: {len(A)} cells | run B: {len(B)} cells | shared: {len(shared)}")
    if not shared:
        print("no shared cells — the two runs did not cover the same grid")
        return 1

    # ---- bytes must be identical. Anything else is an encoder non-determinism
    # bug or a version skew between the two runs, not measurement noise.
    diff = [k for k in shared if A[k]["bytes_av1"] != B[k]["bytes_av1"]]
    print(f"\nbyte identity across runs: {len(shared) - len(diff)}/{len(shared)} cells equal")
    for k in diff[:10]:
        print(f"  DIFFERS {k}: {A[k]['bytes_av1']} -> {B[k]['bytes_av1']}")
    if diff:
        print("  ^ bytes differing BETWEEN runs means the two runs did not measure the")
        print("    same thing (encoder version skew, or true non-determinism).")

    # ---- timing agreement
    per_arm = defaultdict(list)
    per_arm_size = defaultdict(list)
    for k in shared:
        try:
            a, b = float(A[k]["wall_ms_med"]), float(B[k]["wall_ms_med"])
        except ValueError:
            continue
        if a > 0 and b > 0:
            per_arm[k[2]].append(b / a)
            per_arm_size[(k[2], k[1])].append(b / a)

    def stats(v):
        v = sorted(v)
        n = len(v)
        gm = math.exp(sum(math.log(x) for x in v) / n)
        # |log ratio| is the symmetric way to size a disagreement: a 2x and a
        # 0.5x are the same magnitude of error, which a raw percentage hides.
        ad = sorted(abs(math.log(x)) * 100 for x in v)
        return n, gm, v[n // 2], ad[n // 2], ad[min(n - 1, int(n * 0.9))], ad[-1]

    print(f"\ntiming ratio B/A per arm ({'gm'} = geometric mean; dev% = |ln ratio| x100)")
    print(f"{'arm':<10}{'n':>5}{'gm':>8}{'median':>9}{'dev_med%':>10}"
          f"{'dev_p90%':>10}{'dev_max%':>10}")
    for a in sorted(per_arm):
        n, gm, med, d50, d90, dmax = stats(per_arm[a])
        print(f"{a:<10}{n:>5}{gm:>8.3f}{med:>9.3f}{d50:>10.2f}{d90:>10.2f}{dmax:>10.2f}")

    print(f"\nsame, split by size — small cells are dominated by fixed jitter:")
    print(f"{'arm':<10}{'size':>7}{'n':>5}{'gm':>8}{'dev_med%':>10}{'dev_p90%':>10}")
    for k in sorted(per_arm_size, key=lambda x: (x[0], int(x[1]) if x[1].isdigit() else 0)):
        n, gm, med, d50, d90, dmax = stats(per_arm_size[k])
        print(f"{k[0]:<10}{k[1]:>7}{n:>5}{gm:>8.3f}{d50:>10.2f}{d90:>10.2f}")

    print("\nHow to read this: a gm far from 1.000 means the whole run shifted (thermal,")
    print("neighbours, page cache) — a systematic offset that a within-run spread number")
    print("cannot see. dev_p90 is the honest resolution of the instrument: an A/B")
    print("difference smaller than that is not measurable on this box in one sitting.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

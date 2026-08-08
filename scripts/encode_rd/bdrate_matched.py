#!/usr/bin/env python3
"""BD-rate at MATCHED WALL CLOCK, and the decision table that follows from it.

`analyze_matched.py` answers "at time T, how do the two arms compare at ONE
quality target". That is a slice. BD-rate is the integral over a quality RANGE,
which is what you actually want before choosing an encoder — a pair can be
0.95 at q45 and 1.20 at q85, and a single slice hides it.

The construction, which is the part worth reading:

  1. Per (image, size, arm, ladder rung) the rate sweep gives an RD curve.
     Pareto-filter it, then read off `bytes @ Q` and `time @ Q` for every Q in
     a dense quality grid. Outside the rung's achieved span: NA, never
     extrapolated.
  2. That yields, per (image, size, arm, Q), a time-vs-bytes frontier indexed
     by rung. Pareto-filter again (a rung both slower and bigger is never the
     right choice).
  3. Fix a TIME BUDGET T. Interpolate each frontier at T — in log-log, because
     time spans decades — to get `bytes_arm(Q | T)`. An arm whose ladder does
     not reach T at that Q contributes nothing there; it is not extrapolated
     to.
  4. Now each arm has an ordinary RD curve {(Q, bytes | T)} *at equal spend*.
     BD-rate between two of them is the mean of log10(bytes_A) - log10(bytes_B)
     over the overlapping quality range, back-transformed: 10^mean - 1.

Integration is piecewise-linear (trapezoid) over a dense quality grid, not the
classic global cubic fit. With a grid this dense the two agree closely, and the
cubic overshoots badly when a curve has a kink — which these do, because the
quality metric saturates near the 4:2:0 floor. The choice is stated rather than
buried; `--qgrid` controls the density.

    python3 bdrate_matched.py cells.tsv [--metric ssim2_floor]
        [--qgrid 20:88:2] [--times 30,100,300,1000] [--pairs a:b,...]
        [--bands 5,30,50,70,85,100]

Negative BD-rate = A needs FEWER bytes than B for the same quality at the same
measured encode time = A wins.
"""

from __future__ import annotations

import argparse
import math
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from analyze_matched import (  # noqa: E402  (shared, deliberately not duplicated)
    fnum, interp_at_quality, interp_bytes_at_time, pareto_rd,
    pareto_time_bytes, read_cells,
)


def bd_rate(curve_a, curve_b):
    """Bjontegaard delta-rate of A relative to B, as a fraction.

    curve_* = [(quality, bytes), ...]. Returns (bd, qlo, qhi, npts) or None
    when the two curves share fewer than two quality points.

    Negative = A is cheaper in bytes at equal quality.
    """
    da = {q: b for q, b in curve_a if b and b > 0}
    db = {q: b for q, b in curve_b if b and b > 0}
    common = sorted(set(da) & set(db))
    if len(common) < 2:
        return None
    # Trapezoid over log10(bytes) difference, normalised by the quality span,
    # so a wide overlap is not weighted more heavily than a narrow one.
    diffs = [math.log10(da[q]) - math.log10(db[q]) for q in common]
    area = 0.0
    for i in range(len(common) - 1):
        area += (diffs[i] + diffs[i + 1]) / 2.0 * (common[i + 1] - common[i])
    span = common[-1] - common[0]
    if span <= 0:
        return None
    return 10 ** (area / span) - 1.0, common[0], common[-1], len(common)


def build(rows, metric, bytes_col, qgrid):
    """(image, size, arm) -> {Q: pareto frontier [(time, bytes, rung)]}."""
    ok = [r for r in rows
          if not r.get("fail")
          and fnum(r.get(metric)) is not None
          and fnum(r.get("wall_ms_med")) is not None
          and fnum(r.get(bytes_col))]
    fr = defaultdict(lambda: defaultdict(list))
    keys = sorted({(r["image"], r["size_tag"], r["arm"]) for r in ok})
    by = defaultdict(list)
    for r in ok:
        by[(r["image"], r["size_tag"], r["arm"], int(r["ladder"]))].append(r)
    for (img, sz, arm) in keys:
        for (i2, s2, a2, lad), g in by.items():
            if (i2, s2, a2) != (img, sz, arm):
                continue
            pts = pareto_rd([(float(r[bytes_col]), fnum(r[metric]), fnum(r["wall_ms_med"]))
                             for r in g])
            for q in qgrid:
                got = interp_at_quality(pts, q)
                if got:
                    fr[(img, sz, arm)][q].append((got[1], got[0], lad))
    for k in fr:
        for q in fr[k]:
            fr[k][q] = pareto_time_bytes(fr[k][q])
    cls = {(r["image"], r["size_tag"]): r["content_class"] for r in ok}
    return fr, cls, ok


def curve_at_time(frontier_by_q, t, qgrid):
    """RD curve {(Q, bytes)} for one arm constrained to spend exactly t ms."""
    out = []
    for q in qgrid:
        f = frontier_by_q.get(q)
        if not f:
            continue
        b = interp_bytes_at_time(f, t)
        if b:
            out.append((q, b))
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cells")
    ap.add_argument("--metric", default="ssim2_floor")
    ap.add_argument("--bytes-col", default="bytes_av1")
    ap.add_argument("--qgrid", default="20:88:2", help="lo:hi:step of quality targets")
    ap.add_argument("--times", default="auto",
                    help="matched wall-clock budgets in ms, or 'auto' (default) to derive "
                         "them PER SIZE from the measured ladder overlap. A fixed ms budget "
                         "cannot work across sizes: at 64 px every arm finishes in single-"
                         "digit ms, so a 100 ms budget is unreachable for all of them and "
                         "the whole table reads n/a.")
    ap.add_argument("--ntimes", type=int, default=4,
                    help="how many geometric budgets to place inside each size's overlap")
    ap.add_argument("--pairs", default="")
    ap.add_argument("--bands", default="5,30,50,70,85,100",
                    help="quality band edges for the win table")
    ap.add_argument("--per-cell", action="store_true",
                    help="also print every (image,size) row, not just the aggregate")
    args = ap.parse_args()

    q_lo, q_hi, q_st = (float(x) for x in args.qgrid.split(":"))
    qgrid = []
    q = q_lo
    while q <= q_hi + 1e-9:
        qgrid.append(round(q, 4))
        q += q_st
    bands = [float(b) for b in args.bands.split(",")]

    rows, meta = read_cells(args.cells)
    fr, cls, ok = build(rows, args.metric, args.bytes_col, qgrid)
    arms = sorted({k[2] for k in fr})
    cells = sorted({(k[0], k[1]) for k in fr})

    # --- time budgets, per size ---------------------------------------------
    def overlap_for(size, want):
        """Widest time interval where >= `want` arms all have a reachable rung,
        using each arm's median-over-images frontier span at that size."""
        spans = []
        for a in arms:
            los, his = [], []
            for (img, s2) in cells:
                if s2 != size or (img, s2, a) not in fr:
                    continue
                ts = [p[0] for q in fr[(img, s2, a)] for p in fr[(img, s2, a)][q]]
                if ts:
                    los.append(min(ts)); his.append(max(ts))
            if los:
                # Conservative: the budget must be reachable for EVERY image at
                # this size, not the median one. Using medians here produced a
                # "4-arm overlap" at 64 px in which one arm reached zero quality
                # points on more than half the images.
                spans.append((max(los), min(his), a))
        if len(spans) < want:
            return None
        best = None
        import itertools
        for combo in itertools.combinations(spans, want):
            lo = max(x[0] for x in combo); hi = min(x[1] for x in combo)
            if hi > lo and (best is None or hi / lo > best[1] / best[0]):
                best = (lo, hi, [x[2] for x in combo])
        return best

    size_times, size_note = {}, {}
    all_sizes = sorted({s for _, s in cells}, key=lambda s: int(s) if s.isdigit() else 1 << 30)
    if args.times.strip().lower() == "auto":
        for s in all_sizes:
            got, want = None, len(arms)
            while want >= 2 and got is None:
                got = overlap_for(s, want)
                if got is None:
                    want -= 1
            if got is None:
                size_times[s], size_note[s] = [], "no two arms overlap in time"
                continue
            lo, hi, who = got
            n = max(2, args.ntimes)
            size_times[s] = [lo * (hi / lo) ** (i / (n - 1)) for i in range(n)]
            size_note[s] = (f"{want}-arm overlap {lo:.1f}-{hi:.1f} ms "
                            f"({'+'.join(sorted(who))})")
    else:
        fixed = [float(t) for t in args.times.split(",")]
        for s in all_sizes:
            size_times[s], size_note[s] = fixed, "fixed --times"
    times = sorted({round(t, 3) for s in all_sizes for t in size_times[s]})
    pairs = ([tuple(p.split(":")) for p in args.pairs.split(",") if ":" in p]
             or [(a, b) for i, a in enumerate(arms) for b in arms[i + 1:]])

    print("\n".join(meta))
    print(f"\n=== BD-RATE AT MATCHED WALL CLOCK ===")
    print(f"metric={args.metric}  bytes={args.bytes_col}  "
          f"quality grid {q_lo:g}..{q_hi:g} step {q_st:g} ({len(qgrid)} points)")
    print(f"{len(ok)} usable cells, {len(cells)} (image,size) combinations, arms {arms}")
    szs = all_sizes
    print("\ntime budgets, derived PER SIZE (a fixed ms budget is unreachable at 64 px and")
    print("trivial at 1024 — the ladders live in different decades):")
    for s in szs:
        print(f"    @{s:<6} {size_note[s]:<48} -> "
              + ", ".join(f"{t:.1f}" for t in size_times[s]) + " ms")

    # ---------------------------------------------------------- F0 ----------
    print("\n=== F0. reach: at how many of the quality grid points does each arm's")
    print("    ladder actually reach each time budget? An arm absent here is not")
    print("    losing, it is UNABLE to spend that long (or unable to go that fast). ===")
    for s in szs:
        if not size_times[s]:
            continue
        print(f"    @{s} " + "".join(f"{f'{t:.1f}ms':>10}" for t in size_times[s])
              + f"   (of {len(qgrid)} q-points, median over images)")
        for a in arms:
            row = []
            for t in size_times[s]:
                cnt = [len(curve_at_time(fr[(i, s2, a)], t, qgrid))
                       for (i, s2) in cells if s2 == s and (i, s2, a) in fr]
                row.append(f"{sorted(cnt)[len(cnt)//2]:>10}" if cnt else f"{'-':>10}")
            print(f"      {a:<10}" + "".join(row))

    # ---------------------------------------------------------- F1 ----------
    print("\n=== F1. BD-rate A vs B at matched time, aggregated over images ===")
    print("    Negative = A needs fewer bytes at equal quality AND equal encode time.")
    print("    n = images contributing; the median is reported (BD-rate is a percentage,")
    print("    so a median resists the one cell where an overlap is 2 points wide).")
    agg_rows = []
    for (A, B) in pairs:
        if A not in arms or B not in arms:
            continue
        printed = False
        for s in szs:
            for t in size_times[s]:
                per = defaultdict(list)
                spans = defaultdict(list)
                for (img, s2) in cells:
                    if s2 != s:
                        continue
                    ka, kb = (img, s, A), (img, s, B)
                    if ka not in fr or kb not in fr:
                        continue
                    ca = curve_at_time(fr[ka], t, qgrid)
                    cb = curve_at_time(fr[kb], t, qgrid)
                    got = bd_rate(ca, cb)
                    if not got:
                        continue
                    bd, q0, q1, n = got
                    per[(cls.get((img, s), "?"), s)].append(bd)
                    spans[(cls.get((img, s), "?"), s)].append((q0, q1, n))
                    if args.per_cell:
                        print(f"      [{t:.1f}ms] {img[:34]:<34} @{s:<5} {A}/{B} "
                              f"BD {bd*100:+7.2f}%  over q {q0:g}-{q1:g} ({n} pts)")
                for k in sorted(per):
                    v = sorted(per[k])
                    med = v[len(v) // 2]
                    q0 = min(x[0] for x in spans[k]); q1 = max(x[1] for x in spans[k])
                    if not printed:
                        print(f"\n  -- {A} vs {B} --")
                        printed = True
                    agg_rows.append((A, B, t, k[0], k[1], med, len(v)))
                    print(f"    t={t:>8.1f}ms  {k[0]:<9} @{k[1]:<5} n={len(v):<2} "
                          f"BD-rate {med*100:+7.2f}%   [worst {max(v)*100:+7.2f}, "
                          f"best {min(v)*100:+7.2f}]  q~{q0:g}-{q1:g}")
        if not printed:
            print(f"\n  -- {A} vs {B} -- no (image,size) where both arms reach a common "
                  f"time budget over >=2 quality points")

    # ---------------------------------------------------------- F2 ----------
    print("\n=== F2. WHO WINS: smallest bytes at matched time, by content class, size,")
    print("    quality band and time budget. This is the decision table. ===")
    print("    'n/a' = no arm's ladder reaches that time budget in that band.")
    print("    A trailing (k) is how many (image, q-point) votes the winner took.")
    band_lbl = [f"{bands[i]:g}-{bands[i+1]:g}" for i in range(len(bands) - 1)]
    for s in szs:
        if not size_times[s]:
            continue
        classes = sorted({cls.get((i, s2), "?") for (i, s2) in cells if s2 == s})
        for c in classes:
            print(f"\n  size {s} / {c}    [{size_note[s]}]")
            print(f"    {'band':>10}" + "".join(f"{f'{t:.1f}ms':>26}" for t in size_times[s]))
            for bi, bl in enumerate(band_lbl):
                b0, b1 = bands[bi], bands[bi + 1]
                qb = [q for q in qgrid if b0 <= q < b1]
                if not qb:
                    continue
                out = []
                for t in size_times[s]:
                    votes = defaultdict(int)
                    marg = defaultdict(list)
                    for (img, s2) in cells:
                        if s2 != s or cls.get((img, s2), "?") != c:
                            continue
                        for q in qb:
                            cand = {}
                            for a in arms:
                                if (img, s2, a) not in fr:
                                    continue
                                f = fr[(img, s2, a)].get(q)
                                if not f:
                                    continue
                                bb = interp_bytes_at_time(f, t)
                                if bb:
                                    cand[a] = bb
                            if len(cand) >= 2:
                                w = min(cand, key=cand.get)
                                votes[w] += 1
                                second = sorted(cand.values())[1]
                                marg[w].append(second / cand[w])
                    if not votes:
                        out.append(f"{'n/a':>26}")
                        continue
                    w = max(votes, key=votes.get)
                    tot = sum(votes.values())
                    gm = math.exp(sum(math.log(x) for x in marg[w]) / len(marg[w]))
                    out.append(f"{w + f' {votes[w]}/{tot} x{gm:.2f}':>26}")
                print(f"    {bl:>10}" + "".join(out))
    print("\n    x1.NN = geometric-mean byte ratio of the runner-up to the winner")
    print("    (x1.10 means the second-best arm needed 10% more bytes).")

    # ---------------------------------------------------------- F3 ----------
    print("\n=== F3. one-line summary per (class, size, time): the ranking ===")
    print("    COMMON SUPPORT ONLY — the ranking is computed on the (image, quality)")
    print("    points where EVERY listed arm reaches the budget. Averaging each arm")
    print("    over its own reachable set instead would flatter whichever arm only")
    print("    manages the easy cells, which is the opposite of what is wanted.")
    print("    Arms that reach too few common points to rank are named as excluded.")
    for s in szs:
        for c in sorted({cls.get((i, s2), "?") for (i, s2) in cells if s2 == s}):
            for t in size_times[s]:
                # reach[(img,q)][arm] = bytes
                reach = defaultdict(dict)
                for (img, s2) in cells:
                    if s2 != s or cls.get((img, s2), "?") != c:
                        continue
                    for q in qgrid:
                        for a in arms:
                            if (img, s2, a) not in fr:
                                continue
                            f = fr[(img, s2, a)].get(q)
                            if f:
                                bb = interp_bytes_at_time(f, t)
                                if bb:
                                    reach[(img, q)][a] = bb
                if not reach:
                    continue
                # Largest arm subset whose common support is still worth ranking:
                # drop the arm with the fewest reachable points until the shared
                # support stops growing meaningfully.
                present = sorted({a for v in reach.values() for a in v})
                keep = list(present)
                dropped = []
                while len(keep) > 2:
                    common = [v for v in reach.values() if all(a in v for a in keep)]
                    if len(common) >= 5:
                        break
                    worst = min(keep, key=lambda a: sum(1 for v in reach.values() if a in v))
                    keep.remove(worst)
                    dropped.append(worst)
                common = [v for v in reach.values() if all(a in v for a in keep)]
                if len(common) < 2 or len(keep) < 2:
                    print(f"    {c:<9} @{s:<5} t={t:>8.1f}ms : no common support "
                          f"(arms reach disjoint (image,quality) sets)")
                    continue
                tot = defaultdict(list)
                for v in common:
                    best = min(v[a] for a in keep)
                    for a in keep:
                        tot[a].append(v[a] / best)
                rank = sorted((math.exp(sum(math.log(x) for x in v) / len(v)), a)
                              for a, v in tot.items())
                note = f"   [excluded: {','.join(dropped)}]" if dropped else ""
                print(f"    {c:<9} @{s:<5} t={t:>8.1f}ms n={len(common):<3}: "
                      + "  ".join(f"{a} {g:.3f}" for g, a in rank) + note)
    print("\n    ratio is to the per-point best arm; 1.000 = on the frontier at every")
    print("    common point. n is the shared (image, quality) support, identical for")
    print("    every arm in the row.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Simulate a PROBE-based monotonicity guarantee over the armed label data.

The feature-gate (monotone_speed_gate) fixes the s5-valley but pattern-2
(s4 dominates s6/7/8 on bundle-hurts content) is NOT feature-separable. A probe
sidesteps features: for a requested speed, also encode a small ANCHOR set of
other tiers and pick the Pareto-best actually measured. This script evaluates
that policy on the existing sweep (no re-encode) to quantify inversions fixed
and the extra-encode cost — the data behind proposing an opt-in probe mode.

Input: benchmarks/mono_fit_labels_2026-07-06.tsv (24 armed origins, s4-s9 @ q80).
Cols: img fam speed q bytes ssim2 enc_ms

An inversion = a FASTER tier Pareto-dominates a SLOWER one (>=1% fewer bytes OR
>0.2 ssim2 better, AND clearly faster by >20%). "Fix" = after the probe pick,
the chosen point is no longer dominated by any faster tier.
"""
import sys
from collections import defaultdict

BYTES_MARGIN = 0.01     # >=1% fewer bytes counts as meaningfully smaller
SSIM2_MARGIN = 0.20     # >0.2 ssim2 counts as meaningfully better
TIME_MARGIN = 0.80      # "clearly faster" = <80% of the slower tier's time

def load(path):
    rows = defaultdict(dict)  # img -> speed -> (bytes, ssim2, ms)
    with open(path) as f:
        for ln in f:
            if ln.startswith('#') or ln.startswith('img\t') or not ln.strip():
                continue
            p = ln.rstrip('\n').split('\t')
            img, speed, by, ss, ms = p[0], int(p[2]), int(p[4]), float(p[5]), float(p[6])
            rows[img][speed] = (by, ss, ms)
    return rows

def dominates(a, b):
    """Does point a Pareto-dominate b on (bytes, ssim2), meaningfully?"""
    aby, ass, _ = a
    bby, bss, _ = b
    smaller = aby <= bby * (1 - BYTES_MARGIN)
    better = ass >= bss + SSIM2_MARGIN
    not_worse = aby <= bby and ass >= bss
    # meaningfully better on >=1 axis, not worse on the other
    return not_worse and (smaller or better)

def faster(a, b):
    return a[2] < TIME_MARGIN * b[2]

def inversions(pts):
    """List (fast_speed, slow_speed) where the faster tier dominates the slower."""
    out = []
    speeds = sorted(pts)
    for s_slow in speeds:
        for s_fast in speeds:
            if s_fast == s_slow:
                continue
            a, b = pts[s_fast], pts[s_slow]
            if faster(a, b) and dominates(a, b):
                out.append((s_fast, s_slow))
    return out

def probe_pick(pts, requested, anchors):
    """Budget-respecting monotone pick: treat the requested speed's encode time
    as a TIME BUDGET. Among {requested} U anchors that are present AND no slower
    than requested (time <= budget), pick the best RD (highest ssim2, tie-break
    fewest bytes). This is the true RD-vs-time monotone guarantee: the user never
    pays more time than they asked, and gets the best RD reachable in that budget,
    so no faster tier can beat their result. Returns the chosen speed."""
    if requested not in pts:
        return requested
    budget = pts[requested][2]
    cand = [s for s in ([requested] + anchors)
            if s in pts and pts[s][2] <= budget]
    if not cand:
        return requested
    # best RD within budget: max ssim2, then fewest bytes
    return max(cand, key=lambda s: (pts[s][1], -pts[s][0]))

def _pareto_le(a, b):
    """a is at-least-as-good as b on (bytes lower, ssim2 higher)."""
    return a[0] <= b[0] and a[1] >= b[1]

def main():
    path = sys.argv[1] if len(sys.argv) > 1 else 'benchmarks/mono_fit_labels_2026-07-06.tsv'
    rows = load(path)
    policies = {
        'none':        [],                 # baseline: no probe
        'probe-s4':    [4],                # anchor s4 only (pattern-2)
        'probe-s4-s9': [4, 9],             # s4 (pattern-2) + s9 (s5-valley)
        'probe-full':  list(range(1, 11)), # all tiers (the full guarantee)
    }
    REQ = [5, 6, 7, 8]  # the bundle/valley tiers users request that can invert

    print(f"# probe-monotonicity simulation over {len(rows)} armed origins @ q80")
    print(f"# margins: bytes>={BYTES_MARGIN:.0%} ssim2>{SSIM2_MARGIN} time<{TIME_MARGIN:.0%}")
    print(f"# policy=anchor set probed alongside the requested speed; pick=best RD within the")
    print(f"# requested speed's time budget. dRD=ssim2 gain of overrides; dTime=pick vs requested")
    print(f"# (neg = pick is FASTER); wall_x=total probe encode cost / naive single encode.")
    print(f"# {'policy':13} {'req_inv':>8} {'overrides':>10} {'fixed':>6} {'new':>4} "
          f"{'mean_dRD':>9} {'mean_dTime_ms':>13} {'mean_wall_x':>12}")

    # count raw inversions per requested speed across origins
    raw = 0
    for img, pts in rows.items():
        invs = {(f, s) for (f, s) in inversions(pts)}
        raw += sum(1 for (f, s) in invs if s in REQ)

    for name, anchors in policies.items():
        fixed = new = overrides = 0
        drd, dtime, wallx = [], [], []
        for img, pts in rows.items():
            base_inv = {(f, s) for (f, s) in inversions(pts) if s in REQ}
            for s in REQ:
                if s not in pts:
                    continue
                pick = probe_pick(pts, s, anchors) if anchors else s
                was_inv = any(sl == s for (fa, sl) in base_inv)
                chosen = pts[pick]
                now_inv = any(faster(pts[o], chosen) and dominates(pts[o], chosen)
                              for o in pts if o != pick)
                if was_inv and not now_inv:
                    fixed += 1
                if not was_inv and now_inv:
                    new += 1
                if pick != s:
                    overrides += 1
                    drd.append(pts[pick][1] - pts[s][1])
                    dtime.append(pts[pick][2] - pts[s][2])
                if anchors:
                    # the probe encodes requested + every present anchor, keeps the best
                    ran_ms = pts[s][2] + sum(pts[a][2] for a in anchors if a in pts and a != s)
                    wallx.append(ran_ms / pts[s][2])
        mdr = sum(drd) / len(drd) if drd else 0.0
        mdt = sum(dtime) / len(dtime) if dtime else 0.0
        mwx = sum(wallx) / len(wallx) if wallx else 1.0
        print(f"  {name:13} {raw:>8} {overrides:>10} {fixed:>6} {new:>4} "
              f"{mdr:>9.3f} {mdt:>13.0f} {mwx:>12.2f}")

    # per-origin pattern-2 detail
    print("\n# per-origin: does s4 dominate s6? (pattern-2 inversion + probe gain)")
    for img in sorted(rows):
        pts = rows[img]
        if 4 in pts and 6 in pts:
            a, b = pts[4], pts[6]
            if faster(a, b) and dominates(a, b):
                fam = img.split('_')[0]
                print(f"  {fam:6} s4 {a[0]:>7}B/{a[1]:.2f}/{a[2]:.0f}ms  DOMINATES  "
                      f"s6 {b[0]:>7}B/{b[1]:.2f}/{b[2]:.0f}ms  "
                      f"(probe s4: {b[0]-a[0]:+d}B {a[1]-b[1]:+.2f}ss {a[2]-b[2]:+.0f}ms)")

if __name__ == '__main__':
    main()

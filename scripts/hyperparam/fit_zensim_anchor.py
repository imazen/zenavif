#!/usr/bin/env python3
"""Fit the zensim-B score -> AVIF quality anchor curve used by the
`two-pass-zensim` closed loop (`src/two_pass_zensim.rs`).

Deterministic: no randomness, no external deps beyond the stdlib. Re-running
it on the same sweep TSV reproduces the shipped constants byte for byte.

    python3 scripts/hyperparam/fit_zensim_anchor.py <sweep.tsv> [--emit-rust]

Input is the TSV emitted by
`examples/zensim_loop_bench.rs sweep`:
    image  size  w  h  q  qindex  bytes  zensim  dm_mean  enc_ms

What it produces
----------------
1. The anchor knots (ANCHOR_SCORE / ANCHOR_QUALITY in Rust). Per (image,
   size) cell the q->score curve is isotonized (PAVA, weightless), then for
   each score knot the LEFTMOST quality reaching it is read off by linear
   interpolation -- the same "smallest file that reaches the band"
   convention the loop's selection policy uses. The knot's quality is the
   MEDIAN across cells, then isotonized across knots so the shipped curve is
   monotone by construction.

2. The linear fit of quality on score over the central band, reported with
   BOTH intercept and slope (a slope alone would be meaningless -- the whole
   point of the piecewise curve is that the slope is not constant).

3. The reach census: how many cells can reach each knot at all. A knot no
   cell reaches is a codec limit, not a fit failure, and is reported as one.

4. The diffmap-error / quantizer elasticity d ln(dm_mean) / d ln(qindex),
   which is the empirical counterpart of the `gamma` the loop's derived
   spatial `strength = 1/gamma` assumes. INDICATIVE ONLY: qindex is not the
   dequant step (AV1's ac_qlookup is nonlinear), so this is evidence about
   the shape, not a fit of the shipped constant.
"""

import csv
import math
import statistics
import sys
from collections import defaultdict

# Score knots the Rust side stores. Uniform 5-unit spacing across the whole
# usable range: the low band gets exactly the same density as the high band
# (a grid denser at high scores would be calibrating only the easy end).
KNOTS = [20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 85.0, 90.0]

# Band used for the reported linear fit (where every size/content class has
# real coverage; below it the curves are on their floor).
FIT_BAND = (40.0, 90.0)


def pava(ys):
    """Pool-adjacent-violators: nearest non-decreasing sequence in L2."""
    vals = [float(y) for y in ys]
    weights = [1.0] * len(vals)
    i = 0
    while i < len(vals) - 1:
        if vals[i] <= vals[i + 1]:
            i += 1
            continue
        w = weights[i] + weights[i + 1]
        v = (vals[i] * weights[i] + vals[i + 1] * weights[i + 1]) / w
        vals[i : i + 2] = [v]
        weights[i : i + 2] = [w]
        i = max(i - 1, 0)
    out = []
    for v, w in zip(vals, weights):
        out.extend([v] * int(round(w)))
    return out


def leftmost_quality(qs, scores, knot):
    """Lowest quality whose isotonized score reaches `knot`, or None when the
    curve never gets there (a reach limit for that cell)."""
    if scores[-1] < knot:
        return None
    if scores[0] >= knot:
        return qs[0]
    for i in range(len(qs) - 1):
        s0, s1 = scores[i], scores[i + 1]
        if s0 < knot <= s1:
            if s1 - s0 < 1e-12:
                return qs[i + 1]
            return qs[i] + (knot - s0) / (s1 - s0) * (qs[i + 1] - qs[i])
    return qs[-1]


def linfit(xs, ys):
    """Ordinary least squares -> (intercept, slope, r2, n)."""
    n = len(xs)
    mx, my = sum(xs) / n, sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    sxy = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    slope = sxy / sxx if sxx > 0 else float("nan")
    intercept = my - slope * mx
    ss_tot = sum((y - my) ** 2 for y in ys)
    ss_res = sum((y - (intercept + slope * x)) ** 2 for x, y in zip(xs, ys))
    r2 = 1.0 - ss_res / ss_tot if ss_tot > 0 else float("nan")
    return intercept, slope, r2, n


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    path = sys.argv[1]
    emit_rust = "--emit-rust" in sys.argv

    rows = list(csv.DictReader(open(path), delimiter="\t"))
    cells = defaultdict(list)
    for r in rows:
        cells[(r["image"], int(r["size"]))].append(
            (float(r["q"]), float(r["zensim"]), int(r["qindex"]), float(r["dm_mean"]))
        )

    print(f"# rows {len(rows)}  cells {len(cells)}")
    sizes = sorted({s for _, s in cells})
    print(f"# sizes {sizes}")
    print(f"# images {len({i for i, _ in cells})}")

    # ---- 1. anchor knots -------------------------------------------------
    per_knot = defaultdict(list)
    per_knot_by_size = defaultdict(lambda: defaultdict(list))
    for (image, size), pts in cells.items():
        pts.sort()
        qs = [p[0] for p in pts]
        iso = pava([p[1] for p in pts])
        for k in KNOTS:
            q = leftmost_quality(qs, iso, k)
            if q is not None:
                per_knot[k].append(q)
                per_knot_by_size[size][k].append(q)

    print("\n# --- anchor knots (median leftmost quality reaching each score) ---")
    print("score\tq_median\tq_p25\tq_p75\tn_cells\tn_unreached")
    medians = []
    for k in KNOTS:
        v = sorted(per_knot[k])
        unreached = len(cells) - len(v)
        if not v:
            print(f"{k}\tNA\tNA\tNA\t0\t{unreached}")
            medians.append(None)
            continue
        p25 = v[max(0, int(0.25 * (len(v) - 1)))]
        p75 = v[min(len(v) - 1, int(0.75 * (len(v) - 1)))]
        med = statistics.median(v)
        medians.append(med)
        print(f"{k}\t{med:.3f}\t{p25:.3f}\t{p75:.3f}\t{len(v)}\t{unreached}")

    # Fill any all-unreached knot by linear extrapolation from its neighbours
    # so the shipped table has no holes, then isotonize.
    known = [(k, m) for k, m in zip(KNOTS, medians) if m is not None]
    if len(known) < 2:
        print("!! not enough reached knots to fit", file=sys.stderr)
        sys.exit(1)
    filled = []
    for k, m in zip(KNOTS, medians):
        if m is not None:
            filled.append(m)
        elif k < known[0][0]:
            (k0, m0), (k1, m1) = known[0], known[1]
            filled.append(m0 + (k - k0) * (m1 - m0) / (k1 - k0))
        else:
            (k0, m0), (k1, m1) = known[-2], known[-1]
            filled.append(m1 + (k - k1) * (m1 - m0) / (k1 - k0))
    mono = [min(100.0, max(1.0, v)) for v in pava(filled)]

    print("\n# --- per-size medians (is one curve enough, or does size shift it?) ---")
    hdr = "score\t" + "\t".join(f"q@{s}" for s in sizes)
    print(hdr)
    for k in KNOTS:
        cellvals = []
        for s in sizes:
            v = per_knot_by_size[s][k]
            cellvals.append(f"{statistics.median(v):.2f}" if v else "NA")
        print(f"{k}\t" + "\t".join(cellvals))

    # ---- 2. linear fit over the central band -----------------------------
    xs, ys = [], []
    for k, m in zip(KNOTS, mono):
        if FIT_BAND[0] <= k <= FIT_BAND[1]:
            xs.append(k)
            ys.append(m)
    a, b, r2, n = linfit(xs, ys)
    print(
        f"\n# --- linear fit q = a + b*score over score in {FIT_BAND} ---\n"
        f"# intercept a = {a:.4f}   slope b = {b:.4f} quality/score   R2 = {r2:.4f}   n = {n}"
    )
    resid = [y - (a + b * x) for x, y in zip(xs, ys)]
    print(f"# max |residual| vs the piecewise table = {max(abs(r) for r in resid):.3f} quality points")

    # ---- 3. per-cell spread around the shipped curve ---------------------
    devs = []
    for k, m in zip(KNOTS, mono):
        for q in per_knot[k]:
            devs.append(abs(q - m))
    if devs:
        devs.sort()
        print(
            f"\n# --- |per-cell leftmost-q - shipped anchor| ---\n"
            f"# p50 {devs[len(devs)//2]:.2f}  p90 {devs[int(0.9*(len(devs)-1))]:.2f}  "
            f"max {devs[-1]:.2f} quality points  (n={len(devs)})"
        )
        print(
            "# This is the residual the loop's pass-2 correction has to remove;\n"
            "# it is what makes a 1-encode open-loop answer a prediction, not a\n"
            "# convergence."
        )

    # ---- 4. diffmap-error / quantizer elasticity -------------------------
    slopes = []
    for (image, size), pts in cells.items():
        lx, ly = [], []
        for q, score, qindex, dm in pts:
            if qindex > 0 and dm > 0:
                lx.append(math.log(qindex))
                ly.append(math.log(dm))
        if len(lx) >= 5:
            _, s, _, _ = linfit(lx, ly)
            if math.isfinite(s):
                slopes.append(s)
    if slopes:
        slopes.sort()
        print(
            f"\n# --- d ln(diffmap mean) / d ln(qindex), per cell ---\n"
            f"# median {statistics.median(slopes):.3f}  p25 {slopes[int(0.25*(len(slopes)-1))]:.3f}  "
            f"p75 {slopes[int(0.75*(len(slopes)-1))]:.3f}  n={len(slopes)}\n"
            "# INDICATIVE ONLY: qindex is not the dequant step, so this is not a\n"
            "# fit of the loop's spatial `strength` (= 1/gamma). It says the error\n"
            "# signal really does grow as a power of the quantizer, which is the\n"
            "# assumption the derivation rests on."
        )

    # ---- 5. Rust emission ------------------------------------------------
    if emit_rust:
        print("\n// --- paste into src/two_pass_zensim.rs ---")
        print(f"const ANCHOR_SCORE: [f32; {len(KNOTS)}] = [")
        print("    " + ", ".join(f"{k:.1f}" for k in KNOTS) + ",")
        print("];")
        print(f"const ANCHOR_QUALITY: [f32; {len(mono)}] = [")
        print("    " + ", ".join(f"{v:.3f}" for v in mono) + ",")
        print("];")


if __name__ == "__main__":
    main()

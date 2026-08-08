#!/usr/bin/env python3
"""Matched-wall-clock encode RD analysis — the reduction half.

Reads a `run_grid.py` cells TSV and answers the only question worth asking of
four encoders with four incomparable speed knobs:

    at the SAME measured encode time, which one needs fewer bytes to reach the
    same quality?

Comparing at nominal "speed 6" / "preset 6" / "cpu-used 6" is meaningless —
the ladders are not aligned, and on the validation grid here the same nominal
6 spans 10 ms to 228 ms across arms. So the ladder is treated as a free
parameter and eliminated:

  1. Per (image, arm, ladder) the rate sweep gives an RD curve. Interpolate it
     for `bytes @ target quality` and `time @ target quality`. A target outside
     the arm's ACHIEVED quality span at that ladder is NA — never extrapolated.
  2. Per (image, arm) that yields a set of points {(time, bytes)} indexed by
     ladder position: the arm's time-vs-bytes-at-fixed-quality frontier. Drop
     dominated points (a ladder rung both slower AND bigger than another rung
     is never the right choice).
  3. Compare two arms by interpolating log(bytes) against log(time) along each
     frontier, ONLY on the overlap of their measured time ranges. Outside the
     overlap the answer is "these ladders do not meet here", reported as such.

Also emits, because the sweep discipline requires it and because a ms/MP
number without an intercept is meaningless: the per-arm `t = alpha + beta*px`
fit across the size sweep, with alpha (fixed process+init overhead) and beta
(per-megapixel encode cost) reported separately.

    python3 analyze_matched.py cells.tsv [--metric ssim2_floor]
                               [--targets 40,55,70,85] [--pairs a:b,...]
"""

from __future__ import annotations

import argparse
import math
import sys
from collections import defaultdict


def read_cells(path: str) -> tuple[list[dict], list[str]]:
    rows, hdr, meta = [], None, []
    with open(path) as fh:
        for line in fh:
            if line.startswith("#"):
                meta.append(line.rstrip("\n"))
                continue
            f = line.rstrip("\n").split("\t")
            if hdr is None:
                hdr = f
                continue
            rows.append(dict(zip(hdr, f)))
    return rows, meta


def fnum(s):
    try:
        v = float(s)
        return v if math.isfinite(v) else None
    except (TypeError, ValueError):
        return None


def pareto_rd(pts: list[tuple[float, float, float]]) -> list[tuple[float, float, float]]:
    """Keep only RD-undominated (bytes, quality, time) points: drop any point
    that another point beats on BOTH size and quality. Encoder rate ladders
    are not always monotone, and a non-monotone kink turns into a fake
    interpolation crossing if it is left in."""
    out = []
    for p in pts:
        if not any(q[0] <= p[0] and q[1] >= p[1] and q != p for q in pts):
            out.append(p)
    return sorted(out, key=lambda x: x[1])


def interp_at_quality(pts, target):
    """(bytes, time) at `target` quality, or None if outside the span.

    Interpolates in log-space for both: bytes-vs-quality is close to
    exponential over any short quality interval, and so is time. Linear
    interpolation of raw bytes across a 2x gap biases high.
    """
    pts = sorted(pts, key=lambda x: x[1])
    if not pts or target < pts[0][1] or target > pts[-1][1]:
        return None
    for i in range(len(pts) - 1):
        b0, q0, t0 = pts[i]
        b1, q1, t1 = pts[i + 1]
        if q0 <= target <= q1:
            if q1 == q0:
                return b0, t0
            f = (target - q0) / (q1 - q0)
            lb = math.log(b0) + f * (math.log(b1) - math.log(b0))
            lt = (math.log(max(t0, 1e-6)) +
                  f * (math.log(max(t1, 1e-6)) - math.log(max(t0, 1e-6))))
            return math.exp(lb), math.exp(lt)
    return None


def pareto_time_bytes(pts: list[tuple[float, float, int]]):
    """Keep only (time, bytes) points not dominated on both axes."""
    out = []
    for p in pts:
        if not any(q[0] <= p[0] and q[1] <= p[1] and q != p for q in pts):
            out.append(p)
    return sorted(out)


def interp_bytes_at_time(frontier, t):
    """log-log interpolation of bytes at time `t`; None outside the range.
    Never extrapolates — an encoder's ladder simply ends."""
    if not frontier:
        return None
    ts = [p[0] for p in frontier]
    if t < ts[0] or t > ts[-1]:
        return None
    for i in range(len(frontier) - 1):
        t0, b0 = frontier[i][0], frontier[i][1]
        t1, b1 = frontier[i + 1][0], frontier[i + 1][1]
        if t0 <= t <= t1:
            if t1 == t0:
                return b0
            f = (math.log(t) - math.log(t0)) / (math.log(t1) - math.log(t0))
            return math.exp(math.log(b0) + f * (math.log(b1) - math.log(b0)))
    return None


def linfit(xs, ys):
    """Ordinary least squares y = a + b*x, returning (a, b, r2). Used for the
    t = alpha + beta*pixels fit the sweep discipline requires."""
    n = len(xs)
    if n < 2:
        return None, None, None
    mx, my = sum(xs) / n, sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    if sxx == 0:
        return None, None, None
    b = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sxx
    a = my - b * mx
    ss_tot = sum((y - my) ** 2 for y in ys)
    ss_res = sum((y - (a + b * x)) ** 2 for x, y in zip(xs, ys))
    r2 = 1 - ss_res / ss_tot if ss_tot > 0 else 1.0
    return a, b, r2


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cells")
    ap.add_argument("--metric", default="ssim2_floor",
                    help="quality column; *_floor isolates the encoder (ceiling 100), "
                         "*_ref includes the fixed 4:2:0 round-trip cost")
    ap.add_argument("--targets", default="40,55,70,85",
                    help="quality targets; keep low-q density >= high-q density")
    ap.add_argument("--pairs", default="",
                    help="armA:armB[,...]; default = every pair present")
    ap.add_argument("--bytes-col", default="bytes_av1")
    args = ap.parse_args()

    rows, meta = read_cells(args.cells)
    targets = [float(t) for t in args.targets.split(",")]

    ok = [r for r in rows if not r.get("fail") and fnum(r.get(args.metric)) is not None
          and fnum(r.get("wall_ms_med")) is not None and fnum(r.get(args.bytes_col))]
    print("\n".join(meta))
    print(f"\n=== input: {len(rows)} cells, {len(ok)} usable "
          f"(metric={args.metric}, bytes={args.bytes_col}) ===")
    if not ok:
        print("nothing usable — check the fail column")
        return 1

    arms = sorted({r["arm"] for r in ok})
    images = sorted({(r["image"], r["size_tag"]) for r in ok})

    # ------------------------------------------------------ determinism -----
    # run_grid fails a cell the moment two reps of the SAME config produce
    # different byte counts, so a clean run is itself the determinism proof.
    # Stating it explicitly beats leaving it implicit in "0 failed".
    nd = [r for r in rows if "nondetermin" in (r.get("fail") or "")]
    tot_reps = sum(int(r["n_reps_kept"]) for r in ok if r.get("n_reps_kept", "").isdigit())
    print("\n=== A0. byte determinism (same arm, same config, repeated) ===")
    print(f"  {len(ok)} cells re-encoded {tot_reps} times total; "
          f"{len(nd)} produced differing byte counts across reps.")
    if nd:
        for r in nd[:10]:
            print(f"    NONDETERMINISTIC {r['arm']} {r['image']}@{r['size_tag']} "
                  f"{r['ladder_knob']}={r['ladder']} {r['rate_knob']}={r['rate']}: {r['fail']}")
    else:
        print("  Every cell was byte-identical on every repeat, all arms. Bytes are "
              "deterministic at 1 thread / 1 tile; only the clock varies.")

    # Cross-arm byte identity: two arms landing on the same payload size at the
    # same (image, size, ladder, rate) across a whole grid is not a coincidence.
    print("\n=== A1. cross-arm byte agreement (same input, same nominal config) ===")
    keyed = defaultdict(dict)
    for r in ok:
        keyed[(r["image"], r["size_tag"], r["ladder"], r["rate"])][r["arm"]] = r[args.bytes_col]
    for i, a in enumerate(arms):
        for b in arms[i + 1:]:
            both = [(v[a], v[b]) for v in keyed.values() if a in v and b in v]
            if not both:
                continue
            same = sum(1 for x, y in both if x == y)
            flag = "  <-- identical bitstream size on every shared cell" if same == len(both) else ""
            print(f"  {a:<10} vs {b:<10} {same}/{len(both)} cells equal in {args.bytes_col}{flag}")

    # ---------------------------------------------------------- reprod ------
    print("\n=== A. instrument reproducibility (timing spread across reps) ===")
    print(f"{'arm':<10}{'cells':>6}{'reps_med':>10}{'spread_med%':>13}"
          f"{'spread_p90%':>13}{'spread_max%':>13}{'foreign_max%':>14}{'foreign_cores':>15}")
    for a in arms:
        sp = sorted(fnum(r["wall_spread_pct"]) for r in ok
                    if r["arm"] == a and fnum(r["wall_spread_pct"]) is not None)
        reps = sorted(int(r["n_reps_kept"]) for r in ok if r["arm"] == a)
        fg = max((fnum(r["foreign_cpu_pct"]) or 0) for r in ok if r["arm"] == a)
        fc = max((fnum(r.get("foreign_cores")) or 0) for r in ok if r["arm"] == a)
        n = len(sp)
        if not n:
            continue
        print(f"{a:<10}{n:>6}{reps[len(reps)//2]:>10}"
              f"{sp[n//2]:>13.2f}{sp[min(n-1,int(n*0.9))]:>13.2f}{sp[-1]:>13.2f}{fg:>14.1f}{fc:>15.2f}")

    # Spread is dominated by fixed jitter (process spawn, page faults), so it
    # is a much larger FRACTION of a 4 ms cell than of a 4 s one. Reporting one
    # pooled number hides that and makes the instrument look worse (or better)
    # than it is at the size you actually care about.
    print("\n    spread by size — jitter is roughly constant in ms, so it shrinks as a %:")
    szs = sorted({r["size_tag"] for r in ok}, key=lambda s: int(s) if s.isdigit() else 1 << 30)
    print(f"    {'arm':<10}" + "".join(f"{s+'px':>22}" for s in szs))
    print(f"    {'':<10}" + "".join(f"{'med% / med_ms':>22}" for _ in szs))
    for a in arms:
        cells_out = []
        for s in szs:
            g = [r for r in ok if r["arm"] == a and r["size_tag"] == s]
            sp = sorted(x for x in (fnum(r["wall_spread_pct"]) for r in g) if x is not None)
            wm = sorted(x for x in (fnum(r["wall_ms_med"]) for r in g) if x is not None)
            cells_out.append(f"{sp[len(sp)//2]:>10.2f} /{wm[len(wm)//2]:>9.1f}" if sp else f"{'-':>22}")
        print(f"    {a:<10}" + "".join(cells_out))

    # ---------------------------------------------------------- span --------
    print(f"\n=== B. achieved-quality span per (arm, ladder) — an arm can only be "
          f"compared inside its own span ===")
    print(f"{'arm':<10}{'ladder':>7}{'cells':>6}{'q_min':>8}{'q_max':>8}"
          f"{'bytes_min':>10}{'bytes_max':>10}{'wall_ms_med':>12}")
    ladders = defaultdict(list)
    for r in ok:
        ladders[(r["arm"], int(r["ladder"]))].append(r)
    for (a, l) in sorted(ladders):
        g = ladders[(a, l)]
        qs = sorted(fnum(r[args.metric]) for r in g)
        bs = sorted(int(float(r[args.bytes_col])) for r in g)
        ws = sorted(fnum(r["wall_ms_med"]) for r in g)
        print(f"{a:<10}{l:>7}{len(g):>6}{qs[0]:>8.2f}{qs[-1]:>8.2f}"
              f"{bs[0]:>10}{bs[-1]:>10}{ws[len(ws)//2]:>12.2f}")

    # ------------------------------------------------- alpha/beta fit -------
    print("\n=== C. time vs image size ===")
    sizes_present = {r["size_tag"] for r in ok}
    szs2 = sorted(sizes_present, key=lambda s: int(s) if s.isdigit() else 1 << 30)
    if len(szs2) >= 2:
        # The direct table first. It is the honest primary: per-pixel encode
        # cost is NOT constant across sizes, so any single ms/MP number is
        # wrong at every size but one.
        print("    C1. measured per-size cost (median over the rate grid). "
              "ms/MP is NOT constant — that is the point:")
        print(f"    {'arm':<10}{'ladder':>7}" + "".join(f"{s+'px ms':>12}" for s in szs2)
              + "".join(f"{s+'px ms/MP':>13}" for s in szs2))
        for (a, l) in sorted(ladders):
            g = ladders[(a, l)]
            row, rowr = [], []
            for s in szs2:
                v = sorted(fnum(r["wall_ms_med"]) for r in g if r["size_tag"] == s)
                px = next((float(r["px"]) for r in g if r["size_tag"] == s), None)
                if not v or not px:
                    row.append(f"{'-':>12}")
                    rowr.append(f"{'-':>13}")
                    continue
                m = v[len(v) // 2]
                row.append(f"{m:>12.2f}")
                rowr.append(f"{m / (px / 1e6):>13.0f}")
            print(f"    {a:<10}{l:>7}" + "".join(row) + "".join(rowr))
    print("\n    C2. size model, fit PER IMAGE then aggregated across images")
    print("    t_ms = alpha + beta * megapixels, one fit per (arm, ladder, image).")
    print("    Fitting per image is not a nicety: two DIFFERENT images at the same")
    print("    size_tag have slightly different pixel counts (graph@64 = 2432 px,")
    print("    codec_wiki@64 = 2560 px), so pooling them puts two near-identical x")
    print("    values with different content into one regression and the slope explodes.")
    if len(sizes_present) < 2:
        print(f"    SKIPPED: only one size present ({sorted(sizes_present)}). "
              f"The fit needs >= 2 sizes; >= 3 to judge linearity.")
    else:
        print("    exponent = slope of log(t) vs log(px). 1.0 = linear in pixels; <1 means")
        print("    per-pixel cost FALLS with size, which is what C1 shows and what pushes")
        print("    the straight-line alpha down (negative, in the worst case).")
        print(f"{'arm':<10}{'ladder':>7}{'imgs':>5}{'alpha_ms':>10}{'beta_ms/MP':>12}"
              f"{'r2':>8}{'exponent':>10}{'a<0':>5}  note")
        imgs_all = sorted({r["image"] for r in ok})
        local_alpha = defaultdict(list)
        for (a, l) in sorted(ladders):
            g = ladders[(a, l)]
            als, bes, r2s, exps, neg = [], [], [], [], 0
            for img in imgs_all:
                by_px = defaultdict(list)
                for r in g:
                    if r["image"] == img:
                        by_px[float(r["px"]) / 1e6].append(fnum(r["wall_ms_med"]))
                if len(by_px) < 2:
                    continue
                pts = sorted(by_px.items())
                xs = [k for k, _ in pts]
                ys = [sorted(v)[len(v) // 2] for _, v in pts]
                al, be, r2 = linfit(xs, ys)
                if al is None:
                    continue
                _, expo, _ = linfit([math.log(x) for x in xs], [math.log(y) for y in ys])
                als.append(al); bes.append(be); r2s.append(r2)
                if expo is not None:
                    exps.append(expo)
                neg += (al < 0)
                # Local alpha from THIS image's two smallest sizes.
                al2, be2, _ = linfit(xs[:2], ys[:2])
                if al2 is not None:
                    local_alpha[(a, l)].append(al2)
            if not als:
                continue
            med = lambda v: sorted(v)[len(v) // 2]
            note = ""
            if neg:
                note = f"{neg}/{len(als)} images give alpha<0: linear model rejected there"
            elif exps and med(exps) < 0.92:
                note = "strongly sub-linear; alpha absorbs small-size inefficiency too"
            print(f"{a:<10}{l:>7}{len(als):>5}{med(als):>10.2f}{med(bes):>12.1f}"
                  f"{med(r2s):>8.4f}{med(exps) if exps else float('nan'):>10.3f}"
                  f"{neg:>5}  {note}")

        print("\n    C3. local alpha from each image's two SMALLEST sizes, median over images.")
        print("    Over a short pixel range the curve is near-linear, so this is the usable")
        print("    estimate of genuine fixed cost (spawn + init + input parse + write):")
        print(f"{'arm':<10}{'ladder':>7}{'imgs':>5}{'alpha_ms':>10}")
        for (a, l) in sorted(local_alpha):
            v = sorted(local_alpha[(a, l)])
            print(f"{a:<10}{l:>7}{len(v):>5}{v[len(v) // 2]:>10.2f}")

    # ------------------------------------- time-vs-bytes-at-quality ---------
    # frontier[(image,size,arm)][target] = [(time_ms, bytes, ladder), ...]
    frontiers = defaultdict(lambda: defaultdict(list))
    for (img, sz) in images:
        for a in arms:
            for l in sorted({int(r["ladder"]) for r in ok
                             if r["arm"] == a and r["image"] == img and r["size_tag"] == sz}):
                pts = [(float(r[args.bytes_col]), fnum(r[args.metric]), fnum(r["wall_ms_med"]))
                       for r in ok if r["arm"] == a and r["image"] == img
                       and r["size_tag"] == sz and int(r["ladder"]) == l]
                pts = pareto_rd(pts)
                for tq in targets:
                    got = interp_at_quality(pts, tq)
                    if got:
                        frontiers[(img, sz, a)][tq].append((got[1], got[0], l))

    print(f"\n=== D. time-vs-bytes frontier at fixed quality "
          f"(ladder eliminated; dominated rungs dropped) ===")
    for (img, sz) in images:
        for tq in targets:
            any_row = False
            lines = []
            for a in arms:
                fr = pareto_time_bytes(frontiers[(img, sz, a)].get(tq, []))
                if not fr:
                    continue
                any_row = True
                rungs = ",".join(str(p[2]) for p in fr)
                lines.append(f"    {a:<10} t=[{fr[0][0]:8.1f},{fr[-1][0]:9.1f}] ms  "
                             f"bytes=[{fr[-1][1]:7.0f},{fr[0][1]:8.0f}]  rungs({len(fr)}): {rungs}")
            if any_row:
                print(f"  {img} @{sz} target {args.metric}={tq:g}")
                print("\n".join(lines))

    # ------------------------------------------- matched-time compare -------
    pairs = ([tuple(p.split(":")) for p in args.pairs.split(",") if ":" in p]
             or [(a, b) for i, a in enumerate(arms) for b in arms[i + 1:]])

    print(f"\n=== E. MATCHED WALL-CLOCK comparison — bytes ratio A/B at equal measured time ===")
    print("    <1.00 means A is smaller (better) than B at that time budget.")
    print("    Only the OVERLAP of the two arms' measured time ranges is reported;")
    print("    outside it the ladders do not meet and no number is invented.")
    for (A, B) in pairs:
        if A not in arms or B not in arms:
            continue
        print(f"\n  -- {A} vs {B} --")
        for (img, sz) in images:
            for tq in targets:
                fa = pareto_time_bytes(frontiers[(img, sz, A)].get(tq, []))
                fb = pareto_time_bytes(frontiers[(img, sz, B)].get(tq, []))
                if len(fa) < 2 or len(fb) < 2:
                    if fa or fb:
                        why = (f"{A}:{len(fa)} rung(s), {B}:{len(fb)} rung(s)"
                               " — need >=2 each to interpolate")
                        print(f"    {img} @{sz} q={tq:g}: NO COMPARISON ({why})")
                    continue
                lo = max(fa[0][0], fb[0][0])
                hi = min(fa[-1][0], fb[-1][0])
                if lo >= hi:
                    print(f"    {img} @{sz} q={tq:g}: LADDERS DO NOT OVERLAP IN TIME "
                          f"({A} {fa[0][0]:.1f}-{fa[-1][0]:.1f} ms vs "
                          f"{B} {fb[0][0]:.1f}-{fb[-1][0]:.1f} ms)")
                    continue
                # Geometric grid across the overlap: time spans decades.
                ks = 5
                pts = [lo * (hi / lo) ** (i / (ks - 1)) for i in range(ks)]
                cells_out = []
                for t in pts:
                    ba, bb = interp_bytes_at_time(fa, t), interp_bytes_at_time(fb, t)
                    if ba and bb:
                        cells_out.append(f"{t:7.1f}ms {ba / bb:5.3f}")
                if cells_out:
                    print(f"    {img} @{sz} q={tq:g}  overlap {lo:.1f}-{hi:.1f} ms:  "
                          + "  ".join(cells_out))

    # ------------------------------------------- aggregate across images ----
    # Ratios aggregate geometrically, not arithmetically: a pair of cells at
    # 0.5 and 2.0 is neutral overall, and an arithmetic mean would call it
    # +25%. Grouped by content class as well as size, because averaging a
    # screen-content result into a photo median is how a real per-class effect
    # gets buried.
    cls_of = {}
    for r in ok:
        cls_of[(r["image"], r["size_tag"])] = r["content_class"]
    print("\n=== E2. aggregate over images: geometric-mean bytes ratio A/B "
          "at the midpoint of each overlap ===")
    for (A, B) in pairs:
        if A not in arms or B not in arms:
            continue
        printed = False
        for tq in targets:
            per_cls = defaultdict(list)
            for (img, sz) in images:
                fa = pareto_time_bytes(frontiers[(img, sz, A)].get(tq, []))
                fb = pareto_time_bytes(frontiers[(img, sz, B)].get(tq, []))
                if len(fa) < 2 or len(fb) < 2:
                    continue
                lo, hi = max(fa[0][0], fb[0][0]), min(fa[-1][0], fb[-1][0])
                if lo >= hi:
                    continue
                t = math.sqrt(lo * hi)
                ba, bb = interp_bytes_at_time(fa, t), interp_bytes_at_time(fb, t)
                if ba and bb:
                    per_cls[(cls_of.get((img, sz), "?"), sz)].append(ba / bb)
            for k in sorted(per_cls):
                v = per_cls[k]
                gm = math.exp(sum(math.log(x) for x in v) / len(v))
                if not printed:
                    print(f"\n  -- {A} vs {B} --")
                    printed = True
                print(f"    q={tq:<5g} {k[0]:<10} @{k[1]:<6} n={len(v)}  ratio {gm:.3f}"
                      f"  ({'A smaller' if gm < 1 else 'B smaller'} by "
                      f"{abs(1 - gm) * 100:.1f}%)")
        if not printed:
            print(f"\n  -- {A} vs {B} -- no overlapping cell at any target")

    # ------------------------------------------------------- cost model -----
    # Sizing the full sweep BEFORE launching it, from measured per-cell cost
    # rather than a guess. Encode cost is what dominates; scoring measured at
    # 1 ms/variant at 64px and 7 ms/variant at 256px is noise beside it.
    print("\n=== G. measured per-cell cost, and what a full sweep would cost ===")
    print("    per-cell encode cost = median wall x reps. Aggregated per arm and size:")
    tot_s = 0.0
    per_arm_size = defaultdict(float)
    for r in ok:
        w = fnum(r["wall_ms_med"])
        n = int(r["n_reps_kept"]) if r["n_reps_kept"].isdigit() else 0
        if w:
            tot_s += w * max(n, 1) / 1000.0
            per_arm_size[(r["arm"], r["size_tag"])] += w * max(n, 1) / 1000.0
    print(f"    {'arm':<10}" + "".join(f"{s+'px':>12}" for s in szs2) + f"{'total_s':>12}")
    for a in arms:
        row = [f"{per_arm_size.get((a, s), 0.0):>12.1f}" for s in szs2]
        print(f"    {a:<10}" + "".join(row)
              + f"{sum(per_arm_size.get((a, s), 0.0) for s in szs2):>12.1f}")
    n_enc = sum(int(r["n_reps_kept"]) for r in ok if r["n_reps_kept"].isdigit())
    print(f"    THIS GRID: {len(ok)} cells, {n_enc} encodes, {tot_s:.0f}s of encode "
          f"({tot_s / max(n_enc, 1) * 1000:.0f} ms/encode mean)")

    # Mean cost per (arm, ladder, size) cell, for projecting a bigger grid.
    print("\n    Projection basis — mean seconds per CELL (one cell = reps encodes):")
    print(f"    {'arm':<10}{'ladder':>7}" + "".join(f"{s+'px s/cell':>14}" for s in szs2))
    basis = {}
    for (a, l) in sorted(ladders):
        row = []
        for s in szs2:
            g = [r for r in ladders[(a, l)] if r["size_tag"] == s]
            v = [fnum(r["wall_ms_med"]) * int(r["n_reps_kept"]) / 1000.0
                 for r in g if fnum(r["wall_ms_med"]) and r["n_reps_kept"].isdigit()]
            if v:
                basis[(a, l, s)] = sum(v) / len(v)
                row.append(f"{basis[(a, l, s)]:>14.2f}")
            else:
                row.append(f"{'-':>14}")
        print(f"    {a:<10}{l:>7}" + "".join(row))
    print("    Multiply by (images x rates x ladder rungs) for the full grid. Ladder rungs")
    print("    NOT measured here (0-3, 9+) are unknown and are NOT extrapolated: the slow")
    print("    end is where cost explodes, so bound it with a probe before committing.")

    print("\n=== Z. caveats that travel with every number above ===")
    print("  * time = FULL PROCESS WALL CLOCK. Column C's alpha is how much of it is")
    print("    not encoding. Where alpha is a large fraction, the comparison is")
    print("    overhead-contaminated — read C before quoting E.")
    print("  * the svtrs arm additionally decodes a PNG inside its timed region;")
    print("    the y4m arms do not. That sits in its alpha.")
    print("  * single-thread, single-tile. Thread scaling is a separate axis and")
    print("    tiling changes the bitstream, so neither is folded in here.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

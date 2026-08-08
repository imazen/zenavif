#!/usr/bin/env python3
"""The BYTE-axis size model: bytes = alpha + beta * pixels.

`analyze_matched.py` section C fits the same model on the TIME axis. The sweep
discipline asks for both, and for the same reason: a bpp figure without the
intercept is meaningless. A 1 KB fixed header is +0.4 bpp on a 64x64 thumbnail
and ~0 bpp at 4K, so a bitrate model that does not separate
`header_bytes + content_bpp * pixels` will be wrong at one end or the other.

Two fits, because they answer different questions and only one of them is a
clean intercept estimate:

  * FIXED QUANTIZER (the primary). Hold the arm's rate knob constant and vary
    only the pixel count. Nothing else moves, so the intercept is genuinely the
    per-file fixed cost of the bitstream — sequence header, frame header,
    tile-group headers, trailing bits.
  * FIXED QUALITY (secondary). Interpolate each size's RD curve to a common
    achieved quality first. This is the product-relevant version, but it is
    confounded: downscaling changes an image's intrinsic bits-per-pixel, so the
    slope mixes "cost per pixel" with "the small version is a different image".
    Reported, and labelled, rather than passed off as the same measurement.

Also reports the thing the fit exists to expose — the measured bpp inflation on
small images — directly, without a model in the way.

    python3 bytes_model.py cells.tsv [--metric ssim2_floor] [--quality 50,70]
"""

from __future__ import annotations

import argparse
import math
import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from analyze_matched import (  # noqa: E402
    fnum, interp_at_quality, linfit, pareto_rd, read_cells,
)


def med(v):
    v = sorted(v)
    return v[len(v) // 2]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cells")
    ap.add_argument("--metric", default="ssim2_floor")
    ap.add_argument("--bytes-col", default="bytes_av1")
    ap.add_argument("--quality", default="30,50,70,85")
    args = ap.parse_args()

    rows, meta = read_cells(args.cells)
    ok = [r for r in rows if not r.get("fail") and fnum(r.get(args.bytes_col))
          and fnum(r.get("px"))]
    print("\n".join(meta))
    print(f"\n=== BYTE-AXIS SIZE MODEL: bytes = alpha + beta * pixels ===")
    print(f"{len(ok)} usable cells, bytes column {args.bytes_col} "
          f"(AV1 payload, container stripped)")

    szs = sorted({r["size_tag"] for r in ok},
                 key=lambda s: int(s) if s.isdigit() else 1 << 30)
    arms = sorted({r["arm"] for r in ok})

    # ------------------------------------------------------------ B1 --------
    print("\n=== B1. measured bpp at the SAME quantizer, by size. No model.")
    print("    If the fixed cost were zero these rows would be flat; the rise at")
    print("    small sizes IS the intercept, before any line is fitted. ===")
    print(f"    {'arm':<10}{'rung':>5}{'rate':>6}" + "".join(f"{s+'px bpp':>13}" for s in szs)
          + "".join(f"{s+'px B':>11}" for s in szs))
    shown = 0
    for a in arms:
        # one representative mid-ladder rung and one mid rate, per arm
        rungs = sorted({int(r["ladder"]) for r in ok if r["arm"] == a})
        rates = sorted({int(r["rate"]) for r in ok if r["arm"] == a})
        if not rungs or not rates:
            continue
        lad = rungs[len(rungs) // 2]
        for rate in (rates[len(rates) // 4], rates[len(rates) // 2],
                     rates[3 * len(rates) // 4]):
            bpps, bys = [], []
            for s in szs:
                g = [r for r in ok if r["arm"] == a and int(r["ladder"]) == lad
                     and int(r["rate"]) == rate and r["size_tag"] == s]
                if g:
                    bpps.append(f"{med([float(r[args.bytes_col]) * 8 / float(r['px']) for r in g]):>13.4f}")
                    bys.append(f"{med([float(r[args.bytes_col]) for r in g]):>11.0f}")
                else:
                    bpps.append(f"{'-':>13}")
                    bys.append(f"{'-':>11}")
            print(f"    {a:<10}{lad:>5}{rate:>6}" + "".join(bpps) + "".join(bys))
            shown += 1
    if not shown:
        print("    (no cell had two sizes at the same rung and rate)")

    # ------------------------------------------------------------ B2 --------
    print("\n=== B2. fit at FIXED QUANTIZER — the clean intercept.")
    print("    One fit per (arm, rung, rate, image) across that image's sizes, then")
    print("    the median over images and rates. alpha is in BYTES: the per-file cost")
    print("    that does not scale with area. Per image, never pooled — two images at")
    print("    the same size_tag have different pixel counts and pooling them makes")
    print("    the slope explode (see analyze_matched.py C2 for the same trap). ===")
    print(f"    {'arm':<10}{'rung':>5}{'fits':>6}{'alpha_B':>10}{'beta_B/MP':>12}"
          f"{'r2':>8}{'exponent':>10}{'a<0':>5}")
    for a in arms:
        for lad in sorted({int(r["ladder"]) for r in ok if r["arm"] == a}):
            als, bes, r2s, exps, neg = [], [], [], [], 0
            for img in sorted({r["image"] for r in ok}):
                for rate in sorted({int(r["rate"]) for r in ok if r["arm"] == a}):
                    pts = {}
                    for r in ok:
                        if (r["arm"] == a and int(r["ladder"]) == lad
                                and r["image"] == img and int(r["rate"]) == rate):
                            pts[float(r["px"]) / 1e6] = float(r[args.bytes_col])
                    if len(pts) < 2:
                        continue
                    xs = sorted(pts)
                    ys = [pts[x] for x in xs]
                    al, be, r2 = linfit(xs, ys)
                    if al is None:
                        continue
                    als.append(al); bes.append(be); r2s.append(r2); neg += (al < 0)
                    _, ex, _ = linfit([math.log(x) for x in xs],
                                      [math.log(max(y, 1e-9)) for y in ys])
                    if ex is not None:
                        exps.append(ex)
            if als:
                print(f"    {a:<10}{lad:>5}{len(als):>6}{med(als):>10.1f}{med(bes):>12.0f}"
                      f"{med(r2s):>8.4f}{med(exps) if exps else float('nan'):>10.3f}{neg:>5}")

    # ------------------------------------------------------------ B3 --------
    print("\n=== B3. fit at FIXED ACHIEVED QUALITY — product-relevant, confounded.")
    print("    Each (image, size, arm, rung) RD curve is interpolated to a common")
    print("    quality first. CONFOUND: downscaling changes an image's intrinsic")
    print("    bits-per-pixel, so beta here is not purely 'cost per pixel'. ===")
    qs = [float(x) for x in args.quality.split(",")]
    byq = defaultdict(dict)     # (arm,lad,img,q) -> {px: bytes}
    for a in arms:
        for lad in sorted({int(r["ladder"]) for r in ok if r["arm"] == a}):
            for img in sorted({r["image"] for r in ok}):
                for s in szs:
                    g = [r for r in ok if r["arm"] == a and int(r["ladder"]) == lad
                         and r["image"] == img and r["size_tag"] == s
                         and fnum(r.get(args.metric)) is not None
                         and fnum(r.get("wall_ms_med")) is not None]
                    if len(g) < 2:
                        continue
                    pts = pareto_rd([(float(r[args.bytes_col]), fnum(r[args.metric]),
                                      fnum(r["wall_ms_med"])) for r in g])
                    px = float(g[0]["px"]) / 1e6
                    for q in qs:
                        got = interp_at_quality(pts, q)
                        if got:
                            byq[(a, lad, img, q)][px] = got[0]
    print(f"    {'arm':<10}{'q':>5}{'fits':>6}{'alpha_B':>10}{'beta_B/MP':>12}{'r2':>8}{'a<0':>5}")
    for a in arms:
        for q in qs:
            als, bes, r2s, neg = [], [], [], 0
            for k, pts in byq.items():
                if k[0] != a or k[3] != q or len(pts) < 2:
                    continue
                xs = sorted(pts)
                al, be, r2 = linfit(xs, [pts[x] for x in xs])
                if al is None:
                    continue
                als.append(al); bes.append(be); r2s.append(r2); neg += (al < 0)
            if als:
                print(f"    {a:<10}{q:>5g}{len(als):>6}{med(als):>10.1f}"
                      f"{med(bes):>12.0f}{med(r2s):>8.4f}{neg:>5}")

    # ------------------------------------------------------------ B4 --------
    print("\n=== B4. what the intercept costs in bpp, per size. This is the number")
    print("    the discipline is about: the same fixed bytes are a large bitrate at")
    print("    64 px and nothing at 1024. ===")
    print(f"    {'arm':<10}{'alpha_B':>10}" + "".join(f"{s+'px +bpp':>13}" for s in szs))
    for a in arms:
        als = []
        for lad in sorted({int(r["ladder"]) for r in ok if r["arm"] == a}):
            for img in sorted({r["image"] for r in ok}):
                for rate in sorted({int(r["rate"]) for r in ok if r["arm"] == a}):
                    pts = {}
                    for r in ok:
                        if (r["arm"] == a and int(r["ladder"]) == lad
                                and r["image"] == img and int(r["rate"]) == rate):
                            pts[float(r["px"]) / 1e6] = float(r[args.bytes_col])
                    if len(pts) >= 2:
                        xs = sorted(pts)
                        al, _, _ = linfit(xs, [pts[x] for x in xs])
                        if al is not None:
                            als.append(al)
        if not als:
            continue
        al = med(als)
        row = []
        for s in szs:
            px = med([float(r["px"]) for r in ok if r["size_tag"] == s])
            row.append(f"{al * 8 / px:>13.4f}")
        print(f"    {a:<10}{al:>10.1f}" + "".join(row))
    # ------------------------------------------------------------ B5 --------
    # B2's alpha is NOT a header cost when the size span is wide. At fixed
    # quantizer, bytes are strongly sub-linear in pixels (exponent ~0.73-0.78
    # measured), so a single straight line over three decades of pixel count
    # cannot fit, and least squares dumps the curvature into the intercept —
    # producing a "fixed cost" of several kilobytes, which is nonsense for an
    # AV1 still and is nearly identical across all four arms precisely because
    # it is an artifact of the shared curve shape rather than an encoder
    # property. analyze_matched.py C2/C3 flag the same trap on the time axis;
    # this is its byte-axis counterpart.
    print("\n=== B5. the fixed cost done HONESTLY: a local fit over the two smallest")
    print("    sizes, and a hard empirical bound. Prefer these to B2's intercept")
    print("    whenever the exponent above is far from 1.0. ===")
    print(f"    {'arm':<10}{'rung':>5}{'local_alpha_B':>15}{'min_payload_B':>15}"
          f"{'  (min = smallest bytes_av1 anywhere in the grid)':<0}")
    for a in arms:
        for lad in sorted({int(r["ladder"]) for r in ok if r["arm"] == a}):
            loc = []
            for img in sorted({r["image"] for r in ok}):
                for rate in sorted({int(r["rate"]) for r in ok if r["arm"] == a}):
                    pts = {}
                    for r in ok:
                        if (r["arm"] == a and int(r["ladder"]) == lad
                                and r["image"] == img and int(r["rate"]) == rate):
                            pts[float(r["px"]) / 1e6] = float(r[args.bytes_col])
                    if len(pts) < 2:
                        continue
                    xs = sorted(pts)[:2]
                    al, _, _ = linfit(xs, [pts[x] for x in xs])
                    if al is not None:
                        loc.append(al)
            mn = min((float(r[args.bytes_col]) for r in ok
                      if r["arm"] == a and int(r["ladder"]) == lad), default=None)
            if loc and mn is not None:
                print(f"    {a:<10}{lad:>5}{med(loc):>15.1f}{mn:>15.0f}")
    print("\n    The minimum observed payload is the only assumption-free statement")
    print("    available here: a whole still bitstream, headers included, fits in")
    print("    that many bytes, so the fixed part cannot exceed it.")

    print("\n    Container bytes are NOT in any of this: every figure is bytes_av1,")
    print("    the AV1 payload with the IVF wrapper stripped. An AVIF file adds its")
    print("    own ISOBMFF boxes on top — measured separately in the meta.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

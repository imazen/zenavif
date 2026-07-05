#!/usr/bin/env python3
"""S10 program scoreboard: zenavif arms vs zenjpeg anchors (docs/S10_PROGRAM.md).

Reads chain_s10.sh TSVs. For each zr arm TSV and each JPEG anchor config in
the jpeg TSV, builds per-image ssim2->bytes frontiers, interpolates bytes at
matched ssim2 targets, and reports the paired bytes ratio (zr/jpeg, <1 = we
win) per family and overall — plus the encode-ms ratio from the timing TSVs
(enc_int_ms: internal encoder-only, both sides).

Usage:
  analyze_s10.py --jpeg s10_jpeg_t26.tsv --arms mas_s10=s10_mas_s10_t26.tsv ... \
      [--timing-jpeg s10_tim_jpeg.tsv] [--timing arm=tsv ...] [--targets 50,60,70,80]
"""

import argparse
import collections
import csv
import math
import sys


def load_rows(path):
    with open(path) as f:
        return list(csv.DictReader(f, delimiter="\t"))


def frontier(points):
    """[(ssim2, bytes)] -> monotone hull sorted by bytes."""
    pts = sorted(points, key=lambda p: p[1])
    out, top = [], -1e18
    for s, b in pts:
        if s > top:
            out.append((s, b))
            top = s
    return out


def interp(fr, t):
    xs = [p[0] for p in fr]
    ys = [p[1] for p in fr]
    if not xs or t > xs[-1]:
        return None
    if t <= xs[0]:
        return ys[0]
    i = next(k for k, x in enumerate(xs) if x >= t)
    x0, x1, y0, y1 = xs[i - 1], xs[i], ys[i - 1], ys[i]
    if x1 == x0:
        return y1
    return math.exp(math.log(y0) + (math.log(y1) - math.log(y0)) * (t - x0) / (x1 - x0))


def zr_frontiers(rows):
    per = collections.defaultdict(list)
    fam = {}
    for r in rows:
        if r["encoder"] != "zenrav1e":
            continue
        try:
            s, b = float(r["ssim2"]), float(r["bytes"])
        except (ValueError, KeyError):
            continue
        per[r["image"]].append((s, b))
        fam[r["image"]] = r["family"]
    return {img: frontier(p) for img, p in per.items()}, fam


def jpeg_frontiers(rows):
    """-> {config: {image: frontier}}, plus 'best3' composite."""
    per = collections.defaultdict(lambda: collections.defaultdict(list))
    for r in rows:
        if r["encoder"] != "zenjpeg":
            continue
        try:
            s, b = float(r["ssim2"]), float(r["bytes"])
        except (ValueError, KeyError):
            continue
        per[r["fmt"]][r["image"]].append((s, b))
        per["best3"][r["image"]].append((s, b))
    return {cfg: {img: frontier(p) for img, p in d.items()} for cfg, d in per.items()}


def med(v):
    s = sorted(v)
    n = len(s)
    return s[n // 2] if n % 2 else 0.5 * (s[n // 2 - 1] + s[n // 2])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jpeg", required=True)
    ap.add_argument("--arms", nargs="+", metavar="NAME=TSV")
    ap.add_argument("--timing-jpeg", default=None)
    ap.add_argument("--timing", nargs="*", default=[], metavar="NAME=TSV")
    ap.add_argument("--targets", default="50,60,70,80")
    ap.add_argument("--jarms", default="jp3_t0_small_420,moz_tr14.75+dc_small_420,best3")
    args = ap.parse_args()
    targets = [float(t) for t in args.targets.split(",")]

    jf = jpeg_frontiers(load_rows(args.jpeg))

    # ---- timing: median internal ms per arm (solo pass) ----
    tim = {}
    if args.timing_jpeg:
        jr = load_rows(args.timing_jpeg)
        per = collections.defaultdict(list)
        for r in jr:
            if r.get("enc_int_ms", "NA") not in ("NA", ""):
                px = float(r["w"]) * float(r["h"]) / 1e6
                per[r["fmt"]].append(float(r["enc_int_ms"]) / px)
        for cfg, v in per.items():
            tim[f"jpeg:{cfg}"] = med(v)
    for spec in args.timing:
        name, path = spec.split("=", 1)
        v = []
        for r in load_rows(path):
            if r["encoder"] == "zenrav1e" and r.get("enc_int_ms", "NA") not in ("NA", ""):
                px = float(r["w"]) * float(r["h"]) / 1e6
                v.append(float(r["enc_int_ms"]) / px)
        if v:
            tim[name] = med(v)
    if tim:
        print("== solo internal encode ms/MP (median) ==")
        for k in sorted(tim, key=tim.get):
            print(f"  {k:28s} {tim[k]:9.1f}")
        jm = tim.get("jpeg:moz_tr14.75+dc_small_420")
        jd = tim.get("jpeg:jp3_t0_small_420")
        for k, v in sorted(tim.items(), key=lambda x: x[1]):
            if k.startswith("jpeg:"):
                continue
            r1 = f"{v / jm:6.1f}x moz" if jm else ""
            r2 = f"{v / jd:6.1f}x def" if jd else ""
            print(f"  {k:28s} {r1}  {r2}")
        print()

    # ---- bytes ratios at matched ssim2 ----
    jarms = args.jarms.split(",")
    print("== bytes ratio zr/jpeg at matched ssim2 (median [n]; <1 = zr smaller) ==")
    hdr = "arm\tjarm\tfamily\t" + "\t".join(f"ss{t:.0f}" for t in targets)
    print(hdr)
    for spec in args.arms:
        name, path = spec.split("=", 1)
        zf, fam = zr_frontiers(load_rows(path))
        for ja in jarms:
            jfa = jf.get(ja)
            if not jfa:
                continue
            byfam = {t: collections.defaultdict(list) for t in targets}
            unreach = collections.Counter()
            for img, fr in zf.items():
                jfr = jfa.get(img)
                if not jfr:
                    continue
                for t in targets:
                    a, j = interp(fr, t), interp(jfr, t)
                    if a and j:
                        byfam[t][fam[img]].append(a / j)
                    elif j and not a:
                        unreach[t] += 1
            fams = sorted({f for t in targets for f in byfam[t]})
            cells = []
            for t in targets:
                allv = [x for f in byfam[t].values() for x in f]
                cells.append(f"{med(allv):.3f}[{len(allv)}]" if allv else "NA")
            print(f"{name}\t{ja}\tALL\t" + "\t".join(cells))
            for f in fams:
                cells = []
                for t in targets:
                    v = byfam[t].get(f, [])
                    cells.append(f"{med(v):.3f}[{len(v)}]" if v else "NA")
                print(f"{name}\t{ja}\t{f}\t" + "\t".join(cells))
            if unreach:
                ur = " ".join(f"ss{t:.0f}:{c}" for t, c in sorted(unreach.items()))
                print(f"{name}\t{ja}\tUNREACHABLE\t{ur}")
    print()


if __name__ == "__main__":
    main()

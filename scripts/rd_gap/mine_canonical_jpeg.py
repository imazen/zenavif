#!/usr/bin/env python3
"""S10 program phase-1 breadth mine: registry zenavif per-speed frontier vs
zenjpeg frontiers on the canonical picker dataset (2026-06-27).

Reproduces + extends the coordinator's mine ("registry zenavif s8 beats
zenjpeg's best-of-all-configs frontier by 13-18% bytes at matched ssim2 50-80
at 17-37x encode time") with the per-speed trajectory s2/s4/s6/s8 and three
JPEG opponents:
  - jpeg_best : frontier over ALL 54 zenjpeg cells (the strongest opponent)
  - jpeg_moz  : moz_tr14.75+dc frontier over {420,422,444} (mozjpeg-class arm)
  - jpeg_def  : jp3_t0_small_420 (the shipped default stratum)

Method: per (image, arm) build the ssim2->bytes frontier (monotone upper
hull); at matched ssim2 targets interpolate bytes on both sides; report the
paired bytes ratio (avif/jpeg, <1 = avif smaller) and the encode_ms ratio
(avif/jpeg) per content_class and overall. The canonical dataset has NO
s9/s10 cells — that truth comes from the fresh train26 sweep; this mine
bounds the margin trajectory the s10 rebuild must not fall off.

Usage: mine_canonical_jpeg.py [--split train] [--out out.tsv]
Data:  /mnt/v/output/canonical-picker-2026-06-27/{zenavif,zenjpeg}_lossy/<split>.parquet
"""

import argparse
import collections
import sys

import numpy as np
import pyarrow.parquet as pq

BASE = "/mnt/v/output/canonical-picker-2026-06-27"
SSIM2_TARGETS = [50.0, 60.0, 70.0, 80.0]

COLS = [
    "origin_id", "variant_name", "cell", "q", "encoded_bytes", "encode_ms",
    "score_ssim2", "content_class", "size_class", "width", "height",
]


def load(codec, split):
    t = pq.read_table(f"{BASE}/{codec}_lossy/{split}.parquet", columns=COLS)
    return t.to_pydict()


def frontiers(d, arm_of):
    """-> {(variant, armname): sorted [(ssim2, bytes, enc_ms), ...] frontier}"""
    rows = collections.defaultdict(list)
    n = len(d["cell"])
    for i in range(n):
        arm = arm_of(d["cell"][i])
        if arm is None:
            continue
        s = d["score_ssim2"][i]
        b = d["encoded_bytes"][i]
        if s is None or b is None or not np.isfinite(s):
            continue
        key = (d["variant_name"][i], arm)
        rows[key].append((float(s), float(b), float(d["encode_ms"][i] or 0.0)))
    out = {}
    for k, pts in rows.items():
        pts.sort(key=lambda p: p[1])  # by bytes ascending
        best = []
        top = -1e9
        for s, b, ms in pts:  # monotone hull: keep points raising ssim2
            if s > top:
                best.append((s, b, ms))
                top = s
        out[k] = best
    return out


def interp_bytes(frontier, target):
    """bytes needed to reach ssim2 target on this frontier (None = unreachable)."""
    xs = [p[0] for p in frontier]
    ys = [p[1] for p in frontier]
    if not xs or target > xs[-1]:
        return None
    if target <= xs[0]:
        return ys[0]
    i = np.searchsorted(xs, target)
    x0, x1 = xs[i - 1], xs[i]
    y0, y1 = ys[i - 1], ys[i]
    if x1 == x0:
        return y1
    # interpolate in log-bytes (rate is ~exponential in quality)
    ly = np.log(y0) + (np.log(y1) - np.log(y0)) * (target - x0) / (x1 - x0)
    return float(np.exp(ly))


def mean_ms(frontier):
    ms = [p[2] for p in frontier if p[2] > 0]
    return float(np.mean(ms)) if ms else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", default="train")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    av = load("zenavif", args.split)
    jp = load("zenjpeg", args.split)

    # class per variant (same source corpus both datasets)
    cls = {}
    for i, v in enumerate(av["variant_name"]):
        cls[v] = av["content_class"][i]

    def avif_arm(cell):
        # registry zenavif per-speed frontier over its format axes (qm/noqm,
        # 420/444, bd8/bd10, rgb) — the strongest registry config per speed.
        return cell.split("-")[0]  # s2/s4/s6/s8

    def jpeg_best(cell):
        return "jpeg_best"

    def jpeg_moz(cell):
        return "jpeg_moz" if cell.startswith("moz_tr14.75+dc") else None

    def jpeg_def(cell):
        return "jpeg_def" if cell == "jp3_t0_small_420" else None

    fa = frontiers(av, avif_arm)
    fb = frontiers(jp, jpeg_best)
    fm = frontiers(jp, jpeg_moz)
    fd = frontiers(jp, jpeg_def)
    jf = {**fb, **fm, **fd}

    speeds = sorted({k[1] for k in fa})
    jarms = ["jpeg_best", "jpeg_moz", "jpeg_def"]

    out = open(args.out, "w") if args.out else sys.stdout
    print("speed\tjarm\tssim2\tclass\tn\tbytes_ratio_med\tbytes_ratio_p25\t"
          "bytes_ratio_p75\tms_ratio_med", file=out)

    for sp in speeds:
        for ja in jarms:
            # paired per variant
            per_t = {t: collections.defaultdict(list) for t in SSIM2_TARGETS}
            msr = collections.defaultdict(list)
            for (v, arm), fr in fa.items():
                if arm != sp:
                    continue
                jfr = jf.get((v, ja))
                if not jfr:
                    continue
                a_ms, j_ms = mean_ms(fr), mean_ms(jfr)
                if a_ms and j_ms:
                    msr[cls[v]].append(a_ms / j_ms)
                for t in SSIM2_TARGETS:
                    ab = interp_bytes(fr, t)
                    jb = interp_bytes(jfr, t)
                    if ab and jb:
                        per_t[t][cls[v]].append(ab / jb)
            def fmt_ms(v):
                return f"{np.median(v):.2f}" if v else "NA"

            for t in SSIM2_TARGETS:
                allr = [r for c in per_t[t].values() for r in c]
                if allr:
                    m = [r for c in msr.values() for r in c]
                    print(f"{sp}\t{ja}\t{t:.0f}\tALL\t{len(allr)}\t"
                          f"{np.median(allr):.4f}\t{np.percentile(allr, 25):.4f}\t"
                          f"{np.percentile(allr, 75):.4f}\t{fmt_ms(m)}", file=out)
                for c in sorted(per_t[t]):
                    rs = per_t[t][c]
                    if len(rs) >= 8:
                        mm = msr.get(c, [])
                        print(f"{sp}\t{ja}\t{t:.0f}\t{c}\t{len(rs)}\t"
                              f"{np.median(rs):.4f}\t{np.percentile(rs, 25):.4f}\t"
                              f"{np.percentile(rs, 75):.4f}\t{fmt_ms(mm)}", file=out)

    if args.out:
        out.close()


if __name__ == "__main__":
    main()

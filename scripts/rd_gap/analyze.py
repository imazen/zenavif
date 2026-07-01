#!/usr/bin/env python3
"""Compute the zenrav1e-vs-libaom RD gap from a unified rd_gap_results.tsv.

Input TSV columns (tab-separated):
  image  w  h  family  encoder  fmt  q  bytes  bpp  ssim2  enc_ms
where encoder in {zenrav1e, libaom}. Builds an RD frontier per (image, encoder),
reports the paired bpp gap at ssim2 {82,85,88,90} (median + mean, photos vs plots),
the integrated BD-rate, and the median-across-images frontier. Writes rd_gap_summary.csv
next to the input. + = zenrav1e needs MORE bits than libaom (worse).

Usage: analyze.py rd_gap_results.tsv
"""
import sys, csv, collections
import numpy as np

TARGETS = [82, 85, 88, 90]
PLOT_FAMILY = "7"  # 7000-lilith-plots = synthetic screen content (AVIF-hostile regardless)

def frontier(points):
    """points: [(ssim2,bpp)] -> monotone RD frontier (upper-left hull): min bpp per quality."""
    bybpp = sorted(points, key=lambda p: (p[1], -p[0]))
    front, best = [], -1e9
    for s, b in bybpp:
        if s > best:
            front.append((s, b)); best = s
    front.sort(key=lambda p: p[0])
    return front

def bpp_at(front, target):
    if len(front) < 2: return None
    ss = [p[0] for p in front]; bp = [p[1] for p in front]
    if target < ss[0] or target > ss[-1]: return None
    return float(np.interp(target, ss, bp))

def bd_rate(test_front, ref_front):
    """Bjontegaard delta-rate of test(zenrav1e) vs ref(libaom) over overlapping ssim2.
    + = test needs MORE bits. None if <4 pts or <2 ssim2 overlap."""
    def prep(f):
        seen = {}
        for s, b in f: seen[round(s, 4)] = np.log(b)
        xs = sorted(seen); return np.array(xs), np.array([seen[x] for x in xs])
    x1, y1 = prep(ref_front); x2, y2 = prep(test_front)
    if len(x1) < 4 or len(x2) < 4: return None
    lo, hi = max(x1.min(), x2.min()), min(x1.max(), x2.max())
    if hi - lo < 2: return None
    gg = np.linspace(lo, hi, 200)
    trapz = np.trapz if hasattr(np, "trapz") else np.trapezoid
    avg = (trapz(np.interp(gg, x2, y2), gg) - trapz(np.interp(gg, x1, y1), gg)) / (hi - lo)
    return (np.exp(avg) - 1.0) * 100.0

def main():
    if len(sys.argv) < 2:
        raise SystemExit("usage: analyze.py rd_gap_results.tsv")
    path = sys.argv[1]
    pts = collections.defaultdict(list)   # (image, encoder) -> [(ssim2,bpp)]
    fam = {}                               # image -> family
    with open(path) as f:
        r = csv.DictReader(f, delimiter="\t")
        for row in r:
            try:
                s = float(row["ssim2"]); b = float(row["bpp"])
            except (ValueError, KeyError, TypeError):
                continue
            if b <= 0: continue
            pts[(row["image"], row["encoder"])].append((s, b))
            fam[row["image"]] = row.get("family", "?")
    images = sorted({im for (im, _) in pts})
    fr = {k: frontier(v) for k, v in pts.items()}
    common = [im for im in images
              if len(fr.get((im, "zenrav1e"), [])) >= 2 and len(fr.get((im, "libaom"), [])) >= 2]
    only_zr = [im for im in images if (im, "libaom") not in fr]
    print(f"images: {len(images)}  with both encoders: {len(common)}  zenrav1e-only: {len(only_zr)}")
    if not common:
        print("\nNo images have BOTH encoders — libaom side missing (AOMENC unset?).")
        print("zenrav1e RD frontier (median bpp per ssim2) — diff this vs the committed baseline:")
        for t in range(78, 96):
            xs = [bpp_at(fr[(im, "zenrav1e")], t) for im in only_zr]; xs = [x for x in xs if x]
            if xs: print(f"  ssim2 {t}: median bpp {np.median(xs):.4f}  (n={len(xs)})")
        return

    print("\n=== BPP GAP: zenrav1e vs libaom-slow at matched SSIMULACRA2 (+ = zenrav1e worse) ===")
    print(f"{'ssim2':>6} {'n':>4} {'median+%':>9} {'mean%':>7} {'zr_bpp':>9} {'aom_bpp':>9}")
    summary = []
    for t in TARGETS:
        gaps, zr, ao = [], [], []
        for im in common:
            zb = bpp_at(fr[(im, "zenrav1e")], t); ab = bpp_at(fr[(im, "libaom")], t)
            if zb and ab: gaps.append(100 * (zb - ab) / ab); zr.append(zb); ao.append(ab)
        if not gaps:
            print(f"{t:>6} {0:>4}   (no paired coverage)"); continue
        print(f"{t:>6} {len(gaps):>4} {np.median(gaps):>9.1f} {np.mean(gaps):>7.1f} {np.median(zr):>9.4f} {np.median(ao):>9.4f}")
        summary.append((t, len(gaps), np.median(gaps), np.mean(gaps), np.median(zr), np.median(ao)))

    print("\n=== CONTENT SPLIT (photos = family!=%s, plots = family==%s) ===" % (PLOT_FAMILY, PLOT_FAMILY))
    print(f"{'ssim2':>6} {'group':>7} {'n':>3} {'median+%':>9} {'mean%':>7}")
    for t in TARGETS:
        grp = collections.defaultdict(list)
        for im in common:
            zb = bpp_at(fr[(im, "zenrav1e")], t); ab = bpp_at(fr[(im, "libaom")], t)
            if zb and ab:
                grp["plots" if fam.get(im) == PLOT_FAMILY else "photos"].append(100 * (zb - ab) / ab)
        for g in ("photos", "plots"):
            v = grp[g]
            if v: print(f"{t:>6} {g:>7} {len(v):>3} {np.median(v):>+9.1f} {np.mean(v):>+7.1f}")

    bd = [bd_rate(fr[(im, "zenrav1e")], fr[(im, "libaom")]) for im in common]
    bd = [x for x in bd if x is not None]
    if bd:
        print("\n=== BD-RATE zenrav1e vs libaom-slow (integrated; + = zenrav1e needs more bits) ===")
        print(f"  n={len(bd)}  median {np.median(bd):+.1f}%  mean {np.mean(bd):+.1f}%  min {min(bd):+.1f}%  max {max(bd):+.1f}%")

    out = path.rsplit(".", 1)[0] + "_summary.csv"
    with open(out, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["target_ssim2", "n_paired", "median_zenrav1e_larger_pct", "mean_pct", "zenrav1e_median_bpp", "libaom_median_bpp"])
        for s in summary: w.writerow([s[0], s[1], f"{s[2]:.2f}", f"{s[3]:.2f}", f"{s[4]:.4f}", f"{s[5]:.4f}"])
    print(f"\nwrote {out}")

if __name__ == "__main__":
    main()

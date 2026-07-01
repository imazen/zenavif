#!/usr/bin/env python3
"""How much of libaom's RD advantage over zenrav1e comes specifically from palette mode?

Input TSV columns: image w h family palette encoder fmt q bytes bpp ssim2 enc_ms
where palette in {1, 0} (aomenc --enable-palette). Builds an RD frontier per
(image, palette) and reports the paired bpp gap at ssim2 {82,85,88,90} between
palette-off and palette-on (+ = disabling palette costs libaom more bits, i.e.
palette was helping), split photos vs plots (family 7 = 7000-lilith-plots).

Usage: analyze_palette_ablation.py palette_ablation_results.tsv
"""
import sys, csv, collections
import numpy as np

TARGETS = [82, 85, 88, 90]
PLOT_FAMILY = "7"

def frontier(points):
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

def main():
    if len(sys.argv) < 2:
        raise SystemExit("usage: analyze_palette_ablation.py palette_ablation_results.tsv")
    path = sys.argv[1]
    pts = collections.defaultdict(list)   # (image, palette) -> [(ssim2,bpp)]
    fam = {}
    with open(path) as f:
        r = csv.DictReader(f, delimiter="\t")
        for row in r:
            try:
                s = float(row["ssim2"]); b = float(row["bpp"])
            except (ValueError, KeyError, TypeError):
                continue
            if b <= 0: continue
            pts[(row["image"], row["palette"])].append((s, b))
            fam[row["image"]] = row.get("family", "?")
    images = sorted({im for (im, _) in pts})
    fr = {k: frontier(v) for k, v in pts.items()}
    common = [im for im in images
              if len(fr.get((im, "1"), [])) >= 2 and len(fr.get((im, "0"), [])) >= 2]
    print(f"images: {len(images)}  with both palette conditions: {len(common)}")
    if not common:
        print("No images have both palette=1 and palette=0 coverage.")
        return

    print("\n=== BPP COST OF DISABLING PALETTE at matched SSIMULACRA2 (+ = palette was helping) ===")
    print(f"{'ssim2':>6} {'n':>4} {'median+%':>9} {'mean%':>7} {'pal_on_bpp':>11} {'pal_off_bpp':>12}")
    for t in TARGETS:
        gaps, on, off = [], [], []
        for im in common:
            b_on = bpp_at(fr[(im, "1")], t); b_off = bpp_at(fr[(im, "0")], t)
            if b_on and b_off:
                gaps.append(100 * (b_off - b_on) / b_on); on.append(b_on); off.append(b_off)
        if not gaps:
            print(f"{t:>6} {0:>4}   (no paired coverage)"); continue
        print(f"{t:>6} {len(gaps):>4} {np.median(gaps):>9.1f} {np.mean(gaps):>7.1f} {np.median(on):>11.4f} {np.median(off):>12.4f}")

    print("\n=== CONTENT SPLIT (photos = family!=%s, plots = family==%s) ===" % (PLOT_FAMILY, PLOT_FAMILY))
    print(f"{'ssim2':>6} {'group':>7} {'n':>3} {'median+%':>9} {'mean%':>7}")
    for t in TARGETS:
        grp = collections.defaultdict(list)
        for im in common:
            b_on = bpp_at(fr[(im, "1")], t); b_off = bpp_at(fr[(im, "0")], t)
            if b_on and b_off:
                grp["plots" if fam.get(im) == PLOT_FAMILY else "photos"].append(100 * (b_off - b_on) / b_on)
        for g in ("photos", "plots"):
            v = grp[g]
            if v: print(f"{t:>6} {g:>7} {len(v):>3} {np.median(v):>+9.1f} {np.mean(v):>+7.1f}")

    print("\nper-image detail (ssim2=85 if available):")
    for im in common:
        b_on = bpp_at(fr[(im, "1")], 85); b_off = bpp_at(fr[(im, "0")], 85)
        if b_on and b_off:
            print(f"  fam{fam.get(im)} {im}: on={b_on:.4f} off={b_off:.4f} (+{100*(b_off-b_on)/b_on:.1f}%)")

if __name__ == "__main__":
    main()

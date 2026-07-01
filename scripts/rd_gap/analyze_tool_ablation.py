#!/usr/bin/env python3
"""How much does each aomenc tool variant cost vs baseline, at matched SSIMULACRA2?

Input TSV columns: image w h family variant encoder fmt q bytes bpp ssim2 enc_ms
Usage: analyze_tool_ablation.py tool_ablation_results.tsv <baseline_label>
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
    if len(sys.argv) < 3:
        raise SystemExit("usage: analyze_tool_ablation.py tool_ablation_results.tsv <baseline_label>")
    path, baseline = sys.argv[1], sys.argv[2]
    pts = collections.defaultdict(list)   # (image, variant) -> [(ssim2,bpp)]
    fam = {}
    variants = set()
    with open(path) as f:
        r = csv.DictReader(f, delimiter="\t")
        for row in r:
            try:
                s = float(row["ssim2"]); b = float(row["bpp"])
            except (ValueError, KeyError, TypeError):
                continue
            if b <= 0: continue
            pts[(row["image"], row["variant"])].append((s, b))
            fam[row["image"]] = row.get("family", "?")
            variants.add(row["variant"])
    images = sorted({im for (im, _) in pts})
    fr = {k: frontier(v) for k, v in pts.items()}
    others = sorted(v for v in variants if v != baseline)

    for other in others:
        common = [im for im in images
                  if len(fr.get((im, baseline), [])) >= 2 and len(fr.get((im, other), [])) >= 2]
        print(f"\n########## {other} vs {baseline}  (images with both: {len(common)}/{len(images)}) ##########")
        print(f"{'ssim2':>6} {'n':>4} {'median+%':>9} {'mean%':>7} {baseline+'_bpp':>14} {other+'_bpp':>14}")
        for t in TARGETS:
            gaps, base_b, other_b = [], [], []
            for im in common:
                b_base = bpp_at(fr[(im, baseline)], t); b_other = bpp_at(fr[(im, other)], t)
                if b_base and b_other:
                    gaps.append(100 * (b_other - b_base) / b_base); base_b.append(b_base); other_b.append(b_other)
            if not gaps:
                print(f"{t:>6} {0:>4}   (no paired coverage)"); continue
            print(f"{t:>6} {len(gaps):>4} {np.median(gaps):>9.1f} {np.mean(gaps):>7.1f} {np.median(base_b):>14.4f} {np.median(other_b):>14.4f}")

        grp_all = collections.defaultdict(list)
        for t in TARGETS:
            for im in common:
                b_base = bpp_at(fr[(im, baseline)], t); b_other = bpp_at(fr[(im, other)], t)
                if b_base and b_other:
                    grp_all["plots" if fam.get(im) == PLOT_FAMILY else "photos"].append(100 * (b_other - b_base) / b_base)
        for g in ("photos", "plots"):
            v = grp_all[g]
            if v: print(f"  [{g}, all targets pooled] n={len(v)} median={np.median(v):+.1f}% mean={np.mean(v):+.1f}%")

if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Non-tune size-decay isolation A/B analysis (follow-up to analyze_size_decay_ab.py).

Consumes the per-arm TSVs written by scripts/rd_gap/sizedecay_nontune_arms.sh
(sdn_base + sdn_<arm>). Each arm ADDS one coding tool to the tune-off baseline
unconditionally at every quality; conviction per the PRE-REGISTERED rule in
/mnt/v/output/zenavif/sizedecay-nontune-2026-07-03/DECISION_RULE.md:

  w(size) = median BD-rate(base vs arm), POSITIVE = the arm saves bits.
  Convict iff w(256) >= +1.0 AND (w(256)-w(1024) >= +1.0 OR w(1024) <= +0.3).
  Butteraugli veto: median w_ba3n <= -1.0 or w_bamax <= -1.5 at the convicted
  size rejects.

Also reports each arm's vs-cpu2 position per size (cpu2 frontiers from the
label store wedge rows for train, or an aom_only TSV for val).

Usage:
  analyze_sizedecay_nontune.py --dir <dir-with-sdn_*.tsv> [--split train|val]
      [--val-cpu2-tsv <tsv>] [--out-tsv benchmarks/...tsv]
"""
import argparse
import glob
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "rd_gap"))
from bd_arm import bd_rate, frontier, load  # noqa: E402

SIZE_TOKENS = (".s256.png", ".s512.png", ".s1024.png", ".native.png")


def size_class_of(image_name):
    for tok in SIZE_TOKENS:
        if image_name.endswith(tok):
            return tok.split(".")[1][1:] if tok != ".native.png" else "native"
    return None


def origin_of(image_name):
    return image_name.split("_", 1)[0]


def band_gaps(zf, rf, fqs=(0.25, 0.75)):
    if len(zf) < 2 or len(rf) < 2:
        return [None] * len(fqs)
    lo = max(zf[0][0], rf[0][0])
    hi = min(zf[-1][0], rf[-1][0])
    if hi <= lo:
        return [None] * len(fqs)
    out = []
    for fq in fqs:
        s = lo + fq * (hi - lo)
        tb = float(np.interp(s, [p[0] for p in zf], [p[1] for p in zf]))
        rb = float(np.interp(s, [p[0] for p in rf], [p[1] for p in rf]))
        out.append(100.0 * (tb - rb) / rb)
    return out


def px_table(sample_tsv):
    px = {}
    with open(sample_tsv) as f:
        next(f)
        for line in f:
            p, w, h, _fam = line.rstrip("\n").split("\t")
            px[os.path.basename(p)] = (int(w), int(h))
    return px


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", required=True)
    ap.add_argument("--split", default="train")
    ap.add_argument("--sample", default=None)
    ap.add_argument("--val-cpu2-tsv")
    ap.add_argument("--out-tsv")
    args = ap.parse_args()

    here = os.path.dirname(os.path.abspath(__file__))
    sample = args.sample or os.path.join(
        here, "..", "rd_gap", f"sample_sizedecay_{args.split}.tsv")
    pxs = px_table(sample)

    arms = {}
    for p in sorted(glob.glob(os.path.join(args.dir, "sdn_*.tsv"))):
        arm = os.path.basename(p)[4:-4]
        arms[arm] = {
            "ssim2": load(p, "ssim2")[0],
            "butteraugli_3n": load(p, "butteraugli_3n")[0],
            "butteraugli_max": load(p, "butteraugli_max")[0],
        }
    if "base" not in arms:
        sys.exit(f"no sdn_base.tsv in {args.dir}")

    def slot_of(img):
        sc = size_class_of(img)
        if sc in ("256", "512", "1024"):
            return sc
        w, h = pxs.get(img, (0, 0))
        return "1024" if max(w, h) >= 1024 else "odd"

    recs = []
    for img in sorted(arms["base"]["ssim2"]):
        base_f = frontier(arms["base"]["ssim2"][img])
        for arm_name, data in arms.items():
            if arm_name == "base" or img not in data["ssim2"]:
                continue
            af = frontier(data["ssim2"][img])
            # w = BD(base vs arm): positive = base needs more bits = ARM WINS.
            w = bd_rate(base_f, af)
            glo, ghi = band_gaps(base_f, af)
            rec = dict(split=args.split, image=img, origin=origin_of(img),
                       size_class=size_class_of(img), slot=slot_of(img),
                       arm=arm_name, w_ssim2=w, gap_lowq=glo, gap_highq=ghi)
            for bm in ("butteraugli_3n", "butteraugli_max"):
                b = arms["base"][bm].get(img)
                a = data[bm].get(img)
                rec[f"w_{bm}"] = bd_rate(frontier(b), frontier(a)) if b and a else None
            recs.append(rec)
    D = pd.DataFrame(recs)

    print(f"\n######## {args.split}: w = BD(base vs arm) per size — POSITIVE = arm saves bits ########")
    arm_order = [a for a in ("prange464", "prange432", "rdotx", "cdef", "lrf",
                             "segoff", "yuv420", "combo") if a in set(D["arm"])]
    arm_order += [a for a in sorted(set(D["arm"])) if a not in arm_order]
    rows = []
    for a in arm_order:
        for s in ("256", "512", "1024"):
            g = D[(D["arm"] == a) & (D["slot"] == s)]
            if g.empty:
                continue
            v = g["w_ssim2"].dropna()
            rows.append(dict(
                arm=a, slot=s, n=len(v), w_med=v.median(), w_mean=v.mean(),
                arm_better=f"{int((v > 0).sum())}/{len(v)}",
                lowq_med=g["gap_lowq"].median(), highq_med=g["gap_highq"].median(),
                ba3n_med=g["w_butteraugli_3n"].median(),
                bamax_med=g["w_butteraugli_max"].median(),
            ))
    M = pd.DataFrame(rows)
    piv = M.pivot_table(index="arm", columns="slot", values="w_med", sort=False)
    piv = piv.reindex(arm_order)[[c for c in ("256", "512", "1024") if c in piv.columns]]
    if "256" in piv.columns and "1024" in piv.columns:
        piv["decay_win"] = piv["256"] - piv["1024"]
    print(piv.to_string(float_format=lambda x: f"{x:+.2f}"))
    print("\nfull table:")
    print(M.to_string(index=False, float_format=lambda x: f"{x:+.3f}"))

    # verdicts per DECISION_RULE
    print("\n=== verdicts (pre-registered rule) ===")
    for a in arm_order:
        if a == "yuv420":
            tag = "(diagnostic only)"
        else:
            tag = ""
        try:
            w256 = M[(M.arm == a) & (M.slot == "256")].w_med.iloc[0]
            w1024 = M[(M.arm == a) & (M.slot == "1024")].w_med.iloc[0]
            ba3 = M[(M.arm == a) & (M.slot == "256")].ba3n_med.iloc[0]
            bam = M[(M.arm == a) & (M.slot == "256")].bamax_med.iloc[0]
        except IndexError:
            print(f"  {a}: INCOMPLETE (missing slots)")
            continue
        c1 = w256 >= 1.0
        c2 = (w256 - w1024 >= 1.0) or (w1024 <= 0.3)
        veto = (ba3 is not None and ba3 <= -1.0) or (bam is not None and bam <= -1.5)
        verdict = "CONVICTED" if (c1 and c2 and not veto) else \
                  ("VETOED-butteraugli" if (c1 and c2) else "not convicted")
        print(f"  {a}: w256={w256:+.2f} w1024={w1024:+.2f} ba3n256={ba3:+.2f} "
              f"bamax256={bam:+.2f} -> {verdict} {tag}")

    # vs cpu2 (per arm per size)
    if args.split == "train":
        sys.path.insert(0, here)
        from hp_common import _quality_pts, load_store
        store = load_store(sweep_source="wedge-2026-07-03")
        cpu2 = store[(store["arm_id"] == "wedge/aom-cpu2") & (store["crop_label"] == "full")]

        def cpu2_frontier(origin, sc):
            g = cpu2[(cpu2["origin_id"].astype(str) == origin) & (cpu2["size_class"] == sc)]
            return frontier(_quality_pts(g, "ssim2")) if not g.empty else None
    elif args.val_cpu2_tsv:
        cpu_rows, _ = load(args.val_cpu2_tsv, "ssim2", encoder="libaom")

        def cpu2_frontier(origin, sc):
            pts = []
            for img, p in cpu_rows.items():
                if origin_of(img) == origin and size_class_of(img) == sc:
                    pts.extend(p)
            return frontier(pts) if pts else None
    else:
        # val refs from the label store (sizedecay/ref-cpu2-val rows)
        sys.path.insert(0, here)
        from hp_common import _quality_pts, load_store
        store = load_store(sweep_source="sizedecay-2026-07-03",
                           arm_id="sizedecay/ref-cpu2-val")

        def cpu2_frontier(origin, sc):
            g = store[(store["origin_id"].astype(str) == origin)
                      & (store["size_class"] == sc)]
            return frontier(_quality_pts(g, "ssim2")) if not g.empty else None

    if cpu2_frontier:
        rows = []
        for arm_name, data in arms.items():
            for img, pts in data["ssim2"].items():
                o, sc = origin_of(img), size_class_of(img)
                rf = cpu2_frontier(o, sc)
                if rf is None:
                    continue
                rows.append(dict(arm=arm_name, origin=o, slot=slot_of(img),
                                 bd=bd_rate(frontier(pts), rf)))
        C = pd.DataFrame(rows)
        C = C[C.slot.isin(["256", "512", "1024"])]
        tab = C.pivot_table(index="arm", columns="slot", values="bd", aggfunc="median")
        tab = tab.reindex(["base"] + arm_order)
        print("\n=== BD vs libaom cpu2 per size (medians; negative = zr wins) ===")
        print(tab.to_string(float_format=lambda x: f"{x:+.2f}"))

    if args.out_tsv:
        with open(args.out_tsv, "w") as f:
            f.write("# non-tune size-decay isolation A/B: w = BD(base vs arm), positive = arm saves bits vs tune-off baseline\n")
            f.write("# gap_lowq/gap_highq = bpp gap % at 25%/75% of overlapping ssim2 window (base vs arm)\n")
            D.to_csv(f, sep="\t", index=False, float_format="%.4f")
        print(f"\nwrote {args.out_tsv}")


if __name__ == "__main__":
    main()

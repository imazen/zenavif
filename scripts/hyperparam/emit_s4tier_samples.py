#!/usr/bin/env python3
"""Emit the S4-TIER composed-mode per-class sample TSVs (v3 rules).

The s4-tier operating point (FAST_TIER_PARITY_PLAN, last open column) is the
P2 composed fast mode with the tx head's DEEP bound refit at the s4-tier
time budget (fit_s4_tier.py section D: LOOCV 22/24-stable at lam=0.5 AND
0.25 on the fastwins labels; the knapsack's affordable map matches the rule
map's min-set at the cpu2iq-ai wall budget of 6.47x plain-s6):

FROZEN RULES v3 (s4-tier; v2 with ONE bound moved):
  tx head   : pf > 0.8505 AND dcty > 100         -> none  (unchanged W)
              elif pf <= 0.8505 AND dcty < 23.69 -> min   (was 8.352 at s6)
              else                               -> size1
  part head : gradient_fraction_smooth < 0.4105  -> m32   (unchanged; the
              lam=0.25 refit alternative gfs@0.6474 is LOOCV-flat and fires
              m32 onto 6018 whose m32 label is +2.47 harm)

The intra arm (i7 vs the new i5 knob midpoint) is a GLOBAL per-tier choice
measured by the chain's i5axis phase, not a per-image head (P2 head-3
verdict: no per-image structure at n=24).

Also emits sample_s4x_full{ship,m32}.tsv — the full-tx ORACLE-EXTRA images
(clean full-over-min/size1 label wins with no honest gate at n=24: measured
as upper-bound factoring cells, NOT deployed): 8414/6606/5048 at ship,
9074/9868 at m32.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import load_features, load_store, split_of  # noqa: E402

RD_GAP = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../rd_gap")

TX_PF_NONE = 0.8505
TX_DCTY_MIN = 23.69          # <-- the s4-tier refit (s6 head: 8.352)
TX_DCTY_RAZOR = 100.0
PART_GFS_M32 = 0.4105
FEATS = ["patch_fraction", "dct_compressibility_y", "gradient_fraction_smooth"]

FULL_EXTRAS = {  # basename prefix -> partition rung (rule-assigned)
    "8414": "ship", "6606": "ship", "5048": "ship",
    "9074": "m32", "9868": "m32",
}


def choose(pf, dcty, gfs):
    if pf > TX_PF_NONE:
        tx = "none" if dcty > TX_DCTY_RAZOR else "size1"
    else:
        tx = "min" if dcty < TX_DCTY_MIN else "size1"
    part = "m32" if gfs < PART_GFS_M32 else "ship"
    return tx, part


def write_tsv(path, rows):
    with open(path, "w") as f:
        f.write("image\tw\th\tfamily\n")
        for r in rows:
            f.write("\t".join(str(x) for x in r) + "\n")
    print(f"wrote {path} ({len(rows)} images)")


def main():
    feats = load_features(FEATS)
    store = load_store(sweep_source="p1part-2026-07-04")
    t26 = (store[["image_id", "origin_id", "content_class", "feature_join",
                  "w", "h"]].drop_duplicates("image_id").set_index("image_id"))

    paths = {}
    with open(os.path.join(RD_GAP, "sample_images_train26.tsv")) as f:
        next(f)
        for ln in f:
            p, w, h, fam = ln.rstrip("\n").split("\t")
            paths[os.path.basename(p)] = (p, w, h, fam)

    classes, extras = {}, {}
    print("=== train26 s4-tier (v3) classes ===")
    for img, row in t26.iterrows():
        fj = row["feature_join"]
        pf, dcty, gfs = (feats.loc[fj, f] for f in FEATS)
        tx, part = choose(pf, dcty, gfs)
        p, w, h, fam = paths[img]
        classes.setdefault(f"{tx}_{part}", []).append((p, w, h, fam))
        pre = img.split("_")[0]
        if pre in FULL_EXTRAS:
            extras.setdefault(f"full_{FULL_EXTRAS[pre]}", []).append((p, w, h, fam))
        print(f"  {row['origin_id']}  tx={tx:5s} part={part:4s}  "
              f"(pf {pf:.3f} dcty {dcty:.1f} gfs {gfs:.3f})")
    for cls, rows in sorted(classes.items()):
        write_tsv(os.path.join(RD_GAP, f"sample_s4c_{cls}.tsv"), rows)
    for cls, rows in sorted(extras.items()):
        write_tsv(os.path.join(RD_GAP, f"sample_s4x_{cls}.tsv"), rows)

    # --- val (palette-mech corpus, store joins — same recipe as p2heads) ---
    print("\n=== val-1024 s4-tier (v3) classes ===")
    VAL_PNG = "/mnt/v/output/rd-gap-palette-val-2026-07-03/png"
    mech = load_store(sweep_source="palette-mech-ab-2026-07-03")
    vrows = (mech[(mech["split"] == "val")
                  & (mech["size_class"].isin(["1024", "native"]))]
             [["image_id", "origin_id", "content_class", "feature_join",
               "w", "h"]].drop_duplicates("image_id").set_index("image_id"))
    vclasses = {}
    for img, row in vrows.sort_index().iterrows():
        fj = row["feature_join"]
        if fj not in feats.index:
            print(f"  SKIP (feature_join missing): {img}")
            continue
        assert split_of(img) == "val", img
        pf, dcty, gfs = (float(feats.loc[fj, f]) for f in FEATS)
        tx, part = choose(pf, dcty, gfs)
        p = os.path.join(VAL_PNG, img)
        fam = str(row["content_class"]).split("-")[0]
        vclasses.setdefault(f"{tx}_{part}", []).append(
            (p, int(row["w"]), int(row["h"]), fam))
        print(f"  {row['origin_id']:>6} {img[:52]:54s} tx={tx:5s} part={part:4s} "
              f"(pf {pf:.3f} dcty {dcty:.1f} gfs {gfs:.3f})")
    for cls, rows in sorted(vclasses.items()):
        write_tsv(os.path.join(RD_GAP, f"sample_s4valc_{cls}.tsv"), rows)


if __name__ == "__main__":
    main()

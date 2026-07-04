#!/usr/bin/env python3
"""Emit the FROZEN P2 composed-fast-mode per-class sample TSVs (s6).

The composed s6 fast mode = per-image (tx, partition) config chosen by the
two P2 threshold heads (fit records:
benchmarks/hyperparam_tx_budget_2026-07-04.tsv,
benchmarks/hyperparam_partition_budget_2026-07-04.tsv). This script freezes
the deploy rules as explicit constants, applies them to the train26 corpus
(from the label store's feature joins) and to the VAL-LSD 1024-slot corpus
(sample_sizedecay_val.tsv + the features parquet), and writes one sample TSV
per distinct (tx, part) class for the box chain (chain_p2heads.sh) — per-class
env sub-runs need zero harness changes.

FROZEN RULES v2 (s6; train fit 2026-07-04 + the same-day VAL attribution
revision — see benchmarks/rd_gap_p2heads_2026-07-04.tsv):
  tx head   : pf > 0.8505 AND dcty > 100        -> none   (withhold size-RDO;
              the razor-edge line-tiling class only)
              elif pf <= 0.8505 AND dcty < 8.352 -> min   (size1+types+reduced)
              else                               -> size1
  part head : gradient_fraction_smooth < 0.4105  -> m32    (r16m32_bkvg2)
              else                               -> ship   (r16no4_bkvg2)

HISTORY: the v1 rules (pf-only withhold, un-capped min) produced the box
class samples the composed run measured; the VAL factoring cells (p2vx_*)
convicted the pf-only withhold (8103: (none,ship) +18.1 vs (size1,m32)
-1.9) and v2 remaps exactly three images (7028, 5343, 8103) onto classes
whose cells were ALSO measured (p2rx_*/p2vx_*). Re-running this script
emits the v2 classes.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import load_features, load_store, split_of  # noqa: E402

RD_GAP = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../rd_gap")
VAL_SAMPLE = os.path.join(RD_GAP, "sample_sizedecay_val.tsv")

TX_PF_NONE = 0.8505
TX_DCTY_MIN = 8.352
PART_GFS_M32 = 0.4105
FEATS = ["patch_fraction", "dct_compressibility_y", "gradient_fraction_smooth"]


TX_DCTY_RAZOR = 100.0  # v2 conjunctive withhold bound (see docstring)


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

    # train26 sample paths come from the canonical train26 sample TSV.
    paths = {}
    with open(os.path.join(RD_GAP, "sample_images_train26.tsv")) as f:
        next(f)
        for ln in f:
            p, w, h, fam = ln.rstrip("\n").split("\t")
            paths[os.path.basename(p)] = (p, w, h, fam)

    classes = {}
    print("=== train26 composed classes ===")
    for img, row in t26.iterrows():
        fj = row["feature_join"]
        pf, dcty, gfs = (feats.loc[fj, f] for f in FEATS)
        tx, part = choose(pf, dcty, gfs)
        p, w, h, fam = paths[img]
        classes.setdefault(f"{tx}_{part}", []).append((p, w, h, fam))
        print(f"  {row['origin_id']}  tx={tx:5s} part={part:4s}  "
              f"(pf {pf:.3f} dcty {dcty:.1f} gfs {gfs:.3f})")
    for cls, rows in sorted(classes.items()):
        write_tsv(os.path.join(RD_GAP, f"sample_p2c_{cls}.tsv"), rows)

    # --- val: 1024/native slot of the palette-mech VAL corpus (the store's
    # join-verified feature_join column — 108/108 at the mech A/B) ---
    print("\n=== val-1024 composed classes (palette-mech corpus, store joins) ===")
    VAL_PNG = "/mnt/v/output/rd-gap-palette-val-2026-07-03/png"
    mech = load_store(sweep_source="palette-mech-ab-2026-07-03")
    vrows = (mech[(mech["split"] == "val")
                  & (mech["size_class"].isin(["1024", "native"]))]
             [["image_id", "origin_id", "content_class", "feature_join",
               "w", "h"]].drop_duplicates("image_id").set_index("image_id"))
    vclasses, vall = {}, []
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
        rec = (p, int(row["w"]), int(row["h"]), fam)
        vclasses.setdefault(f"{tx}_{part}", []).append(rec)
        vall.append(rec)
        print(f"  {row['origin_id']:>6} {img[:52]:54s} tx={tx:5s} part={part:4s} "
              f"(pf {pf:.3f} dcty {dcty:.1f} gfs {gfs:.3f})")
    write_tsv(os.path.join(RD_GAP, "sample_p2val_all.tsv"), vall)
    for cls, rows in sorted(vclasses.items()):
        write_tsv(os.path.join(RD_GAP, f"sample_p2valc_{cls}.tsv"), rows)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Build harness sample TSVs (image\tw\th\tfamily) for the palette-gate
mechanism A/B from the wedge + val corpus maps.

Sets (sizes = {256, 512, 1024-slot} where 1024-slot = s1024 if the origin has
one, else native (native<=1024 origins)):
  t1_fired_full   train wedge fired-class origins (paletteoff subset), full
  t2_fired_crops  same origins, c50 quadrant crops (1024-scale)
  t3_docs_full    train wedge docs/scans fired-class extras {5318, 6070}
  t4_quiet_full   train wedge quiet-class {6606, 9118, 9098}
  t5_photos_full  train wedge photos {2026, 1238, 1480}
  val_full        all 14 val origins, full
"""
import os
import sys

import pandas as pd

WEDGE = "/mnt/v/output/rd-gap-wedge-2026-07-03"
VAL = "/mnt/v/output/rd-gap-palette-val-2026-07-03"
OUTDIR = sys.argv[1] if len(sys.argv) > 1 else "/mnt/v/output/rd-gap-palette-ab-2026-07-03/samples"
os.makedirs(OUTDIR, exist_ok=True)

FIRED_TRAIN = {"7050", "7052", "7064", "7080", "8196", "8464", "9446", "9908"}
DOCS_TRAIN = {"5318", "6070"}
QUIET_TRAIN = {"6606", "9118", "9098"}
PHOTO_TRAIN = {"2026", "1238", "1480"}


def load_map(root):
    m = pd.read_csv(os.path.join(root, "corpus_map.tsv"), sep="\t", dtype=str)
    m["origin_id"] = m["origin_id"].astype(str)
    m["family"] = m["family"].astype(str)
    m["image"] = m["file"].map(lambda f: os.path.join(root, "png", f))
    return m


def size_slot_full(m, origins):
    """full-crop rows at {256,512,1024-slot} for the given origins."""
    rows = []
    for org, g in m[(m.crop_label == "full") & m.origin_id.isin(origins)].groupby("origin_id"):
        have = set(g.size_class)
        want = {"256", "512"} | ({"1024"} if "1024" in have else {"native"})
        rows.append(g[g.size_class.isin(want)])
    return pd.concat(rows) if rows else pd.DataFrame()


def crops_slot(m, origins):
    return m[m.crop_label.str.startswith("c50") & m.origin_id.isin(origins)]


def emit(df, name):
    out = os.path.join(OUTDIR, f"sample_{name}.tsv")
    with open(out, "w") as f:
        f.write("image\tw\th\tfamily\n")
        # desc-pixel order (big files first — straggler control, same as wedge)
        df = df.assign(px=df.width.astype(int) * df.height.astype(int)).sort_values("px", ascending=False)
        for _, r in df.iterrows():
            assert os.path.exists(r.image), r.image
            f.write(f"{r.image}\t{r.width}\t{r.height}\t{r.family}\n")
    print(f"{name}: {len(df)} files -> {out}")


wm = load_map(WEDGE)
vm = load_map(VAL)

emit(size_slot_full(wm, FIRED_TRAIN), "t1_fired_full")
emit(crops_slot(wm, FIRED_TRAIN), "t2_fired_crops")
emit(size_slot_full(wm, DOCS_TRAIN), "t3_docs_full")
emit(size_slot_full(wm, QUIET_TRAIN), "t4_quiet_full")
emit(size_slot_full(wm, PHOTO_TRAIN), "t5_photos_full")
emit(size_slot_full(vm, set(vm.origin_id.unique())), "val_full")

# combined convenience sets for arm scheduling
t_all = pd.concat([size_slot_full(wm, FIRED_TRAIN | DOCS_TRAIN | QUIET_TRAIN | PHOTO_TRAIN)])
emit(t_all, "train_full_all")
fired_all = pd.concat([size_slot_full(wm, FIRED_TRAIN | DOCS_TRAIN),
                       size_slot_full(vm, {o for o in vm.origin_id.unique()
                                           if o[0] in "5689" or o.startswith("7")})])
emit(fired_all, "timing_firedish")

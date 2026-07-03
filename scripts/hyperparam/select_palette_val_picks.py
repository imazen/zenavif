#!/usr/bin/env python3
"""Select VAL-split origins for the palette-gate mechanism A/B (HYPERPARAM_FIRST_CUT
rule 1 graduation; wedge #6).

Selection is deterministic from the canonical features parquet + the canonical
LSD origin split (origin_split.py; last digit {1,3,5} = val). Per class we pick
origins by their gate behavior (`patch_fraction > 0.197` on full-crop rows at
the swept size classes {256, 512, 1024-or-native-if-smaller}):

  fired classes (7000 plots, 8100 web-screenshots, 9226 ai-products,
                 5300 noaa docs, 6000 patents, 9000 ai-clipart,
                 8000 mobile-screenshots):
    pick A = strongest consistent fire  (max over origins of min-over-sizes
             patch_fraction, among origins firing at every swept size)
    pick B (7000/8100/9226 only) = borderline: for 9226 a MIXED origin (fires
             at >=1 but not all swept sizes, max firing count, then max
             min-pf); for 7000/8100 the weakest consistent fire (min
             min-over-sizes pf among all-size firers)
  quiet classes (6600 scans-illustrations, 9094 ai-illustrations):
    pick = never-fires origin with MAX patch_fraction (closest quiet case)
  photo false-fire check (1000 photos, 2000 people):
    pick = never-fires origin with MAX patch_fraction

Output: picks JSON in the wedge_corpus.rs --picks schema.
"""
import json
import os
import sys

import numpy as np

sys.path.insert(0, os.path.expanduser("~/work/zen/zenavif/scripts/hyperparam"))
sys.path.insert(0, os.path.expanduser("~/work/zen/zenavif/scripts/rd_gap"))
sys.path.insert(0, os.path.expanduser("~/work/zen/zenmetrics/scripts/picker"))
from hp_common import load_features  # noqa: E402
from origin_split import split_of  # noqa: E402

THRESH = 0.197
SWEPT = ["256", "512", "1024", "native"]
OUT = sys.argv[1] if len(sys.argv) > 1 else "/tmp/palette_val_picks.json"

f = load_features(["patch_fraction"]).reset_index()
f["base"] = f["image_path"].str.rsplit("/", n=1).str[-1]
f["split"] = f["base"].map(split_of)
f["cgroup"] = f["content_class"].str.extract(r"^(\d+)")
f["origin"] = f["base"].str.extract(r"^(\d+)_")

va = f[(f["split"] == "val") & (f["crop_label"] == "full")
       & f["size_class"].isin(SWEPT)].copy()

# swept sizes per origin: {256, 512} + ('1024' if present else 'native').
# (origins with native <= 1024 have no '1024' row; their 1024 slot IS native)
def swept_rows(g):
    have = set(g["size_class"])
    want = {"256", "512"} | ({"1024"} if "1024" in have else {"native"})
    return g[g["size_class"].isin(want)]

rows = []
for (cg, org), g in va.groupby(["cgroup", "origin"]):
    s = swept_rows(g)
    if len(s) < 3:
        continue  # missing rendition rows; skip
    pf = s["patch_fraction"].to_numpy()
    rows.append(dict(cgroup=cg, origin=org, n_sizes=len(s),
                     min_pf=float(pf.min()), max_pf=float(pf.max()),
                     fires=int((pf > THRESH).sum()),
                     image_path=g["image_path"].iloc[0],
                     content_class=g["content_class"].iloc[0]))
import pandas as pd  # noqa: E402
R = pd.DataFrame(rows)

picks = []
def add(row, why):
    picks.append(dict(cluster=len(picks), cluster_size=0,
                      image_path=row["image_path"],
                      content_class=row["content_class"],
                      native_longedge=0, origin_id=row["origin"], pick_reason=why))
    print(f"  {row['origin']} ({row['content_class']}): {why} "
          f"min_pf={row['min_pf']:.3f} max_pf={row['max_pf']:.3f} fires={row['fires']}/{row['n_sizes']}")

FIRED = ["7000", "8100", "9226", "5300", "6000", "9000", "8000"]
TWOPICK = {"7000", "8100", "9226"}
for cg in FIRED:
    c = R[R.cgroup == cg]
    allfire = c[c.fires == c.n_sizes]
    if allfire.empty:
        print(f"  WARNING {cg}: no all-size-firing val origin; strongest partial instead")
        add(c.sort_values(["fires", "min_pf"], ascending=False).iloc[0], "strongest-partial-fire")
        continue
    add(allfire.sort_values("min_pf", ascending=False).iloc[0], "strongest-consistent-fire")
    if cg in TWOPICK:
        if cg == "9226":
            mixed = c[(c.fires > 0) & (c.fires < c.n_sizes)]
            if not mixed.empty:
                add(mixed.sort_values(["fires", "min_pf"], ascending=False).iloc[0], "mixed-fire-borderline")
            else:
                add(allfire.sort_values("min_pf").iloc[0], "weakest-consistent-fire")
        else:
            cand = allfire.sort_values("min_pf")
            add(cand.iloc[0], "weakest-consistent-fire")

for cg in ["6600", "9094", "1000", "2000"]:
    c = R[(R.cgroup == cg) & (R.fires == 0)]
    if c.empty:
        print(f"  WARNING {cg}: no never-fire origin")
        continue
    add(c.sort_values("max_pf", ascending=False).iloc[0], "quiet-max-patch-fraction")

# de-dup + emit in the wedge picks schema
with open(OUT, "w") as fh:
    json.dump(picks, fh, indent=1)
print(f"\n{len(picks)} picks -> {OUT}")

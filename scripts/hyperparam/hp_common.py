#!/usr/bin/env python3
"""Shared helpers for hyperparameter-expert rule fitting (FEATURE_HINTS_PLAN §E).

BD conventions are IMPORTED from scripts/rd_gap/bd_arm.py (frontier + log-bytes
trapezoid over overlapping quality; butteraugli negated via -log into a quality
axis) so every number here is directly comparable to the committed sweep
summaries. Do not re-implement them.
"""

import json
import os
import sys

import numpy as np
import pandas as pd
import pyarrow.parquet as pq

ZEN = os.path.expanduser("~/work/zen")
sys.path.insert(0, os.path.join(ZEN, "zenavif/scripts/rd_gap"))
sys.path.insert(0, os.path.join(ZEN, "zenmetrics/scripts/picker"))
from bd_arm import LOWER_BETTER, bd_rate, frontier  # noqa: E402
from origin_split import split_of  # noqa: E402  (re-exported for fit scripts)

STORE = "/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet"
FEATURES_PARQUET = "/mnt/v/output/imazen-26-features/imazen26_features_2026-06-23.parquet"

__all__ = ["load_store", "load_features", "arm_bd_per_image", "bd_rate", "frontier",
           "split_of", "STORE", "FEATURES_PARQUET"]


def load_store(**filters):
    df = pq.read_table(STORE).to_pandas()
    for k, v in filters.items():
        df = df[df[k].isin(v) if isinstance(v, (list, tuple, set)) else df[k] == v]
    return df


def load_features(feature_names=None):
    """Features table keyed by feature_join, with @hash suffixes stripped.

    NOTE: ignores the parquet's own 'split' column (older convention) — use
    origin_split on the image basename instead (the store already carries it).
    """
    t = pq.read_table(FEATURES_PARQUET)
    cols = t.schema.names
    meta = ["image_path", "crop_label", "size_class", "width", "height", "content_class"]
    fmap = {c.split("@")[0]: c for c in cols if "@" in c}
    if feature_names is None:
        feature_names = sorted(fmap)
    missing = [n for n in feature_names if n not in fmap]
    assert not missing, f"unknown features: {missing}"
    df = t.select(meta + [fmap[n] for n in feature_names]).to_pandas()
    df.columns = meta + list(feature_names)
    df["feature_join"] = (df["image_path"] + "|" + df["crop_label"] + "|" + df["size_class"])
    return df.set_index("feature_join")


def _quality_pts(g, metric):
    pts = []
    for v, bpp in zip(g[metric].to_numpy(), g["bpp"].to_numpy()):
        if not np.isfinite(v) or not np.isfinite(bpp) or bpp <= 0:
            continue
        if metric in LOWER_BETTER:
            if v <= 0:
                continue
            v = -np.log(v)
        pts.append((float(v), float(bpp)))
    return pts


def arm_bd_per_image(store, sweep_source, base_arm, test_arm, metric="ssim2",
                     group=("image_id",), corpus=None):
    """Direct per-image BD-rate of test_arm vs base_arm (same grid, same binary
    chain — the 'direct isolation' convention). Negative = test arm needs fewer
    bits at matched quality. Returns DataFrame indexed by `group`."""
    df = store[store["sweep_source"] == sweep_source]
    if corpus:
        df = df[df["corpus"] == corpus]
    base = df[df["arm_id"] == base_arm]
    test = df[df["arm_id"] == test_arm]
    rows = []
    for key, gb in base.groupby(list(group)):
        gt = test
        kt = key if isinstance(key, tuple) else (key,)
        for col, val in zip(group, kt):
            gt = gt[gt[col] == val]
        if gt.empty:
            continue
        bd = bd_rate(frontier(_quality_pts(gt, metric)), frontier(_quality_pts(gb, metric)))
        rows.append(dict(zip(group, kt), bd=bd))
    return pd.DataFrame(rows).set_index(list(group)) if rows else pd.DataFrame()


def veto_table(store, sweep_source, base_arm, test_arm, group=("image_id",), corpus=None):
    """ssim2 + butteraugli BDs per image — the metric-gaming veto view."""
    out = None
    for m, name in [("ssim2", "bd_ssim2"), ("butteraugli_3n", "bd_ba3n"),
                    ("butteraugli_max", "bd_bamax")]:
        t = arm_bd_per_image(store, sweep_source, base_arm, test_arm, metric=m,
                             group=group, corpus=corpus)
        if t.empty:
            continue
        t = t.rename(columns={"bd": name})
        out = t if out is None else out.join(t, how="outer")
    return out


def print_dist(name, v):
    v = np.asarray([x for x in v if x is not None and np.isfinite(x)])
    if len(v) == 0:
        print(f"{name}: no data")
        return
    print(f"{name}: n={len(v)} median {np.median(v):+.3f} mean {np.mean(v):+.3f} "
          f"p10 {np.percentile(v, 10):+.3f} p90 {np.percentile(v, 90):+.3f} "
          f"win {int((v < 0).sum())}/{len(v)}")


def to_tsv(df, path, header_lines=()):
    with open(path, "w") as f:
        for ln in header_lines:
            f.write(f"# {ln}\n")
        df.to_csv(f, sep="\t", index=True, float_format="%.4f")
    print(f"wrote {path}")

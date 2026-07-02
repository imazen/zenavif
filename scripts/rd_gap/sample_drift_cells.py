#!/usr/bin/env python3
"""Stratified sample of canonical zenavif_lossy cells for the P0 label-drift check
(docs/FEATURE_HINTS_PLAN.md P0.2).

Reads the canonical picker train split, picks N_ORIGINS origins spread across the
origin-id space x one rendition each (stratified across the rendition size ladder),
x CELLS x all 7 q points, and writes a TSV whose FIRST column is the absolute local
corpus PNG path (sync.sh contract: `tail -n +2 | cut -f1` = files to ship).

The (cell, fp, q) triple identifies the exact planner cell; examples/drift_reencode.rs
regenerates the plan (modes_full, budget 400, q-grid {5,15,30,50,70,85,95} — pinned by
the run's manifests/box-0.plan.json) and refuses to run on any fingerprint mismatch.

Usage: python3 sample_drift_cells.py [--out drift_sample.tsv]
"""

import argparse
import os
import sys

import pyarrow.parquet as pq

PARQUET = "/mnt/v/output/canonical-picker-2026-07-01-zensimA/zenavif_lossy/train.parquet"
CORPUS = "/mnt/v/output/clean-picker-corpus-2026-06-26"
# 6 cells spanning every knob axis of the 48-cell grid: speeds {2,4,6,8},
# qm off, 4:2:0, 10-bit, RGB (identity matrix), plus the all-default stratum (s4).
CELLS = ["s2", "s4", "s6-420", "s8-noqm", "s4-bd10", "s6-rgb"]
N_ORIGINS = 8
# Rendition pixel-count bands to stratify across (min_px, max_px): 2 small,
# 4 medium, 2 large — drift may be size-dependent, and s2 encode cost stays sane.
SIZE_BANDS = [
    (30_000, 90_000),
    (30_000, 90_000),
    (150_000, 400_000),
    (150_000, 400_000),
    (150_000, 400_000),
    (150_000, 400_000),
    (400_000, 700_000),
    (400_000, 700_000),
]

COLS = [
    "origin_id",
    "variant_name",
    "ref_filename",
    "cell",
    "fp",
    "q",
    "encoded_bytes",
    "encode_ms",
    "score_ssim2",
    "score_zensim",
    "width",
    "height",
]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(os.path.dirname(__file__), "drift_sample.tsv"))
    args = ap.parse_args()

    t = pq.ParquetFile(PARQUET).read(columns=COLS)
    rows = t.to_pylist()

    # index: (origin, variant) -> pixel count; and full row lookup
    by_key = {}
    for r in rows:
        if r["cell"] not in CELLS:
            continue
        key = (r["origin_id"], r["variant_name"])
        by_key.setdefault(key, []).append(r)

    variants = {}  # (origin, variant) -> px
    for (origin, variant), rs in by_key.items():
        px = int(rs[0]["width"]) * int(rs[0]["height"])
        variants[(origin, variant)] = px

    # Deterministic origin spread: sort origins, take evenly spaced picks, and for
    # each pick the variant closest to its size band (skip origins with no variant
    # in band; walk forward). No RNG — reruns are identical.
    origins = sorted({o for (o, _v) in variants})
    if len(origins) < N_ORIGINS:
        sys.exit(f"only {len(origins)} origins with chosen cells")
    stride = len(origins) / N_ORIGINS
    picked = []  # (origin, variant)
    used = set()
    for i, (lo, hi) in enumerate(SIZE_BANDS):
        # walk from the i-th stride position until an origin has a variant in band
        start = int(i * stride)
        chosen = None
        for j in range(len(origins)):
            o = origins[(start + j) % len(origins)]
            if o in used:
                continue
            cands = [
                (abs(px - (lo + hi) // 2), v)
                for (oo, v), px in variants.items()
                if oo == o and lo <= px <= hi
            ]
            if cands:
                cands.sort()
                chosen = (o, cands[0][1])
                break
        if chosen is None:
            sys.exit(f"no origin found with a variant in size band {lo}-{hi}")
        used.add(chosen[0])
        picked.append(chosen)

    out_rows = []
    for key in picked:
        for r in by_key[key]:
            png = os.path.join(CORPUS, r["ref_filename"])
            if not os.path.isfile(png):
                sys.exit(f"missing corpus image: {png}")
            out_rows.append(
                [
                    png,
                    r["cell"],
                    r["fp"],
                    f"{r['q']:g}",
                    str(int(r["encoded_bytes"])),
                    f"{r['encode_ms']:.3f}",
                    f"{r['score_ssim2']:.6f}",
                    f"{r['score_zensim']:.6f}",
                    str(r["origin_id"]),
                    r["variant_name"],
                    str(int(r["width"])),
                    str(int(r["height"])),
                ]
            )

    out_rows.sort(key=lambda x: (x[9], x[1], float(x[3])))
    hdr = [
        "local_png",
        "cell",
        "fp",
        "q",
        "stored_bytes",
        "stored_encode_ms",
        "stored_ssim2",
        "stored_zensim",
        "origin_id",
        "variant_name",
        "width",
        "height",
    ]
    with open(args.out, "w") as f:
        f.write("\t".join(hdr) + "\n")
        for r in out_rows:
            f.write("\t".join(r) + "\n")
    n_img = len(picked)
    print(f"wrote {args.out}: {len(out_rows)} cells = {n_img} images x {len(CELLS)} cells x 7 q")
    for o, v in picked:
        print(f"  origin {o}: {v} ({variants[(o, v)]} px)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

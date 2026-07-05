#!/usr/bin/env python3
"""Byte-continuity gate: a fresh env-off rd_gap TSV must reproduce the label
store's freshest same-config rows byte-for-byte (bytes column per image x q).

The tune-marginal-drift rule requires fresh baselines for every A/B, and THIS
gate is what proves the fresh binary is RD-identical to the store lineage
before any arm data is trusted (the SSIMRD program's 288/288 pattern).

Usage:
  check_continuity.py FRESH.tsv --store labels.parquet --arm ssimrd/base_s2
"""
import argparse
import csv
import os
import sys

import pandas as pd


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("fresh_tsv")
    ap.add_argument(
        "--store",
        default="/mnt/v/output/zenavif/hyperparam-labels-2026-07-03/labels.parquet",
    )
    ap.add_argument("--arm", default="ssimrd/base_s2")
    args = ap.parse_args()

    store = pd.read_parquet(args.store)
    ref = store[store["arm_id"] == args.arm]
    if ref.empty:
        print(f"FATAL: no rows for arm_id={args.arm} in {args.store}")
        return 2
    # store image_id is the rendition basename (no ext) in prior programs;
    # normalize both sides to basename-no-ext.
    refmap = {}
    for _, r in ref.iterrows():
        key = (os.path.splitext(os.path.basename(str(r["image_id"])))[0], int(r["q"]))
        refmap[key] = int(r["bytes"])

    match = mismatch = missing = 0
    bad = []
    with open(args.fresh_tsv) as f:
        for row in csv.DictReader(f, delimiter="\t"):
            key = (os.path.splitext(os.path.basename(row["image"]))[0], int(row["q"]))
            b = int(row["bytes"])
            if key not in refmap:
                missing += 1
                continue
            if refmap[key] == b:
                match += 1
            else:
                mismatch += 1
                bad.append((key, refmap[key], b))

    total = match + mismatch
    print(f"byte-continuity vs {args.arm}: {match}/{total} match, "
          f"{mismatch} mismatch, {missing} fresh rows without a store twin")
    for key, want, got in bad[:10]:
        print(f"  MISMATCH {key}: store={want} fresh={got}")
    if mismatch:
        print("GATE FAILED — the fresh binary is NOT byte-continuous with the store lineage.")
        return 1
    if total == 0:
        print("GATE INCONCLUSIVE — no overlapping cells.")
        return 2
    print("GATE PASSED.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

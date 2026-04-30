"""
Extract a per-(speed, size_class) encode_ms-per-megapixel lookup table
from a predictor_sweep TSV. Used by the auto_tune runtime to mask
cells whose predicted encode_ms exceeds a user-supplied time budget,
without requiring a second MLP head.

Output format: a small JSON sidecar that the auto_tune runtime
deserializes at startup. Schema:

  {
    "schema_version": 1,
    "median_ms_per_mpx": {
      "speed1": {"tiny": 12.5, "small": 47.1, "medium": 220.4, "large": 1040.0},
      "speed2": ...,
      ...
    },
    "p90_ms_per_mpx": { ... }
  }

Usage:
  python3 build_encode_ms_lut.py \
    --pareto benchmarks/rav1e_phase1a_2026-04-30.tsv \
    --output benchmarks/rav1e_encode_ms_lut_v0_1.json
"""

from __future__ import annotations

import argparse
import csv
import json
import statistics
from collections import defaultdict
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pareto", type=Path, required=True)
    ap.add_argument("--output", type=Path, required=True)
    args = ap.parse_args()

    # Group encode_ms / mpx by (speed, size_class).
    samples: dict[tuple[int, str], list[float]] = defaultdict(list)
    with open(args.pareto) as f:
        rdr = csv.DictReader(f, delimiter="\t")
        for r in rdr:
            try:
                speed = int(r["speed"])
                size_class = r["size_class"]
                w = int(r["width"])
                h = int(r["height"])
                encode_ms = float(r["encode_ms"])
            except (ValueError, KeyError, TypeError):
                # Skip partial / corrupt rows (e.g. last in-flight row of
                # a live sweep TSV).
                continue
            if not size_class:
                continue
            mpx = (w * h) / 1_000_000.0
            if mpx <= 0:
                continue
            samples[(speed, size_class)].append(encode_ms / mpx)

    median_lut: dict[str, dict[str, float]] = defaultdict(dict)
    p90_lut: dict[str, dict[str, float]] = defaultdict(dict)
    for (speed, size_class), vals in samples.items():
        if not vals:
            continue
        vals.sort()
        median_lut[f"speed{speed}"][size_class] = round(statistics.median(vals), 3)
        p90_idx = int(len(vals) * 0.9)
        p90_lut[f"speed{speed}"][size_class] = round(
            vals[min(p90_idx, len(vals) - 1)], 3
        )

    out = {
        "schema_version": 1,
        "source": str(args.pareto),
        "median_ms_per_mpx": dict(median_lut),
        "p90_ms_per_mpx": dict(p90_lut),
    }
    args.output.write_text(json.dumps(out, indent=2, sort_keys=True))
    print(f"wrote {args.output} with {len(samples)} (speed, size) cells")
    # Print median latency per cell so user can sanity-check
    print("\nMedian ms/MPx by (speed, size_class):")
    for sk in sorted(median_lut.keys(), key=lambda s: int(s.removeprefix("speed"))):
        szs = median_lut[sk]
        cells = " ".join(f"{sz}={ms}" for sz, ms in sorted(szs.items()))
        print(f"  {sk:8s}  {cells}")


if __name__ == "__main__":
    main()

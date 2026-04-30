"""
Build a per-(speed_cell, target_zq) → q lookup table from a
predictor_sweep TSV. The auto_tune runtime uses this to turn the MLP's
cell prediction into an actual encoder `quality` value:

  picker output  →  cell_id (e.g. speed=4)
  user target_zq →  85
  → look up median q in the cell that got images TO 85 zensim
  → pass that q to the encoder

Output schema:
  {
    "schema_version": 1,
    "cells": ["speed1", "speed2", ..., "speed10"],
    "target_zqs": [30, 35, ..., 95],
    "median_q": [
      [42, 47, ..., 95],   # per target_zq for cell speed1
      [41, 46, ..., 95],   # speed2
      ...
    ]
  }

For each (cell, target_zq) we take the smallest q whose row achieved
zensim ≥ target_zq, median across (image, size). If no rows in the
cell reach a given target_zq, we mark with q=-1 (meaning "this cell
cannot reach this target — runtime should mask it out").

Usage:
  python3 build_quality_lut.py \
    --pareto benchmarks/rav1e_phase1a_2026-04-30.tsv \
    --output benchmarks/rav1e_quality_lut_v0_1.json
"""

from __future__ import annotations

import argparse
import csv
import json
import statistics
from collections import defaultdict
from pathlib import Path


# Match training/rav1e_picker_config.py ZQ_TARGETS
TARGET_ZQS = list(range(30, 70, 5)) + list(range(70, 96, 2))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pareto", type=Path, required=True)
    ap.add_argument("--output", type=Path, required=True)
    args = ap.parse_args()

    # Per (cell, image_path, size_class) -> sorted list of (q, zensim)
    per_cell_image: dict[tuple[str, str, str], list[tuple[int, float]]] = defaultdict(
        list
    )
    cells_seen: set[str] = set()

    with open(args.pareto) as f:
        rdr = csv.DictReader(f, delimiter="\t")
        for r in rdr:
            try:
                speed = int(r["speed"])
                q = int(r["q"])
                zensim = float(r["zensim"])
                image_path = r["image_path"]
                size_class = r["size_class"]
            except (ValueError, KeyError, TypeError):
                continue
            if not image_path or not size_class:
                continue
            cell = f"speed{speed}"
            cells_seen.add(cell)
            per_cell_image[(cell, image_path, size_class)].append((q, zensim))

    cells = sorted(cells_seen, key=lambda s: int(s.removeprefix("speed")))
    median_q: list[list[int]] = []

    for cell in cells:
        per_zq: list[int] = []
        for tz in TARGET_ZQS:
            qs_at_target: list[int] = []
            # For each (image, size), find the smallest q whose zensim >= tz.
            for (c, ip, sz), pairs in per_cell_image.items():
                if c != cell:
                    continue
                pairs_sorted = sorted(pairs, key=lambda p: p[0])
                hit = next((q for q, z in pairs_sorted if z >= tz), None)
                if hit is not None:
                    qs_at_target.append(hit)
            if qs_at_target:
                per_zq.append(int(round(statistics.median(qs_at_target))))
            else:
                per_zq.append(-1)
        median_q.append(per_zq)

    out = {
        "schema_version": 1,
        "source": str(args.pareto),
        "cells": cells,
        "target_zqs": TARGET_ZQS,
        "median_q": median_q,
    }
    args.output.write_text(json.dumps(out, indent=2))
    print(f"wrote {args.output} ({len(cells)} cells × {len(TARGET_ZQS)} target_zqs)")
    # Print a summary so the user can sanity-check
    print("\nMedian q at target_zq=85 by cell:")
    idx_85 = TARGET_ZQS.index(85) if 85 in TARGET_ZQS else len(TARGET_ZQS) // 2
    tz = TARGET_ZQS[idx_85]
    for cell, row in zip(cells, median_q):
        q = row[idx_85]
        marker = "" if q >= 0 else " (unreachable)"
        print(f"  {cell:8s}  target_zq={tz}  → q={q}{marker}")


if __name__ == "__main__":
    main()

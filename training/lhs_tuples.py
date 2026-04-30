"""
Generate Latin Hypercube samples over the surviving Phase 2 knob axes.

Output: a JSON file with N tuples, each describing one configuration
of the v0.2 surviving knobs. The cron wrapper picks one tuple per night
(round-robin by day-of-year) so the corpus accumulates joint coverage
without a single multi-day sweep.

Knob axes (Phase 2 OAT survivors):
  qm                ∈ {0, 1}
  vaq_strength      ∈ {0.5, 1.0, 1.5, 2.0, 3.0}
  seg_boost         ∈ {1.0, 1.25, 1.5, 1.75, 2.0}
  rdo_tx_off        ∈ {0, 1}
  seg_complex_on    ∈ {0, 1}
  bottomup_on       ∈ {0, 1}
  lrf_on            ∈ {0, 1}
  partition_range   ∈ {-1, 0, +1}

Cardinality: 2 × 5 × 5 × 2 × 2 × 2 × 2 × 3 = 4800 tuples — too many to
sweep all. LHS samples N (default 64) representative tuples uniformly.
For the cron rotation, N=64 + day-of-year mod 64 = each tuple covered
once every ~9 weeks → over a quarter, full coverage.

Usage:
  python3 lhs_tuples.py --n 64 --seed 42 \\
    --output training/rav1e_lhs_tuples_v0_2.json
"""

from __future__ import annotations

import argparse
import json
import math
import random
from pathlib import Path


QM = [0, 1]
VAQ_STRENGTH = [0.5, 1.0, 1.5, 2.0, 3.0]
SEG_BOOST = [1.0, 1.25, 1.5, 1.75, 2.0]
RDO_TX_OFF = [0, 1]
SEG_COMPLEX_ON = [0, 1]
BOTTOMUP_ON = [0, 1]
LRF_ON = [0, 1]
PARTITION_RANGE_IDX = [-1, 0, 1]

AXES = [
    ("qm", QM),
    ("vaq_strength", VAQ_STRENGTH),
    ("seg_boost", SEG_BOOST),
    ("rdo_tx_off", RDO_TX_OFF),
    ("seg_complex_on", SEG_COMPLEX_ON),
    ("bottomup_on", BOTTOMUP_ON),
    ("lrf_on", LRF_ON),
    ("partition_range_idx", PARTITION_RANGE_IDX),
]


def latin_hypercube(n: int, seed: int) -> list[dict]:
    """Stratified Latin Hypercube over the discrete axes.

    Each axis splits into `n` strata; each stratum gets exactly one
    sample. For axes with fewer values than `n`, strata index modulo
    cardinality (still uniform marginal in expectation).
    """
    rng = random.Random(seed)
    cols: list[list[object]] = []
    for _name, vals in AXES:
        # Shuffle the axis values so column ordering is independent.
        permuted = list(range(n))
        rng.shuffle(permuted)
        col = [vals[p % len(vals)] for p in permuted]
        cols.append(col)
    rows: list[dict] = []
    for i in range(n):
        row = {name: cols[j][i] for j, (name, _) in enumerate(AXES)}
        rows.append(row)
    return rows


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=64)
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--output", type=Path, required=True)
    args = ap.parse_args()

    tuples = latin_hypercube(args.n, args.seed)
    out = {
        "schema_version": 1,
        "n": args.n,
        "seed": args.seed,
        "axes": [name for name, _ in AXES],
        "tuples": tuples,
    }
    args.output.write_text(json.dumps(out, indent=2))
    print(f"wrote {args.output} with {args.n} tuples")
    # print a few to sanity-check coverage
    for i in [0, args.n // 4, args.n // 2, args.n - 1]:
        print(f"  tuple[{i}]: {tuples[i]}")


if __name__ == "__main__":
    main()

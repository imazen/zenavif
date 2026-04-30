"""
Analyze a Phase 2 OAT TSV and produce per-knob cull recommendations.

For each (image, size, knob, perturbation) row, compute Δ% bytes,
Δ zensim, Δ encode_ms vs the baseline row at the same (image, size).
Aggregate across (image, size) cells: median + p90 + count.

Cull rule (per docs/RAV1E_PICKER_PLAN.md):
  median |Δ% bytes| < 0.5 % AND p90 |Δ% bytes| < 1.5 %  →  drop knob

Output:
  - Pretty-printed table to stdout
  - JSON sidecar (--out-json) with the structured decisions

Usage:
  python3 analyze_phase2_oat.py \
    --in benchmarks/rav1e_phase2_oat_2026-04-30.tsv \
    --out-json benchmarks/rav1e_phase2_decisions_2026-04-30.json
"""

from __future__ import annotations

import argparse
import csv
import json
import statistics
from collections import defaultdict
from pathlib import Path

CULL_MEDIAN_PCT = 0.5
CULL_P90_PCT = 1.5


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="inp", type=Path, required=True)
    ap.add_argument("--out-json", type=Path, default=None)
    args = ap.parse_args()

    # Build {(image, size): {(knob, perturbation): row}}
    cells: dict[tuple[str, str], dict[tuple[str, str], dict]] = defaultdict(dict)
    with open(args.inp) as f:
        rdr = csv.DictReader(f, delimiter="\t")
        for r in rdr:
            try:
                key = (r["image_path"], r["size_class"])
                pert_key = (r["knob"], r["perturbation"])
                cells[key][pert_key] = {
                    "bytes": int(r["bytes"]),
                    "zensim": float(r["zensim"]),
                    "encode_ms": float(r["encode_ms"]),
                }
            except (ValueError, KeyError, TypeError):
                continue

    # For each knob+perturbation, compute Δ vs baseline across cells
    deltas: dict[tuple[str, str], list[dict]] = defaultdict(list)
    skipped_no_baseline = 0
    for cell_key, perts in cells.items():
        baseline = perts.get(("baseline", "default"))
        if not baseline:
            skipped_no_baseline += 1
            continue
        for pk, row in perts.items():
            if pk == ("baseline", "default"):
                continue
            d_bytes_pct = (row["bytes"] - baseline["bytes"]) / max(baseline["bytes"], 1) * 100.0
            d_zensim = row["zensim"] - baseline["zensim"]
            d_ms = row["encode_ms"] - baseline["encode_ms"]
            deltas[pk].append(
                {
                    "image_size": cell_key,
                    "d_bytes_pct": d_bytes_pct,
                    "d_zensim": d_zensim,
                    "d_encode_ms": d_ms,
                    "baseline_bytes": baseline["bytes"],
                    "baseline_zensim": baseline["zensim"],
                    "perturbed_bytes": row["bytes"],
                }
            )

    print(f"Cells with baseline: {len(cells) - skipped_no_baseline}/{len(cells)}")
    print()
    print(
        f"{'knob':<28} {'value':<18} "
        f"{'n':>4} "
        f"{'med Δ%B':>9} {'p90 |Δ%B|':>10} "
        f"{'med Δzs':>8} {'med Δms':>9} "
        f"{'verdict':<8}"
    )
    print("-" * 100)

    decisions: dict[str, dict] = {}
    for pk, rows in sorted(deltas.items(), key=lambda kv: kv[0]):
        knob, value = pk
        n = len(rows)
        if n == 0:
            continue
        d_bytes = [r["d_bytes_pct"] for r in rows]
        abs_d_bytes = [abs(x) for x in d_bytes]
        d_zensim = [r["d_zensim"] for r in rows]
        d_ms = [r["d_encode_ms"] for r in rows]
        med_bytes = statistics.median(d_bytes)
        med_abs_bytes = statistics.median(abs_d_bytes)
        n_p90 = max(0, int(round(0.9 * n)) - 1)
        sorted_abs = sorted(abs_d_bytes)
        p90_abs_bytes = sorted_abs[min(n_p90, n - 1)] if n else 0.0
        med_zs = statistics.median(d_zensim)
        med_ms = statistics.median(d_ms)

        cull = med_abs_bytes < CULL_MEDIAN_PCT and p90_abs_bytes < CULL_P90_PCT
        verdict = "CULL" if cull else "KEEP"
        print(
            f"{knob:<28} {value:<18} "
            f"{n:>4} "
            f"{med_bytes:>+8.2f}% {p90_abs_bytes:>9.2f}% "
            f"{med_zs:>+7.3f} {med_ms:>+8.2f}ms "
            f"{verdict:<8}"
        )
        decisions[f"{knob}/{value}"] = {
            "knob": knob,
            "perturbation": value,
            "n_cells": n,
            "median_d_bytes_pct": round(med_bytes, 3),
            "median_abs_d_bytes_pct": round(med_abs_bytes, 3),
            "p90_abs_d_bytes_pct": round(p90_abs_bytes, 3),
            "median_d_zensim": round(med_zs, 4),
            "median_d_encode_ms": round(med_ms, 2),
            "cull": cull,
        }

    print()
    survivors = sorted({d["knob"] for d in decisions.values() if not d["cull"]})
    culls = sorted({
        d["knob"]
        for d in decisions.values()
        if d["cull"] and d["knob"] not in survivors
    })
    print(f"SURVIVORS ({len(survivors)}): {', '.join(survivors)}")
    print(f"CULLED    ({len(culls)}): {', '.join(culls)}")

    if args.out_json:
        out = {
            "schema_version": 1,
            "source": str(args.inp),
            "cull_thresholds": {
                "median_pct": CULL_MEDIAN_PCT,
                "p90_pct": CULL_P90_PCT,
            },
            "decisions": decisions,
            "survivors": survivors,
            "culled": culls,
        }
        args.out_json.write_text(json.dumps(out, indent=2, sort_keys=True))
        print(f"\nWrote {args.out_json}")


if __name__ == "__main__":
    main()

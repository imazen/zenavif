"""
Filter a predictor_sweep TSV to drop truncated / in-flight rows.

A live `predictor_sweep` writes through a buffered Mutex<Vec<u8>>, so
the last few lines of the file may be partially written. The
`load_pareto` step in `train_hybrid.py` doesn't tolerate truncated
rows (TypeError on `int(r["bytes"])` when bytes is None), so this
helper strips incomplete rows and emits a clean TSV the trainer can
consume.

Usage:
  python3 clean_tsv.py --in benchmarks/rav1e_phase1a_2026-04-30.tsv \
                       --out /tmp/rav1e_phase1a_clean.tsv
"""

from __future__ import annotations

import argparse
from pathlib import Path


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="inp", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    with open(args.inp) as f:
        header = f.readline().rstrip("\n")
        ncols = len(header.split("\t"))
        kept = 0
        dropped = 0
        with open(args.out, "w") as g:
            g.write(header + "\n")
            for line in f:
                if line.endswith("\n"):
                    cols = line.rstrip("\n").split("\t")
                    if len(cols) == ncols and all(c != "" for c in cols):
                        g.write(line)
                        kept += 1
                    else:
                        dropped += 1
                else:
                    # Truncated last line (no terminating newline).
                    dropped += 1
    print(f"kept {kept}, dropped {dropped} rows → {args.out}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Did the encoder actually move? Compare two sweeps cell by cell.

When a sibling crate bumps underneath a measurement, the tempting move is
to assume every prior number is stale and re-run everything. The cheaper
and more honest move is to measure the blast radius: join two sweeps on
identical (image, size, QUANTIZER) triples -- the quantizer, because that
is what the encode actually depends on -- and report how much moved.

Reports the perfect-identity subset separately from the aggregate on
purpose. "72 of 168 cells are byte-identical and the median delta is zero"
is a weaker statement than "these 31 overlapping cells are identical in
both bytes and score"; a skeptical reader should be handed the second one
first, because a small mean can hide a bimodal population and an exact
identity cannot.

SCOPE WARNING, and please carry it into whatever you write: this compares
the cells you happen to have in BOTH sweeps, at whatever encoder config
produced them. A null here means "no material change AT THESE CONFIGS on
THESE CELLS". It does not license "the bump was small" as a general claim
-- encoder changes are usually speed-preset and content dependent, so a
null on a fast-tier photo corpus says nothing about a deep-tier screen
one.

Usage:
    compare_lattice_encoders.py OLD.tsv[.zst] NEW.tsv[.zst] [more_new.tsv ...]
"""

from __future__ import annotations

import csv
import io
import math
import statistics
import subprocess
import sys
from collections import defaultdict


def read(path):
    if path.endswith(".zst"):
        text = subprocess.run(["zstd", "-dc", path], capture_output=True,
                              text=True, check=True).stdout
    else:
        text = open(path).read()
    out = {}
    for r in csv.DictReader(io.StringIO(text), delimiter="\t"):
        if not r.get("image") or r["image"].startswith("#"):
            continue
        try:
            key = (r["image"], int(r["size"]), int(r["qindex"]))
            out[key] = (int(r["bytes"]), float(r["zensim"]))
        except (KeyError, ValueError):
            continue
    return out


def pct(v, p):
    if not v:
        return float("nan")
    s = sorted(v)
    k = (len(s) - 1) * p / 100.0
    lo, hi = math.floor(k), math.ceil(k)
    return s[int(k)] if lo == hi else s[lo] + (s[hi] - s[lo]) * (k - lo)


def main(argv):
    old = read(argv[0])
    new = {}
    for p in argv[1:]:
        new.update(read(p))
    common = sorted(set(old) & set(new))
    print(f"# old rows {len(old)}  new rows {len(new)}  comparable {len(common)}")
    if not common:
        print("# nothing comparable -- the two sweeps share no (image, size, quantizer)")
        return
    ds = [new[k][1] - old[k][1] for k in common]
    db = [(new[k][0] - old[k][0]) / max(1, old[k][0]) for k in common]
    ident_b = [k for k in common if old[k][0] == new[k][0]]
    ident_both = [k for k in ident_b if abs(old[k][1] - new[k][1]) < 1e-4]
    print(f"# byte-identical      : {len(ident_b)}/{len(common)}")
    print(f"# byte+score identical: {len(ident_both)}/{len(common)}")
    print(f"# zensim delta: median {statistics.median(ds):+.4f}  "
          f"|d| median {statistics.median([abs(x) for x in ds]):.4f}  "
          f"|d| p90 {pct([abs(x) for x in ds], 90):.4f}  "
          f"max {max(abs(x) for x in ds):.4f}")
    print(f"# bytes  delta: median {statistics.median(db):+.3%}  "
          f"|d| p90 {pct([abs(x) for x in db], 90):.3%}  "
          f"max {max(abs(x) for x in db):.3%}")

    print("\n# per source (a change concentrated in one source is not a small change)")
    print("source\tn\tident\t|dscore|_med\t|dscore|_max")
    by = defaultdict(list)
    for k in common:
        by[k[0]].append(k)
    for img in sorted(by):
        ks = by[img]
        v = [abs(new[k][1] - old[k][1]) for k in ks]
        i = sum(1 for k in ks if old[k] == new[k])
        print(f"{img[:20]}\t{len(ks)}\t{i}/{len(ks)}\t{statistics.median(v):.4f}\t{max(v):.4f}")

    print("\n# per size")
    print("size\tn\tident\t|dscore|_med\t|dscore|_max")
    bys = defaultdict(list)
    for k in common:
        bys[k[1]].append(k)
    for sz in sorted(bys):
        ks = bys[sz]
        v = [abs(new[k][1] - old[k][1]) for k in ks]
        i = sum(1 for k in ks if old[k] == new[k])
        print(f"{sz}\t{len(ks)}\t{i}/{len(ks)}\t{statistics.median(v):.4f}\t{max(v):.4f}")

    print("\n# SCOPE: this is a null AT THESE CONFIGS ON THESE CELLS only.")


if __name__ == "__main__":
    main(sys.argv[1:])

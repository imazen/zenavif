#!/usr/bin/env python3
"""Cross-arm AV1 *bitstream* identity — sha256 of the payload, not its length.

`analyze_matched.py` section A1 compares `bytes_av1`, a byte COUNT. Two arms
landing on the same count is suggestive, not proof: on the sizing probe, svtc
and svtrs matched on count at 1024 px rung 4 (6179 B both), rung 5 (6386 B
both) and rung 8 (7294 B both) while the actual payloads differed in thousands
of bytes. Reporting count agreement as "byte-for-byte identical" overstates it,
so identity gets its own tool that hashes the bytes.

Reads a cells TSV plus the artifact directory it was written with, strips the
IVF container where present (svtc / aom / zenrav1e write IVF, svtrs writes a
bare OBU stream), and reports per (size, ladder) how many shared cells are
truly identical — plus, where they are not, the offset of the first differing
byte, which distinguishes a header-only difference from a coefficient one.

    python3 payload_identity.py cells.tsv --artifacts ~/tmp/encrd2/artifacts
        [--pairs svtc:svtrs]
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import sys
from collections import defaultdict
from pathlib import Path

EXT = {"svtrs": "obu"}          # everything else writes IVF


def payload(p: Path) -> bytes | None:
    if not p.exists():
        return None
    d = p.read_bytes()
    if len(d) >= 32 and d[:4] == b"DKIF":
        off, out = 32, bytearray()
        while off + 12 <= len(d):
            sz = int.from_bytes(d[off:off + 4], "little")
            off += 12
            out += d[off:off + sz]
            off += sz
        return bytes(out)
    return d


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cells")
    ap.add_argument("--artifacts", required=True)
    ap.add_argument("--pairs", default="svtc:svtrs")
    ap.add_argument("--examples", type=int, default=6,
                    help="how many differing cells to characterise in detail")
    args = ap.parse_args()

    art = Path(args.artifacts).expanduser()
    rows = list(csv.DictReader(
        (l for l in open(args.cells) if not l.startswith("#")), delimiter="\t"))
    idx = {}
    for r in rows:
        if r.get("fail") or not r.get("enc_sha256"):
            continue
        ext = EXT.get(r["arm"], "ivf")
        idx[(r["image"], r["size_tag"], r["ladder"], r["rate"], r["arm"])] = \
            art / f"{r['enc_sha256']}.{ext}"

    for spec in args.pairs.split(","):
        A, B = spec.split(":")
        keys = ({k[:4] for k in idx if k[4] == A} & {k[:4] for k in idx if k[4] == B})
        if not keys:
            print(f"\n{A} vs {B}: no shared (image,size,ladder,rate) cell")
            continue
        per = defaultdict(lambda: [0, 0])        # (size, ladder) -> [same, total]
        shown = []
        missing = 0
        for k in sorted(keys):
            pa, pb = payload(idx[k + (A,)]), payload(idx[k + (B,)])
            if pa is None or pb is None:
                missing += 1
                continue
            g = (k[1], k[2])
            per[g][1] += 1
            if hashlib.sha256(pa).digest() == hashlib.sha256(pb).digest():
                per[g][0] += 1
            elif len(shown) < args.examples:
                n = min(len(pa), len(pb))
                first = next((i for i in range(n) if pa[i] != pb[i]), n)
                nd = sum(1 for i in range(n) if pa[i] != pb[i])
                shown.append((k, len(pa), len(pb), first, nd))
        tot_s = sum(v[0] for v in per.values())
        tot_n = sum(v[1] for v in per.values())
        print(f"\n=== {A} vs {B}: AV1 payload sha256 identity ===")
        print(f"  {tot_s}/{tot_n} shared cells byte-identical"
              + (f"  ({missing} artifact(s) missing)" if missing else ""))
        print(f"  {'size':>8} {'rung':>5} {'identical':>12}")
        for g in sorted(per, key=lambda g: (int(g[0]) if g[0].isdigit() else 1 << 30, int(g[1]))):
            s, n = per[g]
            print(f"  {g[0]:>8} {g[1]:>5} {f'{s}/{n}':>12}"
                  + ("" if s == n else "   <-- divergent"))
        # Same COUNT but different BYTES is the case that makes a count-based
        # check lie, so call it out separately.
        if shown:
            print("  first differing byte offset (0 = header; large = coefficient data):")
            for k, la, lb, first, nd in shown:
                tag = "SAME LENGTH, DIFFERENT BYTES" if la == lb else "different length"
                print(f"    {k[0][:28]:<28} @{k[1]:<5} rung {k[2]:>2} rate {k[3]:>3}: "
                      f"{la} vs {lb} B, first diff @{first}, {nd} bytes differ  [{tag}]")
    return 0


if __name__ == "__main__":
    sys.exit(main())

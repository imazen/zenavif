#!/usr/bin/env python3
"""How much of the measured wall clock is NOT encoding.

The harness times the full process — spawn, dynamic linking, input parse,
encode, output write — because that is the only definition applicable uniformly
to four binaries in three repositories, and it is what an image pipeline
actually pays per image. But it means a matched-wall-clock comparison is only
an ENCODER comparison to the extent that encoding dominates the clock, and at
small sizes it does not.

`analyze_matched.py` section C estimates the non-encode part by fitting
`alpha + beta*pixels`. This gets at the same quantity a second, independent
way, with no model: three of the four arms report their own encode time
(`self_ms` — aomenc's us/frame, SvtAv1EncApp's "Total Encoding Time", rav1e's
fps inverted), so `wall - self` is a direct measurement of the overhead.

Two independent estimates agreeing is worth much more than either alone; where
they disagree, the fit is the thing to distrust, because the arms' self-timers
are measuring their own inner loop.

`svtrs` has no self-timer, and additionally decodes a PNG inside its timed
region that the y4m arms do not — so it is listed with a dash and its overhead
has to come from the fit.

    python3 overhead_share.py cells.tsv
"""

from __future__ import annotations

import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from analyze_matched import fnum, read_cells  # noqa: E402


def med(v):
    v = sorted(v)
    return v[len(v) // 2] if v else None


def main() -> int:
    rows, meta = read_cells(sys.argv[1])
    ok = [r for r in rows if not r.get("fail") and fnum(r.get("wall_ms_med"))]
    szs = sorted({r["size_tag"] for r in ok},
                 key=lambda s: int(s) if s.isdigit() else 1 << 30)
    arms = sorted({r["arm"] for r in ok})
    print("\n".join(meta))
    print("\n=== OVERHEAD SHARE: (wall - self) / wall, from the encoders' own timers ===")
    print("    A matched-wall-clock ranking is an ENCODER ranking only where this is")
    print("    small. Where it approaches 1.0 the comparison is measuring process")
    print("    spawn, not compression.")
    print(f"\n    {'arm':<10}{'rung':>5}" + "".join(f"{s + 'px':>26}" for s in szs))
    print(f"    {'':<10}{'':>5}" + "".join(f"{'wall/self ms  overhead%':>26}" for _ in szs))
    for a in arms:
        for lad in sorted({int(r["ladder"]) for r in ok if r["arm"] == a}):
            cells, any_self = [], False
            for s in szs:
                g = [r for r in ok if r["arm"] == a and int(r["ladder"]) == lad
                     and r["size_tag"] == s]
                w = med([fnum(r["wall_ms_med"]) for r in g if fnum(r["wall_ms_med"])])
                sf = med([fnum(r["self_ms"]) for r in g if fnum(r["self_ms"])])
                if w is None:
                    cells.append(f"{'-':>26}")
                elif sf is None:
                    cells.append(f"{w:>9.2f} /{'  n/a':>7}{'    -':>9}")
                else:
                    any_self = True
                    cells.append(f"{w:>9.2f} /{sf:>7.2f}{(w - sf) / w * 100:>8.1f}%")
            # print every rung for arms with a self timer; for svtrs one line is
            # enough to show the wall clock, since the ratio is unavailable
            if any_self or lad == min({int(r["ladder"]) for r in ok if r["arm"] == a}):
                print(f"    {a:<10}{lad:>5}" + "".join(cells))
    print("\n    svtrs has no self-timer AND pays a PNG decode inside its timed region")
    print("    that the y4m arms do not; its overhead must come from the alpha fit")
    print("    (analyze_matched.py C3), bounded above by `rd_tool prep`, which does")
    print("    strictly more work (PNG decode + RGB->I420 + writing both files).")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Summarize cargo-llvm-cov JSON exports (one per feature combo).

  python3 scripts/cov_summarize.py ~/tmp/zenavif-cov/*.json            # per-combo totals + per-file table
  python3 scripts/cov_summarize.py --cold ~/tmp/zenavif-cov/allsafe.json    # cold source-line ranges
  python3 scripts/cov_summarize.py --funcs ~/tmp/zenavif-cov/allsafe.json   # uncovered functions (noisy: per-binary instantiations)
  python3 scripts/cov_summarize.py --file src/yuv_convert.rs ~/tmp/...json  # one file, per-function

The distinction this exists to make: a source file MISSING from a combo's JSON
was NOT BUILT under that combo. It is not 0%-covered code, and averaging it in
either direction is wrong. Missing files print as `--` and are counted
separately from built-but-uncovered ones.
"""

from __future__ import annotations

import json
import os
import re
import sys
from collections import defaultdict

# The lib is instrumented twice per run (once inside the lib test binary, once
# linked into each integration test), so llvm-cov reports the same source
# function under several v0-mangled symbols that differ only in the crate
# disambiguator. Region/line percentages in the FILE summaries are already
# merged by llvm-cov; the FUNCTION list is not, so group by the
# disambiguator-stripped name and take the max execution count. Without this a
# function that ran in the integration tests reads "0 executions" from the lib
# binary's copy of it.
_DISAMBIG = re.compile(r"Cs[0-9A-Za-z]+_")


def strip_disambig(name: str) -> str:
    return _DISAMBIG.sub("", name)

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def rel(path: str) -> str:
    return os.path.relpath(path, REPO) if path.startswith(REPO) else path


def load(p: str):
    with open(p) as fh:
        d = json.load(fh)
    return d["data"][0]


def pct(cov, cnt):
    return 100.0 * cov / cnt if cnt else float("nan")


def combo_name(p: str) -> str:
    return os.path.basename(p).rsplit(".", 1)[0]


def totals_row(name, t):
    return (
        f"{name:<10} lines {t['lines']['covered']:>6}/{t['lines']['count']:<6} "
        f"{pct(t['lines']['covered'], t['lines']['count']):5.1f}%   "
        f"regions {t['regions']['covered']:>6}/{t['regions']['count']:<6} "
        f"{pct(t['regions']['covered'], t['regions']['count']):5.1f}%   "
        f"fns {t['functions']['covered']:>5}/{t['functions']['count']:<5} "
        f"{pct(t['functions']['covered'], t['functions']['count']):5.1f}%"
    )


def per_file(data, prefix_filter=None):
    out = {}
    for f in data["files"]:
        r = rel(f["filename"])
        if prefix_filter and not r.startswith(prefix_filter):
            continue
        s = f["summary"]
        out[r] = s
    return out


def line_counts(f) -> dict[int, int]:
    """Reconstruct per-line execution counts from llvm-cov segments.

    Merged across every instantiation (llvm-cov emits one segment stream per
    file, not per monomorphization), which is what makes this the honest view:
    the per-FUNCTION list instead carries one entry per test/example binary the
    generic was instantiated into, and the copies inside never-run example
    binaries read cold whatever the library actually executed.
    """
    counts: dict[int, int] = {}
    cur = None
    prev_line = None
    for line, _col, count, has_count, _entry, _gap in f["segments"]:
        if prev_line is not None and cur is not None:
            for ln in range(prev_line, line):
                counts[ln] = max(counts.get(ln, 0), cur)
        if has_count:
            cur = count
            counts[line] = max(counts.get(line, 0), count)
        else:
            cur = None
        prev_line = line
    return counts


def cold_ranges(f, min_len=1):
    counts = line_counts(f)
    cold = sorted(ln for ln, c in counts.items() if c == 0)
    out = []
    for ln in cold:
        if out and ln == out[-1][1] + 1:
            out[-1][1] = ln
        else:
            out.append([ln, ln])
    return [(a, b) for a, b in out if b - a + 1 >= min_len]


def main() -> int:
    args = sys.argv[1:]
    mode = "table"
    only_file = None
    if args and args[0] == "--cold":
        mode, args = "cold", args[1:]
    elif args and args[0] == "--funcs":
        mode, args = "funcs", args[1:]
    elif args and args[0] == "--file":
        mode, only_file, args = "file", args[1], args[2:]
    if not args:
        print(__doc__)
        return 2

    datasets = {combo_name(p): load(p) for p in args}

    if mode == "table":
        print("== per-combo totals (whole workspace incl. members) ==")
        for name, d in datasets.items():
            print(totals_row(name, d["totals"]))
        print()
        files = {n: per_file(d) for n, d in datasets.items()}
        allfiles = sorted({f for m in files.values() for f in m})
        names = list(datasets)
        hdr = "file".ljust(46) + "".join(f"{n:>14}" for n in names)
        print("== per-file REGION coverage (`--` = not built in that combo) ==")
        print(hdr)
        for fn in allfiles:
            cells = []
            for n in names:
                s = files[n].get(fn)
                if s is None:
                    cells.append(f"{'--':>14}")
                else:
                    r = s["regions"]
                    cells.append(f"{pct(r['covered'], r['count']):11.1f}% ")
            print(fn.ljust(46) + "".join(cells))
        print()
        print("== region counts per file (built combos only) ==")
        print("file".ljust(46) + "".join(f"{n:>14}" for n in names))
        for fn in allfiles:
            cells = []
            for n in names:
                s = files[n].get(fn)
                cells.append(f"{'--':>14}" if s is None else f"{s['regions']['covered']}/{s['regions']['count']:<8}".rjust(14))
            print(fn.ljust(46) + "".join(cells))
        return 0

    # --cold / --funcs / --file: single dataset expected (uses the first)
    name, d = next(iter(datasets.items()))

    if mode == "cold":
        print(f"== {name}: cold source-line ranges (never executed), longest first ==")
        rows = []
        for f in d["files"]:
            r = rel(f["filename"])
            rngs = cold_ranges(f, min_len=int(os.environ.get("COV_MIN_COLD", "3")))
            if rngs:
                rows.append((sum(b - a + 1 for a, b in rngs), r, rngs))
        for total, r, rngs in sorted(rows, reverse=True):
            print(f"\n-- {r}: {total} cold lines")
            for a, b in sorted(rngs, key=lambda t: -(t[1] - t[0])):
                print(f"   {r}:{a}-{b}  ({b - a + 1} lines)")
        return 0

    byfile: dict[str, dict[tuple[int, str], dict]] = defaultdict(dict)
    for fn in d["functions"]:
        line = fn["regions"][0][0] if fn["regions"] else 0
        key = (line, strip_disambig(fn["name"]))
        for fname in fn["filenames"]:
            slot = byfile[rel(fname)]
            prev = slot.get(key)
            # Merge duplicate instantiations: keep the highest-count copy, and
            # keep its region tally alongside so "0 executions" means every copy
            # was cold.
            if prev is None or fn["count"] > prev["count"]:
                slot[key] = fn
    if mode == "funcs":
        print(f"== {name}: functions with 0 executions (all instantiations cold), grouped by file ==")
        rows = []
        for f, slot in byfile.items():
            fns = list(slot.values())
            zero = [x for x in fns if x["count"] == 0]
            if zero:
                cold_regions = sum(len(x["regions"]) for x in zero)
                rows.append((cold_regions, len(zero), len(fns), f, zero))
        for cold_regions, nz, tot, f, zero in sorted(rows, reverse=True):
            print(f"\n-- {f}: {nz}/{tot} functions never executed ({cold_regions} cold regions)")
            for x in sorted(zero, key=lambda y: y["regions"][0][0] if y["regions"] else 0):
                line = x["regions"][0][0] if x["regions"] else 0
                print(f"   {f}:{line}  regions={len(x['regions']):<4} {strip_disambig(x['name'])}")
        return 0
    # mode == "file"
    fns = list(byfile.get(only_file, {}).values())
    print(f"== {name}: {only_file} ({len(fns)} distinct functions after instantiation merge) ==")
    for x in sorted(fns, key=lambda y: y["regions"][0][0] if y["regions"] else 0):
        line = x["regions"][0][0] if x["regions"] else 0
        nregion = len(x["regions"])
        uncov = sum(1 for r in x["regions"] if r[4] == 0)
        print(f"   {line:>5}  count={x['count']:<10} regions {nregion - uncov}/{nregion:<5} {strip_disambig(x['name'])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

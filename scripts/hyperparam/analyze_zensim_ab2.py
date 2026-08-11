#!/usr/bin/env python3
"""Two-shot precision at a FIXED budget of two encodes: the error
distribution, decomposed.

Reads `zensim_loop_bench ab2` TSV (real encodes, every arm capped at two
encodes) and, optionally, the dense per-quantizer lattice TSV for the same
cells so the error can be split into the two terms that behave completely
differently:

    |achieved - target|  =  LATTICE term  +  PREDICTION term

  LATTICE term    the distance from the target to the nearest achievable
                  score. Irreducible: no search, however good, lands
                  between two encodes the codec can produce.
  PREDICTION term how far the rule's chosen quantizer sits from the one
                  that WOULD have been nearest. This is the only part any
                  amount of modelling can remove.

Reporting the total alone hides which one is binding, and the answer
decides where effort goes. (It has already been wrong once on this task:
the lattice term was believed to dominate, from a sweep taken on an
integer-QUALITY grid that addresses only 100 of the codec's 256
quantizers.)

Usage:
    analyze_zensim_ab2.py ab2.tsv [--lattice lat.tsv ...]
"""

from __future__ import annotations

import argparse
import csv
import math
import statistics
from collections import defaultdict


def pct(v, p):
    if not v:
        return float("nan")
    s = sorted(v)
    k = (len(s) - 1) * p / 100.0
    lo, hi = math.floor(k), math.ceil(k)
    return s[int(k)] if lo == hi else s[lo] + (s[hi] - s[lo]) * (k - lo)


def load_lattice(paths):
    """(image, size) -> {quantizer: score}."""
    cells = defaultdict(dict)
    for p in paths:
        with open(p) as f:
            for r in csv.DictReader(f, delimiter="\t"):
                cells[(r["image"], int(r["size"]))][int(r["qindex"])] = float(r["zensim"])
    return cells


HDR = ("arm\tn\tmed|err|\tp90\tp99\tmax\tmean_enc\tnearest_hit\twithin1\t"
       "med_lattice\tmed_pred\tover%\tmed_bytes\tmed_ms")


def summarise(rows, label, lattice, out=print):
    if not rows:
        out(f"{label}\t(no rows)")
        return
    err = [abs(r["err"]) for r in rows]
    enc = [r["encodes"] for r in rows]
    over = sum(1 for r in rows if r["err"] > 0) / len(rows)
    lat_terms, pred_terms, hits, within1 = [], [], [], []
    for r in rows:
        key = (r["image"], r["size"])
        tab = lattice.get(key)
        if not tab:
            continue
        t = r["target"]
        best_qi = min(tab, key=lambda q: abs(tab[q] - t))
        lat_terms.append(abs(tab[best_qi] - t))
        # prediction term: what the achieved error would NOT have been if
        # the rule had picked the nearest achievable point instead.
        pred_terms.append(max(0.0, abs(r["err"]) - abs(tab[best_qi] - t)))
        if r["qi"] >= 0:
            hits.append(1.0 if r["qi"] == best_qi else 0.0)
            within1.append(1.0 if abs(r["qi"] - best_qi) <= 1 else 0.0)
    f = lambda v: f"{statistics.mean(v):.1%}" if v else "n/a"
    g = lambda v: f"{statistics.median(v):.4f}" if v else "n/a"
    out(f"{label}\t{len(rows)}\t{statistics.median(err):.4f}\t{pct(err, 90):.4f}\t"
        f"{pct(err, 99):.4f}\t{max(err):.4f}\t{statistics.mean(enc):.3f}\t"
        f"{f(hits)}\t{f(within1)}\t{g(lat_terms)}\t{g(pred_terms)}\t{over:.1%}\t"
        f"{statistics.median([r['bytes'] for r in rows]):.0f}\t"
        f"{statistics.median([r['ms'] for r in rows]):.0f}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ab2")
    ap.add_argument("--lattice", nargs="*", default=[])
    a = ap.parse_args()

    lattice = load_lattice(a.lattice) if a.lattice else {}
    rows = []
    with open(a.ab2) as f:
        for r in csv.DictReader(f, delimiter="\t"):
            rows.append({
                "arm": r["arm"],
                "image": r["image"],
                "size": int(r["size"]),
                "target": float(r["target"]),
                "achieved": float(r["achieved"]),
                "err": float(r["err"]),
                "encodes": int(r["encodes"]),
                "qi": int(r.get("qi", -1)),
                "bytes": int(r["bytes"]),
                "ms": int(r["ms"]),
                "spatial": r.get("spatial", "?"),
                "predicted": float(r["predicted"]) if r.get("predicted", "NaN") not in
                ("NaN", "nan", "") else float("nan"),
            })
    arms = sorted({r["arm"] for r in rows})
    print(f"# rows {len(rows)}  arms {arms}  "
          f"sizes {sorted({r['size'] for r in rows})}  "
          f"cells {len({(r['image'], r['size']) for r in rows})}")
    print(f"# lattice tables loaded for {len(lattice)} cells "
          f"({'decomposition available' if lattice else 'NO decomposition -- pass --lattice'})")
    spat = sorted({r["spatial"] for r in rows})
    print(f"# spatial column values seen: {spat}")
    print()

    print("# --- error distribution at a FIXED budget of 2 encodes, overall ---")
    print(HDR)
    for arm in arms:
        summarise([r for r in rows if r["arm"] == arm], arm, lattice)
    print("#   med_lattice = irreducible: target to nearest ACHIEVABLE score")
    print("#   med_pred    = the rest, i.e. the part modelling can remove")
    print("#   nearest_hit = landed exactly on the nearest achievable point")
    print("#   (blank where the arm does not report which quantizer it chose)")
    print()

    for axis, key in (("size", lambda r: r["size"]),
                      ("target band", lambda r: 20 * (int(r["target"]) // 20))):
        print(f"# --- by {axis} ---")
        print(HDR)
        for v in sorted({key(r) for r in rows}):
            for arm in arms:
                sub = [r for r in rows if r["arm"] == arm and key(r) == v]
                if sub:
                    summarise(sub, f"{arm}@{v}", lattice)
        print()

    # paired deltas against the secant baseline: the only comparison not
    # confounded by which cells each arm happened to do well on
    base = {(r["image"], r["size"], r["target"]): r for r in rows if r["arm"] == "secant2"}
    if base:
        print("# --- paired vs secant2 on identical (image, size, target) ---")
        print("arm\tn\tmean_d|err|\tmed_d|err|\tcloser\tequal\tfurther\tmed_dbytes\tmed_dms")
        for arm in arms:
            if arm == "secant2":
                continue
            d, db, dm = [], [], []
            for r in rows:
                if r["arm"] != arm:
                    continue
                b = base.get((r["image"], r["size"], r["target"]))
                if not b:
                    continue
                d.append(abs(r["err"]) - abs(b["err"]))
                db.append(r["bytes"] - b["bytes"])
                dm.append(r["ms"] - b["ms"])
            if d:
                print(f"{arm}\t{len(d)}\t{statistics.mean(d):+.4f}\t{statistics.median(d):+.4f}\t"
                      f"{sum(x < -1e-9 for x in d) / len(d):.1%}\t"
                      f"{sum(abs(x) <= 1e-9 for x in d) / len(d):.1%}\t"
                      f"{sum(x > 1e-9 for x in d) / len(d):.1%}\t"
                      f"{statistics.median(db):+.0f}\t{statistics.median(dm):+.0f}")
        print()

    # how good is the two-shot's own forecast of where it will land?
    pr = [r for r in rows if r["arm"].startswith("twoshot") and not math.isnan(r["predicted"])]
    if pr:
        print("# --- pass-2 forecast quality (predicted vs achieved score) ---")
        print("arm\tn\tmed|pred-achieved|\tp90\tbias(med signed)")
        for arm in sorted({r["arm"] for r in pr}):
            v = [r["predicted"] - r["achieved"] for r in pr if r["arm"] == arm]
            print(f"{arm}\t{len(v)}\t{statistics.median([abs(x) for x in v]):.4f}\t"
                  f"{pct([abs(x) for x in v], 90):.4f}\t{statistics.median(v):+.4f}")
        print("#   A biased forecast is fixable (re-fit the anchor); a wide but")
        print("#   unbiased one is the translate assumption's own residual.")


if __name__ == "__main__":
    main()

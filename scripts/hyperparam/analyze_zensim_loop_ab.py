#!/usr/bin/env python3
"""Summarize the real-encode A/B between the bracketed secant baseline
(`encode_rgb8_with_target`, TargetMetric::Zensim) and the zensim closed loop
(`encode_rgb8_zensim_loop`) produced by
`examples/zensim_loop_bench.rs ab`.

    python3 scripts/hyperparam/analyze_zensim_loop_ab.py <ab.tsv>

Input columns:
    arm image size w h target achieved err encodes converged bytes ms spatial
    seed_q seed_score anchor_q      (zloop rows only; "NA" on secant rows)

Reports, overall and split by size and by target band:
  * mean / median encodes to converge,
  * converged fraction,
  * the achieved-vs-target error distribution,
  * the 1-encode and 2-encode fractions (the loop's actual goal),
  * paired per-cell deltas (same image, size and target on both arms), which
    is the only comparison that is not confounded by cell mix.

Deterministic; stdlib only.
"""

import csv
import statistics
import sys
from collections import defaultdict


def pct(n, d):
    return f"{100.0 * n / d:.1f}%" if d else "NA"


def summarize(rows, label):
    if not rows:
        return
    enc = [int(r["encodes"]) for r in rows]
    conv = [r["converged"] == "true" for r in rows]
    err = [abs(float(r["err"])) for r in rows]
    err_conv = [abs(float(r["err"])) for r in rows if r["converged"] == "true"]
    ms = [int(r["ms"]) for r in rows]
    n = len(rows)
    print(
        f"{label}\t{n}\t{statistics.mean(enc):.3f}\t{statistics.median(enc):.1f}\t"
        f"{pct(sum(conv), n)}\t{pct(sum(1 for e in enc if e == 1), n)}\t"
        f"{pct(sum(1 for e in enc if e <= 2), n)}\t"
        f"{statistics.median(err):.3f}\t{sorted(err)[int(0.9 * (n - 1))]:.3f}\t"
        f"{(statistics.median(err_conv) if err_conv else float('nan')):.3f}\t"
        f"{statistics.median(ms):.0f}"
    )


HDR = (
    "group\tn\tmean_enc\tmed_enc\tconverged\t1_encode\t<=2_encodes\t"
    "med_|err|\tp90_|err|\tmed_|err|_conv\tmed_ms"
)


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
    arms = sorted({r["arm"] for r in rows})
    sizes = sorted({int(r["size"]) for r in rows})
    spatial = sorted({r["spatial"] for r in rows if r["arm"] == "zloop"})
    print(f"# rows {len(rows)}  arms {arms}  sizes {sizes}")
    print(f"# zloop spatial_applied values seen: {spatial}")
    print(
        "# (spatial_applied=false means the per-superblock hints were computed\n"
        "#  and discarded -- zenravif's FrameHints passthrough is release-gated,\n"
        "#  so this A/B measures the GLOBAL half of the loop only.)"
    )

    print("\n# --- overall ---")
    print(HDR)
    for a in arms:
        summarize([r for r in rows if r["arm"] == a], a)

    print("\n# --- by size (long edge) ---")
    print(HDR)
    for s in sizes:
        for a in arms:
            summarize(
                [r for r in rows if r["arm"] == a and int(r["size"]) == s], f"{a}@{s}"
            )

    print("\n# --- by target band (equal density low and high) ---")
    print(HDR)
    bands = [(0, 40), (40, 60), (60, 80), (80, 101)]
    for loq, hiq in bands:
        for a in arms:
            summarize(
                [
                    r
                    for r in rows
                    if r["arm"] == a and loq <= float(r["target"]) < hiq
                ],
                f"{a}@t[{loq},{hiq})",
            )

    # ---- paired deltas ---------------------------------------------------
    keyed = defaultdict(dict)
    for r in rows:
        keyed[(r["image"], r["size"], r["target"])][r["arm"]] = r
    pairs = [v for v in keyed.values() if len(v) == len(arms) and "zloop" in v]
    if not pairs or "secant" not in arms:
        return
    d_enc = [int(p["zloop"]["encodes"]) - int(p["secant"]["encodes"]) for p in pairs]
    d_err = [
        abs(float(p["zloop"]["err"])) - abs(float(p["secant"]["err"])) for p in pairs
    ]
    d_bytes = [int(p["zloop"]["bytes"]) - int(p["secant"]["bytes"]) for p in pairs]
    n = len(pairs)
    print(f"\n# --- paired per-cell deltas (zloop - secant), n={n} ---")
    print(
        f"# encodes:   mean {statistics.mean(d_enc):+.3f}  median {statistics.median(d_enc):+.1f}  "
        f"fewer {pct(sum(1 for d in d_enc if d < 0), n)}  same {pct(sum(1 for d in d_enc if d == 0), n)}  "
        f"more {pct(sum(1 for d in d_enc if d > 0), n)}"
    )
    print(
        f"# |err|:     mean {statistics.mean(d_err):+.4f}  median {statistics.median(d_err):+.4f}  "
        f"closer {pct(sum(1 for d in d_err if d < -1e-9), n)}  "
        f"equal {pct(sum(1 for d in d_err if abs(d) <= 1e-9), n)}  "
        f"further {pct(sum(1 for d in d_err if d > 1e-9), n)}"
    )
    print(
        f"# bytes:     median {statistics.median(d_bytes):+.0f}  "
        f"(both arms use the same 'smallest file in band' selection policy)"
    )
    only_z = sum(
        1
        for p in pairs
        if p["zloop"]["converged"] == "true" and p["secant"]["converged"] == "false"
    )
    only_s = sum(
        1
        for p in pairs
        if p["zloop"]["converged"] == "false" and p["secant"]["converged"] == "true"
    )
    print(f"# converged: zloop-only {only_z}   secant-only {only_s}   (of {n} cells)")

    # How good is the SEED on its own? This is the honest answer to
    # "does it converge in 1 encode": a 1-encode result is an open-loop
    # PREDICTION, and its error distribution is exactly this.
    seed_err = [
        abs(float(p["zloop"]["seed_score"]) - float(p["zloop"]["target"]))
        for p in pairs
        if p["zloop"].get("seed_score", "NA") != "NA"
    ]
    if seed_err:
        seed_err.sort()
        m = len(seed_err)
        print(
            f"\n# --- pass-1 (open-loop seed) |score - target|, n={m} ---\n"
            f"# p50 {seed_err[m // 2]:.2f}  p90 {seed_err[int(0.9 * (m - 1))]:.2f}  "
            f"max {seed_err[-1]:.2f} zensim points"
        )
        for tol in (0.5, 1.0, 2.0, 3.0, 5.0):
            hit = sum(1 for e in seed_err if e <= tol)
            print(f"#   within +-{tol}: {pct(hit, m)}   <- the 1-encode rate at that tolerance")
        # And how much of the seed's placement comes from the content head
        # rather than the content-blind anchor.
        offs = [
            float(p["zloop"]["seed_q"]) - float(p["zloop"]["anchor_q"])
            for p in pairs
            if p["zloop"].get("anchor_q", "NA") != "NA"
        ]
        if offs:
            nz = sum(1 for o in offs if abs(o) > 1e-6)
            print(
                f"# q0-head content offset vs the bare anchor: nonzero on {pct(nz, len(offs))} "
                f"of cells, median {statistics.median(offs):+.2f}, "
                f"max |offset| {max(abs(o) for o in offs):.2f} quality points"
            )

    # Where each arm's first pass landed is the whole 1-encode story.
    one_z = sum(1 for p in pairs if int(p["zloop"]["encodes"]) == 1)
    one_s = sum(1 for p in pairs if int(p["secant"]["encodes"]) == 1)
    two_z = sum(1 for p in pairs if int(p["zloop"]["encodes"]) <= 2)
    two_s = sum(1 for p in pairs if int(p["secant"]["encodes"]) <= 2)
    print(
        f"# 1-encode:  zloop {pct(one_z, n)}  secant {pct(one_s, n)}\n"
        f"# <=2-encode: zloop {pct(two_z, n)}  secant {pct(two_s, n)}"
    )


if __name__ == "__main__":
    main()

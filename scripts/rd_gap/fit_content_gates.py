#!/usr/bin/env python3
"""Fit the content-gate thresholds for the RD-vs-time monotonicity program.

Joins the armed inversion-label sweep (labels.tsv: img fam speed q bytes ssim2
enc_ms) with zenanalyze features (features.tsv: variant_name ... feat_*), decides
per image whether it INVERTS (a slower tier Pareto-dominated by a clearly-faster
one — same rule as gate_kit's gate-monotone), and reports how the fast_heads gate
features (gradient_fraction_smooth, patch_fraction, dct_compressibility_y) separate
inverting from clean content. Deterministic; re-derivable by re-running.

Usage: fit_content_gates.py LABELS_TSV FEATURES_TSV
"""
import sys
from collections import defaultdict

# "clearly faster" margin (matches gate_kit run_monotone): A faster than B iff
# A.ms < TIME_MARGIN * B.ms. Wide so near-cost tiers never count as inversions.
TIME_MARGIN = 0.80

# "meaningful" RD-domination margin. A strict Pareto win of <1% bytes and
# <0.2 ssim2 is RD noise on a flat ladder (e.g. photo 5004: s9 beats s5 by
# 0.4% bytes / 0.03 ssim2) — not worth the complexity of a content-gate. A
# monotonicity violation counts only if the faster tier is BETTER by at least
# one of these margins. Separates photo-flatness-noise from the real
# synthetic-content inversions (6096/6018: 5-7% bytes, 0.3-1.2 ssim2).
BYTES_MARGIN = 0.01   # faster tier >=1% smaller
SSIM2_MARGIN = 0.20   # or >=0.2 ssim2 better


def _header_line(f):
    """First non-comment, non-blank line (skips '# provenance' preamble)."""
    for ln in f:
        s = ln.rstrip("\n")
        if s and not s.startswith("#"):
            return s
    return ""


def load_labels(path):
    rows = []
    with open(path) as f:
        hdr = _header_line(f).split("\t")
        idx = {c: i for i, c in enumerate(hdr)}
        for ln in f:
            p = ln.rstrip("\n").split("\t")
            if len(p) < len(hdr):
                continue
            try:
                rows.append({
                    "img": p[idx["img"]],
                    "fam": p[idx["fam"]],
                    "speed": int(p[idx["speed"]]),
                    "q": int(p[idx["q"]]),
                    "bytes": int(p[idx["bytes"]]),
                    "ssim2": float(p[idx["ssim2"]]) if p[idx["ssim2"]] != "NA" else None,
                    "ms": float(p[idx["enc_ms"]]),
                })
            except (ValueError, KeyError):
                continue
    return [r for r in rows if r["ssim2"] is not None and r["bytes"] > 0]


def inversions_for_cell(tiers):
    """tiers: list of dicts with speed,bytes,ssim2,ms. Return list of (slow,fast)."""
    out = []
    for b in tiers:
        for a in tiers:
            if a["speed"] == b["speed"]:
                continue
            a_faster = a["ms"] < TIME_MARGIN * b["ms"]
            a_dom = (a["bytes"] <= b["bytes"] and a["ssim2"] >= b["ssim2"]
                     and (a["bytes"] < b["bytes"] or a["ssim2"] > b["ssim2"]))
            meaningful = ((b["bytes"] - a["bytes"]) > BYTES_MARGIN * b["bytes"]
                          or (a["ssim2"] - b["ssim2"]) > SSIM2_MARGIN)
            if a_faster and a_dom and meaningful:
                out.append((b["speed"], a["speed"]))
    return out


def load_features(path):
    feats = {}
    with open(path) as f:
        hdr = _header_line(f).split("\t")
        idx = {c: i for i, c in enumerate(hdr)}
        vn = idx.get("variant_name", 0)
        want = {"gfs": "feat_gradient_fraction_smooth",
                "pf": "feat_patch_fraction",
                "dcty": "feat_dct_compressibility_y"}
        for ln in f:
            p = ln.rstrip("\n").split("\t")
            if len(p) <= vn:
                continue
            key = p[vn]
            feats[key] = {k: (float(p[idx[c]]) if c in idx and p[idx[c]] not in ("", "NA") else float("nan"))
                          for k, c in want.items()}
    return feats


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    labels = load_labels(sys.argv[1])
    feats = load_features(sys.argv[2])

    # group by (img, q)
    cells = defaultdict(list)
    for r in labels:
        cells[(r["img"], r["q"])].append(r)

    # per image: inverts at any q?  collect the inversion pairs
    per_img = defaultdict(lambda: {"inv": set(), "fam": None})
    for (img, q), tiers in cells.items():
        invs = inversions_for_cell(tiers)
        fam = tiers[0]["fam"]
        per_img[img]["fam"] = fam
        for (slow, fast) in invs:
            per_img[img]["inv"].add((q, slow, fast))

    # join features (variant_name = basename without .png)
    print(f"{'img':<52}{'fam':>6}{'inv?':>6}{'gfs':>8}{'pf':>8}{'dcty':>9}  inversions")
    inverting, clean = [], []
    for img in sorted(per_img):
        vn = img[:-4] if img.endswith(".png") else img
        ft = feats.get(vn, {"gfs": float('nan'), "pf": float('nan'), "dcty": float('nan')})
        inv = per_img[img]["inv"]
        tag = "INV" if inv else "."
        pairs = ",".join(f"q{q}:s{s}<s{fst}" for (q, s, fst) in sorted(inv)) if inv else ""
        print(f"{img[:52]:<52}{per_img[img]['fam']:>6}{tag:>6}{ft['gfs']:>8.3f}{ft['pf']:>8.3f}{ft['dcty']:>9.2f}  {pairs}")
        (inverting if inv else clean).append((img, ft))

    # separation report on each gate feature
    def stats(rows, k):
        vals = [ft[k] for _, ft in rows if ft[k] == ft[k]]  # drop nan
        if not vals:
            return "n/a"
        vals.sort()
        return f"min={vals[0]:.3f} med={vals[len(vals)//2]:.3f} max={vals[-1]:.3f}"

    print(f"\n== separation ({len(inverting)} inverting, {len(clean)} clean) ==")
    for k in ("gfs", "pf", "dcty"):
        print(f"  {k:>5}  inverting: {stats(inverting, k):<40} clean: {stats(clean, k)}")

    # candidate: does gfs<T OR (pf-high & dcty-high) separate them?
    print("\n== candidate holistic 'synthetic' gate: gfs < T ==")
    for T in (0.41, 0.45, 0.50, 0.55, 0.60, 0.64):
        tp = sum(1 for _, ft in inverting if ft["gfs"] == ft["gfs"] and ft["gfs"] < T)
        fp = sum(1 for _, ft in clean if ft["gfs"] == ft["gfs"] and ft["gfs"] < T)
        print(f"  T={T:.2f}: catches {tp}/{len(inverting)} inverters, {fp}/{len(clean)} false-fires on clean")

    # simulate the monotone_speed_gate: on gfs<GATE content, remap requested
    # speed 5 -> REMAP (fill the measured s5-valley). Recount inversions on the
    # remapped ladder (s5's row := REMAP's row) and report residuals + the RD
    # cost on false-fires. This is the fit's verification of the shipped gate.
    GATE = 0.64
    for REMAP in (int(sys.argv[3]) if len(sys.argv) > 3 else 4,):
        _simulate(GATE, REMAP, per_img, feats, cells, inversions_for_cell)


def _simulate(GATE, REMAP, per_img, feats, cells, inversions_for_cell):
    print(f"\n== simulate monotone_speed_gate: gfs<{GATE} => s5:=s{REMAP} ==")
    fixed_cnt, residual, newinv, ff_cost = 0, [], [], []
    for img in sorted(per_img):
        vn = img[:-4] if img.endswith(".png") else img
        ft = feats.get(vn, {"gfs": float('nan')})
        fires = ft["gfs"] == ft["gfs"] and ft["gfs"] < GATE
        for q in (50, 80):
            tiers = {t["speed"]: t for t in cells.get((img, q), [])}
            if not tiers or 5 not in tiers or REMAP not in tiers:
                continue
            before = set(inversions_for_cell(list(tiers.values())))
            if fires:
                # remap: s5 slot takes s{REMAP}'s (bytes,ssim2) but is a distinct
                # tier at s{REMAP}'s time — i.e. s5 == sREMAP exactly.
                r = tiers[REMAP]
                tiers = dict(tiers)
                tiers[5] = {"speed": 5, "bytes": r["bytes"], "ssim2": r["ssim2"], "ms": r["ms"]}
                # RD cost on THIS remap vs original s5 (bytes %, ssim2 delta)
                o = {t["speed"]: t for t in cells[(img, q)]}[5]
                ff_cost.append((img, q, (r["bytes"] - o["bytes"]) / o["bytes"], r["ssim2"] - o["ssim2"]))
            after = set(inversions_for_cell(list(tiers.values())))
            gone = before - after
            added = after - before
            # only count s5-involving inversions as "the valley"
            valley = {(s, f) for (s, f) in before if s == 5}
            if fires and valley:
                fixed_cnt += len(valley & gone)
            for (s, f) in after:
                if s == 5 or (fires and (s, f) not in before):
                    residual.append((img, q, s, f))
            for (s, f) in added:
                newinv.append((img, q, s, f))
    print(f"  s5-valley inversions removed: {fixed_cnt}")
    print(f"  NEW inversions introduced by the remap: {len(newinv)}  {newinv[:6]}")
    resid5 = [(i, q, s, f) for (i, q, s, f) in residual if s == 5]
    print(f"  residual s5 inversions after gate: {len(resid5)}  {resid5[:6]}")
    if ff_cost:
        worst = max(ff_cost, key=lambda x: abs(x[2]))
        avg_b = sum(abs(c[2]) for c in ff_cost) / len(ff_cost)
        print(f"  remap RD cost (all fired cells): mean |bytes|={avg_b*100:.2f}%  worst={worst[0]} {worst[2]*100:+.1f}% bytes {worst[3]:+.2f} ssim2")


if __name__ == "__main__":
    main()

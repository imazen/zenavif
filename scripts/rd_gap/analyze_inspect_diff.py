#!/usr/bin/env python3
"""Diff per-block decode decisions + per-syntax-element bit cost between two AV1
bitstreams (typically zenrav1e vs libaom at matched byte size), using aom's own
`inspect` tool JSON dumps (--all) from inspect_diff.sh. Area-weighted (per 4x4
mode-info cell) histograms for blockSize/transformSize/transformType/mode/skip/
palette, plus the accounting bit-cost breakdown by AV1 syntax element
(read_coeffs_txb, read_intra_mode, av1_read_tx_type, read_palette_mode_info,
cfl:*, etc.) via aom's Accounting API.

Usage: analyze_inspect_diff.py a1.json b1.json [a2.json b2.json ...]
       (each a/b pair is one image at matched settings; labels come from the
        json filename stem, e.g. zenrav1e.json/libaom.json)
"""
import json, sys, os, collections

def load(path):
    data = json.load(open(path))
    frames = [f for f in data if f is not None]
    assert len(frames) == 1, f"{path}: expected 1 frame, got {len(frames)}"
    return frames[0]

def rev(m):
    return {v: k for k, v in m.items()}

def cell_hist(frame, field, revmap):
    c = collections.Counter()
    for row in frame[field]:
        for v in row:
            c[revmap.get(v, f"?{v}")] += 1
    return c

def bit_cost_by_symbol(frame):
    syms_map = frame["symbolsMap"]
    bits = collections.Counter()
    for entry in frame["symbols"]:
        if len(entry) == 3:
            sid, b, _s = entry
            bits[syms_map[sid]] += b / 8.0
    return bits

def pct_table(c_a, c_b, label_a, label_b, total_a, total_b, top=10):
    keys = sorted(set(c_a) | set(c_b), key=lambda k: -(c_a.get(k, 0) + c_b.get(k, 0)))
    print(f"  {'':22} {label_a:>12} {label_b:>12} {'delta':>10}")
    for k in keys[:top]:
        pa = 100 * c_a.get(k, 0) / total_a
        pb = 100 * c_b.get(k, 0) / total_b
        print(f"  {k:22} {pa:>11.2f}% {pb:>11.2f}% {pb-pa:>+9.2f}pp")

def analyze_pair(label, a_path, b_path):
    label_a = os.path.splitext(os.path.basename(a_path))[0]
    label_b = os.path.splitext(os.path.basename(b_path))[0]
    fa, fb = load(a_path), load(b_path)
    print(f"\n{'='*70}\n{label}  (baseQIndex: {label_a}={fa['baseQIndex']} {label_b}={fb['baseQIndex']})\n{'='*70}")

    for field, mapname in [("blockSize", "blockSizeMap"), ("transformSize", "transformSizeMap"),
                            ("transformType", "transformTypeMap"), ("mode", "modeMap"),
                            ("uv_mode", "uv_modeMap"), ("skip", "skipMap"),
                            ("palette", "paletteMap"), ("uv_palette", "uv_paletteMap")]:
        ca = cell_hist(fa, field, rev(fa[mapname]))
        cb = cell_hist(fb, field, rev(fb[mapname]))
        ta, tb = sum(ca.values()), sum(cb.values())
        print(f"\n--- {field} (% of 4x4 cells) ---")
        pct_table(ca, cb, label_a, label_b, ta, tb)

    ba, bb = bit_cost_by_symbol(fa), bit_cost_by_symbol(fb)
    tot_a, tot_b = sum(ba.values()), sum(bb.values())
    print(f"\n--- bit cost by syntax element (total: {label_a}={tot_a:.0f}b {label_b}={tot_b:.0f}b, ratio {tot_b/tot_a:.3f}) ---")
    keys = sorted(set(ba) | set(bb), key=lambda k: -(ba.get(k, 0) + bb.get(k, 0)))
    print(f"  {'symbol':24} {label_a+'_bits':>14} {label_b+'_bits':>14} {'b_minus_a':>10} {'a_%tot':>7} {'b_%tot':>7}")
    for k in keys:
        va, vb = ba.get(k, 0), bb.get(k, 0)
        print(f"  {k:24} {va:>14.0f} {vb:>14.0f} {vb-va:>+10.0f} {100*va/tot_a:>6.1f}% {100*vb/tot_b:>6.1f}%")
    return ba, bb

if __name__ == "__main__":
    args = sys.argv[1:]
    if len(args) < 2 or len(args) % 2 != 0:
        raise SystemExit(__doc__)
    pairs = list(zip(args[0::2], args[1::2]))
    agg_a, agg_b = collections.Counter(), collections.Counter()
    for i, (a, b) in enumerate(pairs):
        ba, bb = analyze_pair(f"pair {i+1}: {a} vs {b}", a, b)
        agg_a.update(ba); agg_b.update(bb)

    if len(pairs) > 1:
        print(f"\n\n{'#'*70}\nAGGREGATE across {len(pairs)} pairs (bit cost by syntax element)\n{'#'*70}")
        tot_a, tot_b = sum(agg_a.values()), sum(agg_b.values())
        print(f"total bits: a={tot_a:.0f} b={tot_b:.0f} ratio(b/a)={tot_b/tot_a:.3f}")
        keys = sorted(set(agg_a) | set(agg_b), key=lambda k: -(agg_a.get(k, 0) + agg_b.get(k, 0)))
        print(f"  {'symbol':24} {'a_bits':>14} {'b_bits':>14} {'b_minus_a':>10} {'a_%tot':>7} {'b_%tot':>7}")
        for k in keys:
            va, vb = agg_a.get(k, 0), agg_b.get(k, 0)
            print(f"  {k:24} {va:>14.0f} {vb:>14.0f} {vb-va:>+10.0f} {100*va/tot_a:>6.1f}% {100*vb/tot_b:>6.1f}%")

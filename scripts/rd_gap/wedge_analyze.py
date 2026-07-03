#!/usr/bin/env python3
"""WEDGE-FINDER analysis: size/crop-expanded RD gap zenrav1e vs libaom over the
imazen-26 aligned corpus (see wedge_corpus.rs + _MANIFEST.json).

Inputs are per-arm TSVs in the run_gap.sh row schema (arm identity comes from
WHICH file a row is in, since libaom rows don't self-identify cpu level):

  wedge_analyze.py --zr zr.tsv --cpu2 cpu2.tsv [--cpu0 cpu0.tsv]
                   --map wedge_corpus_map.tsv
                   [--features imazen26_features_2026-06-23.parquet]
                   [--manifest _MANIFEST.json]           (origin_path join for features)
                   [--dataset out.parquet]               (labeled per-cell dataset)
                   [--continuity]                        (1024-full slice vs known positions)

BD conventions match analyze.py exactly (monotone frontier hull; log-bytes
trapezoid over the overlapping ssim2 range; >=4 pts and >=2 ssim2 overlap;
+ = zenrav1e needs more bits). libaom frontiers pool all formats (420/444),
matching the run_gap methodology.
"""
import argparse
import collections
import csv
import json
import math
import sys

import numpy as np

# Web-traffic size weighting for "recoverable bytes" wedge ranking (web is
# small-image-heavy; documented assumption, tune as needed).
TRAFFIC_W = {"256": 0.35, "512": 0.35, "1024": 0.20, "top": 0.10}


def frontier(points):
    bybpp = sorted(points, key=lambda p: (p[1], -p[0]))
    front, best = [], -1e9
    for s, b in bybpp:
        if s > best:
            front.append((s, b))
            best = s
    front.sort(key=lambda p: p[0])
    return front


def bd_rate(test_front, ref_front):
    def prep(f):
        seen = {}
        for s, b in f:
            seen[round(s, 4)] = math.log(b)
        xs = sorted(seen)
        return np.array(xs), np.array([seen[x] for x in xs])

    x1, y1 = prep(ref_front)
    x2, y2 = prep(test_front)
    if len(x1) < 4 or len(x2) < 4:
        return None
    lo, hi = max(x1.min(), x2.min()), min(x1.max(), x2.max())
    if hi - lo < 2:
        return None
    gg = np.linspace(lo, hi, 200)
    trapz = np.trapz if hasattr(np, "trapz") else np.trapezoid
    avg = (trapz(np.interp(gg, x2, y2), gg) - trapz(np.interp(gg, x1, y1), gg)) / (hi - lo)
    return (math.exp(avg) - 1.0) * 100.0


def gap_at_mid(test_front, ref_front):
    """bpp gap % at the midpoint of the overlapping ssim2 window (fam7-continuity style).
    Returns (mid_ssim2, gap_pct, ref_bpp) or None."""
    if len(test_front) < 2 or len(ref_front) < 2:
        return None
    lo = max(test_front[0][0], ref_front[0][0])
    hi = min(test_front[-1][0], ref_front[-1][0])
    if hi <= lo:
        return None
    mid = (lo + hi) / 2
    tb = float(np.interp(mid, [p[0] for p in test_front], [p[1] for p in test_front]))
    rb = float(np.interp(mid, [p[0] for p in ref_front], [p[1] for p in ref_front]))
    return mid, 100.0 * (tb - rb) / rb, rb


def load_rows(path, arm):
    out = []
    with open(path) as f:
        r = csv.DictReader(f, delimiter="\t")
        for row in r:
            try:
                s = float(row["ssim2"])
                b = float(row["bpp"])
                by = int(row["bytes"])
            except (ValueError, KeyError, TypeError):
                continue
            if b <= 0:
                continue
            row["_arm"] = arm
            row["_ssim2"] = s
            row["_bpp"] = b
            row["_bytes"] = by
            out.append(row)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--zr")
    ap.add_argument("--cpu2")
    ap.add_argument("--cpu0")
    ap.add_argument("--map", required=True)
    ap.add_argument("--features")
    ap.add_argument("--manifest")
    ap.add_argument("--dataset")
    ap.add_argument("--continuity", action="store_true")
    ap.add_argument("--summary-tsv")
    args = ap.parse_args()

    cmap = {}
    with open(args.map) as f:
        for row in csv.DictReader(f, delimiter="\t"):
            cmap[row["file"]] = row

    rows = []
    for path, arm in ((args.zr, "zr"), (args.cpu2, "cpu2"), (args.cpu0, "cpu0")):
        if path:
            rr = load_rows(path, arm)
            print(f"loaded {len(rr):5d} rows from {path} (arm={arm})")
            rows.extend(rr)

    # group points per (file, arm); libaom pools formats
    pts = collections.defaultdict(list)
    for r in rows:
        if r["image"] not in cmap:
            print(f"WARN row for unknown file {r['image']}", file=sys.stderr)
            continue
        pts[(r["image"], r["_arm"])].append((r["_ssim2"], r["_bpp"]))
    fr = {k: frontier(v) for k, v in pts.items()}

    files = sorted(cmap)

    def size_slot(m):
        # normalize: the "top" slot is 2048 or native (per-origin ladder cap)
        sc = m["size_class"]
        if m["crop_label"] != "full":
            return "crop"
        return "top" if sc in ("2048", "native") else sc

    # ---------- per-file BD summary ----------
    summ = {}
    for fn in files:
        m = cmap[fn]
        e = {
            "file": fn,
            "origin_id": m["origin_id"],
            "family": m["family"],
            "content_class": m["content_class"],
            "crop_label": m["crop_label"],
            "size_class": m["size_class"],
            "size_slot": size_slot(m),
            "px": int(m["width"]) * int(m["height"]),
        }
        zf = fr.get((fn, "zr"), [])
        for ref in ("cpu2", "cpu0"):
            rf = fr.get((fn, ref), [])
            e[f"bd_{ref}"] = bd_rate(zf, rf) if zf and rf else None
            g = gap_at_mid(zf, rf) if zf and rf else None
            e[f"mid_{ref}"], e[f"gap_{ref}"], e[f"refbpp_{ref}"] = g if g else (None, None, None)
        summ[fn] = e

    have_zr = args.zr is not None and any(fr.get((fn, "zr")) for fn in files)

    # ---------- continuity check (1024/native full slice) ----------
    if args.continuity and have_zr:
        print("\n=== CONTINUITY CHECK: full-crop 1024-slice (+native for <=1024 origins) vs known positions ===")
        photos, plots = [], []
        for fn in files:
            e = summ[fn]
            if e["crop_label"] != "full" or e["bd_cpu2"] is None:
                continue
            if e["size_class"] == "1024" or (e["size_class"] == "native" and e["px"] <= 1100 * 1100):
                (plots if e["family"] == "7000" else photos).append(e)
        if photos:
            v = [e["bd_cpu2"] for e in photos]
            print(f"photos+screens/gen n={len(v)}: BD vs cpu2 median {np.median(v):+.2f}% mean {np.mean(v):+.2f}% "
                  f"(known: legacy-19 s2-best −8.49% med; train26 heterogeneous)")
            for e in sorted(photos, key=lambda x: x["bd_cpu2"]):
                print(f"  {e['bd_cpu2']:+8.2f}%  {e['family']}  {e['file'][:64]}")
        if plots:
            print("fam-7000 plots (palette-auto arm; known band: gap at overlap-mid ≈ +61..99% w/ palette, +163..297% off):")
            for e in plots:
                g = f"{e['gap_cpu2']:+.0f}%" if e["gap_cpu2"] is not None else "NA"
                bd = f"{e['bd_cpu2']:+.1f}%" if e["bd_cpu2"] is not None else "NA"
                print(f"  gap@mid {g:>7}  BD {bd:>8}  mid={e['mid_cpu2'] and round(e['mid_cpu2'],1)}  {e['file'][:64]}")

    # ---------- per-group BD tables ----------
    def table(group_key, title, ref="cpu2"):
        groups = collections.defaultdict(list)
        for fn in files:
            e = summ[fn]
            if e[f"bd_{ref}"] is not None:
                groups[group_key(e)].append(e[f"bd_{ref}"])
        if not groups:
            return
        print(f"\n=== BD-rate zenrav1e vs {ref}: {title} (+ = zenrav1e needs more bits) ===")
        print(f"{'group':>34} {'n':>3} {'median':>8} {'mean':>8} {'win':>4} {'loss':>4}")
        for g in sorted(groups):
            v = groups[g]
            win = sum(1 for x in v if x < 0)
            print(f"{str(g):>34} {len(v):>3} {np.median(v):>+8.2f} {np.mean(v):>+8.2f} {win:>4} {len(v)-win:>4}")

    if have_zr:
        table(lambda e: (e["family"], e["size_slot"]), "family x size_slot")
        table(lambda e: e["size_slot"], "size_slot")
        table(lambda e: e["crop_label"] if e["crop_label"] != "full" else f"full@{e['size_slot']}", "crop")
        table(lambda e: e["family"], "family")
        if args.cpu0:
            table(lambda e: e["size_slot"], "size_slot (vs cpu0-default)", ref="cpu0")

        # per-crop variance within origin (local wedges)
        print("\n=== per-origin c50 crop spread (BD vs cpu2; local wedges the global tune can't fix) ===")
        byorig = collections.defaultdict(list)
        for fn in files:
            e = summ[fn]
            if e["crop_label"].startswith("c50") and e["bd_cpu2"] is not None:
                byorig[e["origin_id"]].append((e["crop_label"], e["bd_cpu2"]))
        spread = []
        for o, v in byorig.items():
            if len(v) >= 3:
                vals = [x[1] for x in v]
                fe = next(e for e in summ.values() if e["origin_id"] == o and e["crop_label"] == "full"
                          and e["size_slot"] in ("1024", "top") and e["bd_cpu2"] is not None)
                spread.append((max(vals) - min(vals), o, min(v, key=lambda x: x[1]), max(v, key=lambda x: x[1]),
                               fe["bd_cpu2"], fe["family"]))
        spread.sort(reverse=True)
        print(f"{'origin':>8} {'fam':>5} {'full_bd':>8} {'best_crop':>16} {'worst_crop':>16} {'spread':>7}")
        for sp, o, best, worst, fullbd, fam in spread:
            print(f"{o:>8} {fam:>5} {fullbd:>+8.2f} {best[0]:>9}{best[1]:>+7.1f} {worst[0]:>9}{worst[1]:>+7.1f} {sp:>7.1f}")

        # ---------- fixed overhead: bytes = alpha + beta*px per (origin, arm, q) over the full-size ladder ----------
        print("\n=== per-size fixed overhead (alpha in bytes = alpha + beta*px fits across the full-crop size ladder) ===")
        by_cell = collections.defaultdict(dict)  # (origin, arm, q_or_cq+fmt) -> {px: bytes}
        for r in rows:
            m = cmap.get(r["image"])
            if not m or m["crop_label"] != "full":
                continue
            px = int(m["width"]) * int(m["height"])
            key = (m["origin_id"], r["_arm"], r["fmt"], r["q"])
            by_cell[key][px] = r["_bytes"]
        alphas = collections.defaultdict(list)
        for (o, arm, fmt, q), d in by_cell.items():
            if len(d) >= 3:
                xs = np.array(sorted(d))
                ys = np.array([d[x] for x in xs])
                beta, alpha = np.polyfit(xs, ys, 1)
                alphas[(arm, q)].append(alpha)
        for arm in ("zr", "cpu2"):
            qs = sorted({q for (a, q) in alphas if a == arm}, key=lambda x: float(x))
            if not qs:
                continue
            print(f"  {arm}: " + "  ".join(
                f"q{q}: α med {np.median(alphas[(arm, q)]):+7.0f}B (n={len(alphas[(arm, q)])})" for q in qs))

        # ---------- wedge ranking ----------
        print("\n=== WEDGE RANKING (traffic-weighted recoverable bytes vs cpu2; group = family x size_slot/crop) ===")
        wg = collections.defaultdict(lambda: [0.0, 0, []])
        for fn in files:
            e = summ[fn]
            if e["gap_cpu2"] is None or e["bd_cpu2"] is None:
                continue
            w = TRAFFIC_W.get(e["size_slot"], TRAFFIC_W["1024"] if e["size_slot"] == "crop" else 0.2)
            # excess bytes at matched quality (overlap midpoint), traffic-weighted
            excess = max(0.0, e["gap_cpu2"] / 100.0) * e["refbpp_cpu2"] * e["px"] / 8.0 * w
            g = (e["family"], e["size_slot"] if e["crop_label"] == "full" else e["crop_label"])
            wg[g][0] += excess
            wg[g][1] += 1
            wg[g][2].append(e["bd_cpu2"])
        ranked = sorted(wg.items(), key=lambda kv: -kv[1][0])
        print(f"{'family':>7} {'slot':>8} {'n':>3} {'wKB_excess':>11} {'bd_med':>8}")
        for (fam, slot), (kb, n, bds) in ranked[:20]:
            print(f"{fam:>7} {slot:>8} {n:>3} {kb/1024.0:>11.1f} {np.median(bds):>+8.2f}")

    # ---------- feature correlation ----------
    feat_join = {}
    if args.features and args.manifest and have_zr:
        import pyarrow.parquet as pq

        man = json.load(open(args.manifest))
        opath = {p["origin_id"]: p["image_path"] for p in man["picks"]}
        t = pq.read_table(args.features)
        names = t.schema.names
        featq = {c.split("@")[0]: c for c in names[9:]}
        d = t.to_pydict()
        idx = {}
        for i in range(t.num_rows):
            idx[(d["image_path"][i], d["crop_label"][i], d["size_class"][i])] = i
        vals = collections.defaultdict(list)  # feat -> [(bd, val)]
        njoin = 0
        for fn in files:
            e = summ[fn]
            if e["bd_cpu2"] is None:
                continue
            key = (opath.get(e["origin_id"]), e["crop_label"], e["size_class"])
            if key not in idx:
                continue
            i = idx[key]
            feat_join[fn] = i
            njoin += 1
            for bare, qcol in featq.items():
                v = d[qcol][i]
                if v is not None and not (isinstance(v, float) and math.isnan(v)):
                    vals[bare].append((e["bd_cpu2"], float(v)))
        print(f"\n=== feature correlation with per-file BD vs cpu2 (spearman, n_join={njoin}) ===")
        from scipy.stats import spearmanr  # noqa: PLC0415

        cors = []
        for bare, pairs in vals.items():
            if len(pairs) >= 30:
                bd, v = zip(*pairs)
                if np.std(v) > 1e-12:
                    rho = spearmanr(bd, v).statistic
                    if not math.isnan(rho):
                        cors.append((abs(rho), rho, bare, len(pairs)))
        cors.sort(reverse=True)
        for _, rho, bare, n in cors[:18]:
            print(f"  {rho:+.3f}  {bare} (n={n})")

    # ---------- labeled dataset ----------
    if args.dataset:
        import pyarrow as pa
        import pyarrow.parquet as pqw

        man = json.load(open(args.manifest)) if args.manifest else {"picks": []}
        opath = {p["origin_id"]: p["image_path"] for p in man["picks"]}
        cols = collections.defaultdict(list)
        for r in rows:
            m = cmap.get(r["image"])
            if not m:
                continue
            e = summ[r["image"]]
            cols["file"].append(r["image"])
            cols["origin_id"].append(m["origin_id"])
            cols["origin_path"].append(opath.get(m["origin_id"], ""))
            cols["content_class"].append(m["content_class"])
            cols["family"].append(m["family"])
            cols["crop_label"].append(m["crop_label"])
            cols["size_class"].append(m["size_class"])
            cols["size_slot"].append(e["size_slot"])
            cols["width"].append(int(m["width"]))
            cols["height"].append(int(m["height"]))
            cols["arm"].append(r["_arm"])
            cols["fmt"].append(r["fmt"])
            cols["q"].append(r["q"])
            cols["bytes"].append(r["_bytes"])
            cols["bpp"].append(r["_bpp"])
            cols["ssim2"].append(r["_ssim2"])
            for c in ("enc_ms", "butteraugli_3n", "butteraugli_max"):
                v = r.get(c)
                try:
                    cols[c].append(float(v))
                except (TypeError, ValueError):
                    cols[c].append(float("nan"))
            for ref in ("cpu2", "cpu0"):
                cols[f"file_bd_{ref}"].append(e[f"bd_{ref}"] if e[f"bd_{ref}"] is not None else float("nan"))
            # canonical feature-row join key into imazen26_features_2026-06-23.parquet
            cols["feature_join"].append(
                f"{opath.get(m['origin_id'], '')}|{m['crop_label']}|{m['size_class']}")
        pqw.write_table(pa.table(cols), args.dataset, compression="zstd")
        print(f"\nwrote {args.dataset} ({len(cols['file'])} rows)")

    if args.summary_tsv and have_zr:
        with open(args.summary_tsv, "w") as f:
            w = csv.writer(f, delimiter="\t")
            w.writerow(["file", "origin_id", "family", "content_class", "crop_label", "size_class",
                        "size_slot", "px", "bd_cpu2", "gap_mid_cpu2", "mid_ssim2_cpu2", "bd_cpu0"])
            for fn in files:
                e = summ[fn]
                w.writerow([fn, e["origin_id"], e["family"], e["content_class"], e["crop_label"],
                            e["size_class"], e["size_slot"], e["px"],
                            *(f"{e[k]:.3f}" if e[k] is not None else "NA"
                              for k in ("bd_cpu2", "gap_cpu2", "mid_cpu2", "bd_cpu0"))])
        print(f"wrote {args.summary_tsv}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Size-decay isolation A/B attribution (HYPERPARAM_FIRST_CUT rule 2 / wedge #3).

Consumes the per-arm TSVs written by scripts/rd_gap/sizedecay_arms.sh
(sd_full / sd_off / sd_no_<mech>) over the photo-like size ladder
{256, 512, 1024} and attributes the tune's size-conditional advantage decay to
mechanisms via leave-one-out contributions:

  contribution(X | size) = BD-rate(full vs no_X) at that size
      (negative = the mechanism saves bits at matched quality; the mechanism
       is CONVICTED for the decay when its contribution shrinks toward 0 or
       flips positive as px falls — especially in the high-q band, where the
       1024->512 decay lives)

  tune_total(size) = BD-rate(full vs off)  — the ceiling the mechanisms sum
       toward (not exactly additive: mechanisms interact, e.g. no_qmcurves
       also neutralizes the qmdist ratio because QM tables vanish).

Band decomposition mirrors fit_size_decay.band_gaps: bpp gap % at the 25%/75%
points of the overlapping ssim2 window of the two arms being compared
(lowq = lower-quality end, highq = upper end).

Usage:
  analyze_size_decay_ab.py --train-dir <dir-with-sd_*.tsv> [--val-dir <dir>]
      [--out-tsv benchmarks/...tsv]
"""
import argparse
import os
import re
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "rd_gap"))
from bd_arm import bd_rate, frontier, load  # noqa: E402

MECHS = ["chromadq", "qmcurves", "boost", "qmdist", "lfsharp"]
SIZE_RE = re.compile(r"\.(full|c50_[a-z]+)\.(?:s(\d+)|(native))\.png$")


def size_class_of(image_name):
    m = SIZE_RE.search(image_name)
    if not m:
        return None
    if m.group(3):
        return "native"
    return m.group(2)


def origin_of(image_name):
    return image_name.split("_", 1)[0]


def px_of(tsv_rows, image):
    r = tsv_rows[image][0]
    return r


def load_arm(path, metric="ssim2"):
    """image -> list[(quality_axis, bpp)] for zenrav1e rows."""
    rows, _ = load(path, metric)
    return rows


def band_gaps(zf, rf):
    """bpp gap % (test vs ref) at 25%/75% of the overlapping ssim2 window."""
    if len(zf) < 2 or len(rf) < 2:
        return None, None
    lo = max(zf[0][0], rf[0][0])
    hi = min(zf[-1][0], rf[-1][0])
    if hi <= lo:
        return None, None
    out = []
    for fq in (0.25, 0.75):
        s = lo + fq * (hi - lo)
        tb = float(np.interp(s, [p[0] for p in zf], [p[1] for p in zf]))
        rb = float(np.interp(s, [p[0] for p in rf], [p[1] for p in rf]))
        out.append(100.0 * (tb - rb) / rb)
    return out[0], out[1]


def px_table(sample_tsvs):
    """image basename -> (w, h) from the harness sample TSVs."""
    px = {}
    for tsv in sample_tsvs:
        if not tsv or not os.path.exists(tsv):
            continue
        with open(tsv) as f:
            next(f)
            for line in f:
                p, w, h, _fam = line.rstrip("\n").split("\t")
                px[os.path.basename(p)] = (int(w), int(h))
    return px


def analyze(dirpath, sample_tsvs, split_name):
    arms = {}
    import glob as _glob
    for p in sorted(_glob.glob(os.path.join(dirpath, "sd_*.tsv"))):
        arm = os.path.basename(p)[3:-4]
        arms[arm] = {
            "ssim2": load_arm(p, "ssim2"),
            "butteraugli_3n": load_arm(p, "butteraugli_3n"),
            "butteraugli_max": load_arm(p, "butteraugli_max"),
        }
    if "full" not in arms:
        print(f"[{split_name}] no sd_full.tsv in {dirpath}", file=sys.stderr)
        return None
    pxs = px_table(sample_tsvs)

    recs = []
    images = sorted(arms["full"]["ssim2"])
    for img in images:
        sc = size_class_of(img)
        if sc is None:
            continue
        w, h = pxs.get(img, (None, None))
        px = (w * h) if w else None
        base_f = arms["full"]["ssim2"][img]
        for arm_name, data in arms.items():
            if arm_name == "full" or img not in data["ssim2"]:
                continue
            # BD of full vs the comparison arm: negative = full wins.
            bd = bd_rate(frontier(base_f), frontier(data["ssim2"][img]))
            glo, ghi = band_gaps(frontier(base_f), frontier(data["ssim2"][img]))
            rec = dict(split=split_name, image=img, origin=origin_of(img),
                       size_class=sc, px=px, arm=arm_name, bd_ssim2=bd,
                       gap_lowq=glo, gap_highq=ghi)
            for bm in ("butteraugli_3n", "butteraugli_max"):
                b = arms["full"][bm].get(img)
                a = data[bm].get(img)
                rec[f"bd_{bm}"] = (
                    bd_rate(frontier(b), frontier(a)) if b and a else None
                )
            recs.append(rec)
    D = pd.DataFrame(recs)

    def slot(rec):
        sc = rec["size_class"]
        if sc in ("256", "512", "1024"):
            return sc
        # native: slot by long edge — 1024-long-edge natives (e.g. a
        # 1024x1024 source, downscale-only) are true 1024-class cells;
        # odd natives (8464 @667) stay out of the slot medians but keep
        # their px for the decay slopes.
        w, h = pxs.get(rec["image"], (0, 0))
        return "1024" if max(w, h) >= 1024 else "odd"

    D["slot"] = D.apply(slot, axis=1)

    print(f"\n################ {split_name} ################")
    print("\n=== tune total: BD(full vs off) per size (medians; negative = tune wins) ===")
    t = D[D["arm"] == "off"]
    tab = t.groupby("slot").agg(
        n=("bd_ssim2", "size"), bd_med=("bd_ssim2", "median"), bd_mean=("bd_ssim2", "mean"),
        better=("bd_ssim2", lambda v: f"{int((v < 0).sum())}/{len(v)}"),
        lowq_med=("gap_lowq", "median"), highq_med=("gap_highq", "median"),
        ba3n_med=("bd_butteraugli_3n", "median"), bamax_med=("bd_butteraugli_max", "median"),
    ).reindex(["256", "512", "1024"])
    print(tab.to_string(float_format=lambda x: f"{x:+.2f}"))

    print("\n=== leave-one-out mechanism contributions: BD(full vs no_X) per size ===")
    print("    (negative = mechanism X saves bits at that size; decay toward 0/+ convicts X)")
    rows = []
    for m in MECHS:
        sub = D[D["arm"] == f"no_{m}"]
        for s in ("256", "512", "1024"):
            g = sub[sub["slot"] == s]
            if g.empty:
                continue
            v = g["bd_ssim2"].dropna()
            rows.append(dict(
                mechanism=m, slot=s, n=len(v),
                bd_med=v.median(), bd_mean=v.mean(),
                better=f"{int((v < 0).sum())}/{len(v)}",
                lowq_med=g["gap_lowq"].median(), highq_med=g["gap_highq"].median(),
                ba3n_med=g["bd_butteraugli_3n"].median(),
                bamax_med=g["bd_butteraugli_max"].median(),
            ))
    M = pd.DataFrame(rows)
    if not M.empty:
        piv = M.pivot_table(index="mechanism", columns="slot",
                            values=["bd_med", "highq_med", "lowq_med"], sort=False)
        print(piv.to_string(float_format=lambda x: f"{x:+.2f}"))
        print("\nfull table:")
        print(M.to_string(index=False, float_format=lambda x: f"{x:+.3f}"))
    else:
        print("    (no no_<mech> arms in this dir)")

    ramp_arms = sorted(a for a in arms if a.startswith("ramp_"))
    if ramp_arms:
        print("\n=== ramp trial arms: BD(full vs ramp) per size ===")
        print("    (ship bar per DECISION_RULE: bd >= +0.3 at convicted sizes i.e. the ramp arm")
        print("     BEATS full there; negative = the ramp LOSES win vs full strength)")
        rows = []
        for a in ramp_arms:
            sub = D[D["arm"] == a]
            for s in ("256", "512", "1024"):
                g = sub[sub["slot"] == s]
                if g.empty:
                    continue
                v = g["bd_ssim2"].dropna()
                rows.append(dict(
                    arm=a, slot=s, n=len(v), bd_med=v.median(), bd_mean=v.mean(),
                    ramp_better=f"{int((v > 0).sum())}/{len(v)}",
                    ba3n_med=g["bd_butteraugli_3n"].median(),
                    bamax_med=g["bd_butteraugli_max"].median(),
                ))
        R = pd.DataFrame(rows)
        print(R.to_string(index=False, float_format=lambda x: f"{x:+.3f}"))
        print("    NOTE: bd here is BD(full vs ramp): POSITIVE = full needs more bits = ramp WINS.")

    print("\n=== per-mechanism contribution decay slope (-d(bd)/d(log2 px), per origin; + = mechanism win decays toward small) ===")
    slopes = []
    for m in MECHS:
        sub = D[(D["arm"] == f"no_{m}") & D["bd_ssim2"].notna() & D["px"].notna()]
        for o, g in sub.groupby("origin"):
            if g["slot"].nunique() < 3:
                continue
            b, _a = np.polyfit(np.log2(g["px"].astype(float)), g["bd_ssim2"], 1)
            slopes.append(dict(mechanism=m, origin=o, slope=-b))
    S = pd.DataFrame(slopes)
    if not S.empty:
        agg = S.groupby("mechanism")["slope"].agg(["median", "mean", "count"])
        print(agg.reindex(MECHS).to_string(float_format=lambda x: f"{x:+.3f}"))
    return D, M, S


def vs_cpu2(dirpath, split_name, origins, cpu2_tsv=None):
    """BD vs libaom cpu2 per (arm, size): decomposes the wedge-observed decay
    into tune-attributable vs baseline-attributable parts. cpu2 frontiers come
    from the label store's wedge rows (train) or an aom_only TSV (val)."""
    if cpu2_tsv:
        cpu_rows, _ = load(cpu2_tsv, "ssim2", encoder="libaom")
        def cpu2_frontier(origin, sc):
            pts = []
            for img, p in cpu_rows.items():
                if origin_of(img) == origin and size_class_of(img) == sc:
                    pts.extend(p)
            return frontier(pts) if pts else None
    else:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        from hp_common import _quality_pts, load_store
        store = load_store(sweep_source="wedge-2026-07-03")
        cpu2 = store[(store["arm_id"] == "wedge/aom-cpu2")
                     & (store["crop_label"] == "full")]
        def cpu2_frontier(origin, sc):
            g = cpu2[(cpu2["origin_id"].astype(str) == origin)
                     & (cpu2["size_class"] == sc)]
            return frontier(_quality_pts(g, "ssim2")) if not g.empty else None

    rows = []
    for arm in ("full", "off"):
        p = os.path.join(dirpath, f"sd_{arm}.tsv")
        if not os.path.exists(p):
            continue
        zr, _ = load(p, "ssim2")
        for img, pts in zr.items():
            o, sc = origin_of(img), size_class_of(img)
            if o not in origins or sc is None:
                continue
            rf = cpu2_frontier(o, sc)
            if rf is None:
                continue
            bd = bd_rate(frontier(pts), rf)
            glo, ghi = band_gaps(frontier(pts), rf)
            slot = sc if sc in ("256", "512", "1024") else "1024"
            rows.append(dict(arm=arm, origin=o, slot=slot, bd=bd,
                             gap_lowq=glo, gap_highq=ghi))
    D = pd.DataFrame(rows)
    if D.empty:
        print(f"[{split_name}] no cpu2 reference rows")
        return
    print(f"\n=== [{split_name}] BD vs libaom cpu2 per size (medians; negative = zr wins) ===")
    tab = D.pivot_table(index="slot", columns="arm",
                        values=["bd", "gap_highq"], aggfunc="median")
    tab = tab.reindex(["256", "512", "1024"])
    if ("bd", "full") in tab.columns and ("bd", "off") in tab.columns:
        tab[("bd", "tune_delta")] = tab[("bd", "full")] - tab[("bd", "off")]
    print(tab.to_string(float_format=lambda x: f"{x:+.2f}"))
    print("    (bd/off = the tune-OFF baseline's own gap — decay here is NOT tune-attributable)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--train-dir", required=True)
    ap.add_argument("--val-dir")
    ap.add_argument("--val-cpu2-tsv", help="aom_only.sh cpu2 TSV over the val corpus")
    ap.add_argument("--train-sample", default=os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "..", "rd_gap", "sample_sizedecay_train.tsv"))
    ap.add_argument("--val-sample", default=os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "..", "rd_gap", "sample_sizedecay_val.tsv"))
    ap.add_argument("--out-tsv")
    args = ap.parse_args()

    def origins_of(sample):
        with open(sample) as f:
            next(f)
            return {os.path.basename(l.split("\t")[0]).split("_", 1)[0] for l in f}

    out_frames = []
    r = analyze(args.train_dir, [args.train_sample], "train")
    if r:
        out_frames.append(r[0])
        vs_cpu2(args.train_dir, "train", origins_of(args.train_sample))
    if args.val_dir:
        r = analyze(args.val_dir, [args.val_sample], "val")
        if r:
            out_frames.append(r[0])
        if args.val_cpu2_tsv:
            vs_cpu2(args.val_dir, "val", origins_of(args.val_sample),
                    cpu2_tsv=args.val_cpu2_tsv)

    if args.out_tsv and out_frames:
        allD = pd.concat(out_frames, ignore_index=True)
        with open(args.out_tsv, "w") as f:
            f.write("# size-decay isolation A/B: leave-one-out mechanism attribution (wedge #3 / HYPERPARAM_FIRST_CUT rule 2)\n")
            f.write("# arm=off rows: BD(full-tune vs tune-off) = total tune win; arm=no_<mech> rows: BD(full vs full-minus-mech) = mechanism contribution (negative = mechanism wins)\n")
            f.write("# gap_lowq/gap_highq = bpp gap %% at 25%%/75%% of the overlapping ssim2 window (fit_size_decay band convention)\n")
            allD.to_csv(f, sep="\t", index=False, float_format="%.4f")
        print(f"\nwrote {args.out_tsv}")


if __name__ == "__main__":
    main()

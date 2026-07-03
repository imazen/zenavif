#!/usr/bin/env python3
"""Palette-gate mechanism A/B analysis (HYPERPARAM_FIRST_CUT rule 1 graduation).

Inputs (see benchmarks/hyperparam_palette_mech_ab_2026-07-03.tsv header for the
run provenance):
  * shipped-cavif config (cavif -s2 --depth 8, ZENRAVIF_TUNE=ssimulacra2, wedge
    binary chain ravif--wedge@9d2b97c -> zenrav1e--wedge@32477046):
      - always arm: fresh sweep over train fired/quiet/photo + val files (12q)
      - off arm: fresh sweep over t3/t4/t5 + val files (12q)
      - auto arm: fresh sweep over val files (12q)
      - off/auto arms on the wedge fired subset (t1 full + t2 crops): REUSED
        from the label store's wedge-2026-07-03 rows (byte-continuity of the
        binary verified: 7052.full.native q60 auto = 2646 bytes, bit-identical)
  * isolated config (rav1e CLI --still-picture --threads 1 --lrf false
    --filter-intra false, 420 y4m, aomdec decode): 3 arms x s{2,6} x 5q fresh
    (the palette-ab-final2 pipeline, scripts/rd_gap/palette_iso_cell.sh)

Analysis:
  1. per-(config x speed x file) direct BDs vs the same-config off arm
     (ssim2 + butteraugli veto columns, hp_common/bd_arm conventions)
  2. three-way gate-mode table per (split x class x size_slot):
     auto vs always vs RULE (fire iff patch_fraction > 0.197 -> always else auto)
  3. val-split confusion of the rule vs "always actually won" (<= -0.5 veto-adj)
  4. false-fire cost: quiet/photo classes' bd_always + timing ratios
  5. threshold refit on val labels (same objective as fit_palette_gate.py):
     does the fitted threshold move?

Usage:
  python3 analyze_palette_mech_ab.py \
    --shipped-always A.tsv --shipped-off B.tsv --shipped-auto C.tsv \
    --iso D.tsv [--timing-off T1.tsv --timing-always T2.tsv] --out-prefix X
"""
import argparse
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import (arm_bd_per_image, load_features, load_store,  # noqa: E402
                       split_of, to_tsv, veto_table)

THRESH = 0.197
WIN_BAR = -0.5
WEDGE_MAP = "/mnt/v/output/rd-gap-wedge-2026-07-03/corpus_map.tsv"
VAL_MAP = "/mnt/v/output/rd-gap-palette-val-2026-07-03/corpus_map.tsv"
WEDGE_ORIGINS = "/mnt/v/output/rd-gap-wedge-2026-07-03/_MANIFEST.json"


def load_meta():
    """file (basename with .png) -> origin/class/crop/size/w/h/split/feature_join."""
    import json
    frames = []
    for root, mp in [("/mnt/v/output/rd-gap-wedge-2026-07-03", WEDGE_MAP),
                     ("/mnt/v/output/rd-gap-palette-val-2026-07-03", VAL_MAP)]:
        m = pd.read_csv(mp, sep="\t", dtype=str)
        if "origin_path" not in m.columns:
            # wedge corpus_map was slimmed (origin_path dropped); rebuild from manifest picks
            picks = {p["origin_id"]: p["image_path"]
                     for p in json.load(open(WEDGE_ORIGINS))["picks"]}
            m["origin_path"] = m["origin_id"].map(picks)
        frames.append(m)
    m = pd.concat(frames, ignore_index=True)
    m["feature_join"] = m["origin_path"] + "|" + m["crop_label"] + "|" + m["size_class"]
    m["base"] = m["origin_path"].str.rsplit("/", n=1).str[-1]
    m["split"] = m["base"].map(split_of)
    m["cgroup"] = m["content_class"].str.extract(r"^(\d+)")
    # size slot: crops stay 'c50'; full crops at {256,512} keep their class;
    # the "1024 slot" = s1024 or native-<=1024
    def slot(r):
        if r["crop_label"] != "full":
            return "c50"
        if r["size_class"] in ("256", "512", "1024"):
            return r["size_class"]
        return "1024" if max(int(r["width"]), int(r["height"])) <= 1024 else "top"
    m["size_slot"] = m.apply(slot, axis=1)
    feats = load_features(["patch_fraction"])
    m["patch_fraction"] = m["feature_join"].map(feats["patch_fraction"])
    m["fires"] = m["patch_fraction"] > THRESH
    return m.set_index("file")


def read_run_gap(path, arm, speed=2):
    df = pd.read_csv(path, sep="\t", comment="#")
    df = df.rename(columns={"image": "image_id"})
    df["arm_id"] = arm
    df["speed"] = speed
    return df[["image_id", "arm_id", "speed", "q", "bytes", "bpp", "ssim2",
               "butteraugli_3n", "butteraugli_max", "enc_ms"]]


def read_iso(path, meta):
    df = pd.read_csv(path, sep="\t", comment="#")
    df["image_id"] = df["image"] + ".png"
    px = df["image_id"].map(meta["width"].astype(int) * meta["height"].astype(int))
    df["bpp"] = df["bytes"] * 8.0 / px
    df = df.rename(columns={"arm": "arm_id", "butter_p3": "butteraugli_3n",
                            "butter_max": "butteraugli_max"})
    return df[["image_id", "arm_id", "speed", "q", "bytes", "bpp", "ssim2",
               "butteraugli_3n", "butteraugli_max", "enc_ms"]]


def store_wedge_rows(meta):
    """Reused off/auto shipped rows for the wedge fired subset (t1+t2)."""
    st = load_store(sweep_source="wedge-2026-07-03")
    st = st[st["arm_id"].isin(["wedge/zr-best_s2", "wedge/zr-paletteoff_s2"])]
    st = st[st["image_id"].isin(meta.index)]
    st = st.rename(columns={})
    out = st[["image_id", "arm_id", "q", "bytes", "bpp", "ssim2",
              "butteraugli_3n", "butteraugli_max", "enc_ms"]].copy()
    out["arm_id"] = out["arm_id"].map({"wedge/zr-best_s2": "auto",
                                       "wedge/zr-paletteoff_s2": "off"})
    out["speed"] = 2
    return out


def bd_frame(rows, meta, config_name):
    """Per-(speed, file) BDs of auto & always vs off + veto columns.

    Arms are restricted to their COMMON q grid per (file, pair) before the
    frontier — the reused store rows are 6-pt while fresh runs are 12-pt,
    and mixing grid densities biases the trapezoid on curvy small-size RD
    curves (measured +1..+4% phantom BD on byte-near-identical photo arms).
    """
    rows = rows.copy()
    rows["sweep_source"] = config_name
    recs = []
    for spd in sorted(rows["speed"].unique()):
        ssub = rows[rows["speed"] == spd]
        # emulate the store shape veto_table expects
        for test in ("always", "auto"):
            have_base = set(ssub.loc[ssub["arm_id"] == "off", "image_id"])
            have_test = set(ssub.loc[ssub["arm_id"] == test, "image_id"])
            files = sorted(have_base & have_test)
            if not files:
                continue
            pair = ssub[ssub["arm_id"].isin(["off", test])].copy()
            common = (pair.groupby(["image_id", "q"])["arm_id"].nunique()
                      .rename("n_arms").reset_index())
            common = common[common["n_arms"] == 2][["image_id", "q"]]
            pair = pair.merge(common, on=["image_id", "q"])
            vt = veto_table(pair, config_name, "off", test)
            for fn in files:
                if fn not in vt.index:
                    continue
                r = vt.loc[fn]
                recs.append(dict(config=config_name, speed=spd, file=fn, arm=test,
                                 bd=r.get("bd_ssim2", np.nan),
                                 ba3n=r.get("bd_ba3n", np.nan),
                                 bamax=r.get("bd_bamax", np.nan)))
    bd = pd.DataFrame(recs)
    if bd.empty:
        return bd
    piv = bd.pivot_table(index=["config", "speed", "file"], columns="arm",
                         values=["bd", "ba3n", "bamax"])
    piv.columns = [f"{a}_{b}" for a, b in piv.columns]
    piv = piv.reset_index()
    for c in ["origin_id", "content_class", "cgroup", "crop_label", "size_slot",
              "split", "patch_fraction", "fires"]:
        piv[c] = piv["file"].map(meta[c])
    return piv


def veto_adjusted_always(df):
    vet = (df["ba3n_always"].to_numpy() > 1.0) | (df["bamax_always"].to_numpy() > 1.5)
    return np.where(vet, np.maximum(df["bd_always"].to_numpy(), 0.0),
                    df["bd_always"].to_numpy()), vet


def three_way(df, label):
    """median bd per (split x cgroup x size_slot) for auto / always / rule."""
    df = df.copy()
    adj, vet = veto_adjusted_always(df)
    df["bd_always_adj"] = adj
    df["vetoed"] = vet
    df["bd_auto_f"] = df["bd_auto"].fillna(0.0)
    df["bd_rule"] = np.where(df["fires"], df["bd_always_adj"], df["bd_auto_f"])
    print(f"\n=== three-way (config={label}): median BD vs off per (split x class x size_slot) ===")
    g = (df.groupby(["speed", "split", "cgroup", "size_slot"])
         .agg(n=("file", "size"), fire=("fires", "sum"),
              auto=("bd_auto_f", "median"), always=("bd_always_adj", "median"),
              rule=("bd_rule", "median"), vetoed=("vetoed", "sum")))
    print(g.to_string(float_format=lambda x: f"{x:+.2f}"))
    return df


def confusion(df, label):
    adj, _ = veto_adjusted_always(df)
    df = df.assign(bd_always_adj=adj)
    won = (df["bd_always_adj"] <= WIN_BAR).to_numpy()
    fire = df["fires"].to_numpy().astype(bool)
    print(f"\n=== {label}: rule confusion vs 'always actually won' (n={len(df)}) ===")
    print(f"  fire&won {int((fire & won).sum()):>3}  fire&lost {int((fire & ~won).sum()):>3}")
    print(f"  miss&won {int((~fire & won).sum()):>3}  quiet&lost {int((~fire & ~won).sum()):>3}")
    fp = df[fire & ~won]
    if len(fp):
        cost = fp["bd_always_adj"]
        print(f"  fire&lost cost: median {cost.median():+.2f} max {cost.max():+.2f}")
        worst = fp.nlargest(5, "bd_always_adj")
        print("  worst false fires:", ", ".join(
            f"{r.file.split('_')[0]}.{r.size_slot}.s{int(r.speed)}({r.bd_always_adj:+.1f})"
            for r in worst.itertuples()))
    fn = df[~fire & won]
    if len(fn):
        print("  missed wins:", ", ".join(
            f"{r.file.split('_')[0]}.{r.size_slot}.s{int(r.speed)}({r.bd_always_adj:+.1f})"
            for r in fn.itertuples()))


def refit_threshold(df, label):
    """Same objective as fit_palette_gate.py (mean veto-adjusted realized BD,
    no-auto-rescue; specificity tiebreak within 0.1) on these labels,
    single-feature patch_fraction gates only."""
    adj, _ = veto_adjusted_always(df)
    pf = df["patch_fraction"].to_numpy()
    ok = np.isfinite(pf) & np.isfinite(adj)
    pf, adj = pf[ok], adj[ok]
    u = np.unique(pf)
    taus = (u[:-1] + u[1:]) / 2.0
    cands = []
    for t in taus:
        fire = pf > t
        cands.append((np.where(fire, adj, 0.0).mean(), int(fire.sum()), t))
    if not cands:
        print(f"  {label}: no candidates")
        return None
    best = min(c[0] for c in cands)
    pool = sorted([c for c in cands if c[0] <= best + 0.1], key=lambda c: (c[1], c[0]))
    m, nf, t = pool[0]
    lo = min(c[2] for c in pool)
    hi = max(c[2] for c in pool)
    cur_fire = pf > THRESH
    cur = np.where(cur_fire, adj, 0.0).mean()
    refit_fire = pf > t
    same_set = bool((cur_fire == refit_fire).all())
    verdict = ("IDENTICAL fire set" if same_set
               else "within fit tolerance" if cur <= best + 0.1
               else "MOVED")
    print(f"  {label}: refit tau={t:.4f} (mean {m:+.3f}, fires {nf}/{len(pf)}); "
          f"candidate plateau [{lo:.4f}, {hi:.4f}]; shipped {THRESH} -> mean {cur:+.3f} "
          f"fires {int(cur_fire.sum())}/{len(pf)} [{verdict}, delta {cur - best:+.3f}]")
    return t


def timing(path_off, path_always, meta, label):
    off = read_run_gap(path_off, "off")
    alw = read_run_gap(path_always, "always")
    j = off.merge(alw, on=["image_id", "q"], suffixes=("_off", "_alw"))
    j["ratio"] = j["enc_ms_alw"] / j["enc_ms_off"]
    j["fires"] = j["image_id"].map(meta["fires"])
    j["cgroup"] = j["image_id"].map(meta["cgroup"])
    print(f"\n=== {label}: always/off enc_ms ratios (RD_CACHE=off cells) ===")
    for f in (True, False):
        s = j[j["fires"] == f]["ratio"]
        if len(s):
            print(f"  gate-{'fired' if f else 'quiet'} files: n={len(s)} "
                  f"median {s.median():.2f}x p90 {s.quantile(0.9):.2f}x max {s.max():.2f}x")
    g = j.groupby("cgroup")["ratio"].median()
    print("  per-class median:", ", ".join(f"{k}:{v:.2f}x" for k, v in g.items()))
    return j[["image_id", "q", "enc_ms_off", "enc_ms_alw", "ratio", "fires", "cgroup"]]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--shipped-always", required=True)
    ap.add_argument("--shipped-off", required=True)
    ap.add_argument("--shipped-auto", required=True)
    ap.add_argument("--shipped-s6-always")
    ap.add_argument("--shipped-s6-off")
    ap.add_argument("--shipped-s6-auto")
    ap.add_argument("--iso", required=True)
    ap.add_argument("--timing-off")
    ap.add_argument("--timing-always")
    ap.add_argument("--out-prefix", default=os.path.join(
        os.path.dirname(os.path.abspath(__file__)),
        "../../benchmarks/hyperparam_palette_mech_ab_2026-07-03"))
    args = ap.parse_args()

    meta = load_meta()
    frames = [
        read_run_gap(args.shipped_always, "always"),
        read_run_gap(args.shipped_off, "off"),
        read_run_gap(args.shipped_auto, "auto"),
        store_wedge_rows(meta),
    ]
    if args.shipped_s6_always:
        frames.append(read_run_gap(args.shipped_s6_always, "always", speed=6))
    if args.shipped_s6_off:
        frames.append(read_run_gap(args.shipped_s6_off, "off", speed=6))
    if args.shipped_s6_auto:
        frames.append(read_run_gap(args.shipped_s6_auto, "auto", speed=6))
    shipped = pd.concat(frames, ignore_index=True)
    iso = read_iso(args.iso, meta)

    S = bd_frame(shipped, meta, "shipped")
    I = bd_frame(iso, meta, "iso")
    both = pd.concat([S, I], ignore_index=True)

    S = three_way(S, "shipped-cavif s2")
    I = three_way(I, "isolated rav1e CLI s2+s6")

    for cfg, df in [("shipped", S), ("iso", I)]:
        for spd in sorted(df["speed"].unique()):
            sub = df[(df["split"] == "val") & (df["speed"] == spd)]
            if len(sub):
                confusion(sub, f"{cfg} s{spd} VAL")
        tr = df[df["split"] == "train"]
        if len(tr):
            confusion(tr, f"{cfg} all-speeds TRAIN")

    print("\n=== threshold refit (objective: mean veto-adjusted realized BD, no-auto rescue) ===")
    for cfg, df in [("shipped", S), ("iso", I)]:
        for split in ("val", "train"):
            sub = df[df["split"] == split]
            if len(sub):
                refit_threshold(sub, f"{cfg} {split} (n={len(sub)})")
        refit_threshold(df, f"{cfg} pooled (n={len(df)})")

    if args.timing_off and args.timing_always:
        tj = timing(args.timing_off, args.timing_always, meta, "shipped s2 timing")
        to_tsv(tj.set_index("image_id"),
               os.path.abspath(args.out_prefix.replace("_ab", "_timing") + ".tsv"),
               ["palette always/off enc_ms ratios, shipped cavif -s2 config, RD_CACHE=off JOBS=8 "
                "(box zenavif-sweep-2, otherwise idle), q {50,75}",
                "fires = the deployed gate (patch_fraction > 0.197) at the encode rendition"])

    hdr = [
        "palette-gate mechanism A/B (HYPERPARAM_FIRST_CUT rule 1 graduation; wedge #6)",
        "arms: palette {off,always,auto} x configs {shipped cavif ss2-tune s2+s6, isolated rav1e CLI s2+s6} x sizes {256,512,1024|native,c50}",
        "binary chain: ravif--wedge@9d2b97c -> zenrav1e--wedge@32477046 (wedge continuity verified bit-exact)",
        f"rule: fire PaletteMode::Always iff patch_fraction > {THRESH} (features at the encode rendition)",
        "bd/ba3n/bamax_{auto,always} = direct BD vs same-config off arm (butteraugli veto: ba3n>+1.0 or bamax>+1.5)",
        "cell = <origin>.<crop-or-size-slot>; full per-run rows + corpus maps: /mnt/v/output/rd-gap-palette-ab-2026-07-03/ (see the .pointer.md)",
    ]
    both_out = both.sort_values(["config", "speed", "split", "cgroup", "size_slot", "file"]).copy()
    both_out["cell"] = (both_out["origin_id"].astype(str) + "."
                        + np.where(both_out["crop_label"] != "full",
                                   both_out["crop_label"], both_out["size_slot"]))
    slim = both_out[["cell", "config", "speed", "split", "cgroup", "size_slot",
                     "patch_fraction", "fires", "bd_auto", "bd_always",
                     "ba3n_always", "bamax_always"]]
    to_tsv(slim.set_index("cell"), os.path.abspath(args.out_prefix + ".tsv"), hdr)


if __name__ == "__main__":
    main()

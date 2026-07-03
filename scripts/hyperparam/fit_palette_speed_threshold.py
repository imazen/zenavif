#!/usr/bin/env python3
"""Speed-conditional palette-gate threshold A/B (follow-up to the mechanism A/B).

Question (HYPERPARAM_FIRST_CUT rule 1, documented follow-up): the mechanism
A/B's val-only threshold refits moved DOWN to 0.046-0.066 — entirely an s6
phenomenon. Is the optimal patch_fraction threshold speed-conditional
(0.197 at s2-class speeds, ~0.05 at s6-class), and does that confirm on the
held-out VAL origins?

Arms: tau in {0.197 (shipped), 0.10, 0.05} + fire-always, per speed {2, 6}.

100% OFFLINE — no fresh encodes. Every (file, speed) cell already has both
palette outcomes measured (off + always + auto), so a threshold arm is a pure
per-cell SELECTION: fire -> the always outcome (veto-adjusted), quiet -> the
auto outcome. Sources:
  * benchmarks/hyperparam_palette_mech_ab_2026-07-03.tsv — the canonical
    per-cell BD table from the mechanism A/B (iso + shipped configs, s2+s6,
    train+val, sizes {256,512,1024,c50,top}); 3 shipped-s2 'top' cells have
    no always-arm BD (dropped, reported).
  * label store palette-ab-final2-2026-07-03 — 24 train26 origins @1024,
    s2+s6 (the original fit labels; vipsthumbnail rendition — 3 origins also
    appear in the mech-iso corpus on different pixels; kept, flagged).

Objectives (both reported; DEPLOY decides):
  * fit view (fit_palette_gate.py continuity): mean(fire ? veto_adj_always : 0)
    — credits a fired win against a zero baseline. This view PHANTOM-credits
    cells where auto's own detection already captured the identical win
    (e.g. 9905.256 s6: bd_auto == bd_always == -4.47, pf 0.1935): lowering
    tau "claims" them but deployment gains nothing.
  * deploy view (three_way bd_rule convention): mean/median of
    (fire ? veto_adj_always : bd_auto) — the honest realized-BD-vs-off of the
    gate policy as shipped. Arm deltas under this view are the real value.

Butteraugli discipline: per-cell veto (ba3n > +1.0 or bamax > +1.5 vs off)
refuses to bank a fired ssim2 win (max(bd,0)); genuine losses stay losses.
NaN butteraugli BDs (no overlapping quality window) cannot veto — the
analyze_palette_mech_ab.py convention, kept.

Decision rule (established): per-speed train fit (plateau + specificity
tiebreak: within 0.1 BD of best, fewest fires — every fire costs palette
search time, s6 med 1.80x on fired files) -> val confirm on ssim2 BD with
the butteraugli veto + a butteraugli-clean check of the incremental claim.
LSD splits come from the canonical origin_split (already stamped in both
sources).

Usage: python3 fit_palette_speed_threshold.py   (writes
benchmarks/hyperparam_palette_speed_ab_2026-07-03.tsv + prints the tables)
"""

import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import (arm_bd_per_image, load_features, load_store,  # noqa: E402
                       to_tsv, veto_table)

HERE = os.path.dirname(os.path.abspath(__file__))
MECH_TSV = os.path.join(HERE, "../../benchmarks/hyperparam_palette_mech_ab_2026-07-03.tsv")
OUT_TSV = os.path.join(HERE, "../../benchmarks/hyperparam_palette_speed_ab_2026-07-03.tsv")

SHIPPED = 0.197
ARMS = [("t0.197", 0.197), ("t0.10", 0.10), ("t0.05", 0.05), ("always", -1.0)]
WIN_BAR = -0.5


def load_cells():
    mech = pd.read_csv(MECH_TSV, sep="\t", comment="#")
    n_nan = int(mech["bd_always"].isna().sum())
    mech = mech[mech["bd_always"].notna()].copy()

    # final2: per-(origin,speed) BDs @1024, the original fit labels.
    store = load_store(sweep_source="palette-ab-final2-2026-07-03")
    feats = load_features(["patch_fraction"])
    meta = (store[["image_id", "origin_id", "content_class", "feature_join"]]
            .drop_duplicates().set_index("image_id"))
    rows = []
    for spd in (2, 6):
        alw = veto_table(store, "palette-ab-final2-2026-07-03",
                         f"palette/off_s{spd}", f"palette/always_s{spd}")
        auto = arm_bd_per_image(store, "palette-ab-final2-2026-07-03",
                                f"palette/off_s{spd}", f"palette/auto_s{spd}")
        for img in alw.index:
            m = meta.loc[img]
            pf = float(feats.loc[m["feature_join"], "patch_fraction"])
            rows.append(dict(
                cell=f"{m['origin_id']}.1024", config="final2", speed=spd,
                split="train", cgroup=str(m["content_class"])[:4],
                size_slot="1024", patch_fraction=pf, fires=pf > SHIPPED,
                bd_auto=float(auto.loc[img, "bd"]) if img in auto.index else np.nan,
                bd_always=float(alw.loc[img, "bd_ssim2"]),
                ba3n_always=float(alw.loc[img, "bd_ba3n"]),
                bamax_always=float(alw.loc[img, "bd_bamax"])))
    f2 = pd.DataFrame(rows)
    cells = pd.concat([mech, f2], ignore_index=True)
    cells["cgroup"] = cells["cgroup"].astype(str)
    print(f"cells: mech {len(mech)} (dropped {n_nan} NaN bd_always: shipped-s2 'top' "
          f"slots without an always arm) + final2 {len(f2)} = {len(cells)}")
    print(cells.groupby(["speed", "split"]).size().rename("n").to_string())
    return cells


def veto_adj(df):
    vet = (df["ba3n_always"].to_numpy() > 1.0) | (df["bamax_always"].to_numpy() > 1.5)
    return np.where(vet, np.maximum(df["bd_always"].to_numpy(), 0.0),
                    df["bd_always"].to_numpy()), vet


def arm_eval(df, tau):
    adj, vet = veto_adj(df)
    fire = df["patch_fraction"].to_numpy() > tau
    auto = df["bd_auto"].fillna(0.0).to_numpy()
    fit = np.where(fire, adj, 0.0)
    dep = np.where(fire, adj, auto)
    won = adj <= WIN_BAR
    return dict(fires=int(fire.sum()), n=len(df),
                fired_vetoed=int((fire & vet).sum()),
                fit_mean=fit.mean(),
                dep_mean=dep.mean(), dep_median=float(np.median(dep)),
                fw=int((fire & won).sum()), fl=int((fire & ~won).sum()),
                mw=int((~fire & won).sum()), ql=int((~fire & ~won).sum()))


def arm_table(df, label):
    print(f"\n=== arms @ {label} (n={len(df)}) ===")
    print(f"  {'arm':>7} {'fires':>9} {'fit_mean':>9} {'dep_mean':>9} {'dep_med':>8} "
          f"{'d_dep_vs.197':>12}  fire&won/fire&lost/miss&won  vetoed_fired")
    base = arm_eval(df, SHIPPED)
    recs = []
    for name, tau in ARMS:
        e = arm_eval(df, tau)
        d = e["dep_mean"] - base["dep_mean"]
        print(f"  {name:>7} {e['fires']:>4}/{e['n']:<4} {e['fit_mean']:>+9.3f} "
              f"{e['dep_mean']:>+9.3f} {e['dep_median']:>+8.3f} {d:>+12.3f}  "
              f"{e['fw']:>3}/{e['fl']:>3}/{e['mw']:>3}{'':>12}{e['fired_vetoed']:>3}")
        recs.append(dict(scope=label, arm=name, tau=tau, **e, d_dep_vs_shipped=d))
    return recs


def refit(df, label, objective):
    """Threshold scan (fire-always included as tau=-1) under the given
    objective; plateau + specificity tiebreak (within 0.1, fewest fires)."""
    adj, _ = veto_adj(df)
    pf = df["patch_fraction"].to_numpy()
    auto = df["bd_auto"].fillna(0.0).to_numpy()
    ok = np.isfinite(pf) & np.isfinite(adj)
    pf, adj, auto = pf[ok], adj[ok], auto[ok]
    u = np.unique(pf)
    taus = np.concatenate([[-1.0], (u[:-1] + u[1:]) / 2.0])
    cands = []
    for t in taus:
        fire = pf > t
        m = (np.where(fire, adj, 0.0) if objective == "fit"
             else np.where(fire, adj, auto)).mean()
        cands.append((m, int(fire.sum()), t))
    best = min(c[0] for c in cands)
    pool = sorted([c for c in cands if c[0] <= best + 0.1], key=lambda c: (c[1], c[0]))
    m, nf, t = pool[0]
    lo, hi = min(c[2] for c in pool), max(c[2] for c in pool)
    cur = next(c[0] for c in cands if abs(c[2] - SHIPPED) < 1e-9) if any(
        abs(c[2] - SHIPPED) < 1e-9 for c in cands) else None
    if cur is None:  # shipped tau not a scan midpoint: evaluate directly
        fire = pf > SHIPPED
        cur = (np.where(fire, adj, 0.0) if objective == "fit"
               else np.where(fire, adj, auto)).mean()
    print(f"  {label} [{objective:>6}]: refit tau={t:.4f} (mean {m:+.3f}, fires {nf}/{len(pf)}); "
          f"plateau [{lo:.4f}, {hi:.4f}]; shipped 0.197 mean {cur:+.3f} (delta {cur - best:+.3f})")
    return t, (lo, hi)


def flips(df, tau, label):
    """Cells the lower threshold flips (tau < pf <= 0.197): the incremental
    claim vs the shipped rule — deploy delta + butteraugli agreement."""
    adj, vet = veto_adj(df)
    d = df.assign(adj=adj, vet=vet)
    d = d[(d["patch_fraction"] > tau) & (d["patch_fraction"] <= SHIPPED)].copy()
    if d.empty:
        print(f"  {label}: no flipped cells")
        return d
    d["dep_delta"] = d["adj"] - d["bd_auto"].fillna(0.0)
    d = d.sort_values("dep_delta")
    print(f"\n  --- {label}: cells flipped by tau={tau} (n={len(d)}, "
          f"dep_delta mean {d['dep_delta'].mean():+.3f} median {d['dep_delta'].median():+.3f}, "
          f"vetoed {int(d['vet'].sum())}) ---")
    show = d[["cell", "config", "cgroup", "size_slot", "patch_fraction",
              "bd_auto", "bd_always", "adj", "dep_delta", "vet",
              "ba3n_always", "bamax_always"]]
    print(show.to_string(index=False, float_format=lambda x: f"{x:+.3f}"))
    return d


def main():
    cells = load_cells()
    recs = []
    for spd in (2, 6):
        s = cells[cells["speed"] == spd]
        tr, va = s[s["split"] == "train"], s[s["split"] == "val"]
        print(f"\n{'=' * 78}\nSPEED {spd}\n{'=' * 78}")
        recs += arm_table(tr, f"s{spd} TRAIN (fit)")
        recs += arm_table(va, f"s{spd} VAL (confirm)")
        for cfg in sorted(va["config"].unique()):
            recs += arm_table(va[va["config"] == cfg], f"s{spd} VAL {cfg}")
        print(f"\n=== s{spd} threshold refits (plateau; specificity tiebreak within 0.1) ===")
        for obj in ("fit", "deploy"):
            refit(tr, f"s{spd} train (n={len(tr)})", obj)
            refit(va, f"s{spd} val   (n={len(va)})", obj)
            refit(s, f"s{spd} pooled(n={len(s)})", obj)
        flips(va, 0.05, f"s{spd} VAL")
        flips(tr, 0.05, f"s{spd} TRAIN")

    hdr = [
        "speed-conditional palette-gate threshold A/B (follow-up to hyperparam_palette_mech_ab; 100% offline store/TSV selection, 0 fresh encodes)",
        "arms: tau {0.197 shipped, 0.10, 0.05} + fire-always, per speed {2,6}; cells = mech-ab TSV (iso+shipped configs) + palette-ab-final2 train26@1024",
        "fit view = mean(fire?veto_adj_always:0) [fit_palette_gate.py continuity]; deploy view = mean(fire?veto_adj_always:bd_auto) [three_way bd_rule] — DEPLOY decides",
        "butteraugli veto per cell (ba3n>+1.0 or bamax>+1.5): fired vetoed wins never banked (max(bd,0))",
        "won bar: veto-adj bd_always <= -0.5; d_dep_vs_shipped = arm dep_mean - shipped-0.197 dep_mean (negative = arm better)",
    ]
    out = pd.DataFrame(recs)
    to_tsv(out.set_index(["scope", "arm"]), os.path.abspath(OUT_TSV), hdr)


if __name__ == "__main__":
    main()

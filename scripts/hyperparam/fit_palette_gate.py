#!/usr/bin/env python3
"""Threshold-rule first cut: zenanalyze palette gate (FEATURE_HINTS §E head, wedge #6).

The encoder's ported AA-aware palette detection stops firing on ANY downscaled
screen content (wedge: auto==off byte ratio ~1.000 at <=512, and already at
1024 for 8196), while forced palette (Always) wins large on the same content.
This fits a features -> "arm PaletteMode::Always" gate on zenanalyze features
computed at the ENCODE rendition — features which (measured here) keep their
screen-vs-photo separation at every size, i.e. they see "this WAS screen
content" through the resampling.

Labels:
  * palette-ab-final2 train26 (24 TRAIN-LSD origins @1024-rendition, s2+s6,
    q{60..220}): per-(origin,speed) direct BD of always-vs-off and auto-vs-off
    (bd_arm.py conventions). THE ground truth for "palette actually won".
  * wedge-2026-07-03 zr(palette-auto) vs zr-paletteoff across sizes
    {256,512,1024,2048|native} on the 59-file fired-class subset: where the
    ported detection fires by size (byte-ratio signal) + auto-vs-off BD.

Deployment semantics: gate fires -> PaletteMode::Always; else keep Auto.
Realized per-cell outcome: fired -> bd_always, else bd_auto. Objective:
minimize mean realized BD (vs palette-off baseline) over the train26 label
cells, both speeds pooled.

Honesty: every labeled origin is TRAIN-split; NO val RD labels exist. LOOCV
(threshold search re-run per left-out origin) is the generalization stand-in;
val-split evidence is limited to FIRING-RATE sanity on the val origins'
precomputed feature rows (no encodes). Forced-palette wins at <=512 are NOT
yet measured anywhere (auto==off there, always arm absent): small-size
recovery numbers are bounded-by-1024-measurements, and the data need is a
palette{off,always} A/B at {256,512} on the wedge fired-class subset.
"""

import itertools
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import (FEATURES_PARQUET, arm_bd_per_image, load_features,  # noqa: E402
                       load_store, split_of, to_tsv, veto_table)

OUT_TSV = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "../../benchmarks/hyperparam_palette_gate_2026-07-03.tsv")

# Pre-registered gate-feature shortlist (wedge #6 correlates + the palette pack;
# palette_density/distinct_color_bins run INVERTED: photos score HIGHER).
SHORTLIST = ["luma_histogram_entropy", "patch_fraction", "flat_color_block_ratio",
             "distinct_color_bins", "palette_density", "grayscale_score"]
WIN_BAR = -0.5  # bd_always <= this counts as "palette actually won"


def train26_labels(store, feats):
    pal = store[store["sweep_source"] == "palette-ab-final2-2026-07-03"]
    meta = (pal[["image_id", "origin_id", "content_class", "feature_join"]]
            .drop_duplicates().set_index("image_id"))
    rows = []
    for spd in (2, 6):
        alw = veto_table(store, "palette-ab-final2-2026-07-03",
                         f"palette/off_s{spd}", f"palette/always_s{spd}")
        auto = arm_bd_per_image(store, "palette-ab-final2-2026-07-03",
                                f"palette/off_s{spd}", f"palette/auto_s{spd}")
        for img in alw.index:
            fj = meta.loc[img, "feature_join"]
            r = dict(cell=f"{meta.loc[img, 'origin_id']}_s{spd}",
                     origin_id=meta.loc[img, "origin_id"],
                     content_class=meta.loc[img, "content_class"], speed=spd,
                     bd_always=alw.loc[img, "bd_ssim2"],
                     ba3n_always=alw.loc[img, "bd_ba3n"],
                     bamax_always=alw.loc[img, "bd_bamax"],
                     bd_auto=float(auto.loc[img, "bd"]) if img in auto.index else np.nan)
            for f in SHORTLIST:
                r[f] = feats.loc[fj, f]
            rows.append(r)
    return pd.DataFrame(rows).set_index("cell")


def wedge_labels(store, feats):
    """auto-vs-off BD + q60 byte ratio per wedge file (paletteoff subset)."""
    w = store[(store["sweep_source"] == "wedge-2026-07-03")
              & (store["arm_id"].isin(["wedge/zr-best_s2", "wedge/zr-paletteoff_s2"]))]
    files = sorted(w.loc[w["arm_id"] == "wedge/zr-paletteoff_s2", "image_id"].unique())
    bd = arm_bd_per_image(store, "wedge-2026-07-03", "wedge/zr-paletteoff_s2",
                          "wedge/zr-best_s2")
    rows = []
    for fn in files:
        g = w[w["image_id"] == fn]
        off = g[g["arm_id"] == "wedge/zr-paletteoff_s2"].set_index("q")["bytes"]
        auto = g[g["arm_id"] == "wedge/zr-best_s2"].set_index("q")["bytes"]
        common = off.index.intersection(auto.index)
        ratio = float((auto[common] / off[common]).mean())
        m = g.iloc[0]
        fj = m["feature_join"]
        r = dict(file=fn, origin_id=m["origin_id"], content_class=m["content_class"],
                 crop_label=m["crop_label"], size_class=m["size_class"],
                 bd_auto_vs_off=float(bd.loc[fn, "bd"]) if fn in bd.index else np.nan,
                 bytes_ratio_auto=ratio,
                 detection_fired=bool(abs(ratio - 1.0) > 0.02))
        for f in SHORTLIST:
            r[f] = feats.loc[fj, f] if fj else np.nan
        rows.append(r)
    return pd.DataFrame(rows).set_index("file")


def gate_space(T):
    """Candidate gates: 1 feature or AND of 2 features, direction per feature
    chosen so 'screen-like' is the firing side (entropy/palette_density/
    distinct_color_bins low; patch/flat/grayscale high)."""
    LOWSIDE = {"luma_histogram_entropy", "palette_density", "distinct_color_bins"}

    def preds(f, t):
        v = T[f].to_numpy()
        return (v < t) if f in LOWSIDE else (v > t)

    def taus(f):
        u = np.unique(T[f].to_numpy())
        return (u[:-1] + u[1:]) / 2.0

    for f in SHORTLIST:
        for t in taus(f):
            yield (f"{f} {'<' if f in LOWSIDE else '>'} {t:.4g}",
                   preds(f, t), ((f, t),))
    for fa, fb in itertools.combinations(SHORTLIST, 2):
        for ta in taus(fa):
            pa = preds(fa, ta)
            for tb in taus(fb):
                yield (f"{fa} {'<' if fa in LOWSIDE else '>'} {ta:.4g} AND "
                       f"{fb} {'<' if fb in LOWSIDE else '>'} {tb:.4g}",
                       pa & preds(fb, tb), ((fa, ta), (fb, tb)))


def always_eff(T):
    """Veto-adjusted always-arm BD: firing on a cell whose always-arm is
    per-cell butteraugli-vetoed (ba3n > +1.0 or bamax > +1.5 vs off) does NOT
    bank the ssim2 win — palette banding gaming ssim2 is exactly what the veto
    protocol exists for — but a genuine loss stays a loss: max(bd_always, 0)."""
    vet = (T["ba3n_always"].to_numpy() > 1.0) | (T["bamax_always"].to_numpy() > 1.5)
    return np.where(vet, np.maximum(T["bd_always"].to_numpy(), 0.0),
                    T["bd_always"].to_numpy())


def realized_bd(T, fire, assume_auto=False):
    """Realized BD of a gate policy.

    assume_auto=False (the FIT objective): non-fired cells get 0 — the
    deployment regime this gate exists for is downscaled screen content where
    the ported AA-detection is DEAD, so Auto contributes nothing and the gate
    is the only chance. Fitted at 1024 (where labels exist), transferred via
    the measured feature-stability across sizes.

    assume_auto=True (reporting): non-fired cells keep the Auto outcome — the
    literal @1024 deployment where detection still partially works."""
    off = T["bd_auto"].fillna(0.0).to_numpy() if assume_auto else np.zeros(len(T))
    return np.where(fire, always_eff(T), off)


def fit_gate(T):
    """Minimize mean realized BD (no-auto objective); among gates within 0.1
    of the best, prefer the FEWEST fires (specificity — every fire costs
    palette-search encode time: measured s2 med 1.07x, s6 med 1.80x), then
    fewer features."""
    cands = []
    for desc, fire, spec in gate_space(T):
        cands.append((realized_bd(T, fire).mean(), int(fire.sum()), len(spec), desc, spec))
    best_m = min(c[0] for c in cands)
    pool = [c for c in cands if c[0] <= best_m + 0.1]
    pool.sort(key=lambda c: (c[1], c[2], c[0]))
    m, _, _, desc, spec = pool[0]
    return m, desc, spec


def apply_spec(spec, df):
    LOWSIDE = {"luma_histogram_entropy", "palette_density", "distinct_color_bins"}
    fire = np.ones(len(df), dtype=bool)
    for f, t in spec:
        v = df[f].to_numpy()
        fire &= (v < t) if f in LOWSIDE else (v > t)
    return fire


def main():
    store = load_store()
    feats = load_features(SHORTLIST)

    T = train26_labels(store, feats)
    W = wedge_labels(store, feats)

    print("=== label structure: train26 palette-ab (always-vs-off = 'palette actually won') ===")
    for spd in (2, 6):
        s = T[T["speed"] == spd]
        won = s["bd_always"] <= WIN_BAR
        missed = s.loc[won, "bd_always"] - s.loc[won, "bd_auto"].fillna(0.0)
        print(f"  s{spd}: palette wins (bd_always <= {WIN_BAR}) on {won.sum()}/{len(s)}; "
              f"auto already captured {sum(s.loc[won, 'bd_auto'].fillna(0) <= WIN_BAR)}; "
              f"missed-BD available (auto-always on winners): mean {missed.mean():+.2f} "
              f"median {missed.median():+.2f} max {missed.max():+.2f}")
    vet = T[(T["bd_always"] <= WIN_BAR)
            & ((T["ba3n_always"] > 1.0) | (T["bamax_always"] > 1.5))]
    print(f"  butteraugli veto on winners: {len(vet)} cells "
          f"({', '.join(vet.index)})" if len(vet) else
          "  butteraugli veto on winners: 0 cells (palette wins are butteraugli-clean)")

    # ---- fit + LOOCV ----
    m_fit, desc, spec = fit_gate(T)
    ones = np.ones(len(T), bool)
    print(f"\n=== gate fit (objective: mean veto-adjusted BD of FIRED cells, no-auto-rescue regime; n={len(T)}) ===")
    print(f"  fire-nothing              : +0.000")
    print(f"  fire-everything (veto-adj): {realized_bd(T, ones).mean():+.3f}")
    print(f"  fitted gate               : {m_fit:+.3f}   RULE: fire iff {desc}")
    fire0 = apply_spec(spec, T)
    print(f"  (@1024-with-auto view: Auto {T['bd_auto'].fillna(0.0).mean():+.3f} | "
          f"Always {realized_bd(T, ones, True).mean():+.3f} | "
          f"gate {realized_bd(T, fire0, True).mean():+.3f})")

    loo_pred = pd.Series(False, index=T.index)
    for org in T["origin_id"].unique():
        tr = T[T["origin_id"] != org]
        _, _, sp = fit_gate(tr)
        te = T[T["origin_id"] == org]
        loo_pred.loc[te.index] = apply_spec(sp, te)
    m_loo = realized_bd(T, loo_pred.to_numpy()).mean()
    print(f"  LOOCV (leave-one-ORIGIN-out): {m_loo:+.3f}")

    fire = apply_spec(spec, T)
    won = (T["bd_always"] <= WIN_BAR).to_numpy()
    print("\n=== confusion vs 'palette actually won' (train26 cells; NO val labels exist) ===")
    print(f"  fire&won {int((fire & won).sum()):>3}  fire&lost {int((fire & ~won).sum()):>3}")
    print(f"  miss&won {int((~fire & won).sum()):>3}  quiet&lost {int((~fire & ~won).sum()):>3}")
    fp = T[fire & ~won]
    if len(fp):
        print("  false fires:", ", ".join(f"{i}({T.loc[i, 'bd_always']:+.2f})" for i in fp.index))
    fn = T[~fire & won]
    if len(fn):
        print("  missed wins:", ", ".join(f"{i}({T.loc[i, 'bd_always']:+.2f})" for i in fn.index))

    # ---- size transfer on the wedge subset ----
    W["gate_fires"] = apply_spec(spec, W)
    print("\n=== size transfer: ported detection vs gate on the wedge paletteoff subset ===")
    tab = (W.groupby("size_class")
           .agg(n=("gate_fires", "size"), detection_fired=("detection_fired", "sum"),
                gate_fires=("gate_fires", "sum"))
           .reindex(["256", "512", "1024", "2048", "native"]).dropna())
    print(tab.to_string())
    nat = W[W["size_class"].isin(["native", "2048"]) & W["detection_fired"]]
    agree = (nat["gate_fires"] == True).sum()  # noqa: E712
    print(f"  native/2048 cells where detection fired: {len(nat)}, gate agrees on {agree}")
    small_gate = W[W["size_class"].isin(["256", "512"]) & W["gate_fires"]]
    small_det = W[W["size_class"].isin(["256", "512"]) & W["detection_fired"]]
    print(f"  <=512 cells: gate fires on {len(small_gate)}, ported detection on {len(small_det)} "
          f"(the downscale blindness the gate exists to fix; forced-palette RD at <=512 UNMEASURED)")

    # ---- encode-time cost of firing (palette-ab enc_ms, same host, single-threaded) ----
    pal = store[store["sweep_source"] == "palette-ab-final2-2026-07-03"]
    ms = pal.pivot_table(index=["image_id", "speed", "q"], columns="arm_id", values="enc_ms")
    ratios = {}
    for spd in (2, 6):
        s = ms.xs(spd, level="speed")
        r = (s[f"palette/always_s{spd}"] / s[f"palette/off_s{spd}"]).dropna()
        ratios[spd] = (r.median(), r.max())
    print("\n=== encode-time cost of forcing palette (always/off enc_ms, within-source) ===")
    for spd, (med, mx) in ratios.items():
        print(f"  s{spd}: median {med:.2f}x  max {mx:.2f}x")

    # ---- val-split firing-rate sanity (features only; no RD labels) ----
    print("\n=== val-origin firing rates (feature rows only — no encodes, no RD labels) ===")
    allf = load_features(SHORTLIST).reset_index()
    allf["base"] = allf["image_path"].str.rsplit("/", n=1).str[-1]
    allf["split"] = allf["base"].map(split_of)
    va = allf[(allf["split"] == "val") & (allf["crop_label"] == "full")
              & allf["size_class"].isin(["256", "512", "1024", "native"])].copy()
    va["gate_fires"] = apply_spec(spec, va)
    va["cgroup"] = va["content_class"].str.extract(r"^(\d+)")
    rates = va.groupby("cgroup")["gate_fires"].agg(["mean", "size"])
    print(rates.to_string(float_format=lambda x: f"{x:.2f}"))

    hdr = [
        "hyperparam-expert first cut: zenanalyze palette gate (FEATURE_HINTS section E; wedge #6)",
        "labels: palette-ab-final2 train26 @1024-rendition s2+s6 (always/auto/off, rav1e CLI isolated config) + wedge zr vs zr-paletteoff across sizes",
        f"RULE (fit on all {len(T)} cells): fire PaletteMode::Always iff {desc}",
        f"no-auto-rescue objective (veto-adjusted): fire-everything {realized_bd(T, np.ones(len(T), bool)).mean():+.3f} / gate {m_fit:+.3f} / gate-LOOCV {m_loo:+.3f}; @1024-with-auto view: Auto {T['bd_auto'].fillna(0.0).mean():+.3f} / gate {realized_bd(T, fire, True).mean():+.3f}",
        "ALL labeled origins are LSD-train; val evidence = firing-rate sanity only; forced-palette at <=512 unmeasured (data need)",
        "columns: per-cell train26 labels (bd_always/bd_auto/butteraugli of always) + gate features + fire/won flags",
    ]
    out = T.copy()
    out["gate_fires"] = fire
    out["won"] = won
    out["loocv_fires"] = loo_pred
    to_tsv(out, os.path.abspath(OUT_TSV), hdr)

    wt = os.path.abspath(OUT_TSV).replace(".tsv", "_wedge.tsv")
    to_tsv(W, wt, ["wedge paletteoff-subset size-transfer table for the palette gate",
                   f"gate: {desc}",
                   "detection_fired = |mean auto/off byte ratio - 1| > 0.02 (flag-bit noise floor)"])


if __name__ == "__main__":
    main()

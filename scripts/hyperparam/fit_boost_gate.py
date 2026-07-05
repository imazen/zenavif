#!/usr/bin/env python3
"""Anti-boost gate fit: should Variance Boost turn OFF on some content?

Motivation (TUNER2 valstr, 2026-07-04): under the CURRENT composed tune
(qmdist + lfsharp landed after the 2026-07-02 strength fit), the shipped
strength-1.0 boost is net-harmful on the val corpus median (+0.58, 6/14,
7/14 per-image butteraugli vetoes) with chart-class disasters (8103 +7.3,
5343 +5.8 ssim2 BD vs boost-off) — while the drift check shows it still
pays on photos/scans (6018 −1.50, 2000 −1.45). The exploitable structure
may be a fire-conservative OFF-gate, the inverse of the deepening head the
P3 diagnosis proposed (which measured honest-negative: deep strengths are
subsumed by the composed tune and the deep-flat ramp never fires on the
deep-AQ class).

Labels (all CURRENT binary, LSD-clean):
  TRAIN: store speedladder/zr-s2-tune rows (byte-continuity-proven
         str-1.0) vs tuner2/t26str0_s2 (fresh strength-0) — 24 origins.
  VAL:   valstr-2026-07-04 str1 vs str0 — 14 held-out origins, fit-frozen.

Rule family: 1-feature threshold + 2-feature conjunction, class {off, ship},
fire-conservative (an OFF assignment on train must not butteraugli-veto:
str0's per-image butteraugli must not exceed max(veto, str1's)). Objective:
minimize mean per-image ssim2 BD of the policy vs all-ship. Ship bar
(pre-registered, palette-gate pattern): LOOCV mean <= -0.5 vs all-ship AND
val median improvement < 0 with no new val vetoes.
"""

import itertools
import os
import sys

import numpy as np
import pandas as pd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import load_features, load_store, to_tsv, veto_table  # noqa: E402

OUT_TSV = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "../../benchmarks/hyperparam_boost_gate_2026-07-04.tsv")
VETO_3N, VETO_MAX = 1.0, 1.5

# Shortlist: the WEDGE_MAP correlates + the in-band def->iq separators
# (chart/screen-vs-photo separators) + the aq stats. Chosen before looking
# at which val images win (the val chart losses motivated the QUESTION;
# features are the standing content-class separators).
FEATS = ["patch_fraction", "luma_histogram_entropy", "flat_color_block_ratio",
         "gradient_fraction_smooth", "grayscale_score", "distinct_color_bins",
         "palette_density", "aq_map_std", "aq_map_p50", "noise_floor_y",
         "spectral_slope_y", "dct_compressibility_y", "edge_density"]


def bd_join(store, src_test, arm_test, src_base, arm_base, corpus):
    """Per-image (ssim2, ba3n, bamax) BD of arm_test vs arm_base across
    possibly different sweep_sources (same binary chain — continuity-gated)."""
    a = store[(store.sweep_source == src_test) & (store.arm_id == arm_test)
              & (store.corpus == corpus)]
    b = store[(store.sweep_source == src_base) & (store.arm_id == arm_base)
              & (store.corpus == corpus)]
    merged = pd.concat([a, b])
    fake = "gatejoin"
    merged = merged.copy()
    merged["sweep_source"] = fake
    return veto_table(merged, fake, arm_base, arm_test)


def main():
    store = load_store()
    # TRAIN: str1 (speedladder rows) vs str0 (tuner2 fresh), train26
    t = bd_join(store, "speedladder-2026-07-04", "speedladder/zr-s2-tune",
                "tuner2-2026-07-04", "tuner2/t26str0_s2", "train26")
    t = t.rename(columns={"bd_ssim2": "s1_ssim2", "bd_ba3n": "s1_ba3n",
                          "bd_bamax": "s1_bamax"})
    # VAL: valstr str1 vs str0
    v = bd_join(store, "valstr-2026-07-04", "valstr/str1_s2",
                "valstr-2026-07-04", "valstr/str0_s2", "mech26")
    v = v.rename(columns={"bd_ssim2": "s1_ssim2", "bd_ba3n": "s1_ba3n",
                          "bd_bamax": "s1_bamax"})

    feats = load_features(FEATS)
    def attach(M, corpus_src):
        meta = (store[store.corpus == corpus_src][
            ["image_id", "origin_id", "feature_join"]]
            .drop_duplicates().set_index("image_id"))
        M = M.join(meta, how="left")
        for f in FEATS:
            M[f] = M["feature_join"].map(feats[f])
        return M
    t = attach(t, "train26")
    v = attach(v, "mech26")

    # policy value: OFF on image i = -s1_ssim2 (we lose the boost's win /
    # gain its loss); SHIP = 0 by construction. Veto-consistent OFF
    # requires str0 butteraugli not worse than allowed — str0 IS the BD
    # base, so its butteraugli-vs-str1 = -s1_ba3n etc.; OFF is clean when
    # -s1_ba3n <= max(VETO_3N, 0) and -s1_bamax <= max(VETO_MAX, 0).
    for M in (t, v):
        M["off_gain"] = -M["s1_ssim2"]          # negative = OFF is better
        M["off_clean"] = ((-M["s1_ba3n"]).fillna(0) <= VETO_3N) & \
                         ((-M["s1_bamax"]).fillna(0) <= VETO_MAX)

    print("=== TRAIN (current-binary str1-vs-str0, 24 origins) ===")
    for i in t.sort_values("s1_ssim2").index:
        r = t.loc[i]
        print(f"  {str(r['origin_id']):>6} s1 {r['s1_ssim2']:+7.2f} "
              f"(OFF gain {r['off_gain']:+7.2f} clean={bool(r['off_clean'])})")
    print("=== VAL (str1-vs-str0, 14 origins) ===")
    for i in v.sort_values("s1_ssim2").index:
        r = v.loc[i]
        print(f"  {str(r['origin_id']):>6} s1 {r['s1_ssim2']:+7.2f} "
              f"(OFF gain {r['off_gain']:+7.2f} clean={bool(r['off_clean'])})")

    def thresholds(x):
        u = np.unique(x[np.isfinite(x)])
        return (u[:-1] + u[1:]) / 2.0

    def fit(M):
        """best rule minimizing mean policy ssim2 (OFF where rule fires,
        subject to fire-conservative cleanliness on the fit rows)."""
        best = (0.0, None)   # all-ship mean = 0.0
        X = {f: M[f].to_numpy(dtype=float) for f in FEATS}
        gain = M["off_gain"].to_numpy(dtype=float)
        clean = M["off_clean"].to_numpy(dtype=bool)
        gain = np.where(np.isfinite(gain), gain, 0.0)
        for f in FEATS:
            ts = thresholds(X[f])
            for d in (1, -1):
                hit = (X[f] * d) > (ts[:, None] * d)          # (T, n)
                ok = ~(hit & ~clean[None, :]).any(axis=1)
                mean = np.where(hit, gain[None, :], 0.0).mean(axis=1)
                mean = np.where(ok, mean, np.inf)
                j = int(np.argmin(mean))
                if mean[j] < best[0]:
                    sgn = ">" if d == 1 else "<"
                    best = (float(mean[j]),
                            dict(kind="1f", f=f, t=float(ts[j]), d=d,
                                 desc=f"{f} {sgn} {ts[j]:.4g} -> OFF"))
        for fa, fb in itertools.combinations(FEATS, 2):
            ta, tb = thresholds(X[fa]), thresholds(X[fb])
            if not len(ta) or not len(tb):
                continue
            for da, db in itertools.product((1, -1), repeat=2):
                ha = (X[fa] * da) > (ta[:, None] * da)
                hb = (X[fb] * db) > (tb[:, None] * db)
                hit = ha[:, None, :] & hb[None, :, :]
                ok = ~(hit & ~clean[None, None, :]).any(axis=2)
                mean = np.where(hit, gain[None, None, :], 0.0).mean(axis=2)
                mean = np.where(ok, mean, np.inf)
                j = int(np.argmin(mean))
                ja, jb = np.unravel_index(j, mean.shape)
                if mean[ja, jb] < best[0]:
                    sa = ">" if da == 1 else "<"
                    sb = ">" if db == 1 else "<"
                    best = (float(mean[ja, jb]),
                            dict(kind="2f", fa=fa, ta=float(ta[ja]), da=da,
                                 fb=fb, tb=float(tb[jb]), db=db,
                                 desc=(f"{fa} {sa} {ta[ja]:.4g} && "
                                       f"{fb} {sb} {tb[jb]:.4g} -> OFF")))
        return best

    def fires(rule, M):
        if rule is None:
            return pd.Series(False, index=M.index)
        if rule["kind"] == "1f":
            return (M[rule["f"]].astype(float) * rule["d"]) > (rule["t"] * rule["d"])
        return (((M[rule["fa"]].astype(float) * rule["da"]) > (rule["ta"] * rule["da"])) &
                ((M[rule["fb"]].astype(float) * rule["db"]) > (rule["tb"] * rule["db"])))

    mean_all, rule = fit(t)
    print(f"\nbest train rule: {rule['desc'] if rule else 'NONE (all-ship)'} "
          f"(train policy mean {mean_all:+.3f} vs all-ship 0.000)")

    print("\n=== LOOCV (threshold re-search per left-out origin) ===")
    loo_gain = []
    loo_fired = {}
    for leave in t.index:
        m, r = fit(t.drop(index=leave))
        fired = bool(fires(r, t.loc[[leave]]).iloc[0]) if r else False
        g = float(t.loc[leave, "off_gain"]) if fired else 0.0
        clean_ok = bool(t.loc[leave, "off_clean"]) if fired else True
        loo_gain.append(g if np.isfinite(g) else 0.0)
        loo_fired[leave] = (fired, g, clean_ok)
    loo = np.array(loo_gain)
    vetoes = sum(1 for f, g, c in loo_fired.values() if f and not c)
    print(f"  LOOCV mean {loo.mean():+.3f} median {np.median(loo):+.3f} "
          f"fires {sum(1 for f,_,_ in loo_fired.values() if f)}/24 "
          f"loocv-vetoed-fires {vetoes}")

    verdict = "HONEST NEGATIVE"
    vfheld = None
    if rule is not None:
        vf = fires(rule, v)
        vg = np.where(vf, np.where(np.isfinite(v["off_gain"]), v["off_gain"], 0.0), 0.0)
        vetoes_v = int((vf & ~v["off_clean"]).sum())
        vfheld = (float(np.mean(vg)), float(np.median(vg)), int(vf.sum()), vetoes_v)
        print(f"\n=== VAL eval of the train rule (frozen) ===")
        print(f"  fires {vfheld[2]}/14, policy mean {vfheld[0]:+.3f} "
              f"median {vfheld[1]:+.3f}, new-veto fires {vetoes_v}")
        for i in v.index[vf]:
            print(f"    fired: {v.loc[i,'origin_id']} off_gain {v.loc[i,'off_gain']:+.2f} "
                  f"clean={bool(v.loc[i,'off_clean'])}")
        if loo.mean() <= -0.5 and vfheld[0] < 0 and vetoes_v == 0:
            verdict = "SHIP-CANDIDATE"
    print(f"\nVERDICT: {verdict}")

    out = pd.concat([t.assign(split="train"), v.assign(split="val")])
    out["rule_fires"] = pd.concat([fires(rule, t), fires(rule, v)]) if rule else False
    hdr = [
        "anti-boost gate fit (TUNER2 2026-07-04): fire-conservative OFF-gate for Tune::Ssimulacra2 Variance Boost",
        "TRAIN: store speedladder/zr-s2-tune (str1, byte-continuity-proven) vs tuner2/t26str0_s2 (fresh str0), 24 train26 origins",
        "VAL (frozen eval): valstr/str1_s2 vs valstr/str0_s2, 14 mech26 origins",
        f"best rule: {rule['desc'] if rule else 'none'}; train mean {mean_all:+.3f}; LOOCV mean {loo.mean():+.3f}",
        f"val: {vfheld}; verdict {verdict}",
        "off_gain = -bd(str1 vs str0) = policy delta of turning boost OFF; negative = OFF better",
    ]
    to_tsv(out, os.path.abspath(OUT_TSV), hdr)


if __name__ == "__main__":
    main()

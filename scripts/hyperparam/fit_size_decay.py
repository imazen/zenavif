#!/usr/bin/env python3
"""Size-conditional tune strength: decay attribution + proposed curve
(FEATURE_HINTS §E head, wedge #3).

The wedge measured that zenrav1e's advantage vs libaom cpu2 decays
monotonically below 1024 (full-crop medians −13.0 @1024 -> −6.5 @512 ->
−1.15 @256) while the fixed overhead α is in our favor — i.e. it is per-pixel
RD behavior, and every Tune::Ssimulacra2 constant (chroma delta-q clamp, ss2 QM
curves, delta-q octile boost, qm-dist ratio, LF sharpness schedule) was fit at
1024 only.

WHAT EXISTING DATA CAN AND CANNOT ATTRIBUTE (stated up front): no per-mechanism
arms exist below 1024 — the mechanism sweeps (tune stages / deltaq strengths /
qmdist / lfsharp) all ran at the 1024 rendition scale. This script therefore
produces a MEASURED NARROWING, not a proof:
  1. per-origin decay slopes d(BD)/d(log2 px) + q-band decomposition (does the
     small-size loss live at the low-q or high-q end?),
  2. content/feature correlates of the decay,
  3. each mechanism's 1024 per-family win fingerprint (from the label store)
     cross-referenced against which families decay,
and specifies the exact per-size mechanism A/B (cheap, cell-cached) that turns
the narrowing into an attribution. The proposed log-px curve is a MEASURED
PROPOSAL for that A/B — NOT landed in zenrav1e.
"""

import os
import sys

import numpy as np
import pandas as pd
from scipy.stats import spearmanr

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from hp_common import load_features, load_store, to_tsv  # noqa: E402
from hp_common import _quality_pts, frontier  # noqa: E402

OUT_TSV = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "../../benchmarks/hyperparam_size_decay_2026-07-03.tsv")

FEATS = ["luma_histogram_entropy", "palette_density", "patch_fraction",
         "flat_color_block_ratio", "grayscale_score", "edge_density",
         "gradient_fraction_smooth", "noise_floor_y", "aq_map_std",
         "high_freq_energy_ratio", "spectral_slope_y"]


def band_gaps(zf, rf):
    """bpp gap % at the 25%/75% points of the overlapping ssim2 window."""
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


def main():
    store = load_store(sweep_source="wedge-2026-07-03")
    w = store[(store["crop_label"] == "full")]

    # ---- per-(origin,size): BD + q-band gaps ----
    rows = []
    for (oid, sc), g in w.groupby(["origin_id", "size_class"]):
        zr = g[g["arm_id"] == "wedge/zr-best_s2"]
        cp = g[g["arm_id"] == "wedge/aom-cpu2"]
        if zr.empty or cp.empty:
            continue
        zf = frontier(_quality_pts(zr, "ssim2"))
        rf = frontier(_quality_pts(cp, "ssim2"))  # pools 420/444 rows (run_gap convention)
        glo, ghi = band_gaps(zf, rf)
        rows.append(dict(origin_id=oid, size_class=sc,
                         px=int(zr["px"].iloc[0]),
                         family=zr["family"].iloc[0],
                         bd_cpu2=zr["file_bd_cpu2"].iloc[0],
                         gap_lowq_pct=glo, gap_highq_pct=ghi,
                         feature_join=zr["feature_join"].iloc[0]))
    D = pd.DataFrame(rows)

    print("=== BD vs cpu2 by size (full crops; medians; recomputes the wedge table from the store) ===")
    D["slot"] = np.where(D["size_class"].isin(["2048", "native"]), "top", D["size_class"])
    tab = D.groupby("slot").agg(n=("bd_cpu2", "size"), bd_med=("bd_cpu2", "median"),
                                gap_lowq_med=("gap_lowq_pct", "median"),
                                gap_highq_med=("gap_highq_pct", "median"))
    tab = tab.reindex(["256", "512", "1024", "top"])
    print(tab.to_string(float_format=lambda x: f"{x:+.2f}"))
    print("  (gap = bpp excess vs cpu2 at the 25%/75% points of the overlapping ssim2 window;")
    print("   lowq = lower-quality end of the window, highq = upper end)")

    print("\n=== same, WITHOUT fam-7000 plots (the palette/intraBC wedge #1 confound) ===")
    nop = D[D["family"] != "7000"]
    t2 = nop.groupby("slot").agg(n=("bd_cpu2", "size"), bd_med=("bd_cpu2", "median"),
                                 gap_lowq_med=("gap_lowq_pct", "median"),
                                 gap_highq_med=("gap_highq_pct", "median")).reindex(["256", "512", "1024", "top"])
    print(t2.to_string(float_format=lambda x: f"{x:+.2f}"))
    print("""  READING: 1024->512 the low-q band HOLDS (-15.3 vs -15.7) while the HIGH-q band
  flips to parity (-5.1 -> +0.55) — the first decay step is ENTIRELY a
  high-quality-end (low qindex) phenomenon. 512->256 then collapses BOTH bands.""")

    # ---- per-origin decay slope: BD vs log2(px), needs >=3 sizes ----
    slopes = []
    for oid, g in D.dropna(subset=["bd_cpu2"]).groupby("origin_id"):
        if g["size_class"].nunique() < 3 or "256" not in set(g["size_class"]):
            continue
        x = np.log2(g["px"].to_numpy().astype(float))
        y = g["bd_cpu2"].to_numpy()
        b, a = np.polyfit(x, y, 1)
        slopes.append(dict(origin_id=oid, family=g["family"].iloc[0],
                           decay_slope=-b,  # positive = loses advantage as size shrinks
                           bd_256=g.loc[g["size_class"] == "256", "bd_cpu2"].iloc[0],
                           bd_big=g.loc[g["size_class"].isin(["1024", "2048", "native"]),
                                        "bd_cpu2"].min(),
                           n_sizes=g["size_class"].nunique()))
    S = pd.DataFrame(slopes).set_index("origin_id").sort_values("decay_slope", ascending=False)
    print("\n=== per-origin decay slope (-d(BD)/d(log2 px); positive = advantage decays toward small) ===")
    print(S.to_string(float_format=lambda x: f"{x:+.2f}"))

    # ---- feature correlates of the decay slope (origin-level, native full-crop features) ----
    feats = load_features(FEATS)
    nat = D[D["size_class"].isin(["native", "2048"])].set_index("origin_id")
    fj = nat.loc[nat.index.intersection(S.index), "feature_join"]
    FX = pd.DataFrame({f: fj.map(feats[f]) for f in FEATS})
    print("\n=== spearman(feature @ native, decay_slope) — which content decays ===")
    cors = []
    for f in FEATS:
        rho, _ = spearmanr(FX[f], S.loc[FX.index, "decay_slope"])
        cors.append((f, rho))
    for f, rho in sorted(cors, key=lambda t: -abs(t[1])):
        print(f"  {f:<28} {rho:+.3f}")

    # ---- mechanism 1024 win fingerprints per family (from the label store) ----
    print("\n=== mechanism per-family ssim2-BD medians at 1024 (the fingerprints to cross-reference) ===")
    print("  [committed summaries recomputed from the store: deltaq/qmdist/lfsharp on train26;")
    print("   tune stages exist only on legacy22 (single-digit families) — approximate class mapping]")
    from hp_common import arm_bd_per_image
    full = load_store()
    mech = {
        "chroma-dq (tune stage1, legacy)": ("tune-ss2-2026-07-02", "ss2/tune-off_s2", "ss2/stage1-chromadq_s2"),
        "ss2-QM (tune stage3 vs 12, legacy)": ("tune-ss2-2026-07-02", "ss2/stage12-lambda_s2", "ss2/stage123-qm_s2"),
        "deltaq boost 1.0 (train26)": ("deltaq-2026-07-02", "deltaq/str0_s2", "deltaq/str1_s2"),
        "qmdist ratio (train26)": ("qmdist-2026-07-03", "qmdist/off_s2", "qmdist/tx3-ratio_s2"),
        "lfsharp still (train26)": ("lfsharp-2026-07-03", "lfsharp/sharp0_s2", "lfsharp/still753_s2"),
    }
    fam_rows = {}
    for name, (src, b, a) in mech.items():
        bd = arm_bd_per_image(full, src, b, a)
        meta = full[full["sweep_source"] == src][["image_id", "family"]].drop_duplicates().set_index("image_id")
        j = bd.join(meta)
        fam_rows[name] = j.groupby("family")["bd"].median()
    FT = pd.DataFrame(fam_rows)
    print(FT.to_string(float_format=lambda x: f"{x:+.2f}"))

    # ---- the measured proposal ----
    med = tab["bd_med"]
    print("""
=== RANKED SUSPECTS (measured narrowing — the A/B convicts) ===
  1. ss2 QM level curves — the dominant 1024 mechanism on exactly the families
     that decay (photo classes: legacy fams 1/3/5/6 medians -9.9..-12.6 vs
     chroma-dq's -0.4..-8.6), and QM's action concentrates where quantization
     is fine — the high-quality band that collapses first at 512.
  2. variance-boost/activity scaling — 8x8 activity stats on a downscaled
     image see a compressed detail spectrum; boost fingerprint -2.0..-5.6 on
     decaying families.
  3. chroma delta-q clamp — clamp(qi/2,0,24) is qindex-proportional, small at
     the high-quality end; mid fingerprint.
  4. LF sharpness still{7,5,3}@{80,160} — low-qindex-scheduled (right band)
     but the fingerprint is tiny (-0.5..-0.8 med): cannot explain a 5-point
     band swing alone.
  CONFOUND (stated): cpu2's own small-size behavior improves too; the tune-off
  arm at {256,512} in the A/B below separates our-tune-decay from baseline
  drift. At 256 BOTH bands collapse — partition/coding defaults join the
  suspects there, not just tune constants.""")
    print(f"""
=== PROPOSAL (measured shape; response coefficient needs the per-size A/B) ===
Advantage medians: {med.get('top', float('nan')):+.2f} (top) / {med.get('1024', float('nan')):+.2f} (1024) / {med.get('512', float('nan')):+.2f} (512) / {med.get('256', float('nan')):+.2f} (256)
Proposed size-conditional tune strength multiplier (log-px linear ramp):
    m(px) = clamp( (log2(px) - 16) / (20 - 16), M256, 1.0 )
  i.e. full strength at >= 2^20 px (1024x1024-class), ramping down to M256 at
  2^16 px (256x256-class), applied to the size-sensitive tune constants
  (chroma delta-q clamp scale, QM curve offset, boost strength — whichever the
  A/B convicts). M256 candidates {{0.0, 0.25, 0.5}}.
DATA NEED (the follow-up mechanism A/B, cell-cached coarse grid): re-run
  {{tune-off, +chroma-dq, +QM-curves, +boost1.0}} x sizes {{256, 512}} on the
  wedge full-crop corpus (16 origins x 6q x 4 arms x 2 sizes = 768 cells) —
  the same isolation ladder the 1024 program used, one size axis added.
""")

    hdr = [
        "hyperparam-expert first cut: size-conditional tune strength — decay attribution (FEATURE_HINTS section E; wedge #3)",
        "source: wedge-2026-07-03 zr vs cpu2 full crops via the label store; BD + 25%/75%-band bpp gaps per (origin,size)",
        "NO per-mechanism arms exist below 1024 — this is a measured narrowing + A/B spec, not a proof (see script docstring)",
        "decay_slope = -d(bd_cpu2)/d(log2 px) over each origin's size ladder (>=3 sizes incl 256); positive = we lose advantage as size shrinks",
    ]
    out = D.merge(S[["decay_slope"]], left_on="origin_id", right_index=True, how="left")
    to_tsv(out.set_index(["origin_id", "size_class"]), os.path.abspath(OUT_TSV), hdr)


if __name__ == "__main__":
    main()

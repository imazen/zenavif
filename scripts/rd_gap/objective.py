#!/usr/bin/env python3
"""The ONE canonical scalar objective for every COOPT_LOOP A/B and joint fit.

Phase 0 of docs/COOPT_LOOP_PLAN.md formalizes the 2026-07-05 evaluation policy
into a single number the fits minimize. Today ~10 analyze_*.py scripts each
re-derive the verdict (per-image BD here, a median there, a hand-applied veto
somewhere else); a joint fit over cached cells needs ONE objective so every
candidate config is scored the same way. This is that function.

The policy (docs/RD_GAP_VS_LIBAOM.md "EVALUATION POLICY", COOPT_LOOP_OBJECTIVE.md):

  minimize bytes at matched quality  (ssim2 BD-rate, negative = arm wins),
  aggregated PER-FAMILY FIRST then cluster_size mass-weighted (the k-means
  subset is DIVERSE not representative, so a plain median over-weights rare
  classes and dilutes photo-dominant effects),
  subject to a HARD butteraugli veto: an arm that improves ssim2 by regressing
  butteraugli beyond the threshold (default +1.0% BD, the pre-registered rule)
  is INFEASIBLE, not merely worse — the metric-gaming guard.

Generalizes bd_arm.py (imported: frontier + bd_rate are the Bjontegaard core)
with family grouping, the veto composition, and the mass weighting.

Usage:
  objective.py BASE.tsv ARM.tsv [--manifest cluster_sizes.tsv] [--veto 1.0] [--json]
  objective.py --selftest        # deterministic self-check, no data files

Emits a per-family table (the policy's "per-family FIRST"), then ONE scalar:
the mass-weighted ssim2 BD-rate in %, or +VETO_PENALTY when the butteraugli
constraint is violated. Lower is better; the incumbent (arm == base) scores 0.0.
"""
import argparse
import collections
import csv
import json
import math
import sys

import numpy as np

# Reuse the Bjontegaard machinery — do NOT re-derive it (the drift lesson: one
# implementation, or verdicts diverge). bd_arm.py lives beside this file.
sys.path.insert(0, __file__.rsplit("/", 1)[0] if "/" in __file__ else ".")
from bd_arm import bd_rate, frontier  # noqa: E402

# An infeasible (butteraugli-vetoed) config is not "a bit worse" — it must be
# unreachable by any minimizer, but finite so multi-start search can still rank
# two infeasible points by how badly they violate.
VETO_PENALTY = 1.0e6
# Metrics whose raw value is lower-better; converted to a quality axis by -log
# so the shared frontier/bd_rate treat higher = better uniformly (bd_arm.py).
LOWER_BETTER = {"butteraugli_3n", "butteraugli_max"}
DEFAULT_VETO_PCT = 1.0  # pre-registered: butteraugli BD > +1.0% is a veto


def load_by_family(path, metric, encoder="zenrav1e"):
  """image -> [(quality, bpp)] frontier points, plus image -> family.

  Mirrors bd_arm.load's column handling (same TSV schema from run_gap.sh:
  image w h family encoder fmt q bytes bpp ssim2 enc_ms butteraugli_3n
  butteraugli_max) but also returns the family map for grouping.
  """
  pts = collections.defaultdict(list)
  fam = {}
  with open(path) as f:
    for r in csv.DictReader(f, delimiter="\t"):
      if r.get("encoder") != encoder:
        continue
      try:
        v = float(r[metric])
        bpp = float(r["bpp"])
      except (ValueError, KeyError):
        continue  # NA cells (butteraugli off, failed decode) — dropped
      if metric in LOWER_BETTER:
        if v <= 0:
          continue
        v = -math.log(v)  # quality axis: higher = better
      pts[r["image"]].append((v, bpp))
      fam[r["image"]] = r.get("family", "?")
  return pts, fam


def per_image_bd(base_path, arm_path, metric, encoder="zenrav1e"):
  """image -> BD-rate% (arm vs base). + = arm needs MORE bits (worse)."""
  base, fam = load_by_family(base_path, metric, encoder)
  arm, _ = load_by_family(arm_path, metric, encoder)
  out = {}
  for img in base:
    if img not in arm:
      continue
    bd = bd_rate(frontier(arm[img]), frontier(base[img]))
    if bd is not None:
      out[img] = bd
  return out, fam


def _weighted(values_by_family, weights):
  """cluster_size mass-weighted mean over families (equal-weight fallback)."""
  num = den = 0.0
  for f, v in values_by_family.items():
    w = weights.get(f, 1.0)
    num += w * v
    den += w
  return num / den if den else float("nan")


def score(base_path, arm_path, weights=None, veto_pct=DEFAULT_VETO_PCT,
          encoder="zenrav1e"):
  """The canonical objective. Returns a dict; ['objective'] is the scalar."""
  weights = weights or {}
  metrics = ["ssim2", "butteraugli_3n", "butteraugli_max"]
  bd, fam = {}, {}
  for m in metrics:
    bd[m], fmap = per_image_bd(base_path, arm_path, m, encoder)
    if not fam:
      fam = fmap

  families = sorted(set(fam.get(i, "?") for i in bd.get("ssim2", {})))
  per_family = {}
  for f in families:
    imgs = [i for i in bd["ssim2"] if fam.get(i) == f]
    if not imgs:
      continue
    row = {"n": len(imgs)}
    for m in metrics:
      vals = [bd[m][i] for i in imgs if i in bd[m]]
      row[m] = float(np.median(vals)) if vals else float("nan")
    # metric-gaming guard is per-family: a family is vetoed if EITHER
    # butteraugli norm regresses past the threshold there.
    row["veto"] = (
      (not math.isnan(row["butteraugli_3n"]) and row["butteraugli_3n"] > veto_pct)
      or (not math.isnan(row["butteraugli_max"]) and row["butteraugli_max"] > veto_pct)
    )
    per_family[f] = row

  ss2_by_family = {f: r["ssim2"] for f, r in per_family.items()
                   if not math.isnan(r["ssim2"])}
  mass_ss2 = _weighted(ss2_by_family, weights)
  # Aggregate veto: the mass-weighted butteraugli BD over families (the
  # pre-registered "median ssim2 rank, butteraugli veto" applied at the
  # aggregate the fit optimizes). Any-family veto is surfaced separately.
  b3_by_family = {f: r["butteraugli_3n"] for f, r in per_family.items()
                  if not math.isnan(r["butteraugli_3n"])}
  bmax_by_family = {f: r["butteraugli_max"] for f, r in per_family.items()
                    if not math.isnan(r["butteraugli_max"])}
  mass_b3 = _weighted(b3_by_family, weights)
  mass_bmax = _weighted(bmax_by_family, weights)
  vetoed = (mass_b3 > veto_pct) or (mass_bmax > veto_pct)

  objective = VETO_PENALTY + max(mass_b3, mass_bmax) if vetoed else mass_ss2
  return {
    "objective": objective,
    "vetoed": vetoed,
    "mass_ssim2_bd": mass_ss2,
    "mass_butteraugli_3n_bd": mass_b3,
    "mass_butteraugli_max_bd": mass_bmax,
    "veto_pct": veto_pct,
    "families_vetoed": [f for f, r in per_family.items() if r["veto"]],
    "per_family": per_family,
    "weighting": "cluster_size" if weights else "equal",
  }


def load_weights(path):
  """family -> cluster_size from a manifest TSV (columns: family cluster_size)."""
  w = {}
  with open(path) as f:
    for r in csv.DictReader(f, delimiter="\t"):
      try:
        w[r["family"]] = float(r["cluster_size"])
      except (ValueError, KeyError):
        continue
  return w


def _report(res, as_json):
  if as_json:
    print(json.dumps(res, indent=2, sort_keys=True))
    return
  print(f"# weighting={res['weighting']}  veto>+{res['veto_pct']}% butteraugli BD")
  print("family\tn\tssim2_bd%\tba3n_bd%\tbamax_bd%\tveto")
  for f, r in sorted(res["per_family"].items()):
    print(f"{f}\t{r['n']}\t{r['ssim2']:+.3f}\t{r['butteraugli_3n']:+.3f}\t"
          f"{r['butteraugli_max']:+.3f}\t{'VETO' if r['veto'] else ''}")
  print(f"# mass ssim2 BD = {res['mass_ssim2_bd']:+.4f}%  "
        f"ba3n = {res['mass_butteraugli_3n_bd']:+.4f}%  "
        f"bamax = {res['mass_butteraugli_max_bd']:+.4f}%")
  if res["vetoed"]:
    print(f"# VETOED (butteraugli constraint) -> objective = {res['objective']:.1f}")
  else:
    print(f"# OBJECTIVE = {res['objective']:+.4f}  (lower=better; incumbent=0)")


def selftest():
  """Deterministic checks with no data files — the veto + weighting logic."""
  import os
  import tempfile

  hdr = "image\tw\th\tfamily\tencoder\tfmt\tq\tbytes\tbpp\tssim2\tenc_ms\tbutteraugli_3n\tbutteraugli_max\n"

  def cell(img, fam, q, bpp, ssim2, ba3, bamax):
    return (f"{img}\t256\t256\t{fam}\tzenrav1e\tavif\t{q}\t0\t{bpp}\t{ssim2}\t"
            f"1.0\t{ba3}\t{bamax}\n")

  def write(rows):
    fd, p = tempfile.mkstemp(suffix=".tsv")
    with os.fdopen(fd, "w") as f:
      f.write(hdr)
      f.writelines(rows)
    return p

  # A 4-point ssim2/bpp frontier per image (bd_rate needs >=4 overlapping).
  # butteraugli must VARY with bitrate (a constant is one quality level -> BD
  # undefined), so carry a decreasing-with-bpp schedule scaled by ba_mul.
  qs = [(20, 0.20, 40.0), (40, 0.40, 55.0), (60, 0.70, 70.0), (80, 1.20, 85.0)]
  BA = [2.0, 1.4, 1.0, 0.7]  # butteraugli falls as bitrate rises

  def frontier_rows(img, fam, bpp_mul, ba_mul):
    return [cell(img, fam, q, bpp * bpp_mul, ss2, BA[i] * ba_mul, BA[i] * ba_mul)
            for i, (q, bpp, ss2) in enumerate(qs)]

  # BASE: two families, benign butteraugli.
  base_rows = (frontier_rows("photoA", "5000", 1.0, 1.0)
               + frontier_rows("plotB", "7000", 1.0, 1.0))
  base = write(base_rows)

  # ARM 1: uses 10% FEWER bits everywhere (bpp_mul 0.9), butteraugli schedule
  # unchanged => same butteraugli at fewer bits (BD better) AND ssim2 BD clearly
  # negative (arm wins), no veto.
  arm_win = write(frontier_rows("photoA", "5000", 0.9, 1.0)
                  + frontier_rows("plotB", "7000", 0.9, 1.0))
  r = score(base, arm_win)
  assert not r["vetoed"], r
  assert r["objective"] < -1.0, f"expected clear win, got {r['objective']}"

  # ARM 2: same bit win BUT butteraugli regressed (ba_mul 1.3 => worse-but-
  # overlapping butteraugli, needing more bits at matched butteraugli despite
  # the 0.9x bpp) -> VETO regardless of the ssim2 gain (metric gaming).
  arm_game = write(frontier_rows("photoA", "5000", 0.9, 1.3)
                   + frontier_rows("plotB", "7000", 0.9, 1.3))
  r2 = score(base, arm_game)
  assert r2["vetoed"], f"expected veto, got {r2}"
  assert r2["objective"] >= VETO_PENALTY, r2

  # ARM 3: incumbent (arm == base) scores ~0 and is not vetoed.
  r3 = score(base, base)
  assert not r3["vetoed"], r3
  assert abs(r3["objective"]) < 1e-6, f"incumbent must be 0, got {r3['objective']}"

  # Mass weighting: a per-photo-only win, diluted under equal weight but
  # dominant when the photo family carries the mass (policy: representative,
  # not diverse).
  arm_photo_only = write(frontier_rows("photoA", "5000", 0.8, 1.0)   # big win
                         + frontier_rows("plotB", "7000", 1.0, 1.0))  # neutral
  eq = score(base, arm_photo_only)
  heavy = score(base, arm_photo_only, weights={"5000": 100.0, "7000": 1.0})
  assert heavy["objective"] < eq["objective"] - 1.0, (
    f"mass weighting must amplify the photo win: eq={eq['objective']} "
    f"heavy={heavy['objective']}")

  for p in (base, arm_win, arm_game, arm_photo_only):
    os.unlink(p)
  print("selftest OK: win<0, metric-gaming vetoed, incumbent=0, mass-weighting amplifies")


def main():
  ap = argparse.ArgumentParser(description=__doc__,
                               formatter_class=argparse.RawDescriptionHelpFormatter)
  ap.add_argument("base", nargs="?")
  ap.add_argument("arm", nargs="?")
  ap.add_argument("--manifest", help="TSV: family<TAB>cluster_size (mass weights)")
  ap.add_argument("--veto", type=float, default=DEFAULT_VETO_PCT,
                  help="butteraugli BD%% veto threshold (default +1.0)")
  ap.add_argument("--encoder", default="zenrav1e")
  ap.add_argument("--json", action="store_true")
  ap.add_argument("--selftest", action="store_true")
  args = ap.parse_args()

  if args.selftest:
    selftest()
    return
  if not args.base or not args.arm:
    ap.error("BASE.tsv and ARM.tsv required (or --selftest)")
  weights = load_weights(args.manifest) if args.manifest else None
  res = score(args.base, args.arm, weights, args.veto, args.encoder)
  _report(res, args.json)


if __name__ == "__main__":
  main()

#!/usr/bin/env python3
"""SPEED-LADDER GAP MAP analysis (2026-07-04).

Consumes the chain_speed_ladder.sh per-arm TSVs and emits:
  1. per-arm timing summary (median wall-ms/MP from the solo RD_CACHE=off pass)
  2. BD-rate matrix: every zr arm vs every aom arm (ssim2 + butteraugli), per corpus,
     photos (fam-7 excluded) and all, with win counts
  3. time-normalized pareto: each arm's (ms/MP, BD vs the aom-cpu6-iq-allintra
     reference) + matched-time pairing per zr tier -> crossover verdict
  4. per-family BD slices for the fast tiers -> ranked wedge list (worst tier x family)

Usage: analyze_speed_ladder.py <dir-with-tsvs> [--corpus t26|leg]
"""
import argparse
import collections
import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bd_arm import load, frontier, bd_rate  # noqa: E402

ZR_SPEEDS = [2, 4, 6, 8, 10]
AOM_CPUS = [2, 4, 6, 8, 9]


def arm_files(d, corpus):
  zr = {f"zr-s{s}-{c}": os.path.join(d, f"zr_{corpus}_s{s}_{c}.tsv")
        for s in ZR_SPEEDS for c in ("tune", "off")}
  aom = {f"aom-cpu{c}{t}-ai": os.path.join(d, f"aom_{corpus}_cpu{c}{t}.tsv")
         for c in AOM_CPUS for t in ("def", "iq")}
  good = {f"aom-{g}-good": os.path.join(d, f"aomgood_{corpus}_{g}.tsv")
          for g in ("cpu2", "cpu0def", "cpu0ss2")}
  return zr, aom, good


def load_families(path):
  fam = {}
  import csv
  with open(path) as f:
    for r in csv.DictReader(f, delimiter="\t"):
      fam[r["image"]] = r.get("family", "?")
  return fam


def per_image_bd(base_path, arm_path, metric, enc_base, enc_arm):
  """dict image -> BD% of arm vs base."""
  base, _ = load(base_path, metric, enc_base)
  arm, _ = load(arm_path, metric, enc_arm)
  out = {}
  for img in sorted(base):
    if img not in arm:
      continue
    bd = bd_rate(frontier(arm[img]), frontier(base[img]))
    if bd is not None:
      out[img] = bd
  return out


def summarize(per_img, fams, photos_only=False):
  vals = [v for k, v in per_img.items()
          if not (photos_only and fams.get(k, "").startswith("7"))]
  if not vals:
    return None
  a = np.array(vals)
  return dict(n=len(a), median=float(np.median(a)), mean=float(a.mean()),
              wins=int((a < 0).sum()))


def timing_summary(d):
  """arm -> (median ms/MP, p25, p75, n) from timing_*.tsv (solo cells)."""
  import csv
  out = {}
  for s in ZR_SPEEDS:
    for c in ("tune", "off"):
      out[f"zr-s{s}-{c}"] = os.path.join(d, f"timing_zr_s{s}_{c}.tsv")
  for cpu in AOM_CPUS:
    for t in ("def", "iq"):
      out[f"aom-cpu{cpu}{t}-ai"] = os.path.join(d, f"timing_aom_cpu{cpu}{t}.tsv")
  res = {}
  for arm, path in out.items():
    if not os.path.exists(path):
      continue
    mspmp = []
    with open(path) as f:
      for r in csv.DictReader(f, delimiter="\t"):
        try:
          px = int(r["w"]) * int(r["h"])
          mspmp.append(float(r["enc_ms"]) / (px / 1e6))
        except (ValueError, KeyError):
          pass
    if mspmp:
      a = np.array(mspmp)
      res[arm] = dict(median=float(np.median(a)), p25=float(np.percentile(a, 25)),
                      p75=float(np.percentile(a, 75)), n=len(a))
  return res


def main():
  ap = argparse.ArgumentParser()
  ap.add_argument("dir")
  ap.add_argument("--corpus", default="t26", choices=["t26", "leg"])
  ap.add_argument("--ref", default="aom-cpu6iq-ai", help="common pareto reference arm")
  ap.add_argument("--metrics", default="ssim2,butteraugli_3n,butteraugli_max")
  args = ap.parse_args()
  d = args.dir
  zr, aom, good = arm_files(d, args.corpus)
  allarms = {**zr, **aom, **good}
  missing = [k for k, v in allarms.items() if not os.path.exists(v)]
  if missing:
    print(f"# MISSING arms (skipped): {', '.join(missing)}")
  arms = {k: v for k, v in allarms.items() if os.path.exists(v)}
  fams = {}
  for v in arms.values():
    fams.update(load_families(v))

  timing = timing_summary(d)
  print("\n== timing (solo RD_CACHE=off, 4 img x 3 q, median wall ms/MP) ==")
  print("arm\tms_per_MP\tp25\tp75\tn")
  for arm in sorted(timing, key=lambda a: timing[a]["median"]):
    t = timing[arm]
    print(f"{arm}\t{t['median']:.0f}\t{t['p25']:.0f}\t{t['p75']:.0f}\t{t['n']}")

  metrics = args.metrics.split(",")
  refs = {**aom, **good}
  print(f"\n== BD matrix (corpus={args.corpus}; photos = fam-7 excluded; neg = zr fewer bits) ==")
  print("zr_arm\tref_arm\tmetric\tscope\tn\tmedian\tmean\twins")
  bd_cache = {}
  for zk, zv in sorted(zr.items()):
    if not os.path.exists(zv):
      continue
    for rk, rv in sorted(refs.items()):
      if not os.path.exists(rv):
        continue
      for m in metrics:
        pim = per_image_bd(rv, zv, m, "libaom", "zenrav1e")
        bd_cache[(zk, rk, m)] = pim
        for scope, ph in (("photos", True), ("all", False)):
          s = summarize(pim, fams, ph)
          if s:
            print(f"{zk}\t{rk}\t{m}\t{scope}\t{s['n']}\t{s['median']:+.2f}\t{s['mean']:+.2f}\t{s['wins']}/{s['n']}")

  # time-normalized pareto vs common reference
  refarm = args.ref
  print(f"\n== time-normalized pareto (BD ssim2 photos vs {refarm}; timing = solo ms/MP) ==")
  print("arm\tms_per_MP\tbd_median_vs_ref\tbd_mean\twins")
  rows = []
  for ak in sorted(arms):
    t = timing.get(ak, {}).get("median")
    if ak == refarm:
      rows.append((ak, t, 0.0, 0.0, "-"))
      continue
    if ak.startswith("zr-"):
      pim = bd_cache.get((ak, refarm, "ssim2"))
    else:
      pim = per_image_bd(arms[refarm], arms[ak], "ssim2", "libaom", "libaom")
    if pim is None:
      continue
    s = summarize(pim, fams, True)
    if s:
      rows.append((ak, t, s["median"], s["mean"], f"{s['wins']}/{s['n']}"))
  for ak, t, med, mean, wins in sorted(rows, key=lambda r: (r[1] is None, r[1])):
    ts = f"{t:.0f}" if t is not None else "NA"
    print(f"{ak}\t{ts}\t{med:+.2f}\t{mean:+.2f}\t{wins}")

  # matched-time pairing per zr tier
  print("\n== matched-wall-time pairing (each zr arm vs nearest-time aom arm) ==")
  print("zr_arm\tms/MP\tnearest_aom\tms/MP\ttime_ratio\tbd_ssim2_med(photos)\tbd_ba3n_med\twins_ssim2")
  for zk in sorted(zr):
    tz = timing.get(zk, {}).get("median")
    if tz is None:
      continue
    best, bd_t = None, None
    for akk in aom:
      ta = timing.get(akk, {}).get("median")
      if ta is None:
        continue
      dlt = abs(math.log(tz / ta))
      if bd_t is None or dlt < bd_t:
        best, bd_t = akk, dlt
    if best is None:
      continue
    ta = timing[best]["median"]
    s = summarize(bd_cache.get((zk, best, "ssim2"), {}), fams, True)
    sb = summarize(bd_cache.get((zk, best, "butteraugli_3n"), {}), fams, True)
    if s:
      print(f"{zk}\t{tz:.0f}\t{best}\t{ta:.0f}\t{tz/ta:.2f}x\t{s['median']:+.2f}\t{(sb or {}).get('median', float('nan')):+.2f}\t{s['wins']}/{s['n']}")

  # per-family slices + wedge list for fast tiers
  print("\n== per-family BD (ssim2) for fast tiers vs matched-time aom; wedge candidates ==")
  print("zr_arm\tref\tfamily\tn\tmedian\tworst_img\tworst_bd")
  wedges = []
  for zk in sorted(zr):
    if zk.split("-")[1] in ("s2",):
      pass  # include s2 for context too
    tz = timing.get(zk, {}).get("median")
    if tz is None:
      continue
    best = min((a for a in aom if timing.get(a)), key=lambda a: abs(math.log(tz / timing[a]["median"])), default=None)
    if best is None:
      continue
    pim = bd_cache.get((zk, best, "ssim2"))
    if not pim:
      continue
    byfam = collections.defaultdict(dict)
    for img, bd in pim.items():
      byfam[fams.get(img, "?")][img] = bd
    for fam in sorted(byfam):
      vals = byfam[fam]
      med = float(np.median(list(vals.values())))
      worst = max(vals.items(), key=lambda kv: kv[1])
      print(f"{zk}\t{best}\t{fam}\t{len(vals)}\t{med:+.2f}\t{worst[0][:44]}\t{worst[1]:+.2f}")
      if zk.split("-")[1] != "s2":
        wedges.append((med, zk, best, fam, len(vals), worst))
  print("\n== WEDGE LIST (worst tier x family cells, fast tiers only, by median BD) ==")
  for med, zk, best, fam, n, worst in sorted(wedges, reverse=True)[:12]:
    print(f"  {med:+8.2f}%  {zk} vs {best}  fam={fam} n={n}  worst={worst[0][:40]} {worst[1]:+.1f}%")


if __name__ == "__main__":
  main()

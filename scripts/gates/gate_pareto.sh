#!/usr/bin/env bash
# gate-pareto (GOAL_PARETO G7): a G1/G2 subset, end to end, with every
# hardening rule from round 1 encoded:
#   - live-checked reference versions (logged into the verdict)
#   - SOLO walls (SVT --lp 1 via svt_cell default; aomenc default; cavif tiles
#     area-capped) — never all-cores numbers
#   - sign-safe score parses (the cells' '[-0-9.]+' fix)
#   - PINNED FORMAT AXIS: everything 4:2:0
#   - per-image MONOTONE-CHECK before any BD is believed
#   - BANDED verdicts (objective.py low/mid/high + ba bands), never one scalar
# Subset: SUBSET_N images (default 6) x 6q, armed s6 vs aom cpu2-ss2-allintra
# + svt p2t4. This is the fast tripwire; full rounds use the same pieces.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"; RG="$HERE/../rd_gap"
OUTD="${OUTD:-/tmp/gate_pareto.$$}"; mkdir -p "$OUTD"
SUBSET_N="${SUBSET_N:-6}"
export SAVE_PNG="${SAVE_PNG:-$HERE/../../target/release/examples/save_png}"
export SCORER="${SCORER:-/home/lilith/work/zen/fast-ssim2/target/release/fast-ssim2-cli}"
export BUTTER="${BUTTER:-/home/lilith/work/butteraugli/target/release/butteraugli}"
export AOMENC="${AOMENC:-/home/lilith/work/aom/build_butteraugli/aomenc}"
export AOMDEC="${AOMDEC:-/home/lilith/work/aom/build_butteraugli/aomdec}"
export CAVIF="${CAVIF:-/home/lilith/work/zen/ravif--cooptloop/target/release/cavif}"
SVTENC="${SVTENC:-/home/lilith/work/zen/svtav1-v4.1.0/Bin/Release/SvtAv1EncApp}"
export RD_CACHE=off
head -1 "$RG/sample_images_train26.tsv" > "$OUTD/subset.tsv"
tail -n +2 "$RG/sample_images_train26.tsv" | head -n "$SUBSET_N" >> "$OUTD/subset.tsv"
export SAMPLE="$OUTD/subset.tsv"
echo "[gate-pareto] refs: aomenc=$("$AOMENC" --help 2>/dev/null | grep -oE 'v[0-9.]+' | head -1) svt=$(LD_LIBRARY_PATH=$(dirname "$SVTENC") "$SVTENC" --version 2>&1 | grep -oE 'v[0-9.]+' | head -1)"
# armed leg (420)
export CAVIF_EXTRA="--yuv 420" QGRID_ZR="30 50 60 75 85 95" ZENRAV1E_SPEED=6
unset AOMENC_RUN
( unset AOMENC; OUT="$OUTD/arm.tsv" bash "$RG/run_gap.sh" >/dev/null 2>&1 )
# aom leg
export AOMFMTS=420 CQGRID_AOM="8 14 20 26 32 38 44 50 56 63"
AOM_CPU=2 AOM_EXTRA="--allintra --tune=ssimulacra2" OUT="$OUTD/aom.tsv" bash "$RG/aom_only.sh" >/dev/null 2>&1
# svt leg (solo by svt_cell default)
TMPD=$(mktemp -d); trap 'rm -rf "$TMPD"' EXIT
TSV="$OUTD/svt.tsv"
echo -e "image\tw\th\tfamily\tencoder\tfmt\tq\tbytes\tbpp\tssim2\tenc_ms\tbutteraugli_3n\tbutteraugli_max" > "$TSV"
tail -n +2 "$SAMPLE" | while IFS=$'\t' read -r img w h fam; do
  for crf in 15 25 35 45 55 65; do
    bash "$RG/svt_cell.sh" "$img" "$fam" 2 "$crf" 4 "$TMPD" 2>/dev/null >> "$TSV" || true
  done
done
# monotone-check every curve, all legs
python3 - "$OUTD" <<'PYEOF'
import csv, os, sys
d = sys.argv[1]; bad = 0
for name, enc in (("arm.tsv", "zenrav1e"), ("aom.tsv", "libaom"), ("svt.tsv", None)):
    by = {}
    for r in csv.DictReader(open(os.path.join(d, name)), delimiter="\t"):
        if enc and r["encoder"] != enc: continue
        try: by.setdefault(r["image"], []).append((float(r["bpp"]), float(r["ssim2"])))
        except ValueError: pass
    for img, pts in by.items():
        pts.sort()
        for (b1, s1), (b2, s2) in zip(pts, pts[1:]):
            if s2 < s1 - 3.0:
                print(f"[gate-pareto] NON-MONOTONE {name} {img[:40]} ({s1:.1f}->{s2:.1f})"); bad += 1
sys.exit(1 if bad > 3 else 0)
PYEOF
[ $? -ne 0 ] && { echo "gate-pareto: FAIL (curve integrity)"; exit 1; }
# banded verdicts (relabel refs for objective's single-encoder filter)
python3 - "$OUTD" <<'PYEOF'
import csv, sys
d = sys.argv[1]
for src, dst, enc in (("aom.tsv", "aom_b.tsv", "libaom"), ("svt.tsv", "svt_b.tsv", "svt-av1-v4.1.0")):
    rows = list(csv.DictReader(open(f"{d}/{src}"), delimiter="\t"))
    cols = list(rows[0].keys())
    with open(f"{d}/{dst}", "w") as f:
        f.write("\t".join(cols) + "\n")
        for r in rows:
            if r["encoder"] != enc: continue
            r = dict(r); r["encoder"] = "zenrav1e"
            f.write("\t".join(r[c] for c in cols) + "\n")
PYEOF
echo "[gate-pareto] vs aom cpu2-ss2-allintra (420):"
python3 "$RG/objective.py" "$OUTD/aom_b.tsv" "$OUTD/arm.tsv" | tail -4
echo "[gate-pareto] vs svt p2t4 (420, solo):"
python3 "$RG/objective.py" "$OUTD/svt_b.tsv" "$OUTD/arm.tsv" | tail -4
echo "gate-pareto: COMPLETE (verdicts above; G1/G2 pass criteria per the charter)"

#!/usr/bin/env bash
# The whole two-shot precision measurement, as one command, so re-running it
# against a different encoder state is not an exercise in remembering flags.
#
#   scripts/hyperparam/run_zensim_two_shot.sh <outdir> [speed] [sizes] [targets]
#
# Produces, in <outdir>:
#   provenance.txt   the exact encoder state (git revs + source content
#                    hashes + binary hash). Without this a result cannot be
#                    attached to an encoder, and this measurement has already
#                    been invalidated once by the encoder moving underneath it.
#   lattice_*.tsv    dense per-QUANTIZER achievable-score tables. Sweeping
#                    quality instead would address only 100 of the codec's
#                    256 quantizers and overstate the lattice by 2.56x.
#   fit.txt          rule comparison + the refit ANCHOR_QUANTIZER constants
#   ab2.tsv          real encodes, six arms, every one capped at 2 encodes
#   ab2_summary.txt  the error distribution decomposed into its LATTICE and
#                    PREDICTION terms -- the headline
#
# TRAIN and VAL manifests are disjoint by SOURCE, not by cell: the same photo
# at two sizes is not two independent samples.
set -euo pipefail

OUT=${1:?usage: run_zensim_two_shot.sh <outdir> [speed] [sizes] [targets]}
SPEED=${2:-6}
SIZES=${3:-64,256,1024}
# Targets cover the LOW range at the same density as the high one. A grid
# that thins out below 60 hides exactly the regime where the score-vs-
# quantizer curve is flattest and the placement is hardest.
TARGETS=${4:-20,25,30,35,40,45,50,55,60,65,70,75,80,85,90}
REPO=$(cd "$(dirname "$0")/../.." && pwd)
ZEN=$(cd "$REPO/.." && pwd)
BIN="$REPO/target/release/examples/zensim_loop_bench"
TRAIN="$REPO/benchmarks/zensim_loop_manifest_2026-08-06.txt"
VAL="$REPO/benchmarks/zensim_lattice_manifest_val_2026-08-06.txt"

mkdir -p "$OUT"

# Build ONCE. Every cell of every arm then runs on the same statically
# linked encoder, so a concurrent edit to a sibling crate cannot move the
# target mid-run -- it can only invalidate the next build.
RUSTC_WRAPPER=${RUSTC_WRAPPER:-sccache} cargo build -j 4 --release \
    --features two-pass-zensim,auto-tune --example zensim_loop_bench

{
    echo "date          $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "host          $(uname -sr) $(uname -m)"
    echo "rustc         $(rustc --version)"
    for r in ravif zenrav1e; do
        d="$ZEN/$r"
        [ -d "$d/.git" ] || continue
        echo "$r HEAD     $(git -C "$d" rev-parse HEAD)"
        echo "$r dirty    $(git -C "$d" status --porcelain | wc -l | tr -d ' ') files"
    done
    for d in "$ZEN"/ravif*/ravif/src "$ZEN"/zenrav1e/src; do
        [ -d "$d" ] || continue
        echo "src hash      $d $( (cd "$d" && find . -name '*.rs' -exec shasum -a 256 {} \; | sort | shasum -a 256) | cut -d' ' -f1)"
    done
    echo "zenavif HEAD  $(git -C "$REPO" rev-parse HEAD)"
    echo "ravif dep     $(grep -m1 '^ravif = ' "$REPO/Cargo.toml")"
    echo "binary sha256 $(shasum -a 256 "$BIN" | cut -d' ' -f1)"
    echo "NOT built with -C target-cpu=native"
} | tee "$OUT/provenance.txt"

# nice -n 5, NOT 19: on macOS a high nice value lands the process in the
# background QoS class (efficiency cores only), ~40x slower.
run() { RAYON_NUM_THREADS=1 nice -n 5 "$BIN" "$@"; }

for split in train val; do
    man=$TRAIN; [ "$split" = val ] && man=$VAL
    echo "[lattice] $split ..." >&2
    run lattice "$man" "$SPEED" "$SIZES" 12 96 > "$OUT/lattice_$split.tsv"
done

python3 "$REPO/scripts/hyperparam/fit_zensim_two_shot.py" \
    --train "$OUT/lattice_train.tsv" --val "$OUT/lattice_val.tsv" \
    --emit-rust > "$OUT/fit.txt"

# The A/B runs on the HELD-OUT sources only. Running it on the sources the
# anchor was fitted on would measure memorisation.
echo "[ab2] ..." >&2
run ab2 "$VAL" "$SPEED" "$SIZES" "$TARGETS" 0.5 > "$OUT/ab2.tsv"

python3 "$REPO/scripts/hyperparam/analyze_zensim_ab2.py" "$OUT/ab2.tsv" \
    --lattice "$OUT/lattice_val.tsv" > "$OUT/ab2_summary.txt"

echo "done -> $OUT" >&2

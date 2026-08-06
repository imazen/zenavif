#!/usr/bin/env bash
# gauntlet — run one cargo subcommand across the feature combos that actually
# gate this crate, printing ONE line per combo. Full logs land in $HOME/tmp.
#
#   scripts/gauntlet.sh check          # cargo check --all-targets, every combo
#   scripts/gauntlet.sh clippy         # clippy -D warnings, every combo
#   scripts/gauntlet.sh nextest        # nextest run, every combo
#   scripts/gauntlet.sh check backends # only the combos matching "backends"
#
# Combos are `name:features`; `-` means default features. The point is to
# catch feature-gate rot in ONE batched pass instead of discovering it one
# slow rebuild at a time.
set -uo pipefail

SUB="${1:?usage: gauntlet.sh <check|clippy|nextest|build> [filter]}"; shift || true
FILTER="${1:-}"
LOGDIR="${QC_LOGDIR:-$HOME/tmp}"; mkdir -p "$LOGDIR"
JOBS="${QC_JOBS:-8}"

# sccache, when present, makes the feature-matrix reruns cheap: each feature
# combination is a distinct compilation the cache keys separately, so the
# second sweep over the matrix is mostly cache hits instead of full rebuilds.
# Opt out with QC_SCCACHE=0.
if [ "${QC_SCCACHE:-1}" = 1 ] && [ -z "${RUSTC_WRAPPER:-}" ] && command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
fi

COMBOS=(
  "default:-"
  "encode:encode,encode-imazen"
  "aom:aom-backend"
  "svt:encode-svt-rs"
  "tq:target-quality"
  "backends:encode-imazen,encode-svt-rs,aom-backend,target-quality"
  "expert:__expert"
  "autotune:auto-tune"
  "twopass:two-pass-butteraugli,encode-imazen"
  "zloop:two-pass-zensim"
  # Everything that is pure Rust: the real "all features" gate for this crate.
  "allsafe:aom-backend,encode,encode-imazen,encode-mono,encode-threading,encode-svt-rs,target-quality,two-pass-butteraugli,two-pass-zensim,__expert,auto-tune,_dev"
  # Literal --all-features. This additionally pulls `unsafe-asm`, i.e. the
  # legacy rav1d C-FFI decoder, whose aarch64 `.S` sources Apple `cc` refuses
  # to assemble (`-march=armv8.6-a`) — so this combo is EXPECTED to fail on
  # macOS aarch64 and is not a CI gate (CI only runs --all-features against
  # zenavif-parse). Kept in the matrix so the failure stays visible and
  # attributed rather than quietly dropped.
  "all:ALLFEATURES"
)

case "$SUB" in
  clippy)  ARGS=(clippy --workspace --all-targets); TAIL=(-- -D warnings) ;;
  # --no-fail-fast: a single early failure otherwise cancels the run and hides
  # every later test, which turns one broken thing into several slow rounds of
  # "fix, rerun, discover the next one".
  nextest) ARGS=(nextest run --workspace --no-fail-fast); TAIL=() ;;
  build)   ARGS=(build --workspace --all-targets); TAIL=() ;;
  *)       ARGS=(check --workspace --all-targets); TAIL=() ;;
esac

fail=0
for c in "${COMBOS[@]}"; do
  name="${c%%:*}"; feats="${c#*:}"
  [ -n "$FILTER" ] && [[ "$name" != *"$FILTER"* ]] && continue
  case "$feats" in
    -)           fargs=() ;;
    ALLFEATURES) fargs=(--all-features) ;;
    *)           fargs=(--features "$feats") ;;
  esac
  LOG="$LOGDIR/gauntlet-$SUB-$name.log"
  start=$(date +%s)
  nice -n 19 cargo "${ARGS[@]}" "${fargs[@]}" -j "$JOBS" "${TAIL[@]}" >"$LOG" 2>&1
  rc=$?
  dur=$(( $(date +%s) - start ))
  nerr=$(grep -c '^error' "$LOG" 2>/dev/null | head -1); nerr=${nerr:-0}
  extra=""
  if [ "$SUB" = nextest ]; then
    extra=$(grep -E '^ *Summary' "$LOG" | tail -1 | sed 's/^ *//')
    [ -z "$extra" ] && extra=$(grep -m1 -E '^error' "$LOG")
  fi
  status=$([ $rc -eq 0 ] && echo PASS || echo "FAIL($rc)")
  [ $rc -eq 0 ] || fail=1
  printf '%-9s %-8s %4ss err=%-3s %s\n' "$name" "$status" "$dur" "$nerr" "$extra"
  if [ $rc -ne 0 ]; then
    grep '^error' "$LOG" | sort -u | head -6 | sed 's/^/    /'
    grep -m4 -E '^ *FAIL \[' "$LOG" | sed 's/^/    /'
  fi
done
echo "logs: $LOGDIR/gauntlet-$SUB-*.log"
exit $fail

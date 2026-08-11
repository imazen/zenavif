#!/usr/bin/env bash
# coverage — per-feature-combo llvm-cov line/region/function coverage, one JSON
# per combo, plus a per-file / per-function summary. Mirrors the combo list in
# scripts/gauntlet.sh EXACTLY so a coverage row and a clippy/nextest row talk
# about the same build.
#
#   scripts/coverage.sh              # every combo
#   scripts/coverage.sh backends     # only combos whose name contains "backends"
#
# Why per combo and not one blended number: most of this crate does not COMPILE
# without its feature, so a single figure silently reports 100% of whatever
# happened to be enabled. A file that is absent from a combo's JSON was not
# measured at 0% — it was not built. The summarizer prints that distinction.
#
# `all` (literal --all-features) is SKIPPED by default: it pulls `unsafe-asm`,
# whose rav1d `.S` sources Apple `cc` refuses to assemble on aarch64 (documented
# in CLAUDE.md). COV_ALL=1 attempts it anyway so the failure stays attributable.
#
# Outputs (JSON is the artifact the summarizer reads; logs hold the test run):
#   $HOME/tmp/zenavif-cov/<combo>.json
#   $HOME/tmp/zenavif-cov/<combo>.log
set -uo pipefail

FILTER="${1:-}"
OUTDIR="${COV_OUTDIR:-$HOME/tmp/zenavif-cov}"; mkdir -p "$OUTDIR"
JOBS="${COV_JOBS:-8}"

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
  "allsafe:aom-backend,encode,encode-imazen,encode-mono,encode-threading,encode-svt-rs,target-quality,two-pass-butteraugli,two-pass-zensim,__expert,auto-tune,_dev"
  "all:ALLFEATURES"
)

fail=0
for c in "${COMBOS[@]}"; do
  name="${c%%:*}"; feats="${c#*:}"
  [ -n "$FILTER" ] && [[ "$name" != *"$FILTER"* ]] && continue
  if [ "$name" = all ] && [ "${COV_ALL:-0}" != 1 ]; then
    printf '%-9s %-9s   (unsafe-asm: Apple cc refuses rav1d .S on aarch64; COV_ALL=1 to try)\n' "$name" "SKIP"
    continue
  fi
  case "$feats" in
    -)           fargs=() ;;
    ALLFEATURES) fargs=(--all-features) ;;
    *)           fargs=(--features "$feats") ;;
  esac
  LOG="$OUTDIR/$name.log"; JSON="$OUTDIR/$name.json"
  start=$(date +%s)
  # --release: the encode combos are unusable at opt-level 0 (a full rav1e
  # encode per test).
  #
  # NOTE: no `--no-clean`. It looks like a free speedup (keep the instrumented
  # deps across combos) and it silently DESTROYS per-combo isolation: the
  # previous combos' binaries and .profraw files stay in the coverage target
  # dir, cargo-llvm-cov globs them all, and every combo's report then merges
  # every other combo's execution data. Measured 2026-08-11: with --no-clean
  # the `default` report listed src/encoder.rs and src/two_pass_zensim.rs (66
  # files, 22.8k lines) — code that combo does not even compile — and all
  # eleven combos came out within 0.1% of each other. A blended number wearing
  # a per-combo label is worse than no number. The price is a rebuild per
  # combo; sccache absorbs most of it.
  # fail-fast off (nextest `coverage` profile): one broken test must not hide
  # the rest of the map.
  # --ignore-run-fail: without it cargo-llvm-cov writes NO report when any test
  # fails, so a combo with one known-failing test (see CLAUDE.md's pre-existing
  # gauntlet failures) contributes nothing to the map at all. The run's PASS/FAIL
  # is still reported per combo from the nextest summary below.
  # `--profile coverage` carries fail-fast = false (.config/nextest.toml) —
  # cargo-llvm-cov rejects the --no-fail-fast flag next to --ignore-run-fail.
  nice -n 19 cargo llvm-cov nextest --release --workspace \
      --ignore-run-fail --profile coverage \
      "${fargs[@]}" -j "$JOBS" --json --output-path "$JSON" >"$LOG" 2>&1
  rc=$?
  dur=$(( $(date +%s) - start ))
  # With --ignore-run-fail the exit status is 0 even when tests failed, so the
  # per-combo status comes from nextest's own summary line (colour codes
  # stripped) — never from $rc alone, or a combo with failing tests would print
  # PASS.
  summary=$(grep -aE 'tests run:' "$LOG" | tail -1 | sed 's/\x1b\[[0-9;]*m//g; s/^ *//')
  [ -z "$summary" ] && summary=$(grep -am1 -E '^error' "$LOG")
  if [ $rc -ne 0 ]; then
    status="BUILD-FAIL($rc)"; fail=1
  elif printf '%s' "$summary" | grep -q 'failed'; then
    status="TESTS-FAIL"; fail=1
  else
    status=PASS
  fi
  printf '%-9s %-12s %4ss %s\n' "$name" "$status" "$dur" "$summary"
done
echo "json: $OUTDIR/*.json   summarize: python3 scripts/cov_summarize.py $OUTDIR/*.json"
exit $fail

#!/usr/bin/env bash
# qc — quiet cargo. Runs a cargo command with the machine-safety defaults
# (nice/ionice, capped -j) and prints a COMPACT summary instead of the full
# firehose. Full output always lands in a log file, path echoed at the end.
#
#   scripts/qc.sh <tag> <cargo-args...>
#
# Examples:
#   scripts/qc.sh chk check --workspace --all-targets
#   scripts/qc.sh t   nextest run --workspace
#
# Why: an agent session reading raw `cargo test` output burns thousands of
# tokens per invocation on progress noise. This prints the counts, the unique
# error/warning headlines, and the failing test names — everything else stays
# on disk for a targeted grep.
set -uo pipefail

TAG="${1:?usage: qc.sh <tag> <cargo-args...>}"; shift
LOGDIR="${QC_LOGDIR:-$HOME/tmp}"
mkdir -p "$LOGDIR"
LOG="$LOGDIR/qc-${TAG}.log"
JOBS="${QC_JOBS:-8}"

# sccache, when present, makes rebuilds after a feature/flag change cheap —
# those change crate metadata and would otherwise be full recompiles of the
# large pure-Rust AV1 ports. Opt out with QC_SCCACHE=0.
if [ "${QC_SCCACHE:-1}" = 1 ] && [ -z "${RUSTC_WRAPPER:-}" ] && command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
fi

# ionice only exists on Linux; nice is portable.
IONICE=""
command -v ionice >/dev/null 2>&1 && IONICE="ionice -c 3"

start=$(date +%s)
# shellcheck disable=SC2086
nice -n 19 $IONICE cargo "$@" -j "$JOBS" >"$LOG" 2>&1
rc=$?
dur=$(( $(date +%s) - start ))

echo "== qc:$TAG rc=$rc ${dur}s =="

# Compiler diagnostics: unique headlines only, capped.
nerr=$(grep -c "^error" "$LOG" 2>/dev/null | head -1)
nwarn=$(grep -c '^warning' "$LOG" 2>/dev/null | head -1); nwarn=${nwarn:-0}
[ "$nerr" -gt 0 ] || [ "$nwarn" -gt 0 ] && echo "diagnostics: $nerr error / $nwarn warning"
if [ "$nerr" -gt 0 ]; then
  echo "--- errors (unique, first 25) ---"
  grep '^error' "$LOG" | sort -u | head -25
  echo "--- first error sites ---"
  grep -m 12 '^  *--> ' "$LOG"
fi

# nextest summary + failures.
if grep -q '^ *Summary' "$LOG"; then
  grep -E '^ *(Summary|Canceling)' "$LOG" | tail -3
  fails=$(grep -E '^ *FAIL \[' "$LOG" | head -40)
  [ -n "$fails" ] && { echo "--- failing tests ---"; echo "$fails"; }
fi

# libtest summary (cargo test / doctests), one line per target that ran.
grep -E '^test result:' "$LOG" | sort | uniq -c | sed 's/^ */  /' | head -20
grep -E '^(failures:|---- .* stdout ----)' "$LOG" | head -20

echo "log: $LOG ($(wc -l <"$LOG" | tr -d ' ') lines)"
exit $rc

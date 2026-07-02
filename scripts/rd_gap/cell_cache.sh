#!/usr/bin/env bash
# Deterministic result cache for rd_gap cells. Source me from a cell script.
#
# Every input that can change a cell's output is hashed into the key, so a hit
# is byte-equivalent to a re-run: encoder binary, image content, quality knob,
# and every env knob the encoders read. Two layers:
#
#   1. ROW cache   — full result row keyed on (encoder-binary, image, knobs).
#      Hit = skip encode+decode+score entirely.
#   2. SCORE cache — (ssim2, butteraugli) keyed on (avif bytes, decoder,
#      scorers). Hit = skip decode+score. Catches arms/knobs that produce
#      byte-identical encodes on some content (gated features fire rarely),
#      and shares scores ACROSS sweep arms.
#
# Enable by exporting RD_CACHE_DIR (run_gap.sh auto-enables /home/lilith/
# sweep_cache when it exists — on the sweep box that path lives inside the
# disk SNAPSHOT, so the cache survives teardown/restore cycles). RD_CACHE=off
# disables even when the dir exists.
#
# CAVEAT: a row-cache hit replays the ORIGINAL enc_ms — fine for RD frontiers
# (bytes/ssim2 are deterministic), WRONG for timing claims. Run timing sweeps
# with RD_CACHE=off.
#
# Concurrency: atomic tmp+mv writes; concurrent writers of the same key write
# identical bytes, last rename wins harmlessly.

_rd_cache_enabled() {
  [ "${RD_CACHE:-on}" != "off" ] && [ -n "${RD_CACHE_DIR:-}" ] && [ -d "${RD_CACHE_DIR:-}" ]
}

# _rd_sha_file <path> — content hash, cached per (dev,inode,mtime,size) in a
# sidecar so big corpora aren't re-hashed every cell.
_rd_sha_file() {
  local f="$1" statk sidecar
  statk=$(stat -c '%d.%i.%Y.%s' "$f" 2>/dev/null) || { echo "nostat"; return; }
  sidecar="$RD_CACHE_DIR/.sha/$(echo "$f.$statk" | sha256sum | cut -c1-40)"
  if [ -s "$sidecar" ]; then cat "$sidecar"; return; fi
  local h; h=$(sha256sum < "$f" | cut -c1-40)
  mkdir -p "$RD_CACHE_DIR/.sha"
  printf '%s' "$h" > "$sidecar.tmp.$$" && mv -f "$sidecar.tmp.$$" "$sidecar"
  printf '%s' "$h"
}

# _rd_env_knobs — every env var that can steer the encoders, canonical order.
# RD_CACHE_EXTRA lets a sweep declare ad-hoc knobs (e.g. dev passthroughs).
_rd_env_knobs() {
  local out=""
  local v
  for v in $(compgen -e | grep -E '^(ZENRAV1E_|ZENRAVIF_|RAV1E_|AOM_)' | sort); do
    out+="$v=${!v};"
  done
  printf '%s|extra=%s' "$out" "${RD_CACHE_EXTRA:-}"
}

# rd_cache_row_key <encoder_bin> <img> <tag...>  → sets RD_ROW_KEY
rd_cache_row_key() {
  local bin="$1" img="$2"; shift 2
  RD_ROW_KEY=$(printf 'row1|%s|%s|%s|%s' \
    "$(_rd_sha_file "$bin")" "$(_rd_sha_file "$img")" "$*" "$(_rd_env_knobs)" \
    | sha256sum | cut -c1-40)
}

rd_cache_row_get() { # → prints cached row, rc 0 on hit
  _rd_cache_enabled || return 1
  local f="$RD_CACHE_DIR/rows/${RD_ROW_KEY:0:2}/$RD_ROW_KEY"
  [ -s "$f" ] && cat "$f"
}

rd_cache_row_put() { # <row>
  _rd_cache_enabled || return 0
  local d="$RD_CACHE_DIR/rows/${RD_ROW_KEY:0:2}"
  mkdir -p "$d"
  printf '%s\n' "$1" > "$d/$RD_ROW_KEY.tmp.$$" && mv -f "$d/$RD_ROW_KEY.tmp.$$" "$d/$RD_ROW_KEY"
}

# rd_cache_score_key <avif> <img> <decoder_bin> <scorer_bin> <butter_bin_or_off>
rd_cache_score_key() {
  local avif="$1" img="$2" dec="$3" sc="$4" bt="$5"
  local bth="off"; [ -n "$bt" ] && [ "$bt" != "off" ] && bth=$(_rd_sha_file "$bt")
  RD_SCORE_KEY=$(printf 'score1|%s|%s|%s|%s|%s' \
    "$(sha256sum < "$avif" | cut -c1-40)" "$(_rd_sha_file "$img")" \
    "$(_rd_sha_file "$dec")" "$(_rd_sha_file "$sc")" "$bth" \
    | sha256sum | cut -c1-40)
}

rd_cache_score_get() { # → prints "ss b3 bmax", rc 0 on hit
  _rd_cache_enabled || return 1
  local f="$RD_CACHE_DIR/scores/${RD_SCORE_KEY:0:2}/$RD_SCORE_KEY"
  [ -s "$f" ] && cat "$f"
}

rd_cache_score_put() { # <ss> <b3> <bmax>
  _rd_cache_enabled || return 0
  local d="$RD_CACHE_DIR/scores/${RD_SCORE_KEY:0:2}"
  mkdir -p "$d"
  printf '%s %s %s\n' "$1" "$2" "$3" > "$d/$RD_SCORE_KEY.tmp.$$" \
    && mv -f "$d/$RD_SCORE_KEY.tmp.$$" "$d/$RD_SCORE_KEY"
}

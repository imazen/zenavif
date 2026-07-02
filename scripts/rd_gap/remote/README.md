# rd_gap remote sweep box (single Hetzner dedicated-CPU server)

Runs the [`../`](../README.md) RD-gap harness on one big Hetzner box
(`zenavif-sweep-1`, **ccx63**: 48 dedicated AMD vCPU, 192 GB RAM, EU) instead of
the contended local workstation. rsync + ssh, nothing fancier.

**Scale rule: this stays ONE box.** The moment you want N boxes, do NOT copy
this script per box — use the zenfleet job system
(`~/work/zen/zenmetrics/scripts/jobsys/`, `zenfleet-ctl declare` + workers),
which owns claim/retry/ledger/teardown for real fleets. One box + rsync is the
right scale for interactive RD iteration; a fleet is zenfleet's job.

## Cost — teardown discipline (READ THIS)

ccx63 is **~EUR 1.61/h gross ≈ EUR 39/day** (fsn1/nbg1/hel1, on-demand hourly).
A box left up idle is a defect:

- `./status.sh` shows state, uptime, and accrued cost, and **warns loudly >12 h**.
- `./teardown.sh --yes` deletes the box (salvaging un-fetched results first).
- Tear down when the day's sweeps are done. Re-provision + sync + build is
  ~15 min end-to-end, far cheaper than an idle night.

## Scripts

| script | what |
|---|---|
| `provision.sh` | create `zenavif-sweep-1` (idempotent), apt deps + rustup stable |
| `sync.sh` | rsync working trees + aom source + corpus subset (delta-fast) |
| `build_remote.sh` | build aomenc/aomdec (rev-stamped, skips when unchanged), cavif, zenavif examples, fast-ssim2-cli; verifies all binaries |
| `run_remote.sh` | run a harness command remotely, env prewired, stream log, auto-fetch OUT tsv into `results/<run-id>/` |
| `status.sh` | box state + hourly cost + uptime (loud >12 h warning) |
| `teardown.sh --yes` | delete the box (only ever the exact `zenavif-sweep-1`) |

Quickstart:

```bash
cd scripts/rd_gap/remote
./provision.sh && ./sync.sh && ./build_remote.sh
./run_remote.sh run_gap.sh                    # full sweep, both encoders
./run_remote.sh AOMENC= OUT=zr_only.tsv run_gap.sh          # zenrav1e only
./run_remote.sh AOM_CPU=0 OUT=aom_cpu0.tsv aom_only.sh      # libaom ref point
./status.sh
./teardown.sh --yes                           # WHEN DONE
```

`run_remote.sh` passes through any `VAR=value` prefix args (`QGRID_ZR`,
`CQGRID_AOM`, `AOMFMTS`, `AOM_CPU`, `AOM_EXTRA`, `SAMPLE`, `JOBS`,
`ZENRAV1E_SPEED`, ...). `AOMENC=` (empty) disables the libaom side. Relative
`OUT` names land in a per-run dir on the box and are fetched to
`remote/results/<run-id>/` locally (gitignored) — results never live inside the
synced repo trees, so re-syncs can't clobber them.

## What gets synced — and the mirrored-absolute-path design

`sync.sh` ships your **local working trees, including uncommitted changes** —
deliberately: the box tests exactly the WIP your checkout builds, without a
commit/push cycle. Synced: `~/work/zen/{ravif, zenrav1e, zenrav1e--phase2v2,
zenavif, zenanalyze, fast-ssim2, zenpixels, zencodec}` (minus `target/`,
`.git/`, `.jj/`, `fuzz/`, `benchmarks/`), `~/work/aom` source (build dirs
excluded), and every image referenced by `../sample_images.tsv`. That includes
zenavif's **gitignored `.cargo/config.toml`** (the `paths` overrides into
zenpixels/zencodec and the dev-only `[patch.crates-io]` zenrav1e →
`zenrav1e--phase2v2` entry) — local resolution overrides are part of the state
under test.

The box **mirrors local absolute paths** (`/home/lilith/work/...`,
`/mnt/v/output/...`) rather than relocating under `/root`, because:

- `ravif/Cargo.toml`'s dev-only `[patch.crates-io]` currently points at the
  **absolute** path `/home/lilith/work/zen/zenrav1e--phase2v2` — a `/root`
  layout would silently build the wrong encoder. If you repoint that patch at
  another workspace, add the workspace to `ZEN_REPOS` in `common.sh`.
- `sample_images.tsv` lists absolute `/mnt/v/...` corpus paths; mirroring means
  the TSV (and every other harness default, e.g. `WORK`) works verbatim with
  **zero path rewriting** on either end.

`zenanalyze` is synced because it is a path dev-dependency of zenavif (needed
to build the examples); `zenrav1e` proper is synced so the patch can be
repointed at it without touching the sync set. libaom is pinned to
`632172a468f5...` (see `../../../docs/RD_GAP_VS_LIBAOM.md`); `sync.sh` refuses
to sync a drifted local aom checkout unless `ALLOW_AOM_DRIFT=1`.

Ad-hoc sample TSVs: `./sync.sh /path/smoke3.tsv` syncs the TSV to
`/home/lilith/sweep_in/smoke3.tsv` on the box **and** the images it references
(at their mirrored paths), then run with `SAMPLE=/home/lilith/sweep_in/smoke3.tsv`.

**Decoder fallback (loud, provenance-stamped):** the zenavif decode examples
sometimes don't build from the WIP tree (e.g. a sibling-repo contract change
mid-flight in zenpixels/zencodec — exactly what happened 2026-07-02).
`sync.sh` therefore also ships the current **local** `save_png`/`extract_av1`/
`decode_avif` binaries; `build_remote.sh` tries the source build first and only
on failure copies the synced binaries into place, printing a huge warning plus
their sha256+build dates, and stamps `target/release/examples/
.synced_from_workstation` so the state is always inspectable. No silent
half-working states: no source build AND no fallback = hard fail. `sync.sh`
also prints which zenrav1e tree ravif's `[patch.crates-io]` pointed at when
the sync ran — concurrent sessions toggle that patch for A/B measurements, so
every sweep should record it.

## Credentials

The Hetzner API token stays in `~/.config/hetzner/credentials` on the
workstation — the scripts load it into `HCLOUD_TOKEN` per-invocation and it is
**never printed, committed, or synced to the box** (the box needs no hcloud
access). SSH uses the existing `zen-arm-dev-20260528` hcloud key
(`~/.ssh/zen-arm-dev`). Every script fails loudly if the token, key, or box is
missing — nothing half-works.

## Measured (2026-07-02, ccx63 fsn1, first bring-up)

- provision (create + apt + rustup): **1m28s**. Full sync: **36–42s** (~66 MB
  wire); re-sync deltas are seconds. Cold build: **~2m20s** total (aom 33s,
  cavif 36s, zenavif examples 47s, fast-ssim2-cli 25s at `-j48`); unchanged
  re-run ≈ 28s.
- **Outputs are bit-identical to local** — the 3-image × Q{60,80} smoke
  reproduced local `bytes`/`bpp`/`ssim2` to the last digit. The harness is
  deterministic across machines; only timing differs.
- **Per-core the box is SLOWER than the workstation**: per-cell cavif enc_ms
  was 0.72–0.79x of local (Hetzner EPYC clocks vs a water-cooled 7950X). The
  box wins on **width + isolation**, not clock: `JOBS=22` (every corpus image
  in parallel, no `nice`, no contention with local agent sessions) vs `JOBS=6`
  under `run-heavy` locally.
- Full 22-image 12-q zenrav1e-only sweep, `JOBS=22`: **11m14s wall measured**
  (264 cells, Σ enc 87.9 min ⇒ 7.8x effective parallelism). This sweep shape is
  **straggler-bound**: the harness runs each image's q-cells serially, and the
  family-7 plots (o_7002 ≈ 11 min serial chain) set the wall — so for a single
  zenrav1e-only sweep the box mainly buys *isolation* (the workstation stays
  free) rather than wall-clock. Width pays directly on the full both-encoder
  grid (44 cells/image), on `AOM_CPU=0` reference runs, and by running
  **several config sweeps concurrently** — the box has huge headroom during
  the straggler tail (load ~2 on 48 cores), so stack sweeps rather than
  leaving it idle.
- The harness itself is unchanged — `run_remote.sh` just sets `CAVIF`,
  `SAVE_PNG`, `SCORER`, `AOMENC`, `AOMDEC` to the box's builds and `JOBS=22`.
- provision-time apt installs are fine here (one interactive pet box, not a
  fleet image; fleet images must bake dependencies — see global CLAUDE.md).

## Iteration speed (added 2026-07-02)

**Deterministic cell cache** (`../cell_cache.sh`, wired into both cell scripts):
row-level (encoder-binary × image × knobs → full row; hit skips everything) +
score-level (avif-bytes × decoder × scorers → ssim2/butteraugli; hit skips
decode+score — shares scores across arms when a gated knob produces identical
bytes on some content). Auto-enabled when `/home/lilith/sweep_cache` exists
(create it on the box once; it lives inside the disk snapshot so the cache
SURVIVES teardown/restore). ~1000× on a row hit (measured 18.5s → 0.017s).
Key includes every `ZENRAV1E_*/ZENRAVIF_*/RAV1E_*/AOM_*` env var + binary +
image content hashes; declare ad-hoc knobs via `RD_CACHE_EXTRA`. **Timing
sweeps must set `RD_CACHE=off`** — cached rows replay the original enc_ms.
Practical effect: re-running a baseline arm is free; aom baselines never
re-encode; a failed sweep re-run only redoes missing cells.

**Two-stage fitting grids**: for multi-arm FITTING, rank arms on a coarse grid
first (`QGRID_ZR="30 50 60 75 85 95"` = half the cells; BD ordering is stable
with 6 points), then confirm ONLY the winner on the full 12-point grid.
Final landing verdicts always use the full grid.

**Arm-parallel boxes**: `BOX_NAME=zenavif-sweep-2 FROM_SNAPSHOT=auto
./provision.sh` brings up a second box from the same snapshot in ~4 min;
`BOX_NAME=... ./run_remote.sh ...` targets it. Cost per cell is identical
(you pay cell-seconds either way) — wall-clock divides by the box count.
Same teardown discipline per box.

# Variant generation in zenavif

Written 2026-06-10, adopting the patterns from zenjpeg's
`docs/VARIANT_GENERATION.md` (the reference write-up; read it first —
this document records only the zenavif-specific audit, decisions, and
deltas). Adoption order followed the recommendation there:
discriminate knobs → `resolve_plan` → fingerprints → sweep planner →
exact trials.

## Where things live

- `EncoderConfig::resolve_plan(PlanInput) -> EncodePlan` +
  `validate()` / `validate_for_input()` — `src/encode_plan.rs`,
  `src/validation.rs` (stable surface, feature `encode`).
- Sweep planner + byte-identity `fingerprint` — `src/sweep.rs`
  (calibration surface, feature `__expert`).
- Encode-level contracts — `tests/encode_contracts.rs` (feature
  `encode`).
- Empirical axis validation — `examples/sweep_validate.rs`
  (feature `__expert`; TSVs in `benchmarks/sweep_validate_*.tsv`).

## The knob audit (dominance / trial / metric)

Every encode knob, classified per the zenjpeg taxonomy:

| knob | class | notes |
|---|---|---|
| quality | metric | fully mediated by the resolved quantizer — q 80.0 ≡ q 80.2 (quantizer 71), byte-proven in `encode_contracts` |
| alpha_quality | metric | alpha-plane quantizer. **Unset follows the color quality** (fixed 2026-06-10; zenravif's default silently pinned it to the q80 equivalent). Swept as a **delta against the grid q** via `KnobProbe::AlphaQualityDelta` (±25, `modes_full_alpha`) — a delta, not an absolute, per zenjpeg's `chroma_quality` lesson |
| alpha_color_mode | metric | pixel-changing on alpha content (Clean rewrites color under transparency, Premultiplied rescales). Probed in `modes_full_alpha`; reported by `EncodePlan::alpha_color_mode` |
| speed | metric | drives the `SpeedTweaks` table (partition range, CDEF/LRF gates, tile floor) |
| qm | metric | ~10 % BD-rate win on stills; default on; forced off by lossless |
| vaq / vaq_strength | metric | strength is **inert when vaq is off**, and **strength 1.0 is a structural no-op even when vaq is on** — the psychovisual/still tunes always compute the activity mask and zenrav1e skips the rescale at 1.0 (`api/internal.rs:1379`). The harness's first run caught the 1.0 axis value as an inert step; the sweep axis is `Option<f64>` now so the no-op spelling can't be curated by accident, and the fingerprint hashes the *active* form |
| tune_still_image | metric | live at low q where CDEF/deblock engage; checked for inertness every harness run |
| lossless | metric | pins quantizer 0, gates QM off |
| seg_boost | metric | 1.0 = off; bounds 0.5–4.0 from zenravif validate |
| trellis | metric | zenrav1e Viterbi DP, default off |
| chroma_subsampling | metric | **new knob** — was hardwired 4:4:4 before; 4:2:0 invalid × RGB model and × 16-bit input (validate rejects) |
| bit_depth | metric | Auto resolves per input bitness (8-bit→8, 16-bit→10) |
| color_model | metric | YCbCr→BT.601 vs identity RGB. **Ignored on the 16-bit entry points** (always identity RGB there — current-implementation fact reported by the plan) |
| pixel_range | metric | full vs limited signal mapping |
| alpha_color_mode | metric | content-dependent (Clean rewrites color under transparency) |
| threads | **metric, surprisingly** | tile count = `min(threads, w·h / min_tile_size²)`; **threads=None substitutes the host's core count → default encodes are not byte-reproducible across machines.** Sweep cells pin `Some(1)` |
| rotation/mirror/CICP cp+tc/CLL/mastering/ICC/EXIF/XMP/gain map | dominance | pure container metadata: present ⇒ emit, no pixel change, no trial needed |
| matrix_coefficients | **dead** | no available backend reads the field — zenravif derives the signaled matrix from `color_model`, and its only reader was the deprecated svtav1 path. Documented on the setter, excluded from the fingerprint (byte-proven), deliberately set-but-informational in the zencodec wrapper for config coherence |
| backend | structural | `Svtav1` is `#[deprecated]` and rejected by `validate()` (previously a silent fallback to zenravif); see open items |
| override_cdef / rdo_tx / sgr / lru_on_skip / seg_complex / bottomup / partition_range / complex_pred / lrf / fast_deblock | metric | preset-derived; an override equal to its preset value is an alias (fingerprint merges it — byte-proven for CDEF) |

**Exact trials: none.** The zenjpeg doc predicted "OBU/layout-level
only (most knobs are metric-class)" and the audit confirms it: the AV1
payload comes from a single encoder invocation, the container layout
is fixed by zenavif-serialize, and no knob produces pixel-identical
alternative bytestreams worth `min(bytes)`-ing. The one built-in
dominance-style decision — dropping an all-opaque alpha plane — already
lives in zenravif and is content-dependent (the plan documents it
rather than guessing).

## The fingerprint, and what it excludes

`sweep::fingerprint` hashes RESOLVED state: backend, quantizers (color
+ alpha), lossless, bit depth, color model, subsampling, pixel range,
alpha mode, qm/vaq/tune/seg_boost/trellis after gates, the speed preset
plus every speed-derived setting **after overrides** (for both the
color and alpha quantizer derivations), threads, and all container
metadata by content.

Exclusions (each one byte-proven in `sweep_validate`, per the
"every exclusion must be proven by encode" rule):

- raw `quality` / `alpha_quality` (mediated by quantizers),
- `vaq_strength` when VAQ is off — and the whole VAQ knob at
  strength 1.0 (the structural no-op; see the audit table),
- `matrix_coefficients` on the zenravif backend,
- override `Option`s (mediated by the resolved speed-derived values).

## What the harness's first run caught (2026-06-10)

Both findings are the zenjpeg pattern playing out as designed — claims
about encoder behavior that read plausibly and were wrong or
incomplete until an encode voted:

1. **`with_vaq(true, 1.0)` is byte-identical to VAQ off** across 24
   encode comparisons. Root cause is structural: zenravif always
   encodes under `Tune::Psychovisual`/`StillImage`, which compute the
   activity mask regardless of `enable_vaq`; the knob's only
   incremental effect is the strength rescale, skipped at 1.0
   (zenrav1e `api/internal.rs:1369–1381`). This also sharpens the old
   "VAQ hurts stills" benchmark note — at the default strength the
   knob does nothing at all; any measured effect was at non-1.0
   strengths. Fixed by making the sweep axis `Option<f64>` and
   teaching the fingerprint the alias.
2. **`LruOnSkip(true)` never changed bytes** — first flagged on the
   photo/noise/checker/gradient corpus (24/24), then re-tested with a
   mostly-flat web-graphic synthetic (`flatlogo256`) added precisely
   to produce skip-heavy loop-restoration units (`lru_on_skip` gates
   the LRU search on all-skip units, zenrav1e `rdo.rs:2410`) — still
   inert, 28/28. On intra-only still images the searched-anyway units
   resolve to the same restoration decisions. Removed from the curated
   `modes_full` probes with the evidence recorded in the provenance
   table; `KnobProbe::LruOnSkip` remains for explicit speed ≤ 1 sweeps
   (the only region where the preset enables it).

## The zenravif mirror problem

zenjpeg's pattern 3 ("introspection calls the same function") cannot
fully hold across the crate boundary: zenravif keeps
`quality_to_quantizer` and `SpeedTweaks::from_my_preset` as
`pub(crate)`. zenavif mirrors them (provenance comments cite zenravif
0.1.3 source lines) and pins the mirrors **by encode**:

- `tests/encode_contracts.rs::quality_is_mediated_by_quantizer` —
  the alias pair q 80.0 ≡ q 80.2 ≠ q 81.0 at the byte level;
- `sweep_validate`'s override==preset pairs (CDEF at low-q) and the
  tiles/threads checks.

If a zenravif bump changes the curve or the tables, those fail loudly.
**Follow-up:** expose the resolution from zenravif itself (target
0.1.4) and delete the mirrors.

## Reproducibility rule for sweeps

Pin `threads(Some(1))` on every sweep cell (the planner does this for
you). Rationale: tiles derive from threads with the host core count as
the unset default, and tile structure changes the bitstream. Single
tile additionally serializes each encode so sweeps parallelize cleanly
*across* cells. The fingerprint hashes threads raw and never merges
`None` with anything.

## MLP / picker training contract

`sweep::feature_columns()` + `SweepCell::feature_row(PlanInput)` are
the training-side bridge to zentrain: one numeric column per knob,
**resolved** where a mediator exists — `quantizer` rather than raw
quality, the post-override speed-derived search settings rather than
the `Option` override spellings. A model trained on resolved values
generalizes across the aliases the fingerprint merges instead of
learning that q 80.0 and q 80.2 are different inputs. Columns are
append-only across versions; encodings (bool 0/1, enum small-int,
−1 sentinels) are documented on `feature_row`. The intended row shape
for a training table is
`(image_id, cell.id, cell.fingerprint, feature_row…, bytes, metric…)`.

Alpha-bearing corpora use `SweepAxes::modes_full_alpha()` — the alpha
probes are kept out of `modes_full` because they are byte-inert without
an alpha plane and would trip the RGB harness's inert-step check. The
harness validates them on a dedicated RGBA leg instead: each alpha
probe must change bytes on alpha content AND leave color-only encodes
byte-identical (no coupling into the color path).

## Harness sizing

AV1 encodes cost 100–1000× a JPEG encode, so the harness uses 256²
crops/synthetics + one 64² tiny, a 4-point explicit q grid
{10, 30, 60, 85} (low-q included per the sweep discipline), and the
dev≤1 stratum subset — minutes, not hours, while still encoding every
single-deviation label. Full grids belong to the real sweep
infrastructure (zenmetrics fleet), not the validation harness.

## Open items

- zenravif 0.1.4: expose quantizer/speed resolution publicly; delete
  the mirrors here.
- 16-bit entry points ignore `color_model` (always identity RGB).
  Honoring YCbCr there would change bytes — queued as a deliberate
  decision, not slipped in.
- The harness's RGBA leg validates alpha-probe *liveness* only; a full
  alpha RD sweep (modes_full_alpha over an RGBA corpus with per-cell
  alpha-aware metrics) belongs to the real sweep infrastructure.
- The svtav1 backend is **deprecated** (2026-06-10): the
  `encode-svtav1` feature was never shipped (svtav1-rs produces
  non-conformant bitstreams; the draft path returned raw OBUs as
  `avif_file`), so the cfg'd encode path and its three differential
  test files were removed — git history (pre-deprecation) has them if
  the experiment resumes. `Av1Backend::Svtav1` carries `#[deprecated]`
  and `validate()` rejects it unconditionally; a working svtav1
  integration would land as a new variant that wraps OBUs in a real
  AVIF container.
- `EncodePlan` reports the **request** (e.g. tiles asked of zenrav1e);
  rav1e may quantize tile counts to powers of two internally. The
  fingerprint is unaffected (equal request ⇒ equal outcome) but plan
  readers should treat `tiles` as the requested value.

# AVIF backend-tuner bake contract

**Status:** proposed by the wiring lane, 2026-09-04. This is what
`zenavif::AvifTuner::from_bytes` validates and refuses on **today**
(`src/backend_tuner/contract.rs`, tests in the same file). If the
training lane needs a different shape, change it here and in that module
together — the loader is the enforcement, this file is the agreement.

The runtime carries **no bundled weights**. The bake is always supplied
by the caller. An opt-in `bundled-bake` feature would be a separate,
user-gated proposal.

---

## 1. Wire format

ZNPR **v3** (header byte 4 = `0x03`), produced by `zenpredict-bake` —
the canonical serializer. Ad-hoc emitters are banned.

`AvifTuner::from_bytes(&[u8])` parses; `from_bytes_with_schema` also
pins `schema_hash` before any section parsing.

## 2. Inputs

| # | what | note |
|---|---|---|
| n | source features, by name | zenanalyze features, **source-only** — reproducible at encode time |
| 1 | `zq_norm` | the caller's target quality ÷ 100 |

**Total width must equal `Model::caller_input_width()`, not
`n_inputs()`.** They differ on a dead-column-pruned bake (a pruned model
declares `FeatureTransform::Drop` on the dead lines and still takes the
full caller width). The loader checks `caller_input_width`.

**No encoder `q` as an input.** `q` is part of the decision the tuner
makes, so feeding it back in would leak the answer. Same convention
`zenpicker`'s cell contract uses.

**Feature names** are zenanalyze snake_case, optionally `feat_`-prefixed.
The runtime resolves them two ways and the bake needs to work under both:

- **Offer reuse** — when the caller passes a `zenanalyze_api::Offer`, the
  tuner qualifies each column via
  `zenanalyze::versioning::feature_version_hash_by_name` and calls
  `Offer::reuse_for`. A column this build cannot qualify makes the whole
  offer unusable (the tuner falls back rather than zero-filling).
- **Own pass** — otherwise it runs `analyze_features_rgb8` over exactly
  the declared columns. A column no zenanalyze feature matches is a
  **loud error**, never a silent zero.

⇒ **Declare features that exist in `FeatureSet::SUPPORTED`.** If the
trainer wants Offer reuse to work across zenanalyze versions, emit
fully-qualified `name@hash8` columns (that is what makes the offer
version-safe); bare names still work, they just always take the own pass.

## 3. Outputs

`n_outputs == cells × heads`.

### Heads — `zenavif.tune.heads`

Comma- or newline-separated, in order.

| head | required | meaning |
|---|---|---|
| `bytes_log` | **yes** | `ln(encoded_bytes)`. The objective the pick argmins over. |
| `quality` | no | the encoder `quality` for this cell at this target. Absent ⇒ the caller's target passes through on the encoder's generic scale. |
| `encode_ms_log` | no | `ln(encode_wall_ms)`. Absent ⇒ the time budget masks on the measured table in `backend_tuner::stub` instead. |

### Layout — `zenavif.tune.layout` (optional)

`cell_major` (default): `out[cell * n_heads + head]`.
`head_major`: `out[head * n_cells + cell]`.

## 4. Cells — `zenavif.tune.cells`

**One label per line** (newline-separated; labels contain commas).

```
backend[,key=value]*
```

```
rav1e,chroma=444,speed=4,tune=still,qm=0
svt,chroma=420,speed=6,svttune=3,qm=1,qmmin=2,qmmax=10
aom,chroma=420,speed=6
```

Backends: `rav1e` | `zenravif` | `zenrav1e` · `svt` | `zenav1svt` |
`zenav1-svt` · `aom` | `zenav1aom` | `zenav1-aom`.

### Knobs are backend-scoped

| knob | backends | maps to |
|---|---|---|
| `speed=0..=10` | all | `EncoderConfig::speed` |
| `chroma=420\|444` | all | `EncoderConfig::chroma_subsampling` |
| `depth=8\|10\|12` | all | `EncoderConfig::bit_depth` |
| `qm=0\|1` | rav1e | `with_qm` (no window on this backend) |
| `tune=still\|psycho` | rav1e | `with_still_image_tuning` |
| `qm=0\|1`, `qmmin=0..=15`, `qmmax=0..=15` | svt | `SvtParams::{enable_qm, min_qm_level, max_qm_level}` |
| `svttune=<u8>` | svt | `SvtParams::tune` |
| `scm=<u8>` | svt | `SvtParams::force_screen_content_mode` |
| `sharp=<i8>` | svt | `SvtParams::sharpness` |

**A knob on the wrong backend is refused, not ignored.** `tune` and
`svttune` are different knobs on different encoders and the grammar keeps
them apart deliberately.

### Two refusals worth knowing before you emit cells

1. **A cell declaring svt knobs needs `__expert`.** `with_svt_params` is
   gated on that (unstable) feature. On a build without it, such a cell
   is a load-time error — the tuner will not encode with the knobs
   silently dropped.
2. **Duplicate configurations are refused.** Two labels that resolve to
   the same config (e.g. `rav1e,speed=6` and `zenravif,speed=6`) fail the
   load rather than minting two cells that mean one thing.

Both rules exist because of what the DOE found in its own harness: `tune=0`
and `screen_content_mode=Some(3)` carried distinct configuration
fingerprints while emitting **byte-identical bitstreams** — the knob
reached the fingerprint, not the encoder — and 8,972 cells were spent
before anyone noticed ([imazen/zenav1-svt#17](https://github.com/imazen/zenav1-svt/issues/17)).
A grid should **byte-check its level-0 arms against the control** before
spending cells on them.

## 5. What the campaign says about which cells are worth having

From `zenmetrics/benchmarks/`: `avif_doe_stageA_2026-09-02.md`,
`avif_backend_selection_2026-09-03.md`,
`avif_eradelta_analysis_2026-09-03.md`,
`avif_speed_instrument_2026-09-03.md`. Sign convention, verbatim:
**"NEGATIVE BD-rate = the arm needs FEWER bits at matched quality = the
arm WINS."** Quality matched on **ssim2**.

- **`scm=3` is speed-7-only.** Byte-identical to the control at speeds 4
  and 6 (288/288 cells at each). At speed 7 it fires on 90/288 cells (10
  of 32 images) and is worth a median **−50.08%** *where it fires*; the
  corpus-wide median is exactly **0.0000%**. **A corpus-median gate will
  read the DOE's largest single-knob win as dead.** Score it conditional
  on content, or lose it.
- **The winning class is plot / screenshot / scan, not "synthetic".**
  AI-generated content shows **0 divergence on 81/81 cells** — it behaves
  like photo. Per class where it fires: scan −87.53, plot −66.93,
  screenshot −25.99.
- **`svttune=3` is a per-image decision.** Largest single main effect
  (−7.69% native s6) and the most variable knob in the block: 8 of 30
  images regressed, worst +19.8%, and at speed 4 the CI crosses zero.
  This is exactly what a bake should be picking per image.
- **QM's benefit grows with preset** (−0.29% s4 / −2.59% s6 / −4.89% s7
  for `qmmin=2,qmmax=10`) but the axis is **categorical, not ordinal** —
  `qmmin=8,qmmax=15` reads only −0.66%. Do not interpolate windows. QM
  also moves **plots the opposite way** (+1.20% at s6).
- **QM × sharpness is a real synergy, speed-6 only.** `sharp=7` costs
  **+7.02%** alone but the joint with `qmmin=2,qmmax=10` reads **−0.03%**
  — a **−4.70 pp** residual (CI [−5.80, −3.99], 26/30 images, p 5.9e−5).
  Neither belongs in a cell without the other.
- **Backend and chroma are totally confounded in the source data.** Every
  svt cell in the campaign is 4:2:0 and every zenravif cell is 4:4:4 —
  verified by reading `av1C` out of 1,114 bitstreams, zero exceptions —
  because the sweep pins 4:2:0 only for svt. So the campaign's
  backend comparison is a *(backend × chroma)* comparison. A zenrav1e
  4:2:0 arm is the #1 ranked data gap and has never been run.
- **svt-as-configured cannot reach ssim2 90 on 16 of 32 references** at
  any q or speed (6/6 plots, 5/5 screenshots, 0/7 photos); zenravif
  misses on 1 of 32. Reach, not bytes, is what separates them at high
  targets.
- **zenav1-aom has no measured knob, RD, or speed number anywhere.**
  Block A3 was never declared. Emitting `aom` cells means training on
  data that does not exist yet.

## 6. Wall time, if you emit an `encode_ms_log` head

The runtime's fallback table is transcribed from
`/mnt/v/output/avif-speed-instrument-2026-09-03/speed_alpha_beta.tsv`
(sha256 `c7f63157de85c68527c949ffa4fa1d797dfead4606774a5f1160ce28012837e7`),
20 `(backend, speed)` rows of `alpha + beta * MP`. A trained head should
beat it, because that table's own instrument says:

- the **pooled** fit is wrong by up to **24.3×** per image (`beta` spreads
  1.95×–24.33× across sources) while per-source fits are clean
  (median R² 0.9928–0.9997) — every one of the 20 arms is flagged
  `linear_model_failed = True, POOLING_NOT_MODEL`;
- **`alpha` is load-bearing** — a bare ms/MP misprices small images ~20×;
- it is **q45-only** (q-flatness was falsified: 75.1% spread on svt), it
  is **wall** time not CPU time (svt threads to ~1.638 cores at native),
  and the absolute values are **r7900x-only** — ratios travel, absolutes
  do not;
- **knob time is not measured at all**, and no DOE cell carries a
  duration (`encode_ms` is not persisted in the fleet path).

Speed is feature-conditioned. A head that conditions on the image is the
point of having one.

## 7. Swapping the real bake in

One step, plus the gate that proves it:

```rust
// before
let tuner: Box<dyn AvifTuning> = Box::new(zenavif::StubTuner::new());
// after
let tuner: Box<dyn AvifTuning> = Box::new(zenavif::AvifTuner::from_bytes(&bake_bytes)?);
```

Then re-run the integration gates against the real model:

```sh
cargo test -p zenavif --features auto-tune,encode,encode-imazen \
    --test backend_tuner_integration
```

`tests/tiny_bake.rs` is the hand-baked two-cell stand-in that keeps the
model path gated until then; point
`a_contract_carrying_bake_drives_the_model_path_end_to_end` at the real
bake and it becomes the real acceptance test. `AvifTune::source()`
reports `Model` vs `Stub`, so a consumer can assert which one answered.

## 8. Checklist for the training lane

- [ ] ZNPR v3 via `zenpredict-bake`
- [ ] `zenavif.tune.cells` — one label per **line**, backend-scoped knobs, no duplicate configs
- [ ] `zenavif.tune.heads` — includes `bytes_log`
- [ ] `zenavif.tune.input_order` — every source feature once + `zq_norm` once, length == `caller_input_width()`
- [ ] `zenavif.tune.layout` if not `cell_major`
- [ ] feature names resolvable in `FeatureSet::SUPPORTED` (qualified `name@hash8` if Offer reuse matters)
- [ ] `zentrain.repro` embedded (input shas, seed, argv, trainer HEAD) per the standing rule
- [ ] level-0 arms byte-checked against the control before spending cells

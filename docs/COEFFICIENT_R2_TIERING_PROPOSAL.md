# Coefficient artifact-tiering + Cloudflare R2 + Vast.ai — proposal

**Status:** draft 2026-04-30. Read-only proposal; the actual PR lands in
`~/work/coefficient/` and is blocked on the prior agent's uncommitted
work being resolved (4 modified files + new untracked .md/.rs from the
`claude-meta-train v0.6` session).

**Audience:** whoever ends up merging this. Designed for a multi-PR
landing rather than one mega-PR.

## The cost problem

R2 charges:
- **$4.50 / million Class A operations** (writes, lists)
- **$0.36 / million Class B operations** (reads)
- **$0.015 / GB-month** storage
- **Free egress**

Coefficient's existing storage layer (`src/store/`) writes one R2 object
per artifact. At our target scale of 10M+ encoded images:

| Naive per-blob | Cost |
|---|---|
| 10M Class A writes | **$45** |
| 10M Class B reads (every train run scans them) | $3.60 |
| ~50 GB blob storage | $0.75/mo |

That's already on the edge of acceptable — and breaks down completely if
we want to scale to 100M (= $450 in writes), or if a worker bug retries
writes 10× ($450 of stutter).

## The artifact-tier insight

We have three categorically different artifact populations:

| Tier | Audience | Cardinality | Per-record size | Visibility |
|---|---|---|---|---|
| **Oracle** | Picker training only | 10M+ | ~100 B (metrics: bytes, encode_ms, zensim, ssim2, butteraugli) | private |
| **Coefficient browser** | Pareto exploration UI | ~100 K – 1 M | encoded blob only (~5–500 KB); source + thumbnails decoded client-side | public |
| **Squintly** | Public demos | ~1 K – 10 K | encoded blob only (~100 KB – 5 MB); source from sources/ bucket | public |

**Decoded reference images are never stored.** Zensim, ssim2, and
butteraugli are computed in-worker from in-memory decoded RGB; the
decoded buffer is dropped on flush. The browser UI displays
`<img src="r2://sources/{sha256}.{ext}">` and
`<img src="r2://browser/{encoding_id}.avif">` side-by-side — the browser
does the decode natively. No PNG round-tripping, ever.

This simplifies tier sizing: the **browser tier** stores only the
encoded artifact (the same bytes the encoder produced; AVIF/WebP/JXL).
The **source bucket** is one-write-per-source, separate from the per-
encoding stream.

**Oracle is 99 % of the volume but 0.1 % of the byte cost.** Aggregating
oracle records into Parquet/JSONL chunks and writing one chunk per
~10 K records collapses 10 M Class A operations to ~1 000.

| Tiered design | Cost |
|---|---|
| Oracle: 10 M records → 1 K Parquet chunks (10 K rows each) | **$0.0045** in writes, ~50 MB Parquet @ $0.0008/mo |
| Browser: 1 M blobs (per-blob, public-bucket) | **$4.50** writes, ~250 GB @ $3.75/mo |
| Squintly: 10 K blobs (per-blob, public-bucket) | **$0.045** writes, ~10 GB @ $0.15/mo |
| **Total** | **~$5 in writes, ~$4/mo storage** |

10× cheaper than naive per-blob, and scales linearly to 100 M without
the write-cost cliff.

## Architecture overview

Three new modules in `coefficient/src/store/`, plus one new module in
`coefficient/src/cloud/`, plus a thin tier-policy layer.

```
src/store/
├── r2.rs              (NEW) — Cloudflare R2 backend (S3-compat, mirrors spaces.rs)
├── batched_row.rs     (NEW) — Buffers MetricRecords in memory, flushes Parquet/JSONL chunks
├── tiered.rs          (NEW) — Routes per-artifact to the right backend
├── public.rs          (EXTEND) — Add R2-custom-domain URL pattern
├── layout.rs          (UNCHANGED) — Hash-prefix layout reused by all backends
└── mod.rs             (EXTEND) — Re-exports + feature gates

src/cloud/
└── vastai_batch.rs    (NEW) — Vast.ai BatchApi impl with prepay-balance check

src/planner/
└── manifest.rs        (EXTEND) — Add `tier_policy` field to JobConfig

config/
└── coefficient.toml   (EXTEND) — [r2], [vastai], [tier_policy] sections
```

### 1. `R2Store` — drop-in S3-compat backend

Mirrors `src/store/spaces.rs` (which uses `aws-sdk-s3` against the DO
endpoint). R2 is also S3-compat, so this is ~250 lines of code that's
mostly endpoint configuration.

```rust
pub struct R2Config {
    pub account_id: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub bucket: String,
    pub public_base: Option<String>, // e.g. "cdn.imazen.dev/zenavif"
}

impl R2Config {
    pub fn endpoint_url(&self) -> String {
        format!("https://{}.r2.cloudflarestorage.com", self.account_id)
    }
}

pub struct R2Store { /* AWS SDK S3 client + config */ }
impl ResultStore for R2Store { /* per-blob CRUD, identical to SpacesStore */ }
```

Feature flag: `r2 = ["dep:aws-sdk-s3", ...]` (same deps as existing
`spaces` feature; can share if we want).

R2 supports **custom domains via Cloudflare** for public access (no S3
signed URLs needed). The `public_base` field hands those URLs to the
existing `PublicUrlConfig` path (`src/store/public.rs`).

### 2. `BatchedRowStore` — the cost-saver

Buffers metric/encoding rows in memory; flushes on **either**
`max_rows` or `max_age` (whichever first). On flush, writes one
Parquet (or JSONL.gz) chunk to the underlying ResultStore.

```rust
pub struct BatchedRowStore<S: ResultStore> {
    inner: S,
    buffer: Mutex<Vec<MetricRecord>>,
    max_rows: usize,    // default 10_000
    max_age: Duration,  // default 5 min
    flush_thread: JoinHandle<()>,
    chunk_index: AtomicU64,
}
```

Key API additions:

```rust
impl<S: ResultStore> BatchedRowStore<S> {
    pub fn metric_put_batched(&self, record: MetricRecord) {
        // Just append to buffer; flush thread handles I/O.
    }

    pub fn flush(&self) -> Result<()> { /* drain + write chunk */ }
}
```

Read path: `metric_get(id)` reads any chunk that contains the row (we
maintain an in-memory index keyed by record id → chunk path, persisted
to a sidecar `chunks_index.json`). For analysis, downstream tools just
read all the Parquet chunks under `metrics/oracle/` directly.

**Format choice: Parquet over JSONL.gz**, because:
- Pareto-front extraction queries (most of what coefficient does) are
  column-oriented — Parquet is 5–10× faster
- Polars / pandas / DuckDB read Parquet natively
- Compression ratio is better (~3× over gzip JSONL for our schema)
- Adds `arrow2` or `parquet` dep, ~3 MB binary cost

### 3. `TieredResultStore` — the routing layer

```rust
pub struct TieredResultStore {
    oracle: Box<dyn ResultStore>,    // typically BatchedRowStore<R2Store>
    browser: Box<dyn ResultStore>,   // typically R2Store with public bucket
    squintly: Box<dyn ResultStore>,  // typically R2Store with public bucket
    policy: Arc<dyn TierPolicy>,
}

pub trait TierPolicy: Send + Sync {
    fn tier_for_encoding(&self, record: &EncodingRecord) -> Tier;
    fn tier_for_metric(&self, record: &MetricRecord) -> Tier;
}

#[derive(Copy, Clone)]
pub enum Tier { Oracle, BrowserPublic, SquintlyPublic }

impl ResultStore for TieredResultStore {
    fn encoding_put(&self, rec: EncodingRecord, blob: Vec<u8>) -> Result<()> {
        match self.policy.tier_for_encoding(&rec) {
            Tier::Oracle => {
                // Skip blob; metric-only.
                // Caller emits metric with the bytes/time fields embedded.
                Ok(())
            }
            Tier::BrowserPublic => self.browser.encoding_put(rec, blob),
            Tier::SquintlyPublic => self.squintly.encoding_put(rec, blob),
        }
    }
    // metric_put: always goes to oracle (BatchedRowStore) regardless of tier;
    // only the BLOB tiering changes which bucket the encoded image lands in.
}
```

`TierPolicy` is pluggable. The default policy (`StandardTierPolicy`):

- **SquintlyPublic**: `encoding.tags.contains("squintly")` (curated set)
- **BrowserPublic**: pareto-near at any of {q=60, q=75, q=85, q=90, q=95}
  AND content-class diversity-sampled
- **Oracle**: everything else

Configurable via `coefficient.toml [tier_policy]` so we can change
ratios without rebuilding workers.

### 4. `VastAiBatchApi` — prepay-safe provider

Implements existing `BatchApi` trait. Two changes vs the GCP/DO impls:

1. **Pre-submit balance check**: `GET /api/v0/users/current/` returns
   account balance. Before submitting a job, estimate cost
   (`manifests.len() × estimated_hours × $/hr`) and refuse if balance
   is below cost + 20% buffer. Returns
   `Err(InsufficientCredits { balance, required })`.

2. **Per-machine auto-stop**: passes `--auto-stop-after-hours N` to the
   rented box's startup script, in case the prepay safety net fails.
   Belt to the prepay suspenders.

The job submission path:
- Find a matching offer via `/api/v0/bundles/` (filter on cores, ram,
  disk, datacenter, reliability ≥ 0.95, verified=true)
- Bid via `/api/v0/asks/{id}/`
- Pass cloud-init that:
  - Clones the repo / pulls the worker image
  - Starts `cloud_worker` with the manifest URL
  - Self-terminates via `/api/v0/instances/{id}/` on completion

Manifests live on R2 (same as artifacts). No GCS dependency for the
Vast.ai path.

Feature flag: `vastai = ["dep:reqwest", "r2"]` (no `gcs` requirement).

### 5. Manifest extension

```rust
pub struct JobConfig {
    pub storage: StorageConfig,    // existing
    pub tier_policy: TierPolicyConfig, // NEW
    pub skip_completed: bool,
    pub continue_on_error: bool,
}

pub struct TierPolicyConfig {
    pub policy_name: String,        // "standard", "all_oracle", "explicit"
    pub squintly_tags: Vec<String>, // ["release-2026-q2", "showcase"]
    pub browser_q_set: Vec<u8>,     // [60, 75, 85, 90, 95]
    pub browser_pareto_threshold_pct: f32, // 5.0
}
```

Backwards-compatible: missing field defaults to "all-oracle" so old
manifests still work and produce only metric records.

## Where each piece lives in the codebase

| New / changed | Path | Estimate |
|---|---|---|
| `R2Store` | `src/store/r2.rs` | ~280 lines, ~½ day |
| `BatchedRowStore` | `src/store/batched_row.rs` | ~400 lines, ~1 day (Parquet schema, flush thread, chunks index) |
| `TieredResultStore` + `TierPolicy` | `src/store/tiered.rs` | ~250 lines, ~½ day |
| `PublicUrlConfig::R2` variant | `src/store/public.rs` | ~30 lines |
| `mod.rs` re-exports + features | `src/store/mod.rs` | ~20 lines |
| `VastAiBatchApi` | `src/cloud/vastai_batch.rs` | ~500 lines, ~1 day |
| `cloud/config.rs` Vast.ai section | `src/cloud/config.rs` | ~40 lines |
| Manifest tier policy | `src/planner/manifest.rs` | ~50 lines |
| Worker tier-routing | `src/bin/cloud_worker.rs` | ~30 lines (wrap GcsStore in TieredResultStore) |
| Cargo features | `Cargo.toml` | ~10 lines |
| Config sections | `coefficient.toml` | ~30 lines |
| Tests | `tests/` | ~200 lines |
| Docs | `docs/STORAGE_TIERING.md` | this doc, moved over |

**Total: ~1900 LOC, ~3–4 days of focused work.**

## Suggested PR split

Single mega-PR is reviewable but slow. I'd split into 4 sequential PRs:

1. **PR 1 — `R2Store` only**. Drop-in next to `SpacesStore`. No tiering,
   no batching, no Vast.ai. Adds the `r2` feature. Lets you point any
   existing config at R2 and it just works (at the naive $45-per-10M
   cost). ~½ day. ~400 LOC.

2. **PR 2 — `BatchedRowStore` + `TieredResultStore`**. Uses
   `R2Store` from PR 1. Adds the cost-saving aggregation + tier
   routing. Ships the Parquet schema + chunks-index format. Worker
   gains the manifest-driven tier policy. ~2 days. ~1000 LOC.

3. **PR 3 — Default tier policies + Squintly / browser bucket
   provisioning docs**. Mostly docs + the `StandardTierPolicy`
   implementation. ~½ day. ~300 LOC.

4. **PR 4 — `VastAiBatchApi`**. Independent of PR 1–3 (could land
   first); tests the batch_api trait against a prepay provider. ~1 day.
   ~500 LOC.

Mergeable in any order after PR 1 lands.

## Format choice for oracle chunks (decided)

Researched 2026-04-30 (general-purpose agent). **Apache Parquet with
zstd compression** is the right answer; the others are clearly worse
for our specific workload.

### Crate pins

```toml
[dependencies]
parquet      = { version = "58.1", default-features = false, features = ["arrow", "async", "snap", "zstd", "lz4"] }
arrow        = "58.1"
arrow-array  = "58.1"
arrow-schema = "58.1"
```

`parquet2` is dead (last release Nov 2022); `arrow2` was deprecated in
favor of unified arrow-rs. Don't pick either.

### Why Parquet beats the alternatives for our schema

- **Dictionary encoding crushes our low-cardinality strings.** `codec`
  has 4 values, `size_class` has 4, `config_name` has bounded
  cardinality. Parquet writes them as int8/int16 indexes + per-rowgroup
  string dictionary; JSONL repeats the strings every row.
- **Predicate + projection pushdown.** Pareto extraction is
  `SELECT bytes, zensim WHERE target_zq=85 GROUP BY source_sha256`.
  Parquet rowgroup statistics let DuckDB skip rowgroups where
  `target_zq` is out of range. **No other format on the shortlist
  matches this.**
- **NaN handling is clean.** `butteraugli` has NaNs; modern arrow-rs
  writer handles min/max stats correctly under Parquet 2.9+ spec.
- **Schema evolution is "good enough":** add nullable columns at the
  end, older chunks read back as `null`. Avoids rewriting on adding
  e.g. `vmaf` later.
- **DuckDB queries Parquet over R2 directly** via `httpfs` extension:
  `read_parquet('r2://bucket/.../**/*.parquet', hive_partitioning=true)`.
  No download / no proxy.
- **Browser readability:** [hyparquet](https://github.com/hyparam/hyparquet)
  is 10 KB minified pure JS, fetches the 512 KB footer via one range
  request, then projects only the columns the UI asks for. Direct fit
  for the coefficient web UI without a server-side proxy.

### Expected size on our schema

Mixed-numeric records (~100 B/row) compress to **~20-30 B/row** under
zstd-3 with dictionary-encoded strings. **A 50 K-row chunk = ~1.2 MB.**
Hits the sweet spot: rowgroup metadata overhead is amortized, R2 PUT
cost per chunk is ~$0.000004.

### What we explicitly rejected

| Format | Why not |
|---|---|
| Arrow IPC / Feather v2 | ~2× larger compressed (no per-column encoding); good only for in-flight workers, bad for storage |
| DuckDB native (.duckdb) | Single-writer per file — multiple Vast.ai workers writing the same file corrupt it |
| JSONL+zstd | 30-50 % bigger than Parquet, no projection/predicate pushdown |
| CBOR / MessagePack | No predicate pushdown; no DuckDB native reader |
| Avro | Better schema evolution irrelevant here; weak JS readers; Rust crate still beta |
| HDF5 | Effectively dead in the Rust analytics world for tabular data |

### R2 layout (revised, hive-partitioned)

```
metrics/
  v1/                              # schema version (bump on incompatible changes)
    codec=zenavif/                 # hive partition — DuckDB skips before fetching
      year=2026/month=04/day=30/
        worker=vast-7a3f/
          chunk-2026-04-30T18-22-04Z-000017.parquet
          chunk-2026-04-30T18-22-04Z-000017.parquet.meta.json   # provenance sidecar
```

DuckDB's `hive_partitioning=true` reads partition keys from the path,
so `WHERE codec='zenavif'` filters before any GET. Cuts R2 read costs.

Filename rule: UTC ISO timestamp + monotonic counter for idempotent
uploads. Glob `chunk-*.parquet`; the `.meta.json` sidecar (commit hash,
host, run_id, no records) stays out of the data glob.

### Worker write path (Rust skeleton)

```rust
use arrow_array::{RecordBatch, /* typed arrays */};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::{WriterProperties, EnabledStatistics};

let props = WriterProperties::builder()
    .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
    .set_dictionary_enabled(true)
    .set_column_dictionary_enabled("codec".into(), true)
    .set_column_dictionary_enabled("config_name".into(), true)
    .set_column_dictionary_enabled("size_class".into(), true)
    .set_statistics_enabled(EnabledStatistics::Page)
    .set_data_page_size_limit(1024 * 1024)        // 1 MiB pages
    .set_max_row_group_size(50_000)
    .build();

let mut w = ArrowWriter::try_new(out, schema, Some(props))?;
w.write(&record_batch_from(&records))?;
w.close()?;
```

### Chunks index — decided: `_index.parquet` per partition

A second small Parquet file per `codec=*/year=*/month=*/day=*/` that
holds `(record_id, chunk_path, row_offset)`. Built by the worker on
flush. Downstream tools that need random access by `record_id` query
the index first, then the data chunk. Avoids the SQLite sidecar
complexity I'd considered.

### DuckDB query path (reference)

```sql
INSTALL httpfs; LOAD httpfs;
CREATE SECRET (TYPE r2, KEY_ID '...', SECRET '...', ACCOUNT_ID '...');

WITH ranked AS (
  SELECT source_sha256, target_zq, codec, config_id, bytes, zensim,
         row_number() OVER (PARTITION BY source_sha256, target_zq
                            ORDER BY bytes ASC) AS rk
  FROM read_parquet('r2://metrics/v1/codec=*/year=2026/**/*.parquet',
                    hive_partitioning=true)
  WHERE zensim >= target_zq
)
SELECT * FROM ranked WHERE rk = 1;
```

This is the single workflow that justifies Parquet over everything
else on the list.

### Migration ceiling

If we ever exceed ~500 K chunks (R2 GET-list churn becomes the
bottleneck), the upgrade is **DuckLake** or **Iceberg** sitting on
top of the existing Parquet files. Both add a manifest layer without
rewriting data. Don't try in-place schema migration on Parquet —
write under `metrics/v2/...` and decommission `v1` after a transition
window.

## Remaining open design questions

1. **Tier policy at write-time vs read-time** — if we promote an
   encoding from Oracle → BrowserPublic later (e.g., user marks it
   for a Squintly demo), do we re-encode and re-upload, or pre-write
   a redacted blob that we materialize on demand? Re-encode is
   simpler and rav1e at speed=4 is ~500 ms per image; not a real
   bottleneck.

2. **Multipart upload for large blobs** — Squintly tier may carry
   5 MB AVIF stills. R2 supports S3-compat multipart but the existing
   `SpacesStore` doesn't use it. Rely on the AWS SDK's auto-multipart
   in the S3 client config (handles >8 MB by default).

3. **In-flight chunk durability** — workers buffer 50 K rows in
   memory before flushing. On Vast.ai host reclaim, that's ~5 min
   of work lost (~$0.10 of compute). Worth accepting; checkpointing
   the buffer to local disk every 30 s would protect against it but
   adds I/O complexity for marginal gain.

## Coordination needed

- **Coefficient repo has uncommitted work** (Cargo.lock + Cargo.toml +
  src/analysis/mod.rs + src/worker/mod.rs from the
  `claude-meta-train v0.6` session). Need that landed or stashed
  before any PR I open.
- **Cloudflare account setup** — R2 bucket creation, custom domain
  config, API tokens with R2 read+write scope. Pre-PR work.
- **Vast.ai account** — API key + initial $25–35 prepaid deposit.
  Pre-PR work for testing.

## Migration / rollout

- Phase 0: PR 1 ships, R2Store available behind `r2` feature.
- Phase 1: Existing GCS / Spaces deployments unchanged. New deployments
  opt into R2 via `coefficient.toml`.
- Phase 2: PR 2 ships, BatchedRowStore + TieredResultStore. Workers
  in new deployments emit Parquet chunks; old GCS deployments unchanged.
- Phase 3: PR 4 ships, Vast.ai backend available. Cron jobs / one-shot
  bursts can target Vast.ai for prepay-bounded compute.
- Phase 4: Migrate zenavif's predictor_sweep to use coefficient as the
  cloud-runner backend (separate work in zenavif repo).

No data migration of the existing GCS/Spaces store needed — all
existing artifacts stay where they are; new artifacts go through the
tiered path.

## Per-blob cost recap (sanity check the design)

For a year of full-throttle operation at 10 M encodes/quarter:

| Tier | Records / qtr | Writes / qtr | Reads / yr | Storage / yr |
|---|---|---|---|---|
| Oracle | 10 M | ~1 K (Parquet chunks) | ~10 K (full scans for retraining) | ~50 MB |
| Browser | 100 K | 100 K | ~1 M (browser pageviews) | ~25 GB |
| Squintly | 1 K | 1 K | ~10 M (CDN-cached) | ~5 GB |
| **Total / qtr** | | **~101 K writes** | | |
| **Total / yr** | | **404 K writes ($1.82)** | **~11 M reads ($3.96)** | **~30 GB ($5.40/yr)** |

Annual R2 bill: **~$11**. Very fine.

For comparison, current naive-per-blob design at 40 M records / yr =
**$180/yr in writes alone**.

---

# Three open questions, answered

## (a) How do we ensure work is never duplicated?

Encodes can be expensive (~minute each at low speed × large image), so
losing a sweep to duplicate work is real money. Coefficient already
solves most of this — but the existing design assumes a single store
that's locally scannable, and breaks down for distributed workers
writing to R2.

### What coefficient already gives us (verified in `src/`)

1. **Deterministic encoding IDs.** `EncodingRecord::id` is
   `sha256(source_hash || codec_name || codec_config || quality)`
   (`src/version/encoding.rs:~180`). Two workers given the same
   inputs produce the same ID — independent of which worker runs the
   encode.
2. **Write-once `SafeStore`.** `src/store/safe.rs:144` rejects an
   `encoding_put` whose ID exists with different bytes (returns
   `PutResult::Conflict`). Idempotent: same bytes for same ID returns
   `AlreadyExists`. So even if two workers race on the same work-unit,
   at most one set of bytes lands.
3. **`skip_completed` in the planner.** `src/planner/mod.rs:334-362`
   loads all known encoding IDs, drops tasks that match. Today this is
   an O(N) full scan — fine at 100k records, breaks at 10M.

### What we add for the R2 / Parquet world

| Mechanism | Cost | Catches |
|---|---|---|
| **Manifest-based work assignment** | $0; in-memory | Different workers don't pick the same units |
| **Pre-flight Bloom filter** (~12 bits/element, 15 MB for 10M IDs, 0.1% FP rate) | one R2 GET per worker (~1 ¢/sweep) | Work units already done in a previous run |
| **Per-partition `_index.parquet`** (record_id → chunk_path, row_offset) | ~50 KB / partition; 5 MB total at 10M scale | Authoritative existence check on Bloom-filter false positives |
| **Idempotent chunk filenames** (include content hash + run_id) | $0 | Crash-then-retry duplicate flushes — compactor dedupes by ID at merge time |

The flow on a Vast.ai worker:

1. Worker boots, fetches manifest = list of `(source_sha256, codec_id,
   config_id, quality)` work units assigned to it.
2. Worker fetches the latest `_dedup.bloom` from R2 (~15 MB once).
3. For each work unit: `bloom.contains(encoding_id)` → if false, encode.
   If true, fetch the partition's `_index.parquet` and confirm; only
   skip on real hit.
4. Encode → buffer → flush chunk every 50K rows OR 5 min.
5. On flush, chunk filename includes `chunk-{timestamp}-{run_id}-
   {sha256_of_payload}.parquet`. Atomic single-PUT to R2.
6. Compactor (nightly cron) merges yesterday's chunks into daily
   files; dedupes by `record_id` at merge time using DuckDB
   `qualify row_number() over (partition by record_id) = 1`. So even
   the bloom-FP-but-actually-different case gets resolved cleanly at
   read time.

The Bloom filter is rebuilt by the compactor as it walks the index.

**Worst-case waste:** ~50K records (one buffer's worth, ~5 min of
compute, ~$0.10) lost on a worker crash. No double-encoded records
ever land in the merged read view — the compactor enforces uniqueness.

### What needs implementing in the PR

- `src/store/dedup.rs` — Bloom filter build + check + serialize to R2
- `src/store/compact.rs` — periodic merge job (DuckDB-driven; ~50 LOC)
- `src/planner/mod.rs:334-362` — switch from full HashSet load to
  Bloom-filter probe (constant-memory)

## (b) How do codecs evolve, and how do we coalesce versions?

This is where coefficient has machinery that **isn't wired in yet**.
The PR's biggest opportunity.

### What exists (and what's missing)

- `src/version/fingerprint.rs:55` — `CanarySet`: 15 fixed
  (source_hash, quality) pairs. Encoding all 15 produces a stable
  signature for a given (codec, version, build commit).
- `src/version/fingerprint.rs:126` — `ConfigFingerprint`:
  `sha256(concat(blob_hashes from CanarySet runs))`. **Two builds
  produce identical fingerprints iff they produce identical bitstreams
  on the canary inputs.** This is exactly the "did this version change
  observable behavior?" test.
- `src/version/fingerprint.rs:246` — `EquivalenceRegistry`: append-
  only map `(codec_name, config_hash, fingerprint_hash) → version_id`.
- **Gap:** the planner's skip path (`src/planner/mod.rs:334-362`)
  hashes the codec_name as part of the encoding ID. If `codec_name`
  encodes the version (e.g. `"zenavif-0.1.7"`), zenavif 0.1.6 and
  0.1.7 produce different encoding IDs and we re-encode even when the
  output is bit-identical.

### Proposed wiring

The fix is to **route codec identity through the EquivalenceRegistry
before computing the encoding ID**. Concretely:

1. **Worker startup**: encode the CanarySet at default quality with
   the active build of each codec. Compute `ConfigFingerprint`.
2. **Worker queries** `EquivalenceRegistry` (small file on R2,
   `r2://config/equivalence.parquet`): "is fingerprint X already
   registered?" → returns `version_id` (a stable u32 / sha256 prefix).
   If new fingerprint, register it, get a new `version_id`.
3. **Manifest carries `version_id`** for each work unit, not the
   crate-version string. So the manifest is identical whether the
   worker is on zenavif 0.1.6 or 0.1.7 with same fingerprint.
4. **Encoding ID becomes**:
   `sha256(source_hash || version_id || config_hash || quality)`.
   Identical fingerprints share the same ID.
5. **`crate_version` and `commit` go into the EncodingRecord**
   metadata as provenance, but don't participate in the ID.

When a real codec change happens (e.g., zenavif 0.2.0 with a new QM
table → different bitstream), the canary fingerprint differs → new
`version_id` is registered → new work units get new IDs → the worker
encodes them. Old `version_id` results stay valid. Picker training
queries can either pin a `version_id` (reproducible) or union over
fingerprint-equivalent ones (latest).

### What needs implementing in the PR

- `src/version/equivalence_registry.rs` — read/write the registry
  parquet on R2 (already exists in-memory at
  `src/version/fingerprint.rs:246`; add persistence + Bloom filter)
- `src/planner/mod.rs:344` — replace `codec.name()` in
  encoding-ID input with `version_id` resolved from the registry
- `src/bin/cloud_worker.rs` — call `register_canary_fingerprint()` at
  startup, pin `version_id` for all subsequent encodes
- `coefficient.toml [version]` — add `equivalence_registry_uri =
  "r2://.../equivalence.parquet"`

This is genuinely the missing piece. ~600 LOC, ~2 days. Worth a
dedicated PR — call it **PR2.5** in the split, between the storage
PRs and the Vast.ai PR.

## (c) Server-free browsing across many parquet files

You don't need a server. Here's the layered design:

### Three layers, each handles a query class

```
Layer        Format                    Size       Updates           Use
─────        ──────                    ────       ───────           ───
Manifest     manifest.parquet           ~100 KB    every chunk flush  "what chunks exist?"
Per-partition _index.parquet            ~50 KB     hourly compaction  "where is record X?"
Data         chunk-*.parquet            ~1.2 MB    every 50K rows     full row data
```

The manifest is a single small parquet at the root of the bucket:

```
manifest.parquet schema:
  partition_path   STRING  (e.g. "metrics/v1/codec=zenavif/year=2026/.../")
  chunk_filename   STRING  (e.g. "chunk-2026-04-30T18-22-04Z-000017.parquet")
  row_count        UINT64
  byte_size        UINT64
  schema_version   STRING  (e.g. "v1.3")
  version_id       UINT32  (codec equivalence class)
  encode_codec     STRING  ("zenavif" | "zenwebp" | "zenjpeg" | "zenjxl")
  date             DATE    (for hive-pruning)
  min_zensim       FLOAT
  max_zensim       FLOAT
```

The browser fetches `manifest.parquet` once on page load (~100 KB,
~30 ms over R2). All "find me chunks where codec=zenavif and
zensim>80" queries answer from the manifest — no chunk reads.

### Querying across files

Two real options for the browser, by complexity:

1. **hyparquet** (10 KB JS, [hyparam/hyparquet](https://github.com/hyparam/hyparquet)) for projection + range-scan reads. Single chunk
   queries: "show me rows from this chunk where bytes < 10000".
   No JOINs. Perfect for "load this one Pareto-frontier slice".

2. **DuckDB-WASM** (~6 MB JS, cached after first load) for joins and
   cross-chunk SQL. Reads R2 chunks via httpfs. The full Pareto
   extraction query, the picker-training prep query, anything with
   `GROUP BY` / `JOIN` / `WINDOW` — all client-side. Same SQL we'd
   run in the local DuckDB CLI.

The natural split: **default views in coefficient browser use
hyparquet** (instant); **drill-down "explore" view uses DuckDB-WASM**
(loads on first interaction). 6 MB cached download is fine for an
analytics tool.

### Do we join the parquet files?

Yes, but at compaction time, not at write time. Three tiers:

```
                Latency target   Format                                   Files at 10M scale
Hot (today)     low write cost   raw chunks, ~50K rows each               2,000+ chunks
Warm (yesterday) low read cost   daily compactions, ~5M rows each          ~30 files
Cold (>30 days) lowest read cost  monthly compactions, ~150M rows each     ~12 files
```

Compaction is one DuckDB statement run nightly:

```sql
COPY (
  SELECT * FROM read_parquet('r2://metrics/v1/codec=*/year=2026/month=04/day=30/**/*.parquet')
  QUALIFY row_number() OVER (PARTITION BY record_id ORDER BY timestamp) = 1
) TO 'r2://metrics/v1/compacted/2026-04-30.parquet' (FORMAT 'parquet', COMPRESSION 'zstd', ROW_GROUP_SIZE 50000);
```

After compaction the day's hot chunks are deleted (or moved to
`raw/2026-04-30/` for ~7 days as a retry safety net).

The browser's default queries hit warm + cold (~40 files at 10M
scale) — DuckDB-WASM glob `read_parquet('r2://.../compacted/*.parquet')`
is fast. Drill-down into "today" hits the hot tier.

### What needs implementing in the PR

- `src/store/manifest.rs` — read/write `manifest.parquet`, append on
  chunk flush
- `src/store/compact.rs` — daily / monthly DuckDB compaction job (call
  via `duckdb` Rust binding or shell out to the CLI)
- `viewer/src/lib/duckdb_wasm.ts` — initialize DuckDB-WASM with R2
  credentials (read-only, public bucket prefers no creds)
- `viewer/src/lib/hyparquet.ts` — projection-only reader for fast
  default views
- `viewer/src/routes/explore/+page.svelte` — wire the two readers to
  the existing explore page

---

# Rescue plan for coefficient's pending work

You said the prior agent's session crashed. Two paths:

1. **Rescue as wip** — I check what was being attempted (look at the
   diff, the .workongoing description "v0.6 (44 features, Spearman∩
   ablation, 192x192x192)" suggests a meta-train experiment), then
   either:
   - commit it as `wip(meta-train): v0.6 — 44 features, Spearman
     ablation` on a `wip/meta-train-v0.6` branch
   - or stash to a `.patch` file under `coefficient/.rescued/` and
     leave a note so the original session can resume

2. **Discard if confirmed unwanted** — only with your explicit
   confirmation per global CLAUDE.md "NEVER discard changes you
   didn't make" rule.

I'd recommend option 1 (commit-to-branch) — it's reversible, preserves
the work, and unblocks the new PR. But that needs your green light
since I'd be committing on a branch and the prior agent might have
intended different commit boundaries.

Tell me to proceed and I'll inspect the diff, propose a commit
message, then commit on a `wip/` branch and push it.

---

# Two more design pinpoints (added 2026-05-01)

## Local box and cloud workers share one substrate

There is no "sync local upstream" step in the final design — the
local 7950X is **just another worker writing to R2 via the same
`BatchedRowStore<R2Store>` path the cloud workers use**. Idempotent
keys (`record_id` derived from source+codec_version_id+config+quality)
mean parallel workers can't double-write or corrupt each other's
chunks; the compactor enforces uniqueness at merge time.

Practical migration sequence:

1. **One-shot migrator** (`zenavif/examples/migrate_tsv_to_r2.rs`,
   post-PR2): walks the existing `benchmarks/rav1e_phase{1a,2,3}_*.tsv`
   files, normalizes to the canonical schema, writes Parquet chunks to
   the right hive partitions on R2. Idempotent — re-runs match by
   `record_id` and skip already-present rows. Run once per historical
   TSV, then those TSVs become reference data.
2. **Sweep harness emits straight to R2.** `predictor_sweep.rs` gets
   an `--r2-bucket` flag (or `R2_BUCKET` env) that swaps the TSV writer
   for `BatchedRowStore<R2Store>`. Local TSV mode survives as
   `--debug-tsv` for ad-hoc analysis but is no longer the canonical
   output.
3. **Cron rewrite.** The nightly LHS rotator stops writing local
   TSVs. Sunday retrain reads from R2 (DuckDB
   `read_parquet('r2://metrics/v1/codec=zenavif/**/*.parquet')`)
   instead of from `benchmarks/`. Local box becomes pure compute, no
   accumulated state.

After step 3, the local 7950X and any rented Vast.ai box are
interchangeable as workers — the only thing that distinguishes them
is `worker_id` in the row metadata.

## Score versions are first-class — same registry as codecs

zensim 0.2.x and zensim 0.4 produce different scores for the same
image pair. A row labelled "zensim=82.3" without a version tag is
**not comparable** across upgrades. Coefficient already has the
machinery for this — `src/version/metric.rs` defines
`MetricVersionDef` parallel to `CodecVersionDef`, with the same
`MatchRules` (semver + commit + required impl). The same gap that
applies to codec versions (not wired into the planner skip path)
applies here, and the same PR2.5 fix — wiring the `CanarySet` +
fingerprint dance into the registry — covers both.

The mechanic is identical:

1. Worker boots with a pinned `ZensimProfile` (e.g. `v0_4_0`).
2. Worker runs the metric `CanarySet` (a small fixed set of
   reference image pairs at known target qualities; pre-computed
   blob hashes stand in for what the canary metric should produce).
3. The hash of the metric's actual outputs over the canary set is
   the `ConfigFingerprint` for this metric build.
4. Equivalence registry on R2 maps
   `(kind=metric, name=zensim, config_hash, fingerprint_hash) →
   version_id`. Two zensim builds with bit-identical canary outputs
   share a `version_id` (e.g. a no-op patch bump). Different output
   → different `version_id`.
5. Every `MetricRecord` carries `(metric_name, version_id)`. The
   picker training pipeline queries by `version_id`, never silently
   mixing scores across incompatible versions.

### Schema impact on the oracle Parquet

The metric record's per-metric `version_id`s land alongside the
score values. ~12 bytes/row total (3 × u32) with very low cardinality
across the corpus, so dictionary encoding makes them ~free in
storage.

```
record_id              STRING
source_sha256          FIXED_SIZE_BINARY(32)
codec_name             STRING       (dict-encoded)
codec_version_id       UINT32       ← from EquivalenceRegistry
config_id              UINT32
target_zq              UINT8
bytes                  UINT64
encode_ms              FLOAT64
zensim                 FLOAT32
zensim_version_id      UINT32       ← from EquivalenceRegistry
ssim2                  FLOAT32
ssim2_version_id       UINT32
butteraugli            FLOAT32  (nullable; sometimes not computed)
butteraugli_version_id UINT32   (nullable iff value is)
timestamp              INT64
worker_id              STRING
```

### Equivalence registry layout on R2

One Parquet file at the bucket root, append-only:

```
r2://metrics/v1/equivalence.parquet
schema:
  kind                STRING   ("codec" | "metric")
  name                STRING   ("zenavif" | "zensim" | "ssim2" | …)
  config_hash         STRING   (sha256 of normalized knob set)
  fingerprint_hash    STRING   (sha256 of canary outputs)
  version_id          UINT32   (assigned monotonically on first sight)
  registered_at       INT64    (unix timestamp)
  crate_version       STRING   (provenance: e.g. "0.4.0")
  commit              STRING   (provenance: git sha)
```

Workers fetch it once at startup (~few KB, ~30ms over R2), look up
or register their canary fingerprint, then pin the resolved
`version_id` for the rest of the session.

### Upgrade workflow (real example)

When zenavif gets a QM behavior change (e.g. the v0.4.2 → v0.4.3
fix that landed earlier this week):

1. Old data: every row has `codec_version_id = 7` (whatever zensim
   resolved the v0.4.2 fingerprint to)
2. Workers come up on v0.4.3
3. Canary fingerprint differs (QM produces different bitstreams)
4. Registry assigns `codec_version_id = 8`
5. New rows tagged with id=8; old rows still tagged with id=7
6. Picker training: query for the codec, decide whether to use the
   latest version_id only, the latest "fingerprint-equivalent set"
   (in this case just {8}), or to union {7, 8} for max sample size
   (acceptable iff the BD-rate delta is small, which it isn't here
   — so we pin 8)
7. Old data isn't deleted; it's just untrained-against. If we want
   to backfill, a re-encode job picks up everything currently
   tagged 7 and re-encodes with v0.4.3, producing rows tagged 8.

zensim version bumps work identically.

### What this means for the picker config

`training/rav1e_picker_config.py` (and the equivalent zenwebp /
zenjpeg / cross-codec configs) needs a way to filter by
`version_id`. Add `CODEC_VERSION_IDS` and `METRIC_VERSION_IDS` lists
(or "latest" sentinels) to the config, and have the trainer's
`load_pareto` filter on those. Default to "latest fingerprint-
equivalent set" so configs don't go stale silently.

---

# Bulk migration of historical work — what to import (added 2026-05-01)

The user has accumulated dozens of hours of encoding + scoring across
multiple codecs and harnesses, **most of it predating the canonical
Parquet schema this proposal designs**. Quick inventory of what's
out there:

## What's locally on disk (≥10 KB benchmarks; non-finance)

| Location | Format | Approx scale | Schema confidence |
|---|---|---|---|
| `~/work/zen/zenjpeg/benchmarks/zq_pareto_2026-04-29.tsv` | TSV | **3.5 M rows / 515 MB** | ✅ canonical (image_path, size_class, w, h, config_id, config_name, q, bytes, …) |
| `~/work/zen/zenjpeg/benchmarks/zq_pareto_2026-04-28.tsv` | TSV | 34 MB | ✅ canonical |
| `~/work/zen/zenwebp/benchmarks/zenwebp_pareto_2026-04-30_combined.tsv` | TSV | **2.7 M rows / 371 MB** | ✅ canonical |
| `~/work/zen/zenwebp/benchmarks/zenwebp_pareto_2026-04-29.tsv` | TSV | 293 MB | ✅ canonical |
| `~/work/zen/zenwebp/benchmarks/zenwebp_pareto_2026-04-30_sizedense.tsv` | TSV | 79 MB | ✅ canonical |
| `~/work/zen/jxl-encoder/jxl-encoder/benchmarks/lossy_pareto_2026-04-30.tsv` | TSV | **610 K rows / 95 MB** | ✅ canonical (20 cols — extends base schema) |
| `~/work/zen/jxl-encoder/jxl-encoder/benchmarks/lossless_pareto_2026-04-30.tsv` | TSV | 22 MB | ✅ canonical |
| `~/work/zen/zenavif/benchmarks/rav1e_phase1a_2026-04-30.tsv` | TSV | 23 MB | ✅ canonical |
| `~/work/zen/zenavif/benchmarks/rav1e_phase2_oat_2026-04-30.tsv` | TSV | 0.5 MB | ✅ canonical |
| Feature TSVs (zenwebp / zenjpeg / zenavif / jxl-encoder) | TSV | tens of MB each | ✅ matches train_hybrid expected shape |
| `/mnt/v/backups/home/oracle-d2-store/oracle-d2/pareto_rows.csv` | CSV | **30 MB** | 🟡 different schema (cross-codec; needs adapter) |
| `/mnt/v/backups/home/oracle-d2-store/oracle-d2/blobs/` | per-encoding blobs | (size pending scan) | 🟡 hash-keyed already |
| `~/work/all-the-images/corpus/encoding_results.json` | huge JSON array | **653 MB**, encoder_id="libjpeg-turbo-1.3.0" etc. | 🟡 different schema, NO scores yet (encode-only) |
| `~/work/codec-eval/results/full_sweep_20260107.csv` | CSV | 6.4 MB | 🟡 older sweep harness, schema TBD |
| `~/work/zen/retired/zenjpeg-dispatch/heuristic_outputs/results.csv` | CSV | 37 MB | 🟡 retired harness |
| `~/work/downscaling-eval/results/cid22_full_2x/jpeg_results_*.json` | JSON | 4–6 MB each | 🟡 downscaling-specific |

Total **~1.45 GB** of TSV/CSV/JSON in `~/work/**/benchmarks/` plus
**30 MB** of `pareto_rows.csv` in the oracle-d2 backup at
`/mnt/v/backups/home/oracle-d2-store/oracle-d2/`. Many CPU-days of
encoding and scoring. More sits on tower NAS, DO Spaces, and GCS
that we'll need to inventory separately.

The oracle-d2 `pareto_rows.csv` (54 K rows) has a richer metric set
than the per-codec sweeps:

```
schema: source_hash, bucket, w, h, codec_name, q, bpp, size_bytes,
        ssim2, ssim2_pareto, butter, butter_pareto,
        dssim, dssim_pareto, zensim
```

**Schema implication:** add `dssim` and `dssim_version_id` to the
canonical Parquet schema. Cheap (~8 B/row, dict-compressed) and
matches the existing oracle-d2 substrate so we don't lose columns
on import. The `*_pareto` flags are derived/cacheable so we recompute
on demand rather than storing.

## Migration tiers

Not all of this is worth importing into the same canonical Parquet
substrate. Three tiers:

### Tier A — bulk-import as-is (highest priority)

**Newest canonical-schema TSVs from the active codec sweeps:**
- zenjpeg: zq_pareto_2026-04-29 + features (3.5 M rows)
- zenwebp: zenwebp_pareto_2026-04-30_combined + features (2.7 M rows)
- jxl-encoder: lossy_pareto_2026-04-30 + features (610 K rows)
- zenavif: rav1e_phase1a + phase2_oat + features (~25 K rows)

These are the substrate the picker training pipelines already use.
Matches the canonical schema (image_path / size_class / w / h /
config_id / config_name / q / bytes / zensim / …) so the migrator is
straightforward: stream-parse TSV → buffer → flush as Parquet to
hive-partitioned R2 prefixes.

**Provenance backfill** — these rows don't have `codec_version_id` or
`metric_version_ids` baked in. Migrator needs:
1. Read the TSV header + a sample of rows to detect the schema
2. Cross-reference the file mtime + git log (`git log --all --since
   "$(stat -c %y $tsv)" -- '*.rs'`) for the codec/metric crate to
   resolve the active versions at sweep time
3. Tag every row with the resolved `codec_version_id` and per-metric
   `metric_version_id`s. If the resolved fingerprint isn't yet in
   the registry, register it using a stored canary set.
4. Rows where provenance can't be resolved cleanly get
   `version_id = legacy_unknown` and a warning entry in the
   migration log

~1 day of work for the migrator (one Rust example), ~half a day to
actually run + verify (depending on R2 upload speed).

### Tier B — adapt-and-import (medium priority)

**Different-schema sources that are still scientifically useful:**
- oracle-d2 substrate (`pareto_rows.csv` — 30 MB cross-codec format
  with codec_name field that conflates name+version like "zenjpeg-444-
  e6-v0.4.2"). Has the picker_features sidecar at
  `feature_utility/`.
- all-the-images encoding_results.json — encoder_id includes version
  ("libjpeg-turbo-1.3.0"), output_hash, output_bytes; NO scores yet.
- codec-eval / downscaling-eval / retired/zenjpeg-dispatch outputs

Each needs a per-source adapter (~50–100 LOC) to unmarshal into the
canonical Parquet shape. Runs after Tier A.

For all-the-images specifically: encoded blobs are already keyed by
`output_hash` so re-scoring against the source corpus is feasible
without re-encoding. Worth doing as a separate scoring pass post-
migration so we have zensim/ssim2/butteraugli on those rows.

### Tier C — reference-only / leave in place (low priority)

- Massive working copies of obsolete sweep outputs (`zq_pareto_2026-
  04-28.tsv` superseded by `2026-04-29.tsv`)
- Per-experiment one-off CSVs that don't generalize (e.g.
  `aq_sharpened_tuning.csv`)
- Train output JSONs (`zq_bytes_hybrid_v2_1_*.json`) — derived
  artifacts, recomputable

These stay on disk as historical reference. Migrating them adds noise
without analytical value. Leave a `legacy/` index file in R2 that
lists the local paths + brief description so they're discoverable.

## What the migrator looks like

A single Rust binary with per-source adapters:

```
coefficient/src/bin/bulk_import.rs
  --source <preset> | --tsv <path> [--schema <schema-name>]
  --r2-bucket <name>
  --equivalence-registry <path>   # for resolving version_ids
  --dry-run                        # print what would be written
  --partition-template <hive-template>
  --max-rows-per-chunk 50000
```

Presets define the per-codec extraction logic:
- `zenjpeg-canonical-v1` — the standard zenjpeg TSV format
- `zenwebp-canonical-v1`
- `zenavif-canonical-v1`
- `jxl-encoder-canonical-v1`
- `oracle-d2-pareto-rows`
- `all-the-images-encoding-results`

Each preset runs:
1. Parse rows
2. Resolve `codec_version_id` and `metric_version_id`s through the
   equivalence registry (registering new entries if needed via a
   manually-maintained `legacy_canary_fingerprints.json`)
3. Stream into `BatchedRowStore<R2Store>` with the canonical hive
   partition path
4. Write a `migration_log.parquet` audit trail with row counts,
   skipped/repaired rows, and the inferred provenance per source TSV

## Approximate row counts after Tier A migration

| Source | Approx rows | Tier |
|---|---:|---|
| zenjpeg combined | 3.5 M | A |
| zenwebp combined | 2.7 M | A |
| jxl-encoder lossy + lossless | 0.7 M | A |
| zenavif phase1a + phase2 | 0.025 M | A |
| oracle-d2 pareto_rows | ~0.09 M | B |
| all-the-images encoding_results | ~5–10 M (encode-only, no scores) | B |
| **Tier A total** | **~7 M rows** | |

Storage cost on R2 after Parquet+zstd: ~7 M × 25 B/row = **175 MB**,
roughly $0.003 / month for storage. Migration writes: 7 M / 50K = **140
chunks** = $0.000 in writes (rounding error).

## Risks / open questions

1. **Re-running Tier A might duplicate rows.** Migrator must use
   deterministic `record_id`s (sha256 of the standard tuple) so
   re-running is idempotent at the merge step. The compactor's
   `qualify row_number() over (partition by record_id) = 1` rule
   covers it — but adds a compaction round.

2. **Legacy `metric_version_id`** for old zensim runs. The local
   zensim crate version at file mtime is the best signal but isn't
   1:1 with output behavior (build flags, archmage feature gates,
   etc.). Worst case: assign each TSV its own `version_id` that
   means "zensim as built on this machine on this date" and let
   the picker filter manually.

3. **`all-the-images` is 4.4 GB of source corpus** (the JSON +
   actual encoded blobs). Worth tier-B importing the metadata; the
   blobs themselves stay where they are (or get hash-deduped into
   `r2://sources/`).

4. **DO Spaces / GCS / Firestore data** — coefficient has been
   writing there. Need read access to those services (scripts owned
   by the deployment, not local) before we can plan that part of
   the migration. Out of scope for the local-disk pass.

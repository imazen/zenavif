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
| **Coefficient browser** | Pareto exploration UI | ~100 K – 1 M | ~10–500 KB (encoded blob + metadata + thumbnail) | public |
| **Squintly** | Public demos | ~1 K – 10 K | ~100 KB – 5 MB (encoded + reference) | public |

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

## Open design questions

1. **Parquet vs Arrow IPC vs JSONL.gz** for oracle chunks? Parquet is
   the right answer for long-term storage but Arrow IPC is faster for
   in-flight workers. Could write Arrow IPC chunks and convert to
   Parquet at compaction time.

2. **Chunks index** — in-memory hash map flushed to JSON sidecar, vs
   a SQLite sidecar that supports range queries on (codec, source_id,
   q). I lean SQLite since downstream picker training already loads
   the chunks for analysis and a SQLite index supports the join cheaply.

3. **Tier policy at write-time vs read-time** — if we promote an
   encoding from Oracle → BrowserPublic later (e.g., user marks it
   for a Squintly demo), do we re-encode and re-upload, or pre-write
   a redacted blob that we materialize on demand? Re-encode is simpler
   and rav1e at speed=4 is ~500 ms per image; not a real bottleneck.

4. **Multipart upload for large blobs** — Squintly tier may carry
   5 MB AVIF stills. R2 supports S3-compat multipart but the existing
   `SpacesStore` doesn't use it. Add multipart support in `R2Store`
   if blob size > 8 MB? Or rely on the AWS SDK's auto-multipart in
   the S3 client config.

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

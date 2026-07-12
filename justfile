# zenavif justfile

# Default recipe
default: check

# Check compilation
check:
    cargo check --all-targets

# Build release
build:
    cargo build --release

# Run tests
test:
    cargo test

# Run clippy with warnings as errors
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Format code + regenerate the public-API surface snapshots (docs/public-api/).
# The snapshot runner lives in the standalone apidoc/ package, so it is never
# built or run by plain `cargo test` or any CI job.
fmt:
    cargo fmt
    cargo test --manifest-path apidoc/Cargo.toml

# Regenerate the public-API surface snapshots only
api-doc:
    cargo test --manifest-path apidoc/Cargo.toml

# Verify the committed snapshots are current
api-doc-check:
    ZEN_API_DOC=check cargo test --manifest-path apidoc/Cargo.toml

# Format check
fmt-check:
    cargo fmt --check

# Build with encode feature
build-encode:
    cargo build --features encode

# Test with encode feature
test-encode:
    cargo test --features encode

# Clippy with encode features
clippy-all:
    cargo clippy --all-targets --features encode -- -D warnings

# Test feature permutations
feature-check:
    cargo test --features encode
    cargo test --features encode-threading

# Full CI check
ci: fmt-check clippy test feature-check

# --- Executable gates (docs/ENGINEERING_BASELINE.md section A) ---
# zenrav1e's halves (gate-identity A1, gate-recon A5) live in ../zenrav1e.

# Gate A3: encoded bytes are independent of thread count (and repeatable).
# Pinned synthetic cells incl. a 2.30 MP multi-tile image; threads
# {1,1,8,auto,auto} legs must be byte-identical. CI runs the --ci subset.
gate-determinism:
    cargo run --release --features encode-imazen,encode-threading --example gate_kit -- determinism

# Gate A2: cross-decoder conformance (the PALCONF protocol) on the pinned
# cell matrix: aomdec decodes every cell cleanly AND byte-agrees (raw planar
# md5) with rav1d-safe. Local-only. AOMDEC is REQUIRED — the caller decides
# the decoder here, never the script silently (dev box canonical:
# /home/lilith/work/aom/build_slow/aomdec). ZENRAV1E (sibling CLI) drives the
# palette/intraBC-armed leg; pass ZENRAV1E='' to skip that leg deliberately.
gate-conformance:
    AOMDEC="${AOMDEC:-$(command -v aomdec || echo /home/lilith/work/aom/build_slow/aomdec)}" \
    ZENRAV1E="${ZENRAV1E-{{justfile_directory()}}/../zenrav1e/target/release/rav1e}" \
    bash scripts/gates/gate_conformance.sh

# Gate A6: coarse perf floors — (bytes, ssim2, enc_ms) per ladder cell
# (s2/s6/s10 x 3 images x 3 qualities) against the machine-scoped envelope
# in benchmarks/gate_ladder_envelope.tsv. Local-only (timing). A de-tuning
# tripwire with generous tolerances, not a benchmark.
gate-ladder:
    cargo run --release --features encode-imazen,encode-threading --example gate_kit -- ladder

# Re-pin the ladder envelope after an INTENTIONAL ladder change (commit the
# TSV diff in the same commit as the change that moved it).
gate-ladder-pin:
    cargo run --release --features encode-imazen,encode-threading --example gate_kit -- ladder --pin

# Invariant: RD improves monotonically with encode TIME (no slower tier
# Pareto-dominated by a faster one). Envelope = benchmarks/gate_monotone_envelope.tsv
# lists KNOWN inversions; fails on any NEW one. Goal state: empty envelope.
gate-monotone:
    cargo run --release --features encode-imazen,encode-threading --example gate_kit -- monotone

# Re-pin after landing a content-gate that REMOVES inversions (the shrinking
# envelope IS the progress) or at the dep-bump flip. Commit the TSV diff.
gate-monotone-pin:
    cargo run --release --features encode-imazen,encode-threading --example gate_kit -- monotone --pin

# All zenavif-side gates (run before + after every refactor commit, per
# docs/ENGINEERING_BASELINE.md section E).
gates: gate-determinism gate-conformance gate-ladder gate-monotone

# Run example decode_avif with test image
decode-test:
    mkdir -p /mnt/v/output/zenavif/test
    cargo run --release --example decode_avif -- {{justfile_directory()}}/../../aom-decode/tests/test.avif /mnt/v/output/zenavif/test/test.png

# Profile decode-from-bytes heap allocations with heaptrack (needs heaptrack installed).
# Defaults to the committed kodim03_yuv420_8bpc.avif (768x512) decoded 8x; pass a path +
# iters to override. Inspect with: heaptrack_print /tmp/zenavif-ht.zst
heaptrack-decode *ARGS:
    cargo build -p zenavif --release --example heaptrack_decode
    rm -f /tmp/zenavif-ht.zst
    heaptrack --output /tmp/zenavif-ht ./target/release/examples/heaptrack_decode {{ARGS}}

# Cross-test i686 (32-bit x86)
test-i686:
    cross test --target i686-unknown-linux-gnu

# Cross-test armv7 (32-bit ARM)
test-armv7:
    cross test --target armv7-unknown-linux-gnueabihf

# Run all cross tests
test-cross: test-i686 test-armv7

# Clean build artifacts
clean:
    cargo clean

# Update dependencies
update:
    cargo update

# Check outdated dependencies
outdated:
    cargo outdated

# Download AVIF test vectors
download-vectors:
    bash scripts/download-avif-test-vectors.sh

# Run integration tests with test vectors
test-integration:
    cargo test --test integration_corpus -- --ignored --nocapture

# Encode-quality sweep. Pass extra flags after `--`, e.g.
#   just sweep -- --image /path/to/ref.png --speeds 1,2,4 --force-bottomup both
sweep *ARGS:
    cargo run --release --example encode_sweep --features encode-imazen,encode-threading -- {{ARGS}}

# Download vectors and run integration tests
test-all: download-vectors test-integration

# Build Docker image for libavif reference generation
docker-build:
    docker build -f Dockerfile.references -t zenavif-references .

# Generate libavif reference images using Docker
generate-references: download-vectors
    @if [ ! -d tests/zenavif-references/.git ]; then \
        echo "Error: tests/zenavif-references repo not found"; \
        echo "Clone it with: git clone <url> tests/zenavif-references"; \
        exit 1; \
    fi
    docker run --rm \
        -v {{justfile_directory()}}/tests/vectors:/vectors:ro \
        -v {{justfile_directory()}}/tests/zenavif-references:/references \
        zenavif-references

# Run pixel verification tests (requires references)
test-pixels:
    cargo test --test pixel_verification -- --ignored --nocapture verify_against_libavif

# Full pixel verification: generate references and test
verify-pixels: generate-references test-pixels

# --- link-u/avif-sample-images corpus (reproduces rav1d-safe#1) ---

# Download link-u/avif-sample-images test corpus
download-linku:
    bash scripts/download-linku-samples.sh

# Generate libavif reference PNGs for link-u corpus via Docker
generate-linku-references: download-linku docker-build
    mkdir -p tests/linku-references
    docker run --rm \
        -v {{justfile_directory()}}/tests/vectors/link-u:/vectors:ro \
        -v {{justfile_directory()}}/tests/linku-references:/references \
        -e VECTORS_DIR=/vectors \
        -e REFERENCES_DIR=/references \
        --entrypoint /usr/local/bin/generate-linku-references.sh \
        zenavif-references

# Decode all link-u samples (no reference comparison, catches panics)
test-linku-decode: download-linku
    cargo test --test linku_corpus -- --ignored --nocapture linku_decode_all

# Compare link-u decode output against libavif references
test-linku: download-linku
    cargo test --test linku_corpus -- --ignored --nocapture linku_pixel_parity

# Full link-u pipeline: download, generate references, compare
verify-linku: generate-linku-references test-linku

# G7 (GOAL_PARETO_FRONT): the pareto tripwire — a G1/G2 subset with every
# round-1 hardening encoded (solo walls, sign-safe, 420-pinned, monotone-check,
# banded verdicts, live-checked reference versions). SUBSET_N env sizes it.
gate-pareto:
    bash scripts/gates/gate_pareto.sh

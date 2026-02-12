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

# Format code
fmt:
    cargo fmt

# Format check
fmt-check:
    cargo fmt --check

# Build with encode feature
build-encode:
    cargo build --features encode

# Test with encode feature
test-encode:
    cargo test --features managed,encode

# Clippy with managed + encode + zencodec features
clippy-all:
    cargo clippy --all-targets --features managed,encode,zencodec -- -D warnings

# Full CI check
ci: fmt-check clippy test

# Run example decode_avif with test image
decode-test:
    mkdir -p /mnt/v/output/zenavif/test
    cargo run --release --example decode_avif -- {{justfile_directory()}}/../../aom-decode/tests/test.avif /mnt/v/output/zenavif/test/test.png

# Cross-test i686 (32-bit x86)
test-i686:
    cross test --no-default-features --features managed --target i686-unknown-linux-gnu

# Cross-test armv7 (32-bit ARM)
test-armv7:
    cross test --no-default-features --features managed --target armv7-unknown-linux-gnueabihf

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
    cargo test --features managed --test integration_corpus -- --ignored --nocapture

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
    cargo test --features managed --test pixel_verification -- --ignored --nocapture verify_against_libavif

# Full pixel verification: generate references and test
verify-pixels: generate-references test-pixels

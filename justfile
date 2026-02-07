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

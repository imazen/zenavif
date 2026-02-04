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

# Clean build artifacts
clean:
    cargo clean

# Update dependencies
update:
    cargo update

# Check outdated dependencies
outdated:
    cargo outdated

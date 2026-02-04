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

# Clean build artifacts
clean:
    cargo clean

# Update dependencies
update:
    cargo update

# Check outdated dependencies
outdated:
    cargo outdated

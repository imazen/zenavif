# zenavif-parse dev commands

# Format + regenerate the public-API surface snapshot (docs/public-api/)
fmt:
    cargo fmt --all
    cargo test --test public_api_doc

# Regenerate the public-API surface snapshot only
api-doc:
    cargo test --test public_api_doc

# Verify the committed snapshot is current (what CI runs)
api-doc-check:
    ZEN_API_DOC=check cargo test --test public_api_doc

# Full local test gate
test:
    cargo test --all-features

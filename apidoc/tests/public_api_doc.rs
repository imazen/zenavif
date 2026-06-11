//! Public-API surface snapshots for the PARENT package (docs/public-api/).
//! Shared implementation + format docs: the `zenutils-apidoc` crate.
//!
//! zenavif uses the default configuration: supported surface = default
//! features; features file = all manifest features except `_*`-prefixed
//! internal gates (`_dev` lands in zenavif.internal.txt). zenavif-parse /
//! zenavif-serialize are registry deps from separate repos — they are not
//! workspace members here, so discovery only snapshots zenavif.
#[test]
fn public_api_surface_docs_are_current() {
    zenutils_apidoc::ApiDoc::new().workspace_dir("..").run();
}

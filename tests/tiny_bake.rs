//! A hand-baked, contract-carrying ZNPR v3 model for the integration
//! test's model-path gate.
//!
//! This is **not** the AVIF tuner bake — that one is the training lane's
//! deliverable. This is the smallest model that satisfies
//! [`zenavif::backend_tuner::TuneContract`], so the plumbing between a
//! bake and an encode can be gated before the real weights exist.
//!
//! Serialization goes through `zenpredict_bake` — the canonical ZNPR v3
//! serializer. Hand-emitting the wire format is banned; the alignment,
//! section ordering and header layout live in one place on purpose.

#![allow(dead_code)]

/// ZNPR needs its f32 sections aligned; `include_bytes!`-style `Vec<u8>`
/// gives no guarantee, so route through an aligned wrapper the way
/// zenpicker's own tests do.
#[repr(C, align(16))]
struct Aligned(Vec<u8>);

/// Two cells, one `bytes_log` head, three inputs.
///
/// Cell 0 (`rav1e,speed=6`) is baked to score lower than cell 1
/// (`rav1e,speed=8`), so the argmin has a determinate answer that does
/// not depend on the fixture: a plumbing gate should fail because the
/// plumbing broke, not because a fixture drifted.
///
/// The two source features are **real zenanalyze features** so the
/// tuner's own-pass resolution path is exercised for real — a fake
/// column name would fail resolution and test nothing.
pub fn two_cell_bake() -> Option<Vec<u8>> {
    let json = r#"{
        "schema_hash": 0,
        "scaler_mean":  [0.0, 0.0, 0.0],
        "scaler_scale": [1.0, 1.0, 1.0],
        "layers": [{
            "in_dim": 3, "out_dim": 2, "activation": "identity", "dtype": "f32",
            "weights": [0.0,0.0,0.0, 0.0,0.0,0.0],
            "biases": [1.0, 2.0]
        }],
        "metadata": [
            {"key": "zenavif.tune.cells", "type": "utf8",
             "text": "rav1e,speed=6\nrav1e,speed=8"},
            {"key": "zenavif.tune.heads", "type": "utf8", "text": "bytes_log"},
            {"key": "zenavif.tune.input_order", "type": "utf8",
             "text": "feat_patch_fraction\nfeat_dct_compressibility_y\nzq_norm"}
        ]
    }"#;
    let bytes = zenpredict_bake::bake_from_json_str(json).ok()?;
    // Round-trip through the aligned wrapper so the returned Vec's data
    // pointer came from an aligned allocation.
    Some(Aligned(bytes).0)
}

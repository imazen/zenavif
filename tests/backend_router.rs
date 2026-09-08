#![cfg(feature = "encode")]

use zenavif::{Av1Backend, EncodeBitDepth, EncodeChromaSubsampling, EncoderConfig, PlanInput};
use zenavif::backend_router::query_still_backends;

#[test]
fn unsupported_svt_format_is_filtered_without_rewriting_request() {
    let config = EncoderConfig::new()
        .backend(Av1Backend::Zenav1Svt)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv444);
    let reports = query_still_backends(&config, PlanInput::rgb8(65, 67));
    assert!(!reports.iter().find(|r| r.backend == Av1Backend::Zenav1Svt).unwrap().supported());
    assert!(reports.iter().find(|r| r.backend == Av1Backend::Zenravif).unwrap().supported());
}

#[cfg(feature = "zenav1-aom-encode")]
#[test]
fn twelve_bit_routes_only_to_a_backend_and_wrapper_that_can_code_it() {
    let config = EncoderConfig::new()
        .bit_depth(EncodeBitDepth::Twelve)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420);
    let reports = query_still_backends(&config, PlanInput::rgb8(65, 67));
    let supported: Vec<_> = reports.iter().filter(|r| r.supported()).map(|r| r.backend).collect();
    assert_eq!(supported, vec![Av1Backend::Zenav1Aom]);
}

#[cfg(feature = "zenav1-svt")]
#[test]
fn native_extensions_remain_eligible() {
    let config = EncoderConfig::new()
        .bit_depth(EncodeBitDepth::Ten)
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420);
    let reports = query_still_backends(&config, PlanInput::rgba8(65, 67));
    assert!(reports.iter().find(|r| r.backend == Av1Backend::Zenav1Svt).unwrap().supported());
    assert!(!reports.iter().find(|r| r.backend == Av1Backend::Zenav1Aom).unwrap().supported());
}

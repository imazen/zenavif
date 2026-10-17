//! Incremental (sans-I/O) decode through `dyn_incremental_decoder`: the
//! testkit's incremental contract on committed AVIF files.

use zenavif::AvifDecoderConfig;
use zencodec::decode::DecoderConfig as _;

#[test]
fn incremental_contract_holds() {
    assert!(AvifDecoderConfig::capabilities().incremental());
    for name in [
        "tests/vectors/libavif/kodim03_yuv420_8bpc.avif",
        "tests/vectors/libavif/clap_irot_imir_non_essential.avif",
        "tests/vectors/zenavif/mono_gradient_10b_full.avif",
        "tests/vectors/zenavif/mono_gradient_8b_p3icc.avif",
        "zenavif-parse/tests/gainmap/seine_sdr_gainmap_srgb.avif",
        "zenavif-parse/tests/colors-animated-8bpc.avif",
    ] {
        let path = format!("{}/{name}", env!("CARGO_MANIFEST_DIR"));
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        zencodec_testkit::check_incremental_capability_honesty(AvifDecoderConfig::new(), &data)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
}

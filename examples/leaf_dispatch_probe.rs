//! Save exact decoded bytes, stream bytes and metadata for bounded witnesses.
#![forbid(unsafe_code)]
#[path = "support/leaf_dispatch.rs"]
mod workload;
fn main() {
    let out = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("output directory required"),
    );
    std::fs::create_dir_all(&out).unwrap();
    for (name, data) in workload::fixtures() {
        for kind in 0..3 {
            let parsed = workload::parse(data, kind);
            std::fs::write(
                out.join(format!("parse-{name}-{kind}.txt")),
                workload::metadata(&parsed),
            )
            .unwrap();
        }
        if name != "animation_metadata" {
            let decoded = workload::decode(data);
            let streamed = workload::stream(data);
            assert_eq!(decoded, streamed, "stream/full: {name}");
            std::fs::write(out.join(format!("decode-{name}.raw")), decoded).unwrap();
            std::fs::write(out.join(format!("stream-{name}.raw")), streamed).unwrap();
        }
    }
    #[cfg(feature = "_dev")]
    for (name, pixels) in workload::yuv_witnesses() {
        std::fs::write(out.join(format!("{name}.rgba")), pixels).unwrap();
    }
}

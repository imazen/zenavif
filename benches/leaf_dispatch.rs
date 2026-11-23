//! Parser reader specializations and production strip-decode measurements.
#![forbid(unsafe_code)]
#[path = "../examples/support/leaf_dispatch.rs"]
mod workload;
use zenbench::prelude::*;
fn bench(suite: &mut Suite) {
    for (name, data) in workload::fixtures() {
        let expected = workload::metadata(&workload::parse(data, 0));
        for kind in 0..3 {
            assert_eq!(expected, workload::metadata(&workload::parse(data, kind)));
            suite.group(format!("parse/{name}/reader{kind}"), |g| {
                g.throughput(Throughput::Bytes(data.len() as u64));
                g.bench("parser", move |b| {
                    b.iter(|| black_box(workload::parse(black_box(data), kind)))
                });
            });
        }
        if name != "animation_metadata" {
            let expected = workload::decode(data);
            assert_eq!(expected, workload::stream(data), "stream/full: {name}");
            suite.group(format!("stream/{name}"), |g| {
                g.throughput(Throughput::Bytes(data.len() as u64));
                g.bench("decode", move |b| {
                    b.iter(|| black_box(workload::stream(black_box(data))))
                });
            });
        }
    }
}
zenbench::main!(bench);

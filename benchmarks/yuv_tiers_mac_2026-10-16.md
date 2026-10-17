# AVIF YUV conversion tiers on macOS ARM64, 2026-10-16

Source: `6fa8b645` on M4 Pro. Command: `~/work/claudehints/scripts/run-heavy --mem 12G --jobs 4 -- just arm-tiers-macos`. The recipe ran `cargo bench -p zenavif --features _dev --bench tier_isolation -- --cpu-policy=unrestricted --format=llm`. Full output: `~/tmp/zenavif-tier_isolation.log` on this Mac.

| Conversion size | NEON mean | Forced scalar mean | Scalar versus NEON |
| --- | ---: | ---: | ---: |
| 64 × 64 | 10.54 µs | 9.64 µs | −8.50%, CI −9.05% to −7.96% |
| 512 × 256 | 229.44 µs | 224.41 µs | −1.72%, CI −3.19% to −0.25%; bench marks this nonsignificant |
| 1920 × 1080 | 3.03 ms | 3.05 ms | +0.64%, CI −0.38% to +1.68%; nonsignificant |

This compares one `yuv420_to_rgb8` conversion kernel, not complete AVIF decode. The scheduler was unrestricted and observed CV was 14–23%. At 1080p the tiers are indistinguishable. The small-image scalar advantage needs an end-to-end decode check and another ARM CPU before changing the shared dispatch.

`run-heavy`: peak RSS 0.25 GiB, minimum available memory 5255 MiB, peak load 4.11.

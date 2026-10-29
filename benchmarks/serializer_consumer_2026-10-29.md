# Native serializer consumer qualification

Source `c98928249b06d01baadfe539cb30719312eb2b31`; resolved lock committed
as `521d4823`. Native zenravif pin moves from `aac04acc` to `fab98d6f`.
The only lock change is that source revision. The encoder production Rust source
is unchanged; its serializer dependency moves to `94116d26`, and one test-only
helper gains the existing stop feature gate.

Command in an isolated export outside workspace patches:

```sh
cargo metadata --format-version 1
just serializer-consumer-check
```

All 11 controls passed: animation admission (5), exact animation timing (2),
and encode contracts (4). Strict library Clippy passed. Features: `encode-imazen`
plus defaults. SVT-specific branches are not covered by this gate. The existing
assertions, ignored-test policy and timing/precision requirements are unchanged.

Receipt on r7900x: `run-heavy: done rc=0 160s | peak-RSS 0.99GiB | min-avail 13782MiB | peak-load 2.05`.
This includes dependency compilation and is not an encoding performance result.

The root patch selects this workspace's serializer. Downstream roots must align
their own serializer/parse sources and regenerate their locks; this test is not
proof of the complete portable zenpipe graph. Fuzz, full API and platform gates
remain separate. No crate was published.

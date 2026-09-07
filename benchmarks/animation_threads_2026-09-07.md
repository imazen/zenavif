# Animation worker forwarding — 2026-09-07

Owner revision: cavif-rs `939fa5a3e6f0717837ff7cd6d314a5644b9abff3`.
Previous canonical pin: `2b9d4335950a952b9faa0bcd624eebf7820e714a`.

The animation owner accepted the requested worker count but created every
encoding context without applying it. The shared sequence encoder now applies
the same worker policy used by still encoding: explicit counts install the
requested pool, zero resolves the current Rayon count, and None retains the
backend default. This covers color and alpha at both coding depths.

The owner test exercises two 65×67 frames in RGB8, RGBA8, RGB16 and RGBA16,
with worker settings 1, 2, 0 and None. It observes actual worker-pool execution
through the cancellation callback and checks complete AVIF byte identity
across settings. Removing only the forwarding call fails the final test:
no worker pool is observed when one worker was requested. Restoring it passes.
No tiling or coding-tool settings were changed.

The canonical regression in `tests/animation_encode_threads.rs` exercises
RGBA8 input at coded depths 8 and 10, with one and two workers. Against the
previous dependency it fails with observed workers [] versus [1]. Against the
published corrected dependency it passes and compares complete AVIF bytes.

Owner validation passes 91 all-feature tests (five existing ignored doctests),
58 no-default-feature tests (four existing ignored doctests), and all-target
all-feature clippy. An initial unrestricted Rust test-harness run hit the
existing five-second still-image timeout test; that test passed alone in
0.01 seconds. The full suite passed with RUST_TEST_THREADS=4. The timeout and
assertion remain unchanged. The shared heavy-job wrapper limits memory to
16G and build/Rayon parallelism to four; Rust test parallelism is also four.

Canonical published-Git integration passes 900/900 all-feature workspace
tests (nine existing skips) and all-feature workspace library clippy with
warnings denied. The fetched owner manifest and eight Rust source files match
the locally verified owner byte-for-byte. The focused canonical test passes
all four depth/worker settings. This change does not remeasure or resolve the
previously recorded still-image ladder and monotonicity failures.
Logs: `~/tmp/slower-preset-probe/animation-threads-*.log`.
The separate quality-ladder investigation remains open; no quality envelope,
expectation or skip was changed, and the wrapper main branches remain pending.

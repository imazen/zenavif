//! Thread-count policy shared by the encode and decode adapters: lowering a
//! [`zencodec::ThreadingPolicy`] to a concrete count, and fitting the encoder
//! thread count to the memory budget.

#[cfg(feature = "encode")]
use whereat::{At, at};
#[cfg(feature = "encode")]
use zencodec::ResourceLimits;

#[cfg(feature = "encode")]
use crate::error::Error;

/// Convert a [`zencodec::ThreadingPolicy`] to a concrete thread count.
///
/// Returns the thread count to pass to rav1e/ravif (encode) or dav1d/rav1d (decode).
/// - `0` means "auto" (let the library pick based on available parallelism).
/// - `1` means single-threaded.
/// - Any other value is the requested thread count.
pub(super) fn policy_to_threads(policy: zencodec::ThreadingPolicy) -> u32 {
    match policy {
        zencodec::ThreadingPolicy::Sequential => 1,
        zencodec::ThreadingPolicy::Parallel => 0, // 0 = auto
        // The enum is #[non_exhaustive] and includes deprecated legacy variants
        // (SingleThread, LimitOrSingle, LimitOrAny, Balanced, Unlimited). 0
        // (auto) is the safe default for any of those — the deprecated arms
        // emit warnings at the construction site, which is where they should
        // be fixed.
        _ => 0,
    }
}

#[cfg(test)]
mod policy_tests {
    use super::policy_to_threads;

    /// The policy → thread-count lowering, asserted on the mapping itself.
    ///
    /// It measured 0 of 6 regions in every feature combo (cargo-llvm-cov,
    /// 2026-08-11): `effective_config` only calls it for a non-`Parallel`
    /// policy and `ResourceLimits::default()` is `Parallel`, so nothing
    /// exercised the lowering. A decode-level test cannot pin this — the
    /// thread count is deliberately invisible in the output pixels
    /// (tests/cov_zencodec.rs asserts that invariance separately), so the
    /// mapping has to be asserted here or not at all.
    #[test]
    fn policy_lowering_is_exact() {
        assert_eq!(
            policy_to_threads(zencodec::ThreadingPolicy::Sequential),
            1,
            "Sequential must mean one thread, not auto"
        );
        assert_eq!(
            policy_to_threads(zencodec::ThreadingPolicy::Parallel),
            0,
            "Parallel must mean auto (0), letting the decoder pick"
        );
        // Deprecated legacy variants lower to auto; the deprecation warning
        // belongs at the caller's construction site.
        #[allow(deprecated)]
        for legacy in [
            zencodec::ThreadingPolicy::SingleThread,
            zencodec::ThreadingPolicy::Balanced,
            zencodec::ThreadingPolicy::Unlimited,
            zencodec::ThreadingPolicy::LimitOrSingle { max_threads: 4 },
            zencodec::ThreadingPolicy::LimitOrAny {
                preferred_max_threads: 4,
            },
        ] {
            assert_eq!(
                policy_to_threads(legacy),
                0,
                "legacy policy {legacy:?} must lower to auto"
            );
        }
    }
}

/// Memory-adaptive concurrency pre-flight shared by the still and animation
/// encode paths: fit the encoder thread count to the memory budget, verify
/// the calibrated thread-aware estimate at the chosen count, and return the
/// thread pin (`Some(n)` only when a reduction is needed) plus the reduction
/// note (never silent).
///
/// Budget semantics (see `crate::heuristics::fit_threads_to_budget`):
/// * explicit `ResourceLimits::max_memory_bytes` is a hard budget — when even
///   the single-threaded conservative peak
///   (`EncodeEstimate::peak_memory_bytes_max`) exceeds it, this errors with
///   the memory-limit error (thread reduction cannot shrink a single-thread
///   peak);
/// * with no explicit limit, 80 % of detected available RAM (Linux
///   `MemAvailable`; no implicit cap elsewhere) bounds the thread choice, and
///   an encode that cannot fit even single-threaded errors with a hint to set
///   `max_memory_bytes` — a clean error beats the kernel OOM-killing the
///   process (measured on 32 GB boxes).
///
/// `bpp` is the caller's input-buffer bytes-per-pixel, which is also the
/// calibrated estimate stratum (3/4/6/8). The f32 paths pass 12/16, which the
/// model treats as ≥ 6 (10-bit stratum) — an over-estimate of their actual
/// 8-bit re-encode, i.e. conservative in the safe direction. The gray path
/// passes 1, a slight under-estimate of its RGB expansion (working-set term
/// dominates; the 2 B/px difference is ~5 %).
#[cfg(feature = "encode")]
pub(super) fn fit_encode_threads_to_memory(
    limits: &ResourceLimits,
    config: &crate::EncoderConfig,
    w: u32,
    h: u32,
    bpp: u8,
) -> Result<(Option<usize>, Option<String>), At<Error>> {
    use crate::heuristics as hx;
    let speed = config.speed_value();
    let requested = config.threads;
    let explicit = limits.max_memory_bytes;
    let budget = explicit.or_else(hx::implicit_memory_budget);
    let (pin, note) = hx::fit_threads_to_budget(w, h, bpp, speed, requested, budget);
    let chosen = pin.unwrap_or_else(|| hx::requested_or_default_threads(requested));
    if let (Some(budget_bytes), Some(est)) = (
        budget,
        hx::estimate_encode_threaded(w, h, bpp, speed, chosen),
    ) && est.peak_memory_bytes_max > budget_bytes
    {
        // The fit already walked to the floor: `chosen` is 1 (or the
        // caller explicitly requested 1), so this encode does not fit
        // the budget at ANY thread count.
        return Err(match limits.check_memory(est.peak_memory_bytes_max) {
            // Explicit limit: the standard `LimitExceeded::Memory`
            // actual/max figures, with context appended.
            Err(e) => at!(Error::ResourceLimit(format!(
                "{e} (calibrated AVIF encode peak estimate: exceeds \
                 max_memory_bytes even single-threaded; reduce dimensions \
                 or raise the limit)"
            ))),
            // No explicit limit (check_memory passes vacuously): the
            // implicit available-RAM budget, with the override hint.
            Ok(()) => at!(Error::ResourceLimit(format!(
                "calibrated AVIF encode peak estimate {est_max} B exceeds the \
                 implicit memory budget {budget_bytes} B (80% of detected \
                 available RAM) even single-threaded; set \
                 ResourceLimits::max_memory_bytes to choose the budget explicitly",
                est_max = est.peak_memory_bytes_max,
            ))),
        });
    }
    Ok((pin, note))
}

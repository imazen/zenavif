//! Allocation helpers honoring the allocation-fallibility preference per call
//! site.
//!
//! An AVIF decode mixes two allocation regimes:
//!
//! * **Big, untrusted-sized buffers** (the full-image RGB(A) output buffer, the
//!   grid-stitch canvas, the crop destination — all sized from the decoded AV1
//!   frame / grid dimensions) default to the *fallible* `try_reserve` path. A
//!   malicious container can declare a huge grid, so we want a graceful
//!   [`Error::ResourceLimit`](crate::error::Error::ResourceLimit) rather than an
//!   allocator abort.
//! * **Small, bounded scratch** (one RGB row used by the per-row YUV→RGB
//!   kernels) defaults to the *infallible* `vec!` path — a single `calloc` is
//!   faster and the size is bounded by the image width, not by an unbounded
//!   attacker-controlled quantity.
//!
//! The preference is a **3-mode, per-site override** of that default:
//! [`AllocPref::Fallible`] / [`AllocPref::Infallible`] force one path
//! everywhere; [`AllocPref::CodecDefault`] keeps each site's own default. The
//! helper signatures therefore take the resolved preference *and* the site
//! default and reconcile them together.
//!
//! This is a crate-local enum (no `zencodec` dependency) so the always-compiled
//! decode path stays decoupled from the optional `zencodec` integration — the
//! `codec` module maps `zencodec::AllocPreference` onto it only at the trait
//! boundary, exactly the way the local
//! [`ThreadingInfo`](crate::heuristics::ThreadingInfo) is mapped onto
//! `zencodec::estimate::ThreadingInformation`.

use whereat::{At, at};

use crate::error::Error;

/// Crate-local mirror of `zencodec::AllocPreference`.
///
/// Kept independent of the optional `zencodec` dependency so the core decode
/// path (which is always compiled) does not gain a hard `zencodec`
/// dependency. The `codec` module converts from `zencodec::AllocPreference`
/// via [`From`] at the trait boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AllocPref {
    /// Let each call site keep its own default fallibility. Preserves existing
    /// behavior. This is the default.
    #[default]
    CodecDefault,
    /// Force the fallible (`try_reserve`) path everywhere: a graceful
    /// out-of-memory error instead of an abort. Prefer for untrusted input.
    Fallible,
    /// Force the infallible (`vec!` / `Vec::with_capacity`) path everywhere:
    /// faster (a single `calloc` for the zeroed case) at the cost of aborting
    /// on OOM. Prefer for trusted sizes and benchmarks.
    Infallible,
}

#[cfg(feature = "zencodec")]
impl From<zencodec::AllocPreference> for AllocPref {
    fn from(pref: zencodec::AllocPreference) -> Self {
        match pref {
            zencodec::AllocPreference::Fallible => AllocPref::Fallible,
            zencodec::AllocPreference::Infallible => AllocPref::Infallible,
            // `CodecDefault` and any future `#[non_exhaustive]` variant keep the
            // codec's own per-site defaults.
            _ => AllocPref::CodecDefault,
        }
    }
}

/// Resolve the 3-mode [`AllocPref`] against THIS site's default fallibility.
///
/// * [`Fallible`](AllocPref::Fallible) → always `true`.
/// * [`Infallible`](AllocPref::Infallible) → always `false`.
/// * [`CodecDefault`](AllocPref::CodecDefault) → the site default, unchanged.
#[inline]
#[must_use]
pub(crate) fn resolve_fallible(pref: AllocPref, site_default_fallible: bool) -> bool {
    match pref {
        AllocPref::Fallible => true,
        AllocPref::Infallible => false,
        AllocPref::CodecDefault => site_default_fallible,
    }
}

/// Allocate `n` elements of `T` (each zero-initialized via `fill`), honoring
/// the per-site fallibility.
///
/// `pref` is the caller's [`AllocPref`]; `site_default_fallible` is this site's
/// default when `pref` is `CodecDefault`.
///
/// * fallible → `try_reserve_exact` then fill, returning
///   [`Error::ResourceLimit`](crate::error::Error::ResourceLimit) on allocation
///   failure (no abort).
/// * infallible → `vec![fill; n]` (aborts on OOM).
///
/// `fill` must be `Copy` so both paths produce byte-identical contents.
pub(crate) fn alloc_filled<T: Copy>(
    pref: AllocPref,
    site_default_fallible: bool,
    fill: T,
    n: usize,
) -> Result<Vec<T>, At<Error>> {
    if resolve_fallible(pref, site_default_fallible) {
        let mut v: Vec<T> = Vec::new();
        v.try_reserve_exact(n).map_err(|_| {
            at!(Error::ResourceLimit(format!(
                "out of memory allocating {} bytes",
                n.saturating_mul(core::mem::size_of::<T>())
            )))
        })?;
        v.resize(n, fill);
        Ok(v)
    } else {
        Ok(vec![fill; n])
    }
}

/// Allocate an empty `Vec<T>` with reserved capacity for `cap` elements,
/// honoring the per-site fallibility (for the `Vec::with_capacity` + extend
/// sites).
///
/// * fallible → `try_reserve_exact`, returning
///   [`Error::ResourceLimit`](crate::error::Error::ResourceLimit) on allocation
///   failure.
/// * infallible → `Vec::with_capacity(cap)` (aborts on OOM).
///
/// The returned `Vec` is empty (length 0); the caller fills it.
pub(crate) fn vec_with_capacity<T>(
    pref: AllocPref,
    site_default_fallible: bool,
    cap: usize,
) -> Result<Vec<T>, At<Error>> {
    if resolve_fallible(pref, site_default_fallible) {
        let mut v: Vec<T> = Vec::new();
        v.try_reserve_exact(cap).map_err(|_| {
            at!(Error::ResourceLimit(format!(
                "out of memory allocating {} bytes",
                cap.saturating_mul(core::mem::size_of::<T>())
            )))
        })?;
        Ok(v)
    } else {
        Ok(Vec::with_capacity(cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `CodecDefault` keeps each site's own default fallibility.

    #[test]
    fn codec_default_keeps_site_default_true() {
        // Big-buffer site (default fallible): CodecDefault stays fallible.
        assert!(resolve_fallible(AllocPref::CodecDefault, true));
    }

    #[test]
    fn codec_default_keeps_site_default_false() {
        // Small-scratch site (default infallible): CodecDefault stays infallible.
        assert!(!resolve_fallible(AllocPref::CodecDefault, false));
    }

    #[test]
    fn explicit_fallible_overrides_any_site_default() {
        assert!(resolve_fallible(AllocPref::Fallible, false));
        assert!(resolve_fallible(AllocPref::Fallible, true));
    }

    #[test]
    fn explicit_infallible_overrides_any_site_default() {
        assert!(!resolve_fallible(AllocPref::Infallible, true));
        assert!(!resolve_fallible(AllocPref::Infallible, false));
    }

    #[test]
    fn alloc_filled_all_modes_equal_bytes() {
        let a = alloc_filled(AllocPref::CodecDefault, true, 0u8, 4096).unwrap();
        let b = alloc_filled(AllocPref::Infallible, true, 0u8, 4096).unwrap();
        let c = alloc_filled(AllocPref::Fallible, false, 0u8, 4096).unwrap();
        assert_eq!(a.len(), 4096);
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert!(a.iter().all(|&x| x == 0));
    }

    #[test]
    fn alloc_filled_nonzero_fill() {
        // A non-zero fill (the U/V plane 128 default) must round-trip on both
        // paths identically.
        let a = alloc_filled(AllocPref::Fallible, true, 128u8, 256).unwrap();
        let b = alloc_filled(AllocPref::Infallible, false, 128u8, 256).unwrap();
        assert_eq!(a, b);
        assert!(a.iter().all(|&x| x == 128));
    }

    #[test]
    fn vec_with_capacity_reserves_and_is_empty() {
        let a: Vec<u8> = vec_with_capacity(AllocPref::Infallible, false, 1024).unwrap();
        let b: Vec<u8> = vec_with_capacity(AllocPref::Fallible, false, 1024).unwrap();
        assert_eq!(a.len(), 0);
        assert_eq!(b.len(), 0);
        assert!(a.capacity() >= 1024);
        assert!(b.capacity() >= 1024);
    }

    #[test]
    fn alloc_filled_fallible_oom_returns_err() {
        // Request an impossibly large allocation; the fallible path must
        // return Err (mapped to ResourceLimit) rather than abort.
        let r = alloc_filled(AllocPref::Fallible, true, 0u8, usize::MAX / 2);
        assert!(r.is_err());
        assert!(matches!(r.unwrap_err().error(), Error::ResourceLimit(_)));
    }

    #[test]
    fn vec_with_capacity_fallible_oom_returns_err() {
        let r: Result<Vec<u8>, _> = vec_with_capacity(AllocPref::Fallible, true, usize::MAX / 2);
        assert!(r.is_err());
        assert!(matches!(r.unwrap_err().error(), Error::ResourceLimit(_)));
    }

    #[cfg(feature = "zencodec")]
    #[test]
    fn from_zencodec_alloc_preference_maps_all_modes() {
        assert_eq!(
            AllocPref::from(zencodec::AllocPreference::Fallible),
            AllocPref::Fallible
        );
        assert_eq!(
            AllocPref::from(zencodec::AllocPreference::Infallible),
            AllocPref::Infallible
        );
        assert_eq!(
            AllocPref::from(zencodec::AllocPreference::CodecDefault),
            AllocPref::CodecDefault
        );
    }
}

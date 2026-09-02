//! Decoder configuration

/// Configuration for AVIF decoding
#[derive(Debug, Clone)]
pub struct DecoderConfig {
    /// Number of threads to use for decoding (0 = auto)
    pub(crate) threads: u32,
    /// Whether to apply film grain synthesis
    pub(crate) apply_grain: bool,
    /// Maximum decoded frame size in total pixels (width * height).
    ///
    /// Enforced pre-flight against the container's declared dimensions, before
    /// any decode work or frame allocation. Defaults to 120_000_000 (120 MP,
    /// admits ~108 MP photos) so untrusted decode is bounded by default.
    /// `0` is the explicit opt-out — no decode-side pixel limit.
    pub(crate) frame_size_limit: u32,
    /// CPU feature flags mask (bitwise AND with detected features).
    /// Use to disable SIMD paths for testing. Default: all enabled.
    /// x86_64: bit 3 = AVX2, bit 2 = SSE4.1, bit 1 = SSSE3, bit 0 = SSE2
    pub(crate) cpu_flags_mask: u32,
    /// Parser peak memory limit in bytes (forwarded to zenavif-parse).
    pub(crate) parser_peak_memory_limit: Option<u64>,
    /// Parser total megapixels limit (forwarded to zenavif-parse).
    pub(crate) parser_total_megapixels_limit: Option<u32>,
    /// Parser max animation frames (forwarded to zenavif-parse).
    pub(crate) parser_max_animation_frames: Option<u32>,
    /// When true, 10/12-bit AV1 content is downscaled to 8-bit RGB output.
    /// Most AVIF encoders (including zenravif) default to 10-bit encoding
    /// even for 8-bit input. This option returns 8-bit output for those files.
    pub(crate) prefer_8bit: bool,
    /// Which AV1 decode kernel serves the container decode paths
    /// (primary/alpha/gain-map items). Default [`crate::DecodeBackend::Rav1dSafe`].
    /// See [`DecoderConfig::decode_backend`] for the zenav1-aom scope caveats.
    pub(crate) decode_backend: crate::DecodeBackend,
    /// Allocation-fallibility preference for zenavif's *own* decode buffers
    /// (the full-image RGB(A) output, the grid-stitch canvas, the crop
    /// destination, and the per-row YUV→RGB scratch). `CodecDefault` keeps each
    /// site's own default (big untrusted buffers fallible, small scratch
    /// infallible). Set from `zencodec::ResourceLimits::prefer_fallible_allocations`
    /// at the `codec` trait boundary; the direct (non-`zencodec`) decode API
    /// leaves it `CodecDefault` so behavior is unchanged. Does not affect the
    /// AV1 frame/tile buffers, which live in the `rav1d-safe` dependency.
    pub(crate) alloc_pref: crate::alloc_util::AllocPref,
}

impl Default for DecoderConfig {
    fn default() -> Self {
        Self {
            // Default to auto-detect threads. For single-frame AVIF, rav1d
            // auto-derives max_frame_delay=1, giving tile parallelism without
            // frame threading overhead.
            threads: 0,
            apply_grain: true,
            // Bound untrusted decode by default: 120 MP (admits ~108 MP photos).
            // The enforcement at `decoder_managed.rs` / `decoder.rs` only fires
            // when this is > 0, so this default is what makes the pre-flight
            // dimension check actually run for `decode()` / `DecoderConfig::default()`.
            // Use `frame_size_limit(0)` to opt out (unbounded).
            frame_size_limit: 120_000_000,
            cpu_flags_mask: u32::MAX,
            // `None` here does NOT mean "unbounded": the parser is constructed
            // from `zenavif_parse::DecodeConfig::default()`, which carries its
            // own sane caps (512 MP / 1 GB peak / 10k frames / 1k tiles). These
            // overrides only *tighten* below the parser's defaults when set.
            parser_peak_memory_limit: None,
            parser_total_megapixels_limit: None,
            parser_max_animation_frames: None,
            prefer_8bit: false,
            decode_backend: crate::DecodeBackend::Rav1dSafe,
            alloc_pref: crate::alloc_util::AllocPref::CodecDefault,
        }
    }
}

impl DecoderConfig {
    /// Create a new decoder configuration with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the number of threads for decoding
    ///
    /// 0 means automatic (use available cores)
    pub fn threads(mut self, threads: u32) -> Self {
        self.threads = threads;
        self
    }

    /// Enable or disable film grain synthesis
    ///
    /// When enabled (default), film grain specified in the AV1 stream
    /// will be synthesized and applied to the decoded image.
    pub fn apply_grain(mut self, apply: bool) -> Self {
        self.apply_grain = apply;
        self
    }

    /// Set maximum frame size limit in total pixels
    ///
    /// If width * height exceeds this limit, decoding fails pre-flight with
    /// [`Error::ImageTooLarge`](crate::Error::ImageTooLarge) before any frame
    /// is allocated. Defaults to `120_000_000` (120 MP). Pass `0` to opt out
    /// (no decode-side pixel limit).
    pub fn frame_size_limit(mut self, limit: u32) -> Self {
        self.frame_size_limit = limit;
        self
    }

    /// Set CPU feature flags mask.
    ///
    /// **Currently inert** (zenavif#18 docs audit): the value is stored and
    /// validated but not yet threaded into SIMD dispatch — the safe decode
    /// path selects its SIMD tier through archmage token detection
    /// unconditionally. Kept on the builder so wiring it up later is not a
    /// breaking change; do not rely on it to force scalar decode today.
    ///
    /// Intended semantics (once wired): mask detected CPU features.
    /// Default is `u32::MAX` (all features enabled).
    ///
    /// # x86_64 flag bits
    /// - `1 << 0` = SSE2
    /// - `1 << 1` = SSSE3
    /// - `1 << 2` = SSE4.1
    /// - `1 << 3` = AVX2
    /// - `1 << 4` = AVX-512 ICL
    ///
    /// Setting to `0` would force scalar-only decode; `0b0111` (7) would allow
    /// up to SSE4.1 but disable AVX2.
    pub fn cpu_flags_mask(mut self, mask: u32) -> Self {
        self.cpu_flags_mask = mask;
        self
    }

    /// Downscale 10/12-bit AV1 output to 8-bit RGB.
    ///
    /// Default: `false`. Enable when decoding files encoded at 10-bit from
    /// 8-bit sources and you want 8-bit output without an external conversion.
    pub fn prefer_8bit(mut self, prefer: bool) -> Self {
        self.prefer_8bit = prefer;
        self
    }

    /// Select the AV1 decode kernel for the container decode paths
    /// (primary + alpha + gain-map items). Default
    /// [`crate::DecodeBackend::Rav1dSafe`].
    ///
    /// [`crate::DecodeBackend::Zenav1Aom`] (feature `zenav1-aom`, EXPERIMENTAL)
    /// routes item decodes through the zenav1-aom pure-Rust decoder —
    /// byte-identical to rav1d-safe on every still tested (8/10/12-bit,
    /// mono, 4:2:0/4:2:2/4:4:4, film grain, all conformance + sweep cells).
    /// Scope caveats: still images only (animation decode returns
    /// [`Error::Unsupported`](crate::Error::Unsupported) — its inter-frame
    /// envelope is in progress), single-threaded, and ~1.4x slower than
    /// rav1d-safe at 8-bit. Not yet recommended for untrusted input in
    /// production (fuzz hardening in progress upstream).
    ///
    /// [`crate::DecodeBackend::Rav1dFfi`] (feature `unsafe-asm`) is NOT
    /// accepted here — it remains a raw-OBU benchmark arm only.
    pub fn decode_backend(mut self, backend: crate::DecodeBackend) -> Self {
        self.decode_backend = backend;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for imazen/zenavif#22 (decode fail-open): the default decode
    /// pixel cap must be 120 MP, not `0` (unbounded). Because the pre-flight
    /// dimension check in `decoder_managed.rs` / `decoder.rs` only fires when
    /// `frame_size_limit > 0`, a `0` default left untrusted `decode()` unbounded.
    #[test]
    fn default_frame_size_limit_is_120mp_not_unbounded() {
        assert_eq!(
            DecoderConfig::default().frame_size_limit,
            120_000_000,
            "default decode frame_size_limit must be 120 MP so untrusted decode is bounded (zenavif#22)"
        );
        assert_eq!(DecoderConfig::new().frame_size_limit, 120_000_000);
    }

    /// The explicit opt-out must still work: `frame_size_limit(0)` disables the
    /// decode-side pixel cap (preserves the documented `0 = unlimited` escape).
    #[test]
    fn frame_size_limit_zero_opts_out() {
        let cfg = DecoderConfig::default().frame_size_limit(0);
        assert_eq!(
            cfg.frame_size_limit, 0,
            "0 must remain the explicit opt-out"
        );
    }
}

//! Decoder configuration

/// Configuration for AVIF decoding
#[derive(Debug, Clone)]
pub struct DecoderConfig {
    /// Number of threads to use for decoding (0 = auto)
    pub(crate) threads: u32,
    /// Whether to apply film grain synthesis
    pub(crate) apply_grain: bool,
    /// Maximum frame size limit in pixels (0 = no limit)
    pub(crate) frame_size_limit: u32,
    /// CPU feature flags mask (bitwise AND with detected features).
    /// Use to disable SIMD paths for testing. Default: all enabled.
    /// x86_64: bit 3 = AVX2, bit 2 = SSE4.1, bit 1 = SSSE3, bit 0 = SSE2
    pub(crate) cpu_flags_mask: u32,
}

impl Default for DecoderConfig {
    fn default() -> Self {
        Self {
            // Default to 1 thread — AVIF is a single-frame format where frame
            // threading provides no benefit (and causes DisjointMut overlap
            // issues with the current guard tracking). Tile threading within
            // the single frame still works fine.
            threads: 1,
            apply_grain: true,
            frame_size_limit: 0,
            cpu_flags_mask: u32::MAX,
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
    /// If width * height exceeds this limit, decoding will fail.
    /// 0 means no limit.
    pub fn frame_size_limit(mut self, limit: u32) -> Self {
        self.frame_size_limit = limit;
        self
    }

    /// Set CPU feature flags mask.
    ///
    /// Controls which SIMD code paths are used by masking detected CPU features.
    /// Default is `u32::MAX` (all features enabled).
    ///
    /// # x86_64 flag bits
    /// - `1 << 0` = SSE2
    /// - `1 << 1` = SSSE3
    /// - `1 << 2` = SSE4.1
    /// - `1 << 3` = AVX2
    /// - `1 << 4` = AVX-512 ICL
    ///
    /// Setting to `0` forces scalar-only decode. Setting to `0b0111` (7) allows
    /// up to SSE4.1 but disables AVX2.
    pub fn cpu_flags_mask(mut self, mask: u32) -> Self {
        self.cpu_flags_mask = mask;
        self
    }
}

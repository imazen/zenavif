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
}

impl Default for DecoderConfig {
    fn default() -> Self {
        Self {
            threads: 0,
            apply_grain: true,
            frame_size_limit: 0,
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
}

//! AVIF source analysis and quality estimation.
//!
//! Extracts metadata from AVIF container and AV1 bitstream headers to
//! estimate the encoding quality and provide re-encoding recommendations,
//! without decoding pixels.
//!
//! # Example
//!
//! ```rust,ignore
//! use zenavif::detect::{probe, QualityEstimate};
//!
//! let avif_data = std::fs::read("photo.avif").unwrap();
//! let info = probe(&avif_data).unwrap();
//!
//! println!("{}x{}, {} bit", info.width, info.height, info.bit_depth);
//! println!("Chroma: {:?}", info.chroma_sampling);
//! println!("Lossless: {:?}", info.lossless);
//!
//! if let Some(q) = &info.quality {
//!     println!("Estimated quality: {:.0} (QP {})", q.estimated_quality, q.quantizer);
//! }
//! ```

/// Result of probing an AVIF file.
#[derive(Debug, Clone)]
pub struct AvifProbe {
    /// Image width.
    pub width: u32,
    /// Image height.
    pub height: u32,
    /// Bit depth (8, 10, or 12).
    pub bit_depth: u8,
    /// AV1 profile (0=Main, 1=High, 2=Professional).
    pub profile: u8,
    /// Whether the image is monochrome.
    pub monochrome: bool,
    /// Chroma subsampling.
    pub chroma_sampling: ChromaSampling,
    /// Whether the image has alpha.
    pub has_alpha: bool,
    /// Whether the image is animated.
    pub has_animation: bool,
    /// Whether the encoding is lossless.
    /// `None` if the frame header could not be parsed to determine this.
    pub lossless: Option<bool>,
    /// Quality estimate from the AV1 quantizer, if extractable.
    pub quality: Option<QualityEstimate>,
    /// Color primaries (CICP).
    pub color_primaries: Option<u8>,
    /// Transfer characteristics (CICP).
    pub transfer_characteristics: Option<u8>,
    /// Matrix coefficients (CICP).
    pub matrix_coefficients: Option<u8>,
    /// Whether full range is used.
    pub full_range: Option<bool>,
    /// ICC color profile, if present.
    pub has_icc_profile: bool,
    /// Recommendations for re-encoding.
    pub recommendations: Vec<Recommendation>,
}

/// Quality estimation from AV1 quantizer parameters.
#[derive(Debug, Clone)]
pub struct QualityEstimate {
    /// AV1 base quantizer index (0-255). Lower = higher quality.
    pub quantizer: u8,
    /// Estimated quality on a 0-100 scale. Higher = better.
    ///
    /// This is a rough mapping from the AV1 QP. Different encoders
    /// (libaom, SVT-AV1, rav1e) map quality differently, so this is
    /// approximate.
    pub estimated_quality: f32,
    /// Confidence in the estimate.
    pub confidence: Confidence,
}

/// Confidence level of the quality estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Extracted from frame header — reliable.
    FromFrameHeader,
    /// Estimated from container metadata — rough.
    Approximate,
}

/// Chroma subsampling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSampling {
    /// 4:4:4 (no subsampling).
    Yuv444,
    /// 4:2:2 (horizontal subsampling).
    Yuv422,
    /// 4:2:0 (horizontal and vertical subsampling).
    Yuv420,
    /// Monochrome (no chroma planes).
    Monochrome,
}

/// Re-encoding recommendations for AVIF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recommendation {
    /// Source is high quality — re-encode at lower quality for size savings.
    ReduceQuality,
    /// Source uses 4:4:4 — 4:2:0 would be much smaller for photos.
    UseChromaSubsampling,
    /// Source is 10/12-bit — 8-bit is sufficient for SDR content.
    ReduceBitDepth,
    /// Source quality is already low — avoid re-encoding to prevent
    /// generation loss.
    AvoidReencoding,
}

/// Errors that can occur during AVIF probing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProbeError {
    /// Data is too short to be an AVIF file.
    TooShort,
    /// Not an AVIF/HEIF file.
    NotAvif,
    /// Container is truncated or malformed.
    Truncated,
    /// Could not find AV1 codec configuration.
    NoAv1Config,
}

impl core::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort => write!(f, "data too short to be an AVIF file"),
            Self::NotAvif => write!(f, "not an AVIF file"),
            Self::Truncated => write!(f, "truncated AVIF file"),
            Self::NoAv1Config => write!(f, "no AV1 codec configuration found"),
        }
    }
}

impl std::error::Error for ProbeError {}

/// Probe an AVIF file from its raw bytes.
///
/// Parses the ISOBMFF container and AV1 bitstream to extract image properties,
/// estimate quality, and detect lossy/lossless encoding. No pixel decoding is
/// performed.
pub fn probe(data: &[u8]) -> Result<AvifProbe, ProbeError> {
    if data.len() < 12 {
        return Err(ProbeError::TooShort);
    }

    // Quick ftyp check before handing off to zenavif-parse
    if &data[4..8] != b"ftyp" {
        return Err(ProbeError::NotAvif);
    }

    let parser = zenavif_parse::AvifParser::from_bytes(data).map_err(|e| match e {
        zenavif_parse::Error::UnexpectedEOF => ProbeError::Truncated,
        zenavif_parse::Error::InvalidData(_) => ProbeError::NotAvif,
        _ => ProbeError::Truncated,
    })?;

    // Extract container-level metadata
    let has_alpha = parser.alpha_data().is_some();
    let has_animation = parser.animation_info().is_some();

    // Parse CICP / ICC from colr box
    let (color_primaries, transfer_characteristics, matrix_coefficients, full_range, has_icc) =
        match parser.color_info() {
            Some(zenavif_parse::ColorInformation::Nclx {
                color_primaries: cp,
                transfer_characteristics: tc,
                matrix_coefficients: mc,
                full_range: fr,
            }) => (
                Some(*cp as u8),
                Some(*tc as u8),
                Some(*mc as u8),
                Some(*fr),
                false,
            ),
            Some(zenavif_parse::ColorInformation::IccProfile(_)) => (None, None, None, None, true),
            None => (None, None, None, None, false),
        };

    // Parse AV1 bitstream for sequence header + frame header
    let primary_data = parser.primary_data().map_err(|_| ProbeError::NoAv1Config)?;
    let meta = zenavif_parse::AV1Metadata::parse_av1_bitstream(&primary_data)
        .map_err(|_| ProbeError::NoAv1Config)?;

    let width = meta.max_frame_width.get();
    let height = meta.max_frame_height.get();
    let bit_depth = meta.bit_depth;
    let profile = meta.seq_profile;
    let monochrome = meta.monochrome;

    let cs = meta.chroma_subsampling;
    let chroma_sampling = if monochrome {
        ChromaSampling::Monochrome
    } else if cs.horizontal && cs.vertical {
        ChromaSampling::Yuv420
    } else if cs.horizontal {
        ChromaSampling::Yuv422
    } else {
        ChromaSampling::Yuv444
    };

    // Lossless detection from frame header
    let lossless = meta.lossless;

    // Quality from frame header QP
    let quality = meta.base_q_idx.map(|qp| QualityEstimate {
        quantizer: qp,
        estimated_quality: qp_to_quality(qp),
        confidence: Confidence::FromFrameHeader,
    });

    // Build recommendations
    let mut recommendations = Vec::new();

    if lossless == Some(true) {
        // No quality reduction recommendations for lossless
    } else {
        if let Some(ref q) = quality {
            if q.estimated_quality > 85.0 {
                recommendations.push(Recommendation::ReduceQuality);
            }
            if q.estimated_quality < 30.0 {
                recommendations.push(Recommendation::AvoidReencoding);
            }
        }
    }

    if chroma_sampling == ChromaSampling::Yuv444 && !monochrome && lossless != Some(true) {
        recommendations.push(Recommendation::UseChromaSubsampling);
    }

    if bit_depth > 8 {
        let is_hdr = transfer_characteristics
            .map(|tc| tc == 16 || tc == 18) // PQ or HLG
            .unwrap_or(false);
        if !is_hdr {
            recommendations.push(Recommendation::ReduceBitDepth);
        }
    }

    Ok(AvifProbe {
        width,
        height,
        bit_depth,
        profile,
        monochrome,
        chroma_sampling,
        has_alpha,
        has_animation,
        lossless,
        quality,
        color_primaries,
        transfer_characteristics,
        matrix_coefficients,
        full_range,
        has_icc_profile: has_icc,
        recommendations,
    })
}

impl AvifProbe {
    /// Estimated source quality (0-100), or `None` if not extractable.
    pub fn estimated_quality(&self) -> Option<f32> {
        self.quality.as_ref().map(|q| q.estimated_quality)
    }

    /// Recommended zenavif quality for re-encoding that matches the source.
    ///
    /// Returns `None` if the quality couldn't be estimated.
    pub fn recommended_quality(&self) -> Option<f32> {
        self.quality.as_ref().map(|q| {
            // Slightly higher than detected to avoid generation loss
            (q.estimated_quality + 2.0).min(100.0)
        })
    }
}

// ── Internal helpers ────────────────────────────────────────────────

/// Map AV1 quantizer (0-255) to quality (0-100).
///
/// AV1 QP 0 = lossless, QP 255 = worst quality.
/// The mapping is roughly linear with a compression curve.
fn qp_to_quality(qp: u8) -> f32 {
    if qp == 0 {
        return 100.0;
    }
    // Approximate inverse of typical quality-to-QP mapping:
    //   quality 100 → QP 0
    //   quality 80  → QP ~30
    //   quality 60  → QP ~80
    //   quality 40  → QP ~140
    //   quality 20  → QP ~200
    //   quality 1   → QP ~255
    //
    // Use a piecewise linear approximation
    let q = qp as f32;
    let quality = 100.0 - (q * 100.0 / 255.0);
    // Apply a mild curve to better match perceptual quality
    let quality = quality * (1.0 + 0.3 * (1.0 - quality / 100.0));
    quality.clamp(1.0, 100.0)
}

impl zencodec::SourceEncodingDetails for AvifProbe {
    fn source_generic_quality(&self) -> Option<f32> {
        self.quality.as_ref().map(|q| q.estimated_quality)
    }

    fn is_lossless(&self) -> bool {
        self.lossless.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qp_to_quality_boundaries() {
        assert_eq!(qp_to_quality(0), 100.0);
        let worst = qp_to_quality(255);
        assert!((1.0..=5.0).contains(&worst), "QP 255 → {worst}");
    }

    #[test]
    fn test_qp_to_quality_monotonic() {
        let mut prev = 100.0f32;
        for qp in 1..=255u8 {
            let q = qp_to_quality(qp);
            assert!(q <= prev, "QP {qp}: {q} > previous {prev}");
            prev = q;
        }
    }

    #[test]
    fn test_probe_too_short() {
        assert_eq!(probe(&[]).unwrap_err(), ProbeError::TooShort);
        assert_eq!(probe(&[0; 11]).unwrap_err(), ProbeError::TooShort);
    }

    #[test]
    fn test_probe_not_avif() {
        // Non-ftyp header → NotAvif before we even reach zenavif-parse
        let mut data = vec![0u8; 32];
        data[0..4].copy_from_slice(&12u32.to_be_bytes());
        data[4..8].copy_from_slice(b"moov");
        data[8..12].copy_from_slice(b"isom");
        assert_eq!(probe(&data).unwrap_err(), ProbeError::NotAvif);

        // Valid ftyp but non-AVIF brand → zenavif-parse rejects it
        let mut data = vec![0u8; 32];
        data[0..4].copy_from_slice(&12u32.to_be_bytes());
        data[4..8].copy_from_slice(b"ftyp");
        data[8..12].copy_from_slice(b"isom");
        let err = probe(&data).unwrap_err();
        assert!(err == ProbeError::NotAvif || err == ProbeError::Truncated);
    }

    /// Probe all test vectors and check that lossless/QP detection works.
    #[test]
    #[ignore] // requires test vectors: cargo test -- --ignored
    fn test_probe_all_vectors() {
        let dir = "tests/vectors/libavif";
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("No test vectors at {dir}");
            return;
        };

        let mut probed = 0;
        let mut with_qp = 0;
        let mut with_lossless = 0;

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("avif") {
                continue;
            }

            let data = std::fs::read(&path).unwrap();
            let result = probe(&data);

            let name = path.file_name().unwrap().to_str().unwrap();
            match result {
                Ok(info) => {
                    assert!(info.width > 0 && info.height > 0, "{name}: zero dimensions");
                    assert!(
                        [8, 10, 12].contains(&info.bit_depth),
                        "{name}: bad bit_depth {}",
                        info.bit_depth
                    );

                    if let Some(ref q) = info.quality {
                        assert!(
                            (1.0..=100.0).contains(&q.estimated_quality),
                            "{name}: quality {:.1} out of range",
                            q.estimated_quality
                        );
                        with_qp += 1;
                    }

                    if info.lossless.is_some() {
                        with_lossless += 1;
                    }

                    eprintln!(
                        "  {name}: {}x{} {}bpc {:?} qp={:?} lossless={:?}",
                        info.width,
                        info.height,
                        info.bit_depth,
                        info.chroma_sampling,
                        info.quality.as_ref().map(|q| q.quantizer),
                        info.lossless,
                    );

                    // Source encoding details trait
                    use zencodec::SourceEncodingDetails;
                    if info.lossless == Some(true) {
                        assert!(info.is_lossless(), "{name}: lossless but trait says false");
                    }

                    probed += 1;
                }
                Err(e) => {
                    eprintln!("  {name}: probe failed: {e}");
                }
            }
        }

        eprintln!(
            "\n  Probed {probed} files, {with_qp} with QP, {with_lossless} with lossless detection"
        );
        assert!(probed > 30, "Expected to probe >30 files, got {probed}");
        assert!(with_qp > 20, "Expected >20 files with QP, got {with_qp}");
        assert!(
            with_lossless > 20,
            "Expected >20 files with lossless detection, got {with_lossless}"
        );
    }
}

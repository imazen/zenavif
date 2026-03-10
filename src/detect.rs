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
/// Parses the ISOBMFF container and AV1 codec configuration to extract
/// image properties and estimate quality. No pixel decoding is performed.
pub fn probe(data: &[u8]) -> Result<AvifProbe, ProbeError> {
    if data.len() < 12 {
        return Err(ProbeError::TooShort);
    }

    // Check for ISOBMFF container (ftyp box)
    let ftyp_size = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
    if &data[4..8] != b"ftyp" {
        return Err(ProbeError::NotAvif);
    }

    // Check major brand
    if data.len() < 12 {
        return Err(ProbeError::Truncated);
    }
    let major_brand = &data[8..12];
    let is_avif = major_brand == b"avif" || major_brand == b"avis" || major_brand == b"mif1";
    if !is_avif {
        // Check compatible brands
        let mut found = false;
        let mut brand_pos = 16; // skip ftyp header + major_brand + minor_version
        while brand_pos + 4 <= ftyp_size.min(data.len()) {
            if &data[brand_pos..brand_pos + 4] == b"avif"
                || &data[brand_pos..brand_pos + 4] == b"avis"
            {
                found = true;
                break;
            }
            brand_pos += 4;
        }
        if !found {
            return Err(ProbeError::NotAvif);
        }
    }

    // Search for av1C box (AV1 Codec Configuration)
    let mut width = 0u32;
    let mut height = 0u32;
    let mut bit_depth = 8u8;
    let mut profile = 0u8;
    let mut monochrome = false;
    let mut subsampling_x = false;
    let mut subsampling_y = false;
    let mut has_alpha = false;
    let mut has_animation = false;
    let mut color_primaries = None;
    let mut transfer_characteristics = None;
    let mut matrix_coefficients = None;
    let mut full_range = None;
    let mut has_icc = false;
    let mut found_av1c = false;
    let mut quantizer: Option<u8> = None;

    // Scan ISOBMFF boxes
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let box_size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let box_type = &data[pos + 4..pos + 8];
        let box_end = if box_size == 0 {
            data.len() // extends to end of file
        } else if box_size == 1 {
            // 64-bit extended size
            if pos + 16 > data.len() {
                break;
            }
            u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap()) as usize
        } else {
            pos + box_size
        };
        let box_end = box_end.min(data.len());

        match box_type {
            b"moov" | b"meta" | b"iprp" | b"ipco" | b"trak" | b"mdia" | b"minf" | b"stbl" => {
                // Container boxes — recurse into contents
                let header_size = if box_type == b"meta" { 12 } else { 8 };
                pos += header_size;
                continue;
            }
            b"av1C" => {
                // AV1 Codec Configuration Record
                let cfg_start = pos + 8;
                if cfg_start + 4 <= box_end {
                    let b0 = data[cfg_start];
                    let b1 = data[cfg_start + 1];
                    let b2 = data[cfg_start + 2];
                    let b3 = data[cfg_start + 3];

                    // marker(1) version(7) | seq_profile(3) seq_level(5)
                    // seq_tier(1) high_bitdepth(1) twelve_bit(1) monochrome(1) ss_x(1) ss_y(1) chroma_pos(2)
                    profile = (b1 >> 5) & 0x07;
                    let high_bd = (b2 >> 6) & 1;
                    let twelve = (b2 >> 5) & 1;
                    monochrome = (b2 >> 4) & 1 != 0;
                    subsampling_x = (b2 >> 3) & 1 != 0;
                    subsampling_y = (b2 >> 2) & 1 != 0;

                    bit_depth = if twelve != 0 {
                        12
                    } else if high_bd != 0 {
                        10
                    } else {
                        8
                    };

                    let _ = b0; // marker + version
                    let _ = b3; // reserved + initial_presentation_delay

                    found_av1c = true;

                    // Try to extract QP from OBU data following the config
                    if cfg_start + 4 < box_end {
                        quantizer = extract_qp_from_obus(&data[cfg_start + 4..box_end]);
                    }
                }
            }
            b"colr" => {
                // Color information
                let colr_start = pos + 8;
                if colr_start + 4 <= box_end {
                    let colr_type = &data[colr_start..colr_start + 4];
                    if colr_type == b"nclx" && colr_start + 11 <= box_end {
                        color_primaries = Some(data[colr_start + 4]);
                        transfer_characteristics = Some(data[colr_start + 6]);
                        matrix_coefficients = Some(data[colr_start + 8]);
                        full_range = Some(data[colr_start + 10] & 0x80 != 0);
                    } else if colr_type == b"prof" || colr_type == b"rICC" {
                        has_icc = true;
                    }
                }
            }
            b"ispe" => {
                // Image spatial extents
                let ispe_start = pos + 8;
                if ispe_start + 12 <= box_end {
                    // version(4) + width(4) + height(4)
                    width = u32::from_be_bytes(
                        data[ispe_start + 4..ispe_start + 8].try_into().unwrap(),
                    );
                    height = u32::from_be_bytes(
                        data[ispe_start + 8..ispe_start + 12].try_into().unwrap(),
                    );
                }
            }
            b"auxC" => {
                // Auxiliary type property — alpha is "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha"
                let aux_start = pos + 8;
                if aux_start + 4 < box_end {
                    let aux_data = &data[aux_start + 4..box_end];
                    if aux_data.windows(5).any(|w| w == b"alpha") {
                        has_alpha = true;
                    }
                }
            }
            b"avis" | b"mvhd" | b"tkhd" => {
                // Animation markers
                if box_type == b"mvhd" || box_type == b"tkhd" {
                    has_animation = true;
                }
            }
            _ => {}
        }

        if box_size < 8 {
            break; // prevent infinite loop
        }
        pos = box_end;
    }

    if !found_av1c {
        // Try to find av1C in raw OBU data (some files embed it differently)
        // For now, fail gracefully
        return Err(ProbeError::NoAv1Config);
    }

    // Also try to extract QP from the mdat box if we haven't found it yet
    if quantizer.is_none() {
        quantizer = find_qp_in_mdat(data);
    }

    let chroma_sampling = if monochrome {
        ChromaSampling::Monochrome
    } else if subsampling_x && subsampling_y {
        ChromaSampling::Yuv420
    } else if subsampling_x {
        ChromaSampling::Yuv422
    } else {
        ChromaSampling::Yuv444
    };

    let quality = quantizer.map(|qp| {
        let estimated = qp_to_quality(qp);
        QualityEstimate {
            quantizer: qp,
            estimated_quality: estimated,
            confidence: Confidence::FromFrameHeader,
        }
    });

    // Build recommendations
    let mut recommendations = Vec::new();

    if let Some(ref q) = quality {
        if q.estimated_quality > 85.0 {
            recommendations.push(Recommendation::ReduceQuality);
        }
        if q.estimated_quality < 30.0 {
            recommendations.push(Recommendation::AvoidReencoding);
        }
    }

    if chroma_sampling == ChromaSampling::Yuv444 && !monochrome {
        recommendations.push(Recommendation::UseChromaSubsampling);
    }

    if bit_depth > 8 {
        // Only suggest reduction if not HDR
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

/// Try to extract the base QP from AV1 OBU data.
///
/// Looks for a frame header OBU and reads the base_q_idx field.
fn extract_qp_from_obus(data: &[u8]) -> Option<u8> {
    let mut pos = 0;
    while pos < data.len() {
        if pos >= data.len() {
            return None;
        }
        let header_byte = data[pos];
        let obu_type = (header_byte >> 3) & 0x0F;
        let has_extension = (header_byte >> 2) & 1 != 0;
        let has_size = (header_byte >> 1) & 1 != 0;
        pos += 1;

        if has_extension {
            if pos >= data.len() {
                return None;
            }
            pos += 1; // skip extension byte
        }

        let obu_size = if has_size {
            let (size, consumed) = read_leb128(&data[pos..])?;
            pos += consumed;
            size as usize
        } else {
            data.len() - pos
        };

        let obu_end = (pos + obu_size).min(data.len());

        // OBU type 1 = Sequence Header
        // OBU type 6 = Frame (contains frame header)
        // OBU type 3 = Frame Header
        if obu_type == 6 || obu_type == 3 {
            // Try to extract base_q_idx from frame header
            if let Some(qp) = parse_frame_header_qp(&data[pos..obu_end]) {
                return Some(qp);
            }
        }

        pos = obu_end;
    }
    None
}

/// Try to find QP from OBU data inside an mdat box.
fn find_qp_in_mdat(data: &[u8]) -> Option<u8> {
    // Search for mdat box
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let box_size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let box_type = &data[pos + 4..pos + 8];

        if box_type == b"mdat" {
            let mdat_start = pos + 8;
            let mdat_end = if box_size > 0 {
                (pos + box_size).min(data.len())
            } else {
                data.len()
            };
            if mdat_start < mdat_end {
                return extract_qp_from_obus(&data[mdat_start..mdat_end]);
            }
        }

        if box_size < 8 {
            break;
        }
        pos += box_size;
    }
    None
}

/// Very basic frame header QP extraction.
///
/// This is a best-effort parse — AV1 frame headers are complex and
/// context-dependent. We try to find base_q_idx which is the primary
/// quality knob.
fn parse_frame_header_qp(data: &[u8]) -> Option<u8> {
    // AV1 frame header parsing requires a bit reader and knowledge of
    // the sequence header to determine field sizes. For a lightweight
    // probe, we use a heuristic: base_q_idx is typically the first
    // 8-bit field after some variable-length fields.
    //
    // In practice, for still images encoded by common encoders, the
    // frame header structure is fairly predictable:
    //
    // For KEY frames with show_existing_frame=0:
    //   show_existing_frame(1) = 0
    //   frame_type(2) = 0 (KEY_FRAME)
    //   show_frame(1) = 1
    //   ... (more fields)
    //   ... base_q_idx(8)
    //
    // This is too fragile without a proper bit reader, so we only
    // attempt it when the data looks like a typical still image frame.

    if data.is_empty() {
        return None;
    }

    // Simple heuristic: for AVIF (still images), base_q_idx is often
    // at a predictable offset. But this is unreliable, so we return None
    // for now and rely on extract_qp_from_obus finding it in OBU data
    // attached to av1C.
    //
    // A proper implementation would need a full AV1 bit reader to handle
    // the variable-length fields that precede base_q_idx.
    None
}

/// Read LEB128 variable-length integer.
fn read_leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut consumed = 0;
    for (i, &byte) in data.iter().enumerate().take(8) {
        value |= ((byte & 0x7F) as u64) << (i * 7);
        consumed = i + 1;
        if byte & 0x80 == 0 {
            return Some((value, consumed));
        }
    }
    if consumed > 0 {
        Some((value, consumed))
    } else {
        None
    }
}

impl zc::SourceEncodingDetails for AvifProbe {
    fn source_generic_quality(&self) -> Option<f32> {
        self.quality.as_ref().map(|q| q.estimated_quality)
    }

    fn is_lossless(&self) -> bool {
        // AVIF supports lossless but we can't reliably detect it from
        // headers alone (QP=0 is near-lossless, not guaranteed lossless).
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qp_to_quality_boundaries() {
        assert_eq!(qp_to_quality(0), 100.0);
        let worst = qp_to_quality(255);
        assert!(worst >= 1.0 && worst <= 5.0, "QP 255 → {worst}");
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
        let mut data = vec![0u8; 32];
        data[0..4].copy_from_slice(&12u32.to_be_bytes());
        data[4..8].copy_from_slice(b"ftyp");
        data[8..12].copy_from_slice(b"isom");
        assert_eq!(probe(&data).unwrap_err(), ProbeError::NotAvif);
    }

    #[test]
    fn test_leb128() {
        assert_eq!(read_leb128(&[0x00]), Some((0, 1)));
        assert_eq!(read_leb128(&[0x7F]), Some((127, 1)));
        assert_eq!(read_leb128(&[0x80, 0x01]), Some((128, 2)));
        assert_eq!(read_leb128(&[0xE5, 0x8E, 0x26]), Some((624_485, 3)));
    }
}

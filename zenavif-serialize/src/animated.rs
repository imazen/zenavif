//! Animated AVIF container serialization.
//!
//! Takes pre-encoded AV1 frame data and produces a valid animated AVIF file
//! with `ftyp(avis) + meta + moov + mdat` structure.

use crate::boxes::{Av1CBox, ClliBox, ColrBox, MdcvBox, PaspBox};
#[path = "animated_metadata.rs"]
mod metadata;

/// Number of additional playbacks after the first presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepetitionCount {
    /// Repeat without a finite end time.
    Infinite,
    /// Repeat this many times; zero plays the sequence once.
    Finite(u32),
}


/// A single pre-encoded animation frame.
#[derive(Clone)]
#[non_exhaustive]
pub struct AnimFrame<'a> {
    /// AV1-encoded color data for this frame.
    pub color: &'a [u8],
    /// AV1-encoded alpha data for this frame (if present).
    pub alpha: Option<&'a [u8]>,
    /// Duration in timescale ticks.
    pub duration: u32,
    /// Whether this is a sync (key) frame.
    pub is_sync: bool,
}

impl<'a> AnimFrame<'a> {
    /// Create a frame with color data and duration. Alpha defaults to `None`, sync to `false`.
    pub fn new(color: &'a [u8], duration: u32) -> Self {
        Self { color, alpha: None, duration, is_sync: false }
    }

    /// Set alpha data for this frame.
    pub fn with_alpha(mut self, alpha: &'a [u8]) -> Self {
        self.alpha = Some(alpha);
        self
    }

    /// Mark this frame as a sync (key) frame.
    pub fn with_sync(mut self, is_sync: bool) -> Self {
        self.is_sync = is_sync;
        self
    }
}

/// Builder for animated AVIF container serialization.
///
/// Holds codec configuration and optional metadata. Call [`serialize`](AnimatedImage::serialize)
/// with per-encode data (dimensions, frames, sequence headers) to produce the AVIF file.
pub struct AnimatedImage {
    timescale: u32,
    repetition: RepetitionCount,
    icc: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
    premultiplied_alpha: bool,
    color_config: Av1CBox,
    alpha_config: Option<Av1CBox>,
    colr: Option<ColrBox>,
    colr_raw: Option<(u16, u16, u16, bool)>,
    clli: Option<ClliBox>,
    mdcv: Option<MdcvBox>,
    pixel_aspect_ratio: Option<PaspBox>,
    rotation: Option<u8>,
    mirror: Option<u8>,
}

impl Default for AnimatedImage {
    fn default() -> Self { Self::new() }
}

impl AnimatedImage {
    /// Create with sensible defaults (timescale=1000ms, infinite loop, 8-bit 4:2:0).
    pub fn new() -> Self {
        Self {
            timescale: 1000,
            repetition: RepetitionCount::Infinite,
            icc: None,
            exif: None,
            xmp: None,
            premultiplied_alpha: false,
            color_config: Av1CBox::default(),
            alpha_config: None,
            colr: None,
            colr_raw: None,
            clli: None,
            mdcv: None,
            pixel_aspect_ratio: None,
            rotation: None,
            mirror: None,
        }
    }

    /// Timescale in ticks per second. Default: 1000 (milliseconds).
    pub fn set_timescale(&mut self, timescale: u32) -> &mut Self { self.timescale = timescale; self }
    /// Total playbacks: 0 = infinite, 1 = play once. Default: infinite.
    pub fn set_loop_count(&mut self, loop_count: u32) -> &mut Self {
        self.repetition = if loop_count == 0 { RepetitionCount::Infinite } else { RepetitionCount::Finite(loop_count - 1) };
        self
    }
    /// Set additional playbacks explicitly (zero means play once).
    pub fn set_repetition_count(&mut self, count: RepetitionCount) -> &mut Self { self.repetition = count; self }
    /// Set the horizontal-to-vertical pixel spacing ratio on the color track
    /// and poster. Both spacings must be positive and equal (AVIF 1.2 section 9.1.2).
    pub fn set_pixel_aspect_ratio(&mut self, h_spacing: u32, v_spacing: u32) -> &mut Self {
        self.pixel_aspect_ratio = Some(PaspBox::new(h_spacing, v_spacing));
        self
    }
    /// Counter-clockwise quarter-turn code (0..=3), applied before mirroring.
    pub fn set_rotation(&mut self, angle: u8) -> &mut Self { self.rotation = Some(angle); self }
    /// Mirror after rotation: 0 exchanges top/bottom, 1 exchanges left/right.
    pub fn set_mirror(&mut self, axis: u8) -> &mut Self { self.mirror = Some(axis); self }
    /// Embed an ICC profile in both the color sample entry and poster.
    pub fn set_icc_profile(&mut self, icc: Vec<u8>) -> &mut Self { self.icc = Some(icc); self }
    /// Embed Exif in the color track and poster. Accepts TIFF bytes or an
    /// already-framed HEIF Exif item, matching the still-image serializer.
    pub fn set_exif(&mut self, exif: Vec<u8>) -> &mut Self { self.exif = Some(exif); self }
    /// Embed XMP in the color track and poster.
    pub fn set_xmp(&mut self, xmp: Vec<u8>) -> &mut Self { self.xmp = Some(xmp); self }
    /// Signal that color samples are already premultiplied by alpha. Does not
    /// alter sample pixels; the caller must supply the corresponding values.
    pub fn set_premultiplied_alpha(&mut self, value: bool) -> &mut Self { self.premultiplied_alpha = value; self }

    /// Validate dimensions, sample layout and durations before writing. The
    /// supplied OBUs must already match the codec configuration and sync flags.
    pub fn try_serialize(&self, width: u32, height: u32, frames: &[AnimFrame<'_>],
                         color_seq_header: &[u8], alpha_seq_header: Option<&[u8]>) -> crate::Result<Vec<u8>> {
        let invalid = |message| whereat::at!(crate::SerializeError::InvalidInput(message));
        if self.rotation.is_some_and(|r| r > 3) || self.mirror.is_some_and(|m| m > 1) {
            return Err(invalid("rotation must be 0..=3 and mirror axis must be 0..=1"));
        }
        if self.pixel_aspect_ratio.is_some_and(|p| p.h_spacing == 0 || p.h_spacing != p.v_spacing) {
            return Err(invalid("AVIF pixel aspect ratio must be positive and 1:1"));
        }
        if width == 0 || height == 0 || width > 65535 || height > 65535 {
            return Err(invalid("animation dimensions must fit the visual sample entry"));
        }
        if self.timescale == 0 || frames.is_empty() || frames.len() > u32::MAX as usize || color_seq_header.is_empty() || !frames[0].is_sync {
            return Err(invalid("animation needs a timescale, sequence header and first sync sample"));
        }
        let alpha = frames[0].alpha.is_some();
        if alpha != self.alpha_config.is_some() || alpha != alpha_seq_header.is_some() || (self.premultiplied_alpha && !alpha) {
            return Err(invalid("alpha samples, configuration and sequence header must agree"));
        }
        if alpha_seq_header.is_some_and(|s| s.is_empty()) { return Err(invalid("empty alpha sequence header")); }
        let mut duration = 0u64;
        // Conservative upper bound for the current 32-bit boxes and offsets.
        let mut size = 65536u64;
        for frame in frames {
            if frame.duration == 0 || frame.color.is_empty() || frame.alpha.is_some() != alpha || frame.alpha.is_some_and(|a| a.is_empty()) {
                return Err(invalid("animation samples must be nonempty, timed and have consistent alpha"));
            }
            duration = duration.checked_add(u64::from(frame.duration)).ok_or_else(|| invalid("animation duration overflow"))?;
            size = size.checked_add(frame.color.len() as u64).and_then(|n| n.checked_add(frame.alpha.map_or(0, |a| a.len()) as u64)).and_then(|n| n.checked_add(32)).ok_or_else(|| invalid("animation size overflow"))?;
        }
        if let RepetitionCount::Finite(count) = self.repetition {
            duration.checked_mul(u64::from(count) + 1).filter(|&n| n != u64::MAX).ok_or_else(|| invalid("repeated duration overflow"))?;
        }
        for bytes in [&self.icc, &self.exif, &self.xmp].into_iter().flatten() {
            if bytes.is_empty() { return Err(invalid("metadata payload must not be empty")); }
            size = size.checked_add((bytes.len() as u64).checked_mul(2).ok_or_else(|| invalid("metadata size overflow"))?).ok_or_else(|| invalid("metadata size overflow"))?;
        }
        size = (color_seq_header.len() as u64).checked_add(alpha_seq_header.map_or(0, |s| s.len()) as u64).and_then(|n| n.checked_mul(2)).and_then(|n| size.checked_add(n)).ok_or_else(|| invalid("configuration size overflow"))?;
        if size > u32::MAX as u64 { return Err(invalid("animation exceeds 32-bit container offsets")); }
        Ok(self.serialize(width, height, frames, color_seq_header, alpha_seq_header))
    }
    /// AV1 codec configuration for the color track.
    pub fn set_color_config(&mut self, config: Av1CBox) -> &mut Self { self.color_config = config; self }
    /// AV1 codec configuration for the alpha track.
    pub fn set_alpha_config(&mut self, config: Av1CBox) -> &mut Self { self.alpha_config = Some(config); self }
    /// CICP color info (nclx).
    pub fn set_colr(&mut self, colr: ColrBox) -> &mut Self { self.colr = Some(colr); self.colr_raw = None; self }
    /// Write exact CICP code points, including values absent from the typed
    /// convenience enums. The last color-description setter takes precedence.
    pub fn set_color_description(&mut self, primaries: u16, transfer: u16, matrix: u16, full_range: bool) -> &mut Self {
        self.colr_raw = Some((primaries, transfer, matrix, full_range)); self
    }
    /// Content Light Level Information (HDR).
    pub fn set_clli(&mut self, clli: ClliBox) -> &mut Self { self.clli = Some(clli); self }
    /// Mastering Display Colour Volume (HDR).
    pub fn set_mdcv(&mut self, mdcv: MdcvBox) -> &mut Self { self.mdcv = Some(mdcv); self }

    /// Serialize an animated AVIF file from pre-encoded AV1 frame data.
    pub fn serialize(&self, width: u32, height: u32, frames: &[AnimFrame<'_>],
                     color_seq_header: &[u8], alpha_seq_header: Option<&[u8]>) -> Vec<u8> {
    let has_alpha = frames.iter().any(|f| f.alpha.is_some())
        && self.alpha_config.is_some()
        && alpha_seq_header.is_some();

    let total_duration: u64 = frames.iter().map(|f| u64::from(f.duration)).sum();
    let presentation_duration = match self.repetition {
        RepetitionCount::Infinite => u64::MAX,
        RepetitionCount::Finite(count) => total_duration.checked_mul(u64::from(count) + 1).expect("repeated animation duration overflow"),
    };
    let sidecars = metadata::sidecars(self);
    let durations: Vec<u32> = frames.iter().map(|f| f.duration).collect();
    let color_frames: Vec<&[u8]> = frames.iter().map(|f| f.color).collect();
    let alpha_frames: Vec<&[u8]> = if has_alpha {
        frames.iter().map(|f| f.alpha.unwrap_or(&[])).collect()
    } else {
        Vec::new()
    };
    let sync_indices: Vec<u32> = frames.iter().enumerate()
        .filter(|(_, f)| f.is_sync)
        .map(|(i, _)| (i + 1) as u32) // 1-indexed
        .collect();

    let next_track_id = if has_alpha { 3 } else { 2 };

    let mut out = Vec::new();

    // ftyp
    write_ftyp(&mut out);

    // Poster items use the first color/alpha samples. Metadata is also
    // associated with the color track so sequence-oriented readers see it.
    let mut images = vec![metadata::ImageItem { id: 1, config: &self.color_config, sequence: color_seq_header, sample_len: color_frames.first().map_or(0, |f| f.len() as u32) }];
    if has_alpha {
        images.push(metadata::ImageItem { id: 2, config: self.alpha_config.as_ref().unwrap(), sequence: alpha_seq_header.unwrap(), sample_len: alpha_frames.first().map_or(0, |f| f.len() as u32) });
    }
    let iloc_offsets = metadata::write_meta(&mut out, width, height, &images, &sidecars, self);

    // moov — each write_track returns the byte position of its stco placeholder.
    let moov_pos = begin_box(&mut out, b"moov");
    write_mvhd(&mut out, self.timescale, presentation_duration, next_track_id);
    let color_stco_pos = write_track(
        &mut out, 1, width, height,
        self.timescale, total_duration,
        &color_frames, &durations, &sync_indices,
        color_seq_header, &self.color_config,
        false, self, presentation_duration, &sidecars,
    );
    let alpha_stco_pos = if has_alpha {
        let alpha_seq = alpha_seq_header.unwrap();
        let alpha_cfg = self.alpha_config.as_ref().unwrap();
        Some(write_track(
            &mut out, 2, width, height,
            self.timescale, total_duration,
            &alpha_frames, &durations, &sync_indices,
            alpha_seq, alpha_cfg,
            true, self, presentation_duration, &sidecars,
        ))
    } else {
        None
    };
    end_box(&mut out, moov_pos);

    // mdat
    let mdat_pos = begin_box(&mut out, b"mdat");
    let mdat_data_start = out.len();
    for frame in &color_frames {
        out.extend_from_slice(frame);
    }
    let alpha_data_start = out.len();
    for frame in &alpha_frames {
        out.extend_from_slice(frame);
    }
    end_box(&mut out, mdat_pos);

    // Patch placeholder offsets at exact recorded positions. We never scan the buffer
    // for sentinel byte patterns: AV1 frame payloads can legitimately contain those
    // bytes (and an attacker could deliberately seed them), so a scan-and-replace
    // approach would silently corrupt user data.
    write_u32_at(&mut out, iloc_offsets[0], mdat_data_start as u32);
    if has_alpha { write_u32_at(&mut out, iloc_offsets[1], alpha_data_start as u32); }
    write_u32_at(&mut out, color_stco_pos, mdat_data_start as u32);
    if let Some(pos) = alpha_stco_pos {
        write_u32_at(&mut out, pos, alpha_data_start as u32);
    }

    out
    }
}

// ─── Low-level helpers ───────────────────────────────────────────────

fn write_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn write_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn write_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Start a box, return position for later size patching.
fn begin_box(out: &mut Vec<u8>, box_type: &[u8; 4]) -> usize {
    let pos = out.len();
    write_u32(out, 0); // placeholder
    out.extend_from_slice(box_type);
    pos
}

/// Patch box size at the given position.
fn end_box(out: &mut [u8], pos: usize) {
    let size = (out.len() - pos) as u32;
    out[pos..pos + 4].copy_from_slice(&size.to_be_bytes());
}

fn write_fullbox(out: &mut Vec<u8>, version: u8, flags: u32) {
    out.push(version);
    out.push((flags >> 16) as u8);
    out.push((flags >> 8) as u8);
    out.push(flags as u8);
}

const STCO_PLACEHOLDER: u32 = 0xDEAD_BEEF;
#[cfg(test)]
const ILOC_PLACEHOLDER: u32 = 0xDEAD_BEE0;

// ─── Top-level boxes ─────────────────────────────────────────────────

fn write_ftyp(out: &mut Vec<u8>) {
    let pos = begin_box(out, b"ftyp");
    out.extend_from_slice(b"avis"); // major brand
    write_u32(out, 0); // minor version
    out.extend_from_slice(b"avis"); // compatible brands
    out.extend_from_slice(b"avif");
    out.extend_from_slice(b"mif1");
    out.extend_from_slice(b"miaf");
    out.extend_from_slice(b"iso8");
    end_box(out, pos);
}

fn write_mvhd(out: &mut Vec<u8>, timescale: u32, duration: u64, next_track_id: u32) {
    let pos = begin_box(out, b"mvhd");
    write_fullbox(out, 1, 0);
    write_u64(out, 0); // creation_time
    write_u64(out, 0); // modification_time
    write_u32(out, timescale);
    write_u64(out, duration);
    write_u32(out, 0x0001_0000); // rate 1.0
    write_u16(out, 0x0100); // volume 1.0
    out.extend_from_slice(&[0u8; 10]); // reserved
    // Identity matrix (3×3 fixed point)
    for &v in &[0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        write_u32(out, v);
    }
    out.extend_from_slice(&[0u8; 24]); // pre_defined
    write_u32(out, next_track_id);
    end_box(out, pos);
}

#[allow(clippy::too_many_arguments)]
fn write_track(
    out: &mut Vec<u8>,
    track_id: u32,
    width: u32,
    height: u32,
    timescale: u32,
    duration: u64,
    frames: &[&[u8]],
    durations: &[u32],
    sync_indices: &[u32],
    seq_header: &[u8],
    av1c: &Av1CBox,
    is_alpha: bool,
    options: &AnimatedImage,
    presentation_duration: u64,
    sidecars: &[metadata::Sidecar],
) -> usize {
    // Records the byte position of the stco chunk_offset placeholder.
    let stco_offset_pos: usize;
    let trak_pos = begin_box(out, b"trak");

    // tkhd
    {
        let pos = begin_box(out, b"tkhd");
        let flags = if is_alpha { 1 } else { 3 }; // enabled | in_movie
        write_fullbox(out, 1, flags);
        write_u64(out, 0); // creation_time
        write_u64(out, 0); // modification_time
        write_u32(out, track_id);
        write_u32(out, 0); // reserved
        write_u64(out, presentation_duration);
        out.extend_from_slice(&[0u8; 8]); // reserved
        write_u16(out, 0); // layer
        write_u16(out, 0); // alternate_group
        write_u16(out, 0); // volume
        write_u16(out, 0); // reserved
        for &v in &[0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
            write_u32(out, v);
        }
        // tkhd width/height are 16.16 fixed-point u32s. Widening to u64 prevents
        // a debug-build panic / release-build wrap when width or height >= 65536;
        // saturate to u32::MAX so very large dimensions still produce a well-formed
        // box (the integer part is clamped to 0xFFFF, which is the spec maximum).
        write_u32(out, fixed_16_16_saturating(width));
        write_u32(out, fixed_16_16_saturating(height));
        end_box(out, pos);
    }

    // Repeat the single media edit until the track presentation duration.
    let edts = begin_box(out, b"edts");
    let elst = begin_box(out, b"elst");
    write_fullbox(out, 1, u32::from(options.repetition != RepetitionCount::Finite(0)));
    write_u32(out, 1);
    write_u64(out, duration);
    write_u64(out, 0);
    write_u16(out, 1); write_u16(out, 0);
    end_box(out, elst); end_box(out, edts);
    if !is_alpha && !sidecars.is_empty() {
        metadata::write_meta(out, width, height, &[], sidecars, options);
    }

    // mdia
    {
        let mdia_pos = begin_box(out, b"mdia");

        // mdhd
        {
            let pos = begin_box(out, b"mdhd");
            write_fullbox(out, 1, 0);
            write_u64(out, 0); // creation_time
            write_u64(out, 0); // modification_time
            write_u32(out, timescale);
            write_u64(out, duration);
            write_u16(out, 0x55C4); // language = "und"
            write_u16(out, 0);
            end_box(out, pos);
        }

        // hdlr
        {
            let pos = begin_box(out, b"hdlr");
            write_fullbox(out, 0, 0);
            write_u32(out, 0);
            if is_alpha {
                out.extend_from_slice(b"auxv");
            } else {
                out.extend_from_slice(b"pict");
            }
            out.extend_from_slice(&[0u8; 12]);
            out.extend_from_slice(if is_alpha { b"Alpha\0" } else { b"Color\0" });
            end_box(out, pos);
        }

        // minf
        {
            let minf_pos = begin_box(out, b"minf");

            // vmhd
            {
                let pos = begin_box(out, b"vmhd");
                write_fullbox(out, 0, 1);
                out.extend_from_slice(&[0u8; 8]); // graphicsmode + opcolor
                end_box(out, pos);
            }

            // dinf + dref
            {
                let dinf_pos = begin_box(out, b"dinf");
                let dref_pos = begin_box(out, b"dref");
                write_fullbox(out, 0, 0);
                write_u32(out, 1);
                let url_pos = begin_box(out, b"url ");
                write_fullbox(out, 0, 1); // self-contained
                end_box(out, url_pos);
                end_box(out, dref_pos);
                end_box(out, dinf_pos);
            }

            // stbl
            {
                let stbl_pos = begin_box(out, b"stbl");

                // stsd with av01 + av1C
                {
                    let pos = begin_box(out, b"stsd");
                    write_fullbox(out, 0, 0);
                    write_u32(out, 1); // entry_count

                    let av01_pos = begin_box(out, b"av01");
                    out.extend_from_slice(&[0u8; 6]); // reserved
                    write_u16(out, 1); // data_reference_index
                    write_u16(out, 0); // pre_defined
                    write_u16(out, 0); // reserved
                    out.extend_from_slice(&[0u8; 12]); // pre_defined
                    // VisualSampleEntry width/height are u16. Saturate rather than
                    // silently wrap: `70000 as u16 = 4464` would emit a corrupted box.
                    write_u16(out, width.min(0xFFFF) as u16);
                    write_u16(out, height.min(0xFFFF) as u16);
                    write_u32(out, 0x0048_0000); // horiz resolution 72dpi
                    write_u32(out, 0x0048_0000); // vert resolution 72dpi
                    write_u32(out, 0); // reserved
                    write_u16(out, 1); // frame_count
                    out.extend_from_slice(&[0u8; 32]); // compressorname
                    write_u16(out, 0x0018); // depth = 24
                    out.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined = -1

                    // av1C sub-box with seq header
                    write_av1c_box(out, av1c, seq_header);
                    if is_alpha {
                        let auxi = begin_box(out, b"auxi");
                        write_fullbox(out, 0, 0);
                        out.extend_from_slice(metadata::ALPHA_TYPE);
                        end_box(out, auxi);
                    } else {
                        metadata::write_color_properties(out, options);
                        metadata::write_transform_properties(out, options);
                    }


                    end_box(out, av01_pos);
                    end_box(out, pos);
                }

                // stts (time-to-sample): run-length encode durations
                {
                    let pos = begin_box(out, b"stts");
                    write_fullbox(out, 0, 0);
                    let mut entries: Vec<(u32, u32)> = Vec::new();
                    for &d in durations {
                        if let Some(last) = entries.last_mut()
                            && last.1 == d {
                                last.0 += 1;
                                continue;
                            }
                        entries.push((1, d));
                    }
                    write_u32(out, entries.len() as u32);
                    for (count, delta) in &entries {
                        write_u32(out, *count);
                        write_u32(out, *delta);
                    }
                    end_box(out, pos);
                }

                // stsc (sample-to-chunk: all samples in one chunk)
                {
                    let pos = begin_box(out, b"stsc");
                    write_fullbox(out, 0, 0);
                    write_u32(out, 1);
                    write_u32(out, 1); // first_chunk
                    write_u32(out, frames.len() as u32); // samples_per_chunk
                    write_u32(out, 1); // sample_description_index
                    end_box(out, pos);
                }

                // stsz (sample sizes)
                {
                    let pos = begin_box(out, b"stsz");
                    write_fullbox(out, 0, 0);
                    write_u32(out, 0); // sample_size = 0 (variable)
                    write_u32(out, frames.len() as u32);
                    for frame in frames {
                        write_u32(out, frame.len() as u32);
                    }
                    end_box(out, pos);
                }

                // stco (chunk offset — placeholder, patched later via stco_offset_pos)
                stco_offset_pos = {
                    let pos = begin_box(out, b"stco");
                    write_fullbox(out, 0, 0);
                    write_u32(out, 1); // entry_count
                    let p = out.len();
                    write_u32(out, STCO_PLACEHOLDER);
                    end_box(out, pos);
                    p
                };

                // stss (sync samples)
                {
                    let pos = begin_box(out, b"stss");
                    write_fullbox(out, 0, 0);
                    write_u32(out, sync_indices.len() as u32);
                    for &idx in sync_indices {
                        write_u32(out, idx);
                    }
                    end_box(out, pos);
                }

                end_box(out, stbl_pos);
            }

            end_box(out, minf_pos);
        }

        end_box(out, mdia_pos);
    }

    // tref for alpha track
    if is_alpha {
        let tref_pos = begin_box(out, b"tref");
        let auxl_pos = begin_box(out, b"auxl");
        write_u32(out, 1); // references track 1 (color)
        end_box(out, auxl_pos);
        end_box(out, tref_pos);
    }

    if !is_alpha && options.premultiplied_alpha {
        let tref = begin_box(out, b"tref");
        let prem = begin_box(out, b"prem");
        write_u32(out, 2);
        end_box(out, prem); end_box(out, tref);
    }
    end_box(out, trak_pos);
    stco_offset_pos
}

// ─── Shared utilities ────────────────────────────────────────────────

fn write_av1c_box(out: &mut Vec<u8>, av1c: &Av1CBox, seq_header: &[u8]) {
    let pos = begin_box(out, b"av1C");
    out.push(0x81); // marker=1, version=1

    let byte1 = (av1c.seq_profile << 5) | av1c.seq_level_idx_0;
    let byte2 =
        u8::from(av1c.seq_tier_0) << 7
        | u8::from(av1c.high_bitdepth) << 6
        | u8::from(av1c.twelve_bit) << 5
        | u8::from(av1c.monochrome) << 4
        | u8::from(av1c.chroma_subsampling_x) << 3
        | u8::from(av1c.chroma_subsampling_y) << 2
        | av1c.chroma_sample_position;

    out.push(byte1);
    out.push(byte2);
    out.push(0x00); // no initial_presentation_delay
    out.extend_from_slice(seq_header);
    end_box(out, pos);
}

fn bit_depth_from_av1c(av1c: &Av1CBox) -> u8 {
    if av1c.twelve_bit { 12 } else if av1c.high_bitdepth { 10 } else { 8 }
}

fn write_colr_nclx(out: &mut Vec<u8>, colr: &ColrBox) {
    let pos = begin_box(out, b"colr");
    out.extend_from_slice(b"nclx");
    write_u16(out, colr.color_primaries as u16);
    write_u16(out, colr.transfer_characteristics as u16);
    write_u16(out, colr.matrix_coefficients as u16);
    out.push(if colr.full_range_flag { 1 << 7 } else { 0 });
    end_box(out, pos);
}

fn write_clli(out: &mut Vec<u8>, clli: &ClliBox) {
    let pos = begin_box(out, b"clli");
    write_u16(out, clli.max_content_light_level);
    write_u16(out, clli.max_pic_average_light_level);
    end_box(out, pos);
}

fn write_mdcv(out: &mut Vec<u8>, mdcv: &MdcvBox) {
    let pos = begin_box(out, b"mdcv");
    for &(x, y) in &mdcv.primaries {
        write_u16(out, x);
        write_u16(out, y);
    }
    write_u16(out, mdcv.white_point.0);
    write_u16(out, mdcv.white_point.1);
    write_u32(out, mdcv.max_luminance);
    write_u32(out, mdcv.min_luminance);
    end_box(out, pos);
}

/// Write a big-endian u32 at an exact byte position.
///
/// Used to patch iloc/stco offset placeholders at the positions recorded when
/// they were emitted. This avoids any buffer-wide scanning for sentinel byte
/// patterns, which would risk corrupting AV1 frame payloads that happen to
/// contain those bytes (a real possibility, and one an attacker could trigger
/// deliberately by seeding the sentinel into encoded data).
fn write_u32_at(out: &mut [u8], pos: usize, value: u32) {
    debug_assert!(pos + 4 <= out.len());
    if pos + 4 <= out.len() {
        out[pos..pos + 4].copy_from_slice(&value.to_be_bytes());
    }
}

/// Convert an unsigned integer dimension to ISO/IEC 14496-12 16.16 fixed-point,
/// saturating at u32::MAX (i.e. integer part clamped to 0xFFFF).
///
/// `value << 16` panics in debug builds and silently wraps in release builds for
/// any `value >= 0x10000`. The tkhd width/height fields are u32 16.16 — values
/// representing > 65535 px integer width can't be encoded exactly, so saturate
/// rather than crash or emit a corrupted box.
fn fixed_16_16_saturating(value: u32) -> u32 {
    let widened = (value as u64) << 16;
    if widened > u32::MAX as u64 { u32::MAX } else { widened as u32 }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_orientation_roundtrip_and_validation() {
        let frames = [AnimFrame::new(b"frame", 1).with_sync(true)];
        for rotation in [None, Some(0), Some(1), Some(2), Some(3)] {
            for mirror in [None, Some(0), Some(1)] {
                let mut image = AnimatedImage::new();
                if let Some(r) = rotation { image.set_rotation(r); }
                if let Some(m) = mirror { image.set_mirror(m); }
                let bytes = image.try_serialize(64, 80, &frames, b"header", None).unwrap();
                let parsed = zenavif_parse_current::AvifParser::from_bytes(&bytes).unwrap();
                assert_eq!(parsed.rotation().map(|r| r.angle), rotation.map(|r| u16::from(r) * 90));
                assert_eq!(parsed.mirror().map(|m| m.axis), mirror);
            }
        }
        for (r, m) in [(4, 0), (255, 0), (0, 2), (0, 255)] {
            let mut image = AnimatedImage::new();
            image.set_rotation(r).set_mirror(m);
            assert!(image.try_serialize(64, 80, &frames, b"header", None).is_err());
        }
    }

    #[test]
    fn animation_pixel_aspect_ratio_roundtrip_and_validation() {
        let frames = [AnimFrame::new(b"frame", 1).with_sync(true)];
        for spacing in [1, 2, u32::MAX] {
            let mut image = AnimatedImage::new();
            image.set_pixel_aspect_ratio(spacing, spacing);
            let bytes = image.try_serialize(64, 64, &frames, b"header", None).unwrap();
            let parsed = zenavif_parse_current::AvifParser::from_bytes(&bytes).unwrap();
            let ratio = parsed.pixel_aspect_ratio().unwrap();
            assert_eq!((ratio.h_spacing, ratio.v_spacing), (spacing, spacing));
        }
        for (h, v) in [(0, 0), (0, 1), (1, 0), (2, 1), (1, 2)] {
            let mut image = AnimatedImage::new();
            image.set_pixel_aspect_ratio(h, v);
            assert!(image.try_serialize(64, 64, &frames, b"header", None).is_err());
        }
    }

    #[test]
    fn repetition_round_trips_with_variable_frame_durations() {
        let frames = [AnimFrame::new(b"a", 100).with_sync(true), AnimFrame::new(b"b", 500)];
        for (repeat, expected) in [(RepetitionCount::Finite(0), 1), (RepetitionCount::Finite(2), 3), (RepetitionCount::Infinite, 0)] {
            let mut image = AnimatedImage::new();
            image.set_repetition_count(repeat);
            let bytes = image.try_serialize(64, 64, &frames, b"header", None).unwrap();
            let parser = zenavif_parse_current::AvifParser::from_bytes(&bytes).unwrap();
            let info = parser.animation_info().unwrap();
            assert_eq!(info.loop_count, expected);
            assert_eq!(info.frame_count, 2);
            assert_eq!(info.timescale, 1000);
        }
    }

    #[test]
    fn checked_animation_rejects_inconsistent_alpha_and_repeat_overflow() {
        let mut image = AnimatedImage::new();
        let frames = [AnimFrame::new(b"a", u32::MAX).with_sync(true), AnimFrame::new(b"b", u32::MAX)];
        image.set_repetition_count(RepetitionCount::Finite(u32::MAX));
        assert!(image.try_serialize(64, 64, &frames, b"header", None).is_err());
        image.set_repetition_count(RepetitionCount::Finite(0));
        image.set_premultiplied_alpha(true);
        assert!(image.try_serialize(64, 64, &frames, b"header", None).is_err());
        image.set_premultiplied_alpha(false).set_alpha_config(mono_av1c());
        assert!(image.try_serialize(64, 64, &frames, b"header", Some(b"alpha")).is_err());
    }

    fn basic_av1c() -> Av1CBox {
        Av1CBox {
            seq_profile: 0,
            seq_level_idx_0: 4,
            seq_tier_0: false,
            high_bitdepth: false,
            twelve_bit: false,
            monochrome: false,
            chroma_subsampling_x: true,
            chroma_subsampling_y: true,
            chroma_sample_position: 0,
        }
    }

    fn mono_av1c() -> Av1CBox {
        Av1CBox {
            seq_profile: 0,
            seq_level_idx_0: 4,
            seq_tier_0: false,
            high_bitdepth: false,
            twelve_bit: false,
            monochrome: true,
            chroma_subsampling_x: true,
            chroma_subsampling_y: true,
            chroma_sample_position: 0,
        }
    }

    #[test]
    fn serialize_color_only() {
        let frames = [
            AnimFrame::new(b"frame1color", 100).with_sync(true),
            AnimFrame::new(b"frame2color", 200),
        ];
        let mut image = AnimatedImage::new();
        image.set_color_config(basic_av1c());
        let avif = image.serialize(64, 64, &frames, b"seqhdr", None);

        // Should start with ftyp avis
        assert_eq!(&avif[4..8], b"ftyp");
        assert_eq!(&avif[8..12], b"avis");

        // Should contain mdat with frame data
        let mdat_str = b"mdat";
        assert!(avif.windows(4).any(|w| w == mdat_str));

        // Frame data should be present
        assert!(avif.windows(b"frame1color".len()).any(|w| w == b"frame1color"));
        assert!(avif.windows(b"frame2color".len()).any(|w| w == b"frame2color"));

        // Parse with zenavif-parse to verify structure
        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let info = parser.animation_info().expect("should have animation info");
        assert_eq!(info.timescale, 1000);
        assert_eq!(info.frame_count, 2);
    }

    #[test]
    fn serialize_with_alpha() {
        let frames = [
            AnimFrame::new(b"c1", 500).with_alpha(b"a1").with_sync(true),
            AnimFrame::new(b"c2", 500).with_alpha(b"a2"),
        ];
        let mut image = AnimatedImage::new();
        image.set_color_config(basic_av1c());
        image.set_alpha_config(mono_av1c());
        let avif = image.serialize(32, 32, &frames, b"colseq", Some(b"alphaseq"));

        assert_eq!(&avif[4..8], b"ftyp");
        assert!(avif.windows(2).any(|w| w == b"c1"));
        assert!(avif.windows(2).any(|w| w == b"a1"));
        assert!(avif.windows(2).any(|w| w == b"c2"));
        assert!(avif.windows(2).any(|w| w == b"a2"));

        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let info = parser.animation_info().expect("should have animation info");
        assert_eq!(info.frame_count, 2);
    }

    #[test]
    fn frame_payload_containing_placeholder_sentinels_is_not_corrupted() {
        // Regression: the old patcher walked the output buffer searching for
        // 0xDEADBEEF / 0xDEADBEE0 and overwrote any 4-byte match. Animation frame
        // payloads can legitimately contain those bytes (and an attacker could
        // deliberately seed them). This test puts both sentinels into multiple
        // frames at varied alignments and asserts the bytes survive serialization
        // intact.
        let stco = STCO_PLACEHOLDER.to_be_bytes();
        let iloc = ILOC_PLACEHOLDER.to_be_bytes();

        let mut frame1 = vec![0xAAu8; 33]; // odd-aligned sentinel
        frame1.extend_from_slice(&stco);
        frame1.extend_from_slice(&[0xBB; 16]);
        frame1.extend_from_slice(&iloc);
        frame1.extend_from_slice(&[0xCC; 32]);

        let mut frame2 = vec![0u8; 4];     // sentinels at start (after offset 0)
        frame2.extend_from_slice(&stco);
        frame2.extend_from_slice(&iloc);
        frame2.extend_from_slice(&[0xEE; 100]);

        let mut alpha1 = vec![0x44u8; 8];
        alpha1.extend_from_slice(&stco);
        alpha1.extend_from_slice(&[0x55; 16]);
        let mut alpha2 = vec![0x66u8; 16];
        alpha2.extend_from_slice(&iloc);
        alpha2.extend_from_slice(&[0x77; 8]);

        let frames = [
            AnimFrame::new(frame1.as_slice(), 100).with_alpha(alpha1.as_slice()).with_sync(true),
            AnimFrame::new(frame2.as_slice(), 200).with_alpha(alpha2.as_slice()),
        ];
        let mut image = AnimatedImage::new();
        image.set_color_config(basic_av1c());
        image.set_alpha_config(mono_av1c());
        let avif = image.serialize(64, 64, &frames, b"colseq", Some(b"alphaseq"));

        // Each frame's bytes must appear verbatim somewhere in the file.
        // (We used to fail this when sentinels in payload were overwritten.)
        assert!(avif.windows(frame1.len()).any(|w| w == frame1.as_slice()),
            "frame1 (with both sentinels) corrupted by placeholder scan");
        assert!(avif.windows(frame2.len()).any(|w| w == frame2.as_slice()),
            "frame2 (with both sentinels) corrupted by placeholder scan");
        assert!(avif.windows(alpha1.len()).any(|w| w == alpha1.as_slice()),
            "alpha1 corrupted by placeholder scan");
        assert!(avif.windows(alpha2.len()).any(|w| w == alpha2.as_slice()),
            "alpha2 corrupted by placeholder scan");

        // Parser still resolves animation structure correctly.
        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let info = parser.animation_info().expect("animation info");
        assert_eq!(info.frame_count, 2);
    }

    #[test]
    fn very_large_width_does_not_panic() {
        // Regression: tkhd encoded width as `width << 16`, which panics in debug
        // builds for any width >= 65536. Saturation via fixed_16_16_saturating
        // produces a well-formed (clamped) box and never panics.
        assert_eq!(fixed_16_16_saturating(0), 0);
        assert_eq!(fixed_16_16_saturating(1), 0x0001_0000);
        assert_eq!(fixed_16_16_saturating(0xFFFF), 0xFFFF_0000);
        assert_eq!(fixed_16_16_saturating(0x1_0000), u32::MAX);
        assert_eq!(fixed_16_16_saturating(70_000), u32::MAX);
        assert_eq!(fixed_16_16_saturating(u32::MAX), u32::MAX);

        let frames = [AnimFrame::new(b"f", 100).with_sync(true)];
        let mut image = AnimatedImage::new();
        image.set_color_config(basic_av1c());
        // 70000 used to panic at write_u32(out, width << 16).
        let avif = image.serialize(70_000, 70_000, &frames, b"seq", None);
        // File is structurally valid.
        assert_eq!(&avif[4..8], b"ftyp");
        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let info = parser.animation_info().expect("animation info");
        assert_eq!(info.frame_count, 1);
    }

    #[test]
    fn frame_durations_roundtrip() {
        let frames = [
            AnimFrame::new(b"f1", 100).with_sync(true),
            AnimFrame::new(b"f2", 200),
            AnimFrame::new(b"f3", 300),
        ];
        let mut image = AnimatedImage::new();
        image.set_color_config(basic_av1c());
        let avif = image.serialize(16, 16, &frames, b"seq", None);
        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let info = parser.animation_info().expect("animation info");
        assert_eq!(info.frame_count, 3);
        assert_eq!(info.timescale, 1000);
    }
}

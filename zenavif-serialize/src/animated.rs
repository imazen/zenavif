//! Animated AVIF container serialization.
//!
//! Takes pre-encoded AV1 frame data and produces a valid animated AVIF file
//! with `ftyp(avis) + meta + moov + mdat` structure.

use crate::boxes::{Av1CBox, ClliBox, ColrBox, MdcvBox};

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
    loop_count: u32,
    color_config: Av1CBox,
    alpha_config: Option<Av1CBox>,
    colr: Option<ColrBox>,
    clli: Option<ClliBox>,
    mdcv: Option<MdcvBox>,
}

impl Default for AnimatedImage {
    fn default() -> Self { Self::new() }
}

impl AnimatedImage {
    /// Create with sensible defaults (timescale=1000ms, infinite loop, 8-bit 4:2:0).
    pub fn new() -> Self {
        Self {
            timescale: 1000,
            loop_count: 0,
            color_config: Av1CBox::default(),
            alpha_config: None,
            colr: None,
            clli: None,
            mdcv: None,
        }
    }

    /// Timescale in ticks per second. Default: 1000 (milliseconds).
    pub fn set_timescale(&mut self, timescale: u32) -> &mut Self { self.timescale = timescale; self }
    /// Loop count: 0 = infinite. Default: 0.
    pub fn set_loop_count(&mut self, loop_count: u32) -> &mut Self { self.loop_count = loop_count; self }
    /// AV1 codec configuration for the color track.
    pub fn set_color_config(&mut self, config: Av1CBox) -> &mut Self { self.color_config = config; self }
    /// AV1 codec configuration for the alpha track.
    pub fn set_alpha_config(&mut self, config: Av1CBox) -> &mut Self { self.alpha_config = Some(config); self }
    /// CICP color info (nclx).
    pub fn set_colr(&mut self, colr: ColrBox) -> &mut Self { self.colr = Some(colr); self }
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

    // meta — declares primary item for still-frame interop
    write_meta(
        &mut out,
        width,
        height,
        color_seq_header,
        color_frames.first().map(|f| f.len() as u32).unwrap_or(0),
        &self.color_config,
        self.colr.as_ref(),
        self.clli.as_ref(),
        self.mdcv.as_ref(),
    );

    // moov
    let moov_pos = begin_box(&mut out, b"moov");
    write_mvhd(&mut out, self.timescale, total_duration, next_track_id);
    write_track(
        &mut out, 1, width, height,
        self.timescale, total_duration,
        &color_frames, &durations, &sync_indices,
        color_seq_header, &self.color_config,
        false,
    );
    if has_alpha {
        let alpha_seq = alpha_seq_header.unwrap();
        let alpha_cfg = self.alpha_config.as_ref().unwrap();
        write_track(
            &mut out, 2, width, height,
            self.timescale, total_duration,
            &alpha_frames, &durations, &sync_indices,
            alpha_seq, alpha_cfg,
            true,
        );
    }
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

    // Patch placeholder offsets
    if has_alpha {
        patch_offset_placeholders(&mut out, &[mdat_data_start as u32, alpha_data_start as u32], mdat_data_start as u32);
    } else {
        patch_offset_placeholders(&mut out, &[mdat_data_start as u32], mdat_data_start as u32);
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

#[allow(clippy::too_many_arguments)]
fn write_meta(
    out: &mut Vec<u8>,
    width: u32,
    height: u32,
    seq_header: &[u8],
    first_frame_len: u32,
    av1c: &Av1CBox,
    colr: Option<&ColrBox>,
    clli: Option<&ClliBox>,
    mdcv: Option<&MdcvBox>,
) {
    let meta_pos = begin_box(out, b"meta");
    write_fullbox(out, 0, 0);

    // hdlr
    {
        let pos = begin_box(out, b"hdlr");
        write_fullbox(out, 0, 0);
        write_u32(out, 0); // pre_defined
        out.extend_from_slice(b"pict");
        out.extend_from_slice(&[0u8; 12]); // reserved
        out.push(0); // name (null-terminated empty)
        end_box(out, pos);
    }

    // pitm
    {
        let pos = begin_box(out, b"pitm");
        write_fullbox(out, 0, 0);
        write_u16(out, 1); // item_id
        end_box(out, pos);
    }

    // iloc
    {
        let pos = begin_box(out, b"iloc");
        write_fullbox(out, 0, 0);
        out.push(0x44); // offset_size=4, length_size=4
        out.push(0x00); // base_offset_size=0, reserved=0
        write_u16(out, 1); // item_count
        write_u16(out, 1); // item_id
        write_u16(out, 0); // data_reference_index
        write_u16(out, 1); // extent_count
        write_u32(out, ILOC_PLACEHOLDER); // extent_offset (patched later)
        write_u32(out, first_frame_len); // extent_length
        end_box(out, pos);
    }

    // iinf
    {
        let iinf_pos = begin_box(out, b"iinf");
        write_fullbox(out, 0, 0);
        write_u16(out, 1); // entry_count

        let infe_pos = begin_box(out, b"infe");
        write_fullbox(out, 2, 0);
        write_u16(out, 1); // item_id
        write_u16(out, 0); // protection_index
        out.extend_from_slice(b"av01");
        out.push(0); // name
        end_box(out, infe_pos);

        end_box(out, iinf_pos);
    }

    // iprp (ipco + ipma)
    {
        let iprp_pos = begin_box(out, b"iprp");

        // ipco
        {
            let ipco_pos = begin_box(out, b"ipco");

            // Property 1: ispe
            {
                let pos = begin_box(out, b"ispe");
                write_fullbox(out, 0, 0);
                write_u32(out, width);
                write_u32(out, height);
                end_box(out, pos);
            }

            // Property 2: av1C
            write_av1c_box(out, av1c, seq_header);

            // Property 3: pixi
            {
                let pos = begin_box(out, b"pixi");
                write_fullbox(out, 0, 0);
                if av1c.monochrome {
                    out.push(1); // 1 channel
                    out.push(bit_depth_from_av1c(av1c));
                } else {
                    out.push(3); // 3 channels
                    let depth = bit_depth_from_av1c(av1c);
                    out.push(depth);
                    out.push(depth);
                    out.push(depth);
                }
                end_box(out, pos);
            }

            // Property 4: colr (optional)
            if let Some(colr) = colr {
                if *colr != ColrBox::default() {
                    write_colr_nclx(out, colr);
                }
            }

            // Property 5: clli (optional)
            if let Some(clli) = clli {
                write_clli(out, clli);
            }

            // Property 6: mdcv (optional)
            if let Some(mdcv) = mdcv {
                write_mdcv(out, mdcv);
            }

            end_box(out, ipco_pos);
        }

        // ipma
        {
            let pos = begin_box(out, b"ipma");
            write_fullbox(out, 0, 0);
            write_u32(out, 1); // entry_count
            write_u16(out, 1); // item_id
            // Count associations: ispe + av1C(essential) + pixi + optional colr/clli/mdcv
            let mut assoc_count: u8 = 3;
            let has_colr = colr.is_some_and(|c| *c != ColrBox::default());
            if has_colr { assoc_count += 1; }
            if clli.is_some() { assoc_count += 1; }
            if mdcv.is_some() { assoc_count += 1; }
            out.push(assoc_count);
            out.push(0x01); // property 1 (ispe), not essential
            out.push(0x82); // property 2 (av1C), essential
            out.push(0x03); // property 3 (pixi), not essential
            let mut next_prop = 4u8;
            if has_colr {
                out.push(next_prop);
                next_prop += 1;
            }
            if clli.is_some() {
                out.push(next_prop);
                next_prop += 1;
            }
            if mdcv.is_some() {
                out.push(next_prop);
                let _ = next_prop;
            }
            end_box(out, pos);
        }

        end_box(out, iprp_pos);
    }

    end_box(out, meta_pos);
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
) {
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
        write_u64(out, duration);
        out.extend_from_slice(&[0u8; 8]); // reserved
        write_u16(out, 0); // layer
        write_u16(out, 0); // alternate_group
        write_u16(out, 0); // volume
        write_u16(out, 0); // reserved
        for &v in &[0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
            write_u32(out, v);
        }
        write_u32(out, width << 16);
        write_u32(out, height << 16);
        end_box(out, pos);
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
                    write_u16(out, width as u16);
                    write_u16(out, height as u16);
                    write_u32(out, 0x0048_0000); // horiz resolution 72dpi
                    write_u32(out, 0x0048_0000); // vert resolution 72dpi
                    write_u32(out, 0); // reserved
                    write_u16(out, 1); // frame_count
                    out.extend_from_slice(&[0u8; 32]); // compressorname
                    write_u16(out, 0x0018); // depth = 24
                    out.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined = -1

                    // av1C sub-box with seq header
                    write_av1c_box(out, av1c, seq_header);

                    end_box(out, av01_pos);
                    end_box(out, pos);
                }

                // stts (time-to-sample): run-length encode durations
                {
                    let pos = begin_box(out, b"stts");
                    write_fullbox(out, 0, 0);
                    let mut entries: Vec<(u32, u32)> = Vec::new();
                    for &d in durations {
                        if let Some(last) = entries.last_mut() {
                            if last.1 == d {
                                last.0 += 1;
                                continue;
                            }
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

                // stco (chunk offset — placeholder, patched later)
                {
                    let pos = begin_box(out, b"stco");
                    write_fullbox(out, 0, 0);
                    write_u32(out, 1); // entry_count
                    write_u32(out, STCO_PLACEHOLDER);
                    end_box(out, pos);
                }

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

    end_box(out, trak_pos);
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

/// Find and replace placeholder values with actual offsets.
fn patch_offset_placeholders(out: &mut [u8], stco_offsets: &[u32], iloc_offset: u32) {
    let stco_placeholder = STCO_PLACEHOLDER.to_be_bytes();
    let iloc_placeholder = ILOC_PLACEHOLDER.to_be_bytes();
    let mut stco_idx = 0;
    let mut i = 0;
    while i + 4 <= out.len() {
        if stco_idx < stco_offsets.len() && out[i..i + 4] == stco_placeholder {
            out[i..i + 4].copy_from_slice(&stco_offsets[stco_idx].to_be_bytes());
            stco_idx += 1;
            i += 4;
        } else if out[i..i + 4] == iloc_placeholder {
            out[i..i + 4].copy_from_slice(&iloc_offset.to_be_bytes());
            i += 4;
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

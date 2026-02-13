//! Animated AVIF container serialization.
//!
//! Takes pre-encoded AV1 frame data and produces a valid animated AVIF file
//! with `ftyp(avis) + meta + moov + mdat` structure.

use crate::boxes::{Av1CBox, ClliBox, ColrBox, MdcvBox};

/// A single pre-encoded animation frame.
#[derive(Clone)]
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

/// Configuration for an animated AVIF image.
pub struct AnimatedImage<'a> {
    pub width: u32,
    pub height: u32,
    /// Timescale (ticks per second). Use 1000 for milliseconds.
    pub timescale: u32,
    /// Loop count: 0 = infinite looping.
    pub loop_count: u32,
    pub frames: &'a [AnimFrame<'a>],
    /// AV1 codec configuration for color track.
    pub color_config: Av1CBox,
    /// AV1 codec configuration for alpha track (if frames have alpha).
    pub alpha_config: Option<Av1CBox>,
    /// AV1 sequence header OBU for color track.
    pub color_seq_header: &'a [u8],
    /// AV1 sequence header OBU for alpha track.
    pub alpha_seq_header: Option<&'a [u8]>,
    /// CICP color info (nclx). Optional — skipped if default.
    pub colr: Option<ColrBox>,
    /// Content Light Level Information (HDR).
    pub clli: Option<ClliBox>,
    /// Mastering Display Colour Volume (HDR).
    pub mdcv: Option<MdcvBox>,
}

/// Serialize an animated AVIF file from pre-encoded AV1 frame data.
///
/// Returns the complete AVIF file as a `Vec<u8>`.
pub fn serialize_animated(image: &AnimatedImage<'_>) -> Vec<u8> {
    let has_alpha = image.frames.iter().any(|f| f.alpha.is_some())
        && image.alpha_config.is_some()
        && image.alpha_seq_header.is_some();

    let total_duration: u64 = image.frames.iter().map(|f| u64::from(f.duration)).sum();
    let durations: Vec<u32> = image.frames.iter().map(|f| f.duration).collect();
    let color_frames: Vec<&[u8]> = image.frames.iter().map(|f| f.color).collect();
    let alpha_frames: Vec<&[u8]> = if has_alpha {
        image.frames.iter().map(|f| f.alpha.unwrap_or(&[])).collect()
    } else {
        Vec::new()
    };
    let sync_indices: Vec<u32> = image.frames.iter().enumerate()
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
        image.width,
        image.height,
        image.color_seq_header,
        color_frames.first().map(|f| f.len() as u32).unwrap_or(0),
        &image.color_config,
        image.colr.as_ref(),
        image.clli.as_ref(),
        image.mdcv.as_ref(),
    );

    // moov
    let moov_pos = begin_box(&mut out, b"moov");
    write_mvhd(&mut out, image.timescale, total_duration, next_track_id);
    write_track(
        &mut out, 1, image.width, image.height,
        image.timescale, total_duration,
        &color_frames, &durations, &sync_indices,
        image.color_seq_header, &image.color_config,
        false,
    );
    if has_alpha {
        let alpha_seq = image.alpha_seq_header.unwrap();
        let alpha_cfg = image.alpha_config.as_ref().unwrap();
        write_track(
            &mut out, 2, image.width, image.height,
            image.timescale, total_duration,
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
        let image = AnimatedImage {
            width: 64,
            height: 64,
            timescale: 1000,
            loop_count: 0,
            frames: &[
                AnimFrame { color: b"frame1color", alpha: None, duration: 100, is_sync: true },
                AnimFrame { color: b"frame2color", alpha: None, duration: 200, is_sync: false },
            ],
            color_config: basic_av1c(),
            alpha_config: None,
            color_seq_header: b"seqhdr",
            alpha_seq_header: None,
            colr: None,
            clli: None,
            mdcv: None,
        };

        let avif = serialize_animated(&image);

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
        let image = AnimatedImage {
            width: 32,
            height: 32,
            timescale: 1000,
            loop_count: 0,
            frames: &[
                AnimFrame { color: b"c1", alpha: Some(b"a1"), duration: 500, is_sync: true },
                AnimFrame { color: b"c2", alpha: Some(b"a2"), duration: 500, is_sync: false },
            ],
            color_config: basic_av1c(),
            alpha_config: Some(mono_av1c()),
            color_seq_header: b"colseq",
            alpha_seq_header: Some(b"alphaseq"),
            colr: None,
            clli: None,
            mdcv: None,
        };

        let avif = serialize_animated(&image);

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
        let image = AnimatedImage {
            width: 16,
            height: 16,
            timescale: 1000,
            loop_count: 0,
            frames: &[
                AnimFrame { color: b"f1", alpha: None, duration: 100, is_sync: true },
                AnimFrame { color: b"f2", alpha: None, duration: 200, is_sync: false },
                AnimFrame { color: b"f3", alpha: None, duration: 300, is_sync: false },
            ],
            color_config: basic_av1c(),
            alpha_config: None,
            color_seq_header: b"seq",
            alpha_seq_header: None,
            colr: None,
            clli: None,
            mdcv: None,
        };

        let avif = serialize_animated(&image);
        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let info = parser.animation_info().expect("animation info");
        assert_eq!(info.frame_count, 3);
        assert_eq!(info.timescale, 1000);
    }
}

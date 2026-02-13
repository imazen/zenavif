//! Grid (tiled) AVIF image serialization.
//!
//! Writes an AVIF file where the primary item is an ImageGrid descriptor
//! with `dimg` references to individual AV1 tile items.

use crate::boxes::*;
use arrayvec::ArrayVec;
use std::io;

/// Configuration for a grid AVIF image.
pub struct GridImage<'a> {
    /// Number of tile rows (1-256).
    pub rows: u8,
    /// Number of tile columns (1-256).
    pub columns: u8,
    /// Output image width in pixels.
    pub output_width: u32,
    /// Output image height in pixels.
    pub output_height: u32,
    /// Tile width in pixels (all tiles same width).
    pub tile_width: u32,
    /// Tile height in pixels (all tiles same height).
    pub tile_height: u32,
    /// AV1-encoded data for each tile, in row-major order.
    /// Length must equal `rows * columns`.
    pub tile_data: &'a [&'a [u8]],
    /// Optional alpha data for each tile (same order as `tile_data`).
    /// If `Some`, length must equal `rows * columns`.
    pub alpha_data: Option<&'a [&'a [u8]]>,
    /// AV1 codec configuration for color tiles.
    pub color_config: Av1CBox,
    /// AV1 codec configuration for alpha tiles (if alpha_data is present).
    pub alpha_config: Option<Av1CBox>,
    /// Bit depth (8, 10, or 12).
    pub depth_bits: u8,
    /// CICP color info. `None` omits the colr box.
    pub colr: Option<ColrBox>,
    /// Whether alpha is premultiplied.
    pub premultiplied_alpha: bool,
}

/// Serialize a grid AVIF image.
///
/// Returns the complete AVIF file as a `Vec<u8>`.
pub fn serialize_grid(image: &GridImage<'_>) -> io::Result<Vec<u8>> {
    let tile_count = image.rows as usize * image.columns as usize;
    if image.tile_data.len() != tile_count {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
            format!("tile_data.len() ({}) != rows*columns ({})", image.tile_data.len(), tile_count)));
    }
    if let Some(alpha) = image.alpha_data {
        if alpha.len() != tile_count {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                format!("alpha_data.len() ({}) != rows*columns ({})", alpha.len(), tile_count)));
        }
    }

    let has_alpha = image.alpha_data.is_some() && image.alpha_config.is_some();

    // Item IDs:
    // 1 = color grid item
    // 2 = alpha grid item (if has_alpha)
    // 3..3+N = color tile items
    // 3+N..3+2N = alpha tile items (if has_alpha)
    let color_grid_id: u16 = 1;
    let alpha_grid_id: u16 = 2;
    let color_tile_base: u16 = if has_alpha { 3 } else { 2 };
    let alpha_tile_base: u16 = color_tile_base + tile_count as u16;

    // Build the ImageGrid descriptor (item data for the grid item)
    let grid_descriptor = make_grid_descriptor(
        image.rows, image.columns,
        image.output_width, image.output_height,
    );

    let alpha_grid_descriptor = if has_alpha {
        Some(make_grid_descriptor(
            image.rows, image.columns,
            image.output_width, image.output_height,
        ))
    } else {
        None
    };

    // Build box structures
    let mut image_items: Vec<InfeBox> = Vec::new();
    let mut ipma_entries: Vec<IpmaEntry> = Vec::new();
    let mut irefs: Vec<IrefEntryBox> = Vec::new();
    let mut ipco = IpcoBox::new();
    const ESSENTIAL_BIT: u8 = 0x80;

    // Shared properties
    let ispe_output = ipco.push(IpcoProp::Ispe(IspeBox {
        width: image.output_width,
        height: image.output_height,
    })).ok_or(io::ErrorKind::InvalidInput)?;

    let ispe_tile = ipco.push(IpcoProp::Ispe(IspeBox {
        width: image.tile_width,
        height: image.tile_height,
    })).ok_or(io::ErrorKind::InvalidInput)?;

    let av1c_color = ipco.push(IpcoProp::Av1C(image.color_config)).ok_or(io::ErrorKind::InvalidInput)?;

    let pixi_color = ipco.push(IpcoProp::Pixi(PixiBox {
        channels: if image.color_config.monochrome { 1 } else { 3 },
        depth: image.depth_bits,
    })).ok_or(io::ErrorKind::InvalidInput)?;

    // Optional colr
    let colr_prop = if let Some(ref colr) = image.colr {
        if *colr != ColrBox::default() {
            Some(ipco.push(IpcoProp::Colr(*colr)).ok_or(io::ErrorKind::InvalidInput)?)
        } else {
            None
        }
    } else {
        None
    };

    // Alpha AV1C + pixi + auxC (if applicable)
    let (av1c_alpha, pixi_alpha, auxc_alpha) = if has_alpha {
        let ac = ipco.push(IpcoProp::Av1C(*image.alpha_config.as_ref().unwrap())).ok_or(io::ErrorKind::InvalidInput)?;
        let pa = ipco.push(IpcoProp::Pixi(PixiBox {
            channels: 1,
            depth: image.depth_bits,
        })).ok_or(io::ErrorKind::InvalidInput)?;
        let auxc = ipco.push(IpcoProp::AuxC(AuxCBox {
            urn: "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha",
        })).ok_or(io::ErrorKind::InvalidInput)?;
        (Some(ac), Some(pa), Some(auxc))
    } else {
        (None, None, None)
    };

    // --- Color grid item ---
    image_items.push(InfeBox {
        id: color_grid_id,
        typ: FourCC(*b"grid"),
        name: "",
        content_type: "",
    });

    // Grid item's ipma: ispe (output), pixi, and optionally colr
    let mut grid_ipma = IpmaEntry {
        item_id: color_grid_id,
        prop_ids: {
            let mut v = ArrayVec::new();
            v.push(ispe_output);
            v.push(pixi_color);
            if let Some(colr_p) = colr_prop {
                v.push(colr_p);
            }
            v
        },
    };
    let _ = &mut grid_ipma; // suppress clippy
    ipma_entries.push(grid_ipma);

    // --- Alpha grid item ---
    if has_alpha {
        image_items.push(InfeBox {
            id: alpha_grid_id,
            typ: FourCC(*b"grid"),
            name: "",
            content_type: "",
        });

        irefs.push(IrefEntryBox {
            from_id: alpha_grid_id,
            to_id: color_grid_id,
            typ: FourCC(*b"auxl"),
        });

        if image.premultiplied_alpha {
            irefs.push(IrefEntryBox {
                from_id: color_grid_id,
                to_id: alpha_grid_id,
                typ: FourCC(*b"prem"),
            });
        }

        ipma_entries.push(IpmaEntry {
            item_id: alpha_grid_id,
            prop_ids: {
                let mut v = ArrayVec::new();
                v.push(ispe_output);
                v.push(pixi_alpha.unwrap());
                v.push(auxc_alpha.unwrap());
                v
            },
        });
    }

    // --- Color tile items ---
    for i in 0..tile_count {
        let tile_id = color_tile_base + i as u16;

        image_items.push(InfeBox {
            id: tile_id,
            typ: FourCC(*b"av01"),
            name: "",
            content_type: "",
        });

        irefs.push(IrefEntryBox {
            from_id: color_grid_id,
            to_id: tile_id,
            typ: FourCC(*b"dimg"),
        });

        ipma_entries.push(IpmaEntry {
            item_id: tile_id,
            prop_ids: {
                let mut v = ArrayVec::new();
                v.push(ispe_tile);
                v.push(av1c_color | ESSENTIAL_BIT);
                v
            },
        });
    }

    // --- Alpha tile items ---
    if has_alpha {
        for i in 0..tile_count {
            let tile_id = alpha_tile_base + i as u16;

            image_items.push(InfeBox {
                id: tile_id,
                typ: FourCC(*b"av01"),
                name: "",
                content_type: "",
            });

            irefs.push(IrefEntryBox {
                from_id: alpha_grid_id,
                to_id: tile_id,
                typ: FourCC(*b"dimg"),
            });

            ipma_entries.push(IpmaEntry {
                item_id: tile_id,
                prop_ids: {
                    let mut v = ArrayVec::new();
                    v.push(ispe_tile);
                    v.push(av1c_alpha.unwrap() | ESSENTIAL_BIT);
                    v
                },
            });
        }
    }

    // Now we need to use the animated.rs approach (begin_box/end_box) because the
    // still-image AvifFile struct doesn't support variable item counts. We use the
    // same pattern: write boxes with size placeholders, then patch offsets.

    let mut out = Vec::new();

    // ftyp
    write_ftyp(&mut out);

    // meta
    write_meta_grid(
        &mut out,
        &image_items,
        &ipma_entries,
        &ipco,
        &irefs,
        color_grid_id,
        &grid_descriptor,
        alpha_grid_descriptor.as_deref(),
        alpha_grid_id,
        image.tile_data,
        image.alpha_data,
        color_tile_base,
        alpha_tile_base,
        tile_count,
        has_alpha,
    );

    // mdat
    let mdat_pos = begin_box(&mut out, b"mdat");
    let mdat_data_start = out.len() as u32;

    // First: grid descriptors, then tiles (color grid, alpha grid, color tiles, alpha tiles)
    // Actually, the iloc offset tracking is done in write_meta_grid with placeholders.
    // We need to write the actual data in iloc order.

    // Grid descriptor data
    out.extend_from_slice(&grid_descriptor);
    if let Some(ref agd) = alpha_grid_descriptor {
        out.extend_from_slice(agd);
    }

    // Color tile data
    for tile in image.tile_data {
        out.extend_from_slice(tile);
    }

    // Alpha tile data
    if let Some(alpha) = image.alpha_data {
        for tile in alpha {
            out.extend_from_slice(tile);
        }
    }

    end_box(&mut out, mdat_pos);

    // Patch iloc offsets
    patch_iloc_offsets(&mut out, mdat_data_start);

    Ok(out)
}

// ─── Helpers ─────────────────────────────────────────────────────────

const ILOC_PLACEHOLDER: u32 = 0xBAAD_F00D;

fn make_grid_descriptor(rows: u8, columns: u8, width: u32, height: u32) -> Vec<u8> {
    let mut desc = Vec::new();
    desc.push(0); // version
    if width > u16::MAX as u32 || height > u16::MAX as u32 {
        desc.push(1); // flags: 32-bit fields
    } else {
        desc.push(0); // flags: 16-bit fields
    }
    desc.push(rows.saturating_sub(1)); // rows_minus_one
    desc.push(columns.saturating_sub(1)); // columns_minus_one
    if width > u16::MAX as u32 || height > u16::MAX as u32 {
        desc.extend_from_slice(&width.to_be_bytes());
        desc.extend_from_slice(&height.to_be_bytes());
    } else {
        desc.extend_from_slice(&(width as u16).to_be_bytes());
        desc.extend_from_slice(&(height as u16).to_be_bytes());
    }
    desc
}

fn write_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn write_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn begin_box(out: &mut Vec<u8>, box_type: &[u8; 4]) -> usize {
    let pos = out.len();
    write_u32(out, 0);
    out.extend_from_slice(box_type);
    pos
}

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

fn write_ftyp(out: &mut Vec<u8>) {
    let pos = begin_box(out, b"ftyp");
    out.extend_from_slice(b"avif");
    write_u32(out, 0);
    out.extend_from_slice(b"avif");
    out.extend_from_slice(b"mif1");
    out.extend_from_slice(b"miaf");
    end_box(out, pos);
}

#[allow(clippy::too_many_arguments)]
fn write_meta_grid(
    out: &mut Vec<u8>,
    image_items: &[InfeBox],
    ipma_entries: &[IpmaEntry],
    ipco: &IpcoBox,
    irefs: &[IrefEntryBox],
    primary_id: u16,
    grid_descriptor: &[u8],
    alpha_grid_descriptor: Option<&[u8]>,
    alpha_grid_id: u16,
    tile_data: &[&[u8]],
    alpha_data: Option<&[&[u8]]>,
    color_tile_base: u16,
    alpha_tile_base: u16,
    tile_count: usize,
    has_alpha: bool,
) {
    let meta_pos = begin_box(out, b"meta");
    write_fullbox(out, 0, 0);

    // hdlr
    {
        let pos = begin_box(out, b"hdlr");
        write_fullbox(out, 0, 0);
        write_u32(out, 0);
        out.extend_from_slice(b"pict");
        out.extend_from_slice(&[0u8; 12]);
        out.push(0);
        end_box(out, pos);
    }

    // pitm
    {
        let pos = begin_box(out, b"pitm");
        write_fullbox(out, 0, 0);
        write_u16(out, primary_id);
        end_box(out, pos);
    }

    // iloc — uses placeholder offsets, patched after mdat
    {
        let pos = begin_box(out, b"iloc");
        write_fullbox(out, 0, 0);
        out.push(0x44); // offset_size=4, length_size=4
        out.push(0x00);

        // Count items: grid descriptors + tiles
        let mut item_count: u16 = 1 + tile_count as u16; // color grid + color tiles
        if has_alpha {
            item_count += 1 + tile_count as u16; // alpha grid + alpha tiles
        }
        write_u16(out, item_count);

        // Color grid item
        write_u16(out, primary_id);
        write_u16(out, 0); // data_reference_index
        write_u16(out, 1); // extent_count
        write_u32(out, ILOC_PLACEHOLDER);
        write_u32(out, grid_descriptor.len() as u32);

        // Alpha grid item
        if has_alpha {
            write_u16(out, alpha_grid_id);
            write_u16(out, 0);
            write_u16(out, 1);
            write_u32(out, ILOC_PLACEHOLDER);
            write_u32(out, alpha_grid_descriptor.map_or(0, |d| d.len() as u32));
        }

        // Color tile items
        for (i, tile) in tile_data.iter().enumerate() {
            write_u16(out, color_tile_base + i as u16);
            write_u16(out, 0);
            write_u16(out, 1);
            write_u32(out, ILOC_PLACEHOLDER);
            write_u32(out, tile.len() as u32);
        }

        // Alpha tile items
        if let Some(alpha) = alpha_data {
            for (i, tile) in alpha.iter().enumerate() {
                write_u16(out, alpha_tile_base + i as u16);
                write_u16(out, 0);
                write_u16(out, 1);
                write_u32(out, ILOC_PLACEHOLDER);
                write_u32(out, tile.len() as u32);
            }
        }

        end_box(out, pos);
    }

    // iinf
    {
        let iinf_pos = begin_box(out, b"iinf");
        write_fullbox(out, 0, 0);
        write_u16(out, image_items.len() as u16);

        for item in image_items {
            let infe_pos = begin_box(out, b"infe");
            write_fullbox(out, 2, 0);
            write_u16(out, item.id);
            write_u16(out, 0); // protection_index
            out.extend_from_slice(&item.typ.0);
            out.push(0); // name (null-terminated)
            if !item.content_type.is_empty() {
                out.extend_from_slice(item.content_type.as_bytes());
                out.push(0);
            }
            end_box(out, infe_pos);
        }

        end_box(out, iinf_pos);
    }

    // iref
    if !irefs.is_empty() {
        let iref_pos = begin_box(out, b"iref");
        write_fullbox(out, 0, 0);
        for entry in irefs {
            let entry_pos = begin_box(out, &entry.typ.0);
            write_u16(out, entry.from_id);
            write_u16(out, 1); // reference_count
            write_u16(out, entry.to_id);
            end_box(out, entry_pos);
        }
        end_box(out, iref_pos);
    }

    // iprp (ipco + ipma)
    {
        let iprp_pos = begin_box(out, b"iprp");

        // ipco — serialize using the MpegBox trait
        {
            let mut tmp = Vec::new();
            let mut w = crate::writer::Writer::new(&mut tmp);
            let _ = ipco.write(&mut w);
            drop(w);
            out.extend_from_slice(&tmp);
        }

        // ipma
        {
            let pos = begin_box(out, b"ipma");
            write_fullbox(out, 0, 0);
            write_u32(out, ipma_entries.len() as u32);
            for entry in ipma_entries {
                write_u16(out, entry.item_id);
                out.push(entry.prop_ids.len() as u8);
                for &p in &entry.prop_ids {
                    out.push(p);
                }
            }
            end_box(out, pos);
        }

        end_box(out, iprp_pos);
    }

    end_box(out, meta_pos);
}

/// Patch iloc placeholder offsets with actual mdat offsets.
/// Items are laid out in iloc order: grid desc(s), then tiles.
fn patch_iloc_offsets(out: &mut [u8], mdat_data_start: u32) {
    let placeholder = ILOC_PLACEHOLDER.to_be_bytes();
    let mut current_offset = mdat_data_start;
    let mut i = 0;

    while i + 4 <= out.len() {
        if out[i..i + 4] == placeholder {
            // Read the length that follows this offset (4 bytes after)
            let len = if i + 8 <= out.len() {
                u32::from_be_bytes([out[i + 4], out[i + 5], out[i + 6], out[i + 7]])
            } else {
                0
            };

            out[i..i + 4].copy_from_slice(&current_offset.to_be_bytes());
            current_offset += len;
            i += 8; // skip past offset + length
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
    fn grid_2x2_roundtrip() {
        let tiles: Vec<Vec<u8>> = (0..4).map(|i| vec![i as u8; 100]).collect();
        let tile_refs: Vec<&[u8]> = tiles.iter().map(|t| t.as_slice()).collect();

        let image = GridImage {
            rows: 2,
            columns: 2,
            output_width: 200,
            output_height: 200,
            tile_width: 100,
            tile_height: 100,
            tile_data: &tile_refs,
            alpha_data: None,
            color_config: basic_av1c(),
            alpha_config: None,
            depth_bits: 8,
            colr: None,
            premultiplied_alpha: false,
        };

        let avif = serialize_grid(&image).unwrap();

        // Verify ftyp
        assert_eq!(&avif[4..8], b"ftyp");
        assert_eq!(&avif[8..12], b"avif");

        // Verify tile data is present in mdat
        for tile in &tiles {
            assert!(avif.windows(tile.len()).any(|w| w == tile.as_slice()),
                "tile data should be in output");
        }

        // Parse with zenavif-parse
        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let grid = parser.grid_config().expect("should have grid config");
        assert_eq!(grid.rows, 2);
        assert_eq!(grid.columns, 2);
        assert_eq!(grid.output_width, 200);
        assert_eq!(grid.output_height, 200);
        assert_eq!(parser.grid_tile_count(), 4);
    }

    #[test]
    fn grid_1x3_roundtrip() {
        let tiles: Vec<Vec<u8>> = (0..3).map(|i| vec![(i + 10) as u8; 50]).collect();
        let tile_refs: Vec<&[u8]> = tiles.iter().map(|t| t.as_slice()).collect();

        let image = GridImage {
            rows: 1,
            columns: 3,
            output_width: 300,
            output_height: 100,
            tile_width: 100,
            tile_height: 100,
            tile_data: &tile_refs,
            alpha_data: None,
            color_config: basic_av1c(),
            alpha_config: None,
            depth_bits: 8,
            colr: None,
            premultiplied_alpha: false,
        };

        let avif = serialize_grid(&image).unwrap();
        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let grid = parser.grid_config().expect("grid config");
        assert_eq!(grid.rows, 1);
        assert_eq!(grid.columns, 3);
        assert_eq!(parser.grid_tile_count(), 3);
    }

    #[test]
    fn grid_with_alpha() {
        let color_tiles: Vec<Vec<u8>> = (0..4).map(|i| vec![i as u8; 80]).collect();
        let alpha_tiles: Vec<Vec<u8>> = (0..4).map(|i| vec![(i + 100) as u8; 40]).collect();
        let color_refs: Vec<&[u8]> = color_tiles.iter().map(|t| t.as_slice()).collect();
        let alpha_refs: Vec<&[u8]> = alpha_tiles.iter().map(|t| t.as_slice()).collect();

        let image = GridImage {
            rows: 2,
            columns: 2,
            output_width: 128,
            output_height: 128,
            tile_width: 64,
            tile_height: 64,
            tile_data: &color_refs,
            alpha_data: Some(&alpha_refs),
            color_config: basic_av1c(),
            alpha_config: Some(mono_av1c()),
            depth_bits: 8,
            colr: None,
            premultiplied_alpha: false,
        };

        let avif = serialize_grid(&image).unwrap();

        // Should contain all color and alpha tile data
        for tile in &color_tiles {
            assert!(avif.windows(tile.len()).any(|w| w == tile.as_slice()));
        }
        for tile in &alpha_tiles {
            assert!(avif.windows(tile.len()).any(|w| w == tile.as_slice()));
        }

        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let grid = parser.grid_config().expect("grid config");
        assert_eq!(grid.rows, 2);
        assert_eq!(grid.columns, 2);
    }

    #[test]
    fn grid_wrong_tile_count_errors() {
        let tiles = vec![vec![0u8; 10]];
        let tile_refs: Vec<&[u8]> = tiles.iter().map(|t| t.as_slice()).collect();

        let image = GridImage {
            rows: 2,
            columns: 2,
            output_width: 200,
            output_height: 200,
            tile_width: 100,
            tile_height: 100,
            tile_data: &tile_refs, // only 1, need 4
            alpha_data: None,
            color_config: basic_av1c(),
            alpha_config: None,
            depth_bits: 8,
            colr: None,
            premultiplied_alpha: false,
        };

        assert!(serialize_grid(&image).is_err());
    }
}

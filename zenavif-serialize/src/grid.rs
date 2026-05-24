//! Grid (tiled) AVIF image serialization.
//!
//! Writes an AVIF file where the primary item is an ImageGrid descriptor
//! with `dimg` references to individual AV1 tile items.

use crate::boxes::*;
use arrayvec::ArrayVec;
use std::io;

/// Builder for grid (tiled) AVIF container serialization.
///
/// Holds codec configuration and optional metadata. Call [`serialize`](GridImage::serialize)
/// with per-encode data (layout, dimensions, tile data) to produce the AVIF file.
pub struct GridImage {
    color_config: Av1CBox,
    alpha_config: Option<Av1CBox>,
    depth_bits: u8,
    colr: Option<ColrBox>,
    premultiplied_alpha: bool,
}

impl Default for GridImage {
    fn default() -> Self { Self::new() }
}

impl GridImage {
    /// Create with sensible defaults (8-bit 4:2:0, no alpha, no colr).
    pub fn new() -> Self {
        Self {
            color_config: Av1CBox::default(),
            alpha_config: None,
            depth_bits: 8,
            colr: None,
            premultiplied_alpha: false,
        }
    }

    /// AV1 codec configuration for color tiles.
    pub fn set_color_config(&mut self, config: Av1CBox) -> &mut Self { self.color_config = config; self }
    /// AV1 codec configuration for alpha tiles.
    pub fn set_alpha_config(&mut self, config: Av1CBox) -> &mut Self { self.alpha_config = Some(config); self }
    /// Bit depth (8, 10, or 12). Default: 8.
    pub fn set_depth_bits(&mut self, depth: u8) -> &mut Self { self.depth_bits = depth; self }
    /// CICP color info (nclx).
    pub fn set_colr(&mut self, colr: ColrBox) -> &mut Self { self.colr = Some(colr); self }
    /// Whether alpha is premultiplied. Default: false.
    pub fn set_premultiplied_alpha(&mut self, premultiplied: bool) -> &mut Self { self.premultiplied_alpha = premultiplied; self }

    /// Serialize a grid AVIF image.
    ///
    /// - `rows`, `columns`: tile grid layout (1-256 each)
    /// - `output_width`, `output_height`: final image dimensions
    /// - `tile_width`, `tile_height`: dimensions of each tile
    /// - `tile_data`: AV1-encoded data for each tile in row-major order (length must equal `rows * columns`)
    /// - `alpha_data`: optional alpha tile data (same order and count as `tile_data`)
    #[allow(clippy::too_many_arguments)]
    pub fn serialize(
        &self,
        rows: u8,
        columns: u8,
        output_width: u32,
        output_height: u32,
        tile_width: u32,
        tile_height: u32,
        tile_data: &[&[u8]],
        alpha_data: Option<&[&[u8]]>,
    ) -> io::Result<Vec<u8>> {
        let tile_count = rows as usize * columns as usize;
        validate_tile_counts(tile_count, tile_data, alpha_data)?;

        let has_alpha = alpha_data.is_some() && self.alpha_config.is_some();
        let ids = ItemIds::assign(tile_count, has_alpha);

        let grid_descriptor = make_grid_descriptor(rows, columns, output_width, output_height);
        let alpha_grid_descriptor = has_alpha
            .then(|| make_grid_descriptor(rows, columns, output_width, output_height));

        let mut ipco = IpcoBox::new();
        let ipco_ids = self.populate_ipco(
            &mut ipco,
            output_width,
            output_height,
            tile_width,
            tile_height,
            has_alpha,
        )?;

        let mut image_items: Vec<InfeBox> = Vec::new();
        let mut ipma_entries: Vec<IpmaEntry> = Vec::new();
        let mut irefs: Vec<IrefEntryBox> = Vec::new();

        self.add_grid_items(
            &mut image_items,
            &mut ipma_entries,
            &mut irefs,
            ids,
            ipco_ids,
            has_alpha,
        );
        add_tile_items(
            &mut image_items,
            &mut ipma_entries,
            &mut irefs,
            ids,
            ipco_ids,
            tile_count,
            has_alpha,
        );

        let mut out = Vec::new();
        write_ftyp(&mut out);
        let iloc_offset_positions = write_meta_grid(
            &mut out,
            &image_items,
            &ipma_entries,
            &ipco,
            &irefs,
            ids.color_grid_id,
            &grid_descriptor,
            alpha_grid_descriptor.as_deref(),
            ids.alpha_grid_id,
            tile_data,
            alpha_data,
            ids.color_tile_base,
            ids.alpha_tile_base,
            tile_count,
            has_alpha,
        );
        let item_offsets = write_mdat(&mut out, &grid_descriptor, alpha_grid_descriptor.as_deref(), tile_data, alpha_data);

        debug_assert_eq!(iloc_offset_positions.len(), item_offsets.len());
        patch_iloc_offsets(&mut out, &iloc_offset_positions, &item_offsets);

        Ok(out)
    }

    fn populate_ipco(
        &self,
        ipco: &mut IpcoBox,
        output_width: u32,
        output_height: u32,
        tile_width: u32,
        tile_height: u32,
        has_alpha: bool,
    ) -> io::Result<IpcoIds> {
        let ispe_output = push_prop(ipco, IpcoProp::Ispe(IspeBox { width: output_width, height: output_height }))?;
        let ispe_tile = push_prop(ipco, IpcoProp::Ispe(IspeBox { width: tile_width, height: tile_height }))?;
        let av1c_color = push_prop(ipco, IpcoProp::Av1C(self.color_config))?;
        let pixi_color = push_prop(
            ipco,
            IpcoProp::Pixi(PixiBox {
                channels: if self.color_config.monochrome { 1 } else { 3 },
                depth: self.depth_bits,
            }),
        )?;

        let colr = match self.colr {
            Some(c) if c != ColrBox::default() => Some(push_prop(ipco, IpcoProp::Colr(c))?),
            _ => None,
        };

        let (av1c_alpha, pixi_alpha, auxc_alpha) = if has_alpha {
            let ac = push_prop(ipco, IpcoProp::Av1C(*self.alpha_config.as_ref().unwrap()))?;
            let pa = push_prop(ipco, IpcoProp::Pixi(PixiBox { channels: 1, depth: self.depth_bits }))?;
            let aux = push_prop(
                ipco,
                IpcoProp::AuxC(AuxCBox { urn: "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha" }),
            )?;
            (Some(ac), Some(pa), Some(aux))
        } else {
            (None, None, None)
        };

        Ok(IpcoIds {
            ispe_output,
            ispe_tile,
            av1c_color,
            pixi_color,
            colr,
            av1c_alpha,
            pixi_alpha,
            auxc_alpha,
        })
    }

    fn add_grid_items(
        &self,
        image_items: &mut Vec<InfeBox>,
        ipma_entries: &mut Vec<IpmaEntry>,
        irefs: &mut Vec<IrefEntryBox>,
        ids: ItemIds,
        ipco_ids: IpcoIds,
        has_alpha: bool,
    ) {
        image_items.push(InfeBox {
            id: ids.color_grid_id,
            typ: FourCC(*b"grid"),
            name: "",
            content_type: "",
        });
        ipma_entries.push(IpmaEntry {
            item_id: ids.color_grid_id,
            prop_ids: prop_ids_from(
                [ipco_ids.ispe_output, ipco_ids.pixi_color]
                    .into_iter()
                    .chain(ipco_ids.colr),
            ),
        });

        if !has_alpha {
            return;
        }

        image_items.push(InfeBox {
            id: ids.alpha_grid_id,
            typ: FourCC(*b"grid"),
            name: "",
            content_type: "",
        });
        irefs.push(IrefEntryBox {
            from_id: ids.alpha_grid_id,
            to_id: ids.color_grid_id,
            typ: FourCC(*b"auxl"),
        });
        if self.premultiplied_alpha {
            irefs.push(IrefEntryBox {
                from_id: ids.color_grid_id,
                to_id: ids.alpha_grid_id,
                typ: FourCC(*b"prem"),
            });
        }
        ipma_entries.push(IpmaEntry {
            item_id: ids.alpha_grid_id,
            prop_ids: prop_ids_from([
                ipco_ids.ispe_output,
                ipco_ids.pixi_alpha.expect("alpha pixi when has_alpha"),
                ipco_ids.auxc_alpha.expect("alpha auxc when has_alpha"),
            ]),
        });
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

const ILOC_PLACEHOLDER: u32 = 0xBAAD_F00D;
const ESSENTIAL_BIT: u8 = 0x80;

#[derive(Clone, Copy)]
struct ItemIds {
    color_grid_id: u16,
    alpha_grid_id: u16,
    color_tile_base: u16,
    alpha_tile_base: u16,
}

impl ItemIds {
    // Item ID layout: 1 = color grid, 2 = alpha grid (when present), then color tiles, then alpha tiles.
    fn assign(tile_count: usize, has_alpha: bool) -> Self {
        let color_tile_base: u16 = if has_alpha { 3 } else { 2 };
        Self {
            color_grid_id: 1,
            alpha_grid_id: 2,
            color_tile_base,
            alpha_tile_base: color_tile_base + tile_count as u16,
        }
    }
}

#[derive(Clone, Copy)]
struct IpcoIds {
    ispe_output: u8,
    ispe_tile: u8,
    av1c_color: u8,
    pixi_color: u8,
    colr: Option<u8>,
    av1c_alpha: Option<u8>,
    pixi_alpha: Option<u8>,
    auxc_alpha: Option<u8>,
}

fn validate_tile_counts(
    tile_count: usize,
    tile_data: &[&[u8]],
    alpha_data: Option<&[&[u8]]>,
) -> io::Result<()> {
    if tile_data.len() != tile_count {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("tile_data.len() ({}) != rows*columns ({})", tile_data.len(), tile_count),
        ));
    }
    if let Some(alpha) = alpha_data
        && alpha.len() != tile_count
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("alpha_data.len() ({}) != rows*columns ({})", alpha.len(), tile_count),
        ));
    }
    Ok(())
}

fn push_prop(ipco: &mut IpcoBox, prop: IpcoProp) -> io::Result<u8> {
    ipco.push(prop).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
}

fn prop_ids_from<I: IntoIterator<Item = u8>>(iter: I) -> ArrayVec<u8, 12> {
    let mut v = ArrayVec::new();
    for id in iter {
        v.push(id);
    }
    v
}

#[derive(Clone, Copy)]
struct TileGroup {
    parent_grid_id: u16,
    base_id: u16,
    ispe_tile: u8,
    av1c_essential: u8,
    tile_count: usize,
}

fn add_tile_items(
    image_items: &mut Vec<InfeBox>,
    ipma_entries: &mut Vec<IpmaEntry>,
    irefs: &mut Vec<IrefEntryBox>,
    ids: ItemIds,
    ipco_ids: IpcoIds,
    tile_count: usize,
    has_alpha: bool,
) {
    add_one_tile_group(image_items, ipma_entries, irefs, TileGroup {
        parent_grid_id: ids.color_grid_id,
        base_id: ids.color_tile_base,
        ispe_tile: ipco_ids.ispe_tile,
        av1c_essential: ipco_ids.av1c_color | ESSENTIAL_BIT,
        tile_count,
    });
    if has_alpha {
        add_one_tile_group(image_items, ipma_entries, irefs, TileGroup {
            parent_grid_id: ids.alpha_grid_id,
            base_id: ids.alpha_tile_base,
            ispe_tile: ipco_ids.ispe_tile,
            av1c_essential: ipco_ids.av1c_alpha.expect("alpha av1c when has_alpha") | ESSENTIAL_BIT,
            tile_count,
        });
    }
}

fn add_one_tile_group(
    image_items: &mut Vec<InfeBox>,
    ipma_entries: &mut Vec<IpmaEntry>,
    irefs: &mut Vec<IrefEntryBox>,
    g: TileGroup,
) {
    for i in 0..g.tile_count {
        let tile_id = g.base_id + i as u16;
        image_items.push(InfeBox {
            id: tile_id,
            typ: FourCC(*b"av01"),
            name: "",
            content_type: "",
        });
        irefs.push(IrefEntryBox {
            from_id: g.parent_grid_id,
            to_id: tile_id,
            typ: FourCC(*b"dimg"),
        });
        ipma_entries.push(IpmaEntry {
            item_id: tile_id,
            prop_ids: prop_ids_from([g.ispe_tile, g.av1c_essential]),
        });
    }
}

/// Append mdat to `out` and return per-item byte offsets in the order matching iloc placeholders:
/// color grid descriptor, optional alpha grid descriptor, color tiles, optional alpha tiles.
///
/// We record exact per-item offsets rather than scanning for sentinel byte patterns — AV1 payloads
/// can legitimately contain any 4-byte sequence, so scan-and-patch is incorrect (and exploitable).
fn write_mdat(
    out: &mut Vec<u8>,
    grid_descriptor: &[u8],
    alpha_grid_descriptor: Option<&[u8]>,
    tile_data: &[&[u8]],
    alpha_data: Option<&[&[u8]]>,
) -> Vec<u32> {
    let mut item_offsets: Vec<u32> = Vec::new();
    let mdat_pos = begin_box(out, b"mdat");

    item_offsets.push(out.len() as u32);
    out.extend_from_slice(grid_descriptor);

    if let Some(agd) = alpha_grid_descriptor {
        item_offsets.push(out.len() as u32);
        out.extend_from_slice(agd);
    }

    for tile in tile_data {
        item_offsets.push(out.len() as u32);
        out.extend_from_slice(tile);
    }

    if let Some(alpha) = alpha_data {
        for tile in alpha {
            item_offsets.push(out.len() as u32);
            out.extend_from_slice(tile);
        }
    }

    end_box(out, mdat_pos);
    item_offsets
}

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
) -> Vec<usize> {
    // Records the exact byte offset of each iloc extent_offset placeholder, so the patch
    // step can write directly to those positions instead of scanning the buffer for a
    // sentinel byte pattern.
    let mut iloc_offset_positions: Vec<usize> = Vec::new();
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
        iloc_offset_positions.push(out.len());
        write_u32(out, ILOC_PLACEHOLDER);
        write_u32(out, grid_descriptor.len() as u32);

        // Alpha grid item
        if has_alpha {
            write_u16(out, alpha_grid_id);
            write_u16(out, 0);
            write_u16(out, 1);
            iloc_offset_positions.push(out.len());
            write_u32(out, ILOC_PLACEHOLDER);
            write_u32(out, alpha_grid_descriptor.map_or(0, |d| d.len() as u32));
        }

        // Color tile items
        for (i, tile) in tile_data.iter().enumerate() {
            write_u16(out, color_tile_base + i as u16);
            write_u16(out, 0);
            write_u16(out, 1);
            iloc_offset_positions.push(out.len());
            write_u32(out, ILOC_PLACEHOLDER);
            write_u32(out, tile.len() as u32);
        }

        // Alpha tile items
        if let Some(alpha) = alpha_data {
            for (i, tile) in alpha.iter().enumerate() {
                write_u16(out, alpha_tile_base + i as u16);
                write_u16(out, 0);
                write_u16(out, 1);
                iloc_offset_positions.push(out.len());
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
    iloc_offset_positions
}

/// Patch iloc extent_offset placeholders at recorded byte positions.
///
/// Writes each item's actual mdat offset into the precise 4-byte slot recorded when
/// the placeholder was emitted. This avoids scanning the output for sentinel byte
/// patterns, which would corrupt user-supplied AV1 tile payloads that happen to
/// contain the same bytes (`ILOC_PLACEHOLDER`) — a real possibility for compressed
/// data, and one an adversary could trigger deliberately.
fn patch_iloc_offsets(out: &mut [u8], iloc_offset_positions: &[usize], item_offsets: &[u32]) {
    debug_assert_eq!(iloc_offset_positions.len(), item_offsets.len());
    for (&pos, &offset) in iloc_offset_positions.iter().zip(item_offsets.iter()) {
        // Defensive bounds check; positions come from our own writes so this is structurally safe.
        if pos + 4 <= out.len() {
            out[pos..pos + 4].copy_from_slice(&offset.to_be_bytes());
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

        let mut image = GridImage::new();
        image.set_color_config(basic_av1c());

        let avif = image.serialize(2, 2, 200, 200, 100, 100, &tile_refs, None).unwrap();

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

        let mut image = GridImage::new();
        image.set_color_config(basic_av1c());

        let avif = image.serialize(1, 3, 300, 100, 100, 100, &tile_refs, None).unwrap();
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

        let mut image = GridImage::new();
        image.set_color_config(basic_av1c());
        image.set_alpha_config(mono_av1c());

        let avif = image.serialize(2, 2, 128, 128, 64, 64, &color_refs, Some(&alpha_refs)).unwrap();

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
    fn grid_tile_data_containing_iloc_sentinel_is_not_corrupted() {
        // Regression: the iloc patcher used to scan the entire output buffer for
        // 0xBAADF00D and overwrite any match. AV1 payloads can legitimately contain
        // those bytes; this test deliberately seeds them and asserts that the tile
        // data is preserved byte-for-byte after serialization.
        let sentinel = ILOC_PLACEHOLDER.to_be_bytes();
        let mut tile0 = vec![0xAAu8; 32];
        tile0.extend_from_slice(&sentinel);            // raw sentinel
        tile0.extend_from_slice(&[0xCC; 4]);            // would be read as "length"
        tile0.extend_from_slice(&sentinel);            // adjacent second sentinel
        tile0.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        tile0.extend_from_slice(&[0xBB; 32]);

        // Other tiles also carry the sentinel mid-payload.
        let mut tile1 = vec![0x55u8; 16];
        tile1.extend_from_slice(&sentinel);
        tile1.extend_from_slice(&[0xEEu8; 80]);
        let tile2 = vec![0x77u8; 64];
        let mut tile3 = vec![0x88u8; 16];
        tile3.extend_from_slice(&sentinel);
        tile3.extend_from_slice(&[0x99u8; 16]);

        let tiles = [tile0.clone(), tile1.clone(), tile2.clone(), tile3.clone()];
        let tile_refs: Vec<&[u8]> = tiles.iter().map(|t| t.as_slice()).collect();

        let mut image = GridImage::new();
        image.set_color_config(basic_av1c());
        let avif = image.serialize(2, 2, 200, 200, 100, 100, &tile_refs, None).unwrap();

        // Resolve each tile via the iloc table and confirm its bytes match exactly.
        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        assert_eq!(parser.grid_tile_count(), 4);
        let originals = [tile0.as_slice(), tile1.as_slice(), tile2.as_slice(), tile3.as_slice()];
        for (i, original) in originals.iter().enumerate() {
            let got = parser.tile_data(i).expect("tile data");
            assert_eq!(got.as_ref(), *original,
                "tile {i} corrupted by placeholder scan or iloc misaligned");
        }
    }

    #[test]
    fn grid_wrong_tile_count_errors() {
        let tiles = [vec![0u8; 10]];
        let tile_refs: Vec<&[u8]> = tiles.iter().map(|t| t.as_slice()).collect();

        let image = GridImage::new();
        // only 1 tile, need 4
        assert!(image.serialize(2, 2, 200, 200, 100, 100, &tile_refs, None).is_err());
    }
}

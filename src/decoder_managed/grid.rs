//! Grid (tiled) AVIF decode and canvas assembly.
//!
//! A container `grid` item is a set of independently-coded AV1 stills laid
//! out in a rows x columns canvas — unrelated to AV1 bitstream tiles, which
//! the decoder handles internally. Tiles are decoded, converted, then
//! byte-stitched; `stitch_tile_images` is shared with the aom backend.

use super::ManagedAvifDecoder;
use crate::error::{Error, Result};
use enough::Stop;
use rav1d_safe::src::managed::Frame;
use whereat::at;
use zenpixels::PixelBuffer;

impl ManagedAvifDecoder {
    /// Decode a grid-based AVIF (tiled image)
    pub(super) fn decode_grid(&mut self, stop: &(impl Stop + ?Sized)) -> Result<PixelBuffer> {
        let grid_config = self
            .parser
            .grid_config()
            .ok_or_else(|| {
                at!(Error::Decode {
                    code: -1,
                    msg: "Expected grid config but found none",
                })
            })?
            .clone();

        self.reject_grid_alpha()?;

        // Decode all tiles
        let mut tile_frames = Vec::new();
        for i in 0..self.parser.grid_tile_count() {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

            let tile_data = self
                .parser
                .tile_data(i)
                .map_err(|e| e.map_error(Error::Parse))?;
            let frame = Self::decode_frame(
                &mut self.decoder,
                &tile_data,
                "Failed to decode grid tile",
                stop,
            )?;

            tile_frames.push(frame);
        }

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        // Stitch tiles together
        self.stitch_tiles(tile_frames, &grid_config, stop)
    }

    /// Grid alpha (per-tile `auxl` alpha items or an alpha grid item) is not
    /// stitched yet. Decoding the color grid alone would silently return
    /// opaque pixels for a file that carries transparency — refuse instead.
    pub(super) fn reject_grid_alpha(&self) -> Result<()> {
        if self.parser.alpha_data().is_some() || self.parser.has_alpha_aux_items() {
            return Err(at!(Error::Unsupported(
                "grid AVIF with alpha auxiliary items: alpha-grid stitching is \
                 not implemented, and decoding only the color grid would \
                 silently drop transparency"
            )));
        }
        Ok(())
    }

    /// Stitch decoded tile frames into a single image
    fn stitch_tiles(
        &self,
        tiles: Vec<Frame>,
        grid_config: &zenavif_parse::GridConfig,
        stop: &(impl Stop + ?Sized),
    ) -> Result<PixelBuffer> {
        if tiles.is_empty() {
            return Err(at!(Error::Malformed("No tiles to stitch")));
        }

        let rows = grid_config.rows as usize;
        let cols = grid_config.columns as usize;

        if tiles.len() != rows * cols {
            return Err(at!(Error::Malformed(
                "Tile count doesn't match grid dimensions"
            )));
        }

        // Get dimensions from first tile (all tiles should be same size)
        let tile_width = tiles[0].width() as usize;
        let tile_height = tiles[0].height() as usize;

        // Calculate output dimensions
        let output_width = if grid_config.output_width > 0 {
            grid_config.output_width as usize
        } else {
            tile_width * cols
        };
        let output_height = if grid_config.output_height > 0 {
            grid_config.output_height as usize
        } else {
            tile_height * rows
        };

        // Convert each tile to RGB/RGBA
        let mut tile_images = Vec::new();
        for tile in tiles {
            let (img, _info) = self.convert_to_image(tile, None, stop)?;
            tile_images.push(img);
        }

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
        self.stitch_tile_images(tile_images, cols, output_width, output_height)
    }

    /// Byte-stitch converted tile images into the grid canvas
    /// (format-agnostic; shared by the rav1d and aom grid paths).
    pub(super) fn stitch_tile_images(
        &self,
        tile_images: Vec<PixelBuffer>,
        cols: usize,
        output_width: usize,
        output_height: usize,
    ) -> Result<PixelBuffer> {
        let descriptor = tile_images[0].descriptor();
        let bpp = descriptor.bytes_per_pixel();
        let (tile_w, tile_h) =
            validate_tile_uniformity(&tile_images, cols, output_width, output_height)?;
        let alloc_size = output_width
            .checked_mul(output_height)
            .and_then(|n| n.checked_mul(bpp))
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        // Full grid-stitch canvas, sized from the (untrusted) grid output
        // dimensions → fallible by default.
        let data = crate::alloc_util::alloc_filled(self.alloc_pref, true, 0u8, alloc_size)?;
        let mut output =
            PixelBuffer::from_vec(data, output_width as u32, output_height as u32, descriptor)
                .map_err(|_| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "failed to create output buffer for grid stitch",
                    })
                })?;

        for (tile_idx, tile) in tile_images.iter().enumerate() {
            let row = tile_idx / cols;
            let col = tile_idx % cols;
            let dst_x = col * tile_w;
            let dst_y = row * tile_h;
            stitch_tile_into_buffer(
                tile,
                &mut output,
                dst_x,
                dst_y,
                output_width,
                output_height,
                bpp,
            );
        }

        Ok(output)
    }

    /// Decode one tile-row of a grid image, returning converted pixel buffers.
    ///
    /// Each tile is decoded from AV1 and color-converted before the next,
    /// so peak memory is one raw Frame + one converted PixelBuffer per tile.
    #[allow(dead_code)]
    pub(crate) fn decode_tile_row(
        &mut self,
        grid_row: usize,
        cols: usize,
        stop: &(impl Stop + ?Sized),
    ) -> Result<Vec<PixelBuffer>> {
        let mut row_tiles = Vec::with_capacity(cols);
        for col in 0..cols {
            let tile_idx = grid_row * cols + col;
            let tile_data = self
                .parser
                .tile_data(tile_idx)
                .map_err(|e| e.map_error(Error::Parse))?;
            let frame = Self::decode_frame(
                &mut self.decoder,
                &tile_data,
                "Failed to decode grid tile",
                stop,
            )?;
            let (pixels, _info) = self.convert_to_image(frame, None, stop)?;
            row_tiles.push(pixels);
        }
        Ok(row_tiles)
    }
}

/// Validate a decoded tile set against the declared grid geometry and return
/// the single authoritative tile size.
///
/// HEIF/MIAF grids require uniform input-image sizes; placement is computed
/// from the validated tile-0 size, never from each tile's own decoded dims —
/// a crafted grid with one differently-sized tile would otherwise be silently
/// misplaced over earlier tiles or leave zero-filled holes (sweep issue #40).
/// Tiles must also cover the declared output canvas; larger coverage is the
/// spec-legal right/bottom-edge crop, clipped per row by the caller.
fn validate_tile_uniformity(
    tile_images: &[PixelBuffer],
    cols: usize,
    output_width: usize,
    output_height: usize,
) -> Result<(usize, usize)> {
    let descriptor = tile_images[0].descriptor();
    let tile_w = tile_images[0].width() as usize;
    let tile_h = tile_images[0].height() as usize;
    for tile in tile_images {
        if tile.width() as usize != tile_w
            || tile.height() as usize != tile_h
            || tile.descriptor() != descriptor
        {
            return Err(at!(Error::Malformed(
                "grid tiles decoded to non-uniform dimensions or formats"
            )));
        }
    }
    let rows = tile_images.len() / cols;
    if tile_w.saturating_mul(cols) < output_width || tile_h.saturating_mul(rows) < output_height {
        return Err(at!(Error::Malformed(
            "grid tiles do not cover the declared output dimensions"
        )));
    }
    Ok((tile_w, tile_h))
}

/// Copy one tile's pixels into the stitched grid output buffer.
///
/// Uses `saturating_sub` for the available width/height so a malformed AVIF
/// whose declared `output_width`/`output_height` are smaller than the actual
/// tile placement (`dst_x`/`dst_y`) does not trigger a usize underflow panic.
/// Tiles that fall entirely outside the declared output area are silently
/// skipped (zero-length copy range).
fn stitch_tile_into_buffer(
    tile: &PixelBuffer,
    output: &mut PixelBuffer,
    dst_x: usize,
    dst_y: usize,
    output_width: usize,
    output_height: usize,
    bpp: usize,
) {
    let tile_w = tile.width() as usize;
    let tile_h = tile.height() as usize;

    // Saturating arithmetic: if the tile's destination origin lies outside the
    // declared output dimensions, avail_* is 0 and we skip the copy entirely.
    // Bailing out also avoids indexing the destination row at `dst_x * bpp`
    // when `dst_x >= output_width` (which would still panic even though
    // copy_bytes is 0, because the slice index range start is out of bounds).
    let avail_h = output_height.saturating_sub(dst_y);
    let avail_w = output_width.saturating_sub(dst_x);
    if avail_h == 0 || avail_w == 0 {
        return;
    }

    let tile_slice = tile.as_slice();
    let mut out_slice = output.as_slice_mut();
    for y in 0..tile_h.min(avail_h) {
        let src = tile_slice.row(y as u32);
        let copy_w = tile_w.min(avail_w);
        let copy_bytes = copy_w * bpp;
        let dst_row = out_slice.row_mut((dst_y + y) as u32);
        let dst_start = dst_x * bpp;
        dst_row[dst_start..dst_start + copy_bytes].copy_from_slice(&src[..copy_bytes]);
    }
}

#[cfg(test)]
mod stitch_tests {
    use super::*;
    use zenpixels::PixelDescriptor;

    fn make_buffer(w: u32, h: u32, fill: u8) -> PixelBuffer {
        let descriptor = PixelDescriptor::RGBA8_SRGB;
        let bpp = descriptor.bytes_per_pixel();
        let data = vec![fill; (w as usize) * (h as usize) * bpp];
        PixelBuffer::from_vec(data, w, h, descriptor).expect("buffer alloc")
    }

    /// Regression test for the H1 finding: a crafted AVIF where a grid tile's
    /// destination origin (dst_x/dst_y) exceeds the declared output dimensions
    /// must not panic with a usize underflow. Before the fix, computing
    /// `output_height - dst_y` underflowed and panicked.
    #[test]
    fn stitch_does_not_panic_when_tile_origin_exceeds_output() {
        let tile = make_buffer(64, 64, 0xAB);
        let mut output = make_buffer(32, 32, 0);
        // dst_y > output_height — would underflow without saturating_sub.
        stitch_tile_into_buffer(&tile, &mut output, 0, 64, 32, 32, 4);
        // Output buffer untouched, no panic.
        assert_eq!(output.as_slice().row(0)[0], 0);
    }

    #[test]
    fn stitch_does_not_panic_when_tile_x_exceeds_output() {
        let tile = make_buffer(64, 64, 0xCD);
        let mut output = make_buffer(32, 32, 0);
        // dst_x > output_width — would underflow without saturating_sub.
        stitch_tile_into_buffer(&tile, &mut output, 64, 0, 32, 32, 4);
        assert_eq!(output.as_slice().row(0)[0], 0);
    }

    /// A tile whose origin is exactly at the output edge contributes nothing
    /// (avail_* == 0) but must not panic.
    #[test]
    fn stitch_zero_avail_at_exact_edge() {
        let tile = make_buffer(16, 16, 0xEE);
        let mut output = make_buffer(32, 32, 0);
        stitch_tile_into_buffer(&tile, &mut output, 32, 0, 32, 32, 4);
        stitch_tile_into_buffer(&tile, &mut output, 0, 32, 32, 32, 4);
        assert_eq!(output.as_slice().row(0)[0], 0);
    }

    /// A tile set with one differently-sized tile must be rejected — placing
    /// it by its own dims (the pre-#40 behavior) silently scrambled the
    /// canvas.
    #[test]
    fn uniformity_rejects_mismatched_tile() {
        let tiles = vec![
            make_buffer(64, 64, 1),
            make_buffer(64, 64, 2),
            make_buffer(32, 32, 3),
            make_buffer(64, 64, 4),
        ];
        assert!(validate_tile_uniformity(&tiles, 2, 128, 128).is_err());
    }

    /// Tiles smaller than the declared canvas coverage must be rejected —
    /// zero-filled holes are not a decode result.
    #[test]
    fn uniformity_rejects_undersized_coverage() {
        let tiles = vec![make_buffer(32, 32, 1), make_buffer(32, 32, 2)];
        // 2 cols x 1 row of 32px tiles cannot cover a declared 128x32 canvas.
        assert!(validate_tile_uniformity(&tiles, 2, 128, 32).is_err());
    }

    /// The spec-legal shape passes: uniform tiles whose coverage equals or
    /// exceeds the output (right/bottom crop).
    #[test]
    fn uniformity_accepts_uniform_tiles_with_edge_crop() {
        let tiles = vec![
            make_buffer(64, 64, 1),
            make_buffer(64, 64, 2),
            make_buffer(64, 64, 3),
            make_buffer(64, 64, 4),
        ];
        // Declared output 100x100 < 128x128 coverage: legal edge crop.
        let (w, h) = validate_tile_uniformity(&tiles, 2, 100, 100).expect("legal grid");
        assert_eq!((w, h), (64, 64));
    }

    /// Sanity check: a normally-placed tile still gets copied.
    #[test]
    fn stitch_copies_within_bounds() {
        let tile = make_buffer(8, 8, 0x42);
        let mut output = make_buffer(16, 16, 0);
        stitch_tile_into_buffer(&tile, &mut output, 0, 0, 16, 16, 4);
        // First row, first pixel byte should now be 0x42.
        assert_eq!(output.as_slice().row(0)[0], 0x42);
    }
}

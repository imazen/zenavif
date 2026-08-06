//! [`AvifStreamingDecoder`] — strip emission for the three streaming shapes
//! (grid tile-row stitching, on-demand YUV strip conversion, and pre-baked
//! full-buffer strips) behind [`zencodec::decode::StreamingDecode`].
//!
//! Not re-exported from `crate`: [`AvifStreamingDecoder`] reaches callers
//! only as `<AvifDecodeJob as DecodeJob>::StreamDec`.

use std::sync::Arc;

use whereat::{At, ResultAtExt as _};
use zencodec::{CodecError, ImageInfo};
use zenpixels::{PixelBuffer, PixelDescriptor, PixelSlice};

use crate::error::Error;

/// Streaming AVIF decoder with real tile-row streaming for grid images.
///
/// For grid (tiled) images, each [`next_batch`](zencodec::decode::StreamingDecode::next_batch)
/// call decodes one tile-row of AV1 tiles, color-converts them, and stitches
/// them into a strip. Peak memory is proportional to one tile-row instead of
/// the full image.
///
/// For non-grid 8-bit color images, the decoded YUV frame is held in memory
/// and converted strip-by-strip on demand. This eliminates the full RGB
/// allocation and keeps the working set in L2 cache.
///
/// For non-grid 16-bit or monochrome images, falls back to full-frame
/// conversion and emits fixed-height strips.
pub struct AvifStreamingDecoder {
    pub(super) info: ImageInfo,
    pub(super) y_offset: u32,
    pub(super) output_width: u32,
    pub(super) output_height: u32,
    /// Grid path: managed decoder for tile-row streaming.
    pub(super) decoder: Option<crate::ManagedAvifDecoder>,
    /// Stop token for cancellable grid decoding.
    pub(super) stop: zencodec::StopToken,
    pub(super) grid_rows: u32,
    pub(super) grid_cols: u32,
    pub(super) current_grid_row: u32,
    /// Pixel descriptor with CICP metadata for strip buffers.
    pub(super) strip_descriptor: PixelDescriptor,
    /// Reusable strip buffer for the current tile-row or strip conversion.
    pub(super) strip_buffer: Option<PixelBuffer>,
    /// Non-grid strip conversion: holds decoded YUV frames, converts on demand.
    pub(super) strip_converter: Option<crate::strip_convert::StripConverter>,
    /// Optimal strip height for the strip converter path.
    pub(super) strip_height: u32,
    /// Class-gated color context applied to every emitted strip: the
    /// scratch `strip_buffer` is rebuilt per batch without one, so the
    /// context is re-attached at emission. `None` when the source
    /// carries no color signaling (or the HDR/bake source buffer had
    /// none).
    pub(super) strip_color_context: Option<Arc<zenpixels::ColorContext>>,
    /// Bake path (`OrientationHint::bakes()`): the fully-decoded, orientation-
    /// baked buffer. Orientation is not strip-local (transposes need the whole
    /// image), so the bake path materializes upright once and emits it in
    /// fixed-height strips. `None` on the preserve path (the default), where the
    /// grid / strip-converter fields drive low-memory streaming unchanged.
    pub(super) baked: Option<PixelBuffer>,
}

impl AvifStreamingDecoder {
    /// Stitch decoded tiles horizontally into `self.strip_buffer`.
    fn stitch_tiles(&mut self, tiles: &[PixelBuffer], strip_h: u32) {
        let bpp = self.strip_descriptor.bytes_per_pixel();
        let mut strip = PixelBuffer::new(self.output_width, strip_h, self.strip_descriptor);
        {
            let mut sm = strip.as_slice_mut();
            for py in 0..strip_h {
                let dst_row = sm.row_mut(py);
                let mut x_offset = 0usize;
                for tile in tiles {
                    let tile_w = tile.width() as usize;
                    let actual_w =
                        tile_w.min((self.output_width as usize).saturating_sub(x_offset));
                    // Guard the source row by each tile's own height: grid tiles in
                    // a row may decode to different heights, and `strip_h` is taken
                    // from the first tile, so a shorter tile would make
                    // `tile.row(py)` panic. A tile that is off-canvas (actual_w ==
                    // 0) or too short for this row contributes nothing; still
                    // advance x_offset so later tiles in the row line up.
                    if actual_w != 0 && py < tile.height() {
                        let tile_slice = tile.as_slice();
                        let src = tile_slice.row(py);
                        let copy_bytes = actual_w * bpp;
                        let dst_start = x_offset * bpp;
                        dst_row[dst_start..dst_start + copy_bytes]
                            .copy_from_slice(&src[..copy_bytes]);
                    }
                    x_offset += tile_w;
                }
            }
        }
        self.strip_buffer = Some(strip);
    }
}

impl zencodec::decode::StreamingDecode for AvifStreamingDecoder {
    type Error = At<CodecError>;

    fn next_batch(&mut self) -> Result<Option<(u32, PixelSlice<'_>)>, At<CodecError>> {
        self.next_batch_inner().map_err(zencodec::CodecError::of)
    }

    fn info(&self) -> &ImageInfo {
        &self.info
    }
}

impl AvifStreamingDecoder {
    fn next_batch_inner(&mut self) -> Result<Option<(u32, PixelSlice<'_>)>, At<Error>> {
        if self.y_offset >= self.output_height {
            return Ok(None);
        }

        // Bake path: emit fixed-height strips copied from the pre-baked,
        // orientation-corrected full buffer.
        if let Some(ref baked) = self.baked {
            let remaining = self.output_height - self.y_offset;
            let h = self.strip_height.min(remaining);
            if h == 0 {
                return Ok(None);
            }
            let desc = self.strip_descriptor;
            let width = self.output_width;
            let strip_buf = self
                .strip_buffer
                .get_or_insert_with(|| PixelBuffer::new(width, h, desc));
            if strip_buf.height() != h {
                *strip_buf = PixelBuffer::new(width, h, desc);
            }
            {
                let baked_slice = baked.as_slice();
                let mut sm = strip_buf.as_slice_mut();
                for row in 0..h {
                    sm.row_mut(row)
                        .copy_from_slice(baked_slice.row(self.y_offset + row));
                }
            }
            let y = self.y_offset;
            self.y_offset += h;
            let slice = self.strip_buffer.as_ref().unwrap().as_slice().erase();
            let slice = match &self.strip_color_context {
                Some(ctx) => slice.with_color_context(Arc::clone(ctx)),
                None => slice,
            };
            return Ok(Some((y, slice)));
        }

        if self.decoder.is_some() {
            // Grid path: decode one tile-row per call.
            if self.current_grid_row >= self.grid_rows {
                return Ok(None);
            }

            let tiles = self.decoder.as_mut().unwrap().decode_tile_row(
                self.current_grid_row as usize,
                self.grid_cols as usize,
                &self.stop,
            )?;

            if tiles.is_empty() {
                return Ok(None);
            }

            let tile_h = tiles[0].height();
            let strip_h = tile_h.min(self.output_height.saturating_sub(self.y_offset));
            if strip_h == 0 {
                return Ok(None);
            }

            self.stitch_tiles(&tiles, strip_h);
            self.current_grid_row += 1;

            let y = self.y_offset;
            self.y_offset += strip_h;
            let slice = self.strip_buffer.as_ref().unwrap().as_slice().erase();
            let slice = match &self.strip_color_context {
                Some(ctx) => slice.with_color_context(Arc::clone(ctx)),
                None => slice,
            };
            return Ok(Some((y, slice)));
        }

        // Non-grid: convert strip from decoded YUV frames on demand.
        if let Some(ref converter) = self.strip_converter {
            let remaining = self.output_height - self.y_offset;
            let h = self.strip_height.min(remaining);
            if h == 0 {
                return Ok(None);
            }

            // Ensure strip buffer exists with the right dimensions
            let desc = self.strip_descriptor;
            let width = self.output_width;
            let strip_buf = self
                .strip_buffer
                .get_or_insert_with(|| PixelBuffer::new(width, self.strip_height, desc));

            // Resize if this is the last strip and it's shorter
            if strip_buf.height() != h {
                *strip_buf = PixelBuffer::new(width, h, desc);
            }

            converter
                .convert_strip(self.y_offset as usize, h as usize, strip_buf)
                .at()?;

            let y = self.y_offset;
            self.y_offset += h;
            let slice = self.strip_buffer.as_ref().unwrap().as_slice().erase();
            let slice = match &self.strip_color_context {
                Some(ctx) => slice.with_color_context(Arc::clone(ctx)),
                None => slice,
            };
            return Ok(Some((y, slice)));
        }

        Ok(None)
    }
}

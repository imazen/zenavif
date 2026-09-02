//! Row-streaming decode output.
//!
//! [`ManagedAvifDecoder::decode_to_sink`] is the streaming counterpart to
//! `decode_full`: single images go through a
//! [`StripConverter`](crate::strip_convert::StripConverter), grid images are
//! stitched one tile-row at a time. Both write into buffers the sink hands
//! out, so nothing ever holds a full RGB image.

use super::ManagedAvifDecoder;
use super::cicp_map::{convert_chroma_sampling, convert_color_range, to_our_yuv_range};
use crate::error::{Error, Result};
use crate::image::{ChromaSampling, ColorRange, ImageInfo};
use enough::Stop;
use whereat::at;
use zenpixels::{PixelBuffer, PixelDescriptor};

impl ManagedAvifDecoder {
    /// Decode frames and return a StripConverter for cache-optimal streaming.
    ///
    /// For 8-bit color images, the decoded YUV frames are held in memory and
    /// converted strip-by-strip on demand. For 16-bit or monochrome, falls back
    /// to full-frame conversion (same allocation as `decode_full`).
    ///
    /// Returns `(StripConverter, ImageInfo)`.
    // WIP: will be wired up as the streaming decode entry point
    #[allow(dead_code)]
    pub(crate) fn decode_to_strip_converter(
        &mut self,
        stop: &(impl Stop + ?Sized),
    ) -> Result<(crate::strip_convert::StripConverter, ImageInfo)> {
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let primary_data = self
            .parser
            .primary_data()
            .map_err(|e| e.map_error(Error::Parse))?;
        let primary_frame = Self::decode_frame(
            &mut self.decoder,
            &primary_data,
            "Failed to decode primary frame",
            stop,
        )?;

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let alpha_frame = if let Some(alpha_result) = self.parser.alpha_data() {
            let alpha_data = alpha_result.map_err(|e| e.map_error(Error::Parse))?;
            Some(Self::decode_frame(
                &mut self.decoder,
                &alpha_data,
                "Failed to decode alpha frame",
                stop,
            )?)
        } else {
            None
        };

        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

        let info = self.build_image_info(&primary_frame, alpha_frame.is_some())?;

        let bit_depth = primary_frame.bit_depth();
        let layout = primary_frame.pixel_layout();
        let chroma_sampling = convert_chroma_sampling(layout);
        let buffer_width = primary_frame.width() as usize;
        let buffer_height = primary_frame.height() as usize;
        let display_width = info.width as usize;
        let display_height = info.height as usize;

        let resolved = self.resolved_matrix_for(&info)?;
        let can_strip = bit_depth == 8
            && !matches!(chroma_sampling, ChromaSampling::Monochrome)
            && buffer_width == display_width
            && buffer_height == display_height
            // Identity (GBR reorder, no matrix) and SMPTE-240M (no
            // in-house table) take the full-conversion path below.
            && resolved.to_our().is_some();

        let mut strip = None;
        let mut fallback_frames = Some((primary_frame, alpha_frame));
        if can_strip {
            let (primary_frame, alpha_frame) = fallback_frames
                .take()
                .expect("frames present before try_new");
            let alpha_range = alpha_frame
                .as_ref()
                .map(|f| convert_color_range(f.color_info().color_range))
                .unwrap_or(ColorRange::Full);

            // Describe the strips with the container's CICP. These two
            // constants are the *only* place a strip descriptor is minted for
            // the 8-bit path, so stamping here is what makes the streaming and
            // row-sink outputs colour-describe their pixels the same way the
            // buffered decode does (zenavif#37) — the adapter above is too
            // late, it only sees whatever this announced.
            let descriptor = crate::convert::descriptor_with_cicp(
                if alpha_frame.is_some() {
                    PixelDescriptor::RGBA8_SRGB
                } else {
                    PixelDescriptor::RGB8_SRGB
                },
                &info,
            );

            match crate::strip_convert::StripConverter::try_new(
                primary_frame,
                alpha_frame,
                chroma_sampling,
                to_our_yuv_range(info.color_range),
                resolved
                    .to_our()
                    .expect("can_strip guarantees an in-house matrix"),
                alpha_range,
                self.parser.premultiplied_alpha(),
                display_width,
                display_height,
                buffer_width,
                buffer_height,
                descriptor,
            ) {
                Ok(converter) => strip = Some(converter),
                // `can_strip` and `try_new` disagreed on strip support
                // (defense in depth, zenavif#18): take the full-conversion
                // fallback instead of aborting.
                Err(frames) => fallback_frames = Some(frames),
            }
        }
        let converter = match (strip, fallback_frames) {
            (Some(converter), _) => converter,
            (None, Some((primary_frame, alpha_frame))) => {
                // Fallback: full conversion for 16-bit, monochrome, or
                // cropped images. This arm mints its descriptor separately
                // from the 8-bit one above, so it needs the same CICP stamp —
                // it is the arm every 10/12-bit HDR file takes, i.e. exactly
                // the one zenavif#37 was reported against.
                let (pixels, _) = self.convert_to_image(primary_frame, alpha_frame, stop)?;
                let desc = crate::convert::descriptor_with_cicp(pixels.descriptor(), &info);
                crate::strip_convert::StripConverter::new_from_pixels(pixels.with_descriptor(desc))
            }
            (None, None) => unreachable!("frames either converted or handed back"),
        };

        Ok((converter, info))
    }

    /// Decode with row-level streaming to a sink.
    ///
    /// For grid images, processes one tile-row at a time: decode tiles,
    /// convert to RGB, stitch into the sink buffer, drop frames.
    ///
    /// For single 8-bit color images, the decoded YUV frame is converted
    /// strip-by-strip directly into the sink's buffers. This eliminates the
    /// full RGB allocation and keeps the working set in L2 cache.
    ///
    /// For 16-bit/monochrome images, falls back to full-frame conversion.
    pub fn decode_to_sink(
        &mut self,
        stop: &(impl Stop + ?Sized),
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<ImageInfo> {
        stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
        #[cfg(feature = "zenav1-aom")]
        if self.decode_backend == crate::DecodeBackend::Zenav1Aom {
            return Err(at!(Error::Unsupported(
                "DecodeBackend::Zenav1Aom does not stream to a row sink yet; use \
                 Rav1dSafe or the whole-image decode entry points"
            )));
        }

        if self.parser.grid_config().is_some() {
            return self.decode_grid_to_sink(stop, sink);
        }

        // Single image: strip conversion, then copy rows to sink
        let (converter, info) = self.decode_to_strip_converter(stop)?;
        let width = converter.display_width() as u32;
        let height = converter.display_height() as u32;
        let desc = converter.descriptor();
        let strip_h = converter.optimal_strip_height();
        let bpp = desc.bytes_per_pixel();

        sink.begin(width, height, desc)
            .map_err(|e| at!(Error::Io(e.to_string())))?;

        // Reusable strip buffer for conversion
        let mut strip_pixels = PixelBuffer::new(width, strip_h as u32, desc);

        let mut y_offset = 0usize;
        while y_offset < height as usize {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

            let h = strip_h.min(height as usize - y_offset);

            // Resize strip buffer for the last (possibly shorter) strip
            if h < strip_h {
                strip_pixels = PixelBuffer::new(width, h as u32, desc);
            }

            converter.convert_strip(y_offset, h, &mut strip_pixels)?;

            // Copy converted rows to sink buffer
            let mut sink_buf = sink
                .provide_next_buffer(y_offset as u32, h as u32, width, desc)
                .map_err(|e| at!(Error::Io(e.to_string())))?;

            let src = strip_pixels.as_slice();
            let row_bytes = width as usize * bpp;
            for row in 0..h {
                let dst_row = sink_buf.row_mut(row as u32);
                let src_row = src.row(row as u32);
                dst_row[..row_bytes].copy_from_slice(&src_row[..row_bytes]);
            }

            y_offset += h;
        }

        sink.finish().map_err(|e| at!(Error::Io(e.to_string())))?;

        Ok(info)
    }

    /// Stream a grid image tile-row by tile-row to a sink.
    fn decode_grid_to_sink(
        &mut self,
        stop: &(impl Stop + ?Sized),
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<ImageInfo> {
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

        let grid_info = self.probe_info()?;
        let grid_rows = grid_config.rows as usize;
        let cols = grid_config.columns as usize;
        let output_width = grid_config.output_width as usize;
        let output_height = grid_config.output_height as usize;

        let mut y_offset = 0u32;
        let mut began = false;
        // HEIF/MIAF grids require uniform input-image sizes; the first decoded
        // tile pins the expected dims and every later tile must match, so a
        // crafted grid can't silently misalign the strip stitch below.
        let mut expected_tile: Option<(u32, u32)> = None;

        for grid_row in 0..grid_rows {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;

            // Decode and convert tiles for this row one at a time.
            // Each tile is decoded then converted before the next, so at most
            // one raw Frame + one converted PixelBuffer per tile is live.
            let mut row_tiles: Vec<PixelBuffer> = Vec::with_capacity(cols);
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
                match expected_tile {
                    None => expected_tile = Some((pixels.width(), pixels.height())),
                    Some(dims) => {
                        if (pixels.width(), pixels.height()) != dims {
                            return Err(at!(Error::Malformed(
                                "grid tiles decoded to non-uniform dimensions"
                            )));
                        }
                    }
                }
                row_tiles.push(pixels);
            }

            // Get descriptor and tile height from the first tile, described
            // with the container's CICP (zenavif#37) — the streaming grid path
            // does the same to its stitched strips.
            let desc = crate::convert::descriptor_with_cicp(row_tiles[0].descriptor(), &grid_info);
            let bpp = desc.bytes_per_pixel();
            let tile_h = row_tiles[0].height() as usize;

            // Last tile-row may be clipped to output dimensions
            let strip_h = tile_h.min(output_height.saturating_sub(y_offset as usize));
            if strip_h == 0 {
                break;
            }

            // Signal begin on the first strip
            if !began {
                sink.begin(output_width as u32, output_height as u32, desc)
                    .map_err(|e| at!(Error::Io(e.to_string())))?;
                began = true;
            }

            // Provide buffer from sink and stitch tiles into it
            let mut sink_buf = sink
                .provide_next_buffer(y_offset, strip_h as u32, output_width as u32, desc)
                .map_err(|e| at!(Error::Io(e.to_string())))?;
            for py in 0..strip_h {
                let dst_row = sink_buf.row_mut(py as u32);
                let mut x_offset = 0usize;
                for tile in &row_tiles {
                    let tile_w = tile.width() as usize;
                    let actual_w = tile_w.min(output_width.saturating_sub(x_offset));
                    // Guard the source row by each tile's own height: tiles in a
                    // grid row may decode to different heights and `strip_h` comes
                    // from the first tile, so a shorter tile would make
                    // `tile.row(py)` panic. Off-canvas (actual_w == 0) or too-short
                    // tiles contribute nothing; still advance x_offset.
                    if actual_w != 0 && (py as u32) < tile.height() {
                        let tile_slice = tile.as_slice();
                        let src = tile_slice.row(py as u32);
                        let copy_bytes = actual_w * bpp;
                        let dst_start = x_offset * bpp;
                        dst_row[dst_start..dst_start + copy_bytes]
                            .copy_from_slice(&src[..copy_bytes]);
                    }
                    x_offset += tile_w;
                }
            }

            y_offset += strip_h as u32;
        }

        if began {
            sink.finish().map_err(|e| at!(Error::Io(e.to_string())))?;
        }

        self.probe_info()
    }
}

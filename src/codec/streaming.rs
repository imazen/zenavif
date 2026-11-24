//! [`AvifStreamingDecoder`] — strip emission for the three streaming shapes
//! (grid tile-row stitching, on-demand YUV strip conversion, and pre-baked
//! full-buffer strips) behind [`zencodec::decode::StreamingDecode`].
//!
//! Not re-exported from `crate`: [`AvifStreamingDecoder`] reaches callers
//! only as `<AvifDecodeJob as DecodeJob>::StreamDec`.

use whereat::{At, ResultAtExt as _, at};
use zencodec::{CodecError, ImageInfo};
use zenpixels::{ImageSpec, PixelBuf, PixelView};

use crate::error::Error;

/// Why the two `expect`s below cannot fire.
///
/// `strip_buffer` is populated by `get_or_insert_with` immediately above each
/// use, and `decoder` is checked with `is_some()` immediately above its use.
/// Both are re-looked-up rather than held because the first borrow must end
/// before `self.y_offset` / `self.stitch_tiles` can be touched — the
/// re-lookup is a borrow-checker accommodation, not a state check, so there
/// is no reachable `None` to turn into an error.
const STRIP_BUFFER_JUST_SET: &str =
    "strip_buffer was populated by get_or_insert_with immediately above";
const GRID_DECODER_JUST_CHECKED: &str =
    "grid decoder presence was checked by is_some() immediately above";

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
    /// The job's control, polled to cancel grid decoding.
    pub(super) pulse: zencodec::SharedPulse,
    pub(super) grid_rows: u32,
    pub(super) grid_cols: u32,
    pub(super) current_grid_row: u32,
    /// Pixel descriptor with CICP metadata for strip buffers.
    pub(super) strip_descriptor: ImageSpec,
    /// Reusable strip buffer for the current tile-row or strip conversion.
    pub(super) strip_buffer: Option<PixelBuf>,
    /// Non-grid strip conversion: holds decoded YUV frames, converts on demand.
    pub(super) strip_converter: Option<crate::strip_convert::StripConverter>,
    /// Optimal strip height for the strip converter path.
    pub(super) strip_height: u32,
    /// Descriptive HDR metadata shared by the borrowed strips.
    pub(super) strip_hdr: zenpixels::HdrMetadata,
    pub(super) strip_origin: zenpixels::ColorOrigin,
    /// Bake path (`OrientationHint::bakes()`): the fully-decoded, orientation-
    /// baked buffer. Orientation is not strip-local (transposes need the whole
    /// image), so the bake path materializes upright once and emits it in
    /// fixed-height strips. `None` on the preserve path (the default), where the
    /// grid / strip-converter fields drive low-memory streaming unchanged.
    pub(super) baked: Option<PixelBuf>,
    /// The reduction `preferred` asked for, when it differs from
    /// `strip_descriptor` (zenavif#36). `None` means the native strip is what
    /// the caller wanted (or nothing they asked for could be produced), and
    /// strips are emitted with no conversion at all — the pre-#36 behaviour,
    /// unchanged.
    ///
    /// Chosen once at construction by
    /// [`negotiate_strip_descriptor`](super::negotiate::negotiate_strip_descriptor)
    /// so that the format announced to the caller and the format every strip
    /// is actually written in cannot drift apart.
    pub(super) output_descriptor: Option<ImageSpec>,
    /// Destination for the converted strip when `output_descriptor` is set.
    /// Separate from `strip_buffer` because the converter still writes the
    /// native format into that one.
    pub(super) output_buffer: Option<PixelBuf>,
}

impl AvifStreamingDecoder {
    /// Stitch decoded tiles horizontally into `self.strip_buffer`.
    fn stitch_tiles(&mut self, tiles: &[PixelBuf], strip_h: u32) -> crate::error::Result<()> {
        let bpp = self.strip_descriptor.layout.bytes_per_pixel();
        let mut strip = PixelBuf::new(self.output_width, strip_h, self.strip_descriptor.clone())
            .map_err(|_| at!(Error::OutOfMemory))?;
        {
            let mut sm = strip.as_view_mut();
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
                        let tile_slice = tile.as_view();
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
        Ok(())
    }

    /// Hand out the strip sitting in `strip_buffer`, applying the negotiated
    /// reduction and re-attaching the colour context.
    ///
    /// All three strip sources (bake, grid tile-row, on-demand YUV) funnel
    /// through here so a format decision cannot be applied to two of them and
    /// forgotten on the third — which is exactly the shape of zenavif#35/#36.
    fn emit(&mut self, y: u32) -> Result<Option<(u32, PixelView<'_>)>, At<Error>> {
        if let Some(target) = &self.output_descriptor {
            let native = self.strip_buffer.take().expect(STRIP_BUFFER_JUST_SET);
            // Keep the native scratch for the next strip; only its contents
            // are consumed by the conversion.
            let dims = (native.width(), native.height());
            let native_desc = native.spec().clone();
            let converted = super::negotiate::apply_strip_reduction(native, target.clone())
                .ok_or_else(|| {
                    at!(Error::Decode {
                        code: -1,
                        msg: "negotiated strip conversion failed after its plan was probed",
                    })
                })?;
            self.strip_buffer = Some(
                PixelBuf::new(dims.0, dims.1, native_desc).map_err(|_| at!(Error::OutOfMemory))?,
            );
            self.output_buffer = Some(converted);
            let slice = self
                .output_buffer
                .as_ref()
                .expect("output_buffer was just set")
                .as_view()
                .with_origin(&self.strip_origin)
                .with_hdr(self.strip_hdr);
            return Ok(Some((y, slice)));
        }
        let slice = self
            .strip_buffer
            .as_ref()
            .expect(STRIP_BUFFER_JUST_SET)
            .as_view()
            .with_origin(&self.strip_origin)
            .with_hdr(self.strip_hdr);
        Ok(Some((y, slice)))
    }
}

impl zencodec::decode::StreamingDecode for AvifStreamingDecoder {
    type Error = At<CodecError>;

    fn next_batch(&mut self) -> Result<Option<(u32, PixelView<'_>)>, At<CodecError>> {
        self.next_batch_inner().map_err(zencodec::CodecError::of)
    }

    fn info(&self) -> &ImageInfo {
        &self.info
    }
}

impl AvifStreamingDecoder {
    fn next_batch_inner(&mut self) -> Result<Option<(u32, PixelView<'_>)>, At<Error>> {
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
            let width = self.output_width;
            if self.strip_buffer.as_ref().is_none_or(|b| b.height() != h) {
                self.strip_buffer = Some(
                    PixelBuf::new(width, h, self.strip_descriptor.clone())
                        .map_err(|_| at!(Error::OutOfMemory))?,
                );
            }
            let strip_buf = self.strip_buffer.as_mut().expect(STRIP_BUFFER_JUST_SET);
            {
                let baked_slice = baked.as_view();
                let mut sm = strip_buf.as_view_mut();
                for row in 0..h {
                    sm.row_mut(row)
                        .copy_from_slice(baked_slice.row(self.y_offset + row));
                }
            }
            let y = self.y_offset;
            self.y_offset += h;
            return self.emit(y);
        }

        if self.decoder.is_some() {
            // Grid path: decode one tile-row per call.
            if self.current_grid_row >= self.grid_rows {
                return Ok(None);
            }

            let tiles = self
                .decoder
                .as_mut()
                .expect(GRID_DECODER_JUST_CHECKED)
                .decode_tile_row(
                    self.current_grid_row as usize,
                    self.grid_cols as usize,
                    &self.pulse,
                )?;

            if tiles.is_empty() {
                return Ok(None);
            }

            let tile_h = tiles[0].height();
            let strip_h = tile_h.min(self.output_height.saturating_sub(self.y_offset));
            if strip_h == 0 {
                return Ok(None);
            }

            self.stitch_tiles(&tiles, strip_h)?;
            self.current_grid_row += 1;

            let y = self.y_offset;
            self.y_offset += strip_h;
            return self.emit(y);
        }

        // Non-grid: convert strip from decoded YUV frames on demand.
        if let Some(ref mut converter) = self.strip_converter {
            let remaining = self.output_height - self.y_offset;
            let h = self.strip_height.min(remaining);
            if h == 0 {
                return Ok(None);
            }

            // Ensure strip buffer exists with the right dimensions
            let width = self.output_width;
            if self.strip_buffer.as_ref().is_none_or(|b| b.height() != h) {
                self.strip_buffer = Some(
                    PixelBuf::new(width, h, self.strip_descriptor.clone())
                        .map_err(|_| at!(Error::OutOfMemory))?,
                );
            }
            let strip_buf = self.strip_buffer.as_mut().expect(STRIP_BUFFER_JUST_SET);

            converter
                .convert_strip(self.y_offset as usize, h as usize, strip_buf)
                .at()?;

            let y = self.y_offset;
            self.y_offset += h;
            return self.emit(y);
        }

        Ok(None)
    }
}

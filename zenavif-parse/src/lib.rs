#![deny(unsafe_code)]
#![allow(clippy::missing_safety_doc)]
//! Module for parsing ISO Base Media Format aka video/mp4 streams.
//!
//! This crate is written entirely in safe Rust code except for the C FFI bindings.

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use arrayvec::ArrayVec;
use log::{debug, warn};

use bitreader::BitReader;
use byteorder::ReadBytesExt;
use fallible_collections::{TryClone, TryReserveError};
use std::borrow::Cow;
use std::convert::{TryFrom, TryInto as _};

use std::io::{Read, Take};
use std::num::NonZeroU32;
use std::ops::{Range, RangeFrom};

mod obu;

mod boxes;
use crate::boxes::{BoxType, FourCC};

/// This crate can be used from C.
pub mod c_api;

pub use enough::{Stop, StopReason, Unstoppable};

// Arbitrary buffer size limit used for raw read_bufs on a box.
// const BUF_SIZE_LIMIT: u64 = 10 * 1024 * 1024;

/// A trait to indicate a type can be infallibly converted to `u64`.
/// This should only be implemented for infallible conversions, so only unsigned types are valid.
trait ToU64 {
    fn to_u64(self) -> u64;
}

/// Statically verify that the platform `usize` can fit within a `u64`.
/// If the size won't fit on the given platform, this will fail at compile time, but if a type
/// which can fail `TryInto<usize>` is used, it may panic.
impl ToU64 for usize {
    fn to_u64(self) -> u64 {
        const _: () = assert!(std::mem::size_of::<usize>() <= std::mem::size_of::<u64>());
        self.try_into().ok().unwrap()
    }
}

/// A trait to indicate a type can be infallibly converted to `usize`.
/// This should only be implemented for infallible conversions, so only unsigned types are valid.
pub(crate) trait ToUsize {
    fn to_usize(self) -> usize;
}

/// Statically verify that the given type can fit within a `usize`.
/// If the size won't fit on the given platform, this will fail at compile time, but if a type
/// which can fail `TryInto<usize>` is used, it may panic.
macro_rules! impl_to_usize_from {
    ( $from_type:ty ) => {
        impl ToUsize for $from_type {
            fn to_usize(self) -> usize {
                const _: () = assert!(std::mem::size_of::<$from_type>() <= std::mem::size_of::<usize>());
                self.try_into().ok().unwrap()
            }
        }
    };
}

impl_to_usize_from!(u8);
impl_to_usize_from!(u16);
impl_to_usize_from!(u32);

/// Indicate the current offset (i.e., bytes already read) in a reader
trait Offset {
    fn offset(&self) -> u64;
}

/// Wraps a reader to track the current offset
struct OffsetReader<'a, T> {
    reader: &'a mut T,
    offset: u64,
}

impl<'a, T> OffsetReader<'a, T> {
    fn new(reader: &'a mut T) -> Self {
        Self { reader, offset: 0 }
    }
}

impl<T> Offset for OffsetReader<'_, T> {
    fn offset(&self) -> u64 {
        self.offset
    }
}

impl<T: Read> Read for OffsetReader<'_, T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes_read = self.reader.read(buf)?;
        self.offset = self
            .offset
            .checked_add(bytes_read.to_u64())
            .ok_or(Error::Unsupported("total bytes read too large for offset type"))?;
        Ok(bytes_read)
    }
}

#[doc(hidden)]
pub type TryVec<T> = fallible_collections::TryVec<T>;
type TryString = fallible_collections::TryVec<u8>;

// To ensure we don't use stdlib allocating types by accident
#[allow(dead_code)]
struct Vec;
#[allow(dead_code)]
struct Box;
#[allow(dead_code)]
struct HashMap;
#[allow(dead_code)]
struct String;

/// Describes parser failures.
///
/// This enum wraps the standard `io::Error` type, unified with
/// our own parser error states and those of crates we use.
#[derive(Debug)]
pub enum Error {
    /// Parse error caused by corrupt or malformed data.
    InvalidData(&'static str),
    /// Parse error caused by limited parser support rather than invalid data.
    Unsupported(&'static str),
    /// Reflect `std::io::ErrorKind::UnexpectedEof` for short data.
    UnexpectedEOF,
    /// Propagate underlying errors from `std::io`.
    Io(std::io::Error),
    /// `read_mp4` terminated without detecting a moov box.
    NoMoov,
    /// Out of memory
    OutOfMemory,
    /// Resource limit exceeded during parsing
    ResourceLimitExceeded(&'static str),
    /// Operation was stopped/cancelled
    Stopped(enough::StopReason),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::InvalidData(s) | Self::Unsupported(s) | Self::ResourceLimitExceeded(s) => s,
            Self::UnexpectedEOF => "EOF",
            Self::Io(err) => return err.fmt(f),
            Self::NoMoov => "Missing Moov box",
            Self::OutOfMemory => "OOM",
            Self::Stopped(reason) => return write!(f, "Stopped: {}", reason),
        };
        f.write_str(msg)
    }
}

impl std::error::Error for Error {}

impl From<bitreader::BitReaderError> for Error {
    #[cold]
    #[cfg_attr(debug_assertions, track_caller)]
    fn from(err: bitreader::BitReaderError) -> Self {
        log::warn!("bitreader: {err}");
        debug_assert!(!matches!(err, bitreader::BitReaderError::TooManyBitsForType { .. })); // bug
        Self::InvalidData("truncated bits")
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::UnexpectedEof => Self::UnexpectedEOF,
            _ => Self::Io(err),
        }
    }
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(_: std::string::FromUtf8Error) -> Self {
        Self::InvalidData("invalid utf8")
    }
}

impl From<std::num::TryFromIntError> for Error {
    fn from(_: std::num::TryFromIntError) -> Self {
        Self::Unsupported("integer conversion failed")
    }
}

impl From<Error> for std::io::Error {
    fn from(err: Error) -> Self {
        let kind = match err {
            Error::InvalidData(_) => std::io::ErrorKind::InvalidData,
            Error::UnexpectedEOF => std::io::ErrorKind::UnexpectedEof,
            Error::Io(io_err) => return io_err,
            _ => std::io::ErrorKind::Other,
        };
        Self::new(kind, err)
    }
}

impl From<TryReserveError> for Error {
    fn from(_: TryReserveError) -> Self {
        Self::OutOfMemory
    }
}

impl From<enough::StopReason> for Error {
    fn from(reason: enough::StopReason) -> Self {
        Self::Stopped(reason)
    }
}

/// Result shorthand using our Error enum.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Basic ISO box structure.
///
/// mp4 files are a sequence of possibly-nested 'box' structures.  Each box
/// begins with a header describing the length of the box's data and a
/// four-byte box type which identifies the type of the box. Together these
/// are enough to interpret the contents of that section of the file.
///
/// See ISO 14496-12:2015 § 4.2
#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    /// Box type.
    name: BoxType,
    /// Size of the box in bytes.
    size: u64,
    /// Offset to the start of the contained data (or header size).
    offset: u64,
    /// Uuid for extended type.
    #[allow(unused)]
    uuid: Option<[u8; 16]>,
}

impl BoxHeader {
    /// 4-byte size + 4-byte type
    const MIN_SIZE: u64 = 8;
    /// 4-byte size + 4-byte type + 16-byte size
    const MIN_LARGE_SIZE: u64 = 16;
}

/// File type box 'ftyp'.
#[derive(Debug)]
#[allow(unused)]
struct FileTypeBox {
    major_brand: FourCC,
    minor_version: u32,
    compatible_brands: TryVec<FourCC>,
}

// Handler reference box 'hdlr'
#[derive(Debug)]
#[allow(unused)]
struct HandlerBox {
    handler_type: FourCC,
}

#[derive(Debug)]
#[allow(unused)]
pub(crate) struct AV1ConfigBox {
    pub(crate) profile: u8,
    pub(crate) level: u8,
    pub(crate) tier: u8,
    pub(crate) bit_depth: u8,
    pub(crate) monochrome: bool,
    pub(crate) chroma_subsampling_x: u8,
    pub(crate) chroma_subsampling_y: u8,
    pub(crate) chroma_sample_position: u8,
    pub(crate) initial_presentation_delay_present: bool,
    pub(crate) initial_presentation_delay_minus_one: u8,
    pub(crate) config_obus: TryVec<u8>,
}

/// Options for parsing AVIF files
///
/// Prefer using [`DecodeConfig::lenient()`] with [`AvifParser`] instead.
#[derive(Debug, Clone, Copy)]
#[derive(Default)]
pub struct ParseOptions {
    /// Enable lenient parsing mode
    ///
    /// When true, non-critical validation errors (like non-zero flags in boxes
    /// that expect zero flags) will be ignored instead of returning errors.
    /// This allows parsing of slightly malformed but otherwise valid AVIF files.
    ///
    /// Default: false (strict validation)
    pub lenient: bool,
}

/// Configuration for parsing AVIF files with resource limits and validation options
///
/// Provides fine-grained control over resource consumption during AVIF parsing,
/// allowing defensive parsing against malicious or malformed files.
///
/// Resource limits are checked **before** allocations occur, preventing out-of-memory
/// conditions from malicious files that claim unrealistic dimensions or counts.
///
/// # Examples
///
/// ```rust
/// use avif_parse::DecodeConfig;
///
/// // Default limits (suitable for most apps)
/// let config = DecodeConfig::default();
///
/// // Strict limits for untrusted input
/// let config = DecodeConfig::default()
///     .with_peak_memory_limit(100_000_000)  // 100MB
///     .with_total_megapixels_limit(64)       // 64MP max
///     .with_max_animation_frames(100);       // 100 frames
///
/// // No limits (backwards compatible with read_avif)
/// let config = DecodeConfig::unlimited();
/// ```
#[derive(Debug, Clone)]
pub struct DecodeConfig {
    /// Maximum peak heap memory usage in bytes.
    /// Default: 1GB (1,000,000,000 bytes)
    pub peak_memory_limit: Option<u64>,

    /// Maximum total megapixels for grid images.
    /// Default: 512 megapixels
    pub total_megapixels_limit: Option<u32>,

    /// Maximum number of animation frames.
    /// Default: 10,000 frames
    pub max_animation_frames: Option<u32>,

    /// Maximum number of grid tiles.
    /// Default: 1,000 tiles
    pub max_grid_tiles: Option<u32>,

    /// Enable lenient parsing mode.
    /// Default: false (strict validation)
    pub lenient: bool,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            peak_memory_limit: Some(1_000_000_000),
            total_megapixels_limit: Some(512),
            max_animation_frames: Some(10_000),
            max_grid_tiles: Some(1_000),
            lenient: false,
        }
    }
}

impl DecodeConfig {
    /// Create a configuration with no resource limits.
    ///
    /// Equivalent to the behavior of `read_avif()` before resource limits were added.
    pub fn unlimited() -> Self {
        Self {
            peak_memory_limit: None,
            total_megapixels_limit: None,
            max_animation_frames: None,
            max_grid_tiles: None,
            lenient: false,
        }
    }

    /// Set the peak memory limit in bytes
    pub fn with_peak_memory_limit(mut self, bytes: u64) -> Self {
        self.peak_memory_limit = Some(bytes);
        self
    }

    /// Set the total megapixels limit for grid images
    pub fn with_total_megapixels_limit(mut self, megapixels: u32) -> Self {
        self.total_megapixels_limit = Some(megapixels);
        self
    }

    /// Set the maximum animation frame count
    pub fn with_max_animation_frames(mut self, frames: u32) -> Self {
        self.max_animation_frames = Some(frames);
        self
    }

    /// Set the maximum grid tile count
    pub fn with_max_grid_tiles(mut self, tiles: u32) -> Self {
        self.max_grid_tiles = Some(tiles);
        self
    }

    /// Enable lenient parsing mode
    pub fn lenient(mut self, lenient: bool) -> Self {
        self.lenient = lenient;
        self
    }
}

/// Grid configuration for tiled/grid-based AVIF images
#[derive(Debug, Clone, PartialEq)]
/// Grid image configuration
///
/// For tiled/grid AVIF images, this describes the grid layout.
/// Grid images are composed of multiple AV1 image items (tiles) arranged in a rectangular grid.
///
/// ## Grid Layout Determination
///
/// Grid layout can be specified in two ways:
/// 1. **Explicit ImageGrid property box** - contains rows, columns, and output dimensions
/// 2. **Calculated from ispe properties** - when no ImageGrid box exists, dimensions are
///    calculated by dividing the grid item's dimensions by a tile's dimensions
///
/// ## Output Dimensions
///
/// - `output_width` and `output_height` may be 0, indicating the decoder should calculate
///   them from the tile dimensions
/// - When non-zero, they specify the exact output dimensions of the composed image
pub struct GridConfig {
    /// Number of tile rows (1-256)
    pub rows: u8,
    /// Number of tile columns (1-256)
    pub columns: u8,
    /// Output width in pixels (0 = calculate from tiles)
    pub output_width: u32,
    /// Output height in pixels (0 = calculate from tiles)
    pub output_height: u32,
}

/// Frame information for animated AVIF
#[deprecated(since = "1.5.0", note = "Use `AvifParser::frame()` which returns `FrameRef` instead")]
#[derive(Debug)]
pub struct AnimationFrame {
    /// AV1 bitstream data for this frame
    pub data: TryVec<u8>,
    /// Duration in milliseconds (0 if unknown)
    pub duration_ms: u32,
}

/// Animation configuration for animated AVIF (avis brand)
#[deprecated(since = "1.5.0", note = "Use `AvifParser::animation_info()` and `AvifParser::frames()` instead")]
#[derive(Debug)]
#[allow(deprecated)]
pub struct AnimationConfig {
    /// Number of times to loop (0 = infinite)
    pub loop_count: u32,
    /// All frames in the animation
    pub frames: TryVec<AnimationFrame>,
}

// Internal structures for animation parsing

#[derive(Debug)]
struct MovieHeader {
    _timescale: u32,
    _duration: u64,
}

#[derive(Debug)]
struct MediaHeader {
    timescale: u32,
    _duration: u64,
}

#[derive(Debug)]
struct TimeToSampleEntry {
    sample_count: u32,
    sample_delta: u32,
}

#[derive(Debug)]
struct SampleToChunkEntry {
    first_chunk: u32,
    samples_per_chunk: u32,
    _sample_description_index: u32,
}

#[derive(Debug)]
struct SampleTable {
    time_to_sample: TryVec<TimeToSampleEntry>,
    sample_to_chunk: TryVec<SampleToChunkEntry>,
    sample_sizes: TryVec<u32>,
    chunk_offsets: TryVec<u64>,
}

#[deprecated(since = "1.5.0", note = "Use `AvifParser` for zero-copy parsing instead")]
#[derive(Debug, Default)]
#[allow(deprecated)]
pub struct AvifData {
    /// AV1 data for the color channels.
    ///
    /// The collected data indicated by the `pitm` box, See ISO 14496-12:2015 § 8.11.4
    pub primary_item: TryVec<u8>,
    /// AV1 data for alpha channel.
    ///
    /// Associated alpha channel for the primary item, if any
    pub alpha_item: Option<TryVec<u8>>,
    /// If true, divide RGB values by the alpha value.
    ///
    /// See `prem` in MIAF § 7.3.5.2
    pub premultiplied_alpha: bool,

    /// Grid configuration for tiled images.
    ///
    /// If present, the image is a grid and `grid_tiles` contains the tile data.
    /// Grid layout is determined either from an explicit ImageGrid property box or
    /// calculated from ispe (Image Spatial Extents) properties.
    ///
    /// ## Example
    ///
    /// ```no_run
    /// #[allow(deprecated)]
    /// use std::fs::File;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// #[allow(deprecated)]
    /// let data = avif_parse::read_avif(&mut File::open("image.avif")?)?;
    ///
    /// if let Some(grid) = data.grid_config {
    ///     println!("Grid: {}×{} tiles", grid.rows, grid.columns);
    ///     println!("Output: {}×{}", grid.output_width, grid.output_height);
    ///     println!("Tile count: {}", data.grid_tiles.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub grid_config: Option<GridConfig>,

    /// AV1 payloads for grid image tiles.
    ///
    /// Empty for non-grid images. For grid images, contains one entry per tile.
    ///
    /// **Tile ordering:** Tiles are guaranteed to be in the correct order for grid assembly,
    /// sorted by their dimgIdx (reference index). This is row-major order: tiles in the first
    /// row from left to right, then the second row, etc.
    pub grid_tiles: TryVec<TryVec<u8>>,

    /// Animation configuration (for animated AVIF with avis brand)
    ///
    /// When present, primary_item contains the first frame
    pub animation: Option<AnimationConfig>,
}

// # Memory Usage
//
// This implementation loads all image data into owned vectors (`TryVec<u8>`), which has
// memory implications depending on the file type:
//
// - **Static images**: Single copy of compressed data (~5-50KB typical)
//   - `primary_item`: compressed AV1 data
//   - `alpha_item`: compressed alpha data (if present)
//
// - **Grid images**: All tiles loaded (~100KB-2MB for large grids)
//   - `grid_tiles`: one compressed tile per grid cell
//
// - **Animated images**: All frames loaded eagerly (⚠️ HIGH MEMORY)
//   - Internal mdat boxes: ~500KB for 95-frame video
//   - Extracted frames: ~500KB duplicated in `animation.frames[].data`
//   - **Total: ~2× file size in memory**
//
// For large animated files, consider using a streaming approach or processing frames
// individually rather than loading the entire `AvifData` structure.

#[allow(deprecated)]
impl AvifData {
    #[deprecated(since = "1.5.0", note = "Use `AvifParser::from_reader()` instead")]
    pub fn from_reader<R: Read>(reader: &mut R) -> Result<Self> {
        read_avif(reader)
    }

    /// Parses AV1 data to get basic properties of the opaque channel
    pub fn primary_item_metadata(&self) -> Result<AV1Metadata> {
        AV1Metadata::parse_av1_bitstream(&self.primary_item)
    }

    /// Parses AV1 data to get basic properties about the alpha channel, if any
    pub fn alpha_item_metadata(&self) -> Result<Option<AV1Metadata>> {
        self.alpha_item.as_deref().map(AV1Metadata::parse_av1_bitstream).transpose()
    }
}

/// See [`AvifData::primary_item_metadata()`]
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct AV1Metadata {
    /// Should be true for non-animated AVIF
    pub still_picture: bool,
    pub max_frame_width: NonZeroU32,
    pub max_frame_height: NonZeroU32,
    /// 8, 10, or 12
    pub bit_depth: u8,
    /// 0, 1 or 2 for the level of complexity
    pub seq_profile: u8,
    /// Horizontal and vertical. `false` is full-res.
    pub chroma_subsampling: (bool, bool),
    pub monochrome: bool,
}

impl AV1Metadata {
    /// Parses raw AV1 bitstream (OBU sequence header) only.
    ///
    /// This is for the bare image payload from an encoder, not an AVIF/HEIF file.
    /// To parse AVIF files, see [`AvifData::from_reader()`].
    #[inline(never)]
    pub fn parse_av1_bitstream(obu_bitstream: &[u8]) -> Result<Self> {
        let h = obu::parse_obu(obu_bitstream)?;
        Ok(Self {
            still_picture: h.still_picture,
            max_frame_width: h.max_frame_width,
            max_frame_height: h.max_frame_height,
            bit_depth: h.color.bit_depth,
            seq_profile: h.seq_profile,
            chroma_subsampling: h.color.chroma_subsampling,
            monochrome: h.color.monochrome,
        })
    }
}

/// A single frame from an animated AVIF, with zero-copy when possible.
///
/// The `data` field is `Cow::Borrowed` when the frame lives in a single
/// contiguous mdat extent, and `Cow::Owned` when extents must be concatenated.
pub struct FrameRef<'a> {
    pub data: Cow<'a, [u8]>,
    pub duration_ms: u32,
}

/// Byte range of a media data box within the file.
struct MdatBounds {
    offset: u64,
    length: u64,
}

/// Where an item's data lives: construction method + extent ranges.
struct ItemExtents {
    construction_method: ConstructionMethod,
    extents: TryVec<ExtentRange>,
}

/// Zero-copy AVIF parser backed by a borrowed or owned byte buffer.
///
/// `AvifParser` records byte offsets during parsing but does **not** copy
/// mdat payload data. Data access methods return `Cow<[u8]>` — borrowed
/// when the item is a single contiguous extent, owned when extents must
/// be concatenated.
///
/// # Constructors
///
/// | Method | Lifetime | Zero-copy? |
/// |--------|----------|------------|
/// | [`from_bytes`](Self::from_bytes) | `'data` | Yes — borrows the slice |
/// | [`from_owned`](Self::from_owned) | `'static` | Within the owned buffer |
/// | [`from_reader`](Self::from_reader) | `'static` | Reads all, then owned |
///
/// # Example
///
/// ```no_run
/// use avif_parse::AvifParser;
///
/// let bytes = std::fs::read("image.avif")?;
/// let parser = AvifParser::from_bytes(&bytes)?;
/// let primary = parser.primary_data()?; // Cow::Borrowed for single-extent
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct AvifParser<'data> {
    raw: Cow<'data, [u8]>,
    mdat_bounds: TryVec<MdatBounds>,
    idat: Option<TryVec<u8>>,
    primary: ItemExtents,
    alpha: Option<ItemExtents>,
    grid_config: Option<GridConfig>,
    tiles: TryVec<ItemExtents>,
    animation_data: Option<AnimationParserData>,
    premultiplied_alpha: bool,
}

struct AnimationParserData {
    media_timescale: u32,
    sample_table: SampleTable,
    loop_count: u32,
}

/// Animation metadata from [`AvifParser`]
#[derive(Debug, Clone, Copy)]
pub struct AnimationInfo {
    pub frame_count: usize,
    pub loop_count: u32,
}

/// Parsed structure from the box-level parse pass (no mdat data).
struct ParsedStructure {
    meta: AvifInternalMeta,
    mdat_bounds: TryVec<MdatBounds>,
    animation_data: Option<(u32, SampleTable, u32)>,
}

impl<'data> AvifParser<'data> {
    // ========================================
    // Constructors
    // ========================================

    /// Parse AVIF from a borrowed byte slice (true zero-copy).
    ///
    /// The returned parser borrows `data` — single-extent items will be
    /// returned as `Cow::Borrowed` slices into this buffer.
    pub fn from_bytes(data: &'data [u8]) -> Result<Self> {
        Self::from_bytes_with_config(data, &DecodeConfig::unlimited(), &Unstoppable)
    }

    /// Parse AVIF from a borrowed byte slice with resource limits.
    pub fn from_bytes_with_config(
        data: &'data [u8],
        config: &DecodeConfig,
        stop: &dyn Stop,
    ) -> Result<Self> {
        let parsed = Self::parse_raw(data, config, stop)?;
        Self::build(Cow::Borrowed(data), parsed, config)
    }

    /// Parse AVIF from an owned buffer.
    ///
    /// The returned parser owns the data — single-extent items will still
    /// be returned as `Cow::Borrowed` slices (borrowing from the internal buffer).
    pub fn from_owned(data: std::vec::Vec<u8>) -> Result<AvifParser<'static>> {
        AvifParser::from_owned_with_config(data, &DecodeConfig::unlimited(), &Unstoppable)
    }

    /// Parse AVIF from an owned buffer with resource limits.
    pub fn from_owned_with_config(
        data: std::vec::Vec<u8>,
        config: &DecodeConfig,
        stop: &dyn Stop,
    ) -> Result<AvifParser<'static>> {
        let parsed = AvifParser::parse_raw(&data, config, stop)?;
        AvifParser::build(Cow::Owned(data), parsed, config)
    }

    /// Parse AVIF from a reader (reads all bytes, then parses).
    pub fn from_reader<R: Read>(reader: &mut R) -> Result<AvifParser<'static>> {
        AvifParser::from_reader_with_config(reader, &DecodeConfig::unlimited(), &Unstoppable)
    }

    /// Parse AVIF from a reader with resource limits.
    pub fn from_reader_with_config<R: Read>(
        reader: &mut R,
        config: &DecodeConfig,
        stop: &dyn Stop,
    ) -> Result<AvifParser<'static>> {
        let mut buf = std::vec::Vec::new();
        reader.read_to_end(&mut buf)?;
        AvifParser::from_owned_with_config(buf, config, stop)
    }

    // ========================================
    // Internal: parse pass (records offsets, no mdat copy)
    // ========================================

    /// Parse the AVIF box structure from raw bytes, recording mdat offsets
    /// without copying mdat content.
    fn parse_raw(data: &[u8], config: &DecodeConfig, stop: &dyn Stop) -> Result<ParsedStructure> {
        let parse_opts = ParseOptions { lenient: config.lenient };
        let mut cursor = std::io::Cursor::new(data);
        let mut f = OffsetReader::new(&mut cursor);
        let mut iter = BoxIter::new(&mut f);

        // 'ftyp' box must occur first; see ISO 14496-12:2015 § 4.3.1
        if let Some(mut b) = iter.next_box()? {
            if b.head.name == BoxType::FileTypeBox {
                let ftyp = read_ftyp(&mut b)?;
                if ftyp.major_brand != b"avif" && ftyp.major_brand != b"avis" {
                    return Err(Error::InvalidData("ftyp must be 'avif' or 'avis'"));
                }
            } else {
                return Err(Error::InvalidData("'ftyp' box must occur first"));
            }
        }

        let mut meta = None;
        let mut mdat_bounds = TryVec::new();
        let mut animation_data: Option<(u32, SampleTable, u32)> = None;

        while let Some(mut b) = iter.next_box()? {
            stop.check()?;

            match b.head.name {
                BoxType::MetadataBox => {
                    if meta.is_some() {
                        return Err(Error::InvalidData(
                            "There should be zero or one meta boxes per ISO 14496-12:2015 § 8.11.1.1",
                        ));
                    }
                    meta = Some(read_avif_meta(&mut b, &parse_opts)?);
                }
                BoxType::MovieBox => {
                    if let Some((media_timescale, sample_table)) = read_moov(&mut b)? {
                        animation_data = Some((media_timescale, sample_table, 0));
                    }
                }
                BoxType::MediaDataBox => {
                    if b.bytes_left() > 0 {
                        let offset = b.offset();
                        let length = b.bytes_left();
                        mdat_bounds.push(MdatBounds { offset, length })?;
                    }
                    // Skip the content — we'll slice into raw later
                    skip_box_content(&mut b)?;
                }
                _ => skip_box_content(&mut b)?,
            }

            check_parser_state(&b.head, &b.content)?;
        }

        let meta = meta.ok_or(Error::InvalidData("missing meta"))?;

        Ok(ParsedStructure { meta, mdat_bounds, animation_data })
    }

    /// Build an AvifParser from raw bytes + parsed structure.
    fn build(raw: Cow<'data, [u8]>, parsed: ParsedStructure, config: &DecodeConfig) -> Result<Self> {
        let tracker = ResourceTracker::new(config);
        let meta = parsed.meta;

        // Get primary item extents
        let primary = Self::get_item_extents(&meta, meta.primary_item_id)?;

        // Find alpha item and get its extents
        let alpha_item_id = meta
            .item_references
            .iter()
            .filter(|iref| {
                iref.to_item_id == meta.primary_item_id
                    && iref.from_item_id != meta.primary_item_id
                    && iref.item_type == b"auxl"
            })
            .map(|iref| iref.from_item_id)
            .find(|&item_id| {
                meta.properties.iter().any(|prop| {
                    prop.item_id == item_id
                        && match &prop.property {
                            ItemProperty::AuxiliaryType(urn) => {
                                urn.type_subtype().0 == b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha"
                            }
                            _ => false,
                        }
                })
            });

        let alpha = alpha_item_id
            .map(|id| Self::get_item_extents(&meta, id))
            .transpose()?;

        // Check for premultiplied alpha
        let premultiplied_alpha = alpha_item_id.map_or(false, |alpha_id| {
            meta.item_references.iter().any(|iref| {
                iref.from_item_id == meta.primary_item_id
                    && iref.to_item_id == alpha_id
                    && iref.item_type == b"prem"
            })
        });

        // Check if primary item is a grid (tiled image)
        let is_grid = meta
            .item_infos
            .iter()
            .find(|x| x.item_id == meta.primary_item_id)
            .map_or(false, |info| info.item_type == b"grid");

        // Extract grid configuration and tile extents if this is a grid
        let (grid_config, tiles) = if is_grid {
            let mut tiles_with_index: TryVec<(u32, u16)> = TryVec::new();
            for iref in meta.item_references.iter() {
                if iref.from_item_id == meta.primary_item_id && iref.item_type == b"dimg" {
                    tiles_with_index.push((iref.to_item_id, iref.reference_index))?;
                }
            }

            tracker.validate_grid_tiles(tiles_with_index.len() as u32)?;
            tiles_with_index.sort_by_key(|&(_, idx)| idx);

            let mut tile_extents = TryVec::new();
            for (tile_id, _) in tiles_with_index.iter() {
                tile_extents.push(Self::get_item_extents(&meta, *tile_id)?)?;
            }

            let mut tile_ids = TryVec::new();
            for (tile_id, _) in tiles_with_index.iter() {
                tile_ids.push(*tile_id)?;
            }

            let grid_config = Self::calculate_grid_config(&meta, &tile_ids)?;
            (Some(grid_config), tile_extents)
        } else {
            (None, TryVec::new())
        };

        // Store animation metadata if present
        let animation_data = if let Some((media_timescale, sample_table, loop_count)) = parsed.animation_data {
            tracker.validate_animation_frames(sample_table.sample_sizes.len() as u32)?;
            Some(AnimationParserData { media_timescale, sample_table, loop_count })
        } else {
            None
        };

        // Clone idat
        let idat = if let Some(ref idat_data) = meta.idat {
            let mut cloned = TryVec::new();
            cloned.extend_from_slice(idat_data)?;
            Some(cloned)
        } else {
            None
        };

        Ok(Self {
            raw,
            mdat_bounds: parsed.mdat_bounds,
            idat,
            primary,
            alpha,
            grid_config,
            tiles,
            animation_data,
            premultiplied_alpha,
        })
    }

    // ========================================
    // Internal helpers
    // ========================================

    /// Get item extents (construction method + ranges) from metadata.
    fn get_item_extents(meta: &AvifInternalMeta, item_id: u32) -> Result<ItemExtents> {
        let item = meta
            .iloc_items
            .iter()
            .find(|item| item.item_id == item_id)
            .ok_or(Error::InvalidData("item not found in iloc"))?;

        let mut extents = TryVec::new();
        for extent in &item.extents {
            extents.push(extent.extent_range.clone())?;
        }
        Ok(ItemExtents {
            construction_method: item.construction_method,
            extents,
        })
    }

    /// Resolve an item's data from the raw buffer, returning `Cow::Borrowed`
    /// for single-extent file items and `Cow::Owned` for multi-extent or idat.
    fn resolve_item(&self, item: &ItemExtents) -> Result<Cow<'_, [u8]>> {
        match item.construction_method {
            ConstructionMethod::Idat => self.resolve_idat_extents(&item.extents),
            ConstructionMethod::File => self.resolve_file_extents(&item.extents),
            ConstructionMethod::Item => Err(Error::Unsupported("construction_method 'item' not supported")),
        }
    }

    /// Resolve file-based extents from the raw buffer.
    fn resolve_file_extents(&self, extents: &[ExtentRange]) -> Result<Cow<'_, [u8]>> {
        let raw = self.raw.as_ref();

        // Fast path: single extent → borrow directly from raw
        if extents.len() == 1 {
            let extent = &extents[0];
            let (start, end) = self.extent_byte_range(extent)?;
            let slice = raw.get(start..end).ok_or(Error::InvalidData("extent out of bounds in raw buffer"))?;
            return Ok(Cow::Borrowed(slice));
        }

        // Multi-extent: concatenate into owned buffer
        let mut data = TryVec::new();
        for extent in extents {
            let (start, end) = self.extent_byte_range(extent)?;
            let slice = raw.get(start..end).ok_or(Error::InvalidData("extent out of bounds in raw buffer"))?;
            data.extend_from_slice(slice)?;
        }
        Ok(Cow::Owned(data.to_vec()))
    }

    /// Convert an ExtentRange to a (start, end) byte range within the raw buffer.
    fn extent_byte_range(&self, extent: &ExtentRange) -> Result<(usize, usize)> {
        let file_offset = extent.start();
        let start = usize::try_from(file_offset)?;

        match extent {
            ExtentRange::WithLength(range) => {
                let len = range.end.checked_sub(range.start)
                    .ok_or(Error::InvalidData("extent range start > end"))?;
                let end = start.checked_add(usize::try_from(len)?)
                    .ok_or(Error::InvalidData("extent end overflow"))?;
                Ok((start, end))
            }
            ExtentRange::ToEnd(_) => {
                // Find the mdat that contains this offset and use its bounds
                for mdat in &self.mdat_bounds {
                    if file_offset >= mdat.offset && file_offset < mdat.offset + mdat.length {
                        let end = usize::try_from(mdat.offset + mdat.length)?;
                        return Ok((start, end));
                    }
                }
                // Fall back to end of raw buffer
                Ok((start, self.raw.len()))
            }
        }
    }

    /// Resolve idat-based extents.
    fn resolve_idat_extents(&self, extents: &[ExtentRange]) -> Result<Cow<'_, [u8]>> {
        let idat_data = self.idat.as_ref()
            .ok_or(Error::InvalidData("idat box missing but construction_method is Idat"))?;

        if extents.len() == 1 {
            let extent = &extents[0];
            let start = usize::try_from(extent.start())?;
            let slice = match extent {
                ExtentRange::WithLength(range) => {
                    let len = usize::try_from(range.end - range.start)?;
                    idat_data.get(start..start + len)
                        .ok_or(Error::InvalidData("idat extent out of bounds"))?
                }
                ExtentRange::ToEnd(_) => {
                    idat_data.get(start..)
                        .ok_or(Error::InvalidData("idat extent out of bounds"))?
                }
            };
            return Ok(Cow::Borrowed(slice));
        }

        // Multi-extent idat: concatenate
        let mut data = TryVec::new();
        for extent in extents {
            let start = usize::try_from(extent.start())?;
            let slice = match extent {
                ExtentRange::WithLength(range) => {
                    let len = usize::try_from(range.end - range.start)?;
                    idat_data.get(start..start + len)
                        .ok_or(Error::InvalidData("idat extent out of bounds"))?
                }
                ExtentRange::ToEnd(_) => {
                    idat_data.get(start..)
                        .ok_or(Error::InvalidData("idat extent out of bounds"))?
                }
            };
            data.extend_from_slice(slice)?;
        }
        Ok(Cow::Owned(data.to_vec()))
    }

    /// Resolve a single animation frame from the raw buffer.
    fn resolve_frame(&self, index: usize) -> Result<FrameRef<'_>> {
        let anim = self.animation_data.as_ref()
            .ok_or(Error::InvalidData("not an animated AVIF"))?;

        if index >= anim.sample_table.sample_sizes.len() {
            return Err(Error::InvalidData("frame index out of bounds"));
        }

        let duration_ms = self.calculate_frame_duration(&anim.sample_table, anim.media_timescale, index)?;
        let (offset, size) = self.calculate_sample_location(&anim.sample_table, index)?;

        let start = usize::try_from(offset)?;
        let end = start.checked_add(size as usize)
            .ok_or(Error::InvalidData("frame end overflow"))?;

        let raw = self.raw.as_ref();
        let slice = raw.get(start..end)
            .ok_or(Error::InvalidData("frame not found in raw buffer"))?;

        Ok(FrameRef {
            data: Cow::Borrowed(slice),
            duration_ms,
        })
    }

    /// Calculate grid configuration from metadata.
    fn calculate_grid_config(meta: &AvifInternalMeta, tile_ids: &[u32]) -> Result<GridConfig> {
        // Try explicit grid property first
        for prop in &meta.properties {
            if prop.item_id == meta.primary_item_id {
                if let ItemProperty::ImageGrid(grid) = &prop.property {
                    return Ok(grid.clone());
                }
            }
        }

        // Fall back to ispe calculation
        let grid_dims = meta
            .properties
            .iter()
            .find(|p| p.item_id == meta.primary_item_id)
            .and_then(|p| match &p.property {
                ItemProperty::ImageSpatialExtents(e) => Some(e),
                _ => None,
            });

        let tile_dims = tile_ids.first().and_then(|&tile_id| {
            meta.properties
                .iter()
                .find(|p| p.item_id == tile_id)
                .and_then(|p| match &p.property {
                    ItemProperty::ImageSpatialExtents(e) => Some(e),
                    _ => None,
                })
        });

        if let (Some(grid), Some(tile)) = (grid_dims, tile_dims) {
            if tile.width != 0
                && tile.height != 0
                && grid.width % tile.width == 0
                && grid.height % tile.height == 0
            {
                let columns = grid.width / tile.width;
                let rows = grid.height / tile.height;

                if columns <= 255 && rows <= 255 {
                    return Ok(GridConfig {
                        rows: rows as u8,
                        columns: columns as u8,
                        output_width: grid.width,
                        output_height: grid.height,
                    });
                }
            }
        }

        let tile_count = tile_ids.len();
        Ok(GridConfig {
            rows: tile_count.min(255) as u8,
            columns: 1,
            output_width: 0,
            output_height: 0,
        })
    }

    /// Calculate frame duration from sample table.
    fn calculate_frame_duration(
        &self,
        st: &SampleTable,
        timescale: u32,
        index: usize,
    ) -> Result<u32> {
        let mut current_sample = 0;
        for entry in &st.time_to_sample {
            if current_sample + entry.sample_count as usize > index {
                let duration_ms = if timescale > 0 {
                    ((entry.sample_delta as u64) * 1000) / (timescale as u64)
                } else {
                    0
                };
                return Ok(duration_ms as u32);
            }
            current_sample += entry.sample_count as usize;
        }
        Ok(0)
    }

    /// Calculate sample location (offset and size) from sample table.
    fn calculate_sample_location(&self, st: &SampleTable, index: usize) -> Result<(u64, u32)> {
        let sample_size = *st
            .sample_sizes
            .get(index)
            .ok_or(Error::InvalidData("sample index out of bounds"))?;

        let mut current_sample = 0;
        for (chunk_map_idx, entry) in st.sample_to_chunk.iter().enumerate() {
            let next_first_chunk = st
                .sample_to_chunk
                .get(chunk_map_idx + 1)
                .map(|e| e.first_chunk)
                .unwrap_or(u32::MAX);

            for chunk_idx in entry.first_chunk..next_first_chunk {
                if chunk_idx == 0 || (chunk_idx as usize) > st.chunk_offsets.len() {
                    break;
                }

                let chunk_offset = st.chunk_offsets[(chunk_idx - 1) as usize];

                for sample_in_chunk in 0..entry.samples_per_chunk {
                    if current_sample == index {
                        let mut offset_in_chunk = 0u64;
                        for s in 0..sample_in_chunk {
                            let prev_idx = current_sample.saturating_sub((sample_in_chunk - s) as usize);
                            if let Some(&prev_size) = st.sample_sizes.get(prev_idx) {
                                offset_in_chunk += prev_size as u64;
                            }
                        }

                        return Ok((chunk_offset + offset_in_chunk, sample_size));
                    }
                    current_sample += 1;
                }
            }
        }

        Err(Error::InvalidData("sample not found in chunk table"))
    }

    // ========================================
    // Public data access API (one way each)
    // ========================================

    /// Get primary item data.
    ///
    /// Returns `Cow::Borrowed` for single-extent items, `Cow::Owned` for multi-extent.
    pub fn primary_data(&self) -> Result<Cow<'_, [u8]>> {
        self.resolve_item(&self.primary)
    }

    /// Get alpha item data, if present.
    pub fn alpha_data(&self) -> Option<Result<Cow<'_, [u8]>>> {
        self.alpha.as_ref().map(|item| self.resolve_item(item))
    }

    /// Get grid tile data by index.
    pub fn tile_data(&self, index: usize) -> Result<Cow<'_, [u8]>> {
        let item = self.tiles.get(index)
            .ok_or(Error::InvalidData("tile index out of bounds"))?;
        self.resolve_item(item)
    }

    /// Get a single animation frame by index.
    pub fn frame(&self, index: usize) -> Result<FrameRef<'_>> {
        self.resolve_frame(index)
    }

    /// Iterate over all animation frames.
    pub fn frames(&self) -> FrameIterator<'_> {
        let count = self
            .animation_info()
            .map(|info| info.frame_count)
            .unwrap_or(0);
        FrameIterator { parser: self, index: 0, count }
    }

    // ========================================
    // Metadata (no data access)
    // ========================================

    /// Get animation metadata (if animated).
    pub fn animation_info(&self) -> Option<AnimationInfo> {
        self.animation_data.as_ref().map(|data| AnimationInfo {
            frame_count: data.sample_table.sample_sizes.len(),
            loop_count: data.loop_count,
        })
    }

    /// Get grid configuration (if grid image).
    pub fn grid_config(&self) -> Option<&GridConfig> {
        self.grid_config.as_ref()
    }

    /// Get number of grid tiles.
    pub fn grid_tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Check if alpha channel uses premultiplied alpha.
    pub fn premultiplied_alpha(&self) -> bool {
        self.premultiplied_alpha
    }

    /// Parse AV1 metadata from the primary item.
    pub fn primary_metadata(&self) -> Result<AV1Metadata> {
        let data = self.primary_data()?;
        AV1Metadata::parse_av1_bitstream(&data)
    }

    /// Parse AV1 metadata from the alpha item, if present.
    pub fn alpha_metadata(&self) -> Option<Result<AV1Metadata>> {
        self.alpha.as_ref().map(|item| {
            let data = self.resolve_item(item)?;
            AV1Metadata::parse_av1_bitstream(&data)
        })
    }

    // ========================================
    // Conversion
    // ========================================

    /// Convert to [`AvifData`] (eagerly loads all frames and tiles).
    ///
    /// Provided for migration from the eager API. Prefer using `AvifParser`
    /// methods directly.
    #[deprecated(since = "1.5.0", note = "Use AvifParser methods directly instead of converting to AvifData")]
    #[allow(deprecated)]
    pub fn to_avif_data(&self) -> Result<AvifData> {
        let primary_data = self.primary_data()?;
        let mut primary_item = TryVec::new();
        primary_item.extend_from_slice(&primary_data)?;

        let alpha_item = match self.alpha_data() {
            Some(Ok(data)) => {
                let mut v = TryVec::new();
                v.extend_from_slice(&data)?;
                Some(v)
            }
            Some(Err(e)) => return Err(e),
            None => None,
        };

        let mut grid_tiles = TryVec::new();
        for i in 0..self.grid_tile_count() {
            let data = self.tile_data(i)?;
            let mut v = TryVec::new();
            v.extend_from_slice(&data)?;
            grid_tiles.push(v)?;
        }

        let animation = if let Some(info) = self.animation_info() {
            let mut frames = TryVec::new();
            for i in 0..info.frame_count {
                let frame_ref = self.frame(i)?;
                let mut data = TryVec::new();
                data.extend_from_slice(&frame_ref.data)?;
                frames.push(AnimationFrame { data, duration_ms: frame_ref.duration_ms })?;
            }
            Some(AnimationConfig {
                loop_count: info.loop_count,
                frames,
            })
        } else {
            None
        };

        Ok(AvifData {
            primary_item,
            alpha_item,
            premultiplied_alpha: self.premultiplied_alpha,
            grid_config: self.grid_config.clone(),
            grid_tiles,
            animation,
        })
    }
}

/// Iterator over animation frames.
///
/// Created by [`AvifParser::frames()`]. Yields [`FrameRef`] on demand.
pub struct FrameIterator<'a> {
    parser: &'a AvifParser<'a>,
    index: usize,
    count: usize,
}

impl<'a> Iterator for FrameIterator<'a> {
    type Item = Result<FrameRef<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.count {
            return None;
        }
        let result = self.parser.frame(self.index);
        self.index += 1;
        Some(result)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.count.saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for FrameIterator<'_> {
    fn len(&self) -> usize {
        self.count.saturating_sub(self.index)
    }
}

struct AvifInternalMeta {
    item_references: TryVec<SingleItemTypeReferenceBox>,
    properties: TryVec<AssociatedProperty>,
    primary_item_id: u32,
    iloc_items: TryVec<ItemLocationBoxItem>,
    item_infos: TryVec<ItemInfoEntry>,
    idat: Option<TryVec<u8>>,
}

/// A Media Data Box
/// See ISO 14496-12:2015 § 8.1.1
struct MediaDataBox {
    /// Offset of `data` from the beginning of the file. See `ConstructionMethod::File`
    offset: u64,
    data: TryVec<u8>,
}

impl MediaDataBox {
    /// Check whether the beginning of `extent` is within the bounds of the `MediaDataBox`.
    /// We assume extents to not cross box boundaries. If so, this will cause an error
    /// in `read_extent`.
    fn contains_extent(&self, extent: &ExtentRange) -> bool {
        if self.offset <= extent.start() {
            let start_offset = extent.start() - self.offset;
            start_offset < self.data.len().to_u64()
        } else {
            false
        }
    }

    /// Check whether `extent` covers the `MediaDataBox` exactly.
    fn matches_extent(&self, extent: &ExtentRange) -> bool {
        if self.offset == extent.start() {
            match extent {
                ExtentRange::WithLength(range) => {
                    if let Some(end) = self.offset.checked_add(self.data.len().to_u64()) {
                        end == range.end
                    } else {
                        false
                    }
                },
                ExtentRange::ToEnd(_) => true,
            }
        } else {
            false
        }
    }

    /// Copy the range specified by `extent` to the end of `buf` or return an error if the range
    /// is not fully contained within `MediaDataBox`.
    fn read_extent(&self, extent: &ExtentRange, buf: &mut TryVec<u8>) -> Result<()> {
        let start_offset = extent
            .start()
            .checked_sub(self.offset)
            .ok_or(Error::InvalidData("mdat does not contain extent"))?;
        let slice = match extent {
            ExtentRange::WithLength(range) => {
                let range_len = range
                    .end
                    .checked_sub(range.start)
                    .ok_or(Error::InvalidData("range start > end"))?;
                let end = start_offset
                    .checked_add(range_len)
                    .ok_or(Error::InvalidData("extent end overflow"))?;
                self.data.get(start_offset.try_into()?..end.try_into()?)
            },
            ExtentRange::ToEnd(_) => self.data.get(start_offset.try_into()?..),
        };
        let slice = slice.ok_or(Error::InvalidData("extent crosses box boundary"))?;
        buf.extend_from_slice(slice)?;
        Ok(())
    }

}

/// Used for 'infe' boxes within 'iinf' boxes
/// See ISO 14496-12:2015 § 8.11.6
/// Only versions {2, 3} are supported
#[derive(Debug)]
struct ItemInfoEntry {
    item_id: u32,
    item_type: FourCC,
}

/// See ISO 14496-12:2015 § 8.11.12
#[derive(Debug)]
struct SingleItemTypeReferenceBox {
    item_type: FourCC,
    from_item_id: u32,
    to_item_id: u32,
    /// Index of this reference within the list of references of the same type from the same item
    /// (0-based). This is the dimgIdx for grid tiles.
    reference_index: u16,
}

/// Potential sizes (in bytes) of variable-sized fields of the 'iloc' box
/// See ISO 14496-12:2015 § 8.11.3
#[derive(Debug)]
enum IlocFieldSize {
    Zero,
    Four,
    Eight,
}

impl IlocFieldSize {
    const fn to_bits(&self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::Four => 32,
            Self::Eight => 64,
        }
    }
}

impl TryFrom<u8> for IlocFieldSize {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Zero),
            4 => Ok(Self::Four),
            8 => Ok(Self::Eight),
            _ => Err(Error::InvalidData("value must be in the set {0, 4, 8}")),
        }
    }
}

#[derive(PartialEq)]
enum IlocVersion {
    Zero,
    One,
    Two,
}

impl TryFrom<u8> for IlocVersion {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Zero),
            1 => Ok(Self::One),
            2 => Ok(Self::Two),
            _ => Err(Error::Unsupported("unsupported version in 'iloc' box")),
        }
    }
}

/// Used for 'iloc' boxes
/// See ISO 14496-12:2015 § 8.11.3
/// `base_offset` is omitted since it is integrated into the ranges in `extents`
/// `data_reference_index` is omitted, since only 0 (i.e., this file) is supported
#[derive(Debug)]
struct ItemLocationBoxItem {
    item_id: u32,
    construction_method: ConstructionMethod,
    /// Unused for `ConstructionMethod::Idat`
    extents: TryVec<ItemLocationBoxExtent>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ConstructionMethod {
    File,
    Idat,
    #[allow(dead_code)] // TODO: see https://github.com/mozilla/mp4parse-rust/issues/196
    Item,
}

/// `extent_index` is omitted since it's only used for `ConstructionMethod::Item` which
/// is currently not implemented.
#[derive(Clone, Debug)]
struct ItemLocationBoxExtent {
    extent_range: ExtentRange,
}

#[derive(Clone, Debug)]
enum ExtentRange {
    WithLength(Range<u64>),
    ToEnd(RangeFrom<u64>),
}

impl ExtentRange {
    const fn start(&self) -> u64 {
        match self {
            Self::WithLength(r) => r.start,
            Self::ToEnd(r) => r.start,
        }
    }
}

/// See ISO 14496-12:2015 § 4.2
struct BMFFBox<'a, T> {
    head: BoxHeader,
    content: Take<&'a mut T>,
}

impl<T: Read> BMFFBox<'_, T> {
    fn read_into_try_vec(&mut self) -> std::io::Result<TryVec<u8>> {
        let limit = self.content.limit();
        // For size=0 boxes, size is set to u64::MAX, but after subtracting offset
        // (8 or 16 bytes), the limit will be slightly less. Check for values very
        // close to u64::MAX to detect these cases.
        let mut vec = if limit >= u64::MAX - BoxHeader::MIN_LARGE_SIZE {
            // Unknown size (size=0 box), read without pre-allocation
            std::vec::Vec::new()
        } else {
            let mut v = std::vec::Vec::new();
            v.try_reserve_exact(limit as usize)
                .map_err(|_| std::io::ErrorKind::OutOfMemory)?;
            v
        };
        self.content.read_to_end(&mut vec)?; // The default impl
        Ok(vec.into())
    }
}

#[test]
fn box_read_to_end() {
    let tmp = &mut b"1234567890".as_slice();
    let mut src = BMFFBox {
        head: BoxHeader { name: BoxType::FileTypeBox, size: 5, offset: 0, uuid: None },
        content: <_ as Read>::take(tmp, 5),
    };
    let buf = src.read_into_try_vec().unwrap();
    assert_eq!(buf.len(), 5);
    assert_eq!(buf, b"12345".as_ref());
}

#[test]
fn box_read_to_end_oom() {
    let tmp = &mut b"1234567890".as_slice();
    let mut src = BMFFBox {
        head: BoxHeader { name: BoxType::FileTypeBox, size: 5, offset: 0, uuid: None },
        // Use a very large value to trigger OOM, but not near u64::MAX (which indicates size=0 boxes)
        content: <_ as Read>::take(tmp, u64::MAX / 2),
    };
    assert!(src.read_into_try_vec().is_err());
}

struct BoxIter<'a, T> {
    src: &'a mut T,
}

impl<T: Read> BoxIter<'_, T> {
    fn new(src: &mut T) -> BoxIter<'_, T> {
        BoxIter { src }
    }

    fn next_box(&mut self) -> Result<Option<BMFFBox<'_, T>>> {
        let r = read_box_header(self.src);
        match r {
            Ok(h) => Ok(Some(BMFFBox {
                head: h,
                content: self.src.take(h.size - h.offset),
            })),
            Err(Error::UnexpectedEOF) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl<T: Read> Read for BMFFBox<'_, T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.content.read(buf)
    }
}

impl<T: Offset> Offset for BMFFBox<'_, T> {
    fn offset(&self) -> u64 {
        self.content.get_ref().offset()
    }
}

impl<T: Read> BMFFBox<'_, T> {
    fn bytes_left(&self) -> u64 {
        self.content.limit()
    }

    const fn get_header(&self) -> &BoxHeader {
        &self.head
    }

    fn box_iter(&mut self) -> BoxIter<'_, Self> {
        BoxIter::new(self)
    }
}

impl<T> Drop for BMFFBox<'_, T> {
    fn drop(&mut self) {
        if self.content.limit() > 0 {
            let name: FourCC = From::from(self.head.name);
            debug!("Dropping {} bytes in '{}'", self.content.limit(), name);
        }
    }
}

/// Read and parse a box header.
///
/// Call this first to determine the type of a particular mp4 box
/// and its length. Used internally for dispatching to specific
/// parsers for the internal content, or to get the length to
/// skip unknown or uninteresting boxes.
///
/// See ISO 14496-12:2015 § 4.2
fn read_box_header<T: ReadBytesExt>(src: &mut T) -> Result<BoxHeader> {
    let size32 = be_u32(src)?;
    let name = BoxType::from(be_u32(src)?);
    let size = match size32 {
        // valid only for top-level box and indicates it's the last box in the file.  usually mdat.
        0 => {
            // Size=0 means box extends to EOF (ISOBMFF spec allows this for last box)
            u64::MAX
        },
        1 => {
            let size64 = be_u64(src)?;
            if size64 < BoxHeader::MIN_LARGE_SIZE {
                return Err(Error::InvalidData("malformed wide size"));
            }
            size64
        },
        _ => {
            if u64::from(size32) < BoxHeader::MIN_SIZE {
                return Err(Error::InvalidData("malformed size"));
            }
            u64::from(size32)
        },
    };
    let mut offset = match size32 {
        1 => BoxHeader::MIN_LARGE_SIZE,
        _ => BoxHeader::MIN_SIZE,
    };
    let uuid = if name == BoxType::UuidBox {
        if size >= offset + 16 {
            let mut buffer = [0u8; 16];
            let count = src.read(&mut buffer)?;
            offset += count.to_u64();
            if count == 16 {
                Some(buffer)
            } else {
                debug!("malformed uuid (short read), skipping");
                None
            }
        } else {
            debug!("malformed uuid, skipping");
            None
        }
    } else {
        None
    };
    assert!(offset <= size);
    Ok(BoxHeader { name, size, offset, uuid })
}

/// Parse the extra header fields for a full box.
fn read_fullbox_extra<T: ReadBytesExt>(src: &mut T) -> Result<(u8, u32)> {
    let version = src.read_u8()?;
    let flags_a = src.read_u8()?;
    let flags_b = src.read_u8()?;
    let flags_c = src.read_u8()?;
    Ok((
        version,
        u32::from(flags_a) << 16 | u32::from(flags_b) << 8 | u32::from(flags_c),
    ))
}

// Parse the extra fields for a full box whose flag fields must be zero.
fn read_fullbox_version_no_flags<T: ReadBytesExt>(src: &mut T, options: &ParseOptions) -> Result<u8> {
    let (version, flags) = read_fullbox_extra(src)?;

    if flags != 0 && !options.lenient {
        return Err(Error::Unsupported("expected flags to be 0"));
    }

    Ok(version)
}

/// Skip over the entire contents of a box.
fn skip_box_content<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<()> {
    // Skip the contents of unknown chunks.
    let to_skip = {
        let header = src.get_header();
        debug!("{header:?} (skipped)");
        header
            .size
            .checked_sub(header.offset)
            .ok_or(Error::InvalidData("header offset > size"))?
    };
    assert_eq!(to_skip, src.bytes_left());
    skip(src, to_skip)
}

/// Skip over the remain data of a box.
fn skip_box_remain<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<()> {
    let remain = {
        let header = src.get_header();
        let len = src.bytes_left();
        debug!("remain {len} (skipped) in {header:?}");
        len
    };
    skip(src, remain)
}

struct ResourceTracker<'a> {
    config: &'a DecodeConfig,
    current_memory: u64,
    peak_memory: u64,
}

impl<'a> ResourceTracker<'a> {
    fn new(config: &'a DecodeConfig) -> Self {
        Self {
            config,
            current_memory: 0,
            peak_memory: 0,
        }
    }

    fn reserve(&mut self, bytes: u64) -> Result<()> {
        self.current_memory = self.current_memory.saturating_add(bytes);
        self.peak_memory = self.peak_memory.max(self.current_memory);

        if let Some(limit) = self.config.peak_memory_limit {
            if self.peak_memory > limit {
                return Err(Error::ResourceLimitExceeded("peak memory limit exceeded"));
            }
        }

        Ok(())
    }

    fn release(&mut self, bytes: u64) {
        self.current_memory = self.current_memory.saturating_sub(bytes);
    }

    fn validate_total_megapixels(&self, width: u32, height: u32) -> Result<()> {
        if let Some(limit) = self.config.total_megapixels_limit {
            let megapixels = (width as u64)
                .checked_mul(height as u64)
                .ok_or(Error::InvalidData("dimension overflow"))?
                / 1_000_000;

            if megapixels > limit as u64 {
                return Err(Error::ResourceLimitExceeded("total megapixels limit exceeded"));
            }
        }

        Ok(())
    }

    fn validate_animation_frames(&self, count: u32) -> Result<()> {
        if let Some(limit) = self.config.max_animation_frames {
            if count > limit {
                return Err(Error::ResourceLimitExceeded("animation frame count limit exceeded"));
            }
        }

        Ok(())
    }

    fn validate_grid_tiles(&self, count: u32) -> Result<()> {
        if let Some(limit) = self.config.max_grid_tiles {
            if count > limit {
                return Err(Error::ResourceLimitExceeded("grid tile count limit exceeded"));
            }
        }

        Ok(())
    }
}

/// Read the contents of an AVIF file with resource limits and cancellation support
///
/// This is the primary parsing function with full control over resource limits
/// and cooperative cancellation via the [`Stop`] trait.
///
/// # Arguments
///
/// * `f` - Reader for the AVIF file
/// * `config` - Resource limits and parsing options
/// * `stop` - Cancellation token (use [`Unstoppable`] if not needed)
#[deprecated(since = "1.5.0", note = "Use `AvifParser::from_reader_with_config()` instead")]
#[allow(deprecated)]
pub fn read_avif_with_config<T: Read>(
    f: &mut T,
    config: &DecodeConfig,
    stop: &dyn Stop,
) -> Result<AvifData> {
    let mut tracker = ResourceTracker::new(config);
    let mut f = OffsetReader::new(f);

    let mut iter = BoxIter::new(&mut f);

    // 'ftyp' box must occur first; see ISO 14496-12:2015 § 4.3.1
    if let Some(mut b) = iter.next_box()? {
        if b.head.name == BoxType::FileTypeBox {
            let ftyp = read_ftyp(&mut b)?;
            // Accept both 'avif' (single-frame) and 'avis' (animated) brands
            if ftyp.major_brand != b"avif" && ftyp.major_brand != b"avis" {
                warn!("major_brand: {}", ftyp.major_brand);
                return Err(Error::InvalidData("ftyp must be 'avif' or 'avis'"));
            }
            let _is_animated = ftyp.major_brand == b"avis";
        } else {
            return Err(Error::InvalidData("'ftyp' box must occur first"));
        }
    }

    let mut meta = None;
    let mut mdats = TryVec::new();
    let mut animation_data: Option<(u32, SampleTable)> = None;

    let parse_opts = ParseOptions { lenient: config.lenient };

    while let Some(mut b) = iter.next_box()? {
        stop.check()?;

        match b.head.name {
            BoxType::MetadataBox => {
                if meta.is_some() {
                    return Err(Error::InvalidData("There should be zero or one meta boxes per ISO 14496-12:2015 § 8.11.1.1"));
                }
                meta = Some(read_avif_meta(&mut b, &parse_opts)?);
            },
            BoxType::MovieBox => {
                animation_data = read_moov(&mut b)?;
            },
            BoxType::MediaDataBox => {
                if b.bytes_left() > 0 {
                    let offset = b.offset();
                    let size = b.bytes_left();
                    tracker.reserve(size)?;
                    let data = b.read_into_try_vec()?;
                    tracker.release(size);
                    mdats.push(MediaDataBox { offset, data })?;
                }
            },
            _ => skip_box_content(&mut b)?,
        }

        check_parser_state(&b.head, &b.content)?;
    }

    let meta = meta.ok_or(Error::InvalidData("missing meta"))?;

    // Check if primary item is a grid (tiled image)
    let is_grid = meta
        .item_infos
        .iter()
        .find(|x| x.item_id == meta.primary_item_id)
        .map_or(false, |info| {
            let is_g = info.item_type == b"grid";
            if is_g {
                log::debug!("Grid image detected: primary_item_id={}", meta.primary_item_id);
            }
            is_g
        });

    // Extract grid configuration if this is a grid image
    let mut grid_config = if is_grid {
        meta.properties
            .iter()
            .find(|prop| {
                prop.item_id == meta.primary_item_id
                    && matches!(prop.property, ItemProperty::ImageGrid(_))
            })
            .and_then(|prop| match &prop.property {
                ItemProperty::ImageGrid(config) => {
                    log::debug!("Grid: found explicit ImageGrid property: {:?}", config);
                    Some(config.clone())
                },
                _ => None,
            })
    } else {
        None
    };

    // Find tile item IDs if this is a grid
    let tile_item_ids: TryVec<u32> = if is_grid {
        // Collect tiles with their reference index
        let mut tiles_with_index: TryVec<(u32, u16)> = TryVec::new();
        for iref in meta.item_references.iter() {
            // Grid items reference tiles via "dimg" (derived image) type
            if iref.from_item_id == meta.primary_item_id && iref.item_type == b"dimg" {
                tiles_with_index.push((iref.to_item_id, iref.reference_index))?;
            }
        }

        // Validate tile count
        tracker.validate_grid_tiles(tiles_with_index.len() as u32)?;

        // Sort tiles by reference_index to get correct grid order
        tiles_with_index.sort_by_key(|&(_, idx)| idx);

        // Extract just the IDs in sorted order
        let mut ids = TryVec::new();
        for (tile_id, _) in tiles_with_index.iter() {
            ids.push(*tile_id)?;
        }

        // No logging here - too verbose for production

        // If no ImageGrid property found, calculate grid layout from ispe dimensions
        if grid_config.is_none() && !ids.is_empty() {
            // Try to calculate grid dimensions from ispe properties
            let grid_dims = meta.properties.iter()
                .find(|p| p.item_id == meta.primary_item_id)
                .and_then(|p| match &p.property {
                    ItemProperty::ImageSpatialExtents(e) => Some(e),
                    _ => None,
                });

            let tile_dims = ids.first().and_then(|&tile_id| {
                meta.properties.iter()
                    .find(|p| p.item_id == tile_id)
                    .and_then(|p| match &p.property {
                        ItemProperty::ImageSpatialExtents(e) => Some(e),
                        _ => None,
                    })
            });

            if let (Some(grid), Some(tile)) = (grid_dims, tile_dims) {
                // Validate grid output dimensions
                tracker.validate_total_megapixels(grid.width, grid.height)?;

                // Validate tile dimensions are non-zero (already validated in read_ispe, but defensive)
                if tile.width == 0 || tile.height == 0 {
                    log::warn!("Grid: tile has zero dimensions, using fallback");
                } else if grid.width % tile.width == 0 && grid.height % tile.height == 0 {
                    // Calculate grid layout: grid_dims ÷ tile_dims
                    let columns = grid.width / tile.width;
                    let rows = grid.height / tile.height;

                    // Validate grid dimensions fit in u8 (max 255×255 grid)
                    if columns > 255 || rows > 255 {
                        log::warn!("Grid: calculated dimensions {}×{} exceed 255, using fallback", rows, columns);
                    } else {
                        log::debug!("Grid: calculated {}×{} layout from ispe dimensions", rows, columns);
                        grid_config = Some(GridConfig {
                            rows: rows as u8,
                            columns: columns as u8,
                            output_width: grid.width,
                            output_height: grid.height,
                        });
                    }
                } else {
                    log::warn!("Grid: dimension mismatch - grid {}×{} not evenly divisible by tile {}×{}, using fallback",
                              grid.width, grid.height, tile.width, tile.height);
                }
            }

            // Fallback: if calculation failed or ispe not available, use N×1 inference
            if grid_config.is_none() {
                log::debug!("Grid: using fallback {}×1 layout inference", ids.len());
                grid_config = Some(GridConfig {
                    rows: ids.len() as u8,  // Changed: vertical stack
                    columns: 1,              // Changed: single column
                    output_width: 0,  // Will be calculated from tiles
                    output_height: 0, // Will be calculated from tiles
                });
            }
        }

        ids
    } else {
        TryVec::new()
    };

    let alpha_item_id = meta
        .item_references
        .iter()
        // Auxiliary image for the primary image
        .filter(|iref| {
            iref.to_item_id == meta.primary_item_id
                && iref.from_item_id != meta.primary_item_id
                && iref.item_type == b"auxl"
        })
        .map(|iref| iref.from_item_id)
        // which has the alpha property
        .find(|&item_id| {
            meta.properties.iter().any(|prop| {
                prop.item_id == item_id
                    && match &prop.property {
                        ItemProperty::AuxiliaryType(urn) => {
                            urn.type_subtype().0 == b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha"
                        }
                        _ => false,
                    }
            })
        });

    let mut context = AvifData {
        premultiplied_alpha: alpha_item_id.map_or(false, |alpha_item_id| {
            meta.item_references.iter().any(|iref| {
                iref.from_item_id == meta.primary_item_id
                    && iref.to_item_id == alpha_item_id
                    && iref.item_type == b"prem"
            })
        }),
        ..Default::default()
    };

    // Helper to extract item data from either mdat or idat
    let mut extract_item_data = |loc: &ItemLocationBoxItem, buf: &mut TryVec<u8>| -> Result<()> {
        match loc.construction_method {
            ConstructionMethod::File => {
                for extent in loc.extents.iter() {
                    let mut found = false;
                    for mdat in mdats.iter_mut() {
                        if mdat.matches_extent(&extent.extent_range) {
                            buf.append(&mut mdat.data)?;
                            found = true;
                            break;
                        } else if mdat.contains_extent(&extent.extent_range) {
                            mdat.read_extent(&extent.extent_range, buf)?;
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        return Err(Error::InvalidData("iloc contains an extent that is not in mdat"));
                    }
                }
                Ok(())
            },
            ConstructionMethod::Idat => {
                let idat_data = meta.idat.as_ref().ok_or(Error::InvalidData("idat box missing but construction_method is Idat"))?;
                for extent in loc.extents.iter() {
                    match &extent.extent_range {
                        ExtentRange::WithLength(range) => {
                            let start = usize::try_from(range.start).map_err(|_| Error::InvalidData("extent start too large"))?;
                            let end = usize::try_from(range.end).map_err(|_| Error::InvalidData("extent end too large"))?;
                            if end > idat_data.len() {
                                return Err(Error::InvalidData("extent exceeds idat size"));
                            }
                            buf.extend_from_slice(&idat_data[start..end]).map_err(|_| Error::OutOfMemory)?;
                        },
                        ExtentRange::ToEnd(range) => {
                            let start = usize::try_from(range.start).map_err(|_| Error::InvalidData("extent start too large"))?;
                            if start >= idat_data.len() {
                                return Err(Error::InvalidData("extent start exceeds idat size"));
                            }
                            buf.extend_from_slice(&idat_data[start..]).map_err(|_| Error::OutOfMemory)?;
                        },
                    }
                }
                Ok(())
            },
            ConstructionMethod::Item => {
                Err(Error::Unsupported("construction_method 'item' not supported"))
            },
        }
    };

    // load data of relevant items
    // For grid images, we need to load tiles in the order specified by iref
    if is_grid {
        // Extract each tile in order
        for (idx, &tile_id) in tile_item_ids.iter().enumerate() {
            if idx % 16 == 0 {
                stop.check()?;
            }

            let mut tile_data = TryVec::new();

            if let Some(loc) = meta.iloc_items.iter().find(|loc| loc.item_id == tile_id) {
                extract_item_data(loc, &mut tile_data)?;
            } else {
                return Err(Error::InvalidData("grid tile not found in iloc"));
            }

            context.grid_tiles.push(tile_data)?;
        }

        // Set grid_config in context
        context.grid_config = grid_config;
    } else {
        // Standard single-frame AVIF: load primary_item and optional alpha_item
        for loc in meta.iloc_items.iter() {
            let item_data = if loc.item_id == meta.primary_item_id {
                &mut context.primary_item
            } else if Some(loc.item_id) == alpha_item_id {
                context.alpha_item.get_or_insert_with(TryVec::new)
            } else {
                continue;
            };

            extract_item_data(loc, item_data)?;
        }
    }

    // Extract animation frames if this is an animated AVIF
    if let Some((media_timescale, sample_table)) = animation_data {
        let frame_count = sample_table.sample_sizes.len() as u32;
        tracker.validate_animation_frames(frame_count)?;

        log::debug!("Animation: extracting frames (media_timescale={})", media_timescale);
        match extract_animation_frames(&sample_table, media_timescale, &mut mdats) {
            Ok(frames) => {
                if !frames.is_empty() {
                    log::debug!("Animation: extracted {} frames", frames.len());
                    context.animation = Some(AnimationConfig {
                        loop_count: 0, // TODO: parse from edit list or meta
                        frames,
                    });
                }
            }
            Err(e) => {
                log::warn!("Animation: failed to extract frames: {}", e);
            }
        }
    }

    Ok(context)
}

/// Read the contents of an AVIF file with custom parsing options
///
/// Uses unlimited resource limits for backwards compatibility.
///
/// # Arguments
///
/// * `f` - Reader for the AVIF file
/// * `options` - Parsing options (e.g., lenient mode)
#[deprecated(since = "1.5.0", note = "Use `AvifParser::from_reader_with_config()` with `DecodeConfig::lenient()` instead")]
#[allow(deprecated)]
pub fn read_avif_with_options<T: Read>(f: &mut T, options: &ParseOptions) -> Result<AvifData> {
    let config = DecodeConfig::unlimited().lenient(options.lenient);
    read_avif_with_config(f, &config, &Unstoppable)
}

/// Read the contents of an AVIF file
///
/// Metadata is accumulated and returned in [`AvifData`] struct.
/// Uses strict validation and unlimited resource limits by default.
///
/// For resource limits, use [`read_avif_with_config`].
/// For lenient parsing, use [`read_avif_with_options`].
#[deprecated(since = "1.5.0", note = "Use `AvifParser::from_reader()` instead")]
#[allow(deprecated)]
pub fn read_avif<T: Read>(f: &mut T) -> Result<AvifData> {
    read_avif_with_options(f, &ParseOptions::default())
}

/// Parse a metadata box in the context of an AVIF
/// Currently requires the primary item to be an av01 item type and generates
/// an error otherwise.
/// See ISO 14496-12:2015 § 8.11.1
fn read_avif_meta<T: Read + Offset>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<AvifInternalMeta> {
    let version = read_fullbox_version_no_flags(src, options)?;

    if version != 0 {
        return Err(Error::Unsupported("unsupported meta version"));
    }

    let mut primary_item_id = None;
    let mut item_infos = None;
    let mut iloc_items = None;
    let mut item_references = TryVec::new();
    let mut properties = TryVec::new();
    let mut idat = None;

    let mut iter = src.box_iter();
    while let Some(mut b) = iter.next_box()? {
        match b.head.name {
            BoxType::ItemInfoBox => {
                if item_infos.is_some() {
                    return Err(Error::InvalidData("There should be zero or one iinf boxes per ISO 14496-12:2015 § 8.11.6.1"));
                }
                item_infos = Some(read_iinf(&mut b, options)?);
            },
            BoxType::ItemLocationBox => {
                if iloc_items.is_some() {
                    return Err(Error::InvalidData("There should be zero or one iloc boxes per ISO 14496-12:2015 § 8.11.3.1"));
                }
                iloc_items = Some(read_iloc(&mut b, options)?);
            },
            BoxType::PrimaryItemBox => {
                if primary_item_id.is_some() {
                    return Err(Error::InvalidData("There should be zero or one iloc boxes per ISO 14496-12:2015 § 8.11.4.1"));
                }
                primary_item_id = Some(read_pitm(&mut b, options)?);
            },
            BoxType::ImageReferenceBox => {
                item_references.append(&mut read_iref(&mut b, options)?)?;
            },
            BoxType::ImagePropertiesBox => {
                properties = read_iprp(&mut b, options)?;
            },
            BoxType::ItemDataBox => {
                if idat.is_some() {
                    return Err(Error::InvalidData("There should be zero or one idat boxes"));
                }
                idat = Some(b.read_into_try_vec()?);
            },
            _ => skip_box_content(&mut b)?,
        }

        check_parser_state(&b.head, &b.content)?;
    }

    let primary_item_id = primary_item_id.ok_or(Error::InvalidData("Required pitm box not present in meta box"))?;

    let item_infos = item_infos.ok_or(Error::InvalidData("iinf missing"))?;

    if let Some(item_info) = item_infos.iter().find(|x| x.item_id == primary_item_id) {
        // Allow both "av01" (standard single-frame) and "grid" (tiled) types
        if item_info.item_type != b"av01" && item_info.item_type != b"grid" {
            warn!("primary_item_id type: {}", item_info.item_type);
            return Err(Error::InvalidData("primary_item_id type is not av01 or grid"));
        }
    } else {
        return Err(Error::InvalidData("primary_item_id not present in iinf box"));
    }

    Ok(AvifInternalMeta {
        properties,
        item_references,
        primary_item_id,
        iloc_items: iloc_items.ok_or(Error::InvalidData("iloc missing"))?,
        item_infos,
        idat,
    })
}

/// Parse a Primary Item Box
/// See ISO 14496-12:2015 § 8.11.4
fn read_pitm<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<u32> {
    let version = read_fullbox_version_no_flags(src, options)?;

    let item_id = match version {
        0 => be_u16(src)?.into(),
        1 => be_u32(src)?,
        _ => return Err(Error::Unsupported("unsupported pitm version")),
    };

    Ok(item_id)
}

/// Parse an Item Information Box
/// See ISO 14496-12:2015 § 8.11.6
fn read_iinf<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<TryVec<ItemInfoEntry>> {
    let version = read_fullbox_version_no_flags(src, options)?;

    match version {
        0 | 1 => (),
        _ => return Err(Error::Unsupported("unsupported iinf version")),
    }

    let entry_count = if version == 0 {
        be_u16(src)?.to_usize()
    } else {
        be_u32(src)?.to_usize()
    };
    let mut item_infos = TryVec::with_capacity(entry_count)?;

    let mut iter = src.box_iter();
    while let Some(mut b) = iter.next_box()? {
        if b.head.name != BoxType::ItemInfoEntry {
            return Err(Error::InvalidData("iinf box should contain only infe boxes"));
        }

        item_infos.push(read_infe(&mut b)?)?;

        check_parser_state(&b.head, &b.content)?;
    }

    Ok(item_infos)
}

/// Parse an Item Info Entry
/// See ISO 14496-12:2015 § 8.11.6.2
fn read_infe<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<ItemInfoEntry> {
    // According to the standard, it seems the flags field should be 0, but
    // at least one sample AVIF image has a nonzero value.
    let (version, _) = read_fullbox_extra(src)?;

    // mif1 brand (see ISO 23008-12:2017 § 10.2.1) only requires v2 and 3
    let item_id = match version {
        2 => be_u16(src)?.into(),
        3 => be_u32(src)?,
        _ => return Err(Error::Unsupported("unsupported version in 'infe' box")),
    };

    let item_protection_index = be_u16(src)?;

    if item_protection_index != 0 {
        return Err(Error::Unsupported("protected items (infe.item_protection_index != 0) are not supported"));
    }

    let item_type = FourCC::from(be_u32(src)?);
    debug!("infe item_id {item_id} item_type: {item_type}");

    // There are some additional fields here, but they're not of interest to us
    skip_box_remain(src)?;

    Ok(ItemInfoEntry { item_id, item_type })
}

fn read_iref<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<TryVec<SingleItemTypeReferenceBox>> {
    let mut item_references = TryVec::new();
    let version = read_fullbox_version_no_flags(src, options)?;
    if version > 1 {
        return Err(Error::Unsupported("iref version"));
    }

    let mut iter = src.box_iter();
    while let Some(mut b) = iter.next_box()? {
        let from_item_id = if version == 0 {
            be_u16(&mut b)?.into()
        } else {
            be_u32(&mut b)?
        };
        let reference_count = be_u16(&mut b)?;
        for reference_index in 0..reference_count {
            let to_item_id = if version == 0 {
                be_u16(&mut b)?.into()
            } else {
                be_u32(&mut b)?
            };
            if from_item_id == to_item_id {
                return Err(Error::InvalidData("from_item_id and to_item_id must be different"));
            }
            item_references.push(SingleItemTypeReferenceBox {
                item_type: b.head.name.into(),
                from_item_id,
                to_item_id,
                reference_index,
            })?;
        }
        check_parser_state(&b.head, &b.content)?;
    }
    Ok(item_references)
}

fn read_iprp<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<TryVec<AssociatedProperty>> {
    let mut iter = src.box_iter();
    let mut properties = TryVec::new();
    let mut associations = TryVec::new();

    while let Some(mut b) = iter.next_box()? {
        match b.head.name {
            BoxType::ItemPropertyContainerBox => {
                properties = read_ipco(&mut b, options)?;
            },
            BoxType::ItemPropertyAssociationBox => {
                associations = read_ipma(&mut b)?;
            },
            _ => return Err(Error::InvalidData("unexpected ipco child")),
        }
    }

    let mut associated = TryVec::new();
    for a in associations {
        let index = match a.property_index {
            0 => continue,
            x => x as usize - 1,
        };
        if let Some(prop) = properties.get(index) {
            if *prop != ItemProperty::Unsupported {
                associated.push(AssociatedProperty {
                    item_id: a.item_id,
                    property: prop.try_clone()?,
                })?;
            }
        }
    }
    Ok(associated)
}

/// Image spatial extents (dimensions)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageSpatialExtents {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ItemProperty {
    Channels(ArrayVec<u8, 16>),
    AuxiliaryType(AuxiliaryTypeProperty),
    ImageSpatialExtents(ImageSpatialExtents),
    ImageGrid(GridConfig),
    Unsupported,
}

impl TryClone for ItemProperty {
    fn try_clone(&self) -> Result<Self, TryReserveError> {
        Ok(match self {
            Self::Channels(val) => Self::Channels(val.clone()),
            Self::AuxiliaryType(val) => Self::AuxiliaryType(val.try_clone()?),
            Self::ImageSpatialExtents(val) => Self::ImageSpatialExtents(*val),
            Self::ImageGrid(val) => Self::ImageGrid(val.clone()),
            Self::Unsupported => Self::Unsupported,
        })
    }
}

struct Association {
    item_id: u32,
    #[allow(unused)]
    essential: bool,
    property_index: u16,
}

pub(crate) struct AssociatedProperty {
    pub item_id: u32,
    pub property: ItemProperty,
}

fn read_ipma<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<TryVec<Association>> {
    let (version, flags) = read_fullbox_extra(src)?;

    let mut associations = TryVec::new();

    let entry_count = be_u32(src)?;
    for _ in 0..entry_count {
        let item_id = if version == 0 {
            be_u16(src)?.into()
        } else {
            be_u32(src)?
        };
        let association_count = src.read_u8()?;
        for _ in 0..association_count {
            let num_association_bytes = if flags & 1 == 1 { 2 } else { 1 };
            let association = &mut [0; 2][..num_association_bytes];
            src.read_exact(association)?;
            let mut association = BitReader::new(association);
            let essential = association.read_bool()?;
            let property_index = association.read_u16(association.remaining().try_into()?)?;
            associations.push(Association {
                item_id,
                essential,
                property_index,
            })?;
        }
    }
    Ok(associations)
}

fn read_ipco<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<TryVec<ItemProperty>> {
    let mut properties = TryVec::new();

    let mut iter = src.box_iter();
    while let Some(mut b) = iter.next_box()? {
        // Must push for every property to have correct index for them
        let prop = match b.head.name {
            BoxType::PixelInformationBox => ItemProperty::Channels(read_pixi(&mut b, options)?),
            BoxType::AuxiliaryTypeProperty => ItemProperty::AuxiliaryType(read_auxc(&mut b, options)?),
            BoxType::ImageSpatialExtentsBox => ItemProperty::ImageSpatialExtents(read_ispe(&mut b, options)?),
            BoxType::ImageGridBox => ItemProperty::ImageGrid(read_grid(&mut b, options)?),
            _ => {
                skip_box_remain(&mut b)?;
                ItemProperty::Unsupported
            },
        };
        properties.push(prop)?;
    }
    Ok(properties)
}

fn read_pixi<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<ArrayVec<u8, 16>> {
    let version = read_fullbox_version_no_flags(src, options)?;
    if version != 0 {
        return Err(Error::Unsupported("pixi version"));
    }

    let num_channels = usize::from(src.read_u8()?);
    let mut channels = ArrayVec::new();
    channels.extend((0..num_channels.min(channels.capacity())).map(|_| 0));
    debug_assert_eq!(num_channels, channels.len());
    src.read_exact(&mut channels).map_err(|_| Error::InvalidData("invalid num_channels"))?;

    // In lenient mode, skip any extra bytes (e.g., extended_pixi.avif has 6 extra bytes)
    if options.lenient && src.bytes_left() > 0 {
        skip(src, src.bytes_left())?;
    }

    check_parser_state(&src.head, &src.content)?;
    Ok(channels)
}

#[derive(Debug, PartialEq)]
struct AuxiliaryTypeProperty {
    aux_data: TryString,
}

impl AuxiliaryTypeProperty {
    #[must_use]
    fn type_subtype(&self) -> (&[u8], &[u8]) {
        let split = self.aux_data.iter().position(|&b| b == b'\0')
            .map(|pos| self.aux_data.split_at(pos));
        if let Some((aux_type, rest)) = split {
            (aux_type, &rest[1..])
        } else {
            (&self.aux_data, &[])
        }
    }
}

impl TryClone for AuxiliaryTypeProperty {
    fn try_clone(&self) -> Result<Self, TryReserveError> {
        Ok(Self {
            aux_data: self.aux_data.try_clone()?,
        })
    }
}

fn read_auxc<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<AuxiliaryTypeProperty> {
    let version = read_fullbox_version_no_flags(src, options)?;
    if version != 0 {
        return Err(Error::Unsupported("auxC version"));
    }

    let aux_data = src.read_into_try_vec()?;

    Ok(AuxiliaryTypeProperty { aux_data })
}

/// Parse an Image Spatial Extents property box
/// See ISO/IEC 23008-12:2017 § 6.5.3
fn read_ispe<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<ImageSpatialExtents> {
    let _version = read_fullbox_version_no_flags(src, options)?;
    // Version is always 0 for ispe

    let width = be_u32(src)?;
    let height = be_u32(src)?;

    // Validate dimensions are non-zero (0×0 images are invalid)
    if width == 0 || height == 0 {
        return Err(Error::InvalidData("ispe dimensions cannot be zero"));
    }

    Ok(ImageSpatialExtents { width, height })
}

/// Parse a Movie Header box (mvhd)
/// See ISO/IEC 14496-12:2015 § 8.2.2
fn read_mvhd<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<MovieHeader> {
    let version = src.read_u8()?;
    let _flags = [src.read_u8()?, src.read_u8()?, src.read_u8()?];

    let (timescale, duration) = if version == 1 {
        let _creation_time = be_u64(src)?;
        let _modification_time = be_u64(src)?;
        let timescale = be_u32(src)?;
        let duration = be_u64(src)?;
        (timescale, duration)
    } else {
        let _creation_time = be_u32(src)?;
        let _modification_time = be_u32(src)?;
        let timescale = be_u32(src)?;
        let duration = be_u32(src)?;
        (timescale, duration as u64)
    };

    // Skip rest of mvhd (rate, volume, matrix, etc.)
    skip_box_remain(src)?;

    Ok(MovieHeader { _timescale: timescale, _duration: duration })
}

/// Parse a Media Header box (mdhd)
/// See ISO/IEC 14496-12:2015 § 8.4.2
fn read_mdhd<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<MediaHeader> {
    let version = src.read_u8()?;
    let _flags = [src.read_u8()?, src.read_u8()?, src.read_u8()?];

    let (timescale, duration) = if version == 1 {
        let _creation_time = be_u64(src)?;
        let _modification_time = be_u64(src)?;
        let timescale = be_u32(src)?;
        let duration = be_u64(src)?;
        (timescale, duration)
    } else {
        let _creation_time = be_u32(src)?;
        let _modification_time = be_u32(src)?;
        let timescale = be_u32(src)?;
        let duration = be_u32(src)?;
        (timescale, duration as u64)
    };

    // Skip language and pre_defined
    skip_box_remain(src)?;

    Ok(MediaHeader { timescale, _duration: duration })
}

/// Parse Time To Sample box (stts)
/// See ISO/IEC 14496-12:2015 § 8.6.1.2
fn read_stts<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<TryVec<TimeToSampleEntry>> {
    let _version = src.read_u8()?;
    let _flags = [src.read_u8()?, src.read_u8()?, src.read_u8()?];
    let entry_count = be_u32(src)?;

    let mut entries = TryVec::new();
    for _ in 0..entry_count {
        entries.push(TimeToSampleEntry {
            sample_count: be_u32(src)?,
            sample_delta: be_u32(src)?,
        })?;
    }

    Ok(entries)
}

/// Parse Sample To Chunk box (stsc)
/// See ISO/IEC 14496-12:2015 § 8.7.4
fn read_stsc<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<TryVec<SampleToChunkEntry>> {
    let _version = src.read_u8()?;
    let _flags = [src.read_u8()?, src.read_u8()?, src.read_u8()?];
    let entry_count = be_u32(src)?;

    let mut entries = TryVec::new();
    for _ in 0..entry_count {
        entries.push(SampleToChunkEntry {
            first_chunk: be_u32(src)?,
            samples_per_chunk: be_u32(src)?,
            _sample_description_index: be_u32(src)?,
        })?;
    }

    Ok(entries)
}

/// Parse Sample Size box (stsz)
/// See ISO/IEC 14496-12:2015 § 8.7.3
fn read_stsz<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<TryVec<u32>> {
    let _version = src.read_u8()?;
    let _flags = [src.read_u8()?, src.read_u8()?, src.read_u8()?];
    let sample_size = be_u32(src)?;
    let sample_count = be_u32(src)?;

    let mut sizes = TryVec::new();
    if sample_size == 0 {
        // Variable sizes - read each one
        for _ in 0..sample_count {
            sizes.push(be_u32(src)?)?;
        }
    } else {
        // Constant size for all samples
        for _ in 0..sample_count {
            sizes.push(sample_size)?;
        }
    }

    Ok(sizes)
}

/// Parse Chunk Offset box (stco or co64)
/// See ISO/IEC 14496-12:2015 § 8.7.5
fn read_chunk_offsets<T: Read>(src: &mut BMFFBox<'_, T>, is_64bit: bool) -> Result<TryVec<u64>> {
    let _version = src.read_u8()?;
    let _flags = [src.read_u8()?, src.read_u8()?, src.read_u8()?];
    let entry_count = be_u32(src)?;

    let mut offsets = TryVec::new();
    for _ in 0..entry_count {
        let offset = if is_64bit {
            be_u64(src)?
        } else {
            be_u32(src)? as u64
        };
        offsets.push(offset)?;
    }

    Ok(offsets)
}

/// Parse Sample Table box (stbl)
/// See ISO/IEC 14496-12:2015 § 8.5
fn read_stbl<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<SampleTable> {
    let mut time_to_sample = TryVec::new();
    let mut sample_to_chunk = TryVec::new();
    let mut sample_sizes = TryVec::new();
    let mut chunk_offsets = TryVec::new();

    let mut iter = src.box_iter();
    while let Some(mut b) = iter.next_box()? {
        match b.head.name {
            BoxType::TimeToSampleBox => {
                time_to_sample = read_stts(&mut b)?;
            }
            BoxType::SampleToChunkBox => {
                sample_to_chunk = read_stsc(&mut b)?;
            }
            BoxType::SampleSizeBox => {
                sample_sizes = read_stsz(&mut b)?;
            }
            BoxType::ChunkOffsetBox => {
                chunk_offsets = read_chunk_offsets(&mut b, false)?;
            }
            BoxType::ChunkLargeOffsetBox => {
                chunk_offsets = read_chunk_offsets(&mut b, true)?;
            }
            _ => {
                skip_box_remain(&mut b)?;
            }
        }
    }

    Ok(SampleTable {
        time_to_sample,
        sample_to_chunk,
        sample_sizes,
        chunk_offsets,
    })
}

/// Parse animation from moov box
/// Returns (media_timescale, sample_table)
fn read_moov<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<Option<(u32, SampleTable)>> {
    let mut media_timescale: Option<u32> = None;
    let mut sample_table: Option<SampleTable> = None;

    let mut iter = src.box_iter();
    while let Some(mut b) = iter.next_box()? {
        match b.head.name {
            BoxType::MovieHeaderBox => {
                let _mvhd = read_mvhd(&mut b)?;
            }
            BoxType::TrackBox => {
                // Parse track recursively
                // Only use first video track, but consume all tracks
                if media_timescale.is_none() {
                    if let Some((timescale, stbl)) = read_trak(&mut b)? {
                        media_timescale = Some(timescale);
                        sample_table = Some(stbl);
                    }
                } else {
                    skip_box_remain(&mut b)?;
                }
            }
            _ => {
                skip_box_remain(&mut b)?;
            }
        }
    }

    if let (Some(timescale), Some(stbl)) = (media_timescale, sample_table) {
        Ok(Some((timescale, stbl)))
    } else {
        Ok(None)
    }
}

/// Parse track box (trak)
/// Returns (media_timescale, sample_table) if this is a valid video track
fn read_trak<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<Option<(u32, SampleTable)>> {
    let mut iter = src.box_iter();
    while let Some(mut b) = iter.next_box()? {
        if b.head.name == BoxType::MediaBox {
            return read_mdia(&mut b);
        } else {
            skip_box_remain(&mut b)?;
        }
    }
    Ok(None)
}

/// Parse media box (mdia)
fn read_mdia<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<Option<(u32, SampleTable)>> {
    let mut media_timescale = 1000; // default
    let mut sample_table: Option<SampleTable> = None;

    let mut iter = src.box_iter();
    while let Some(mut b) = iter.next_box()? {
        match b.head.name {
            BoxType::MediaHeaderBox => {
                let mdhd = read_mdhd(&mut b)?;
                media_timescale = mdhd.timescale;
            }
            BoxType::MediaInformationBox => {
                sample_table = read_minf(&mut b)?;
            }
            _ => {
                skip_box_remain(&mut b)?;
            }
        }
    }

    if let Some(stbl) = sample_table {
        Ok(Some((media_timescale, stbl)))
    } else {
        Ok(None)
    }
}

/// Parse media information box (minf)
fn read_minf<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<Option<SampleTable>> {
    let mut iter = src.box_iter();
    while let Some(mut b) = iter.next_box()? {
        if b.head.name == BoxType::SampleTableBox {
            return Ok(Some(read_stbl(&mut b)?));
        } else {
            skip_box_remain(&mut b)?;
        }
    }
    Ok(None)
}

/// Extract animation frames using sample table
#[allow(deprecated)]
fn extract_animation_frames(
    sample_table: &SampleTable,
    media_timescale: u32,
    mdats: &mut [MediaDataBox],
) -> Result<TryVec<AnimationFrame>> {
    let mut frames = TryVec::new();

    // Build sample-to-chunk mapping (expand into per-sample info)
    let mut sample_to_chunk_map = TryVec::new();
    for (i, entry) in sample_table.sample_to_chunk.iter().enumerate() {
        let next_first_chunk = sample_table
            .sample_to_chunk
            .get(i + 1)
            .map(|e| e.first_chunk)
            .unwrap_or(u32::MAX);

        for chunk_idx in entry.first_chunk..next_first_chunk {
            if chunk_idx > sample_table.chunk_offsets.len() as u32 {
                break;
            }
            sample_to_chunk_map.push((chunk_idx, entry.samples_per_chunk))?;
        }
    }

    // Calculate frame durations from time-to-sample
    let mut frame_durations = TryVec::new();
    for entry in &sample_table.time_to_sample {
        for _ in 0..entry.sample_count {
            // Convert from media timescale to milliseconds
            let duration_ms = if media_timescale > 0 {
                ((entry.sample_delta as u64) * 1000) / (media_timescale as u64)
            } else {
                0
            };
            frame_durations.push(duration_ms as u32)?;
        }
    }

    // Extract each frame
    let sample_count = sample_table.sample_sizes.len();
    let mut current_sample = 0;

    for (chunk_idx_1based, samples_in_chunk) in &sample_to_chunk_map {
        let chunk_idx = (*chunk_idx_1based as usize).saturating_sub(1);
        if chunk_idx >= sample_table.chunk_offsets.len() {
            continue;
        }

        let chunk_offset = sample_table.chunk_offsets[chunk_idx];

        for sample_in_chunk in 0..*samples_in_chunk {
            if current_sample >= sample_count {
                break;
            }

            let sample_size = sample_table.sample_sizes[current_sample];
            let duration_ms = frame_durations.get(current_sample).copied().unwrap_or(0);

            // Calculate offset within chunk
            let mut offset_in_chunk = 0u64;
            for s in 0..sample_in_chunk {
                let prev_sample = current_sample.saturating_sub((sample_in_chunk - s) as usize);
                if prev_sample < sample_count {
                    offset_in_chunk += sample_table.sample_sizes[prev_sample] as u64;
                }
            }

            let sample_offset = chunk_offset + offset_in_chunk;

            // Extract frame data from mdat
            let mut frame_data = TryVec::new();
            let mut found = false;

            for mdat in mdats.iter_mut() {
                let range = ExtentRange::WithLength(Range {
                    start: sample_offset,
                    end: sample_offset + sample_size as u64,
                });

                if mdat.contains_extent(&range) {
                    mdat.read_extent(&range, &mut frame_data)?;
                    found = true;
                    break;
                }
            }

            if !found {
                log::warn!("Animation frame {} not found in mdat", current_sample);
            }

            frames.push(AnimationFrame {
                data: frame_data,
                duration_ms,
            })?;

            current_sample += 1;
        }
    }

    Ok(frames)
}

/// Parse an ImageGrid property box
/// See ISO/IEC 23008-12:2017 § 6.6.2.3
fn read_grid<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<GridConfig> {
    let version = read_fullbox_version_no_flags(src, options)?;
    if version > 0 {
        return Err(Error::Unsupported("grid version > 0"));
    }

    let flags_byte = src.read_u8()?;
    let rows = src.read_u8()?;
    let columns = src.read_u8()?;

    // flags & 1 determines field size: 0 = 16-bit, 1 = 32-bit
    let (output_width, output_height) = if flags_byte & 1 == 0 {
        // 16-bit fields
        (u32::from(be_u16(src)?), u32::from(be_u16(src)?))
    } else {
        // 32-bit fields
        (be_u32(src)?, be_u32(src)?)
    };

    Ok(GridConfig {
        rows,
        columns,
        output_width,
        output_height,
    })
}

/// Parse an item location box inside a meta box
/// See ISO 14496-12:2015 § 8.11.3
fn read_iloc<T: Read>(src: &mut BMFFBox<'_, T>, options: &ParseOptions) -> Result<TryVec<ItemLocationBoxItem>> {
    let version: IlocVersion = read_fullbox_version_no_flags(src, options)?.try_into()?;

    let iloc = src.read_into_try_vec()?;
    let mut iloc = BitReader::new(&iloc);

    let offset_size: IlocFieldSize = iloc.read_u8(4)?.try_into()?;
    let length_size: IlocFieldSize = iloc.read_u8(4)?.try_into()?;
    let base_offset_size: IlocFieldSize = iloc.read_u8(4)?.try_into()?;

    let index_size: Option<IlocFieldSize> = match version {
        IlocVersion::One | IlocVersion::Two => Some(iloc.read_u8(4)?.try_into()?),
        IlocVersion::Zero => {
            let _reserved = iloc.read_u8(4)?;
            None
        },
    };

    let item_count = match version {
        IlocVersion::Zero | IlocVersion::One => iloc.read_u32(16)?,
        IlocVersion::Two => iloc.read_u32(32)?,
    };

    let mut items = TryVec::with_capacity(item_count.to_usize())?;

    for _ in 0..item_count {
        let item_id = match version {
            IlocVersion::Zero | IlocVersion::One => iloc.read_u32(16)?,
            IlocVersion::Two => iloc.read_u32(32)?,
        };

        // The spec isn't entirely clear how an `iloc` should be interpreted for version 0,
        // which has no `construction_method` field. It does say:
        // "For maximum compatibility, version 0 of this box should be used in preference to
        //  version 1 with `construction_method==0`, or version 2 when possible."
        // We take this to imply version 0 can be interpreted as using file offsets.
        let construction_method = match version {
            IlocVersion::Zero => ConstructionMethod::File,
            IlocVersion::One | IlocVersion::Two => {
                let _reserved = iloc.read_u16(12)?;
                match iloc.read_u16(4)? {
                    0 => ConstructionMethod::File,
                    1 => ConstructionMethod::Idat,
                    2 => return Err(Error::Unsupported("construction_method 'item_offset' is not supported")),
                    _ => return Err(Error::InvalidData("construction_method is taken from the set 0, 1 or 2 per ISO 14496-12:2015 § 8.11.3.3")),
                }
            },
        };

        let data_reference_index = iloc.read_u16(16)?;

        if data_reference_index != 0 {
            return Err(Error::Unsupported("external file references (iloc.data_reference_index != 0) are not supported"));
        }

        let base_offset = iloc.read_u64(base_offset_size.to_bits())?;
        let extent_count = iloc.read_u16(16)?;

        if extent_count < 1 {
            return Err(Error::InvalidData("extent_count must have a value 1 or greater per ISO 14496-12:2015 § 8.11.3.3"));
        }

        let mut extents = TryVec::with_capacity(extent_count.to_usize())?;

        for _ in 0..extent_count {
            // Parsed but currently ignored, see `ItemLocationBoxExtent`
            let _extent_index = match &index_size {
                None | Some(IlocFieldSize::Zero) => None,
                Some(index_size) => {
                    debug_assert!(version == IlocVersion::One || version == IlocVersion::Two);
                    Some(iloc.read_u64(index_size.to_bits())?)
                },
            };

            // Per ISO 14496-12:2015 § 8.11.3.1:
            // "If the offset is not identified (the field has a length of zero), then the
            //  beginning of the source (offset 0) is implied"
            // This behavior will follow from BitReader::read_u64(0) -> 0.
            let extent_offset = iloc.read_u64(offset_size.to_bits())?;
            let extent_length = iloc.read_u64(length_size.to_bits())?;

            // "If the length is not specified, or specified as zero, then the entire length of
            //  the source is implied" (ibid)
            let start = base_offset
                .checked_add(extent_offset)
                .ok_or(Error::InvalidData("offset calculation overflow"))?;
            let extent_range = if extent_length == 0 {
                ExtentRange::ToEnd(RangeFrom { start })
            } else {
                let end = start
                    .checked_add(extent_length)
                    .ok_or(Error::InvalidData("end calculation overflow"))?;
                ExtentRange::WithLength(Range { start, end })
            };

            extents.push(ItemLocationBoxExtent { extent_range })?;
        }

        items.push(ItemLocationBoxItem { item_id, construction_method, extents })?;
    }

    if iloc.remaining() == 0 {
        Ok(items)
    } else {
        Err(Error::InvalidData("invalid iloc size"))
    }
}

/// Parse an ftyp box.
/// See ISO 14496-12:2015 § 4.3
fn read_ftyp<T: Read>(src: &mut BMFFBox<'_, T>) -> Result<FileTypeBox> {
    let major = be_u32(src)?;
    let minor = be_u32(src)?;
    let bytes_left = src.bytes_left();
    if bytes_left % 4 != 0 {
        return Err(Error::InvalidData("invalid ftyp size"));
    }
    // Is a brand_count of zero valid?
    let brand_count = bytes_left / 4;
    let mut brands = TryVec::with_capacity(brand_count.try_into()?)?;
    for _ in 0..brand_count {
        brands.push(be_u32(src)?.into())?;
    }
    Ok(FileTypeBox {
        major_brand: From::from(major),
        minor_version: minor,
        compatible_brands: brands,
    })
}

#[cfg_attr(debug_assertions, track_caller)]
fn check_parser_state<T>(header: &BoxHeader, left: &Take<T>) -> Result<(), Error> {
    let limit = left.limit();
    // Allow fully consumed boxes, or size=0 boxes (where original size was u64::MAX)
    if limit == 0 || header.size == u64::MAX {
        Ok(())
    } else {
        debug_assert_eq!(0, limit, "bad parser state bytes left");
        Err(Error::InvalidData("unread box content or bad parser sync"))
    }
}

/// Skip a number of bytes that we don't care to parse.
fn skip<T: Read>(src: &mut T, bytes: u64) -> Result<()> {
    std::io::copy(&mut src.take(bytes), &mut std::io::sink())?;
    Ok(())
}

fn be_u16<T: ReadBytesExt>(src: &mut T) -> Result<u16> {
    src.read_u16::<byteorder::BigEndian>().map_err(From::from)
}

fn be_u32<T: ReadBytesExt>(src: &mut T) -> Result<u32> {
    src.read_u32::<byteorder::BigEndian>().map_err(From::from)
}

fn be_u64<T: ReadBytesExt>(src: &mut T) -> Result<u64> {
    src.read_u64::<byteorder::BigEndian>().map_err(From::from)
}

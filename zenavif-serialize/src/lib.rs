//! # AVIF image serializer (muxer)
//!
//! ## Usage
//!
//! 1. Compress pixels using an AV1 encoder, such as [rav1e](https://lib.rs/rav1e). [libaom](https://lib.rs/libaom-sys) works too.
//!
//! 2. Call `avif_serialize::serialize_to_vec(av1_data, None, width, height, 8)`
//!
//! See [cavif](https://github.com/kornelski/cavif-rs) for a complete implementation.

pub mod animated;
mod boxes;
pub mod constants;
pub mod grid;
mod writer;

use crate::boxes::*;
use arrayvec::ArrayVec;
use std::io;

// Re-export box types needed by the public API
pub use crate::boxes::{Av1CBox, ClapBox, ClliBox, ColrBox, ColrIccBox, IrotBox, ImirBox, MdcvBox, PaspBox};

/// Chroma subsampling configuration for AV1 encoding.
///
/// `(false, false)` = 4:4:4 (no subsampling).
/// `(true, true)` = 4:2:0 (both axes subsampled).
/// `(true, false)` = 4:2:2 (horizontal only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromaSubsampling {
    /// Whether the horizontal (X) axis is subsampled.
    pub horizontal: bool,
    /// Whether the vertical (Y) axis is subsampled.
    pub vertical: bool,
}

impl ChromaSubsampling {
    /// 4:4:4 — no chroma subsampling.
    pub const NONE: Self = Self { horizontal: false, vertical: false };
    /// 4:2:0 — both axes subsampled.
    pub const YUV420: Self = Self { horizontal: true, vertical: true };
    /// 4:2:2 — horizontal subsampling only.
    pub const YUV422: Self = Self { horizontal: true, vertical: false };
}

impl From<(bool, bool)> for ChromaSubsampling {
    fn from((h, v): (bool, bool)) -> Self {
        Self { horizontal: h, vertical: v }
    }
}

impl From<ChromaSubsampling> for (bool, bool) {
    fn from(cs: ChromaSubsampling) -> Self {
        (cs.horizontal, cs.vertical)
    }
}

/// Config for the serialization (allows setting advanced image properties).
///
/// See [`Aviffy::new`].
pub struct Aviffy {
    premultiplied_alpha: bool,
    colr: ColrBox,
    clli: Option<ClliBox>,
    mdcv: Option<MdcvBox>,
    irot: Option<IrotBox>,
    imir: Option<ImirBox>,
    clap: Option<ClapBox>,
    pasp: Option<PaspBox>,
    icc_profile: Option<Vec<u8>>,
    min_seq_profile: u8,
    chroma_subsampling: ChromaSubsampling,
    monochrome: bool,
    width: u32,
    height: u32,
    bit_depth: u8,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
    gain_map: Option<GainMapConfig>,
}

/// Configuration for an ISO 21496-1 gain map embedded in the AVIF container.
///
/// The gain map enables adaptive HDR rendering: an SDR base image combined
/// with a gain map and tone-map metadata allows reconstructing an HDR rendition.
struct GainMapConfig {
    /// AV1-encoded gain map image data.
    av1_data: Vec<u8>,
    /// Width of the gain map image in pixels.
    width: u32,
    /// Height of the gain map image in pixels.
    height: u32,
    /// Bit depth of the gain map image (8, 10, or 12).
    bit_depth: u8,
    /// ISO 21496-1 binary metadata blob (the `tmap` item payload).
    metadata: Vec<u8>,
    /// CICP color information for the alternate (typically HDR) rendition.
    /// Stored as a `colr` property on the `tmap` item.
    alt_colr: Option<ColrBox>,
    /// Chroma subsampling of the gain map AV1 data.
    chroma_subsampling: ChromaSubsampling,
    /// Whether the gain map is monochrome.
    monochrome: bool,
}

/// Makes an AVIF file given encoded AV1 data (create the data with [`rav1e`](https://lib.rs/rav1e))
///
/// `color_av1_data` is already-encoded AV1 image data for the color channels (YUV, RGB, etc.).
/// [You can parse this information out of AV1 payload with `avif-parse`](https://docs.rs/zenavif-parse/latest/zenavif_parse/struct.AV1Metadata.html).
///
/// The color image should have been encoded without chroma subsampling AKA YUV444 (`Cs444` in `rav1e`)
/// AV1 handles full-res color so effortlessly, you should never need chroma subsampling ever again.
///
/// Optional `alpha_av1_data` is a monochrome image (`rav1e` calls it "YUV400"/`Cs400`) representing transparency.
/// Alpha adds a lot of header bloat, so don't specify it unless it's necessary.
///
/// `width`/`height` is image size in pixels. It must of course match the size of encoded image data.
/// `depth_bits` should be 8, 10 or 12, depending on how the image was encoded.
///
/// Color and alpha must have the same dimensions and depth.
///
/// Data is written (streamed) to `into_output`.
pub fn serialize<W: io::Write>(into_output: W, color_av1_data: &[u8], alpha_av1_data: Option<&[u8]>, width: u32, height: u32, depth_bits: u8) -> io::Result<()> {
    Aviffy::new()
        .set_width(width)
        .set_height(height)
        .set_bit_depth(depth_bits)
        .write_slice(into_output, color_av1_data, alpha_av1_data)
}

impl Default for Aviffy {
    fn default() -> Self {
        Self::new()
    }
}

impl Aviffy {
    /// You will have to set image properties to match the AV1 bitstream.
    ///
    /// [You can get this information out of the AV1 payload with `avif-parse`](https://docs.rs/zenavif-parse/latest/zenavif_parse/struct.AV1Metadata.html).
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            premultiplied_alpha: false,
            min_seq_profile: 1,
            chroma_subsampling: ChromaSubsampling::NONE,
            monochrome: false,
            width: 0,
            height: 0,
            bit_depth: 0,
            colr: ColrBox::default(),
            clli: None,
            mdcv: None,
            irot: None,
            imir: None,
            clap: None,
            pasp: None,
            icc_profile: None,
            exif: None,
            xmp: None,
            gain_map: None,
        }
    }

    /// If set, must match the AV1 color payload, and will result in `colr` box added to AVIF.
    /// Defaults to BT.601, because that's what Safari assumes when `colr` is missing.
    /// Other browsers are smart enough to read this from the AV1 payload instead.
    #[inline]
    pub fn set_matrix_coefficients(&mut self, matrix_coefficients: constants::MatrixCoefficients) -> &mut Self {
        self.colr.matrix_coefficients = matrix_coefficients;
        self
    }

    #[doc(hidden)]
    pub fn matrix_coefficients(&mut self, matrix_coefficients: constants::MatrixCoefficients) -> &mut Self {
        self.set_matrix_coefficients(matrix_coefficients)
    }

    /// If set, must match the AV1 color payload, and will result in `colr` box added to AVIF.
    /// Defaults to sRGB.
    #[inline]
    pub fn set_transfer_characteristics(&mut self, transfer_characteristics: constants::TransferCharacteristics) -> &mut Self {
        self.colr.transfer_characteristics = transfer_characteristics;
        self
    }

    #[doc(hidden)]
    pub fn transfer_characteristics(&mut self, transfer_characteristics: constants::TransferCharacteristics) -> &mut Self {
        self.set_transfer_characteristics(transfer_characteristics)
    }

    /// If set, must match the AV1 color payload, and will result in `colr` box added to AVIF.
    /// Defaults to sRGB/Rec.709.
    #[inline]
    pub fn set_color_primaries(&mut self, color_primaries: constants::ColorPrimaries) -> &mut Self {
        self.colr.color_primaries = color_primaries;
        self
    }

    #[doc(hidden)]
    pub fn color_primaries(&mut self, color_primaries: constants::ColorPrimaries) -> &mut Self {
        self.set_color_primaries(color_primaries)
    }

    /// If set, must match the AV1 color payload, and will result in `colr` box added to AVIF.
    /// Defaults to full.
    #[inline]
    pub fn set_full_color_range(&mut self, full_range: bool) -> &mut Self {
        self.colr.full_range_flag = full_range;
        self
    }

    #[doc(hidden)]
    pub fn full_color_range(&mut self, full_range: bool) -> &mut Self {
        self.set_full_color_range(full_range)
    }

    /// Set Content Light Level Information for HDR (CEA-861.3).
    ///
    /// `max_content_light_level` (MaxCLL) is the maximum light level of any single pixel in cd/m².
    /// `max_pic_average_light_level` (MaxFALL) is the maximum frame-average light level in cd/m².
    ///
    /// Adds a `clli` property box to the AVIF container.
    #[inline]
    pub fn set_content_light_level(&mut self, max_content_light_level: u16, max_pic_average_light_level: u16) -> &mut Self {
        self.clli = Some(ClliBox {
            max_content_light_level,
            max_pic_average_light_level,
        });
        self
    }

    /// Set Mastering Display Colour Volume for HDR (SMPTE ST 2086).
    ///
    /// `primaries` are the display primaries in CIE 1931 xy × 50000.
    /// Order: \[green, blue, red\] per SMPTE ST 2086.
    /// `white_point` uses the same encoding (e.g. D65 = (15635, 16450)).
    ///
    /// `max_luminance` and `min_luminance` are in cd/m² × 10000
    /// (e.g. 1000 cd/m² = 10_000_000, 0.005 cd/m² = 50).
    ///
    /// Adds an `mdcv` property box to the AVIF container.
    #[inline]
    pub fn set_mastering_display(&mut self, primaries: [(u16, u16); 3], white_point: (u16, u16), max_luminance: u32, min_luminance: u32) -> &mut Self {
        self.mdcv = Some(MdcvBox {
            primaries,
            white_point,
            max_luminance,
            min_luminance,
        });
        self
    }

    /// Set image rotation (counter-clockwise).
    ///
    /// `angle` is a raw code: 0=0°, 1=90°, 2=180°, 3=270°.
    /// Adds an `irot` property box.
    #[inline]
    pub fn set_rotation(&mut self, angle: u8) -> &mut Self {
        self.irot = Some(IrotBox { angle: angle & 0x03 });
        self
    }

    /// Set image mirror axis.
    ///
    /// `axis` = 0: vertical axis (left-right flip).
    /// `axis` = 1: horizontal axis (top-bottom flip).
    /// Adds an `imir` property box.
    #[inline]
    pub fn set_mirror(&mut self, axis: u8) -> &mut Self {
        self.imir = Some(ImirBox { axis: axis & 0x01 });
        self
    }

    /// Set clean aperture (crop rectangle).
    ///
    /// All values are rational numbers. Defines a centered crop region.
    /// Adds a `clap` property box.
    #[inline]
    pub fn set_clean_aperture(&mut self, clap: ClapBox) -> &mut Self {
        self.clap = Some(clap);
        self
    }

    /// Set pixel aspect ratio.
    ///
    /// `h_spacing` / `v_spacing` defines the ratio. For square pixels use (1, 1).
    /// Adds a `pasp` property box.
    #[inline]
    pub fn set_pixel_aspect_ratio(&mut self, h_spacing: u32, v_spacing: u32) -> &mut Self {
        self.pasp = Some(PaspBox { h_spacing, v_spacing });
        self
    }

    /// Set XMP metadata to be included in the AVIF file as a separate item.
    ///
    /// The data should be a valid XMP/RDF XML document.
    #[inline]
    pub fn set_xmp(&mut self, xmp: Vec<u8>) -> &mut Self {
        self.xmp = Some(xmp);
        self
    }

    /// Set ICC color profile to embed in the AVIF file.
    ///
    /// This adds a `colr` box with colour_type='prof'. When set, this
    /// replaces the nclx `colr` box (they are mutually exclusive per spec,
    /// though some files include both).
    #[inline]
    pub fn set_icc_profile(&mut self, icc_data: Vec<u8>) -> &mut Self {
        self.icc_profile = Some(icc_data);
        self
    }

    /// Set gain map data for ISO 21496-1 tone mapping.
    ///
    /// Creates three items in the AVIF container:
    /// - A gain map image item (`av01`) containing the AV1-encoded gain map
    /// - A `tmap` derived image item referencing both the primary image and gain map
    /// - The `tmap` item's payload containing the ISO 21496-1 metadata
    ///
    /// The `metadata` blob is the raw ISO 21496-1 binary format (version byte,
    /// minimum_version, writer_version, flags, headroom, per-channel parameters).
    ///
    /// The gain map image is typically a lower-resolution, monochrome or RGB image
    /// encoding the per-pixel gain needed to reconstruct the HDR rendition from
    /// the SDR base.
    #[inline]
    pub fn set_gain_map(&mut self, av1_data: Vec<u8>, width: u32, height: u32, bit_depth: u8, metadata: Vec<u8>) -> &mut Self {
        self.gain_map = Some(GainMapConfig {
            av1_data,
            width,
            height,
            bit_depth,
            metadata,
            alt_colr: None,
            chroma_subsampling: ChromaSubsampling::YUV420,
            monochrome: false,
        });
        self
    }

    /// Set the color information for the alternate (typically HDR) rendition.
    ///
    /// This CICP colr box is attached as a property of the `tmap` item,
    /// telling decoders what colour space the tone-mapped output targets.
    /// Only meaningful if [`set_gain_map`](Self::set_gain_map) has been called.
    #[inline]
    pub fn set_gain_map_alt_colr(&mut self, colr: ColrBox) -> &mut Self {
        if let Some(ref mut gm) = self.gain_map {
            gm.alt_colr = Some(colr);
        }
        self
    }

    /// Set chroma subsampling for the gain map AV1 data.
    ///
    /// Defaults to 4:2:0 if not called. Only meaningful if
    /// [`set_gain_map`](Self::set_gain_map) has been called.
    #[inline]
    pub fn set_gain_map_chroma_subsampling(&mut self, subsampling: impl Into<ChromaSubsampling>) -> &mut Self {
        if let Some(ref mut gm) = self.gain_map {
            gm.chroma_subsampling = subsampling.into();
        }
        self
    }

    /// Set whether the gain map image is monochrome.
    ///
    /// Defaults to false. Only meaningful if
    /// [`set_gain_map`](Self::set_gain_map) has been called.
    #[inline]
    pub fn set_gain_map_monochrome(&mut self, monochrome: bool) -> &mut Self {
        if let Some(ref mut gm) = self.gain_map {
            gm.monochrome = monochrome;
        }
        self
    }

    /// Makes an AVIF file given encoded AV1 data (create the data with [`rav1e`](https://lib.rs/rav1e))
    ///
    /// `color_av1_data` is already-encoded AV1 image data for the color channels (YUV, RGB, etc.).
    /// The color image should have been encoded without chroma subsampling AKA YUV444 (`Cs444` in `rav1e`)
    /// AV1 handles full-res color so effortlessly, you should never need chroma subsampling ever again.
    ///
    /// Optional `alpha_av1_data` is a monochrome image (`rav1e` calls it "YUV400"/`Cs400`) representing transparency.
    /// Alpha adds a lot of header bloat, so don't specify it unless it's necessary.
    ///
    /// `width`/`height` is image size in pixels. It must of course match the size of encoded image data.
    /// `depth_bits` should be 8, 10 or 12, depending on how the image has been encoded in AV1.
    ///
    /// Color and alpha must have the same dimensions and depth.
    ///
    /// Data is written (streamed) to `into_output`.
    #[inline]
    pub fn write<W: io::Write>(&self, into_output: W, color_av1_data: &[u8], alpha_av1_data: Option<&[u8]>, width: u32, height: u32, depth_bits: u8) -> io::Result<()> {
        self.make_boxes(color_av1_data, alpha_av1_data, width, height, depth_bits)?.write(into_output)
    }

    /// See [`Self::write`]
    #[inline]
    pub fn write_slice<W: io::Write>(&self, into_output: W, color_av1_data: &[u8], alpha_av1_data: Option<&[u8]>) -> io::Result<()> {
        self.make_boxes(color_av1_data, alpha_av1_data, self.width, self.height, self.bit_depth)?.write(into_output)
    }

    fn make_boxes<'data>(&'data self, color_av1_data: &'data [u8], alpha_av1_data: Option<&'data [u8]>, width: u32, height: u32, depth_bits: u8) -> io::Result<AvifFile<'data>> {
        if ![8, 10, 12].contains(&depth_bits) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "depth must be 8/10/12"));
        }

        let mut image_items = ArrayVec::new();
        let mut iloc_items = ArrayVec::new();
        let mut ipma_entries = ArrayVec::new();
        let mut irefs = ArrayVec::new();
        let mut multi_irefs: ArrayVec<IrefMultiEntryBox, 2> = ArrayVec::new();
        let mut ipco = IpcoBox::new();
        let color_image_id: u16 = 1;
        let alpha_image_id: u16 = 2;
        let mut next_item_id: u16 = 3;
        const ESSENTIAL_BIT: u8 = 0x80;
        let color_depth_bits = depth_bits;
        let alpha_depth_bits = depth_bits; // Sadly, the spec requires these to match.

        image_items.push(InfeBox {
            id: color_image_id,
            typ: FourCC(*b"av01"),
            name: "",
            content_type: "",
        });

        let ispe_prop = ipco.push(IpcoProp::Ispe(IspeBox { width, height })).ok_or(io::ErrorKind::InvalidInput)?;

        // This is redundant, but Chrome wants it, and checks that it matches :(
        let av1c_color_prop = ipco.push(IpcoProp::Av1C(Av1CBox {
            seq_profile: self.min_seq_profile.max(if color_depth_bits >= 12 { 2 } else { 0 }),
            seq_level_idx_0: 31,
            seq_tier_0: false,
            high_bitdepth: color_depth_bits >= 10,
            twelve_bit: color_depth_bits >= 12,
            monochrome: self.monochrome,
            chroma_subsampling_x: self.chroma_subsampling.horizontal,
            chroma_subsampling_y: self.chroma_subsampling.vertical,
            chroma_sample_position: 0,
        })).ok_or(io::ErrorKind::InvalidInput)?;

        // Useless bloat
        let pixi_3 = ipco.push(IpcoProp::Pixi(PixiBox {
            channels: 3,
            depth: color_depth_bits,
        })).ok_or(io::ErrorKind::InvalidInput)?;

        let mut ipma = IpmaEntry {
            item_id: color_image_id,
            prop_ids: from_array([ispe_prop, av1c_color_prop | ESSENTIAL_BIT, pixi_3]),
        };

        // ICC profile takes precedence over nclx if both set
        if let Some(ref icc_data) = self.icc_profile {
            let colr_icc_prop = ipco.push(IpcoProp::ColrIcc(ColrIccBox {
                icc_data: icc_data.clone(),
            })).ok_or(io::ErrorKind::InvalidInput)?;
            ipma.prop_ids.push(colr_icc_prop);
        } else if self.colr != ColrBox::default() {
            // Redundant info, already in AV1
            let colr_color_prop = ipco.push(IpcoProp::Colr(self.colr)).ok_or(io::ErrorKind::InvalidInput)?;
            ipma.prop_ids.push(colr_color_prop);
        }

        if let Some(clli) = self.clli {
            let clli_prop = ipco.push(IpcoProp::Clli(clli)).ok_or(io::ErrorKind::InvalidInput)?;
            ipma.prop_ids.push(clli_prop);
        }

        if let Some(mdcv) = self.mdcv {
            let mdcv_prop = ipco.push(IpcoProp::Mdcv(mdcv)).ok_or(io::ErrorKind::InvalidInput)?;
            ipma.prop_ids.push(mdcv_prop);
        }

        if let Some(irot) = self.irot {
            let irot_prop = ipco.push(IpcoProp::Irot(irot)).ok_or(io::ErrorKind::InvalidInput)?;
            ipma.prop_ids.push(irot_prop | ESSENTIAL_BIT);
        }

        if let Some(imir) = self.imir {
            let imir_prop = ipco.push(IpcoProp::Imir(imir)).ok_or(io::ErrorKind::InvalidInput)?;
            ipma.prop_ids.push(imir_prop | ESSENTIAL_BIT);
        }

        if let Some(clap) = self.clap {
            let clap_prop = ipco.push(IpcoProp::Clap(clap)).ok_or(io::ErrorKind::InvalidInput)?;
            ipma.prop_ids.push(clap_prop | ESSENTIAL_BIT);
        }

        if let Some(pasp) = self.pasp {
            let pasp_prop = ipco.push(IpcoProp::Pasp(pasp)).ok_or(io::ErrorKind::InvalidInput)?;
            ipma.prop_ids.push(pasp_prop);
        }

        ipma_entries.push(ipma);

        if let Some(exif_data) = self.exif.as_deref() {
            let exif_id = next_item_id;
            next_item_id += 1;

            image_items.push(InfeBox {
                id: exif_id,
                typ: FourCC(*b"Exif"),
                name: "",
                content_type: "",
            });

            iloc_items.push(IlocItem {
                id: exif_id,
                extents: [IlocExtent { data: exif_data }],
            });

            irefs.push(IrefEntryBox {
                from_id: exif_id,
                to_id: color_image_id,
                typ: FourCC(*b"cdsc"),
            });
        }

        if let Some(xmp_data) = self.xmp.as_deref() {
            let xmp_id = next_item_id;
            next_item_id += 1;

            image_items.push(InfeBox {
                id: xmp_id,
                typ: FourCC(*b"mime"),
                name: "",
                content_type: "application/rdf+xml",
            });

            iloc_items.push(IlocItem {
                id: xmp_id,
                extents: [IlocExtent { data: xmp_data }],
            });

            irefs.push(IrefEntryBox {
                from_id: xmp_id,
                to_id: color_image_id,
                typ: FourCC(*b"cdsc"),
            });
        }

        if let Some(alpha_data) = alpha_av1_data {
            image_items.push(InfeBox {
                id: alpha_image_id,
                typ: FourCC(*b"av01"),
                name: "",
                content_type: "",
            });

            irefs.push(IrefEntryBox {
                from_id: alpha_image_id,
                to_id: color_image_id,
                typ: FourCC(*b"auxl"),
            });

            if self.premultiplied_alpha {
                irefs.push(IrefEntryBox {
                    from_id: color_image_id,
                    to_id: alpha_image_id,
                    typ: FourCC(*b"prem"),
                });
            }

            let av1c_alpha_prop = ipco.push(boxes::IpcoProp::Av1C(Av1CBox {
                seq_profile: if alpha_depth_bits >= 12 { 2 } else { 0 },
                seq_level_idx_0: 31,
                seq_tier_0: false,
                high_bitdepth: alpha_depth_bits >= 10,
                twelve_bit: alpha_depth_bits >= 12,
                monochrome: true,
                chroma_subsampling_x: true,
                chroma_subsampling_y: true,
                chroma_sample_position: 0,
            })).ok_or(io::ErrorKind::InvalidInput)?;

            // So pointless
            let pixi_1 = ipco.push(IpcoProp::Pixi(PixiBox {
                channels: 1,
                depth: alpha_depth_bits,
            })).ok_or(io::ErrorKind::InvalidInput)?;

            // that's a silly way to add 1 bit of information, isn't it?
            let auxc_prop = ipco.push(IpcoProp::AuxC(AuxCBox {
                urn: "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha",
            })).ok_or(io::ErrorKind::InvalidInput)?;

            ipma_entries.push(IpmaEntry {
                item_id: alpha_image_id,
                prop_ids: from_array([ispe_prop, av1c_alpha_prop | ESSENTIAL_BIT, auxc_prop, pixi_1]),
            });

            // Use interleaved color and alpha, with alpha first.
            // Makes it possible to display partial image.
            iloc_items.push(IlocItem {
                id: alpha_image_id,
                extents: [IlocExtent { data: alpha_data }],
            });
        }
        // Gain map: gain map image item (av01) + tmap derived image
        if let Some(ref gm) = self.gain_map {
            let gm_depth = gm.bit_depth;
            let gain_map_id = next_item_id;
            next_item_id += 1;
            let tmap_id = next_item_id;
            next_item_id += 1;
            let _ = next_item_id;

            // Gain map image item (av01)
            image_items.push(InfeBox {
                id: gain_map_id,
                typ: FourCC(*b"av01"),
                name: "",
                content_type: "",
            });

            // Gain map ispe (may differ from primary dimensions)
            let gm_ispe = ipco.push(IpcoProp::Ispe(IspeBox {
                width: gm.width,
                height: gm.height,
            })).ok_or(io::ErrorKind::InvalidInput)?;

            // Gain map av1C
            let gm_av1c = ipco.push(IpcoProp::Av1C(Av1CBox {
                seq_profile: if gm_depth >= 12 { 2 } else { 0 },
                seq_level_idx_0: 31,
                seq_tier_0: false,
                high_bitdepth: gm_depth >= 10,
                twelve_bit: gm_depth >= 12,
                monochrome: gm.monochrome,
                chroma_subsampling_x: gm.chroma_subsampling.horizontal,
                chroma_subsampling_y: gm.chroma_subsampling.vertical,
                chroma_sample_position: 0,
            })).ok_or(io::ErrorKind::InvalidInput)?;

            ipma_entries.push(IpmaEntry {
                item_id: gain_map_id,
                prop_ids: from_array([gm_ispe, gm_av1c | ESSENTIAL_BIT]),
            });

            // Gain map image data in iloc
            iloc_items.push(IlocItem {
                id: gain_map_id,
                extents: [IlocExtent { data: &gm.av1_data }],
            });

            // tmap derived image item
            image_items.push(InfeBox {
                id: tmap_id,
                typ: FourCC(*b"tmap"),
                name: "",
                content_type: "",
            });

            // tmap item properties: optional colr for alternate rendition
            if let Some(alt_colr) = gm.alt_colr {
                let tmap_colr = ipco.push(IpcoProp::Colr(alt_colr)).ok_or(io::ErrorKind::InvalidInput)?;
                ipma_entries.push(IpmaEntry {
                    item_id: tmap_id,
                    prop_ids: from_array([tmap_colr]),
                });
            }

            // tmap payload (ISO 21496-1 metadata) in iloc
            iloc_items.push(IlocItem {
                id: tmap_id,
                extents: [IlocExtent { data: &gm.metadata }],
            });

            // dimg reference: tmap -> [primary, gain_map]
            // Must be a single iref entry with reference_count=2 so the parser
            // assigns reference_index 0 to primary and 1 to gain_map.
            let mut to_ids = ArrayVec::new();
            to_ids.push(color_image_id);
            to_ids.push(gain_map_id);
            multi_irefs.push(IrefMultiEntryBox {
                from_id: tmap_id,
                to_ids,
                typ: FourCC(*b"dimg"),
            });
        }

        iloc_items.push(IlocItem {
            id: color_image_id,
            extents: [IlocExtent { data: color_av1_data }],
        });

        Ok(AvifFile {
            ftyp: FtypBox {
                major_brand: FourCC(*b"avif"),
                minor_version: 0,
                compatible_brands: [FourCC(*b"mif1"), FourCC(*b"miaf")].into(),
            },
            meta: MetaBox {
                hdlr: HdlrBox {},
                iinf: IinfBox { items: image_items },
                pitm: PitmBox(color_image_id),
                iloc: IlocBox {
                    absolute_offset_start: None,
                    items: iloc_items,
                },
                iprp: IprpBox {
                    ipco,
                    // It's not enough to define these properties,
                    // they must be assigned to the image
                    ipma: IpmaBox { entries: ipma_entries },
                },
                iref: IrefBox { entries: irefs, multi_entries: multi_irefs },
            },
            // Here's the actual data. If HEIF wasn't such a kitchen sink, this
            // would have been the only data this file needs.
            mdat: MdatBox,
        })
    }

    /// Panics if the input arguments were invalid. Use [`Self::write`] to handle the errors.
    #[must_use]
    #[track_caller]
    pub fn to_vec(&self, color_av1_data: &[u8], alpha_av1_data: Option<&[u8]>, width: u32, height: u32, depth_bits: u8) -> Vec<u8> {
        let mut file = self.make_boxes(color_av1_data, alpha_av1_data, width, height, depth_bits).unwrap();
        let mut out = Vec::new();
        file.write_to_vec(&mut out).unwrap();
        out
    }

    /// Set chroma subsampling. Use [`ChromaSubsampling::NONE`] for 4:4:4,
    /// [`ChromaSubsampling::YUV420`] for 4:2:0, etc.
    ///
    /// Also accepts `(bool, bool)` tuples for backward compatibility.
    ///
    /// `chroma_sample_position` is always 0. Don't use chroma subsampling with AVIF.
    #[inline]
    pub fn set_chroma_subsampling(&mut self, subsampling: impl Into<ChromaSubsampling>) -> &mut Self {
        self.chroma_subsampling = subsampling.into();
        self
    }

    /// Set whether the image is monochrome (grayscale).
    /// This is used to set the `monochrome` flag in the AV1 sequence header.
    #[inline]
    pub fn set_monochrome(&mut self, monochrome: bool) -> &mut Self {
        self.monochrome = monochrome;
        self
    }

    /// Set exif metadata to be included in the AVIF file as a separate item.
    #[inline]
    pub fn set_exif(&mut self, exif: Vec<u8>) -> &mut Self {
        self.exif = Some(exif);
        self
    }

    /// Sets minimum required
    ///
    /// Higher bit depth may increase this
    #[inline]
    pub fn set_seq_profile(&mut self, seq_profile: u8) -> &mut Self {
        self.min_seq_profile = seq_profile;
        self
    }

    #[inline]
    pub fn set_width(&mut self, width: u32) -> &mut Self {
        self.width = width;
        self
    }

    #[inline]
    pub fn set_height(&mut self, height: u32) -> &mut Self {
        self.height = height;
        self
    }

    /// 8, 10 or 12.
    #[inline]
    pub fn set_bit_depth(&mut self, bit_depth: u8) -> &mut Self {
        self.bit_depth = bit_depth;
        self
    }

    /// Set whether image's colorspace uses premultiplied alpha, i.e. RGB channels were multiplied by their alpha value,
    /// so that transparent areas are all black. Image decoders will be instructed to undo the premultiplication.
    ///
    /// Premultiplied alpha images usually compress better and tolerate heavier compression, but
    /// may not be supported correctly by less capable AVIF decoders.
    ///
    /// This just sets the configuration property. The pixel data must have already been processed before compression.
    /// If a decoder displays semitransparent colors too dark, it doesn't support premultiplied alpha.
    /// If a decoder displays semitransparent colors too bright, you didn't premultiply the colors before encoding.
    ///
    /// If you're not using premultiplied alpha, consider bleeding RGB colors into transparent areas,
    /// otherwise there may be unwanted outlines around edges of transparency.
    #[inline]
    pub fn set_premultiplied_alpha(&mut self, is_premultiplied: bool) -> &mut Self {
        self.premultiplied_alpha = is_premultiplied;
        self
    }

    #[doc(hidden)]
    pub fn premultiplied_alpha(&mut self, is_premultiplied: bool) -> &mut Self {
        self.set_premultiplied_alpha(is_premultiplied)
    }
}

#[inline(always)]
fn from_array<const L1: usize, const L2: usize, T: Copy>(array: [T; L1]) -> ArrayVec<T, L2> {
    assert!(L1 <= L2);
    let mut tmp = ArrayVec::new_const();
    let _ = tmp.try_extend_from_slice(&array);
    tmp
}

/// See [`serialize`] for description. This one makes a `Vec` instead of using `io::Write`.
#[must_use]
#[track_caller]
pub fn serialize_to_vec(color_av1_data: &[u8], alpha_av1_data: Option<&[u8]>, width: u32, height: u32, depth_bits: u8) -> Vec<u8> {
    Aviffy::new().to_vec(color_av1_data, alpha_av1_data, width, height, depth_bits)
}

#[test]
fn test_roundtrip_parse_mp4() {
    let test_img = b"av12356abc";
    let avif = serialize_to_vec(test_img, None, 10, 20, 8);

    let ctx = mp4parse::read_avif(&mut avif.as_slice(), mp4parse::ParseStrictness::Normal).unwrap();

    assert_eq!(&test_img[..], ctx.primary_item_coded_data().unwrap());
}

#[test]
fn test_roundtrip_parse_mp4_alpha() {
    let test_img = b"av12356abc";
    let test_a = b"alpha";
    let avif = serialize_to_vec(test_img, Some(test_a), 10, 20, 8);

    let ctx = mp4parse::read_avif(&mut avif.as_slice(), mp4parse::ParseStrictness::Normal).unwrap();

    assert_eq!(&test_img[..], ctx.primary_item_coded_data().unwrap());
    assert_eq!(&test_a[..], ctx.alpha_item_coded_data().unwrap());
}

#[test]
fn test_roundtrip_parse_exif() {
    let test_img = b"av12356abc";
    let test_a = b"alpha";
    let avif = Aviffy::new()
        .set_exif(b"lol".to_vec())
        .to_vec(test_img, Some(test_a), 10, 20, 8);

    let ctx = mp4parse::read_avif(&mut avif.as_slice(), mp4parse::ParseStrictness::Normal).unwrap();

    assert_eq!(&test_img[..], ctx.primary_item_coded_data().unwrap());
    assert_eq!(&test_a[..], ctx.alpha_item_coded_data().unwrap());
}

#[test]
fn test_roundtrip_parse_avif() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let test_alpha = [77, 88, 99];
    let avif = serialize_to_vec(&test_img, Some(&test_alpha), 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();

    assert_eq!(&test_img[..], parser.primary_data().unwrap().as_ref());
    assert_eq!(&test_alpha[..], parser.alpha_data().unwrap().unwrap().as_ref());
}

#[test]
fn test_roundtrip_parse_avif_colr() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let test_alpha = [77, 88, 99];
    let avif = Aviffy::new()
        .matrix_coefficients(constants::MatrixCoefficients::Bt709)
        .to_vec(&test_img, Some(&test_alpha), 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();

    assert_eq!(&test_img[..], parser.primary_data().unwrap().as_ref());
    assert_eq!(&test_alpha[..], parser.alpha_data().unwrap().unwrap().as_ref());
}

#[test]
fn premultiplied_flag() {
    let test_img = [1,2,3,4];
    let test_alpha = [55,66,77,88,99];
    let avif = Aviffy::new().premultiplied_alpha(true).to_vec(&test_img, Some(&test_alpha), 5, 5, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();

    assert!(parser.premultiplied_alpha());
    assert_eq!(&test_img[..], parser.primary_data().unwrap().as_ref());
    assert_eq!(&test_alpha[..], parser.alpha_data().unwrap().unwrap().as_ref());
}

#[test]
fn size_required() {
    assert!(Aviffy::new().set_bit_depth(10).write_slice(&mut vec![], &[], None).is_err());
}

#[test]
fn depth_required() {
    assert!(Aviffy::new().set_width(1).set_height(1).write_slice(&mut vec![], &[], None).is_err());
}

#[test]
fn clli_roundtrip() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let avif = Aviffy::new()
        .set_content_light_level(1000, 400)
        .to_vec(&test_img, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    let cll = parser.content_light_level().expect("clli box should be present");
    assert_eq!(cll.max_content_light_level, 1000);
    assert_eq!(cll.max_pic_average_light_level, 400);
}

#[test]
fn mdcv_roundtrip() {
    let test_img = [1, 2, 3, 4, 5, 6];
    // BT.2020 primaries (standard encoding: CIE xy × 50000)
    let primaries = [
        (8500, 39850),   // green
        (6550, 2300),    // blue
        (35400, 14600),  // red
    ];
    let white_point = (15635, 16450); // D65
    let max_luminance = 10_000_000; // 1000 cd/m²
    let min_luminance = 1;          // 0.0001 cd/m²

    let avif = Aviffy::new()
        .set_mastering_display(primaries, white_point, max_luminance, min_luminance)
        .to_vec(&test_img, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    let mdcv = parser.mastering_display().expect("mdcv box should be present");
    assert_eq!(mdcv.primaries, primaries);
    assert_eq!(mdcv.white_point, white_point);
    assert_eq!(mdcv.max_luminance, max_luminance);
    assert_eq!(mdcv.min_luminance, min_luminance);
}

#[test]
fn hdr10_full_metadata() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let test_alpha = [77, 88, 99];
    let primaries = [
        (8500, 39850),
        (6550, 2300),
        (35400, 14600),
    ];
    let white_point = (15635, 16450);

    let avif = Aviffy::new()
        .set_transfer_characteristics(constants::TransferCharacteristics::Smpte2084)
        .set_color_primaries(constants::ColorPrimaries::Bt2020)
        .set_matrix_coefficients(constants::MatrixCoefficients::Bt2020Ncl)
        .set_content_light_level(4000, 1000)
        .set_mastering_display(primaries, white_point, 40_000_000, 50)
        .to_vec(&test_img, Some(&test_alpha), 10, 20, 10);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();

    // Verify CLLI
    let cll = parser.content_light_level().expect("clli box should be present");
    assert_eq!(cll.max_content_light_level, 4000);
    assert_eq!(cll.max_pic_average_light_level, 1000);

    // Verify MDCV
    let mdcv = parser.mastering_display().expect("mdcv box should be present");
    assert_eq!(mdcv.primaries, primaries);
    assert_eq!(mdcv.white_point, white_point);
    assert_eq!(mdcv.max_luminance, 40_000_000);
    assert_eq!(mdcv.min_luminance, 50);

    // Verify data integrity
    assert_eq!(parser.primary_data().unwrap().as_ref(), &test_img[..]);
    assert_eq!(parser.alpha_data().unwrap().unwrap().as_ref(), &test_alpha[..]);
}

#[test]
fn no_hdr_metadata_by_default() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let avif = serialize_to_vec(&test_img, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    assert!(parser.content_light_level().is_none());
    assert!(parser.mastering_display().is_none());
}

#[test]
fn rotation_roundtrip() {
    let test_img = [1, 2, 3, 4, 5, 6];
    for angle in 0..4u8 {
        let avif = Aviffy::new()
            .set_rotation(angle)
            .to_vec(&test_img, None, 10, 20, 8);

        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let rot = parser.rotation().expect("irot box should be present");
        let expected_angle = match angle {
            0 => 0,
            1 => 90,
            2 => 180,
            3 => 270,
            _ => unreachable!(),
        };
        assert_eq!(rot.angle, expected_angle, "angle code {angle}");

        // Verify data still parses
        assert_eq!(parser.primary_data().unwrap().as_ref(), &test_img[..]);
    }
}

#[test]
fn mirror_roundtrip() {
    let test_img = [1, 2, 3, 4, 5, 6];
    for axis in 0..2u8 {
        let avif = Aviffy::new()
            .set_mirror(axis)
            .to_vec(&test_img, None, 10, 20, 8);

        let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
        let mir = parser.mirror().expect("imir box should be present");
        assert_eq!(mir.axis, axis);
    }
}

#[test]
fn clap_roundtrip() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let avif = Aviffy::new()
        .set_clean_aperture(ClapBox {
            width_n: 800, width_d: 1,
            height_n: 600, height_d: 1,
            horiz_off_n: 0, horiz_off_d: 1,
            vert_off_n: 0, vert_off_d: 1,
        })
        .to_vec(&test_img, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    let clap = parser.clean_aperture().expect("clap box should be present");
    assert_eq!(clap.width_n, 800);
    assert_eq!(clap.width_d, 1);
    assert_eq!(clap.height_n, 600);
    assert_eq!(clap.height_d, 1);
    assert_eq!(clap.horiz_off_n, 0);
    assert_eq!(clap.horiz_off_d, 1);
    assert_eq!(clap.vert_off_n, 0);
    assert_eq!(clap.vert_off_d, 1);
}

#[test]
fn clap_with_negative_offset() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let avif = Aviffy::new()
        .set_clean_aperture(ClapBox {
            width_n: 640, width_d: 1,
            height_n: 480, height_d: 1,
            horiz_off_n: -10, horiz_off_d: 1,
            vert_off_n: -20, vert_off_d: 1,
        })
        .to_vec(&test_img, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    let clap = parser.clean_aperture().expect("clap box should be present");
    assert_eq!(clap.horiz_off_n, -10);
    assert_eq!(clap.vert_off_n, -20);
}

#[test]
fn pasp_roundtrip() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let avif = Aviffy::new()
        .set_pixel_aspect_ratio(2, 1)
        .to_vec(&test_img, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    let pasp = parser.pixel_aspect_ratio().expect("pasp box should be present");
    assert_eq!(pasp.h_spacing, 2);
    assert_eq!(pasp.v_spacing, 1);
}

#[test]
fn icc_profile_roundtrip() {
    let test_img = [1, 2, 3, 4, 5, 6];
    // Fake ICC profile data (real profiles are much larger, but any bytes work for roundtrip)
    let fake_icc = vec![0x00, 0x00, 0x00, 0x18, b'a', b'c', b's', b'p',
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let avif = Aviffy::new()
        .set_icc_profile(fake_icc.clone())
        .to_vec(&test_img, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    let color_info = parser.color_info().expect("colr box should be present");
    match color_info {
        zenavif_parse::ColorInformation::IccProfile(data) => {
            assert_eq!(data.as_slice(), &fake_icc[..]);
        }
        _ => panic!("expected ICC profile color info, got {:?}", color_info),
    }
}

#[test]
fn icc_overrides_nclx() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let fake_icc = vec![1, 2, 3, 4];
    // Setting both ICC and nclx: ICC should win
    let avif = Aviffy::new()
        .set_color_primaries(constants::ColorPrimaries::Bt2020)
        .set_icc_profile(fake_icc.clone())
        .to_vec(&test_img, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    match parser.color_info() {
        Some(zenavif_parse::ColorInformation::IccProfile(data)) => {
            assert_eq!(data.as_slice(), &fake_icc[..]);
        }
        other => panic!("expected ICC profile, got {:?}", other),
    }
}

#[test]
fn xmp_roundtrip() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let xmp_data = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'><test/></x:xmpmeta>".to_vec();

    let avif = Aviffy::new()
        .set_xmp(xmp_data.clone())
        .to_vec(&test_img, None, 10, 20, 8);

    // Verify the primary data is intact
    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    assert_eq!(parser.primary_data().unwrap().as_ref(), &test_img[..]);

    // Verify XMP data is somewhere in the file
    let xmp_str = b"<x:xmpmeta";
    assert!(avif.windows(xmp_str.len()).any(|w| w == xmp_str),
        "XMP data should be present in AVIF file");
}

#[test]
fn rotation_and_mirror_combined() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let avif = Aviffy::new()
        .set_rotation(1)  // 90° CCW
        .set_mirror(0)    // vertical axis
        .to_vec(&test_img, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    let rot = parser.rotation().expect("irot should be present");
    let mir = parser.mirror().expect("imir should be present");
    assert_eq!(rot.angle, 90);
    assert_eq!(mir.axis, 0);
}

#[test]
fn all_properties_combined() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let test_alpha = [77, 88, 99];
    let avif = Aviffy::new()
        .set_rotation(2)
        .set_mirror(1)
        .set_clean_aperture(ClapBox {
            width_n: 8, width_d: 1,
            height_n: 18, height_d: 1,
            horiz_off_n: 0, horiz_off_d: 1,
            vert_off_n: 0, vert_off_d: 1,
        })
        .set_pixel_aspect_ratio(1, 1)
        .set_content_light_level(1000, 400)
        .set_mastering_display(
            [(8500, 39850), (6550, 2300), (35400, 14600)],
            (15635, 16450), 10_000_000, 50,
        )
        .to_vec(&test_img, Some(&test_alpha), 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    assert_eq!(parser.rotation().unwrap().angle, 180);
    assert_eq!(parser.mirror().unwrap().axis, 1);
    assert!(parser.clean_aperture().is_some());
    assert!(parser.pixel_aspect_ratio().is_some());
    assert!(parser.content_light_level().is_some());
    assert!(parser.mastering_display().is_some());

    assert_eq!(parser.primary_data().unwrap().as_ref(), &test_img[..]);
    assert_eq!(parser.alpha_data().unwrap().unwrap().as_ref(), &test_alpha[..]);
}

// Tests using avif-parse (upstream parser) to verify broad compatibility.
// avif-parse v2.0 only exposes primary_item, alpha_item, premultiplied_alpha,
// clli, and mdcv. For other features we verify it at least parses without panic.

/// Helper: parse with avif-parse, return AvifData. Panics on parse failure.
#[cfg(test)]
fn parse_with_avif_parse(avif: &[u8]) -> avif_parse::AvifData {
    avif_parse::read_avif(&mut &*avif)
        .unwrap_or_else(|e| panic!("avif-parse failed to parse: {e:?}"))
}

#[test]
fn avif_parse_basic_roundtrip() {
    let color = b"av1colordata";
    let avif = serialize_to_vec(color, None, 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    assert_eq!(parsed.primary_item.as_slice(), &color[..]);
    assert!(parsed.alpha_item.is_none());
}

#[test]
fn avif_parse_alpha_roundtrip() {
    let color = b"av1colordata";
    let alpha = b"alphadata";
    let avif = serialize_to_vec(color, Some(alpha), 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    assert_eq!(parsed.primary_item.as_slice(), &color[..]);
    assert_eq!(parsed.alpha_item.unwrap().as_slice(), &alpha[..]);
}

#[test]
fn avif_parse_premultiplied_alpha() {
    let color = [1, 2, 3, 4];
    let alpha = [55, 66, 77, 88];
    let avif = Aviffy::new().premultiplied_alpha(true)
        .to_vec(&color, Some(&alpha), 5, 5, 8);
    let parsed = parse_with_avif_parse(&avif);
    assert!(parsed.premultiplied_alpha);
    assert_eq!(parsed.primary_item.as_slice(), &color[..]);
    assert_eq!(parsed.alpha_item.unwrap().as_slice(), &alpha[..]);
}

#[test]
fn avif_parse_clli_roundtrip() {
    let img = [1, 2, 3, 4, 5, 6];
    let avif = Aviffy::new()
        .set_content_light_level(1000, 400)
        .to_vec(&img, None, 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    let cll = parsed.content_light_level.expect("clli should be present");
    assert_eq!(cll.max_content_light_level, 1000);
    assert_eq!(cll.max_pic_average_light_level, 400);
}

#[test]
fn avif_parse_mdcv_roundtrip() {
    let img = [1, 2, 3, 4, 5, 6];
    let primaries = [(8500, 39850), (6550, 2300), (35400, 14600)];
    let avif = Aviffy::new()
        .set_mastering_display(primaries, (15635, 16450), 10_000_000, 50)
        .to_vec(&img, None, 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    let mdcv = parsed.mastering_display.expect("mdcv should be present");
    assert_eq!(mdcv.primaries, primaries);
    assert_eq!(mdcv.white_point, (15635, 16450));
    assert_eq!(mdcv.max_luminance, 10_000_000);
    assert_eq!(mdcv.min_luminance, 50);
}

#[test]
fn avif_parse_hdr10_full() {
    let img = [1, 2, 3, 4, 5, 6];
    let alpha = [77, 88, 99];
    let avif = Aviffy::new()
        .set_transfer_characteristics(constants::TransferCharacteristics::Smpte2084)
        .set_color_primaries(constants::ColorPrimaries::Bt2020)
        .set_matrix_coefficients(constants::MatrixCoefficients::Bt2020Ncl)
        .set_content_light_level(4000, 1000)
        .set_mastering_display(
            [(8500, 39850), (6550, 2300), (35400, 14600)],
            (15635, 16450), 40_000_000, 50,
        )
        .to_vec(&img, Some(&alpha), 10, 20, 10);
    let parsed = parse_with_avif_parse(&avif);
    assert_eq!(parsed.primary_item.as_slice(), &img[..]);
    assert_eq!(parsed.alpha_item.unwrap().as_slice(), &alpha[..]);
    assert!(parsed.content_light_level.is_some());
    assert!(parsed.mastering_display.is_some());
}

// The following tests verify avif-parse doesn't crash on features it
// doesn't expose fields for (transforms, metadata, animation, grid).

#[test]
fn avif_parse_survives_rotation() {
    let img = [1, 2, 3, 4, 5, 6];
    for angle in 0..4u8 {
        let avif = Aviffy::new().set_rotation(angle).to_vec(&img, None, 10, 20, 8);
        let parsed = parse_with_avif_parse(&avif);
        assert_eq!(parsed.primary_item.as_slice(), &img[..]);
    }
}

#[test]
fn avif_parse_survives_mirror() {
    let img = [1, 2, 3, 4, 5, 6];
    for axis in 0..2u8 {
        let avif = Aviffy::new().set_mirror(axis).to_vec(&img, None, 10, 20, 8);
        let parsed = parse_with_avif_parse(&avif);
        assert_eq!(parsed.primary_item.as_slice(), &img[..]);
    }
}

#[test]
fn avif_parse_survives_clap() {
    let img = [1, 2, 3, 4, 5, 6];
    let avif = Aviffy::new()
        .set_clean_aperture(ClapBox {
            width_n: 800, width_d: 1,
            height_n: 600, height_d: 1,
            horiz_off_n: 0, horiz_off_d: 1,
            vert_off_n: 0, vert_off_d: 1,
        })
        .to_vec(&img, None, 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    assert_eq!(parsed.primary_item.as_slice(), &img[..]);
}

#[test]
fn avif_parse_survives_pasp() {
    let img = [1, 2, 3, 4, 5, 6];
    let avif = Aviffy::new().set_pixel_aspect_ratio(2, 1).to_vec(&img, None, 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    assert_eq!(parsed.primary_item.as_slice(), &img[..]);
}

#[test]
fn avif_parse_survives_icc_profile() {
    let img = [1, 2, 3, 4, 5, 6];
    let icc = vec![0, 0, 0, 24, b'a', b'c', b's', b'p', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let avif = Aviffy::new().set_icc_profile(icc).to_vec(&img, None, 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    assert_eq!(parsed.primary_item.as_slice(), &img[..]);
}

#[test]
fn avif_parse_survives_exif() {
    let img = b"av1colordata";
    let avif = Aviffy::new()
        .set_exif(b"exifdata".to_vec())
        .to_vec(img, None, 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    assert_eq!(parsed.primary_item.as_slice(), &img[..]);
}

#[test]
fn avif_parse_survives_xmp() {
    let img = [1, 2, 3, 4, 5, 6];
    let avif = Aviffy::new()
        .set_xmp(b"<x:xmpmeta/>".to_vec())
        .to_vec(&img, None, 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    assert_eq!(parsed.primary_item.as_slice(), &img[..]);
}

#[test]
fn avif_parse_survives_all_properties() {
    let img = [1, 2, 3, 4, 5, 6];
    let alpha = [77, 88, 99];
    let avif = Aviffy::new()
        .set_rotation(2)
        .set_mirror(1)
        .set_clean_aperture(ClapBox {
            width_n: 8, width_d: 1,
            height_n: 18, height_d: 1,
            horiz_off_n: 0, horiz_off_d: 1,
            vert_off_n: 0, vert_off_d: 1,
        })
        .set_pixel_aspect_ratio(1, 1)
        .set_content_light_level(1000, 400)
        .set_mastering_display(
            [(8500, 39850), (6550, 2300), (35400, 14600)],
            (15635, 16450), 10_000_000, 50,
        )
        .set_exif(b"exif".to_vec())
        .set_xmp(b"<xmp/>".to_vec())
        .to_vec(&img, Some(&alpha), 10, 20, 8);
    let parsed = parse_with_avif_parse(&avif);
    assert_eq!(parsed.primary_item.as_slice(), &img[..]);
    assert_eq!(parsed.alpha_item.unwrap().as_slice(), &alpha[..]);
    assert!(parsed.content_light_level.is_some());
    assert!(parsed.mastering_display.is_some());
}

#[test]
fn avif_parse_survives_animated() {
    use crate::animated::{AnimatedImage, AnimFrame};
    let mut anim = AnimatedImage::new();
    anim.set_timescale(1000);
    anim.set_color_config(boxes::Av1CBox {
        seq_profile: 0, seq_level_idx_0: 4, seq_tier_0: false,
        high_bitdepth: false, twelve_bit: false, monochrome: false,
        chroma_subsampling_x: true, chroma_subsampling_y: true,
        chroma_sample_position: 0,
    });
    let frames = [
        AnimFrame::new(b"frame0data", 33).with_sync(true),
        AnimFrame::new(b"frame1data", 33),
    ];
    let avif = anim.serialize(320, 240, &frames, b"seqhdr", None);
    // avif-parse may or may not support avis brand, but must not crash
    let _ = avif_parse::read_avif(&mut avif.as_slice());
}

#[test]
fn avif_parse_survives_grid() {
    use crate::grid::GridImage;
    let tiles: Vec<Vec<u8>> = (0..4).map(|i| vec![i as u8; 100]).collect();
    let tile_refs: Vec<&[u8]> = tiles.iter().map(|t| t.as_slice()).collect();

    let mut grid = GridImage::new();
    grid.set_color_config(boxes::Av1CBox {
        seq_profile: 0, seq_level_idx_0: 4, seq_tier_0: false,
        high_bitdepth: false, twelve_bit: false, monochrome: false,
        chroma_subsampling_x: true, chroma_subsampling_y: true,
        chroma_sample_position: 0,
    });
    let avif = grid.serialize(2, 2, 200, 200, 100, 100, &tile_refs, None).unwrap();
    // avif-parse may or may not support grid, but must not crash
    let _ = avif_parse::read_avif(&mut avif.as_slice());
}

// ─── Gain map / tmap tests ───────────────────────────────────────────

/// Build a minimal ISO 21496-1 metadata blob (single-channel).
///
/// Format: version(u8) + minimum_version(u16) + writer_version(u16) + flags(u8)
///   + base_hdr_headroom(u32×2) + alternate_hdr_headroom(u32×2)
///   + per-channel: gain_map_min(i32+u32) + gain_map_max(i32+u32)
///     + gamma(u32×2) + base_offset(i32+u32) + alternate_offset(i32+u32)
#[cfg(test)]
fn make_test_tmap_metadata(
    is_multichannel: bool,
    use_base_colour_space: bool,
    base_headroom_n: u32,
    base_headroom_d: u32,
    alt_headroom_n: u32,
    alt_headroom_d: u32,
) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(0); // version
    buf.extend_from_slice(&0u16.to_be_bytes()); // minimum_version
    buf.extend_from_slice(&0u16.to_be_bytes()); // writer_version
    let flags = (u8::from(is_multichannel) << 7) | (u8::from(use_base_colour_space) << 6);
    buf.push(flags);

    buf.extend_from_slice(&base_headroom_n.to_be_bytes());
    buf.extend_from_slice(&base_headroom_d.to_be_bytes());
    buf.extend_from_slice(&alt_headroom_n.to_be_bytes());
    buf.extend_from_slice(&alt_headroom_d.to_be_bytes());

    let channel_count = if is_multichannel { 3 } else { 1 };
    for _ in 0..channel_count {
        // gain_map_min = 0/1
        buf.extend_from_slice(&0i32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        // gain_map_max = 1/1
        buf.extend_from_slice(&1i32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        // gamma = 1/1
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        // base_offset = 0/1
        buf.extend_from_slice(&0i32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
        // alternate_offset = 0/1
        buf.extend_from_slice(&0i32.to_be_bytes());
        buf.extend_from_slice(&1u32.to_be_bytes());
    }
    buf
}

#[test]
fn gain_map_roundtrip() {
    let primary_data = b"primary_av1_data";
    let gain_map_data = b"gain_map_av1_data";
    let metadata = make_test_tmap_metadata(false, true, 0, 1, 1, 1);

    let avif = Aviffy::new()
        .set_gain_map(gain_map_data.to_vec(), 4, 4, 8, metadata.clone())
        .to_vec(primary_data, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();

    // Primary data intact
    assert_eq!(parser.primary_data().unwrap().as_ref(), &primary_data[..]);

    // Gain map metadata should be detected
    let gm_meta = parser.gain_map_metadata().expect("gain map metadata should be present");
    assert!(!gm_meta.is_multichannel);
    assert!(gm_meta.use_base_colour_space);
    assert_eq!(gm_meta.base_hdr_headroom_n, 0);
    assert_eq!(gm_meta.base_hdr_headroom_d, 1);
    assert_eq!(gm_meta.alternate_hdr_headroom_n, 1);
    assert_eq!(gm_meta.alternate_hdr_headroom_d, 1);

    // Gain map image data should be extractable
    let gm_data = parser.gain_map_data().expect("gain map data should be present").unwrap();
    assert_eq!(gm_data.as_ref(), &gain_map_data[..]);
}

#[test]
fn gain_map_with_alpha() {
    let primary_data = b"primary_av1";
    let alpha_data = b"alpha_av1";
    let gain_map_data = b"gm_av1_data";
    let metadata = make_test_tmap_metadata(false, true, 0, 1, 1, 1);

    let avif = Aviffy::new()
        .set_gain_map(gain_map_data.to_vec(), 4, 4, 8, metadata)
        .to_vec(primary_data, Some(alpha_data), 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();

    // Primary + alpha data intact
    assert_eq!(parser.primary_data().unwrap().as_ref(), &primary_data[..]);
    assert_eq!(parser.alpha_data().unwrap().unwrap().as_ref(), &alpha_data[..]);

    // Gain map detected
    let gm_data = parser.gain_map_data().expect("gain map data present").unwrap();
    assert_eq!(gm_data.as_ref(), &gain_map_data[..]);
    assert!(parser.gain_map_metadata().is_some());
}

#[test]
fn gain_map_multichannel_metadata() {
    let primary_data = [1, 2, 3, 4, 5, 6];
    let gain_map_data = [10, 20, 30, 40];
    let metadata = make_test_tmap_metadata(true, false, 0, 1, 3, 1);

    let avif = Aviffy::new()
        .set_gain_map(gain_map_data.to_vec(), 2, 2, 8, metadata.clone())
        .to_vec(&primary_data, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    let gm_meta = parser.gain_map_metadata().expect("gain map metadata present");
    assert!(gm_meta.is_multichannel);
    assert!(!gm_meta.use_base_colour_space);
    assert_eq!(gm_meta.alternate_hdr_headroom_n, 3);
}

#[test]
fn gain_map_metadata_field_exact() {
    // Known metadata blob -> embed -> extract -> verify field-by-field
    let primary_data = [1, 2, 3, 4];
    let gain_map_data = [99, 88, 77];
    let metadata = make_test_tmap_metadata(false, true, 0, 1, 6, 1);

    let avif = Aviffy::new()
        .set_gain_map(gain_map_data.to_vec(), 1, 1, 8, metadata.clone())
        .to_vec(&primary_data, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    let gm_meta = parser.gain_map_metadata().expect("gain map metadata present");

    assert_eq!(gm_meta.alternate_hdr_headroom_n, 6);
    assert_eq!(gm_meta.alternate_hdr_headroom_d, 1);
    assert_eq!(gm_meta.channels[0].gain_map_min_n, 0);
    assert_eq!(gm_meta.channels[0].gain_map_min_d, 1);
    assert_eq!(gm_meta.channels[0].gain_map_max_n, 1);
    assert_eq!(gm_meta.channels[0].gain_map_max_d, 1);
    assert_eq!(gm_meta.channels[0].gamma_n, 1);
    assert_eq!(gm_meta.channels[0].gamma_d, 1);
    assert_eq!(gm_meta.channels[0].base_offset_n, 0);
    assert_eq!(gm_meta.channels[0].base_offset_d, 1);
    assert_eq!(gm_meta.channels[0].alternate_offset_n, 0);
    assert_eq!(gm_meta.channels[0].alternate_offset_d, 1);
}

#[test]
fn gain_map_alt_colr_roundtrip() {
    let primary_data = [1, 2, 3, 4, 5, 6];
    let gain_map_data = [10, 20, 30];
    let metadata = make_test_tmap_metadata(false, true, 0, 1, 1, 1);

    let alt_colr = ColrBox {
        color_primaries: constants::ColorPrimaries::Bt2020,
        transfer_characteristics: constants::TransferCharacteristics::Smpte2084,
        matrix_coefficients: constants::MatrixCoefficients::Bt2020Ncl,
        full_range_flag: false,
    };

    let avif = Aviffy::new()
        .set_gain_map(gain_map_data.to_vec(), 2, 2, 8, metadata)
        .set_gain_map_alt_colr(alt_colr)
        .to_vec(&primary_data, None, 10, 20, 8);

    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();

    // Gain map data intact
    let gm_data = parser.gain_map_data().expect("gain map data present").unwrap();
    assert_eq!(gm_data.as_ref(), &gain_map_data[..]);

    // Verify alternate color info
    let alt = parser.gain_map_color_info().expect("alt color info should be present");
    match alt {
        zenavif_parse::ColorInformation::Nclx {
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            full_range,
        } => {
            assert_eq!(*color_primaries, 9); // BT.2020
            assert_eq!(*transfer_characteristics, 16); // PQ
            assert_eq!(*matrix_coefficients, 9); // BT.2020 NCL
            assert!(!full_range);
        }
        other => panic!("expected NCLX color info, got: {:?}", other),
    }
}

#[test]
fn no_gain_map_by_default() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let avif = serialize_to_vec(&test_img, None, 10, 20, 8);
    let parser = zenavif_parse::AvifParser::from_bytes(&avif).unwrap();
    assert!(parser.gain_map_metadata().is_none(), "no gain map metadata by default");
    assert!(parser.gain_map_data().is_none(), "no gain map data by default");
}

#[test]
fn avif_parse_survives_gain_map() {
    let primary_data = b"primary_color";
    let gain_map_data = b"gain_map_pixels";
    let metadata = make_test_tmap_metadata(false, true, 0, 1, 1, 1);

    let avif = Aviffy::new()
        .set_gain_map(gain_map_data.to_vec(), 4, 4, 8, metadata)
        .to_vec(primary_data, None, 10, 20, 8);

    // avif-parse (older parser) must not crash
    let _ = avif_parse::read_avif(&mut avif.as_slice());
}

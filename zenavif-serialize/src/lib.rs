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
mod writer;

use crate::boxes::*;
use arrayvec::ArrayVec;
use std::io;

// Re-export box types needed by the public API
pub use crate::boxes::{Av1CBox, ClapBox, ClliBox, ColrBox, ColrIccBox, IrotBox, ImirBox, MdcvBox, PaspBox};

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
    chroma_subsampling: (bool, bool),
    monochrome: bool,
    width: u32,
    height: u32,
    bit_depth: u8,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
}

/// Makes an AVIF file given encoded AV1 data (create the data with [`rav1e`](https://lib.rs/rav1e))
///
/// `color_av1_data` is already-encoded AV1 image data for the color channels (YUV, RGB, etc.).
/// [You can parse this information out of AV1 payload with `avif-parse`](https://docs.rs/avif-parse/latest/avif_parse/struct.AV1Metadata.html).
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
    /// [You can get this information out of the AV1 payload with `avif-parse`](https://docs.rs/avif-parse/latest/avif_parse/struct.AV1Metadata.html).
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self {
            premultiplied_alpha: false,
            min_seq_profile: 1,
            chroma_subsampling: (false, false),
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
            chroma_subsampling_x: self.chroma_subsampling.0,
            chroma_subsampling_y: self.chroma_subsampling.1,
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
            let _ = next_item_id; // suppress unused warning

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
                iref: IrefBox { entries: irefs },
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

    /// `(false, false)` is 4:4:4
    /// `(true, true)` is 4:2:0
    ///
    /// `chroma_sample_position` is always 0. Don't use chroma subsampling with AVIF.
    #[inline]
    pub fn set_chroma_subsampling(&mut self, subsampled_xy: (bool, bool)) -> &mut Self {
        self.chroma_subsampling = subsampled_xy;
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

    let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();

    assert_eq!(&test_img[..], ctx.primary_item.as_slice());
    assert_eq!(&test_alpha[..], ctx.alpha_item.as_deref().unwrap());
}

#[test]
fn test_roundtrip_parse_avif_colr() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let test_alpha = [77, 88, 99];
    let avif = Aviffy::new()
        .matrix_coefficients(constants::MatrixCoefficients::Bt709)
        .to_vec(&test_img, Some(&test_alpha), 10, 20, 8);

    let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();

    assert_eq!(&test_img[..], ctx.primary_item.as_slice());
    assert_eq!(&test_alpha[..], ctx.alpha_item.as_deref().unwrap());
}

#[test]
fn premultiplied_flag() {
    let test_img = [1,2,3,4];
    let test_alpha = [55,66,77,88,99];
    let avif = Aviffy::new().premultiplied_alpha(true).to_vec(&test_img, Some(&test_alpha), 5, 5, 8);

    let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();

    assert!(ctx.premultiplied_alpha);
    assert_eq!(&test_img[..], ctx.primary_item.as_slice());
    assert_eq!(&test_alpha[..], ctx.alpha_item.as_deref().unwrap());
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

    let parser = avif_parse::read_avif(&mut avif.as_slice()).unwrap();
    let cll = parser.content_light_level.expect("clli box should be present");
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

    let parser = avif_parse::read_avif(&mut avif.as_slice()).unwrap();
    let mdcv = parser.mastering_display.expect("mdcv box should be present");
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

    let parser = avif_parse::read_avif(&mut avif.as_slice()).unwrap();

    // Verify CLLI
    let cll = parser.content_light_level.expect("clli box should be present");
    assert_eq!(cll.max_content_light_level, 4000);
    assert_eq!(cll.max_pic_average_light_level, 1000);

    // Verify MDCV
    let mdcv = parser.mastering_display.expect("mdcv box should be present");
    assert_eq!(mdcv.primaries, primaries);
    assert_eq!(mdcv.white_point, white_point);
    assert_eq!(mdcv.max_luminance, 40_000_000);
    assert_eq!(mdcv.min_luminance, 50);

    // Verify data integrity
    let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();
    assert_eq!(ctx.primary_item.as_slice(), &test_img[..]);
    assert_eq!(ctx.alpha_item.as_deref().unwrap(), &test_alpha[..]);
}

#[test]
fn no_hdr_metadata_by_default() {
    let test_img = [1, 2, 3, 4, 5, 6];
    let avif = serialize_to_vec(&test_img, None, 10, 20, 8);

    let parser = avif_parse::read_avif(&mut avif.as_slice()).unwrap();
    assert!(parser.content_light_level.is_none());
    assert!(parser.mastering_display.is_none());
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
        let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();
        assert_eq!(ctx.primary_item.as_slice(), &test_img[..]);
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
    let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();
    assert_eq!(ctx.primary_item.as_slice(), &test_img[..]);

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

    let ctx = avif_parse::read_avif(&mut avif.as_slice()).unwrap();
    assert_eq!(ctx.primary_item.as_slice(), &test_img[..]);
    assert_eq!(ctx.alpha_item.as_deref().unwrap(), &test_alpha[..]);
}

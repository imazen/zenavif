#![allow(dead_code)]

use zenpixels::{AlphaMode, ColorSpace, ImageSpec, PixelBuf, PixelType, PixelView, SampleLayout};

pub fn from_imgref<P: PixelType>(img: imgref::ImgRef<'_, P>, spec: ImageSpec) -> PixelView<'_> {
    assert_eq!(spec.layout, P::LAYOUT);
    PixelView::new(
        bytemuck::cast_slice(img.buf()),
        img.width() as u32,
        img.height() as u32,
        img.stride() * core::mem::size_of::<P>(),
        spec,
    )
    .unwrap()
}

pub fn from_pixels<P: PixelType>(data: Vec<P>, w: u32, h: u32) -> PixelBuf {
    let color = if P::LAYOUT.order.is_gray_family() {
        ImageSpec::gray8_srgb().color
    } else if P::LAYOUT.order.has_alpha_slot() {
        ColorSpace::SRGB_ALPHA
    } else {
        ColorSpace::SRGB
    };
    PixelBuf::from_vec(data, w, h, ImageSpec::new(P::LAYOUT, color)).unwrap()
}

pub trait BufTestExt {
    fn try_as_imgref<P: PixelType>(&self) -> Option<imgref::ImgRef<'_, P>>;
    fn copy_to_contiguous_bytes(&self) -> Vec<u8>;
}
impl BufTestExt for PixelBuf {
    fn try_as_imgref<P: PixelType>(&self) -> Option<imgref::ImgRef<'_, P>> {
        self.as_view().try_as_imgref().ok()
    }

    fn copy_to_contiguous_bytes(&self) -> Vec<u8> {
        self.as_view().try_contiguous_bytes().unwrap().into_owned()
    }
}

pub const fn native_spec(layout: SampleLayout) -> ImageSpec {
    let alpha = if layout.order.has_alpha_slot() {
        AlphaMode::Straight
    } else {
        AlphaMode::None
    };
    let color = if layout.order.is_gray_family() {
        ColorSpace::Gray {
            transfer: zenpixels::Transfer::Unknown,
            range: zenpixels::SignalRange::Full,
            alpha,
            white: None,
        }
    } else {
        ColorSpace::Rgb {
            primaries: zenpixels::Primaries::Unknown,
            transfer: zenpixels::Transfer::Unknown,
            range: zenpixels::SignalRange::Full,
            alpha,
            white: None,
        }
    };
    ImageSpec::new(layout, color)
}

pub trait SpecTestExt {
    fn transfer(&self) -> zenpixels::Transfer;
    fn primaries(&self) -> zenpixels::Primaries;
}
impl SpecTestExt for ImageSpec {
    fn transfer(&self) -> zenpixels::Transfer {
        match self.color {
            ColorSpace::Gray { transfer, .. } | ColorSpace::Rgb { transfer, .. } => transfer,
            _ => zenpixels::Transfer::Unknown,
        }
    }

    fn primaries(&self) -> zenpixels::Primaries {
        match self.color {
            ColorSpace::Rgb { primaries, .. } => primaries,
            _ => zenpixels::Primaries::Unknown,
        }
    }
}

//! Pixel storage and color operations shared by the native and codec adapters.

#[cfg(feature = "encode")]
use zenpixels::PixelView;

use zenpixels::{
    AlphaMode, BufferError, ColorSpace, PixelBuf, PixelFormat, PixelType, PixelViewMut, Primaries,
    SampleLayout, SignalRange, Transfer,
};

pub(crate) const fn native_spec(layout: SampleLayout) -> PixelFormat {
    let alpha = if layout.order.has_alpha_slot() {
        AlphaMode::Straight
    } else {
        AlphaMode::None
    };
    let color = if layout.order.is_gray_family() {
        ColorSpace::Gray {
            transfer: Transfer::Unknown,
            range: SignalRange::Full,
            alpha,
            white: None,
        }
    } else {
        ColorSpace::Rgb {
            primaries: Primaries::Unknown,
            transfer: Transfer::Unknown,
            range: SignalRange::Full,
            alpha,
            white: None,
        }
    };
    PixelFormat::new(layout, color)
}

pub(crate) fn from_pixels<P: PixelType + zenpixels::TypedElement>(
    data: Vec<P>,
    w: u32,
    h: u32,
) -> Result<PixelBuf, whereat::At<BufferError>> {
    PixelBuf::from_typed(data, w, h, native_spec(P::LAYOUT))
}

#[cfg(feature = "encode")]
pub(crate) fn try_from_imgref<P: PixelType>(
    image: imgref::ImgRef<'_, P>,
    spec: PixelFormat,
) -> crate::error::Result<PixelView<'_>> {
    let invalid = || {
        whereat::at!(crate::Error::InvalidBuffer(
            "invalid imgref geometry or layout".into()
        ))
    };
    let width = u32::try_from(image.width()).map_err(|_| invalid())?;
    let height = u32::try_from(image.height()).map_err(|_| invalid())?;
    let stride = image
        .stride()
        .checked_mul(core::mem::size_of::<P>())
        .ok_or_else(invalid)?;
    if spec.layout != P::LAYOUT {
        return Err(invalid());
    }
    PixelView::new_with_format_cow(
        bytemuck::cast_slice(image.buf()),
        width,
        height,
        stride,
        zenpixels::FormatCow::from_owned(spec),
    )
    .map_err(|_| invalid())
}

#[cfg(all(feature = "encode", test))]
pub(crate) fn from_imgref<P: PixelType>(
    image: imgref::ImgRef<'_, P>,
    spec: PixelFormat,
) -> PixelView<'_> {
    try_from_imgref(image, spec).unwrap()
}

pub(crate) trait PixelBufExt {
    fn try_as_imgref<P: PixelType>(&self) -> Option<imgref::ImgRef<'_, P>>;
    fn retag(self, spec: PixelFormat) -> Self;
    fn convert_to(
        &self,
        spec: PixelFormat,
    ) -> Result<PixelBuf, whereat::At<zenpixels_convert::ConvertError>>;
}

impl PixelBufExt for PixelBuf {
    fn try_as_imgref<P: PixelType>(&self) -> Option<imgref::ImgRef<'_, P>> {
        self.as_view().as_imgref().ok()
    }

    fn retag(self, spec: PixelFormat) -> Self {
        assert_eq!(self.format().layout, spec.layout);
        let facts = self.facts();
        let mut retagged = self
            .relabel_format(spec)
            .expect("valid color for the same layout");
        retagged.assume_facts(facts);
        retagged
    }

    fn convert_to(
        &self,
        spec: PixelFormat,
    ) -> Result<PixelBuf, whereat::At<zenpixels_convert::ConvertError>> {
        let buf = zenpixels_convert::convert_to_buf_with_peak(
            &self.as_view(),
            &spec,
            &zenpixels_convert::ConvertPolicy::default(),
            zenpixels::PeakSource::Declared,
        )?;
        // AVIF provenance records the original container carriers even when
        // the representation plan changes the active sample interpretation.
        Ok(buf.with_origin(self.origin().clone()))
    }
}

pub(crate) fn as_imgref_mut<'a, P: PixelType>(
    view: &'a mut PixelViewMut<'_>,
) -> Option<imgref::ImgRefMut<'a, P>> {
    let bpp = core::mem::size_of::<P>();
    let stride = view.stride_bytes();
    if view.format().layout != P::LAYOUT || !stride.is_multiple_of(bpp) {
        return None;
    }
    let (w, h) = (view.width() as usize, view.height() as usize);
    let pixels = bytemuck::try_cast_slice_mut(view.as_strided_bytes_mut()).ok()?;
    Some(imgref::ImgRefMut::new_stride(pixels, w, h, stride / bpp))
}

pub(crate) trait PixelFormatExt {
    fn transfer(&self) -> Transfer;
    fn primaries(&self) -> Primaries;
    fn range(&self) -> SignalRange;
    fn layout_compatible(&self, layout: SampleLayout) -> bool;
}

impl PixelFormatExt for PixelFormat {
    fn transfer(&self) -> Transfer {
        match self.color {
            ColorSpace::Rgb { transfer, .. } | ColorSpace::Gray { transfer, .. } => transfer,
            _ => Transfer::Unknown,
        }
    }

    fn primaries(&self) -> Primaries {
        match self.color {
            ColorSpace::Rgb { primaries, .. } => primaries,
            _ => Primaries::Unknown,
        }
    }

    fn range(&self) -> SignalRange {
        match self.color {
            ColorSpace::Rgb { range, .. } | ColorSpace::Gray { range, .. } => range,
            _ => SignalRange::Full,
        }
    }

    fn layout_compatible(&self, layout: SampleLayout) -> bool {
        self.layout == layout
    }
}

pub(crate) fn orient(
    pixels: &PixelBuf,
    orientation: zenpixels::Orientation,
) -> crate::error::Result<PixelBuf> {
    zenpixels_convert::apply_orientation(&pixels.as_view(), orientation).map_err(|_| {
        whereat::at!(crate::Error::InvalidBuffer(
            "could not orient pixels".into()
        ))
    })
}

#[cfg(all(feature = "encode", test))]
pub(crate) fn from_native_imgref<P: PixelType>(img: imgref::ImgRef<'_, P>) -> PixelView<'_> {
    let mut spec = native_spec(P::LAYOUT);
    match P::LAYOUT.sample {
        zenpixels::SampleType::U8 => {
            spec = spec
                .with_transfer(Transfer::Srgb)
                .with_primaries(Primaries::Bt709);
        }
        sample if sample.is_float() => spec = spec.with_transfer(Transfer::Linear),
        _ => {}
    }
    from_imgref(img, spec)
}

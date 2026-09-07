//! Resolve clean aperture exactly and crop before animation orientation.
use crate::alloc_util::{AllocPref, vec_with_capacity};
use crate::error::{Error, Result};
use crate::image::ImageInfo;
use enough::Stop;
use whereat::at;
use zenpixels::PixelBuffer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AnimationCrop {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl AnimationCrop {
    /// Resolve source rationals without rounding. Native APIs retain arbitrary
    /// rational apertures; this pixel-buffer adapter requires an integral crop.
    pub(super) fn resolve(info: &mut ImageInfo) -> Result<Option<Self>> {
        let Some(clap) = info.clean_aperture else {
            return Ok(None);
        };
        let invalid = || {
            at!(Error::InvalidParameters(
                "invalid animation clean aperture".into()
            ))
        };
        if clap.width_d == 0 || clap.height_d == 0 || clap.horiz_off_d == 0 || clap.vert_off_d == 0
        {
            return Err(invalid());
        }
        if clap.width_n % clap.width_d != 0 || clap.height_n % clap.height_d != 0 {
            return Err(at!(Error::Unsupported(
                "fractional animation clean aperture requires resampling; native APIs preserve its exact rationals"
            )));
        }
        let width = clap.width_n / clap.width_d;
        let height = clap.height_n / clap.height_d;
        if width == 0 || height == 0 || width > info.width || height > info.height {
            return Err(invalid());
        }
        let origin = |canvas: u32, size: u32, offset_n: i32, offset_d: u32| -> Result<u32> {
            let numerator =
                i128::from(canvas - size) * i128::from(offset_d) + 2 * i128::from(offset_n);
            let denominator = 2 * i128::from(offset_d);
            if numerator < 0 {
                return Err(invalid());
            }
            if numerator % denominator != 0 {
                return Err(at!(Error::Unsupported(
                    "fractional animation crop origin requires resampling; native APIs preserve its exact rationals"
                )));
            }
            let value = u32::try_from(numerator / denominator).map_err(|_| invalid())?;
            if value > canvas - size {
                return Err(invalid());
            }
            Ok(value)
        };
        let crop = Self {
            x: origin(info.width, width, clap.horiz_off_n, clap.horiz_off_d)?,
            y: origin(info.height, height, clap.vert_off_n, clap.vert_off_d)?,
            width,
            height,
        };
        info.width = width;
        info.height = height;
        Ok(Some(crop))
    }

    pub(super) fn apply(
        self,
        pixels: PixelBuffer,
        alloc_pref: AllocPref,
        stop: &(impl Stop + ?Sized),
    ) -> Result<PixelBuffer> {
        if self.width > pixels.width()
            || self.height > pixels.height()
            || self.x > pixels.width() - self.width
            || self.y > pixels.height() - self.height
        {
            return Err(at!(Error::InvalidParameters(
                "animation frame is smaller than its clean aperture".into()
            )));
        }
        if self.x == 0
            && self.y == 0
            && self.width == pixels.width()
            && self.height == pixels.height()
        {
            return Ok(pixels);
        }
        let descriptor = pixels.descriptor();
        let bpp = descriptor.bytes_per_pixel();
        let row_bytes = (self.width as usize)
            .checked_mul(bpp)
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let len = row_bytes
            .checked_mul(self.height as usize)
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        let mut data = vec_with_capacity(alloc_pref, true, len)?;
        let start = (self.x as usize)
            .checked_mul(bpp)
            .ok_or_else(|| at!(Error::OutOfMemory))?;
        for y in self.y..self.y + self.height {
            stop.check().map_err(|e| at!(Error::Cancelled(e)))?;
            data.extend_from_slice(&pixels.as_slice().row(y)[start..start + row_bytes]);
        }
        PixelBuffer::from_vec(data, self.width, self.height, descriptor).map_err(|_| {
            at!(Error::InvalidParameters(
                "invalid cropped animation buffer".into()
            ))
        })
    }
}

#[test]
fn exact_crop_validation_is_transactional() {
    use crate::CleanAperture;
    let aperture = CleanAperture {
        width_n: 98,
        width_d: 2,
        height_n: 106,
        height_d: 2,
        horiz_off_n: -20,
        horiz_off_d: 4,
        vert_off_n: -8,
        vert_off_d: 4,
    };
    let mut info = ImageInfo {
        width: 65,
        height: 67,
        clean_aperture: Some(aperture),
        ..ImageInfo::default()
    };
    assert_eq!(
        AnimationCrop::resolve(&mut info).unwrap(),
        Some(AnimationCrop {
            x: 3,
            y: 5,
            width: 49,
            height: 53
        })
    );
    for bad in [
        CleanAperture {
            width_d: 0,
            ..aperture
        },
        CleanAperture {
            width_n: 97,
            ..aperture
        },
        CleanAperture {
            width_n: 0,
            ..aperture
        },
        CleanAperture {
            horiz_off_n: i32::MAX,
            ..aperture
        },
        CleanAperture {
            horiz_off_n: i32::MIN,
            ..aperture
        },
        CleanAperture {
            horiz_off_n: -19,
            ..aperture
        },
    ] {
        let mut info = ImageInfo {
            width: 65,
            height: 67,
            clean_aperture: Some(bad),
            ..ImageInfo::default()
        };
        assert!(AnimationCrop::resolve(&mut info).is_err());
        assert_eq!((info.width, info.height), (65, 67));
    }
}

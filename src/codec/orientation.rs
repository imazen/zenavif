//! The `irot`/`imir` to EXIF-orientation mapping, and the
//! [`zencodec::OrientationHint`] preserve-vs-bake policy the decode adapters
//! share.

use zencodec::ImageInfo;
use zenpixels::PixelBuffer;

/// Convert AVIF rotation + mirror properties to EXIF orientation.
///
/// AVIF uses separate `irot` (rotation) and `imir` (mirror) boxes.
/// The display pipeline applies: mirror first, then rotate (both CCW).
pub(super) fn avif_to_orientation(
    rotation: Option<&zenavif_parse::ImageRotation>,
    mirror: Option<&zenavif_parse::ImageMirror>,
) -> zencodec::Orientation {
    use zencodec::Orientation;
    let angle = rotation.map(|r| r.angle).unwrap_or(0);
    match (mirror.map(|m| m.axis), angle) {
        (None, 0) => Orientation::Identity,
        (None, 90) => Orientation::Rotate270,
        (None, 180) => Orientation::Rotate180,
        (None, 270) => Orientation::Rotate90,
        (Some(0), 0) => Orientation::FlipH,
        (Some(0), 90) => Orientation::Transpose,
        (Some(0), 180) => Orientation::FlipV,
        (Some(0), 270) => Orientation::Transverse,
        (Some(1), 0) => Orientation::FlipV,
        (Some(1), 90) => Orientation::Transverse,
        (Some(1), 180) => Orientation::FlipH,
        (Some(1), 270) => Orientation::Transpose,
        _ => Orientation::Identity,
    }
}

// ── Orientation hint: Preserve (default) vs bake ────────────────────────────
//
// The zencodec adapter honors `OrientationHint` the same way zenjpeg and heic
// do, so the codecs report orientation consistently. zenavif's orientation
// source is the container's `irot`/`imir` transform boxes (NOT EXIF); the
// native decoder leaves pixels in stored orientation, so the adapter is what
// applies the transform when the caller asks for it.

/// Whether the orientation hint requests baking the image's orientation into
/// the decoded pixels. `Correct`/`CorrectAndTransform` bake; `Preserve`,
/// `ExactTransform`, and any future variant do not (the safe default — keep
/// pixels in stored orientation and report the orientation on `ImageInfo`).
/// Mirrors heic's and zenjpeg's policy so the codecs agree.
pub(super) fn will_auto_orient(hint: zencodec::OrientationHint) -> bool {
    use zencodec::OrientationHint;
    matches!(
        hint,
        OrientationHint::Correct | OrientationHint::CorrectAndTransform(_)
    )
}

/// The image's intrinsic orientation from its `irot`/`imir` container boxes —
/// the net transform that, applied to the stored pixels, yields the upright
/// (display) image. Equals what [`avif_to_orientation`] computes.
pub(super) fn intrinsic_orientation(native: &crate::image::ImageInfo) -> zencodec::Orientation {
    avif_to_orientation(native.rotation.as_ref(), native.mirror.as_ref())
}

/// Bake the resolved orientation into a decoded buffer when the hint is on the
/// bake path, and report the resulting `(orientation, width, height)` to put on
/// `ImageInfo` / `OutputInfo`.
///
/// - `Preserve` (default): pixels are untouched; report the stored dims + the
///   intrinsic orientation tag (callers apply it via `display_width/height`).
/// - bake path (`Correct`/`CorrectAndTransform`): physically apply the
///   intrinsic orientation (zenavif resolves the net transform to the intrinsic,
///   matching heic); report the upright display dims + `Orientation::Identity`.
///   A no-op bake (already-upright image) still reports `Identity` per the
///   `OrientationHint::bakes()` contract.
pub(super) fn bake_orientation(
    pixels: PixelBuffer,
    native: &crate::image::ImageInfo,
    hint: zencodec::OrientationHint,
) -> (PixelBuffer, zencodec::Orientation, u32, u32) {
    let intrinsic = intrinsic_orientation(native);
    if !will_auto_orient(hint) {
        let (w, h) = (pixels.width(), pixels.height());
        return (pixels, intrinsic, w, h);
    }
    let baked = if intrinsic.is_identity() {
        pixels
    } else {
        zenpixels_convert::orient::apply_orientation(pixels.as_slice(), intrinsic)
    };
    let (w, h) = (baked.width(), baked.height());
    (baked, zencodec::Orientation::Identity, w, h)
}

/// Resolve the dims + orientation tag to report on `ImageInfo`/`OutputInfo`
/// **without** a decoded buffer (probe paths). `native.width`/`height` are the
/// stored (coded) dims. Mirrors [`bake_orientation`]'s reporting.
pub(super) fn reported_dims_and_orientation(
    native: &crate::image::ImageInfo,
    hint: zencodec::OrientationHint,
) -> (u32, u32, zencodec::Orientation) {
    let intrinsic = intrinsic_orientation(native);
    if !will_auto_orient(hint) {
        return (native.width, native.height, intrinsic);
    }
    let (w, h) = intrinsic.output_dimensions(native.width, native.height);
    (w, h, zencodec::Orientation::Identity)
}

/// Rewrite an `ImageInfo` (built by [`convert_native_info`], which reports the
/// `Preserve` view: stored dims + intrinsic tag) into the resolved reporting for
/// `hint`. A no-op when the hint preserves; on the bake path it swaps to display
/// dims + `Orientation::Identity`.
pub(super) fn apply_reported_orientation(
    mut info: ImageInfo,
    native: &crate::image::ImageInfo,
    hint: zencodec::OrientationHint,
) -> ImageInfo {
    if !will_auto_orient(hint) {
        return info;
    }
    let (w, h, orientation) = reported_dims_and_orientation(native, hint);
    info.width = w;
    info.height = h;
    info.with_orientation(orientation)
}

/// Convert EXIF orientation to AVIF rotation raw code + mirror axis.
///
/// Inverse of [`avif_to_orientation`]. Returns `(rotation_code, mirror_axis)`.
/// Rotation codes: 0=0°, 1=90°CCW, 2=180°, 3=270°CCW.
#[cfg(feature = "encode")]
pub(super) fn orientation_to_avif(orientation: zencodec::Orientation) -> (Option<u8>, Option<u8>) {
    use zencodec::Orientation;
    match orientation {
        Orientation::Identity => (None, None),
        Orientation::FlipH => (Some(0), Some(0)), // mirror=0, no rotation
        Orientation::Rotate180 => (Some(2), None), // 180° CCW
        Orientation::FlipV => (Some(2), Some(0)), // mirror=0, 180° CCW
        Orientation::Transpose => (Some(1), Some(0)), // mirror=0, 90° CCW
        Orientation::Rotate90 => (Some(3), None), // 270° CCW = 90° CW
        Orientation::Transverse => (Some(3), Some(0)), // mirror=0, 270° CCW
        Orientation::Rotate270 => (Some(1), None), // 90° CCW = 270° CW
        _ => (None, None),
    }
}

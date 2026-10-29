//! Shared validation for the retained job and whole-raster startup.
use zencodec::encode::{EncodeDeclaration, EncodePlan, OrientationPlan};
use zencodec::{At, CodecError, ErrorCategory, ResourceLimits, UnsupportedOperation};
use zenpixels::PixelDescriptor;

pub(super) fn refusal(error: impl core::error::Error + Send + Sync + 'static) -> At<CodecError> {
    whereat::at!(CodecError::from_parts(
        Some("zenavif"),
        ErrorCategory::Request(zencodec::RequestError::Unsupported(
            UnsupportedOperation::EncodePlanning
        )),
        Box::new(error),
    ))
}

pub(super) fn color_supported(config: &crate::EncoderConfig) -> bool {
    config.icc_profile.is_none()
        && matches!(config.color_primaries, None | Some(1))
        && matches!(config.transfer_characteristics, None | Some(13))
        && config.pixel_range.is_none()
        && config.gain_map.is_none()
}

pub(super) fn plan(
    declaration: &EncodeDeclaration<'_>,
    orientation: Option<OrientationPlan>,
    applied: zencodec::Orientation,
    limits: &ResourceLimits,
) -> Result<EncodePlan, At<CodecError>> {
    // Native one-shot encoding does not consume ColorContext or verify exact
    // grids. Do not turn those unsupported declarations into successful plans.
    if declaration.color.is_some()
        || declaration.exact_depth.is_some()
        || !matches!(
            declaration.descriptor,
            PixelDescriptor::RGB8_SRGB | PixelDescriptor::RGBA8_SRGB
        )
    {
        return Err(refusal(UnsupportedOperation::EncodePlanning));
    }
    let mut plan = EncodePlan::whole_raster();
    plan.orientation = orientation;
    plan.validate(declaration).map_err(refusal)?;
    let (width, height) = applied.output_dimensions(declaration.width, declaration.height);
    limits.check_dimensions(width, height).map_err(|error| {
        use zencodec::CategorizedError as _;
        whereat::at!(CodecError::from_parts(
            Some("zenavif"),
            error.category(),
            Box::new(error)
        ))
    })?;
    Ok(plan)
}

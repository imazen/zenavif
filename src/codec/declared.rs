//! Shared validation for the retained job and whole-raster startup.
use zencodec::encode::{EncodeDeclaration, EncodePlan, OrientationPlan};
use zencodec::{At, CodecError, ErrorCategory, ResourceLimits, UnsupportedOperation};
use zenpixels::ImageSpec;

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
    declaration: &EncodeDeclaration,
    orientation: Option<OrientationPlan>,
    applied: zencodec::Orientation,
    limits: &ResourceLimits,
) -> Result<EncodePlan, At<CodecError>> {
    declaration.validate_spec().map_err(refusal)?;
    // The declared AVIF path binds sRGB raster input and cannot verify exact
    // grids.
    if declaration.exact_depth.is_some()
        || zencodec::encode::best_encode_format(
            declaration.descriptor.clone(),
            &[ImageSpec::rgb8_srgb(), ImageSpec::rgba8_srgb()],
        )
        .is_none()
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

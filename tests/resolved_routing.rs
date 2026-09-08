#![cfg(feature = "encode")]
use almost_enough::{StopToken, Unstoppable};
use imgref::Img;
use rgb::{Rgb, Rgba};
#[cfg(feature = "zenav1-aom-encode")]
use zenavif::EncodeBitDepth;
use zenavif::backend_router::{
    BackendSelection, Effort, ParityReference, RouteCalibration, RouteReason, RoutingRequest,
    StillPolicy,
};
use zenavif::{Av1Backend, EncodeChromaSubsampling, EncoderConfig, PlanInput};
fn stop() -> StopToken {
    StopToken::new(Unstoppable)
}
fn config(backend: Av1Backend) -> EncoderConfig {
    EncoderConfig::new()
        .backend(backend)
        .speed(10)
        .threads(Some(1))
        .chroma_subsampling(EncodeChromaSubsampling::Yuv420)
}
#[test]
fn rejects_invalid_effort_and_incompatible_parity_before_encoding() {
    for e in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert!(Effort::new(e).is_err());
    }
    let request = RoutingRequest::default()
        .with_policy(StillPolicy::SvtParity(ParityReference::Mainline420))
        .with_backend(BackendSelection::Explicit(Av1Backend::Zenravif));
    assert!(
        config(Av1Backend::Zenravif)
            .resolve_route(request, PlanInput::rgb8(32, 32))
            .is_err()
    );
    assert!(
        config(Av1Backend::Zenravif)
            .resolve_route(RoutingRequest::default(), PlanInput::rgb8(0, 32))
            .is_err()
    );
}
#[test]
fn uncalibrated_route_executes_preferred_backend_and_rejects_changed_input() {
    let cfg = config(Av1Backend::Zenravif);
    let route = cfg
        .resolve_route(RoutingRequest::default(), PlanInput::rgb8(32, 32))
        .unwrap();
    assert_eq!(route.reason(), RouteReason::PreferredBackendSupported);
    assert_eq!(route.calibration(), RouteCalibration::Unavailable);
    let image = Img::new(vec![Rgb::new(80, 100, 120); 32 * 32], 32, 32);
    let routed = route.encode_rgb8(image.as_ref(), stop()).unwrap();
    let explicit = zenavif::encode_rgb8(image.as_ref(), &cfg, stop()).unwrap();
    assert_eq!(routed.avif_file, explicit.avif_file);
    zenavif::decode(&routed.avif_file).unwrap();
    let wrong = Img::new(vec![Rgb::new(80, 100, 120); 16 * 16], 16, 16);
    assert!(route.encode_rgb8(wrong.as_ref(), stop()).is_err());
    let wrong_kind = Img::new(vec![Rgba::new(80, 100, 120, 128); 32 * 32], 32, 32);
    assert!(route.encode_rgba8(wrong_kind.as_ref(), stop()).is_err());
}
#[cfg(feature = "zenav1-aom-encode")]
#[test]
fn twelve_bit_requirement_routes_to_aom_and_replays_the_exact_configuration() {
    let cfg = config(Av1Backend::Zenav1Svt).bit_depth(EncodeBitDepth::Twelve);
    let route = cfg
        .resolve_route(RoutingRequest::default(), PlanInput::rgb8(32, 32))
        .unwrap();
    assert_eq!(route.backend(), Av1Backend::Zenav1Aom);
    assert_eq!(route.reason(), RouteReason::CapabilityFallback);
    let img = Img::new(vec![Rgb::new(80, 100, 120); 32 * 32], 32, 32);
    let routed = route.encode_rgb8(img.as_ref(), stop()).unwrap();
    let explicit = zenavif::encode_rgb8(img.as_ref(), route.config(), stop()).unwrap();
    assert_eq!(routed.avif_file, explicit.avif_file);
    let parser = zenavif_parse::AvifParser::from_owned_with_config(
        routed.avif_file.clone(),
        &Default::default(),
        &Unstoppable,
    )
    .unwrap();
    assert_eq!(parser.primary_metadata().unwrap().bit_depth, 12);
    zenavif::decode(&routed.avif_file).unwrap();
    let exact =
        RoutingRequest::default().with_backend(BackendSelection::Explicit(Av1Backend::Zenav1Svt));
    assert!(cfg.resolve_route(exact, PlanInput::rgb8(32, 32)).is_err());
}
#[cfg(feature = "zenav1-aom-encode")]
#[test]
fn alpha_requirement_falls_back_and_actual_rgba_encode_keeps_alpha() {
    let cfg = config(Av1Backend::Zenav1Aom);
    let route = cfg
        .resolve_route(RoutingRequest::default(), PlanInput::rgba8(32, 32))
        .unwrap();
    assert_ne!(route.backend(), Av1Backend::Zenav1Aom);
    let img = Img::new(vec![Rgba::new(80, 100, 120, 128); 32 * 32], 32, 32);
    let out = route.encode_rgba8(img.as_ref(), stop()).unwrap();
    assert!(out.alpha_byte_size > 0);
    zenavif::decode(&out.avif_file).unwrap();
}
#[cfg(feature = "zenav1-svt")]
#[test]
fn parity_and_native_research_resolution_are_wired_into_real_svt_execution() {
    let cfg = config(Av1Backend::Zenravif);
    let request = RoutingRequest::default()
        .with_policy(StillPolicy::SvtParity(ParityReference::Mainline420))
        .with_effort(Effort::new(1.0).unwrap());
    let route = cfg.resolve_route(request, PlanInput::rgb8(32, 32)).unwrap();
    assert_eq!(route.backend(), Av1Backend::Zenav1Svt);
    assert_eq!(route.reason(), RouteReason::StrictParity);
    assert_eq!(route.primary_native_settings().value, -1);
    assert!(
        route
            .primary_native_settings()
            .reference
            .unwrap()
            .contains("svt-mainline-4.2.0")
    );
    assert!(
        route
            .config()
            .clone()
            .backend(Av1Backend::Zenravif)
            .validate()
            .is_err()
    );
    let img = Img::new(vec![Rgb::new(80, 100, 120); 32 * 32], 32, 32);
    let out = route.encode_rgb8(img.as_ref(), stop()).unwrap();
    zenavif::decode(&out.avif_file).unwrap();
    assert!(
        cfg.resolve_route(request, PlanInput::rgba8(32, 32))
            .is_err()
    );
}
#[cfg(not(feature = "zenav1-svt"))]
#[test]
fn unavailable_parity_backend_cannot_silently_route_to_rav1e() {
    let request =
        RoutingRequest::default().with_policy(StillPolicy::SvtParity(ParityReference::Mainline420));
    assert!(
        config(Av1Backend::Zenravif)
            .resolve_route(request, PlanInput::rgb8(32, 32))
            .is_err()
    );
}

#[cfg(all(feature = "zenav1-aom-encode", feature = "zenav1-svt"))]
#[test]
fn both_deep_input_entry_points_execute_and_preserve_requested_depth_and_alpha() {
    let deep = PlanInput {
        width: 32,
        height: 32,
        input_is_16bit: true,
        input_has_alpha: false,
    };
    let cfg = config(Av1Backend::Zenav1Svt).bit_depth(EncodeBitDepth::Twelve);
    let route = cfg.resolve_route(RoutingRequest::default(), deep).unwrap();
    assert_eq!(route.backend(), Av1Backend::Zenav1Aom);
    let rgb = Img::new(vec![Rgb::new(12345u16, 23456, 34567); 1024], 32, 32);
    let out = route.encode_rgb16(rgb.as_ref(), stop()).unwrap();
    let parser = zenavif_parse::AvifParser::from_owned_with_config(
        out.avif_file.clone(),
        &Default::default(),
        &Unstoppable,
    )
    .unwrap();
    assert_eq!(parser.primary_metadata().unwrap().bit_depth, 12);
    zenavif::decode(&out.avif_file).unwrap();
    let rgba = Img::new(vec![Rgba::new(12345u16, 23456, 34567, 32768); 1024], 32, 32);
    let route = config(Av1Backend::Zenav1Aom)
        .resolve_route(
            RoutingRequest::default(),
            PlanInput {
                input_has_alpha: true,
                ..deep
            },
        )
        .unwrap();
    assert_eq!(route.backend(), Av1Backend::Zenav1Svt);
    let out = route.encode_rgba16(rgba.as_ref(), stop()).unwrap();
    assert!(out.alpha_byte_size > 0);
    zenavif::decode(&out.avif_file).unwrap();
}

#[cfg(all(feature = "zenav1-svt", feature = "__expert"))]
#[test]
fn fingerprints_distinguish_native_effort_and_pinned_reference() {
    let cfg = config(Av1Backend::Zenav1Svt);
    let input = PlanInput::rgb8(32, 32);
    let route = |reference, effort| {
        cfg.resolve_route(
            RoutingRequest::default()
                .with_policy(StillPolicy::SvtParity(reference))
                .with_effort(Effort::new(effort).unwrap()),
            input,
        )
        .unwrap()
    };
    let main = route(ParityReference::Mainline420, 1.0);
    let hybrid = route(ParityReference::Hybrid3115, 1.0);
    let fast = route(ParityReference::Mainline420, 0.0);
    assert_ne!(
        zenavif::sweep::fingerprint(main.config()),
        zenavif::sweep::fingerprint(hybrid.config())
    );
    assert_ne!(
        zenavif::sweep::fingerprint(main.config()),
        zenavif::sweep::fingerprint(fast.config())
    );
}

#[cfg(feature = "zenav1-svt")]
#[test]
fn explicit_zen_enhancement_is_encoded_and_cannot_be_dropped_by_routing() {
    use zenavif::backend_router::SvtEnhancement;
    let cfg = config(Av1Backend::Zenav1Svt);
    let input = PlanInput::rgb8(32, 32);
    let request = RoutingRequest::default().with_effort(Effort::new(1.0).unwrap());
    let plain = cfg.resolve_route(request, input).unwrap();
    let enhanced_request = request.with_svt_enhancement(SvtEnhancement::AomIntraEdgeFilter);
    let enhanced = cfg.resolve_route(enhanced_request, input).unwrap();
    let replanned = enhanced
        .config()
        .resolve_route(RoutingRequest::default(), input)
        .unwrap();
    assert_eq!(
        replanned.primary_native_settings(),
        enhanced.primary_native_settings()
    );
    assert_eq!(replanned.svt_enhancements(), enhanced.svt_enhancements());
    assert!(
        enhanced
            .svt_enhancements()
            .contains(SvtEnhancement::AomIntraEdgeFilter)
    );
    let image = Img::new(vec![Rgb::new(80, 100, 120); 1024], 32, 32);
    let a = plain.encode_rgb8(image.as_ref(), stop()).unwrap();
    let b = enhanced.encode_rgb8(image.as_ref(), stop()).unwrap();
    assert_ne!(a.avif_file, b.avif_file);
    zenavif::decode(&b.avif_file).unwrap();
    assert!(
        cfg.resolve_route(
            enhanced_request.with_backend(BackendSelection::Explicit(Av1Backend::Zenravif)),
            input
        )
        .is_err()
    );
    assert!(
        cfg.resolve_route(
            enhanced_request.with_policy(StillPolicy::SvtParity(ParityReference::Mainline420)),
            input
        )
        .is_err()
    );
    assert!(
        cfg.resolve_route(
            enhanced_request.with_effort(Effort::new(0.0).unwrap()),
            input
        )
        .is_err()
    );
    #[cfg(feature = "__expert")]
    assert_ne!(
        zenavif::sweep::fingerprint(plain.config()),
        zenavif::sweep::fingerprint(enhanced.config())
    );
}

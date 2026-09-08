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
/// **Alpha no longer forces a fallback away from aom — it is served in place.**
///
/// This test used to assert `assert_ne!(route.backend(), Zenav1Aom)`, because
/// the aom seam refused RGBA outright and the router had to route around it.
/// The seam now emits the auxiliary Cs400 full-range item AVIF requires, so
/// routing away would be routing away from a backend that supports the
/// request. The assertion is inverted rather than deleted: the property worth
/// gating was never "some other backend runs", it is that **the encode keeps
/// the alpha**, and that half is unchanged and still asserted below.
#[cfg(feature = "zenav1-aom-encode")]
#[test]
fn alpha_is_served_by_aom_in_place_and_the_rgba_encode_keeps_alpha() {
    let cfg = config(Av1Backend::Zenav1Aom);
    let route = cfg
        .resolve_route(RoutingRequest::default(), PlanInput::rgba8(32, 32))
        .unwrap();
    assert_eq!(
        route.backend(),
        Av1Backend::Zenav1Aom,
        "aom supports auxiliary alpha now; routing away from it would be a stale capability model"
    );
    assert_eq!(
        route.reason(),
        RouteReason::PreferredBackendSupported,
        "and the reason must say the preferred backend was supported, not that it fell back"
    );
    let img = Img::new(vec![Rgba::new(80, 100, 120, 128); 32 * 32], 32, 32);
    let out = route.encode_rgba8(img.as_ref(), stop()).unwrap();
    assert!(out.alpha_byte_size > 0, "the auxiliary alpha item must carry bytes");
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

#[cfg(all(
    feature = "encode-mono",
    feature = "zenav1-svt",
    feature = "zenav1-aom-encode"
))]
#[test]
fn monochrome_query_encode_and_strided_input_agree() {
    use zenavif::backend_router::{StillInput, query_still_backends};
    let input = StillInput::Gray8 {
        width: 17,
        height: 19,
    };
    let pixels: Vec<u8> = (0..23 * 19)
        .map(|i| ((i * 31 + i / 23) % 256) as u8)
        .collect();
    let padded = imgref::Img::new_stride(pixels, 17, 19, 23);
    let tight = Img::new(padded.rows().flatten().copied().collect::<Vec<_>>(), 17, 19);
    for backend in [
        Av1Backend::Zenravif,
        Av1Backend::Zenav1Svt,
        Av1Backend::Zenav1Aom,
    ] {
        for depth in [
            EncodeBitDepth::Eight,
            EncodeBitDepth::Ten,
            EncodeBitDepth::Twelve,
        ] {
            let cfg = config(backend).bit_depth(depth);
            let report = query_still_backends(&cfg, input)
                .into_iter()
                .find(|r| r.backend == backend)
                .unwrap();
            let encoded = zenavif::encode_gray8(padded.as_ref(), &cfg, stop());
            assert_eq!(
                report.supported(),
                encoded.is_ok(),
                "{backend:?} {depth:?}: {report:?} {encoded:?}"
            );
            if report.supported() {
                let route = cfg
                    .resolve_route(
                        RoutingRequest::default().with_backend(BackendSelection::Explicit(backend)),
                        input,
                    )
                    .unwrap();
                let bytes = route
                    .encode_gray8(tight.as_ref(), stop())
                    .unwrap()
                    .avif_file;
                assert_eq!(bytes, encoded.unwrap().avif_file);
                zenavif::decode(&bytes).unwrap();
                let rgb = Img::new(vec![Rgb::new(0, 0, 0); 17 * 19], 17, 19);
                assert!(route.encode_rgb8(rgb.as_ref(), stop()).is_err());
            }
        }
    }
    let research = config(Av1Backend::Zenav1Svt)
        .resolve_route(
            RoutingRequest::default()
                .with_backend(BackendSelection::Explicit(Av1Backend::Zenav1Svt))
                .with_effort(Effort::new(1.0).unwrap()),
            input,
        )
        .unwrap();
    assert_eq!(research.primary_native_settings().value, -1);
    zenavif::decode(
        &research
            .encode_gray8(tight.as_ref(), stop())
            .unwrap()
            .avif_file,
    )
    .unwrap();
    assert!(
        config(Av1Backend::Zenav1Svt)
            .resolve_route(
                RoutingRequest::default()
                    .with_policy(StillPolicy::SvtParity(ParityReference::Mainline420)),
                input
            )
            .is_err()
    );
}

#[cfg(not(feature = "encode-mono"))]
#[test]
fn missing_monochrome_feature_is_reported_before_selection() {
    let source = zenavif::backend_router::StillInput::Gray8 {
        width: 17,
        height: 19,
    };
    assert!(
        config(Av1Backend::Zenravif)
            .resolve_route(RoutingRequest::default(), source)
            .is_err()
    );
}

#[cfg(feature = "routing-replay")]
#[test]
fn portable_replay_preserves_bytes_metadata_and_complete_cache_identity() {
    use zenavif::backend_router::ResolvedRoute;
    let build = "test-build-manifest-sha256";
    let cfg = config(Av1Backend::Zenravif)
        .xmp(b"<x:xmpmeta>test</x:xmpmeta>".to_vec())
        .content_light_level(1000, 400);
    let source = PlanInput::rgba8(17, 19);
    let route = cfg
        .resolve_route(RoutingRequest::default(), source)
        .unwrap();
    let json = route.to_replay_json(build).unwrap();
    let restored = ResolvedRoute::from_replay_json(&json, build).unwrap();
    assert_eq!(restored.to_replay_json(build).unwrap(), json);
    let image = Img::new(
        (0..17 * 19)
            .map(|i| Rgba::new(i as u8, (i * 13) as u8, (i * 17) as u8, 128))
            .collect::<Vec<_>>(),
        17,
        19,
    );
    let bytes = route
        .encode_rgba8(image.as_ref(), stop())
        .unwrap()
        .avif_file;
    assert_eq!(
        restored
            .encode_rgba8(image.as_ref(), stop())
            .unwrap()
            .avif_file,
        bytes
    );
    zenavif::decode(&bytes).unwrap();
    let key = route.cache_key(build, [0; 32]).unwrap();
    assert_eq!(key, restored.cache_key(build, [0; 32]).unwrap());
    assert_ne!(key, route.cache_key(build, [1; 32]).unwrap());
    assert_ne!(key, route.cache_key("different-build", [0; 32]).unwrap());
    let changed = cfg
        .xmp(b"different-metadata".to_vec())
        .resolve_route(RoutingRequest::default(), source)
        .unwrap();
    assert_ne!(key, changed.cache_key(build, [0; 32]).unwrap());
    assert!(ResolvedRoute::from_replay_json(&json, "different-build").is_err());
    for (field, value) in [
        ("schema", serde_json::json!(999)),
        ("features", serde_json::json!(["not-compiled"])),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut record: serde_json::Value = serde_json::from_str(&json).unwrap();
        record[field] = value;
        assert!(
            ResolvedRoute::from_replay_json(&record.to_string(), build).is_err(),
            "{field}"
        );
    }
    let mut record: serde_json::Value = serde_json::from_str(&json).unwrap();
    record["config"]["unexpected_control"] = serde_json::json!(true);
    assert!(ResolvedRoute::from_replay_json(&record.to_string(), build).is_err());
    let unpinned = config(Av1Backend::Zenravif)
        .threads(None)
        .resolve_route(RoutingRequest::default(), source)
        .unwrap();
    assert!(unpinned.to_replay_json(build).is_err());
}

#[cfg(all(feature = "routing-replay", feature = "zenav1-svt"))]
#[test]
fn portable_replay_preserves_signed_preset_reference_and_enhancements() {
    use zenavif::backend_router::{ResolvedRoute, SvtEnhancement};
    let image = Img::new(
        (0..32 * 32)
            .map(|i| Rgb::new(i as u8, (i * 13) as u8, (i * 17) as u8))
            .collect::<Vec<_>>(),
        32,
        32,
    );
    for request in [
        RoutingRequest::default().with_policy(StillPolicy::SvtParity(ParityReference::Mainline420)),
        RoutingRequest::default().with_policy(StillPolicy::SvtParity(ParityReference::Hybrid3115)),
        RoutingRequest::default().with_svt_enhancement(SvtEnhancement::AomIntraEdgeFilter),
    ] {
        let route = config(Av1Backend::Zenav1Svt)
            .resolve_route(
                request.with_effort(Effort::new(1.0).unwrap()),
                PlanInput::rgb8(32, 32),
            )
            .unwrap();
        let json = route.to_replay_json("test-svt-build").unwrap();
        let restored = ResolvedRoute::from_replay_json(&json, "test-svt-build").unwrap();
        assert_eq!(
            restored.primary_native_settings(),
            route.primary_native_settings()
        );
        assert_eq!(restored.svt_enhancements(), route.svt_enhancements());
        assert_eq!(
            restored
                .encode_rgb8(image.as_ref(), stop())
                .unwrap()
                .avif_file,
            route.encode_rgb8(image.as_ref(), stop()).unwrap().avif_file
        );
        let mut record: serde_json::Value = serde_json::from_str(&json).unwrap();
        let original = record.clone();
        record["effort"] = serde_json::json!(0.5);
        assert!(ResolvedRoute::from_replay_json(&record.to_string(), "test-svt-build").is_err());
        record = original;
        record["config"]["svt_route_preset"] = serde_json::json!(-2);
        assert!(ResolvedRoute::from_replay_json(&record.to_string(), "test-svt-build").is_err());
    }
}

#[cfg(all(
    feature = "zenav1-svt",
    feature = "zenav1-aom-encode",
    feature = "encode-imazen"
))]
#[test]
fn support_audit_refuses_ignored_controls_and_preserves_transfer_codes() {
    let source = PlanInput::rgb8(16, 16);
    let image = Img::new(vec![Rgb::new(64, 96, 128); 256], 16, 16);
    for backend in [Av1Backend::Zenav1Svt, Av1Backend::Zenav1Aom] {
        for cfg in [
            config(backend).matrix_coefficients(1),
            config(backend).color_primaries(5),
            config(backend).with_vaq(true, 2.0),
        ] {
            assert!(cfg.validate_still_input(source).is_err());
            assert!(zenavif::encode_rgb8(image.as_ref(), &cfg, stop()).is_err());
        }
        for tc in [1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18] {
            let cfg = config(backend).transfer_characteristics(tc);
            cfg.validate_still_input(source).unwrap();
            let encoded = zenavif::encode_rgb8(image.as_ref(), &cfg, stop()).unwrap();
            let parser = zenavif_parse::AvifParser::from_owned_with_config(
                encoded.avif_file.clone(),
                &Default::default(),
                &Unstoppable,
            )
            .unwrap();
            let actual_tc = match parser.nclx_color_info() {
                Some(zenavif_parse::ColorInformation::Nclx {
                    transfer_characteristics,
                    ..
                }) => *transfer_characteristics as u8,
                None => parser.primary_metadata().unwrap().transfer_characteristics,
                _ => panic!("nclx query returned ICC"),
            };
            assert_eq!(actual_tc, tc, "{backend:?}");
            zenavif::decode(&encoded.avif_file).unwrap();
        }
        let cfg = config(backend).quality(0.0);
        cfg.validate_still_input(source).unwrap();
        zenavif::encode_rgb8(image.as_ref(), &cfg, stop()).unwrap();
    }
}

#[cfg(all(feature = "zenav1-svt", feature = "zenav1-aom-encode"))]
#[test]
fn adapter_preserves_icc_and_explicit_cicp_independently() {
    let image = Img::new(vec![Rgb::new(80, 100, 120); 16 * 16], 16, 16);
    let icc = vec![42; 128]; // opaque metadata payload: this test does not interpret the profile
    for backend in [Av1Backend::Zenav1Svt, Av1Backend::Zenav1Aom] {
        let cfg = config(backend)
            .color_primaries(9)
            .transfer_characteristics(16)
            .icc_profile(icc.clone());
        let route = cfg
            .resolve_route(
                RoutingRequest::default().with_backend(BackendSelection::Explicit(backend)),
                PlanInput::rgb8(16, 16),
            )
            .unwrap();
        let encoded = route.encode_rgb8(image.as_ref(), stop()).unwrap();
        let parser = zenavif_parse::AvifParser::from_owned_with_config(
            encoded.avif_file,
            &Default::default(),
            &Unstoppable,
        )
        .unwrap();
        assert!(
            matches!(parser.color_info(), Some(zenavif_parse::ColorInformation::IccProfile(bytes)) if bytes == &icc)
        );
        assert!(matches!(
            parser.nclx_color_info(),
            Some(zenavif_parse::ColorInformation::Nclx {
                color_primaries: 9,
                transfer_characteristics: 16,
                ..
            })
        ));
    }
}

#[cfg(all(feature = "encode-mono", feature = "zenav1-aom-encode"))]
#[test]
fn monochrome_aom_range_matches_displayed_gray_samples() {
    for value in [0, 16, 64, 128, 235, 255] {
        let image = Img::new(vec![value; 16 * 16], 16, 16);
        let cfg = config(Av1Backend::Zenav1Aom).quality(100.0);
        let encoded = zenavif::encode_gray8(image.as_ref(), &cfg, stop()).unwrap();
        let decoded = zenavif::decode(&encoded.avif_file).unwrap();
        let rgb = decoded.try_as_imgref::<Rgb<u8>>().unwrap();
        for pixel in rgb.rows().flatten() {
            assert_eq!(*pixel, Rgb::new(value, value, value), "input gray={value}");
        }
    }
}

#[cfg(all(
    feature = "zenav1-svt",
    feature = "routing-replay",
    feature = "encode-mono"
))]
#[test]
fn film_grain_is_queried_encoded_replayed_and_never_dropped() {
    use zenavif::backend_router::{
        ResolvedRoute, StillInput, SvtFilmGrainConfig, SvtFilmGrainTable,
    };
    let mut table = SvtFilmGrainTable {
        apply_grain: true,
        num_y_points: 2,
        scaling_shift: 8,
        ar_coeff_shift: 6,
        ..Default::default()
    };
    table.scaling_points_y[0] = [0, 80];
    table.scaling_points_y[1] = [255, 80];
    let grain = SvtFilmGrainConfig {
        table: Some(table),
        ..Default::default()
    };
    let cfg = config(Av1Backend::Zenav1Svt).with_svt_film_grain(grain);
    let source = PlanInput::rgb8(32, 32);
    let route = cfg
        .resolve_route(
            RoutingRequest::default()
                .with_policy(StillPolicy::SvtParity(ParityReference::Mainline420)),
            source,
        )
        .unwrap();
    let image = Img::new(vec![Rgb::new(128, 128, 128); 32 * 32], 32, 32);
    let encoded = route.encode_rgb8(image.as_ref(), stop()).unwrap();
    let plain_route = config(Av1Backend::Zenav1Svt)
        .resolve_route(
            RoutingRequest::default()
                .with_policy(StillPolicy::SvtParity(ParityReference::Mainline420)),
            source,
        )
        .unwrap();
    let plain = plain_route.encode_rgb8(image.as_ref(), stop()).unwrap();
    assert_ne!(plain.avif_file, encoded.avif_file);
    let decoded = zenavif::decode(&encoded.avif_file).unwrap();
    assert!(
        decoded
            .try_as_imgref::<Rgb<u8>>()
            .unwrap()
            .rows()
            .flatten()
            .any(|p| *p != Rgb::new(128, 128, 128)),
        "grain must reach displayed samples"
    );
    let record = route.to_replay_json("grain-test-build").unwrap();
    let replay = ResolvedRoute::from_replay_json(&record, "grain-test-build").unwrap();
    assert_eq!(
        replay
            .encode_rgb8(image.as_ref(), stop())
            .unwrap()
            .avif_file,
        encoded.avif_file
    );
    assert_ne!(
        route.cache_key("grain-test-build", [0; 32]).unwrap(),
        plain_route.cache_key("grain-test-build", [0; 32]).unwrap()
    );
    #[cfg(feature = "__expert")]
    assert_ne!(
        zenavif::sweep::fingerprint(route.config()),
        zenavif::sweep::fingerprint(plain_route.config())
    );
    assert!(
        cfg.resolve_route(
            RoutingRequest::default()
                .with_backend(BackendSelection::Explicit(Av1Backend::Zenravif)),
            source
        )
        .is_err()
    );
    assert!(
        cfg.resolve_route(
            RoutingRequest::default(),
            StillInput::Gray8 {
                width: 32,
                height: 32
            }
        )
        .is_err()
    );
    let invalid = config(Av1Backend::Zenav1Svt).with_svt_film_grain(SvtFilmGrainConfig {
        denoise_strength: 51,
        ..Default::default()
    });
    assert!(
        invalid
            .resolve_route(RoutingRequest::default(), source)
            .is_err()
    );
    assert!(zenavif::encode_rgb8(image.as_ref(), &invalid, stop()).is_err());
}

#[cfg(all(
    feature = "encode-mono",
    feature = "zenav1-svt",
    feature = "zenav1-aom-encode"
))]
#[test]
fn support_matrix_matches_all_five_pixel_entry_points() {
    use zenavif::backend_router::{StillInput, query_still_backends};
    let rgb8 = Img::new(vec![Rgb::new(60u8, 100, 140); 17 * 19], 17, 19);
    let rgba8 = Img::new(vec![Rgba::new(60u8, 100, 140, 128); 17 * 19], 17, 19);
    let rgb16 = Img::new(vec![Rgb::new(15420u16, 25700, 35980); 17 * 19], 17, 19);
    let rgba16 = Img::new(
        vec![Rgba::new(15420u16, 25700, 35980, 32896); 17 * 19],
        17,
        19,
    );
    let gray = Img::new(vec![100u8; 17 * 19], 17, 19);
    let mut cells = 0;
    for backend in [
        Av1Backend::Zenravif,
        Av1Backend::Zenav1Svt,
        Av1Backend::Zenav1Aom,
    ] {
        for depth in [
            EncodeBitDepth::Eight,
            EncodeBitDepth::Ten,
            EncodeBitDepth::Twelve,
        ] {
            for chroma in [
                EncodeChromaSubsampling::Yuv420,
                EncodeChromaSubsampling::Yuv444,
            ] {
                for input in [
                    StillInput::Rgb8 {
                        width: 17,
                        height: 19,
                    },
                    StillInput::Rgba8 {
                        width: 17,
                        height: 19,
                    },
                    StillInput::Rgb16 {
                        width: 17,
                        height: 19,
                    },
                    StillInput::Rgba16 {
                        width: 17,
                        height: 19,
                    },
                    StillInput::Gray8 {
                        width: 17,
                        height: 19,
                    },
                ] {
                    let cfg = config(backend).bit_depth(depth).chroma_subsampling(chroma);
                    let report = query_still_backends(&cfg, input)
                        .into_iter()
                        .find(|r| r.backend == backend)
                        .unwrap();
                    let result = match input {
                        StillInput::Rgb8 { .. } => {
                            zenavif::encode_rgb8(rgb8.as_ref(), &cfg, stop())
                        }
                        StillInput::Rgba8 { .. } => {
                            zenavif::encode_rgba8(rgba8.as_ref(), &cfg, stop())
                        }
                        StillInput::Rgb16 { .. } => {
                            zenavif::encode_rgb16(rgb16.as_ref(), &cfg, stop())
                        }
                        StillInput::Rgba16 { .. } => {
                            zenavif::encode_rgba16(rgba16.as_ref(), &cfg, stop())
                        }
                        StillInput::Gray8 { .. } => {
                            zenavif::encode_gray8(gray.as_ref(), &cfg, stop())
                        }
                    };
                    assert_eq!(
                        report.supported(),
                        result.is_ok(),
                        "{backend:?} {depth:?} {chroma:?} {input:?}: {report:?} {result:?}"
                    );
                    if let Ok(encoded) = result {
                        zenavif::decode(&encoded.avif_file).unwrap();
                    }
                    cells += 1;
                }
            }
        }
    }
    assert_eq!(cells, 90);
    assert!(
        config(Av1Backend::Zenravif)
            .bit_depth(EncodeBitDepth::Twelve)
            .chroma_subsampling(EncodeChromaSubsampling::Yuv444)
            .resolve_route(RoutingRequest::default(), PlanInput::rgb8(17, 19))
            .is_err()
    );
}

#[cfg(all(feature = "zenav1-svt", feature = "__expert"))]
#[test]
fn strict_still_policy_refuses_the_legacy_tune_number_collision() {
    let mut params = zenavif::expert::SvtParams::default();
    params.tune = 5; // Adapter FilmGrain versus C's all-intra-refused VMAF.
    let cfg = config(Av1Backend::Zenav1Svt).with_svt_params(params);
    assert!(
        cfg.resolve_route(
            RoutingRequest::default()
                .with_policy(StillPolicy::SvtParity(ParityReference::Mainline420)),
            PlanInput::rgb8(16, 16)
        )
        .is_err()
    );
    params.tune = 255;
    assert!(
        config(Av1Backend::Zenav1Svt)
            .with_svt_params(params)
            .validate_still_input(PlanInput::rgb8(16, 16))
            .is_err()
    );
    params.tune = 1;
    params.ac_bias = f64::NAN;
    assert!(
        config(Av1Backend::Zenav1Svt)
            .with_svt_params(params)
            .validate_still_input(PlanInput::rgb8(16, 16))
            .is_err()
    );
}

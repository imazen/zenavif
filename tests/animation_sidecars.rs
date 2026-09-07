#[path = "support/animation.rs"]
mod animation_support;
use zenavif::{DecoderConfig, ManagedAvifDecoder};
use zenavif_serialize::animated::{AnimFrame, AnimatedImage};
const SAMPLE: &[u8] = &[
    10, 14, 2, 0, 0, 44, 29, 229, 101, 64, 39, 145, 2, 2, 2, 132, 50, 28, 16, 0, 136, 0, 6, 24, 32,
    130, 8, 36, 0, 173, 64, 89, 124, 214, 69, 97, 11, 21, 193, 8, 150, 63, 234, 231, 8, 96,
];
const EXIF: &[u8] = b"II\x2a\0\x08\0\0\0\0\0\0\0\0\0";
const XMP: &[u8] = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>";
#[test]
fn track_sidecars_survive_without_a_poster() {
    let mut mux = AnimatedImage::new();
    mux.set_exif(EXIF.to_vec()).set_xmp(XMP.to_vec());
    let mut data = mux
        .try_serialize(
            150,
            150,
            &[AnimFrame::new(SAMPLE, 17).with_sync(true)],
            &SAMPLE[..16],
            None,
        )
        .unwrap();
    animation_support::remove_poster(&mut data);
    let decoder = ManagedAvifDecoder::new(&data, &DecoderConfig::default()).unwrap();
    let info = decoder.probe_animation_info().unwrap();
    assert_eq!(info.exif.as_deref(), Some(EXIF));
    assert_eq!(info.xmp.as_deref(), Some(XMP));
}

fn original() -> Vec<u8> {
    let mut mux = AnimatedImage::new();
    mux.set_exif(EXIF.to_vec()).set_xmp(XMP.to_vec());
    mux.try_serialize(
        150,
        150,
        &[
            AnimFrame::new(SAMPLE, 17).with_sync(true),
            AnimFrame::new(SAMPLE, 33).with_sync(true),
        ],
        &SAMPLE[..16],
        None,
    )
    .unwrap()
}
fn positions(data: &[u8], fourcc: &[u8]) -> Vec<usize> {
    data.windows(fourcc.len())
        .enumerate()
        .filter_map(|(i, b)| (b == fourcc).then_some(i))
        .collect()
}
#[test]
fn track_and_poster_sidecars_stay_independent_across_decoders() {
    use std::borrow::Cow;
    use zenavif::{AnimationDecoder, AvifDecoderConfig, DecodeBackend, Unstoppable};
    use zencodec::decode::{AnimationFrameDecoder as _, DecodeJob as _, DecoderConfig as _};
    for scenario in 0..6 {
        let mut data = original();
        let idats = positions(&data, b"idat");
        assert_eq!(idats.len(), 2);
        let mut poster_exif = EXIF.to_vec();
        let mut poster_xmp = XMP.to_vec();
        // Preserve lengths and offsets, but give the poster distinct payloads.
        poster_exif[8] = 1;
        poster_xmp[XMP.len() - 3] = b' ';
        data[idats[0] + 8..idats[0] + 8 + EXIF.len()].copy_from_slice(&poster_exif);
        data[idats[0] + 8 + EXIF.len()..idats[0] + 8 + EXIF.len() + XMP.len()]
            .copy_from_slice(&poster_xmp);
        if scenario == 1 {
            animation_support::remove_poster(&mut data);
        }
        if scenario == 2 {
            let metas = positions(&data, b"meta");
            // The real nested meta comes after moov, before the track's idat.
            let moov = positions(&data, b"moov")[0];
            let nested = *metas.iter().find(|&&p| p > moov && p < idats[1]).unwrap();
            data[nested..nested + 4].copy_from_slice(b"free");
        }
        if scenario == 3 {
            // A MIME item other than RDF/XML must not be exposed as XMP.
            let mime = positions(&data, b"application/rdf+xml");
            assert_eq!(mime.len(), 2);
            data[mime[1]..mime[1] + 18].copy_from_slice(b"application/notxmp");
        }
        if scenario >= 4 {
            // Put track metadata in a separate top-level mdat; leave poster idat
            // untouched. Scenario 5 adds padding before the TIFF header.
            let padding = if scenario == 5 {
                b"Exif\0\0".as_slice()
            } else {
                &[]
            };
            let mut payload = (padding.len() as u32).to_be_bytes().to_vec();
            payload.extend_from_slice(padding);
            payload.extend_from_slice(EXIF);
            let exif_len = payload.len();
            payload.extend_from_slice(XMP);
            let offset = data.len() + 8;
            let iloc = positions(&data, b"iloc")[1];
            assert_eq!(&data[iloc + 4..iloc + 12], &[1, 0, 0, 0, 0x44, 0, 0, 2]);
            for (entry, start, length) in [
                (iloc + 12, offset, exif_len),
                (iloc + 28, offset + exif_len, XMP.len()),
            ] {
                data[entry + 2..entry + 4].copy_from_slice(&0u16.to_be_bytes());
                data[entry + 8..entry + 12].copy_from_slice(&(start as u32).to_be_bytes());
                data[entry + 12..entry + 16].copy_from_slice(&(length as u32).to_be_bytes());
            }
            data.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
            data.extend_from_slice(b"mdat");
            data.extend_from_slice(&payload);
        }
        if let Some(dir) = std::env::var_os("ZENAVIF_SIDECAR_ARTIFACTS") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                std::path::Path::new(&dir).join(format!("sidecars-{scenario}.avif")),
                &data,
            )
            .unwrap();
        }
        let want_exif = (scenario != 2).then_some(EXIF);
        let want_xmp = (scenario != 2 && scenario != 3).then_some(XMP);
        for parser in [
            zenavif_parse::AvifParser::from_bytes(&data).unwrap(),
            zenavif_parse::AvifParser::from_owned(data.clone()).unwrap(),
        ] {
            assert_eq!(
                parser.animation_exif().transpose().unwrap().as_deref(),
                want_exif
            );
            assert_eq!(
                parser.animation_xmp().transpose().unwrap().as_deref(),
                want_xmp
            );
            if scenario != 1 {
                assert_eq!(parser.exif().unwrap().unwrap().as_ref(), poster_exif);
                assert_eq!(parser.xmp().unwrap().unwrap().as_ref(), poster_xmp);
            }
        }
        for backend in [
            DecodeBackend::Rav1dSafe,
            #[cfg(feature = "zenav1-aom")]
            DecodeBackend::Zenav1Aom,
        ] {
            let config = DecoderConfig::new().decode_backend(backend);
            let mut managed = ManagedAvifDecoder::new(&data, &config).unwrap();
            let probe = managed.probe_animation_info().unwrap();
            assert_eq!(probe.exif.as_deref(), want_exif);
            assert_eq!(probe.xmp.as_deref(), want_xmp);
            let eager = managed.decode_animation(&Unstoppable).unwrap();
            assert_eq!(eager.info.exif.as_deref(), want_exif);
            assert_eq!(eager.info.xmp.as_deref(), want_xmp);
            let mut native = AnimationDecoder::new(&data, &config).unwrap();
            assert_eq!(native.info().exif.as_deref(), want_exif);
            assert_eq!(native.info().xmp.as_deref(), want_xmp);
            for reference in &eager.frames {
                let frame = native.next_frame(&Unstoppable).unwrap().unwrap();
                for y in 0..150 {
                    assert_eq!(
                        frame.pixels.as_slice().row(y),
                        reference.pixels.as_slice().row(y)
                    );
                }
            }
            let mut cfg = AvifDecoderConfig::new();
            *cfg.inner_mut() = config;
            let probe = cfg.clone().job().probe(&data).unwrap();
            assert_eq!(probe.embedded_metadata.exif.as_deref(), want_exif);
            assert_eq!(probe.embedded_metadata.xmp.as_deref(), want_xmp);
            let mut codec = cfg
                .job()
                .animation_frame_decoder(Cow::Borrowed(&data), &[])
                .unwrap();
            assert_eq!(codec.info().embedded_metadata.exif.as_deref(), want_exif);
            assert_eq!(codec.info().embedded_metadata.xmp.as_deref(), want_xmp);
            for reference in &eager.frames {
                let frame = codec.render_next_frame(None).unwrap().unwrap();
                for y in 0..150 {
                    assert_eq!(frame.pixels().row(y), reference.pixels.as_slice().row(y));
                }
            }
        }
    }
}

#[test]
fn malformed_track_exif_is_an_error_not_missing_metadata() {
    for offset in [EXIF.len() as u32, u32::MAX] {
        let mut data = original();
        let idat = positions(&data, b"idat")[1];
        data[idat + 4..idat + 8].copy_from_slice(&offset.to_be_bytes());
        let p = zenavif_parse::AvifParser::from_bytes(&data).unwrap();
        assert!(p.animation_exif().unwrap().is_err());
        assert_eq!(p.exif().unwrap().unwrap().as_ref(), EXIF);
        let decoder = ManagedAvifDecoder::new(&data, &DecoderConfig::new()).unwrap();
        assert!(decoder.probe_animation_info().is_err());
        assert!(zenavif::AnimationDecoder::new(&data, &DecoderConfig::new()).is_err());
    }
}

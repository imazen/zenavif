#![cfg(feature = "eager")]
#![allow(deprecated)]

// Produced by the parent workspace's tests/animation_sidecars.rs, scenario 1:
// two real SVT color samples, 17/33 ms, track-local Exif/XMP, no poster.
const DATA: &[u8] = include_bytes!("fixtures/track-sidecars.avif");
const EXIF: &[u8] = b"II\x2a\0\x08\0\0\0\0\0\0\0\0\0";
const XMP: &[u8] = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>";

#[test]
fn eager_parser_and_conversion_retain_posterless_track_sidecars() {
    let parser = zenavif_parse::AvifParser::from_bytes(DATA).unwrap();
    let converted = parser.to_avif_data().unwrap();
    let eager = zenavif_parse::read_avif(&mut &DATA[..]).unwrap();
    for result in [converted, eager] {
        let animation = result.animation.expect("posterless animation discarded");
        assert_eq!(animation.exif.as_deref(), Some(EXIF));
        assert_eq!(animation.xmp.as_deref(), Some(XMP));
        assert_eq!(animation.frames.len(), 2);
        assert_eq!(animation.frames[0].duration_ms, 17);
        assert_eq!(animation.frames[1].duration_ms, 33);
        for i in 0..2 {
            assert_eq!(&animation.frames[i].data[..], parser.frame(i).unwrap().data.as_ref());
        }
    }
}

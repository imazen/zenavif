use super::*;

fn without_poster(mut bytes: Vec<u8>) -> Vec<u8> {
    let mut cursor = 0;
    let mut replaced = false;
    while cursor < bytes.len() {
        let size = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
        assert!(size >= 8 && cursor + size <= bytes.len());
        if &bytes[cursor + 4..cursor + 8] == b"meta" {
            bytes[cursor + 4..cursor + 8].copy_from_slice(b"free");
            replaced = true;
        }
        cursor += size;
    }
    assert!(replaced);
    bytes
}

#[test]
fn track_hdr_metadata_survives_without_poster() {
    let mut image = AnimatedImage::new();
    let mut cclv = CclvBox::new();
    cclv.primaries = Some([(-5_000_000, 5_000_000), (0, 1), (2, 3)]);
    cclv.min_luminance = Some(0);
    cclv.max_luminance = Some(u32::MAX);
    cclv.avg_luminance = Some(1);
    image.set_amve(AmveBox::new(100_000, 15635, 16450))
        .set_cclv(cclv).set_clli(ClliBox::new(1000, 400))
        .set_mdcv(MdcvBox::new([(1, 2), (3, 4), (5, 6)], (7, 8), 10_000_000, 50));
    let data = image.try_serialize(64, 64, &[AnimFrame::new(b"sample", 1).with_sync(true)], b"header", None).unwrap();
    for bytes in [data.clone(), without_poster(data)] {
        let parser = zenavif_parse_current::AvifParser::from_bytes(&bytes).unwrap();
        assert_eq!(parser.content_light_level().unwrap().max_content_light_level, 1000);
        assert_eq!(parser.mastering_display().unwrap().min_luminance, 50);
        assert_eq!(parser.ambient_viewing().unwrap().ambient_illuminance, 100_000);
        assert_eq!(parser.ambient_viewing().unwrap().ambient_light_x, 15635);
        let parsed = parser.content_colour_volume().unwrap();
        assert_eq!(parsed.primaries, cclv.primaries);
        assert_eq!(parsed.min_luminance, cclv.min_luminance);
        assert_eq!(parsed.max_luminance, cclv.max_luminance);
        assert_eq!(parsed.avg_luminance, cclv.avg_luminance);
        assert_eq!(parser.animation_info().unwrap().frame_count, 1);
        assert_eq!(parser.frame(0).unwrap().data.as_ref(), b"sample");
    }
}

fn payload_range(data: &[u8], path: &[[u8; 4]]) -> std::ops::Range<usize> {
    let mut region = 0..data.len();
    for (depth, wanted) in path.iter().enumerate() {
        let mut cursor = region.start;
        let mut found = None;
        while cursor < region.end {
            let size = u32::from_be_bytes(data[cursor..cursor + 4].try_into().unwrap()) as usize;
            assert!(size >= 8 && cursor + size <= region.end);
            if &data[cursor + 4..cursor + 8] == wanted {
                found = Some(cursor + 8..cursor + size);
                break;
            }
            cursor += size;
        }
        region = found.expect("metadata box not found");
        if depth + 1 != path.len() {
            region.start += match wanted { b"meta" => 4, b"stsd" => 8, b"av01" => 78, _ => 0 };
        }
    }
    region
}

#[test]
#[allow(deprecated)]
fn cclv_optional_fields_preserve_absence_signed_values_and_track_metadata() {
    for mask in 1..16u8 {
        let mut cclv = CclvBox::new();
        cclv.primaries = (mask & 8 != 0).then_some([(-5_000_000, 5_000_000), (-1, 0), (1, 2)]);
        cclv.min_luminance = (mask & 4 != 0).then_some(0);
        cclv.max_luminance = (mask & 2 != 0).then_some(u32::MAX);
        cclv.avg_luminance = (mask & 1 != 0).then_some(12345);
        let mut image = AnimatedImage::new();
        image.set_cclv(cclv).set_amve(AmveBox::new(u32::MAX, 0, 50000));
        let data = image.try_serialize(64, 64, &[AnimFrame::new(b"sample", 1).with_sync(true)], b"header", None).unwrap();
        let payload = &data[payload_range(&data, &[*b"meta", *b"iprp", *b"ipco", *b"cclv"])];
        assert_eq!(payload[0], mask << 2);
        assert_eq!(payload.len(), 1 + usize::from(mask & 8 != 0) * 24 + (mask & 7).count_ones() as usize * 4);
        if mask & 8 != 0 { assert_eq!(&payload[1..5], &(-5_000_000i32).to_be_bytes()); }
        let parser = zenavif_parse_current::AvifParser::from_bytes(&data).unwrap();
        let hdr = parser.animation_info().unwrap().hdr;
        assert_eq!(hdr.content_colour_volume, parser.content_colour_volume().copied());
        let parsed = hdr.content_colour_volume.unwrap();
        assert_eq!((parsed.primaries, parsed.min_luminance, parsed.max_luminance, parsed.avg_luminance),
                   (cclv.primaries, cclv.min_luminance, cclv.max_luminance, cclv.avg_luminance));
        let eager = zenavif_parse_current::read_avif(&mut &data[..]).unwrap();
        assert_eq!(eager.animation.unwrap().hdr, hdr);
        let owned = zenavif_parse_current::AvifParser::from_reader(&mut &without_poster(data)[..]).unwrap();
        assert_eq!(owned.animation_info().unwrap().hdr, hdr);
        assert_eq!(owned.content_colour_volume(), Some(&parsed));
    }
}

#[test]
fn poster_and_track_hdr_are_independent() {
    let mut image = AnimatedImage::new();
    image.set_amve(AmveBox::new(100_000, 15635, 16450));
    let mut data = image.try_serialize(64, 64, &[AnimFrame::new(b"sample", 1).with_sync(true)], b"header", None).unwrap();
    let poster = payload_range(&data, &[*b"meta", *b"iprp", *b"ipco", *b"amve"]);
    data[poster.start..poster.start + 4].copy_from_slice(&200_000u32.to_be_bytes());
    let parser = zenavif_parse_current::AvifParser::from_bytes(&data).unwrap();
    assert_eq!(parser.ambient_viewing().unwrap().ambient_illuminance, 200_000);
    assert_eq!(parser.animation_info().unwrap().hdr.ambient_viewing.unwrap().ambient_illuminance, 100_000);
}

#[test]
fn invalid_hdr_metadata_is_rejected_before_serialization() {
    let frames = [AnimFrame::new(b"sample", 1).with_sync(true)];
    for amve in [AmveBox::new(0, 0, 0), AmveBox::new(1, 50001, 0), AmveBox::new(1, 0, 50001)] {
        let mut image = AnimatedImage::new();
        image.set_amve(amve);
        assert!(image.try_serialize(64, 64, &frames, b"header", None).is_err());
    }
    for bad in [-5_000_001, 5_000_001, i32::MIN, i32::MAX] {
        let mut image = AnimatedImage::new();
        let mut cclv = CclvBox::new();
        cclv.primaries = Some([(bad, 0); 3]);
        image.set_cclv(cclv);
        assert!(image.try_serialize(64, 64, &frames, b"header", None).is_err());
    }
}

#[test]
fn cclv_requires_content_and_ordered_luminance() {
    for (min, max, avg) in [
        (None, None, None),
        (Some(2), Some(1), None),
        (Some(2), None, Some(1)),
        (None, Some(1), Some(2)),
        (Some(1), Some(3), Some(4)),
    ] {
        let mut cclv = CclvBox::new();
        cclv.min_luminance = min;
        cclv.max_luminance = max;
        cclv.avg_luminance = avg;
        let mut image = AnimatedImage::new();
        image.set_cclv(cclv);
        assert!(image.try_serialize(64, 64, &[AnimFrame::new(b"sample", 1).with_sync(true)], b"header", None).is_err());
    }
}

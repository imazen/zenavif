//! Shared item metadata for the animation poster and color track.
use super::*;

pub(super) struct ImageItem<'a> {
    pub id: u16,
    pub config: &'a Av1CBox,
    pub sequence: &'a [u8],
    pub sample_len: u32,
}

pub(super) struct Sidecar {
    id: u16,
    kind: [u8; 4],
    content_type: &'static [u8],
    payload: Vec<u8>,
}

pub(super) fn sidecars(image: &AnimatedImage) -> Vec<Sidecar> {
    let mut items = Vec::new();
    if let Some(exif) = image.exif.as_deref() {
        let payload = crate::exif_extents(exif).iter().flat_map(|e| e.data.iter().copied()).collect();
        items.push(Sidecar { id: 3, kind: *b"Exif", content_type: b"", payload });
    }
    if let Some(xmp) = image.xmp.as_ref() {
        items.push(Sidecar { id: 4, kind: *b"mime", content_type: b"application/rdf+xml\0", payload: xmp.clone() });
    }
    items
}

// Image data uses file offsets (construction_method=0); metadata is local
// idat data (construction_method=1). No sentinel scanning or payload patching.
pub(super) fn write_meta(
    out: &mut Vec<u8>, width: u32, height: u32,
    images: &[ImageItem<'_>], sidecars: &[Sidecar], options: &AnimatedImage,
) -> Vec<usize> {
    let meta = begin_box(out, b"meta");
    write_fullbox(out, 0, 0);
    let hdlr = begin_box(out, b"hdlr");
    write_fullbox(out, 0, 0);
    write_u32(out, 0);
    out.extend_from_slice(b"pict");
    out.extend_from_slice(&[0; 13]);
    end_box(out, hdlr);
    if !images.is_empty() {
        let pitm = begin_box(out, b"pitm");
        write_fullbox(out, 0, 0);
        write_u16(out, 1);
        end_box(out, pitm);
    }
    let iloc = begin_box(out, b"iloc");
    write_fullbox(out, 1, 0);
    out.extend_from_slice(&[0x44, 0]);
    write_u16(out, (images.len() + sidecars.len()) as u16);
    let mut offsets = Vec::new();
    for image in images {
        write_u16(out, image.id);
        write_u16(out, 0); // file construction method
        write_u16(out, 0); // data reference
        write_u16(out, 1); // extent count
        offsets.push(out.len());
        write_u32(out, 0);
        write_u32(out, image.sample_len);
    }
    let mut offset = 0u32;
    for item in sidecars {
        write_u16(out, item.id);
        write_u16(out, 1); // idat construction method
        write_u16(out, 0);
        write_u16(out, 1);
        write_u32(out, offset);
        write_u32(out, item.payload.len() as u32);
        offset += item.payload.len() as u32;
    }
    end_box(out, iloc);
    let iinf = begin_box(out, b"iinf");
    write_fullbox(out, 0, 0);
    write_u16(out, (images.len() + sidecars.len()) as u16);
    for image in images {
        write_infe(out, image.id, b"av01", b"", image.id == 2);
    }
    for item in sidecars {
        write_infe(out, item.id, &item.kind, item.content_type, false);
    }
    end_box(out, iinf);
    if !images.is_empty() {
        let iref = begin_box(out, b"iref");
        write_fullbox(out, 0, 0);
        for item in sidecars { write_reference(out, b"cdsc", item.id, 1); }
        if images.len() == 2 {
            write_reference(out, b"auxl", 2, 1);
            if options.premultiplied_alpha { write_reference(out, b"prem", 1, 2); }
        }
        end_box(out, iref);
        let iprp = begin_box(out, b"iprp");
        let ipco = begin_box(out, b"ipco");
        let mut next = 1u8;
        let mut associations = Vec::new();
        for image in images {
            let mut props = Vec::new();
            let ispe = begin_box(out, b"ispe");
            write_fullbox(out, 0, 0);
            write_u32(out, width); write_u32(out, height);
            end_box(out, ispe);
            props.push(next); next += 1;
            write_av1c_box(out, image.config, image.sequence);
            props.push(next | 0x80); next += 1;
            let pixi = begin_box(out, b"pixi");
            write_fullbox(out, 0, 0);
            let channels = if image.config.monochrome { 1 } else { 3 };
            out.push(channels);
            for _ in 0..channels { out.push(bit_depth_from_av1c(image.config)); }
            end_box(out, pixi);
            props.push(next); next += 1;
            if image.id == 2 {
                let auxc = begin_box(out, b"auxC");
                write_fullbox(out, 0, 0);
                out.extend_from_slice(ALPHA_TYPE);
                end_box(out, auxc);
                props.push(next); next += 1;
            } else {
                let count = write_color_properties(out, options);
                for _ in 0..count { props.push(next); next += 1; }
            }
            associations.push((image.id, props));
        }
        end_box(out, ipco);
        let ipma = begin_box(out, b"ipma");
        write_fullbox(out, 0, 0);
        write_u32(out, associations.len() as u32);
        for (id, props) in associations {
            write_u16(out, id); out.push(props.len() as u8); out.extend_from_slice(&props);
        }
        end_box(out, ipma);
        end_box(out, iprp);
    }
    if !sidecars.is_empty() {
        let idat = begin_box(out, b"idat");
        for item in sidecars { out.extend_from_slice(&item.payload); }
        end_box(out, idat);
    }
    end_box(out, meta);
    offsets
}

pub(super) const ALPHA_TYPE: &[u8] = b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0";

fn write_infe(out: &mut Vec<u8>, id: u16, kind: &[u8; 4], content_type: &[u8], hidden: bool) {
    let pos = begin_box(out, b"infe");
    write_fullbox(out, 2, u32::from(hidden));
    write_u16(out, id); write_u16(out, 0);
    out.extend_from_slice(kind); out.push(0);
    out.extend_from_slice(content_type);
    end_box(out, pos);
}

fn write_reference(out: &mut Vec<u8>, kind: &[u8; 4], from: u16, to: u16) {
    let pos = begin_box(out, kind);
    write_u16(out, from); write_u16(out, 1); write_u16(out, to);
    end_box(out, pos);
}

pub(super) fn write_color_properties(out: &mut Vec<u8>, options: &AnimatedImage) -> u8 {
    let mut count = 0;
    // Explicit defaults are still explicit properties. Do not omit a colr
    // merely because it equals the library's default color description.
    if let Some((primaries, transfer, matrix, full_range)) = options.colr_raw {
        let pos = begin_box(out, b"colr");
        out.extend_from_slice(b"nclx");
        write_u16(out, primaries); write_u16(out, transfer); write_u16(out, matrix);
        out.push(u8::from(full_range) << 7); end_box(out, pos); count += 1;
    } else if let Some(colr) = options.colr.as_ref() { write_colr_nclx(out, colr); count += 1; }
    if let Some(icc) = options.icc.as_ref() {
        let pos = begin_box(out, b"colr");
        out.extend_from_slice(b"prof"); out.extend_from_slice(icc);
        end_box(out, pos); count += 1;
    }
    if let Some(pasp) = options.pixel_aspect_ratio {
        let pos = begin_box(out, b"pasp");
        write_u32(out, pasp.h_spacing); write_u32(out, pasp.v_spacing);
        end_box(out, pos); count += 1;
    }
    if let Some(clli) = options.clli.as_ref() { write_clli(out, clli); count += 1; }
    if let Some(mdcv) = options.mdcv.as_ref() { write_mdcv(out, mdcv); count += 1; }
    count
}

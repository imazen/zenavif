//! Adversarial AVIF inventory cases from the zenavif#61 review, built in
//! test code from `tests/vectors` files (no committed binaries).
//!
//! - Each case pins the disposition of the parts that hold planted
//!   sentinels.
//! - Every case meets the metadata oracle: each `Metadata(Exif/Xmp/Icc)` leaf
//!   is what some zencodec entry point (probe_full, `Decode::decode`,
//!   `animation_frame_decoder`) reports, and every report is covered by such
//!   leaves; a successful decode means some part is `ImageData`.
//! - The reverse direction: flipping the payload of any consumed leaf changes
//!   what those entry points return, or the part's detail says why not.

use std::borrow::Cow;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use zenavif::AvifDecoderConfig;
use zencodec::decode::{AnimationFrameDecoder, Decode, DecodeJob, DecoderConfig};
use zencodec::inventory::{Disposition, Inventory, MetadataKind, Part, PartKind, PartTag};

// ── A small ISOBMFF builder and editor ──────────────────────────────────────

mod bmff {
    pub fn be(d: &[u8], at: usize, n: usize) -> u64 {
        d[at..at + n]
            .iter()
            .fold(0u64, |v, &b| (v << 8) | u64::from(b))
    }

    pub fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(payload);
        v
    }

    pub fn full(typ: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![version];
        p.extend_from_slice(&flags.to_be_bytes()[1..]);
        p.extend_from_slice(payload);
        bx(typ, &p)
    }

    pub fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    pub fn infe(id: u16, typ: &[u8; 4], name: &[u8], ct: Option<&[u8]>) -> Vec<u8> {
        let mut p = id.to_be_bytes().to_vec();
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(typ);
        p.extend_from_slice(name);
        p.push(0);
        if let Some(ct) = ct {
            p.extend_from_slice(ct);
            p.push(0);
        }
        full(b"infe", 2, 0, &p)
    }

    pub fn iref_entry(typ: &[u8; 4], from: u16, to: &[u16]) -> Vec<u8> {
        let mut p = from.to_be_bytes().to_vec();
        p.extend_from_slice(&(to.len() as u16).to_be_bytes());
        for t in to {
            p.extend_from_slice(&t.to_be_bytes());
        }
        bx(typ, &p)
    }

    /// `(item, construction method, [(offset, length)])`.
    pub type Loc = (u16, u16, Vec<(u32, u32)>);

    /// Version 1, 4-byte offsets and lengths, no base offset or index.
    pub fn iloc(locs: &[Loc]) -> Vec<u8> {
        let mut p = vec![0x44, 0x00];
        p.extend_from_slice(&(locs.len() as u16).to_be_bytes());
        for (id, method, exts) in locs {
            p.extend_from_slice(&id.to_be_bytes());
            p.extend_from_slice(&method.to_be_bytes());
            p.extend_from_slice(&0u16.to_be_bytes());
            p.extend_from_slice(&(exts.len() as u16).to_be_bytes());
            for (o, l) in exts {
                p.extend_from_slice(&o.to_be_bytes());
                p.extend_from_slice(&l.to_be_bytes());
            }
        }
        full(b"iloc", 1, 0, &p)
    }

    /// `(item, [(essential, 1-based index)])`.
    pub type Assoc = Vec<(u32, Vec<(bool, u16)>)>;

    pub fn ipma(entries: &Assoc, version: u8, flags: u32) -> Vec<u8> {
        let mut p = (entries.len() as u32).to_be_bytes().to_vec();
        for (item, assoc) in entries {
            if version == 0 {
                p.extend_from_slice(&(*item as u16).to_be_bytes());
            } else {
                p.extend_from_slice(&item.to_be_bytes());
            }
            p.push(assoc.len() as u8);
            for &(ess, idx) in assoc {
                if flags & 1 != 0 {
                    p.extend_from_slice(&((if ess { 0x8000 } else { 0 }) | idx).to_be_bytes());
                } else {
                    p.push((if ess { 0x80 } else { 0 }) | idx as u8);
                }
            }
        }
        full(b"ipma", version, flags, &p)
    }

    pub fn parse_ipma(b: &[u8]) -> (u8, u32, Assoc) {
        let v = b[8];
        let flags = be(b, 9, 3) as u32;
        let mut p = 12;
        let n = be(b, p, 4);
        p += 4;
        let mut out = Vec::new();
        for _ in 0..n {
            let item = if v == 0 {
                p += 2;
                be(b, p - 2, 2) as u32
            } else {
                p += 4;
                be(b, p - 4, 4) as u32
            };
            let c = b[p];
            p += 1;
            let mut assoc = Vec::new();
            for _ in 0..c {
                if flags & 1 != 0 {
                    let x = be(b, p, 2) as u16;
                    p += 2;
                    assoc.push((x & 0x8000 != 0, x & 0x7fff));
                } else {
                    let x = b[p];
                    p += 1;
                    assoc.push((x & 0x80 != 0, u16::from(x & 0x7f)));
                }
            }
            out.push((item, assoc));
        }
        (v, flags, out)
    }

    #[derive(Clone, Copy, Debug)]
    pub struct At {
        pub pos: usize,
        pub hdr: usize,
        pub end: usize,
        pub typ: [u8; 4],
    }

    fn boxes(d: &[u8], start: usize, end: usize) -> Vec<At> {
        let mut out = Vec::new();
        let mut pos = start;
        while pos + 8 <= end {
            let mut size = be(d, pos, 4) as usize;
            let typ: [u8; 4] = d[pos + 4..pos + 8].try_into().unwrap();
            let mut hdr = 8;
            if size == 1 {
                size = be(d, pos + 8, 8) as usize;
                hdr = 16;
            } else if size == 0 {
                size = end - pos;
            }
            if size < hdr || pos + size > end {
                break;
            }
            out.push(At {
                pos,
                hdr,
                end: pos + size,
                typ,
            });
            pos += size;
        }
        out
    }

    const PLAIN: [&[u8; 4]; 12] = [
        b"moov", b"trak", b"mdia", b"minf", b"stbl", b"edts", b"dinf", b"iprp", b"ipco", b"grpl",
        b"tref", b"udta",
    ];

    fn children_start(d: &[u8], b: &At) -> Option<usize> {
        let skip = match &b.typ {
            t if PLAIN.contains(&t) => 0,
            b"meta" | b"iref" => 4,
            b"dref" | b"stsd" => 8,
            b"iinf" => {
                if d[b.pos + b.hdr] == 0 {
                    6
                } else {
                    8
                }
            }
            b"av01" => 78,
            _ => return None,
        };
        Some(b.pos + b.hdr + skip)
    }

    /// The `nth` box matching `path`, depth first.
    pub fn locate(d: &[u8], path: &[&[u8; 4]], nth: usize) -> At {
        locate_all(d, path)[nth]
    }

    /// Every box matching `path`, depth first.
    pub fn locate_all(d: &[u8], path: &[&[u8; 4]]) -> Vec<At> {
        fn rec(d: &[u8], s: usize, e: usize, path: &[&[u8; 4]], hits: &mut Vec<At>) {
            for b in boxes(d, s, e) {
                if &b.typ == path[0] {
                    if path.len() == 1 {
                        hits.push(b);
                    } else if let Some(cs) = children_start(d, &b) {
                        rec(d, cs, b.end, &path[1..], hits);
                    }
                }
            }
        }
        let mut hits = Vec::new();
        rec(d, 0, d.len(), path, &mut hits);
        hits
    }

    /// Boxes whose content strictly contains `at`.
    fn ancestors(d: &[u8], at: usize, start: usize, end: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for b in boxes(d, start, end) {
            if b.pos + b.hdr <= at && at < b.end {
                out.push((b.pos, b.hdr));
                if let Some(cs) = children_start(d, &b) {
                    out.extend(ancestors(d, at, cs, b.end));
                }
            }
        }
        out
    }

    fn grow(d: &mut [u8], list: &[(usize, usize)], delta: i64) {
        for &(pos, hdr) in list {
            if hdr == 16 {
                let s = be(d, pos + 8, 8) as i64 + delta;
                d[pos + 8..pos + 16].copy_from_slice(&(s as u64).to_be_bytes());
            } else {
                let s = be(d, pos, 4) as i64;
                if s != 0 {
                    d[pos..pos + 4].copy_from_slice(&((s + delta) as u32).to_be_bytes());
                }
            }
        }
    }

    /// Shift the top-level iloc's method-0 offsets and every stco/co64 entry
    /// that points at or past `at`.
    fn fix_offsets(d: &mut [u8], at: usize, delta: i64) {
        let iloc = locate_all(d, &[b"meta", b"iloc"]).first().copied();
        if let Some(b) = iloc {
            let mut p = b.pos + b.hdr;
            let v = d[p];
            p += 4;
            let (osz, lsz) = (usize::from(d[p] >> 4), usize::from(d[p] & 15));
            let bsz = usize::from(d[p + 1] >> 4);
            let isz = if v >= 1 {
                usize::from(d[p + 1] & 15)
            } else {
                0
            };
            p += 2;
            let n = if v == 2 {
                p += 4;
                be(d, p - 4, 4)
            } else {
                p += 2;
                be(d, p - 2, 2)
            };
            for _ in 0..n {
                p += if v == 2 { 4 } else { 2 };
                let mut method = 0;
                if v >= 1 {
                    method = be(d, p, 2) & 15;
                    p += 2;
                }
                p += 2;
                let base = if bsz > 0 { be(d, p, bsz) } else { 0 } as usize;
                p += bsz;
                let ne = be(d, p, 2);
                p += 2;
                for _ in 0..ne {
                    p += isz;
                    let o = if osz > 0 { be(d, p, osz) } else { 0 } as usize;
                    if method == 0 && osz > 0 && base + o >= at {
                        let no = (o as i64 + delta) as u64;
                        d[p..p + osz].copy_from_slice(&no.to_be_bytes()[8 - osz..]);
                    }
                    p += osz + lsz;
                }
            }
        }
        for (typ, w) in [(b"stco", 4usize), (b"co64", 8)] {
            let path: [&[u8; 4]; 6] = [b"moov", b"trak", b"mdia", b"minf", b"stbl", typ];
            for b in locate_all(d, &path) {
                let mut p = b.pos + b.hdr + 4;
                let n = be(d, p, 4);
                p += 4;
                for _ in 0..n {
                    let o = be(d, p, w) as usize;
                    if o >= at {
                        let no = (o as i64 + delta) as u64;
                        d[p..p + w].copy_from_slice(&no.to_be_bytes()[8 - w..]);
                    }
                    p += w;
                }
            }
        }
    }

    fn insert_with(
        d: &[u8],
        at: usize,
        payload: &[u8],
        anc: &[(usize, usize)],
        fix: bool,
    ) -> Vec<u8> {
        let mut out = d[..at].to_vec();
        out.extend_from_slice(payload);
        out.extend_from_slice(&d[at..]);
        grow(&mut out, anc, payload.len() as i64);
        if fix {
            fix_offsets(&mut out, at, payload.len() as i64);
        }
        out
    }

    pub fn append_child(d: &[u8], path: &[&[u8; 4]], payload: &[u8], fix: bool) -> Vec<u8> {
        let b = locate(d, path, 0);
        let mut anc = ancestors(d, b.pos, 0, d.len());
        anc.push((b.pos, b.hdr));
        insert_with(d, b.end, payload, &anc, fix)
    }

    pub fn insert_before(d: &[u8], path: &[&[u8; 4]], payload: &[u8], fix: bool) -> Vec<u8> {
        let b = locate(d, path, 0);
        let anc = ancestors(d, b.pos, 0, d.len());
        insert_with(d, b.pos, payload, &anc, fix)
    }

    pub fn insert_after(d: &[u8], path: &[&[u8; 4]], payload: &[u8], fix: bool) -> Vec<u8> {
        let b = locate(d, path, 0);
        let anc = ancestors(d, b.pos, 0, d.len());
        insert_with(d, b.end, payload, &anc, fix)
    }

    pub fn get_box(d: &[u8], path: &[&[u8; 4]]) -> Vec<u8> {
        let b = locate(d, path, 0);
        d[b.pos..b.end].to_vec()
    }

    pub fn rewrite_box(d: &[u8], path: &[&[u8; 4]], new: &[u8]) -> Vec<u8> {
        let b = locate(d, path, 0);
        let anc = ancestors(d, b.pos, 0, d.len());
        let delta = new.len() as i64 - (b.end - b.pos) as i64;
        let mut out = d[..b.pos].to_vec();
        out.extend_from_slice(new);
        out.extend_from_slice(&d[b.end..]);
        grow(&mut out, &anc, delta);
        fix_offsets(&mut out, b.end, delta);
        out
    }

    /// `(offset, length)` of an item's extents in the top-level iloc.
    pub fn item_data(d: &[u8], item: u32) -> Vec<(usize, usize)> {
        let b = locate(d, &[b"meta", b"iloc"], 0);
        let mut p = b.pos + b.hdr;
        let v = d[p];
        p += 4;
        let (osz, lsz) = (usize::from(d[p] >> 4), usize::from(d[p] & 15));
        let bsz = usize::from(d[p + 1] >> 4);
        let isz = if v >= 1 {
            usize::from(d[p + 1] & 15)
        } else {
            0
        };
        p += 2;
        let n = if v == 2 {
            p += 4;
            be(d, p - 4, 4)
        } else {
            p += 2;
            be(d, p - 2, 2)
        };
        for _ in 0..n {
            let id = if v == 2 {
                p += 4;
                be(d, p - 4, 4)
            } else {
                p += 2;
                be(d, p - 2, 2)
            } as u32;
            if v >= 1 {
                p += 2;
            }
            p += 2;
            let base = if bsz > 0 { be(d, p, bsz) } else { 0 } as usize;
            p += bsz;
            let ne = be(d, p, 2);
            p += 2;
            let mut exts = Vec::new();
            for _ in 0..ne {
                p += isz;
                let o = if osz > 0 { be(d, p, osz) } else { 0 } as usize;
                p += osz;
                let l = if lsz > 0 { be(d, p, lsz) } else { 0 } as usize;
                p += lsz;
                exts.push((base + o, l));
            }
            if id == item {
                return exts;
            }
        }
        Vec::new()
    }
}

use bmff::{Assoc, bx, cat, full};

fn vector(rel: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/vectors")
        .join(rel);
    std::fs::read(&p).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (scripts/download-avif-test-vectors.sh)",
            p.display()
        )
    })
}

// ── Still-image cases (built from libavif/white_1x1.avif) ─────────────────

struct White {
    ftyp: Vec<u8>,
    hdlr: Vec<u8>,
    ispe: Vec<u8>,
    pixi: Vec<u8>,
    av1c: Vec<u8>,
    colr: Vec<u8>,
    av1: Vec<u8>,
}

fn white() -> White {
    let w = vector("libavif/white_1x1.avif");
    let prop = |t: &[u8; 4]| bmff::get_box(&w, &[b"meta", b"iprp", b"ipco", t]);
    let (off, len) = bmff::item_data(&w, 1)[0];
    White {
        ftyp: bmff::get_box(&w, &[b"ftyp"]),
        hdlr: bmff::get_box(&w, &[b"meta", b"hdlr"]),
        ispe: prop(b"ispe"),
        pixi: prop(b"pixi"),
        av1c: prop(b"av1C"),
        colr: prop(b"colr"),
        av1: w[off..off + len].to_vec(),
    }
}

/// `(id, type, name, content type)`.
type ItemDecl = (u16, [u8; 4], Vec<u8>, Option<Vec<u8>>);

/// The `still()` layout of the review's generator: ftyp, meta (hdlr, pitm 1,
/// iloc, iinf, iref, iprp, extras), extra top-level boxes, mdat.
#[derive(Default)]
struct Still {
    items: Vec<ItemDecl>,
    props: Vec<Vec<u8>>,
    assoc: Assoc,
    chunks: Vec<(Option<u16>, Vec<u8>)>,
    irefs: Vec<([u8; 4], u16, Vec<u16>)>,
    extra_meta: Vec<u8>,
    top_before_mdat: Vec<u8>,
    meta_last: bool,
    iprp_override: Option<Vec<u8>>,
}

impl Still {
    fn base(w: &White) -> Self {
        Self {
            items: vec![(1, *b"av01", b"Color".to_vec(), None)],
            props: vec![
                w.ispe.clone(),
                w.pixi.clone(),
                w.av1c.clone(),
                w.colr.clone(),
            ],
            assoc: vec![(1, vec![(false, 1), (false, 2), (true, 3), (false, 4)])],
            chunks: vec![(Some(1), w.av1.clone())],
            ..Self::default()
        }
    }

    fn build(&self, w: &White) -> Vec<u8> {
        let once = |mdat_at: usize| -> Vec<u8> {
            let mut ex: Vec<(u16, Vec<(u32, u32)>)> = Vec::new();
            let mut at = mdat_at + 8;
            for (id, c) in &self.chunks {
                if let Some(id) = id {
                    match ex.iter_mut().find(|(i, _)| i == id) {
                        Some((_, v)) => v.push((at as u32, c.len() as u32)),
                        None => ex.push((*id, vec![(at as u32, c.len() as u32)])),
                    }
                }
                at += c.len();
            }
            for (id, ..) in &self.items {
                if !ex.iter().any(|(i, _)| i == id) {
                    ex.push((*id, vec![(0, 1)]));
                }
            }
            let mut iinf_p = (self.items.len() as u16).to_be_bytes().to_vec();
            for (id, t, n, ct) in &self.items {
                iinf_p.extend(bmff::infe(*id, t, n, ct.as_deref()));
            }
            let iinf = full(b"iinf", 0, 0, &iinf_p);
            let locs: Vec<bmff::Loc> = ex.into_iter().map(|(i, e)| (i, 0, e)).collect();
            let iloc = bmff::iloc(&locs);
            let iref = if self.irefs.is_empty() {
                Vec::new()
            } else {
                let mut p = Vec::new();
                for (t, f, to) in &self.irefs {
                    p.extend(bmff::iref_entry(t, *f, to));
                }
                full(b"iref", 0, 0, &p)
            };
            let iprp = self.iprp_override.clone().unwrap_or_else(|| {
                bx(
                    b"iprp",
                    &cat(&[
                        &bx(b"ipco", &self.props.concat()),
                        &bmff::ipma(&self.assoc, 0, 0),
                    ]),
                )
            });
            let meta = full(
                b"meta",
                0,
                0,
                &cat(&[
                    &w.hdlr,
                    &full(b"pitm", 0, 0, &1u16.to_be_bytes()),
                    &iloc,
                    &iinf,
                    &iref,
                    &iprp,
                    &self.extra_meta,
                ]),
            );
            let mdat = bx(
                b"mdat",
                &self
                    .chunks
                    .iter()
                    .map(|(_, c)| c.as_slice())
                    .collect::<Vec<_>>()
                    .concat(),
            );
            if self.meta_last {
                cat(&[&w.ftyp, &self.top_before_mdat, &mdat, &meta])
            } else {
                cat(&[&w.ftyp, &meta, &self.top_before_mdat, &mdat])
            }
        };
        let d = once(0);
        let at = d.windows(4).position(|x| x == b"mdat").unwrap() - 4;
        let d = once(at);
        assert_eq!(d.windows(4).position(|x| x == b"mdat").unwrap() - 4, at);
        d
    }
}

const ICC_FIRST: &[u8] = b"ICC-SENTINEL-FIRST-IPRP!";

fn icc_colr() -> Vec<u8> {
    bx(
        b"colr",
        &cat(&[b"prof", &(ICC_FIRST.len() as u32).to_be_bytes(), ICC_FIRST]),
    )
}

const EXIF: &[u8] = b"\0\0\0\0MM\0*\0\0\0\x08\0\0EXIF-SENTINEL";

fn base_props(w: &White) -> Vec<u8> {
    cat(&[&w.ispe, &w.pixi, &w.av1c, &w.colr])
}

fn base_ipma() -> Vec<u8> {
    bmff::ipma(
        &vec![(1, vec![(false, 1), (false, 2), (true, 3), (false, 4)])],
        0,
        0,
    )
}

fn dup_iprp(w: &White) -> Vec<u8> {
    let iprp1 = bx(
        b"iprp",
        &cat(&[
            &bx(
                b"ipco",
                &cat(&[&icc_colr(), &w.ispe, &bx(b"zPrv", b"IPRP1-PRIVATE")]),
            ),
            &bmff::ipma(&vec![(1, vec![(false, 1), (false, 2)])], 0, 0),
        ]),
    );
    let iprp2 = bx(b"iprp", &cat(&[&bx(b"ipco", &base_props(w)), &base_ipma()]));
    Still {
        iprp_override: Some(cat(&[&iprp1, &iprp2])),
        ..Still::base(w)
    }
    .build(w)
}

fn dup_ipco(w: &White) -> Vec<u8> {
    let iprp = bx(
        b"iprp",
        &cat(&[
            &bx(
                b"ipco",
                &cat(&[&icc_colr(), &w.ispe, &bx(b"zPrv", b"IPCO1-PRIVATE")]),
            ),
            &bx(b"ipco", &base_props(w)),
            &base_ipma(),
        ]),
    );
    Still {
        iprp_override: Some(iprp),
        ..Still::base(w)
    }
    .build(w)
}

fn dup_ipma(w: &White) -> Vec<u8> {
    let iprp = bx(
        b"iprp",
        &cat(&[
            &bx(b"ipco", &cat(&[&base_props(w), &icc_colr()])),
            &bmff::ipma(&vec![(1, vec![(false, 5), (false, 1), (true, 3)])], 0, 0),
            &base_ipma(),
        ]),
    );
    Still {
        iprp_override: Some(iprp),
        ..Still::base(w)
    }
    .build(w)
}

fn with_exif_items(w: &White, items: &[(u16, [u8; 4], &[u8])]) -> Vec<u8> {
    let mut s = Still::base(w);
    for (id, t, n) in items {
        s.items.push((*id, *t, n.to_vec(), None));
    }
    s.chunks.push((Some(2), EXIF.to_vec()));
    s.irefs.push((*b"cdsc", 2, vec![1]));
    s.build(w)
}

fn dup_infe_primary(w: &White) -> Vec<u8> {
    let mut s = Still::base(w);
    s.items.push((1, *b"grid", b"G".to_vec(), None));
    s.build(w)
}

fn meta_last_overrun(w: &White) -> Vec<u8> {
    let mut d = Still {
        meta_last: true,
        ..Still::base(w)
    }
    .build(w);
    let m = d.windows(4).position(|x| x == b"meta").unwrap() - 4;
    let size = bmff::be(&d, m, 4) as u32 + 40;
    d[m..m + 4].copy_from_slice(&size.to_be_bytes());
    d
}

fn iprp_overrun_in_meta(w: &White) -> Vec<u8> {
    let mut d = Still::base(w).build(w);
    let i = d.windows(4).position(|x| x == b"iprp").unwrap() - 4;
    let size = bmff::be(&d, i, 4) as u32 + 16;
    d[i..i + 4].copy_from_slice(&size.to_be_bytes());
    d
}

/// The primary's AV1 data inside a `free` box, a `uuid` box, or after the
/// last box; the mdat holds junk.
fn primary_at(w: &White, place: &str) -> Vec<u8> {
    let once = |at: u32| -> Vec<u8> {
        let iinf = full(
            b"iinf",
            0,
            0,
            &cat(&[&1u16.to_be_bytes(), &bmff::infe(1, b"av01", b"Color", None)]),
        );
        let iloc = bmff::iloc(&[(1, 0, vec![(at, w.av1.len() as u32)])]);
        let iprp = bx(b"iprp", &cat(&[&bx(b"ipco", &base_props(w)), &base_ipma()]));
        let meta = full(
            b"meta",
            0,
            0,
            &cat(&[
                &w.hdlr,
                &full(b"pitm", 0, 0, &1u16.to_be_bytes()),
                &iloc,
                &iinf,
                &iprp,
            ]),
        );
        let junk = bx(b"mdat", b"MDAT-JUNK-SENTINEL");
        match place {
            "free" => cat(&[&w.ftyp, &meta, &bx(b"free", &w.av1), &junk]),
            "uuid" => cat(&[
                &w.ftyp,
                &meta,
                &bx(b"uuid", &cat(&[b"0123456789abcdef", &w.av1])),
                &junk,
            ]),
            _ => cat(&[&w.ftyp, &meta, &junk, &w.av1]),
        }
    };
    let d = once(0);
    let at = d
        .windows(w.av1.len())
        .rposition(|x| x == w.av1.as_slice())
        .unwrap();
    let d = once(at as u32);
    assert_eq!(
        d.windows(w.av1.len())
            .rposition(|x| x == w.av1.as_slice())
            .unwrap(),
        at
    );
    d
}

fn nclx_short(w: &White) -> Vec<u8> {
    let mut s = Still::base(w);
    s.props[3] = bx(b"colr", b"nclx\0\x09\0\x10\0\x09");
    s.build(w)
}

fn grid_prop_on_av01(w: &White) -> Vec<u8> {
    let mut s = Still::base(w);
    s.props.push(full(
        b"grid",
        0,
        0,
        &cat(&[
            &[0, 0, 0],
            &1u16.to_be_bytes(),
            &1u16.to_be_bytes(),
            b"GRID-TAIL-SENTINEL",
        ]),
    ));
    s.assoc = vec![(
        1,
        vec![(false, 1), (false, 2), (true, 3), (false, 4), (false, 5)],
    )];
    s.build(w)
}

/// A version-1 iref (32-bit item IDs) behind a 64-bit size header.
fn iref_largesize_v1(w: &White) -> Vec<u8> {
    let once = |mdat_at: usize| -> Vec<u8> {
        let exif_at = (mdat_at + 8 + w.av1.len()) as u32;
        let iinf = full(
            b"iinf",
            0,
            0,
            &cat(&[
                &2u16.to_be_bytes(),
                &bmff::infe(1, b"av01", b"Color", None),
                &bmff::infe(2, b"Exif", b"", None),
            ]),
        );
        let iloc = bmff::iloc(&[
            (1, 0, vec![((mdat_at + 8) as u32, w.av1.len() as u32)]),
            (2, 0, vec![(exif_at, EXIF.len() as u32)]),
        ]);
        let entry = bx(
            b"cdsc",
            &cat(&[
                &2u32.to_be_bytes(),
                &1u16.to_be_bytes(),
                &1u32.to_be_bytes(),
            ]),
        );
        let payload = cat(&[&[1, 0, 0, 0], &entry]);
        let iref = cat(&[
            &1u32.to_be_bytes(),
            b"iref",
            &((payload.len() + 16) as u64).to_be_bytes(),
            &payload,
        ]);
        let iprp = bx(b"iprp", &cat(&[&bx(b"ipco", &base_props(w)), &base_ipma()]));
        let meta = full(
            b"meta",
            0,
            0,
            &cat(&[
                &w.hdlr,
                &full(b"pitm", 0, 0, &1u16.to_be_bytes()),
                &iloc,
                &iinf,
                &iref,
                &iprp,
            ]),
        );
        cat(&[&w.ftyp, &meta, &bx(b"mdat", &cat(&[&w.av1, EXIF]))])
    };
    let d = once(0);
    let at = d.windows(4).position(|x| x == b"mdat").unwrap() - 4;
    once(at)
}

const XMP: &[u8] = b"<?xpacket begin?><x:xmpmeta/><?xpacket end=\"w\"?>XMP-TAIL-SENTINEL";

fn meta_items(w: &White) -> Vec<u8> {
    let mut s = Still::base(w);
    s.items.push((2, *b"Exif", Vec::new(), None));
    s.items.push((
        3,
        *b"mime",
        b"XMPNAME-SENTINEL".to_vec(),
        Some(b"application/rdf+xml".to_vec()),
    ));
    s.items.push((4, *b"Exif", Vec::new(), None));
    s.chunks.push((
        Some(4),
        b"\0\0\0\0MM\0*\0\0\0\x08\0\0EXIF4-SENTINE".to_vec(),
    ));
    s.chunks.push((Some(2), EXIF.to_vec()));
    s.chunks.push((Some(3), XMP.to_vec()));
    s.irefs = vec![(*b"cdsc", 2, vec![1]), (*b"cdsc", 3, vec![1])];
    s.build(w)
}

/// An EXIF item whose offset field (500) points past its end.
fn exif_bad_offset(w: &White) -> Vec<u8> {
    let mut s = Still::base(w);
    s.items.push((2, *b"Exif", Vec::new(), None));
    s.chunks.push((
        Some(2),
        cat(&[&500u32.to_be_bytes(), b"MM\0*\0\0\0\x08\0\0BAD-OFFSET-SENT"]),
    ));
    s.irefs.push((*b"cdsc", 2, vec![1]));
    s.build(w)
}

fn second_ftyp(_: &White) -> Vec<u8> {
    let d = vector("libavif/white_1x1.avif");
    cat(&[&d, &bx(b"ftyp", b"avif\0\0\0\0SECOND-FTYP-SENT")])
}

// ── Animation cases (built from libavif/colors-animated-8bpc.avif) ─────────

struct Anim {
    ftyp: Vec<u8>,
    hdlr: Vec<u8>,
    props: Vec<u8>,
    payload: Vec<u8>,
    /// The poster's offset and length inside `payload`.
    poster: (usize, usize),
    moov: Vec<u8>,
    p0: usize,
}

fn anim() -> Anim {
    let a = vector("libavif/colors-animated-8bpc.avif");
    let props: Vec<u8> = [b"ispe", b"pixi", b"av1C", b"colr"]
        .iter()
        .map(|t| bmff::get_box(&a, &[b"meta", b"iprp", b"ipco", t]))
        .collect::<Vec<_>>()
        .concat();
    let m = bmff::locate(&a, &[b"mdat"], 0);
    let p0 = m.pos + m.hdr;
    let (off, len) = bmff::item_data(&a, 1)[0];
    Anim {
        ftyp: bmff::get_box(&a, &[b"ftyp"]),
        hdlr: bmff::get_box(&a, &[b"meta", b"hdlr"]),
        props,
        payload: a[p0..m.end].to_vec(),
        poster: (off - p0, len),
        moov: bmff::get_box(&a, &[b"moov"]),
        p0,
    }
}

/// Shift every stco entry of a standalone moov.
fn shift_stco(moov: &[u8], delta: i64) -> Vec<u8> {
    let mut d = moov.to_vec();
    for b in bmff::locate_all(
        moov,
        &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stco"],
    ) {
        let mut p = b.pos + b.hdr + 4;
        let n = bmff::be(&d, p, 4);
        p += 4;
        for _ in 0..n {
            let o = bmff::be(&d, p, 4) as i64 + delta;
            d[p..p + 4].copy_from_slice(&(o as u32).to_be_bytes());
            p += 4;
        }
    }
    d
}

const EXIF_P: &[u8] = b"\0\0\0\0MM\0*\0\0\0\x08\0\0PRIMARY-EXIF-SENT";
const EXIF_T: &[u8] = b"\0\0\0\0MM\0*\0\0\0\x08\0\0TRACK-EXIF-SENTINEL";

/// Where a track meta's Exif item lives.
enum TrackExif<'a> {
    None,
    /// In the mdat, with extra track-meta children.
    Mdat(&'a [u8], Vec<u8>),
    /// In the track meta's idat, followed by slack.
    Idat(&'a [u8]),
}

fn anim_build(
    a: &Anim,
    primary_exif: Option<&[u8]>,
    track: TrackExif<'_>,
    moov_edit: Option<&dyn Fn(Vec<u8>) -> Vec<u8>>,
) -> Vec<u8> {
    let once = |p1: usize| -> Vec<u8> {
        let mut items = vec![bmff::infe(1, b"av01", b"Color", None)];
        let mut locs = vec![(
            1u16,
            0u16,
            vec![((a.poster.0 + p1) as u32, a.poster.1 as u32)],
        )];
        let mut at = p1 + a.payload.len();
        let mut iref = Vec::new();
        if let Some(e) = primary_exif {
            items.push(bmff::infe(2, b"Exif", b"", None));
            locs.push((2, 0, vec![(at as u32, e.len() as u32)]));
            at += e.len();
            iref = full(b"iref", 0, 0, &bmff::iref_entry(b"cdsc", 2, &[1]));
        }
        let mut iinf_p = (items.len() as u16).to_be_bytes().to_vec();
        for i in &items {
            iinf_p.extend_from_slice(i);
        }
        let iprp = bx(
            b"iprp",
            &cat(&[
                &bx(b"ipco", &a.props),
                &bmff::ipma(
                    &vec![(1, vec![(false, 1), (false, 2), (true, 3), (false, 4)])],
                    0,
                    0,
                ),
            ]),
        );
        let meta = full(
            b"meta",
            0,
            0,
            &cat(&[
                &a.hdlr,
                &full(b"pitm", 0, 0, &1u16.to_be_bytes()),
                &bmff::iloc(&locs),
                &full(b"iinf", 0, 0, &iinf_p),
                &iref,
                &iprp,
            ]),
        );
        let mut mv = shift_stco(&a.moov, p1 as i64 - a.p0 as i64);
        let track_iinf = full(
            b"iinf",
            0,
            0,
            &cat(&[
                &1u16.to_be_bytes(),
                &bmff::infe(1, b"Exif", b"TRACKEXIFNAME", None),
            ]),
        );
        let mut tail = Vec::new();
        match &track {
            TrackExif::None => {}
            TrackExif::Mdat(e, extra) => {
                let tm = full(
                    b"meta",
                    0,
                    0,
                    &cat(&[
                        &a.hdlr,
                        &bmff::iloc(&[(1, 0, vec![(at as u32, e.len() as u32)])]),
                        &track_iinf,
                        extra,
                    ]),
                );
                mv = bmff::append_child(&mv, &[b"moov", b"trak"], &tm, false);
                tail = e.to_vec();
            }
            TrackExif::Idat(e) => {
                let idat = bx(b"idat", &cat(&[e, b"IDAT-SLACK-SENTINEL"]));
                let tm = full(
                    b"meta",
                    0,
                    0,
                    &cat(&[
                        &a.hdlr,
                        &bmff::iloc(&[(1, 1, vec![(0, e.len() as u32)])]),
                        &track_iinf,
                        &idat,
                    ]),
                );
                mv = bmff::append_child(&mv, &[b"moov", b"trak"], &tm, false);
            }
        }
        if let Some(edit) = moov_edit {
            mv = edit(mv);
        }
        let mdat = bx(
            b"mdat",
            &cat(&[&a.payload, primary_exif.unwrap_or(&[]), &tail]),
        );
        cat(&[&a.ftyp, &meta, &mv, &mdat])
    };
    let d = once(0);
    let p1 = d.windows(4).rposition(|x| x == b"mdat").unwrap() + 4;
    let d = once(p1);
    assert_eq!(d.windows(4).rposition(|x| x == b"mdat").unwrap() + 4, p1);
    d
}

const AV01_PATH: [&[u8; 4]; 7] = [
    b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd", b"av01",
];

/// A second av01 sample entry carrying an ICC colr: read_stsd stops after
/// the first entry with an av1C.
fn second_entry(mv: Vec<u8>) -> Vec<u8> {
    let b = bmff::locate(&mv, &AV01_PATH, 0);
    let icc = bx(
        b"colr",
        &cat(&[
            b"prof",
            &28u32.to_be_bytes(),
            b"SECOND-ENTRY-ICC-SENTINEL!!!",
        ]),
    );
    let mut entry = cat(&[&mv[b.pos..b.end], &icc]);
    let n = entry.len() as u32;
    entry[..4].copy_from_slice(&n.to_be_bytes());
    let mut mv2 = bmff::insert_after(&mv, &AV01_PATH, &entry, false);
    let s = bmff::locate(&mv2, &AV01_PATH[..6], 0);
    mv2[s.pos + s.hdr + 4..s.pos + s.hdr + 8].copy_from_slice(&2u32.to_be_bytes());
    mv2
}

/// An mdhd after the stbl inside minf (read_minf returns at the stbl).
fn minf_tail(mv: Vec<u8>) -> Vec<u8> {
    let mut mdhd = bmff::get_box(&mv, &[b"moov", b"trak", b"mdia", b"mdhd"]);
    let ts = bmff::be(&mdhd, 20, 4) as u32;
    mdhd[20..24].copy_from_slice(&(ts * 4).to_be_bytes());
    bmff::append_child(&mv, &[b"moov", b"trak", b"mdia", b"minf"], &mdhd, false)
}

/// Two moov boxes: the first (with an ICC colr in its sample entry) is
/// replaced by the second.
fn anim_dup_moov(a: &Anim) -> Vec<u8> {
    let d = anim_build(a, None, TrackExif::None, None);
    let mv = bmff::get_box(&d, &[b"moov"]);
    let icc = bx(
        b"colr",
        &cat(&[b"prof", &26u32.to_be_bytes(), b"FIRST-MOOV-ICC-SENTINEL!!!"]),
    );
    let mv1 = bmff::append_child(&mv, &AV01_PATH, &icc, false);
    bmff::insert_before(&d, &[b"moov"], &mv1, true)
}

// ── Gain-map cases (supported_gainmap_writer_version_with_extra_bytes) ─────

fn gain_map_base() -> Vec<u8> {
    vector("libavif/supported_gainmap_writer_version_with_extra_bytes.avif")
}

/// `(id, type)` of every infe of the top-level meta.
fn item_types(d: &[u8]) -> Vec<(u32, [u8; 4])> {
    bmff::locate_all(d, &[b"meta", b"iinf", b"infe"])
        .iter()
        .map(|b| {
            let c = &d[b.pos + b.hdr..b.end];
            (bmff::be(c, 4, 2) as u32, c[8..12].try_into().unwrap())
        })
        .collect()
}

/// An ICC colr associated with the tmap item before its nclx.
fn gm_tmap_icc_first() -> Vec<u8> {
    let g = gain_map_base();
    let tmap = item_types(&g)
        .into_iter()
        .find(|(_, t)| t == b"tmap")
        .unwrap()
        .0;
    let n_props = bmff::locate_all(&g, &[b"meta", b"iprp", b"ipco"])
        .first()
        .map(|b| {
            let mut n = 0;
            let mut p = b.pos + b.hdr;
            while p + 8 <= b.end {
                p += bmff::be(&g, p, 4) as usize;
                n += 1;
            }
            n
        })
        .unwrap();
    let icc = bx(
        b"colr",
        &cat(&[
            b"prof",
            &30u32.to_be_bytes(),
            b"TMAP-ALT-ICC-SENTINEL-30BYTES!",
        ]),
    );
    let d = bmff::append_child(&g, &[b"meta", b"iprp", b"ipco"], &icc, true);
    let (v, flags, mut entries) =
        bmff::parse_ipma(&bmff::get_box(&d, &[b"meta", b"iprp", b"ipma"]));
    for (item, assoc) in &mut entries {
        if *item == tmap {
            assoc.insert(0, (false, (n_props + 1) as u16));
        }
    }
    bmff::rewrite_box(
        &d,
        &[b"meta", b"iprp", b"ipma"],
        &bmff::ipma(&entries, v, flags),
    )
}

/// The gain-map image's bytes after its first 40 replaced by a sentinel.
fn gm_frame_overwritten() -> Vec<u8> {
    let g = gain_map_base();
    let tmap = item_types(&g)
        .into_iter()
        .find(|(_, t)| t == b"tmap")
        .unwrap()
        .0;
    // The tmap's second dimg input is the gain-map image (iref version 0).
    let gm = bmff::locate_all(&g, &[b"meta", b"iref", b"dimg"])
        .iter()
        .map(|b| &g[b.pos + b.hdr..b.end])
        .find(|c| bmff::be(c, 0, 2) as u32 == tmap)
        .map(|c| bmff::be(c, 6, 2) as u32)
        .unwrap();
    let (off, len) = bmff::item_data(&g, gm)[0];
    let mut d = g.clone();
    let sent = b"GAINMAP-FRAME-SENTINEL-".repeat(16);
    d[off + 40..off + len].copy_from_slice(&sent[..len - 40]);
    d
}

// ── Oracles ─────────────────────────────────────────────────────────────────

fn hash<T: Hash + ?Sized>(v: &T) -> u64 {
    let mut s = DefaultHasher::new();
    v.hash(&mut s);
    s.finish()
}

fn inventory_of(data: &[u8]) -> Inventory {
    let inv = AvifDecoderConfig::new()
        .job()
        .inventory(data)
        .expect("inventory errs only on cancellation or limits")
        .expect("zenavif declares the inventory capability");
    inv.validate()
        .unwrap_or_else(|e| panic!("invalid inventory: {e}\n{inv}"));
    inv
}

/// What the zencodec entry points report for a file.
#[derive(Default)]
struct Reports {
    exif: Vec<Vec<u8>>,
    xmp: Vec<Vec<u8>>,
    icc: Vec<Vec<u8>>,
    decoded: bool,
    /// A digest of everything returned: info, pixels, frames, errors.
    sig: Vec<u64>,
}

fn reports(data: &[u8]) -> Reports {
    let mut r = Reports::default();
    let take = |r: &mut Reports, info: &zencodec::ImageInfo| {
        if let Some(e) = &info.embedded_metadata.exif {
            r.exif.push(e.to_vec());
        }
        if let Some(x) = &info.embedded_metadata.xmp {
            r.xmp.push(x.to_vec());
        }
        if let Some(i) = &info.source_color.icc_profile {
            r.icc.push(i.to_vec());
        }
        r.sig.push(hash(&format!("{info:?}")));
    };
    match AvifDecoderConfig::new().job().probe_full(data) {
        Ok(info) => take(&mut r, &info),
        Err(e) => r.sig.push(hash(&format!("probe: {e}"))),
    }
    match AvifDecoderConfig::new()
        .job()
        .decoder(Cow::Borrowed(data), &[])
        .and_then(Decode::decode)
    {
        Ok(out) => {
            r.decoded = true;
            take(&mut r, out.info());
            r.sig.push(hash(out.pixels().as_strided_bytes()));
        }
        Err(e) => r.sig.push(hash(&format!("decode: {e}"))),
    }
    match AvifDecoderConfig::new()
        .job()
        .animation_frame_decoder(Cow::Borrowed(data), &[])
    {
        Ok(mut a) => {
            let info = a.info().clone();
            take(&mut r, &info);
            for _ in 0..64 {
                match a.render_next_frame_owned(None) {
                    Ok(Some(f)) => r.sig.push(hash(f.pixels().as_strided_bytes())),
                    Ok(None) => break,
                    Err(e) => {
                        r.sig.push(hash(&format!("frame: {e}")));
                        break;
                    }
                }
            }
        }
        Err(e) => r.sig.push(hash(&format!("anim: {e}"))),
    }
    r
}

/// The bytes each `Metadata(kind)` unit hands to the caller: an extent or
/// property minus its children that are not that metadata (an EXIF offset
/// field and padding); a `colr` property or sample-entry child minus its
/// header and colour type.
fn metadata_units(data: &[u8], inv: &Inventory, kind: MetadataKind) -> Vec<Vec<u8>> {
    let parts = inv.parts();
    let mut out = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if p.disposition != Disposition::Metadata(kind) {
            continue;
        }
        // Top-most: the parent is not the same metadata.
        if p.parent
            .and_then(|q| inv.get(q))
            .is_some_and(|q| q.disposition == p.disposition)
        {
            continue;
        }
        let mut b: Vec<u8> = Vec::new();
        let kids: Vec<&Part> = parts
            .iter()
            .filter(|c| c.parent.map(|q| q.index()) == Some(i))
            .collect();
        let mut at = p.range.start;
        for c in kids.iter().filter(|c| c.disposition != p.disposition) {
            b.extend_from_slice(&data[at as usize..c.range.start as usize]);
            at = c.range.end;
        }
        b.extend_from_slice(&data[at as usize..p.range.end as usize]);
        if p.tag == PartTag::FourCc(*b"colr") {
            b.drain(..12.min(b.len()));
        }
        out.push(b);
    }
    out
}

/// The metadata the inventory marks is what some entry point reports, both
/// ways round; a decode that succeeds has image data.
fn check_oracle(name: &str, data: &[u8], inv: &Inventory, r: &Reports) {
    for (kind, reported) in [
        (MetadataKind::Exif, &r.exif),
        (MetadataKind::Xmp, &r.xmp),
        (MetadataKind::Icc, &r.icc),
    ] {
        let units = metadata_units(data, inv, kind);
        for u in &units {
            assert!(
                reported.contains(u),
                "{name}: a {kind:?} part ({} bytes) that no entry point reports\n{inv}",
                u.len()
            );
        }
        for want in reported {
            assert!(
                units.contains(want),
                "{name}: {kind:?} ({} bytes) reported, but no part is that metadata\n{inv}",
                want.len()
            );
        }
    }
    if r.decoded {
        assert!(
            inv.parts()
                .iter()
                .any(|p| p.disposition == Disposition::ImageData),
            "{name}: decodes, but no part is image data\n{inv}"
        );
    }
}

/// Details that say why flipping a consumed part's bytes changes nothing
/// the entry points return.
const NO_CHANGE: &str = "no change in what the caller receives";

/// Flip the payload of every consumed leaf: what the entry points return
/// must change, or the part's detail must say why not.
fn check_reverse(name: &str, data: &[u8], inv: &Inventory, base: &Reports) -> Vec<String> {
    let parts = inv.parts();
    let mut has_child = vec![false; parts.len()];
    for p in parts {
        if let Some(q) = p.parent {
            has_child[q.index()] = true;
        }
    }
    let mut failures = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if has_child[i] || !p.disposition.is_consumed() {
            continue;
        }
        let mut r = p.range.start as usize..p.range.end as usize;
        if matches!(p.kind, PartKind::Box | PartKind::Property | PartKind::Item) {
            let hdr = if bmff::be(data, r.start, 4) == 1 {
                16
            } else {
                8
            };
            r.start = (r.start + hdr).min(r.end);
        }
        if r.is_empty() {
            continue;
        }
        // The whole payload, then only its second half (a colr keeps its
        // colour type and loses its values).
        let half = r.start + r.len() / 2..r.end;
        let changes = [r.clone(), half]
            .into_iter()
            .filter(|x| !x.is_empty())
            .any(|x| {
                let mut m = data.to_vec();
                for b in &mut m[x] {
                    *b ^= 0xA5;
                }
                reports(&m).sig != base.sig
            });
        if !changes && !p.detail.as_deref().is_some_and(|d| d.contains(NO_CHANGE)) {
            failures.push(format!(
                "{name}: {} {} {}..{} {} — flipping its {} bytes changes nothing the caller receives ({:?})",
                p.kind.name(),
                p.tag,
                p.range.start,
                p.range.end,
                p.disposition,
                r.len(),
                p.detail
            ));
        }
    }
    failures
}

/// The deepest part covering `at`.
fn leaf_at(inv: &Inventory, at: u64) -> &Part {
    let parts = inv.parts();
    let depth = |p: &Part| {
        let mut d = 0;
        let mut q = p.parent;
        while let Some(x) = q {
            d += 1;
            q = parts[x.index()].parent;
        }
        d
    };
    parts
        .iter()
        .filter(|p| p.range.start <= at && at < p.range.end)
        .max_by_key(|p| depth(p))
        .expect("every byte is covered")
}

fn find(data: &[u8], s: &[u8]) -> Vec<u64> {
    (0..data.len().saturating_sub(s.len() - 1))
        .filter(|&i| &data[i..i + s.len()] == s)
        .map(|i| i as u64)
        .collect()
}

// ── The cases ───────────────────────────────────────────────────────────────

type Check = Box<dyn Fn(&[u8], &Inventory)>;

struct Case {
    name: &'static str,
    data: Vec<u8>,
    /// zenavif-parse rejects the file: nothing decodes.
    rejected: bool,
    /// `(sentinel, disposition of the leaf holding each occurrence)`.
    sentinels: Vec<(&'static [u8], Disposition)>,
    extra: Option<Check>,
}

fn case(name: &'static str, data: Vec<u8>, sentinels: Vec<(&'static [u8], Disposition)>) -> Case {
    Case {
        name,
        data,
        rejected: false,
        sentinels,
        extra: None,
    }
}

/// The leaf at the last byte of `needle` (inside coded data, past any
/// sequence header).
fn last_byte_is(needle: Vec<u8>, d: Disposition) -> Check {
    Box::new(move |data, inv| {
        let at =
            find(data, &needle).last().copied().expect("needle present") + needle.len() as u64 - 1;
        let leaf = leaf_at(inv, at);
        assert_eq!(leaf.disposition, d, "leaf at {at}: {leaf:?}\n{inv}");
    })
}

/// A property part with this tag and label has this disposition.
fn property_is(tag: &'static [u8; 4], label: Option<&'static str>, d: Disposition) -> Check {
    Box::new(move |_, inv| {
        let hits: Vec<&Part> = inv
            .parts()
            .iter()
            .filter(|p| {
                p.kind == PartKind::Property
                    && p.tag == PartTag::FourCc(*tag)
                    && (label.is_none() || p.label.as_deref() == label)
            })
            .collect();
        assert!(
            !hits.is_empty(),
            "no {} property\n{inv}",
            String::from_utf8_lossy(tag)
        );
        for p in hits {
            assert_eq!(p.disposition, d, "{p:?}\n{inv}");
        }
    })
}

fn cases() -> Vec<Case> {
    use Disposition as D;
    use MetadataKind as K;
    let w = white();
    let a = anim();
    let av1 = w.av1.clone();
    let mut v = vec![
        case("base", Still::base(&w).build(&w), vec![]),
        case(
            "dup_iprp",
            dup_iprp(&w),
            vec![(ICC_FIRST, D::Dropped), (b"IPRP1-PRIVATE", D::Dropped)],
        ),
        case(
            "dup_ipco",
            dup_ipco(&w),
            vec![(ICC_FIRST, D::Dropped), (b"IPCO1-PRIVATE", D::Dropped)],
        ),
        case("dup_ipma", dup_ipma(&w), vec![(ICC_FIRST, D::Unreferenced)]),
        case(
            "dup_infe_exif_first",
            with_exif_items(&w, &[(2, *b"Exif", b""), (2, *b"zzzz", b"priv")]),
            vec![(b"EXIF-SENTINEL", D::Metadata(K::Exif))],
        ),
        case(
            "dup_infe_exif_second",
            with_exif_items(&w, &[(2, *b"zzzz", b"priv"), (2, *b"Exif", b"")]),
            vec![(b"EXIF-SENTINEL", D::Unknown)],
        ),
        Case {
            extra: Some(last_byte_is(av1.clone(), D::ImageData)),
            ..case("dup_infe_primary", dup_infe_primary(&w), vec![])
        },
        Case {
            extra: Some(last_byte_is(av1.clone(), D::ImageData)),
            ..case("meta_last_overrun", meta_last_overrun(&w), vec![])
        },
        Case {
            extra: Some(Box::new(|d, inv| {
                property_is(b"ispe", None, Disposition::Structure)(d, inv);
                property_is(b"av1C", None, Disposition::Structure)(d, inv);
                property_is(
                    b"colr",
                    Some("nclx"),
                    Disposition::Metadata(MetadataKind::Cicp),
                )(d, inv);
            })),
            ..case("iprp_overrun_in_meta", iprp_overrun_in_meta(&w), vec![])
        },
        Case {
            extra: Some(last_byte_is(av1.clone(), D::ImageData)),
            ..case(
                "primary_in_free",
                primary_at(&w, "free"),
                vec![(b"MDAT-JUNK-SENTINEL", D::Unreferenced)],
            )
        },
        Case {
            extra: Some(last_byte_is(av1.clone(), D::ImageData)),
            ..case(
                "primary_in_uuid",
                primary_at(&w, "uuid"),
                vec![(b"MDAT-JUNK-SENTINEL", D::Unreferenced)],
            )
        },
        Case {
            extra: Some(property_is(b"colr", Some("nclx"), D::Unknown)),
            ..case("nclx_short", nclx_short(&w), vec![])
        },
        case(
            "grid_prop_on_av01",
            grid_prop_on_av01(&w),
            vec![(b"GRID-TAIL-SENTINEL", D::Dropped)],
        ),
        case(
            "iref_largesize_v1",
            iref_largesize_v1(&w),
            vec![(b"EXIF-SENTINEL", D::Metadata(K::Exif))],
        ),
        case(
            "meta_items",
            meta_items(&w),
            vec![
                (b"EXIF-SENTINEL", D::Metadata(K::Exif)),
                (b"EXIF4-SENTINE", D::Skipped),
                (b"XMPNAME-SENTINEL", D::Dropped),
                (b"XMP-TAIL-SENTINEL", D::Metadata(K::Xmp)),
            ],
        ),
        case(
            "exif_bad_offset",
            exif_bad_offset(&w),
            vec![(b"BAD-OFFSET-SENT", D::Malformed)],
        ),
        case(
            "second_ftyp",
            second_ftyp(&w),
            vec![(b"SECOND-FTYP-SENT", D::Skipped)],
        ),
        case(
            "anim_track_exif",
            anim_build(
                &a,
                None,
                TrackExif::Mdat(EXIF_T, bx(b"zPrv", b"TRACKMETA-PRIVATE-SENTINEL")),
                None,
            ),
            vec![
                (b"TRACK-EXIF-SENTINEL", D::Metadata(K::Exif)),
                (b"TRACKMETA-PRIVATE-SENTINEL", D::Unknown),
            ],
        ),
        case(
            "anim_both_exif",
            anim_build(&a, Some(EXIF_P), TrackExif::Mdat(EXIF_T, Vec::new()), None),
            vec![
                (b"PRIMARY-EXIF-SENT", D::Metadata(K::Exif)),
                (b"TRACK-EXIF-SENTINEL", D::Metadata(K::Exif)),
            ],
        ),
        case(
            "anim_track_idat",
            anim_build(&a, None, TrackExif::Idat(EXIF_T), None),
            vec![
                (b"TRACK-EXIF-SENTINEL", D::Metadata(K::Exif)),
                (b"IDAT-SLACK-SENTINEL", D::Unreferenced),
            ],
        ),
        case(
            "anim_two_av01_entries",
            anim_build(&a, None, TrackExif::None, Some(&second_entry)),
            vec![(b"SECOND-ENTRY-ICC-SENTINEL!!!", D::Skipped)],
        ),
        Case {
            rejected: true,
            ..case(
                "anim_mdhd_after_stbl",
                anim_build(&a, None, TrackExif::None, Some(&minf_tail)),
                vec![],
            )
        },
        case(
            "anim_dup_moov",
            anim_dup_moov(&a),
            vec![(b"FIRST-MOOV-ICC-SENTINEL!!!", D::Dropped)],
        ),
        case(
            "gm_tmap_icc_first",
            gm_tmap_icc_first(),
            vec![(b"TMAP-ALT-ICC-SENTINEL", D::Metadata(K::GainMap))],
        ),
        case("gm_frame_overwritten", gm_frame_overwritten(), vec![]),
    ];
    v.push(case(
        "anim_base",
        anim_build(&a, None, TrackExif::None, None),
        vec![],
    ));
    v
}

#[test]
fn review_cases_conform_and_pin_their_sentinels() {
    let mut failures = Vec::new();
    for c in cases() {
        let r = reports(&c.data);
        if c.rejected {
            assert!(
                !r.decoded,
                "{}: expected zenavif-parse to reject it",
                c.name
            );
        } else {
            zencodec_testkit::check_inventory(AvifDecoderConfig::new(), &c.data)
                .unwrap_or_else(|e| panic!("{}: {e}", c.name));
        }
        let inv = inventory_of(&c.data);
        check_oracle(c.name, &c.data, &inv, &r);
        for (s, want) in &c.sentinels {
            let hits = find(&c.data, s);
            assert!(
                !hits.is_empty(),
                "{}: sentinel {:?} not in the file",
                c.name,
                String::from_utf8_lossy(s)
            );
            for at in hits {
                let leaf = leaf_at(&inv, at);
                if leaf.disposition != *want {
                    failures.push(format!(
                        "{}: {:?} at {at} is in {} {} {}..{} {} (want {want}): {:?}",
                        c.name,
                        String::from_utf8_lossy(s),
                        leaf.kind.name(),
                        leaf.tag,
                        leaf.range.start,
                        leaf.range.end,
                        leaf.disposition,
                        leaf.detail
                    ));
                }
            }
        }
        if let Some(extra) = &c.extra {
            extra(&c.data, &inv);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn flipping_a_consumed_part_changes_the_output() {
    let mut failures = Vec::new();
    for c in cases() {
        let inv = inventory_of(&c.data);
        let base = reports(&c.data);
        failures.extend(check_reverse(c.name, &c.data, &inv, &base));
    }
    assert!(
        failures.is_empty(),
        "{} parts:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// An iloc with more extents than the walker's budget (65,536): the bytes
/// no listed extent covers may be item data, so they are Unknown, not slack.
#[test]
fn iloc_budget_exhaustion_leaves_the_rest_unknown() {
    let w = white();
    let once = |mdat_at: u32| -> Vec<u8> {
        // Two items of 40,000 one-byte extents each, all at the mdat start.
        let mut p = vec![0x44, 0x00];
        p.extend_from_slice(&2u16.to_be_bytes());
        for id in [1u16, 2] {
            p.extend_from_slice(&id.to_be_bytes());
            p.extend_from_slice(&0u16.to_be_bytes());
            p.extend_from_slice(&0u16.to_be_bytes());
            p.extend_from_slice(&40_000u16.to_be_bytes());
            for _ in 0..40_000 {
                p.extend_from_slice(&(mdat_at + 8).to_be_bytes());
                p.extend_from_slice(&1u32.to_be_bytes());
            }
        }
        let meta = full(
            b"meta",
            0,
            0,
            &cat(&[
                &w.hdlr,
                &full(b"pitm", 0, 0, &1u16.to_be_bytes()),
                &full(b"iloc", 1, 0, &p),
            ]),
        );
        cat(&[&w.ftyp, &meta, &bx(b"mdat", b"BUDGET-TAIL-SENTINEL")])
    };
    let d = once(0);
    let at = d.windows(4).rposition(|x| x == b"mdat").unwrap() as u32 - 4;
    let d = once(at);
    let inv = inventory_of(&d);
    let s = find(&d, b"TAIL-SENTINEL")[0];
    let leaf = leaf_at(&inv, s);
    assert_eq!(leaf.disposition, Disposition::Unknown, "{leaf:?}");
}

/// An XMP item whose extent starts inside the EXIF item's padding (the bytes
/// the TIFF-header offset skips): the shared bytes are handed over as XMP,
/// so they must not stay Dropped padding.
#[test]
fn shared_bytes_inside_exif_padding_stay_consumed() {
    let w = white();
    let exif = b"\0\0\0\x08PADPADPAMM\0*\0\0\0\x08\0\0EXIF-SENTINEL".to_vec();
    let xmp = b"<?xpacket begin?><x:xmpmeta/><?xpacket end=\"w\"?>".to_vec();
    let mut s = Still::base(&w);
    s.items.push((2, *b"Exif", Vec::new(), None));
    s.items.push((
        3,
        *b"mime",
        Vec::new(),
        Some(b"application/rdf+xml".to_vec()),
    ));
    s.chunks.push((Some(2), exif.clone()));
    s.chunks.push((Some(3), xmp.clone()));
    s.irefs = vec![(*b"cdsc", 2, vec![1]), (*b"cdsc", 3, vec![1])];
    let mut d = s.build(&w);
    // Item 3's extent: start it 6 bytes into item 2, inside its padding.
    let back = exif.len() as u32 - 6;
    let b = bmff::locate(&d, &[b"meta", b"iloc"], 0);
    let mut p = b.pos + b.hdr + 4 + 2 + 2;
    loop {
        let id = bmff::be(&d, p, 2);
        let n = bmff::be(&d, p + 6, 2) as usize;
        if id == 3 {
            let o = bmff::be(&d, p + 8, 4) as u32 - back;
            let l = bmff::be(&d, p + 12, 4) as u32 + back;
            d[p + 8..p + 12].copy_from_slice(&o.to_be_bytes());
            d[p + 12..p + 16].copy_from_slice(&l.to_be_bytes());
            break;
        }
        p += 8 + 8 * n;
    }
    let inv = inventory_of(&d);
    let pad = find(&d, b"PADPADPA")[0];
    for at in pad + 2..pad + 8 {
        let leaf = leaf_at(&inv, at);
        assert!(leaf.disposition.is_consumed(), "byte {at}: {leaf:?}\n{inv}");
    }
}

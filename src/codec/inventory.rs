//! Structural inventory of an AVIF file, for zencodec's
//! `DecodeJob::inventory`.
//!
//! [`inventory`] walks every ISOBMFF box without decoding pixels and records
//! what the zencodec decode path (`ManagedAvifDecoder` over
//! `zenavif_parse::AvifParser`) does with each byte.
//!
//! zenavif-parse is strict: when it rejects a file, the decode fails and
//! nothing reaches the caller, so every item and property is `Dropped` and the
//! reason is recorded on the `ftyp`/`meta` part. When it accepts the file, the
//! item-selection rules below replicate `AvifParser::build` (zenavif-parse
//! `src/lib.rs`) and the conversion in `codec/info.rs`; each rule cites the
//! code it mirrors. Animation samples are located through the parser itself
//! (`AvifParser::frame`, which borrows the input), so their ranges are exact.
//!
//! Shape of the result:
//! - every box is a `Box` part, recursively for container boxes (`meta`,
//!   `iinf`, `iprp`, `ipco`, `iref`, `dinf`, `grpl`, `moov`/`trak`/…), and
//!   each container declares a `body` its children tile;
//! - every `infe` is an `Item` part tagged with its item ID;
//! - every `ipco` child is a `Property` part;
//! - every `iloc` extent, and every animation sample, is an `Extent` part
//!   inside the `mdat` or `idat` that holds it; the other `mdat`/`idat` bytes
//!   are gaps. Extents that overlap or point outside every `mdat`/`idat` are
//!   listed in the `iloc` part's detail instead;
//! - bytes after the last box are `Trailing`; a box that runs past its
//!   container, or bytes inside a container too short for a box header, are
//!   `Malformed`.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use whereat::{At, at};
use zencodec::ImageFormat;
use zencodec::inventory::{
    DEFAULT_MAX_PARTS, Disposition, Inventory, InventoryError, MetadataKind, Part, PartId,
    PartKind, PartTag,
};

use crate::error::Error;

/// Containers nested deeper than this are reported whole, not walked.
const MAX_DEPTH: usize = 32;
/// Labels are copied from the file, at most this many bytes.
const MAX_LABEL: usize = 64;
/// Remarks kept per part; the rest are counted.
const MAX_NOTES: usize = 24;
/// Extents read from one `iloc` box; more are counted, not placed.
const MAX_ILOC_EXTENTS: usize = 1 << 16;

/// zenavif-parse `AvifParser::build`: the only alpha URN it recognises.
const ALPHA_URN: &[u8] = b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha";
/// zenavif-parse `is_depth_auxiliary_urn`.
const DEPTH_URNS: [&[u8]; 2] = [
    b"urn:mpeg:mpegB:cicp:systems:auxiliary:depth",
    b"urn:mpeg:hevc:2015:auxid:2",
];

/// How the job uses the gain map (codec/decode_job.rs `decoder_inner`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GainMapUse {
    /// The gain-map item is read for its sequence header and reported in
    /// `ImageInfo::gain_map`; its pixels are decoded only on request
    /// (`extract_gain_map`, `GainMapRender::Components`).
    Describe,
    /// `GainMapRender::ReconstructHdr`: the gain map shapes the output pixels.
    Reconstruct,
}

/// The parts of the job's configuration that change what the decode reads.
#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub(crate) gain_map: GainMapUse,
    /// `DecodePolicy` keeps these (codec/info.rs `apply_decode_policy`).
    pub(crate) keep_icc: bool,
    pub(crate) keep_exif: bool,
    pub(crate) keep_xmp: bool,
    /// The container parser configuration the decode uses
    /// (`ManagedAvifDecoder::new_for_source`).
    pub(crate) parse_config: zenavif_parse::DecodeConfig,
}

/// Map every structural part of `data`. Errors only on cancellation or when
/// the part cap is reached.
pub(crate) fn inventory(
    data: &[u8],
    opts: &Options,
    stop: &dyn enough::Stop,
) -> Result<Inventory, At<Error>> {
    let mut w = Walker::new(data, stop);
    w.walk(None, 0, data.len() as u64, Ctx::Top, 0)?;

    let parsed = zenavif_parse::AvifParser::from_bytes_with_config(data, &opts.parse_config, stop);
    match parsed {
        Ok(parser) => {
            let model = Model::build(&w, &parser, opts);
            w.resolve(Some((&parser, &model)))?;
        }
        Err(e) => {
            if let zenavif_parse::Error::Stopped(reason) = e.error() {
                return Err(at!(Error::Cancelled(*reason)));
            }
            let why = format!(
                "zenavif-parse rejects this file ({}); nothing is decoded",
                e.error()
            );
            let target = w
                .metas
                .first()
                .copied()
                .or(w.ftyp)
                .or(if w.nodes.is_empty() { None } else { Some(0) });
            if let Some(t) = target {
                w.note(t, why);
            }
            w.resolve(None)?;
        }
    }
    w.emit()
}

// ── Box walk ─────────────────────────────────────────────────────────────

/// One part, before it is pushed into the [`Inventory`].
struct Node {
    parent: Option<usize>,
    kind: PartKind,
    tag: PartTag,
    label: Option<String>,
    range: Range<u64>,
    body: Option<Range<u64>>,
    disp: Disposition,
    notes: Vec<String>,
    dropped_notes: usize,
    /// For `mdat`/`idat`: the disposition of body bytes no extent covers.
    gap_fill: Option<Disposition>,
    /// Sub-ranges the decoder does not read (a tail after the fields its
    /// parser reads), added as child parts when the part ends up consumed.
    inner: Vec<(Range<u64>, Disposition, String)>,
}

/// Where a box sits, which decides how zenavif-parse treats it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ctx {
    Top,
    /// Children of the top-level `meta` (`read_avif_meta_scoped`).
    Meta,
    Iinf,
    Iprp,
    Ipco,
    Iref,
    /// A `moov` subtree (`read_moov`), read for animation.
    Moov,
    /// Sample entries in a track's `stsd` (`read_stsd`).
    Stsd,
    /// Children of an `av01` sample entry (`read_stsd`).
    Av01Entry,
    /// A track-level `meta` (`read_avif_meta_scoped`, no primary required),
    /// and everything below it: read for the track's EXIF/XMP sidecars.
    TrackMeta,
    /// Entity groups in `grpl` (`read_grpl`): parsed, never used.
    Grpl,
    /// A subtree zenavif-parse does not read.
    Unread,
}

/// How a box's children are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Leaf,
    /// Children start right after the header.
    Plain,
    /// FullBox: 4 bytes of version and flags, then children.
    Full,
    /// FullBox plus a `u32` entry count (`dref`, `stsd`).
    FullCount,
    /// `iinf`: FullBox plus a `u16` (version 0) or `u32` entry count.
    Iinf,
    /// A visual sample entry: 78 bytes of fixed fields, then children.
    VisualSampleEntry,
    /// An audio sample entry: 28 bytes of fixed fields, then children.
    AudioSampleEntry,
    /// A `meta` below the top level: QuickTime-style (no version/flags)
    /// when `hdlr` follows the header directly, ISO FullBox otherwise.
    NestedMeta,
    /// `mdat`/`idat`: the payload holds item extents.
    Data,
}

struct Hdr {
    typ: [u8; 4],
    /// 8, or 16 with a 64-bit size.
    base_header: u64,
    /// `base_header` plus 16 for a `uuid` usertype.
    header: u64,
    end: u64,
    usertype: Option<[u8; 16]>,
}

enum HdrOutcome {
    Ok(Hdr),
    /// Fewer bytes than a box header needs.
    Short,
    /// The size field is smaller than the header.
    BadSize {
        typ: [u8; 4],
        size: u64,
    },
    /// The box runs past its container.
    Overrun {
        typ: [u8; 4],
        declared: u64,
    },
}

fn get(data: &[u8], r: Range<u64>) -> Option<&[u8]> {
    let s = usize::try_from(r.start).ok()?;
    let e = usize::try_from(r.end).ok()?;
    data.get(s..e)
}

fn slice(data: &[u8], r: Range<u64>) -> &[u8] {
    get(data, r).unwrap_or(&[])
}

fn be_u32(data: &[u8], at: u64) -> Option<u32> {
    let b = get(data, at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn fourcc_at(data: &[u8], at: u64) -> Option<[u8; 4]> {
    let b = get(data, at..at.checked_add(4)?)?;
    Some([b[0], b[1], b[2], b[3]])
}

/// Box header, with the size rules of zenavif-parse's `read_box_header`.
fn read_header(data: &[u8], pos: u64, limit: u64) -> HdrOutcome {
    let avail = limit.saturating_sub(pos);
    if avail < 8 {
        return HdrOutcome::Short;
    }
    let (Some(size32), Some(typ)) = (be_u32(data, pos), fourcc_at(data, pos + 4)) else {
        return HdrOutcome::Short;
    };
    let (size, base_header) = match size32 {
        1 => {
            if avail < 16 {
                return HdrOutcome::Short;
            }
            let (Some(hi), Some(lo)) = (be_u32(data, pos + 8), be_u32(data, pos + 12)) else {
                return HdrOutcome::Short;
            };
            ((u64::from(hi) << 32) | u64::from(lo), 16)
        }
        0 => (avail, 8),
        n => (u64::from(n), 8),
    };
    if size < base_header {
        return HdrOutcome::BadSize { typ, size };
    }
    if size > avail {
        return HdrOutcome::Overrun {
            typ,
            declared: size,
        };
    }
    let mut header = base_header;
    let mut usertype = None;
    if &typ == b"uuid" && size >= base_header + 16 {
        let u = slice(data, pos + base_header..pos + base_header + 16);
        if let Ok(u) = <[u8; 16]>::try_from(u) {
            usertype = Some(u);
            header += 16;
        }
    }
    HdrOutcome::Ok(Hdr {
        typ,
        base_header,
        header,
        end: pos + size,
        usertype,
    })
}

fn fourcc_str(cc: &[u8; 4]) -> String {
    if cc.iter().all(|b| (0x20..0x7f).contains(b)) {
        cc.iter().map(|&b| b as char).collect()
    } else {
        format!("0x{:08X}", u32::from_be_bytes(*cc))
    }
}

/// A label copied from the file: at most [`MAX_LABEL`] bytes, up to the
/// first NUL, invalid UTF-8 replaced.
fn label_from(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let b = &bytes[..end.min(MAX_LABEL)];
    if b.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(b).into_owned())
}

fn uuid_str(u: &[u8; 16]) -> String {
    let mut s = String::with_capacity(36);
    for (i, b) in u.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The byte up to the first NUL, and the rest after it.
fn cstr(b: &[u8]) -> (&[u8], Option<&[u8]>) {
    match b.iter().position(|&c| c == 0) {
        Some(i) => (&b[..i], Some(&b[i + 1..])),
        None => (b, None),
    }
}

// ── Minimal readers for the boxes the model needs ───────────────────────

/// An `infe` entry.
#[derive(Clone, Debug)]
struct Infe {
    id: u32,
    version: u8,
    typ: [u8; 4],
    protection: u16,
    hidden: bool,
    name: Vec<u8>,
    content_type: Vec<u8>,
}

impl Infe {
    /// zenavif-parse `read_infe`: XMP is a `mime` item whose content type is
    /// exactly `application/rdf+xml`.
    fn is_xmp(&self) -> bool {
        &self.typ == b"mime" && self.content_type == b"application/rdf+xml"
    }
}

fn read_infe(c: &[u8]) -> Option<Infe> {
    let version = *c.first()?;
    let hidden = c.get(3)? & 1 != 0;
    let mut p = 4usize;
    let take = |p: &mut usize, n: usize| -> Option<&[u8]> {
        let s = c.get(*p..p.checked_add(n)?)?;
        *p += n;
        Some(s)
    };
    let (id, protection, typ) = match version {
        0 | 1 => {
            let id = u16::from_be_bytes(take(&mut p, 2)?.try_into().ok()?);
            let prot = u16::from_be_bytes(take(&mut p, 2)?.try_into().ok()?);
            (u32::from(id), prot, *b"    ")
        }
        2 | 3 => {
            let id = if version == 2 {
                u32::from(u16::from_be_bytes(take(&mut p, 2)?.try_into().ok()?))
            } else {
                u32::from_be_bytes(take(&mut p, 4)?.try_into().ok()?)
            };
            let prot = u16::from_be_bytes(take(&mut p, 2)?.try_into().ok()?);
            let typ: [u8; 4] = take(&mut p, 4)?.try_into().ok()?;
            (id, prot, typ)
        }
        _ => return None,
    };
    let rest = c.get(p..).unwrap_or(&[]);
    let (name, after) = cstr(rest);
    let content_type = match (&typ, version, after) {
        (b"mime", _, Some(a)) | (_, 0 | 1, Some(a)) => cstr(a).0.to_vec(),
        (b"uri ", _, Some(a)) => cstr(a).0.to_vec(),
        _ => Vec::new(),
    };
    Some(Infe {
        id,
        version,
        typ,
        protection,
        hidden,
        name: name.to_vec(),
        content_type,
    })
}

/// One `iloc` entry.
#[derive(Clone, Debug)]
struct Loc {
    id: u32,
    /// 0 file, 1 idat, 2 item.
    method: u8,
    data_ref: u16,
    base: u64,
    /// `(offset, length)`; length 0 means "to the end of the source".
    extents: Vec<(u64, u64)>,
}

struct Bits<'a> {
    c: &'a [u8],
    p: usize,
}

impl Bits<'_> {
    fn n(&mut self, bytes: u8) -> Option<u64> {
        let b = self
            .c
            .get(self.p..self.p.checked_add(usize::from(bytes))?)?;
        self.p += usize::from(bytes);
        Some(b.iter().fold(0u64, |v, &x| (v << 8) | u64::from(x)))
    }
}

/// `(entries, extents not read because of [`MAX_ILOC_EXTENTS`])`.
fn read_iloc(c: &[u8]) -> Result<(Vec<Loc>, usize), String> {
    let version = *c.first().ok_or("empty iloc")?;
    if version > 2 {
        return Err(format!("iloc version {version}"));
    }
    let size = |n: u8| -> Result<u8, String> {
        match n {
            0 | 4 | 8 => Ok(n),
            _ => Err(format!("iloc field size {n}")),
        }
    };
    let b4 = *c.get(4).ok_or("iloc too short")?;
    let b5 = *c.get(5).ok_or("iloc too short")?;
    let (off_sz, len_sz, base_sz) = (size(b4 >> 4)?, size(b4 & 15)?, size(b5 >> 4)?);
    let idx_sz = if version >= 1 { size(b5 & 15)? } else { 0 };
    let mut r = Bits { c, p: 6 };
    let short = || "iloc truncated".to_string();
    let count = r.n(if version == 2 { 4 } else { 2 }).ok_or_else(short)?;
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut skipped = 0usize;
    for _ in 0..count {
        let id = r.n(if version == 2 { 4 } else { 2 }).ok_or_else(short)? as u32;
        let method = if version >= 1 {
            (r.n(2).ok_or_else(short)? & 15) as u8
        } else {
            0
        };
        let data_ref = r.n(2).ok_or_else(short)? as u16;
        let base = r.n(base_sz).ok_or_else(short)?;
        let n = r.n(2).ok_or_else(short)? as usize;
        let mut extents = Vec::new();
        for _ in 0..n {
            r.n(idx_sz).ok_or_else(short)?;
            let o = r.n(off_sz).ok_or_else(short)?;
            let l = r.n(len_sz).ok_or_else(short)?;
            if total < MAX_ILOC_EXTENTS {
                extents.push((o, l));
                total += 1;
            } else {
                skipped += 1;
            }
        }
        out.push(Loc {
            id,
            method,
            data_ref,
            base,
            extents,
        });
        if out.len() >= MAX_ILOC_EXTENTS {
            break;
        }
    }
    Ok((out, skipped))
}

/// One `iref` child: `(type, from, [to])`.
fn read_iref_entry(c: &[u8], version: u8) -> Option<(u32, Vec<u32>)> {
    let w = if version == 0 { 2 } else { 4 };
    let mut r = Bits { c, p: 0 };
    let from = r.n(w)? as u32;
    let n = r.n(2)?;
    let mut to = Vec::new();
    for _ in 0..n {
        to.push(r.n(w)? as u32);
    }
    Some((from, to))
}

/// `ipma`: `(item, essential, 1-based property index)` in order.
fn read_ipma(c: &[u8]) -> Option<Vec<(u32, bool, u16)>> {
    let version = *c.first()?;
    let wide = c.get(3)? & 1 != 0;
    let mut r = Bits { c, p: 4 };
    let count = r.n(4)?;
    let mut out = Vec::new();
    for _ in 0..count {
        let id = r.n(if version == 0 { 2 } else { 4 })? as u32;
        let n = r.n(1)?;
        for _ in 0..n {
            let v = r.n(if wide { 2 } else { 1 })?;
            let (essential, idx) = if wide {
                (v & 0x8000 != 0, (v & 0x7FFF) as u16)
            } else {
                (v & 0x80 != 0, (v & 0x7F) as u16)
            };
            out.push((id, essential, idx));
        }
        if out.len() > 1 << 20 {
            break;
        }
    }
    Some(out)
}

// ── The walk ─────────────────────────────────────────────────────────────

struct InfeRec {
    node: usize,
    info: Option<Infe>,
}

struct PropRec {
    node: usize,
    typ: [u8; 4],
    /// The `colr` colour type.
    colour_type: Option<[u8; 4]>,
    /// The `auxC` URN.
    urn: Option<Vec<u8>>,
}

struct Walker<'a> {
    data: &'a [u8],
    stop: &'a dyn enough::Stop,
    nodes: Vec<Node>,
    max_nodes: usize,
    ftyp: Option<usize>,
    metas: Vec<usize>,
    moovs: Vec<usize>,
    mdats: Vec<usize>,
    /// `(node, child of the top-level meta)`.
    idats: Vec<(usize, bool)>,
    pitm: Option<(usize, u32)>,
    infes: Vec<InfeRec>,
    /// `(node, entries or the reason they could not be read, extents not read)`.
    ilocs: Vec<(usize, Result<Vec<Loc>, String>, usize)>,
    props: Vec<PropRec>,
    /// `(node, type, from, [to])`.
    irefs: Vec<(usize, [u8; 4], u32, Vec<u32>)>,
    ipma: Vec<(u32, bool, u16)>,
    ipmas: Vec<usize>,
}

impl<'a> Walker<'a> {
    fn new(data: &'a [u8], stop: &'a dyn enough::Stop) -> Self {
        Self {
            data,
            stop,
            nodes: Vec::new(),
            max_nodes: DEFAULT_MAX_PARTS as usize,
            ftyp: None,
            metas: Vec::new(),
            moovs: Vec::new(),
            mdats: Vec::new(),
            idats: Vec::new(),
            pitm: None,
            infes: Vec::new(),
            ilocs: Vec::new(),
            props: Vec::new(),
            irefs: Vec::new(),
            ipma: Vec::new(),
            ipmas: Vec::new(),
        }
    }

    fn check_stop(&self) -> Result<(), At<Error>> {
        self.stop.check().map_err(|r| at!(Error::Cancelled(r)))
    }

    fn add(&mut self, node: Node) -> Result<usize, At<Error>> {
        if self.nodes.len() >= self.max_nodes {
            return Err(at!(Error::ResourceLimit(
                "inventory exceeds the zencodec part cap".to_string()
            )));
        }
        self.nodes.push(node);
        Ok(self.nodes.len() - 1)
    }

    fn note(&mut self, node: usize, note: impl Into<String>) {
        if let Some(n) = self.nodes.get_mut(node) {
            if n.notes.len() < MAX_NOTES {
                n.notes.push(note.into());
            } else {
                n.dropped_notes += 1;
            }
        }
    }

    fn leaf(
        parent: Option<usize>,
        kind: PartKind,
        tag: PartTag,
        range: Range<u64>,
        disp: Disposition,
    ) -> Node {
        Node {
            parent,
            kind,
            tag,
            label: None,
            range,
            body: None,
            disp,
            notes: Vec::new(),
            dropped_notes: 0,
            gap_fill: None,
            inner: Vec::new(),
        }
    }

    fn malformed(
        &mut self,
        parent: Option<usize>,
        tag: PartTag,
        range: Range<u64>,
        why: String,
    ) -> Result<(), At<Error>> {
        let kind = if tag == PartTag::None {
            PartKind::Gap
        } else {
            PartKind::Box
        };
        let mut n = Self::leaf(parent, kind, tag, range, Disposition::Malformed);
        n.notes.push(why);
        self.add(n)?;
        Ok(())
    }

    fn walk(
        &mut self,
        parent: Option<usize>,
        start: u64,
        end: u64,
        ctx: Ctx,
        depth: usize,
    ) -> Result<(), At<Error>> {
        let mut pos = start;
        while pos < end {
            self.check_stop()?;
            match read_header(self.data, pos, end) {
                HdrOutcome::Short => {
                    if ctx != Ctx::Top {
                        self.malformed(
                            parent,
                            PartTag::None,
                            pos..end,
                            format!("{} bytes, too short for a box header", end - pos),
                        )?;
                    }
                    // At the top level, `fill_gaps(None, Trailing)` covers it.
                    return Ok(());
                }
                HdrOutcome::BadSize { typ, size } => {
                    self.malformed(
                        parent,
                        PartTag::FourCc(typ),
                        pos..end,
                        format!("size field {size} is smaller than the box header"),
                    )?;
                    return Ok(());
                }
                HdrOutcome::Overrun { typ, declared } => {
                    let room = if ctx == Ctx::Top {
                        "the file"
                    } else {
                        "its container"
                    };
                    self.malformed(
                        parent,
                        PartTag::FourCc(typ),
                        pos..end,
                        format!("declares {declared} bytes, {room} has {} left", end - pos),
                    )?;
                    return Ok(());
                }
                HdrOutcome::Ok(h) => {
                    self.visit(parent, pos, &h, ctx, depth)?;
                    pos = h.end;
                }
            }
        }
        Ok(())
    }

    fn visit(
        &mut self,
        parent: Option<usize>,
        pos: u64,
        h: &Hdr,
        ctx: Ctx,
        depth: usize,
    ) -> Result<(), At<Error>> {
        let range = pos..h.end;
        let (disp, mut layout, child_ctx, why) = classify(ctx, &h.typ);
        let mut kind = match ctx {
            Ctx::Ipco => PartKind::Property,
            _ => PartKind::Box,
        };
        let mut tag = PartTag::FourCc(h.typ);
        let mut label = h.usertype.as_ref().map(uuid_str);
        let mut notes: Vec<String> = Vec::new();
        if let Some(why) = why {
            notes.push(why.to_string());
        }
        if &h.typ == b"uuid" && h.usertype.is_none() {
            notes.push("uuid box too short for its 16-byte usertype".to_string());
        }
        if depth >= MAX_DEPTH && layout != Layout::Leaf {
            layout = Layout::Leaf;
            notes.push(format!("nested deeper than {MAX_DEPTH} boxes; not walked"));
        }
        let content = pos + h.base_header..h.end;
        let c = slice(self.data, content.clone());

        let mut infe = None;
        match (ctx, &h.typ) {
            (Ctx::Top, b"ftyp") => {
                label = label_from(c.get(..4).unwrap_or(&[]));
                let brands: Vec<String> = c
                    .get(8..)
                    .unwrap_or(&[])
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .take(16)
                    .map(fourcc_str)
                    .collect();
                if !brands.is_empty() {
                    notes.push(format!("compatible brands {}", brands.join(",")));
                }
            }
            (_, b"hdlr") => label = label_from(c.get(8..12).unwrap_or(&[])),
            (Ctx::Iinf, b"infe") => {
                let info = read_infe(c);
                match &info {
                    Some(i) => {
                        kind = PartKind::Item;
                        tag = PartTag::Code(i.id);
                        label = label_from(&i.name).or_else(|| Some(fourcc_str(&i.typ)));
                        notes.push(format!("type {}", fourcc_str(&i.typ)));
                        if !i.content_type.is_empty() {
                            notes.push(format!(
                                "content type {:?}",
                                label_from(&i.content_type).unwrap_or_default()
                            ));
                        }
                        if i.hidden {
                            notes.push("hidden".to_string());
                        }
                        // zenavif-parse `read_infe` errors on both.
                        if i.version < 2 {
                            notes.push(format!(
                                "infe version {}: zenavif-parse rejects it",
                                i.version
                            ));
                        }
                        if i.protection != 0 {
                            notes.push(format!(
                                "protected (item_protection_index {}): zenavif-parse rejects it",
                                i.protection
                            ));
                        }
                    }
                    None => notes.push("unreadable infe entry".to_string()),
                }
                infe = Some(info);
            }
            _ => {}
        }

        let body = match layout {
            Layout::Leaf => None,
            Layout::Plain | Layout::Data => Some(pos + h.header..h.end),
            Layout::Full => Some(pos + h.header + 4..h.end),
            Layout::FullCount => Some(pos + h.header + 8..h.end),
            // The fixed fields become a child part (below), so the body
            // starts right after the header.
            Layout::VisualSampleEntry if h.end >= pos + h.header + 78 => {
                Some(pos + h.header..h.end)
            }
            Layout::AudioSampleEntry if h.end >= pos + h.header + 28 => Some(pos + h.header..h.end),
            Layout::VisualSampleEntry | Layout::AudioSampleEntry => None,
            Layout::NestedMeta => {
                if fourcc_at(self.data, pos + h.header + 4) == Some(*b"hdlr") {
                    notes.push("QuickTime-style meta (no version/flags)".to_string());
                    Some(pos + h.header..h.end)
                } else {
                    Some(pos + h.header + 4..h.end)
                }
            }
            Layout::Iinf => match slice(self.data, pos + h.header..h.end).first() {
                Some(0) => Some(pos + h.header + 6..h.end),
                Some(_) => Some(pos + h.header + 8..h.end),
                None => None,
            },
        };
        let body = match body {
            Some(b) if b.start <= b.end => Some(b),
            _ if layout != Layout::Leaf => {
                notes.push("too short for its fixed fields; contents not walked".to_string());
                None
            }
            _ => None,
        };

        let mut node = Self::leaf(parent, kind, tag, range, disp);
        node.label = label;
        node.notes = notes;
        node.body = body.clone();
        if layout == Layout::Leaf {
            node.inner = inner_parts(ctx, &h.typ, content.clone(), c);
        }
        if layout == Layout::Data && body.is_some() {
            node.gap_fill = Some(Disposition::Unreferenced);
        }
        let id = self.add(node)?;

        match (ctx, &h.typ) {
            (Ctx::Top, b"ftyp") => {
                if self.ftyp.is_none() {
                    self.ftyp = Some(id);
                }
            }
            (Ctx::Top, b"meta") => self.metas.push(id),
            (Ctx::Top, b"moov") => self.moovs.push(id),
            (Ctx::Top, b"mdat") => self.mdats.push(id),
            (Ctx::Meta, b"pitm") => {
                let v = c.first().copied();
                let item = match v {
                    Some(0) => c
                        .get(4..6)
                        .map(|b| u32::from(u16::from_be_bytes([b[0], b[1]]))),
                    Some(1) => c
                        .get(4..8)
                        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
                    _ => None,
                };
                if let Some(item) = item
                    && self.pitm.is_none()
                {
                    self.pitm = Some((id, item));
                }
            }
            (Ctx::Meta, b"iloc") => {
                let r = read_iloc(c);
                let (entries, skipped) = match r {
                    Ok((e, s)) => (Ok(e), s),
                    Err(e) => (Err(e), 0),
                };
                self.ilocs.push((id, entries, skipped));
            }
            (Ctx::Meta, b"idat") => self.idats.push((id, true)),
            (_, b"idat") => self.idats.push((id, false)),
            (Ctx::Meta, b"iref") => {}
            (Ctx::Iref, typ) => {
                let version = parent
                    .and_then(|p| self.nodes.get(p))
                    .and_then(|p| slice(self.data, p.range.clone()).get(8).copied())
                    .unwrap_or(0);
                if let Some((from, to)) = read_iref_entry(c, version) {
                    self.irefs.push((id, *typ, from, to));
                } else {
                    self.note(id, "unreadable reference entry");
                }
            }
            (Ctx::Iprp, b"ipma") => {
                if let Some(v) = read_ipma(c) {
                    self.ipma.extend(v);
                }
                self.ipmas.push(id);
            }
            (Ctx::Ipco, typ) => {
                let colour_type = (typ == b"colr")
                    .then(|| c.get(..4).and_then(|b| <[u8; 4]>::try_from(b).ok()))
                    .flatten();
                let urn = (typ == b"auxC")
                    .then(|| c.get(4..).map(|b| cstr(b).0.to_vec()))
                    .flatten();
                if let Some(ct) = colour_type {
                    self.nodes[id].label = Some(fourcc_str(&ct));
                }
                if let Some(u) = &urn {
                    self.nodes[id].label = label_from(u);
                }
                self.props.push(PropRec {
                    node: id,
                    typ: *typ,
                    colour_type,
                    urn,
                });
            }
            _ => {}
        }
        if let Some(info) = infe {
            self.infes.push(InfeRec { node: id, info });
        }

        if let Some(b) = body
            && layout != Layout::Data
        {
            let mut start = b.start;
            let fixed = match layout {
                Layout::VisualSampleEntry => 78,
                Layout::AudioSampleEntry => 28,
                _ => 0,
            };
            if fixed > 0 {
                // `read_stsd` skips the fixed fields (compressorname included).
                let mut n = Self::leaf(
                    Some(id),
                    PartKind::Gap,
                    PartTag::None,
                    start..start + fixed,
                    Disposition::Skipped,
                );
                n.notes.push(
                    "sample entry fixed fields (sizes, resolution, compressorname): zenavif-parse skips them"
                        .to_string(),
                );
                self.add(n)?;
                start += fixed;
            }
            self.walk(Some(id), start, b.end, child_ctx, depth + 1)?;
        }
        Ok(())
    }
}

/// zenavif-parse's treatment of a box type in a context:
/// `(disposition, layout, context for its children, remark)`.
fn classify(ctx: Ctx, typ: &[u8; 4]) -> (Disposition, Layout, Ctx, Option<&'static str>) {
    use Disposition as D;
    match ctx {
        // zenavif-parse `AvifParser::parse_raw`.
        Ctx::Top => match typ {
            b"ftyp" => (D::Structure, Layout::Leaf, ctx, None),
            b"meta" => (D::Structure, Layout::Full, Ctx::Meta, None),
            b"moov" => (
                D::Structure,
                Layout::Plain,
                Ctx::Moov,
                Some("read for the image sequence"),
            ),
            b"mdat" => (D::Structure, Layout::Data, ctx, None),
            b"free" | b"skip" | b"wide" => (D::Padding, Layout::Leaf, ctx, None),
            b"uuid" => (D::Unknown, Layout::Leaf, ctx, None),
            b"pdin" | b"moof" | b"mfra" | b"styp" | b"sidx" | b"ssix" | b"prft" | b"meco"
            | b"emsg" | b"jumb" => (
                D::Skipped,
                Layout::Leaf,
                ctx,
                Some("zenavif-parse skips this box"),
            ),
            _ => (D::Unknown, Layout::Leaf, ctx, None),
        },
        // `read_avif_meta_scoped`.
        Ctx::Meta => match typ {
            b"hdlr" => (
                D::Structure,
                Layout::Leaf,
                ctx,
                Some("handler must be pict"),
            ),
            b"pitm" | b"iloc" => (D::Structure, Layout::Leaf, ctx, None),
            b"iinf" => (D::Structure, Layout::Iinf, Ctx::Iinf, None),
            b"iprp" => (D::Structure, Layout::Plain, Ctx::Iprp, None),
            b"iref" => (D::Structure, Layout::Full, Ctx::Iref, None),
            b"idat" => (D::Structure, Layout::Data, ctx, None),
            b"grpl" => (
                D::Dropped,
                Layout::Plain,
                Ctx::Grpl,
                Some("entity groups are parsed and never used"),
            ),
            b"dinf" => (
                D::Skipped,
                Layout::Plain,
                Ctx::Unread,
                Some("zenavif-parse skips this box"),
            ),
            b"free" | b"skip" => (D::Padding, Layout::Leaf, ctx, None),
            b"uuid" => (D::Unknown, Layout::Leaf, ctx, None),
            b"ipro" | b"fiin" | b"ipmc" | b"xml " | b"bxml" => (
                D::Skipped,
                Layout::Leaf,
                ctx,
                Some("zenavif-parse skips this box"),
            ),
            _ => (D::Unknown, Layout::Leaf, ctx, None),
        },
        // `read_iinf`: anything but infe is an error.
        Ctx::Iinf => (D::Structure, Layout::Leaf, ctx, None),
        // `read_iprp`: anything but ipco/ipma is an error.
        Ctx::Iprp => match typ {
            b"ipco" => (D::Structure, Layout::Plain, Ctx::Ipco, None),
            _ => (D::Structure, Layout::Leaf, ctx, None),
        },
        // Properties and references: decided in the second pass.
        Ctx::Ipco => (D::Unknown, Layout::Leaf, ctx, None),
        Ctx::Iref => (D::Dropped, Layout::Leaf, ctx, None),
        // `read_moov`/`read_trak`/`read_mdia`/`read_minf`/`read_stbl`.
        Ctx::Moov => {
            let layout = generic_layout(typ);
            // `read_moov`, `read_trak`, `read_mdia`, `read_minf`, `read_stbl`;
            // `stss` and other children are skipped.
            let read = matches!(
                typ,
                b"mvhd"
                    | b"trak"
                    | b"tkhd"
                    | b"tref"
                    | b"edts"
                    | b"elst"
                    | b"mdia"
                    | b"mdhd"
                    | b"hdlr"
                    | b"minf"
                    | b"stbl"
                    | b"stts"
                    | b"stsc"
                    | b"stsz"
                    | b"stco"
                    | b"co64"
            );
            match typ {
                b"free" | b"skip" => (D::Padding, Layout::Leaf, ctx, None),
                b"uuid" => (D::Unknown, Layout::Leaf, ctx, None),
                b"stsd" => (D::Structure, Layout::FullCount, Ctx::Stsd, None),
                b"meta" => (
                    D::Structure,
                    Layout::Full,
                    Ctx::TrackMeta,
                    Some("track metadata (EXIF/XMP sidecars)"),
                ),
                _ if read => (D::Structure, layout, ctx, None),
                _ => (D::Skipped, layout, Ctx::Unread, None),
            }
        }
        Ctx::Stsd => match typ {
            b"av01" => (
                D::Structure,
                Layout::VisualSampleEntry,
                Ctx::Av01Entry,
                None,
            ),
            _ => (
                D::Skipped,
                generic_layout(typ),
                Ctx::Unread,
                Some("zenavif-parse reads only av01 sample entries"),
            ),
        },
        Ctx::Av01Entry => match typ {
            b"av1C" | b"colr" | b"irot" | b"imir" | b"clap" | b"pasp" | b"clli" | b"mdcv"
            | b"cclv" | b"amve" => (D::Structure, Layout::Leaf, ctx, None),
            b"free" | b"skip" => (D::Padding, Layout::Leaf, ctx, None),
            b"uuid" => (D::Unknown, Layout::Leaf, ctx, None),
            _ => (D::Skipped, generic_layout(typ), Ctx::Unread, None),
        },
        Ctx::TrackMeta => match typ {
            b"free" | b"skip" => (D::Padding, Layout::Leaf, ctx, None),
            b"uuid" => (D::Unknown, Layout::Leaf, ctx, None),
            b"dinf" => (D::Skipped, Layout::Plain, Ctx::Unread, None),
            _ => (D::Structure, generic_layout(typ), ctx, None),
        },
        Ctx::Grpl => (D::Dropped, Layout::Leaf, ctx, None),
        Ctx::Unread => match typ {
            b"free" | b"skip" => (D::Padding, Layout::Leaf, ctx, None),
            b"uuid" => (D::Unknown, Layout::Leaf, ctx, None),
            _ => (D::Skipped, generic_layout(typ), ctx, None),
        },
    }
}

fn be32_at(c: &[u8], at: usize) -> Option<u64> {
    let b = c.get(at..at.checked_add(4)?)?;
    Some(u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])))
}

/// Bytes of a leaf box's payload that zenavif-parse never reads, as child
/// parts to add when the box is consumed. `content` is the payload after the
/// 8- or 16-byte header. Read lengths mirror zenavif-parse's `read_*`.
fn inner_parts(
    ctx: Ctx,
    typ: &[u8; 4],
    content: Range<u64>,
    c: &[u8],
) -> Vec<(Range<u64>, Disposition, String)> {
    let mut out = Vec::new();
    let len = c.len();
    let push =
        |out: &mut Vec<(Range<u64>, Disposition, String)>, from: usize, to: usize, why: &str| {
            if from < to && to <= len {
                out.push((
                    content.start + from as u64..content.start + to as u64,
                    Disposition::Dropped,
                    why.to_string(),
                ));
            }
        };
    let v1 = c.first() == Some(&1);
    let property = matches!(ctx, Ctx::Ipco | Ctx::Av01Entry);
    let read: Option<(usize, &str)> = match (ctx, typ) {
        (Ctx::Meta | Ctx::Moov | Ctx::TrackMeta, b"hdlr") => Some((
            12,
            "reserved fields and handler name: zenavif-parse reads only the handler type",
        )),
        (Ctx::Iinf | Ctx::TrackMeta, b"infe") => {
            let fixed = if c.first() == Some(&3) { 14 } else { 12 };
            if c.get(8..12) == Some(b"mime") || c.get(10..14) == Some(b"mime") {
                // `read_infe` reads the name and content type of a mime item,
                // and uses only the content type.
                let rest = c.get(fixed..).unwrap_or(&[]);
                let name_end = rest.iter().position(|&b| b == 0).map(|e| fixed + e + 1);
                if let Some(ne) = name_end {
                    push(&mut out, fixed, ne, "item name: zenavif-parse discards it");
                    let ct_end = c[ne..].iter().position(|&b| b == 0).map(|e| ne + e + 1);
                    if let Some(ce) = ct_end {
                        push(
                            &mut out,
                            ce,
                            len,
                            "after the content type (content encoding): not used",
                        );
                    }
                }
                None
            } else {
                Some((
                    fixed,
                    "item name and the rest of the entry: zenavif-parse skips them",
                ))
            }
        }
        (Ctx::Iprp, b"ipma") => ipma_read_len(c).map(|r| (r, "after the last association read")),
        (_, b"ispe") if property => Some((12, "after the width and height read")),
        (_, b"irot" | b"imir") if property => Some((1, "after the one byte read")),
        (_, b"clap") if property => Some((32, "after the 8 clap fields read")),
        (_, b"pasp") if property => Some((8, "after the two spacings read")),
        (_, b"clli") if property => Some((4, "after the two light levels read")),
        (_, b"mdcv") if property => Some((24, "after the 24 bytes read")),
        (_, b"amve") if property => Some((8, "after the 8 bytes read")),
        (_, b"a1op") if property => Some((1, "after the operating point read")),
        (_, b"lsel") if property => Some((2, "after the layer ID read")),
        (_, b"a1lx") if property => Some((
            if c.first().is_some_and(|f| f & 1 != 0) {
                13
            } else {
                7
            },
            "after the layer sizes read",
        )),
        (_, b"cclv") if property => c.first().map(|&f| {
            let mut n = 1;
            if f & 0x20 != 0 {
                n += 24;
            }
            n += 4 * [0x10, 0x08, 0x04].iter().filter(|&&b| f & b != 0).count();
            (n, "after the colour-volume fields read")
        }),
        (_, b"av1C") if property => Some((
            4,
            "configOBUs: zenavif-parse skips them (the sequence header comes from the item data)",
        )),
        (_, b"auxC") if property => {
            // `type_subtype` splits at the NUL: only the URN is used.
            let urn_end = c
                .get(4..)
                .and_then(|r| r.iter().position(|&b| b == 0))
                .map(|e| 4 + e + 1);
            urn_end.map(|e| (e, "auxiliary subtype data: not used"))
        }
        (_, b"colr") if property => match c.get(..4) {
            Some(b"nclx") => Some((11, "after the nclx fields read")),
            Some(b"rICC" | b"prof") => {
                if let Some(declared) = be32_at(c, 4) {
                    let end = 4u64.saturating_add(declared);
                    if end < len as u64 {
                        out.push((
                            content.start + end..content.end,
                            Disposition::Unreferenced,
                            format!(
                                "after the ICC profile's declared {declared} bytes; reported with the profile"
                            ),
                        ));
                    }
                }
                None
            }
            _ => None,
        },
        (Ctx::Moov, b"mvhd" | b"mdhd") => Some((
            if v1 { 32 } else { 20 },
            "after the time scale and duration read",
        )),
        (Ctx::Moov, b"tkhd") => Some((
            if v1 { 36 } else { 24 },
            "after the track ID and duration read (width, height, matrix skipped)",
        )),
        (Ctx::Moov, b"elst") => be32_at(c, 4).map(|n| {
            let per = if v1 { 20 } else { 12 };
            (
                usize::try_from(n.saturating_mul(per).saturating_add(8)).unwrap_or(usize::MAX),
                "after the edit entries read",
            )
        }),
        (Ctx::Moov, b"stts" | b"stsc" | b"stco" | b"co64") => be32_at(c, 4).map(|n| {
            let per: u64 = match typ {
                b"stts" | b"co64" => 8,
                b"stsc" => 12,
                _ => 4,
            };
            (
                usize::try_from(n.saturating_mul(per).saturating_add(8)).unwrap_or(usize::MAX),
                "after the entries read",
            )
        }),
        (Ctx::Moov, b"stsz") => match (be32_at(c, 4), be32_at(c, 8)) {
            (Some(0), Some(n)) => Some((
                usize::try_from(n.saturating_mul(4).saturating_add(12)).unwrap_or(usize::MAX),
                "after the sample sizes read",
            )),
            _ => Some((12, "after the sample size and count read")),
        },
        _ => None,
    };
    if let Some((r, why)) = read
        && r < len
    {
        push(&mut out, r, len, why);
    }
    out
}

/// zenavif-parse `read_ipma`: the bytes its entry loop reads.
fn ipma_read_len(c: &[u8]) -> Option<usize> {
    let version = *c.first()?;
    let wide = c.get(3)? & 1 != 0;
    let count = be32_at(c, 4)?;
    let mut pos = 8usize;
    let id = if version == 0 { 2 } else { 4 };
    for _ in 0..count {
        pos = pos.checked_add(id + 1)?;
        let n = usize::from(*c.get(pos - 1)?);
        pos = pos.checked_add(n * if wide { 2 } else { 1 })?;
        if pos > c.len() {
            return None;
        }
    }
    Some(pos)
}

/// Container layouts used to enumerate subtrees.
fn generic_layout(typ: &[u8; 4]) -> Layout {
    match typ {
        b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"udta" | b"mvex" | b"dinf"
        | b"mfra" | b"moof" | b"traf" | b"tref" | b"grpl" | b"sinf" | b"schi" | b"rinf"
        | b"meco" | b"iprp" | b"ipco" | b"strk" | b"strd" => Layout::Plain,
        b"dref" | b"stsd" => Layout::FullCount,
        b"meta" => Layout::NestedMeta,
        b"iinf" => Layout::Iinf,
        b"iref" => Layout::Full,
        b"av01" | b"hvc1" | b"hev1" | b"avc1" | b"avc3" | b"mp4v" | b"encv" => {
            Layout::VisualSampleEntry
        }
        b"mp4a" | b"Opus" | b"fLaC" | b"ac-3" | b"ec-3" | b"alac" | b"ipcm" | b"fpcm" | b"mha1"
        | b"mhm1" | b"enca" => Layout::AudioSampleEntry,
        _ => Layout::Leaf,
    }
}

// ── What the decode reads ────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Use {
    disp: Disposition,
    why: String,
}

fn rank(d: Disposition) -> u8 {
    match d {
        Disposition::ImageData => 9,
        Disposition::Metadata(_) => 8,
        Disposition::Structure => 7,
        Disposition::Dropped => 4,
        Disposition::Skipped => 3,
        Disposition::Unknown => 2,
        Disposition::Unreferenced => 1,
        _ => 0,
    }
}

fn set<K: Ord>(map: &mut BTreeMap<K, Use>, key: K, disp: Disposition, why: impl Into<String>) {
    let why = why.into();
    match map.get(&key) {
        Some(old) if rank(old.disp) >= rank(disp) => {}
        _ => {
            map.insert(key, Use { disp, why });
        }
    }
}

/// Properties zenavif-parse models (`read_ipco`); everything else is
/// `ItemProperty::Unsupported`.
fn supported_property(p: &PropRec) -> bool {
    match &p.typ {
        b"pixi" | b"auxC" | b"ispe" | b"grid" | b"av1C" | b"irot" | b"imir" | b"clap" | b"pasp"
        | b"clli" | b"mdcv" | b"cclv" | b"amve" | b"a1op" | b"lsel" | b"a1lx" => true,
        b"colr" => matches!(
            p.colour_type.as_ref(),
            Some(b"nclx") | Some(b"rICC") | Some(b"prof")
        ),
        _ => false,
    }
}

/// A property's "kind" for first-wins selection (`find_prop!`,
/// `item_color_properties`): ICC and nclx `colr` are separate kinds.
fn prop_kind(p: &PropRec) -> [u8; 4] {
    match (&p.typ, p.colour_type.as_ref()) {
        (b"colr", Some(b"nclx")) => *b"nclx",
        (b"colr", Some(_)) => *b"ICC ",
        (t, _) => *t,
    }
}

#[derive(Default)]
struct Model {
    items: BTreeMap<u32, Use>,
    /// Keyed by 0-based `ipco` index.
    props: BTreeMap<usize, Use>,
    /// References the decode consults, by `iref` child node.
    refs: BTreeMap<usize, Use>,
    /// Animation samples: `(range, use, label)`.
    samples: Vec<(Range<u64>, Use, &'static str)>,
    /// Track-level metadata items located through the parser.
    sidecars: Vec<(Range<u64>, Use, &'static str)>,
}

/// The byte range `s` occupies inside `base`, when it borrows from it.
fn locate(base: &[u8], s: &[u8]) -> Option<Range<u64>> {
    let b = base.as_ptr() as usize;
    let p = s.as_ptr() as usize;
    if s.is_empty() || p < b {
        return None;
    }
    let off = p - b;
    let end = off.checked_add(s.len())?;
    (end <= base.len()).then_some(off as u64..end as u64)
}

impl Model {
    fn build(w: &Walker<'_>, parser: &zenavif_parse::AvifParser<'_>, opts: &Options) -> Self {
        let mut m = Self::default();
        let infos: BTreeMap<u32, &Infe> = w
            .infes
            .iter()
            .filter_map(|r| r.info.as_ref())
            .map(|i| (i.id, i))
            .collect();
        // `read_iref`: one reference per (type, from, to), in file order.
        let refs: Vec<(usize, [u8; 4], u32, u32, u16)> = w
            .irefs
            .iter()
            .flat_map(|(node, typ, from, to)| {
                to.iter()
                    .enumerate()
                    .map(move |(i, &t)| (*node, *typ, *from, t, i as u16))
            })
            .collect();
        // `read_iprp`: supported properties associated with an item, in ipma
        // order.
        let assoc: Vec<(u32, usize)> = w
            .ipma
            .iter()
            .filter(|(_, _, idx)| *idx > 0)
            .map(|&(item, _, idx)| (item, usize::from(idx) - 1))
            .filter(|&(_, i)| w.props.get(i).is_some_and(supported_property))
            .collect();
        // The first associated property of each kind, per item.
        let first_of = |item: u32, kind: [u8; 4]| -> Option<usize> {
            assoc
                .iter()
                .find(|&&(it, i)| it == item && prop_kind(&w.props[i]) == kind)
                .map(|&(_, i)| i)
        };
        let props_of = |item: u32| -> Vec<usize> {
            assoc
                .iter()
                .filter(|&&(it, _)| it == item)
                .map(|&(_, i)| i)
                .collect()
        };
        let urn_of = |item: u32| -> Vec<Vec<u8>> {
            props_of(item)
                .into_iter()
                .filter_map(|i| w.props[i].urn.clone())
                .collect()
        };

        let Some(&(_, primary)) = w.pitm.as_ref() else {
            // A pure image sequence: frames only.
            m.frames(parser, w.data);
            return m;
        };
        let ptype = infos.get(&primary).map(|i| i.typ);

        // Primary item (`AvifParser::build`, `ManagedAvifDecoder::decode`).
        match ptype.as_ref() {
            Some(b"grid") => {
                // `calculate_grid_config` takes the layout from an `ImageGrid`
                // property or from ispe; `probe_info` reads the descriptor via
                // `primary_data()` and ignores it.
                set(
                    &mut m.items,
                    primary,
                    Disposition::Dropped,
                    "grid descriptor: read by probe_info, but zenavif-parse derives the layout from ispe (calculate_grid_config)",
                );
                let mut tiles: Vec<(u32, u16)> = refs
                    .iter()
                    .filter(|r| r.2 == primary && &r.1 == b"dimg")
                    .map(|r| (r.3, r.4))
                    .collect();
                tiles.sort_by_key(|&(_, idx)| idx);
                for (n, &(t, _)) in tiles.iter().enumerate() {
                    set(
                        &mut m.items,
                        t,
                        Disposition::ImageData,
                        format!("grid tile {n} of the primary image"),
                    );
                    for i in props_of(t) {
                        set(
                            &mut m.props,
                            i,
                            Disposition::Dropped,
                            format!("item {t} (grid tile): not consulted"),
                        );
                    }
                }
                if let Some(&(t0, _)) = tiles.first()
                    && let Some(i) = first_of(t0, *b"ispe")
                {
                    set(
                        &mut m.props,
                        i,
                        Disposition::Structure,
                        format!("item {t0}: first tile's ispe sets the grid layout"),
                    );
                }
            }
            _ => set(
                &mut m.items,
                primary,
                Disposition::ImageData,
                "primary image",
            ),
        }
        m.primary_props(&w.props, primary, &props_of, &first_of, opts);

        // Alpha: the first auxl → primary item with the CICP alpha URN.
        let auxl: Vec<u32> = refs
            .iter()
            .filter(|r| &r.1 == b"auxl" && r.3 == primary && r.2 != primary)
            .map(|r| r.2)
            .collect();
        let alpha = auxl
            .iter()
            .copied()
            .find(|&a| urn_of(a).iter().any(|u| u == ALPHA_URN));
        if let Some(a) = alpha {
            set(
                &mut m.items,
                a,
                Disposition::ImageData,
                "alpha plane of the primary image",
            );
            for i in props_of(a) {
                let (d, why) = if &w.props[i].typ == b"auxC" {
                    (Disposition::Structure, "identifies the alpha plane")
                } else {
                    (Disposition::Dropped, "not consulted for the alpha plane")
                };
                set(&mut m.props, i, d, format!("item {a}: {why}"));
            }
        }
        // Depth: located by zenavif-parse, never reported by zenavif.
        if let Some(d) = auxl.iter().copied().find(|&d| {
            Some(d) != alpha && urn_of(d).iter().any(|u| DEPTH_URNS.contains(&u.as_slice()))
        }) {
            set(
                &mut m.items,
                d,
                Disposition::Skipped,
                "depth image: zenavif-parse locates it, but zenavif never reports a depth map (ImageInfo::depth_map is always None)",
            );
        }
        for (node, typ, from, to, _) in &refs {
            let used = match typ {
                b"dimg" => *from == primary && ptype.as_ref() == Some(b"grid"),
                b"auxl" => *to == primary,
                b"prem" => *from == primary && Some(*to) == alpha,
                b"cdsc" => *to == primary,
                _ => false,
            };
            if used {
                set(
                    &mut m.refs,
                    *node,
                    Disposition::Structure,
                    "consulted by AvifParser::build",
                );
            }
        }

        // EXIF / XMP: the first of each linked to the primary by cdsc.
        let mut exif = None;
        let mut xmp = None;
        for r in refs.iter().filter(|r| &r.1 == b"cdsc" && r.3 == primary) {
            let Some(info) = infos.get(&r.2) else {
                continue;
            };
            if &info.typ == b"Exif" && exif.is_none() {
                exif = Some(r.2);
            } else if info.is_xmp() && xmp.is_none() {
                xmp = Some(r.2);
            }
        }
        if let Some(e) = exif {
            let (d, why) = if opts.keep_exif {
                (
                    Disposition::Metadata(MetadataKind::Exif),
                    "EXIF of the primary image (first Exif item linked by cdsc)",
                )
            } else {
                (Disposition::Dropped, "EXIF removed by the DecodePolicy")
            };
            set(&mut m.items, e, d, why);
        }
        if let Some(x) = xmp {
            let (d, why) = if opts.keep_xmp {
                (
                    Disposition::Metadata(MetadataKind::Xmp),
                    "XMP of the primary image (first XMP item linked by cdsc)",
                )
            } else {
                (Disposition::Dropped, "XMP removed by the DecodePolicy")
            };
            set(&mut m.items, x, d, why);
        }

        // Gain map: the first tmap item whose first input is the primary.
        if let Some(tmap) = w
            .infes
            .iter()
            .filter_map(|r| r.info.as_ref())
            .find(|i| &i.typ == b"tmap")
            .map(|i| i.id)
        {
            let mut inputs: Vec<(u32, u16)> = refs
                .iter()
                .filter(|r| r.2 == tmap && &r.1 == b"dimg")
                .map(|r| (r.3, r.4))
                .collect();
            inputs.sort_by_key(|&(_, idx)| idx);
            if inputs.len() >= 2 && inputs[0].0 == primary && parser.gain_map_metadata().is_some() {
                let gm = inputs[1].0;
                set(
                    &mut m.items,
                    tmap,
                    Disposition::Metadata(MetadataKind::GainMap),
                    "ISO 21496-1 gain-map parameters, reported in ImageInfo::gain_map",
                );
                let gm_disp = match opts.gain_map {
                    GainMapUse::Describe => Disposition::Metadata(MetadataKind::GainMap),
                    GainMapUse::Reconstruct => Disposition::ImageData,
                };
                set(
                    &mut m.items,
                    gm,
                    gm_disp,
                    "gain-map image: its sequence header sizes ImageInfo::gain_map; pixels decoded on request",
                );
                if let Some(i) = first_of(tmap, *b"nclx").or_else(|| first_of(tmap, *b"ICC ")) {
                    set(
                        &mut m.props,
                        i,
                        Disposition::Metadata(MetadataKind::GainMap),
                        format!("item {tmap}: alternate rendition colour"),
                    );
                }
                for i in props_of(tmap).into_iter().chain(props_of(gm)) {
                    set(
                        &mut m.props,
                        i,
                        Disposition::Dropped,
                        "gain-map item property: not consulted",
                    );
                }
                for (node, typ, from, _, _) in &refs {
                    if *from == tmap && typ == b"dimg" {
                        set(
                            &mut m.refs,
                            *node,
                            Disposition::Structure,
                            "tmap inputs: base and gain map",
                        );
                    }
                }
            }
        }

        for r in refs.iter().filter(|r| &r.1 == b"thmb" && r.3 == primary) {
            set(
                &mut m.items,
                r.2,
                Disposition::Skipped,
                "thumbnail of the primary image; zenavif does not read thumbnails",
            );
        }
        m.frames(parser, w.data);
        m
    }

    /// `AvifParser::build` `find_prop!` / `item_color_properties` for the
    /// primary item, and what codec/info.rs reports from each.
    fn primary_props(
        &mut self,
        props: &[PropRec],
        primary: u32,
        props_of: &dyn Fn(u32) -> Vec<usize>,
        first_of: &dyn Fn(u32, [u8; 4]) -> Option<usize>,
        opts: &Options,
    ) {
        use Disposition as D;
        use MetadataKind as K;
        for i in props_of(primary) {
            let p = &props[i];
            let kind = prop_kind(p);
            if first_of(primary, kind) != Some(i) {
                set(
                    &mut self.props,
                    i,
                    D::Dropped,
                    format!(
                        "item {primary}: superseded (zenavif-parse takes the first of each kind)"
                    ),
                );
                continue;
            }
            let (d, why) = match &kind {
                b"ispe" => (D::Structure, "image extents (megapixel limit)"),
                b"av1C" => (D::Structure, "decoder configuration (bit depth)"),
                b"grid" => (D::Structure, "grid layout"),
                b"ICC " if opts.keep_icc => {
                    (D::Metadata(K::Icc), "ICC profile, reported in SourceColor")
                }
                b"ICC " => (D::Dropped, "ICC profile removed by the DecodePolicy"),
                b"nclx" => (
                    D::Metadata(K::Cicp),
                    "nclx colour: reported as CICP and used for YUV conversion",
                ),
                b"irot" | b"imir" => (
                    D::Metadata(K::Orientation),
                    "orientation, reported in ImageInfo",
                ),
                b"clli" | b"mdcv" => (
                    D::Metadata(K::HdrStatic),
                    "HDR static metadata, reported in SourceColor",
                ),
                b"clap" | b"pasp" => (
                    D::Dropped,
                    "kept in the native ImageInfo only; zencodec neither reports nor applies it",
                ),
                b"cclv" | b"amve" | b"a1op" | b"lsel" | b"a1lx" | b"pixi" => {
                    (D::Dropped, "parsed; the decoder never uses it")
                }
                _ => (D::Dropped, "parsed; not consulted for the primary image"),
            };
            set(&mut self.props, i, d, format!("item {primary}: {why}"));
        }
    }

    /// Animation samples, located through the parser (`AvifParser::frame`
    /// borrows the input), and the track-level EXIF/XMP sidecars.
    fn frames(&mut self, parser: &zenavif_parse::AvifParser<'_>, data: &[u8]) {
        let Some(info) = parser.animation_info() else {
            return;
        };
        for i in 0..info.frame_count {
            let Ok(f) = parser.frame(i) else {
                break;
            };
            if let Some(r) = locate(data, &f.data) {
                self.samples.push((
                    r,
                    Use {
                        disp: Disposition::ImageData,
                        why: format!("animation frame {i} (animation_frame_decoder)"),
                    },
                    "color sample",
                ));
            }
            if let Some(a) = &f.alpha_data
                && let Some(r) = locate(data, a)
            {
                self.samples.push((
                    r,
                    Use {
                        disp: Disposition::ImageData,
                        why: format!("alpha of animation frame {i} (animation_frame_decoder)"),
                    },
                    "alpha sample",
                ));
            }
        }
        // `probe_info` reports the track's metadata only for a pure sequence.
        let pure = parser.primary_data().is_ok_and(|d| d.is_empty());
        let mut sidecar = |r: Option<Range<u64>>, kind: MetadataKind, label: &'static str| {
            if let Some(r) = r {
                let u = if pure {
                    Use {
                        disp: Disposition::Metadata(kind),
                        why: "track metadata, reported for the image sequence".to_string(),
                    }
                } else {
                    Use {
                        disp: Disposition::Skipped,
                        why: "track metadata; the primary image's metadata is reported instead"
                            .to_string(),
                    }
                };
                self.sidecars.push((r, u, label));
            }
        };
        if let Some(Ok(e)) = parser.animation_exif() {
            // `strip_exif` drops the offset prefix; the reported bytes start here.
            sidecar(locate(data, &e), MetadataKind::Exif, "track Exif");
        }
        if let Some(Ok(x)) = parser.animation_xmp() {
            sidecar(locate(data, &x), MetadataKind::Xmp, "track XMP");
        }
    }
}

/// An item no decode rule picked.
fn default_use(info: &Infe) -> Use {
    let (disp, why) = match &info.typ {
        b"Exif" => (
            Disposition::Skipped,
            "Exif item not reported (only the first linked to the primary by cdsc is)".to_string(),
        ),
        b"mime" if info.is_xmp() => (
            Disposition::Skipped,
            "XMP item not reported (only the first linked to the primary by cdsc is)".to_string(),
        ),
        b"mime" => (
            Disposition::Skipped,
            "mime item; zenavif reads only XMP".to_string(),
        ),
        b"uri " => (
            Disposition::Skipped,
            "uri item; zenavif does not read it".to_string(),
        ),
        b"av01" | b"grid" | b"iovl" | b"iden" | b"tmap" | b"hvc1" | b"jpeg" | b"unci" => (
            Disposition::Skipped,
            "image item the zencodec decode does not use".to_string(),
        ),
        t => (
            Disposition::Unknown,
            format!("item type {} zenavif does not recognise", fourcc_str(t)),
        ),
    };
    Use { disp, why }
}

struct Cand {
    item: u32,
    /// How remarks name this extent.
    who: String,
    range: Range<u64>,
    idat: bool,
    disp: Disposition,
    why: String,
    label: Option<String>,
    tag: PartTag,
    note_to: usize,
}

impl Walker<'_> {
    /// The top-level box holding `at`, for remarks.
    fn top_box_at(&self, at: u64) -> String {
        self.nodes
            .iter()
            .find(|n| n.parent.is_none() && n.range.start <= at && at < n.range.end)
            .map(|n| format!("{} at {}", n.tag, n.range.start))
            .unwrap_or_else(|| "no box".to_string())
    }

    /// Second pass: dispositions that depend on the whole file.
    fn resolve(
        &mut self,
        parsed: Option<(&zenavif_parse::AvifParser<'_>, &Model)>,
    ) -> Result<(), At<Error>> {
        let rejected = || Use {
            disp: Disposition::Dropped,
            why: "zenavif-parse rejects the file, so nothing is decoded".to_string(),
        };
        let model = parsed.map(|(_, m)| m);
        let item_use = |id: u32, info: Option<&Infe>| -> Use {
            match model {
                None => rejected(),
                Some(m) => match m.items.get(&id) {
                    Some(u) => u.clone(),
                    None => match info {
                        Some(i) => default_use(i),
                        None => Use {
                            disp: Disposition::Unknown,
                            why: "no infe entry declares this item".to_string(),
                        },
                    },
                },
            }
        };

        // Items.
        let infes = std::mem::take(&mut self.infes);
        let mut declared: BTreeMap<u32, (Option<String>, Infe)> = BTreeMap::new();
        for rec in &infes {
            let Some(info) = &rec.info else {
                self.nodes[rec.node].disp = Disposition::Malformed;
                continue;
            };
            let (disp, why) =
                if let std::collections::btree_map::Entry::Vacant(slot) = declared.entry(info.id) {
                    slot.insert((self.nodes[rec.node].label.clone(), info.clone()));
                    let u = item_use(info.id, Some(info));
                    if u.disp.is_consumed() {
                        (Disposition::Structure, u.why)
                    } else {
                        (u.disp, u.why)
                    }
                } else {
                    (
                        Disposition::Dropped,
                        "repeats an earlier item ID".to_string(),
                    )
                };
            self.nodes[rec.node].disp = disp;
            self.note(rec.node, why);
        }

        // References.
        for (node, typ, _, _) in self.irefs.clone() {
            let (disp, why) = match model.and_then(|m| m.refs.get(&node)) {
                Some(u) => (u.disp, u.why.clone()),
                None if model.is_none() => (Disposition::Dropped, rejected().why),
                None => (
                    Disposition::Dropped,
                    format!(
                        "parsed; the decode never consults this {} reference",
                        fourcc_str(&typ)
                    ),
                ),
            };
            self.nodes[node].disp = disp;
            self.note(node, why);
        }

        // Properties.
        let props = std::mem::take(&mut self.props);
        let associated: BTreeSet<usize> = self
            .ipma
            .iter()
            .filter(|(_, _, i)| *i > 0)
            .map(|&(_, _, i)| usize::from(i) - 1)
            .collect();
        for (index, rec) in props.iter().enumerate() {
            self.note(rec.node, format!("property #{}", index + 1));
            let (disp, why) = if model.is_none() {
                (Disposition::Dropped, rejected().why)
            } else if !supported_property(rec) {
                if &rec.typ == b"colr" {
                    (
                        Disposition::Unknown,
                        "colour type zenavif-parse does not recognise".to_string(),
                    )
                } else {
                    (
                        Disposition::Unknown,
                        "zenavif-parse has no parser for this property".to_string(),
                    )
                }
            } else if let Some(u) = model.and_then(|m| m.props.get(&index)) {
                (u.disp, u.why.clone())
            } else if associated.contains(&index) {
                (
                    Disposition::Skipped,
                    "associated only with items the zencodec decode does not use".to_string(),
                )
            } else {
                (
                    Disposition::Unreferenced,
                    "no ipma entry references this property".to_string(),
                )
            };
            self.nodes[rec.node].disp = disp;
            self.note(rec.node, why);
        }
        self.props = props;

        let idat_body = self
            .idats
            .iter()
            .find(|(_, top)| *top)
            .and_then(|&(n, _)| self.nodes[n].body.clone());

        // Item extents (zenavif-parse `read_iloc`, `resolve_item`).
        let file_len = self.data.len() as u64;
        let mdat_bodies: Vec<Range<u64>> = self
            .mdats
            .iter()
            .filter_map(|&m| self.nodes[m].body.clone())
            .collect();
        let mut cands: Vec<Cand> = Vec::new();
        let mut located: BTreeSet<u32> = BTreeSet::new();
        for (note_to, entries, skipped) in std::mem::take(&mut self.ilocs) {
            let locs = match entries {
                Ok(v) => v,
                Err(e) => {
                    self.note(note_to, format!("unreadable: {e}"));
                    continue;
                }
            };
            let n_ext: usize = locs.iter().map(|l| l.extents.len()).sum();
            self.note(
                note_to,
                format!("{} item locations, {n_ext} extents", locs.len()),
            );
            if skipped > 0 {
                self.note(note_to, format!("{skipped} further extents not listed"));
            }
            for loc in locs {
                self.check_stop()?;
                let id = loc.id;
                let info = declared.get(&id);
                let u = if located.insert(id) {
                    item_use(id, info.map(|(_, i)| i))
                } else {
                    Use {
                        disp: Disposition::Dropped,
                        why: "repeats an earlier iloc entry for this item".to_string(),
                    }
                };
                let label = info.and_then(|(l, _)| l.clone());
                if loc.data_ref != 0 {
                    self.note(
                        note_to,
                        format!("item {id}: data reference {} (external file) is not supported by zenavif-parse", loc.data_ref),
                    );
                    continue;
                }
                let src = match loc.method {
                    0 => 0..file_len,
                    1 => match &idat_body {
                        Some(b) => b.clone(),
                        None => {
                            self.note(
                                note_to,
                                format!("item {id}: construction method 1 but no idat"),
                            );
                            continue;
                        }
                    },
                    m => {
                        self.note(
                            note_to,
                            format!("item {id}: construction method {m} is not supported by zenavif-parse; extents not placed"),
                        );
                        continue;
                    }
                };
                let n = loc.extents.len();
                for (k, &(off, len)) in loc.extents.iter().enumerate() {
                    let k = k + 1;
                    let Some(start) = src
                        .start
                        .checked_add(loc.base)
                        .and_then(|v| v.checked_add(off))
                    else {
                        self.note(
                            note_to,
                            format!("item {id} extent {k}/{n}: offset overflows"),
                        );
                        continue;
                    };
                    // Length 0 reads to the end of the source: the mdat that
                    // holds the start, else the file (`resolve_extents_from_raw`).
                    let end = if len == 0 {
                        if loc.method == 1 {
                            src.end
                        } else {
                            mdat_bodies
                                .iter()
                                .find(|b| b.start <= start && start < b.end)
                                .map_or(file_len, |b| b.end)
                        }
                    } else {
                        match start.checked_add(len) {
                            Some(e) => e,
                            None => {
                                self.note(
                                    note_to,
                                    format!("item {id} extent {k}/{n}: length overflows"),
                                );
                                continue;
                            }
                        }
                    };
                    if end > src.end || start >= end {
                        let what = if loc.method == 1 { "idat" } else { "file" };
                        self.note(
                            note_to,
                            format!("item {id} extent {k}/{n} at {start}..{end} runs past the end of the {what} ({})", src.end),
                        );
                        continue;
                    }
                    let mut why = u.why.clone();
                    if len == 0 {
                        why.push_str("; length 0: runs to the end of its source");
                    }
                    cands.push(Cand {
                        item: id,
                        who: format!("item {id} extent {k}/{n}"),
                        range: start..end,
                        idat: loc.method == 1,
                        disp: u.disp,
                        why,
                        label: label.clone(),
                        tag: PartTag::Code(id),
                        note_to,
                    });
                }
            }
        }
        // Animation samples and track sidecars, located through the parser.
        if let Some(m) = model {
            let note_to = self.moovs.first().copied().unwrap_or(0);
            for (r, u, label) in m.samples.iter().chain(m.sidecars.iter()) {
                cands.push(Cand {
                    item: 0,
                    who: u.why.clone(),
                    range: r.clone(),
                    idat: false,
                    disp: u.disp,
                    why: u.why.clone(),
                    label: Some((*label).to_string()),
                    tag: PartTag::None,
                    note_to,
                });
            }
        }

        // Place each extent in the mdat/idat body that holds it.
        let mut holders: Vec<(usize, bool)> = self.mdats.iter().map(|&m| (m, false)).collect();
        holders.extend(
            self.idats
                .iter()
                .filter(|(_, top)| *top)
                .map(|&(n, _)| (n, true)),
        );
        holders.retain(|&(h, _)| self.nodes[h].body.is_some());
        let mut placed: BTreeMap<usize, Vec<Cand>> = BTreeMap::new();
        for cand in cands {
            let holder = holders.iter().copied().find(|&(h, is_idat)| {
                let b = self.nodes[h].body.clone().unwrap_or(0..0);
                (is_idat == cand.idat || !cand.idat)
                    && b.start <= cand.range.start
                    && cand.range.end <= b.end
            });
            match holder {
                Some((h, _)) => placed.entry(h).or_default().push(cand),
                None => {
                    let place = self.top_box_at(cand.range.start);
                    self.note(
                        cand.note_to,
                        format!(
                            "{} at {}..{} lies outside every mdat/idat ({place})",
                            cand.who, cand.range.start, cand.range.end
                        ),
                    );
                }
            }
        }
        for (holder, mut list) in placed {
            list.sort_by(|a, b| {
                a.range
                    .start
                    .cmp(&b.range.start)
                    .then(rank(b.disp).cmp(&rank(a.disp)))
            });
            let mut last: Option<(u64, String)> = None;
            for cand in list {
                if let Some((end, other)) = &last
                    && cand.range.start < *end
                {
                    self.note(
                        cand.note_to,
                        format!(
                            "{} at {}..{} shares bytes with {other}; not listed as a part",
                            cand.who, cand.range.start, cand.range.end
                        ),
                    );
                    continue;
                }
                last = Some((cand.range.end, cand.who.clone()));
                let mut node = Self::leaf(
                    Some(holder),
                    PartKind::Extent,
                    cand.tag,
                    cand.range,
                    cand.disp,
                );
                node.label = cand.label;
                if cand.item != 0 && cand.who.ends_with("/1") {
                    node.notes.push(cand.why);
                } else if cand.item != 0 {
                    node.notes.push(format!("{}; {}", cand.who, cand.why));
                } else {
                    node.notes.push(cand.why);
                }
                if cand.disp == Disposition::ImageData {
                    node.notes.push(
                        "coded-unit framing inside is not walked yet: bytes after the last OBU are not distinguished"
                            .to_string(),
                    );
                }
                self.add(node)?;
            }
        }

        self.split_metadata_extents(model.is_some());
        self.add_inner_parts()?;

        if !self.moovs.is_empty() {
            for m in self.mdats.clone() {
                if self.nodes[m].body.is_some() {
                    self.nodes[m].gap_fill = Some(Disposition::Skipped);
                    self.note(
                        m,
                        "the file has a moov: bytes outside the listed extents may be samples of tracks zenavif does not decode",
                    );
                }
            }
        }
        Ok(())
    }

    /// Child parts for bytes inside consumed leaves that the decoder never
    /// reads.
    fn add_inner_parts(&mut self) -> Result<(), At<Error>> {
        let mut has_child = vec![false; self.nodes.len()];
        for n in &self.nodes {
            if let Some(p) = n.parent {
                has_child[p] = true;
            }
        }
        for (i, &has_children) in has_child.iter().enumerate() {
            if self.nodes[i].inner.is_empty() || !self.nodes[i].disp.is_consumed() || has_children {
                continue;
            }
            let inner = std::mem::take(&mut self.nodes[i].inner);
            for (r, d, why) in inner {
                let mut n = Self::leaf(Some(i), PartKind::Gap, PartTag::None, r, d);
                n.notes.push(why);
                self.add(n)?;
            }
        }
        Ok(())
    }

    /// Split single-extent metadata items at their internal ends: the EXIF
    /// offset field and the bytes it skips (`strip_exif`), the XMP packet
    /// trailer, the ISO 21496-1 payload length.
    fn split_metadata_extents(&mut self, parsed: bool) {
        if !parsed {
            return;
        }
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            if n.kind != PartKind::Extent || n.tag == PartTag::None {
                continue;
            }
            let r = n.range.clone();
            let d = slice(self.data, r.clone());
            let label = n.notes.first().cloned().unwrap_or_default();
            let mut parts: Vec<(Range<u64>, Disposition, String)> = Vec::new();
            match n.disp {
                Disposition::Metadata(MetadataKind::Exif) if !label.starts_with("item") => {
                    let Some(off) = be32_at(d, 0) else { continue };
                    let tiff = 4u64.saturating_add(off);
                    if tiff >= d.len() as u64 {
                        continue;
                    }
                    parts.push((
                        r.start..r.start + 4,
                        Disposition::Structure,
                        "exif_tiff_header_offset".to_string(),
                    ));
                    if off > 0 {
                        parts.push((
                            r.start + 4..r.start + tiff,
                            Disposition::Dropped,
                            "bytes the TIFF-header offset skips".to_string(),
                        ));
                    }
                    parts.push((
                        r.start + tiff..r.end,
                        Disposition::Metadata(MetadataKind::Exif),
                        "TIFF data, reported in ImageInfo".to_string(),
                    ));
                    self.nodes[i].body = Some(r.clone());
                }
                Disposition::Metadata(MetadataKind::Xmp) if !label.starts_with("item") => {
                    if let Some(end) = xmp_packet_end(d)
                        && end < d.len()
                    {
                        parts.push((
                            r.start + end as u64..r.end,
                            Disposition::Unreferenced,
                            "after the XMP packet trailer; reported with the packet".to_string(),
                        ));
                    }
                }
                Disposition::Metadata(MetadataKind::GainMap) if !label.starts_with("item") => {
                    // The tmap payload starts with its version byte (0).
                    if label.starts_with("ISO 21496-1")
                        && let Some(end) = iso21496_avif_len(d)
                        && end < d.len()
                    {
                        parts.push((
                            r.start + end as u64..r.end,
                            Disposition::Dropped,
                            "after the ISO 21496-1 gain-map metadata".to_string(),
                        ));
                    }
                }
                _ => {}
            }
            self.nodes[i].inner.extend(parts);
        }
    }

    fn emit(self) -> Result<Inventory, At<Error>> {
        fn cap(_: InventoryError) -> At<Error> {
            at!(Error::ResourceLimit(
                "inventory exceeds the zencodec part cap".to_string()
            ))
        }
        let mut inv = Inventory::new(ImageFormat::Avif, self.data.len() as u64);
        let mut ids: Vec<PartId> = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let mut part = Part::new(n.kind, n.tag.clone(), n.range.clone(), n.disp);
            if let Some(l) = &n.label {
                part = part.with_label(l.clone());
            }
            if let Some(b) = &n.body {
                part = part.with_body(b.clone());
            }
            if !n.notes.is_empty() || n.dropped_notes > 0 {
                let mut d = n.notes.join("; ");
                if n.dropped_notes > 0 {
                    d.push_str(&format!("; {} more remarks", n.dropped_notes));
                }
                part = part.with_detail(d);
            }
            let parent = n.parent.map(|p| ids[p]);
            ids.push(inv.push(parent, part).map_err(cap)?);
        }
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(d) = n.gap_fill
                && n.body.is_some()
            {
                inv.fill_gaps(Some(ids[i]), d).map_err(cap)?;
            }
        }
        inv.fill_gaps(None, Disposition::Trailing).map_err(cap)?;
        Ok(inv)
    }
}

/// End of an XMP packet: just past `?>` of the `<?xpacket end=…?>` trailer.
fn xmp_packet_end(d: &[u8]) -> Option<usize> {
    let marker = b"<?xpacket end=";
    let at = d.windows(marker.len()).rposition(|w| w == marker)?;
    let close = d[at..].windows(2).position(|w| w == b"?>")?;
    Some(at + close + 2)
}

/// Length of an ISO 21496-1 payload in the AVIF `tmap` form (version byte
/// first): version(1) minimum_version(2) writer_version(2) flags(1), then
/// the headrooms and per-channel fractions.
fn iso21496_avif_len(d: &[u8]) -> Option<usize> {
    let flags = *d.get(5)?;
    let channels = if flags & 0x80 != 0 { 3 } else { 1 };
    let body = if flags & 0x08 != 0 {
        4 + 8 + 20 * channels
    } else {
        16 + 40 * channels
    };
    Some(6 + body)
}

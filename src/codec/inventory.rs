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
    /// The gain-map item is read for its sequence and first frame headers
    /// (`convert_gain_map_info`), which size `ImageInfo::gain_map`.
    Describe,
    /// `extract_gain_map` (the raw item in a `GainMapSource`) or
    /// `GainMapRender::Components` (decoded into a `DecodedGainMap`).
    Surface,
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
    /// The container parser configuration `probe`/`probe_full` use
    /// (`probe_inner`: the codec configuration, without the job's limits).
    pub(crate) parse_config: zenavif_parse::DecodeConfig,
    /// The decoders' configuration when the job's `ResourceLimits` make it
    /// stricter (`effective_config`): a file it rejects still has its
    /// metadata probed, but nothing decodes.
    pub(crate) decode_parse_config: Option<zenavif_parse::DecodeConfig>,
    /// `ResourceLimits::max_pixels`, which becomes the decoder's
    /// `frame_size_limit` (`ManagedAvifDecoder::new_for_source`).
    pub(crate) max_pixels: Option<u64>,
    /// `DecodePolicy::allow_animation`: `animation_frame_decoder` refuses
    /// when false.
    pub(crate) allow_animation: bool,
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
    w.decide_structure(opts);

    let parsed = zenavif_parse::AvifParser::from_bytes_with_config(data, &opts.parse_config, stop);
    match parsed {
        Ok(parser) => {
            let mut model = Model::build(&w, &parser, opts)?;
            if let Some(dc) = &opts.decode_parse_config {
                match zenavif_parse::AvifParser::from_bytes_with_config(data, dc, stop) {
                    Ok(_) => {}
                    Err(e) => {
                        if let zenavif_parse::Error::Stopped(reason) = e.error() {
                            return Err(at!(Error::Cancelled(*reason)));
                        }
                        model.refuse_decode(&format!(
                            "the decoders reject the file under the job's ResourceLimits ({}); probe and probe_full do not apply them",
                            e.error()
                        ));
                    }
                }
            }
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
    /// Box header length (8, 16, or 24/32 with a `uuid` usertype); 0 for
    /// parts that are not boxes.
    header: u64,
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
    /// A track-level `meta` (`read_avif_meta_scoped`, no primary required):
    /// read for the track's EXIF/XMP sidecars.
    TrackMeta,
    /// `infe` entries of a track-level `meta`.
    TrackIinf,
    /// Children of a track's `tref` (`read_tref`).
    Tref,
    /// A subtree zenavif-parse reads and then discards.
    Dropped,
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
    /// The box runs past its container: `clamped` ends at the container's
    /// end, as zenavif-parse's `BoxIter::next_box` reads it.
    Overrun {
        clamped: Hdr,
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
    let declared = size;
    let size = size.min(avail);
    let mut header = base_header;
    let mut usertype = None;
    if &typ == b"uuid" && size >= base_header + 16 {
        let u = slice(data, pos + base_header..pos + base_header + 16);
        if let Ok(u) = <[u8; 16]>::try_from(u) {
            usertype = Some(u);
            header += 16;
        }
    }
    let h = Hdr {
        typ,
        base_header,
        header,
        end: pos + size,
        usertype,
    };
    if declared > avail {
        HdrOutcome::Overrun {
            clamped: h,
            declared,
        }
    } else {
        HdrOutcome::Ok(h)
    }
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

/// `(entries, extents not listed)`. `budget` is the number of extents still
/// to list across every `iloc` read; extents past it are counted, and
/// skipped arithmetically rather than read one by one (zero-width fields
/// make an extent cost no input bytes). The outer `Err` is cancellation.
#[allow(clippy::type_complexity)]
fn read_iloc(
    c: &[u8],
    budget: &mut usize,
    stop: &dyn enough::Stop,
) -> Result<Result<(Vec<Loc>, usize), String>, enough::StopReason> {
    let version = match c.first() {
        Some(&v) if v <= 2 => v,
        Some(&v) => return Ok(Err(format!("iloc version {v}"))),
        None => return Ok(Err("empty iloc".to_string())),
    };
    let size = |n: u8| -> Result<u8, String> {
        match n {
            0 | 4 | 8 => Ok(n),
            _ => Err(format!("iloc field size {n}")),
        }
    };
    let (Some(&b4), Some(&b5)) = (c.get(4), c.get(5)) else {
        return Ok(Err("iloc too short".to_string()));
    };
    let sizes = (|| -> Result<(u8, u8, u8, u8), String> {
        Ok((
            size(b4 >> 4)?,
            size(b4 & 15)?,
            size(b5 >> 4)?,
            if version >= 1 { size(b5 & 15)? } else { 0 },
        ))
    })();
    let (off_sz, len_sz, base_sz, idx_sz) = match sizes {
        Ok(v) => v,
        Err(e) => return Ok(Err(e)),
    };
    let per_extent = usize::from(idx_sz) + usize::from(off_sz) + usize::from(len_sz);
    let mut r = Bits { c, p: 6 };
    let short = || "iloc truncated".to_string();
    let Some(count) = r.n(if version == 2 { 4 } else { 2 }) else {
        return Ok(Err(short()));
    };
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for k in 0..count {
        if k % 4096 == 0 {
            stop.check()?;
        }
        let entry = (|| -> Option<(u32, u8, u16, u64, usize)> {
            let id = r.n(if version == 2 { 4 } else { 2 })? as u32;
            let method = if version >= 1 {
                (r.n(2)? & 15) as u8
            } else {
                0
            };
            let data_ref = r.n(2)? as u16;
            let base = r.n(base_sz)?;
            let n = r.n(2)? as usize;
            Some((id, method, data_ref, base, n))
        })();
        let Some((id, method, data_ref, base, n)) = entry else {
            return Ok(Err(short()));
        };
        // Entries past the item cap keep nothing: their extents are counted.
        let take = if out.len() < MAX_ILOC_EXTENTS {
            n.min(*budget)
        } else {
            0
        };
        let mut extents = Vec::with_capacity(take.min(64));
        for _ in 0..take {
            let (Some(_), Some(o), Some(l)) = (r.n(idx_sz), r.n(off_sz), r.n(len_sz)) else {
                return Ok(Err(short()));
            };
            extents.push((o, l));
        }
        *budget -= take;
        // The rest of this entry's extents: counted, not read.
        let rest = n - take;
        let Some(end) = rest
            .checked_mul(per_extent)
            .and_then(|b| r.p.checked_add(b))
            .filter(|&e| e <= c.len())
        else {
            return Ok(Err(short()));
        };
        r.p = end;
        skipped += rest;
        if out.len() < MAX_ILOC_EXTENTS {
            out.push(Loc {
                id,
                method,
                data_ref,
                base,
                extents,
            });
        }
    }
    Ok(Ok((out, skipped)))
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
    /// The `ipco` holding it.
    ipco: usize,
    /// Payload length (after the box header).
    len: usize,
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
    /// Extents left to read across every `iloc` box.
    iloc_budget: usize,
    /// Some `iloc` extents were not read: the budget ran out.
    iloc_budget_exhausted: bool,
    props: Vec<PropRec>,
    /// `(node, type, from, [to])`.
    irefs: Vec<(usize, [u8; 4], u32, Vec<u32>)>,
    /// `iprp` boxes of the top-level `meta`, in order.
    iprps: Vec<usize>,
    /// `(ipco node, its iprp)`.
    ipcos: Vec<(usize, usize)>,
    /// `(ipma node, its iprp, entries)`.
    ipmas: Vec<IpmaRec>,
    /// Every top-level `ftyp`.
    ftyps: Vec<usize>,
    traks: Vec<TrakRec>,
    /// `trak` node → index into `traks`.
    trak_index: BTreeMap<usize, usize>,
    track_metas: Vec<TrackMetaRec>,
    /// Track `meta` node → index into `track_metas`.
    track_meta_index: BTreeMap<usize, usize>,
    /// The tracks `associate_tracks` keeps (indices into `traks`).
    color_trak: Option<usize>,
    alpha_trak: Option<usize>,
}

/// `(ipma node, its iprp, entries)`.
type IpmaRec = (usize, usize, Option<Vec<(u32, bool, u16)>>);

/// A `trak`, with what `read_trak` and `associate_tracks` look at.
#[derive(Default)]
struct TrakRec {
    node: usize,
    moov: Option<usize>,
    /// `mdia`/`hdlr` handler type (the last one read).
    handler: Option<[u8; 4]>,
    /// `tkhd` track ID (the last one read; 0 when absent).
    track_id: u32,
    /// Track IDs in `tref`/`auxl`.
    auxl: Vec<u32>,
    /// A `minf` holds an `stbl`: `read_trak` returns the track.
    has_stbl: bool,
    stsds: Vec<usize>,
}

/// A track-level `meta`, with what `read_trak` takes from it.
struct TrackMetaRec {
    trak: Option<usize>,
    iloc: Option<Result<Vec<Loc>, String>>,
    /// `(node, entry)` in order.
    infes: Vec<(usize, Infe)>,
    idat: Option<usize>,
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
            iloc_budget: MAX_ILOC_EXTENTS,
            iloc_budget_exhausted: false,
            props: Vec::new(),
            irefs: Vec::new(),
            iprps: Vec::new(),
            ipcos: Vec::new(),
            ipmas: Vec::new(),
            ftyps: Vec::new(),
            traks: Vec::new(),
            trak_index: BTreeMap::new(),
            track_metas: Vec::new(),
            track_meta_index: BTreeMap::new(),
            color_trak: None,
            alpha_trak: None,
        }
    }

    /// The nearest ancestor of `from` (inclusive) that is a box of type `typ`.
    fn ancestor(&self, from: Option<usize>, typ: &[u8; 4]) -> Option<usize> {
        let mut at = from;
        while let Some(i) = at {
            if self.nodes[i].tag == PartTag::FourCc(*typ) && self.nodes[i].kind == PartKind::Box {
                return Some(i);
            }
            at = self.nodes[i].parent;
        }
        None
    }

    /// zenavif-parse `read_avif_meta_scoped` / `read_iprp`: a later `iprp`
    /// replaces an earlier one, and inside it a later `ipco` or `ipma`
    /// replaces an earlier one. Returns the active `iprp`, the `props`
    /// indices of the active `ipco` (its property order), and the active
    /// `ipma` entries.
    #[allow(clippy::type_complexity)]
    fn active_properties(&self) -> (Option<usize>, Vec<usize>, &[(u32, bool, u16)]) {
        self.active_properties_of(&self.props)
    }

    #[allow(clippy::type_complexity)]
    fn active_properties_of(
        &self,
        all: &[PropRec],
    ) -> (Option<usize>, Vec<usize>, &[(u32, bool, u16)]) {
        let Some(&iprp) = self.iprps.last() else {
            return (None, Vec::new(), &[]);
        };
        let ipco = self
            .ipcos
            .iter()
            .rev()
            .find(|&&(_, p)| p == iprp)
            .map(|&(n, _)| n);
        let props = match ipco {
            Some(c) => (0..all.len()).filter(|&i| all[i].ipco == c).collect(),
            None => Vec::new(),
        };
        let ipma = self
            .ipmas
            .iter()
            .rev()
            .find(|(_, p, _)| *p == iprp)
            .and_then(|(_, _, e)| e.as_deref())
            .unwrap_or(&[]);
        (Some(iprp), props, ipma)
    }

    fn trak_of(&self, from: Option<usize>) -> Option<usize> {
        let t = self.ancestor(from, b"trak")?;
        self.trak_index.get(&t).copied()
    }

    fn track_meta_mut(&mut self, node: Option<usize>) -> Option<&mut TrackMetaRec> {
        let i = *self.track_meta_index.get(&node?)?;
        self.track_metas.get_mut(i)
    }

    /// Children of every node, in order.
    fn children_index(&self) -> Vec<Vec<usize>> {
        let mut kids = vec![Vec::new(); self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(p) = n.parent {
                kids[p].push(i);
            }
        }
        kids
    }

    /// `root` and every node below it.
    fn subtree(kids: &[Vec<usize>], root: usize) -> Vec<usize> {
        let mut out = vec![root];
        let mut k = 0;
        while k < out.len() {
            out.extend_from_slice(&kids[out[k]]);
            k += 1;
        }
        out
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
            header: 0,
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
                HdrOutcome::Overrun { clamped, declared } => {
                    let room = if ctx == Ctx::Top {
                        "the file"
                    } else {
                        "its container"
                    };
                    let what = format!("declares {declared} bytes, {room} has {} left", end - pos);
                    if overrun_is_read(ctx, &clamped.typ) {
                        // `BoxIter::next_box` clamps the box to what is left
                        // and the reader parses it truncated.
                        let id = self.visit(parent, pos, &clamped, ctx, depth)?;
                        self.note(
                            id,
                            format!(
                                "{what}: zenavif-parse reads it clamped to that (BoxIter::next_box)"
                            ),
                        );
                    } else {
                        self.malformed(
                            parent,
                            PartTag::FourCc(clamped.typ),
                            pos..end,
                            format!(
                                "{what}: zenavif-parse skips this box with skip_box_content, which rejects the file (box content size mismatch)"
                            ),
                        )?;
                    }
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
    ) -> Result<usize, At<Error>> {
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
            (Ctx::Iinf | Ctx::TrackIinf, b"infe") => {
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
        node.header = h.header;
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
                self.ftyps.push(id);
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
                // zenavif-parse rejects a second iloc (and a second meta), so
                // only the first iloc of the first meta can be used. One
                // extent budget covers every iloc this walk reads.
                let first = self.metas.len() == 1 && self.ilocs.is_empty();
                let (entries, skipped) = if first {
                    match read_iloc(c, &mut self.iloc_budget, self.stop) {
                        Ok(Ok((e, s))) => (Ok(e), s),
                        Ok(Err(e)) => (Err(e), 0),
                        Err(r) => return Err(at!(Error::Cancelled(r))),
                    }
                } else {
                    (
                        Err("not read: zenavif-parse rejects a second iloc or meta".to_string()),
                        0,
                    )
                };
                self.ilocs.push((id, entries, skipped));
            }
            (Ctx::Meta, b"idat") => self.idats.push((id, true)),
            (_, b"idat") => {
                self.idats.push((id, false));
                if ctx == Ctx::TrackMeta
                    && let Some(m) = self.track_meta_mut(parent)
                    && m.idat.is_none()
                {
                    m.idat = Some(id);
                }
            }
            (Ctx::Meta, b"iref") => {}
            (Ctx::Iref, typ) => {
                // The version byte follows the iref box header (8 or 16
                // bytes).
                let version = parent
                    .and_then(|p| self.nodes.get(p))
                    .and_then(|p| {
                        slice(self.data, p.range.clone())
                            .get(p.header as usize)
                            .copied()
                    })
                    .unwrap_or(0);
                if let Some((from, to)) = read_iref_entry(c, version) {
                    self.irefs.push((id, *typ, from, to));
                } else {
                    self.note(id, "unreadable reference entry");
                }
            }
            (Ctx::Meta, b"iprp") => self.iprps.push(id),
            (Ctx::Iprp, b"ipco") => {
                if let Some(p) = parent {
                    self.ipcos.push((id, p));
                }
            }
            (Ctx::Iprp, b"ipma") => {
                if let Some(p) = parent {
                    self.ipmas.push((id, p, read_ipma(c)));
                }
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
                    ipco: parent.unwrap_or(0),
                    len: c.len(),
                    typ: *typ,
                    colour_type,
                    urn,
                });
            }
            // `read_moov` / `read_trak` / `read_mdia` / `read_minf`: what
            // `associate_tracks` decides with.
            (Ctx::Moov, b"trak") => {
                let moov = self.ancestor(parent, b"moov");
                self.trak_index.insert(id, self.traks.len());
                self.traks.push(TrakRec {
                    node: id,
                    moov,
                    ..TrakRec::default()
                });
            }
            (Ctx::Moov, b"tkhd") => {
                let tid = match c.first() {
                    Some(1) => be32_at(c, 20),
                    Some(_) => be32_at(c, 12),
                    None => None,
                };
                if let (Some(t), Some(tid)) = (self.trak_of(parent), tid) {
                    self.traks[t].track_id = tid as u32;
                }
            }
            (Ctx::Moov, b"hdlr") => {
                let in_mdia =
                    parent.is_some_and(|p| self.nodes[p].tag == PartTag::FourCc(*b"mdia"));
                if in_mdia
                    && let Some(t) = self.trak_of(parent)
                    && let Some(ht) = c.get(8..12).and_then(|b| <[u8; 4]>::try_from(b).ok())
                {
                    self.traks[t].handler = Some(ht);
                }
            }
            (Ctx::Moov, b"stbl") => {
                let in_minf =
                    parent.is_some_and(|p| self.nodes[p].tag == PartTag::FourCc(*b"minf"));
                if in_minf && let Some(t) = self.trak_of(parent) {
                    self.traks[t].has_stbl = true;
                }
            }
            (Ctx::Moov, b"stsd") => {
                if let Some(t) = self.trak_of(parent) {
                    self.traks[t].stsds.push(id);
                }
            }
            (Ctx::Tref, b"auxl") => {
                // `read_tref`: whole 4-byte track IDs only.
                if c.len() >= 4
                    && c.len().is_multiple_of(4)
                    && let Some(t) = self.trak_of(parent)
                {
                    let ids: Vec<u32> = c
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| u32::from_be_bytes(*b))
                        .collect();
                    self.traks[t].auxl.extend(ids);
                }
            }
            (Ctx::Moov, b"meta") => {
                let trak = self.trak_of(parent);
                self.track_meta_index.insert(id, self.track_metas.len());
                self.track_metas.push(TrackMetaRec {
                    trak,
                    iloc: None,
                    infes: Vec::new(),
                    idat: None,
                });
            }
            (Ctx::TrackMeta, b"iloc") => {
                let r = match read_iloc(c, &mut self.iloc_budget, self.stop) {
                    Ok(Ok((e, skipped))) => {
                        self.iloc_budget_exhausted |= skipped > 0;
                        Ok(e)
                    }
                    Ok(Err(e)) => Err(e),
                    Err(r) => return Err(at!(Error::Cancelled(r))),
                };
                if let Some(m) = self.track_meta_mut(parent)
                    && m.iloc.is_none()
                {
                    m.iloc = Some(r);
                }
            }
            _ => {}
        }
        if let Some(info) = infe {
            if ctx == Ctx::TrackIinf {
                let meta = parent.and_then(|p| self.nodes[p].parent);
                if let Some(info) = info
                    && let Some(m) = self.track_meta_mut(meta)
                {
                    m.infes.push((id, info));
                }
            } else {
                self.infes.push(InfeRec { node: id, info });
            }
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
        Ok(id)
    }
}

impl Walker<'_> {
    /// Structural decisions the walk can make before parsing: which
    /// `ftyp`, `moov`, tracks and sample entries zenavif-parse uses.
    fn decide_structure(&mut self, opts: &Options) {
        use Disposition as D;
        // `parse_raw` reads only the first box as `ftyp`; later ones go to
        // `skip_box_content`.
        for &f in self.ftyps.iter().skip(1) {
            self.nodes[f].disp = D::Skipped;
            self.nodes[f].notes.push(
                "not the first box: zenavif-parse reads only the first ftyp and skips this one"
                    .to_string(),
            );
        }
        // `read_moov` returns the tracks whose `minf` holds an `stbl`;
        // `parse_raw` keeps the last moov that returns any.
        let parsed = |w: &Self, m: usize| {
            w.traks
                .iter()
                .filter(|t| t.moov == Some(m) && t.has_stbl)
                .count()
        };
        let used = self
            .moovs
            .iter()
            .copied()
            .rev()
            .find(|&m| parsed(self, m) > 0);
        let mut roots: Vec<(usize, &'static str)> = Vec::new();
        for &m in &self.moovs {
            if Some(m) == used {
                continue;
            }
            roots.push((
                m,
                if parsed(self, m) > 0 {
                    "a later moov with tracks replaces this one (parse_raw keeps the last)"
                } else {
                    "no track with a sample table: read_moov parses it and keeps nothing"
                },
            ));
        }
        if let Some(m) = used {
            // `associate_tracks`.
            let tracks: Vec<usize> = (0..self.traks.len())
                .filter(|&t| self.traks[t].moov == Some(m) && self.traks[t].has_stbl)
                .collect();
            let color = tracks
                .iter()
                .copied()
                .find(|&t| self.traks[t].handler.as_ref() == Some(b"pict"))
                .or_else(|| {
                    tracks
                        .iter()
                        .copied()
                        .find(|&t| self.traks[t].handler.as_ref() != Some(b"soun"))
                });
            let alpha = color.and_then(|c| {
                let cid = self.traks[c].track_id;
                tracks.iter().copied().find(|&t| {
                    t != c
                        && matches!(
                            self.traks[t].handler.as_ref(),
                            Some(b"auxv") | Some(b"pict")
                        )
                        && self.traks[t].auxl.contains(&cid)
                })
            });
            self.color_trak = color;
            self.alpha_trak = alpha;
            for t in 0..self.traks.len() {
                if self.traks[t].moov != Some(m) || Some(t) == color || Some(t) == alpha {
                    continue;
                }
                roots.push((
                    self.traks[t].node,
                    if self.traks[t].has_stbl {
                        "associate_tracks keeps only the colour and alpha tracks"
                    } else {
                        "no sample table: read_trak returns nothing for it"
                    },
                ));
            }
            let kids = self.children_index();
            for (t, alpha) in [(color, false), (alpha, true)] {
                if let Some(t) = t {
                    for stsd in self.traks[t].stsds.clone() {
                        self.decide_stsd(&kids, stsd, alpha, opts);
                    }
                }
            }
            // zenavif#62: read_minf returns at the first stbl.
            for t in [color, alpha].into_iter().flatten() {
                let tnode = self.traks[t].node;
                let minfs: Vec<usize> = (0..self.nodes.len())
                    .filter(|&i| {
                        self.nodes[i].tag == PartTag::FourCc(*b"minf")
                            && self.ancestor(Some(i), b"trak") == Some(tnode)
                    })
                    .collect();
                for minf in minfs {
                    let mk = &kids[minf];
                    if let Some(k) = mk
                        .iter()
                        .position(|&i| self.nodes[i].tag == PartTag::FourCc(*b"stbl"))
                    {
                        for &i in &mk[k + 1..] {
                            self.mark_desynced(&kids, i, "read_minf returns at the first stbl");
                        }
                    }
                }
            }
            // The alpha track's `tref`/`auxl` names the colour track.
            if let (Some(a), Some(c)) = (alpha, color) {
                let anode = self.traks[a].node;
                let cid = self.traks[c].track_id;
                for i in 0..self.nodes.len() {
                    if self.nodes[i].tag == PartTag::FourCc(*b"auxl")
                        && self.ancestor(Some(i), b"trak") == Some(anode)
                    {
                        self.nodes[i].disp = D::Structure;
                        self.nodes[i].notes.push(format!(
                            "names colour track {cid}: associate_tracks pairs the alpha track with it"
                        ));
                    }
                }
            }
        }
        // Dropped subtrees: everything below a dropped root that the walk
        // called consumed.
        let mut dropped = vec![false; self.nodes.len()];
        for (r, why) in roots {
            dropped[r] = true;
            self.nodes[r].disp = D::Dropped;
            self.nodes[r].notes.push(why.to_string());
        }
        for i in 0..self.nodes.len() {
            if let Some(p) = self.nodes[i].parent
                && dropped[p]
            {
                dropped[i] = true;
                if self.nodes[i].disp.is_consumed() {
                    self.nodes[i].disp = D::Dropped;
                }
            }
        }
    }

    /// `read_stsd`: entries in order, up to `entry_count`, until an `av01`
    /// entry with an `av1C` has been read; most fields are last-wins across
    /// the entries read, `colr` first-of-kind (`retain_color_property`).
    fn decide_stsd(&mut self, kids: &[Vec<usize>], stsd: usize, alpha: bool, opts: &Options) {
        use Disposition as D;
        use MetadataKind as K;
        let r = self.nodes[stsd].range.clone();
        let content = r.start + self.nodes[stsd].header;
        let count = be_u32(self.data, content + 4).unwrap_or(0) as usize;
        let entries: Vec<usize> = kids[stsd]
            .iter()
            .copied()
            .filter(|&i| self.nodes[i].kind == PartKind::Box)
            .collect();
        let mut done = false;
        let mut read: Vec<usize> = Vec::new();
        for (k, &e) in entries.iter().enumerate() {
            let is_av01 = self.nodes[e].tag == PartTag::FourCc(*b"av01");
            let why = if done {
                Some("read_stsd stops after the first av01 entry with an av1C")
            } else if k >= count {
                Some("beyond the stsd entry_count")
            } else {
                None
            };
            if let Some(why) = why {
                self.skip_subtree(kids, e, why);
                continue;
            }
            if !is_av01 {
                continue;
            }
            let ek = &kids[e];
            if ek
                .iter()
                .any(|&c| self.nodes[c].tag == PartTag::FourCc(*b"av1C"))
            {
                done = true;
            }
            read.extend_from_slice(ek);
        }
        // zenavif#62: read_stsd breaks without consuming the rest of the
        // stsd, so the stbl iterator reads its next box header from those
        // bytes.
        let stopped_at = read.last().and_then(|&c| self.nodes[c].parent);
        let unread = match stopped_at {
            Some(e) => self.nodes[e].range.end < self.nodes[stsd].range.end,
            None => false,
        };
        if unread && let Some(stbl) = self.nodes[stsd].parent {
            let end = self.nodes[stsd].range.end;
            let later: Vec<usize> = kids[stbl]
                .iter()
                .copied()
                .filter(|&i| self.nodes[i].range.start >= end)
                .collect();
            for i in later {
                self.mark_desynced(kids, i, "read_stsd stops before the end of the stsd");
            }
        }
        // Across the entries read: last-wins kinds, first-wins colours.
        let kind_of = |w: &Self, c: usize| -> Option<[u8; 4]> {
            match &w.nodes[c].tag {
                PartTag::FourCc([b'c', b'o', b'l', b'r']) => {
                    let cs = w.nodes[c].range.start + w.nodes[c].header;
                    match fourcc_at(w.data, cs) {
                        Some(ct) if &ct == b"nclx" && w.nodes[c].range.end - cs >= 11 => {
                            Some(*b"nclx")
                        }
                        Some(ct) if &ct == b"prof" || &ct == b"rICC" => Some(*b"ICC "),
                        _ => None,
                    }
                }
                PartTag::FourCc(t) => Some(*t),
                _ => None,
            }
        };
        let mut chosen: BTreeMap<[u8; 4], usize> = BTreeMap::new();
        for &c in &read {
            let Some(k) = kind_of(self, c) else { continue };
            match &k {
                b"nclx" | b"ICC " => {
                    chosen.entry(k).or_insert(c);
                }
                _ => {
                    chosen.insert(k, c);
                }
            }
        }
        for &c in &read {
            let PartTag::FourCc(t) = self.nodes[c].tag.clone() else {
                continue;
            };
            let k = kind_of(self, c);
            let is_chosen = k.is_some_and(|k| chosen.get(&k) == Some(&c));
            let (d, why): (Disposition, &str) = match (&t, k.as_ref()) {
                (b"colr", None) => (
                    D::Unknown,
                    "colour type zenavif-parse does not read, or an nclx shorter than 11 bytes",
                ),
                (
                    b"av1C" | b"colr" | b"irot" | b"imir" | b"clap" | b"pasp" | b"clli" | b"mdcv"
                    | b"cclv" | b"amve",
                    _,
                ) if !is_chosen => (
                    D::Dropped,
                    "replaced by a later one of the same kind (or, for colr, an earlier one wins)",
                ),
                (b"av1C", _) => (D::Structure, "decoder configuration of the track"),
                _ if alpha => (
                    D::Dropped,
                    "alpha track: only its AV1 configuration is used",
                ),
                (b"colr", Some(b"nclx")) => (
                    D::Metadata(K::Cicp),
                    "track nclx colour: probe_animation_info reports it and the frames convert with it",
                ),
                (b"colr", _) if opts.keep_icc => (
                    D::Metadata(K::Icc),
                    "track ICC profile: probe_animation_info reports it",
                ),
                (b"colr", _) => (D::Dropped, "ICC profile removed by the DecodePolicy"),
                (b"irot" | b"imir", _) => (
                    D::Metadata(K::Orientation),
                    "track orientation: probe_animation_info reports it",
                ),
                (b"clap", _) => (
                    D::Structure,
                    "clean aperture: animation frames are cropped to it (AnimationCrop)",
                ),
                (b"clli" | b"mdcv", _) => (
                    D::Metadata(K::HdrStatic),
                    "track HDR static metadata: probe_animation_info reports it",
                ),
                (b"pasp", _) => (
                    D::Dropped,
                    "parsed; zencodec neither reports nor applies pixel aspect",
                ),
                (b"cclv" | b"amve", _) => {
                    (D::Dropped, "parsed; zencodec's ImageInfo cannot carry it")
                }
                _ => continue,
            };
            self.nodes[c].disp = d;
            self.nodes[c].notes.push(why.to_string());
        }
    }

    /// zenavif#62: a box zenavif-parse reads out of sync, after a reader
    /// returned without consuming its box.
    fn mark_desynced(&mut self, kids: &[Vec<usize>], root: usize, cause: &str) {
        for i in Self::subtree(kids, root) {
            if self.nodes[i].disp.is_consumed() {
                self.nodes[i].disp = Disposition::Dropped;
            }
        }
        self.nodes[root].notes.push(format!(
            "{cause}, so zenavif-parse reads the following bytes out of sync (zenavif#62); what this box holds makes no change in what the caller receives"
        ));
    }

    /// Mark a sample entry and its consumed descendants `Skipped`.
    fn skip_subtree(&mut self, kids: &[Vec<usize>], root: usize, why: &str) {
        for i in Self::subtree(kids, root) {
            if self.nodes[i].disp.is_consumed() {
                self.nodes[i].disp = Disposition::Skipped;
            }
        }
        self.nodes[root].disp = Disposition::Skipped;
        self.nodes[root].notes.push(why.to_string());
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
                b"tref" => (D::Structure, Layout::Plain, Ctx::Tref, None),
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
        // `read_avif_meta_scoped(.., require_primary = false)`; `read_trak`
        // keeps only the first Exif and first XMP item, its iloc and idat.
        Ctx::TrackMeta => match typ {
            b"hdlr" => (
                D::Structure,
                Layout::Leaf,
                ctx,
                Some("handler must be pict"),
            ),
            b"iloc" => (D::Structure, Layout::Leaf, ctx, None),
            b"iinf" => (D::Structure, Layout::Iinf, Ctx::TrackIinf, None),
            b"idat" => (D::Structure, Layout::Data, ctx, None),
            b"pitm" | b"iref" | b"iprp" | b"grpl" => (
                D::Dropped,
                generic_layout(typ),
                Ctx::Dropped,
                Some(
                    "track metadata: parsed, then discarded (read_trak keeps only the Exif/XMP items)",
                ),
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
        // `read_iinf`: decided in the second pass.
        Ctx::TrackIinf => (D::Skipped, Layout::Leaf, ctx, None),
        // `read_tref`: each child is a list of track IDs.
        Ctx::Tref => (D::Dropped, Layout::Leaf, ctx, None),
        Ctx::Dropped => match typ {
            b"free" | b"skip" => (D::Padding, Layout::Leaf, ctx, None),
            b"uuid" => (D::Unknown, Layout::Leaf, ctx, None),
            _ => (D::Dropped, generic_layout(typ), ctx, None),
        },
        Ctx::Grpl => (D::Dropped, Layout::Leaf, ctx, None),
        Ctx::Unread => match typ {
            b"free" | b"skip" => (D::Padding, Layout::Leaf, ctx, None),
            b"uuid" => (D::Unknown, Layout::Leaf, ctx, None),
            _ => (D::Skipped, generic_layout(typ), ctx, None),
        },
    }
}

/// Whether zenavif-parse reads a box of this type here with a parser that
/// accepts it clamped to its container. At the top level and in `meta`,
/// boxes it does not parse go to `skip_box_content`, which rejects a size
/// that runs past the container; deeper down, unread boxes go to
/// `skip_box_remain`, which skips whatever is left.
fn overrun_is_read(ctx: Ctx, typ: &[u8; 4]) -> bool {
    match ctx {
        Ctx::Top => matches!(typ, b"ftyp" | b"meta" | b"moov"),
        Ctx::Meta | Ctx::TrackMeta => matches!(
            typ,
            b"hdlr" | b"pitm" | b"iloc" | b"iinf" | b"iref" | b"iprp" | b"idat" | b"grpl"
        ),
        _ => true,
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
        (Ctx::Iinf | Ctx::TrackIinf, b"infe") => {
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
        // `read_grid`: version/flags, flags byte, rows, columns, then two
        // 16- or 32-bit sizes.
        (Ctx::Ipco, b"grid") => c.get(4).map(|&f| {
            (
                if f & 1 == 0 { 11 } else { 15 },
                "after the grid fields read",
            )
        }),
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
                        // zenavif-parse copies the whole payload, so the
                        // caller receives these bytes with the profile.
                        out.push((
                            content.start + end..content.end,
                            Disposition::Metadata(MetadataKind::Icc),
                            format!(
                                "after the ICC profile's declared {declared} bytes; the caller receives them with the profile"
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
        // `read_colr` fails on an nclx shorter than its 11 bytes, and
        // `read_ipco` turns that into `Unsupported`.
        b"colr" => match p.colour_type.as_ref() {
            Some(b"nclx") => p.len >= 11,
            Some(b"rICC") | Some(b"prof") => true,
            _ => false,
        },
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
    /// The colour track's EXIF/XMP items.
    sidecars: Vec<Sidecar>,
    /// Track-level `infe` nodes `read_trak` uses.
    track_items: Vec<(usize, Use)>,
    /// More samples than an inventory may hold.
    too_many: bool,
    /// `parser.frame(0)` fails, so `probe_animation_info` fails: nothing
    /// the colour track reports reaches the caller.
    track_probe_fails: bool,
}

/// A track-level metadata item: its bytes, how they are used, and the
/// track `idat` holding them (`None`: the file).
struct Sidecar {
    range: Range<u64>,
    u: Use,
    label: &'static str,
    item: u32,
    idat: Option<usize>,
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
    fn build(
        w: &Walker<'_>,
        parser: &zenavif_parse::AvifParser<'_>,
        opts: &Options,
    ) -> Result<Self, At<Error>> {
        let mut m = Self::default();
        // `item_infos.iter().find(..)`: the first infe of an ID counts.
        let mut infos: BTreeMap<u32, &Infe> = BTreeMap::new();
        for i in w.infes.iter().filter_map(|r| r.info.as_ref()) {
            infos.entry(i.id).or_insert(i);
        }
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
        // order; indices into the active ipco.
        let (_, active, ipma) = w.active_properties();
        let mut assoc: Vec<(u32, usize)> = Vec::new();
        for (k, &(item, _, idx)) in ipma.iter().enumerate() {
            if k % 4096 == 0 {
                w.check_stop()?;
            }
            if idx == 0 {
                continue;
            }
            if let Some(&i) = active.get(usize::from(idx) - 1)
                && supported_property(&w.props[i])
            {
                assoc.push((item, i));
            }
        }
        // Per item: its properties in order, and the first of each kind.
        let mut by_item: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
        let mut first: BTreeMap<(u32, [u8; 4]), usize> = BTreeMap::new();
        for &(item, i) in &assoc {
            by_item.entry(item).or_default().push(i);
            first.entry((item, prop_kind(&w.props[i]))).or_insert(i);
        }
        let first_of =
            |item: u32, kind: [u8; 4]| -> Option<usize> { first.get(&(item, kind)).copied() };
        let props_of =
            |item: u32| -> Vec<usize> { by_item.get(&item).cloned().unwrap_or_default() };
        let urn_of = |item: u32| -> Vec<Vec<u8>> {
            props_of(item)
                .into_iter()
                .filter_map(|i| w.props[i].urn.clone())
                .collect()
        };
        // The first colr of either kind, in association order
        // (`gain_map_color_info`: `find_map` over the tmap's properties).
        let first_colr = |item: u32| -> Option<usize> {
            props_of(item)
                .into_iter()
                .find(|&i| &w.props[i].typ == b"colr")
        };

        let Some(&(_, primary)) = w.pitm.as_ref() else {
            // A pure image sequence: frames only.
            m.frames(w, parser, opts)?;
            m.apply_frame_size_limit(parser, opts);
            return Ok(m);
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
        let is_grid = ptype.as_ref() == Some(b"grid");
        m.primary_props(&w.props, primary, is_grid, &props_of, &first_of, opts);

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
            let (d, why) = if matches!(parser.exif(), Some(Err(_))) {
                // `strip_exif` fails: probe_info swallows it (no EXIF is
                // reported) and decode_full propagates it.
                (
                    Disposition::Malformed,
                    "EXIF item whose offset field points past its end: probe reports no EXIF, and Decode::decode fails (strip_exif: EXIF offset exceeds item size; zenavif#63)",
                )
            } else if opts.keep_exif {
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
                let (gm_disp, gm_why) = match opts.gain_map {
                    GainMapUse::Describe => (
                        Disposition::Metadata(MetadataKind::GainMap),
                        "gain-map image: only its sequence and first frame headers are read (convert_gain_map_info sizes ImageInfo::gain_map)",
                    ),
                    GainMapUse::Surface => (
                        Disposition::Metadata(MetadataKind::GainMap),
                        "gain-map image: handed to the caller (extract_gain_map: GainMapSource; GainMapRender::Components: decoded to DecodedGainMap)",
                    ),
                    GainMapUse::Reconstruct => (
                        Disposition::ImageData,
                        "gain-map image: decoded and applied to the base (GainMapRender::ReconstructHdr)",
                    ),
                };
                set(&mut m.items, gm, gm_disp, gm_why);
                if let Some(i) = first_colr(tmap) {
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
        m.frames(w, parser, opts)?;
        m.apply_frame_size_limit(parser, opts);
        Ok(m)
    }

    /// The decoders refuse the file: nothing becomes pixels, while what the
    /// probe reports stays metadata.
    fn refuse_decode(&mut self, why: &str) {
        let uses = self
            .items
            .values_mut()
            .chain(self.samples.iter_mut().map(|(_, u, _)| u));
        for u in uses {
            if u.disp == Disposition::ImageData {
                u.disp = Disposition::Dropped;
                u.why = format!("{}; {why}", u.why);
            }
        }
    }

    /// `ManagedAvifDecoder::new_for_source`: with `ResourceLimits::max_pixels`
    /// set, the decode refuses an image whose size exceeds it (from the grid,
    /// the primary's sequence header, or the first frame for animation).
    fn apply_frame_size_limit(&mut self, parser: &zenavif_parse::AvifParser<'_>, opts: &Options) {
        let Some(limit) = opts.max_pixels else {
            return;
        };
        let limit = limit.min(u64::from(u32::MAX));
        let too_big = |w: u32, h: u32| u64::from(w.saturating_mul(h)) > limit;
        let (w, h) = if let Some(g) = parser.grid_config() {
            (g.output_width, g.output_height)
        } else if let Ok(meta) = parser.primary_metadata() {
            (meta.max_frame_width.get(), meta.max_frame_height.get())
        } else {
            (0, 0)
        };
        if too_big(w, h) {
            for u in self.items.values_mut() {
                if u.disp == Disposition::ImageData {
                    u.disp = Disposition::Dropped;
                    u.why = format!(
                        "{}; Decode::decode refuses: {w}x{h} exceeds ResourceLimits::max_pixels ({limit})",
                        u.why
                    );
                }
            }
        }
        let anim = parser.frame(0).ok().and_then(|f| {
            zenavif_parse::AV1Metadata::parse_av1_bitstream(&f.data)
                .ok()
                .map(|m| (m.max_frame_width.get(), m.max_frame_height.get()))
        });
        if let Some((aw, ah)) = anim
            && too_big(aw, ah)
        {
            for (_, u, _) in &mut self.samples {
                if u.disp == Disposition::ImageData {
                    u.disp = Disposition::Dropped;
                    u.why = format!(
                        "{}; animation_frame_decoder refuses: {aw}x{ah} exceeds ResourceLimits::max_pixels ({limit})",
                        u.why
                    );
                }
            }
        }
    }

    /// `AvifParser::build` `find_prop!` / `item_color_properties` for the
    /// primary item, and what codec/info.rs reports from each.
    fn primary_props(
        &mut self,
        props: &[PropRec],
        primary: u32,
        is_grid: bool,
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
                b"grid" if is_grid => (D::Structure, "grid layout (calculate_grid_config)"),
                b"grid" => (
                    D::Dropped,
                    "parsed; calculate_grid_config runs only for a grid primary",
                ),
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
    /// borrows the input), and the colour track's EXIF/XMP sidecars.
    fn frames(
        &mut self,
        w: &Walker<'_>,
        parser: &zenavif_parse::AvifParser<'_>,
        opts: &Options,
    ) -> Result<(), At<Error>> {
        let Some(info) = parser.animation_info() else {
            return Ok(());
        };
        let data = w.data;
        // `animation_frame_decoder_inner` refuses when the policy forbids
        // animation; `Decode::decode` never reads samples.
        let (disp, refused) = if opts.allow_animation {
            (Disposition::ImageData, "")
        } else {
            (
                Disposition::Dropped,
                "; DecodePolicy forbids animation, so animation_frame_decoder refuses",
            )
        };
        // A frame the parser cannot locate fails `animation_frame_decoder`
        // (and, for frame 0, `probe_animation_info`).
        let first_bad = (0..info.frame_count).find(|&i| parser.frame(i).is_err());
        // probe_animation_info reads frame 0 whatever the frame count.
        self.track_probe_fails = parser.frame(0).is_err();
        let (disp, refused) = match first_bad {
            Some(i) => (
                Disposition::Dropped,
                format!("; frame {i} cannot be located, so animation_frame_decoder fails"),
            ),
            None => (disp, refused.to_string()),
        };
        for i in 0..info.frame_count {
            if i % 1024 == 0 {
                w.check_stop()?;
            }
            // Every sample becomes a part: stop at the part cap; `resolve`
            // turns the overflow into the cap error.
            if self.samples.len() > DEFAULT_MAX_PARTS as usize {
                self.too_many = true;
                return Ok(());
            }
            let Ok(f) = parser.frame(i) else {
                break;
            };
            if let Some(r) = locate(data, &f.data) {
                self.samples.push((
                    r,
                    Use {
                        disp,
                        why: format!("animation frame {i} (animation_frame_decoder){refused}"),
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
                        disp,
                        why: format!(
                            "alpha of animation frame {i} (animation_frame_decoder){refused}"
                        ),
                    },
                    "alpha sample",
                ));
            }
        }
        self.track_sidecars(w, parser, opts);
        Ok(())
    }

    /// `read_trak`: the first Exif and the first XMP item of the colour
    /// track's `meta`, read through that meta's `iloc` and `idat`.
    /// `probe_animation_info` reports them whenever a sequence exists
    /// (probe, probe_full and animation_frame_decoder), while
    /// `Decode::decode` reports the primary item's.
    fn track_sidecars(
        &mut self,
        w: &Walker<'_>,
        parser: &zenavif_parse::AvifParser<'_>,
        opts: &Options,
    ) {
        let Some(tm) = w
            .track_metas
            .iter()
            .find(|m| m.trak.is_some() && m.trak == w.color_trak)
        else {
            return;
        };
        let probe_fails = self.track_probe_fails;
        let locs: &[Loc] = match &tm.iloc {
            Some(Ok(l)) => l,
            _ => &[],
        };
        let data = w.data;
        let idat_body = tm.idat.and_then(|n| w.nodes[n].body.clone());
        let entry_point = "probe, probe_full and animation_frame_decoder report it (probe_animation_info); Decode::decode reports the primary item's instead (zenavif#64)";
        for kind in [MetadataKind::Exif, MetadataKind::Xmp] {
            let pick = tm.infes.iter().find(|(_, i)| match kind {
                MetadataKind::Exif => &i.typ == b"Exif",
                _ => i.is_xmp(),
            });
            let Some((infe_node, info)) = pick else {
                continue;
            };
            let (label, keep, parsed) = match kind {
                MetadataKind::Exif => ("track Exif", opts.keep_exif, parser.animation_exif()),
                _ => ("track XMP", opts.keep_xmp, parser.animation_xmp()),
            };
            let what = if kind == MetadataKind::Exif {
                "EXIF"
            } else {
                "XMP"
            };
            let u = match &parsed {
                _ if probe_fails => Use {
                    disp: Disposition::Dropped,
                    why: format!(
                        "colour track {what}: probe_animation_info fails (frame 0 cannot be located), so no entry point reports it"
                    ),
                },
                Some(Err(_)) => Use {
                    disp: Disposition::Malformed,
                    why: format!(
                        "colour track {what} that zenavif-parse cannot read (EXIF offset past the item's end): probe_animation_info fails, so probe, probe_full and animation_frame_decoder error (zenavif#63)"
                    ),
                },
                _ if keep => Use {
                    disp: Disposition::Metadata(kind),
                    why: format!("colour track {what}: {entry_point}"),
                },
                _ => Use {
                    disp: Disposition::Dropped,
                    why: format!("colour track {what} removed by the DecodePolicy"),
                },
            };
            self.track_items.push((
                *infe_node,
                Use {
                    disp: if u.disp.is_consumed() {
                        Disposition::Structure
                    } else {
                        u.disp
                    },
                    why: format!("the colour track's {what} item (read_trak)"),
                },
            ));
            // `AvifParser::get_item_extents` + `resolve_item_with_idat`: a
            // single extent, in the file or in this meta's idat.
            let Some(loc) = locs.iter().find(|l| l.id == info.id) else {
                continue;
            };
            let [(off, len)] = loc.extents[..] else {
                continue;
            };
            if loc.data_ref != 0 {
                continue;
            }
            let (src, idat) = match (loc.method, &idat_body) {
                (0, _) => (0..data.len() as u64, None),
                (1, Some(b)) => (b.clone(), tm.idat),
                _ => continue,
            };
            let Some(start) = src
                .start
                .checked_add(loc.base)
                .and_then(|v| v.checked_add(off))
            else {
                continue;
            };
            // Length 0 reads to the end of its source: the idat, or the mdat
            // that holds the start, else the file (`resolve_extents_from_raw`).
            let end = if len == 0 {
                match idat {
                    Some(_) => Some(src.end),
                    None => Some(
                        w.mdats
                            .iter()
                            .filter_map(|&m| w.nodes[m].body.clone())
                            .find(|b| b.start <= start && start < b.end)
                            .map_or(src.end, |b| b.end),
                    ),
                }
            } else {
                start.checked_add(len)
            };
            let Some(end) = end.filter(|&e| e <= src.end && start < e) else {
                continue;
            };
            // The bytes the parser hands over must be these (after the EXIF
            // offset prefix).
            let raw = slice(data, start..end);
            let same = match (&parsed, kind) {
                (Some(Ok(b)), MetadataKind::Exif) => be32_at(raw, 0)
                    .and_then(|o| usize::try_from(o).ok()?.checked_add(4))
                    .and_then(|t| raw.get(t..))
                    .is_some_and(|tiff| tiff == &b[..]),
                (Some(Ok(b)), _) => raw == &b[..],
                (Some(Err(_)), _) => true,
                (None, _) => false,
            };
            if same {
                self.sidecars.push(Sidecar {
                    range: start..end,
                    u,
                    label,
                    item: info.id,
                    idat,
                });
            }
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
    /// The `idat` that must hold it (a track meta's sidecar).
    holder: Option<usize>,
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

        // Metadata a box reports through a path that fails: everything when
        // zenavif-parse rejects the file; the colour track's when
        // `probe_animation_info` fails.
        let track_fails = model.is_some_and(|m| m.track_probe_fails);
        let color_node = self.color_trak.map(|t| self.traks[t].node);
        for i in 0..self.nodes.len() {
            if !matches!(
                self.nodes[i].disp,
                Disposition::Metadata(_) | Disposition::ImageData
            ) {
                continue;
            }
            if model.is_none() {
                self.nodes[i].disp = Disposition::Dropped;
            } else if track_fails
                && color_node.is_some()
                && self.ancestor(Some(i), b"trak") == color_node
            {
                self.nodes[i].disp = Disposition::Dropped;
                self.nodes[i].notes.push(
                    "probe_animation_info fails (frame 0 cannot be located), so nothing the colour track reports reaches the caller"
                        .to_string(),
                );
            }
        }

        // Track-level items `read_trak` uses; the rest of a used track
        // meta's items are not read.
        for (node, u) in model.map(|m| m.track_items.clone()).unwrap_or_default() {
            self.nodes[node].disp = u.disp;
            self.note(node, u.why);
        }
        for i in 0..self.track_metas.len() {
            for k in 0..self.track_metas[i].infes.len() {
                let node = self.track_metas[i].infes[k].0;
                if self.nodes[node]
                    .notes
                    .iter()
                    .all(|n| !n.contains("read_trak"))
                {
                    self.note(
                        node,
                        "track metadata item read_trak does not use (it takes the first Exif and XMP items of the colour track)",
                    );
                }
            }
        }

        // Properties: only the last ipco of the last iprp counts
        // (`read_iprp`), and only the last ipma.
        let props = std::mem::take(&mut self.props);
        let (active_iprp, active, ipma) = {
            let (a, b, c) = self.active_properties_of(&props);
            (a, b, c.to_vec())
        };
        let associated: BTreeSet<usize> = ipma
            .iter()
            .filter(|(_, _, i)| *i > 0)
            .filter_map(|&(_, _, i)| active.get(usize::from(i) - 1).copied())
            .collect();
        for &n in &self.iprps.clone() {
            if Some(n) != active_iprp {
                self.nodes[n].disp = Disposition::Dropped;
                self.note(
                    n,
                    "a later iprp replaces this one (read_avif_meta_scoped keeps the last)",
                );
            }
        }
        let active_ipco = self
            .ipcos
            .iter()
            .rev()
            .find(|&&(_, p)| Some(p) == active_iprp)
            .map(|&(n, _)| n);
        for (n, p) in self.ipcos.clone() {
            if Some(p) == active_iprp && Some(n) != active_ipco {
                self.nodes[n].disp = Disposition::Dropped;
                self.note(
                    n,
                    "a later ipco replaces this one (read_iprp keeps the last)",
                );
            }
        }
        let active_ipma = self
            .ipmas
            .iter()
            .rev()
            .find(|(_, p, _)| Some(*p) == active_iprp)
            .map(|(n, _, _)| *n);
        for (n, p, _) in self.ipmas.clone() {
            if Some(p) == active_iprp && Some(n) != active_ipma {
                self.nodes[n].disp = Disposition::Dropped;
                self.note(
                    n,
                    "a later ipma replaces this one (read_iprp keeps the last)",
                );
            }
        }
        for (gi, rec) in props.iter().enumerate() {
            let pos = props[..gi].iter().filter(|p| p.ipco == rec.ipco).count();
            self.note(rec.node, format!("property #{}", pos + 1));
            let in_active = active.contains(&gi);
            let (disp, why) = if model.is_none() {
                (Disposition::Dropped, rejected().why)
            } else if !in_active {
                (
                    Disposition::Dropped,
                    "in an ipco (or iprp) that a later one replaces".to_string(),
                )
            } else if !supported_property(rec) {
                if &rec.typ == b"colr" {
                    (
                        Disposition::Unknown,
                        "colour type zenavif-parse does not recognise, or an nclx shorter than 11 bytes (read_colr fails; the property is Unsupported)".to_string(),
                    )
                } else {
                    (
                        Disposition::Unknown,
                        "zenavif-parse has no parser for this property".to_string(),
                    )
                }
            } else if let Some(u) = model.and_then(|m| m.props.get(&gi)) {
                (u.disp, u.why.clone())
            } else if associated.contains(&gi) {
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
        if model.is_some_and(|m| m.too_many) {
            return Err(at!(Error::ResourceLimit(
                "inventory exceeds the zencodec part cap".to_string()
            )));
        }
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
                self.iloc_budget_exhausted = true;
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
                    if cands.len() >= self.max_nodes {
                        return Err(at!(Error::ResourceLimit(
                            "inventory exceeds the zencodec part cap".to_string()
                        )));
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
                        holder: None,
                    });
                }
            }
        }
        // Animation samples, located through the parser, and the colour
        // track's metadata items.
        if let Some(m) = model {
            let note_to = self.moovs.first().copied().unwrap_or(0);
            for (r, u, label) in &m.samples {
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
                    holder: None,
                });
            }
            for sc in &m.sidecars {
                cands.push(Cand {
                    item: sc.item,
                    who: format!("{} (item {})", sc.label, sc.item),
                    range: sc.range.clone(),
                    idat: sc.idat.is_some(),
                    disp: sc.u.disp,
                    why: sc.u.why.clone(),
                    label: Some(sc.label.to_string()),
                    tag: PartTag::Code(sc.item),
                    note_to,
                    holder: sc.idat,
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
        let mut top_level: Vec<Cand> = Vec::new();
        // Leaf parts an extent may nest in (no body, no children to come),
        // and the top-level parts, both sorted by start; leaves never
        // overlap.
        let mut leaves: Vec<(u64, u64, usize)> = {
            let mut has_child = vec![false; self.nodes.len()];
            for n in &self.nodes {
                if let Some(p) = n.parent {
                    has_child[p] = true;
                }
            }
            (0..self.nodes.len())
                .filter(|&i| {
                    let n = &self.nodes[i];
                    !has_child[i]
                        && n.body.is_none()
                        && n.inner.is_empty()
                        && n.kind != PartKind::Extent
                })
                .map(|i| (self.nodes[i].range.start, self.nodes[i].range.end, i))
                .collect()
        };
        leaves.sort_unstable();
        let mut tops: Vec<(u64, u64)> = self
            .nodes
            .iter()
            .filter(|n| n.parent.is_none())
            .map(|n| (n.range.start, n.range.end))
            .collect();
        tops.sort_unstable();
        for cand in cands {
            let within = |h: usize| {
                let b = self.nodes[h].body.clone().unwrap_or(0..0);
                b.start <= cand.range.start && cand.range.end <= b.end
            };
            let holder = match cand.holder {
                Some(h) => within(h).then_some(h),
                None => holders
                    .iter()
                    .copied()
                    .find(|&(h, is_idat)| (is_idat == cand.idat || !cand.idat) && within(h))
                    .map(|(h, _)| h),
            };
            if let Some(h) = holder {
                placed.entry(h).or_default().push(cand);
                continue;
            }
            // zenavif-parse slices the extent wherever it points
            // (`resolve_file_extents`). Inside a leaf box, the extent
            // becomes the leaf's child; past the last box, a top-level part.
            if !cand.idat {
                let at = leaves.partition_point(|&(s, _, _)| s <= cand.range.start);
                let host = at
                    .checked_sub(1)
                    .map(|k| leaves[k])
                    .filter(|&(_, e, _)| cand.range.end <= e)
                    .map(|(_, _, i)| i);
                let k = tops.partition_point(|&(s, _)| s < cand.range.end);
                let outside_boxes = k == 0 || tops[k - 1].1 <= cand.range.start;
                if let Some(h) = host {
                    if self.nodes[h].notes.len() < MAX_NOTES {
                        self.note(
                            h,
                            format!(
                                "holds {} at {}..{}: the iloc points inside this box",
                                cand.who, cand.range.start, cand.range.end
                            ),
                        );
                    }
                    placed.entry(h).or_default().push(cand);
                    continue;
                }
                if outside_boxes {
                    top_level.push(cand);
                    continue;
                }
            }
            if self.nodes[cand.note_to].notes.len() < MAX_NOTES {
                let place = self.top_box_at(cand.range.start);
                self.note(
                    cand.note_to,
                    format!(
                        "{} at {}..{} lies outside every mdat/idat ({place}); not listed as a part",
                        cand.who, cand.range.start, cand.range.end
                    ),
                );
            } else {
                self.note(cand.note_to, String::new());
            }
        }
        // Past the last box: top-level extents, in order, without overlaps.
        top_level.sort_by_key(|c| c.range.start);
        let mut last_end = 0u64;
        for cand in top_level {
            if cand.range.start < last_end {
                continue;
            }
            last_end = cand.range.end;
            let mut node = Self::leaf(None, PartKind::Extent, cand.tag, cand.range, cand.disp);
            node.label = cand.label;
            node.notes.push(format!(
                "{}; {}; after the last box: the iloc points past every box",
                cand.who, cand.why
            ));
            self.add(node)?;
        }
        for (holder, list) in placed {
            // Consumed extents first, so an unconsumed one never covers bytes
            // the decode reads. Parts cannot overlap: pass 1 lists the
            // consumed extents that share no bytes, pass 2 the unshared bytes
            // of the consumed ones that do, pass 3 the unconsumed extents
            // that share no bytes (the rest are only noted).
            let (mut first, mut second): (Vec<Cand>, Vec<Cand>) =
                list.into_iter().partition(|c| c.disp.is_consumed());
            let order = |a: &Cand, b: &Cand| {
                a.range
                    .start
                    .cmp(&b.range.start)
                    .then(rank(b.disp).cmp(&rank(a.disp)))
            };
            first.sort_by(order);
            second.sort_by(order);
            // Listed ranges: start -> (end, who); disjoint.
            let mut taken: BTreeMap<u64, (u64, String)> = BTreeMap::new();
            let shared_with = |taken: &BTreeMap<u64, (u64, String)>, r: &Range<u64>| {
                let mut v: Vec<(u64, u64, String)> = taken
                    .range(..r.end)
                    .rev()
                    .take_while(|(_, (e, _))| *e > r.start)
                    .map(|(&s, (e, w))| (s, *e, w.clone()))
                    .collect();
                v.reverse();
                v
            };
            let mut deferred: Vec<Cand> = Vec::new();
            for (pass, cand) in first
                .into_iter()
                .map(|c| (1u8, c))
                .chain(second.into_iter().map(|c| (3u8, c)))
            {
                self.check_stop()?;
                if pass == 3 && !deferred.is_empty() {
                    self.place_pieces(
                        holder,
                        std::mem::take(&mut deferred),
                        &mut taken,
                        &shared_with,
                    )?;
                }
                let shared = shared_with(&taken, &cand.range);
                if shared.is_empty() {
                    taken.insert(cand.range.start, (cand.range.end, cand.who.clone()));
                    self.place_extent(holder, cand)?;
                    continue;
                }
                let other = shared[0].2.clone();
                let what = if pass == 1 {
                    "only its unshared bytes are listed"
                } else {
                    "not listed as a part"
                };
                if self.nodes[cand.note_to].notes.len() < MAX_NOTES {
                    self.note(
                        cand.note_to,
                        format!(
                            "{} at {}..{} shares bytes with {other}; {what}",
                            cand.who, cand.range.start, cand.range.end
                        ),
                    );
                } else {
                    self.note(cand.note_to, String::new());
                }
                if pass == 1 {
                    deferred.push(cand);
                }
            }
            if !deferred.is_empty() {
                self.place_pieces(holder, deferred, &mut taken, &shared_with)?;
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
        // The extent budget ran out: bytes no listed extent covers may still
        // be item data the decoder reads, so they are Unknown, not slack.
        if self.iloc_budget_exhausted {
            let holders: Vec<usize> = self
                .mdats
                .iter()
                .copied()
                .chain(self.idats.iter().map(|&(n, _)| n))
                .collect();
            for h in holders {
                if self.nodes[h].body.is_some() {
                    self.nodes[h].gap_fill = Some(Disposition::Unknown);
                    self.note(
                        h,
                        format!(
                            "the iloc extent budget ({MAX_ILOC_EXTENTS}) ran out: bytes outside the listed extents may be item data the decoder reads"
                        ),
                    );
                }
            }
        }
        Ok(())
    }

    /// One extent as an `Extent` part in its holder.
    fn place_extent(&mut self, holder: usize, cand: Cand) -> Result<(), At<Error>> {
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
        Ok(())
    }

    /// The bytes of consumed extents that listed ones do not cover, as
    /// parts of their own. `split_metadata_extents` skips these pieces
    /// (their first note starts with "item").
    #[allow(clippy::type_complexity)]
    fn place_pieces(
        &mut self,
        holder: usize,
        deferred: Vec<Cand>,
        taken: &mut BTreeMap<u64, (u64, String)>,
        shared_with: &dyn Fn(&BTreeMap<u64, (u64, String)>, &Range<u64>) -> Vec<(u64, u64, String)>,
    ) -> Result<(), At<Error>> {
        for cand in deferred {
            self.check_stop()?;
            let shared = shared_with(taken, &cand.range);
            let other = shared.first().map(|x| x.2.clone()).unwrap_or_default();
            let mut pieces = Vec::new();
            let mut at = cand.range.start;
            for (s, e, _) in &shared {
                if *s > at {
                    pieces.push(at..*s);
                }
                at = at.max(*e);
            }
            if at < cand.range.end {
                pieces.push(at..cand.range.end);
            }
            for piece in pieces {
                taken.insert(piece.start, (piece.end, cand.who.clone()));
                let mut node = Self::leaf(
                    Some(holder),
                    PartKind::Extent,
                    cand.tag.clone(),
                    piece,
                    cand.disp,
                );
                node.label = cand.label.clone();
                node.notes.push(format!(
                    "item data: bytes of {} that {other} and the other listed extents do not cover; {}",
                    cand.who, cand.why
                ));
                self.add(node)?;
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
            let parent_disp = self.nodes[i].disp;
            for (r, d, why) in inner {
                // A tail handed over with its blob carries the blob's
                // disposition (an ICC profile reported as the gain map's
                // alternate colour is gain-map metadata).
                let d = match (d, parent_disp) {
                    (Disposition::Metadata(_), Disposition::Metadata(_)) => parent_disp,
                    _ => d,
                };
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
                            Disposition::Metadata(MetadataKind::Xmp),
                            "after the XMP packet trailer; the caller receives it with the packet"
                                .to_string(),
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

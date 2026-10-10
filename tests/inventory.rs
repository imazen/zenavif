//! Structural inventory (`DecodeJob::inventory`, `src/codec/inventory.rs`).
//!
//! - Every committed `tests/vectors` AVIF, and encoder output (with
//!   `encode`), passes `zencodec_testkit::check_inventory` (whole file, file +
//!   junk, every truncation); a file whose inventory attributes nothing to
//!   image data must also fail to decode.
//! - The EXIF, XMP and ICC bytes the inventory marks as reported are the bytes
//!   `probe_full` reports.
//! - A synthetic AVIF holding every box and item kind zenavif handles, private
//!   units and trailing bytes has a pinned part list.
//! - `AVIF_INVENTORY_CORPUS=<dir>` runs the same checks over a corpus
//!   directory (`just inventory-corpus`), and with
//!   `INVENTORY_ORACLE_EXIFTOOL=<exiftool>` every box exiftool lists must match
//!   a part (`just inventory-oracle`).
//!
//! The env-gated tests are decided by the caller (justfile / CI): with the
//! variable unset they do nothing and say so.

use std::path::{Path, PathBuf};

use zenavif::AvifDecoderConfig;
use zencodec::decode::{DecodeJob, DecoderConfig};
use zencodec::inventory::{Disposition, Inventory, MetadataKind, Part, PartKind, PartTag};

fn vectors_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("avif"))
        {
            out.push(p);
        }
    }
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    out
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

fn bytes<'a>(data: &'a [u8], p: &Part) -> &'a [u8] {
    &data[p.range.start as usize..p.range.end as usize]
}

/// `check_inventory` for a file the inventory says the decoder turns into
/// pixels; for one it says nothing decodes, the same guarantees minus "some
/// part is image data", plus proof that the decode really fails.
fn check_file(data: &[u8], name: &str, undecodable: &mut usize) {
    let inv = inventory_of(data);
    if inv
        .parts()
        .iter()
        .any(|p| p.disposition == Disposition::ImageData)
    {
        zencodec_testkit::check_inventory(AvifDecoderConfig::new(), data)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        return;
    }
    *undecodable += 1;
    let decoded = AvifDecoderConfig::new()
        .job()
        .decoder(std::borrow::Cow::Borrowed(data), &[])
        .and_then(zencodec::decode::Decode::decode);
    assert!(
        decoded.is_err(),
        "{name}: the inventory attributes no bytes to image data, but the decode succeeds\n{inv}"
    );
    let mut junked = data.to_vec();
    junked.extend((0..37u8).map(|i| i.wrapping_mul(97) ^ 0x5A));
    let inv = inventory_of(&junked);
    let mut has_child = vec![false; inv.parts().len()];
    for p in inv.parts() {
        if let Some(q) = p.parent {
            has_child[q.index()] = true;
        }
    }
    for (i, p) in inv.parts().iter().enumerate() {
        if !has_child[i] && p.range.end > data.len() as u64 {
            assert!(
                !p.disposition.is_consumed(),
                "{name}: appended junk reported as {}\n{inv}",
                p.disposition
            );
        }
    }
    let len = data.len();
    let mut cuts: Vec<usize> = vec![0, 1, 2, 3, 4, 8, 16];
    for (num, den) in [(1, 8), (1, 4), (3, 8), (1, 2), (5, 8), (3, 4), (7, 8)] {
        cuts.push(len * num / den);
    }
    cuts.push(len.saturating_sub(1));
    cuts.retain(|&n| n < len);
    for n in cuts {
        let inv = inventory_of(&data[..n]);
        assert_eq!(inv.input_len(), n as u64, "{name}");
    }
}

#[test]
fn committed_vectors_inventories_conform() {
    let files = files_under(&vectors_dir());
    assert!(
        files.len() >= 10,
        "tests/vectors missing or thin ({} files)",
        files.len()
    );
    let mut undecodable = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        check_file(&data, &path.display().to_string(), &mut undecodable);
    }
    eprintln!("{} files, {undecodable} without image data", files.len());
}

#[cfg(feature = "encode")]
#[test]
fn encoder_output_inventory_conforms() {
    let (w, h) = (40u32, 24u32);
    let pixels: Vec<u8> = (0..w * h)
        .flat_map(|i| [(i % w * 6) as u8, (i / w * 10) as u8, 128])
        .collect();
    let img = zenpixels::PixelBuffer::from_vec(pixels, w, h, zenpixels::PixelDescriptor::RGB8_SRGB)
        .unwrap();
    let encoded = zenavif::encode(&img).unwrap();
    zencodec_testkit::check_inventory(AvifDecoderConfig::new(), &encoded.avif_file).unwrap();
}

/// The EXIF, XMP and ICC bytes the inventory marks as reported are exactly
/// what `probe_full` reports, and nothing else is marked.
#[test]
fn reported_metadata_matches_probe_full() {
    let mut files = files_under(&vectors_dir());
    if let Some(dir) = corpus_dir() {
        files.extend(files_under(&dir));
    }
    let mut checked = [0usize; 3];
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let Ok(info) = AvifDecoderConfig::new().job().probe_full(&data) else {
            continue;
        };
        let inv = inventory_of(&data);
        let mut has_child = vec![false; inv.parts().len()];
        for p in inv.parts() {
            if let Some(q) = p.parent {
                has_child[q.index()] = true;
            }
        }
        // EXIF: the leaf (a split extent's TIFF child is what is reported).
        // XMP and ICC: the whole extent or property, trailer included.
        // Track-level sidecars (tag None) are reported only for pure
        // sequences, which `probe_full` covers through the same accessor.
        let of_kind = |k: MetadataKind| -> Vec<&Part> {
            inv.parts()
                .iter()
                .enumerate()
                .filter(|(i, p)| {
                    p.disposition == Disposition::Metadata(k)
                        && (k != MetadataKind::Exif || !has_child[*i])
                        && matches!(
                            p.kind,
                            PartKind::Extent | PartKind::Property | PartKind::Gap
                        )
                })
                .map(|(_, p)| p)
                .collect()
        };
        let name = path.display();

        let exif = of_kind(MetadataKind::Exif);
        match &info.embedded_metadata.exif {
            Some(want) => {
                assert_eq!(exif.len(), 1, "{name}: one EXIF part expected\n{inv}");
                assert_eq!(
                    bytes(&data, exif[0]),
                    &want[..],
                    "{name}: EXIF bytes differ\n{inv}"
                );
                checked[0] += 1;
            }
            None => assert!(
                exif.is_empty(),
                "{name}: EXIF marked but not reported\n{inv}"
            ),
        }
        let xmp = of_kind(MetadataKind::Xmp);
        match &info.embedded_metadata.xmp {
            Some(want) => {
                assert_eq!(xmp.len(), 1, "{name}: one XMP part expected\n{inv}");
                assert_eq!(
                    bytes(&data, xmp[0]),
                    &want[..],
                    "{name}: XMP bytes differ\n{inv}"
                );
                checked[1] += 1;
            }
            None => assert!(xmp.is_empty(), "{name}: XMP marked but not reported\n{inv}"),
        }
        let icc: Vec<&Part> = of_kind(MetadataKind::Icc)
            .into_iter()
            .filter(|p| p.kind == PartKind::Property)
            .collect();
        match &info.source_color.icc_profile {
            Some(want) => {
                assert_eq!(icc.len(), 1, "{name}: one ICC property expected\n{inv}");
                // colr header (8) + colour type (4).
                assert_eq!(
                    &bytes(&data, icc[0])[12..],
                    &want[..],
                    "{name}: ICC bytes differ"
                );
                checked[2] += 1;
            }
            None => assert!(icc.is_empty(), "{name}: ICC marked but not reported\n{inv}"),
        }
    }
    eprintln!(
        "cross-checked EXIF in {}, XMP in {}, ICC in {} files",
        checked[0], checked[1], checked[2]
    );
}

// ── Synthetic fixture ───────────────────────────────────────────────────────

fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(typ);
    v.extend_from_slice(payload);
    v
}

fn full(typ: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = vec![version];
    p.extend_from_slice(&flags.to_be_bytes()[1..]);
    p.extend_from_slice(payload);
    bx(typ, &p)
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

fn infe(id: u16, typ: &[u8; 4], name: &str, content_type: Option<&str>, hidden: bool) -> Vec<u8> {
    let mut p = id.to_be_bytes().to_vec();
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(typ);
    p.extend_from_slice(name.as_bytes());
    p.push(0);
    if let Some(ct) = content_type {
        p.extend_from_slice(ct.as_bytes());
        p.push(0);
    }
    full(b"infe", 2, u32::from(hidden), &p)
}

fn iref_entry(typ: &[u8; 4], from: u16, to: &[u16]) -> Vec<u8> {
    let mut p = from.to_be_bytes().to_vec();
    p.extend_from_slice(&(to.len() as u16).to_be_bytes());
    for t in to {
        p.extend_from_slice(&t.to_be_bytes());
    }
    bx(typ, &p)
}

/// One `iloc` v1 entry: `(item, construction method, [(offset, length)])`,
/// 4-byte offsets and lengths, no base offset.
type Loc = (u16, u8, Vec<(u32, u32)>);

fn iloc(locs: &[Loc]) -> Vec<u8> {
    let mut p = vec![0x44, 0x00];
    p.extend_from_slice(&(locs.len() as u16).to_be_bytes());
    for (id, method, extents) in locs {
        p.extend_from_slice(&id.to_be_bytes());
        p.extend_from_slice(&[0, *method]);
        p.extend_from_slice(&[0, 0]);
        p.extend_from_slice(&(extents.len() as u16).to_be_bytes());
        for (o, l) in extents {
            p.extend_from_slice(&o.to_be_bytes());
            p.extend_from_slice(&l.to_be_bytes());
        }
    }
    full(b"iloc", 1, 0, &p)
}

/// ISO 21496-1 `tmap` payload (single channel, full fractions), then 3
/// bytes no reader uses.
fn tmap_payload() -> Vec<u8> {
    let mut p = vec![0u8, 0, 0, 0, 0, 0]; // version, min/writer version, flags
    let frac = |n: u32, d: u32| [n.to_be_bytes(), d.to_be_bytes()].concat();
    p.extend(frac(0, 1)); // base headroom
    p.extend(frac(1, 1)); // alternate headroom
    p.extend(frac(0, 1)); // gain map min
    p.extend(frac(1, 1)); // gain map max
    p.extend(frac(1, 1)); // gamma
    p.extend(frac(1, 64)); // base offset
    p.extend(frac(1, 64)); // alternate offset
    p.extend_from_slice(b"XYZ");
    p
}

/// An AVIF holding every box and item kind zenavif handles: primary, alpha,
/// thumbnail, gain map (tmap), EXIF with padding, XMP with a trailer, other
/// mime and private items, unknown and unreferenced properties, property
/// tails, slack in mdat and idat, and trailing bytes.
fn synthetic_avif() -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"avif\0\0\0\0mif1avifmiaf");
    let hdlr = full(b"hdlr", 0, 0, b"\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0zen\0");
    let dinf = bx(
        b"dinf",
        &full(
            b"dref",
            0,
            0,
            &cat(&[1u32.to_be_bytes().to_vec(), full(b"url ", 0, 1, b"")]),
        ),
    );
    let pitm = full(b"pitm", 0, 0, &1u16.to_be_bytes());
    let iinf = full(
        b"iinf",
        0,
        0,
        &cat(&[
            9u16.to_be_bytes().to_vec(),
            infe(1, b"av01", "Color", None, false),
            infe(2, b"Exif", "", None, true),
            infe(3, b"mime", "XMP", Some("application/rdf+xml"), true),
            infe(4, b"mime", "notes", Some("text/plain"), true),
            infe(5, b"zzzz", "private", None, true),
            infe(6, b"av01", "Alpha", None, true),
            infe(7, b"av01", "Thumb", None, false),
            infe(8, b"tmap", "GMap", None, false),
            infe(9, b"av01", "GMapImage", None, true),
        ]),
    );
    let iref = full(
        b"iref",
        0,
        0,
        &cat(&[
            iref_entry(b"auxl", 6, &[1]),
            iref_entry(b"cdsc", 2, &[1]),
            iref_entry(b"cdsc", 3, &[1]),
            iref_entry(b"thmb", 7, &[1]),
            iref_entry(b"dimg", 8, &[1, 9]),
            iref_entry(b"zRef", 5, &[1]),
        ]),
    );
    let ispe = |w: u32, h: u32, tail: &[u8]| {
        full(
            b"ispe",
            0,
            0,
            &cat(&[
                w.to_be_bytes().to_vec(),
                h.to_be_bytes().to_vec(),
                tail.to_vec(),
            ]),
        )
    };
    let ipco = bx(
        b"ipco",
        &cat(&[
            bx(b"av1C", &[0x81, 0x00, 0x0C, 0x00, 0x0A, 0x0B]), // 1: 2 configOBU bytes
            ispe(64, 64, b"xyz"),                               // 2: 3-byte tail
            bx(b"colr", b"nclx\0\x01\0\x0d\0\x06\x80"),         // 3
            bx(
                b"colr",
                &cat(&[
                    b"prof".to_vec(),
                    20u32.to_be_bytes().to_vec(),
                    b"sixteen-icc-body".to_vec(),
                    b"after".to_vec(),
                ]),
            ), // 4: ICC declaring 20 bytes, then 5 more
            bx(b"irot", &[1]),                                  // 5
            full(b"pixi", 0, 0, &[3, 8, 8, 8]),                 // 6
            bx(b"zPrv", b"private property"),                   // 7
            bx(b"clli", &[0x03, 0xE8, 0x00, 0xC8]),             // 8
            full(
                b"auxC",
                0,
                0,
                b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0ab",
            ), // 9
            ispe(16, 16, b""),                                  // 10
            bx(b"uuid", b"0123456789abcdef-payload"),           // 11
            bx(b"colr", b"nclx\0\x09\0\x10\0\x09\x80"),         // 12: tmap alternate
        ]),
    );
    let ipma = full(
        b"ipma",
        0,
        0,
        &cat(&[
            5u32.to_be_bytes().to_vec(),
            vec![0, 1, 7, 0x81, 2, 3, 4, 0x85, 6, 8],
            vec![0, 6, 3, 0x81, 2, 9],
            vec![0, 7, 2, 0x81, 10],
            vec![0, 8, 2, 2, 12],
            vec![0, 9, 2, 0x81, 10],
        ]),
    );
    let iprp = bx(b"iprp", &cat(&[ipco, ipma]));
    let notes = b"free text!".to_vec();
    let idat = bx(b"idat", &cat(&[notes.clone(), b"idt".to_vec()]));
    let grpl = bx(
        b"grpl",
        &full(
            b"altr",
            0,
            0,
            &cat(&[
                9u32.to_be_bytes().to_vec(),
                2u32.to_be_bytes().to_vec(),
                1u32.to_be_bytes().to_vec(),
                8u32.to_be_bytes().to_vec(),
            ]),
        ),
    );
    let zmet = bx(b"zMet", b"private meta child");
    let free = bx(b"free", b"stale editor data");
    let uuid = bx(
        b"uuid",
        b"\xbe\x7a\xcf\xcb\x97\xa9\x42\xe8\x9c\x71\x99\x94\x91\xe3\xaf\xacprivate payload",
    );

    let primary = b"PRIMARY-AV1-OBU-DATA".to_vec();
    let slack = b"slack".to_vec();
    let exif = b"\0\0\0\x02PPMM\0*\0\0\0\x08\0\0".to_vec();
    let xmp = b"<?xpacket begin?><x:xmpmeta/><?xpacket end=\"w\"?>TAIL".to_vec();
    let private = b"PRIVATE!".to_vec();
    let alpha = b"ALPHA-AV1".to_vec();
    let thumb = b"THUMB-AV1".to_vec();
    let tmap = tmap_payload();
    let gmap = b"GAIN-MAP-AV1".to_vec();
    let mdat_payload = cat(&[
        primary.clone(),
        slack.clone(),
        exif.clone(),
        xmp.clone(),
        private.clone(),
        alpha.clone(),
        thumb.clone(),
        tmap.clone(),
        gmap.clone(),
    ]);

    let build = |mdat_at: u32| -> Vec<u8> {
        let mut at = mdat_at + 8;
        let mut next = |n: usize| {
            let o = at;
            at += n as u32;
            (o, n as u32)
        };
        let p = next(primary.len());
        next(slack.len());
        let e = next(exif.len());
        let x = next(xmp.len());
        let v = next(private.len());
        let a = next(alpha.len());
        let t = next(thumb.len());
        let tm = next(tmap.len());
        let g = next(gmap.len());
        let iloc = iloc(&[
            (1, 0, vec![p]),
            (2, 0, vec![e]),
            (3, 0, vec![x]),
            (4, 1, vec![(0, notes.len() as u32)]),
            (5, 0, vec![v]),
            (6, 0, vec![a]),
            (7, 0, vec![t]),
            (8, 0, vec![tm]),
            (9, 0, vec![g]),
        ]);
        let meta = full(
            b"meta",
            0,
            0,
            &cat(&[
                hdlr.clone(),
                dinf.clone(),
                pitm.clone(),
                iloc,
                iinf.clone(),
                iref.clone(),
                iprp.clone(),
                idat.clone(),
                grpl.clone(),
                zmet.clone(),
            ]),
        );
        cat(&[
            ftyp.clone(),
            meta,
            free.clone(),
            uuid.clone(),
            bx(b"mdat", &mdat_payload),
            b"TRAILER".to_vec(),
        ])
    };
    let draft = build(0);
    let mdat_at = draft.windows(4).position(|w| w == b"mdat").unwrap() as u32 - 4;
    build(mdat_at)
}

#[test]
fn synthetic_fixture_parses_and_conforms() {
    let data = synthetic_avif();
    // The fixture must be one zenavif-parse accepts, or every item would be
    // `Dropped` and the pinned list would test the rejection path only.
    zenavif_parse::AvifParser::from_bytes(&data).expect("zenavif-parse accepts the fixture");
    zencodec_testkit::check_inventory(AvifDecoderConfig::new(), &data).unwrap();
}

/// The exact part list of the synthetic fixture, in file order.
#[test]
fn synthetic_fixture_part_list_is_pinned() {
    let data = synthetic_avif();
    let inv = inventory_of(&data);
    let mut got = Vec::new();
    fn walk(
        inv: &Inventory,
        parent: Option<zencodec::inventory::PartId>,
        depth: usize,
        out: &mut Vec<String>,
    ) {
        for id in inv.children(parent) {
            let p = inv.get(id).unwrap();
            out.push(format!(
                "{}{} {} {}..{} {}{}",
                "  ".repeat(depth),
                p.kind.name(),
                p.tag,
                p.range.start,
                p.range.end,
                p.disposition,
                p.label
                    .as_deref()
                    .map(|l| format!(" {l:?}"))
                    .unwrap_or_default()
            ));
            walk(inv, Some(id), depth + 1, out);
        }
    }
    walk(&inv, None, 0, &mut got);
    let expected = EXPECTED_SYNTHETIC
        .trim_matches('\n')
        .lines()
        .collect::<Vec<_>>();
    let got_ref: Vec<&str> = got.iter().map(String::as_str).collect();
    assert_eq!(got_ref, expected, "\n{}\n{inv}", got.join("\n"));
}

const EXPECTED_SYNTHETIC: &str = "
box ftyp 0..28 structure \"avif\"
box meta 28..1090 structure
  box hdlr 40..76 structure \"pict\"
    gap - 60..76 dropped
  box dinf 76..112 skipped
    box dref 84..112 skipped
      box url  100..112 skipped
  box pitm 112..126 structure
  box iloc 126..286 structure
  box iinf 286..563 structure
    item 0x1 300..326 structure \"Color\"
      gap - 320..326 dropped
    item 0x2 326..347 structure \"Exif\"
      gap - 346..347 dropped
    item 0x3 347..391 structure \"XMP\"
      gap - 367..371 dropped
    item 0x4 391..428 skipped \"notes\"
    item 0x5 428..456 unknown \"private\"
    item 0x6 456..482 structure \"Alpha\"
      gap - 476..482 dropped
    item 0x7 482..508 skipped \"Thumb\"
    item 0x8 508..533 structure \"GMap\"
      gap - 528..533 dropped
    item 0x9 533..563 structure \"GMapImage\"
      gap - 553..563 dropped
  box iref 563..661 structure
    box auxl 575..589 structure
    box cdsc 589..603 structure
    box cdsc 603..617 structure
    box thmb 617..631 dropped
    box dimg 631..647 structure
    box zRef 647..661 dropped
  box iprp 661..1007 structure
    box ipco 669..960 structure
      property av1C 677..691 structure
        gap - 689..691 dropped
      property ispe 691..714 structure
        gap - 711..714 dropped
      property colr 714..733 metadata(cicp) \"nclx\"
      property colr 733..770 metadata(icc) \"prof\"
        gap - 765..770 unreferenced
      property irot 770..779 metadata(orientation)
      property pixi 779..795 dropped
      property zPrv 795..819 unknown
      property clli 819..831 metadata(hdr-static)
      property auxC 831..889 structure \"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\"
        gap - 887..889 dropped
      property ispe 889..909 dropped
      property uuid 909..941 unknown \"30313233-3435-3637-3839-616263646566\"
      property colr 941..960 metadata(gain-map) \"nclx\"
    box ipma 960..1007 structure
  box idat 1007..1028 structure
    extent 0x4 1015..1025 skipped \"notes\"
    gap - 1025..1028 unreferenced
  box grpl 1028..1064 dropped
    box altr 1036..1064 dropped
  box zMet 1064..1090 unknown
box free 1090..1115 padding
box uuid 1115..1154 unknown \"be7acfcb-97a9-42e8-9c71-999491e3afac\"
box mdat 1154..1358 structure
  extent 0x1 1162..1182 image-data \"Color\"
  gap - 1182..1187 unreferenced
  extent 0x2 1187..1203 metadata(exif) \"Exif\"
    gap - 1187..1191 structure
    gap - 1191..1193 dropped
    gap - 1193..1203 metadata(exif)
  extent 0x3 1203..1255 metadata(xmp) \"XMP\"
    gap - 1251..1255 unreferenced
  extent 0x5 1255..1263 unknown \"private\"
  extent 0x6 1263..1272 image-data \"Alpha\"
  extent 0x7 1272..1281 skipped \"Thumb\"
  extent 0x8 1281..1346 metadata(gain-map) \"GMap\"
    gap - 1343..1346 dropped
  extent 0x9 1346..1358 metadata(gain-map) \"GMapImage\"
gap - 1358..1365 trailing
";

// ── Caller-gated corpus and oracle runs ─────────────────────────────────────

fn corpus_dir() -> Option<PathBuf> {
    std::env::var_os("AVIF_INVENTORY_CORPUS").map(PathBuf::from)
}

/// `AVIF_INVENTORY_CORPUS=<dir>`: every AVIF file under it.
#[test]
fn corpus_inventories_conform() {
    let Some(dir) = corpus_dir() else {
        eprintln!("AVIF_INVENTORY_CORPUS unset: corpus run not requested (just inventory-corpus)");
        return;
    };
    let files = files_under(&dir);
    assert!(!files.is_empty(), "no AVIF files under {}", dir.display());
    let mut undecodable = 0;
    let mut totals: std::collections::BTreeMap<&'static str, u64> = Default::default();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        check_file(&data, &path.display().to_string(), &mut undecodable);
        for (d, n) in inventory_of(&data).bytes_by_disposition() {
            *totals.entry(d).or_default() += n;
        }
    }
    eprintln!(
        "{} files checked ({undecodable} without image data); bytes by disposition: {totals:?}",
        files.len()
    );
}

/// One unit `exiftool -v3` lists: its type and payload position.
struct OracleUnit {
    fourcc: String,
    payload_at: u64,
    payload_len: u64,
}

/// `- Tag 'ftyp' (20 bytes):` followed by `0008: 68 65 …`: four-character
/// box names with the absolute offset of their payload. Tags inside an
/// embedded ICC profile are skipped (they are not boxes).
fn exiftool_units(exiftool: &Path, file: &Path) -> Vec<OracleUnit> {
    let out = std::process::Command::new(exiftool)
        .arg("-v3")
        .arg(file)
        .output()
        .unwrap_or_else(|e| panic!("running {}: {e}", exiftool.display()));
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    let depth = |l: &str| {
        l.chars()
            .take_while(|c| *c == ' ' || *c == '|')
            .filter(|c| *c == '|')
            .count()
    };
    let mut inside_icc: Option<usize> = None;
    let mut units = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let d = depth(line);
        if let Some(icc) = inside_icc {
            if d > icc {
                continue;
            }
            inside_icc = None;
        }
        if line.contains("+ [ICC_Profile directory") {
            inside_icc = Some(d);
            continue;
        }
        let Some(at) = line.find("- Tag '") else {
            continue;
        };
        let rest = &line[at + 7..];
        let Some(q) = rest.find('\'') else {
            continue;
        };
        let name = &rest[..q];
        if name.chars().count() != 4 {
            continue;
        }
        let Some(len) = rest[q..]
            .strip_prefix("' (")
            .and_then(|r| r.split(' ').next())
            .and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        if len == 0 {
            continue;
        }
        let Some(hex) = lines.get(i + 1).and_then(|l| {
            let t = l.trim_start_matches([' ', '|']);
            let colon = t.find(':')?;
            u64::from_str_radix(&t[..colon], 16).ok()
        }) else {
            continue;
        };
        units.push(OracleUnit {
            fourcc: name.to_string(),
            payload_at: hex,
            payload_len: len,
        });
    }
    units
}

/// `INVENTORY_ORACLE_EXIFTOOL=<exiftool>` + `AVIF_INVENTORY_CORPUS=<dir>`:
/// every box exiftool lists appears in the inventory with the same payload
/// offset and length.
#[test]
fn exiftool_oracle_agrees() {
    let Some(tool) = std::env::var_os("INVENTORY_ORACLE_EXIFTOOL").map(PathBuf::from) else {
        eprintln!(
            "INVENTORY_ORACLE_EXIFTOOL unset: oracle run not requested (just inventory-oracle)"
        );
        return;
    };
    let dir = corpus_dir().expect("INVENTORY_ORACLE_EXIFTOOL needs AVIF_INVENTORY_CORPUS");
    let files = files_under(&dir);
    assert!(
        files.len() >= 20,
        "the oracle run needs at least 20 files, found {}",
        files.len()
    );
    let mut rows = Vec::new();
    let mut mismatches = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let inv = inventory_of(&data);
        let units = exiftool_units(&tool, path);
        let mut matched = 0;
        for u in &units {
            let hit = inv.parts().iter().any(|p| {
                let tag_ok = match (&p.kind, &p.tag) {
                    (PartKind::Item, _) => u.fourcc == "infe",
                    (_, PartTag::FourCc(cc)) => cc.as_slice() == u.fourcc.as_bytes(),
                    _ => false,
                };
                if !tag_ok {
                    return false;
                }
                let start = p.range.start as usize;
                let size32 = u32::from_be_bytes(data[start..start + 4].try_into().unwrap());
                let h = if size32 == 1 { 16 } else { 8 };
                let mut ok = p.range.start + h == u.payload_at
                    && p.range.end - u.payload_at == u.payload_len;
                if !ok && u.fourcc == "uuid" {
                    ok = p.range.start + h + 16 == u.payload_at
                        && p.range.end - u.payload_at == u.payload_len;
                }
                ok
            });
            if hit {
                matched += 1;
            } else {
                mismatches.push(format!(
                    "{}: exiftool '{}' payload {}+{} has no matching part",
                    path.display(),
                    u.fourcc,
                    u.payload_at,
                    u.payload_len
                ));
            }
        }
        let name = path
            .strip_prefix(&dir)
            .unwrap_or(path)
            .display()
            .to_string();
        rows.push(format!(
            "| {name} | {} | {matched} | {} |",
            units.len(),
            inv.parts().len()
        ));
    }
    eprintln!("| file | exiftool units | matched | inventory parts |\n|---|---|---|---|");
    for r in &rows {
        eprintln!("{r}");
    }
    eprintln!("{} mismatches", mismatches.len());
    for m in &mismatches {
        eprintln!("{m}");
    }
    assert!(
        mismatches.is_empty(),
        "{} exiftool units without a matching part",
        mismatches.len()
    );
}

#![no_main]

// Structural inventory fuzzer: `DecodeJob::inventory` must never panic, and
// every inventory it returns must cover the input exactly and pass
// `Inventory::validate`. The first byte toggles HDR reconstruction, which
// changes the gain map's disposition. `metadata_oracle` then checks the
// dispositions against what the decoder reports.

use libfuzzer_sys::fuzz_target;
use zencodec::decode::{DecodeJob, DecoderConfig};

fuzz_target!(|data: &[u8]| {
    let mut job = zenavif::AvifDecoderConfig::new().job();
    if data.first().is_some_and(|b| b & 1 != 0) {
        job = job.with_gain_map_render(zencodec::GainMapRender::ReconstructHdr {
            target_headroom: None,
        });
    }
    if let Ok(Some(inv)) = job.inventory(data) {
        assert_eq!(inv.input_len(), data.len() as u64);
        if let Err(e) = inv.validate() {
            panic!("invalid inventory: {e}\n{inv}");
        }
    }
    metadata_oracle(data);
});

/// The metadata and image-data oracle, under a small pixel limit so the
/// decode stays cheap (the inventory models the same limit): what
/// `probe_full` reports is a `Metadata` part, a still image's `Metadata`
/// parts are what it reports, and a decode that succeeds has image data.
fn metadata_oracle(data: &[u8]) {
    use std::borrow::Cow;
    use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
    use zencodec::inventory::{Disposition as D, Inventory, MetadataKind as K, PartKind, PartTag};
    let job = || {
        zenavif::AvifDecoderConfig::new()
            .job()
            .with_limits(zencodec::ResourceLimits::none().with_max_pixels(1 << 16))
    };
    let Ok(Some(inv)) = job().inventory(data) else {
        return;
    };
    // The bytes each top-most Metadata(kind) part hands over: minus its
    // non-metadata children, and a colr minus its header and colour type.
    let units = |inv: &Inventory, kind: K| -> Vec<Vec<u8>> {
        let parts = inv.parts();
        let mut out = Vec::new();
        for (i, p) in parts.iter().enumerate() {
            if p.disposition() != D::Metadata(kind)
                || p.parent()
                    .and_then(|q| inv.get(q))
                    .is_some_and(|q| q.disposition() == p.disposition())
            {
                continue;
            }
            let mut b = Vec::new();
            let mut at = p.range().start as usize;
            for c in parts.iter().filter(|c| {
                c.parent().map(|q| q.index()) == Some(i) && c.disposition() != p.disposition()
            }) {
                b.extend_from_slice(&data[at..c.range().start as usize]);
                at = c.range().end as usize;
            }
            b.extend_from_slice(&data[at..p.range().end as usize]);
            if *p.tag() == PartTag::FourCc(*b"colr") {
                b.drain(..12.min(b.len()));
            }
            out.push(b);
        }
        out
    };
    // A file with a moov may report the track's metadata (probe) and the
    // primary's (decode): only a still must match probe_full both ways.
    let animated = inv
        .parts()
        .iter()
        .any(|p| p.kind() == PartKind::Box && *p.tag() == PartTag::FourCc(*b"moov"));
    if let Ok(info) = job().probe_full(data) {
        for (kind, reported) in [
            (K::Exif, info.embedded_metadata.exif.as_deref()),
            (K::Xmp, info.embedded_metadata.xmp.as_deref()),
            (K::Icc, info.source_color.icc_profile.as_deref()),
        ] {
            let u = units(&inv, kind);
            if let Some(r) = reported {
                // Exactly one part, or (when items share bytes, which parts
                // cannot) somewhere every byte of it is in consumed parts.
                let covered = || {
                    // Each byte takes the disposition of the deepest part
                    // over it (parents come before their children).
                    let mut consumed = vec![false; data.len()];
                    for p in inv.parts() {
                        consumed[p.range().start as usize..p.range().end as usize]
                            .fill(p.disposition().is_consumed());
                    }
                    (0..(data.len() + 1).saturating_sub(r.len()))
                        .filter(|&k| data[k..k + r.len()] == *r)
                        .any(|k| consumed[k..k + r.len()].iter().all(|&c| c))
                };
                assert!(
                    u.iter().any(|x| x == r) || covered(),
                    "probe_full reports {kind:?} that no part holds\n{inv}"
                );
            }
            if !animated {
                for x in &u {
                    assert_eq!(
                        Some(&x[..]),
                        reported,
                        "a {kind:?} part probe_full does not report\n{inv}"
                    );
                }
            }
        }
    }
    if job()
        .decoder(Cow::Borrowed(data), &[])
        .and_then(Decode::decode)
        .is_ok()
    {
        assert!(
            inv.parts().iter().any(|p| p.disposition() == D::ImageData),
            "the decode succeeds, but no part is image data\n{inv}"
        );
    }
}

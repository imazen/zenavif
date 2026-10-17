//! The muxer: progressive (`moov` at the end, one patch for the `mdat`
//! size) or fragmented (`moov` up front, then `moof` + `mdat` pairs; no
//! patches).

use alloc::vec;
use alloc::vec::Vec;

use enough::Stop;
use zencodec::encode::OutputBuffer;
use zencodec_media::container::{MuxFinish, MuxHint, MuxTiming, Muxer, Patch};
use zencodec_media::packet::{Packet, RandomAccess};
use zencodec_media::session::{ProtocolViolation, SessionError};
use zencodec_media::track::{CodecId, TrackInfo, TrackKind};

use crate::Mp4Error;

type Res<T> = Result<T, SessionError<Mp4Error>>;

fn err<T>(e: Mp4Error) -> Res<T> {
    Err(SessionError::Codec(e))
}

/// The movie timescale (`mvhd`, `tkhd`, `elst` durations).
const MOVIE_TIMESCALE: u32 = 1000;
/// A fragment closes at this many bytes even without a random-access point.
const FRAGMENT_BYTES: usize = 8 << 20;

pub(crate) fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let len = body.len() + 8;
    let mut v = Vec::with_capacity(len + 8);
    match u32::try_from(len) {
        Ok(n) => {
            v.extend_from_slice(&n.to_be_bytes());
            v.extend_from_slice(kind);
        }
        Err(_) => {
            v.extend_from_slice(&1u32.to_be_bytes());
            v.extend_from_slice(kind);
            v.extend_from_slice(&(len as u64 + 8).to_be_bytes());
        }
    }
    v.extend_from_slice(body);
    v
}

pub(crate) fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 4);
    b.push(version);
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

#[derive(Clone, Copy, Debug)]
struct MuxSample {
    /// File offset (progressive) or offset in the fragment's data.
    offset: u64,
    size: u32,
    dts: i64,
    cts: i64,
    /// Known once the next sample's dts arrives (or from the packet).
    duration: Option<u64>,
    sync: bool,
}

#[derive(Debug)]
struct MuxTrack {
    info: TrackInfo,
    timescale: u32,
    handler: [u8; 4],
    /// `None` for an AV1 track still waiting for a sequence header.
    entry: Option<Vec<u8>>,
    /// Progressive: every sample. Fragmented: samples not yet in a fragment.
    samples: Vec<MuxSample>,
    /// Fragmented: the payloads of `samples`.
    data: Vec<Vec<u8>>,
    last_dts: Option<i64>,
    /// Decode time after the last sample written to a fragment.
    written_dts: Option<i64>,
    /// Total duration in media ticks (for headers).
    end_dts: i64,
    /// The last step between decode times, for the final sample.
    last_step: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Progressive,
    Fragmented,
}

/// Pull-output MP4 writer. [`Mp4Muxer::new`] writes a progressive file
/// (`moov` at the end; `finish` patches the `mdat` size), or a fragmented
/// one when `begin` gets [`MuxHint::forward_only`];
/// [`Mp4Muxer::fragmented`] always fragments. Fragments start at video
/// random-access points (or every 8 MiB without video).
///
/// Track clocks must be `1/timescale` (MP4 counts integer ticks of a
/// per-track timescale). Configuration records come from `codec_private`
/// in the forms [`Mp4Demuxer`](crate::Mp4Demuxer) returns them.
#[derive(Debug)]
pub struct Mp4Muxer {
    out: OutputBuffer,
    want: Option<Layout>,
    layout: Layout,
    begun: bool,
    tracks: Vec<MuxTrack>,
    /// Offset of the `mdat` header (progressive).
    mdat_at: u64,
    /// Fragment sequence number.
    sequence: u32,
    /// Fragmented: whether `moov` is out (it waits for derived `av1C`s).
    header_written: bool,
    /// Payload bytes waiting in the open fragment.
    fragment_bytes: usize,
}

impl Default for Mp4Muxer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mp4Muxer {
    /// Progressive, or fragmented for a forward-only destination.
    pub fn new() -> Self {
        Self {
            out: OutputBuffer::new(),
            want: None,
            layout: Layout::Progressive,
            begun: false,
            tracks: Vec::new(),
            mdat_at: 0,
            sequence: 0,
            header_written: false,
            fragment_bytes: 0,
        }
    }

    /// Always fragmented: no patches, playable while it is written.
    pub fn fragmented() -> Self {
        Self {
            want: Some(Layout::Fragmented),
            ..Self::new()
        }
    }

    fn movie_ticks(&self, t: &MuxTrack, ticks: i64) -> u64 {
        let ticks = u128::try_from(ticks.max(0)).unwrap_or(0);
        u64::try_from(ticks * u128::from(MOVIE_TIMESCALE) / u128::from(t.timescale))
            .unwrap_or(u64::MAX)
    }

    /// Give the last held sample of `track` its duration from `dts`.
    fn close_previous(track: &mut MuxTrack, dts: i64) {
        if let Some(prev) = track.samples.last_mut()
            && prev.duration.is_none()
        {
            let d = u64::try_from(dts - prev.dts).unwrap_or(0);
            prev.duration = Some(d);
            track.last_step = Some(d);
        }
    }

    fn ftyp(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"isom");
        b.extend_from_slice(&0x200u32.to_be_bytes());
        b.extend_from_slice(b"isom");
        b.extend_from_slice(b"iso2");
        if self.layout == Layout::Fragmented {
            b.extend_from_slice(b"iso5");
            b.extend_from_slice(b"iso6");
        }
        if self.tracks.iter().any(|t| *t.info.codec() == CodecId::Av1) {
            b.extend_from_slice(b"av01");
        }
        b.extend_from_slice(b"mp41");
        bx(b"ftyp", &b)
    }

    /// `moov`; with `tables` false (fragmented) the sample tables are empty
    /// and `mvex` declares the fragments.
    fn moov(&self, tables: bool) -> Res<Vec<u8>> {
        let duration = self
            .tracks
            .iter()
            .map(|t| self.movie_ticks(t, t.end_dts))
            .max()
            .unwrap_or(0);
        let mut body = mvhd(
            if tables { duration } else { 0 },
            self.tracks.len() as u32 + 1,
        );
        for (i, t) in self.tracks.iter().enumerate() {
            body.extend_from_slice(&self.trak(i, t, tables)?);
        }
        if !tables {
            let mut mvex = Vec::new();
            for i in 0..self.tracks.len() {
                let mut trex = (i as u32 + 1).to_be_bytes().to_vec();
                trex.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                mvex.extend_from_slice(&full(b"trex", 0, 0, &trex));
            }
            body.extend_from_slice(&bx(b"mvex", &mvex));
        }
        Ok(bx(b"moov", &body))
    }

    fn trak(&self, i: usize, t: &MuxTrack, tables: bool) -> Res<Vec<u8>> {
        let id = i as u32 + 1;
        let media_duration = if tables {
            u64::try_from(t.end_dts).unwrap_or(0)
        } else {
            0
        };
        let movie_duration = if tables {
            self.movie_ticks(t, t.end_dts)
        } else {
            0
        };
        let video = t.info.kind() == TrackKind::Video;
        let (w, h) = t
            .info
            .video_info()
            .map(|v| v.display.unwrap_or((v.coded_width, v.coded_height)))
            .unwrap_or((0, 0));
        // tkhd, version 1 for 64-bit times.
        let mut tkhd = vec![0u8; 16];
        tkhd.extend_from_slice(&id.to_be_bytes());
        tkhd.extend_from_slice(&[0; 4]);
        tkhd.extend_from_slice(&movie_duration.to_be_bytes());
        tkhd.extend_from_slice(&[0; 8]);
        tkhd.extend_from_slice(&[0, 0, 0, 0]); // layer, alternate group
        tkhd.extend_from_slice(if t.handler == *b"soun" {
            &[1, 0]
        } else {
            &[0, 0]
        });
        tkhd.extend_from_slice(&[0, 0]);
        tkhd.extend_from_slice(&IDENTITY);
        tkhd.extend_from_slice(&(w << 16).to_be_bytes());
        tkhd.extend_from_slice(&(h << 16).to_be_bytes());
        let mut trak = full(b"tkhd", 1, 3, &tkhd);
        // A fragmented moov is written before the duration is known: its
        // edit has segment_duration 0, which runs to the end of the media.
        if let Some(delay) = t.info.edit_delay_ticks()
            && delay >= 0
        {
            let mut elst = 1u32.to_be_bytes().to_vec();
            elst.extend_from_slice(&movie_duration.to_be_bytes());
            elst.extend_from_slice(&delay.to_be_bytes());
            elst.extend_from_slice(&0x0001_0000u32.to_be_bytes());
            trak.extend_from_slice(&bx(b"edts", &full(b"elst", 1, 0, &elst)));
        }
        let mut mdhd = vec![0u8; 16];
        mdhd.extend_from_slice(&t.timescale.to_be_bytes());
        mdhd.extend_from_slice(&media_duration.to_be_bytes());
        mdhd.extend_from_slice(&[0x55, 0xC4, 0, 0]); // "und"
        let mut hdlr = vec![0u8; 4];
        hdlr.extend_from_slice(&t.handler);
        hdlr.extend_from_slice(&[0; 12]);
        hdlr.extend_from_slice(if video {
            b"VideoHandler\0"
        } else {
            b"SoundHandler\0"
        });
        let header = if video {
            full(b"vmhd", 0, 1, &[0; 8])
        } else if t.handler == *b"soun" {
            full(b"smhd", 0, 0, &[0; 4])
        } else {
            full(b"nmhd", 0, 0, &[])
        };
        let dref = full(
            b"dref",
            0,
            0,
            &[&1u32.to_be_bytes()[..], &full(b"url ", 0, 1, &[])].concat(),
        );
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend_from_slice(
            t.entry
                .as_ref()
                .ok_or(SessionError::Codec(Mp4Error::Missing("av1C")))?,
        );
        let mut stbl = full(b"stsd", 0, 0, &stsd);
        stbl.extend_from_slice(&if tables {
            sample_tables(t)
        } else {
            empty_tables()
        });
        let minf = bx(
            b"minf",
            &[header, bx(b"dinf", &dref), bx(b"stbl", &stbl)].concat(),
        );
        let mdia = bx(
            b"mdia",
            &[full(b"mdhd", 1, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf].concat(),
        );
        trak.extend_from_slice(&mdia);
        Ok(bx(b"trak", &trak))
    }

    /// Fragmented: write `moov` once every sample entry is known.
    fn write_header(&mut self) -> Res<()> {
        if !self.header_written && self.tracks.iter().all(|t| t.entry.is_some()) {
            let moov = self.moov(false)?;
            self.out.push(&moov);
            self.header_written = true;
        }
        Ok(())
    }

    /// Write one fragment of every sample whose duration is known (all of
    /// them when `last`).
    fn flush_fragment(&mut self, last: bool) -> Res<()> {
        let mut trafs = Vec::new();
        let mut payloads: Vec<Vec<u8>> = Vec::new();
        let mut taken: Vec<(usize, usize)> = Vec::new();
        for (i, t) in self.tracks.iter_mut().enumerate() {
            let ready = if last {
                if let Some(s) = t.samples.last_mut()
                    && s.duration.is_none()
                {
                    // The last sample of the stream: no successor; keep the
                    // previous step if there was one.
                    s.duration = Some(0);
                }
                t.samples.len()
            } else {
                t.samples
                    .iter()
                    .take_while(|s| s.duration.is_some())
                    .count()
            };
            if ready > 0 {
                taken.push((i, ready));
            }
        }
        if taken.is_empty() {
            return Ok(());
        }
        self.write_header()?;
        if !self.header_written {
            return err(Mp4Error::Missing("av1C"));
        }
        self.sequence += 1;
        // Build twice: the data offsets depend on the moof's own size.
        let mut moof_len = 0usize;
        for pass in 0..2 {
            trafs.clear();
            payloads.clear();
            let mut data_pos = moof_len + 8;
            for &(i, n) in &taken {
                let t = &self.tracks[i];
                let samples = &t.samples[..n];
                let any_cts = samples.iter().any(|s| s.cts != 0);
                let negative = samples.iter().any(|s| s.cts < 0);
                let mut flags = 0x1 | 0x100 | 0x200 | 0x400;
                if any_cts {
                    flags |= 0x800;
                }
                let mut trun = (n as u32).to_be_bytes().to_vec();
                trun.extend_from_slice(&(data_pos as i32).to_be_bytes());
                for s in samples {
                    trun.extend_from_slice(&(s.duration.unwrap_or(0) as u32).to_be_bytes());
                    trun.extend_from_slice(&s.size.to_be_bytes());
                    let sflags: u32 = if s.sync { 0x0200_0000 } else { 0x0101_0000 };
                    trun.extend_from_slice(&sflags.to_be_bytes());
                    if any_cts {
                        trun.extend_from_slice(&(s.cts as i32).to_be_bytes());
                    }
                }
                let base = t.written_dts.unwrap_or(samples[0].dts);
                let base = u64::try_from(samples[0].dts.max(base)).unwrap_or(0);
                let traf = [
                    full(b"tfhd", 0, 0x02_0000, &(i as u32 + 1).to_be_bytes()),
                    full(b"tfdt", 1, 0, &base.to_be_bytes()),
                    full(b"trun", u8::from(negative), flags, &trun),
                ]
                .concat();
                trafs.push(bx(b"traf", &traf));
                for d in &t.data[..n] {
                    data_pos += d.len();
                    if pass == 1 {
                        payloads.push(d.clone());
                    }
                }
            }
            let moof = bx(
                b"moof",
                &[
                    full(b"mfhd", 0, 0, &self.sequence.to_be_bytes()),
                    trafs.concat(),
                ]
                .concat(),
            );
            if pass == 0 {
                moof_len = moof.len();
            } else {
                let data: usize = payloads.iter().map(Vec::len).sum();
                self.out.push(&moof);
                if u32::try_from(data + 8).is_ok() {
                    self.out.push(&((data + 8) as u32).to_be_bytes());
                    self.out.push(b"mdat");
                } else {
                    return err(Mp4Error::Overflow);
                }
                for p in &payloads {
                    self.out.push(p);
                }
            }
        }
        for &(i, n) in &taken {
            let t = &mut self.tracks[i];
            let end = t.samples[n - 1].dts + t.samples[n - 1].duration.unwrap_or(0) as i64;
            t.written_dts = Some(end);
            t.samples.drain(..n);
            t.data.drain(..n);
        }
        self.fragment_bytes = self
            .tracks
            .iter()
            .flat_map(|t| t.data.iter())
            .map(Vec::len)
            .sum();
        Ok(())
    }
}

const IDENTITY: [u8; 36] = [
    0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0,
];

fn mvhd(duration: u64, next_track: u32) -> Vec<u8> {
    let mut b = vec![0u8; 16];
    b.extend_from_slice(&MOVIE_TIMESCALE.to_be_bytes());
    b.extend_from_slice(&duration.to_be_bytes());
    b.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    b.extend_from_slice(&[1, 0]);
    b.extend_from_slice(&[0; 10]);
    b.extend_from_slice(&IDENTITY);
    b.extend_from_slice(&[0; 24]);
    b.extend_from_slice(&next_track.to_be_bytes());
    full(b"mvhd", 1, 0, &b)
}

fn empty_tables() -> Vec<u8> {
    [
        full(b"stts", 0, 0, &[0; 4]),
        full(b"stsc", 0, 0, &[0; 4]),
        full(b"stsz", 0, 0, &[0; 8]),
        full(b"stco", 0, 0, &[0; 4]),
    ]
    .concat()
}

/// `stts`, `ctts`, `stss`, `stsc`, `stsz`, `stco`/`co64` for a progressive
/// track. A chunk is a run of the track's samples adjacent in the file.
fn sample_tables(t: &MuxTrack) -> Vec<u8> {
    let s = &t.samples;
    let mut out = Vec::new();
    // stts
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for x in s {
        let d = x.duration.unwrap_or(0) as u32;
        match runs.last_mut() {
            Some((n, dd)) if *dd == d => *n += 1,
            _ => runs.push((1, d)),
        }
    }
    let mut b = (runs.len() as u32).to_be_bytes().to_vec();
    for (n, d) in &runs {
        b.extend_from_slice(&n.to_be_bytes());
        b.extend_from_slice(&d.to_be_bytes());
    }
    out.extend_from_slice(&full(b"stts", 0, 0, &b));
    // ctts
    if s.iter().any(|x| x.cts != 0) {
        let negative = s.iter().any(|x| x.cts < 0);
        let mut runs: Vec<(u32, i64)> = Vec::new();
        for x in s {
            match runs.last_mut() {
                Some((n, c)) if *c == x.cts => *n += 1,
                _ => runs.push((1, x.cts)),
            }
        }
        let mut b = (runs.len() as u32).to_be_bytes().to_vec();
        for (n, c) in &runs {
            b.extend_from_slice(&n.to_be_bytes());
            b.extend_from_slice(&(*c as i32).to_be_bytes());
        }
        out.extend_from_slice(&full(b"ctts", u8::from(negative), 0, &b));
    }
    // stss
    if s.iter().any(|x| !x.sync) {
        let keys: Vec<u32> = s
            .iter()
            .enumerate()
            .filter(|(_, x)| x.sync)
            .map(|(i, _)| i as u32 + 1)
            .collect();
        let mut b = (keys.len() as u32).to_be_bytes().to_vec();
        for k in keys {
            b.extend_from_slice(&k.to_be_bytes());
        }
        out.extend_from_slice(&full(b"stss", 0, 0, &b));
    }
    // Chunks: runs of samples adjacent in the file.
    let mut chunks: Vec<(u64, u32, u64)> = Vec::new();
    for x in s {
        match chunks.last_mut() {
            Some((_, n, end)) if *end == x.offset => {
                *n += 1;
                *end += u64::from(x.size);
            }
            _ => chunks.push((x.offset, 1, x.offset + u64::from(x.size))),
        }
    }
    let mut stsc: Vec<(u32, u32)> = Vec::new();
    for (i, (_, n, _)) in chunks.iter().enumerate() {
        if stsc.last().is_none_or(|&(_, m)| m != *n) {
            stsc.push((i as u32 + 1, *n));
        }
    }
    let mut b = (stsc.len() as u32).to_be_bytes().to_vec();
    for (first, n) in &stsc {
        b.extend_from_slice(&first.to_be_bytes());
        b.extend_from_slice(&n.to_be_bytes());
        b.extend_from_slice(&1u32.to_be_bytes());
    }
    out.extend_from_slice(&full(b"stsc", 0, 0, &b));
    // stsz
    let fixed = s
        .first()
        .map(|x| x.size)
        .filter(|&z| s.iter().all(|x| x.size == z));
    let mut b = fixed.unwrap_or(0).to_be_bytes().to_vec();
    b.extend_from_slice(&(s.len() as u32).to_be_bytes());
    if fixed.is_none() {
        for x in s {
            b.extend_from_slice(&x.size.to_be_bytes());
        }
    }
    out.extend_from_slice(&full(b"stsz", 0, 0, &b));
    // stco / co64
    let wide = chunks.iter().any(|(o, _, _)| *o > u64::from(u32::MAX));
    let mut b = (chunks.len() as u32).to_be_bytes().to_vec();
    for (o, _, _) in &chunks {
        if wide {
            b.extend_from_slice(&o.to_be_bytes());
        } else {
            b.extend_from_slice(&(*o as u32).to_be_bytes());
        }
    }
    out.extend_from_slice(&full(if wide { b"co64" } else { b"stco" }, 0, 0, &b));
    out
}

/// A configuration box: its type and body.
type ConfigBox<'a> = ([u8; 4], &'a [u8]);

/// The sample entry for a track, and its handler.
fn sample_entry(t: &TrackInfo, av1c: Option<&[u8]>) -> Result<([u8; 4], Vec<u8>), Mp4Error> {
    let private = t
        .codec_private()
        .map(|p| p.as_ref())
        .filter(|p| !p.is_empty())
        .or(av1c);
    match t.kind() {
        TrackKind::Video => {
            let v = t.video_info().ok_or(Mp4Error::Missing("video info"))?;
            let (fourcc, config): ([u8; 4], Option<ConfigBox<'_>>) = match t.codec() {
                CodecId::Av1 => (
                    *b"av01",
                    Some((*b"av1C", private.ok_or(Mp4Error::Missing("av1C"))?)),
                ),
                CodecId::Vp9 => (*b"vp09", private.map(|p| (*b"vpcC", p))),
                CodecId::H264 => (
                    *b"avc1",
                    Some((*b"avcC", private.ok_or(Mp4Error::Missing("avcC"))?)),
                ),
                CodecId::Hevc => (
                    *b"hvc1",
                    Some((*b"hvcC", private.ok_or(Mp4Error::Missing("hvcC"))?)),
                ),
                CodecId::Other(tag) => (fourcc_of(tag.as_str())?, None),
                _ => return Err(Mp4Error::UnsupportedCodec),
            };
            // MPEG-4 Part 2 keeps its decoder configuration in an esds.
            let mp4v_esds = (fourcc == *b"mp4v")
                .then(|| private.map(|dsi| esds(0x20, 0x11, dsi)))
                .flatten();
            let w = u16::try_from(v.coded_width).map_err(|_| Mp4Error::DimensionsTooLarge)?;
            let h = u16::try_from(v.coded_height).map_err(|_| Mp4Error::DimensionsTooLarge)?;
            let mut b = vec![0u8; 6];
            b.extend_from_slice(&1u16.to_be_bytes());
            b.extend_from_slice(&[0; 16]);
            b.extend_from_slice(&w.to_be_bytes());
            b.extend_from_slice(&h.to_be_bytes());
            b.extend_from_slice(&0x0048_0000u32.to_be_bytes());
            b.extend_from_slice(&0x0048_0000u32.to_be_bytes());
            b.extend_from_slice(&[0; 4]);
            b.extend_from_slice(&1u16.to_be_bytes());
            b.extend_from_slice(&[0; 32]);
            b.extend_from_slice(&[0x00, 0x18, 0xFF, 0xFF]);
            if let Some((kind, body)) = config {
                b.extend_from_slice(&bx(&kind, body));
            }
            if let Some(e) = mp4v_esds {
                b.extend_from_slice(&e);
            }
            if let Some(c) = v.color {
                let mut colr = b"nclx".to_vec();
                colr.extend_from_slice(&u16::from(c.color_primaries).to_be_bytes());
                colr.extend_from_slice(&u16::from(c.transfer_characteristics).to_be_bytes());
                colr.extend_from_slice(&u16::from(c.matrix_coefficients).to_be_bytes());
                colr.push(if c.full_range { 0x80 } else { 0 });
                b.extend_from_slice(&bx(b"colr", &colr));
            }
            if let Some((dw, dh)) = v.display
                && dh == v.coded_height
                && dw != v.coded_width
            {
                let mut pasp = dw.to_be_bytes().to_vec();
                pasp.extend_from_slice(&v.coded_width.to_be_bytes());
                b.extend_from_slice(&bx(b"pasp", &pasp));
            }
            Ok((*b"vide", bx(&fourcc, &b)))
        }
        TrackKind::Audio => {
            let a = t.audio_info().ok_or(Mp4Error::Missing("audio info"))?;
            let (fourcc, config) = match t.codec() {
                CodecId::Opus => (
                    *b"Opus",
                    bx(
                        b"dOps",
                        &dops(private.ok_or(Mp4Error::Missing("OpusHead"))?)?,
                    ),
                ),
                CodecId::Aac => (
                    *b"mp4a",
                    esds(
                        0x40,
                        0x15,
                        private.ok_or(Mp4Error::Missing("AudioSpecificConfig"))?,
                    ),
                ),
                CodecId::Flac => {
                    let p = private.ok_or(Mp4Error::Missing("FLAC header"))?;
                    let blocks = p.strip_prefix(b"fLaC").ok_or(Mp4Error::BadCodecPrivate)?;
                    (*b"fLaC", full(b"dfLa", 0, 0, blocks))
                }
                _ => return Err(Mp4Error::UnsupportedCodec),
            };
            let mut b = vec![0u8; 6];
            b.extend_from_slice(&1u16.to_be_bytes());
            b.extend_from_slice(&[0; 8]);
            b.extend_from_slice(&a.channels.count.to_be_bytes());
            b.extend_from_slice(&16u16.to_be_bytes());
            b.extend_from_slice(&[0; 4]);
            let rate = if a.sample_rate < 65_536 {
                a.sample_rate << 16
            } else {
                0
            };
            b.extend_from_slice(&rate.to_be_bytes());
            b.extend_from_slice(&config);
            Ok((*b"soun", bx(&fourcc, &b)))
        }
        _ => Err(Mp4Error::UnsupportedCodec),
    }
}

fn fourcc_of(tag: &str) -> Result<[u8; 4], Mp4Error> {
    let b = tag.as_bytes();
    if b.len() != 4 {
        return Err(Mp4Error::UnsupportedCodec);
    }
    Ok([b[0], b[1], b[2], b[3]])
}

/// `OpusHead` (RFC 7845, little-endian) as a `dOps` body (big-endian).
fn dops(head: &[u8]) -> Result<Vec<u8>, Mp4Error> {
    if head.len() < 19 || &head[..8] != b"OpusHead" {
        return Err(Mp4Error::BadCodecPrivate);
    }
    let channels = head[9];
    let mut b = vec![0, channels];
    b.extend_from_slice(&u16::from_le_bytes([head[10], head[11]]).to_be_bytes());
    b.extend_from_slice(
        &u32::from_le_bytes([head[12], head[13], head[14], head[15]]).to_be_bytes(),
    );
    b.extend_from_slice(&u16::from_le_bytes([head[16], head[17]]).to_be_bytes());
    let family = head[18];
    b.push(family);
    if family != 0 {
        let table = head
            .get(19..21 + usize::from(channels))
            .ok_or(Mp4Error::BadCodecPrivate)?;
        b.extend_from_slice(table);
    }
    Ok(b)
}

/// Decoder-specific info wrapped in an `esds`: object type `oti` (0x40
/// AAC, 0x20 MPEG-4 Visual), stream type byte `stream` (0x15 audio, 0x11
/// visual).
fn esds(oti: u8, stream: u8, asc: &[u8]) -> Vec<u8> {
    fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
        let n = body.len() as u32;
        let mut v = vec![tag];
        v.extend_from_slice(&[
            0x80 | ((n >> 21) & 0x7F) as u8,
            0x80 | ((n >> 14) & 0x7F) as u8,
            0x80 | ((n >> 7) & 0x7F) as u8,
            (n & 0x7F) as u8,
        ]);
        v.extend_from_slice(body);
        v
    }
    let mut dcd = vec![oti, stream, 0, 0, 0];
    dcd.extend_from_slice(&[0; 8]);
    dcd.extend_from_slice(&descriptor(0x05, asc));
    let mut es = vec![0, 0, 0];
    es.extend_from_slice(&descriptor(0x04, &dcd));
    es.extend_from_slice(&descriptor(0x06, &[0x02]));
    full(b"esds", 0, 0, &descriptor(0x03, &es))
}

impl Muxer for Mp4Muxer {
    type Error = Mp4Error;

    fn begin(&mut self, tracks: &[TrackInfo], hint: MuxHint) -> Res<MuxTiming> {
        if self.begun {
            return Err(SessionError::Protocol(ProtocolViolation::BeginTwice));
        }
        self.layout = self.want.unwrap_or(if hint.forward_only {
            Layout::Fragmented
        } else {
            Layout::Progressive
        });
        for t in tracks {
            let tb = t.time_base();
            if tb.numerator() != 1 {
                return err(Mp4Error::UnsupportedTimeBase);
            }
            // An AV1 track without its record gets one from its first
            // sequence header (IVF carries none).
            let (handler, entry) = match sample_entry(t, None) {
                Ok((h, e)) => (h, Some(e)),
                Err(Mp4Error::Missing("av1C")) => (*b"vide", None),
                Err(e) => return err(e),
            };
            self.tracks.push(MuxTrack {
                info: t.clone(),
                timescale: tb.denominator(),
                handler,
                entry,
                samples: Vec::new(),
                data: Vec::new(),
                last_dts: None,
                written_dts: None,
                end_dts: 0,
                last_step: None,
            });
        }
        self.begun = true;
        self.out.push(&self.ftyp());
        Ok(match self.layout {
            Layout::Progressive => {
                self.mdat_at = self.out.total();
                // A 64-bit size, patched at finish.
                self.out
                    .push(&[0, 0, 0, 1, b'm', b'd', b'a', b't', 0, 0, 0, 0, 0, 0, 0, 0]);
                MuxTiming::streaming_with_patches()
            }
            Layout::Fragmented => {
                self.write_header()?;
                MuxTiming::streaming()
            }
        })
    }

    fn write_packet(&mut self, mut packet: Packet, stop: &dyn Stop) -> Res<()> {
        stop.check()?;
        if !self.begun {
            return Err(SessionError::Protocol(ProtocolViolation::NoBegin));
        }
        let index = packet.track() as usize;
        let Some(track) = self.tracks.get(index) else {
            return err(Mp4Error::UnknownTrack(packet.track()));
        };
        if packet.config_epoch() != track.info.config_epoch() {
            return Err(SessionError::Unsupported(
                zencodec_media::session::UnsupportedMedia::ConfigChange,
            ));
        }
        let pts = packet
            .pts()
            .ok_or(SessionError::Protocol(ProtocolViolation::MissingTimestamp))?;
        if pts.base() != track.info.time_base() {
            return Err(SessionError::Protocol(ProtocolViolation::ClockMismatch));
        }
        let dts = packet.dts().map_or(pts.ticks(), |d| d.ticks());
        if track.last_dts.is_some_and(|last| dts <= last) {
            return err(Mp4Error::DecodeOrder);
        }
        if track.entry.is_none()
            && let Some(record) = crate::av1::av1c_from_temporal_unit(packet.bytes())
        {
            let (_, e) = sample_entry(&track.info, Some(&record)).map_err(SessionError::Codec)?;
            self.tracks[index].entry = Some(e);
        }
        let track = &self.tracks[index];
        let video = track.info.kind() == TrackKind::Video;
        let sync = packet.random_access() == RandomAccess::Yes;
        let duration = packet.duration_ticks();
        let payload = packet
            .take()
            .ok_or(SessionError::Protocol(ProtocolViolation::EmptyPacket))?;
        let size =
            u32::try_from(payload.len()).map_err(|_| SessionError::Codec(Mp4Error::Overflow))?;
        if self.layout == Layout::Fragmented
            && ((video && sync) || self.fragment_bytes >= FRAGMENT_BYTES)
        {
            // Close the open fragment before a video random-access point.
            let t = &mut self.tracks[index];
            Self::close_previous(t, dts);
            self.flush_fragment(false)?;
        }
        let t = &mut self.tracks[index];
        Self::close_previous(t, dts);
        let offset = match self.layout {
            Layout::Progressive => self.out.total(),
            Layout::Fragmented => 0,
        };
        t.samples.push(MuxSample {
            offset,
            size,
            dts,
            cts: pts.ticks() - dts,
            duration,
            sync,
        });
        t.last_dts = Some(dts);
        t.end_dts = t.end_dts.max(dts + duration.unwrap_or(0) as i64).max(dts);
        match self.layout {
            Layout::Progressive => self.out.push(payload.as_bytes()),
            Layout::Fragmented => {
                self.fragment_bytes += payload.len();
                t.data.push(payload.into_vec());
            }
        }
        Ok(())
    }

    fn pending_output(&self) -> &[u8] {
        self.out.pending()
    }

    fn consume_output(&mut self, n: usize) {
        self.out.consume(n);
    }

    fn take_output(&mut self, into: &mut Vec<u8>) {
        self.out.take_into(into);
    }

    fn finish(mut self, stop: &dyn Stop) -> Res<MuxFinish> {
        stop.check()?;
        if !self.begun {
            return Err(SessionError::Protocol(ProtocolViolation::NoBegin));
        }
        let mut patches = Vec::new();
        match self.layout {
            Layout::Progressive => {
                for t in &mut self.tracks {
                    // The last sample: its own duration, else the step before.
                    let step = t.last_step;
                    if let Some(s) = t.samples.last_mut()
                        && s.duration.is_none()
                    {
                        s.duration = Some(step.unwrap_or(0));
                    }
                    if let Some(s) = t.samples.last() {
                        t.end_dts = t.end_dts.max(s.dts + s.duration.unwrap_or(0) as i64);
                    }
                }
                let mdat_end = self.out.total();
                let size = mdat_end - self.mdat_at;
                patches.push(Patch {
                    offset: self.mdat_at + 8,
                    bytes: size.to_be_bytes().to_vec(),
                });
                let moov = self.moov(true)?;
                self.out.push(&moov);
            }
            Layout::Fragmented => {
                for t in &mut self.tracks {
                    let step = t.last_step;
                    if let Some(s) = t.samples.last_mut()
                        && s.duration.is_none()
                    {
                        s.duration = Some(step.unwrap_or(0));
                    }
                }
                self.flush_fragment(true)?;
                self.write_header()?;
                if !self.header_written {
                    return err(Mp4Error::Missing("av1C"));
                }
            }
        }
        let mut done = MuxFinish::default();
        done.tail = self.out.into_remaining();
        done.patches = patches;
        Ok(done)
    }
}

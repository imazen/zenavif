//! The demuxer: a top-level box scan with range requests, `moov` and `moof`
//! parsed whole, samples read in file order.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use enough::Stop;
use zencodec::time::{FrameDuration, TimeBase, Timestamp};
use zencodec_media::container::{
    AccessPattern, DemuxEvent, DemuxProgress, Demuxer, Input, Request, SeekMode, SeekResult,
    SeekScope, SeekStatus, SeekTarget,
};
use zencodec_media::limits::MediaLimits;
use zencodec_media::packet::{Packet, RandomAccess};
use zencodec_media::session::{SessionError, UnsupportedMedia};
use zencodec_media::track::{
    AudioTrackInfo, ChannelLayout, CodecId, TrackInfo, TrackKind, VideoTrackInfo,
};
use zenpixels::Cicp;

use crate::boxes::{self, Reader, children, find};
use crate::{Mp4Error, codec_from_sample_entry};

type Res<T> = Result<T, SessionError<Mp4Error>>;

fn err<T>(e: Mp4Error) -> Res<T> {
    Err(SessionError::Codec(e))
}

/// One sample, located and timed.
#[derive(Clone, Copy, Debug)]
struct Sample {
    track: usize,
    offset: u64,
    size: u32,
    dts: i64,
    pts: i64,
    duration: u32,
    sync: bool,
    ordinal: u64,
}

/// `trex` defaults for fragments.
#[derive(Clone, Copy, Debug, Default)]
struct TrackDefaults {
    duration: u32,
    size: u32,
    flags: u32,
}

#[derive(Clone, Copy, Debug)]
struct TrackState {
    id: u32,
    /// Decode time of the next fragment sample when no `tfdt` says.
    next_dts: i64,
    next_ordinal: u64,
    defaults: TrackDefaults,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Scan,
    Emit,
    /// A seek in a fragmented file is indexing every `moof`.
    Index,
    Done,
}

/// Sans-I/O MP4 reader. See the [crate docs](crate).
#[derive(Debug)]
pub struct Mp4Demuxer {
    limits: MediaLimits,
    requests: Vec<Request>,
    tracks: Vec<TrackInfo>,
    states: Vec<TrackState>,
    /// Next top-level box.
    pos: u64,
    movie: bool,
    fragmented: bool,
    /// Samples still to emit, in file order.
    pending: VecDeque<Sample>,
    /// Every sample in file order, for seeking: from `moov` in a
    /// progressive file, from the fragment index in a fragmented one.
    all: Vec<Sample>,
    phase: Phase,
    /// Offset after `moov`, where a fragment index scan starts.
    moov_end: u64,
    /// Track states as `moov` left them, for a fragment index scan.
    initial_states: Vec<TrackState>,
    /// Every fragment is in `all`.
    indexed: bool,
    index_pos: u64,
    pending_seek: Option<SeekTarget>,
}

impl Mp4Demuxer {
    /// A reader enforcing `limits` (box sizes, tracks, sample counts, packet
    /// sizes).
    pub fn new(limits: MediaLimits) -> Self {
        Self {
            limits,
            requests: Vec::new(),
            tracks: Vec::new(),
            states: Vec::new(),
            pos: 0,
            movie: false,
            fragmented: false,
            pending: VecDeque::new(),
            all: Vec::new(),
            phase: Phase::Scan,
            moov_end: 0,
            initial_states: Vec::new(),
            indexed: false,
            index_pos: 0,
            pending_seek: None,
        }
    }

    /// Whether the file is fragmented (`moov` with `mvex`).
    pub fn is_fragmented(&self) -> bool {
        self.fragmented
    }

    fn need(&mut self, r: Request) -> Res<DemuxProgress> {
        self.requests.push(r);
        Ok(DemuxProgress::need(0))
    }

    fn scan(&mut self, input: Input<'_>) -> Res<DemuxProgress> {
        if input.offset != self.pos {
            return self.need(Request::at_least(self.pos, 16));
        }
        let b = input.bytes;
        if b.is_empty() && input.reaches_end() {
            if !self.movie {
                return err(Mp4Error::NoMovie);
            }
            self.phase = Phase::Done;
            return Ok(DemuxProgress::new(0, DemuxEvent::Done));
        }
        let h = match boxes::header(b).map_err(SessionError::Codec)? {
            Some(h) => h,
            None if input.reaches_end() => return err(Mp4Error::Truncated),
            None => return self.need(Request::at_least(self.pos, 16)),
        };
        let size = match h.size {
            Some(s) => s,
            None => match input.total_len {
                Some(total) => total - self.pos,
                None => return self.need(Request::Len),
            },
        };
        let next = self
            .pos
            .checked_add(size)
            .ok_or(SessionError::Codec(Mp4Error::Overflow))?;
        match &h.kind {
            b"moov" | b"moof" => {
                if &h.kind == b"moov" && self.movie {
                    return err(Mp4Error::Duplicate("moov"));
                }
                if &h.kind == b"moof" && !self.movie {
                    return err(Mp4Error::NoMovie);
                }
                self.limits
                    .check_header_bytes(size)
                    .map_err(SessionError::Limit)?;
                let len =
                    usize::try_from(size).map_err(|_| SessionError::Codec(Mp4Error::Overflow))?;
                if b.len() < len {
                    if input.reaches_end() {
                        return err(Mp4Error::Truncated);
                    }
                    return self.need(Request::at_least(self.pos, len));
                }
                let body = &b[h.header_len..len];
                let start = self.pos;
                self.pos = next;
                if &h.kind == b"moov" {
                    self.movie(body)?;
                    self.moov_end = next;
                    self.phase = if self.pending.is_empty() && self.fragmented {
                        Phase::Scan
                    } else {
                        Phase::Emit
                    };
                    Ok(DemuxProgress::new(len, DemuxEvent::Headers))
                } else {
                    let samples = self.fragment(body, start)?;
                    self.pending.extend(samples);
                    self.phase = Phase::Emit;
                    self.emit_or_request(len, input.total_len)
                }
            }
            b"mdat" if h.size.is_none() && !self.movie => err(Mp4Error::MovieAfterOpenMdat),
            _ => {
                if input.total_len.is_some_and(|t| next > t) {
                    return err(Mp4Error::Truncated);
                }
                self.pos = next;
                if self.movie && !self.fragmented {
                    // A progressive file's samples come from moov; nothing
                    // after it matters.
                    self.phase = Phase::Done;
                    return Ok(DemuxProgress::new(0, DemuxEvent::Done));
                }
                self.need(Request::at_least(self.pos, 16))
            }
        }
    }

    /// After consuming `consumed` bytes, ask for the next pending sample.
    fn emit_or_request(&mut self, consumed: usize, total: Option<u64>) -> Res<DemuxProgress> {
        match self.pending.front() {
            Some(s) => {
                past_end(s, total)?;
                self.requests
                    .push(Request::range(s.offset, u64::from(s.size)));
                Ok(DemuxProgress::need(consumed))
            }
            None => {
                self.phase = if self.fragmented && !self.indexed {
                    Phase::Scan
                } else {
                    Phase::Done
                };
                self.requests.push(Request::at_least(self.pos, 16));
                Ok(DemuxProgress::need(consumed))
            }
        }
    }

    fn emit(&mut self, input: Input<'_>) -> Res<DemuxProgress> {
        if let Some(s) = self.pending.front() {
            past_end(s, input.total_len)?;
        }
        let Some(&s) = self.pending.front() else {
            if self.fragmented && !self.indexed {
                self.phase = Phase::Scan;
                return self.need(Request::at_least(self.pos, 16));
            }
            self.phase = Phase::Done;
            return Ok(DemuxProgress::new(0, DemuxEvent::Done));
        };
        let len = s.size as usize;
        if input.offset != s.offset || input.bytes.len() < len {
            if input.offset == s.offset && input.reaches_end() {
                return err(Mp4Error::Truncated);
            }
            return self.need(Request::range(s.offset, u64::from(s.size)));
        }
        self.pending.pop_front();
        let mut data = Vec::new();
        data.try_reserve_exact(len)
            .map_err(|_| SessionError::Codec(Mp4Error::Overflow))?;
        data.extend_from_slice(&input.bytes[..len]);
        let tb = self.tracks[s.track].time_base();
        let packet = Packet::from_vec(data)
            .map_err(|_| SessionError::Codec(Mp4Error::EmptySample))?
            .with_track(s.track as u32)
            .with_pts(Timestamp::new(s.pts, tb))
            .with_dts(Timestamp::new(s.dts, tb))
            .with_duration_ticks(u64::from(s.duration))
            .with_random_access(if s.sync {
                RandomAccess::Yes
            } else {
                RandomAccess::No
            })
            .with_ordinal(s.ordinal)
            .with_input_offset(s.offset);
        Ok(DemuxProgress::new(len, DemuxEvent::Packet(packet)))
    }

    /// Parse `moov`: tracks, and for a progressive file every sample.
    fn movie(&mut self, body: &[u8]) -> Res<()> {
        let mvhd = find(body, b"mvhd")
            .map_err(SessionError::Codec)?
            .ok_or(SessionError::Codec(Mp4Error::Missing("mvhd")))?;
        let movie_timescale = {
            let mut r = Reader::new(mvhd);
            let (v, _) = r.full().map_err(SessionError::Codec)?;
            r.skip(if v == 1 { 16 } else { 8 })
                .map_err(SessionError::Codec)?;
            r.u32().map_err(SessionError::Codec)?
        };
        let mut samples = Vec::new();
        let mut trex: Vec<(u32, TrackDefaults)> = Vec::new();
        if let Some(mvex) = find(body, b"mvex").map_err(SessionError::Codec)? {
            self.fragmented = true;
            for child in children(mvex) {
                let (k, b) = child.map_err(SessionError::Codec)?;
                if &k == b"trex" {
                    let mut r = Reader::new(b);
                    r.full().map_err(SessionError::Codec)?;
                    let id = r.u32().map_err(SessionError::Codec)?;
                    r.u32().map_err(SessionError::Codec)?; // description index
                    let d = TrackDefaults {
                        duration: r.u32().map_err(SessionError::Codec)?,
                        size: r.u32().map_err(SessionError::Codec)?,
                        flags: r.u32().map_err(SessionError::Codec)?,
                    };
                    self.limits
                        .check_table_entries(trex.len() as u64 + 1)
                        .map_err(SessionError::Limit)?;
                    trex.push((id, d));
                }
            }
        }
        for child in children(body) {
            let (k, trak) = child.map_err(SessionError::Codec)?;
            if &k != b"trak" {
                continue;
            }
            let index = self.tracks.len();
            self.limits
                .check_tracks(index as u64 + 1)
                .map_err(SessionError::Limit)?;
            let parsed = parse_track(trak, index, movie_timescale, &self.limits, &mut samples)?;
            let defaults = trex
                .iter()
                .find(|(id, _)| *id == parsed.id)
                .map(|(_, d)| *d)
                .unwrap_or_default();
            self.states.push(TrackState {
                id: parsed.id,
                next_dts: parsed.end_dts,
                next_ordinal: parsed.samples,
                defaults,
            });
            self.tracks.push(parsed.info);
        }
        self.movie = true;
        self.initial_states = self.states.clone();
        samples.sort_by_key(|s: &Sample| s.offset);
        // A fragmented file's index extends these with every `moof`.
        self.all = samples.clone();
        self.pending = samples.into();
        Ok(())
    }

    /// One step of a fragment index scan: parse each `moof` from the end of
    /// `moov` to the end of the file into `all`, skipping everything else.
    fn index_step(&mut self, input: Input<'_>) -> Res<DemuxProgress> {
        let pos = self.index_pos;
        if input.offset != pos {
            return self.need(Request::at_least(pos, 16));
        }
        let b = input.bytes;
        if b.is_empty() && input.reaches_end() {
            self.all.sort_by_key(|s| s.offset);
            self.indexed = true;
            let target = self
                .pending_seek
                .take()
                .expect("an index scan serves a seek");
            let r = self.position(&target)?;
            return Ok(DemuxProgress::new(0, DemuxEvent::Seeked(r)));
        }
        let h = match boxes::header(b).map_err(SessionError::Codec)? {
            Some(h) => h,
            None if input.reaches_end() => return err(Mp4Error::Truncated),
            None => return self.need(Request::at_least(pos, 16)),
        };
        let size = match (h.size, input.total_len) {
            (Some(s), _) => s,
            (None, Some(total)) => total - pos,
            (None, None) => return self.need(Request::Len),
        };
        let next = pos
            .checked_add(size)
            .ok_or(SessionError::Codec(Mp4Error::Overflow))?;
        if input.total_len.is_some_and(|t| next > t) {
            return err(Mp4Error::Truncated);
        }
        if &h.kind == b"moof" {
            self.limits
                .check_header_bytes(size)
                .map_err(SessionError::Limit)?;
            let len = usize::try_from(size).map_err(|_| SessionError::Codec(Mp4Error::Overflow))?;
            if b.len() < len {
                return self.need(Request::at_least(pos, len));
            }
            let samples = self.fragment(&b[h.header_len..len], pos)?;
            self.limits
                .check_table_entries((self.all.len() + samples.len()) as u64)
                .map_err(SessionError::Limit)?;
            self.all.extend(samples);
        }
        self.index_pos = next;
        self.need(Request::at_least(next, 16))
    }

    /// Choose the sample `target` names in `all` and resume emitting there.
    fn position(&mut self, target: &SeekTarget) -> Res<SeekResult> {
        let track = target.track as usize;
        let tb = self
            .tracks
            .get(track)
            .ok_or(SessionError::Codec(Mp4Error::NoSuchSample))?
            .time_base();
        let t = target.time;
        let value = |pts: i64| {
            i128::from(pts) * i128::from(tb.numerator()) * i128::from(t.base().denominator())
        };
        let want =
            i128::from(t.ticks()) * i128::from(t.base().numerator()) * i128::from(tb.denominator());
        let candidates = self.all.iter().enumerate().filter(|(_, s)| {
            s.track == track && (s.sync || matches!(target.scope, SeekScope::Any))
        });
        let chosen = match target.mode {
            SeekMode::Nearest => candidates.min_by(|(_, a), (_, b)| {
                (value(a.pts) - want)
                    .abs()
                    .cmp(&(value(b.pts) - want).abs())
                    .then(a.pts.cmp(&b.pts))
            }),
            SeekMode::AtOrBefore => candidates
                .filter(|(_, s)| value(s.pts) <= want)
                .max_by_key(|(_, s)| s.pts),
            SeekMode::AtOrAfter => candidates
                .filter(|(_, s)| value(s.pts) >= want)
                .min_by_key(|(_, s)| s.pts),
            _ => return Err(SessionError::Unsupported(UnsupportedMedia::Seek)),
        };
        let (at, s) = chosen.ok_or(SessionError::Codec(Mp4Error::NoSuchSample))?;
        let s = *s;
        self.pending = self.all[at..].iter().copied().collect();
        self.phase = Phase::Emit;
        Ok(SeekResult::new(
            Timestamp::new(s.pts, tb),
            s.ordinal,
            if s.sync {
                RandomAccess::Yes
            } else {
                RandomAccess::No
            },
            0,
        ))
    }

    /// Parse `moof` (at file offset `moof_start`) into samples, in file
    /// order, advancing the tracks' decode times and ordinals.
    fn fragment(&mut self, body: &[u8], moof_start: u64) -> Res<Vec<Sample>> {
        let c = SessionError::Codec;
        let mut out = Vec::new();
        for child in children(body) {
            let (k, traf) = child.map_err(c)?;
            if &k != b"traf" {
                continue;
            }
            let tfhd = find(traf, b"tfhd")
                .map_err(c)?
                .ok_or(c(Mp4Error::Missing("tfhd")))?;
            let mut r = Reader::new(tfhd);
            let (_, flags) = r.full().map_err(c)?;
            let id = r.u32().map_err(c)?;
            let track = self
                .states
                .iter()
                .position(|t| t.id == id)
                .ok_or(c(Mp4Error::UnknownTrack(id)))?;
            let mut defaults = self.states[track].defaults;
            let base = if flags & 0x1 != 0 {
                r.u64().map_err(c)?
            } else {
                moof_start
            };
            if flags & 0x2 != 0 {
                r.u32().map_err(c)?; // description index
            }
            if flags & 0x8 != 0 {
                defaults.duration = r.u32().map_err(c)?;
            }
            if flags & 0x10 != 0 {
                defaults.size = r.u32().map_err(c)?;
            }
            if flags & 0x20 != 0 {
                defaults.flags = r.u32().map_err(c)?;
            }
            if let Some(tfdt) = find(traf, b"tfdt").map_err(c)? {
                let mut r = Reader::new(tfdt);
                let (v, _) = r.full().map_err(c)?;
                let t = if v == 1 {
                    r.u64().map_err(c)?
                } else {
                    u64::from(r.u32().map_err(c)?)
                };
                self.states[track].next_dts =
                    i64::try_from(t).map_err(|_| c(Mp4Error::Overflow))?;
            }
            let mut data_pos = base;
            for trun in children(traf) {
                let (k, trun) = trun.map_err(c)?;
                if &k != b"trun" {
                    continue;
                }
                let mut r = Reader::new(trun);
                let (v, flags) = r.full().map_err(c)?;
                let count = r.u32().map_err(c)?;
                // Indexing retains the samples from all earlier moofs.
                // Check their combined size before expanding this trun.
                let retained = if self.phase == Phase::Index {
                    self.all.len() as u64
                } else {
                    0
                };
                self.limits
                    .check_table_entries(retained + out.len() as u64 + u64::from(count))
                    .map_err(SessionError::Limit)?;
                if flags & 0x1 != 0 {
                    let off = i64::from(r.i32().map_err(c)?);
                    data_pos = base.checked_add_signed(off).ok_or(c(Mp4Error::Overflow))?;
                }
                let first_flags = if flags & 0x4 != 0 {
                    Some(r.u32().map_err(c)?)
                } else {
                    None
                };
                for n in 0..count {
                    let duration = if flags & 0x100 != 0 {
                        r.u32().map_err(c)?
                    } else {
                        defaults.duration
                    };
                    let size = if flags & 0x200 != 0 {
                        r.u32().map_err(c)?
                    } else {
                        defaults.size
                    };
                    let sflags = if flags & 0x400 != 0 {
                        r.u32().map_err(c)?
                    } else if n == 0 && first_flags.is_some() {
                        first_flags.unwrap_or_default()
                    } else {
                        defaults.flags
                    };
                    let cts = if flags & 0x800 != 0 {
                        let raw = r.u32().map_err(c)?;
                        if v == 0 {
                            i64::from(raw)
                        } else {
                            i64::from(raw as i32)
                        }
                    } else {
                        0
                    };
                    if size == 0 {
                        return err(Mp4Error::EmptySample);
                    }
                    self.limits
                        .check_packet_bytes(u64::from(size))
                        .map_err(SessionError::Limit)?;
                    let st = &mut self.states[track];
                    let dts = st.next_dts;
                    out.push(Sample {
                        track,
                        offset: data_pos,
                        size,
                        dts,
                        pts: dts.checked_add(cts).ok_or(c(Mp4Error::Overflow))?,
                        duration,
                        sync: sflags & 0x0001_0000 == 0,
                        ordinal: st.next_ordinal,
                    });
                    st.next_ordinal += 1;
                    st.next_dts = dts
                        .checked_add(i64::from(duration))
                        .ok_or(c(Mp4Error::Overflow))?;
                    data_pos = data_pos
                        .checked_add(u64::from(size))
                        .ok_or(c(Mp4Error::Overflow))?;
                }
            }
        }
        out.sort_by_key(|s| s.offset);
        Ok(out)
    }
}

/// A sample that ends past the end of the file is truncation, not a
/// request the driver cannot meet.
fn past_end(s: &Sample, total: Option<u64>) -> Res<()> {
    match total {
        Some(t) if s.offset.saturating_add(u64::from(s.size)) > t => err(Mp4Error::Truncated),
        _ => Ok(()),
    }
}

/// A parsed `trak`.
struct ParsedTrack {
    id: u32,
    info: TrackInfo,
    /// Samples it declared.
    samples: u64,
    /// Decode time after its last sample.
    end_dts: i64,
}

struct Entry {
    codec: CodecId,
    private: Option<Vec<u8>>,
    video: Option<VideoTrackInfo>,
    audio: Option<AudioTrackInfo>,
    codec_delay: Option<FrameDuration>,
}

fn parse_track(
    trak: &[u8],
    index: usize,
    movie_timescale: u32,
    limits: &MediaLimits,
    samples: &mut Vec<Sample>,
) -> Res<ParsedTrack> {
    let c = SessionError::Codec;
    let tkhd = find(trak, b"tkhd")
        .map_err(c)?
        .ok_or(c(Mp4Error::Missing("tkhd")))?;
    let id = {
        let mut r = Reader::new(tkhd);
        let (v, _) = r.full().map_err(c)?;
        r.skip(if v == 1 { 16 } else { 8 }).map_err(c)?;
        r.u32().map_err(c)?
    };
    let mdia = find(trak, b"mdia")
        .map_err(c)?
        .ok_or(c(Mp4Error::Missing("mdia")))?;
    let mdhd = find(mdia, b"mdhd")
        .map_err(c)?
        .ok_or(c(Mp4Error::Missing("mdhd")))?;
    let (timescale, duration) = {
        let mut r = Reader::new(mdhd);
        let (v, _) = r.full().map_err(c)?;
        if v == 1 {
            r.skip(16).map_err(c)?;
            (r.u32().map_err(c)?, r.u64().map_err(c)?)
        } else {
            r.skip(8).map_err(c)?;
            (r.u32().map_err(c)?, u64::from(r.u32().map_err(c)?))
        }
    };
    if timescale == 0 {
        return err(Mp4Error::ZeroTimescale);
    }
    let handler = {
        let hdlr = find(mdia, b"hdlr")
            .map_err(c)?
            .ok_or(c(Mp4Error::Missing("hdlr")))?;
        let mut r = Reader::new(hdlr);
        r.full().map_err(c)?;
        r.u32().map_err(c)?;
        r.fourcc().map_err(c)?
    };
    let stbl = find(mdia, b"minf")
        .map_err(c)?
        .and_then(|minf| find(minf, b"stbl").transpose())
        .transpose()
        .map_err(c)?
        .ok_or(c(Mp4Error::Missing("stbl")))?;
    let entry = sample_entry(stbl)?;
    let tb = TimeBase::new(1, timescale).map_err(|_| c(Mp4Error::ZeroTimescale))?;
    let kind = match &handler {
        b"vide" => TrackKind::Video,
        b"soun" => TrackKind::Audio,
        b"text" | b"sbtl" | b"subt" => TrackKind::Subtitle,
        _ => TrackKind::Data,
    };
    let mut builder = match (kind, entry.video, entry.audio) {
        (TrackKind::Video, Some(v), _) => TrackInfo::video(index as u32, entry.codec, tb, v),
        (TrackKind::Audio, _, Some(a)) => TrackInfo::audio(index as u32, entry.codec, tb, a),
        (k, _, _) => TrackInfo::other(index as u32, k, entry.codec, tb),
    };
    if let Some(p) = entry.private {
        builder = builder.with_codec_private(Arc::from(p));
    }
    if let Some(d) = entry.codec_delay {
        builder = builder.with_codec_delay(d);
    }
    if let Some(delay) = edit_delay(trak, movie_timescale, timescale)? {
        builder = builder.with_edit_delay_ticks(delay);
    }
    if duration != 0 && duration != u64::from(u32::MAX) && duration != u64::MAX {
        builder = builder.with_declared_duration_ticks(duration);
    }
    let (count, end_dts) = sample_table(stbl, index, limits, samples)?;
    if count > 0 {
        builder = builder.with_declared_packets(count);
    }
    let info = builder
        .build()
        .map_err(|_| c(Mp4Error::InconsistentTable("track")))?;
    Ok(ParsedTrack {
        id,
        info,
        samples: count,
        end_dts,
    })
}

/// The delay a single edit implies: its media time, less any leading empty
/// edit (converted from the movie timescale).
fn edit_delay(trak: &[u8], movie_ts: u32, media_ts: u32) -> Res<Option<i64>> {
    let c = SessionError::Codec;
    let Some(edts) = find(trak, b"edts").map_err(c)? else {
        return Ok(None);
    };
    let Some(elst) = find(edts, b"elst").map_err(c)? else {
        return Ok(None);
    };
    let mut r = Reader::new(elst);
    let (v, _) = r.full().map_err(c)?;
    let n = r.u32().map_err(c)?;
    let mut empty: u64 = 0;
    for _ in 0..n {
        let (dur, media_time) = if v == 1 {
            (r.u64().map_err(c)?, r.u64().map_err(c)? as i64)
        } else {
            (
                u64::from(r.u32().map_err(c)?),
                i64::from(r.i32().map_err(c)?),
            )
        };
        r.u32().map_err(c)?; // rate
        if media_time == -1 {
            empty = empty.saturating_add(dur);
            continue;
        }
        let empty_media = if movie_ts == 0 {
            0
        } else {
            i64::try_from(u128::from(empty) * u128::from(media_ts) / u128::from(movie_ts))
                .map_err(|_| c(Mp4Error::Overflow))?
        };
        return Ok(Some(media_time - empty_media));
    }
    Ok(None)
}

/// The first (and only) sample description.
fn sample_entry(stbl: &[u8]) -> Res<Entry> {
    let c = SessionError::Codec;
    let stsd = find(stbl, b"stsd")
        .map_err(c)?
        .ok_or(c(Mp4Error::Missing("stsd")))?;
    let mut r = Reader::new(stsd);
    r.full().map_err(c)?;
    let count = r.u32().map_err(c)?;
    if count == 0 {
        return err(Mp4Error::Missing("sample description"));
    }
    if count > 1 {
        return err(Mp4Error::MultipleSampleDescriptions);
    }
    let (fourcc, body) = children(r.rest())
        .next()
        .ok_or(c(Mp4Error::Truncated))?
        .map_err(c)?;
    let mut e = Reader::new(body);
    e.skip(8).map_err(c)?; // reserved, data reference index
    let mut entry = Entry {
        codec: codec_from_sample_entry(fourcc, None),
        private: None,
        video: None,
        audio: None,
        codec_delay: None,
    };
    if is_visual(&fourcc) {
        e.skip(16).map_err(c)?;
        let w = u32::from(e.u16().map_err(c)?);
        let h = u32::from(e.u16().map_err(c)?);
        e.skip(50).map_err(c)?;
        let mut v = VideoTrackInfo::new(w, h);
        for child in children(e.rest()) {
            let (k, b) = child.map_err(c)?;
            match &k {
                // An empty record (FFmpeg's empty_moov before the encoder
                // produced one) is no record.
                b"av1C" | b"vpcC" | b"avcC" | b"hvcC" if !b.is_empty() => {
                    entry.private = Some(b.to_vec());
                }
                b"esds" => {
                    let (_, dsi) = esds(b).map_err(c)?;
                    entry.private = dsi;
                }
                b"colr" if b.len() >= 11 && &b[..4] == b"nclx" => {
                    let mut r = Reader::new(&b[4..]);
                    let p = r.u16().map_err(c)?;
                    let t = r.u16().map_err(c)?;
                    let m = r.u16().map_err(c)?;
                    let full = r.u8().map_err(c)? & 0x80 != 0;
                    if let (Ok(p), Ok(t), Ok(m)) =
                        (u8::try_from(p), u8::try_from(t), u8::try_from(m))
                    {
                        v = v.with_color(Cicp::new(p, t, m, full));
                    }
                }
                b"pasp" if b.len() >= 8 => {
                    let mut r = Reader::new(b);
                    let (hs, vs) = (r.u32().map_err(c)?, r.u32().map_err(c)?);
                    if hs != 0 && vs != 0 && hs != vs {
                        let dw = u64::from(w) * u64::from(hs) / u64::from(vs);
                        v = v.with_display(u32::try_from(dw).unwrap_or(u32::MAX), h);
                    }
                }
                _ => {}
            }
        }
        entry.video = Some(v);
    } else if is_audio(&fourcc) {
        let version = e.u16().map_err(c)?;
        e.skip(6).map_err(c)?; // revision, vendor
        let mut channels = e.u16().map_err(c)?;
        e.skip(6).map_err(c)?; // sample size, compression id, packet size
        let mut rate = e.u32().map_err(c)? >> 16;
        match version {
            1 => e.skip(16).map_err(c)?,
            2 => {
                e.skip(4).map_err(c)?;
                let r64 = f64::from_bits(e.u64().map_err(c)?);
                rate = if r64.is_finite() && r64 > 0.0 && r64 < 4.0e9 {
                    r64 as u32
                } else {
                    0
                };
                channels = u16::try_from(e.u32().map_err(c)?).unwrap_or(0);
                e.skip(20).map_err(c)?;
            }
            _ => {}
        }
        for child in children(e.rest()) {
            let (k, b) = child.map_err(c)?;
            match &k {
                b"dOps" => {
                    let (head, pre_skip, ch) = opus_head(b).map_err(c)?;
                    entry.private = Some(head);
                    channels = u16::from(ch);
                    rate = 48_000;
                    entry.codec_delay = FrameDuration::new(u64::from(pre_skip), 48_000).ok();
                }
                b"esds" => {
                    let (oti, asc) = esds(b).map_err(c)?;
                    entry.codec = codec_from_sample_entry(fourcc, Some(oti));
                    entry.private = asc;
                }
                b"dfLa" if b.len() >= 4 => {
                    let mut p = b"fLaC".to_vec();
                    p.extend_from_slice(&b[4..]);
                    entry.private = Some(p);
                }
                _ => {}
            }
        }
        entry.audio = Some(AudioTrackInfo::new(rate, ChannelLayout::new(channels)));
    }
    Ok(entry)
}

fn is_visual(fourcc: &[u8; 4]) -> bool {
    matches!(
        fourcc,
        b"av01" | b"vp09" | b"vp08" | b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"mp4v" | b"jpeg"
    )
}

fn is_audio(fourcc: &[u8; 4]) -> bool {
    matches!(
        fourcc,
        b"Opus" | b"mp4a" | b"fLaC" | b"ac-3" | b"ec-3" | b"ipcm" | b"lpcm" | b"sowt" | b"twos"
    )
}

/// `dOps` (big-endian) as an `OpusHead` (little-endian, RFC 7845), with the
/// pre-skip and channel count.
fn opus_head(dops: &[u8]) -> Result<(Vec<u8>, u16, u8), Mp4Error> {
    let mut r = Reader::new(dops);
    r.u8()?; // version 0
    let channels = r.u8()?;
    let pre_skip = r.u16()?;
    let rate = r.u32()?;
    let gain = r.u16()?;
    let family = r.u8()?;
    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(channels);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&rate.to_le_bytes());
    head.extend_from_slice(&gain.to_le_bytes());
    head.push(family);
    if family != 0 {
        // Stream count, coupled count, then the mapping: same order.
        let table = r.take(2 + usize::from(channels))?;
        head.extend_from_slice(table);
    }
    Ok((head, pre_skip, channels))
}

/// An `esds`: the object type indication and the decoder-specific info.
fn esds(b: &[u8]) -> Result<(u8, Option<Vec<u8>>), Mp4Error> {
    fn descriptor<'a>(r: &mut Reader<'a>) -> Result<(u8, Reader<'a>), Mp4Error> {
        let tag = r.u8()?;
        let mut len = 0usize;
        for _ in 0..4 {
            let b = r.u8()?;
            len = (len << 7) | usize::from(b & 0x7F);
            if b & 0x80 == 0 {
                break;
            }
        }
        Ok((tag, Reader::new(r.take(len)?)))
    }
    let mut r = Reader::new(b);
    r.full()?;
    let (tag, mut es) = descriptor(&mut r)?;
    if tag != 0x03 {
        return Err(Mp4Error::Missing("ES_Descriptor"));
    }
    es.u16()?;
    let flags = es.u8()?;
    if flags & 0x80 != 0 {
        es.u16()?;
    }
    if flags & 0x40 != 0 {
        let n = es.u8()?;
        es.skip(usize::from(n))?;
    }
    if flags & 0x20 != 0 {
        es.u16()?;
    }
    while es.remaining() > 0 {
        let (tag, mut d) = descriptor(&mut es)?;
        if tag != 0x04 {
            continue;
        }
        let oti = d.u8()?;
        d.skip(12)?;
        while d.remaining() > 0 {
            let (tag, info) = descriptor(&mut d)?;
            if tag == 0x05 {
                return Ok((oti, Some(info.rest().to_vec())));
            }
        }
        return Ok((oti, None));
    }
    Err(Mp4Error::Missing("DecoderConfigDescriptor"))
}

/// Expand a progressive sample table into `out`; the sample count and the
/// decode time after the last sample.
fn sample_table(
    stbl: &[u8],
    track: usize,
    limits: &MediaLimits,
    out: &mut Vec<Sample>,
) -> Res<(u64, i64)> {
    let c = SessionError::Codec;
    let table = |n: u32| {
        limits
            .check_table_entries(u64::from(n))
            .map_err(SessionError::Limit)
    };
    // Sizes.
    let sizes: Vec<u32> = match find(stbl, b"stsz").map_err(c)? {
        Some(stsz) => {
            let mut r = Reader::new(stsz);
            r.full().map_err(c)?;
            let fixed = r.u32().map_err(c)?;
            let n = r.u32().map_err(c)?;
            table(n)?;
            // Every track contributes to the same retained seek index.
            // Bound that total before even allocating the sizes table.
            limits
                .check_table_entries(out.len() as u64 + u64::from(n))
                .map_err(SessionError::Limit)?;
            if fixed != 0 {
                vec![fixed; n as usize]
            } else {
                (0..n)
                    .map(|_| r.u32())
                    .collect::<Result<_, _>>()
                    .map_err(c)?
            }
        }
        None => return Ok((0, 0)),
    };
    if sizes.is_empty() {
        return Ok((0, 0));
    }
    // Chunk offsets.
    let offsets: Vec<u64> = if let Some(stco) = find(stbl, b"stco").map_err(c)? {
        let mut r = Reader::new(stco);
        r.full().map_err(c)?;
        let n = r.u32().map_err(c)?;
        table(n)?;
        (0..n)
            .map(|_| r.u32().map(u64::from))
            .collect::<Result<_, _>>()
            .map_err(c)?
    } else if let Some(co64) = find(stbl, b"co64").map_err(c)? {
        let mut r = Reader::new(co64);
        r.full().map_err(c)?;
        let n = r.u32().map_err(c)?;
        table(n)?;
        (0..n)
            .map(|_| r.u64())
            .collect::<Result<_, _>>()
            .map_err(c)?
    } else {
        return err(Mp4Error::Missing("stco"));
    };
    // Samples per chunk.
    let stsc = find(stbl, b"stsc")
        .map_err(c)?
        .ok_or(c(Mp4Error::Missing("stsc")))?;
    let mut runs = Vec::new();
    {
        let mut r = Reader::new(stsc);
        r.full().map_err(c)?;
        let n = r.u32().map_err(c)?;
        table(n)?;
        for _ in 0..n {
            let first = r.u32().map_err(c)?;
            let per = r.u32().map_err(c)?;
            r.u32().map_err(c)?; // description index (one description)
            runs.push((first, per));
        }
    }
    // Times.
    let stts = find(stbl, b"stts")
        .map_err(c)?
        .ok_or(c(Mp4Error::Missing("stts")))?;
    let mut deltas = Vec::new();
    {
        let mut r = Reader::new(stts);
        r.full().map_err(c)?;
        let n = r.u32().map_err(c)?;
        table(n)?;
        for _ in 0..n {
            deltas.push((r.u32().map_err(c)?, r.u32().map_err(c)?));
        }
    }
    let mut cts = Vec::new();
    if let Some(ctts) = find(stbl, b"ctts").map_err(c)? {
        let mut r = Reader::new(ctts);
        let (v, _) = r.full().map_err(c)?;
        let n = r.u32().map_err(c)?;
        table(n)?;
        for _ in 0..n {
            let count = r.u32().map_err(c)?;
            let raw = r.u32().map_err(c)?;
            // Version 0 is unsigned on paper; writers put negative offsets
            // there too, and every reader takes them as signed.
            let _ = v;
            cts.push((count, i64::from(raw as i32)));
        }
    }
    let sync: Option<Vec<u32>> = match find(stbl, b"stss").map_err(c)? {
        Some(stss) => {
            let mut r = Reader::new(stss);
            r.full().map_err(c)?;
            let n = r.u32().map_err(c)?;
            table(n)?;
            Some(
                (0..n)
                    .map(|_| r.u32())
                    .collect::<Result<_, _>>()
                    .map_err(c)?,
            )
        }
        None => None,
    };

    let n = sizes.len();
    let start = out.len();
    out.try_reserve(n).map_err(|_| c(Mp4Error::Overflow))?;
    // Locate: walk chunks.
    let mut sample = 0usize;
    for (ri, &(first, per)) in runs.iter().enumerate() {
        let last = runs
            .get(ri + 1)
            .map_or(offsets.len() as u32, |&(next, _)| next.saturating_sub(1));
        if first == 0 || first > last + 1 {
            return err(Mp4Error::InconsistentTable("stsc"));
        }
        for chunk in first..=last {
            let mut off = *offsets
                .get(chunk as usize - 1)
                .ok_or(c(Mp4Error::InconsistentTable("stco")))?;
            for _ in 0..per {
                let Some(&size) = sizes.get(sample) else {
                    return err(Mp4Error::InconsistentTable("stsc runs past stsz"));
                };
                if size == 0 {
                    return err(Mp4Error::EmptySample);
                }
                limits
                    .check_packet_bytes(u64::from(size))
                    .map_err(SessionError::Limit)?;
                out.push(Sample {
                    track,
                    offset: off,
                    size,
                    dts: 0,
                    pts: 0,
                    duration: 0,
                    sync: true,
                    ordinal: sample as u64,
                });
                off = off
                    .checked_add(u64::from(size))
                    .ok_or(c(Mp4Error::Overflow))?;
                sample += 1;
            }
        }
    }
    if sample != n {
        return err(Mp4Error::InconsistentTable(
            "chunks hold fewer samples than stsz",
        ));
    }
    // Time.
    let mut dts: i64 = 0;
    let mut i = start;
    for &(count, delta) in &deltas {
        for _ in 0..count {
            let Some(s) = out.get_mut(i) else {
                return err(Mp4Error::InconsistentTable("stts runs past stsz"));
            };
            s.dts = dts;
            s.pts = dts;
            s.duration = delta;
            dts = dts
                .checked_add(i64::from(delta))
                .ok_or(c(Mp4Error::Overflow))?;
            i += 1;
        }
    }
    if i != out.len() {
        return err(Mp4Error::InconsistentTable(
            "stts covers fewer samples than stsz",
        ));
    }
    let mut i = start;
    for &(count, offset) in &cts {
        for _ in 0..count {
            let Some(s) = out.get_mut(i) else {
                return err(Mp4Error::InconsistentTable("ctts runs past stsz"));
            };
            s.pts = s.dts.checked_add(offset).ok_or(c(Mp4Error::Overflow))?;
            i += 1;
        }
    }
    if let Some(sync) = sync {
        for s in &mut out[start..] {
            s.sync = false;
        }
        for k in sync {
            let s = (k as usize)
                .checked_sub(1)
                .and_then(|k| out.get_mut(start + k))
                .ok_or(c(Mp4Error::InconsistentTable(
                    "stss names a sample past stsz",
                )))?;
            s.sync = true;
        }
    }
    Ok((n as u64, dts))
}

impl Demuxer for Mp4Demuxer {
    type Error = Mp4Error;

    fn step(&mut self, input: Input<'_>, stop: &dyn Stop) -> Res<DemuxProgress> {
        stop.check()?;
        self.requests.clear();
        match self.phase {
            Phase::Scan => self.scan(input),
            Phase::Emit => self.emit(input),
            Phase::Index => self.index_step(input),
            Phase::Done => Ok(DemuxProgress::new(0, DemuxEvent::Done)),
        }
    }

    fn requested(&self) -> &[Request] {
        &self.requests
    }

    fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    fn access_pattern(&self) -> AccessPattern {
        // moov may follow mdat, and samples are read by offset.
        AccessPattern::RandomAccess
    }

    /// Seek by time. A progressive file seeks in its `moov` tables at once.
    /// A fragmented file's first seek indexes every `moof` from the end of
    /// `moov` to the end of the file (range requests; `mdat` is skipped),
    /// with exact decode times and per-track ordinals; later seeks reuse
    /// the index.
    fn seek(&mut self, target: &SeekTarget) -> Res<SeekStatus> {
        if !self.movie {
            return err(Mp4Error::HeadersFirst);
        }
        if self.fragmented && !self.indexed {
            self.pending_seek = Some(*target);
            self.pending.clear();
            self.states = self.initial_states.clone();
            self.index_pos = self.moov_end;
            self.phase = Phase::Index;
            return Ok(SeekStatus::Scanning);
        }
        Ok(SeekStatus::Positioned(self.position(target)?))
    }
}

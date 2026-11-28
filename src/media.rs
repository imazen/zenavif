//! AV1 video sessions for `zencodec-media`: rav1d-safe behind
//! [`VideoDecoder`] and, with the `media-encode` feature, zenrav1e behind
//! `VideoEncoder` (`Av1VideoEncoder`).
//!
//! [`Av1VideoDecoder`] takes AV1 temporal units as packets (from IVF, WebM,
//! MP4) and returns refcounted frames that map rav1d's own picture buffers
//! without copying; a frame stays valid after the decoder resets or drops.
//!
//! - **Configuration.** `configure` takes the track's `av1C` record when the
//!   container has one; its `configOBUs` (the sequence header) are prepended
//!   to the first packet of the epoch, and again after a `reset`, so decoding
//!   can start at a random-access point that does not repeat them.
//! - **Timing.** Packets carry their presentation timestamp, duration and
//!   ordinal through rav1d unchanged. Frames retain the clock, epoch,
//!   container color and input offset captured when their packet was accepted,
//!   even when a later configuration is installed before output is received.
//!   Missing timestamps stay missing.
//! - **Interpretation.** Coded and render size, subsampling, chroma siting
//!   (`chroma_sample_position`, unknown kept), sample depth (10- and 12-bit
//!   codes LSB-aligned in 16-bit words), plane roles (identity matrix planes
//!   are G, B, R) and colour claims: the bitstream's raw CICP, the
//!   container's, and the resolved one (bitstream fields, with the
//!   container filling those the bitstream leaves unspecified).
//! - **Backpressure.** One accepted packet may wait inside the session while
//!   rav1d holds output; `send` then answers `ReceivePending` until a
//!   `receive` makes room.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use enough::Stop;
use zencodec::plane::{AnyPlane, PlanarLayout, PlaneEncoding, PlaneRef, PlaneSpec};
use zencodec::time::Timestamp;
use zencodec_media::frame::{
    ColorClaim, Frame, FrameGeometry, FrameInterpretation, FrameTiming, MapError, MappedPlanes,
    Picture, PlaneRole, Presentation,
};
use zencodec_media::packet::Packet;
use zencodec_media::session::{
    CodecConfig, ProtocolViolation, Receive, SendStatus, SessionError, UnsupportedMedia,
    VideoDecoder, VideoStreamInfo,
};
use zencodec_media::track::CodecId;
use zenpixels::sample::{ChromaSiting, ChromaSubsampling as Subsampling, SampleEncoding};
use zenpixels::{Cicp, SampleType};

/// Errors from the AV1 sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Av1SessionError {
    /// rav1d refused the stream.
    Decode(rav1d_safe::Error),
    /// A bit depth other than 8, 10 or 12.
    UnsupportedBitDepth(u8),
    /// Encoder settings the AV1 encoder cannot honour.
    Configuration(String),
    /// The encoder failed.
    Encode(String),
    /// A frame whose size, layout, depth or siting differs from the
    /// encoder's settings: no conversion is implicit.
    InputMismatch,
    /// A presentation timestamp not later than the previous frame's.
    NonMonotonicTimestamp,
    /// A native decoded presentation lost its accepted packet's context.
    MissingPacketContext,
    /// rav1d refused the stream, retaining its native error and location trace.
    ///
    /// Shared ownership preserves the trace when the session error is cloned.
    /// New native failures use this variant; `Decode` remains available for
    /// callers that constructed untraced errors with the earlier API.
    DecodeTraced(Arc<whereat::At<rav1d_safe::Error>>),
}

impl core::fmt::Display for Av1SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "AV1 decode: {e}"),
            Self::DecodeTraced(e) => write!(f, "AV1 decode: {e}"),
            Self::UnsupportedBitDepth(b) => write!(f, "unsupported AV1 bit depth {b}"),
            Self::Configuration(m) => write!(f, "AV1 encoder settings: {m}"),
            Self::Encode(m) => write!(f, "AV1 encode: {m}"),
            Self::InputMismatch => f.write_str("frame does not match the AV1 encoder's settings"),
            Self::NonMonotonicTimestamp => {
                f.write_str("frame timestamp is not later than the previous one")
            }
            Self::MissingPacketContext => {
                f.write_str("decoded AV1 presentation has no accepted packet context")
            }
        }
    }
}

impl std::error::Error for Av1SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            Self::DecodeTraced(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

type Res<T> = Result<T, SessionError<Av1SessionError>>;

fn decode_err(e: whereat::At<rav1d_safe::Error>) -> SessionError<Av1SessionError> {
    // A cancelled decode is the caller's stop, not a stream error.
    match e.error() {
        rav1d_safe::Error::Cancelled => SessionError::Cancelled(enough::StopReason::Cancelled),
        _ => SessionError::Codec(Av1SessionError::DecodeTraced(Arc::new(e.at()))),
    }
}

/// The `configOBUs` of an `av1C` record (ISO/IEC 14496-12 AV1 binding):
/// everything after its 4-byte header. A bare sequence header (no `av1C`
/// marker bit) is taken whole.
/// What each AV1 plane holds: G, B, R under the identity matrix (AV1 spec
/// 6.4.2), Y, Cb, Cr otherwise.
fn plane_roles(subsampling: Subsampling, matrix_coefficients: u8) -> Vec<PlaneRole> {
    match (subsampling, matrix_coefficients) {
        (Subsampling::Mono, _) => vec![PlaneRole::Luma],
        (_, 0) => vec![PlaneRole::Green, PlaneRole::Blue, PlaneRole::Red],
        _ => vec![PlaneRole::Luma, PlaneRole::Cb, PlaneRole::Cr],
    }
}

/// Length of a temporal delimiter OBU at the start of `b`, or 0. Container
/// configuration OBUs go after it: a temporal unit starts with its
/// delimiter.
fn leading_temporal_delimiter(b: &[u8]) -> usize {
    const OBU_TEMPORAL_DELIMITER: u8 = 2;
    let Some(&header) = b.first() else { return 0 };
    // A delimiter has no payload: forbidden bit clear, type 2, a size
    // field, size 0.
    if header & 0x80 != 0 || (header >> 3) & 0xF != OBU_TEMPORAL_DELIMITER || header & 0x2 == 0 {
        return 0;
    }
    let mut i = 1 + usize::from(header & 0x4 != 0);
    let (mut nonzero, mut shift) = (false, 0u32);
    loop {
        let Some(&byte) = b.get(i) else { return 0 };
        i += 1;
        // Only zero is meaningful for a temporal delimiter. Testing the
        // payload bits avoids overflow and shifts past usize on 32-bit.
        nonzero |= byte & 0x7F != 0;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 56 {
            return 0;
        }
    }
    if nonzero { 0 } else { i }
}

fn config_obus(private: &[u8]) -> Option<&[u8]> {
    match private.first() {
        // marker (1) + version (7 bits) = 1: an av1C record.
        Some(0x81) => private.get(4..).filter(|o| !o.is_empty()),
        Some(_) => Some(private),
        None => None,
    }
}

/// rav1d's AV1 decoder as a [`VideoDecoder`] session.
pub struct Av1VideoDecoder {
    dec: rav1d_safe::Decoder,
    owned_stop: Option<Arc<dyn Stop>>,
    config: Option<Arc<CodecConfig>>,
    max_extent: Option<(u32, u32)>,
    expected_av1c: Option<(u8, Subsampling, ChromaSiting)>,
    expected_gbr: Option<bool>,
    held_caller_pictures: Arc<AtomicUsize>,
    max_caller_held: Option<usize>,
    /// Sequence-header OBUs still to prepend to the next packet.
    prefix: Option<Arc<[u8]>>,
    /// An accepted packet rav1d did not take yet.
    stash: Option<rav1d_safe::Packet>,
    /// `end_input` was called; rav1d is told once the stash is empty.
    ended: bool,
    told_end: bool,
    next_ordinal: u64,
    info: Option<VideoStreamInfo>,
}

/// Travels through native input, reference and presentation ownership. Packet
/// ordinals are caller provenance, not a map key or a retirement watermark.
struct PacketContext {
    config: Arc<CodecConfig>,
    ordinal: u64,
    input_offset: Option<u64>,
}

impl core::fmt::Debug for Av1VideoDecoder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Av1VideoDecoder")
            .field("configured", &self.config.is_some())
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl Av1VideoDecoder {
    /// A session with rav1d's default settings.
    ///
    /// # Errors
    ///
    /// When rav1d cannot start (thread or allocation failure).
    pub fn new() -> Result<Self, Av1SessionError> {
        Self::with_settings(rav1d_safe::Settings::default())
    }

    /// A session with explicit rav1d settings (threads, strictness, film
    /// grain, operating point and native picture-plane admission).
    ///
    /// `max_picture_bytes` admits native plane allocations across reference
    /// and caller-retained pictures before allocating their buffers. It does
    /// not bound input, codec workspace, metadata or allocator overhead.
    /// Native refusal retains its location trace through this adapter.
    ///
    /// # Errors
    ///
    /// When rav1d cannot start.
    pub fn with_settings(settings: rav1d_safe::Settings) -> Result<Self, Av1SessionError> {
        let dec = rav1d_safe::Decoder::with_settings(settings)
            .map_err(|e| Av1SessionError::DecodeTraced(Arc::new(e.at())))?;
        Ok(Self {
            dec,
            owned_stop: None,
            config: None,
            max_extent: None,
            expected_av1c: None,
            expected_gbr: None,
            held_caller_pictures: Arc::new(AtomicUsize::new(0)),
            max_caller_held: None,
            prefix: None,
            stash: None,
            ended: false,
            told_end: false,
            next_ordinal: 0,
            info: None,
        })
    }

    pub(crate) fn with_max_extent(mut self, extent: (u32, u32)) -> Self {
        self.max_extent = Some(extent);
        self
    }

    pub(crate) fn with_expected_av1c(
        mut self,
        hint: (u8, Subsampling, ChromaSiting),
        expected_gbr: Option<bool>,
    ) -> Self {
        self.expected_av1c = Some(hint);
        self.expected_gbr = expected_gbr;
        self
    }

    pub(crate) fn with_max_caller_held(mut self, max_held: usize) -> Self {
        self.max_caller_held = Some(max_held.max(1));
        self
    }

    /// Install an owned stop that rav1d can poll while decoding.
    ///
    /// This supplements the borrowed pulse on each session call and is kept
    /// across reset. Replace it with `Arc::new(enough::Unstoppable)` to clear.
    pub fn with_stop(mut self, stop: Arc<dyn Stop>) -> Self {
        self.dec.set_stop(Some(Arc::clone(&stop)));
        self.owned_stop = Some(stop);
        self
    }

    fn arm_prefix(&mut self) {
        self.prefix = self
            .config
            .as_ref()
            .and_then(|c| c.private.as_ref())
            .and_then(|p| config_obus(p).map(Arc::from));
    }

    /// Hand the stashed packet to rav1d; `true` when the stash is empty.
    fn flush_stash(&mut self) -> Res<bool> {
        let Some(p) = self.stash.as_mut() else {
            return Ok(true);
        };
        let result = self.dec.send_packet(p);
        if let Some(owned) = &self.owned_stop {
            owned.check()?;
        }
        match result.map_err(decode_err)? {
            rav1d_safe::SendStatus::Accepted => {
                self.stash = None;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn tell_end_if_ready(&mut self) -> Res<()> {
        if self.ended && !self.told_end && self.flush_stash()? {
            self.dec.end_input();
            self.told_end = true;
        }
        Ok(())
    }

    fn frame(&mut self, f: rav1d_safe::Frame) -> Res<Frame> {
        let context = f
            .context()
            .and_then(|value| value.downcast_ref::<PacketContext>())
            .ok_or(SessionError::Codec(Av1SessionError::MissingPacketContext))?;
        let config = &context.config;
        let bits = f.bit_depth();
        let encoding = match bits {
            8 => SampleEncoding::new(SampleType::U8, 8, 0).expect("valid 8-bit encoding"),
            10 | 12 => SampleEncoding::new(SampleType::U16, bits, 0).expect("valid depth"),
            b => return Err(SessionError::Codec(Av1SessionError::UnsupportedBitDepth(b))),
        };
        let raw = f.raw_color_info();
        let subsampling = match f.pixel_layout() {
            rav1d_safe::PixelLayout::I400 => Subsampling::Mono,
            rav1d_safe::PixelLayout::I420 => Subsampling::Yuv420,
            rav1d_safe::PixelLayout::I422 => Subsampling::Yuv422,
            _ => Subsampling::Yuv444,
        };
        let siting = match raw.chroma_sample_position {
            1 => ChromaSiting::Left,
            2 => ChromaSiting::TopLeft,
            _ => ChromaSiting::Unknown,
        };
        let (w, h) = (f.width(), f.height());
        if let Some((mw, mh)) = self.max_extent
            && (w > mw || h > mh)
        {
            return Err(SessionError::Unsupported(UnsupportedMedia::Codec));
        }
        let render = Some(f.render_size()).filter(|&r| r != (w, h));
        let mut geometry = FrameGeometry::full(w, h);
        geometry.render = render;
        let bitstream = Cicp::new(
            raw.primaries,
            raw.transfer_characteristics,
            raw.matrix_coefficients,
            raw.full_range,
        );
        let container = config.video.and_then(|v| v.color);
        let resolved = resolve_color(bitstream, container);
        let roles = plane_roles(subsampling, resolved.matrix_coefficients);
        let layout = PlanarLayout::new(
            roles
                .into_iter()
                .map(|role| PlaneSpec::new(role, PlaneEncoding::Code(encoding)))
                .collect(),
            subsampling,
            siting,
        )
        .map_err(|_| SessionError::Codec(Av1SessionError::InputMismatch))?;
        let interpretation = FrameInterpretation::new(geometry, layout)
            .with_color(ColorClaim::new(Some(bitstream), container, Some(resolved)))
            .with_config_epoch(config.epoch);
        // Frames under one interpretation share one Arc.
        let interpretation = match &self.info {
            Some(i) if *i.interpretation == interpretation => Arc::clone(&i.interpretation),
            _ => {
                let a = Arc::new(interpretation);
                self.info = Some(VideoStreamInfo::new(Arc::clone(&a)));
                a
            }
        };
        let pts = Some(f.timestamp())
            .filter(|&t| t != i64::MIN)
            .map(|t| Timestamp::new(t, config.time_base));
        let mut timing = FrameTiming::default();
        timing.pts = pts;
        if let Some(d) = u64::try_from(f.duration()).ok().filter(|&d| d > 0) {
            timing = timing.with_duration_ticks(d);
        }
        timing = timing.with_packet(Some(context.ordinal), context.input_offset);
        let mut presentation = Presentation::default();
        presentation.keyframe = f.is_keyframe();
        presentation.show_existing = f.is_show_existing();
        if let Some((eb, es, ec)) = self.expected_av1c
            && (bits != eb || subsampling != es || siting != ec)
        {
            return Err(SessionError::Unsupported(UnsupportedMedia::Codec));
        }
        if let Some(want_gbr) = self.expected_gbr
            && (resolved.matrix_coefficients == 0) != want_gbr
        {
            return Err(SessionError::Unsupported(UnsupportedMedia::Codec));
        }
        let lease = if let Some(max_held) = self.max_caller_held {
            let mut cur = self.held_caller_pictures.load(Ordering::Acquire);
            loop {
                if cur >= max_held {
                    return Err(SessionError::Limit(
                        zencodec_media::limits::MediaLimitExceeded::check(
                            zencodec::LimitKind::Frames,
                            (cur + 1) as u64,
                            max_held as u64,
                        )
                        .expect_err("caller held frames exceeded"),
                    ));
                }
                match self.held_caller_pictures.compare_exchange_weak(
                    cur,
                    cur + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break Some(HeldLease(Arc::clone(&self.held_caller_pictures))),
                    Err(actual) => cur = actual,
                }
            }
        } else {
            None
        };
        Ok(Frame::new(
            Arc::new(Av1Picture {
                frame: f,
                _lease: lease,
            }),
            interpretation,
            timing,
            presentation,
        ))
    }
}

/// The bitstream's claim, with the container filling the fields the
/// bitstream leaves unspecified (code 2). The range flag is always coded in
/// AV1, so it comes from the bitstream.
fn resolve_color(bitstream: Cicp, container: Option<Cicp>) -> Cicp {
    let Some(c) = container else {
        return bitstream;
    };
    let pick = |b: u8, c: u8| if b == 2 { c } else { b };
    Cicp::new(
        pick(bitstream.color_primaries, c.color_primaries),
        pick(
            bitstream.transfer_characteristics,
            c.transfer_characteristics,
        ),
        pick(bitstream.matrix_coefficients, c.matrix_coefficients),
        bitstream.full_range,
    )
}

impl VideoDecoder for Av1VideoDecoder {
    type Error = Av1SessionError;

    fn configure(&mut self, config: &CodecConfig, pulse: Option<&dyn zencodec::Pulse>) -> Res<()> {
        let pulse = pulse.unwrap_or(&zencodec::NoPulse);
        pulse.check()?;
        if let Some(owned) = &self.owned_stop {
            owned.check()?;
        }
        if config.codec != CodecId::Av1 {
            return Err(SessionError::Unsupported(UnsupportedMedia::Codec));
        }
        self.config = Some(Arc::new(config.clone()));
        self.arm_prefix();
        Ok(())
    }

    fn send(
        &mut self,
        packet: &mut Packet,
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Res<SendStatus> {
        let pulse = pulse.unwrap_or(&zencodec::NoPulse);
        pulse.check()?;
        if let Some(owned) = &self.owned_stop {
            owned.check()?;
        }
        let Some(config) = &self.config else {
            return Err(SessionError::Protocol(ProtocolViolation::NotConfigured));
        };
        if self.ended {
            return Err(SessionError::Protocol(ProtocolViolation::SendAfterEnd));
        }
        if packet.config_epoch() != config.epoch {
            return Err(SessionError::Protocol(ProtocolViolation::EpochMismatch));
        }
        if [packet.pts(), packet.dts()]
            .into_iter()
            .flatten()
            .any(|t| t.base() != config.time_base)
        {
            return Err(SessionError::Protocol(ProtocolViolation::ClockMismatch));
        }
        let ordinal = packet.ordinal().unwrap_or(self.next_ordinal);
        // rav1d reserves MIN for an absent timestamp, zero for an absent
        // duration, and negative offsets for absent ordinals. Refuse metadata
        // it cannot carry instead of silently changing the presentation.
        if packet.pts().is_some_and(|t| t.ticks() == i64::MIN)
            || packet
                .duration_ticks()
                .is_some_and(|d| d == 0 || d > i64::MAX as u64)
            || ordinal > i64::MAX as u64
            || packet.discard().is_some()
            || !packet.additions().is_empty()
        {
            return Err(SessionError::Unsupported(UnsupportedMedia::Container));
        }
        if !self.flush_stash()? {
            return Ok(SendStatus::ReceivePending);
        }
        let pts = packet.pts().map_or(i64::MIN, |t| t.ticks());
        let duration = packet.duration_ticks().unwrap_or(0) as i64;
        let context = PacketContext {
            config: Arc::clone(self.config.as_ref().expect("validated configuration")),
            ordinal,
            input_offset: packet.input_offset(),
        };
        let payload = packet
            .take()
            .ok_or(SessionError::Protocol(ProtocolViolation::EmptyPacket))?;
        self.next_ordinal = ordinal + 1;
        let bytes = match self.prefix.take() {
            Some(prefix) => {
                let payload = payload.as_bytes();
                let td = leading_temporal_delimiter(payload);
                let mut v = Vec::with_capacity(prefix.len() + payload.len());
                v.extend_from_slice(&payload[..td]);
                v.extend_from_slice(&prefix);
                v.extend_from_slice(&payload[td..]);
                v
            }
            None => payload.into_vec(),
        };
        let mut p = rav1d_safe::Packet::new(bytes)
            .map_err(decode_err)?
            .with_timestamp(pts)
            .with_duration(duration)
            .with_offset(ordinal as i64)
            .with_context(Arc::new(context));
        let result = self.dec.send_packet(&mut p);
        if let Some(owned) = &self.owned_stop {
            owned.check()?;
        }
        match result.map_err(decode_err)? {
            rav1d_safe::SendStatus::Accepted => {}
            // Accepted by the session; rav1d takes it once output drains.
            _ => self.stash = Some(p),
        }
        Ok(SendStatus::Accepted)
    }

    fn receive(&mut self, pulse: Option<&dyn zencodec::Pulse>) -> Res<Receive<Frame>> {
        let pulse = pulse.unwrap_or(&zencodec::NoPulse);
        pulse.check()?;
        if let Some(owned) = &self.owned_stop {
            owned.check()?;
        }
        if self.config.is_none() {
            return Err(SessionError::Protocol(ProtocolViolation::NotConfigured));
        }
        loop {
            self.tell_end_if_ready()?;
            let result = self.dec.receive();
            if let Some(owned) = &self.owned_stop {
                owned.check()?;
            }
            match result.map_err(decode_err)? {
                rav1d_safe::ReceiveStatus::Frame(f) => return Ok(Receive::Ready(self.frame(f)?)),
                rav1d_safe::ReceiveStatus::EndOfStream => return Ok(Receive::EndOfStream),
                _ => {
                    // Room may have opened for a stashed packet.
                    if self.stash.is_some() && self.flush_stash()? {
                        continue;
                    }
                    if self.ended && !self.told_end {
                        // The stash still waits on output rav1d has not
                        // produced; nothing more can happen.
                        return Ok(Receive::NeedInput);
                    }
                    return Ok(Receive::NeedInput);
                }
            }
        }
    }

    fn end_input(&mut self, pulse: Option<&dyn zencodec::Pulse>) -> Res<()> {
        if let Some(p) = pulse {
            p.check()?;
        }
        if let Some(owned) = &self.owned_stop {
            owned.check()?;
        }
        self.ended = true;
        self.tell_end_if_ready()
    }

    fn reset(&mut self, pulse: Option<&dyn zencodec::Pulse>) -> Res<()> {
        if let Some(p) = pulse {
            p.check()?;
        }
        if let Some(owned) = &self.owned_stop {
            owned.check()?;
        }
        self.dec.reset();
        self.stash = None;
        self.ended = false;
        self.told_end = false;
        self.next_ordinal = 0;
        self.arm_prefix();
        Ok(())
    }

    fn stream_info(&self) -> Option<&VideoStreamInfo> {
        self.info.as_ref()
    }
}

struct HeldLease(Arc<AtomicUsize>);

impl Drop for HeldLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A rav1d picture as a zencodec-media [`Picture`]: refcounted, mapped in
/// place.
struct Av1Picture {
    frame: rav1d_safe::Frame,
    _lease: Option<HeldLease>,
}

impl Picture for Av1Picture {
    fn map(&self) -> Result<Box<dyn MappedPlanes + '_>, MapError> {
        let bits = self.frame.bit_depth();
        Ok(match self.frame.planes() {
            rav1d_safe::Planes::Depth8(p) => Box::new(Planes8 {
                views: [Some(p.y()), p.u(), p.v()],
            }),
            rav1d_safe::Planes::Depth16(p) => Box::new(Planes16 {
                views: [Some(p.y()), p.u(), p.v()],
                encoding: SampleEncoding::new(SampleType::U16, bits, 0)
                    .map_err(|_| MapError::Unavailable)?,
            }),
        })
    }
}

struct Planes8<'a> {
    views: [Option<rav1d_safe::PlaneView8<'a>>; 3],
}

impl MappedPlanes for Planes8<'_> {
    fn plane_count(&self) -> usize {
        self.views.iter().flatten().count()
    }

    fn plane(&self, index: usize) -> Option<AnyPlane<'_>> {
        let v = self.views.get(index)?.as_ref()?;
        PlaneRef::new(v.as_slice(), v.width(), v.height(), v.stride())
            .ok()
            .map(AnyPlane::U8)
    }

    fn encoding(&self, index: usize) -> Option<PlaneEncoding> {
        self.views.get(index)?.as_ref()?;
        Some(PlaneEncoding::Code(
            SampleEncoding::new(SampleType::U8, 8, 0).expect("valid 8-bit encoding"),
        ))
    }
}

struct Planes16<'a> {
    views: [Option<rav1d_safe::PlaneView16<'a>>; 3],
    encoding: SampleEncoding,
}

impl MappedPlanes for Planes16<'_> {
    fn plane_count(&self) -> usize {
        self.views.iter().flatten().count()
    }

    fn plane(&self, index: usize) -> Option<AnyPlane<'_>> {
        let v = self.views.get(index)?.as_ref()?;
        PlaneRef::new(v.as_slice(), v.width(), v.height(), v.stride())
            .ok()
            .map(AnyPlane::U16)
    }

    fn encoding(&self, index: usize) -> Option<PlaneEncoding> {
        self.views.get(index)?.as_ref()?;
        Some(PlaneEncoding::Code(self.encoding))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_goes_after_the_temporal_delimiter() {
        // TD with a size field, then a frame OBU.
        assert_eq!(
            leading_temporal_delimiter(&[0x12, 0x00, 0x32, 0x01, 0xAA]),
            2
        );
        // TD with an extension byte.
        assert_eq!(leading_temporal_delimiter(&[0x16, 0x00, 0x00, 0x32]), 3);
        // Not a TD (sequence header first), no size field, truncated sizes.
        assert_eq!(leading_temporal_delimiter(&[0x0A, 0x01, 0x00]), 0);
        assert_eq!(leading_temporal_delimiter(&[0x10, 0x32]), 0);
        assert_eq!(leading_temporal_delimiter(&[0x12, 0x80]), 0);
        assert_eq!(leading_temporal_delimiter(&[0x12, 0x05, 0x00]), 0);
        // A payload is not a delimiter's; nor is the forbidden bit.
        assert_eq!(leading_temporal_delimiter(&[0x12, 0x01, 0x00, 0x32]), 0);
        assert_eq!(leading_temporal_delimiter(&[0x92, 0x00, 0x32]), 0);
        assert_eq!(leading_temporal_delimiter(&[]), 0);
    }

    #[test]
    fn temporal_delimiter_sizes_do_not_depend_on_word_width() {
        // Six-byte zero LEB: the old usize accumulator shifted by 35.
        assert_eq!(
            leading_temporal_delimiter(&[0x12, 0x80, 0x80, 0x80, 0x80, 0x80, 0]),
            7
        );
        // Nonzero bits above bit 31 must not truncate to a zero size.
        assert_eq!(
            leading_temporal_delimiter(&[0x12, 0x80, 0x80, 0x80, 0x80, 0x10]),
            0
        );
        assert_eq!(
            leading_temporal_delimiter(&[0x12, 0x80, 0x80, 0x80, 0x80, 0x80]),
            0
        );
    }

    fn refuses_packet(mut packet: Packet, clock_mismatch: bool) {
        let mut decoder = Av1VideoDecoder::new().unwrap();
        decoder
            .configure(
                &CodecConfig::new(CodecId::Av1, zencodec::time::TimeBase::MILLIS),
                Some(&zencodec::NoPulse),
            )
            .unwrap();
        let original = packet.bytes().to_vec();
        let result = decoder.send(&mut packet, Some(&zencodec::NoPulse));
        if clock_mismatch {
            assert!(
                matches!(
                    result,
                    Err(SessionError::Protocol(ProtocolViolation::ClockMismatch))
                ),
                "{result:?}"
            );
        } else {
            assert!(
                matches!(
                    result,
                    Err(SessionError::Unsupported(UnsupportedMedia::Container))
                ),
                "{result:?}"
            );
        }
        assert_eq!(
            packet.bytes(),
            original,
            "refusal must preserve the payload"
        );
    }

    #[test]
    fn packet_timestamps_must_use_the_configured_clock() {
        let other = Timestamp::new(12, zencodec::time::TimeBase::hz(90_000).unwrap());
        for packet in [
            Packet::from_vec(vec![0x12, 0]).unwrap().with_pts(other),
            Packet::from_vec(vec![0x12, 0]).unwrap().with_dts(other),
        ] {
            refuses_packet(packet, true);
        }
    }

    #[test]
    fn packet_metadata_must_fit_the_backend() {
        let packet = || Packet::from_vec(vec![0x12, 0]).unwrap();
        refuses_packet(
            packet().with_pts(Timestamp::new(i64::MIN, zencodec::time::TimeBase::MILLIS)),
            false,
        );
        refuses_packet(packet().with_duration_ticks(i64::MAX as u64 + 1), false);
        refuses_packet(packet().with_duration_ticks(0), false);
        refuses_packet(packet().with_ordinal(u64::MAX), false);
    }

    #[test]
    fn packet_additions_are_refused_instead_of_lost() {
        use zencodec_media::packet::{PacketAddition, PacketPayload};
        let packet = Packet::from_vec(vec![0x12, 0])
            .unwrap()
            .with_addition(PacketAddition::new(1, PacketPayload::Owned(vec![1, 2])));
        refuses_packet(packet, false);
    }

    #[test]
    fn packet_discard_is_refused_instead_of_lost() {
        use zencodec_media::packet::Discard;
        let duration =
            zencodec::time::ticks_to_duration(1, zencodec::time::TimeBase::MILLIS).unwrap();
        for discard in [
            Discard::start(duration),
            Discard::end(duration),
            Discard::start(duration).with_end(duration),
        ] {
            refuses_packet(
                Packet::from_vec(vec![0x12, 0])
                    .unwrap()
                    .with_discard(discard),
                false,
            );
        }
    }

    #[test]
    fn config_obus_skip_the_av1c_header() {
        assert_eq!(config_obus(&[0x81, 0, 0, 0, 0x0A, 1]), Some(&[0x0A, 1][..]));
        assert_eq!(config_obus(&[0x81, 0, 0, 0]), None);
        assert_eq!(config_obus(&[0x0A, 1]), Some(&[0x0A, 1][..]));
        assert_eq!(config_obus(&[]), None);
    }

    #[test]
    fn container_fills_only_unspecified_fields() {
        let bs = Cicp::new(2, 16, 2, false);
        let ct = Cicp::new(9, 1, 9, true);
        assert_eq!(resolve_color(bs, Some(ct)), Cicp::new(9, 16, 9, false));
        assert_eq!(resolve_color(bs, None), bs);
    }
}

/// Adapter forwarding a [`zencodec::SharedPulse`] as an [`enough::Stop`] for
/// native decoder/encoder backends.
struct PulseStop(zencodec::SharedPulse);

impl Stop for PulseStop {
    fn check(&self) -> Result<(), enough::StopReason> {
        self.0.check()
    }

    fn may_stop(&self) -> bool {
        self.0.may_stop()
    }
}

/// Shareable [`VideoDecodeFactory`](zencodec_media::registry::VideoDecodeFactory)
/// for AV1 streams backed by `rav1d-safe` ([`Av1VideoDecoder`]).
#[derive(Clone, Debug, Default)]
pub struct Av1DecodeFactory {
    threads: Option<u32>,
}

impl Av1DecodeFactory {
    /// Create a factory using `rav1d-safe`'s default settings (respecting
    /// `JobResources` threading and memory caps).
    #[must_use]
    pub const fn new() -> Self {
        Self { threads: None }
    }

    /// Cap decoder threads when `JobResources` allows parallel execution.
    #[must_use]
    pub const fn with_threads(mut self, threads: u32) -> Self {
        self.threads = Some(threads);
        self
    }

    fn open_concrete(
        &self,
        plan: &zencodec_media::registry::VideoDecodePlan,
        run: &zencodec::JobResources,
    ) -> Result<Av1VideoDecoder, zencodec_media::registry::MediaError> {
        run.check_stop()?;
        if plan.config.codec != CodecId::Av1 {
            return Err(zencodec_media::registry::MediaError::Unsupported(
                UnsupportedMedia::Codec,
            ));
        }
        let mut settings = rav1d_safe::Settings::default();
        if run.limits().threading == zencodec::ThreadingPolicy::Sequential {
            settings.threads = 1;
        } else if let Some(t) = self.threads {
            settings.threads = t;
        }
        if let Some(mem) = run.limits().max_memory_bytes {
            settings.max_picture_bytes = usize::try_from(mem).ok();
        }
        let limits = run.limits();
        if limits.max_memory_bytes == Some(0) || limits.max_pixels == Some(0) {
            return Err(zencodec_media::registry::MediaError::Invalid(
                "AV1 decoder refused zero memory or pixel limit",
            ));
        }
        let (pw, ph) = plan.maximum_coded_extent;
        if pw == 0 || ph == 0 {
            return Err(zencodec_media::registry::MediaError::Invalid(
                "AV1 maximum coded extent must be non-zero",
            ));
        }
        let bound_w = limits.max_width.map_or(pw, |l| l.min(pw));
        let bound_h = limits.max_height.map_or(ph, |l| l.min(ph));
        if bound_w == 0 || bound_h == 0 || limits.check_dimensions(bound_w, bound_h).is_err() {
            return Err(zencodec_media::registry::MediaError::Invalid(
                "AV1 coded extent exceeds JobResources dimension limits",
            ));
        }
        let extent_pixels = u64::from(bound_w).saturating_mul(u64::from(bound_h));
        let pixel_cap = limits
            .max_pixels
            .map_or(extent_pixels, |p| p.min(extent_pixels));
        settings.frame_size_limit = u32::try_from(pixel_cap).unwrap_or(u32::MAX).max(1);
        let mut dec = Av1VideoDecoder::with_settings(settings)
            .map_err(zencodec_media::registry::MediaError::backend)?
            .with_max_extent((bound_w, bound_h))
            .with_max_caller_held(plan.max_retained_pictures.saturating_sub(8).max(1))
            .with_stop(Arc::new(PulseStop(run.pulse().clone())));
        if let Some(hint) = plan.config.private.as_deref().and_then(av1c_layout_hint) {
            let expected_gbr = if matches!(hint.1, Subsampling::Yuv444) {
                plan.config
                    .video
                    .and_then(|v| v.color)
                    .filter(|c| c.matrix_coefficients != 2)
                    .map(|c| c.matrix_coefficients == 0)
            } else {
                Some(false)
            };
            dec = dec.with_expected_av1c(hint, expected_gbr);
        }
        dec.configure(&plan.config, Some(run.pulse()))
            .map_err(zencodec_media::registry::MediaError::from)?;
        Ok(dec)
    }
}

fn av1c_layout_hint(private: &[u8]) -> Option<(u8, Subsampling, ChromaSiting)> {
    if private.len() < 4 || private[0] != 0x81 {
        return None;
    }
    let profile = (private[1] >> 5) & 0x07;
    if profile > 2 {
        return None;
    }
    let high_bitdepth = (private[2] >> 6) & 1 != 0;
    let twelve_bit = (private[2] >> 5) & 1 != 0;
    let bit_depth = if profile == 2 && high_bitdepth {
        if twelve_bit { 12 } else { 10 }
    } else if high_bitdepth {
        10
    } else {
        8
    };
    let monochrome = (private[2] >> 4) & 1 != 0;
    let sub_x = (private[2] >> 3) & 1 != 0;
    let sub_y = (private[2] >> 2) & 1 != 0;
    let subsampling = if monochrome {
        Subsampling::Mono
    } else if sub_x && sub_y {
        Subsampling::Yuv420
    } else if sub_x && !sub_y {
        Subsampling::Yuv422
    } else if !sub_x && !sub_y {
        Subsampling::Yuv444
    } else {
        return None;
    };
    let siting = if matches!(subsampling, Subsampling::Yuv420) {
        match private[2] & 0x03 {
            1 => ChromaSiting::Left,
            2 => ChromaSiting::TopLeft,
            _ => ChromaSiting::Unknown,
        }
    } else {
        ChromaSiting::Unknown
    };
    Some((bit_depth, subsampling, siting))
}

fn av1c_possible_layouts(
    hint: (u8, Subsampling, ChromaSiting),
    container_color: Option<Cicp>,
) -> Option<Vec<PlanarLayout>> {
    let (bits, sub, siting) = hint;
    let st = if bits == 8 {
        SampleType::U8
    } else {
        SampleType::U16
    };
    let enc = SampleEncoding::new(st, bits, 0).ok()?;
    let make = |matrix: u8| {
        PlanarLayout::new(
            plane_roles(sub, matrix)
                .into_iter()
                .map(|r| PlaneSpec::new(r, PlaneEncoding::Code(enc)))
                .collect(),
            sub,
            siting,
        )
        .ok()
    };
    if matches!(sub, Subsampling::Yuv444) {
        match container_color.map(|c| c.matrix_coefficients) {
            Some(0) => Some(vec![make(0)?]),
            Some(m) if m != 2 => Some(vec![make(1)?]),
            // Matrix is unspecified in container + av1C: bitstream could emit YUV or GBR.
            _ => Some(vec![make(1)?, make(0)?]),
        }
    } else {
        Some(vec![make(1)?])
    }
}

impl zencodec_media::registry::VideoDecodeFactory for Av1DecodeFactory {
    fn quote(
        &self,
        config: &CodecConfig,
        need: &zencodec_media::registry::VideoDecodeNeeds,
        run: &zencodec::JobResources,
    ) -> Result<zencodec_media::registry::VideoDecodePlan, zencodec_media::registry::MediaError>
    {
        run.check_stop()?;
        if config.codec != CodecId::Av1 {
            return Err(zencodec_media::registry::MediaError::Unsupported(
                UnsupportedMedia::Codec,
            ));
        }
        if run.limits().max_memory_bytes == Some(0) {
            return Err(zencodec_media::registry::MediaError::Invalid(
                "AV1 decoder refused zero memory limit",
            ));
        }
        let (w, h) = need.maximum_coded_extent;
        if w == 0 || h == 0 {
            return Err(zencodec_media::registry::MediaError::Invalid(
                "AV1 maximum coded extent must be non-zero",
            ));
        }
        if let Err(_e) = run.limits().check_dimensions(w, h) {
            return Err(zencodec_media::registry::MediaError::Invalid(
                "AV1 coded extent exceeds JobResources dimension limits",
            ));
        }
        // AV1 requires 8 internal reference picture slots plus at least 1 caller-leased picture.
        if need.max_held_pictures < 9 {
            return Err(zencodec_media::registry::MediaError::Unsupported(
                UnsupportedMedia::Codec,
            ));
        }
        let av1c_hint = config.private.as_deref().and_then(av1c_layout_hint);
        if !need.accepted_layouts.is_empty() {
            // Every layout that the pinned `av1C` (+ container color) can validly produce must
            // be present in `need.accepted_layouts`; otherwise refuse rather than risk emitting
            // an unaccepted siting, depth, subsampling, or 4:4:4 role order.
            let Some(hint) = av1c_hint else {
                return Err(zencodec_media::registry::MediaError::Unsupported(
                    UnsupportedMedia::Codec,
                ));
            };
            let Some(required_layouts) =
                av1c_possible_layouts(hint, config.video.and_then(|v| v.color))
            else {
                return Err(zencodec_media::registry::MediaError::Unsupported(
                    UnsupportedMedia::Codec,
                ));
            };
            if !required_layouts
                .iter()
                .all(|req| need.accepted_layouts.contains(req))
            {
                return Err(zencodec_media::registry::MediaError::Unsupported(
                    UnsupportedMedia::Codec,
                ));
            }
        }
        let mut plan = zencodec_media::registry::VideoDecodePlan::new(
            config.clone(),
            need.maximum_coded_extent,
            zencodec::source::NativeBound::Unknown,
            need.max_held_pictures,
        );
        plan.is_send = true;
        Ok(plan)
    }

    fn open(
        &self,
        plan: &zencodec_media::registry::VideoDecodePlan,
        run: &zencodec::JobResources,
    ) -> Result<
        Box<zencodec_media::registry::LocalVideoDecoder>,
        zencodec_media::registry::MediaError,
    > {
        let dec = self.open_concrete(plan, run)?;
        Ok(Box::new(zencodec_media::session::VideoDecoderShim(dec)))
    }

    fn open_send(
        &self,
        plan: &zencodec_media::registry::VideoDecodePlan,
        run: &zencodec::JobResources,
    ) -> Result<Box<zencodec_media::session::DynVideoDecoder>, zencodec_media::registry::MediaError>
    {
        let dec = self.open_concrete(plan, run)?;
        Ok(zencodec_media::session::erase_video_decoder(dec))
    }
}

#[cfg(feature = "media-encode")]
pub use encode::{Av1EncodeFactory, Av1EncodeSettings, Av1VideoBackend, Av1VideoEncoder};

#[cfg(feature = "media-encode")]
mod encode {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use enough::Stop;
    use num_traits::FromPrimitive;
    use zencodec::plane::{AnyPlane, PlaneEncoding};
    use zencodec::time::{TimeBase, Timestamp};
    use zencodec_media::frame::Frame;
    use zencodec_media::packet::{Packet, RandomAccess};
    use zencodec_media::session::{
        CodecConfig, ProtocolViolation, Receive, SendStatus, SessionError, VideoEncoder,
    };
    use zencodec_media::track::{CodecId, VideoTrackInfo};
    use zenpixels::Cicp;
    use zenpixels::sample::{ChromaSiting, ChromaSubsampling as Subsampling, SampleEncoding};
    use zenrav1e::prelude::{ChromaSampling, EncoderStatus, FrameType, Pixel};
    use zenrav1e::{Context, EncoderConfig};

    use super::{Av1SessionError, Res, plane_roles};

    /// What [`Av1VideoEncoder`] encodes: frames of exactly this shape.
    #[derive(Clone, Debug)]
    #[non_exhaustive]
    pub struct Av1EncodeSettings {
        /// Picture width.
        pub width: u32,
        /// Picture height.
        pub height: u32,
        /// 8, 10 or 12.
        pub bit_depth: u8,
        /// Chroma subsampling of the input planes.
        pub subsampling: Subsampling,
        /// Chroma siting of 4:2:0 input; AV1 signals no other.
        pub siting: ChromaSiting,
        /// Colour signalled in the sequence header (range flag included).
        pub color: Cicp,
        /// The encoder behind the session.
        pub backend: Av1VideoBackend,
        /// The backend's own speed preset, slowest first: zenrav1e 0 to 10,
        /// zenav1-svt 0 to 13 (clamped).
        pub speed: u8,
        /// Base quantizer index, 0 (lossless) to 255. zenav1-svt takes
        /// `quantizer / 4` on its 0 to 63 QP scale.
        pub quantizer: u8,
        /// Longest distance between key frames.
        pub key_frame_interval: u64,
        /// Encoder threads (0: zenrav1e's choice).
        pub threads: usize,
        /// Frames in flight before `send` answers `ReceivePending`, which it
        /// does only once a packet is ready: the encoder's lookahead may hold
        /// more.
        pub max_queued_frames: usize,
        /// Nominal frames per second, numerator and denominator. zenrav1e
        /// sizes its tiling (the AV1 level's tile-rate minimum) and scene
        /// detection from it. It is not written to the bitstream and has no
        /// bearing on timestamps, which come from the frames.
        pub frame_rate: (u32, u32),
        pub(crate) low_latency: bool,
        pub(crate) rdo_lookahead_frames: Option<usize>,
    }

    impl Av1EncodeSettings {
        /// 8-bit 4:2:0 of `width` x `height`, unspecified colour (limited
        /// range), speed 6, quantizer 100, a key frame at least every 240
        /// frames, a nominal 30 frames per second.
        pub fn new(width: u32, height: u32) -> Self {
            Self {
                width,
                height,
                bit_depth: 8,
                subsampling: Subsampling::Yuv420,
                siting: ChromaSiting::Unknown,
                color: Cicp::new(2, 2, 2, false),
                backend: Av1VideoBackend::Zenrav1e,
                speed: 6,
                quantizer: 100,
                key_frame_interval: 240,
                threads: 0,
                max_queued_frames: 64,
                frame_rate: (30, 1),
                low_latency: false,
                rdo_lookahead_frames: None,
            }
        }

        /// The coded depth.
        pub fn with_bit_depth(mut self, bits: u8) -> Self {
            self.bit_depth = bits;
            self
        }

        /// The input subsampling.
        pub fn with_subsampling(mut self, subsampling: Subsampling) -> Self {
            self.subsampling = subsampling;
            self
        }

        /// The 4:2:0 chroma siting.
        pub fn with_siting(mut self, siting: ChromaSiting) -> Self {
            self.siting = siting;
            self
        }

        /// The signalled colour.
        pub fn with_color(mut self, color: Cicp) -> Self {
            self.color = color;
            self
        }

        /// The encoder behind the session.
        pub fn with_backend(mut self, backend: Av1VideoBackend) -> Self {
            self.backend = backend;
            self
        }

        /// The speed preset.
        pub fn with_speed(mut self, speed: u8) -> Self {
            self.speed = speed;
            self
        }

        /// The base quantizer; 0 is lossless.
        pub fn with_quantizer(mut self, quantizer: u8) -> Self {
            self.quantizer = quantizer;
            self
        }

        /// The key-frame interval.
        pub fn with_key_frame_interval(mut self, frames: u64) -> Self {
            self.key_frame_interval = frames;
            self
        }

        /// Encoder threads.
        pub fn with_threads(mut self, threads: usize) -> Self {
            self.threads = threads;
            self
        }

        /// Attempt to pull output after this many frames are in flight.
        ///
        /// This is not a hard input-capacity or memory limit. A backend needing
        /// lookahead may accept more frames until output becomes available.
        /// Zero is rejected when constructing the session.
        pub fn with_output_pull_threshold(mut self, frames: usize) -> Self {
            self.max_queued_frames = frames;
            self
        }

        /// Compatibility spelling for the output pull threshold, not a hard cap.
        #[deprecated(
            note = "use with_output_pull_threshold; this setting is not a hard queue or memory bound"
        )]
        pub fn with_max_queued_frames(self, frames: usize) -> Self {
            self.with_output_pull_threshold(frames)
        }

        /// The nominal frame rate, `numerator / denominator` frames per
        /// second.
        pub fn with_frame_rate(mut self, numerator: u32, denominator: u32) -> Self {
            self.frame_rate = (numerator, denominator);
            self
        }
    }

    /// Which AV1 encoder an [`Av1VideoEncoder`] runs.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    pub enum Av1VideoBackend {
        /// zenrav1e: every depth and subsampling, lookahead with hidden
        /// frames.
        #[default]
        Zenrav1e,
        /// zenav1-svt (needs the `zenav1-svt` feature): one packet per
        /// frame, no lookahead, every frame a key frame
        /// (`key_frame_interval` must be 1). Its low-delay inter frames can
        /// be corrupt (libaom and rav1d reject them; imazen/zenav1-svt
        /// issue 26), so the session does not offer them yet. 8-bit mono,
        /// 4:2:0 and 4:4:4, 10-bit mono and 4:2:0; 4:2:2 is not ported and
        /// 12-bit is refused, as by SVT-AV1.
        Zenav1Svt,
    }

    fn cfg_err(m: &str) -> Av1SessionError {
        Av1SessionError::Configuration(m.into())
    }

    enum Backend {
        U8(Box<Context<u8>>),
        U16(Box<Context<u16>>),
        #[cfg(feature = "zenav1-svt")]
        Svt(Box<svt::SvtSession>),
    }

    /// zenrav1e's AV1 encoder as a [`VideoEncoder`] session.
    ///
    /// Frames must match [`Av1EncodeSettings`] exactly, or `send` refuses
    /// them with [`Av1SessionError::InputMismatch`]: size (uncropped), depth
    /// (LSB-aligned), subsampling, 4:2:0 siting, plane roles (Y, Cb, Cr; G,
    /// B, R under the identity matrix; no alpha), a known colour claim equal
    /// to the signalled one, and pictures holding exactly those planes at
    /// those sizes. Nothing is converted. Timestamps must increase strictly.
    ///
    /// Packets are temporal units in the low-overhead format, each starting
    /// with a temporal delimiter (ISOBMFF and Matroska muxers drop it). They
    /// carry the presentation timestamp and duration of the frame they show
    /// and are random-access points exactly when they are key frames.
    ///
    /// `output_config` carries the 4-byte `av1C` record with no
    /// `configOBUs`: every key frame repeats the sequence header in band,
    /// which ISOBMFF and Matroska both allow for streams with sync samples.
    pub struct Av1VideoEncoder {
        backend: Backend,
        settings: Av1EncodeSettings,
        clock: TimeBase,
        output: CodecConfig,
        owned_stop: Option<Arc<dyn Stop>>,
        /// Timing of frames sent and not yet out, by input frame number.
        queued: BTreeMap<u64, (Timestamp, Option<u64>)>,
        submitted: u64,
        emitted: u64,
        last_pts: Option<i64>,
        /// A packet pulled to answer `ReceivePending`, handed out first.
        ready: Option<Packet>,
        ended: bool,
        drained: bool,
    }

    enum Pulled {
        Packet(Packet),
        NeedInput,
        End,
    }

    impl core::fmt::Debug for Av1VideoEncoder {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("Av1VideoEncoder")
                .field("settings", &self.settings)
                .field("queued", &self.queued.len())
                .finish_non_exhaustive()
        }
    }

    fn build_rav1e_config(s: &Av1EncodeSettings) -> Result<zenrav1e::Config, Av1SessionError> {
        let chroma_sampling = match s.subsampling {
            Subsampling::Mono => ChromaSampling::Cs400,
            Subsampling::Yuv420 => ChromaSampling::Cs420,
            Subsampling::Yuv422 => ChromaSampling::Cs422,
            _ => ChromaSampling::Cs444,
        };
        use zenrav1e::color::ChromaSamplePosition;
        let chroma_sample_position = match (s.subsampling, s.siting) {
            (Subsampling::Yuv420, ChromaSiting::Left) => ChromaSamplePosition::Vertical,
            (Subsampling::Yuv420, ChromaSiting::TopLeft) => ChromaSamplePosition::Colocated,
            (_, ChromaSiting::Unknown) => ChromaSamplePosition::Unknown,
            _ => {
                return Err(cfg_err(
                    "AV1 signals chroma siting for 4:2:0 only, left or top-left",
                ));
            }
        };
        let c = s.color;
        if c.matrix_coefficients == 0 && !matches!(s.subsampling, Subsampling::Yuv444) {
            return Err(cfg_err("the identity matrix requires 4:4:4"));
        }
        let color_description = match (
            c.color_primaries,
            c.transfer_characteristics,
            c.matrix_coefficients,
        ) {
            (2, 2, 2) => None,
            (p, t, m) => Some(zenrav1e::color::ColorDescription {
                color_primaries: zenrav1e::color::ColorPrimaries::from_u8(p)
                    .ok_or_else(|| cfg_err("colour primaries code unknown to zenrav1e"))?,
                transfer_characteristics: zenrav1e::color::TransferCharacteristics::from_u8(t)
                    .ok_or_else(|| cfg_err("transfer code unknown to zenrav1e"))?,
                matrix_coefficients: zenrav1e::color::MatrixCoefficients::from_u8(m)
                    .ok_or_else(|| cfg_err("matrix code unknown to zenrav1e"))?,
            }),
        };
        let mut enc = EncoderConfig::with_speed_preset(s.speed.min(10));
        enc.width = s.width as usize;
        enc.height = s.height as usize;
        enc.bit_depth = usize::from(s.bit_depth);
        enc.chroma_sampling = chroma_sampling;
        enc.chroma_sample_position = chroma_sample_position;
        enc.pixel_range = if c.full_range {
            zenrav1e::color::PixelRange::Full
        } else {
            zenrav1e::color::PixelRange::Limited
        };
        enc.color_description = color_description;
        enc.quantizer = usize::from(s.quantizer);
        enc.min_quantizer = 0;
        enc.max_key_frame_interval = s.key_frame_interval.max(1);
        enc.min_key_frame_interval = enc.min_key_frame_interval.min(enc.max_key_frame_interval);
        enc.enable_timing_info = false;
        enc.still_picture = false;
        if s.low_latency {
            enc.low_latency = true;
        }
        if let Some(la) = s.rdo_lookahead_frames {
            enc.speed_settings.rdo_lookahead_frames = la;
        }
        enc.time_base =
            zenrav1e::prelude::Rational::new(u64::from(s.frame_rate.1), u64::from(s.frame_rate.0));
        let config = zenrav1e::Config::new()
            .with_encoder_config(enc)
            .with_threads(s.threads);
        config.validate().map_err(|e| cfg_err(&e.to_string()))?;
        Ok(config)
    }

    impl Av1VideoEncoder {
        /// An encoder for `settings`, timestamps on `clock`.
        ///
        /// # Errors
        ///
        /// [`Av1SessionError::Configuration`] for settings AV1 or zenrav1e
        /// cannot honour.
        pub fn new(settings: Av1EncodeSettings, clock: TimeBase) -> Result<Self, Av1SessionError> {
            let s = &settings;
            if !matches!(s.bit_depth, 8 | 10 | 12) {
                return Err(Av1SessionError::UnsupportedBitDepth(s.bit_depth));
            }
            if s.width == 0 || s.height == 0 || s.max_queued_frames == 0 {
                return Err(cfg_err("width, height and queue length must be positive"));
            }
            if s.frame_rate.0 == 0 || s.frame_rate.1 == 0 {
                return Err(cfg_err("the frame rate must be positive"));
            }
            let c = s.color;
            if c.matrix_coefficients == 0 && !matches!(s.subsampling, Subsampling::Yuv444) {
                return Err(cfg_err("the identity matrix requires 4:4:4"));
            }
            match (s.subsampling, s.siting) {
                (Subsampling::Yuv420, ChromaSiting::Left | ChromaSiting::TopLeft)
                | (_, ChromaSiting::Unknown) => {}
                _ => {
                    return Err(cfg_err(
                        "AV1 signals chroma siting for 4:2:0 only, left or top-left",
                    ));
                }
            }
            let (backend, mut av1c) = match s.backend {
                Av1VideoBackend::Zenrav1e => {
                    let config = build_rav1e_config(s)?;
                    let context_err = |e: zenrav1e::InvalidConfig| cfg_err(&e.to_string());
                    if s.bit_depth == 8 {
                        let context: Context<u8> = config.new_context().map_err(context_err)?;
                        let av1c = context.container_sequence_header();
                        (Backend::U8(Box::new(context)), av1c)
                    } else {
                        let context: Context<u16> = config.new_context().map_err(context_err)?;
                        let av1c = context.container_sequence_header();
                        (Backend::U16(Box::new(context)), av1c)
                    }
                }
                #[cfg(feature = "zenav1-svt")]
                Av1VideoBackend::Zenav1Svt => {
                    let (session, av1c) = svt::SvtSession::new(s)?;
                    (Backend::Svt(Box::new(session)), av1c)
                }
                #[cfg(not(feature = "zenav1-svt"))]
                Av1VideoBackend::Zenav1Svt => {
                    return Err(cfg_err("zenav1-svt needs the zenav1-svt feature"));
                }
            };
            // zenrav1e's record writes chroma_sample_position 0 whatever the
            // sequence header says, and subsampling_y 0 for monochrome
            // (AV1 sets both subsampling flags there). ISOBMFF requires the
            // record to agree with the sequence header, so the format byte
            // is rebuilt from the settings; tier is kept.
            let mono = matches!(s.subsampling, Subsampling::Mono);
            let (sub_x, sub_y) = match s.subsampling {
                Subsampling::Yuv444 => (0, 0),
                Subsampling::Yuv422 => (1, 0),
                _ => (1, 1),
            };
            let csp_code = match (s.subsampling, s.siting) {
                (Subsampling::Yuv420, ChromaSiting::Left) => 1,
                (Subsampling::Yuv420, ChromaSiting::TopLeft) => 2,
                _ => 0,
            };
            if let Some(flags) = av1c.get_mut(2) {
                *flags = (*flags & 0x80)
                    | u8::from(s.bit_depth > 8) << 6
                    | u8::from(s.bit_depth == 12) << 5
                    | u8::from(mono) << 4
                    | sub_x << 3
                    | sub_y << 2
                    | csp_code;
            }
            let output = CodecConfig::new(CodecId::Av1, clock)
                .with_private(Arc::from(av1c))
                .with_video(VideoTrackInfo::new(s.width, s.height).with_color(c));
            Ok(Self {
                backend,
                settings,
                clock,
                output,
                owned_stop: None,
                queued: BTreeMap::new(),
                submitted: 0,
                emitted: 0,
                last_pts: None,
                ready: None,
                ended: false,
                drained: false,
            })
        }

        /// Install an owned stop that the encoder backend can poll.
        ///
        /// It supplements the borrowed per-call pulse. Replace it with
        /// `Arc::new(enough::Unstoppable)` to clear it.
        pub fn with_stop(mut self, stop: Arc<dyn Stop>) -> Self {
            match &mut self.backend {
                Backend::U8(c) => c.set_stop(Arc::clone(&stop)),
                Backend::U16(c) => c.set_stop(Arc::clone(&stop)),
                #[cfg(feature = "zenav1-svt")]
                Backend::Svt(svt) => {
                    svt.pipeline.stop = almost_enough::StopToken::new(Arc::clone(&stop));
                }
            }
            self.owned_stop = Some(stop);
            self
        }

        fn matches(&self, frame: &Frame) -> bool {
            let i = frame.interpretation();
            let s = &self.settings;
            let g = &i.geometry;
            (g.coded_width, g.coded_height) == (s.width, s.height)
                && (g.crop.x, g.crop.y, g.crop.width, g.crop.height) == (0, 0, s.width, s.height)
                && i.layout.subsampling() == s.subsampling
                && i.uniform_code_encoding().is_some_and(|e| e.code_bits() == s.bit_depth && e.bit_shift() == 0)
                && (!matches!(s.subsampling, Subsampling::Yuv420) || i.layout.siting() == s.siting)
                && i.layout.planes().iter().map(|p| p.role()).eq(plane_roles(s.subsampling, s.color.matrix_coefficients))
                // A known colour claim must be the signalled one: coding a
                // full-range frame as limited (or YUV as GBR) is a silent
                // conversion. An unknown claim takes the settings' word.
                && i.color.resolved.is_none_or(|c| c == s.color)
        }

        /// Width and height of each plane, in plane order.
        fn plane_dims(&self) -> Vec<(usize, usize)> {
            let s = &self.settings;
            let luma = (s.width as usize, s.height as usize);
            match s.subsampling.chroma_size(s.width, s.height) {
                None => vec![luma],
                Some((w, h)) => vec![luma, (w as usize, h as usize), (w as usize, h as usize)],
            }
        }

        /// The next packet from zenrav1e, with the timing of the frame it
        /// shows.
        fn pull(&mut self) -> Res<Pulled> {
            loop {
                let got = match &mut self.backend {
                    Backend::U8(c) => c
                        .receive_packet()
                        .map(|p| (p.data, p.input_frameno, p.frame_type)),
                    Backend::U16(c) => c
                        .receive_packet()
                        .map(|p| (p.data, p.input_frameno, p.frame_type)),
                    #[cfg(feature = "zenav1-svt")]
                    Backend::Svt(svt) => {
                        return Ok(match svt.out.pop_front() {
                            Some(p) => Pulled::Packet(p),
                            None if self.ended => Pulled::End,
                            None => Pulled::NeedInput,
                        });
                    }
                };
                if let Some(owned) = &self.owned_stop {
                    owned.check()?;
                }
                let encode_err = |m: String| SessionError::Codec(Av1SessionError::Encode(m));
                match got {
                    Ok((data, input, frame_type)) => {
                        let (pts, duration) = self
                            .queued
                            .remove(&input)
                            .ok_or_else(|| encode_err("packet for a frame never sent".into()))?;
                        let ra = if frame_type == FrameType::KEY {
                            RandomAccess::Yes
                        } else {
                            RandomAccess::No
                        };
                        let mut p = Packet::from_vec(data)
                            .map_err(|e| encode_err(e.to_string()))?
                            .with_pts(pts)
                            .with_random_access(ra)
                            .with_ordinal(self.emitted);
                        if let Some(d) = duration {
                            p = p.with_duration_ticks(d);
                        }
                        self.emitted += 1;
                        return Ok(Pulled::Packet(p));
                    }
                    Err(EncoderStatus::Encoded) => continue,
                    Err(EncoderStatus::NeedMoreData) if !self.ended => {
                        return Ok(Pulled::NeedInput);
                    }
                    Err(EncoderStatus::LimitReached) if self.ended => return Ok(Pulled::End),
                    Err(e) => return Err(encode_err(format!("{e:?}"))),
                }
            }
        }
    }

    /// Copy the frame's planes into a zenrav1e frame and send it. The
    /// picture must hold exactly the planes `dims` names, each in the
    /// interpretation's sample encoding: a short, oversized or differently
    /// coded plane is refused, never cropped or padded.
    fn copy_and_send<P: Pixel>(
        ctx: &mut Context<P>,
        frame: &Frame,
        dims: &[(usize, usize)],
        encoding: SampleEncoding,
    ) -> Result<(), SendError> {
        let mapped = frame.map().map_err(|_| SendError::Input)?;
        if mapped.plane_count() != dims.len() {
            return Err(SendError::Input);
        }
        for (index, &(w, h)) in dims.iter().enumerate() {
            let plane = mapped.plane(index).ok_or(SendError::Input)?;
            if (plane.width(), plane.height()) != (w, h)
                || mapped.encoding(index) != Some(PlaneEncoding::Code(encoding))
            {
                return Err(SendError::Input);
            }
        }
        let mut out = ctx.new_frame();
        for (index, &(w, h)) in dims.iter().enumerate() {
            let plane = mapped.plane(index).ok_or(SendError::Input)?;
            let mut slice = out.planes[index].mut_slice(Default::default());
            for (y, row) in slice.rows_iter_mut().take(h).enumerate() {
                let dst = row.get_mut(..w).ok_or(SendError::Input)?;
                match &plane {
                    AnyPlane::U8(src) => {
                        let src = src.row(y).ok_or(SendError::Input)?;
                        for (d, &v) in dst.iter_mut().zip(src) {
                            *d = P::cast_from(v);
                        }
                    }
                    AnyPlane::U16(src) => {
                        let src = src.row(y).ok_or(SendError::Input)?;
                        for (d, &v) in dst.iter_mut().zip(src) {
                            *d = P::cast_from(v);
                        }
                    }
                    _ => return Err(SendError::Input),
                }
            }
        }
        // EnoughData comes only after flush or for still pictures, both
        // excluded before this call; it is an encoder error here.
        ctx.send_frame(out)
            .map_err(|e| SendError::Encoder(format!("{e:?}")))
    }

    enum SendError {
        Input,
        Encoder(String),
    }

    /// A frame's planes copied tightly packed, after the same checks as
    /// [`copy_and_send`].
    #[cfg(feature = "zenav1-svt")]
    enum Packed {
        Eight(Vec<Vec<u8>>),
        Wide(Vec<Vec<u16>>),
    }

    #[cfg(feature = "zenav1-svt")]
    fn packed_planes(
        frame: &Frame,
        dims: &[(usize, usize)],
        encoding: SampleEncoding,
    ) -> Result<Packed, SendError> {
        let mapped = frame.map().map_err(|_| SendError::Input)?;
        if mapped.plane_count() != dims.len() {
            return Err(SendError::Input);
        }
        let mut eight = Vec::new();
        let mut wide = Vec::new();
        for (index, &(w, h)) in dims.iter().enumerate() {
            let plane = mapped.plane(index).ok_or(SendError::Input)?;
            if (plane.width(), plane.height()) != (w, h)
                || mapped.encoding(index) != Some(PlaneEncoding::Code(encoding))
            {
                return Err(SendError::Input);
            }
            let (mut p8, mut p16) = (Vec::new(), Vec::new());
            for y in 0..h {
                match &plane {
                    AnyPlane::U8(r) => {
                        let r = r.row(y).ok_or(SendError::Input)?;
                        p8.extend_from_slice(r.get(..w).ok_or(SendError::Input)?)
                    }
                    AnyPlane::U16(r) => {
                        let r = r.row(y).ok_or(SendError::Input)?;
                        p16.extend_from_slice(r.get(..w).ok_or(SendError::Input)?)
                    }
                    _ => return Err(SendError::Input),
                }
            }
            eight.push(p8);
            wide.push(p16);
        }
        Ok(if encoding.code_bits() == 8 {
            Packed::Eight(eight)
        } else {
            Packed::Wide(wide)
        })
    }

    #[cfg(feature = "zenav1-svt")]
    mod svt {
        use std::collections::VecDeque;

        use svtav1::pipeline::{ChromaFormat, EncodePipeline, RcConfig, RcMode};
        use zencodec_media::packet::Packet;
        use zenpixels::sample::{ChromaSiting, ChromaSubsampling as Subsampling};

        use super::{Av1EncodeSettings, Packed, SendError, cfg_err};
        use crate::media::Av1SessionError;

        /// zenav1-svt, all-intra: each frame is one temporal unit, out at
        /// once.
        pub(super) struct SvtSession {
            pub(super) pipeline: EncodePipeline,
            subsampling: Subsampling,
            width: usize,
            intra_period: u64,
            sent: u64,
            /// Packets out and not yet received.
            pub(super) out: VecDeque<Packet>,
        }

        impl SvtSession {
            /// The session and its `av1C` record (format byte left to the
            /// caller).
            pub(super) fn new(s: &Av1EncodeSettings) -> Result<(Self, Vec<u8>), Av1SessionError> {
                if s.key_frame_interval != 1 {
                    return Err(cfg_err(
                        "zenav1-svt runs all-intra here (its inter frames can be corrupt, \
                         imazen/zenav1-svt issue 26): key_frame_interval must be 1",
                    ));
                }
                match (s.bit_depth, s.subsampling) {
                    (_, Subsampling::Yuv422) => {
                        return Err(cfg_err("zenav1-svt has not ported 4:2:2"));
                    }
                    (8, _) | (10, Subsampling::Mono | Subsampling::Yuv420) => {}
                    (10, _) => {
                        return Err(cfg_err("zenav1-svt codes 10-bit mono and 4:2:0 only"));
                    }
                    _ => {
                        return Err(cfg_err(
                            "zenav1-svt codes 8- and 10-bit only, as SVT-AV1 does",
                        ));
                    }
                }
                let fps = f64::from(s.frame_rate.0) / f64::from(s.frame_rate.1);
                let rc = RcConfig {
                    mode: RcMode::Cqp,
                    qp: (s.quantizer / 4).min(63),
                    framerate: fps,
                    ..RcConfig::default()
                };
                let preset = svtav1::avif::NativePreset::new(s.speed.min(13) as i8)
                    .ok_or_else(|| cfg_err("zenav1-svt preset out of range"))?;
                let intra_period = u32::try_from(s.key_frame_interval.max(1)).unwrap_or(u32::MAX);
                let mut pipeline =
                    EncodePipeline::new_with_preset(s.width, s.height, preset, rc, 0, intra_period)
                        .with_image_sequence();
                pipeline = match s.subsampling {
                    Subsampling::Mono => pipeline,
                    Subsampling::Yuv420 => pipeline.with_chroma_format(Some(ChromaFormat::Yuv420)),
                    Subsampling::Yuv422 => pipeline.with_chroma_format(Some(ChromaFormat::Yuv422)),
                    _ => pipeline.with_chroma_format(Some(ChromaFormat::Yuv444)),
                };
                let csp = match (s.subsampling, s.siting) {
                    (Subsampling::Yuv420, ChromaSiting::Left) => 1,
                    (Subsampling::Yuv420, ChromaSiting::TopLeft) => 2,
                    _ => 0,
                };
                let c = s.color;
                pipeline = pipeline
                    .with_bit_depth(s.bit_depth)
                    .with_color_description(svtav1::pipeline::ColorDescription {
                        color_primaries: c.color_primaries,
                        transfer_characteristics: c.transfer_characteristics,
                        matrix_coefficients: c.matrix_coefficients,
                        full_range: c.full_range,
                    })
                    .with_chroma_sample_position(csp)
                    .with_thread_count(s.threads);
                let profile: u8 = match s.subsampling {
                    Subsampling::Yuv444 => 1,
                    Subsampling::Yuv422 => 2,
                    _ => 0,
                };
                let level = svtav1::pipeline::compute_seq_level_idx(s.width, s.height, fps);
                let av1c = vec![0x81, profile << 5 | (level & 0x1F), 0, 0];
                Ok((
                    Self {
                        pipeline,
                        subsampling: s.subsampling,
                        width: s.width as usize,
                        intra_period: u64::from(intra_period),
                        sent: 0,
                        out: VecDeque::new(),
                    },
                    av1c,
                ))
            }

            /// Encode one frame; its temporal unit and whether it is a key
            /// frame.
            pub(super) fn encode(&mut self, planes: Packed) -> Result<(Vec<u8>, bool), SendError> {
                let w = self.width;
                let p = &mut self.pipeline;
                let err = |e: whereat::At<svtav1::pipeline::EncodeError>| {
                    SendError::Encoder(format!("{e:?}"))
                };
                let bytes = match (planes, self.subsampling) {
                    (Packed::Eight(v), Subsampling::Mono) => p.try_encode_frame(&v[0], w),
                    (Packed::Eight(v), Subsampling::Yuv420) => {
                        p.try_encode_frame_420(&v[0], &v[1], &v[2], w)
                    }
                    (Packed::Eight(v), Subsampling::Yuv422) => {
                        p.try_encode_frame_422(&v[0], &v[1], &v[2], w)
                    }
                    (Packed::Eight(v), _) => p.try_encode_frame_444(&v[0], &v[1], &v[2], w),
                    (Packed::Wide(v), Subsampling::Mono) => p.try_encode_frame_hbd(&v[0], w),
                    (Packed::Wide(v), _) => p.try_encode_frame_420_hbd(&v[0], &v[1], &v[2], w),
                }
                .map_err(err)?;
                let key = self.sent.is_multiple_of(self.intra_period);
                self.sent += 1;
                Ok((bytes, key))
            }

            /// Low delay holds nothing back; anything here has no frame to
            /// time it by.
            pub(super) fn flush(&mut self) -> Result<(), Av1SessionError> {
                let rest = self
                    .pipeline
                    .try_flush()
                    .map_err(|e| Av1SessionError::Encode(format!("{e:?}")))?;
                if rest.is_empty() {
                    Ok(())
                } else {
                    Err(Av1SessionError::Encode(
                        "zenav1-svt flushed untimed output".into(),
                    ))
                }
            }
        }
    }

    impl VideoEncoder for Av1VideoEncoder {
        type Error = Av1SessionError;

        fn send(&mut self, frame: &Frame, pulse: Option<&dyn zencodec::Pulse>) -> Res<SendStatus> {
            let pulse = pulse.unwrap_or(&zencodec::NoPulse);
            pulse.check()?;
            if let Some(owned) = &self.owned_stop {
                owned.check()?;
            }
            if self.ended {
                return Err(SessionError::Protocol(ProtocolViolation::SendAfterEnd));
            }
            let pts = frame
                .timing()
                .pts
                .ok_or(SessionError::Protocol(ProtocolViolation::MissingTimestamp))?;
            if pts.base() != self.clock {
                return Err(SessionError::Protocol(ProtocolViolation::ClockMismatch));
            }
            if self.last_pts.is_some_and(|last| pts.ticks() <= last) {
                return Err(SessionError::Codec(Av1SessionError::NonMonotonicTimestamp));
            }
            if !self.matches(frame) {
                return Err(SessionError::Codec(Av1SessionError::InputMismatch));
            }
            if self.ready.is_some() {
                return Ok(SendStatus::ReceivePending);
            }
            #[cfg(feature = "zenav1-svt")]
            {
                let dims = self.plane_dims();
                let max = self.settings.max_queued_frames;
                if let Backend::Svt(svt) = &mut self.backend {
                    if svt.out.len() >= max {
                        return Ok(SendStatus::ReceivePending);
                    }
                    let input_err = |e| {
                        SessionError::Codec(match e {
                            SendError::Input => Av1SessionError::InputMismatch,
                            SendError::Encoder(m) => Av1SessionError::Encode(m),
                        })
                    };
                    let planes = packed_planes(
                        frame,
                        &dims,
                        frame
                            .interpretation()
                            .uniform_code_encoding()
                            .ok_or(SessionError::Codec(Av1SessionError::InputMismatch))?,
                    )
                    .map_err(input_err)?;
                    let encoded = svt.encode(planes);
                    if let Some(owned) = &self.owned_stop {
                        owned.check()?;
                    }
                    let (bytes, key) = encoded.map_err(input_err)?;
                    let mut p = Packet::from_vec(bytes)
                        .map_err(|e| SessionError::Codec(Av1SessionError::Encode(e.to_string())))?
                        .with_pts(pts)
                        .with_random_access(if key {
                            RandomAccess::Yes
                        } else {
                            RandomAccess::No
                        })
                        .with_ordinal(self.emitted);
                    if let Some(d) = frame.timing().duration_ticks {
                        p = p.with_duration_ticks(d);
                    }
                    svt.out.push_back(p);
                    self.emitted += 1;
                    self.submitted += 1;
                    self.last_pts = Some(pts.ticks());
                    return Ok(SendStatus::Accepted);
                }
            }
            if self.queued.len() >= self.settings.max_queued_frames {
                // Push back only with a packet to hand out, so the
                // `receive` that follows is `Ready`.
                if let Pulled::Packet(p) = self.pull()? {
                    self.ready = Some(p);
                    return Ok(SendStatus::ReceivePending);
                }
                if self.settings.low_latency {
                    return Err(SessionError::Codec(Av1SessionError::Encode(String::from(
                        "encoder exceeded admitted in-flight frame bound",
                    ))));
                }
            }
            let dims = self.plane_dims();
            let encoding = frame
                .interpretation()
                .uniform_code_encoding()
                .ok_or(SessionError::Codec(Av1SessionError::InputMismatch))?;
            let sent = match &mut self.backend {
                Backend::U8(c) => copy_and_send(c, frame, &dims, encoding),
                Backend::U16(c) => copy_and_send(c, frame, &dims, encoding),
                // Sent above.
                #[cfg(feature = "zenav1-svt")]
                Backend::Svt(_) => Ok(()),
            };
            match sent {
                Ok(()) => {}
                Err(SendError::Input) => {
                    return Err(SessionError::Codec(Av1SessionError::InputMismatch));
                }
                Err(SendError::Encoder(m)) => {
                    return Err(SessionError::Codec(Av1SessionError::Encode(m)));
                }
            }
            self.queued
                .insert(self.submitted, (pts, frame.timing().duration_ticks));
            self.submitted += 1;
            self.last_pts = Some(pts.ticks());
            Ok(SendStatus::Accepted)
        }

        fn receive(&mut self, pulse: Option<&dyn zencodec::Pulse>) -> Res<Receive<Packet>> {
            let pulse = pulse.unwrap_or(&zencodec::NoPulse);
            pulse.check()?;
            if let Some(owned) = &self.owned_stop {
                owned.check()?;
            }
            if let Some(p) = self.ready.take() {
                return Ok(Receive::Ready(p));
            }
            if self.drained {
                return Ok(Receive::EndOfStream);
            }
            Ok(match self.pull()? {
                Pulled::Packet(p) => Receive::Ready(p),
                Pulled::NeedInput => Receive::NeedInput,
                Pulled::End => {
                    self.drained = true;
                    Receive::EndOfStream
                }
            })
        }

        fn end_input(&mut self, pulse: Option<&dyn zencodec::Pulse>) -> Res<()> {
            if let Some(p) = pulse {
                p.check()?;
            }
            if let Some(owned) = &self.owned_stop {
                owned.check()?;
            }
            if !self.ended {
                match &mut self.backend {
                    Backend::U8(c) => c.flush(),
                    Backend::U16(c) => c.flush(),
                    #[cfg(feature = "zenav1-svt")]
                    Backend::Svt(svt) => svt.flush().map_err(SessionError::Codec)?,
                }
                self.ended = true;
            }
            Ok(())
        }

        fn output_config(&self) -> Option<&CodecConfig> {
            Some(&self.output)
        }
    }

    /// Shareable [`VideoEncodeFactory`](zencodec_media::registry::VideoEncodeFactory)
    /// for AV1 streams backed by `zenrav1e` (or `zenav1-svt` when enabled).
    #[derive(Clone, Debug)]
    pub struct Av1EncodeFactory {
        backend: Av1VideoBackend,
        speed: u8,
        default_quantizer: u8,
        key_frame_interval: u64,
        threads: usize,
    }

    impl Default for Av1EncodeFactory {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Av1EncodeFactory {
        /// Create a factory targeting `zenrav1e` at speed 6, quantizer 100.
        #[must_use]
        pub const fn new() -> Self {
            Self {
                backend: Av1VideoBackend::Zenrav1e,
                speed: 6,
                default_quantizer: 100,
                key_frame_interval: 240,
                threads: 0,
            }
        }

        /// Select the underlying AV1 encoder backend.
        #[must_use]
        pub const fn with_backend(mut self, backend: Av1VideoBackend) -> Self {
            self.backend = backend;
            if matches!(backend, Av1VideoBackend::Zenav1Svt) {
                self.key_frame_interval = 1;
            }
            self
        }

        /// Configure speed preset (zenrav1e `0..=10`, zenav1-svt `0..=13`).
        #[must_use]
        pub const fn with_speed(mut self, speed: u8) -> Self {
            self.speed = speed;
            self
        }

        /// Configure default quantizer (`0` = lossless ..= `255`) when no fidelity is specified.
        #[must_use]
        pub const fn with_quantizer(mut self, quantizer: u8) -> Self {
            self.default_quantizer = quantizer;
            self
        }

        /// Configure maximum keyframe interval.
        #[must_use]
        pub const fn with_key_frame_interval(mut self, interval: u64) -> Self {
            self.key_frame_interval = interval;
            self
        }

        /// Configure thread count (`0` = backend default; overridden to `1` when
        /// `JobResources` specifies `ThreadingPolicy::Sequential`).
        #[must_use]
        pub const fn with_threads(mut self, threads: usize) -> Self {
            self.threads = threads;
            self
        }

        fn build_settings(
            &self,
            interp: &zencodec_media::frame::FrameInterpretation,
            fidelity: Option<&zencodec::encode::Fidelity>,
            max_in_flight: usize,
            run: &zencodec::JobResources,
        ) -> Result<(Av1EncodeSettings, u32), zencodec_media::registry::MediaError> {
            use zencodec_media::registry::MediaError;
            use zencodec_media::session::UnsupportedMedia;

            let geom = interp.geometry;
            if geom.coded_width == 0
                || geom.coded_height == 0
                || geom.crop.x != 0
                || geom.crop.y != 0
                || geom.crop.width != geom.coded_width
                || geom.crop.height != geom.coded_height
            {
                return Err(MediaError::Unsupported(UnsupportedMedia::Codec));
            }
            // Validate exact layout matching `Av1VideoEncoder::matches` statically without
            // constructing a native encoder context: uniform LSB-aligned 8/10/12-bit + plane roles.
            let enc = interp
                .uniform_code_encoding()
                .filter(|e| matches!(e.code_bits(), 8 | 10 | 12) && e.bit_shift() == 0)
                .ok_or(MediaError::Unsupported(UnsupportedMedia::Codec))?;
            let bit_depth = enc.code_bits();
            let subsampling = interp.layout.subsampling();
            let siting = interp.layout.siting();
            match (subsampling, siting) {
                (Subsampling::Yuv420, ChromaSiting::Left | ChromaSiting::TopLeft)
                | (_, ChromaSiting::Unknown) => {}
                _ => return Err(MediaError::Unsupported(UnsupportedMedia::Codec)),
            }
            let color = interp.color.resolved.unwrap_or(Cicp::new(2, 2, 2, false));
            if color.matrix_coefficients == 0 && !matches!(subsampling, Subsampling::Yuv444) {
                return Err(MediaError::Unsupported(UnsupportedMedia::Codec));
            }
            if !interp
                .layout
                .planes()
                .iter()
                .map(|p| p.role())
                .eq(plane_roles(subsampling, color.matrix_coefficients))
            {
                return Err(MediaError::Unsupported(UnsupportedMedia::Codec));
            }
            let (low_latency, rdo_lookahead, lag_frames, pull_threshold) = match self.backend {
                Av1VideoBackend::Zenrav1e => {
                    if (
                        color.color_primaries,
                        color.transfer_characteristics,
                        color.matrix_coefficients,
                    ) != (2, 2, 2)
                        && (zenrav1e::color::ColorPrimaries::from_u8(color.color_primaries)
                            .is_none()
                            || zenrav1e::color::TransferCharacteristics::from_u8(
                                color.transfer_characteristics,
                            )
                            .is_none()
                            || zenrav1e::color::MatrixCoefficients::from_u8(
                                color.matrix_coefficients,
                            )
                            .is_none())
                    {
                        return Err(MediaError::Unsupported(UnsupportedMedia::Codec));
                    }
                    // Low-latency mode (`group_input_len = 1`, `keyframe_lookahead_distance = 2`)
                    // with `rdo_lookahead_frames = 1` requires `rdo_lookahead_frames + 1 = 2`
                    // frames in `FrameInvariants`, so `output_frameno = 1` requires
                    // `lookahead_end >= 1 + 2 + 1 = 4` (5 frames `0..=4` in flight before packet 0).
                    const MIN_LOW_LATENCY_IN_FLIGHT: usize = 5;
                    if max_in_flight < MIN_LOW_LATENCY_IN_FLIGHT {
                        return Err(MediaError::Unsupported(UnsupportedMedia::Codec));
                    }
                    let bound = max_in_flight.min(64);
                    (true, Some(1), bound as u32, bound)
                }
                Av1VideoBackend::Zenav1Svt => {
                    #[cfg(not(feature = "zenav1-svt"))]
                    {
                        return Err(MediaError::Unsupported(UnsupportedMedia::Codec));
                    }
                    #[cfg(feature = "zenav1-svt")]
                    {
                        if max_in_flight < 1 {
                            return Err(MediaError::Unsupported(UnsupportedMedia::Codec));
                        }
                        let svt_supported = match (bit_depth, subsampling) {
                            (8, Subsampling::Mono | Subsampling::Yuv420 | Subsampling::Yuv444) => {
                                true
                            }
                            (10, Subsampling::Mono | Subsampling::Yuv420) => true,
                            _ => false,
                        };
                        if !svt_supported {
                            return Err(MediaError::Unsupported(UnsupportedMedia::Codec));
                        }
                        (false, None, 0, 1)
                    }
                }
            };
            let quantizer = match fidelity {
                None => self.default_quantizer,
                Some(zencodec::encode::Fidelity::Lossless) => 0,
                Some(zencodec::encode::Fidelity::Lossy(
                    zencodec::encode::LossyTarget::CodecSpecificQuality(q),
                )) => {
                    if !q.is_finite() {
                        return Err(MediaError::Invalid("AV1 quality must be finite"));
                    }
                    let clamped = q.clamp(0.0, 100.0);
                    ((100.0 - clamped) * 2.55 + 0.5) as u8
                }
                Some(_) => return Err(MediaError::Unsupported(UnsupportedMedia::Codec)),
            };
            let threads = if run.limits().threading == zencodec::ThreadingPolicy::Sequential {
                1
            } else {
                self.threads
            };
            let mut settings = Av1EncodeSettings::new(geom.coded_width, geom.coded_height)
                .with_bit_depth(bit_depth)
                .with_subsampling(subsampling)
                .with_siting(siting)
                .with_color(color)
                .with_backend(self.backend)
                .with_speed(self.speed)
                .with_quantizer(quantizer)
                .with_key_frame_interval(self.key_frame_interval)
                .with_threads(threads)
                .with_output_pull_threshold(pull_threshold);
            settings.low_latency = low_latency;
            settings.rdo_lookahead_frames = rdo_lookahead;
            if matches!(self.backend, Av1VideoBackend::Zenav1Svt) {
                settings.key_frame_interval = 1;
            } else {
                // Validate full native zenrav1e configuration (including u16::MAX dimensions)
                // without constructing a native encoder context.
                build_rav1e_config(&settings)
                    .map_err(|_| MediaError::Unsupported(UnsupportedMedia::Codec))?;
            }
            Ok((settings, lag_frames))
        }

        fn open_concrete(
            &self,
            plan: &zencodec_media::registry::VideoEncodePlan,
            run: &zencodec::JobResources,
        ) -> Result<Av1VideoEncoder, zencodec_media::registry::MediaError> {
            run.check_stop()?;
            if plan.codec != CodecId::Av1 {
                return Err(zencodec_media::registry::MediaError::Unsupported(
                    zencodec_media::session::UnsupportedMedia::Codec,
                ));
            }
            if run.limits().max_memory_bytes == Some(0) {
                return Err(zencodec_media::registry::MediaError::Invalid(
                    "AV1 encoder refused zero memory limit",
                ));
            }
            let min_in_flight = match self.backend {
                Av1VideoBackend::Zenav1Svt => 1,
                Av1VideoBackend::Zenrav1e => (plan.lag_frames as usize).max(5),
            };
            let (settings, _) = self.build_settings(
                &plan.interpretation,
                plan.fidelity.as_ref(),
                min_in_flight,
                run,
            )?;
            let enc = Av1VideoEncoder::new(settings, plan.clock)
                .map_err(|e| match e {
                    Av1SessionError::UnsupportedBitDepth(_) | Av1SessionError::Configuration(_) => {
                        zencodec_media::registry::MediaError::Unsupported(
                            zencodec_media::session::UnsupportedMedia::Codec,
                        )
                    }
                    other => zencodec_media::registry::MediaError::backend(other),
                })?
                .with_stop(Arc::new(super::PulseStop(run.pulse().clone())));
            Ok(enc)
        }
    }

    impl zencodec_media::registry::VideoEncodeFactory for Av1EncodeFactory {
        fn quote(
            &self,
            need: &zencodec_media::registry::VideoEncodeNeeds,
            run: &zencodec::JobResources,
        ) -> Result<zencodec_media::registry::VideoEncodePlan, zencodec_media::registry::MediaError>
        {
            run.check_stop()?;
            if need.codec != CodecId::Av1 {
                return Err(zencodec_media::registry::MediaError::Unsupported(
                    zencodec_media::session::UnsupportedMedia::Codec,
                ));
            }
            if run.limits().max_memory_bytes == Some(0) {
                return Err(zencodec_media::registry::MediaError::Invalid(
                    "AV1 encoder refused zero memory limit",
                ));
            }
            let geom = need.interpretation.geometry;
            if run
                .limits()
                .check_dimensions(geom.coded_width, geom.coded_height)
                .is_err()
            {
                return Err(zencodec_media::registry::MediaError::Invalid(
                    "AV1 encode dimensions exceed JobResources limits",
                ));
            }
            let (_settings, lag) = self.build_settings(
                &need.interpretation,
                need.fidelity.as_ref(),
                need.max_in_flight_frames,
                run,
            )?;
            let mut plan = zencodec_media::registry::VideoEncodePlan::new(
                CodecId::Av1,
                Arc::clone(&need.interpretation),
                need.clock,
                zencodec::source::NativeBound::Unknown,
                lag,
            )
            .with_fidelity(need.fidelity);
            plan.is_send = true;
            Ok(plan)
        }

        fn open(
            &self,
            plan: &zencodec_media::registry::VideoEncodePlan,
            run: &zencodec::JobResources,
        ) -> Result<
            Box<zencodec_media::registry::LocalVideoEncoder>,
            zencodec_media::registry::MediaError,
        > {
            let enc = self.open_concrete(plan, run)?;
            Ok(Box::new(zencodec_media::session::VideoEncoderShim(enc)))
        }

        fn open_send(
            &self,
            plan: &zencodec_media::registry::VideoEncodePlan,
            run: &zencodec::JobResources,
        ) -> Result<
            Box<zencodec_media::session::DynVideoEncoder>,
            zencodec_media::registry::MediaError,
        > {
            let enc = self.open_concrete(plan, run)?;
            Ok(zencodec_media::session::erase_video_encoder(enc))
        }
    }
}

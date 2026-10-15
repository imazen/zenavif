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
//!   ordinal through rav1d unchanged; frames report them on the configured
//!   clock. Missing timestamps stay missing.
//! - **Interpretation.** Coded and render size, subsampling, chroma siting
//!   (`chroma_sample_position`, unknown kept), sample depth (10- and 12-bit
//!   codes LSB-aligned in 16-bit words), plane roles (identity matrix planes
//!   are G, B, R) and colour claims: the bitstream's raw CICP, the
//!   container's, and the resolved one (bitstream fields, with the
//!   container filling those the bitstream leaves unspecified).
//! - **Backpressure.** One accepted packet may wait inside the session while
//!   rav1d holds output; `send` then answers `ReceivePending` until a
//!   `receive` makes room.

use std::collections::BTreeMap;
use std::sync::Arc;

use enough::Stop;
use zencodec::time::Timestamp;
use zencodec_media::frame::{
    ChromaSiting, ColorClaim, Frame, FrameGeometry, FrameInterpretation, FrameTiming, MapError,
    MappedPlanes, Picture, PlaneRole, PlaneSamples, PlaneView, Presentation, SampleEncoding,
    Subsampling,
};
use zencodec_media::packet::Packet;
use zencodec_media::session::{
    CodecConfig, ProtocolViolation, Receive, SendStatus, SessionError, UnsupportedMedia,
    VideoDecoder, VideoStreamInfo,
};
use zencodec_media::track::CodecId;
use zenpixels::{ChannelType, Cicp};

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
}

impl core::fmt::Display for Av1SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "AV1 decode: {e}"),
            Self::UnsupportedBitDepth(b) => write!(f, "unsupported AV1 bit depth {b}"),
            Self::Configuration(m) => write!(f, "AV1 encoder settings: {m}"),
            Self::Encode(m) => write!(f, "AV1 encode: {m}"),
            Self::InputMismatch => f.write_str("frame does not match the AV1 encoder's settings"),
            Self::NonMonotonicTimestamp => {
                f.write_str("frame timestamp is not later than the previous one")
            }
        }
    }
}

impl std::error::Error for Av1SessionError {}

type Res<T> = Result<T, SessionError<Av1SessionError>>;

fn decode_err(e: whereat::At<rav1d_safe::Error>) -> SessionError<Av1SessionError> {
    // A cancelled decode is the caller's stop, not a stream error.
    match e.error() {
        rav1d_safe::Error::Cancelled => SessionError::Cancelled(enough::StopReason::Cancelled),
        other => SessionError::Codec(Av1SessionError::Decode(other.clone())),
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
    let (mut size, mut shift) = (0usize, 0u32);
    loop {
        let Some(&byte) = b.get(i) else { return 0 };
        i += 1;
        size |= usize::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 56 {
            return 0;
        }
    }
    if size == 0 { i } else { 0 }
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
    config: Option<CodecConfig>,
    /// Sequence-header OBUs still to prepend to the next packet.
    prefix: Option<Arc<[u8]>>,
    /// An accepted packet rav1d did not take yet.
    stash: Option<rav1d_safe::Packet>,
    /// `end_input` was called; rav1d is told once the stash is empty.
    ended: bool,
    told_end: bool,
    /// Input offsets by packet ordinal (rav1d carries one i64 per packet).
    offsets: BTreeMap<u64, u64>,
    next_ordinal: u64,
    info: Option<VideoStreamInfo>,
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
    /// grain, operating point).
    ///
    /// # Errors
    ///
    /// When rav1d cannot start.
    pub fn with_settings(settings: rav1d_safe::Settings) -> Result<Self, Av1SessionError> {
        let dec = rav1d_safe::Decoder::with_settings(settings)
            .map_err(|e| Av1SessionError::Decode(e.error().clone()))?;
        Ok(Self {
            dec,
            config: None,
            prefix: None,
            stash: None,
            ended: false,
            told_end: false,
            offsets: BTreeMap::new(),
            next_ordinal: 0,
            info: None,
        })
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
        match self.dec.send_packet(p).map_err(decode_err)? {
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
        let config = self.config.as_ref().expect("frames follow configure");
        let bits = f.bit_depth();
        let encoding = match bits {
            8 => SampleEncoding::U8,
            10 | 12 => SampleEncoding::new(ChannelType::U16, bits, 0).expect("valid depth"),
            b => return Err(SessionError::Codec(Av1SessionError::UnsupportedBitDepth(b))),
        };
        let raw = f.raw_color_info();
        let subsampling = match f.pixel_layout() {
            rav1d_safe::PixelLayout::I400 => Subsampling::Mono,
            rav1d_safe::PixelLayout::I420 => Subsampling::Yuv420,
            rav1d_safe::PixelLayout::I422 => Subsampling::Yuv422,
            _ => Subsampling::Yuv444,
        };
        let roles = plane_roles(subsampling, raw.matrix_coefficients);
        let siting = match raw.chroma_sample_position {
            1 => ChromaSiting::Left,
            2 => ChromaSiting::TopLeft,
            _ => ChromaSiting::Unknown,
        };
        let (w, h) = (f.width(), f.height());
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
        let interpretation =
            FrameInterpretation::new(geometry, subsampling, siting, encoding, roles)
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
        let ordinal = u64::try_from(f.input_offset()).ok();
        let mut timing = FrameTiming::default();
        timing.pts = pts;
        if let Some(d) = u64::try_from(f.duration()).ok().filter(|&d| d > 0) {
            timing = timing.with_duration_ticks(d);
        }
        let input_offset = ordinal.and_then(|o| self.offsets.get(&o).copied());
        // Offsets of packets at or before this one are no longer needed,
        // except for show-existing repeats, which carry their own ordinal.
        if let Some(o) = ordinal {
            self.offsets.retain(|&k, _| k > o);
        }
        timing = timing.with_packet(ordinal, input_offset);
        let mut presentation = Presentation::default();
        presentation.keyframe = f.is_keyframe();
        presentation.show_existing = f.is_show_existing();
        Ok(Frame::new(
            Arc::new(Av1Picture(f)),
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

    fn configure(&mut self, config: &CodecConfig, stop: &dyn Stop) -> Res<()> {
        stop.check()?;
        if config.codec != CodecId::Av1 {
            return Err(SessionError::Unsupported(UnsupportedMedia::Codec));
        }
        self.config = Some(config.clone());
        self.arm_prefix();
        Ok(())
    }

    fn send(&mut self, packet: &mut Packet, stop: &dyn Stop) -> Res<SendStatus> {
        stop.check()?;
        let Some(config) = &self.config else {
            return Err(SessionError::Protocol(ProtocolViolation::NotConfigured));
        };
        if self.ended {
            return Err(SessionError::Protocol(ProtocolViolation::SendAfterEnd));
        }
        if packet.config_epoch() != config.epoch {
            return Err(SessionError::Protocol(ProtocolViolation::EpochMismatch));
        }
        if !self.flush_stash()? {
            return Ok(SendStatus::ReceivePending);
        }
        let pts = packet.pts().map_or(i64::MIN, |t| t.ticks());
        let duration = packet
            .duration_ticks()
            .and_then(|d| i64::try_from(d).ok())
            .unwrap_or(0);
        let ordinal = packet.ordinal().unwrap_or(self.next_ordinal);
        self.next_ordinal = ordinal + 1;
        if let Some(offset) = packet.input_offset() {
            self.offsets.insert(ordinal, offset);
        }
        let payload = packet
            .take()
            .ok_or(SessionError::Protocol(ProtocolViolation::EmptyPacket))?;
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
            .with_offset(i64::try_from(ordinal).unwrap_or(-1));
        match self.dec.send_packet(&mut p).map_err(decode_err)? {
            rav1d_safe::SendStatus::Accepted => {}
            // Accepted by the session; rav1d takes it once output drains.
            _ => self.stash = Some(p),
        }
        Ok(SendStatus::Accepted)
    }

    fn receive(&mut self, stop: &dyn Stop) -> Res<Receive<Frame>> {
        stop.check()?;
        if self.config.is_none() {
            return Err(SessionError::Protocol(ProtocolViolation::NotConfigured));
        }
        loop {
            self.tell_end_if_ready()?;
            match self.dec.receive().map_err(decode_err)? {
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

    fn end_input(&mut self) -> Res<()> {
        self.ended = true;
        self.tell_end_if_ready()
    }

    fn reset(&mut self) -> Res<()> {
        self.dec.reset();
        self.stash = None;
        self.ended = false;
        self.told_end = false;
        self.offsets.clear();
        self.arm_prefix();
        Ok(())
    }

    fn stream_info(&self) -> Option<&VideoStreamInfo> {
        self.info.as_ref()
    }
}

/// A rav1d picture as a zencodec-media [`Picture`]: refcounted, mapped in
/// place.
struct Av1Picture(rav1d_safe::Frame);

impl Picture for Av1Picture {
    fn map(&self) -> Result<Box<dyn MappedPlanes + '_>, MapError> {
        let bits = self.0.bit_depth();
        Ok(match self.0.planes() {
            rav1d_safe::Planes::Depth8(p) => Box::new(Planes8 {
                views: [Some(p.y()), p.u(), p.v()],
            }),
            rav1d_safe::Planes::Depth16(p) => Box::new(Planes16 {
                views: [Some(p.y()), p.u(), p.v()],
                encoding: SampleEncoding::new(ChannelType::U16, bits, 0)
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

    fn plane(&self, index: usize) -> Option<PlaneView<'_>> {
        let v = self.views.get(index)?.as_ref()?;
        PlaneView::new(
            PlaneSamples::U8(v.as_slice()),
            v.width(),
            v.height(),
            v.stride(),
            SampleEncoding::U8,
        )
        .ok()
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

    fn plane(&self, index: usize) -> Option<PlaneView<'_>> {
        let v = self.views.get(index)?.as_ref()?;
        PlaneView::new(
            PlaneSamples::U16(v.as_slice()),
            v.width(),
            v.height(),
            v.stride() * 2,
            self.encoding,
        )
        .ok()
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

#[cfg(feature = "media-encode")]
pub use encode::{Av1EncodeSettings, Av1VideoEncoder};

#[cfg(feature = "media-encode")]
mod encode {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use enough::Stop;
    use num_traits::FromPrimitive;
    use zencodec::time::{TimeBase, Timestamp};
    use zencodec_media::frame::{ChromaSiting, Frame, PlaneSamples, SampleEncoding, Subsampling};
    use zencodec_media::packet::{Packet, RandomAccess};
    use zencodec_media::session::{
        CodecConfig, ProtocolViolation, Receive, SendStatus, SessionError, VideoEncoder,
    };
    use zencodec_media::track::{CodecId, VideoTrackInfo};
    use zenpixels::Cicp;
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
        /// zenrav1e speed preset, 0 (slowest) to 10.
        pub speed: u8,
        /// Base quantizer, 0 (lossless) to 255.
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
                speed: 6,
                quantizer: 100,
                key_frame_interval: 240,
                threads: 0,
                max_queued_frames: 64,
                frame_rate: (30, 1),
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

        /// The session's queue length.
        pub fn with_max_queued_frames(mut self, frames: usize) -> Self {
            self.max_queued_frames = frames;
            self
        }

        /// The nominal frame rate, `numerator / denominator` frames per
        /// second.
        pub fn with_frame_rate(mut self, numerator: u32, denominator: u32) -> Self {
            self.frame_rate = (numerator, denominator);
            self
        }
    }

    fn cfg_err(m: &str) -> Av1SessionError {
        Av1SessionError::Configuration(m.into())
    }

    enum Backend {
        U8(Box<Context<u8>>),
        U16(Box<Context<u16>>),
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
            // Timestamps come from the frames; nothing in the bitstream
            // claims an equal-picture rate. zenrav1e's time base is seconds
            // per frame: the nominal rate, never the tick clock (a 1/90000
            // clock read as 90000 fps forces a tile per superblock row).
            enc.enable_timing_info = false;
            enc.still_picture = false;
            enc.time_base = zenrav1e::prelude::Rational::new(
                u64::from(s.frame_rate.1),
                u64::from(s.frame_rate.0),
            );
            let config = zenrav1e::Config::new()
                .with_encoder_config(enc)
                .with_threads(s.threads);
            let backend = if s.bit_depth == 8 {
                Backend::U8(Box::new(
                    config.new_context().map_err(|e| cfg_err(&e.to_string()))?,
                ))
            } else {
                Backend::U16(Box::new(
                    config.new_context().map_err(|e| cfg_err(&e.to_string()))?,
                ))
            };
            let mut av1c = match &backend {
                Backend::U8(c) => c.container_sequence_header(),
                Backend::U16(c) => c.container_sequence_header(),
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
                queued: BTreeMap::new(),
                submitted: 0,
                emitted: 0,
                last_pts: None,
                ready: None,
                ended: false,
                drained: false,
            })
        }

        fn matches(&self, frame: &Frame) -> bool {
            let i = frame.interpretation();
            let s = &self.settings;
            let g = &i.geometry;
            (g.coded_width, g.coded_height) == (s.width, s.height)
                && (g.crop.x, g.crop.y, g.crop.width, g.crop.height) == (0, 0, s.width, s.height)
                && i.subsampling == s.subsampling
                && i.encoding.code_bits() == s.bit_depth
                && i.encoding.bit_shift() == 0
                && (!matches!(s.subsampling, Subsampling::Yuv420) || i.siting == s.siting)
                && i.roles == plane_roles(s.subsampling, s.color.matrix_coefficients)
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
                };
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
            if (plane.width(), plane.height()) != (w, h) || plane.encoding() != encoding {
                return Err(SendError::Input);
            }
        }
        let mut out = ctx.new_frame();
        for (index, &(w, h)) in dims.iter().enumerate() {
            let plane = mapped.plane(index).ok_or(SendError::Input)?;
            let mut slice = out.planes[index].mut_slice(Default::default());
            for (y, row) in slice.rows_iter_mut().take(h).enumerate() {
                let dst = row.get_mut(..w).ok_or(SendError::Input)?;
                match plane.row(y).ok_or(SendError::Input)? {
                    PlaneSamples::U8(src) => {
                        for (d, &v) in dst.iter_mut().zip(src) {
                            *d = P::cast_from(v);
                        }
                    }
                    PlaneSamples::U16(src) => {
                        for (d, &v) in dst.iter_mut().zip(src) {
                            *d = P::cast_from(v);
                        }
                    }
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

    impl VideoEncoder for Av1VideoEncoder {
        type Error = Av1SessionError;

        fn send(&mut self, frame: &Frame, stop: &dyn Stop) -> Res<SendStatus> {
            stop.check()?;
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
            if self.queued.len() >= self.settings.max_queued_frames {
                // Push back only with a packet to hand out, so the
                // `receive` that follows is `Ready`.
                if let Pulled::Packet(p) = self.pull()? {
                    self.ready = Some(p);
                    return Ok(SendStatus::ReceivePending);
                }
            }
            let dims = self.plane_dims();
            let encoding = frame.interpretation().encoding;
            let sent = match &mut self.backend {
                Backend::U8(c) => copy_and_send(c, frame, &dims, encoding),
                Backend::U16(c) => copy_and_send(c, frame, &dims, encoding),
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

        fn receive(&mut self, stop: &dyn Stop) -> Res<Receive<Packet>> {
            stop.check()?;
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

        fn end_input(&mut self) -> Res<()> {
            if !self.ended {
                match &mut self.backend {
                    Backend::U8(c) => c.flush(),
                    Backend::U16(c) => c.flush(),
                }
                self.ended = true;
            }
            Ok(())
        }

        fn output_config(&self) -> Option<&CodecConfig> {
            Some(&self.output)
        }
    }
}

//! MP4 (ISO base media file format, ISO/IEC 14496-12) as a sans-I/O
//! [`Demuxer`](zencodec_media::container::Demuxer) and a pull-output
//! [`Muxer`](zencodec_media::container::Muxer) for `zencodec-media`.
//!
//! - [`Mp4Demuxer`] reads progressive files (one `moov` holding every
//!   sample table, before or after `mdat`) and fragmented files (`moov` with
//!   `mvex`, then `moof` + `mdat` pairs). Samples come out in file order
//!   with decode and presentation timestamps (`stts`, `ctts` or `trun`),
//!   durations and random-access evidence (`stss`, sample flags). Edit lists
//!   are carried, not applied, through the complete
//!   [`TrackInfo::timeline`](zencodec_media::track::TrackInfo::timeline).
//!   Ordered gaps, media segments, exact rates and independent clocks survive;
//!   unsupported edit versions refuse rather than becoming identity.
//!   Codec configuration records become `codec_private`: `av1C`, `vpcC`,
//!   `avcC` and `hvcC` payloads as they are, Opus `dOps` as an `OpusHead`,
//!   AAC `esds` as its AudioSpecificConfig, FLAC `dfLa` as `fLaC` plus its
//!   metadata blocks. Seeking works in both layouts: a fragmented file's
//!   first seek indexes every `moof`, skipping each `mdat`.
//! - [`Mp4Muxer`] writes progressive files (`mdat` streamed, `moov` at the
//!   end, one patch for the `mdat` size) or, for destinations that cannot
//!   seek back, fragmented ones (`moov` first, a fragment per video
//!   random-access point, no patches). Sample durations come from the next
//!   sample's decode time, so they are exact; the last uses its packet's
//!   duration or the step before. An AV1 track without an `av1C` gets one
//!   built from its first sequence header (the fragmented `moov` waits
//!   for it).
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use core::fmt;

use zencodec_media::track::{CodecId, CodecTag};

mod av1;
mod boxes;
mod demux;
mod mux;

pub use demux::Mp4Demuxer;
pub use mux::Mp4Muxer;

/// Malformed or unsupported MP4.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mp4Error {
    /// A box or field runs past its parent or the file.
    Truncated,
    /// A box size smaller than its header.
    BadBoxSize,
    /// The file has no `moov`.
    NoMovie,
    /// An `mdat` that runs to the end of the file comes before `moov`.
    MovieAfterOpenMdat,
    /// A box this demuxer needs appears twice (`moov`, a track's `stbl`).
    Duplicate(&'static str),
    /// A track lacks a box it needs.
    Missing(&'static str),
    /// A timescale of zero.
    ZeroTimescale,
    /// A sample table that contradicts itself (counts, chunk runs, offsets).
    InconsistentTable(&'static str),
    /// A sample with no bytes.
    EmptySample,
    /// More than one sample description in a track: configuration changes
    /// mid-track are not supported yet.
    MultipleSampleDescriptions,
    /// A fragment names a track `moov` does not declare.
    UnknownTrack(u32),
    /// A timestamp or offset overflows 64 bits.
    Overflow,
    /// A seek before the headers were read.
    HeadersFirst,
    /// No sample satisfies the seek.
    NoSuchSample,
    /// The muxer has no MP4 sample entry for this codec.
    UnsupportedCodec,
    /// Width or height over 65535.
    DimensionsTooLarge,
    /// `codec_private` is not in the form this codec's sample entry needs.
    BadCodecPrivate,
    /// A track clock other than `1/timescale`.
    UnsupportedTimeBase,
    /// Decode times that do not increase within a track.
    DecodeOrder,
    /// Exact timing was requested but a packet omitted its sample duration.
    MissingSampleDuration,
}

impl fmt::Display for Mp4Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("MP4 box or field is truncated"),
            Self::BadBoxSize => f.write_str("MP4 box size is smaller than its header"),
            Self::NoMovie => f.write_str("MP4 file has no moov box"),
            Self::MovieAfterOpenMdat => {
                f.write_str("MP4 mdat runs to the end of the file before any moov")
            }
            Self::Duplicate(b) => write!(f, "MP4 has a second {b} box"),
            Self::Missing(b) => write!(f, "MP4 track has no {b} box"),
            Self::ZeroTimescale => f.write_str("MP4 timescale is zero"),
            Self::InconsistentTable(t) => write!(f, "MP4 sample table is inconsistent: {t}"),
            Self::EmptySample => f.write_str("MP4 sample has no bytes"),
            Self::MultipleSampleDescriptions => {
                f.write_str("MP4 track has more than one sample description")
            }
            Self::UnknownTrack(id) => write!(f, "MP4 fragment names unknown track {id}"),
            Self::Overflow => f.write_str("MP4 timestamp or offset overflows"),
            Self::HeadersFirst => f.write_str("seek before the MP4 headers were read"),
            Self::NoSuchSample => f.write_str("no MP4 sample satisfies the seek"),
            Self::UnsupportedCodec => f.write_str("no MP4 sample entry for this codec"),
            Self::DimensionsTooLarge => f.write_str("MP4 sample entries hold 16-bit dimensions"),
            Self::BadCodecPrivate => {
                f.write_str("codec_private is not in the form the MP4 sample entry needs")
            }
            Self::UnsupportedTimeBase => f.write_str("MP4 track clocks must be 1/timescale"),
            Self::DecodeOrder => f.write_str("MP4 decode times must increase within a track"),
            Self::MissingSampleDuration => {
                f.write_str("exact MP4 timing requires an explicit packet duration")
            }
        }
    }
}

impl core::error::Error for Mp4Error {}

/// The codec an MP4 sample entry names (its four-character code; `mp4a`
/// is resolved by its object type indication, `oti`).
pub fn codec_from_sample_entry(fourcc: [u8; 4], oti: Option<u8>) -> CodecId {
    match &fourcc {
        b"av01" => CodecId::Av1,
        b"vp09" => CodecId::Vp9,
        b"vp08" => CodecId::Vp8,
        b"avc1" | b"avc3" => CodecId::H264,
        b"hvc1" | b"hev1" => CodecId::Hevc,
        b"Opus" => CodecId::Opus,
        b"fLaC" => CodecId::Flac,
        b"mp4a" if oti == Some(0x40) || matches!(oti, Some(0x66..=0x68)) => CodecId::Aac,
        b"mp4a" => {
            let mut tag = String::from("mp4a");
            if let Some(o) = oti {
                tag.push_str(&alloc::format!(".{o:02X}"));
            }
            CodecId::Other(CodecTag::new(&tag))
        }
        _ => CodecId::Other(CodecTag::new(&String::from_utf8_lossy(&fourcc))),
    }
}

//! MP4 (ISO base media file format, ISO/IEC 14496-12) as a sans-I/O
//! [`Demuxer`](zencodec_media::container::Demuxer) and a pull-output
//! [`Muxer`](zencodec_media::container::Muxer) for `zencodec-media`.
//!
//! - [`Mp4Demuxer`] reads progressive files (one `moov` holding every
//!   sample table, before or after `mdat`) and fragmented files (`moov` with
//!   `mvex`, then `moof` + `mdat` pairs). Samples come out in file order
//!   with decode and presentation timestamps (`stts`, `ctts` or `trun`),
//!   durations and random-access evidence (`stss`, sample flags). Edit lists
//!   are carried, not applied: a single edit becomes
//!   [`TrackInfo::edit_delay_ticks`](zencodec_media::track::TrackInfo::edit_delay_ticks).
//!   Codec configuration records become `codec_private`: `av1C`, `vpcC`,
//!   `avcC` and `hvcC` payloads as they are, Opus `dOps` as an `OpusHead`,
//!   AAC `esds` as its AudioSpecificConfig, FLAC `dfLa` as `fLaC` plus its
//!   metadata blocks.
//! - The muxer is not written yet.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use core::fmt;

use zencodec_media::track::{CodecId, CodecTag};

mod boxes;
mod demux;

pub use demux::Mp4Demuxer;

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

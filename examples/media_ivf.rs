//! Raw planar video <-> IVF through the `media` sessions, for differential
//! checks against FFmpeg (`scripts/media-ffmpeg-oracle.sh`).
//!
//! ```text
//! media_ivf encode <in.yuv> <pix_fmt> <width> <height> <out.ivf> [quantizer] [speed] [svt]
//! media_ivf decode <in.ivf> <out.yuv>
//! ```
//!
//! A trailing `svt` encodes with zenav1-svt (all-intra; needs the
//! `zenav1-svt` feature) instead of zenrav1e.
//!
//! `pix_fmt` uses FFmpeg's names: `gray`, `yuv420p`, `yuv422p`, `yuv444p`,
//! each optionally with a `10le` or `12le` suffix. Frames are timestamped
//! 0, 1, 2, ... on a 1/25 clock. `decode` writes the planes in the same
//! layout (16-bit samples little-endian) and prints one line per frame:
//! `pts keyframe`.

use std::io::Write;
use std::sync::Arc;

use enough::Unstoppable;
use zenavif::media::{Av1EncodeSettings, Av1VideoDecoder, Av1VideoEncoder};
use zencodec::time::{TimeBase, Timestamp};
use zencodec_media::frame::{
    ChromaSiting, Frame, FrameGeometry, FrameInterpretation, FrameTiming, OwnedPlane, OwnedPlanes,
    PlaneData, PlaneRole, PlaneSamples, Presentation, SampleEncoding, Subsampling,
};
use zencodec_media::packet::Packet;
use zencodec_media::session::{CodecConfig, Receive, SendStatus, VideoDecoder, VideoEncoder};
use zencodec_media::track::CodecId;
use zenpixels::ChannelType;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn pix_fmt(name: &str) -> Res<(Subsampling, u8)> {
    let (base, bits) = match name.strip_suffix("10le") {
        Some(b) => (b, 10),
        None => match name.strip_suffix("12le") {
            Some(b) => (b, 12),
            None => (name, 8),
        },
    };
    let sub = match base {
        "gray" => Subsampling::Mono,
        "yuv420p" => Subsampling::Yuv420,
        "yuv422p" => Subsampling::Yuv422,
        "yuv444p" => Subsampling::Yuv444,
        _ => return Err(format!("unsupported pix_fmt {name}").into()),
    };
    Ok((sub, bits))
}

fn plane_dims(sub: Subsampling, w: usize, h: usize) -> Vec<(usize, usize)> {
    match sub {
        Subsampling::Mono => vec![(w, h)],
        Subsampling::Yuv420 => vec![
            (w, h),
            (w.div_ceil(2), h.div_ceil(2)),
            (w.div_ceil(2), h.div_ceil(2)),
        ],
        Subsampling::Yuv422 => vec![(w, h), (w.div_ceil(2), h), (w.div_ceil(2), h)],
        _ => vec![(w, h); 3],
    }
}

fn ivf_header(out: &mut Vec<u8>, w: u16, h: u16, tb: TimeBase, frames: u32) {
    out.extend_from_slice(b"DKIF");
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"AV01");
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&tb.denominator().to_le_bytes());
    out.extend_from_slice(&tb.numerator().to_le_bytes());
    out.extend_from_slice(&frames.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
}

fn encode(args: &[String]) -> Res<()> {
    let [input, fmt, w, h, output, rest @ ..] = args else {
        return Err(
            "encode <in.yuv> <pix_fmt> <width> <height> <out.ivf> [quantizer] [speed] [svt]".into(),
        );
    };
    let (sub, bits) = pix_fmt(fmt)?;
    let (w, h): (u32, u32) = (w.parse()?, h.parse()?);
    let quantizer: u8 = rest.first().map_or(Ok(100), |q| q.parse())?;
    let speed: u8 = rest.get(1).map_or(Ok(6), |s| s.parse())?;
    let svt = rest.get(2).is_some_and(|b| b == "svt");
    let raw = std::fs::read(input)?;
    let dims = plane_dims(sub, w as usize, h as usize);
    let bps = if bits == 8 { 1 } else { 2 };
    let frame_bytes: usize = dims.iter().map(|(pw, ph)| pw * ph * bps).sum();
    if raw.len() % frame_bytes != 0 {
        return Err("input is not a whole number of frames".into());
    }
    let tb = TimeBase::new(1, 25)?;
    let encoding = if bits == 8 {
        SampleEncoding::U8
    } else {
        SampleEncoding::new(ChannelType::U16, bits, 0)?
    };
    let roles = if matches!(sub, Subsampling::Mono) {
        vec![PlaneRole::Luma]
    } else {
        vec![PlaneRole::Luma, PlaneRole::Cb, PlaneRole::Cr]
    };
    let interp = Arc::new(FrameInterpretation::new(
        FrameGeometry::full(w, h),
        sub,
        ChromaSiting::Unknown,
        encoding,
        roles,
    ));
    let mut settings = Av1EncodeSettings::new(w, h)
        .with_bit_depth(bits)
        .with_subsampling(sub)
        .with_quantizer(quantizer)
        .with_speed(speed);
    if svt {
        settings = settings
            .with_backend(zenavif::media::Av1VideoBackend::Zenav1Svt)
            .with_key_frame_interval(1);
    }
    let mut enc = Av1VideoEncoder::new(settings, tb)?;
    let mut packets: Vec<Packet> = Vec::new();
    for (i, chunk) in raw.chunks_exact(frame_bytes).enumerate() {
        let mut at = 0;
        let planes = dims
            .iter()
            .map(|&(pw, ph)| {
                let n = pw * ph * bps;
                let bytes = &chunk[at..at + n];
                at += n;
                let data = if bits == 8 {
                    PlaneData::U8(bytes.to_vec())
                } else {
                    PlaneData::U16(
                        bytes
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|b| u16::from_le_bytes(*b))
                            .collect(),
                    )
                };
                OwnedPlane {
                    data,
                    width: pw,
                    height: ph,
                    stride_bytes: pw * bps,
                }
            })
            .collect();
        let frame = Frame::new(
            Arc::new(OwnedPlanes::new(planes, encoding)?),
            Arc::clone(&interp),
            FrameTiming::at(Timestamp::new(i as i64, tb)).with_duration_ticks(1),
            Presentation::default(),
        );
        loop {
            match enc.send(&frame, &Unstoppable)? {
                SendStatus::Accepted => break,
                _ => {
                    if let Receive::Ready(p) = enc.receive(&Unstoppable)? {
                        packets.push(p);
                    }
                }
            }
        }
        while let Receive::Ready(p) = enc.receive(&Unstoppable)? {
            packets.push(p);
        }
    }
    enc.end_input()?;
    while let Receive::Ready(p) = enc.receive(&Unstoppable)? {
        packets.push(p);
    }
    let mut out = Vec::new();
    ivf_header(&mut out, w as u16, h as u16, tb, packets.len() as u32);
    for p in &packets {
        out.extend_from_slice(&(p.bytes().len() as u32).to_le_bytes());
        out.extend_from_slice(&p.pts().ok_or("packet without pts")?.ticks().to_le_bytes());
        out.extend_from_slice(p.bytes());
    }
    std::fs::write(output, out)?;
    Ok(())
}

fn decode(args: &[String]) -> Res<()> {
    let [input, output] = args else {
        return Err("decode <in.ivf> <out.yuv>".into());
    };
    let bytes = std::fs::read(input)?;
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    if bytes.get(..4) != Some(b"DKIF") {
        return Err("not an IVF file".into());
    }
    let tb = TimeBase::new(u32_at(20), u32_at(16))?;
    let mut pos = usize::from(u16::from_le_bytes([bytes[6], bytes[7]]));
    let mut dec = Av1VideoDecoder::new()?;
    dec.configure(&CodecConfig::new(CodecId::Av1, tb), &Unstoppable)?;
    let mut frames = Vec::new();
    let mut ordinal = 0;
    while pos + 12 <= bytes.len() {
        let len = u32_at(pos) as usize;
        let pts = i64::from_le_bytes(bytes[pos + 4..pos + 12].try_into().unwrap());
        let mut p = Packet::from_vec(bytes[pos + 12..pos + 12 + len].to_vec())?
            .with_pts(Timestamp::new(pts, tb))
            .with_ordinal(ordinal);
        ordinal += 1;
        pos += 12 + len;
        loop {
            match dec.send(&mut p, &Unstoppable)? {
                SendStatus::Accepted => break,
                _ => {
                    if let Receive::Ready(f) = dec.receive(&Unstoppable)? {
                        frames.push(f);
                    }
                }
            }
        }
        while let Receive::Ready(f) = dec.receive(&Unstoppable)? {
            frames.push(f);
        }
    }
    dec.end_input()?;
    while let Receive::Ready(f) = dec.receive(&Unstoppable)? {
        frames.push(f);
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(output)?);
    let stdout = std::io::stdout();
    let mut log = stdout.lock();
    for f in &frames {
        let m = f.map()?;
        for c in 0..m.plane_count() {
            let plane = m.plane(c).ok_or("plane")?;
            for y in 0..plane.height() {
                match plane.row(y).ok_or("row")? {
                    PlaneSamples::U8(r) => out.write_all(&r[..plane.width()])?,
                    PlaneSamples::U16(r) => {
                        for v in &r[..plane.width()] {
                            out.write_all(&v.to_le_bytes())?;
                        }
                    }
                }
            }
        }
        let pts = f.timing().pts.map_or(-1, |t| t.ticks());
        writeln!(log, "{pts} {}", u8::from(f.presentation().keyframe))?;
    }
    out.flush()?;
    Ok(())
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("encode") => encode(&args[1..]),
        Some("decode") => decode(&args[1..]),
        _ => Err("usage: media_ivf encode|decode ...".into()),
    }
}

//! SVT sync-sample animation using the same pixel coding as still images.
use super::*;
use crate::encoder::AnimationInput;
use rgb::{RGB8, RGB16, RGBA8, RGBA16};
use zenavif_serialize::{
    Av1CBox,
    animated::{AnimFrame, AnimatedImage},
};

pub(crate) fn encode_animation_rgb8<F: AnimationInput<RGB8>>(
    frames: &[F],
    timescale: u32,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<crate::EncodedAnimation> {
    encode_frames(
        frames,
        timescale,
        config,
        stop,
        |f| (f.pixels().width(), f.pixels().height(), f.duration_ticks()),
        |f, mode, token| encode_rgb8_frame(f.pixels(), config, token, mode),
    )
}

pub(crate) fn encode_animation_rgba8<F: AnimationInput<RGBA8>>(
    frames: &[F],
    timescale: u32,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<crate::EncodedAnimation> {
    encode_frames(
        frames,
        timescale,
        config,
        stop,
        |f| (f.pixels().width(), f.pixels().height(), f.duration_ticks()),
        |f, mode, token| encode_rgba8_frame(f.pixels(), config, token, mode),
    )
}

pub(crate) fn encode_animation_rgb16<F: AnimationInput<RGB16>>(
    frames: &[F],
    timescale: u32,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<crate::EncodedAnimation> {
    encode_frames(
        frames,
        timescale,
        config,
        stop,
        |f| (f.pixels().width(), f.pixels().height(), f.duration_ticks()),
        |f, mode, token| encode_rgb16_frame(f.pixels(), config, token, mode),
    )
}

pub(crate) fn encode_animation_rgba16<F: AnimationInput<RGBA16>>(
    frames: &[F],
    timescale: u32,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
) -> Result<crate::EncodedAnimation> {
    encode_frames(
        frames,
        timescale,
        config,
        stop,
        |f| (f.pixels().width(), f.pixels().height(), f.duration_ticks()),
        |f, mode, token| encode_rgba16_frame(f.pixels(), config, token, mode),
    )
}

fn encode_frames<F>(
    frames: &[F],
    timescale: u32,
    config: &EncoderConfig,
    stop: almost_enough::StopToken,
    layout: impl Fn(&F) -> (usize, usize, u32),
    encode: impl Fn(&F, FrameMode, almost_enough::StopToken) -> Result<CodedSvtFrame>,
) -> Result<crate::EncodedAnimation> {
    stop.check().map_err(|e| at!(Error::from(e)))?;
    reject_unsupported_config(config)?;
    let first = frames
        .first()
        .ok_or_else(|| at!(Error::Encode("animation needs at least one frame".into())))?;
    let (width, height, _) = layout(first);
    if width == 0 || height == 0 || width > u16::MAX as usize || height > u16::MAX as usize {
        return Err(at!(Error::Encode(
            "animation dimensions must be in 1..=65535".into()
        )));
    }
    if frames.len() > u32::MAX as usize {
        return Err(at!(Error::Encode(
            "animation frame count exceeds u32".into()
        )));
    }
    let mut shortest = u32::MAX;
    let mut total_duration_ticks = 0u64;
    for frame in frames {
        stop.check().map_err(|e| at!(Error::from(e)))?;
        let (w, h, duration) = layout(frame);
        if (w, h) != (width, height) || duration == 0 {
            return Err(at!(Error::Encode(
                "animation frames need equal dimensions and positive durations".into()
            )));
        }
        shortest = shortest.min(duration);
        total_duration_ticks = total_duration_ticks
            .checked_add(u64::from(duration))
            .ok_or_else(|| at!(Error::Encode("animation duration overflow".into())))?;
    }
    let framerate = f64::from(timescale) / f64::from(shortest);
    let mode = FrameMode::Sequence { framerate };
    let mut coded = Vec::new();
    coded
        .try_reserve_exact(frames.len())
        .map_err(|_| at!(Error::OutOfMemory))?;
    for frame in frames {
        stop.check().map_err(|e| at!(Error::from(e)))?;
        coded.push(encode(frame, mode, stop.clone())?);
    }
    let first = &coded[0];
    let color_seq = sequence_header(&first.color)?;
    let alpha_seq = first.alpha.as_deref().map(sequence_header).transpose()?;
    let mut av1_config = Av1CBox::default();
    av1_config.high_bitdepth = first.bit_depth > 8;
    av1_config.monochrome = first.monochrome;
    av1_config.seq_level_idx_0 =
        svtav1::entropy::obu::compute_seq_level_idx(first.width, first.height, framerate);
    let mut mux = AnimatedImage::new();
    mux.set_timescale(timescale)
        .set_color_config(av1_config)
        .set_color_description(
            u16::from(first.color_primaries),
            u16::from(first.transfer_characteristics),
            u16::from(MATRIX_COEFFICIENTS_BT601),
            true,
        );
    mux.set_premultiplied_alpha(
        alpha_seq.is_some() && config.alpha_color_mode == crate::EncodeAlphaMode::Premultiplied,
    );
    if alpha_seq.is_some() {
        let mut alpha_config = av1_config;
        alpha_config.monochrome = true;
        mux.set_alpha_config(alpha_config);
    }
    if let Some(icc) = &config.icc_profile {
        mux.set_icc_profile(icc.clone());
    }
    if let Some(exif) = &config.exif {
        mux.set_exif(exif.clone());
    }
    if let Some(xmp) = &config.xmp {
        mux.set_xmp(xmp.clone());
    }
    if let Some(angle) = config.rotation {
        mux.set_rotation(angle);
    }
    if let Some(axis) = config.mirror {
        mux.set_mirror(axis);
    }
    if let Some((cll, fall)) = config.content_light_level {
        mux.set_clli(zenavif_serialize::ClliBox::new(cll, fall));
    }
    if let Some(md) = config.mastering_display {
        mux.set_mdcv(zenavif_serialize::MdcvBox::new(
            md.primaries,
            md.white_point,
            md.max_luminance,
            md.min_luminance,
        ));
    }
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(frames.len())
        .map_err(|_| at!(Error::OutOfMemory))?;
    for (pixels, source) in coded.iter().zip(frames) {
        let sample =
            AnimFrame::new(without_delimiter(&pixels.color), layout(source).2).with_sync(true);
        samples.push(if let Some(alpha) = &pixels.alpha {
            sample.with_alpha(without_delimiter(alpha))
        } else {
            sample
        });
    }
    stop.check().map_err(|e| at!(Error::from(e)))?;
    let avif_file = mux
        .try_serialize(first.width, first.height, &samples, color_seq, alpha_seq)
        .map_err(|e| {
            at!(Error::Encode(format!(
                "AVIF animation serialization failed: {e}"
            )))
        })?;
    stop.check().map_err(|e| at!(Error::from(e)))?;
    Ok(crate::EncodedAnimation {
        avif_file,
        frame_count: frames.len(),
        total_duration_ticks,
        timescale,
        total_duration_ms: u64::try_from(
            u128::from(total_duration_ticks) * 1000 / u128::from(timescale),
        )
        .map_err(|_| {
            at!(Error::Encode(
                "animation millisecond duration overflow".into()
            ))
        })?,
    })
}

fn without_delimiter(data: &[u8]) -> &[u8] {
    data.strip_prefix(&[0x12, 0x00]).unwrap_or(data)
}

/// Borrow the complete sequence-header OBU, including its size prefix, for av1C.
fn sequence_header(data: &[u8]) -> Result<&[u8]> {
    let mut pos = 0;
    while let Some(&header) = data.get(pos) {
        let start = pos;
        pos += 1 + usize::from(header & 4 != 0);
        if header & 2 == 0 {
            break;
        }
        let mut size = 0u64;
        let mut complete = false;
        for shift in (0..56).step_by(7) {
            let Some(&byte) = data.get(pos) else {
                break;
            };
            pos += 1;
            let Some(part) = u64::from(byte & 127).checked_shl(shift) else {
                break;
            };
            size |= part;
            if byte & 128 == 0 {
                complete = true;
                break;
            }
        }
        let Some(end) = usize::try_from(size)
            .ok()
            .and_then(|size| pos.checked_add(size))
            .filter(|&end| complete && end <= data.len())
        else {
            break;
        };
        if (header >> 3) & 15 == 1 {
            return Ok(&data[start..end]);
        }
        pos = end;
    }
    Err(at!(Error::Encode(
        "SVT did not emit a valid sequence header".into()
    )))
}

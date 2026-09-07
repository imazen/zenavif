//! Repetition policy for containers produced by our animation encoders.
//!
//! Both backends use zenavif-serialize's version-1 movie/track headers and
//! single media edit. Change only fixed-width presentation fields; preserve
//! every sample, offset, property and metadata box. Validate the complete
//! generated layout before mutation. This is not a general AVIF editor.
use crate::{Result, error::Error};
use core::ops::Range;
use whereat::at;

#[derive(Clone)]
struct BoxRef {
    kind: [u8; 4],
    body: Range<usize>,
}
fn invalid() -> whereat::At<Error> {
    at!(Error::Encode(
        "unexpected generated animation layout for repetition update".into()
    ))
}
fn bytes<const N: usize>(data: &[u8], at: usize, end: usize) -> Result<[u8; N]> {
    let last = at
        .checked_add(N)
        .filter(|&v| v <= end)
        .ok_or_else(invalid)?;
    data.get(at..last)
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())
}
fn boxes(data: &[u8], range: Range<usize>) -> Result<Vec<BoxRef>> {
    let mut result = Vec::new();
    let mut pos = range.start;
    while pos < range.end {
        let n = u32::from_be_bytes(bytes(data, pos, range.end)?);
        let kind = bytes(data, pos + 4, range.end)?;
        let (n, header) = if n == 1 {
            (
                usize::try_from(u64::from_be_bytes(bytes(data, pos + 8, range.end)?))
                    .map_err(|_| invalid())?,
                16,
            )
        } else if n == 0 {
            (range.end - pos, 8)
        } else {
            (n as usize, 8)
        };
        let end = pos
            .checked_add(n)
            .filter(|&v| n >= header && v <= range.end)
            .ok_or_else(invalid)?;
        result.try_reserve(1).map_err(|_| at!(Error::OutOfMemory))?;
        result.push(BoxRef {
            kind,
            body: pos + header..end,
        });
        pos = end;
    }
    Ok(result)
}
fn one(items: &[BoxRef], kind: &[u8; 4]) -> Result<BoxRef> {
    let mut found = items.iter().filter(|b| &b.kind == kind);
    let b = found.next().ok_or_else(invalid)?;
    if found.next().is_some() {
        return Err(invalid());
    }
    Ok(b.clone())
}
fn version_one(data: &[u8], b: &BoxRef) -> Result<()> {
    if bytes::<1>(data, b.body.start, b.body.end)? != [1] {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn set_loop_count(data: &mut [u8], count: u32) -> Result<()> {
    let top = boxes(data, 0..data.len())?;
    let moov = one(&top, b"moov")?;
    let movie = boxes(data, moov.body)?;
    let mvhd = one(&movie, b"mvhd")?;
    version_one(data, &mvhd)?;
    if mvhd.body.len() < 32 {
        return Err(invalid());
    }
    let movie_duration = mvhd.body.start + 24;
    bytes::<8>(data, movie_duration, mvhd.body.end)?;
    let mut edits = Vec::new();
    let mut cycle = None;
    for track in movie.iter().filter(|b| &b.kind == b"trak") {
        let contents = boxes(data, track.body.clone())?;
        let tkhd = one(&contents, b"tkhd")?;
        version_one(data, &tkhd)?;
        if tkhd.body.len() < 36 {
            return Err(invalid());
        }
        let duration = tkhd.body.start + 28;
        bytes::<8>(data, duration, tkhd.body.end)?;
        let edts = one(&contents, b"edts")?;
        let elst = one(&boxes(data, edts.body)?, b"elst")?;
        version_one(data, &elst)?;
        if elst.body.len() < 28 {
            return Err(invalid());
        }
        if u32::from_be_bytes(bytes(data, elst.body.start + 4, elst.body.end)?) != 1 {
            return Err(invalid());
        }
        let segment = u64::from_be_bytes(bytes(data, elst.body.start + 8, elst.body.end)?);
        if segment == 0
            || cycle.is_some_and(|old| old != segment)
            || bytes::<8>(data, elst.body.start + 16, elst.body.end)? != [0; 8]
            || bytes::<4>(data, elst.body.start + 24, elst.body.end)? != [0, 1, 0, 0]
        {
            return Err(invalid());
        }
        cycle = Some(segment);
        edits.try_reserve(1).map_err(|_| at!(Error::OutOfMemory))?;
        edits.push((duration, elst.body.start));
    }
    let cycle = cycle.ok_or_else(invalid)?;
    let duration = if count == 0 {
        u64::MAX
    } else {
        cycle
            .checked_mul(u64::from(count))
            .filter(|&v| v != u64::MAX)
            .ok_or_else(|| at!(Error::Encode("repeated animation duration overflow".into())))?
    };
    data[movie_duration..movie_duration + 8].copy_from_slice(&duration.to_be_bytes());
    for (at, flags) in edits {
        data[at..at + 8].copy_from_slice(&duration.to_be_bytes());
        data[flags..flags + 4].copy_from_slice(&[1, 0, 0, u8::from(count != 1)]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflowing_repetition_does_not_partially_mutate_output() {
        use zenavif_serialize::animated::{AnimFrame, AnimatedImage};
        // Structural fixture: the repetition update must never inspect or
        // rewrite these opaque sample bytes.
        let frames = vec![AnimFrame::new(&[1, 2, 3], u32::MAX).with_sync(true); 3];
        let mut file = AnimatedImage::new()
            .try_serialize(32, 32, &frames, &[4, 5, 6], None)
            .unwrap();
        let original = file.clone();
        assert!(set_loop_count(&mut file, u32::MAX).is_err());
        assert_eq!(
            file, original,
            "validation must finish before any fields are changed"
        );
        assert!(set_loop_count(&mut file, 1).is_ok());
    }
}

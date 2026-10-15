//! An `av1C` record built from a temporal unit's sequence header, for AV1
//! tracks that reach the muxer without one (IVF has none; FFmpeg's
//! `empty_moov` writes an empty one).

use alloc::vec::Vec;

/// MSB-first bits.
struct Bits<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn f(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.b.get(self.pos / 8)?;
            v = v << 1 | u32::from((byte >> (7 - self.pos % 8)) & 1);
            self.pos += 1;
        }
        Some(v)
    }
}

/// The first sequence header OBU of a temporal unit (OBUs with size
/// fields), as `(whole OBU, payload)`. Only the OBUs before it are parsed.
fn sequence_header(mut b: &[u8]) -> Option<(&[u8], &[u8])> {
    const SEQUENCE_HEADER: u8 = 1;
    while !b.is_empty() {
        let header = b[0];
        if header & 0x80 != 0 || header & 0x2 == 0 {
            return None;
        }
        let mut i = 1 + usize::from(header & 0x4 != 0);
        let (mut size, mut shift) = (0usize, 0u32);
        loop {
            let byte = *b.get(i)?;
            i += 1;
            size |= usize::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 56 {
                return None;
            }
        }
        let end = i.checked_add(size).filter(|&e| e <= b.len())?;
        if (header >> 3) & 0xF == SEQUENCE_HEADER {
            return Some((&b[..end], &b[i..end]));
        }
        b = &b[end..];
    }
    None
}

/// The `av1C` record (with the sequence header as its `configOBUs`) for a
/// temporal unit that carries a sequence header, or `None`.
pub(crate) fn av1c_from_temporal_unit(tu: &[u8]) -> Option<Vec<u8>> {
    let (obu, payload) = sequence_header(tu)?;
    let mut r = Bits { b: payload, pos: 0 };
    let profile = r.f(3)?;
    r.f(1)?; // still picture
    let reduced = r.f(1)?;
    let (level, tier) = if reduced == 1 {
        (r.f(5)?, 0)
    } else {
        let timing = r.f(1)?;
        let mut decoder_model = 0;
        let mut buffer_delay_len = 0;
        if timing == 1 {
            r.f(32)?;
            r.f(32)?;
            if r.f(1)? == 1 {
                // num_ticks_per_picture_minus_1, uvlc
                let mut zeros = 0;
                while r.f(1)? == 0 {
                    zeros += 1;
                    if zeros > 32 {
                        return None;
                    }
                }
                if zeros > 0 {
                    r.f(zeros)?;
                }
            }
            decoder_model = r.f(1)?;
            if decoder_model == 1 {
                buffer_delay_len = r.f(5)? as usize + 1;
                r.f(32)?;
                r.f(10)?;
            }
        }
        let initial_display_delay = r.f(1)?;
        let ops = r.f(5)? + 1;
        let (mut level0, mut tier0) = (0, 0);
        for op in 0..ops {
            r.f(12)?;
            let lvl = r.f(5)?;
            let t = if lvl > 7 { r.f(1)? } else { 0 };
            if decoder_model == 1 && r.f(1)? == 1 {
                r.f(buffer_delay_len)?;
                r.f(buffer_delay_len)?;
                r.f(1)?;
            }
            if initial_display_delay == 1 && r.f(1)? == 1 {
                r.f(4)?;
            }
            if op == 0 {
                (level0, tier0) = (lvl, t);
            }
        }
        (level0, tier0)
    };
    let wb = r.f(4)? as usize + 1;
    let hb = r.f(4)? as usize + 1;
    r.f(wb)?;
    r.f(hb)?;
    if reduced == 0 && r.f(1)? == 1 {
        let delta = r.f(4)? as usize + 2;
        let _ = delta;
        r.f(3)?;
    }
    r.f(3)?;
    if reduced == 0 {
        r.f(4)?;
        let order_hint = r.f(1)?;
        if order_hint == 1 {
            r.f(2)?;
        }
        let force_sct = if r.f(1)? == 1 { 2 } else { r.f(1)? };
        if force_sct > 0 && r.f(1)? == 0 {
            r.f(1)?;
        }
        if order_hint == 1 {
            r.f(3)?;
        }
    }
    r.f(3)?;
    let high = r.f(1)?;
    let twelve = if profile == 2 && high == 1 {
        r.f(1)?
    } else {
        0
    };
    let mono = if profile == 1 { 0 } else { r.f(1)? };
    let (mut cp, mut tc, mut mc) = (2, 2, 2);
    if r.f(1)? == 1 {
        cp = r.f(8)?;
        tc = r.f(8)?;
        mc = r.f(8)?;
    }
    let (sx, sy, mut csp) = if mono == 1 {
        (1, 1, 0)
    } else if (cp, tc, mc) == (1, 13, 0) {
        (0, 0, 0)
    } else {
        r.f(1)?;
        match profile {
            0 => (1, 1, 0),
            1 => (0, 0, 0),
            _ if twelve == 1 => {
                let sx = r.f(1)?;
                let sy = if sx == 1 { r.f(1)? } else { 0 };
                (sx, sy, 0)
            }
            _ => (1, 0, 0),
        }
    };
    if mono == 0 && sx == 1 && sy == 1 {
        csp = r.f(2)?;
    }
    let mut record = Vec::with_capacity(4 + obu.len());
    record.push(0x81);
    record.push((profile << 5 | level) as u8);
    record.push((tier << 7 | high << 6 | twelve << 5 | mono << 4 | sx << 3 | sy << 2 | csp) as u8);
    record.push(0);
    record.extend_from_slice(obu);
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// FFmpeg (libaom) writes these `av1C` records; their `configOBUs`
    /// behind a temporal delimiter must give back the same bytes. 8-bit
    /// 4:2:0 (profile 0) and 10-bit 4:4:4 (profile 1), both FFmpeg 8.0.1.
    #[test]
    fn ffmpeg_records_are_rebuilt_exactly() {
        for record in [
            "81000c000a0a000000032dffcdaf9004",
            "812040000a0a20000002a5ff36be5080",
        ] {
            let record = hex(record);
            let mut tu = vec![0x12, 0x00];
            tu.extend_from_slice(&record[4..]);
            assert_eq!(av1c_from_temporal_unit(&tu), Some(record.clone()));
        }
        assert_eq!(
            av1c_from_temporal_unit(&[0x12, 0x00]),
            None,
            "no sequence header"
        );
        assert_eq!(av1c_from_temporal_unit(&[0x0A, 0x05, 0]), None, "truncated");
    }

    extern crate std;
    use std::vec;
}

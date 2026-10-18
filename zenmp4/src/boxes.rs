//! Reading ISOBMFF boxes out of byte slices.

use crate::Mp4Error;

/// A big-endian reader over one box body.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.b.len() - self.pos
    }

    pub(crate) fn rest(&self) -> &'a [u8] {
        &self.b[self.pos..]
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], Mp4Error> {
        let end = self.pos.checked_add(n).ok_or(Mp4Error::Truncated)?;
        let s = self.b.get(self.pos..end).ok_or(Mp4Error::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<(), Mp4Error> {
        self.take(n).map(|_| ())
    }

    pub(crate) fn u8(&mut self) -> Result<u8, Mp4Error> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, Mp4Error> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(crate) fn u24(&mut self) -> Result<u32, Mp4Error> {
        let b = self.take(3)?;
        Ok(u32::from_be_bytes([0, b[0], b[1], b[2]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, Mp4Error> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, Mp4Error> {
        self.u32().map(|v| v as i32)
    }

    pub(crate) fn u64(&mut self) -> Result<u64, Mp4Error> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_be_bytes(a))
    }

    pub(crate) fn fourcc(&mut self) -> Result<[u8; 4], Mp4Error> {
        let b = self.take(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }

    /// A full box's version and flags.
    pub(crate) fn full(&mut self) -> Result<(u8, u32), Mp4Error> {
        Ok((self.u8()?, self.u24()?))
    }
}

/// A box header: type, header length, and total size (`None`: to the end
/// of the enclosing space).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) kind: [u8; 4],
    pub(crate) header_len: usize,
    pub(crate) size: Option<u64>,
}

/// Parse a box header from the start of `b`. `Ok(None)` means more bytes
/// are needed (at most 16).
pub(crate) fn header(b: &[u8]) -> Result<Option<Header>, Mp4Error> {
    if b.len() < 8 {
        return Ok(None);
    }
    let size32 = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let kind = [b[4], b[5], b[6], b[7]];
    let (header_len, size) = match size32 {
        0 => (8, None),
        1 => {
            if b.len() < 16 {
                return Ok(None);
            }
            let mut a = [0u8; 8];
            a.copy_from_slice(&b[8..16]);
            (16, Some(u64::from_be_bytes(a)))
        }
        n => (8, Some(u64::from(n))),
    };
    // A `uuid` box carries a 16-byte extended type after the header.
    let header_len = if &kind == b"uuid" {
        header_len + 16
    } else {
        header_len
    };
    if let Some(s) = size
        && s < header_len as u64
    {
        return Err(Mp4Error::BadBoxSize);
    }
    // UUID boxes include a 16-byte user type even when their size is zero
    // (extends to the parent's end). Do not hand an incomplete header to
    // children(), which slices the body at header_len.
    if b.len() < header_len {
        return Ok(None);
    }
    Ok(Some(Header {
        kind,
        header_len,
        size,
    }))
}

/// The child boxes of a container body, as `(type, body)`.
pub(crate) fn children(body: &[u8]) -> Children<'_> {
    Children { b: body }
}

pub(crate) struct Children<'a> {
    b: &'a [u8],
}

impl<'a> Iterator for Children<'a> {
    type Item = Result<([u8; 4], &'a [u8]), Mp4Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.b.is_empty() {
            return None;
        }
        let h = match header(self.b) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.b = &[];
                return Some(Err(Mp4Error::Truncated));
            }
            Err(e) => {
                self.b = &[];
                return Some(Err(e));
            }
        };
        let total = match h.size {
            None => self.b.len(),
            Some(s) => match usize::try_from(s) {
                Ok(s) if s <= self.b.len() => s,
                _ => {
                    self.b = &[];
                    return Some(Err(Mp4Error::Truncated));
                }
            },
        };
        let body = &self.b[h.header_len..total];
        self.b = &self.b[total..];
        Some(Ok((h.kind, body)))
    }
}

/// The first child of `kind`, if any.
pub(crate) fn find<'a>(body: &'a [u8], kind: &[u8; 4]) -> Result<Option<&'a [u8]>, Mp4Error> {
    for child in children(body) {
        let (k, b) = child?;
        if &k == kind {
            return Ok(Some(b));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers() {
        let h = header(&[0, 0, 0, 16, b'f', b't', b'y', b'p'])
            .unwrap()
            .unwrap();
        assert_eq!((h.kind, h.header_len, h.size), (*b"ftyp", 8, Some(16)));
        assert_eq!(header(&[0, 0, 0, 1, b'm', b'd', b'a', b't']).unwrap(), None);
        let large = [0, 0, 0, 1, b'm', b'd', b'a', b't', 0, 0, 0, 1, 0, 0, 0, 0];
        let h = header(&large).unwrap().unwrap();
        assert_eq!((h.header_len, h.size), (16, Some(1 << 32)));
        let open = header(&[0, 0, 0, 0, b'm', b'd', b'a', b't'])
            .unwrap()
            .unwrap();
        assert_eq!(open.size, None);
        assert_eq!(
            header(&[0, 0, 0, 7, b'f', b'r', b'e', b'e']),
            Err(Mp4Error::BadBoxSize)
        );
    }

    #[test]
    fn truncated_open_uuid_headers_are_refused() {
        let mut uuid = [0u8; 24];
        uuid[4..8].copy_from_slice(b"uuid");
        for cut in 8..24 {
            assert_eq!(header(&uuid[..cut]), Ok(None));
            assert_eq!(
                children(&uuid[..cut]).next(),
                Some(Err(Mp4Error::Truncated))
            );
        }
        assert_eq!(children(&uuid).next(), Some(Ok((*b"uuid", &[][..]))));
    }

    #[test]
    fn children_walk_and_refuse_overruns() {
        let body = [
            0, 0, 0, 9, b'a', b'a', b'a', b'a', 7, // one child
            0, 0, 0, 8, b'b', b'b', b'b', b'b', // empty child
        ];
        let kids: Vec<_> = children(&body).map(|c| c.unwrap()).collect();
        assert_eq!(kids, vec![(*b"aaaa", &[7u8][..]), (*b"bbbb", &[][..])]);
        let bad = [0, 0, 0, 20, b'a', b'a', b'a', b'a'];
        assert_eq!(children(&bad).next(), Some(Err(Mp4Error::Truncated)));
    }

    extern crate std;
    use std::vec;
    use std::vec::Vec;
}

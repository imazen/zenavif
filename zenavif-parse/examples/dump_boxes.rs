//! Dump all boxes in an AVIF file, with correct container nesting.
//!
//! Fixes imazen/zenavif#57: `meta` is a FullBox (4-byte version/flags
//! before its children), recursions are bounded by the container's end,
//! 64-bit sizes print untruncated, and a size-0 box extends to the
//! container's end rather than ending the walk.
use std::env;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

struct Header {
    size: u64,
    box_type: [u8; 4],
    /// Bytes of header consumed (8, or 16 for a 64-bit size).
    header_len: u64,
}

fn read_box_header<R: Read>(r: &mut R, remaining: u64) -> std::io::Result<Option<Header>> {
    if remaining < 8 {
        return Ok(None);
    }
    let mut buf = [0u8; 8];
    match r.read(&mut buf)? {
        n if n < 8 => return Ok(None),
        _ => {}
    }

    let size32 = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let box_type = [buf[4], buf[5], buf[6], buf[7]];

    let (size, header_len) = match size32 {
        0 => (u64::MAX, 8), // extends to the container's end
        1 => {
            if remaining < 16 {
                return Ok(None);
            }
            let mut size_buf = [0u8; 8];
            r.read_exact(&mut size_buf)?;
            (u64::from_be_bytes(size_buf), 16)
        }
        _ => (u64::from(size32), 8),
    };

    Ok(Some(Header { size, box_type, header_len }))
}

fn dump_boxes<R: Read + Seek>(r: &mut R, end: u64, depth: usize) -> std::io::Result<()> {
    loop {
        let pos = r.stream_position()?;
        if pos >= end {
            break;
        }
        let Some(h) = read_box_header(r, end - pos)? else {
            break;
        };
        let content_start = pos + h.header_len;
        let box_end = if h.size == u64::MAX {
            end
        } else {
            content_start + h.size.saturating_sub(h.header_len)
        };
        let box_end = box_end.min(end);

        let indent = "  ".repeat(depth);
        let type_str = String::from_utf8_lossy(&h.box_type);
        let shown_size = if h.size == u64::MAX { 0 } else { h.size };
        println!("{}[{}] {} size={}", indent, pos, type_str, shown_size);

        // `meta` is a FullBox: 4 bytes of version/flags precede children.
        let child_start = if &h.box_type == b"meta" {
            content_start + 4
        } else {
            content_start
        };

        let is_container = matches!(
            &h.box_type,
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"meta" | b"iprp"
                | b"ipco" | b"moof" | b"traf" | b"edts" | b"mdra" | b"grpl"
        );
        if is_container && child_start < box_end {
            r.seek(SeekFrom::Start(child_start))?;
            dump_boxes(r, box_end, depth + 1)?;
        }
        // Always land on the next sibling, whatever the recursion did.
        r.seek(SeekFrom::Start(box_end))?;
    }

    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <avif-file>", args[0]);
        std::process::exit(1);
    }

    let path = &args[1];
    let mut f = File::open(path).expect("Failed to open file");
    let file_len = f.metadata().expect("Failed to stat file").len();

    println!("Boxes in {}:", path);
    dump_boxes(&mut f, file_len, 0).expect("Failed to dump boxes");
}

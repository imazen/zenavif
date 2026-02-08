//! Dump all boxes in an AVIF file
use std::env;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

fn read_box_header<R: Read>(r: &mut R) -> std::io::Result<Option<(u32, [u8; 4], u64)>> {
    let mut buf = [0u8; 8];
    if r.read(&mut buf)? < 8 {
        return Ok(None);
    }
    
    let mut size = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as u64;
    let box_type = [buf[4], buf[5], buf[6], buf[7]];
    
    if size == 1 {
        // 64-bit size
        let mut size_buf = [0u8; 8];
        r.read_exact(&mut size_buf)?;
        size = u64::from_be_bytes(size_buf);
    }
    
    Ok(Some((size as u32, box_type, size)))
}

fn dump_boxes<R: Read + Seek>(r: &mut R, depth: usize) -> std::io::Result<()> {
    let start_pos = r.stream_position()?;
    
    loop {
        let pos = r.stream_position()?;
        match read_box_header(r)? {
            None => break,
            Some((size32, box_type, size64)) => {
                let indent = "  ".repeat(depth);
                let type_str = String::from_utf8_lossy(&box_type);
                println!("{}[{}] {} size={}", indent, pos, type_str, size32);
                
                let content_size = if size32 == 0 {
                    // size=0 means "to end of file"
                    return Ok(());
                } else if size32 == 1 {
                    size64.saturating_sub(16)
                } else {
                    size64.saturating_sub(8)
                };

                // Recursively dump container boxes
                let is_container = matches!(&box_type, b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"meta" | b"iprp" | b"ipco");
                if is_container && content_size > 0 && content_size < 1_000_000 {
                    dump_boxes(r, depth + 1)?;
                } else if content_size > 0 {
                    // Skip to next box
                    r.seek(SeekFrom::Current(content_size as i64))?;
                }
            }
        }
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
    
    println!("Boxes in {}:", path);
    dump_boxes(&mut f, 0).expect("Failed to dump boxes");
}

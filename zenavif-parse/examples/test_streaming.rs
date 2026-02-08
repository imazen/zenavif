//! Test streaming animation parser
use avif_parse::AvifParser;
use std::fs::File;

fn main() {
    env_logger::init();

    let path = "link-u-samples/star-8bpc.avifs";
    let mut f = File::open(path).expect("Failed to open file");

    println!("Testing streaming parser...\n");

    // Parse with streaming API
    let parser = AvifParser::from_reader(&mut f).expect("Failed to parse");

    println!("Primary item: {} bytes", parser.primary_item().unwrap().len());

    if let Some(info) = parser.animation_info() {
        println!("\n=== Animation (Streaming) ===");
        println!("Frame count: {}", info.frame_count);
        println!("Loop count: {}", info.loop_count);

        println!("\nExtracting frames on-demand:");
        for i in 0..info.frame_count.min(5) {
            let frame = parser.animation_frame(i).expect("Failed to get frame");
            println!("  Frame {}: {} bytes, {}ms", i, frame.data.len(), frame.duration_ms);
        }
        if info.frame_count > 5 {
            println!("  ... ({} more frames)", info.frame_count - 5);
        }
    } else {
        println!("Not an animated AVIF");
    }

    println!("\n✓ Streaming parser test passed!");
}

//! Test streaming animation parser
use avif_parse::AvifParser;

fn main() {
    env_logger::init();

    let path = "link-u-samples/star-8bpc.avifs";
    let bytes = std::fs::read(path).expect("Failed to read file");

    println!("Testing zero-copy parser...\n");

    let parser = AvifParser::from_bytes(&bytes).expect("Failed to parse");

    println!("Primary item: {} bytes", parser.primary_data().unwrap().len());

    if let Some(info) = parser.animation_info() {
        println!("\n=== Animation ===");
        println!("Frame count: {}", info.frame_count);
        println!("Loop count: {}", info.loop_count);

        println!("\nExtracting frames on-demand:");
        for i in 0..info.frame_count.min(5) {
            let frame = parser.frame(i).expect("Failed to get frame");
            let borrowed = matches!(frame.data, std::borrow::Cow::Borrowed(_));
            println!(
                "  Frame {}: {} bytes ({}), {}ms",
                i,
                frame.data.len(),
                if borrowed { "zero-copy" } else { "copied" },
                frame.duration_ms,
            );
        }
        if info.frame_count > 5 {
            println!("  ... ({} more frames)", info.frame_count - 5);
        }
    } else {
        println!("Not an animated AVIF");
    }

    // Demonstrate grid support if applicable
    if let Some(grid) = parser.grid_config() {
        println!("\n=== Grid Image ===");
        println!("Grid layout: {}x{} tiles", grid.rows, grid.columns);
        println!("Output dimensions: {}x{}", grid.output_width, grid.output_height);
        println!("Total tiles: {}", parser.grid_tile_count());

        if parser.grid_tile_count() > 0 {
            let tile = parser.tile_data(0).expect("Failed to get tile");
            println!("  Tile 0: {} bytes", tile.len());
        }
    }

    println!("\nDone!");
}

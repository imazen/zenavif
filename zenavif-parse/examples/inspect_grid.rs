//! Inspect grid AVIF structure
#![allow(deprecated)]
use avif_parse::read_avif;
use std::env;
use std::fs::File;

fn main() {
    env_logger::init();
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <avif-file>", args[0]);
        std::process::exit(1);
    }

    let path = &args[1];
    let mut f = File::open(path).expect("Failed to open file");
    
    match read_avif(&mut f) {
        Ok(avif) => {
            println!("File: {}", path);
            println!("Primary item size: {} bytes", avif.primary_item.len());
            println!("Grid config: {:?}", avif.grid_config);
            println!("Number of tiles: {}", avif.grid_tiles.len());
            
            if avif.grid_config.is_some() {
                for (i, tile) in avif.grid_tiles.iter().enumerate() {
                    println!("  Tile {}: {} bytes", i, tile.len());
                }
            }
            
            if avif.alpha_item.is_some() {
                println!("Has alpha: yes");
            }
        }
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    }
}
